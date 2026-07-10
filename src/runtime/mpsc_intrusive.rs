//! `Queue<T>` — an unbounded, intrusive, lock-free multi-producer /
//! single-consumer queue (Vyukov-style: single atomic swap per push, no
//! CAS-retry-loop, no ABA risk since nodes are never reused).
//!
//! Built as a standalone, reusable primitive (not interner-specific) —
//! this is the piece the original axOS design conversations wanted for the
//! "version store" concurrency model ("CAS on version head pointers,
//! multi-producer multi-consumer queue for publishing changes") but never
//! actually built; `non_blocking_memory::SpscQueue` is the only queue that
//! exists today, and it is explicitly single-producer-only by its own doc
//! comment.
//!
//! ## Why this needs a higher evidence bar than most modules here
//!
//! `hotmem.rs` has a real, confirmed use-after-free in its history from
//! underestimating exactly this class of difficulty (a manufactured
//! `unsafe impl Sync` over an `UnsafeCell`-wrapped struct with no actual
//! proof that no `&mut`/`&` alias crossed threads). This module is not to
//! be treated as "sound" on the strength of the reasoning in this doc
//! comment alone — see the crate's test suite for the loom exhaustive
//! interleaving test, the Miri pass, the adversarial yield-injection stress
//! test, and the ASan/TSan passes this module requires before any caller
//! relies on it.
//!
//! ## Algorithm
//!
//! `head: AtomicPtr<Node<T>>` — every producer thread contends on this via
//! a single `swap`, never a CAS-retry-loop. `tail: *mut Node<T>` —
//! consumer-only, never touched by a producer, so it needs no atomicity of
//! its own (protected by the single-consumer contract, not by a lock). A
//! permanent `stub` sentinel node avoids a null-head special case at
//! construction.
//!
//! `push`: build the node, `swap` it into `head` (getting back the
//! previous head, `prev`), then `Release`-store `node` into `prev.next`.
//! `prev` cannot have been freed by the consumer before this store: the
//! consumer only frees a node after observing its `.next` as non-null and
//! advancing `tail` past it, and `prev.next` is null (by construction)
//! until precisely this store — the atomic `swap` in `head` guarantees
//! exactly one producer thread ever holds a given `prev` pointer to write
//! into, so there is no race on who performs that store.
//!
//! `pop` (consumer-only, never called concurrently with itself): walks
//! `tail.next`. Three outcomes: `Value(v)` (dequeued and freed the old
//! `tail` node), `Empty` (queue genuinely has nothing new since last
//! drained to `head`), or `Inconsistent` (a producer has executed its
//! `head.swap` but not yet its `next.store` — a transient gap, not a
//! logical state; the caller must retry, not treat this as empty).
//!
//! ## The wake signal is NOT part of the lock-free data structure
//!
//! `push`/`pop` themselves take no lock. Going idle-to-sleep when the queue
//! is empty still needs a real blocking primitive — busy-polling was
//! already measured as a real regression elsewhere in this crate
//! (`channels.rs`'s `wake()` doc comment: a busy-spin `wait` contended
//! directly with the sender's own lock and was slower than the synchronous
//! path it was meant to beat). An earlier version of `pop_blocking` used a
//! `Mutex<()>` + `Condvar` with a 2ms `wait_timeout` as the safety net for
//! the check-then-block race — correct, but it woke an idle consumer ~500
//! times a second forever (measured: 488 voluntary context switches per
//! idle second in `tests/mpsc_wake_battle_test.rs`).
//!
//! The current wake is a **parked-consumer slot**: `waiter` is an
//! `AtomicPtr` to a `Box<Thread>` handle, null when no consumer is
//! committing to sleep. `thread::park`/`unpark`'s token semantics replace
//! the timeout: an `unpark` that races ahead of the `park` banks a token
//! that makes the `park` return immediately, so a wakeup can be made
//! *unlosable* by ordering alone — no lock, no timeout, no idle churn.
//!
//! The ordering argument is the classic store→load / store→load exchange:
//!
//! * consumer: (C1) publish handle into `waiter`, (C2) re-check the queue,
//!   (C3) `park` only if still empty;
//! * producer: (P1) publish the node, (P2) read `waiter` and unpark if
//!   non-null.
//!
//! The lost wakeup requires C2 to miss P1 *and* P2 to miss C1. Both sides
//! place a `SeqCst` fence between their store and their load (C1/fence/C2,
//! P1/fence/P2), and `waiter`'s operations are `SeqCst`, so the two fences
//! are totally ordered: whichever fence comes second makes the other side's
//! store visible to the load that follows it — one of the two misses is
//! impossible. This is the same strength-over-minimalism call as
//! `interner_shard.rs`'s Acquire-where-Relaxed-would-do: the fences cost
//! nothing measurable next to `push`'s existing `AcqRel` swap.
//!
//! Handle ownership is transferred by `swap`: whichever side swaps a
//! non-null pointer out of `waiter` owns the `Box<Thread>` and is the only
//! one that frees it — the atomic swap makes a double-free structurally
//! impossible. A stale `unpark` (consumer already took a different item and
//! re-parked) is harmless: it wakes the same consumer thread spuriously
//! once, and the retry loop re-checks the queue on every wake anyway.

use std::ptr;
use std::sync::atomic::{fence, AtomicPtr, Ordering};
use std::thread::Thread;
use std::time::Instant;

struct Node<T> {
    next: AtomicPtr<Node<T>>,
    /// `None` only for the permanent `stub` sentinel; every real node
    /// carries `Some(value)`, fully written before the node is ever
    /// published (made reachable) to another thread.
    value: Option<T>,
}

/// Result of a single non-blocking `pop` attempt. Three-way on purpose —
/// see the module doc comment for why `Inconsistent` must never be treated
/// as `Empty` by a caller that cares about correctness (only
/// `pop_blocking`'s retry loop is allowed to conflate them into "keep
/// trying").
#[derive(Debug)]
pub enum PopResult<T> {
    Value(T),
    Empty,
    Inconsistent,
}

pub struct Queue<T> {
    head: AtomicPtr<Node<T>>,
    /// Consumer-only. Not an atomic — protected by the single-consumer
    /// contract on `pop`/`pop_blocking`, the same way `ReaderHandle`'s
    /// per-thread state in `non_blocking_memory.rs` needs no atomicity of
    /// its own.
    tail: std::cell::UnsafeCell<*mut Node<T>>,
    /// Parked-consumer slot: null when no consumer is committing to sleep;
    /// otherwise a `Box<Thread>` raw pointer published by `pop_blocking`
    /// just before it parks. Ownership transfers to whichever side `swap`s
    /// the non-null pointer out (see module doc comment).
    waiter: AtomicPtr<Thread>,
}

// SAFETY: producers only ever touch `head` (atomic) and construct/publish
// nodes that become immutable once reachable; the consumer only touches
// `tail`/frees nodes under the single-consumer contract documented on
// `pop`. No `&mut` ever aliases a `&` across threads on the same node: a
// node's `value`/`next` fields are written exactly once by its producer
// before publish, then only ever read (never mutated) until the single
// consumer thread takes ownership (`Box::from_raw`) and frees it — at
// which point no other thread holds a live reference to it, by the
// argument in the module doc comment.
unsafe impl<T: Send> Send for Queue<T> {}
unsafe impl<T: Send> Sync for Queue<T> {}

impl<T> Queue<T> {
    pub fn new() -> Self {
        let stub = Box::into_raw(Box::new(Node { next: AtomicPtr::new(ptr::null_mut()), value: None }));
        Queue {
            head: AtomicPtr::new(stub),
            tail: std::cell::UnsafeCell::new(stub),
            waiter: AtomicPtr::new(ptr::null_mut()),
        }
    }

    /// Push `value`. Safe to call from any number of producer threads
    /// concurrently with each other and with `pop`/`pop_blocking`.
    pub fn push(&self, value: T) {
        let node = Box::into_raw(Box::new(Node { next: AtomicPtr::new(ptr::null_mut()), value: Some(value) }));
        let prev = self.head.swap(node, Ordering::AcqRel);
        // SAFETY: see module doc comment — `prev` cannot be freed by the
        // consumer before this store.
        unsafe {
            (*prev).next.store(node, Ordering::Release);
        }
        // Wake protocol step P1 is the publish above; this fence + the
        // waiter read below are P2 (see module doc comment). The load-first
        // shape keeps the common no-sleeper case to a read, avoiding every
        // producer RMW-ing the same cacheline just to confirm it's null.
        fence(Ordering::SeqCst);
        if !self.waiter.load(Ordering::SeqCst).is_null() {
            let w = self.waiter.swap(ptr::null_mut(), Ordering::SeqCst);
            if !w.is_null() {
                // SAFETY: the swap transferred ownership of the Box to us —
                // no other thread can have received this same pointer.
                unsafe { Box::from_raw(w) }.unpark();
            }
        }
    }

    /// Attempt one dequeue. SAFETY / CONTRACT: must be called only from a
    /// single consumer thread — never concurrently with another `pop` or
    /// `pop_blocking` call (multiple concurrent consumers are out of scope
    /// for this queue; it is MPSC, not MPMC).
    ///
    /// Invariant: `tail` always points to a node whose value has ALREADY
    /// been returned (or the permanent `stub`, which never holds a value)
    /// — it is a "last consumed" marker, not the next item itself. The
    /// actual next unreturned item, if any, is `tail.next`. A prior version
    /// of this method special-cased skipping the stub as a value-less
    /// "free move" before re-deriving `next` — that shifted every
    /// subsequent item back by one and specifically dropped the LAST item
    /// pushed in a burst (nothing ever advanced past it), which hung a
    /// stress test waiting for a phantom item. The unified form below
    /// requires no stub special-case: when `tail == stub`, `tail.next` (if
    /// any) is simply the first real item, handled by the same general
    /// path as every other item.
    pub fn pop(&self) -> PopResult<T> {
        unsafe {
            let tail = *self.tail.get();
            let next = (*tail).next.load(Ordering::Acquire);

            if !next.is_null() {
                *self.tail.get() = next;
                let value = (*next).value.take().expect("real (non-stub) node must carry Some(value)");
                drop(Box::from_raw(tail));
                return PopResult::Value(value);
            }

            if self.head.load(Ordering::Acquire) != tail {
                // A producer has swapped `head` but not yet published
                // `prev.next` — transient, not logically empty.
                return PopResult::Inconsistent;
            }
            PopResult::Empty
        }
    }

    /// Take back (and free) our published waiter handle if no producer
    /// claimed it. Called by the consumer after every wake / aborted sleep
    /// so a stale handle never lingers in the slot.
    fn reclaim_waiter(&self) {
        let w = self.waiter.swap(ptr::null_mut(), Ordering::SeqCst);
        if !w.is_null() {
            // SAFETY: swap transferred ownership back to us (see module doc
            // comment's ownership rule).
            unsafe { drop(Box::from_raw(w)) };
        }
    }

    /// Block the calling (single consumer) thread until a value is
    /// available. See module doc comment for the park/unpark wake protocol
    /// and why the idle-wait is neither a busy-spin nor a timeout poll.
    pub fn pop_blocking(&self) -> T {
        loop {
            match self.pop() {
                PopResult::Value(v) => return v,
                PopResult::Inconsistent => std::thread::yield_now(),
                PopResult::Empty => {
                    // C1: publish our handle. The slot must be null here —
                    // we are the only thread that stores non-null, and we
                    // always reclaim before leaving this arm.
                    let me = Box::into_raw(Box::new(std::thread::current()));
                    let prev = self.waiter.swap(me, Ordering::SeqCst);
                    debug_assert!(prev.is_null(), "single-consumer contract violated");
                    // C2: re-check AFTER publishing (see module doc comment
                    // — this ordering is the whole anti-lost-wakeup story).
                    fence(Ordering::SeqCst);
                    match self.pop() {
                        PopResult::Value(v) => {
                            self.reclaim_waiter();
                            return v;
                        }
                        PopResult::Inconsistent => {
                            self.reclaim_waiter();
                            std::thread::yield_now();
                        }
                        PopResult::Empty => {
                            // C3: sleep. A producer that claimed our handle
                            // before this park banked an unpark token, so
                            // park() returns immediately — the wakeup is
                            // unlosable. Spurious returns just re-loop.
                            std::thread::park();
                            self.reclaim_waiter();
                        }
                    }
                }
            }
        }
    }

    /// Deadline-bounded `pop_blocking`: block until a value is available OR
    /// `deadline` passes, whichever first. Returns `None` on timeout. Same
    /// single-consumer contract and same C1/C2/C3 wake protocol as
    /// `pop_blocking` — `park_timeout` replaces `park`, and the deadline is
    /// re-evaluated on every loop pass (spurious wakeups included). A value
    /// already in the queue is returned even if the deadline has passed:
    /// drain-before-timeout, so callers never see a timeout while data sits
    /// unread.
    pub fn pop_blocking_until(&self, deadline: Instant) -> Option<T> {
        loop {
            match self.pop() {
                PopResult::Value(v) => return Some(v),
                PopResult::Inconsistent => std::thread::yield_now(),
                PopResult::Empty => {
                    let now = Instant::now();
                    if now >= deadline {
                        return None;
                    }
                    let me = Box::into_raw(Box::new(std::thread::current()));
                    let prev = self.waiter.swap(me, Ordering::SeqCst);
                    debug_assert!(prev.is_null(), "single-consumer contract violated");
                    if !prev.is_null() {
                        // SAFETY: swap gave us ownership of the stale box.
                        unsafe { drop(Box::from_raw(prev)) };
                    }
                    fence(Ordering::SeqCst);
                    match self.pop() {
                        PopResult::Value(v) => {
                            self.reclaim_waiter();
                            return Some(v);
                        }
                        PopResult::Inconsistent => {
                            self.reclaim_waiter();
                            std::thread::yield_now();
                        }
                        PopResult::Empty => {
                            std::thread::park_timeout(deadline - now);
                            self.reclaim_waiter();
                        }
                    }
                }
            }
        }
    }
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        // SAFETY: by Drop time there is no concurrent producer/consumer
        // access (same assumption `BridgedCell`'s Drop impl documents) —
        // walk from `tail` to the end of the chain, freeing every node,
        // and free a stale waiter handle if one was left published.
        unsafe {
            let w = self.waiter.load(Ordering::Relaxed);
            if !w.is_null() {
                drop(Box::from_raw(w));
            }
            let mut cur = *self.tail.get();
            while !cur.is_null() {
                let next = (*cur).next.load(Ordering::Relaxed);
                drop(Box::from_raw(cur));
                cur = next;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn single_thread_fifo_order() {
        let q: Queue<i32> = Queue::new();
        q.push(1);
        q.push(2);
        q.push(3);
        assert!(matches!(q.pop(), PopResult::Value(1)));
        assert!(matches!(q.pop(), PopResult::Value(2)));
        assert!(matches!(q.pop(), PopResult::Value(3)));
        assert!(matches!(q.pop(), PopResult::Empty));
    }

    #[test]
    fn pop_blocking_single_thread_roundtrip() {
        let q: Queue<i32> = Queue::new();
        q.push(42);
        assert_eq!(q.pop_blocking(), 42);
    }

    #[test]
    fn concurrent_producers_single_consumer_no_loss_no_duplication() {
        const PRODUCERS: usize = 16;
        const PER_PRODUCER: usize = 2_000;

        let q = Arc::new(Queue::<(usize, usize)>::new());
        let handles: Vec<_> = (0..PRODUCERS)
            .map(|pid| {
                let q = Arc::clone(&q);
                thread::spawn(move || {
                    for i in 0..PER_PRODUCER {
                        q.push((pid, i));
                    }
                })
            })
            .collect();

        let mut seen: HashSet<(usize, usize)> = HashSet::new();
        let mut got = 0usize;
        while got < PRODUCERS * PER_PRODUCER {
            let v = q.pop_blocking();
            assert!(seen.insert(v), "duplicate item {:?}", v);
            got += 1;
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(seen.len(), PRODUCERS * PER_PRODUCER);
        for pid in 0..PRODUCERS {
            for i in 0..PER_PRODUCER {
                assert!(seen.contains(&(pid, i)), "missing ({}, {})", pid, i);
            }
        }
    }

    /// Per-producer FIFO ordering must hold even though cross-producer
    /// interleaving is unspecified: producer P's items must be dequeued in
    /// the order P pushed them.
    #[test]
    fn per_producer_order_preserved_under_contention() {
        const PRODUCERS: usize = 8;
        const PER_PRODUCER: usize = 1_000;

        let q = Arc::new(Queue::<(usize, usize)>::new());
        let handles: Vec<_> = (0..PRODUCERS)
            .map(|pid| {
                let q = Arc::clone(&q);
                thread::spawn(move || {
                    for i in 0..PER_PRODUCER {
                        q.push((pid, i));
                    }
                })
            })
            .collect();

        let mut last_seen = vec![None; PRODUCERS];
        let mut got = 0usize;
        while got < PRODUCERS * PER_PRODUCER {
            let (pid, i) = q.pop_blocking();
            if let Some(last) = last_seen[pid] {
                assert!(i == last + 1, "producer {} out of order: {} after {}", pid, i, last);
            } else {
                assert_eq!(i, 0, "producer {}'s first item wasn't index 0", pid);
            }
            last_seen[pid] = Some(i);
            got += 1;
        }
        for h in handles {
            h.join().unwrap();
        }
    }

    /// Adversarial: inject a yield right at the swap/store gap `pop`'s
    /// `Inconsistent` case exists to handle, from many producer threads,
    /// hammering the consumer's ability to correctly retry rather than
    /// misreport empty. Styled on hotmem.rs's `uaf_isolation_probe`.
    #[test]
    fn adversarial_yield_at_swap_store_gap_never_loses_or_corrupts() {
        const PRODUCERS: usize = 12;
        const PER_PRODUCER: usize = 500;

        let q = Arc::new(Queue::<usize>::new());
        let handles: Vec<_> = (0..PRODUCERS)
            .map(|pid| {
                let q = Arc::clone(&q);
                thread::spawn(move || {
                    for i in 0..PER_PRODUCER {
                        // Force scheduler churn right around the swap/store
                        // window on every push, maximizing the chance the
                        // consumer observes the transient Inconsistent gap.
                        thread::yield_now();
                        q.push(pid * PER_PRODUCER + i);
                        thread::yield_now();
                    }
                })
            })
            .collect();

        let mut seen: HashSet<usize> = HashSet::new();
        while seen.len() < PRODUCERS * PER_PRODUCER {
            match q.pop() {
                PopResult::Value(v) => {
                    assert!(seen.insert(v), "duplicate item {}", v);
                }
                PopResult::Inconsistent | PopResult::Empty => thread::yield_now(),
            }
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(seen.len(), PRODUCERS * PER_PRODUCER);
    }
}

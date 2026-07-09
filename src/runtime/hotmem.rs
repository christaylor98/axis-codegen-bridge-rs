//! AXVERITY_HOTMEM_CONSUMER_IMPLEMENTATION_V1 — hot mem arena, first slice.
//!
//! Exposes `non_blocking_memory::BridgedCell`/`ReaderRegistry` (already
//! built, already tested — 66 tests, ASan-clean, see that module's own doc
//! comment) to M1 as a single process-wide, epoch-gated, best-effort cache
//! fed asynchronously from the WAL write path. This module owns NO new
//! reclamation logic — it only wires the existing engine to the bridge's
//! `fn(Value) -> Value` dispatch ABI and picks the concurrency discipline
//! that makes doing so sound.
//!
//! ## OPEN, UNRESOLVED SOUNDNESS ISSUE — do not treat this module as sound
//!
//! This module wraps the arena in `UnsafeCell` with a manually-asserted
//! `unsafe impl Sync`, on the theory that a single writer thread taking
//! conceptual `&mut BridgedCell` never overlaps with reader threads taking
//! concurrent `&BridgedCell`, so the atomic head/floor protocol alone is
//! enough to make it sound. **That theory does not hold.** Rust's aliasing
//! rules require *some* synchronization between a live `&mut T` on one
//! thread and a live `&T` on another thread into the *same* allocation —
//! the atomic protocol only proves the data a reader can reach stays
//! consistent, it does not license bypassing that requirement. Tellingly,
//! EVERY existing adversarial concurrency test in `non_blocking_memory.rs`
//! for exactly this write_lazy/read_ref interaction
//! (`write_lazy_stress_concurrent_safe`,
//! `adversarial_a_readref_held_concurrent_writes_and_reclaim`, and the
//! double-buffer equivalent) wraps the `BridgedCell` in `Mutex`, including
//! on the *reader* side — this module's raw-`UnsafeCell` bypass is the only
//! caller anywhere in the codebase NOT covered by that "66 tests, ASan-clean"
//! claim.
//!
//! Confirmed empirically, in two stages:
//!   1. `hotmem_read` originally called `BridgedCell::read()` (owned,
//!      pins-nothing). Since it never pinned the reader floor, `write_lazy`'s
//!      immediate-free optimization always believed no reader existed and
//!      freed the superseded Block right away — a plain use-after-free.
//!      Reproduced as glibc heap corruption
//!      (`free(): unaligned chunk detected in tcache 2`) within ~3s under
//!      concurrent pg_server INSERT load. FIXED: `hotmem_read` now uses
//!      `read_ref()`, which pins the floor before dereferencing and releases
//!      it only after the byte copy is made (see that function's own doc
//!      comment below).
//!   2. Even with (1) fixed, an isolated stress test hammering only
//!      `hotmem_write`/`hotmem_read` in a tight loop — no SQL parsing, no
//!      wire protocol, no reclog — reproduced invalid-UTF8 corrupted reads
//!      within seconds, escalating to a hard SIGSEGV (see
//!      `uaf_isolation_probe` below; `cargo test --release --lib
//!      uaf_isolation_probe -- --nocapture --ignored`). This is the
//!      `UnsafeCell`-aliasing violation described above, NOT another
//!      instance of bug (1) — grepped the whole crate, confirmed there is no
//!      second `BridgedCell::read()` call site.
//!
//! **A `Mutex`-wrapped arena was drafted as the mechanical fix (matching the
//! crate's own validated pattern) but explicitly REJECTED (Chris,
//! 2026-07-09): "no mutex, ever — it's a crutch with massive overhead, and
//! avoiding exactly that is the point of this mem manager." A lock-free
//! redesign is planned instead; this module is reverted to its pre-Mutex,
//! read_ref-fixed-but-still-unsound state as the known-bad baseline for that
//! redesign to replace.** Do not add new readers/writers against this
//! module's current `UnsafeCell` scheme, and do not reintroduce a `Mutex`
//! here without checking back — both are considered closed directions.
//!
//! **Exactly one thread ever calls `hotmem_write`: the dedicated
//! "hotmem-frame" channel janitor thread.** Every producer (pg_server
//! workers, CLI one-shots) only ever calls `channel_send` — fire-and-forget,
//! never touches the arena directly. That invariant holds today but, per the
//! above, is not sufficient on its own for soundness.
//!
//! The arena is explicitly NOT durable and NOT critical — the WAL is still
//! the durability boundary (see `wal_put.m1`'s `channel_send` hook); losing
//! an arena update, or the whole arena on process death, is by design a
//! no-op for correctness. That is what makes the best-effort `channel_send`
//! hand-off (rather than a blocking/acked path) the right choice here.

use std::cell::{Cell as StdCell, RefCell, UnsafeCell};
use std::sync::{Arc, OnceLock};

use super::non_blocking_memory::{BridgedCell, ReaderHandle, ReaderRegistry};
use super::value::Value;

/// The arena payload: a shared reference to the exact byte buffer the WAL
/// write already built. Moving an `Arc` into `write_lazy` is a refcount
/// bump, never a copy of the underlying bytes — the arena never owns a
/// second copy of anything a writer produced.
type Payload = Arc<Vec<u8>>;

/// `UnsafeCell` wrapper — KNOWN UNSOUND, see module doc comment. Kept as the
/// baseline for the planned lock-free redesign (a `Mutex` was drafted and
/// rejected — no lock, ever).
struct Arena {
    cell: UnsafeCell<BridgedCell<Payload>>,
}

// SAFETY: NOT actually established — see module doc comment's "OPEN,
// UNRESOLVED SOUNDNESS ISSUE" section. This assertion is the thing that
// needs to become true under the redesign, not a proof that it already is.
unsafe impl Sync for Arena {}

static ARENA: OnceLock<Arena> = OnceLock::new();
static READERS: OnceLock<ReaderRegistry> = OnceLock::new();

fn arena() -> &'static Arena {
    ARENA.get_or_init(|| Arena {
        cell: UnsafeCell::new(BridgedCell::new()),
    })
}

fn readers() -> &'static ReaderRegistry {
    READERS.get_or_init(ReaderRegistry::new)
}

thread_local! {
    /// This thread's reader slot, lazily acquired. `ReaderHandle`'s Drop
    /// releases the floor the instant the thread exits (normal or panic
    /// unwind) — see `ReaderRegistry::acquire`'s own doc comment.
    static READER: RefCell<Option<ReaderHandle<'static>>> = const { RefCell::new(None) };

    /// (epoch, missed) from this thread's most recent `hotmem_read` — M1
    /// has no tuples, so `hotmem_epoch`/`hotmem_missed` split this out.
    static LAST: StdCell<(i64, i64)> = const { StdCell::new((0, 0)) };
}

/// `hotmem_write(arg: Value) -> Value`   —   List(Bytes) -> Unit
///
/// The `wait()` handler for the "hotmem-frame" channel. `wait()` drains
/// every pending message into one batch per call (see `channels.rs`) and
/// hands the whole batch to ONE handler invocation — mirrors
/// `wal_fast_batch_write` in `logbuf.rs` exactly, same reason: the batch
/// already arrived as owned, final values, so writing each straight into
/// the arena needs no intermediate accumulator.
///
/// SAFETY: must only ever be reachable as the wait-handler on the single
/// dedicated hotmem-janitor thread. See module doc comment.
#[track_caller]
pub fn hotmem_write(arg: Value) -> Value {
    let items = match arg {
        Value::List(items) => items,
        other => panic!("hotmem_write: expected List, got {:?}", other),
    };
    if items.is_empty() {
        return Value::Unit;
    }
    let a = arena();
    let reg = readers();
    // SAFETY: sole writer thread invariant — see module doc comment.
    let cell = unsafe { &mut *a.cell.get() };
    for item in items {
        let bytes = match item {
            Value::Bytes(b) => b,
            other => panic!("hotmem_write: expected Bytes item, got {:?}", other),
        };
        // SAFETY: single-writer-per-cell, upheld by the same invariant.
        unsafe {
            cell.write_lazy(Arc::new(bytes), reg);
        }
    }
    // Opportunistic, watermark-gated — bounds retired-list growth without
    // scanning reader floors on every single write. See
    // `BridgedCell::reclaim_if_watermark`'s own doc comment.
    cell.reclaim_if_watermark(reg);
    Value::Unit
}

/// `hotmem_reader_start(arg: Value) -> Value`   —   Unit -> Unit
///
/// Lazily acquires this thread's `ReaderHandle`, idempotent. Call once per
/// consumer thread before the first `hotmem_read`.
pub fn hotmem_reader_start(_arg: Value) -> Value {
    READER.with(|r| {
        if r.borrow().is_none() {
            *r.borrow_mut() = Some(readers().acquire());
        }
    });
    Value::Unit
}

/// `hotmem_read(arg: Value) -> Value`   —   Int(last_epoch) -> Bytes
///
/// Pinned read (`BridgedCell::read_ref`, NOT `BridgedCell::read`). Still
/// materializes an owned `Bytes` for the M1/Rust FFI boundary (M1's `Bytes`
/// ABI is an owned buffer), but does so through the floor-pinning path so
/// the writer's `write_lazy` immediate-free optimization cannot free the
/// Block out from under a read in progress.
///
/// FIXED (use-after-free): this previously called the "owned, pins-nothing"
/// `BridgedCell::read()` fast path. That path's own doc comment
/// (non_blocking_memory.rs, `BridgedCell::read`'s "F2 HAZARD" note) warns it
/// is sound ONLY when the single-writer contract structurally prevents
/// concurrent reclamation — NOT the case here, since this module
/// deliberately shares the arena across threads via `UnsafeCell` so one
/// janitor thread writes while other threads read concurrently. Because
/// `hotmem_read` never pinned a floor, `write_lazy` (in `hotmem_write`,
/// above) always saw `floor_min() == u64::MAX` and freed the superseded
/// Block immediately — racing a concurrent `read()` that had already loaded
/// the (now-dangling) pointer but not yet finished cloning out of it.
/// Reproduced empirically as glibc-detected heap corruption
/// (`free(): unaligned chunk detected in tcache 2`) within seconds under
/// concurrent INSERT load. `read_ref` pins the reader's floor BEFORE the
/// pointer is dereferenced, so `write_lazy` retires (rather than frees) any
/// Block a live read might still reach; the pin is released only after this
/// function has already copied the bytes out.
///
/// Auto-acquires a reader handle if `hotmem_reader_start` was not called
/// first, so callers who only ever read from one thread can skip the
/// explicit start call. Returns empty `Bytes` if the cell has never been
/// written.
pub fn hotmem_read(arg: Value) -> Value {
    let last_epoch = match arg {
        Value::Int(n) => n.max(0) as u64,
        other => panic!("hotmem_read: expected Int, got {:?}", other),
    };
    let a = arena();
    // SAFETY: `&self` read, concurrent with the sole writer thread's
    // `&mut self` window — sound under the atomic head/floor protocol
    // documented on `BridgedCell`/`Cell`. See module doc comment.
    let cell = unsafe { &*a.cell.get() };
    READER.with(|r| {
        if r.borrow().is_none() {
            *r.borrow_mut() = Some(readers().acquire());
        }
        let borrowed = r.borrow();
        let handle = borrowed
            .as_ref()
            .expect("hotmem_read: reader handle just acquired above");
        match cell.read_ref(handle, last_epoch) {
            None => {
                LAST.with(|l| l.set((0, 0)));
                Value::Bytes(Vec::new())
            }
            Some(read_ref) => {
                let epoch = read_ref.epoch;
                let missed = read_ref.missed;
                // Copy out while the floor is still pinned; `read_ref`'s
                // Drop (end of this arm) releases the pin only after this
                // clone is done — this ordering is what closes the race.
                let bytes = (**read_ref).clone();
                LAST.with(|l| l.set((epoch as i64, missed as i64)));
                Value::Bytes(bytes)
            }
        }
    })
}

/// `hotmem_epoch(arg: Value) -> Value`   —   Unit -> Int
///
/// Epoch from this thread's most recent `hotmem_read`.
pub fn hotmem_epoch(_arg: Value) -> Value {
    Value::Int(LAST.with(|l| l.get().0))
}

/// `hotmem_missed(arg: Value) -> Value`   —   Unit -> Int
///
/// Writes missed since the previous `hotmem_read`'s `last_epoch`, as of
/// this thread's most recent `hotmem_read` call. 0 == fully current.
pub fn hotmem_missed(_arg: Value) -> Value {
    Value::Int(LAST.with(|l| l.get().1))
}

// KNOWN-FAILING regression probe for the open UnsafeCell-aliasing soundness
// issue documented at the top of this module. Isolates hotmem_write/
// hotmem_read from SQL parsing, the wire protocol, and reclog entirely —
// pure concurrent write/read against this module's own arena. `#[ignore]`d
// so a normal `cargo test` run stays green; run explicitly
// (`cargo test --release --lib uaf_isolation_probe -- --ignored --nocapture`)
// to reproduce the corruption against the current UnsafeCell scheme, or to
// confirm a future lock-free redesign actually closes it (this test should
// go from failing to passing with ZERO changes to the test itself once the
// redesign lands — that's the acceptance check).
#[cfg(test)]
mod uaf_isolation_probe {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtoOrd};
    use std::sync::Arc as StdArc;
    use std::thread;
    use std::time::Duration;

    #[test]
    #[ignore = "known-failing: open UnsafeCell-aliasing soundness issue, see module doc comment"]
    fn concurrent_write_read_never_corrupts() {
        let stop = StdArc::new(AtomicBool::new(false));
        let bad = StdArc::new(AtomicUsize::new(0));
        let writes = StdArc::new(AtomicUsize::new(0));
        let reads = StdArc::new(AtomicUsize::new(0));
        let nonempty_reads = StdArc::new(AtomicUsize::new(0));

        const TAG_LEN: usize = "PAYLOAD-00000000000000000000-END".len();

        let w_stop = stop.clone();
        let w_writes = writes.clone();
        let writer = thread::spawn(move || {
            let mut i: u64 = 0;
            while !w_stop.load(AtoOrd::Relaxed) {
                let payload = format!("PAYLOAD-{:020}-END", i).into_bytes();
                hotmem_write(Value::List(vec![Value::Bytes(payload)]));
                w_writes.fetch_add(1, AtoOrd::Relaxed);
                i += 1;
            }
        });

        let r_stop = stop.clone();
        let r_bad = bad.clone();
        let r_reads = reads.clone();
        let r_nonempty = nonempty_reads.clone();
        let reader = thread::spawn(move || {
            let mut last_epoch: i64 = 0;
            while !r_stop.load(AtoOrd::Relaxed) {
                let v = hotmem_read(Value::Int(last_epoch));
                r_reads.fetch_add(1, AtoOrd::Relaxed);
                if let Value::Bytes(b) = v {
                    if !b.is_empty() {
                        r_nonempty.fetch_add(1, AtoOrd::Relaxed);
                        match String::from_utf8(b) {
                            Ok(s) => {
                                let ok = s.len() == TAG_LEN
                                    && s.starts_with("PAYLOAD-")
                                    && s.ends_with("-END");
                                if !ok {
                                    eprintln!("MALFORMED (valid utf8 but wrong shape): {:?}", s);
                                    r_bad.fetch_add(1, AtoOrd::Relaxed);
                                }
                            }
                            Err(e) => {
                                eprintln!("INVALID UTF-8: {}", e);
                                r_bad.fetch_add(1, AtoOrd::Relaxed);
                            }
                        }
                    }
                } else {
                    eprintln!("hotmem_read returned non-Bytes: {:?}", v);
                    r_bad.fetch_add(1, AtoOrd::Relaxed);
                }
                last_epoch = LAST.with(|l| l.get().0);
            }
        });

        thread::sleep(Duration::from_secs(4));
        stop.store(true, AtoOrd::Relaxed);
        writer.join().unwrap();
        reader.join().unwrap();

        let w = writes.load(AtoOrd::Relaxed);
        let r = reads.load(AtoOrd::Relaxed);
        let ne = nonempty_reads.load(AtoOrd::Relaxed);
        let b = bad.load(AtoOrd::Relaxed);
        eprintln!("writes={w} reads={r} nonempty_reads={ne} bad={b}");
        assert_eq!(b, 0, "corrupted/malformed reads observed: {b}");
    }
}

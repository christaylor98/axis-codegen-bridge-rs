//! Static-topology channels + event subscription for H1/M1 concurrent process
//! loops (BRIDGE_ASYNC_PRIMITIVES_V1).
//!
//! Three foreign fns compose to express IPC between process loops that run on
//! separate execution contexts (threads, via the `--entries` driver):
//!
//!   * `channel_send(name, data)` — enqueue `data` on the named channel.
//!   * `event_subscribe(name)`    — this context will `wait` on the named channel.
//!   * `wait(handler)`            — block until a subscribed channel has data,
//!                                  then call `handler(List)` synchronously.
//!
//! ## Invariants (hard limits from the intent)
//!
//! * **CLOSURE_RULE_HARD** — `wait`'s handler is a bare `fn(Value) -> Value`
//!   pointer, moved into the call, invoked once inside `wait`'s own frame, and
//!   dropped when `wait` returns. Nothing (no struct field, thread-local, or
//!   global) stores it, so it cannot escape the frame or be re-invoked from a
//!   timer / interrupt / async context. The illegal state is unrepresentable:
//!   there is no storage path to abuse.
//!
//! * **WAIT_ALWAYS_LIST** — the handler's sole argument is always a
//!   `Value::List`, never a bare scalar, regardless of how many messages were
//!   drained (0 is not delivered — `wait` blocks until ≥1).
//!
//! * **CHANNELS_STATIC** — the set of legal channel names is fixed by the
//!   registry (`channel <name> … end`) and enforced at emit time: a
//!   `channel_send` to an undeclared name is a hard compile error (see
//!   `emit/rust_05.rs`), never a silent no-op. At runtime a channel's buffer is
//!   created on first touch by either endpoint, so send/subscribe ordering
//!   across contexts is race-free — the *names* are still compile-time literals,
//!   so this is not dynamic channel creation.

use super::value::{get_str, Value};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::mpsc_intrusive;

/// A single channel's buffer. Two backends selected by the process-global
/// `AXVERITY_CHANNEL_QUEUE` flag (read once):
///   * `mutex`    (default) — one `VecDeque` guarded by std's `Mutex`; senders
///                push the back, `wait` drains the front. Rust's stdlib lock.
///   * `lockfree` — OUR own `mpsc_intrusive::Queue` (the same lock-free MPSC
///                primitive that already backs `unified_wait`'s inbox): `push`
///                is a single atomic swap from any number of producers; `wait`
///                drains via the non-blocking `pop`. The receiver's OS-level
///                Condvar wait is unchanged (irreducible — a blocking wait can't
///                be made lock-free; same reason as oneshot's condvar).
///
/// AXVERITY_BRIDGE_LOCKFREE_EXPERIMENT_V1: getting our own IPC primitive onto the
/// live path (not to win a microbenchmark — the send is already non-blocking in
/// both backends — but so the native lock-free substrate becomes load-bearing
/// and hardened rather than a shelved museum piece we bypass in favor of std).
///
/// CONTRACT NARROWING (flagged, not silent — hard-limit #5): `mpsc_intrusive` is
/// MPSC, so the `lockfree` backend requires SINGLE-CONSUMER-per-channel (exactly
/// one context ever `wait`s on a given channel). The current static topology
/// (index-frame/hotmem-frame/wal-fast-frame each have one consumer) satisfies
/// this; the `mutex` backend keeps the original multi-consumer-tolerant behavior.
struct Channel {
    queue: Mutex<VecDeque<Value>>,           // `mutex` backend (default)
    lfq: mpsc_intrusive::Queue<Value>,       // `lockfree` backend (our primitive)
}

/// Backend selector, read once. Any value other than `lockfree` (incl. unset)
/// keeps the default `Mutex<VecDeque>` path.
fn channel_queue_lockfree() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| matches!(std::env::var("AXVERITY_CHANNEL_QUEUE").as_deref(), Ok("lockfree")))
}

/// Process-global channel registry, keyed by declared channel name.
fn registry() -> &'static Mutex<HashMap<String, Arc<Channel>>> {
    static REG: OnceLock<Mutex<HashMap<String, Arc<Channel>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Return the buffer for `name`, creating an empty one on first touch.
///
/// Creation is keyed on a compile-time-literal name — the emit-time gate rejects
/// sends to names absent from the static `channel` declarations — so this is not
/// dynamic channel creation; the topology is fixed at build time. Get-or-create
/// (rather than reject-if-absent) makes send/subscribe ordering across contexts
/// race-free, exactly as the old `signals.rs` `OnceLock` channels were.
fn channel_for(name: &str) -> Arc<Channel> {
    let mut reg = registry().lock().unwrap();
    reg.entry(name.to_string())
        .or_insert_with(|| Arc::new(Channel {
            queue: Mutex::new(VecDeque::new()),
            lfq: mpsc_intrusive::Queue::new(),
        }))
        .clone()
}

/// One process-global wake signal, shared by every channel. `wait` used to
/// busy-spin (`yield_now` in a tight loop) instead of blocking — found to be
/// a real regression, not a neutral simplification: the constant re-lock of
/// a channel's queue mutex from the spin loop directly contended with
/// `channel_send`'s own lock on the same mutex, measured slower than the
/// synchronous fsync path this async design was meant to beat
/// (AXVERITY_PGSERVER_FAST_MODE spike, `top -H` showed the waiting thread
/// pinned near 100% CPU). This single shared `Condvar` fixes that: `wait`
/// genuinely blocks (no CPU spend, no lock contention) until notified.
///
/// A single global pair (rather than one Condvar per channel) is deliberate:
/// `wait` blocks on however many channels a context subscribed to via
/// `event_subscribe`, and std's `Condvar` has no built-in "wait on any of N"
/// primitive. Every `channel_send`, on any channel, notifies this one
/// Condvar; a waiter re-checks all of ITS OWN subscribed channels' queues
/// after waking, so it only reacts to messages it actually asked for. The
/// short `wait_timeout` (not `wait` unbounded) is a safety net for the
/// unavoidable check-then-block race inherent to using a separate
/// mutex/condvar pair from the channel's own queue mutex (a message can
/// land and notify between a waiter's queue-check and its call into
/// `cvar.wait`) — bounding a missed wakeup to a few ms costs nothing
/// measurable against a millisecond-scale I/O batch, and is far simpler and
/// less error-prone than restructuring every channel to share one lock.
fn wake() -> &'static (Mutex<()>, Condvar) {
    static WAKE: OnceLock<(Mutex<()>, Condvar)> = OnceLock::new();
    WAKE.get_or_init(|| (Mutex::new(()), Condvar::new()))
}

thread_local! {
    /// Channels the current execution context waits on. Per-context: the bridge
    /// owns subscriptions; H1 holds nothing between calls.
    static SUBSCRIPTIONS: RefCell<Vec<String>> = RefCell::new(Vec::new());
}

fn name_of(v: &Value) -> String {
    match v {
        Value::Str(h) => get_str(h),
        other => panic!("channel name must be Text, got {:?}", other),
    }
}

/// `channel_send(name: Text, data: Value) -> Unit`.
///
/// Calling convention: unary `Value::Tuple([name, data])` (the data-fn convention
/// the emitter uses for 2-arg bridge fns).
#[track_caller]
pub fn channel_send(args: Value) -> Value {
    let (name, data) = match args {
        Value::Tuple(mut es) if es.len() == 2 => {
            let data = es.pop().unwrap();
            let name = es.pop().unwrap();
            (name_of(&name), data)
        }
        other => panic!("channel_send: expected Tuple(Text, Value), got {:?}", other),
    };
    let chan = channel_for(&name);
    if channel_queue_lockfree() {
        chan.lfq.push(data); // our lock-free MPSC primitive; single atomic swap
    } else {
        chan.queue.lock().unwrap().push_back(data);
    }
    // Wake any thread blocked in wait() — see wake()'s doc comment. Notifying
    // after releasing the queue lock (implicit: the MutexGuard above already
    // dropped at the end of the previous statement) keeps this off the
    // queue's own critical section.
    wake().1.notify_all();
    Value::Unit
}

/// `channel_depth(name: Text) -> Int` — TEMPORARY instrumentation
/// (AXVERITY_INSERT_PATH_TIMING_AUDIT_V1): the current number of pending items in
/// `name`'s unbounded queue (e.g. sealed-but-not-yet-flushed block jobs on
/// "hotmem-frame"). Read-only: locks the queue, reads len, unlocks. No effect on
/// any functional path.
#[track_caller]
pub fn channel_depth(name: Value) -> Value {
    let n = name_of(&name);
    let chan = channel_for(&n);
    let d = chan.queue.lock().unwrap().len();
    Value::Int(d as i64)
}

/// `event_subscribe(name: Text) -> Unit`. Registers the current context as a
/// waiter on `name` and declares the channel buffer eagerly. Idempotent per
/// context — subscribing twice to the same name is a no-op.
#[track_caller]
pub fn event_subscribe(name: Value) -> Value {
    let n = name_of(&name);
    let _ = channel_for(&n); // declare the buffer eagerly (race-free with senders)
    SUBSCRIPTIONS.with(|s| {
        let mut s = s.borrow_mut();
        if !s.iter().any(|c| c == &n) {
            s.push(n);
        }
    });
    Value::Unit
}

/// `wait(handler: Fn) -> Value`. Blocks until at least one subscribed channel
/// has a message, drains every currently-pending message across all subscribed
/// channels into a single `Value::List`, then calls `handler(list)`
/// synchronously and returns its result.
///
/// `handler` is a bare `fn(Value) -> Value` pointer (see CLOSURE_RULE_HARD in the
/// module docs): it is a local, used exactly once, and never stored.
#[track_caller]
pub fn wait(handler: fn(Value) -> Value) -> Value {
    // Snapshot the buffers this context subscribes to. `channel_for` is
    // get-or-create, so a subscribed-but-never-sent channel simply stays empty.
    let chans: Vec<Arc<Channel>> =
        SUBSCRIPTIONS.with(|s| s.borrow().iter().map(|n| channel_for(n)).collect());
    if chans.is_empty() {
        panic!("wait: current context has no subscriptions (call event_subscribe first)");
    }

    let lockfree = channel_queue_lockfree();
    let mut drained: Vec<Value> = Vec::new();
    loop {
        for chan in &chans {
            if lockfree {
                // Single-consumer drain (this waiting context is the channel's
                // sole consumer — see Channel's contract note). `Inconsistent`
                // (a producer mid-push) is treated like empty for this round;
                // the 2ms wake re-check below picks the item up next iteration,
                // exactly as the mutex path's check-then-block safety net does.
                loop {
                    match chan.lfq.pop() {
                        mpsc_intrusive::PopResult::Value(v) => drained.push(v),
                        mpsc_intrusive::PopResult::Empty
                        | mpsc_intrusive::PopResult::Inconsistent => break,
                    }
                }
            } else {
                let mut q = chan.queue.lock().unwrap();
                while let Some(v) = q.pop_front() {
                    drained.push(v);
                }
            }
        }
        if !drained.is_empty() {
            break;
        }
        // Genuinely block (no CPU spend, no lock contention) instead of the
        // old yield_now busy-spin — see wake()'s doc comment for why a short
        // timeout, not an unbounded wait, is the correct safety net here.
        let (lock, cvar) = wake();
        let guard = lock.lock().unwrap();
        let _ = cvar.wait_timeout(guard, Duration::from_millis(2)).unwrap();
    }

    // Synchronous, in-frame invocation. WAIT_ALWAYS_LIST: the argument is a List.
    handler(Value::List(drained))
}

// ═════════════════════════════════════════════════════════════════════════════
// Bounded, block-on-full channel (BRIDGE_BOUNDED_CHANNEL_V1,
// AXVERITY_HOTPATH_PARALLEL_DISPATCH_V1)
//
// A SECOND, DISTINCT channel type — the existing unbounded `Channel` above is
// left exactly as-is (GC / fastmode / hotmem depend on its current
// grow-without-bound, never-block, drain-everything semantics). This one is the
// recovery-log group-commit transport and it exists to give the intent's
// CHANNELS_BLOCK_NOT_DROP hard limit a real mechanism: a full queue BLOCKS the
// producer (backpressure), it never drops and never grows without bound.
//
//   * send  — push one item; if the queue is at capacity, BLOCK the caller on
//             `not_full` until the janitor drains room. No drop, no error.
//   * drain — the janitor's batched pull: block until ≥1 item, then accumulate
//             into one batch until the batch-size CAP is reached OR the batch
//             WINDOW elapses since the first item — whichever comes first
//             (the intent's "flush on (window-timer OR batch-size-cap), first
//             wins"). Draining frees space and wakes blocked producers.
//
// Capacity, cap and window are all runtime-configurable and are NOT assigned a
// tuned value by this build (intent hard limit NO_HARDCODED_TUNING_VALUES): the
// capacity comes from `AXVERITY_RECLOG_CAP`; cap and window are passed in by the
// caller (reclog.rs reads them from env). The defaults below are explicit
// PLACEHOLDERS marked TODO-TUNE, present only so the path runs before the
// separate empirical tuning step — correctness (block-not-drop) does not depend
// on the specific value.
// ═════════════════════════════════════════════════════════════════════════════

/// Placeholder queue capacity (backpressure bound) — TODO-TUNE. Overridden by
/// `AXVERITY_RECLOG_CAP`. Not a tuning decision; just a value large enough to
/// run and small enough that "full" is reachable in a saturation test.
const BOUNDED_CAP_DEFAULT: usize = 1024;

fn bounded_cap_env() -> usize {
    std::env::var("AXVERITY_RECLOG_CAP")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(BOUNDED_CAP_DEFAULT)
}

/// A single bounded channel: one `VecDeque` guarded by a mutex, plus two
/// condvars — `not_full` wakes blocked producers when the janitor drains,
/// `not_empty` wakes the janitor when a producer sends.
struct BoundedChannel {
    queue: Mutex<VecDeque<Value>>,
    cap: usize,
    not_full: Condvar,
    not_empty: Condvar,
}

/// Process-global bounded-channel registry, SEPARATE from the unbounded
/// `registry()` above so the two channel families never alias a name.
fn bounded_registry() -> &'static Mutex<HashMap<String, Arc<BoundedChannel>>> {
    static REG: OnceLock<Mutex<HashMap<String, Arc<BoundedChannel>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Get-or-create the bounded channel named `name`, capacity fixed at first
/// touch from `AXVERITY_RECLOG_CAP`. Get-or-create (not reject-if-absent) makes
/// send/drain ordering across the producer and janitor threads race-free, same
/// as the unbounded `channel_for`.
fn bounded_channel_for(name: &str) -> Arc<BoundedChannel> {
    let mut reg = bounded_registry().lock().unwrap();
    reg.entry(name.to_string())
        .or_insert_with(|| {
            Arc::new(BoundedChannel {
                queue: Mutex::new(VecDeque::new()),
                cap: bounded_cap_env(),
                not_full: Condvar::new(),
                not_empty: Condvar::new(),
            })
        })
        .clone()
}

/// Push `item`, blocking the caller while the queue is at capacity (backpressure).
/// Internal Rust entry — reclog.rs submits through here.
pub(crate) fn bounded_send_blocking(name: &str, item: Value) {
    let ch = bounded_channel_for(name);
    let mut q = ch.queue.lock().unwrap();
    while q.len() >= ch.cap {
        // BLOCK on full — CHANNELS_BLOCK_NOT_DROP. No drop, no error, no growth
        // past `cap`. The janitor's drain wakes us via `not_full`.
        q = ch.not_full.wait(q).unwrap();
    }
    q.push_back(item);
    drop(q);
    ch.not_empty.notify_one();
}

/// Non-blocking TRY-send for SHADOW taps (BRIDGE_SLABSHADOW_V1,
/// AXVERITY_RECLOG_SLA_BLOCK_SHADOW_VALIDATION_V1): push if below capacity,
/// DROP otherwise — returns whether the item was enqueued. NEVER blocks and
/// never grows the queue past `cap`, so a dead/lagging consumer costs the
/// producer exactly one lock + len check per call (bounded memory, bounded
/// latency — the failure-isolation contract a measurement-only tap needs).
/// Deliberately NOT used by the recovery-log path, whose CHANNELS_BLOCK_NOT_DROP
/// hard limit requires `bounded_send_blocking` above.
pub(crate) fn bounded_try_send(name: &str, item: Value) -> bool {
    let ch = bounded_channel_for(name);
    let mut q = ch.queue.lock().unwrap();
    if q.len() >= ch.cap {
        return false;
    }
    q.push_back(item);
    drop(q);
    ch.not_empty.notify_one();
    true
}

/// Current depth of a bounded channel (test-support; the bounded twin of
/// `channel_depth`). Locks, reads len, unlocks. cfg(test): only the shadow
/// drills use it today — lift the gate if runtime instrumentation ever needs it.
#[cfg(test)]
pub(crate) fn bounded_len(name: &str) -> usize {
    bounded_channel_for(name).queue.lock().unwrap().len()
}

/// Block until ≥1 item, then accumulate one batch until `max` items OR
/// `window_ms` since the first item — first wins. Draining frees room and wakes
/// blocked producers. Internal Rust entry — reclog.rs's janitor drains here.
pub(crate) fn bounded_drain_batch(name: &str, max: usize, window_ms: u64) -> Vec<Value> {
    let ch = bounded_channel_for(name);
    let cap = max.max(1);
    let window = Duration::from_millis(window_ms);
    let mut q = ch.queue.lock().unwrap();

    // Block until the batch has its first item.
    while q.is_empty() {
        q = ch.not_empty.wait(q).unwrap();
    }

    let mut out: Vec<Value> = Vec::new();
    while out.len() < cap {
        match q.pop_front() {
            Some(v) => out.push(v),
            None => break,
        }
    }

    // Accumulate more until the CAP or the WINDOW is hit, whichever first. The
    // window is measured from the first item (batch start ≈ now).
    let start = Instant::now();
    while out.len() < cap {
        let elapsed = start.elapsed();
        if elapsed >= window {
            break; // window expired — flush what we have
        }
        let remaining = window - elapsed;
        let (guard, timed_out) = {
            let (g, t) = ch.not_empty.wait_timeout(q, remaining).unwrap();
            (g, t.timed_out())
        };
        q = guard;
        while out.len() < cap {
            match q.pop_front() {
                Some(v) => out.push(v),
                None => break,
            }
        }
        if timed_out {
            break;
        }
    }

    drop(q);
    // We drained ≥1 item, so there is now room — wake any blocked producers.
    ch.not_full.notify_all();
    out
}

// ── Value-ABI wrappers (THREE_PIECE: also in emit/rust_05.rs + axis-bridge.axreg) ──

/// `bchan_send(name: Text, item: Value) -> Unit`. Blocking on full.
///
/// Calling convention: unary `Value::Tuple([name, item])` (the 2-arg data-fn
/// convention, same as `channel_send`).
#[track_caller]
pub fn bchan_send(args: Value) -> Value {
    let (name, item) = match args {
        Value::Tuple(mut es) if es.len() == 2 => {
            let item = es.pop().unwrap();
            let name = es.pop().unwrap();
            (name_of(&name), item)
        }
        other => panic!("bchan_send: expected Tuple(Text, Value), got {:?}", other),
    };
    bounded_send_blocking(&name, item);
    Value::Unit
}

/// `bchan_drain(name: Text, max: Int, window_ms: Int) -> List`. Blocks until ≥1
/// item, returns one batch (cap-or-window bounded) as a `Value::List`.
#[track_caller]
pub fn bchan_drain(args: Value) -> Value {
    let (name, max, window_ms) = match args {
        Value::Tuple(es) if es.len() == 3 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("bchan_drain: expected Tuple(Text, Int, Int), got {:?}", other),
    };
    let name = name_of(&name);
    let max = match max {
        Value::Int(n) if n >= 1 => n as usize,
        Value::Int(_) => 1,
        other => panic!("bchan_drain: arg 1 (max) expected Int, got {:?}", other),
    };
    let window_ms = match window_ms {
        Value::Int(n) if n >= 0 => n as u64,
        Value::Int(_) => 0,
        other => panic!("bchan_drain: arg 2 (window_ms) expected Int, got {:?}", other),
    };
    Value::List(bounded_drain_batch(&name, max, window_ms))
}

// ── Unbounded channel_send → event_subscribe → wait unit coverage ────────────
//
// AXVERITY_BRIDGE_LOCKFREE_EXPERIMENT_V1: the unbounded IPC primitive shipped
// with NO direct Rust unit test — only indirect M1 `--entries` coverage. These
// put the primitive itself under test: (a) `wait` blocks until a producer sends,
// (b) it drains EVERY pending message across ALL subscribed channels into one
// List in subscription/FIFO order, and (c) it does NOT busy-spin while blocked
// (per-thread CPU time ≪ wall time — the design correction in `wake()`'s doc).
// Distinct channel names per test (the registry is process-global); SUBSCRIPTIONS
// is thread-local so each test thread starts unsubscribed.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::value::intern_str;
    use std::cell::RefCell;
    use std::thread;
    use std::time::{Duration, Instant};

    fn nm(s: &str) -> Value { Value::Str(intern_str(s)) }
    fn send(ch: &str, n: i64) {
        channel_send(Value::Tuple(vec![nm(ch), Value::Int(n)]));
    }

    thread_local! {
        // `wait` runs the handler synchronously ON the waiting thread, so this
        // thread-local correctly captures that thread's drained batch.
        static DRAINED: RefCell<Vec<i64>> = RefCell::new(Vec::new());
    }
    fn capture(v: Value) -> Value {
        if let Value::List(items) = v {
            DRAINED.with(|d| {
                let mut d = d.borrow_mut();
                for it in items {
                    if let Value::Int(n) = it { d.push(n); }
                }
            });
        }
        Value::Unit
    }
    fn take_drained() -> Vec<i64> { DRAINED.with(|d| std::mem::take(&mut *d.borrow_mut())) }

    fn thread_cpu_nanos() -> u128 {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: valid out-param; CLOCK_THREAD_CPUTIME_ID reads THIS thread's CPU time.
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts); }
        ts.tv_sec as u128 * 1_000_000_000 + ts.tv_nsec as u128
    }

    /// (b)+(c): `wait` drains every pending message across ALL subscribed channels
    /// into ONE List, in subscription order with FIFO within each channel.
    #[test]
    fn wait_drains_all_subscribed_channels_in_order() {
        let (a, b) = ("t_order_a", "t_order_b");
        event_subscribe(nm(a));
        event_subscribe(nm(b));
        // Enqueue everything BEFORE waiting so the first wait() drains it all.
        send(a, 10); send(a, 11);
        send(b, 20); send(b, 21);
        let _ = take_drained();
        wait(capture);
        // subscription order [a, b]; FIFO within each.
        assert_eq!(take_drained(), vec![10, 11, 20, 21], "drain-all / fan-in order wrong");
    }

    /// (a): `wait` BLOCKS until a producer on another thread sends, then returns
    /// with exactly that message (never an empty early return).
    #[test]
    fn wait_blocks_until_message_arrives() {
        event_subscribe(nm("t_block"));
        let _ = take_drained();
        let producer = thread::spawn(|| {
            thread::sleep(Duration::from_millis(60));
            send("t_block", 77);
        });
        let t0 = Instant::now();
        wait(capture); // must block ~60ms until the producer sends
        let waited = t0.elapsed();
        producer.join().unwrap();
        assert_eq!(take_drained(), vec![77], "wait returned without the produced message");
        assert!(waited >= Duration::from_millis(40),
            "wait returned in {waited:?} — it did not actually block for the producer");
    }

    /// (c): while blocked, `wait` must NOT busy-spin — its per-thread CPU time
    /// over a ~120ms block is a small fraction of wall time. A `yield_now` spin
    /// (the regression `wake()` documents) would make CPU ≈ wall.
    #[test]
    fn wait_does_not_busy_spin() {
        event_subscribe(nm("t_nospin"));
        let _ = take_drained();
        let producer = thread::spawn(|| {
            thread::sleep(Duration::from_millis(120));
            send("t_nospin", 5);
        });
        let cpu0 = thread_cpu_nanos();
        let wall0 = Instant::now();
        wait(capture);
        let cpu = thread_cpu_nanos() - cpu0;
        let wall = wall0.elapsed().as_nanos();
        producer.join().unwrap();
        assert_eq!(take_drained(), vec![5]);
        assert!(wall > 80_000_000, "wall {wall}ns too short to be a meaningful block");
        // Generous 25% ceiling tolerates the 2ms re-check loop + spurious
        // cross-test condvar wakeups under the parallel test runner.
        assert!((cpu as f64) < 0.25 * (wall as f64),
            "wait burned {cpu}ns CPU over {wall}ns wall — looks like a busy-spin");
    }
}

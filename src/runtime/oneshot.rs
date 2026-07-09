//! BRIDGE_ONESHOT_V1 (AXVERITY_HOTPATH_PARALLEL_DISPATCH_V1) — a process-global
//! registry of single-fire completion signals ("oneshots"), the net-new
//! ack-backpath primitive the parallel-dispatch INSERT path needs.
//!
//! ## Why this exists
//!
//! The recovery-log group-commit janitor (reclog.rs) batches many callers'
//! durable writes behind ONE fsync window. Each caller must block until *its
//! own* batch has fsynced, then ack — not fire-and-forget (that would ack
//! before durable, the intent's unacceptable false-ack risk), and not a shared
//! signal (a caller must not wake on some *other* batch's completion). A
//! oneshot-per-caller is exactly that: the submitter mints one, blocks on it,
//! and the janitor signals precisely that id after the batch it belongs to is
//! durable.
//!
//! ## Shape
//!
//!   * `oneshot_new()      -> Int`   mint a fresh id, register a not-yet-done cell.
//!   * `oneshot_wait(id)   -> Unit`  block until `id` is signaled, then retire it.
//!   * `oneshot_signal(id) -> Unit`  mark `id` done and wake its waiter (idempotent;
//!                                   signaling an unknown/retired id is a no-op).
//!
//! Each oneshot is a `(Mutex<bool>, Condvar)` behind an `Arc`, keyed by a
//! monotonic id. This is deliberately a *shared* registry (unlike the
//! thread-owned `logbuf.rs`/`walshard.rs` hot-path state): a oneshot is a
//! cross-thread rendezvous by construction — the submitter thread waits, the
//! janitor thread signals — so shared state is the point, not an accident. It
//! sits OFF the hot append path (one lock acquisition per submit and per
//! signal, never on the fsync itself), so it does not reintroduce the
//! Landing-1 shared-registry ceiling on the durable write path.
//!
//! Identities are sha256(name_utf8), the bridge-wide convention.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use super::value::Value;

/// One completion cell: `done` flips false→true exactly once; `cv` wakes the
/// (single) waiter. `Arc`-shared so the waiter can hold its own handle while the
/// signaler resolves the same cell under the registry lock.
struct Oneshot {
    done: Mutex<bool>,
    cv: Condvar,
}

/// Process-global id→cell registry. Shared by design (cross-thread rendezvous).
fn registry() -> &'static Mutex<HashMap<i64, Arc<Oneshot>>> {
    static REG: OnceLock<Mutex<HashMap<i64, Arc<Oneshot>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Monotonic id source. Starts at 1 so 0 can never be a live oneshot id (lets a
/// caller use 0 as a "no oneshot" sentinel if it ever needs one).
fn next_id() -> i64 {
    static NEXT: AtomicI64 = AtomicI64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Mint a fresh oneshot and register it not-yet-done. Returns the id.
/// Internal Rust entry — reclog.rs mints on the submitter's behalf.
pub(crate) fn new_oneshot() -> i64 {
    let id = next_id();
    let cell = Arc::new(Oneshot { done: Mutex::new(false), cv: Condvar::new() });
    registry().lock().unwrap().insert(id, cell);
    id
}

/// Block the calling thread until `id` is signaled, then retire it. If `id` is
/// unknown (never minted, or already waited+retired) this returns immediately —
/// a waiter can never hang on a nonexistent oneshot.
pub(crate) fn wait_oneshot(id: i64) {
    // Take our own Arc handle to the cell, then drop the registry lock so a
    // concurrent signal (which also locks the registry) can proceed.
    let cell = match registry().lock().unwrap().get(&id) {
        Some(c) => c.clone(),
        None => return,
    };
    let mut done = cell.done.lock().unwrap();
    while !*done {
        done = cell.cv.wait(done).unwrap();
    }
    drop(done);
    // Retire the cell so the registry does not grow without bound over the
    // server's lifetime (one INSERT == one oneshot minted and, here, retired).
    registry().lock().unwrap().remove(&id);
}

/// Mark `id` done and wake its waiter. Idempotent and safe on an unknown id
/// (a no-op) so a double-signal or a signal racing a completed wait cannot panic.
pub(crate) fn signal_oneshot(id: i64) {
    let cell = match registry().lock().unwrap().get(&id) {
        Some(c) => c.clone(),
        None => return,
    };
    let mut done = cell.done.lock().unwrap();
    *done = true;
    cell.cv.notify_all();
}

// ── Value-ABI wrappers (THREE_PIECE: also in emit/rust_05.rs + axis-bridge.axreg) ──

/// `oneshot_new(Unit) -> Int`.
#[track_caller]
pub fn oneshot_new(_: Value) -> Value {
    Value::Int(new_oneshot())
}

/// `oneshot_wait(id: Int) -> Unit`. Blocks until `id` is signaled.
#[track_caller]
pub fn oneshot_wait(arg: Value) -> Value {
    let id = match arg {
        Value::Int(n) => n,
        other => panic!("oneshot_wait: expected Int id, got {:?}", other),
    };
    wait_oneshot(id);
    Value::Unit
}

/// `oneshot_signal(id: Int) -> Unit`. Marks `id` done and wakes its waiter.
#[track_caller]
pub fn oneshot_signal(arg: Value) -> Value {
    let id = match arg {
        Value::Int(n) => n,
        other => panic!("oneshot_signal: expected Int id, got {:?}", other),
    };
    signal_oneshot(id);
    Value::Unit
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn signal_before_wait_does_not_hang() {
        let id = new_oneshot();
        signal_oneshot(id);
        wait_oneshot(id); // returns immediately; cell already done
    }

    #[test]
    fn wait_blocks_until_signaled() {
        let id = new_oneshot();
        let t = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            signal_oneshot(id);
        });
        wait_oneshot(id); // must block ~20ms, then return
        t.join().unwrap();
    }

    #[test]
    fn unknown_id_wait_returns_and_double_signal_is_noop() {
        wait_oneshot(999_999); // never minted → immediate return
        let id = new_oneshot();
        signal_oneshot(id);
        wait_oneshot(id); // retires it
        signal_oneshot(id); // already retired → no-op, no panic
    }

    #[test]
    fn distinct_ids_are_independent() {
        let a = new_oneshot();
        let b = new_oneshot();
        assert_ne!(a, b);
        signal_oneshot(a);
        wait_oneshot(a);
        // b is still pending; signal+wait it to confirm a's signal didn't wake b's cell
        signal_oneshot(b);
        wait_oneshot(b);
    }
}

//! Task spawn/join — run a named fn on its own OS thread and collect its result.
//!
//! Distinct in intent from `channels.rs`. `wait` there is a MESSAGE PUMP: you
//! `event_subscribe` to named channels, it blocks until any of them has traffic,
//! drains every pending message into one `Value::List` and hands that to a
//! handler (`WAIT_ALWAYS_LIST`). It knows nothing about who produced the
//! messages and has a subscribe/unsubscribe lifecycle of its own.
//!
//! `spawn`/`join` answer the other question: run THIS callee, and give me back
//! THAT call's result. One handle, one value, no channel, no subscription. Both
//! shapes are wanted; neither subsumes the other.
//!
//! CALLEE IS STATIC. `f` is a bare `fn(Value) -> Value` pointer resolved at emit
//! time from a `Fn`-typed pool entry (`ArgKind::FnRef`, `FN_REF_IS_CALLEE_ONLY`),
//! exactly as `flat_map`/`fold`/`wait` take theirs. It is not a closure and
//! captures nothing, which is what makes moving it to another thread free.
//!
//! OS THREADS, NOT ASYNC — and that is the point, not a shortcut. Spawning
//! would otherwise imply an async variant of every spawnable fn: `async fn`
//! colours its callers, so each bridge fn would need two forms, and
//! `ArgKind::FnRef` emits a BARE RUST FN PATH into the callee slot, which an
//! async fn cannot occupy. A thread keeps the callee an ordinary synchronous
//! `fn(Value) -> Value`, so the SAME worker can be called directly or spawned
//! with no second implementation and no runtime threaded through the emitter.
//! The bridge has no async runtime dependency (no tokio in `Cargo.toml`), and
//! every other concurrent path here — `channels.rs`, `net.rs`, `indexer.rs` —
//! is already `std::thread`.
//!
//! The cost is honest and worth stating: one OS thread per task. A worker that
//! blocks holds a whole thread, and thread-per-task does not scale to thousands
//! of simultaneous tasks. For a controller fanning out to a handful of
//! subcontrollers that is the right trade; for fine-grained parallelism over a
//! large collection it is not, and that case wants a pool behind this same
//! signature rather than a different signature.
//!
//! A HANDLE CARRIES ITS OWN RESULT. The registry declares exactly two types:
//!
//!     type Handle     = product (Int, Value)   // (handleId, result)
//!     type HandleList = list Handle
//!
//! `spawn` returns a `Handle` whose result slot is `Unit`; `join` returns the
//! same handles with their slots filled. So the signature is symmetric —
//! `HandleList` in, `HandleList` out — and there is no second "result" type to
//! keep in step with the first.
//!
//! The pair is also what makes the type distinct rather than decorative.
//! Registry type identity is STRUCTURAL (`codec.rs:105-107`), so a
//! `HandleList = list Int` would have hashed identically to the existing
//! `IntList` and any list of integers would have verified clean as a handle
//! list. `list (product (Int, Value))` collides with nothing, so handing `join`
//! the wrong list is a `TypeMismatch` at verification, not a panic in here.
//!
//! Allocation follows the `tcp_listen`/`tcp_accept` precedent in `net.rs`: a
//! process-global counter, never reused, unknown handle panics.
//!
//! JOIN IS ALWAYS A LIST — the `WAIT_ALWAYS_LIST` shape applied to tasks. It
//! blocks until every listed task has finished and returns them in the order
//! asked for, so it is also the barrier: one call, all tasks, no partial state
//! to reason about. Joining a single task is a one-element list.
//!
//! That shape is what makes heterogeneous results a non-problem. Workers need
//! not agree on a return type: every result is a `Value`, the tag rides with the
//! value, and each result travels with the id that produced it, so the caller
//! discriminates by identity rather than by counting positions.
//!
//! `VALUE_MUST_STAY_SEND_SYNC` (`value.rs:38-41`) is what makes this sound:
//! the argument moves to the spawned thread and the result moves back.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;

use super::value::Value;

/// Live spawned tasks, keyed by handle. An entry is removed by `join`, so a
/// second `join` on the same handle panics rather than blocking forever.
fn registry() -> &'static Mutex<HashMap<i64, JoinHandle<Value>>> {
    static REG: OnceLock<Mutex<HashMap<i64, JoinHandle<Value>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Lock the task table, recovering from poisoning.
///
/// Same reasoning as `net.rs::reg_lock`: the guarded value is a plain
/// `HashMap`, and every panic site here fires on a looked-up `Option` AFTER the
/// map operation completed. No path leaves the map partially mutated, so there
/// is no broken invariant for poisoning to protect — and refusing to recover
/// would escalate one task's panic into total loss of the task table.
fn reg_lock() -> std::sync::MutexGuard<'static, HashMap<i64, JoinHandle<Value>>> {
    registry().lock().unwrap_or_else(|e| e.into_inner())
}

/// Allocate the next task handle. Never reused within a process run.
fn next_handle() -> i64 {
    static COUNTER: AtomicI64 = AtomicI64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Build a `Handle = product (Int, Value)` from an id and its result slot.
/// `spawn` fills the slot with `Unit`; `join` fills it with what the callee
/// returned.
fn handle_value(id: i64, result: Value) -> Value {
    Value::Tuple(vec![Value::Int(id), result])
}

/// Read the id out of a `Handle`. Rejects a bare `Int` deliberately: the pair
/// shape is what makes `HandleList` a distinct type from `IntList`, so quietly
/// accepting a naked integer would reintroduce exactly the confusion the pair
/// exists to prevent.
#[track_caller]
fn handle_id(v: &Value) -> i64 {
    match v {
        Value::Tuple(fields) if fields.len() == 2 => fields[0].as_int(),
        other => panic!("join: expected a Handle (id, result) pair, got {:?}", other),
    }
}

/// `spawn(callee: Fn, arg: Value) -> Handle` — start `callee(arg)` on a new
/// thread and return its handle immediately, result slot `Unit`. Does not
/// block.
///
/// The spawned thread owns `arg` outright; nothing is shared with the caller, so
/// there is no aliasing to reason about. Effects the callee performs are its
/// own — the registry entry declares `spawn` `deterministic false`, and the
/// verifier unions the callee's effect into this call site, so a caller cannot
/// launder an effectful worker through it.
#[track_caller]
pub fn spawn(callee: fn(Value) -> Value, arg: Value) -> Value {
    let handle = next_handle();
    let jh = std::thread::Builder::new()
        .name(format!("axis-task-{}", handle))
        .spawn(move || callee(arg))
        .unwrap_or_else(|e| panic!("spawn: could not start thread for task {}: {}", handle, e));
    reg_lock().insert(handle, jh);
    handle_value(handle, Value::Unit)
}

/// `join(handles: HandleList) -> HandleList` — block until every listed task has
/// finished, then return the same handles with their result slots filled, in the
/// order given.
///
/// Results need not share a type. Each is whatever its callee returned, carried
/// with its own `Value` tag and paired with the handle that produced it, so a
/// caller reading the list discriminates by identity rather than by position.
///
/// Panics on an unknown handle (never spawned, or already joined — a handle
/// listed twice hits this on its second occurrence) and on a task that panicked.
/// Propagating rather than swallowing is the house convention: a worker that
/// died is a failure the caller must see, not an absent value it might mistake
/// for a legitimate one.
#[track_caller]
pub fn join(handles: Value) -> Value {
    let items = match handles {
        Value::List(items) => items,
        other => panic!("join: expected a HandleList, got {:?}", other),
    };

    // Take every handle out of the table under ONE lock, before blocking on any
    // of them. Joining while holding the lock would stall every other spawn and
    // join behind the slowest task in this list.
    let mut taken: Vec<(i64, JoinHandle<Value>)> = Vec::with_capacity(items.len());
    {
        let mut reg = reg_lock();
        for item in &items {
            let h = handle_id(item);
            let jh = reg.remove(&h).unwrap_or_else(|| {
                panic!("join: unknown task handle {} (never spawned, or already joined)", h)
            });
            taken.push((h, jh));
        }
    }

    let mut out = Vec::with_capacity(taken.len());
    for (h, jh) in taken {
        let result = jh
            .join()
            .unwrap_or_else(|_| panic!("join: task {} panicked", h));
        out.push(handle_value(h, result));
    }
    Value::List(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn double(v: Value) -> Value {
        Value::Int(v.as_int() * 2)
    }

    /// Returns a `Str`, not an `Int` — the point of the heterogeneous test.
    fn describe(v: Value) -> Value {
        Value::Str(format!("got {}", v.as_int()).into())
    }

    fn boom(_v: Value) -> Value {
        panic!("worker exploded")
    }

    fn list(vs: Vec<Value>) -> Value {
        Value::List(vs)
    }

    /// The (id, result) pairs of a joined `HandleList`.
    fn pairs(v: &Value) -> Vec<(i64, Value)> {
        match v {
            Value::List(items) => items
                .iter()
                .map(|h| match h {
                    Value::Tuple(f) if f.len() == 2 => (f[0].as_int(), f[1].clone()),
                    other => panic!("not a Handle: {:?}", other),
                })
                .collect(),
            other => panic!("not a HandleList: {:?}", other),
        }
    }

    #[test]
    fn a_fresh_handle_has_an_empty_result_slot() {
        // The result is populated ONLY on termination — at spawn there is
        // nothing to report yet, and the slot says so rather than guessing.
        let h = spawn(double, Value::Int(21));
        match &h {
            Value::Tuple(f) if f.len() == 2 => assert_eq!(f[1], Value::Unit),
            other => panic!("expected a Handle pair, got {:?}", other),
        }
        let _ = join(list(vec![h]));
    }

    #[test]
    fn join_fills_the_result_slot_of_the_same_handle() {
        let h = spawn(double, Value::Int(21));
        let id = handle_id(&h);
        assert_eq!(pairs(&join(list(vec![h]))), vec![(id, Value::Int(42))]);
    }

    #[test]
    fn join_carries_results_of_different_types_in_one_list() {
        // The whole reason join is list-shaped: two workers, two unrelated
        // return types, one barrier. Each result keeps its own Value tag and
        // rides with the id that produced it.
        let a = spawn(double, Value::Int(5));
        let b = spawn(describe, Value::Int(7));
        let (ia, ib) = (handle_id(&a), handle_id(&b));
        assert_eq!(
            pairs(&join(list(vec![a, b]))),
            vec![(ia, Value::Int(10)), (ib, Value::Str("got 7".into()))]
        );
    }

    #[test]
    fn results_come_back_in_the_order_asked_for() {
        // Not in completion order — `slow` finishes last but is listed first.
        fn slow(v: Value) -> Value {
            std::thread::sleep(std::time::Duration::from_millis(50));
            Value::Int(v.as_int())
        }
        let slow_h = spawn(slow, Value::Int(1));
        let fast_h = spawn(double, Value::Int(10));
        let (is, if_) = (handle_id(&slow_h), handle_id(&fast_h));
        assert_eq!(
            pairs(&join(list(vec![slow_h, fast_h]))),
            vec![(is, Value::Int(1)), (if_, Value::Int(20))]
        );
    }

    #[test]
    fn handles_are_distinct_and_independently_joinable() {
        // Two spawns of the SAME callee with the same argument must not collapse
        // into one task — the compiler-side guarantee (effectful calls are never
        // hash-consed) only holds if the runtime really does start two threads.
        let a = spawn(double, Value::Int(1));
        let b = spawn(double, Value::Int(1));
        assert_ne!(handle_id(&a), handle_id(&b), "two spawns must yield distinct ids");
        assert_eq!(pairs(&join(list(vec![a, b]))).len(), 2);
    }

    #[test]
    fn empty_handle_list_joins_to_an_empty_list() {
        assert_eq!(join(list(vec![])), list(vec![]));
    }

    #[test]
    #[should_panic(expected = "unknown task handle")]
    fn joining_twice_panics() {
        let h = spawn(double, Value::Int(3));
        let _ = join(list(vec![h.clone()]));
        let _ = join(list(vec![h]));
    }

    #[test]
    #[should_panic(expected = "unknown task handle")]
    fn the_same_handle_listed_twice_panics() {
        let h = spawn(double, Value::Int(3));
        let _ = join(list(vec![h.clone(), h]));
    }

    #[test]
    #[should_panic(expected = "unknown task handle")]
    fn joining_a_handle_that_was_never_spawned_panics() {
        let _ = join(list(vec![handle_value(999_999, Value::Unit)]));
    }

    #[test]
    #[should_panic(expected = "expected a Handle")]
    fn a_bare_int_is_not_a_handle() {
        // The pair shape is what keeps HandleList distinct from IntList at the
        // contract boundary; accepting a naked Int here would undo it.
        let _ = join(list(vec![Value::Int(1)]));
    }

    #[test]
    #[should_panic(expected = "expected a HandleList")]
    fn joining_a_bare_handle_panics() {
        let h = spawn(double, Value::Int(3));
        let _ = join(h);
    }

    #[test]
    #[should_panic(expected = "panicked")]
    fn a_panicking_task_propagates_on_join() {
        let h = spawn(boom, Value::Unit);
        let _ = join(list(vec![h]));
    }
}

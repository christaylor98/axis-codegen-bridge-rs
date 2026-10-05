//! Iteration / list-builder primitives for M1.
//!
//! Higher-order primitives (`foreach`, `loop_count`) take their callee as a
//! native Rust `fn(Value) -> Value` pointer — NOT as a `Value`. The emitter
//! resolves a `Fn`-typed pool entry's 32-byte identity payload to the bare
//! Rust fn path at translation time. There is no `Value::Fn` variant; a `Fn`
//! reference is never data. See BRIDGE_FOREIGN_FN_FNREF_M1.

use super::value::{intern_tag, truthy, Value};

/// LOOPS_STOP_ON_UNKNOWN_V1: a callback that yields an Unknown or an Err (a failed assert / raise / fault in the loop
/// body) ends the loop, and that value is the loop's result -- sticky, as everywhere else. Before this a driver
/// kept going: an Unknown counts as truthy, so `while` re-ran its body on the poisoned state up to the iteration cap
/// (found: a PyAx program spun at 100% CPU for 10 minutes after a failed assert).
fn stuck(v: &Value) -> bool {
    super::fault::is_unknown(v) || super::fault::is_err(v)
}

// ── List builders ────────────────────────────────────────────────────────────

/// `range(start, end) -> ValueList(Int)` — half-open `[start, end)`.
///
/// Calling convention: unary `Value::Tuple([start, end])`.
#[track_caller]
pub fn range(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() == 2 => {
            let s = es[0].as_int();
            let e = es[1].as_int();
            let items: Vec<Value> = if e > s {
                (s..e).map(Value::Int).collect()
            } else {
                Vec::new()
            };
            Value::List(super::value::ListBuf::from(items))
        }
        _ => panic!("range: expected Tuple(Int, Int), got {:?}", args),
    }
}

// ── Higher-order primitives (native multi-arg) ──────────────────────────────

/// `foreach(xs, callee) -> Unit` — applies `callee` to each element for its
/// effect; discards the result. Effect: `fullIo`.
///
/// Native multi-arg Rust signature — `callee` is a bare fn path resolved at
/// emit time from a `Fn`-typed pool entry.
#[track_caller]
pub fn foreach(list: Value, callee: fn(Value) -> Value) -> Value {
    match list {
        Value::List(items) => {
            for item in items {
                let r = callee(item);
                if stuck(&r) {
                    return r;
                }
            }
            Value::Unit
        }
        other => panic!("foreach: expected List, got {:?}", other),
    }
}

/// `loop_count(n, init, step) -> Value` — apply `step(acc)` `n` times,
/// starting from `init`. `n` is an `Int`; `step: fn(Value) -> Value`.
#[track_caller]
pub fn loop_count(n: i64, init: Value, step: fn(Value) -> Value) -> Value {
    let mut acc = init;
    let iters = if n > 0 { n as u64 } else { 0 };
    for _ in 0..iters {
        acc = step(acc);
        if stuck(&acc) {
            return acc;
        }
    }
    acc
}

/// `loop_while(init, cond, step, max) -> Value` — repeat `acc = step(acc)`
/// while `cond(acc)` is truthy, capped at `max` iterations (mech-gen-safe).
#[track_caller]
pub fn loop_while(
    init: Value,
    cond: fn(Value) -> Value,
    step: fn(Value) -> Value,
    max: Value,
) -> Value {
    let limit = max.as_int();
    let mut acc = init;
    let iters = if limit > 0 { limit as u64 } else { 0 };
    for _ in 0..iters {
        let c = cond(acc.clone());
        if stuck(&c) {
            return c;
        }
        if !truthy(&c) {
            break;
        }
        acc = step(acc);
        if stuck(&acc) {
            return acc;
        }
    }
    acc
}

/// `fold(xs, init, step) -> Value` — left fold. Threads ONLY the accumulator
/// through `step`; the list is iterated natively here. `step` receives a
/// `Tuple(acc, elem)` and returns the new acc.
///
/// This is the O(n) counterpart to accumulating with `loop_while`: `loop_while`
/// re-clones its ENTIRE threaded state every iteration (`cond(acc.clone())`), so
/// when the unconsumed input rides in that state the cost is O(n^2). `fold` keeps
/// the input out of the threaded value — only the (small) accumulator is
/// re-materialized per element. AXSEM_W2_ACCUM_PERF_V1.
///
/// Native multi-arg Rust signature — `step` is a bare fn path resolved at emit
/// time from a `Fn`-typed pool entry (arg kinds `[Data, Data, FnRef]`).
#[track_caller]
pub fn fold(list: Value, init: Value, step: fn(Value) -> Value) -> Value {
    match list {
        Value::List(items) => {
            let mut acc = init;
            for item in items {
                acc = step(Value::Tuple(vec![acc, item]));
                if stuck(&acc) {
                    return acc;
                }
            }
            acc
        }
        other => panic!("fold: expected List, got {:?}", other),
    }
}

// ── Phase 2: P1 vocabulary — HOFs ───────────────────────────────────────────

/// `flat_map(xs, callee) -> ValueList` — apply `callee` to each element,
/// flatten the resulting `ValueList`s into one.
#[track_caller]
pub fn flat_map(list: Value, callee: fn(Value) -> Value) -> Value {
    match list {
        Value::List(items) => {
            let mut out: Vec<Value> = Vec::new();
            for item in items {
                match callee(item) {
                    r if stuck(&r) => return r,
                    Value::List(inner) => out.extend(inner),
                    other => panic!(
                        "flat_map: callee must return ValueList, got {:?}",
                        other
                    ),
                }
            }
            Value::List(super::value::ListBuf::from(out))
        }
        other => panic!("flat_map: expected List, got {:?}", other),
    }
}

/// `filter(xs, pred) -> ValueList` — keep only elements where `pred` returns
/// truthy. Mirrors `any`'s single-arg-predicate shape; unlike `any`/`all` it
/// returns a `List`, not a `Bool`.
#[track_caller]
pub fn filter(list: Value, pred: fn(Value) -> Value) -> Value {
    match list {
        Value::List(items) => {
            let mut out: Vec<Value> = Vec::new();
            for item in items {
                let r = pred(item.clone());
                if stuck(&r) {
                    return r;
                }
                if truthy(&r) {
                    out.push(item);
                }
            }
            Value::List(super::value::ListBuf::from(out))
        }
        other => panic!("filter: expected List, got {:?}", other),
    }
}

/// `map(xs, callee) -> ValueList` — transform each element via `callee`.
/// Mirrors `flat_map`'s single-arg-callee shape but does not flatten —
/// `callee`'s return is pushed as-is, one in, one out.
#[track_caller]
pub fn map(list: Value, callee: fn(Value) -> Value) -> Value {
    match list {
        Value::List(items) => {
            let mut out: Vec<Value> = Vec::new();
            for item in items {
                let r = callee(item);
                if stuck(&r) {
                    return r;
                }
                out.push(r);
            }
            Value::List(super::value::ListBuf::from(out))
        }
        other => panic!("map: expected List, got {:?}", other),
    }
}

/// `any(xs, pred) -> Bool` — true if any element makes `pred` return truthy.
#[track_caller]
pub fn any(list: Value, pred: fn(Value) -> Value) -> Value {
    match list {
        Value::List(items) => {
            for item in items {
                let r = pred(item);
                if stuck(&r) {
                    return r;
                }
                if truthy(&r) {
                    return Value::Bool(true);
                }
            }
            Value::Bool(false)
        }
        other => panic!("any: expected List, got {:?}", other),
    }
}

/// `all(xs, pred) -> Bool` — true if every element makes `pred` return truthy.
#[track_caller]
pub fn all(list: Value, pred: fn(Value) -> Value) -> Value {
    match list {
        Value::List(items) => {
            for item in items {
                let r = pred(item);
                if stuck(&r) {
                    return r;
                }
                if !truthy(&r) {
                    return Value::Bool(false);
                }
            }
            Value::Bool(true)
        }
        other => panic!("all: expected List, got {:?}", other),
    }
}

/// `find_index(xs, pred) -> Int` — index of the first element where `pred`
/// returns truthy, or `-1` if none. (No `Option` because M1 lacks one.)
#[track_caller]
pub fn find_index(list: Value, pred: fn(Value) -> Value) -> Value {
    match list {
        Value::List(items) => {
            for (i, item) in items.into_iter().enumerate() {
                let r = pred(item);
                if stuck(&r) {
                    return r;
                }
                if truthy(&r) {
                    return Value::Int(i as i64);
                }
            }
            Value::Int(-1)
        }
        other => panic!("find_index: expected List, got {:?}", other),
    }
}

/// `count(xs, pred) -> Int` — count of elements where `pred` returns truthy.
#[track_caller]
pub fn count(list: Value, pred: fn(Value) -> Value) -> Value {
    match list {
        Value::List(items) => {
            let mut n: i64 = 0;
            for item in items {
                let r = pred(item);
                if stuck(&r) {
                    return r;
                }
                if truthy(&r) {
                    n += 1;
                }
            }
            Value::Int(n)
        }
        other => panic!("count: expected List, got {:?}", other),
    }
}

// ── Phase 2: P1 vocabulary — data fns (unary Tuple convention) ───────────────

/// `range_step(start, end, step) -> ValueList(Int)` — half-open `[start, end)`
/// with stride `step`. `step` must be non-zero.
#[track_caller]
pub fn range_step(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() == 3 => {
            let s = es[0].as_int();
            let e = es[1].as_int();
            let step = es[2].as_int();
            if step == 0 {
                panic!("range_step: step must be non-zero");
            }
            let mut out: Vec<Value> = Vec::new();
            let mut i = s;
            if step > 0 {
                while i < e {
                    out.push(Value::Int(i));
                    i += step;
                }
            } else {
                while i > e {
                    out.push(Value::Int(i));
                    i += step;
                }
            }
            Value::List(super::value::ListBuf::from(out))
        }
        _ => panic!("range_step: expected Tuple(Int, Int, Int), got {:?}", args),
    }
}

/// `repeat(v, n) -> ValueList` — `n` copies of `v`.
#[track_caller]
pub fn repeat(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() == 2 => {
            let v = es[0].clone();
            let n = es[1].as_int();
            let count = if n > 0 { n as usize } else { 0 };
            Value::List(super::value::ListBuf::from(vec![v; count]))
        }
        _ => panic!("repeat: expected Tuple(Value, Int), got {:?}", args),
    }
}

/// `enumerate(xs) -> ValueList(Value(Int, T))` — pair each element with its
/// index. The pair is an M1 compound `Value` (a Ctor tagged "Value"), built
/// the same way as `value_make`.
#[track_caller]
pub fn enumerate(list: Value) -> Value {
    match list {
        Value::List(items) => {
            let tag = intern_tag("Value");
            let pairs: Vec<Value> = items
                .into_iter()
                .enumerate()
                .map(|(i, v)| Value::Ctor {
                    tag,
                    fields: vec![Value::Int(i as i64), v],
                })
                .collect();
            Value::List(super::value::ListBuf::from(pairs))
        }
        other => panic!("enumerate: expected List, got {:?}", other),
    }
}

/// `zip(xs, ys) -> ValueList(Value(A, B))` — pair elements positionally;
/// truncates to the shorter list.
#[track_caller]
pub fn zip(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() == 2 => match (&es[0], &es[1]) {
            (Value::List(xs), Value::List(ys)) => {
                let tag = intern_tag("Value");
                let pairs: Vec<Value> = xs
                    .iter()
                    .zip(ys.iter())
                    .map(|(a, b)| Value::Ctor {
                        tag,
                        fields: vec![a.clone(), b.clone()],
                    })
                    .collect();
                Value::List(super::value::ListBuf::from(pairs))
            }
            (a, b) => panic!("zip: expected Tuple(List, List), got ({:?}, {:?})", a, b),
        },
        _ => panic!("zip: expected Tuple(List, List), got {:?}", args),
    }
}

/// `take(xs, n) -> ValueList` — first `n` elements (or all if shorter).
#[track_caller]
pub fn take(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() == 2 => match (&es[0], &es[1]) {
            (Value::List(items), Value::Int(n)) => {
                let k = if *n > 0 { *n as usize } else { 0 };
                Value::List(items.iter().take(k).cloned().collect())
            }
            (a, b) => panic!("take: expected Tuple(List, Int), got ({:?}, {:?})", a, b),
        },
        _ => panic!("take: expected Tuple(List, Int), got {:?}", args),
    }
}

/// `drop(xs, n) -> ValueList` — all elements after the first `n`.
#[track_caller]
pub fn drop(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() == 2 => match (&es[0], &es[1]) {
            (Value::List(items), Value::Int(n)) => {
                let k = if *n > 0 { *n as usize } else { 0 };
                Value::List(items.iter().skip(k).cloned().collect())
            }
            (a, b) => panic!("drop: expected Tuple(List, Int), got ({:?}, {:?})", a, b),
        },
        _ => panic!("drop: expected Tuple(List, Int), got {:?}", args),
    }
}

/// `slice(xs, start, end) -> ValueList` — half-open `[start, end)`; bounds
/// are clamped to `[0, len(xs)]`.
#[track_caller]
pub fn slice(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() == 3 => match (&es[0], &es[1], &es[2]) {
            (Value::List(items), Value::Int(s), Value::Int(e)) => {
                let len = items.len() as i64;
                let lo = (*s).clamp(0, len) as usize;
                let hi = (*e).clamp(0, len) as usize;
                let hi = hi.max(lo);
                Value::List(super::value::ListBuf::from(items[lo..hi].to_vec()))
            }
            (a, b, c) => panic!(
                "slice: expected Tuple(List, Int, Int), got ({:?}, {:?}, {:?})",
                a, b, c
            ),
        },
        _ => panic!("slice: expected Tuple(List, Int, Int), got {:?}", args),
    }
}

/// `flatten(xs) -> ValueList` — concatenate a list of lists.
#[track_caller]
pub fn flatten(list: Value) -> Value {
    match list {
        Value::List(items) => {
            // SHARED_LIST_V1: the result grows from the first list's own storage -- no copy when it is unshared, and
            // `flatten([xs])` (how PyAx re-types a list) is xs itself. Only the later lists' elements are moved in.
            let mut out: Option<super::value::ListBuf> = None;
            for item in items {
                match item {
                    Value::List(inner) => match out.as_mut() {
                        None => out = Some(inner),
                        Some(acc) if inner.is_empty() => { let _ = acc; }
                        Some(acc) => acc.extend(inner),
                    },
                    other => panic!(
                        "flatten: element must be List, got {:?}",
                        other
                    ),
                }
            }
            Value::List(out.unwrap_or_default())
        }
        other => panic!("flatten: expected List, got {:?}", other),
    }
}

#[cfg(test)]
mod stop_on_unknown_tests {
    // LOOPS_STOP_ON_UNKNOWN_V1: a callback yielding an Unknown/Err ends the driver with that value.
    use super::*;
    use crate::runtime::fault;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static STEPS: AtomicUsize = AtomicUsize::new(0);

    fn bad() -> Value { fault::unknown("test", "step", "boom", "here") }
    fn ints(xs: &[i64]) -> Value { Value::List(xs.iter().map(|x| Value::Int(*x)).collect()) }

    fn step_poisons_at_3(acc: Value) -> Value {
        STEPS.fetch_add(1, Ordering::SeqCst);
        if acc.as_int() >= 3 { bad() } else { Value::Int(acc.as_int() + 1) }
    }
    fn always(_: Value) -> Value { Value::Bool(true) }
    fn cond_unknown(_: Value) -> Value { bad() }
    fn neg_is_bad(v: Value) -> Value { if v.as_int() < 0 { bad() } else { Value::Bool(v.as_int() > 0) } }
    fn fold_step(args: Value) -> Value {
        match args { Value::Tuple(es) => if es[1].as_int() < 0 { bad() } else { Value::Int(es[0].as_int() + es[1].as_int()) }, _ => unreachable!() }
    }

    #[test]
    fn loop_while_stops_when_the_step_yields_an_unknown() {
        STEPS.store(0, Ordering::SeqCst);
        let r = loop_while(Value::Int(0), always, step_poisons_at_3, Value::Int(1_000_000_000));
        assert!(fault::is_unknown(&r));
        assert_eq!(STEPS.load(Ordering::SeqCst), 4);              // 0,1,2 -> 3, then the poisoned step: not 10^9
    }

    #[test]
    fn loop_while_stops_when_the_condition_yields_an_unknown() {
        let r = loop_while(Value::Int(0), cond_unknown, step_poisons_at_3, Value::Int(10));
        assert!(fault::is_unknown(&r));
    }

    #[test]
    fn fold_map_filter_and_the_predicates_stop_on_an_unknown() {
        assert!(fault::is_unknown(&fold(ints(&[1, 2, -1, 4]), Value::Int(0), fold_step)));
        assert_eq!(fold(ints(&[1, 2, 3]), Value::Int(0), fold_step), Value::Int(6));   // and still fold
        for r in [map(ints(&[1, -1, 2]), neg_is_bad), filter(ints(&[1, -1, 2]), neg_is_bad),
                  any(ints(&[0, -1, 2]), neg_is_bad), all(ints(&[1, -1, 2]), neg_is_bad),
                  count(ints(&[1, -1, 2]), neg_is_bad), find_index(ints(&[0, -1, 2]), neg_is_bad),
                  foreach(ints(&[1, -1, 2]), neg_is_bad), loop_count(10, Value::Int(0), step_poisons_at_3)] {
            assert!(fault::is_unknown(&r), "{:?}", r);
        }
        assert_eq!(filter(ints(&[0, 1, 2]), neg_is_bad), ints(&[1, 2]));               // unchanged otherwise
    }
}

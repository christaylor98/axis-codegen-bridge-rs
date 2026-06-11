//! Iteration / list-builder primitives for M1.
//!
//! Higher-order primitives (`foreach`, `loop_count`) take their callee as a
//! native Rust `fn(Value) -> Value` pointer — NOT as a `Value`. The emitter
//! resolves a `Fn`-typed pool entry's 32-byte identity payload to the bare
//! Rust fn path at translation time. There is no `Value::Fn` variant; a `Fn`
//! reference is never data. See BRIDGE_FOREIGN_FN_FNREF_M1.

use super::value::{intern_tag, truthy, Value};

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
            Value::List(items)
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
                let _ = callee(item);
            }
            Value::Unit
        }
        other => panic!("foreach: expected List, got {:?}", other),
    }
}

/// `loop_count(n, init, step) -> Value` — apply `step(acc)` `n` times,
/// starting from `init`. `n` is an `Int`; `step: fn(Value) -> Value`.
#[track_caller]
pub fn loop_count(n: Value, init: Value, step: fn(Value) -> Value) -> Value {
    let count = n.as_int();
    let mut acc = init;
    let iters = if count > 0 { count as u64 } else { 0 };
    for _ in 0..iters {
        acc = step(acc);
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
        if !truthy(&cond(acc.clone())) {
            break;
        }
        acc = step(acc);
    }
    acc
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
                    Value::List(inner) => out.extend(inner),
                    other => panic!(
                        "flat_map: callee must return ValueList, got {:?}",
                        other
                    ),
                }
            }
            Value::List(out)
        }
        other => panic!("flat_map: expected List, got {:?}", other),
    }
}

/// `any(xs, pred) -> Bool` — true if any element makes `pred` return truthy.
#[track_caller]
pub fn any(list: Value, pred: fn(Value) -> Value) -> Value {
    match list {
        Value::List(items) => {
            for item in items {
                if truthy(&pred(item)) {
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
                if !truthy(&pred(item)) {
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
                if truthy(&pred(item)) {
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
                if truthy(&pred(item)) {
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
            Value::List(out)
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
            Value::List(vec![v; count])
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
            Value::List(pairs)
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
                Value::List(pairs)
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
                Value::List(items[lo..hi].to_vec())
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
            let mut out: Vec<Value> = Vec::new();
            for item in items {
                match item {
                    Value::List(inner) => out.extend(inner),
                    other => panic!(
                        "flatten: element must be List, got {:?}",
                        other
                    ),
                }
            }
            Value::List(out)
        }
        other => panic!("flatten: expected List, got {:?}", other),
    }
}

use super::arith::value_eq;
use super::value::{Value, get_str};

// ── M1_VALUELIST_NARROWING_V1 ─────────────────────────────────────────────
//
// Every other fn in this file is element-agnostic: `Value::List(Vec<Value>)`
// is a single untyped vector, and every op above is a match on the List tag
// that never inspects an element. A narrowing conversion is the first
// operation that must — it exists to reject a ValueList whose elements are
// not all the target tag, and NO_DEFAULTING_EVER means there is no arm that
// substitutes a value or silently drops a mismatched one: a bad element is a
// panic, not a computation.
//
// The panic must name the offending index and the actual tag found (Chris's
// binding requirement on this intent) — "expected Int" alone tells a caller
// nothing about WHERE the list went wrong, the same defect as int_div's bare
// "division by zero".

fn value_tag_name(v: &Value) -> &'static str {
    match v {
        Value::Int(_) => "Int",
        Value::Bool(_) => "Bool",
        Value::Str(_) => "Text",
        Value::Unit => "Unit",
        Value::Tuple(_) => "Tuple",
        Value::List(_) => "List",
        Value::Ctor { .. } => "Ctor",
        Value::Dec(_) => "Dec",
        Value::Float(_) => "Float",
        Value::Bytes(_) => "Bytes",
    }
}

/// Shared element-inspecting loop behind all three `value_list_to_*_list`
/// narrowing fns. An empty list narrows unconditionally and vacuously: the
/// target type comes from which of the three callers you reached, not from
/// inspecting contents, so there is no element-type ambiguity for an empty
/// list to get wrong.
#[track_caller]
fn narrow_value_list(fn_name: &str, expected: &str, list: Value, is_expected: fn(&Value) -> bool) -> Value {
    match list {
        Value::List(items) => {
            for (idx, item) in items.iter().enumerate() {
                if !is_expected(item) {
                    panic!(
                        "{fn_name}: element {idx} is {actual}, expected {expected}",
                        actual = value_tag_name(item)
                    );
                }
            }
            Value::List(items)
        }
        other => panic!("{fn_name}: expected ValueList, got {actual}", actual = value_tag_name(&other)),
    }
}

#[track_caller]
pub fn value_list_to_int_list(list: Value) -> Value {
    narrow_value_list("value_list_to_int_list", "Int", list, |v| matches!(v, Value::Int(_)))
}

#[track_caller]
pub fn value_list_to_text_list(list: Value) -> Value {
    narrow_value_list("value_list_to_text_list", "Text", list, |v| matches!(v, Value::Str(_)))
}

#[track_caller]
pub fn value_list_to_bool_list(list: Value) -> Value {
    narrow_value_list("value_list_to_bool_list", "Bool", list, |v| matches!(v, Value::Bool(_)))
}

/// Scalar sibling of `narrow_value_list`: checks a single `Value`'s tag
/// instead of walking a list's elements.
#[track_caller]
fn narrow_value(fn_name: &str, expected: &str, v: Value, is_expected: fn(&Value) -> bool) -> Value {
    if is_expected(&v) {
        v
    } else {
        panic!("{fn_name}: expected {expected}, got {actual}", actual = value_tag_name(&v));
    }
}

#[track_caller]
pub fn value_to_int(v: Value) -> Value {
    narrow_value("value_to_int", "Int", v, |v| matches!(v, Value::Int(_)))
}

#[track_caller]
pub fn value_to_text(v: Value) -> Value {
    narrow_value("value_to_text", "Text", v, |v| matches!(v, Value::Str(_)))
}

#[track_caller]
pub fn value_to_bool(v: Value) -> Value {
    narrow_value("value_to_bool", "Bool", v, |v| matches!(v, Value::Bool(_)))
}

#[track_caller]
pub fn list_nil(_: Value) -> Value {
    Value::List(vec![])
}

/// Build an M1 ValueList from its elements. Lowering target of
/// `ValueList(T)(a, b, ...)`. Variadic, same calling convention as value_make.
#[track_caller]
pub fn list_make(args: Value) -> Value {
    Value::List(super::tuple::fields_from_variadic(args))
}

#[track_caller]
pub fn list_cons(elem: Value, tail: Value) -> Value {
    match tail {
        Value::List(tail) => {
            let mut v = vec![elem];
            v.extend(tail);
            Value::List(v)
        }
        _ => Value::List(vec![elem]),
    }
}

#[track_caller]
pub fn list_len(list: Value) -> Value {
    match list {
        Value::List(es) => Value::Int(es.len() as i64),
        _ => panic!("list_len: expected List"),
    }
}

#[track_caller]
pub fn list_get(list: Value, idx: i64) -> Value {
    match list {
        Value::List(elems) => elems[idx as usize].clone(),
        _ => panic!("list_get: expected List"),
    }
}

#[track_caller]
pub fn list_get_at(list: Value, idx: i64) -> Value {
    if idx < 0 { return super::option::option_none(); }
    match list {
        Value::List(elems) => match elems.get(idx as usize) {
            Some(v) => super::option::option_some(v.clone()),
            None    => super::option::option_none(),
        },
        _ => panic!("list_get_at: expected List"),
    }
}

#[track_caller]
pub fn list_append(list: Value, elem: Value) -> Value {
    match list {
        // `list` already arrived as an owned clone (native call site's
        // `.clone()` accessor) — push in place, no second clone needed
        // (the old boxed path cloned once to unwrap the Tuple arg, then
        // cloned `elems` again here; this is strictly cheaper, not just
        // relocated).
        Value::List(mut elems) => {
            elems.push(elem);
            Value::List(elems)
        }
        _ => panic!("list_append: expected List as first element"),
    }
}

#[track_caller]
pub fn list_concat(a: Value, b: Value) -> Value {
    match (a, b) {
        (Value::List(mut a), Value::List(b)) => {
            a.extend(b);
            Value::List(a)
        }
        _ => panic!("list_concat: expected two Lists"),
    }
}

#[track_caller]
pub fn list_reverse(list: Value) -> Value {
    match list {
        Value::List(mut es) => { es.reverse(); Value::List(es) }
        _ => panic!("list_reverse: expected List"),
    }
}

#[track_caller]
pub fn list_head(list: Value) -> Value {
    match list {
        Value::List(es) if !es.is_empty() => es[0].clone(),
        Value::List(_) => panic!("list_head: called on empty list"),
        _ => panic!("list_head: expected List"),
    }
}

#[track_caller]
pub fn list_last(list: Value) -> Value {
    match list {
        Value::List(es) if !es.is_empty() => es[es.len() - 1].clone(),
        Value::List(_) => panic!("list_last: called on empty list"),
        _ => panic!("list_last: expected List"),
    }
}

#[track_caller]
pub fn list_tail(list: Value) -> Value {
    match list {
        Value::List(es) if !es.is_empty() => Value::List(es[1..].to_vec()),
        Value::List(_) => panic!("list_tail: called on empty list"),
        _ => panic!("list_tail: expected List"),
    }
}

#[track_caller]
pub fn list_is_empty(list: Value) -> Value {
    match list {
        Value::List(es) => Value::Bool(es.is_empty()),
        _ => panic!("list_is_empty: expected List"),
    }
}

#[track_caller]
pub fn list_of_1(v: Value) -> Value {
    Value::List(vec![v])
}

#[track_caller]
pub fn list_of_2(a: Value, b: Value) -> Value {
    Value::List(vec![a, b])
}

#[track_caller]
pub fn list_of_3(a: Value, b: Value, c: Value) -> Value {
    Value::List(vec![a, b, c])
}

/// Returns 1 if list[index] exists and str_len(list[index]) ≤ max_len, else 0. OOB-safe.
#[track_caller]
pub fn list_str_len_lte_if_some(list: Value, idx: i64, max_len: i64) -> Value {
    if idx < 0 { return Value::Int(0); }
    match list {
        Value::List(elems) => match elems.get(idx as usize) {
            Some(Value::Str(s)) => {
                let len = get_str(s).chars().count() as i64;
                Value::Int(if len <= max_len { 1 } else { 0 })
            }
            Some(_) => panic!("list_str_len_lte_if_some: list element is not Str"),
            None    => Value::Int(0),
        },
        _ => panic!("list_str_len_lte_if_some: expected List"),
    }
}

/// Get list[i] and println the value if it exists; return Unit either way.
/// Used by the unrolled forEach loop in 0.5 bundles where CIf branches are
/// evaluated eagerly — inlining the None check into Rust avoids option_unwrap(None).
#[track_caller]
pub fn list_get_println_if_some(list: Value, idx: i64) -> Value {
    if idx < 0 { return Value::Unit; }
    match list {
        Value::List(elems) => match elems.get(idx as usize) {
            Some(v) => super::io::io_println(v.clone()),
            None    => Value::Unit,
        },
        _ => panic!("list_get_println_if_some: expected List"),
    }
}

// ── B06-A: search / order / aggregate ───────────────────────────────────────
//
// Element-agnostic ValueList ops, same class as everything above: a match on
// the List tag, never a new representation. `list_contains`/`list_index_of`
// compare with `value_eq` (arith.rs) so list equality never drifts from
// scalar equality. `list_sort`/`list_min`/`list_max` share one total order
// over the three scalar Value variants (Int by value, Text bytewise, Bool
// false<true) — `scalar_cmp` panics by design on a mixed-variant or
// non-scalar (Unit/Tuple/List/Ctor/Dec/Float/Bytes) comparison, naming both
// offending tags, the same "index instead of a bare complaint" discipline as
// `narrow_value_list` above.

#[track_caller]
pub fn list_contains(list: Value, needle: Value) -> Value {
    match list {
        Value::List(elems) => Value::Bool(
            elems.into_iter().any(|e| matches!(value_eq(e, needle.clone()), Value::Bool(true))),
        ),
        other => panic!("list_contains: expected ValueList, got {actual}", actual = value_tag_name(&other)),
    }
}

#[track_caller]
pub fn list_index_of(list: Value, needle: Value) -> Value {
    match list {
        Value::List(elems) => {
            for (idx, e) in elems.into_iter().enumerate() {
                if matches!(value_eq(e, needle.clone()), Value::Bool(true)) {
                    return Value::Int(idx as i64);
                }
            }
            Value::Int(-1)
        }
        other => panic!("list_index_of: expected ValueList, got {actual}", actual = value_tag_name(&other)),
    }
}

/// Total order over the three scalar `Value` variants. Panics naming both
/// tags on a mixed-variant or non-scalar comparison — there is no ordering
/// across variants for this fn to guess at.
fn scalar_cmp(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Str(x), Value::Str(y)) => x.as_bytes().cmp(y.as_bytes()),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        _ => panic!(
            "list_sort: cannot compare {a} and {b} — expected matching Int, Text or Bool elements",
            a = value_tag_name(a), b = value_tag_name(b),
        ),
    }
}

#[track_caller]
pub fn list_sort(list: Value) -> Value {
    match list {
        Value::List(mut elems) => {
            elems.sort_by(scalar_cmp);
            Value::List(elems)
        }
        other => panic!("list_sort: expected ValueList, got {actual}", actual = value_tag_name(&other)),
    }
}

#[track_caller]
pub fn list_min(list: Value) -> Value {
    match list {
        Value::List(elems) => {
            let mut iter = elems.into_iter();
            let mut best = match iter.next() {
                Some(v) => v,
                None => panic!("list_min: called on empty list"),
            };
            for e in iter {
                if scalar_cmp(&e, &best) == std::cmp::Ordering::Less {
                    best = e;
                }
            }
            best
        }
        other => panic!("list_min: expected ValueList, got {actual}", actual = value_tag_name(&other)),
    }
}

#[track_caller]
pub fn list_max(list: Value) -> Value {
    match list {
        Value::List(elems) => {
            let mut iter = elems.into_iter();
            let mut best = match iter.next() {
                Some(v) => v,
                None => panic!("list_max: called on empty list"),
            };
            for e in iter {
                if scalar_cmp(&e, &best) == std::cmp::Ordering::Greater {
                    best = e;
                }
            }
            best
        }
        other => panic!("list_max: expected ValueList, got {actual}", actual = value_tag_name(&other)),
    }
}

#[track_caller]
pub fn list_sum(list: Value) -> Value {
    match list {
        Value::List(elems) => {
            let mut total: i64 = 0;
            for (idx, e) in elems.iter().enumerate() {
                match e {
                    Value::Int(n) => {
                        total = total
                            .checked_add(*n)
                            .unwrap_or_else(|| panic!("list_sum: overflow at element {idx}"));
                    }
                    other => panic!(
                        "list_sum: element {idx} is {actual}, expected Int",
                        actual = value_tag_name(other)
                    ),
                }
            }
            Value::Int(total)
        }
        other => panic!("list_sum: expected ValueList, got {actual}", actual = value_tag_name(&other)),
    }
}

#[track_caller]
pub fn list_all_true(list: Value) -> Value {
    match list {
        Value::List(elems) => {
            for (idx, e) in elems.iter().enumerate() {
                match e {
                    Value::Bool(b) => {
                        if !b {
                            return Value::Bool(false);
                        }
                    }
                    other => panic!(
                        "list_all_true: element {idx} is {actual}, expected Bool",
                        actual = value_tag_name(other)
                    ),
                }
            }
            Value::Bool(true)
        }
        other => panic!("list_all_true: expected ValueList, got {actual}", actual = value_tag_name(&other)),
    }
}

#[track_caller]
pub fn list_any_true(list: Value) -> Value {
    match list {
        Value::List(elems) => {
            for (idx, e) in elems.iter().enumerate() {
                match e {
                    Value::Bool(b) => {
                        if *b {
                            return Value::Bool(true);
                        }
                    }
                    other => panic!(
                        "list_any_true: element {idx} is {actual}, expected Bool",
                        actual = value_tag_name(other)
                    ),
                }
            }
            Value::Bool(false)
        }
        other => panic!("list_any_true: expected ValueList, got {actual}", actual = value_tag_name(&other)),
    }
}

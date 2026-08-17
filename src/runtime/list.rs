use super::value::{Value, get_str};

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

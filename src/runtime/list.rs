use super::value::Value;

pub fn list_nil(_: Value) -> Value {
    Value::List(vec![])
}

pub fn list_cons(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match &es[1] {
            Value::List(tail) => {
                let mut v = vec![es[0].clone()];
                v.extend(tail.clone());
                Value::List(v)
            }
            _ => Value::List(vec![es[0].clone()]),
        },
        _ => panic!("list_cons: expected Tuple(elem, List)"),
    }
}

pub fn list_len(list: Value) -> Value {
    match list {
        Value::List(es) => Value::Int(es.len() as i64),
        _ => panic!("list_len: expected List"),
    }
}

pub fn list_get(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => {
            let idx = match &es[1] { Value::Int(n) => *n as usize, _ => panic!("list_get: expected Int index") };
            match &es[0] {
                Value::List(elems) => elems[idx].clone(),
                _ => panic!("list_get: expected List"),
            }
        }
        _ => panic!("list_get: expected Tuple(List, Int)"),
    }
}

pub fn list_get_at(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => {
            let idx = match &es[1] { Value::Int(n) => *n, _ => panic!("list_get_at: expected Int index") };
            if idx < 0 { return super::option::option_none(); }
            match &es[0] {
                Value::List(elems) => match elems.get(idx as usize) {
                    Some(v) => super::option::option_some(v.clone()),
                    None    => super::option::option_none(),
                },
                _ => panic!("list_get_at: expected List"),
            }
        }
        _ => panic!("list_get_at: expected Tuple(List, Int)"),
    }
}

pub fn list_append(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match &es[0] {
            Value::List(elems) => {
                let mut v = elems.clone();
                v.push(es[1].clone());
                Value::List(v)
            }
            _ => panic!("list_append: expected List as first element"),
        },
        _ => panic!("list_append: expected Tuple(List, elem)"),
    }
}

pub fn list_concat(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::List(a), Value::List(b)) => {
                let mut v = a.clone();
                v.extend(b.clone());
                Value::List(v)
            }
            _ => panic!("list_concat: expected two Lists"),
        },
        _ => panic!("list_concat: expected Tuple(List, List)"),
    }
}

pub fn list_reverse(list: Value) -> Value {
    match list {
        Value::List(mut es) => { es.reverse(); Value::List(es) }
        _ => panic!("list_reverse: expected List"),
    }
}

pub fn list_head(list: Value) -> Value {
    match list {
        Value::List(es) if !es.is_empty() => es[0].clone(),
        Value::List(_) => panic!("list_head: called on empty list"),
        _ => panic!("list_head: expected List"),
    }
}

pub fn list_tail(list: Value) -> Value {
    match list {
        Value::List(es) if !es.is_empty() => Value::List(es[1..].to_vec()),
        Value::List(_) => panic!("list_tail: called on empty list"),
        _ => panic!("list_tail: expected List"),
    }
}

pub fn list_is_empty(list: Value) -> Value {
    match list {
        Value::List(es) => Value::Bool(es.is_empty()),
        _ => panic!("list_is_empty: expected List"),
    }
}

pub fn list_of_1(v: Value) -> Value {
    Value::List(vec![v])
}

pub fn list_of_2(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => Value::List(vec![es[0].clone(), es[1].clone()]),
        _ => panic!("list_of_2: expected Tuple(a, b)"),
    }
}

pub fn list_of_3(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 3 => Value::List(vec![es[0].clone(), es[1].clone(), es[2].clone()]),
        _ => panic!("list_of_3: expected Tuple(a, b, c)"),
    }
}

/// Get list[i] and println the value if it exists; return Unit either way.
/// Used by the unrolled forEach loop in 0.5 bundles where CIf branches are
/// evaluated eagerly — inlining the None check into Rust avoids option_unwrap(None).
pub fn list_get_println_if_some(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => {
            let idx = match &es[1] { Value::Int(n) => *n, _ => panic!("list_get_println_if_some: expected Int index") };
            if idx < 0 { return Value::Unit; }
            match &es[0] {
                Value::List(elems) => match elems.get(idx as usize) {
                    Some(v) => super::io::io_println(v.clone()),
                    None    => Value::Unit,
                },
                _ => panic!("list_get_println_if_some: expected List"),
            }
        }
        _ => panic!("list_get_println_if_some: expected Tuple(List, Int)"),
    }
}

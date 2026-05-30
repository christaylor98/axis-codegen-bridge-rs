use super::value::Value;

pub fn list_nil() -> Value {
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

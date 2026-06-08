use super::value::{Value, truthy};

#[track_caller]
pub fn bool_and(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => Value::Bool(truthy(&es[0]) && truthy(&es[1])),
        _ => panic!("bool_and: expected Tuple with 2 elements"),
    }
}

#[track_caller]
pub fn bool_or(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => Value::Bool(truthy(&es[0]) || truthy(&es[1])),
        _ => panic!("bool_or: expected Tuple with 2 elements"),
    }
}

#[track_caller]
pub fn bool_not(v: Value) -> Value {
    Value::Bool(!truthy(&v))
}

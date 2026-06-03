use super::value::{Value, get_tag_name};

pub fn tuple_field(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => {
            let idx = match &es[1] { Value::Int(n) => *n as usize, _ => return Value::Unit };
            match &es[0] {
                Value::Tuple(fields) => fields.get(idx).cloned().unwrap_or(Value::Unit),
                Value::List(items) => items.get(idx).cloned().unwrap_or(Value::Unit),
                _ => Value::Unit,
            }
        }
        _ => Value::Unit,
    }
}

pub fn ctor_field(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => {
            let idx = match &es[1] { Value::Int(n) => *n as usize, _ => return Value::Unit };
            match &es[0] {
                Value::Ctor { fields, .. } => fields.get(idx).cloned().unwrap_or(Value::Unit),
                _ => Value::Unit,
            }
        }
        _ => Value::Unit,
    }
}

pub fn ctor_is_ok(v: Value) -> Value {
    match v {
        Value::Ctor { tag, .. } if get_tag_name(tag) == "Ok" => Value::Bool(true),
        _ => Value::Bool(false),
    }
}

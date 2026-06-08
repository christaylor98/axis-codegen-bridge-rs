use super::value::{Value, get_tag_name, get_str};

#[track_caller]
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

#[track_caller]
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

#[track_caller]
pub fn ctor_is_ok(v: Value) -> Value {
    match v {
        Value::Ctor { tag, .. } if get_tag_name(tag) == "Ok" => Value::Bool(true),
        _ => Value::Bool(false),
    }
}

/// Unwrap a Result(Text) (as produced by fs_read_text): returns the Ok payload,
/// panics on Err with the message. Monomorphic over Text — mirrors option_unwrap.
#[track_caller]
pub fn result_text_unwrap(v: Value) -> Value {
    match v {
        Value::Ctor { tag, fields } if get_tag_name(tag) == "Ok" => {
            fields.into_iter().next().unwrap_or(Value::Unit)
        }
        Value::Ctor { tag, fields } if get_tag_name(tag) == "Err" => {
            let msg = match fields.into_iter().next() {
                Some(Value::Str(h)) => get_str(h),
                _ => "unknown error".to_string(),
            };
            panic!("result_text_unwrap: Err({})", msg)
        }
        _ => panic!("result_text_unwrap: expected Result Ctor (Ok/Err), got {:?}", v),
    }
}

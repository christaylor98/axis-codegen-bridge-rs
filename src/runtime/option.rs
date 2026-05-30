use super::value::{Value, intern_tag, get_tag_name};

pub fn option_none() -> Value {
    Value::Ctor { tag: intern_tag("None"), fields: vec![] }
}

pub fn option_some(v: Value) -> Value {
    Value::Ctor { tag: intern_tag("Some"), fields: vec![v] }
}

pub fn option_is_none(opt: Value) -> Value {
    match opt {
        Value::Ctor { tag, ref fields } if get_tag_name(tag) == "None" && fields.is_empty() => Value::Bool(true),
        _ => Value::Bool(false),
    }
}

pub fn option_is_some(opt: Value) -> Value {
    match opt {
        Value::Ctor { tag, ref fields } if get_tag_name(tag) == "Some" && fields.len() == 1 => Value::Bool(true),
        _ => Value::Bool(false),
    }
}

/// Unary wrapper for option_none — takes any Value and returns None.
/// Used in dispatch tables that require fn(Value) -> Value.
pub fn option_none_fn(_: Value) -> Value { option_none() }

pub fn option_unwrap(opt: Value) -> Value {
    match opt {
        Value::Ctor { tag, fields } if get_tag_name(tag) == "Some" && fields.len() == 1 => fields.into_iter().next().unwrap(),
        Value::Ctor { tag, .. }    if get_tag_name(tag) == "None" => panic!("option_unwrap: called on None"),
        _ => panic!("option_unwrap: not an option value"),
    }
}

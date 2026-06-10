use super::value::{Value, get_tag_name, get_str, intern_tag};

/// Recover a variadic field list from the bridge calling convention:
/// 0 args -> Unit -> [], 1 arg -> bare value -> [v], N args -> Tuple(xs) -> xs.
/// Shared by the M1 compound constructors (value_make / list_make).
pub(crate) fn fields_from_variadic(args: Value) -> Vec<Value> {
    match args {
        Value::Unit => vec![],
        Value::Tuple(es) => es,
        other => vec![other],
    }
}

/// Build an M1 compound Value from its fields. Lowering target of
/// `Value(T..)(a, b, ...)`. Variadic. The result is a Ctor tagged "Value" so a
/// single-field value is never confused with the N-arg Tuple wrapper (this is
/// what makes the nested `list_make(value_make(a, b))` case unambiguous).
#[track_caller]
pub fn value_make(args: Value) -> Value {
    Value::Ctor { tag: intern_tag("Value"), fields: fields_from_variadic(args) }
}

/// Field accessors for an M1 compound Value (value_0/1/2). Read the Nth field
/// of a Ctor; tolerate Tuple/List shapes defensively.
fn value_field(v: Value, idx: usize) -> Value {
    match v {
        Value::Ctor { fields, .. } => fields.get(idx).cloned().unwrap_or(Value::Unit),
        Value::Tuple(es) => es.get(idx).cloned().unwrap_or(Value::Unit),
        Value::List(es) => es.get(idx).cloned().unwrap_or(Value::Unit),
        _ => Value::Unit,
    }
}

#[track_caller]
pub fn value_0(v: Value) -> Value { value_field(v, 0) }
#[track_caller]
pub fn value_1(v: Value) -> Value { value_field(v, 1) }
#[track_caller]
pub fn value_2(v: Value) -> Value { value_field(v, 2) }

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

use super::value::{Value, intern_tag};

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
            let idx = match &es[1] {
                Value::Int(n) => *n as usize,
                other => panic!("tuple_field: arg 1 expected Int, got {:?}", other),
            };
            match &es[0] {
                Value::Tuple(fields) => fields.get(idx).cloned().unwrap_or(Value::Unit),
                Value::List(items) => items.get(idx).cloned().unwrap_or(Value::Unit),
                other => panic!(
                    "tuple_field: expected Tuple or List, got {:?} (use ctor_field for a Value(..)(..)-constructed Ctor)",
                    other
                ),
            }
        }
        other => panic!("tuple_field: expected Tuple(Value, Int), got {:?}", other),
    }
}

#[track_caller]
pub fn ctor_field(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => {
            let idx = match &es[1] {
                Value::Int(n) => *n as usize,
                other => panic!("ctor_field: arg 1 expected Int, got {:?}", other),
            };
            match &es[0] {
                Value::Ctor { fields, .. } => fields.get(idx).cloned().unwrap_or(Value::Unit),
                other => panic!(
                    "ctor_field: expected Ctor, got {:?} (use tuple_field for a raw Tuple/List)",
                    other
                ),
            }
        }
        other => panic!("ctor_field: expected Tuple(Value, Int), got {:?}", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctor_field_reads_value_make_output() {
        let v = value_make(Value::Tuple(vec![Value::Int(0), Value::Int(0), Value::Int(9)]));
        assert_eq!(ctor_field(Value::Tuple(vec![v.clone(), Value::Int(0)])), Value::Int(0));
        assert_eq!(ctor_field(Value::Tuple(vec![v, Value::Int(2)])), Value::Int(9));
    }

    #[test]
    #[should_panic(expected = "tuple_field: expected Tuple or List, got Ctor")]
    fn tuple_field_panics_on_ctor() {
        let v = value_make(Value::Tuple(vec![Value::Int(0), Value::Int(0), Value::Int(9)]));
        tuple_field(Value::Tuple(vec![v, Value::Int(0)]));
    }

    #[test]
    #[should_panic(expected = "ctor_field: expected Ctor, got Tuple")]
    fn ctor_field_panics_on_tuple() {
        let t = Value::Tuple(vec![Value::Int(1), Value::Int(2)]);
        ctor_field(Value::Tuple(vec![t, Value::Int(0)]));
    }

    #[test]
    fn tuple_field_still_reads_tuple_and_list() {
        let t = Value::Tuple(vec![Value::Int(1), Value::Int(2)]);
        assert_eq!(tuple_field(Value::Tuple(vec![t, Value::Int(1)])), Value::Int(2));
        let l = Value::List(vec![Value::Int(5), Value::Int(6)]);
        assert_eq!(tuple_field(Value::Tuple(vec![l, Value::Int(0)])), Value::Int(5));
    }
}


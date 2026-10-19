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
/// FAULT_AS_UNKNOWN step 3: a missing field is not Unit -- it fails loudly (an Unknown under the guard).
#[track_caller]
fn value_field(v: Value, idx: usize) -> Value {
    let n = match &v {
        Value::Ctor { fields, .. } => fields.len(),
        Value::Tuple(es) => es.len(),
        Value::List(es) => es.len(),
        other => panic!("value_{}: not a compound value: {:?}", idx, other),
    };
    match v {
        Value::Ctor { fields, .. } => fields.into_iter().nth(idx),
        Value::Tuple(es) => es.into_iter().nth(idx),
        Value::List(es) => es.into_iter().nth(idx),
        _ => None,
    }.unwrap_or_else(|| panic!("value_{}: index out of range (the value has {} fields)", idx, n))
}

#[track_caller]
pub fn value_0(v: Value) -> Value { value_field(v, 0) }
#[track_caller]
pub fn value_1(v: Value) -> Value { value_field(v, 1) }
#[track_caller]
pub fn value_2(v: Value) -> Value { value_field(v, 2) }

#[track_caller]
pub fn tuple_field(bundle: Value, idx: i64) -> Value {
    let oob = |n: usize| -> ! { panic!("tuple_field: index {} out of range (length {})", idx, n) };
    let i = usize::try_from(idx).ok();
    match bundle {
        Value::Tuple(mut fields) => { let n = fields.len(); match i.filter(|i| *i < n) { Some(i) => fields.swap_remove(i), None => oob(n) } }
        Value::List(mut items) => { let n = items.len(); match i.filter(|i| *i < n) { Some(i) => items.swap_remove(i), None => oob(n) } }
        other => panic!(
            "tuple_field: expected Tuple or List, got {:?} (use ctor_field for a Value(..)(..)-constructed Ctor)",
            other
        ),
    }
}

#[track_caller]
pub fn ctor_field(bundle: Value, idx: i64) -> Value {
    match bundle {
        Value::Ctor { mut fields, .. } => {
            // the Ctor is owned here: move the field out, never clone it (a list in a loop's state is not copied)
            let n = fields.len();
            match usize::try_from(idx).ok().filter(|i| *i < n) {
                Some(i) => fields.swap_remove(i),
                None => panic!("ctor_field: index {} out of range ({} fields)", idx, n),
            }
        }
        other => panic!(
            "ctor_field: expected Ctor, got {:?} (use tuple_field for a raw Tuple/List)",
            other
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctor_field_reads_value_make_output() {
        let v = value_make(Value::Tuple(vec![Value::Int(0), Value::Int(0), Value::Int(9)]));
        assert_eq!(ctor_field(v.clone(), 0), Value::Int(0));
        assert_eq!(ctor_field(v, 2), Value::Int(9));
    }

    #[test]
    #[should_panic(expected = "tuple_field: expected Tuple or List, got Ctor")]
    fn tuple_field_panics_on_ctor() {
        let v = value_make(Value::Tuple(vec![Value::Int(0), Value::Int(0), Value::Int(9)]));
        tuple_field(v, 0);
    }

    #[test]
    #[should_panic(expected = "ctor_field: expected Ctor, got Tuple")]
    fn ctor_field_panics_on_tuple() {
        let t = Value::Tuple(vec![Value::Int(1), Value::Int(2)]);
        ctor_field(t, 0);
    }

    #[test]
    fn tuple_field_still_reads_tuple_and_list() {
        let t = Value::Tuple(vec![Value::Int(1), Value::Int(2)]);
        assert_eq!(tuple_field(t, 1), Value::Int(2));
        let l = Value::List(vec![Value::Int(5), Value::Int(6)].into());
        assert_eq!(tuple_field(l, 0), Value::Int(5));
    }
}


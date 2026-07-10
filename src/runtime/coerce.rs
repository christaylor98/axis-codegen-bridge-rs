// BRIDGE_VALUE_COERCION_V1 — value-coercion family.
//
// Eight leaves: six single-arm converters and two tag-dispatch dispatchers.
//
// Tag-order convention (POSITIONAL, binding): arg1 handles Int, arg2 Dec, arg3 Float.
// Dispatchers reach a non-numeric tag => panic (EXHAUSTIVE_OR_PANIC).
//
// Each converter is a HARD ERROR on the wrong input tag — the dispatcher is
// the only legitimate caller, and it has already pre-selected by tag.

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive as _;
use super::value::Value;

// ── Converters ────────────────────────────────────────────────────────────────

pub fn int_to_dec(v: Value) -> Value {
    match v {
        Value::Int(n) => Value::Dec(Decimal::from(n)),
        other => panic!("int_to_dec: expected Int, got {:?}", other),
    }
}

pub fn dec_id(v: Value) -> Value {
    match v {
        Value::Dec(_) => v,
        other => panic!("dec_id: expected Dec, got {:?}", other),
    }
}

pub fn float_to_dec(v: Value) -> Value {
    match v {
        Value::Float(f) => match Decimal::from_f64_retain(f) {
            Some(d) => Value::Dec(d),
            None => panic!(
                "float_to_dec: f64 {} is not representable as Decimal (NaN, infinity, or out of range)",
                f
            ),
        },
        other => panic!("float_to_dec: expected Float, got {:?}", other),
    }
}

pub fn int_to_float(v: Value) -> Value {
    match v {
        Value::Int(n) => Value::Float(n as f64),
        other => panic!("int_to_float: expected Int, got {:?}", other),
    }
}

pub fn dec_to_float(v: Value) -> Value {
    match v {
        Value::Dec(d) => match d.to_f64() {
            Some(f) => Value::Float(f),
            None => panic!("dec_to_float: Decimal {} not representable as f64", d),
        },
        other => panic!("dec_to_float: expected Dec, got {:?}", other),
    }
}

pub fn float_id(v: Value) -> Value {
    match v {
        Value::Float(_) => v,
        other => panic!("float_id: expected Float, got {:?}", other),
    }
}

// ── Dispatchers ───────────────────────────────────────────────────────────────
//
// HOF leaves with three FnRef slots — declared in
// `src/emit/rust_05.rs::fn_arg_kinds()` as `[Data, FnRef, FnRef, FnRef]`.
// The emitter substitutes the three converter paths into a native multi-arg
// Rust call at codegen time. There is no Fn value at runtime, consistent with
// FN_REF_IS_NOT_A_VALUE / FN_REF_IS_CALLEE_ONLY.

pub fn bridge_to_dec(
    v: Value,
    f_int: fn(Value) -> Value,
    f_dec: fn(Value) -> Value,
    f_float: fn(Value) -> Value,
) -> Value {
    match &v {
        Value::Int(_)   => f_int(v),
        Value::Dec(_)   => f_dec(v),
        Value::Float(_) => f_float(v),
        other => panic!("bridge_to_dec: non-numeric Value tag: {:?}", other),
    }
}

pub fn bridge_to_float(
    v: Value,
    f_int: fn(Value) -> Value,
    f_dec: fn(Value) -> Value,
    f_float: fn(Value) -> Value,
) -> Value {
    match &v {
        Value::Int(_)   => f_int(v),
        Value::Dec(_)   => f_dec(v),
        Value::Float(_) => f_float(v),
        other => panic!("bridge_to_float: non-numeric Value tag: {:?}", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn int_to_dec_basic() {
        assert_eq!(int_to_dec(Value::Int(42)), Value::Dec(Decimal::from(42)));
        assert_eq!(int_to_dec(Value::Int(-7)), Value::Dec(Decimal::from(-7)));
    }

    #[test]
    fn dec_id_passthrough() {
        let d = Decimal::from_str("3.14159265358979").unwrap();
        assert_eq!(dec_id(Value::Dec(d)), Value::Dec(d));
    }

    #[test]
    fn float_to_dec_roundtrip_decimal_repr() {
        // 0.1f64 has no exact finite decimal that fits in i64; but Decimal
        // can hold a high-precision approximation that round-trips visually.
        let f = 0.1f64;
        let d = match float_to_dec(Value::Float(f)) {
            Value::Dec(d) => d,
            other => panic!("expected Dec, got {:?}", other),
        };
        // The dec value's f64 conversion should be exactly f again — within
        // f64 precision (the Decimal carries more digits than f64 has).
        assert_eq!(d.to_f64().unwrap(), f);
    }

    #[test]
    fn int_to_float_basic() {
        assert_eq!(int_to_float(Value::Int(5)), Value::Float(5.0));
        assert_eq!(int_to_float(Value::Int(-3)), Value::Float(-3.0));
    }

    #[test]
    fn dec_to_float_basic() {
        let d = Decimal::from_str("2.5").unwrap();
        assert_eq!(dec_to_float(Value::Dec(d)), Value::Float(2.5));
    }

    #[test]
    fn float_id_passthrough() {
        assert_eq!(float_id(Value::Float(1.5)), Value::Float(1.5));
    }

    #[test]
    fn bridge_to_dec_dispatches_by_tag() {
        // Int arm
        assert_eq!(
            bridge_to_dec(Value::Int(7), int_to_dec, dec_id, float_to_dec),
            Value::Dec(Decimal::from(7))
        );
        // Dec arm (identity)
        let d = Decimal::from_str("1.25").unwrap();
        assert_eq!(
            bridge_to_dec(Value::Dec(d), int_to_dec, dec_id, float_to_dec),
            Value::Dec(d)
        );
        // Float arm
        let from_float = bridge_to_dec(Value::Float(2.5), int_to_dec, dec_id, float_to_dec);
        match from_float {
            Value::Dec(got) => assert_eq!(got.to_f64().unwrap(), 2.5),
            other => panic!("expected Dec, got {:?}", other),
        }
    }

    #[test]
    fn bridge_to_float_dispatches_by_tag() {
        assert_eq!(
            bridge_to_float(Value::Int(4), int_to_float, dec_to_float, float_id),
            Value::Float(4.0)
        );
        let d = Decimal::from_str("0.5").unwrap();
        assert_eq!(
            bridge_to_float(Value::Dec(d), int_to_float, dec_to_float, float_id),
            Value::Float(0.5)
        );
        assert_eq!(
            bridge_to_float(Value::Float(9.0), int_to_float, dec_to_float, float_id),
            Value::Float(9.0)
        );
    }

    #[test]
    #[should_panic(expected = "non-numeric Value tag")]
    fn bridge_to_dec_panics_on_string() {
        let _ = bridge_to_dec(
            Value::Str("".into()),
            int_to_dec,
            dec_id,
            float_to_dec,
        );
    }

    #[test]
    #[should_panic(expected = "non-numeric Value tag")]
    fn bridge_to_float_panics_on_unit() {
        let _ = bridge_to_float(
            Value::Unit,
            int_to_float,
            dec_to_float,
            float_id,
        );
    }
}

use super::value::{Value, truthy};

/// assert(Bool) → Unit: panics if condition is false, returns Unit if true.
/// Identity: sha256("assert") = 0x25450689…edae0a (BRIDGE_TESTKIT_FINALIZE_V1).
#[track_caller]
pub fn ax_assert(v: Value) -> Value {
    if !truthy(&v) {
        panic!("assertion failed");
    }
    Value::Unit
}

#[track_caller]
pub fn bool_and(a: Value, b: Value) -> Value {
    Value::Bool(truthy(&a) && truthy(&b))
}

#[track_caller]
pub fn bool_or(a: Value, b: Value) -> Value {
    Value::Bool(truthy(&a) || truthy(&b))
}

#[track_caller]
pub fn bool_not(v: Value) -> Value {
    Value::Bool(!truthy(&v))
}

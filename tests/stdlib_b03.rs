//! RETURNED TO THE BRIDGE 2026-09-16: Chris ruled the float primitives stay
//! in the bridge, so their tests come back with them.
//!
//! stdlib(B03-T): float arithmetic (AXIS_STDLIB_DESIGN_V1).
//!
//! Written from the B03 spec table BEFORE reading the A-stage implementation
//! (T-stage rule: expectations come from the spec, not the code). Each fn is
//! called directly through `axis_codegen_bridge::runtime::…`; every panic
//! condition in the spec's semantics column gets a `#[should_panic]` test.
//!
//! Rust call paths assumed here follow the established B01/B02 sibling
//! conventions in `src/runtime/arith.rs` (dec_* family) and
//! `src/runtime/coerce.rs` (int_to_float already lands `Value::Float`):
//!   - binary Float fns (float_eq, float_lt, dec_add, ...) take a boxed
//!     `Value::Tuple(vec![a, b])`: `float_add`, `float_sub`, `float_mul`,
//!     `float_div`, `float_pow`, `float_min`, `float_max`.
//!   - unary Float fns (dec_neg, dec_abs, dec_to_int, dec_to_text) take a
//!     bare `Value::Float(...)`: `float_neg`, `float_abs`, `float_sqrt`,
//!     `float_floor`, `float_ceil`, `float_round`, `float_is_nan`,
//!     `float_to_int`, `float_to_text`.
//!   - `str_to_float` mirrors `str_to_dec`'s native `Arc<str>` param.
//! If A's landed signatures differ, this file is adjusted to match before
//! being run — see the T-stage report's DISCREPANCIES section for any case
//! where the assumption above did not hold.

use axis_codegen_bridge::runtime::arith;
use axis_codegen_bridge::runtime::value::{intern_str, Value};

fn f(x: f64) -> Value {
    Value::Float(x)
}

fn t2(a: Value, b: Value) -> Value {
    Value::Tuple(vec![a, b])
}

fn as_float(v: Value) -> f64 {
    match v {
        Value::Float(x) => x,
        other => panic!("expected Float, got {:?}", other),
    }
}

// ── float_add / float_sub / float_mul ──────────────────────────────────────

#[test]
fn float_add_basic() {
    assert_eq!(arith::float_add(t2(f(2.5), f(1.5))), f(4.0));
    assert_eq!(arith::float_add(t2(f(-1.0), f(1.0))), f(0.0));
}

#[test]
fn float_sub_basic() {
    assert_eq!(arith::float_sub(t2(f(5.0), f(2.5))), f(2.5));
}

#[test]
fn float_mul_basic() {
    assert_eq!(arith::float_mul(t2(f(3.0), f(2.0))), f(6.0));
}

// ── float_div ────────────────────────────────────────────────────────────

#[test]
fn float_div_basic() {
    assert_eq!(arith::float_div(t2(f(6.0), f(3.0))), f(2.0));
}

#[test]
fn float_div_by_zero_is_inf_not_panic() {
    assert_eq!(as_float(arith::float_div(t2(f(1.0), f(0.0)))), f64::INFINITY);
    assert_eq!(as_float(arith::float_div(t2(f(-1.0), f(0.0)))), f64::NEG_INFINITY);
}

#[test]
fn float_div_zero_by_zero_is_nan() {
    assert!(as_float(arith::float_div(t2(f(0.0), f(0.0)))).is_nan());
}

// ── float_neg / float_abs ───────────────────────────────────────────────

#[test]
fn float_neg_basic() {
    assert_eq!(arith::float_neg(f(3.5)), f(-3.5));
    assert_eq!(arith::float_neg(f(-3.5)), f(3.5));
}

#[test]
fn float_abs_basic() {
    assert_eq!(arith::float_abs(f(-3.5)), f(3.5));
    assert_eq!(arith::float_abs(f(3.5)), f(3.5));
}

// ── float_sqrt ───────────────────────────────────────────────────────────

#[test]
fn float_sqrt_basic() {
    assert_eq!(arith::float_sqrt(f(4.0)), f(2.0));
    assert_eq!(arith::float_sqrt(f(2.0)), f(std::f64::consts::SQRT_2));
}

#[test]
fn float_sqrt_negative_is_nan() {
    assert!(as_float(arith::float_sqrt(f(-1.0))).is_nan());
}

// ── float_pow ────────────────────────────────────────────────────────────

#[test]
fn float_pow_basic() {
    assert_eq!(arith::float_pow(t2(f(2.0), f(10.0))), f(1024.0));
    assert_eq!(arith::float_pow(t2(f(9.0), f(0.5))), f(3.0));
}

// ── float_floor / float_ceil / float_round ─────────────────────────────

#[test]
fn float_floor_basic() {
    assert_eq!(arith::float_floor(f(1.7)), f(1.0));
    assert_eq!(arith::float_floor(f(-1.2)), f(-2.0));
}

#[test]
fn float_ceil_basic() {
    assert_eq!(arith::float_ceil(f(1.2)), f(2.0));
    assert_eq!(arith::float_ceil(f(-1.7)), f(-1.0));
}

#[test]
fn float_round_half_away_from_zero() {
    assert_eq!(arith::float_round(f(2.5)), f(3.0));
    assert_eq!(arith::float_round(f(-2.5)), f(-3.0));
    assert_eq!(arith::float_round(f(2.4)), f(2.0));
}

// ── float_min / float_max ───────────────────────────────────────────────

#[test]
fn float_min_basic() {
    assert_eq!(arith::float_min(t2(f(1.5), f(2.25))), f(1.5));
    assert_eq!(arith::float_min(t2(f(2.25), f(1.5))), f(1.5));
}

#[test]
fn float_min_nan_ignoring() {
    assert_eq!(arith::float_min(t2(f(1.0), f(f64::NAN))), f(1.0));
    assert_eq!(arith::float_min(t2(f(f64::NAN), f(1.0))), f(1.0));
}

#[test]
fn float_max_basic() {
    assert_eq!(arith::float_max(t2(f(1.5), f(2.25))), f(2.25));
    assert_eq!(arith::float_max(t2(f(2.25), f(1.5))), f(2.25));
}

#[test]
fn float_max_nan_ignoring() {
    assert_eq!(arith::float_max(t2(f(1.0), f(f64::NAN))), f(1.0));
    assert_eq!(arith::float_max(t2(f(f64::NAN), f(1.0))), f(1.0));
}

// ── float_is_nan ─────────────────────────────────────────────────────────

#[test]
fn float_is_nan_basic() {
    assert_eq!(arith::float_is_nan(f(f64::NAN)), Value::Bool(true));
    assert_eq!(arith::float_is_nan(f(1.0)), Value::Bool(false));
}

// ── float_to_int ─────────────────────────────────────────────────────────

#[test]
fn float_to_int_truncates_toward_zero() {
    assert_eq!(arith::float_to_int(f(9.9)), Value::Int(9));
    assert_eq!(arith::float_to_int(f(-9.9)), Value::Int(-9));
    assert_eq!(arith::float_to_int(f(9.0)), Value::Int(9));
}

#[test]
#[should_panic]
fn float_to_int_nan_panics() {
    arith::float_to_int(f(f64::NAN));
}

#[test]
#[should_panic]
fn float_to_int_inf_panics() {
    arith::float_to_int(f(f64::INFINITY));
}

#[test]
#[should_panic]
fn float_to_int_out_of_range_panics() {
    arith::float_to_int(f(1e30));
}

// ── float_to_text ────────────────────────────────────────────────────────

#[test]
fn float_to_text_basic() {
    assert_eq!(arith::float_to_text(f(2.5)), Value::Str(intern_str("2.5")));
    assert_eq!(arith::float_to_text(f(7.0)), Value::Str(intern_str("7")));
    assert_eq!(arith::float_to_text(f(-7.0)), Value::Str(intern_str("-7")));
}

// ── str_to_float ─────────────────────────────────────────────────────────

#[test]
fn str_to_float_basic() {
    assert_eq!(as_float(arith::str_to_float(std::sync::Arc::from("2.5"))), 2.5);
    assert_eq!(as_float(arith::str_to_float(std::sync::Arc::from("-3"))), -3.0);
}

#[test]
#[should_panic]
fn str_to_float_invalid_input_panics() {
    arith::str_to_float(std::sync::Arc::from("not-a-number"));
}

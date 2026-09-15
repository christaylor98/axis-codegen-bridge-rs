//! RETURNED TO THE BRIDGE 2026-09-16: Chris ruled the dec primitives stay
//! in the bridge, so their tests come back with them. Split out of what was
//! briefly axis-stdlib-working/crate/tests/stdlib_b02.rs; the int half of
//! that file stayed behind, because five of those six fns are M1 now.
//!
//! stdlib(B02-T): int + dec arithmetic completions (AXIS_STDLIB_DESIGN_V1).
//!
//! Written from the B02 spec table BEFORE reading the A-stage implementation
//! (T-stage rule: expectations come from the spec, not the code). Each fn is
//! called directly through `axis_codegen_bridge::runtime::…`; every panic
//! condition in the spec's semantics column gets a `#[should_panic]` test.
//!
//! Rust call paths assumed here follow the established sibling conventions in
//! `src/runtime/arith.rs`:
//!   - single/multi native-Int fns (int_eq, int_abs, int_min, int_max, ...)
//!     take plain `i64` params: `int_neg`, `int_ne`, `int_pow`, `int_sign`,
//!     `int_is_even`, `int_is_odd`.
//!   - binary Dec fns (dec_eq, dec_div, dec_lt, ...) take `Value::Tuple(vec![a, b])`:
//!     `dec_add`, `dec_sub`, `dec_mul`, `dec_min`, `dec_max`.
//!   - unary Dec fns (dec_to_text) take a bare `Value::Dec(...)`: `dec_neg`,
//!     `dec_abs`, `dec_to_int`.
//!   - `dec_round` is mixed-type binary (Dec, Int) -> boxed `Value::Tuple`.
//!   - `str_to_dec` mirrors `str_to_int`'s native `Arc<str>` param.
//! If A's landed signatures differ, this file is adjusted to match before
//! being run — see the T-stage report's DISCREPANCIES section for any case
//! where the assumption above did not hold.

use axis_codegen_bridge::runtime::arith;
use axis_codegen_bridge::runtime::value::{Decimal, Value};
use std::str::FromStr;

fn dec(s: &str) -> Value {
    Value::Dec(Decimal::from_str(s).unwrap())
}

fn t2(a: Value, b: Value) -> Value {
    Value::Tuple(vec![a, b])
}

fn as_dec(v: Value) -> Decimal {
    match v {
        Value::Dec(d) => d,
        other => panic!("expected Dec, got {:?}", other),
    }
}

// ── dec_add / dec_sub / dec_mul ─────────────────────────────────────────

#[test]
fn dec_add_basic() {
    assert_eq!(arith::dec_add(t2(dec("7"), dec("2.5"))), dec("9.5"));
}

#[test]
#[should_panic]
fn dec_add_overflow_panics() {
    arith::dec_add(t2(Value::Dec(Decimal::MAX), dec("1")));
}

#[test]
fn dec_sub_basic() {
    assert_eq!(arith::dec_sub(t2(dec("7"), dec("2.5"))), dec("4.5"));
}

#[test]
#[should_panic]
fn dec_sub_overflow_panics() {
    arith::dec_sub(t2(Value::Dec(Decimal::MIN), dec("1")));
}

#[test]
fn dec_mul_basic() {
    assert_eq!(arith::dec_mul(t2(dec("7"), dec("2.5"))), dec("17.5"));
}

#[test]
#[should_panic]
fn dec_mul_overflow_panics() {
    arith::dec_mul(t2(Value::Dec(Decimal::MAX), dec("2")));
}

// ── dec_neg / dec_abs ───────────────────────────────────────────────────

#[test]
fn dec_neg_basic() {
    assert_eq!(arith::dec_neg(dec("2.5")), dec("-2.5"));
    assert_eq!(arith::dec_neg(dec("-2.5")), dec("2.5"));
}

#[test]
fn dec_abs_basic() {
    assert_eq!(arith::dec_abs(dec("-2.5")), dec("2.5"));
    assert_eq!(arith::dec_abs(dec("2.5")), dec("2.5"));
}

// ── dec_min / dec_max ───────────────────────────────────────────────────

#[test]
fn dec_min_basic() {
    assert_eq!(arith::dec_min(t2(dec("1.5"), dec("2.25"))), dec("1.5"));
    assert_eq!(arith::dec_min(t2(dec("2.25"), dec("1.5"))), dec("1.5"));
}

#[test]
fn dec_max_basic() {
    assert_eq!(arith::dec_max(t2(dec("1.5"), dec("2.25"))), dec("2.25"));
    assert_eq!(arith::dec_max(t2(dec("2.25"), dec("1.5"))), dec("2.25"));
}

// ── dec_round ───────────────────────────────────────────────────────────

#[test]
fn dec_round_midpoint_away_from_zero() {
    assert_eq!(arith::dec_round(t2(dec("2.5"), Value::Int(0))), dec("3"));
    assert_eq!(arith::dec_round(t2(dec("-2.5"), Value::Int(0))), dec("-3"));
    assert_eq!(arith::dec_round(t2(dec("1.005"), Value::Int(2))), dec("1.01"));
    assert_eq!(arith::dec_round(t2(dec("1.234"), Value::Int(2))), dec("1.23"));
}

#[test]
#[should_panic]
fn dec_round_negative_places_panics() {
    arith::dec_round(t2(dec("2.5"), Value::Int(-1)));
}

// ── dec_to_int ──────────────────────────────────────────────────────────

#[test]
fn dec_to_int_truncates_toward_zero() {
    assert_eq!(arith::dec_to_int(dec("9.9")), Value::Int(9));
    assert_eq!(arith::dec_to_int(dec("-9.9")), Value::Int(-9));
    assert_eq!(arith::dec_to_int(dec("9.0")), Value::Int(9));
}

#[test]
#[should_panic]
fn dec_to_int_out_of_range_panics() {
    arith::dec_to_int(Value::Dec(Decimal::MAX));
}

// ── str_to_dec ──────────────────────────────────────────────────────────

#[test]
fn str_to_dec_basic() {
    assert_eq!(as_dec(arith::str_to_dec(std::sync::Arc::from("2.5"))), Decimal::from_str("2.5").unwrap());
    assert_eq!(as_dec(arith::str_to_dec(std::sync::Arc::from("-3"))), Decimal::from_str("-3").unwrap());
}

#[test]
#[should_panic]
fn str_to_dec_invalid_input_panics() {
    arith::str_to_dec(std::sync::Arc::from("not-a-number"));
}

//! stdlib(B07-T): typed list predicates with callback contracts (AXIS_STDLIB_DESIGN_V1).
//!
//! Written from the B07 spec table BEFORE reading the A-stage implementation
//! (T-stage rule: expectations come from the spec, not the code). All twelve
//! fns are mint.py aliases with no Rust of their own; each dispatches to one
//! of the four existing element-agnostic HOF leaves in `runtime::iter`:
//!   - int_list_any / text_list_any / bool_list_any             -> iter::any
//!   - int_list_all / text_list_all / bool_list_all             -> iter::all
//!   - int_list_find_index / text_list_find_index / bool_list_find_index -> iter::find_index
//!   - int_list_count / text_list_count / bool_list_count       -> iter::count
//!
//! Each typed alias carries an element-typed `callback 1 (T) -> Bool` contract
//! in the registry stanza (verified at M1 compile time, not by Rust types —
//! `iter::any` etc. take `fn(Value) -> Value` regardless of element type), so
//! the Rust-level behavior under test is identical across the three typed
//! aliases sharing one underlying fn; only the constructed list's element
//! type varies here.

use axis_codegen_bridge::runtime::iter;
use axis_codegen_bridge::runtime::value::{intern_str, Value};

fn i(n: i64) -> Value {
    Value::Int(n)
}

fn t(x: &str) -> Value {
    Value::Str(intern_str(x))
}

fn b(x: bool) -> Value {
    Value::Bool(x)
}

fn int_list(xs: &[i64]) -> Value {
    Value::List(xs.iter().map(|n| i(*n)).collect())
}

fn text_list(xs: &[&str]) -> Value {
    Value::List(xs.iter().map(|x| t(x)).collect())
}

fn bool_list(xs: &[bool]) -> Value {
    Value::List(xs.iter().map(|x| b(*x)).collect())
}

// Predicates used as `fn(Value) -> Value` callbacks, one per element type,
// matching the `callback 1 (T) -> Bool` contract each typed alias declares.

fn pred_is_even(v: Value) -> Value {
    match v {
        Value::Int(n) => Value::Bool(n % 2 == 0),
        other => panic!("pred_is_even: expected Int, got {:?}", other),
    }
}

fn pred_is_nonempty(v: Value) -> Value {
    match v {
        Value::Str(s) => Value::Bool(!s.is_empty()),
        other => panic!("pred_is_nonempty: expected Text, got {:?}", other),
    }
}

fn pred_is_true(v: Value) -> Value {
    match v {
        Value::Bool(x) => Value::Bool(x),
        other => panic!("pred_is_true: expected Bool, got {:?}", other),
    }
}

// ── int_list_any / text_list_any / bool_list_any -> iter::any ────────────

#[test]
fn int_list_any_some_match() {
    assert_eq!(iter::any(int_list(&[1, 3, 4, 5]), pred_is_even), b(true));
}

#[test]
fn int_list_any_no_match() {
    assert_eq!(iter::any(int_list(&[1, 3, 5]), pred_is_even), b(false));
}

#[test]
fn int_list_any_empty_is_false() {
    assert_eq!(iter::any(Value::List(vec![]), pred_is_even), b(false));
}

#[test]
fn text_list_any_some_match() {
    assert_eq!(
        iter::any(text_list(&["", "", "x"]), pred_is_nonempty),
        b(true)
    );
}

#[test]
fn text_list_any_no_match() {
    assert_eq!(iter::any(text_list(&["", ""]), pred_is_nonempty), b(false));
}

#[test]
fn bool_list_any_some_match() {
    assert_eq!(iter::any(bool_list(&[false, true]), pred_is_true), b(true));
}

#[test]
fn bool_list_any_no_match() {
    assert_eq!(iter::any(bool_list(&[false, false]), pred_is_true), b(false));
}

#[test]
#[should_panic]
fn list_any_non_list_panics() {
    iter::any(i(5), pred_is_even);
}

// ── int_list_all / text_list_all / bool_list_all -> iter::all ────────────

#[test]
fn int_list_all_all_match() {
    assert_eq!(iter::all(int_list(&[2, 4, 6]), pred_is_even), b(true));
}

#[test]
fn int_list_all_one_mismatch() {
    assert_eq!(iter::all(int_list(&[2, 3, 4]), pred_is_even), b(false));
}

#[test]
fn int_list_all_empty_is_true() {
    assert_eq!(iter::all(Value::List(vec![]), pred_is_even), b(true));
}

#[test]
fn text_list_all_all_match() {
    assert_eq!(
        iter::all(text_list(&["a", "b"]), pred_is_nonempty),
        b(true)
    );
}

#[test]
fn text_list_all_one_mismatch() {
    assert_eq!(
        iter::all(text_list(&["a", ""]), pred_is_nonempty),
        b(false)
    );
}

#[test]
fn bool_list_all_all_match() {
    assert_eq!(iter::all(bool_list(&[true, true]), pred_is_true), b(true));
}

#[test]
fn bool_list_all_one_mismatch() {
    assert_eq!(
        iter::all(bool_list(&[true, false]), pred_is_true),
        b(false)
    );
}

#[test]
#[should_panic]
fn list_all_non_list_panics() {
    iter::all(t("x"), pred_is_nonempty);
}

// ── int_list_find_index / text_list_find_index / bool_list_find_index ────
// ── -> iter::find_index ───────────────────────────────────────────────

#[test]
fn int_list_find_index_hit() {
    assert_eq!(
        iter::find_index(int_list(&[1, 3, 4, 5]), pred_is_even),
        i(2)
    );
}

#[test]
fn int_list_find_index_miss_is_negative_one() {
    assert_eq!(iter::find_index(int_list(&[1, 3, 5]), pred_is_even), i(-1));
}

#[test]
fn int_list_find_index_empty_is_negative_one() {
    assert_eq!(iter::find_index(Value::List(vec![]), pred_is_even), i(-1));
}

#[test]
fn text_list_find_index_hit() {
    assert_eq!(
        iter::find_index(text_list(&["", "", "x"]), pred_is_nonempty),
        i(2)
    );
}

#[test]
fn text_list_find_index_miss() {
    assert_eq!(
        iter::find_index(text_list(&["", ""]), pred_is_nonempty),
        i(-1)
    );
}

#[test]
fn bool_list_find_index_hit() {
    assert_eq!(
        iter::find_index(bool_list(&[false, false, true]), pred_is_true),
        i(2)
    );
}

#[test]
fn bool_list_find_index_miss() {
    assert_eq!(
        iter::find_index(bool_list(&[false, false]), pred_is_true),
        i(-1)
    );
}

#[test]
#[should_panic]
fn list_find_index_non_list_panics() {
    iter::find_index(b(true), pred_is_true);
}

// ── int_list_count / text_list_count / bool_list_count -> iter::count ────

#[test]
fn int_list_count_basic() {
    assert_eq!(iter::count(int_list(&[1, 2, 3, 4, 5]), pred_is_even), i(2));
}

#[test]
fn int_list_count_none_match() {
    assert_eq!(iter::count(int_list(&[1, 3, 5]), pred_is_even), i(0));
}

#[test]
fn int_list_count_empty_is_zero() {
    assert_eq!(iter::count(Value::List(vec![]), pred_is_even), i(0));
}

#[test]
fn text_list_count_basic() {
    assert_eq!(
        iter::count(text_list(&["", "a", "", "b"]), pred_is_nonempty),
        i(2)
    );
}

#[test]
fn bool_list_count_basic() {
    assert_eq!(
        iter::count(bool_list(&[true, false, true, true]), pred_is_true),
        i(3)
    );
}

#[test]
#[should_panic]
fn list_count_non_list_panics() {
    iter::count(Value::Unit, pred_is_even);
}

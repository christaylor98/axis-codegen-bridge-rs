//! stdlib(B06-T): list search / order / aggregate (AXIS_STDLIB_DESIGN_V1).
//!
//! Written from the B06 spec table BEFORE reading the A-stage implementation
//! (T-stage rule: expectations come from the spec, not the code). Eight fns
//! are new agnostic Rust over ValueList (list_contains, list_index_of,
//! list_sort, list_min, list_max, list_sum, list_all_true, list_any_true);
//! everything else is a mint.py alias with no Rust of its own, tested against
//! its `source` fn directly:
//!   - int_list_contains / text_list_contains / bool_list_contains -> list::list_contains
//!   - int_list_index_of / text_list_index_of / bool_list_index_of -> list::list_index_of
//!   - int_list_sort / text_list_sort                              -> list::list_sort
//!   - int_list_min / text_list_min                                -> list::list_min
//!   - int_list_max / text_list_max                                -> list::list_max
//!   - int_list_sum                                                -> list::list_sum
//!   - bool_list_and                                                -> list::list_all_true
//!   - bool_list_or                                                 -> list::list_any_true
//!   - int_list_range                                               -> iter::range
//!   - text_list_join                                               -> str_ops::str_join
//!
//! `list_sort`/`list_min`/`list_max` share one total order over the three
//! scalar Value variants (Int by value, Text bytewise, Bool false<true) and
//! panic on a mixed-variant or non-scalar comparison — both are exercised
//! at the ValueList level since a typed list is homogeneous by construction
//! and cannot reach that panic through its own contract.

use axis_codegen_bridge::runtime::iter;
use axis_codegen_bridge::runtime::list;
use axis_codegen_bridge::runtime::str_ops;
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

// ── text_list_join -> str_ops::str_join ──────────────────────────────────

#[test]
fn text_list_join_basic() {
    assert_eq!(
        str_ops::str_join(text_list(&["a", "b", "c"]), intern_str(",")),
        t("a,b,c")
    );
}

#[test]
fn text_list_join_empty_list_is_empty_text() {
    assert_eq!(str_ops::str_join(Value::List(vec![]), intern_str(",")), t(""));
}

// ── list_contains (new rust) ──────────────────────────────────────────────

#[test]
fn list_contains_hit() {
    assert_eq!(list::list_contains(int_list(&[1, 2, 3]), i(2)), b(true));
}

#[test]
fn list_contains_miss() {
    assert_eq!(list::list_contains(int_list(&[1, 2, 3]), i(9)), b(false));
}

// ── list_index_of (new rust) ──────────────────────────────────────────────

#[test]
fn list_index_of_hit() {
    assert_eq!(list::list_index_of(text_list(&["a", "b", "c"]), t("b")), i(1));
}

#[test]
fn list_index_of_miss_is_negative_one() {
    assert_eq!(list::list_index_of(text_list(&["a", "b", "c"]), t("z")), i(-1));
}

// ── list_sort (new rust) ────────────────────────────────────────────────

#[test]
fn list_sort_int_ascending() {
    assert_eq!(list::list_sort(int_list(&[3, 1, 2])), int_list(&[1, 2, 3]));
}

#[test]
fn list_sort_text_bytewise() {
    assert_eq!(
        list::list_sort(text_list(&["banana", "apple", "cherry"])),
        text_list(&["apple", "banana", "cherry"])
    );
}

#[test]
fn list_sort_bool_false_before_true() {
    assert_eq!(
        list::list_sort(bool_list(&[true, false, true, false])),
        bool_list(&[false, false, true, true])
    );
}

#[test]
#[should_panic]
fn list_sort_mixed_variants_panics() {
    list::list_sort(Value::List(vec![i(1), t("a")]));
}

#[test]
#[should_panic]
fn list_sort_non_scalar_elements_panics() {
    list::list_sort(Value::List(vec![Value::List(vec![]), Value::List(vec![])]));
}

// ── list_min / list_max (new rust) ────────────────────────────────────────

#[test]
fn list_min_int() {
    assert_eq!(list::list_min(int_list(&[3, 1, 2])), i(1));
}

#[test]
fn list_max_int() {
    assert_eq!(list::list_max(int_list(&[3, 1, 2])), i(3));
}

#[test]
#[should_panic]
fn list_min_empty_panics() {
    list::list_min(Value::List(vec![]));
}

#[test]
#[should_panic]
fn list_max_empty_panics() {
    list::list_max(Value::List(vec![]));
}

#[test]
#[should_panic]
fn list_min_mixed_variants_panics() {
    list::list_min(Value::List(vec![i(1), t("a")]));
}

#[test]
#[should_panic]
fn list_max_mixed_variants_panics() {
    list::list_max(Value::List(vec![i(1), t("a")]));
}

// ── list_sum (new rust) ────────────────────────────────────────────────

#[test]
fn list_sum_basic() {
    assert_eq!(list::list_sum(int_list(&[1, 2, 3])), i(6));
}

#[test]
fn list_sum_empty_is_zero() {
    assert_eq!(list::list_sum(Value::List(vec![])), i(0));
}

#[test]
#[should_panic]
fn list_sum_overflow_panics() {
    list::list_sum(Value::List(vec![i(i64::MAX), i(1)]));
}

#[test]
#[should_panic]
fn list_sum_non_int_element_panics() {
    list::list_sum(Value::List(vec![i(1), t("a")]));
}

// ── list_all_true / list_any_true (new rust) ────────────────────────────

#[test]
fn list_all_true_all_true() {
    assert_eq!(list::list_all_true(bool_list(&[true, true])), b(true));
}

#[test]
fn list_all_true_one_false() {
    assert_eq!(list::list_all_true(bool_list(&[true, false])), b(false));
}

#[test]
fn list_all_true_empty_is_true() {
    assert_eq!(list::list_all_true(Value::List(vec![])), b(true));
}

#[test]
#[should_panic]
fn list_all_true_non_bool_element_panics() {
    list::list_all_true(Value::List(vec![i(1)]));
}

#[test]
fn list_any_true_one_true() {
    assert_eq!(list::list_any_true(bool_list(&[false, true])), b(true));
}

#[test]
fn list_any_true_all_false() {
    assert_eq!(list::list_any_true(bool_list(&[false, false])), b(false));
}

#[test]
fn list_any_true_empty_is_false() {
    assert_eq!(list::list_any_true(Value::List(vec![])), b(false));
}

#[test]
#[should_panic]
fn list_any_true_non_bool_element_panics() {
    list::list_any_true(Value::List(vec![i(1)]));
}

// ── int_list_contains / text_list_contains / bool_list_contains -> list_contains ──

#[test]
fn int_list_contains_hit() {
    assert_eq!(list::list_contains(int_list(&[1, 2, 3]), i(3)), b(true));
}

#[test]
fn int_list_contains_miss() {
    assert_eq!(list::list_contains(int_list(&[1, 2, 3]), i(9)), b(false));
}

#[test]
fn text_list_contains_hit() {
    assert_eq!(list::list_contains(text_list(&["a", "b"]), t("b")), b(true));
}

#[test]
fn text_list_contains_miss() {
    assert_eq!(list::list_contains(text_list(&["a", "b"]), t("z")), b(false));
}

#[test]
fn bool_list_contains_hit() {
    assert_eq!(list::list_contains(bool_list(&[false, false]), b(false)), b(true));
}

#[test]
fn bool_list_contains_miss() {
    assert_eq!(list::list_contains(bool_list(&[false, false]), b(true)), b(false));
}

// ── int_list_index_of / text_list_index_of / bool_list_index_of -> list_index_of ──

#[test]
fn int_list_index_of_hit() {
    assert_eq!(list::list_index_of(int_list(&[10, 20, 30]), i(20)), i(1));
}

#[test]
fn int_list_index_of_miss() {
    assert_eq!(list::list_index_of(int_list(&[10, 20, 30]), i(99)), i(-1));
}

#[test]
fn text_list_index_of_hit() {
    assert_eq!(list::list_index_of(text_list(&["a", "b"]), t("a")), i(0));
}

#[test]
fn text_list_index_of_miss() {
    assert_eq!(list::list_index_of(text_list(&["a", "b"]), t("z")), i(-1));
}

#[test]
fn bool_list_index_of_hit() {
    assert_eq!(list::list_index_of(bool_list(&[false, true]), b(true)), i(1));
}

#[test]
fn bool_list_index_of_miss() {
    assert_eq!(
        list::list_index_of(Value::List(vec![]), b(true)),
        i(-1)
    );
}

// ── int_list_sort / text_list_sort -> list_sort ──────────────────────────

#[test]
fn int_list_sort_basic() {
    assert_eq!(list::list_sort(int_list(&[5, 3, 4, 1, 2])), int_list(&[1, 2, 3, 4, 5]));
}

#[test]
fn text_list_sort_basic() {
    assert_eq!(
        list::list_sort(text_list(&["c", "a", "b"])),
        text_list(&["a", "b", "c"])
    );
}

// ── int_list_min / int_list_max -> list_min / list_max ───────────────────

#[test]
fn int_list_min_basic() {
    assert_eq!(list::list_min(int_list(&[5, 3, 4])), i(3));
}

#[test]
#[should_panic]
fn int_list_min_empty_panics() {
    list::list_min(Value::List(vec![]));
}

#[test]
fn int_list_max_basic() {
    assert_eq!(list::list_max(int_list(&[5, 3, 4])), i(5));
}

#[test]
#[should_panic]
fn int_list_max_empty_panics() {
    list::list_max(Value::List(vec![]));
}

// ── text_list_min / text_list_max -> list_min / list_max ─────────────────

#[test]
fn text_list_min_basic() {
    assert_eq!(list::list_min(text_list(&["banana", "apple", "cherry"])), t("apple"));
}

#[test]
#[should_panic]
fn text_list_min_empty_panics() {
    list::list_min(Value::List(vec![]));
}

#[test]
fn text_list_max_basic() {
    assert_eq!(list::list_max(text_list(&["banana", "apple", "cherry"])), t("cherry"));
}

#[test]
#[should_panic]
fn text_list_max_empty_panics() {
    list::list_max(Value::List(vec![]));
}

// ── int_list_sum -> list_sum ───────────────────────────────────────────

#[test]
fn int_list_sum_basic() {
    assert_eq!(list::list_sum(int_list(&[1, 2, 3, 4])), i(10));
}

#[test]
fn int_list_sum_empty_is_zero() {
    assert_eq!(list::list_sum(Value::List(vec![])), i(0));
}

// ── bool_list_and -> list_all_true ────────────────────────────────────

#[test]
fn bool_list_and_all_true() {
    assert_eq!(list::list_all_true(bool_list(&[true, true, true])), b(true));
}

#[test]
fn bool_list_and_one_false() {
    assert_eq!(list::list_all_true(bool_list(&[true, false, true])), b(false));
}

#[test]
fn bool_list_and_empty_is_true() {
    assert_eq!(list::list_all_true(Value::List(vec![])), b(true));
}

// ── bool_list_or -> list_any_true ─────────────────────────────────────

#[test]
fn bool_list_or_one_true() {
    assert_eq!(list::list_any_true(bool_list(&[false, false, true])), b(true));
}

#[test]
fn bool_list_or_all_false() {
    assert_eq!(list::list_any_true(bool_list(&[false, false])), b(false));
}

#[test]
fn bool_list_or_empty_is_false() {
    assert_eq!(list::list_any_true(Value::List(vec![])), b(false));
}

// ── int_list_range -> iter::range ─────────────────────────────────────
// range(args: Tuple(Int, Int)) -> List; [lo, hi).

#[test]
fn int_list_range_basic() {
    assert_eq!(
        iter::range(Value::Tuple(vec![i(2), i(5)])),
        int_list(&[2, 3, 4])
    );
}

#[test]
fn int_list_range_hi_le_lo_is_empty() {
    assert_eq!(
        iter::range(Value::Tuple(vec![i(5), i(5)])),
        Value::List(vec![])
    );
    assert_eq!(
        iter::range(Value::Tuple(vec![i(5), i(2)])),
        Value::List(vec![])
    );
}

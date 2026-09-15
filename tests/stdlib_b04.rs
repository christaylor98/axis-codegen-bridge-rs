//! stdlib(B04-T): text completions (AXIS_STDLIB_DESIGN_V1).
//!
//! Written from the B04 spec table BEFORE reading the A-stage implementation
//! (T-stage rule: expectations come from the spec, not the code). Each fn is
//! called directly through `axis_codegen_bridge::runtime::…`; every panic
//! condition in the spec's semantics column gets a `#[should_panic]` test.
//!
//! Rust call paths assumed here follow the established str_ops.rs sibling
//! conventions (str_len, str_trim, str_index_of, str_count-shaped):
//!   - unary Text->Bool/Text fns take a bare native `Arc<str>` param:
//!     `str_is_empty`, `str_trim_start`, `str_trim_end`, `str_reverse`,
//!     `str_is_digits`, `str_is_alpha`, `str_is_space`.
//!   - unary Text->TextList fns take a bare native `Arc<str>` param and
//!     return `Value::List(Vec<Value::Str>)` (TextList is the same runtime
//!     representation str_split already returns): `str_lines`, `str_chars`,
//!     `str_split_whitespace`.
//!   - binary Text,Text fns take two native `Arc<str>` params, mirroring
//!     str_index_of: `str_last_index_of`, `str_count`.
//! If A's landed signatures differ, this file is adjusted to match before
//! being run — see the T-stage report's DISCREPANCIES section for any case
//! where the assumption above did not hold.

use axis_codegen_bridge::runtime::str_ops;
use axis_codegen_bridge::runtime::value::{intern_str, Value};
use std::sync::Arc;

fn s(x: &str) -> Arc<str> {
    Arc::from(x)
}

fn text(x: &str) -> Value {
    Value::Str(intern_str(x))
}

fn text_list(items: &[&str]) -> Value {
    Value::List(items.iter().map(|x| text(x)).collect())
}

// ── str_is_empty ────────────────────────────────────────────────────────

#[test]
fn str_is_empty_basic() {
    assert_eq!(str_ops::str_is_empty(s("")), Value::Bool(true));
    assert_eq!(str_ops::str_is_empty(s("a")), Value::Bool(false));
}

// ── str_trim_start / str_trim_end ───────────────────────────────────────

#[test]
fn str_trim_start_basic() {
    assert_eq!(str_ops::str_trim_start(s("  hi  ")), text("hi  "));
    assert_eq!(str_ops::str_trim_start(s("hi")), text("hi"));
}

#[test]
fn str_trim_end_basic() {
    assert_eq!(str_ops::str_trim_end(s("  hi  ")), text("  hi"));
    assert_eq!(str_ops::str_trim_end(s("hi")), text("hi"));
}

#[test]
fn str_trim_start_end_unicode_whitespace() {
    // U+2003 EM SPACE
    assert_eq!(str_ops::str_trim_start(s("\u{2003}hi")), text("hi"));
    assert_eq!(str_ops::str_trim_end(s("hi\u{2003}")), text("hi"));
}

// ── str_lines ────────────────────────────────────────────────────────────

#[test]
fn str_lines_basic() {
    assert_eq!(str_ops::str_lines(s("a\nb\nc")), text_list(&["a", "b", "c"]));
}

#[test]
fn str_lines_strips_trailing_cr() {
    assert_eq!(str_ops::str_lines(s("a\r\nb\nc")), text_list(&["a", "b", "c"]));
}

#[test]
fn str_lines_empty_text_yields_empty_list() {
    assert_eq!(str_ops::str_lines(s("")), Value::List(vec![]));
}

// ── str_chars ────────────────────────────────────────────────────────────

#[test]
fn str_chars_basic() {
    assert_eq!(str_ops::str_chars(s("abc")), text_list(&["a", "b", "c"]));
}

#[test]
fn str_chars_one_element_per_scalar_value() {
    assert_eq!(str_ops::str_chars(s("h\u{e9}llo")), text_list(&["h", "\u{e9}", "l", "l", "o"]));
}

#[test]
fn str_chars_empty_text_yields_empty_list() {
    assert_eq!(str_ops::str_chars(s("")), Value::List(vec![]));
}

// ── str_split_whitespace ────────────────────────────────────────────────

#[test]
fn str_split_whitespace_basic() {
    assert_eq!(
        str_ops::str_split_whitespace(s("  a  b\tc ")),
        text_list(&["a", "b", "c"])
    );
}

#[test]
fn str_split_whitespace_no_empty_elements() {
    assert_eq!(str_ops::str_split_whitespace(s("   ")), Value::List(vec![]));
    assert_eq!(str_ops::str_split_whitespace(s("")), Value::List(vec![]));
}

// ── str_reverse ──────────────────────────────────────────────────────────

#[test]
fn str_reverse_basic() {
    assert_eq!(str_ops::str_reverse(s("abc")), text("cba"));
}

#[test]
fn str_reverse_by_scalar_value() {
    assert_eq!(str_ops::str_reverse(s("h\u{e9}llo")), text("oll\u{e9}h"));
}

#[test]
fn str_reverse_empty() {
    assert_eq!(str_ops::str_reverse(s("")), text(""));
}

// ── str_last_index_of ───────────────────────────────────────────────────

#[test]
fn str_last_index_of_basic() {
    assert_eq!(str_ops::str_last_index_of(s("abcabc"), s("bc")), Value::Int(4));
}

#[test]
fn str_last_index_of_not_found_is_negative_one() {
    assert_eq!(str_ops::str_last_index_of(s("abcabc"), s("zz")), Value::Int(-1));
}

#[test]
fn str_last_index_of_single_occurrence() {
    assert_eq!(str_ops::str_last_index_of(s("abcabc"), s("a")), Value::Int(3));
}

// ── str_count ────────────────────────────────────────────────────────────

#[test]
fn str_count_non_overlapping() {
    assert_eq!(str_ops::str_count(s("ababab"), s("ab")), Value::Int(3));
}

#[test]
fn str_count_overlapping_needle_counted_non_overlapping() {
    // "aaaa" contains "aa" at positions 0 and 2 non-overlapping -> 2, not 3.
    assert_eq!(str_ops::str_count(s("aaaa"), s("aa")), Value::Int(2));
}

#[test]
fn str_count_no_match_is_zero() {
    assert_eq!(str_ops::str_count(s("abc"), s("z")), Value::Int(0));
}

#[test]
#[should_panic]
fn str_count_empty_needle_panics() {
    str_ops::str_count(s("abc"), s(""));
}

// ── str_is_digits ────────────────────────────────────────────────────────

#[test]
fn str_is_digits_basic() {
    assert_eq!(str_ops::str_is_digits(s("12345")), Value::Bool(true));
    assert_eq!(str_ops::str_is_digits(s("123a5")), Value::Bool(false));
}

#[test]
fn str_is_digits_empty_is_false() {
    assert_eq!(str_ops::str_is_digits(s("")), Value::Bool(false));
}

// ── str_is_alpha ─────────────────────────────────────────────────────────

#[test]
fn str_is_alpha_basic() {
    assert_eq!(str_ops::str_is_alpha(s("hello")), Value::Bool(true));
    assert_eq!(str_ops::str_is_alpha(s("hell0")), Value::Bool(false));
}

#[test]
fn str_is_alpha_empty_is_false() {
    assert_eq!(str_ops::str_is_alpha(s("")), Value::Bool(false));
}

// ── str_is_space ─────────────────────────────────────────────────────────

#[test]
fn str_is_space_basic() {
    assert_eq!(str_ops::str_is_space(s("   ")), Value::Bool(true));
    assert_eq!(str_ops::str_is_space(s("  x")), Value::Bool(false));
}

#[test]
fn str_is_space_empty_is_false() {
    assert_eq!(str_ops::str_is_space(s("")), Value::Bool(false));
}

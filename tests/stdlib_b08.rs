//! stdlib(B08-T): bytes_empty / bytes_eq / bytes_index_of (AXIS_STDLIB_DESIGN_V1).
//!
//! Written from the B08 spec table BEFORE reading the A-stage implementation
//! (T-stage rule: expectations come from the spec, not the code). All three
//! are new agnostic Rust over Bytes in `runtime::bytes_codec`:
//!   - bytes_empty(Unit) -> Bytes            — zero-length Bytes
//!   - bytes_eq(Bytes, Bytes) -> Bool         — byte-wise equality
//!   - bytes_index_of(Bytes, Bytes) -> Int    — index of first occurrence of
//!     needle in haystack, else -1; empty needle returns 0
//!
//! `int16_be_decode` / `int32_be_decode` / `int64_be_encode` / `int64_be_decode`
//! also landed in the A-stage commit but are out of scope for this brief's
//! table — not tested here (they are exercised at the M1 level in proofs/B08).

use axis_codegen_bridge::runtime::bytes_codec::{bytes_empty, bytes_eq, bytes_index_of};
use axis_codegen_bridge::runtime::value::Value;

fn bytes(xs: &[u8]) -> Value {
    Value::Bytes(xs.to_vec())
}

// ── bytes_empty ──────────────────────────────────────────────────────────

#[test]
fn bytes_empty_is_zero_length() {
    assert_eq!(bytes_empty(Value::Unit), bytes(&[]));
}

// ── bytes_eq ─────────────────────────────────────────────────────────────

#[test]
fn bytes_eq_equal_bytes_is_true() {
    assert_eq!(bytes_eq(vec![1, 2, 3], vec![1, 2, 3]), Value::Bool(true));
}

#[test]
fn bytes_eq_different_content_same_length_is_false() {
    assert_eq!(bytes_eq(vec![1, 2, 3], vec![1, 2, 4]), Value::Bool(false));
}

#[test]
fn bytes_eq_different_length_is_false() {
    assert_eq!(bytes_eq(vec![1, 2, 3], vec![1, 2]), Value::Bool(false));
}

#[test]
fn bytes_eq_both_empty_is_true() {
    assert_eq!(bytes_eq(vec![], vec![]), Value::Bool(true));
}

// ── bytes_index_of ───────────────────────────────────────────────────────

#[test]
fn bytes_index_of_found_at_start() {
    assert_eq!(bytes_index_of(vec![1, 2, 3, 4], vec![1, 2]), Value::Int(0));
}

#[test]
fn bytes_index_of_found_in_middle() {
    assert_eq!(bytes_index_of(vec![9, 1, 2, 3, 9], vec![2, 3]), Value::Int(2));
}

#[test]
fn bytes_index_of_found_at_end() {
    assert_eq!(bytes_index_of(vec![1, 2, 3, 4], vec![3, 4]), Value::Int(2));
}

#[test]
fn bytes_index_of_not_found_is_negative_one() {
    assert_eq!(bytes_index_of(vec![1, 2, 3], vec![9, 9]), Value::Int(-1));
}

#[test]
fn bytes_index_of_empty_needle_returns_zero() {
    assert_eq!(bytes_index_of(vec![1, 2, 3], vec![]), Value::Int(0));
}

#[test]
fn bytes_index_of_empty_needle_in_empty_haystack_returns_zero() {
    assert_eq!(bytes_index_of(vec![], vec![]), Value::Int(0));
}

#[test]
fn bytes_index_of_nonempty_needle_in_empty_haystack_is_negative_one() {
    assert_eq!(bytes_index_of(vec![], vec![1]), Value::Int(-1));
}

#[test]
fn bytes_index_of_needle_longer_than_haystack_is_negative_one() {
    assert_eq!(bytes_index_of(vec![1, 2], vec![1, 2, 3]), Value::Int(-1));
}

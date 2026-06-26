//! BRIDGE_HASH_PRIMITIVE_M1 acceptance — content_hash and hash256_parse.
//!
//! Tests T1..T15 enumerated in the BRIDGE_HASH_PRIMITIVE_M1 intent.

use axis_codegen_bridge::runtime::hash::{content_hash, hash256_parse};
use axis_codegen_bridge::runtime::value::{get_str, intern_str, Value};
use sha2::{Digest, Sha256};

fn bytes_value(bytes: &[u8]) -> Value {
    Value::List(bytes.iter().map(|&b| Value::Int(b as i64)).collect())
}

fn str_of(v: &Value) -> String {
    match v {
        Value::Str(h) => get_str(*h),
        _ => panic!("expected Str, got {:?}", v),
    }
}

// ── T1: known SHA-256 vector for empty input ────────────────────────────────

#[test]
fn t1_content_hash_empty_bytes_matches_known_sha256_vector() {
    let r = content_hash(Value::List(vec![]));
    let expected =
        "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    assert_eq!(r, Value::Str(intern_str(expected)));
}

// ── T2: output has the locked prefix ────────────────────────────────────────

#[test]
fn t2_content_hash_output_has_sha256_prefix() {
    let r = content_hash(Value::List(vec![Value::Int(42)]));
    assert!(str_of(&r).starts_with("sha256:"));
}

// ── T3: output is always exactly 71 chars ──────────────────────────────────

#[test]
fn t3_content_hash_output_is_always_71_chars() {
    for bytes in [b"".as_slice(), b"a".as_slice(), b"hello world".as_slice()] {
        let r = content_hash(bytes_value(bytes));
        assert_eq!(str_of(&r).len(), 71, "input: {:?}", bytes);
    }
}

// ── T4: bridge output matches direct sha2 computation ──────────────────────

#[test]
fn t4_content_hash_matches_reference_impl() {
    let expected = format!(
        "sha256:{}",
        Sha256::digest(b"hello")
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>()
    );
    let r = content_hash(bytes_value(b"hello"));
    assert_eq!(r, Value::Str(intern_str(&expected)));
}

// ── T5: deterministic ───────────────────────────────────────────────────────

#[test]
fn t5_content_hash_deterministic() {
    let a = content_hash(bytes_value(b"axis"));
    let b = content_hash(bytes_value(b"axis"));
    assert_eq!(a, b);
}

// ── T6: parse accepts a valid 64-hex address ────────────────────────────────

#[test]
fn t6_hash256_parse_accepts_valid_address() {
    let valid = format!("sha256:{}", "a".repeat(64));
    let r = hash256_parse(Value::Str(intern_str(&valid)));
    assert_eq!(str_of(&r), valid);
}

// ── T7: 63-char hex must reject (explicit axVerity criterion) ──────────────

#[test]
#[should_panic(expected = "hash256_parse: invalid input")]
fn t7_hash256_parse_rejects_63_hex_chars() {
    let short = format!("sha256:{}", "a".repeat(63));
    hash256_parse(Value::Str(intern_str(&short)));
}

// ── T8: missing prefix rejected ────────────────────────────────────────────

#[test]
#[should_panic(expected = "hash256_parse: invalid input")]
fn t8_hash256_parse_rejects_missing_prefix() {
    let no_prefix = "a".repeat(64);
    hash256_parse(Value::Str(intern_str(&no_prefix)));
}

// ── T9: 65-char hex rejected ───────────────────────────────────────────────

#[test]
#[should_panic(expected = "hash256_parse: invalid input")]
fn t9_hash256_parse_rejects_65_hex_chars() {
    let long = format!("sha256:{}", "a".repeat(65));
    hash256_parse(Value::Str(intern_str(&long)));
}

// ── T10: non-hex char in body rejected ─────────────────────────────────────

#[test]
#[should_panic(expected = "hash256_parse: invalid input")]
fn t10_hash256_parse_rejects_non_hex_chars_in_body() {
    // 63 'a' + one 'z' = 64 chars total, but 'z' is not hex.
    let bad = format!("sha256:{}z", "a".repeat(63));
    hash256_parse(Value::Str(intern_str(&bad)));
}

// ── T11: byte > 255 panics ─────────────────────────────────────────────────

#[test]
#[should_panic(expected = "UNKNOWN gate")]
fn t11_content_hash_rejects_out_of_range_byte() {
    content_hash(Value::List(vec![Value::Int(256)]));
}

// ── T12: negative byte panics ──────────────────────────────────────────────

#[test]
#[should_panic(expected = "UNKNOWN gate")]
fn t12_content_hash_rejects_negative_byte() {
    content_hash(Value::List(vec![Value::Int(-1)]));
}

// ── T13: non-List input panics ─────────────────────────────────────────────

#[test]
#[should_panic(expected = "UNKNOWN gate")]
fn t13_content_hash_rejects_non_list() {
    content_hash(Value::Int(0));
}

// ── T14: non-Text input to parse panics ────────────────────────────────────

#[test]
#[should_panic(expected = "UNKNOWN gate")]
fn t14_hash256_parse_rejects_non_text_input() {
    hash256_parse(Value::Int(0));
}

// ── T15: round-trip — content_hash output parses cleanly ───────────────────

#[test]
fn t15_content_hash_output_feeds_hash256_parse() {
    let addr = content_hash(Value::List(vec![Value::Int(1), Value::Int(2)]));
    let parsed = hash256_parse(addr.clone());
    assert_eq!(parsed, addr);
}

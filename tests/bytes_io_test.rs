//! BRIDGE_BYTES_IO_M1 acceptance — text_to_bytes, fs_write_bytes, fs_read_bytes,
//! bytes_to_text.

use std::process::id as pid;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axis_codegen_bridge::runtime::bytes_io::{
    bytes_to_text, fs_read_bytes, fs_write_bytes, text_to_bytes,
};
use axis_codegen_bridge::runtime::value::{get_str, intern_str, Value};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_tmp_path(label: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("/tmp/axv_bytes_io_test_{}_{}_{}_{}", label, pid(), nanos, n)
}

fn s(v: &str) -> Value { Value::Str(intern_str(v)) }

// ── T1: text_to_bytes is UTF-8 encoding ─────────────────────────────────────

#[test]
fn t1_text_to_bytes_ascii() {
    let r = text_to_bytes(intern_str("hello"));
    assert_eq!(r, Value::Bytes(b"hello".to_vec()));
}

#[test]
fn t2_text_to_bytes_utf8() {
    let r = text_to_bytes(intern_str("héllo")); // é = 0xC3 0xA9
    assert_eq!(r, Value::Bytes(vec![b'h', 0xC3, 0xA9, b'l', b'l', b'o']));
}

#[test]
fn t3_text_to_bytes_empty() {
    let r = text_to_bytes(intern_str(""));
    assert_eq!(r, Value::Bytes(vec![]));
}

#[test]
#[should_panic(expected = "expected Text, got")]
fn t4_text_to_bytes_rejects_non_text() {
    // text_to_bytes now takes a native Arc<str> directly — a wrong dynamic
    // type can no longer reach the fn body itself (compile-time enforced).
    // The type check that used to live in the body now lives in the call
    // site's `.as_text()` accessor, so that's what this exercises.
    let _ = text_to_bytes(Value::Int(0).as_text());
}

// ── T5..T8: fs_write_bytes / fs_read_bytes round-trip ───────────────────────

#[test]
fn t5_round_trip_ascii() {
    let path = unique_tmp_path("rt_ascii");
    let payload = b"hello".to_vec();

    let w = fs_write_bytes(intern_str(&path), payload.clone());
    assert_eq!(w, Value::Unit);

    let r = fs_read_bytes(intern_str(&path));
    assert_eq!(r, Value::Bytes(payload));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn t6_round_trip_binary_with_nulls_and_high_bytes() {
    let path = unique_tmp_path("rt_bin");
    let payload: Vec<u8> = (0u8..=255u8).collect();

    let w = fs_write_bytes(intern_str(&path), payload.clone());
    assert_eq!(w, Value::Unit);

    let r = fs_read_bytes(intern_str(&path));
    assert_eq!(r, Value::Bytes(payload));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn t7_round_trip_empty() {
    let path = unique_tmp_path("rt_empty");
    let w = fs_write_bytes(intern_str(&path), vec![]);
    assert_eq!(w, Value::Unit);

    let r = fs_read_bytes(intern_str(&path));
    assert_eq!(r, Value::Bytes(vec![]));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn t8_write_is_atomic_no_partial_file_visible_after_second_write() {
    // After a successful second write the file holds the second payload —
    // never a mix. (Cannot test crash atomicity directly in a unit test;
    // this checks that the rename produces a clean swap.)
    let path = unique_tmp_path("rt_atomic");
    let v1 = b"first".to_vec();
    let v2 = b"second-different-length".to_vec();

    let w1 = fs_write_bytes(intern_str(&path), v1.clone());
    assert_eq!(w1, Value::Unit);
    let w2 = fs_write_bytes(intern_str(&path), v2.clone());
    assert_eq!(w2, Value::Unit);

    let r = fs_read_bytes(intern_str(&path));
    assert_eq!(r, Value::Bytes(v2));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn t9_text_to_bytes_round_trips_via_filesystem() {
    let path = unique_tmp_path("rt_text");
    let payload = text_to_bytes(intern_str("hello, axVerity"));

    let w = fs_write_bytes(intern_str(&path), payload.as_bytes());
    assert_eq!(w, Value::Unit);
    let r = fs_read_bytes(intern_str(&path));
    assert_eq!(r, payload);

    let _ = std::fs::remove_file(&path);
}

// ── T10..T12: error surfaces (panics) ───────────────────────────────────────

#[test]
#[should_panic(expected = "fs_read_bytes(")]
fn t10_fs_read_bytes_missing_file_panics() {
    let path = unique_tmp_path("nope");
    fs_read_bytes(intern_str(&path));
}

#[test]
#[should_panic(expected = "fs_write_bytes(")]
fn t11_fs_write_bytes_to_bad_dir_panics() {
    let path = "/nonexistent_axv_dir_abc/sub/file.bin".to_string();
    fs_write_bytes(intern_str(&path), b"x".to_vec());
}

#[test]
#[should_panic(expected = "expected Bytes, got")]
fn t12_fs_write_bytes_rejects_non_bytes_content() {
    // Same shift as T4: the content-type check now lives in the call site's
    // `.as_bytes()` accessor, not in fs_write_bytes's own body.
    let _ = fs_write_bytes(intern_str(&unique_tmp_path("typecheck")), s("not bytes").as_bytes());
}

// ── T13..T17: bytes_to_text ────────────────────────────────────────────────

#[test]
fn t13_bytes_to_text_round_trips_ascii() {
    let r = bytes_to_text(text_to_bytes(intern_str("hello")).as_bytes());
    match r {
        Value::Str(h) => assert_eq!(get_str(h), "hello"),
        other => panic!("expected Text, got {:?}", other),
    }
}

#[test]
fn t14_bytes_to_text_round_trips_utf8() {
    let r = bytes_to_text(text_to_bytes(intern_str("héllo, 世界")).as_bytes());
    match r {
        Value::Str(h) => assert_eq!(get_str(h), "héllo, 世界"),
        other => panic!("expected Text, got {:?}", other),
    }
}

#[test]
fn t15_bytes_to_text_empty() {
    let r = bytes_to_text(vec![]);
    match r {
        Value::Str(h) => assert_eq!(get_str(h), ""),
        other => panic!("expected Text, got {:?}", other),
    }
}

#[test]
#[should_panic(expected = "bytes_to_text: invalid UTF-8")]
fn t16_bytes_to_text_invalid_utf8_panics() {
    bytes_to_text(vec![0xFF]);
}

#[test]
#[should_panic(expected = "expected Bytes, got")]
fn t17_bytes_to_text_rejects_non_bytes() {
    // Same shift as T4/T12: the shape check now lives in `.as_bytes()`.
    let _ = bytes_to_text(s("not bytes").as_bytes());
}

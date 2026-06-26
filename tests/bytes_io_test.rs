//! BRIDGE_BYTES_IO_M1 acceptance — text_to_bytes, fs_write_bytes, fs_read_bytes,
//! bytes_to_text.

use std::process::id as pid;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axis_codegen_bridge::runtime::bytes_io::{
    bytes_to_text, fs_read_bytes, fs_write_bytes, text_to_bytes,
};
use axis_codegen_bridge::runtime::value::{get_str, get_tag_name, intern_str, Value};

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

fn ctor_tag(v: &Value) -> String {
    match v {
        Value::Ctor { tag, .. } => get_tag_name(*tag),
        _ => panic!("expected Ctor, got {:?}", v),
    }
}

fn ctor_field0<'a>(v: &'a Value) -> &'a Value {
    match v {
        Value::Ctor { fields, .. } if !fields.is_empty() => &fields[0],
        _ => panic!("expected non-empty Ctor, got {:?}", v),
    }
}

// ── T1: text_to_bytes is UTF-8 encoding ─────────────────────────────────────

#[test]
fn t1_text_to_bytes_ascii() {
    let r = text_to_bytes(s("hello"));
    assert_eq!(r, Value::Bytes(b"hello".to_vec()));
}

#[test]
fn t2_text_to_bytes_utf8() {
    let r = text_to_bytes(s("héllo")); // é = 0xC3 0xA9
    assert_eq!(r, Value::Bytes(vec![b'h', 0xC3, 0xA9, b'l', b'l', b'o']));
}

#[test]
fn t3_text_to_bytes_empty() {
    let r = text_to_bytes(s(""));
    assert_eq!(r, Value::Bytes(vec![]));
}

#[test]
#[should_panic(expected = "text_to_bytes: expected Text")]
fn t4_text_to_bytes_rejects_non_text() {
    text_to_bytes(Value::Int(0));
}

// ── T5..T8: fs_write_bytes / fs_read_bytes round-trip ───────────────────────

#[test]
fn t5_round_trip_ascii() {
    let path = unique_tmp_path("rt_ascii");
    let payload = b"hello".to_vec();

    let w = fs_write_bytes(Value::Tuple(vec![s(&path), Value::Bytes(payload.clone())]));
    assert_eq!(ctor_tag(&w), "Ok");

    let r = fs_read_bytes(s(&path));
    assert_eq!(ctor_tag(&r), "Ok");
    assert_eq!(ctor_field0(&r), &Value::Bytes(payload));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn t6_round_trip_binary_with_nulls_and_high_bytes() {
    let path = unique_tmp_path("rt_bin");
    let payload: Vec<u8> = (0u8..=255u8).collect();

    let w = fs_write_bytes(Value::Tuple(vec![s(&path), Value::Bytes(payload.clone())]));
    assert_eq!(ctor_tag(&w), "Ok");

    let r = fs_read_bytes(s(&path));
    assert_eq!(ctor_tag(&r), "Ok");
    assert_eq!(ctor_field0(&r), &Value::Bytes(payload));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn t7_round_trip_empty() {
    let path = unique_tmp_path("rt_empty");
    let w = fs_write_bytes(Value::Tuple(vec![s(&path), Value::Bytes(vec![])]));
    assert_eq!(ctor_tag(&w), "Ok");

    let r = fs_read_bytes(s(&path));
    assert_eq!(ctor_tag(&r), "Ok");
    assert_eq!(ctor_field0(&r), &Value::Bytes(vec![]));

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

    let w1 = fs_write_bytes(Value::Tuple(vec![s(&path), Value::Bytes(v1.clone())]));
    assert_eq!(ctor_tag(&w1), "Ok");
    let w2 = fs_write_bytes(Value::Tuple(vec![s(&path), Value::Bytes(v2.clone())]));
    assert_eq!(ctor_tag(&w2), "Ok");

    let r = fs_read_bytes(s(&path));
    assert_eq!(ctor_field0(&r), &Value::Bytes(v2));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn t9_text_to_bytes_round_trips_via_filesystem() {
    let path = unique_tmp_path("rt_text");
    let payload = text_to_bytes(s("hello, axVerity"));

    let w = fs_write_bytes(Value::Tuple(vec![s(&path), payload.clone()]));
    assert_eq!(ctor_tag(&w), "Ok");
    let r = fs_read_bytes(s(&path));
    assert_eq!(ctor_field0(&r), &payload);

    let _ = std::fs::remove_file(&path);
}

// ── T10..T12: error surfaces ────────────────────────────────────────────────

#[test]
fn t10_fs_read_bytes_missing_file_returns_err() {
    let path = unique_tmp_path("nope");
    let r = fs_read_bytes(s(&path));
    assert_eq!(ctor_tag(&r), "Err");
    match ctor_field0(&r) {
        Value::Str(h) => {
            let msg = get_str(*h);
            assert!(msg.contains(&path), "Err message should include path: {}", msg);
        }
        other => panic!("Err payload should be Text, got {:?}", other),
    }
}

#[test]
fn t11_fs_write_bytes_to_bad_dir_returns_err() {
    // /nonexistent/sub/path should fail at the open-for-tmp step.
    let path = "/nonexistent_axv_dir_abc/sub/file.bin".to_string();
    let r = fs_write_bytes(Value::Tuple(vec![s(&path), Value::Bytes(b"x".to_vec())]));
    assert_eq!(ctor_tag(&r), "Err");
}

#[test]
#[should_panic(expected = "fs_write_bytes: arg 1 expected Bytes")]
fn t12_fs_write_bytes_rejects_non_bytes_content() {
    let path = unique_tmp_path("typecheck");
    fs_write_bytes(Value::Tuple(vec![s(&path), s("not bytes")]));
}

// ── T13..T17: bytes_to_text ────────────────────────────────────────────────

#[test]
fn t13_bytes_to_text_round_trips_ascii() {
    // result_text_unwrap(bytes_to_text(text_to_bytes("hello"))) == "hello"
    let r = bytes_to_text(text_to_bytes(s("hello")));
    assert_eq!(ctor_tag(&r), "Ok");
    match ctor_field0(&r) {
        Value::Str(h) => assert_eq!(get_str(*h), "hello"),
        other => panic!("Ok payload should be Text, got {:?}", other),
    }
}

#[test]
fn t14_bytes_to_text_round_trips_utf8() {
    let r = bytes_to_text(text_to_bytes(s("héllo, 世界")));
    assert_eq!(ctor_tag(&r), "Ok");
    match ctor_field0(&r) {
        Value::Str(h) => assert_eq!(get_str(*h), "héllo, 世界"),
        other => panic!("Ok payload should be Text, got {:?}", other),
    }
}

#[test]
fn t15_bytes_to_text_empty() {
    let r = bytes_to_text(Value::Bytes(vec![]));
    assert_eq!(ctor_tag(&r), "Ok");
    match ctor_field0(&r) {
        Value::Str(h) => assert_eq!(get_str(*h), ""),
        other => panic!("Ok payload should be Text, got {:?}", other),
    }
}

#[test]
fn t16_bytes_to_text_invalid_utf8_returns_err() {
    // 0xFF on its own is not valid UTF-8.
    let r = bytes_to_text(Value::Bytes(vec![0xFF]));
    assert_eq!(ctor_tag(&r), "Err");
    match ctor_field0(&r) {
        Value::Str(_) => {} // any message is fine
        other => panic!("Err payload should be Text, got {:?}", other),
    }
}

#[test]
#[should_panic(expected = "bytes_to_text: expected Bytes")]
fn t17_bytes_to_text_rejects_non_bytes() {
    bytes_to_text(s("not bytes"));
}

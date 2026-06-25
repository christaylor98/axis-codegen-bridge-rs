//! BRIDGE_HASH_PRIMITIVE_M1 — content hashing foreign primitives.
//!
//! Resolves hld:axverity-hash-dependency. Two bridge functions:
//!
//!   * `content_hash(bytes: ValueList) -> Text`
//!       SHA-256 of the byte sequence, returned as `"sha256:{64-hex}"` (71 chars).
//!
//!   * `hash256_parse(text: Text) -> ResultText`
//!       Validates `"sha256:{64-hex}"`. `Ok(text)` on match, `Err(msg)` otherwise.
//!       A 63-char hex portion MUST return Err (axVerity criterion).
//!
//! Reference impl (read-only): axis-lang-lab-working/src/fabric/hash.rs
//! Bridge produces the same SHA-256 bytes — both use the `sha2` crate.

use sha2::{Digest, Sha256};

use super::value::{get_str, intern_str, intern_tag, Value};

// ── Byte extractor (UNKNOWN gate — panic, no silent coercion) ────────────────
//
// Same shape as axbi.rs::input_to_bytes; kept local to preserve the module
// boundary between hash and axbi.

#[cold]
#[track_caller]
fn hard_fail_ch(msg: &str) -> ! {
    panic!("content_hash UNKNOWN gate: {}", msg)
}

fn input_to_bytes(v: Value) -> Vec<u8> {
    match v {
        Value::List(es) => es
            .into_iter()
            .map(|e| match e {
                Value::Int(n) if (0..=255).contains(&n) => n as u8,
                Value::Int(n) => hard_fail_ch(&format!("byte value {n} out of 0..=255")),
                _ => hard_fail_ch("byte list element is not Int"),
            })
            .collect(),
        _ => hard_fail_ch("input must be a List of Int bytes"),
    }
}

// ── content_hash ─────────────────────────────────────────────────────────────

/// `content_hash(bytes: ValueList) -> Text`
///
/// SHA-256 of the byte sequence. Always returns exactly 71 chars:
/// `"sha256:"` (7) + 64 lowercase hex chars.
#[track_caller]
pub fn content_hash(v: Value) -> Value {
    let bytes = input_to_bytes(v);
    let digest = Sha256::digest(&bytes);
    let hex: String = digest.iter().map(|b| format!("{:02x}", b)).collect();
    Value::Str(intern_str(&format!("sha256:{}", hex)))
}

// ── hash256_parse ────────────────────────────────────────────────────────────

/// `hash256_parse(text: Text) -> ResultText`
///
/// Returns `Ok(text)` if input is exactly `"sha256:"` + 64 lowercase hex chars,
/// else `Err(msg)`. Hex portion of any other length (including 63) -> Err.
#[track_caller]
pub fn hash256_parse(v: Value) -> Value {
    let s = match v {
        Value::Str(h) => get_str(h),
        _ => panic!("hash256_parse UNKNOWN gate: input must be Text"),
    };

    let check: Result<(), String> = (|| {
        let hex = s
            .strip_prefix("sha256:")
            .ok_or_else(|| {
                let head = &s[..s.len().min(20)];
                format!("expected 'sha256:' prefix, got: {}", head)
            })?;
        if hex.len() != 64 {
            return Err(format!(
                "expected 64 hex chars after 'sha256:', got {}",
                hex.len()
            ));
        }
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("non-hex character in hash".to_string());
        }
        Ok(())
    })();

    match check {
        Ok(()) => Value::Ctor {
            tag: intern_tag("Ok"),
            fields: vec![Value::Str(intern_str(&s))],
        },
        Err(e) => Value::Ctor {
            tag: intern_tag("Err"),
            fields: vec![Value::Str(intern_str(&e))],
        },
    }
}

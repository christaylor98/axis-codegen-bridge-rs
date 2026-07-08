//! BRIDGE_BYTES_IO_M1 — text_to_bytes, fs_write_bytes, fs_read_bytes,
//! bytes_hash, fs_mkdir_p, bytes_to_text.
//!
//! Leaf foreign primitives for the M1 surface to convert Text to a Bytes
//! blob, round-trip blobs through the filesystem, SHA-256 a Bytes blob,
//! idempotently create directories, and decode a Bytes blob back to Text.
//!
//!   * `text_to_bytes(Text) -> Bytes`
//!         UTF-8 encode the Text and wrap as `Value::Bytes`.
//!
//!   * `fs_write_bytes(path: Text, content: Bytes) -> Unit`
//!         Durable write: write `<path>.tmp`, fsync the tmp file, rename
//!         atomically over `<path>`, fsync the parent directory. The parent
//!         directory fsync is not optional — without it the rename itself is
//!         not durable across crash. If the parent dir cannot be fsynced
//!         (e.g. read-only mount), the call panics — never silently skip.
//!
//!   * `fs_read_bytes(path: Text) -> Bytes`
//!         `std::fs::read(path)`. Panics on any OS error.
//!
//!   * `bytes_hash(Bytes) -> Text`
//!         SHA-256 of a Bytes blob, returned as `"sha256:{64-hex}"`. Same
//!         crypto as `content_hash` but consumes `Value::Bytes` directly so
//!         the bridge avoids the per-element `List<Int>` coercion.
//!
//!   * `fs_mkdir_p(Text) -> Unit`
//!         `std::fs::create_dir_all` — recursive idempotent directory create.
//!         Panics on any OS error.
//!
//!   * `bytes_to_text(Bytes) -> Text`
//!         Checked UTF-8 decode. Returns the decoded Text. Panics on invalid
//!         UTF-8. Inverse of `text_to_bytes` for valid UTF-8 inputs.
//!
//! Identities are sha256(name_utf8) — same convention as the rest of the
//! bridge leaf primitives.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use super::value::{Value, get_str, intern_str};

// ── text_to_bytes ────────────────────────────────────────────────────────────

#[track_caller]
pub fn text_to_bytes(v: Value) -> Value {
    match v {
        Value::Str(h) => Value::Bytes(get_str(h).into_bytes()),
        other => panic!("text_to_bytes: expected Text, got {:?}", other),
    }
}

// ── fs_write_bytes ───────────────────────────────────────────────────────────

#[track_caller]
pub fn fs_write_bytes(args: Value) -> Value {
    let (path, content) = match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("fs_write_bytes: expected Tuple(Text, Bytes), got {:?}", other),
    };
    let path = match path {
        Value::Str(h) => get_str(h),
        other => panic!("fs_write_bytes: arg 0 expected Text, got {:?}", other),
    };
    let content = match content {
        Value::Bytes(bs) => bs,
        other => panic!("fs_write_bytes: arg 1 expected Bytes, got {:?}", other),
    };
    if let Err(e) = write_durable(&path, &content) {
        panic!("fs_write_bytes({}): {}", path, e);
    }
    Value::Unit
}

// ── fs_read_bytes ────────────────────────────────────────────────────────────

#[track_caller]
pub fn fs_read_bytes(path: Value) -> Value {
    let path_str = match path {
        Value::Str(h) => get_str(h),
        other => panic!("fs_read_bytes: expected Text, got {:?}", other),
    };
    match std::fs::read(&path_str) {
        Ok(bs) => Value::Bytes(bs),
        Err(e) => panic!("fs_read_bytes({}): {}", path_str, e),
    }
}

// ── Durable write helper ─────────────────────────────────────────────────────
//
// Crash-safe write protocol:
//   1. write content to `<path>.tmp` (truncate, create as needed)
//   2. fsync(tmp_file) — content reaches disk
//   3. rename(tmp_file, path) — atomic on the same filesystem
//   4. fsync(parent_dir) — the rename's new directory entry reaches disk
//
// Step 4 is required: rename(2) commits the directory metadata in cache, but
// without fsync on the parent, a crash before the next dir flush can lose the
// directory entry while the inode itself is durable on disk.
//
// On platforms where opening a directory for fsync is not supported, the call
// fails with an OS error — that surfaces as Err to M1 rather than being
// silently skipped, per the handover's "no platform exceptions" rule.

fn write_durable(path: &str, content: &[u8]) -> std::io::Result<()> {
    let path = Path::new(path);
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("fs_write_bytes: path '{}' has no parent directory", path.display()),
        )
    })?;
    let parent = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };

    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("fs_write_bytes: path '{}' has no file name", path.display()),
        )
    })?;
    // UNIQUE temp name per in-flight write (AXVERITY_ACCEPTLOOP_SHARD_DISPATCH).
    // The old fixed "<file>.tmp" is a concurrency race: two threads (or
    // processes) writing the SAME target share one temp path, so the first
    // thread's rename(tmp -> target) pulls the temp out from under the second,
    // whose own rename then fails ENOEXIST ("No such file or directory"). This
    // was latent while every writer of a shared path (ledger.current,
    // <name>.current) was serialized by the single accept loop or an external
    // flock; the shared-listener worker pool writes them concurrently. A temp
    // name unique per (pid, monotonic counter) makes fs_write_bytes
    // concurrency-safe BY CONSTRUCTION — no lock, and single-threaded behavior
    // is byte-identical (still a temp-then-atomic-rename in the same dir). The
    // last durable rename wins the target, which is the intended
    // last-writer-wins for a derived .current cache.
    static WRITE_SEQ: AtomicU64 = AtomicU64::new(0);
    let uniq = WRITE_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut tmp_name = file_name.to_os_string();
    tmp_name.push(format!(".tmp.{}.{}", std::process::id(), uniq));
    let tmp_path = parent.join(&tmp_name);

    {
        let mut tmp = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)?;
        tmp.write_all(content)?;
        tmp.sync_all()?;
    }

    std::fs::rename(&tmp_path, path)?;

    let dir = File::open(parent)?;
    dir.sync_all()?;

    Ok(())
}

// ── bytes_hash ───────────────────────────────────────────────────────────────

/// `bytes_hash(Bytes) -> Text`
///
/// SHA-256 of a Bytes blob. Always returns exactly 71 chars: `"sha256:"`
/// + 64 lowercase hex chars. Same crypto as `content_hash`, but consumes
/// `Value::Bytes` directly without per-element list coercion.
#[track_caller]
pub fn bytes_hash(v: Value) -> Value {
    match v {
        Value::Bytes(b) => {
            let digest = Sha256::digest(&b);
            let hex: String = digest.iter().map(|byte| format!("{:02x}", byte)).collect();
            Value::Str(intern_str(&format!("sha256:{}", hex)))
        }
        other => panic!("bytes_hash: expected Bytes, got {:?}", other),
    }
}

// ── fs_mkdir_p ───────────────────────────────────────────────────────────────

/// `fs_mkdir_p(Text) -> Unit`
///
/// Recursive idempotent directory create (`std::fs::create_dir_all`). Panics
/// on any OS error.
#[track_caller]
pub fn fs_mkdir_p(v: Value) -> Value {
    let path = match v {
        Value::Str(h) => get_str(h),
        other => panic!("fs_mkdir_p: expected Text path, got {:?}", other),
    };
    if let Err(e) = std::fs::create_dir_all(&path) {
        panic!("fs_mkdir_p({}): {}", path, e);
    }
    Value::Unit
}

// ── bytes_to_text ────────────────────────────────────────────────────────────

/// `bytes_to_text(Bytes) -> Text`
///
/// Checked UTF-8 decode. Returns the decoded Text. Panics on invalid UTF-8.
/// Symmetric inverse of `text_to_bytes` for valid UTF-8 inputs.
#[track_caller]
pub fn bytes_to_text(v: Value) -> Value {
    match v {
        Value::Bytes(b) => match String::from_utf8(b) {
            Ok(s) => Value::Str(intern_str(&s)),
            Err(e) => panic!("bytes_to_text: invalid UTF-8: {}", e),
        },
        other => panic!("bytes_to_text: expected Bytes, got {:?}", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::value::intern_str;

    fn bytes(v: Value) -> Vec<u8> {
        match v {
            Value::Bytes(b) => b,
            other => panic!("expected Bytes, got {:?}", other),
        }
    }

    /// text_to_bytes already emits full UTF-8 (String::into_bytes), NOT ASCII —
    /// this regression test locks that in. AXLANG_TURN_0002 asked to "fix
    /// ASCII-only" behavior; inspection at baseline cd6cc1d6 showed the fn was
    /// already correct, so this guards the property rather than changing code.
    #[test]
    fn text_to_bytes_round_trips_full_utf8() {
        // "aé€": a=0x61, é=U+00E9 (C3 A9), €=U+20AC (E2 82 AC).
        let s = "aé€";
        let expect = vec![0x61, 0xC3, 0xA9, 0xE2, 0x82, 0xAC];
        assert_eq!(bytes(text_to_bytes(Value::Str(intern_str(s)))), expect);
        assert_eq!(s.as_bytes().to_vec(), expect, "sanity: matches Rust's own UTF-8");

        // Round-trips back through bytes_to_text unchanged.
        let back = bytes_to_text(Value::Bytes(expect));
        assert_eq!(back, Value::Str(intern_str(s)));

        // A 4-byte emoji (😀 = U+1F600 -> F0 9F 98 80) also survives.
        let emoji = "😀";
        assert_eq!(bytes(text_to_bytes(Value::Str(intern_str(emoji)))), vec![0xF0, 0x9F, 0x98, 0x80]);
    }

    /// ASCII behaviour is byte-identical to before the UTF-8 audit — no caller
    /// that fed ASCII can observe any change.
    #[test]
    fn text_to_bytes_ascii_unchanged() {
        assert_eq!(bytes(text_to_bytes(Value::Str(intern_str("abc")))), vec![0x61, 0x62, 0x63]);
    }
}

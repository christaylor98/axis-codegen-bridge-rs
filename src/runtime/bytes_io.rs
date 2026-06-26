//! BRIDGE_BYTES_IO_M1 — text_to_bytes, fs_write_bytes, fs_read_bytes.
//!
//! Three leaf foreign primitives for the M1 surface to convert Text to a
//! Bytes blob and to round-trip blobs through the filesystem.
//!
//!   * `text_to_bytes(Text) -> Bytes`
//!         UTF-8 encode the Text and wrap as `Value::Bytes`.
//!
//!   * `fs_write_bytes(path: Text, content: Bytes) -> ResultUnit`
//!         Durable write: write `<path>.tmp`, fsync the tmp file, rename
//!         atomically over `<path>`, fsync the parent directory. The parent
//!         directory fsync is not optional — without it the rename itself is
//!         not durable across crash. If the parent dir cannot be fsynced
//!         (e.g. read-only mount), surface as Err — never silently skip.
//!
//!   * `fs_read_bytes(path: Text) -> ResultBytes`
//!         `std::fs::read(path)` wrapped in Ok(Bytes) on success, Err(Text)
//!         on any OS error.
//!
//! Identities are sha256(name_utf8) — same convention as the rest of the
//! bridge leaf primitives.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

use super::value::{Value, get_str, intern_str, intern_tag};

// ── Result constructors ──────────────────────────────────────────────────────

fn ok_unit() -> Value {
    Value::Ctor { tag: intern_tag("Ok"), fields: vec![Value::Unit] }
}

fn ok_bytes(bs: Vec<u8>) -> Value {
    Value::Ctor { tag: intern_tag("Ok"), fields: vec![Value::Bytes(bs)] }
}

fn err_text(msg: String) -> Value {
    Value::Ctor { tag: intern_tag("Err"), fields: vec![Value::Str(intern_str(&msg))] }
}

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
    match write_durable(&path, &content) {
        Ok(()) => ok_unit(),
        Err(e) => err_text(format!("fs_write_bytes({}): {}", path, e)),
    }
}

// ── fs_read_bytes ────────────────────────────────────────────────────────────

#[track_caller]
pub fn fs_read_bytes(path: Value) -> Value {
    let path_str = match path {
        Value::Str(h) => get_str(h),
        other => panic!("fs_read_bytes: expected Text, got {:?}", other),
    };
    match std::fs::read(&path_str) {
        Ok(bs) => ok_bytes(bs),
        Err(e) => err_text(format!("fs_read_bytes({}): {}", path_str, e)),
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
    let mut tmp_name = file_name.to_os_string();
    tmp_name.push(".tmp");
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

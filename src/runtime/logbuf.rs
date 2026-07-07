//! BRIDGE_LOGBUF_V1 (spike:axverity-spike1) — a thin, Rust-owned durable
//! append buffer. logbuf_open / logbuf_append / logbuf_sync / logbuf_read /
//! logbuf_len.
//!
//! ## What is thin here (spike1 hard-limit THIN_SUBSTRATE)
//!
//! The bridge owns exactly three things: the in-memory BUFFER bytes, the TAIL
//! offset, and the FSYNC mechanism. It owns NOTHING else. Record framing,
//! content hashing, and commit policy (how many appends per sync) all stay in
//! M1 — that is why `logbuf_append` takes opaque `Bytes` and returns only a
//! byte offset: no notion of a "record" or a hash ever enters this file. (The
//! forbidden fat `log_append_object` would bake framing/hashing into Rust
//! here; it is deliberately absent.)
//!
//! ## The buffer model (spike1 fork: Option B — a real Vec<u8>)
//!
//!   append(h, bytes)  = buf.extend_from_slice(bytes)      // memcpy, no syscall
//!   sync(h)           = file.write_all(&buf); file.fsync(); file_len += n; buf.clear()
//!
//! Appends are a memcpy into a process-local `Vec<u8>` — no `write(2)`, no
//! `fsync(2)`. One `sync` amortizes one `write` + one `fsync` over however
//! many records were appended since the last sync. This is the property
//! Spike 1 exists to prove correct (NOT to benchmark — Spike 2 owns speed).
//! It is genuinely distinct from routing every append into the page cache via
//! a `write(2)` syscall (the rejected Option A), and `logbuf_read` proves the
//! distinction: a read BEFORE sync returns the buffered bytes while the file
//! on disk is still empty.
//!
//! ## Offsets and the logical stream
//!
//! The logical log is `file[0..file_len]` followed by `buf`. Total logical
//! length is `file_len + buf.len()`. `logbuf_append` returns the LOGICAL start
//! offset of the record it just buffered (`file_len + buf.len()` before the
//! extend) — a stable position that is still valid after the buffer is flushed
//! to the file, so M1 can record it in a pointer index. `logbuf_read(h, off,
//! len)` serves `[off, off+len)` of that logical stream from whichever side
//! (file and/or still-buffered) the range falls on, clamped at the logical
//! end (short read past EOF, matching `fs_read_range`).
//!
//! ## Durability
//!
//! `logbuf_open` fsyncs the parent directory ONCE after creating the file, so
//! the file's directory entry is durable (mirrors write_durable step 4 in
//! bytes_io.rs). Thereafter `logbuf_sync` fsyncs the file's data only. This
//! file does NOT touch the frozen `store_write` / `push_object` write path
//! (spike1 hard-limit WRITE_PATH_UNTOUCHED); it is a new parallel primitive.
//!
//! Handle registry, next-handle counter, and opaque-Int handles follow the
//! exact pattern established by net.rs. Single-writer only for Spike 1 —
//! multi-writer / per-thread handles under load are explicitly out of scope,
//! so ops hold the map lock for their (short) duration.
//!
//! Identities are sha256(name_utf8), the bridge-wide convention.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};

use super::value::{get_str, Value};

/// One open log: the append file, the Rust-owned buffer, and the count of
/// bytes already flushed to the file.
struct LogBuf {
    path: String,
    file: File,     // opened for append; target of sync's write_all + fsync
    buf: Vec<u8>,   // Rust-owned in-memory buffer (Option B)
    file_len: u64,  // bytes already written+synced to `file`
}

/// Process-global log table, keyed by integer handle (mirrors net.rs).
fn registry() -> &'static Mutex<HashMap<i64, LogBuf>> {
    static REG: OnceLock<Mutex<HashMap<i64, LogBuf>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Allocate the next opaque handle (never reused within a process run).
fn next_handle() -> i64 {
    static COUNTER: AtomicI64 = AtomicI64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// `logbuf_open(path: Text) -> Int`
///
/// Open (creating if absent) `path` for append and register a fresh buffer.
/// Fsyncs the parent directory once so the file's directory entry is durable.
/// Returns the opaque handle. Panics on any OS error.
#[track_caller]
pub fn logbuf_open(arg: Value) -> Value {
    let path = match arg {
        Value::Str(h) => get_str(h),
        other => panic!("logbuf_open: expected Text path, got {:?}", other),
    };
    let file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .unwrap_or_else(|e| panic!("logbuf_open({}): open: {}", path, e));
    let file_len = file
        .metadata()
        .unwrap_or_else(|e| panic!("logbuf_open({}): stat: {}", path, e))
        .len();

    // One-time parent-dir fsync — the new file's directory entry reaches disk.
    let parent = Path::new(&path).parent().unwrap_or_else(|| Path::new(""));
    let parent = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
    let dir = File::open(parent)
        .unwrap_or_else(|e| panic!("logbuf_open({}): open parent {:?}: {}", path, parent, e));
    if let Err(e) = dir.sync_all() {
        panic!("logbuf_open({}): parent-dir fsync: {}", path, e);
    }

    let h = next_handle();
    registry().lock().unwrap().insert(
        h,
        LogBuf { path, file, buf: Vec::new(), file_len },
    );
    Value::Int(h)
}

/// `logbuf_append(handle: Int, data: Bytes) -> Int`
///
/// Append `data` to the in-memory buffer (a memcpy — no syscall, no fsync).
/// Returns the LOGICAL start offset of this record, for M1 to frame/index.
///
/// -1 is RESERVED as a future buffer-full signal (M1 handles rotation on -1).
/// Spike 1 enforces no cap, so this never returns -1 — the Int return simply
/// must not foreclose it. Rotation itself is out of scope for Spike 1.
#[track_caller]
pub fn logbuf_append(args: Value) -> Value {
    let (h, data) = match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("logbuf_append: expected Tuple(Int, Bytes), got {:?}", other),
    };
    let h = match h {
        Value::Int(n) => n,
        other => panic!("logbuf_append: arg 0 expected Int handle, got {:?}", other),
    };
    let data = match data {
        Value::Bytes(b) => b,
        other => panic!("logbuf_append: arg 1 expected Bytes, got {:?}", other),
    };
    let mut reg = registry().lock().unwrap();
    let lb = reg
        .get_mut(&h)
        .unwrap_or_else(|| panic!("logbuf_append: unknown handle {}", h));
    let start = lb.file_len + lb.buf.len() as u64;
    lb.buf.extend_from_slice(&data);
    Value::Int(start as i64)
}

/// `logbuf_sync(handle: Int) -> Unit`
///
/// Flush the buffer to the file (one `write_all`) and fsync the file (one
/// `fsync`), then clear the buffer. A sync with an empty buffer is a no-op
/// (the file is already durable from a prior sync). Panics on any OS error.
#[track_caller]
pub fn logbuf_sync(arg: Value) -> Value {
    let h = match arg {
        Value::Int(n) => n,
        other => panic!("logbuf_sync: expected Int handle, got {:?}", other),
    };
    let mut reg = registry().lock().unwrap();
    let lb = reg
        .get_mut(&h)
        .unwrap_or_else(|| panic!("logbuf_sync: unknown handle {}", h));
    if lb.buf.is_empty() {
        return Value::Unit;
    }
    if let Err(e) = lb.file.write_all(&lb.buf) {
        panic!("logbuf_sync({}): write_all: {}", lb.path, e);
    }
    if let Err(e) = lb.file.sync_all() {
        panic!("logbuf_sync({}): fsync: {}", lb.path, e);
    }
    lb.file_len += lb.buf.len() as u64;
    lb.buf.clear();
    Value::Unit
}

/// `logbuf_read(handle: Int, offset: Int, len: Int) -> Bytes`
///
/// Read `[offset, offset+len)` of the logical stream (`file` ++ `buf`),
/// clamped at the logical end. Serves each half of the range from the side it
/// falls on, so a read is correct BEFORE a sync (from the buffer) and after
/// (from the file). Panics on unknown handle, negative bounds, or OS error.
#[track_caller]
pub fn logbuf_read(args: Value) -> Value {
    let (h, off, len) = match args {
        Value::Tuple(es) if es.len() == 3 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("logbuf_read: expected Tuple(Int, Int, Int), got {:?}", other),
    };
    let h = match h {
        Value::Int(n) => n,
        other => panic!("logbuf_read: arg 0 expected Int handle, got {:?}", other),
    };
    let off = match off {
        Value::Int(n) => n,
        other => panic!("logbuf_read: arg 1 expected Int offset, got {:?}", other),
    };
    let len = match len {
        Value::Int(n) => n,
        other => panic!("logbuf_read: arg 2 expected Int len, got {:?}", other),
    };
    if off < 0 || len < 0 {
        panic!("logbuf_read: negative offset={} or len={}", off, len);
    }
    let reg = registry().lock().unwrap();
    let lb = reg
        .get(&h)
        .unwrap_or_else(|| panic!("logbuf_read: unknown handle {}", h));

    let total = lb.file_len + lb.buf.len() as u64;
    let (off, len) = (off as u64, len as u64);
    let start = off.min(total);
    let end = off.checked_add(len).map(|e| e.min(total)).unwrap_or(total);

    let mut out: Vec<u8> = Vec::with_capacity((end - start) as usize);
    // File portion: [start, min(end, file_len)).
    if start < lb.file_len {
        let fend = end.min(lb.file_len);
        let mut f = File::open(&lb.path)
            .unwrap_or_else(|e| panic!("logbuf_read({}): reopen: {}", lb.path, e));
        f.seek(SeekFrom::Start(start))
            .unwrap_or_else(|e| panic!("logbuf_read({}): seek {}: {}", lb.path, start, e));
        let mut fb = Vec::new();
        (&mut f)
            .take(fend - start)
            .read_to_end(&mut fb)
            .unwrap_or_else(|e| panic!("logbuf_read({}): read: {}", lb.path, e));
        out.extend_from_slice(&fb);
    }
    // Buffer portion: the part of [start, end) at or past file_len.
    if end > lb.file_len {
        let bstart = (start.max(lb.file_len) - lb.file_len) as usize;
        let bend = (end - lb.file_len) as usize;
        out.extend_from_slice(&lb.buf[bstart..bend]);
    }
    Value::Bytes(out)
}

/// `logbuf_len(handle: Int) -> Int`
///
/// Current total logical length: bytes synced to the file plus bytes still
/// buffered. Panics on unknown handle.
#[track_caller]
pub fn logbuf_len(arg: Value) -> Value {
    let h = match arg {
        Value::Int(n) => n,
        other => panic!("logbuf_len: expected Int handle, got {:?}", other),
    };
    let reg = registry().lock().unwrap();
    let lb = reg
        .get(&h)
        .unwrap_or_else(|| panic!("logbuf_len: unknown handle {}", h));
    Value::Int((lb.file_len + lb.buf.len() as u64) as i64)
}

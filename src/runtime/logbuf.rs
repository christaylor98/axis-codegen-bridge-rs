//! BRIDGE_LOGBUF_V1 (spike:axverity-spike1) — a thin, Rust-owned durable
//! append buffer. logbuf_open / logbuf_append / logbuf_sync / logbuf_read /
//! logbuf_len / logbuf_flush (AXVERITY_PGSERVER_FAST_MODE — write without
//! fsync, see logbuf_flush's own doc comment for why this is needed).
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
//! Appends are a memcpy into a thread-local `Vec<u8>` — no `write(2)`, no
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
//! durability sequence (`write_all` → `sync_all` → advance `file_len` → clear
//! the buffer) is the exact frontier Spike 3 proved STRONG (zero acknowledged
//! loss, never-corrupt, forward hash-checked recovery) and is UNCHANGED by the
//! thread-owned storage model below.
//!
//! ## Storage model — THREAD-OWNED buffers, NO shared registry
//!   (AXVERITY_WRITE_PATH_INTEGRATION Landing 1: NO_SHARED_REGISTRY /
//!    NO_ARC_MUTEX_ON_BUFFERS / THREAD_OWNED_BUFFERS_NO_REGISTRY)
//!
//! Each open `LogBuf` lives in **thread-local storage** (`LOGS`), reachable
//! ONLY by the thread that opened it. There is no process-global handle→buffer
//! registry and no lock anywhere on the append/sync/read path: `logbuf_append`
//! and `logbuf_sync` touch nothing but the calling thread's own `Vec<u8>` and
//! `File`. Two writer threads therefore share NOTHING and can never contend.
//!
//! The handle counter is thread-local too (`NEXT`, starting at 1 per thread),
//! so a `--entries` writer thread behaves *identically* to an independent
//! process: its first `logbuf_open` returns handle 1, exactly as a fresh
//! process's does. That is the whole point of Landing 1 — the N-process
//! reference curve (each process sharing nothing) is the target the thread
//! curve must now match, and it can only match it if threads share nothing
//! either. No `Mutex`, no `RwLock`, no `Arc`, no process-global atomic sits on
//! the hot path; the append path shares nothing and locks nothing.
//!
//! ### Why this is the ABI-correct realization of "thread-owned"
//!
//! The M1↔Rust boundary dispatches `logbuf_*` as free functions over the
//! `Value` ABI (an `Int` handle in, a `Value` out); there is no way to *move* a
//! Rust-owned `Vec<u8>` into an M1-visible thread closure, so "the buffer is a
//! local variable of the writer thread" cannot be expressed directly. Two
//! candidates were rejected: a `HashMap<i64, LogBuf>` behind a raw pointer
//! carried as the handle (`unsafe`, UB the instant two threads alias a handle,
//! and a disguised shared-mutable-state hazard); and a shared lock-free/sharded
//! map (still shared state, and NO_ARC_MUTEX_ON_BUFFERS forbids a "cleverer
//! lock"). Thread-local storage is the safe realization of "thread-OWNED,
//! reachable only by its owning thread": the handle indexes a per-thread table,
//! and a handle used on the wrong thread fails fast (unknown handle) rather than
//! aliasing another thread's buffer.
//!
//! Identities are sha256(name_utf8), the bridge-wide convention.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use super::value::{get_str, Value};

/// One open log: the append file, the Rust-owned buffer, and the count of
/// bytes already flushed to the file.
struct LogBuf {
    path: String,
    file: File,     // opened for append; target of sync's write_all + fsync
    buf: Vec<u8>,   // thread-owned in-memory buffer (Option B)
    file_len: u64,  // bytes already written+synced to `file`
}

thread_local! {
    /// Per-thread log table, keyed by integer handle. THREAD-LOCAL, never
    /// shared: reachable only by the thread that opened the handle. This is the
    /// "thread-owned buffers, no shared registry" storage — the append path
    /// touches only this thread's own map, so no two writer threads ever
    /// contend and no lock is taken anywhere on the hot path.
    static LOGS: RefCell<HashMap<i64, LogBuf>> = RefCell::new(HashMap::new());

    /// Per-thread handle counter (never reused within a thread). Thread-local so
    /// each writer thread's first `logbuf_open` returns handle 1 — identical to
    /// a fresh process — and no process-global atomic sits on the open path.
    static NEXT: Cell<i64> = const { Cell::new(1) };
}

/// Allocate the next opaque handle for the calling thread.
fn next_handle() -> i64 {
    NEXT.with(|c| {
        let n = c.get();
        c.set(n + 1);
        n
    })
}

/// `logbuf_open(path: Text) -> Int`
///
/// Open (creating if absent) `path` for append and register a fresh buffer in
/// THIS thread's local table. Fsyncs the parent directory once so the file's
/// directory entry is durable. Returns the opaque handle. Panics on any OS
/// error.
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
    LOGS.with(|logs| {
        logs.borrow_mut().insert(
            h,
            LogBuf { path, file, buf: Vec::new(), file_len },
        );
    });
    Value::Int(h)
}

/// `logbuf_append(handle: Int, data: Bytes) -> Int`
///
/// Append `data` to the calling thread's in-memory buffer (a memcpy — no
/// syscall, no fsync, no lock). Returns the LOGICAL start offset of this
/// record, for M1 to frame/index.
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
    LOGS.with(|logs| {
        let mut logs = logs.borrow_mut();
        let lb = logs
            .get_mut(&h)
            .unwrap_or_else(|| panic!("logbuf_append: unknown handle {} (not opened on this thread)", h));
        let start = lb.file_len + lb.buf.len() as u64;
        lb.buf.extend_from_slice(&data);
        Value::Int(start as i64)
    })
}

/// `logbuf_sync(handle: Int) -> Unit`
///
/// Flush the calling thread's buffer to its file (one `write_all`) and fsync
/// the file (one `fsync`), then clear the buffer. A sync with an empty buffer
/// is a no-op (the file is already durable from a prior sync). No lock is held
/// across the fsync — the file and buffer are thread-owned. Panics on any OS
/// error.
#[track_caller]
pub fn logbuf_sync(arg: Value) -> Value {
    let h = match arg {
        Value::Int(n) => n,
        other => panic!("logbuf_sync: expected Int handle, got {:?}", other),
    };
    LOGS.with(|logs| {
        let mut logs = logs.borrow_mut();
        let lb = logs
            .get_mut(&h)
            .unwrap_or_else(|| panic!("logbuf_sync: unknown handle {} (not opened on this thread)", h));
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
    })
}

/// `logbuf_flush(handle: Int) -> Unit`
///
/// AXVERITY_PGSERVER_FAST_MODE. Flush the calling thread's buffer to its file
/// (one `write_all`) WITHOUT fsyncing it, then clear the buffer — the
/// write-half of `logbuf_sync` with the fsync half removed. This is the
/// primitive FAST mode needs and `logbuf_append` alone cannot provide:
/// `append` is a pure in-memory memcpy (no syscall at all, see the module
/// doc), so skipping `sync` entirely — the naive first attempt at FAST mode —
/// silently drops the data instead of merely deferring its fsync (confirmed
/// empirically: a 0-byte WAL segment after a "successful" fast-mode INSERT).
/// `logbuf_flush` closes that gap: after this returns, the bytes have reached
/// the OS via `write(2)` and sit in the page cache — durable across the
/// writing process's own crash/SIGKILL (the turn-0012 crash-test property),
/// but not across a real power loss/OS crash before some later `fsync`
/// reclaims them. This is exactly axVerity FAST mode's specified guarantee
/// (specs/axverity-durability-model-payload-wal.md §9: "ack on landing in the
/// buffer before sync... survives OS crash, loses a bounded window on power
/// loss, never corrupt") — no more, no less. A sync with an empty buffer is a
/// no-op, same as `logbuf_sync`. Panics on any OS error.
#[track_caller]
pub fn logbuf_flush(arg: Value) -> Value {
    let h = match arg {
        Value::Int(n) => n,
        other => panic!("logbuf_flush: expected Int handle, got {:?}", other),
    };
    LOGS.with(|logs| {
        let mut logs = logs.borrow_mut();
        let lb = logs
            .get_mut(&h)
            .unwrap_or_else(|| panic!("logbuf_flush: unknown handle {} (not opened on this thread)", h));
        if lb.buf.is_empty() {
            return Value::Unit;
        }
        if let Err(e) = lb.file.write_all(&lb.buf) {
            panic!("logbuf_flush({}): write_all: {}", lb.path, e);
        }
        lb.file_len += lb.buf.len() as u64;
        lb.buf.clear();
        Value::Unit
    })
}

/// `wal_fast_batch_write(batch: Value) -> Value`
///
/// AXVERITY_PGSERVER_FAST_MODE — the janitor's `wait()` handler. `wait`
/// drains every message pending on a subscribed channel into one
/// `Value::List` and calls its handler with that list ONCE (see
/// channels.rs); this IS that handler, so it must be `fn(Value) -> Value` —
/// the same reason `logbuf_flush` had to be a bridge primitive rather than
/// M1 source (the M1 surface parser has no bare-`Value`-param wall, contrary
/// to an earlier finding in this codebase — `Value(Bytes)`/`ValueList(Bytes)`
/// tagged forms parse fine — but `wait`'s OWN handler slot is a raw untagged
/// `fn(Value) -> Value` at the Rust ABI level, which only a bridge built-in
/// can satisfy directly without an extra M1-level wrapper hop).
///
/// Each item in the batch is a `Value::Bytes` — an already-framed,
/// already-immutable WAL record that will never be mutated again. Because it
/// can never change, a copy is never needed to keep it safe: this writes
/// each item's OWN backing buffer straight to the file via `write_all`, with
/// NO intermediate accumulation into a second buffer first. This
/// deliberately bypasses `LogBuf.buf` (the `logbuf_append`/`logbuf_sync`
/// accumulator) — appending each item there first would `extend_from_slice`
/// (memcpy) it into a second copy for no reason, since the batch already
/// arrived as a `Vec` of complete, final, owned buffers in one shot. One
/// `fsync` covers the whole batch — real group commit, sized by however many
/// frames a single `wait()` call happened to drain.
///
/// Handle 1 is a deliberate, documented convention, not a magic number: the
/// janitor thread's FIRST `logbuf_open` call (at startup, before this
/// handler is ever reachable) is guaranteed to return handle 1 — thread-local
/// handle counters start at 1 per thread (see `next_handle`/`NEXT` above).
/// `wait`'s handler signature has no room for a handle argument, so a fixed,
/// pre-established handle is the only option — same discipline as
/// `wal_put_fast.m1`'s fixed shard-string convention elsewhere in this spike.
/// Panics on any OS error, or if the batch contains a non-Bytes item.
#[track_caller]
pub fn wal_fast_batch_write(arg: Value) -> Value {
    let items = match arg {
        Value::List(items) => items,
        other => panic!("wal_fast_batch_write: expected List, got {:?}", other),
    };
    if items.is_empty() {
        return Value::Unit;
    }
    LOGS.with(|logs| {
        let mut logs = logs.borrow_mut();
        let lb = logs.get_mut(&1).unwrap_or_else(|| {
            panic!("wal_fast_batch_write: handle 1 not open on this thread (janitor must logbuf_open first)")
        });
        let mut total: u64 = 0;
        for item in &items {
            let bytes = match item {
                Value::Bytes(b) => b,
                other => panic!("wal_fast_batch_write: expected Bytes item, got {:?}", other),
            };
            if let Err(e) = lb.file.write_all(bytes) {
                panic!("wal_fast_batch_write({}): write_all: {}", lb.path, e);
            }
            total += bytes.len() as u64;
        }
        if let Err(e) = lb.file.sync_all() {
            panic!("wal_fast_batch_write({}): fsync: {}", lb.path, e);
        }
        lb.file_len += total;
        Value::Unit
    })
}

/// `logbuf_read(handle: Int, offset: Int, len: Int) -> Bytes`
///
/// Read `[offset, offset+len)` of the logical stream (`file` ++ `buf`),
/// clamped at the logical end. Serves each half of the range from the side it
/// falls on, so a read is correct BEFORE a sync (from the buffer) and after
/// (from the file). Operates on the calling thread's own buffer/file. Panics on
/// unknown handle, negative bounds, or OS error.
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
    LOGS.with(|logs| {
        let logs = logs.borrow();
        let lb = logs
            .get(&h)
            .unwrap_or_else(|| panic!("logbuf_read: unknown handle {} (not opened on this thread)", h));

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
    })
}

/// `logbuf_len(handle: Int) -> Int`
///
/// Current total logical length: bytes synced to the file plus bytes still
/// buffered, for the calling thread's handle. Panics on unknown handle.
#[track_caller]
pub fn logbuf_len(arg: Value) -> Value {
    let h = match arg {
        Value::Int(n) => n,
        other => panic!("logbuf_len: expected Int handle, got {:?}", other),
    };
    LOGS.with(|logs| {
        let logs = logs.borrow();
        let lb = logs
            .get(&h)
            .unwrap_or_else(|| panic!("logbuf_len: unknown handle {} (not opened on this thread)", h));
        Value::Int((lb.file_len + lb.buf.len() as u64) as i64)
    })
}

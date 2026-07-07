//! BRIDGE_WAL_SEG_ALLOC_V1 (AXVERITY_WAL_ALLOCATION_AND_BLOB_PATH, Landing A) —
//! pre-allocated fixed-size WAL segment files.
//!
//! `req-wal-segment-allocation`: the disk-allocation unit for the WAL is a
//! pre-allocated, fixed-size segment file — `fallocate(2)` reserves the whole
//! extent up front so every append after creation is a pure data write (no
//! per-append block-map metadata churn, no fragmentation), instead of a file
//! that grows byte-by-byte.
//!
//! ## Why `FALLOC_FL_KEEP_SIZE` and not `ftruncate`-to-full (the one-way door)
//!
//! `KEEP_SIZE` reserves the disk blocks for `[0, size)` but leaves the file's
//! logical length (`st_size`) equal to the bytes actually written. So **EOF
//! still marks the true data frontier**, and the Spike-3 forward-scan recovery
//! (short read of the 74-byte header ⇒ frontier) is byte-identical to the
//! non-preallocated path: the pre-allocated-but-unwritten region is never read
//! (it is past EOF). `ftruncate`-to-full would inflate `st_size`, break
//! `logbuf`'s append/EOF model, and force a zero-scan recovery — a durability
//! risk we decline (priority: preserve-proven-properties HIGH, durability
//! regression UNACCEPTABLE). `KEEP_SIZE` also lets `logbuf.rs` stay untouched:
//! `fs_prealloc` is a separate call the M1 writer makes before `logbuf_open`.
//!
//! Best-effort per filesystem: a filesystem without `fallocate` support
//! (`EOPNOTSUPP`/`ENOSYS`, e.g. some tmpfs) degrades to a no-op — the segment
//! simply append-grows, correctness preserved, only the metadata-churn
//! optimization unavailable there.

use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::path::Path;

use super::value::{get_str, Value};

/// Reserve `size` bytes of extent for `path` via `fallocate(KEEP_SIZE)`,
/// creating the file if absent. Returns true if the reservation was applied,
/// false if the filesystem does not support it (a tolerated no-op).
fn prealloc_file(path: &str, size: i64) -> bool {
    let f = OpenOptions::new()
        .create(true)
        .write(true)
        .open(path)
        .unwrap_or_else(|e| panic!("fs_prealloc({}): open: {}", path, e));
    if size <= 0 {
        return true;
    }
    // FALLOC_FL_KEEP_SIZE = 0x01: reserve blocks without changing st_size.
    let ret = unsafe { libc::fallocate(f.as_raw_fd(), libc::FALLOC_FL_KEEP_SIZE, 0, size as libc::off_t) };
    if ret == 0 {
        return true;
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        // Filesystem doesn't support fallocate — tolerate (append-grow fallback).
        Some(libc::EOPNOTSUPP) | Some(libc::ENOSYS) => false,
        _ => panic!("fs_prealloc({}, {}): fallocate: {}", path, size, err),
    }
}

/// `fs_prealloc(path: Text, size: Int) -> Unit`
///
/// Create `path` if needed and reserve a `size`-byte extent (KEEP_SIZE). Idempotent
/// (re-reserving an existing segment is harmless). Panics on a genuine OS error;
/// a filesystem that lacks `fallocate` is a tolerated no-op.
#[track_caller]
pub fn fs_prealloc(args: Value) -> Value {
    let (path, size) = match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("fs_prealloc: expected Tuple(Text, Int), got {:?}", other),
    };
    let path = match path {
        Value::Str(h) => get_str(h),
        other => panic!("fs_prealloc: arg 0 expected Text, got {:?}", other),
    };
    let size = match size {
        Value::Int(n) => n,
        other => panic!("fs_prealloc: arg 1 expected Int, got {:?}", other),
    };
    prealloc_file(&path, size);
    Value::Unit
}

const DEFAULT_SEG_BYTES: i64 = 64 * 1024 * 1024;

fn seg_size_env() -> i64 {
    std::env::var("AXVERITY_WAL_SEG_BYTES")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_SEG_BYTES)
}

fn seg_path(prefix: &str, seq: i64) -> String {
    format!("{}{}.log", prefix, seq)
}

/// `wal_write_seg(seg_prefix: Text, frame_size: Int) -> Int`
///
/// Pick (and pre-allocate) the WAL segment sequence number the next frame of
/// `frame_size` bytes should be appended to, for the shard whose segments are
/// `<seg_prefix><seq>.log`. Rotation: if the active (highest-`seq`) segment is
/// non-empty and `current_size + frame_size` would exceed the segment size
/// (env `AXVERITY_WAL_SEG_BYTES`, default 64 MiB), mint `seq+1`. A frame is
/// never split across segments — we rotate *before* the frame that would
/// overflow — so each frame is wholly within one segment. The returned segment
/// is always pre-allocated (KEEP_SIZE) before return. Returns the sequence Int.
#[track_caller]
pub fn wal_write_seg(args: Value) -> Value {
    let (prefix, frame_size) = match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("wal_write_seg: expected Tuple(Text, Int), got {:?}", other),
    };
    let prefix = match prefix {
        Value::Str(h) => get_str(h),
        other => panic!("wal_write_seg: arg 0 expected Text, got {:?}", other),
    };
    let frame_size = match frame_size {
        Value::Int(n) => n,
        other => panic!("wal_write_seg: arg 1 expected Int, got {:?}", other),
    };
    let seg_size = seg_size_env();

    // Highest existing segment sequence (or -1 if none yet).
    let mut hi: i64 = -1;
    let mut k: i64 = 0;
    while Path::new(&seg_path(&prefix, k)).exists() {
        hi = k;
        k += 1;
    }

    let chosen: i64 = if hi < 0 {
        0
    } else {
        let cur = &seg_path(&prefix, hi);
        let cur_size = std::fs::metadata(cur).map(|m| m.len() as i64).unwrap_or(0);
        if cur_size > 0 && cur_size + frame_size > seg_size {
            hi + 1
        } else {
            hi
        }
    };
    prealloc_file(&seg_path(&prefix, chosen), seg_size);
    Value::Int(chosen)
}

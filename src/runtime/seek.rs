//! BRIDGE_SEEK_V1 — fs_read_range(path, offset, len) -> Bytes.
//!
//! Range read via seek. Opens `path`, seeks to `offset`, and returns up to
//! `len` bytes. Unlike `bytes_io::fs_read_bytes` (whole-file read) this does
//! NOT read past `[offset, offset+len)`, so a caller holding a pointer index
//! (pack tier, log tier) pays O(len) rather than O(file-size) — this is the
//! seek primitive turn:axverity:0013/0014 identified as the real fix for
//! pack_read's O(pack-size) warm-read cost.
//!
//! Over-EOF is CLAMPED to the available bytes (a short read, the standard
//! read(2) semantics), NOT a panic: `fs_read_range(p, off, len)` equals
//! `whole_file_read(p)[off .. off+len]` under clamping-slice semantics, which
//! is what makes SPIKE1_SEEK's "equals a whole-file slice" assertion hold
//! exactly against a Python/dd reference slice (Python slicing clamps too).
//! NOTE: `bytes_codec::bytes_slice` instead PANICS on an over-range end; this
//! primitive deliberately clamps. The two agree exactly for every in-bounds
//! range — i.e. every real call site that reads a known (offset,len) from an
//! index — and only differ past EOF.
//!
//! THIN substrate (spike1 hard-limit THIN_SUBSTRATE): raw bytes only. No
//! record framing, no hashing, no length prefixes — the caller (M1) owns all
//! of that. Identity is sha256("fs_read_range"), the bridge-wide convention.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use super::value::{get_str, Value};

/// `fs_read_range(path: Text, offset: Int, len: Int) -> Bytes`
///
/// Panics on: non-Tuple/arity mismatch, wrong arg types, negative offset/len,
/// or an OS error opening/seeking/reading `path` (same panic-on-OS-error
/// discipline as the rest of the fs surface — surfaces to M1 as a process
/// abort rather than a silent empty read).
#[track_caller]
pub fn fs_read_range(args: Value) -> Value {
    let (path, offset, len) = match args {
        Value::Tuple(es) if es.len() == 3 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("fs_read_range: expected Tuple(Text, Int, Int), got {:?}", other),
    };
    let path = match path {
        Value::Str(h) => get_str(h),
        other => panic!("fs_read_range: arg 0 expected Text, got {:?}", other),
    };
    let offset = match offset {
        Value::Int(n) => n,
        other => panic!("fs_read_range: arg 1 expected Int offset, got {:?}", other),
    };
    let len = match len {
        Value::Int(n) => n,
        other => panic!("fs_read_range: arg 2 expected Int len, got {:?}", other),
    };
    if offset < 0 || len < 0 {
        panic!("fs_read_range: negative offset={} or len={}", offset, len);
    }

    let mut f = match File::open(&path) {
        Ok(f) => f,
        Err(e) => panic!("fs_read_range({}): open: {}", path, e),
    };
    if let Err(e) = f.seek(SeekFrom::Start(offset as u64)) {
        panic!("fs_read_range({}): seek to {}: {}", path, offset, e);
    }
    // `take(len)` bounds the read to at most `len` bytes and stops at EOF,
    // giving the clamp-at-EOF (short read) semantics documented above.
    let mut buf = Vec::new();
    if let Err(e) = f.take(len as u64).read_to_end(&mut buf) {
        panic!("fs_read_range({}): read len={} at offset={}: {}", path, len, offset, e);
    }
    Value::Bytes(buf)
}

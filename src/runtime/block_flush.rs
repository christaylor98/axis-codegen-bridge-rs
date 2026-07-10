//! BLOCK_FLUSH_V1 (AXVERITY_FRONTEND_WRITEPATH_INTEGRATION_V1) — the async seal
//! flush worker's `wait()` handler. This is the OFF-THREAD half of the live-path
//! seal: the sealing INSERT (lib/pg_hotblk_write.m1) does only the cheap,
//! in-memory work inline (manifest line, Active->Sealed CAS, a bounded memcpy of
//! the block out of the arena, reclaim), then fires the block's bytes at THIS
//! worker as a fire-and-forget `channel_send`. The worker owns the two expensive
//! / order-sensitive steps:
//!
//!   1. `fs_write_bytes`-equivalent DURABLE write of `<flush_dir>/block-<seq>.bin`
//!      (the 4 MiB flush) — moved off the request thread so a client INSERT never
//!      blocks on disk I/O (the defect this landing exists to fix: the seal check
//!      is inline+non-blocking, but the flush it triggers must NOT be).
//!   2. AFTER the .bin is durable, `channel_send("index-frame", descriptor)` — the
//!      seal->indexer notify. This ordering is LOAD-BEARING: the frozen
//!      `index_build_batch` (indexer.rs) READS `block-<seq>.bin` and fail-stops if
//!      it is missing, so the notify must follow the durable write. That is why
//!      the inline path cannot fire `index-frame` itself and this worker does.
//!
//! ## Why this is a Rust builtin (not M1)
//!
//! It is a `wait()` handler, and `wait`'s handler slot is a raw
//! `fn(Value) -> Value` at the Rust ABI (channels.rs; CLAUDE.md §15) — an M1
//! composite cannot fill it, the same forced split as `index_build_batch` /
//! `wal_fast_batch_write` / `hotmem_write`. This is I/O GLUE (frame-parse +
//! durable write + notify), NOT seal logic — all seal logic stays the spike's M1
//! fns. `channel_send` here targets `index-frame` (≠ the `hotmem-frame` this
//! worker drains), and `wait()` releases every channel-queue lock before invoking
//! this handler (channels.rs:183), so the re-entrant send never deadlocks.
//!
//! ## Substrate: this REPLACES `hotmem_write` on the repurposed `hotmem-frame`
//! thread. Decision 2 of this landing removed the INSERT path's publish to the
//! (known-unsound) `hotmem.rs` arena; that freed the `hotmem-frame` channel and
//! its `--entries` janitor thread, which now drain framed seal-flush jobs instead
//! — no new channel, no new thread, and the last live coupling to `hotmem.rs`
//! gone.
//!
//! ## Job encoding (dual, both accepted)
//!
//!   * `Value::Ctor`/`Tuple` with 5 fields `(block_seq: Int, flush_dir: Text,
//!     byte_len: Int, idx_cell: Int, bytes: Bytes)` — the no-copy shape (the block
//!     bytes ride as a moved field).
//!   * a single `Value::Bytes` frame `"<seq>\t<dir>\t<len>\t<cell>\n" ++ <bytes>`
//!     — the fallback if M1 will not typecheck a `Bytes` element in a `Value(..)`
//!     ctor. The header ends at the FIRST `\n`; block bytes (which may contain any
//!     byte) follow verbatim and are validated against `byte_len`.
//!
//! Identities are sha256(name_utf8), the bridge-wide convention.

use std::io::Write;

use super::value::{get_str, intern_str, Value};

/// One parsed, ready-to-write flush job.
struct Job {
    block_seq: i64,
    flush_dir: String,
    byte_len: i64,
    idx_cell: i64,
    bytes: Vec<u8>,
}

fn as_int(field: &'static str, v: Value) -> i64 {
    match v {
        Value::Int(n) => n,
        other => panic!("block_flush_write: {} expected Int, got {:?}", field, other),
    }
}

fn as_text(field: &'static str, v: Value) -> String {
    match v {
        Value::Str(h) => get_str(&h),
        other => panic!("block_flush_write: {} expected Text, got {:?}", field, other),
    }
}

fn as_bytes(field: &'static str, v: Value) -> Vec<u8> {
    match v {
        Value::Bytes(b) => b,
        other => panic!("block_flush_write: {} expected Bytes, got {:?}", field, other),
    }
}

/// Parse a single framed-Bytes job: `"<seq>\t<dir>\t<len>\t<cell>\n" ++ <bytes>`.
fn parse_framed(frame: Vec<u8>) -> Job {
    let nl = frame
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or_else(|| panic!("block_flush_write: framed job has no header terminator"));
    let header = std::str::from_utf8(&frame[..nl])
        .unwrap_or_else(|e| panic!("block_flush_write: header not UTF-8: {}", e));
    let mut it = header.split('\t');
    let block_seq: i64 = it
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("block_flush_write: bad block_seq in header {:?}", header));
    let flush_dir = it
        .next()
        .unwrap_or_else(|| panic!("block_flush_write: missing flush_dir in header {:?}", header))
        .to_string();
    let byte_len: i64 = it
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("block_flush_write: bad byte_len in header {:?}", header));
    let idx_cell: i64 = it
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("block_flush_write: bad idx_cell in header {:?}", header));
    let bytes = frame[nl + 1..].to_vec();
    Job { block_seq, flush_dir, byte_len, idx_cell, bytes }
}

/// Parse one drained channel item into a Job (Ctor/Tuple 5-field, or framed Bytes).
fn parse_job(item: Value) -> Job {
    match item {
        Value::Bytes(frame) => parse_framed(frame),
        Value::Ctor { fields, .. } => job_from_fields("Ctor", fields),
        Value::Tuple(es) => job_from_fields("Tuple", es),
        other => panic!(
            "block_flush_write: expected a 5-field job (Ctor/Tuple) or a framed Bytes, got {:?}",
            other
        ),
    }
}

fn job_from_fields(shape: &'static str, fields: Vec<Value>) -> Job {
    if fields.len() != 5 {
        panic!(
            "block_flush_write: {} job must have 5 fields (block_seq, flush_dir, byte_len, idx_cell, bytes), got {}",
            shape,
            fields.len()
        );
    }
    let mut it = fields.into_iter();
    let block_seq = as_int("block_seq", it.next().unwrap());
    let flush_dir = as_text("flush_dir", it.next().unwrap());
    let byte_len = as_int("byte_len", it.next().unwrap());
    let idx_cell = as_int("idx_cell", it.next().unwrap());
    let bytes = as_bytes("bytes", it.next().unwrap());
    Job { block_seq, flush_dir, byte_len, idx_cell, bytes }
}

/// Durable, atomic write of `<flush_dir>/block-<seq>.bin` — tmp + fsync + rename
/// + parent-dir fsync, matching bytes_io.rs / fs_write_bytes semantics so a
/// SIGKILL mid-flush never leaves a torn `.bin` the indexer would read (the
/// indexer fail-stops on a short read; content is still durable in the reclog).
fn write_bin_durable(flush_dir: &str, block_seq: i64, byte_len: i64, bytes: &[u8]) {
    if byte_len >= 0 && bytes.len() != byte_len as usize {
        panic!(
            "block_flush_write: block-{} carries {} bytes, seal recorded byte_len={} (torn hand-off?)",
            block_seq,
            bytes.len(),
            byte_len
        );
    }
    std::fs::create_dir_all(flush_dir)
        .unwrap_or_else(|e| panic!("block_flush_write: mkdir {}: {}", flush_dir, e));
    let path = format!("{}/block-{}.bin", flush_dir, block_seq);
    let tmp = format!("{}/block-{}.bin.tmp.{}", flush_dir, block_seq, std::process::id());
    {
        let mut f = std::fs::File::create(&tmp)
            .unwrap_or_else(|e| panic!("block_flush_write: create {}: {}", tmp, e));
        f.write_all(bytes)
            .unwrap_or_else(|e| panic!("block_flush_write: write {}: {}", tmp, e));
        f.sync_all()
            .unwrap_or_else(|e| panic!("block_flush_write: fsync {}: {}", tmp, e));
    }
    std::fs::rename(&tmp, &path)
        .unwrap_or_else(|e| panic!("block_flush_write: rename {} -> {}: {}", tmp, path, e));
    // Parent-dir fsync so the rename (the .bin's directory entry) is durable.
    if let Ok(dir) = std::fs::File::open(flush_dir) {
        let _ = dir.sync_all();
    }
}

/// `block_flush_write(arg: Value) -> Value` — the `wait()` handler.
///
/// Drains a `List` of framed seal-flush jobs (or a bare job on the direct-call
/// path), durably writes each block's `.bin`, then fires the `index-frame`
/// seal->indexer notify per block AFTER its `.bin` is durable. Returns
/// `Int(n)` = blocks flushed this call.
#[track_caller]
pub fn block_flush_write(arg: Value) -> Value {
    let items = match arg {
        Value::List(items) => items,
        Value::Unit => return Value::Unit,
        bare @ (Value::Bytes(_) | Value::Ctor { .. } | Value::Tuple(_)) => vec![bare],
        other => panic!(
            "block_flush_write: expected List of jobs or a bare job, got {:?}",
            other
        ),
    };
    let mut n = 0i64;
    for item in items {
        let job = parse_job(item);
        write_bin_durable(&job.flush_dir, job.block_seq, job.byte_len, &job.bytes);
        // Seal->indexer notify, AFTER the .bin is durable. Descriptor shape is
        // exactly what index_build_batch's unpack_descriptor accepts (4 fields).
        let descriptor = Value::Tuple(vec![
            Value::Int(job.block_seq),
            Value::Str(intern_str(&job.flush_dir)),
            Value::Int(job.byte_len),
            Value::Int(job.idx_cell),
        ]);
        super::channels::channel_send(Value::Tuple(vec![
            Value::Str(intern_str("index-frame")),
            descriptor,
        ]));
        n += 1;
    }
    Value::Int(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};

    fn unique_dir(tag: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!("axv-flush-{}-{}-{}", tag, std::process::id(), nanos));
        std::fs::create_dir_all(&d).unwrap();
        d.to_string_lossy().into_owned()
    }

    fn ctor_job(seq: i64, dir: &str, bytes: &[u8], cell: i64) -> Value {
        Value::Ctor {
            tag: 0,
            fields: vec![
                Value::Int(seq),
                Value::Str(intern_str(dir)),
                Value::Int(bytes.len() as i64),
                Value::Int(cell),
                Value::Bytes(bytes.to_vec()),
            ],
        }
    }

    fn framed_job(seq: i64, dir: &str, bytes: &[u8], cell: i64) -> Value {
        let mut frame = format!("{}\t{}\t{}\t{}\n", seq, dir, bytes.len(), cell).into_bytes();
        frame.extend_from_slice(bytes);
        Value::Bytes(frame)
    }

    #[test]
    fn ctor_job_writes_durable_bin() {
        let dir = unique_dir("ctor");
        let payload = b"block-0-contents-ctor".to_vec();
        let cell: &'static AtomicI64 = Box::leak(Box::new(AtomicI64::new(0)));
        let addr = cell as *const AtomicI64 as i64;
        let out = block_flush_write(Value::List(vec![ctor_job(0, &dir, &payload, addr)]));
        assert_eq!(out, Value::Int(1));
        let got = std::fs::read(format!("{}/block-0.bin", dir)).unwrap();
        assert_eq!(got, payload);
    }

    #[test]
    fn framed_job_writes_durable_bin() {
        let dir = unique_dir("framed");
        // include a newline and a tab INSIDE the block bytes to prove the header
        // split (first \n only) is robust to binary payloads.
        let payload = b"line1\nline2\tcol\x00\xff".to_vec();
        let out = block_flush_write(Value::List(vec![framed_job(3, &dir, &payload, 0)]));
        assert_eq!(out, Value::Int(1));
        let got = std::fs::read(format!("{}/block-3.bin", dir)).unwrap();
        assert_eq!(got, payload);
    }

    #[test]
    fn batch_of_mixed_shapes() {
        let dir = unique_dir("mixed");
        let a = b"aaaa".to_vec();
        let b = b"bbbbbb".to_vec();
        let out = block_flush_write(Value::List(vec![
            ctor_job(0, &dir, &a, 0),
            framed_job(1, &dir, &b, 0),
        ]));
        assert_eq!(out, Value::Int(2));
        assert_eq!(std::fs::read(format!("{}/block-0.bin", dir)).unwrap(), a);
        assert_eq!(std::fs::read(format!("{}/block-1.bin", dir)).unwrap(), b);
    }

    #[test]
    fn empty_drain_is_noop() {
        assert_eq!(block_flush_write(Value::Unit), Value::Unit);
        assert_eq!(block_flush_write(Value::List(vec![])), Value::Int(0));
    }

    #[test]
    #[should_panic(expected = "torn hand-off")]
    fn byte_len_mismatch_fails_stop() {
        let dir = unique_dir("torn");
        let frame = {
            let mut f = format!("0\t{}\t{}\t0\n", dir, 999).into_bytes();
            f.extend_from_slice(b"short");
            Value::Bytes(f)
        };
        block_flush_write(Value::List(vec![frame]));
    }

    #[test]
    fn four_mib_block_roundtrips() {
        let dir = unique_dir("4mib");
        let payload: Vec<u8> = (0..4 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
        block_flush_write(Value::List(vec![ctor_job(9, &dir, &payload, 0)]));
        assert_eq!(std::fs::read(format!("{}/block-9.bin", dir)).unwrap(), payload);
    }
}

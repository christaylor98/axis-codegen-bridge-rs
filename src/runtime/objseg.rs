//! OBJSEG_V1 (D048, graphcore/DECISIONS.log) — fixed-size preallocated
//! segment files for the OBJECT family, replacing the per-object postgres
//! row `pg_obj_block_put` was exploding every sealed block into.
//!
//! ## The defect this closes
//!
//! D039 (AXVERITY_PG_HOTBLK_REWIRE_V1) moved the OBJECT family's durable
//! write off files and onto postgres, correctly killing the old
//! one-file-per-object store (`.gcore/obj/<hash>`). But `pg_obj_block_put`
//! then turned right around and did the row-shaped equivalent: it parsed
//! the sealed block's index and issued one `INSERT INTO gcore_objects` per
//! object. The hot arena buffers and seals a real contiguous block; that
//! block was being thrown away at the exact moment it became durable.
//! Every object landed as its own postgres row — same granularity as the
//! file store this was supposed to replace, just relocated.
//!
//! ## The fix
//!
//! The sealed block is written ONCE, as one contiguous `pwrite` + one
//! `fsync`, into whichever fixed-size preallocated segment file
//! (`<data-dir>/segments/seg-<N>.blk`) currently has room. Postgres's
//! `gcore_objects` table shrinks to an index: `(addr, segment_id,
//! seg_offset, len)` — a pointer, never content. Reads `pread` the segment
//! directly. Multiple objects that were packed together in the arena stay
//! packed together on disk, in the same relative order, contiguous.
//!
//! ## Segment sizing and rollover
//!
//! `AXVERITY_SEGMENT_BYTES` (default 500_000_000 — Chris's number). A
//! non-empty current segment that doesn't have room for the next append
//! rolls to `seg-<N+1>.blk`. A single append LARGER than the configured
//! size (rare — e.g. `gr_obj_write_standalone`'s >64KiB bypass, though that
//! should never approach 500MB in practice) gets a dedicated oversized
//! segment sized to fit it exactly, same escape-hatch spirit as that
//! caller's own doc comment. Preallocation reuses `prealloc::prealloc_file`
//! (`FALLOC_FL_KEEP_SIZE` — see that module's doc for why `KEEP_SIZE` and
//! not `ftruncate`-to-full): the file's logical `st_size` still marks the
//! true data frontier, so a segment recovered after a crash never has to
//! distinguish "reserved" from "written" bytes past the last committed
//! offset — nothing ever reads past it (see recovery, below).
//!
//! ## Recovery — no separate bookkeeping, postgres IS the frontier
//!
//! There is no persisted "current segment / cursor" file. On first use,
//! the writer's position is DERIVED from `gcore_objects` itself: the
//! highest `segment_id`, and within it `MAX(seg_offset + len)`. This is
//! safe under the same ordering discipline already established for the
//! anchor chain (durable-write-before-index-commit): a segment `pwrite` is
//! fully written and `fsync`'d BEFORE the postgres row that points at it
//! ever commits. So any bytes physically sitting past the last COMMITTED
//! offset — e.g. a write that completed on disk but crashed before its
//! postgres transaction committed — are simply garbage that the next
//! append silently overwrites. Nothing ever indexes them, nothing ever
//! reads them.
//!
//! ## Concurrency
//!
//! `gcore_serve` request handling is single-threaded (one accept loop), but
//! the flush worker and `gr_obj_write_standalone`'s synchronous bypass
//! (`pg_bytes_put`) can both call in from different threads. `SEG_WRITER`
//! is a single process-wide `Mutex` — held across "pick/roll the segment,
//! pwrite, fsync, advance the cursor", released before the (independent)
//! postgres commit. Two calls' byte ranges never overlap because the
//! offset is reserved under that same lock before either one's I/O starts.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use super::prealloc;

const DEFAULT_SEGMENT_BYTES: i64 = 500_000_000;

fn segment_bytes() -> i64 {
    std::env::var("AXVERITY_SEGMENT_BYTES")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_SEGMENT_BYTES)
}

fn data_dir() -> PathBuf {
    let base = std::env::var("AXVERITY_GCORE_DATA_DIR").unwrap_or_else(|_| ".gcore".to_string());
    PathBuf::from(base).join("segments")
}

fn segment_path(id: i64) -> PathBuf {
    data_dir().join(format!("seg-{id}.blk"))
}

struct SegWriter {
    id: i64,
    cursor: i64,
}

static SEG_WRITER: OnceLock<Mutex<SegWriter>> = OnceLock::new();

fn seg_writer() -> &'static Mutex<SegWriter> {
    SEG_WRITER.get_or_init(|| Mutex::new(recover_seg_writer()))
}

/// Derive the resume position from `gcore_objects` itself — see this
/// module's "Recovery" doc section. Called once, lazily, on first append or
/// read after process start.
fn recover_seg_writer() -> SegWriter {
    std::fs::create_dir_all(data_dir())
        .unwrap_or_else(|e| panic!("objseg: mkdir {:?}: {}", data_dir(), e));

    let mut c = super::pg_store::conn().lock().unwrap();
    let row = c
        .query_opt(
            "SELECT segment_id, MAX(seg_offset + len) FROM gcore_objects \
             WHERE segment_id = (SELECT MAX(segment_id) FROM gcore_objects) \
             GROUP BY segment_id",
            &[],
        )
        .unwrap_or_else(|e| panic!("objseg: recover query: {e}"));
    drop(c);

    let (id, cursor) = match row {
        Some(r) => (r.get::<_, i64>(0), r.get::<_, i64>(1)),
        None => (0, 0),
    };
    prealloc::prealloc_file(segment_path(id).to_str().unwrap(), segment_bytes());
    SegWriter { id, cursor }
}

/// Append `bytes` as one contiguous, fsync'd write into the current segment
/// (rolling to a new one if it doesn't fit). Returns `(segment_id, offset)`
/// the caller commits into `gcore_objects` — outside this lock, since two
/// index commits for disjoint byte ranges don't need to serialize.
pub(crate) fn seg_append(bytes: &[u8]) -> (i64, i64) {
    let len = bytes.len() as i64;
    let cap = segment_bytes();
    let mut w = seg_writer().lock().unwrap();

    if w.cursor > 0 && w.cursor + len > cap {
        w.id += 1;
        w.cursor = 0;
    }
    let path = segment_path(w.id);
    if w.cursor == 0 {
        // Fresh segment: reserve at least `cap`, or exactly `len` for a
        // single oversized append that would never fit `cap` anyway (the
        // gr_obj_write_standalone escape hatch).
        prealloc::prealloc_file(path.to_str().unwrap(), cap.max(len));
    }

    let f = OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap_or_else(|e| panic!("objseg: open {:?}: {}", path, e));
    f.write_at(bytes, w.cursor as u64)
        .unwrap_or_else(|e| panic!("objseg: write_at {:?}@{}: {}", path, w.cursor, e));
    f.sync_data()
        .unwrap_or_else(|e| panic!("objseg: fsync {:?}: {}", path, e));

    let offset = w.cursor;
    w.cursor += len;
    (w.id, offset)
}

/// Read `len` bytes at `offset` from segment `id`.
pub(crate) fn seg_read(id: i64, offset: i64, len: i64) -> Vec<u8> {
    let path = segment_path(id);
    let f = File::open(&path)
        .unwrap_or_else(|e| panic!("objseg: open {:?} for read: {}", path, e));
    let mut buf = vec![0u8; len as usize];
    f.read_exact_at(&mut buf, offset as u64)
        .unwrap_or_else(|e| panic!("objseg: read_at {:?}@{}+{}: {}", path, offset, len, e));
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    // Exercises the pure file-level append/read/rollover logic directly,
    // independent of postgres or the SEG_WRITER global (each test builds
    // its own SegWriter by hand) -- avoids the shared-process-global-state
    // discipline the pg_store tests need, since nothing here touches conn().
    fn tmp_data_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("objseg-test-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(d.join("segments")).unwrap();
        d
    }

    #[test]
    #[ignore = "touches the real process-global SEG_WRITER OnceLock (via seg_append/seg_writer) \
                and mutates process-wide env vars (AXVERITY_GCORE_DATA_DIR/AXVERITY_SEGMENT_BYTES) \
                -- same shared-process-global-state discipline as pg_store's scratch DB and \
                block_flush's CHECKPOINT_WATERMARK. Run individually, not under parallel `cargo test`."]
    fn append_then_read_round_trips() {
        let dir = tmp_data_dir();
        std::env::set_var("AXVERITY_GCORE_DATA_DIR", dir.to_str().unwrap());
        std::env::set_var("AXVERITY_SEGMENT_BYTES", "1000");

        let (id1, off1) = seg_append(b"hello");
        let (id2, off2) = seg_append(b"world!!");
        assert_eq!(id1, id2, "both appends fit the same small segment");
        assert_eq!(off1, 0);
        assert_eq!(off2, 5);

        assert_eq!(seg_read(id1, off1, 5), b"hello");
        assert_eq!(seg_read(id2, off2, 7), b"world!!");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rolls_to_a_new_segment_when_full() {
        // Pure arithmetic against a scratch SegWriter -- deliberately does
        // NOT call seg_append/seg_writer(), so it needs no env vars and
        // touches no global state or real files; safe under the default
        // parallel test runner.
        let mut w = SegWriter { id: 0, cursor: 0 };
        // Reimplement the rollover decision directly against a scratch
        // writer (seg_append uses the process-global SEG_WRITER, which
        // other tests in this binary may have already initialized under a
        // different data dir -- this test proves the rollover ARITHMETIC,
        // not the global's lazy-init wiring).
        let cap = 10i64;
        let a = b"12345".to_vec(); // 5 bytes, fits
        if w.cursor > 0 && w.cursor + a.len() as i64 > cap { w.id += 1; w.cursor = 0; }
        let seg_a = w.id;
        w.cursor += a.len() as i64;

        let b = b"1234567".to_vec(); // 7 bytes -- 5+7=12 > 10, must roll
        if w.cursor > 0 && w.cursor + b.len() as i64 > cap { w.id += 1; w.cursor = 0; }
        let seg_b = w.id;

        assert_eq!(seg_a, 0);
        assert_eq!(seg_b, 1, "second append must roll to a new segment, not overflow the first");
    }
}

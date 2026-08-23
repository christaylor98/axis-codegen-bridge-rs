//! BRIDGE_PWRITE_RAW_V1 (axVerity-working2, AXVERITY_EXTENT_WRITE_PATH) —
//! write a raw memory range into an EXISTING file at a byte offset, with a
//! caller-selected durability mode.
//!
//! ## Why this exists
//!
//! `fs_write_raw` (bytes_io.rs) is the only raw-memory-to-disk primitive, and
//! it writes a WHOLE FILE through `write_durable`: temp file + fsync + rename +
//! parent-directory fsync. That is two fsyncs per call, and the parent-dir
//! fsync is paid ONLY because every block is a newly created file whose
//! directory entry has to be made durable before the write can be acked.
//!
//! The extent design removes that: `fs_prealloc` reserves one large segment
//! up front, and each block is written INTO it at `block_seq * capacity`. The
//! directory entry is then durable once, at segment creation, instead of once
//! per block. No temp file, no rename, no per-block parent-dir fsync.
//!
//! The `Bytes`-taking append primitives (`mmapseg_append`, `logbuf_append`)
//! cannot serve this path: they take `Value::Bytes`, so routing a multi-MiB
//! block through them would materialize a `Vec<u8>` copy of every block across
//! the Value ABI — exactly the cost axVerity-working2's raw-memory write path
//! exists to avoid.
//!
//! ## Durability is a KNOB, not a constant (deliberate)
//!
//! `sync_mode` is a caller-supplied parameter and is intended to STAY one:
//! the right durability/throughput trade-off differs per deployment, and this
//! primitive is the measurement surface for choosing it. The modes are ordered
//! weakest to strongest and are documented here so a caller picks with the
//! trade-off in front of them:
//!
//!   0 = NONE      — no sync. Bytes are in the kernel page cache. Survives a
//!                   process crash / SIGKILL (the kernel still writes them
//!                   back); does NOT survive power loss. Lower bound on cost.
//!   1 = FDATASYNC — `sync_data()`. Flushes data, and only the metadata needed
//!                   to read it back. On ext4 this skips the jbd2 inode-journal
//!                   update that `fsync` forces, which is the specific overhead
//!                   the write plateau was previously attributed to.
//!   2 = FSYNC     — `sync_all()`. Data + all inode metadata. What every other
//!                   durable path in this bridge uses today, uniformly and
//!                   without any measurement having chosen it.
//!
//! Mode 3 (O_DIRECT) is REJECTED at the call, loudly, rather than silently
//! omitted: O_DIRECT requires the source buffer, the file offset and the length
//! to be sector-aligned (512 B or 4096 B depending on the device), and
//! `mem_reserve_raw` allocates with `ALIGN = 8` (rawmem.rs:66). An O_DIRECT arm
//! is therefore unreachable without either raising that alignment for every
//! caller of the shared allocator or adding an aligned-reserve variant. That is
//! a separate, deliberate decision — not something to paper over with a bounce
//! buffer, which would reintroduce exactly the copy this primitive avoids.
//!
//! ## What this primitive does NOT do
//!
//! It does not fsync the parent directory. That is correct — the whole point is
//! to stop paying it per block — but it means the file's DIRECTORY ENTRY is not
//! durable just because a write into it was. The creator of the segment must
//! call `fs_sync_dir` once, after creating it and before relying on it. That is
//! a separate primitive precisely so the once-per-segment cost cannot get
//! silently folded into the per-block path.
//!
//! It also does not extend or verify the file. Writing at an offset beyond the
//! current end leaves a hole; the caller is responsible for writing blocks in
//! increasing offset order (axVerity-working2's flusher ejects in strict FIFO
//! seq order, so it does).

use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;

use super::value::Value;

pub const SYNC_NONE: i64 = 0;
pub const SYNC_FDATASYNC: i64 = 1;
pub const SYNC_FSYNC: i64 = 2;

/// `fs_pwrite_raw(path: Text, ptr: Int, offset: Int, len: Int, file_off: Int, sync_mode: Int) -> Unit`
///
/// Write `len` bytes from `[ptr+offset, ptr+offset+len)` into `path` at byte
/// offset `file_off`, then apply `sync_mode` (see the module header). The file
/// is created if absent and is NEVER truncated — an existing pre-allocated
/// extent is written into, not replaced.
///
/// Panics on a negative offset/len/file_off, on an unknown `sync_mode`, on
/// `sync_mode == 3` (O_DIRECT, see header), and on any OS error. Caller is
/// responsible for `ptr+offset` through `ptr+offset+len` being a valid,
/// initialized range — same unchecked contract as `fs_write_raw`.
#[track_caller]
pub fn fs_pwrite_raw(
    path: std::sync::Arc<str>,
    ptr: i64,
    offset: i64,
    len: i64,
    file_off: i64,
    sync_mode: i64,
) -> Value {
    if offset < 0 || len < 0 || file_off < 0 {
        panic!(
            "fs_pwrite_raw({}): offset, len and file_off must be >= 0, got offset={}, len={}, file_off={}",
            path, offset, len, file_off
        );
    }
    if sync_mode == 3 {
        panic!(
            "fs_pwrite_raw({}): sync_mode 3 (O_DIRECT) is not reachable — \
             mem_reserve_raw allocates with 8-byte alignment (rawmem.rs ALIGN) \
             and O_DIRECT requires sector alignment of buffer, offset and length. \
             Raising the shared allocator's alignment or adding an aligned-reserve \
             variant is a separate decision.",
            path
        );
    }
    if sync_mode < SYNC_NONE || sync_mode > SYNC_FSYNC {
        panic!(
            "fs_pwrite_raw({}): unknown sync_mode {} (0=none, 1=fdatasync, 2=fsync)",
            path, sync_mode
        );
    }

    let f = OpenOptions::new()
        .write(true)
        .create(true)
        .open(&*path)
        .unwrap_or_else(|e| panic!("fs_pwrite_raw({}): open: {}", path, e));

    let slice = unsafe {
        let src = (ptr as *const u8).add(offset as usize);
        std::slice::from_raw_parts(src, len as usize)
    };

    if let Err(e) = f.write_all_at(slice, file_off as u64) {
        panic!(
            "fs_pwrite_raw({}, file_off={}, len={}): write: {}",
            path, file_off, len, e
        );
    }

    let synced = match sync_mode {
        SYNC_NONE => Ok(()),
        SYNC_FDATASYNC => f.sync_data(),
        SYNC_FSYNC => f.sync_all(),
        _ => unreachable!("sync_mode validated above"),
    };
    if let Err(e) = synced {
        panic!("fs_pwrite_raw({}): sync (mode {}): {}", path, sync_mode, e);
    }

    Value::Unit
}

/// `fs_sync_dir(path: Text) -> Unit`
///
/// fsync a DIRECTORY, making the creation/rename of entries within it durable.
///
/// This is the once-per-segment half of the cost `fs_pwrite_raw` removes from
/// the per-block path. `fs_prealloc` creates a segment file but does NOT make
/// its directory entry durable, so a crash immediately after creating a segment
/// and writing blocks into it could leave a file the directory does not
/// reference. Call this once, on the segment's parent directory, after
/// creating the segment.
///
/// Panics on any OS error — never a silent skip, matching `write_durable`'s
/// stance that an un-fsyncable parent directory is a hard failure rather than
/// a degraded mode.
#[track_caller]
pub fn fs_sync_dir(path: std::sync::Arc<str>) -> Value {
    let d = File::open(&*path)
        .unwrap_or_else(|e| panic!("fs_sync_dir({}): open: {}", path, e));
    if let Err(e) = d.sync_all() {
        panic!("fs_sync_dir({}): fsync: {}", path, e);
    }
    Value::Unit
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("pwrite_test_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A block written at an offset into a pre-allocated extent reads back
    /// byte-identical, and does NOT disturb its neighbours.
    #[test]
    fn writes_blocks_into_a_prealloc_extent_at_offsets() {
        let d = tmpdir("extent");
        let seg = d.join("seg-0.bin");
        let path: Arc<str> = Arc::from(seg.to_str().unwrap());
        super::super::prealloc::fs_prealloc(path.clone(), 3 * 4096);

        // Three distinct blocks, written in increasing offset order as the
        // flusher ejects them (strict FIFO seq order).
        let blocks: Vec<Vec<u8>> = (0u8..3).map(|k| vec![0xA0 + k; 4096]).collect();
        for (k, b) in blocks.iter().enumerate() {
            fs_pwrite_raw(
                path.clone(),
                b.as_ptr() as i64,
                0,
                b.len() as i64,
                (k * 4096) as i64,
                SYNC_FDATASYNC,
            );
        }

        let got = std::fs::read(&seg).unwrap();
        assert_eq!(got.len(), 3 * 4096, "extent holds exactly the blocks written");
        for (k, b) in blocks.iter().enumerate() {
            assert_eq!(&got[k * 4096..(k + 1) * 4096], &b[..], "block {} round-trips", k);
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Writing does not truncate: an existing extent's other bytes survive.
    #[test]
    fn does_not_truncate_an_existing_file() {
        let d = tmpdir("notrunc");
        let seg = d.join("seg.bin");
        std::fs::write(&seg, vec![0xFFu8; 1024]).unwrap();
        let path: Arc<str> = Arc::from(seg.to_str().unwrap());

        let payload = vec![0x11u8; 16];
        fs_pwrite_raw(path, payload.as_ptr() as i64, 0, 16, 512, SYNC_NONE);

        let got = std::fs::read(&seg).unwrap();
        assert_eq!(got.len(), 1024, "file was not truncated");
        assert_eq!(&got[512..528], &payload[..], "target range updated");
        assert!(got[..512].iter().all(|b| *b == 0xFF), "bytes before are intact");
        assert!(got[528..].iter().all(|b| *b == 0xFF), "bytes after are intact");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The `offset` argument selects a sub-range of the source buffer, exactly
    /// as fs_write_raw's does — this is the block-within-arena case.
    #[test]
    fn honours_the_source_offset() {
        let d = tmpdir("srcoff");
        let seg = d.join("seg.bin");
        let path: Arc<str> = Arc::from(seg.to_str().unwrap());

        let arena: Vec<u8> = (0..=255u8).collect();
        fs_pwrite_raw(path.clone(), arena.as_ptr() as i64, 100, 8, 0, SYNC_FSYNC);

        let got = std::fs::read(&seg).unwrap();
        assert_eq!(got, &arena[100..108], "wrote arena[100..108], not arena[0..8]");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Every sync mode is accepted and produces identical bytes — the knob
    /// changes cost, never content.
    #[test]
    fn all_sync_modes_produce_identical_bytes() {
        let d = tmpdir("modes");
        let payload = vec![0x5Au8; 2048];
        let mut outs = Vec::new();
        for mode in [SYNC_NONE, SYNC_FDATASYNC, SYNC_FSYNC] {
            let seg = d.join(format!("seg-{}.bin", mode));
            let path: Arc<str> = Arc::from(seg.to_str().unwrap());
            fs_pwrite_raw(path, payload.as_ptr() as i64, 0, payload.len() as i64, 0, mode);
            outs.push(std::fs::read(&seg).unwrap());
        }
        assert!(outs.iter().all(|o| *o == payload), "content is mode-independent");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// O_DIRECT is refused LOUDLY with the alignment reason, not silently absent.
    #[test]
    #[should_panic(expected = "O_DIRECT")]
    fn o_direct_is_refused_with_the_alignment_reason() {
        let d = tmpdir("odirect");
        let path: Arc<str> = Arc::from(d.join("s.bin").to_str().unwrap());
        let b = vec![0u8; 8];
        fs_pwrite_raw(path, b.as_ptr() as i64, 0, 8, 0, 3);
    }

    #[test]
    #[should_panic(expected = "unknown sync_mode")]
    fn unknown_sync_mode_panics() {
        let d = tmpdir("badmode");
        let path: Arc<str> = Arc::from(d.join("s.bin").to_str().unwrap());
        let b = vec![0u8; 8];
        fs_pwrite_raw(path, b.as_ptr() as i64, 0, 8, 0, 9);
    }

    #[test]
    #[should_panic(expected = "must be >= 0")]
    fn negative_file_offset_panics() {
        let d = tmpdir("negoff");
        let path: Arc<str> = Arc::from(d.join("s.bin").to_str().unwrap());
        let b = vec![0u8; 8];
        fs_pwrite_raw(path, b.as_ptr() as i64, 0, 8, -1, SYNC_NONE);
    }

    #[test]
    fn sync_dir_fsyncs_a_directory() {
        let d = tmpdir("syncdir");
        std::fs::write(d.join("f.bin"), b"x").unwrap();
        fs_sync_dir(Arc::from(d.to_str().unwrap()));
        let _ = std::fs::remove_dir_all(&d);
    }
}

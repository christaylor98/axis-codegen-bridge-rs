//! AXVERITY_INDEXER_THREADING_V1 — background index-build wait-handler.
//! T2 (batched per-shard artifacts): the index is stored in per-worker
//! APPEND-ONLY SEGMENT files, one appended write per drained batch — NEVER one
//! file per block. The original per-block `.idx` shape was exactly the
//! one-file-per-key regression Landing A's generalized rule exists to kill
//! ("nothing defaults to one file per key; an index is batched per shard,
//! pack-shaped, never per key" — axVerity CLAUDE.md §19). This module now
//! mirrors `walindex.rs`'s discipline instead:
//!
//!   * WRITE — each worker thread owns its own segment file
//!     `<flush_dir>/index/iseg-<nanos>-<pid>`, minted THREAD-LOCALLY on first
//!     use (per dir). The whole drained batch is serialized into one buffer
//!     and lands in ONE `O_APPEND` write. No shared state on the write path:
//!     no Mutex, no registry, no cross-thread file sharing. (Spike-4 check:
//!     the only process-global here is the `std::time` + pid used ONCE per
//!     worker-thread per dir to mint a unique segment name — startup-only,
//!     the same posture as net.rs's handle counter; nothing global is touched
//!     per block or per batch.)
//!   * ENTRY — one self-validating text line per block:
//!     `ISEG1\t<block_seq>\t<sha256:hex>\t<len>\t<succinct>\n`.
//!     A torn tail (SIGKILL mid-append) fails parse and is skipped — the
//!     index is a REBUILDABLE CACHE over immutable content-addressed blocks
//!     (battle-suite S2 proved recovery), so a torn line degrades to
//!     "not indexed", never to a wrong answer.
//!   * READ — `idxseg_lookup` scans the dir's segments in sorted-name order
//!     (names embed nanos, so sorted ≈ append order) and returns the LAST
//!     valid entry for the block. Last-wins is what makes REPAIR-BY-APPEND
//!     work: a corrupted entry is superseded by re-indexing (a fresh, later
//!     segment), no in-place rewrite ever. Duplicate entries are harmless by
//!     construction — a block is immutable, so every valid entry for a seq
//!     is byte-identical in meaning.
//!   * REBUILD — `index_rebuild_dir` re-indexes EVERY `block-*.bin` in one
//!     process with one segment append (the storm-recovery path; invoking a
//!     per-block CLI 300 times would recreate one-file-per-key through the
//!     back door).
//!
//! Everything below the artifact format is unchanged from the pre-T2 module:
//! `index_build_batch` is the `wait()` handler (`fn(Value) -> Value` at the
//! Rust ABI — an M1 composite cannot fill that slot, CLAUDE.md §15), blocks
//! are read READ-ONLY from the durable flushed `.bin`, the Merkle hash is the
//! same sha2 digest as `content_hash`/`bytes_hash`, the succinct structure is
//! a SEAM STUB, and each block's index-status cell flips Unindexed(0) ->
//! Indexed(1) in ONE CAS — after the batch append, so a reader that observes
//! `Indexed` can always find the entry; a reader observing `Unindexed` falls
//! back to a raw scan, never a skip.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicI64, Ordering};

use sha2::{Digest, Sha256};

use super::value::{get_str, intern_str, Value};

const UNINDEXED: i64 = 0;
const INDEXED: i64 = 1;

thread_local! {
    /// This worker thread's segment path per flush_dir. THREAD-LOCAL, never
    /// shared — same "thread-owned, no shared registry" storage discipline as
    /// logbuf.rs / walindex.rs. Minted once per (thread, dir).
    static SEGS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

fn as_int(field: &'static str, v: Value) -> i64 {
    match v {
        Value::Int(n) => n,
        other => panic!("indexer: {} expected Int, got {:?}", field, other),
    }
}

fn as_text(field: &'static str, v: Value) -> String {
    match v {
        Value::Str(h) => get_str(&h),
        other => panic!("indexer: {} expected Text, got {:?}", field, other),
    }
}

/// A fully-computed index entry, not yet written.
struct Entry {
    flush_dir: String,
    line: String,
    idx_cell: i64,
}

/// SEAM STUB for the succinct structure (real builder is separate M1 work;
/// only this fn changes when it lands).
fn build_succinct_stub(bytes: &[u8]) -> String {
    format!("SUCCINCT_STUB len={}", bytes.len())
}

/// Read the block READ-ONLY, hash it, and produce its entry line. Panics on a
/// missing `.bin` (fail-stop, never a silent skip) and on a torn/short flush
/// when `byte_len >= 0` is supplied.
fn build_entry(block_seq: i64, flush_dir: String, byte_len: i64, idx_cell: i64) -> Entry {
    let bin_path = format!("{}/block-{}.bin", flush_dir, block_seq);
    let bytes = std::fs::read(&bin_path)
        .unwrap_or_else(|e| panic!("index_build_batch: read {}: {}", bin_path, e));
    if byte_len >= 0 && bytes.len() != byte_len as usize {
        panic!(
            "index_build_batch: {} is {} bytes, seal recorded byte_len={} (torn/short flush?)",
            bin_path,
            bytes.len(),
            byte_len
        );
    }
    let digest = Sha256::digest(&bytes);
    let merkle: String = digest.iter().map(|b| format!("{:02x}", b)).collect();
    let line = format!(
        "ISEG1\t{}\tsha256:{}\t{}\t{}\n",
        block_seq,
        merkle,
        bytes.len(),
        build_succinct_stub(&bytes)
    );
    Entry { flush_dir, line, idx_cell }
}

fn unpack_descriptor(item: Value) -> (i64, String, i64, i64) {
    let fields: Vec<Value> = match item {
        Value::Ctor { fields, .. } => fields,
        Value::Tuple(es) => es,
        other => panic!(
            "index_build_batch: expected a 4-field descriptor (Ctor or Tuple), got {:?}",
            other
        ),
    };
    if fields.len() != 4 {
        panic!(
            "index_build_batch: descriptor must have 4 fields (block_seq, flush_dir, byte_len, idx_cell), got {}",
            fields.len()
        );
    }
    let mut it = fields.into_iter();
    let block_seq = as_int("block_seq", it.next().unwrap());
    let flush_dir = as_text("flush_dir", it.next().unwrap());
    let byte_len = as_int("byte_len", it.next().unwrap());
    let idx_cell = as_int("idx_cell", it.next().unwrap());
    (block_seq, flush_dir, byte_len, idx_cell)
}

/// This thread's segment path for `dir`, minted on first use. Uniqueness =
/// nanos + pid (cross-process safe: a recovery CLI gets its own, LATER-named
/// segment, so its entries win the sorted-order last-wins read).
fn segment_path_for(dir: &str) -> String {
    SEGS.with(|segs| {
        let mut segs = segs.borrow_mut();
        if let Some(p) = segs.get(dir) {
            return p.clone();
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        let p = format!("{}/index/iseg-{:019}-{}", dir, nanos, std::process::id());
        segs.insert(dir.to_string(), p.clone());
        p
    })
}

/// Append the batch's entries: grouped by flush_dir, ONE write per dir.
fn append_entries(entries: &[Entry]) {
    let mut by_dir: HashMap<&str, String> = HashMap::new();
    for e in entries {
        by_dir.entry(&e.flush_dir).or_default().push_str(&e.line);
    }
    for (dir, buf) in by_dir {
        let idx_dir = format!("{}/index", dir);
        std::fs::create_dir_all(&idx_dir)
            .unwrap_or_else(|e| panic!("index_build_batch: mkdir {}: {}", idx_dir, e));
        let seg = segment_path_for(dir);
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&seg)
            .unwrap_or_else(|e| panic!("index_build_batch: open {}: {}", seg, e));
        f.write_all(buf.as_bytes())
            .unwrap_or_else(|e| panic!("index_build_batch: append {}: {}", seg, e));
    }
}

/// `index_build_batch(arg: Value) -> Value`   —   the `wait()` handler.
///
/// Accepts a List of descriptors (the channel-drain path) or a BARE
/// descriptor (the direct-call path used by `src/index_one.m1`). Each
/// descriptor is `(block_seq: Int, flush_dir: Text, byte_len: Int,
/// idx_cell: Int)`; `idx_cell == 0` means "no cell" (rebuild/recovery paths)
/// and skips the flip. Returns Int(n) = blocks indexed this call.
///
/// Ordering: ALL entries are computed, then appended (one write per dir),
/// then ALL cells flip — `Indexed` implies the entry is visible.
#[track_caller]
pub fn index_build_batch(arg: Value) -> Value {
    let items = match arg {
        Value::List(items) => items,
        bare @ (Value::Ctor { .. } | Value::Tuple(_)) => vec![bare],
        other => panic!(
            "index_build_batch: expected List of descriptors or a bare descriptor, got {:?}",
            other
        ),
    };
    let entries: Vec<Entry> = items
        .into_iter()
        .map(|item| {
            let (seq, dir, len, cell) = unpack_descriptor(item);
            build_entry(seq, dir, len, cell)
        })
        .collect();
    append_entries(&entries);
    // Single-pass atomic flips, LAST (ATOMIC_SINGLE_PASS_STATE_FLIP). A failed
    // CAS means already Indexed (duplicate delivery) — no third state.
    for e in &entries {
        if e.idx_cell != 0 {
            let cell = unsafe { &*(e.idx_cell as *const AtomicI64) };
            let _ = cell.compare_exchange(UNINDEXED, INDEXED, Ordering::SeqCst, Ordering::SeqCst);
        }
    }
    Value::Int(entries.len() as i64)
}

/// Parse one segment line; None if torn/invalid (skipped, never trusted).
fn parse_entry_line(line: &str) -> Option<(i64, String)> {
    let mut parts = line.split('\t');
    if parts.next()? != "ISEG1" {
        return None;
    }
    let seq: i64 = parts.next()?.parse().ok()?;
    let merkle = parts.next()?;
    if !merkle.starts_with("sha256:") || merkle.len() != 71 {
        return None;
    }
    let len: i64 = parts.next()?.parse().ok()?;
    let succinct = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    Some((seq, format!("{}\t{}\t{}", merkle, len, succinct)))
}

/// Scan `dir`'s segments (sorted names ≈ append order) for `seq`'s LAST valid
/// entry. Returns the entry payload `"<sha256:hex>\t<len>\t<succinct>"` or
/// `None`.
fn lookup(dir: &str, seq: i64) -> Option<String> {
    let idx_dir = format!("{}/index", dir);
    let mut seg_names: Vec<String> = match std::fs::read_dir(&idx_dir) {
        Err(_) => return None, // never indexed at all
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("iseg-"))
            .collect(),
    };
    seg_names.sort();
    let mut found: Option<String> = None;
    for name in seg_names {
        let path = format!("{}/{}", idx_dir, name);
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        for line in text.lines() {
            if let Some((s, entry)) = parse_entry_line(line) {
                if s == seq {
                    found = Some(entry); // last valid wins (repair-by-append)
                }
            }
        }
    }
    found
}

/// `idxseg_lookup(dir: Text, block_seq: Int) -> Text`
///
/// The read side for M1 (`lib_async/block_read_indexed.m1`): returns the
/// block's entry payload `"<sha256:hex>\t<len>\t<succinct>"`, or `""` when no
/// valid entry exists (never indexed, or every entry for it is torn) — the M1
/// caller maps `""` to the NOT_INDEXED sentinel.
#[track_caller]
pub fn idxseg_lookup(args: Value) -> Value {
    let (dir_v, seq_v) = match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("idxseg_lookup: expected (Text, Int), got {:?}", other),
    };
    let dir = as_text("dir", dir_v);
    let seq = as_int("block_seq", seq_v);
    Value::Str(intern_str(&lookup(&dir, seq).unwrap_or_default()))
}

/// `index_rebuild_dir(dir: Text) -> Int`
///
/// Storm-recovery: re-index EVERY `block-<seq>.bin` in `dir` in this one
/// process — one segment file, one append. Idempotent (duplicate entries are
/// harmless; last-wins read). No cells (they died with the crashed process).
/// Returns the number of blocks indexed.
#[track_caller]
pub fn index_rebuild_dir(arg: Value) -> Value {
    let dir = as_text("dir", arg);
    let mut seqs: Vec<i64> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("index_rebuild_dir: read_dir {}: {}", dir, e))
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.strip_prefix("block-")?.strip_suffix(".bin")?.parse::<i64>().ok()
        })
        .collect();
    seqs.sort_unstable();
    let entries: Vec<Entry> = seqs
        .iter()
        .map(|&seq| build_entry(seq, dir.clone(), -1, 0))
        .collect();
    append_entries(&entries);
    Value::Int(entries.len() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mint_cell(initial: i64) -> i64 {
        let leaked: &'static AtomicI64 = Box::leak(Box::new(AtomicI64::new(initial)));
        leaked as *const AtomicI64 as i64
    }

    fn cell_load(addr: i64) -> i64 {
        let cell = unsafe { &*(addr as *const AtomicI64) };
        cell.load(Ordering::SeqCst)
    }

    fn unique_dir(tag: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!("axv-idx-{}-{}-{}", tag, std::process::id(), nanos));
        std::fs::create_dir_all(&d).unwrap();
        d.to_string_lossy().into_owned()
    }

    fn descriptor(block_seq: i64, flush_dir: &str, byte_len: i64, idx_cell: i64) -> Value {
        Value::Tuple(vec![
            Value::Int(block_seq),
            Value::Str(intern_str(flush_dir)),
            Value::Int(byte_len),
            Value::Int(idx_cell),
        ])
    }

    fn expected_sha256(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        digest.iter().map(|b| format!("{:02x}", b)).collect()
    }

    /// The reader-visible entry for a block, via the same fn M1 uses.
    fn lookup_str(dir: &str, seq: i64) -> String {
        match idxseg_lookup(Value::Tuple(vec![
            Value::Str(intern_str(dir)),
            Value::Int(seq),
        ])) {
            Value::Str(h) => get_str(&h),
            other => panic!("idxseg_lookup returned {:?}", other),
        }
    }

    fn assert_entry_correct(dir: &str, seq: i64, payload: &[u8]) {
        let entry = lookup_str(dir, seq);
        assert!(!entry.is_empty(), "block {} has no valid entry", seq);
        let mut f = entry.split('\t');
        assert_eq!(f.next().unwrap(), format!("sha256:{}", expected_sha256(payload)));
        assert_eq!(f.next().unwrap(), payload.len().to_string());
        assert!(f.next().unwrap().starts_with("SUCCINCT_STUB"));
    }

    fn seg_count(dir: &str) -> usize {
        std::fs::read_dir(format!("{}/index", dir))
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.file_name().to_string_lossy().starts_with("iseg-"))
                    .count()
            })
            .unwrap_or(0)
    }

    #[test]
    fn indexes_one_block_writes_entry_and_flips_cell() {
        let dir = unique_dir("one");
        let payload = b"PAYLOAD-block-0-contents".to_vec();
        std::fs::write(format!("{}/block-0.bin", dir), &payload).unwrap();
        let cell = mint_cell(UNINDEXED);
        let out = index_build_batch(Value::List(vec![descriptor(0, &dir, payload.len() as i64, cell)]));
        assert_eq!(out, Value::Int(1));
        assert_entry_correct(&dir, 0, &payload);
        assert_eq!(cell_load(cell), INDEXED);
    }

    #[test]
    fn accepts_ctor_shape_descriptor_the_m1_runtime_path() {
        let dir = unique_dir("ctor");
        let payload = b"ctor-path-body".to_vec();
        std::fs::write(format!("{}/block-7.bin", dir), &payload).unwrap();
        let cell = mint_cell(UNINDEXED);
        let desc = Value::Ctor {
            tag: 0,
            fields: vec![
                Value::Int(7),
                Value::Str(intern_str(&dir)),
                Value::Int(payload.len() as i64),
                Value::Int(cell),
            ],
        };
        index_build_batch(Value::List(vec![desc]));
        assert_entry_correct(&dir, 7, &payload);
        assert_eq!(cell_load(cell), INDEXED);
    }

    #[test]
    fn bare_descriptor_direct_call_path() {
        let dir = unique_dir("bare");
        let payload = b"bare-descriptor-body".to_vec();
        std::fs::write(format!("{}/block-0.bin", dir), &payload).unwrap();
        let cell = mint_cell(UNINDEXED);
        let out = index_build_batch(descriptor(0, &dir, payload.len() as i64, cell));
        assert_eq!(out, Value::Int(1));
        assert_eq!(cell_load(cell), INDEXED);
        assert_entry_correct(&dir, 0, &payload);
    }

    #[test]
    fn whole_batch_lands_in_one_segment_file() {
        // THE T2 property: a 500-block batch produces ONE segment file on this
        // thread — never one file per key.
        let dir = unique_dir("onefile");
        let mut items = Vec::new();
        let mut payloads = Vec::new();
        for seq in 0..500i64 {
            let payload = format!("b{}", seq).into_bytes();
            std::fs::write(format!("{}/block-{}.bin", dir, seq), &payload).unwrap();
            items.push(descriptor(seq, &dir, payload.len() as i64, mint_cell(UNINDEXED)));
            payloads.push(payload);
        }
        index_build_batch(Value::List(items));
        assert_eq!(seg_count(&dir), 1, "batched artifacts must be ONE file per worker");
        for (seq, p) in payloads.iter().enumerate() {
            assert_entry_correct(&dir, seq as i64, p);
        }
    }

    #[test]
    fn redelivery_is_idempotent_no_third_state() {
        let dir = unique_dir("idem");
        let payload = b"idempotent-body".to_vec();
        std::fs::write(format!("{}/block-0.bin", dir), &payload).unwrap();
        let cell = mint_cell(UNINDEXED);
        let d = || Value::List(vec![descriptor(0, &dir, payload.len() as i64, cell)]);
        index_build_batch(d());
        assert_eq!(cell_load(cell), INDEXED);
        index_build_batch(d()); // duplicate entry appended; last-wins read, same content
        assert_eq!(cell_load(cell), INDEXED);
        assert_entry_correct(&dir, 0, &payload);
    }

    #[test]
    fn torn_tail_line_is_skipped_never_trusted() {
        let dir = unique_dir("torn");
        let payload = b"torn-tail-victim".to_vec();
        std::fs::write(format!("{}/block-0.bin", dir), &payload).unwrap();
        index_build_batch(descriptor(0, &dir, payload.len() as i64, mint_cell(UNINDEXED)));
        // SIGKILL-mid-append simulation: truncate the segment mid-line.
        let seg = std::fs::read_dir(format!("{}/index", dir))
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().starts_with("iseg-"))
            .unwrap()
            .path();
        let bytes = std::fs::read(&seg).unwrap();
        std::fs::write(&seg, &bytes[..bytes.len() / 2]).unwrap();
        assert_eq!(lookup_str(&dir, 0), "", "torn entry must parse out, not lie");
    }

    #[test]
    fn repair_by_append_wins_over_corrupted_entry() {
        let dir = unique_dir("repair");
        let payload = b"repairable-body".to_vec();
        std::fs::write(format!("{}/block-0.bin", dir), &payload).unwrap();
        index_build_batch(descriptor(0, &dir, payload.len() as i64, mint_cell(UNINDEXED)));
        // corrupt the stored merkle IN PLACE (valid shape, wrong hash)
        let seg = std::fs::read_dir(format!("{}/index", dir))
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().starts_with("iseg-"))
            .unwrap()
            .path();
        let text = std::fs::read_to_string(&seg).unwrap().replace(
            &expected_sha256(&payload)[..8],
            "00000000",
        );
        std::fs::write(&seg, text).unwrap();
        let bad = lookup_str(&dir, 0);
        assert!(bad.starts_with("sha256:00000000"), "corrupted entry visible pre-repair");
        // repair: rebuild appends a fresh, LATER segment; last-wins read heals
        std::thread::sleep(std::time::Duration::from_millis(2)); // distinct nanos name
        // rebuild runs on another thread so THIS thread's cached segment name
        // (same file we corrupted) isn't reused — mirrors the real recovery
        // shape, which is always a fresh process.
        let d2 = dir.clone();
        std::thread::spawn(move || index_rebuild_dir(Value::Str(intern_str(&d2))))
            .join()
            .unwrap();
        assert_entry_correct(&dir, 0, &payload);
    }

    #[test]
    fn rebuild_dir_reindexes_everything_in_one_segment() {
        let dir = unique_dir("rebuild");
        let mut payloads = Vec::new();
        for seq in 0..40i64 {
            let payload = format!("rebuild-{}", seq).into_bytes();
            std::fs::write(format!("{}/block-{}.bin", dir, seq), &payload).unwrap();
            payloads.push(payload);
        }
        let out = index_rebuild_dir(Value::Str(intern_str(&dir)));
        assert_eq!(out, Value::Int(40));
        assert_eq!(seg_count(&dir), 1, "rebuild must produce ONE segment");
        for (seq, p) in payloads.iter().enumerate() {
            assert_entry_correct(&dir, seq as i64, p);
        }
    }

    #[test]
    fn concurrent_hammer_8_threads_200_blocks_each() {
        use std::thread;
        let dir = unique_dir("hammer");
        let mut all: Vec<(i64, Vec<u8>, i64)> = Vec::new();
        for seq in 0..1600i64 {
            let payload = format!("hammer-block-{}-body-{}", seq, "x".repeat(64)).into_bytes();
            std::fs::write(format!("{}/block-{}.bin", dir, seq), &payload).unwrap();
            all.push((seq, payload, mint_cell(UNINDEXED)));
        }
        let handles: Vec<_> = (0..8)
            .map(|t| {
                let chunk: Vec<(i64, i64, usize)> = all[t * 200..(t + 1) * 200]
                    .iter()
                    .map(|(s, p, c)| (*s, *c, p.len()))
                    .collect();
                let d = dir.clone();
                thread::spawn(move || {
                    // one batch per thread: the realistic drained-batch shape
                    let items: Vec<Value> = chunk
                        .iter()
                        .map(|&(seq, cell, len)| descriptor(seq, &d, len as i64, cell))
                        .collect();
                    index_build_batch(Value::List(items));
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        // 8 worker threads -> at most 8 segment files, one each (thread-owned)
        assert!(seg_count(&dir) <= 8, "expected <=8 per-worker segments, got {}", seg_count(&dir));
        for (seq, payload, cell) in &all {
            assert_eq!(cell_load(*cell), INDEXED, "block {} not flipped", seq);
            assert_entry_correct(&dir, *seq, payload);
        }
    }

    #[test]
    fn duplicate_race_8_threads_same_block() {
        use std::thread;
        let dir = unique_dir("duprace");
        let payload = b"contended-block-body".to_vec();
        std::fs::write(format!("{}/block-0.bin", dir), &payload).unwrap();
        let cell = mint_cell(UNINDEXED);
        let len = payload.len() as i64;
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let d = dir.clone();
                thread::spawn(move || {
                    for _ in 0..50 {
                        index_build_batch(descriptor(0, &d, len, cell));
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(cell_load(cell), INDEXED);
        assert_entry_correct(&dir, 0, &payload);
    }

    #[test]
    fn binary_non_utf8_block_indexes_correctly() {
        let dir = unique_dir("binary");
        let payload: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        assert!(String::from_utf8(payload.clone()).is_err(), "fixture must be non-UTF8");
        std::fs::write(format!("{}/block-0.bin", dir), &payload).unwrap();
        let cell = mint_cell(UNINDEXED);
        index_build_batch(descriptor(0, &dir, payload.len() as i64, cell));
        assert_eq!(cell_load(cell), INDEXED);
        assert_entry_correct(&dir, 0, &payload);
    }

    #[test]
    fn empty_block_indexes_with_empty_sha() {
        let dir = unique_dir("empty");
        std::fs::write(format!("{}/block-0.bin", dir), b"").unwrap();
        let cell = mint_cell(UNINDEXED);
        index_build_batch(descriptor(0, &dir, 0, cell));
        assert_eq!(cell_load(cell), INDEXED);
        let entry = lookup_str(&dir, 0);
        assert!(entry.starts_with(
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\t0\t"
        ));
    }

    #[test]
    fn spike_sized_4mib_and_oversize_32mib_blocks() {
        let dir = unique_dir("large");
        for (seq, mib) in [(0i64, 4usize), (1, 32)] {
            let payload: Vec<u8> = (0..mib * 1024 * 1024).map(|i| (i % 251) as u8).collect();
            std::fs::write(format!("{}/block-{}.bin", dir, seq), &payload).unwrap();
            let cell = mint_cell(UNINDEXED);
            index_build_batch(descriptor(seq, &dir, payload.len() as i64, cell));
            assert_eq!(cell_load(cell), INDEXED, "{}MiB block not flipped", mib);
            assert_entry_correct(&dir, seq, &payload);
        }
    }

    #[test]
    fn ten_thousand_descriptor_batch() {
        let dir = unique_dir("batch10k");
        let mut cells = Vec::new();
        let mut items = Vec::new();
        for seq in 0..10_000i64 {
            let payload = format!("b{}", seq).into_bytes();
            std::fs::write(format!("{}/block-{}.bin", dir, seq), &payload).unwrap();
            let cell = mint_cell(UNINDEXED);
            cells.push(cell);
            items.push(descriptor(seq, &dir, payload.len() as i64, cell));
        }
        let out = index_build_batch(Value::List(items));
        assert_eq!(out, Value::Int(10_000));
        assert_eq!(seg_count(&dir), 1, "10k-block batch must land in one segment");
        for (seq, cell) in cells.iter().enumerate() {
            assert_eq!(cell_load(*cell), INDEXED, "block {} not flipped", seq);
        }
    }

    #[test]
    #[should_panic(expected = "read")]
    fn missing_bin_is_fail_stop() {
        let dir = unique_dir("missing");
        let cell = mint_cell(UNINDEXED);
        index_build_batch(descriptor(42, &dir, 10, cell));
    }

    #[test]
    #[should_panic(expected = "torn/short flush")]
    fn short_flush_is_surfaced_not_silently_indexed() {
        let dir = unique_dir("short");
        std::fs::write(format!("{}/block-0.bin", dir), b"only-8b?").unwrap();
        let cell = mint_cell(UNINDEXED);
        index_build_batch(Value::List(vec![descriptor(0, &dir, 999, cell)]));
    }

    #[test]
    fn unicode_and_space_flush_dir() {
        let base = unique_dir("uni");
        let dir = format!("{}/flush dir — bloc κ", base);
        std::fs::create_dir_all(&dir).unwrap();
        let payload = b"unicode-dir-body".to_vec();
        std::fs::write(format!("{}/block-0.bin", dir), &payload).unwrap();
        let cell = mint_cell(UNINDEXED);
        index_build_batch(descriptor(0, &dir, payload.len() as i64, cell));
        assert_eq!(cell_load(cell), INDEXED);
        assert_entry_correct(&dir, 0, &payload);
    }

    #[test]
    fn never_indexed_block_lookup_is_empty() {
        let dir = unique_dir("noidx");
        std::fs::write(format!("{}/block-0.bin", dir), b"sealed-not-indexed").unwrap();
        assert_eq!(lookup_str(&dir, 0), "");
    }

    // ── Spike-4 fan-in contention sweep (intent's REQUIRED next-test) ───────
    //
    // Measures the ONE shared coordination point in the seal->indexer path:
    // the channels.rs registry mutex + global condvar behind channel_send/wait.
    // PASS/FAIL bar (AXVERITY_INDEXER_THREADING_V1 r2): NO regression toward
    // the Spike-4 degrading-with-cores signature; hard assert is the egregious
    // collapse only (agg(16) > 0.5 * agg(1)).
    //   cargo test --release --lib contention_fanin_sweep -- --ignored --nocapture
    #[test]
    #[ignore = "explicit contention sweep — run with --ignored --nocapture"]
    fn contention_fanin_sweep() {
        use std::sync::atomic::AtomicUsize;
        use std::thread;
        use std::time::Instant;

        static RECEIVED: AtomicUsize = AtomicUsize::new(0);
        fn count_handler(arg: Value) -> Value {
            if let Value::List(items) = arg {
                RECEIVED.fetch_add(items.len(), Ordering::SeqCst);
            }
            Value::Unit
        }

        fn text(s: &str) -> Value {
            Value::Str(intern_str(s))
        }

        const TOTAL: usize = 200_000;
        let mut results: Vec<(usize, f64)> = Vec::new();

        for p in [1usize, 4, 16] {
            let chan = format!("bat-fan-{}", p);
            RECEIVED.store(0, Ordering::SeqCst);
            let c_chan = chan.clone();
            let consumer = thread::spawn(move || {
                super::super::channels::event_subscribe(text(&c_chan));
                while RECEIVED.load(Ordering::SeqCst) < TOTAL {
                    super::super::channels::wait(count_handler);
                }
            });
            let per = TOTAL / p;
            let start = Instant::now();
            let producers: Vec<_> = (0..p)
                .map(|t| {
                    let p_chan = chan.clone();
                    thread::spawn(move || {
                        for i in 0..per {
                            let desc = Value::Ctor {
                                tag: 0,
                                fields: vec![
                                    Value::Int((t * per + i) as i64),
                                    text("/tmp/fanin"),
                                    Value::Int(64),
                                    Value::Int(0),
                                ],
                            };
                            super::super::channels::channel_send(Value::Tuple(vec![
                                text(&p_chan),
                                desc,
                            ]));
                        }
                    })
                })
                .collect();
            for h in producers {
                h.join().unwrap();
            }
            consumer.join().unwrap();
            let secs = start.elapsed().as_secs_f64();
            let thr = TOTAL as f64 / secs;
            eprintln!(
                "FANIN P={:2}  total={}  elapsed={:.3}s  aggregate={:.0} msgs/s",
                p, TOTAL, secs, thr
            );
            results.push((p, thr));
        }
        let agg1 = results[0].1;
        let agg16 = results[2].1;
        eprintln!(
            "FANIN SUMMARY  agg(1)={:.0}  agg(4)={:.0}  agg(16)={:.0}  ratio16/1={:.2}",
            agg1, results[1].1, agg16, agg16 / agg1
        );
        assert!(
            agg16 > 0.5 * agg1,
            "SPIKE-4 SIGNATURE: aggregate collapsed under fan-in (agg16={:.0} vs agg1={:.0})",
            agg16,
            agg1
        );
    }

    // ── Index-build throughput probe (tuning baseline, run explicitly) ──────
    //   cargo test --release --lib index_throughput_probe -- --ignored --nocapture
    #[test]
    #[ignore = "explicit perf probe — run with --ignored --nocapture"]
    fn index_throughput_probe() {
        use std::thread;
        use std::time::Instant;
        const MIB: usize = 1024 * 1024;
        const BLOCK: usize = 4 * MIB;
        const N: usize = 64;

        let dir = unique_dir("tput");
        let mut work: Vec<(i64, i64)> = Vec::new();
        for seq in 0..N as i64 {
            let payload: Vec<u8> = (0..BLOCK).map(|i| ((i as i64 + seq) % 251) as u8).collect();
            std::fs::write(format!("{}/block-{}.bin", dir, seq), &payload).unwrap();
            work.push((seq, mint_cell(UNINDEXED)));
        }
        let t = Instant::now();
        for &(seq, cell) in &work {
            index_build_batch(descriptor(seq, &dir, BLOCK as i64, cell));
        }
        let s1 = t.elapsed().as_secs_f64();
        let mb = (N * BLOCK) as f64 / MIB as f64;
        eprintln!(
            "TPUT 1-thread : {} x 4MiB = {:.0} MiB in {:.3}s -> {:.0} MiB/s, {:.0} blocks/s",
            N, mb, s1, mb / s1, N as f64 / s1
        );
        let cells2: Vec<i64> = (0..N).map(|_| mint_cell(UNINDEXED)).collect();
        let t = Instant::now();
        let handles: Vec<_> = (0..8)
            .map(|w| {
                let d = dir.clone();
                let chunk: Vec<(i64, i64)> = (0..N)
                    .filter(|i| i % 8 == w)
                    .map(|i| (i as i64, cells2[i]))
                    .collect();
                thread::spawn(move || {
                    for (seq, cell) in chunk {
                        index_build_batch(descriptor(seq, &d, BLOCK as i64, cell));
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let s8 = t.elapsed().as_secs_f64();
        eprintln!(
            "TPUT 8-thread : {:.0} MiB in {:.3}s -> {:.0} MiB/s, {:.0} blocks/s ({:.1}x)",
            mb, s8, mb / s8, N as f64 / s8, s1 / s8
        );
        let payload: Vec<u8> = (0..BLOCK).map(|i| (i % 251) as u8).collect();
        let t = Instant::now();
        for _ in 0..N {
            let _ = Sha256::digest(&payload);
        }
        let sh = t.elapsed().as_secs_f64();
        eprintln!(
            "TPUT sha256   : {:.0} MiB in {:.3}s -> {:.0} MiB/s (pure hash, 1 thread)",
            mb, sh, mb / sh
        );
    }

    // ── T1 SPIKE: Merkle-over-frame-hashes vs whole-block rehash ────────────
    // (record-size-conditional — see the 2026-07-10 spike commit for verdict)
    //   cargo test --release --lib t1_merkle_spike -- --ignored --nocapture
    #[test]
    #[ignore = "explicit T1 spike — run with --ignored --nocapture"]
    fn t1_merkle_spike() {
        use std::time::Instant;
        const MIB: usize = 1024 * 1024;
        const BLOCK: usize = 4 * MIB;
        const REPS: usize = 32;

        fn hex32(d: &[u8]) -> String {
            d.iter().map(|b| format!("{:02x}", b)).collect()
        }
        fn unhex(s: &[u8]) -> [u8; 32] {
            let mut out = [0u8; 32];
            for i in 0..32 {
                let hi = (s[2 * i] as char).to_digit(16).unwrap() as u8;
                let lo = (s[2 * i + 1] as char).to_digit(16).unwrap() as u8;
                out[i] = hi << 4 | lo;
            }
            out
        }
        fn build_framed_block(payload_len: usize) -> (Vec<u8>, usize) {
            let mut block = Vec::with_capacity(BLOCK + 256);
            let mut n = 0usize;
            let frame_len = 64 + 10 + payload_len;
            while block.len() + frame_len <= BLOCK {
                let payload: Vec<u8> = (0..payload_len).map(|i| ((i + n) % 251) as u8).collect();
                let h = hex32(&Sha256::digest(&payload));
                block.extend_from_slice(h.as_bytes());
                block.extend_from_slice(format!("{:010}", payload_len).as_bytes());
                block.extend_from_slice(&payload);
                n += 1;
            }
            (block, n)
        }
        fn parse_leaves(block: &[u8]) -> Vec<[u8; 32]> {
            let mut leaves = Vec::new();
            let mut off = 0usize;
            while off + 74 <= block.len() {
                let leaf = unhex(&block[off..off + 64]);
                let len: usize =
                    std::str::from_utf8(&block[off + 64..off + 74]).unwrap().parse().unwrap();
                leaves.push(leaf);
                off += 74 + len;
            }
            leaves
        }
        fn merkle_tree_root(mut level: Vec<[u8; 32]>) -> [u8; 32] {
            while level.len() > 1 {
                level = level
                    .chunks(2)
                    .map(|pair| {
                        let mut h = Sha256::new();
                        h.update(pair[0]);
                        h.update(*pair.last().unwrap());
                        h.finalize().into()
                    })
                    .collect();
            }
            level[0]
        }
        fn linear_root(leaves: &[[u8; 32]]) -> [u8; 32] {
            let mut h = Sha256::new();
            for l in leaves {
                h.update(l);
            }
            h.finalize().into()
        }

        eprintln!("T1 SPIKE  block=4MiB reps={REPS}  (times are per-block, warm)");
        for payload_len in [100usize, 1024, 8192] {
            let (block, n) = build_framed_block(payload_len);
            let leaf_frac = (n * 32) as f64 / block.len() as f64;
            let t = Instant::now();
            for _ in 0..REPS {
                let _ = Sha256::digest(&block);
            }
            let base = t.elapsed().as_secs_f64() / REPS as f64;
            let t = Instant::now();
            for _ in 0..REPS {
                let leaves = parse_leaves(&block);
                let _ = merkle_tree_root(leaves);
            }
            let tree = t.elapsed().as_secs_f64() / REPS as f64;
            let t = Instant::now();
            for _ in 0..REPS {
                let leaves = parse_leaves(&block);
                let _ = linear_root(&leaves);
            }
            let linear = t.elapsed().as_secs_f64() / REPS as f64;
            let t = Instant::now();
            for _ in 0..REPS {
                let _ = parse_leaves(&block);
            }
            let parse = t.elapsed().as_secs_f64() / REPS as f64;
            eprintln!(
                "T1 rec={:5}B n={:6} leaves={:4.1}% | base {:7.3}ms | tree {:7.3}ms ({:4.1}x) | linear {:7.3}ms ({:4.1}x) | parse-only {:7.3}ms",
                payload_len, n, leaf_frac * 100.0,
                base * 1e3,
                tree * 1e3, base / tree,
                linear * 1e3, base / linear,
                parse * 1e3
            );
        }
    }
}

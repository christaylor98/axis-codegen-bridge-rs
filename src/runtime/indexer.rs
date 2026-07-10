//! AXVERITY_INDEXER_THREADING_V1 — background index-build wait-handler.
//!
//! `index_build_batch` is the `wait()` handler run on each indexer `--entries`
//! worker thread (paired with the thin M1 loop-step
//! `lib_async/indexer_worker_step.m1`, exactly as `hotmem_write`/
//! `wal_fast_batch_write` pair with their janitor steps). `wait()`'s handler
//! slot is a raw untagged `fn(Value) -> Value` at the Rust ABI, which an M1
//! composite cannot fill (its param would be a bare `Value::List`, which the
//! surface parser rejects — CLAUDE.md §15), so the per-block build lives here
//! in Rust rather than in an M1 `foreach`. The Merkle hash is the SAME sha2
//! digest as the bridge's `content_hash`/`bytes_hash`.
//!
//! It consumes the drained batch of seal-event descriptors produced by the
//! seal-site tap's fire-and-forget `channel_send` (an UNBOUNDED, non-blocking
//! hand-off — `SEAL_HANDOFF_O1_NONBLOCKING`; backlog is bounded by the
//! resident-skip-list cap + raw-scan fallback, NOT by backpressure, since
//! backpressure would block the writer). Each descriptor is a
//! `Value::Tuple([Int(block_seq), Str(flush_dir), Int(byte_len),
//! Int(idx_cell_addr)])`, and for each SEALED, DURABLE block this handler:
//!
//!   1. reads the block's flushed bytes READ-ONLY from
//!      `<flush_dir>/block-<block_seq>.bin`. This is the durable, immutable
//!      flush artifact the hotwrite-spike seal path already writes
//!      (`hrw_seal_flush_reclaim`); reading it — rather than the still-resident
//!      hot-mem `ptr` — sidesteps the reclaim/use-after-free lifetime hazard
//!      the (unbuilt) `Arc<BlockLease>` lease of
//!      `decl:hotmem-staging-batch-design-v1` was designed to solve.
//!      `SEALED_BLOCKS_READ_ONLY`: this handler never writes block bytes.
//!   2. computes the Merkle hash (`sha2::Sha256` — byte-identical to
//!      `content_hash`/`bytes_hash`),
//!   3. builds the succinct structure — SEAM STUB only: the real succinct
//!      builder (rank/select structure) is separate M1 work, explicitly out of
//!      scope for AXVERITY_INDEXER_THREADING_V1,
//!   4. writes the index artifact to `<flush_dir>/index/block-<block_seq>.idx`,
//!   5. flips the per-block index-status cell Unindexed(0) -> Indexed(1) with a
//!      SINGLE `compare_exchange` (`ATOMIC_SINGLE_PASS_STATE_FLIP` — one CAS, no
//!      third "partially indexed" state). The flip is LAST, after the artifact
//!      is durably written, so a reader that observes `Indexed` can always find
//!      the artifact. A reader that observes `Unindexed` must fall back to a raw
//!      scan of the block — never a skip (the read-omission-is-unacceptable
//!      invariant). Re-delivery of a descriptor is idempotent: the artifact is
//!      rewritten identically and the second CAS simply fails (still `Indexed`).
//!
//! ## Concurrency posture (checked against Spike-4 / NO_NEW_GLOBAL_MUTEX)
//!
//! This handler holds NO shared bridge state: it touches only per-call file
//! I/O and the caller-owned `AtomicI64` index-status cell (addressed by value,
//! same discipline as `rawmem.rs`'s `cell_cas_raw` — the `Int` a descriptor
//! carries IS the address). There is no process-global mutex, registry, or
//! `HashMap` on this path — so N indexer worker threads running
//! `index_build_batch` concurrently share nothing here. The one shared
//! coordination point in the end-to-end path is the EXISTING channel-registry
//! mutex + global condvar behind `channel_send`/`wait` (`channels.rs`); fanning
//! every shard's seals through it is exactly what the intent's required
//! Spike-4 fan-in next-test measures. This module adds no new one.
//!
//! Each idx cell is a distinct `AtomicI64` per block (minted by the seal tap via
//! `cell_new_raw`), so two workers indexing two different blocks never touch the
//! same cell; two workers racing the SAME block (a duplicate descriptor) is
//! resolved by the CAS (exactly one wins the 0->1 transition; both write the
//! same artifact bytes).

use std::sync::atomic::{AtomicI64, Ordering};

use sha2::{Digest, Sha256};

use super::value::{get_str, Value};

const UNINDEXED: i64 = 0;
const INDEXED: i64 = 1;

fn as_int(field: &'static str, v: Value) -> i64 {
    match v {
        Value::Int(n) => n,
        other => panic!("index_build_batch: {} expected Int, got {:?}", field, other),
    }
}

fn as_text(field: &'static str, v: Value) -> String {
    match v {
        Value::Str(h) => get_str(&h),
        other => panic!("index_build_batch: {} expected Text, got {:?}", field, other),
    }
}

/// `index_build_batch(arg: Value) -> Value`   —   List(Tuple descriptor) -> Unit
///
/// The `wait()` handler. `wait` drains every pending seal descriptor into one
/// `Value::List` and hands the whole batch to one call (`WAIT_ALWAYS_LIST`,
/// `channels.rs`); this is the DESIRED batch-indexing behavior here (each
/// descriptor is an independent, unordered index-build task — no per-message
/// ack/ping-pong coordination is needed, unlike the GC tick/ack loop, because
/// the producer is fire-and-forget).
#[track_caller]
pub fn index_build_batch(arg: Value) -> Value {
    let items = match arg {
        Value::List(items) => items,
        other => panic!("index_build_batch: expected List, got {:?}", other),
    };
    for item in items {
        index_one(item);
    }
    Value::Unit
}

fn index_one(item: Value) {
    // The descriptor is built in M1 via `Value(Int,Text,Int,Int)(..)`, which the
    // bridge emits as a `Value::Ctor { fields }` (NOT a `Value::Tuple` — only raw
    // bridge fns like `mem_reserve_raw` return `Value::Tuple`; see
    // lib_hotwrite_workload/hrw_mint_block.m1's header). Accept BOTH: `Ctor` for
    // the real M1 runtime path, `Tuple` for a bridge-side/test-built descriptor.
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

    // 1. READ-ONLY durable bytes. The flushed .bin is immutable once written by
    //    the seal path; we never touch the (possibly-reclaimed) hot-mem ptr.
    let bin_path = format!("{}/block-{}.bin", flush_dir, block_seq);
    let bytes = std::fs::read(&bin_path)
        .unwrap_or_else(|e| panic!("index_build_batch: read {}: {}", bin_path, e));
    // byte_len is the seal-time recorded length; the durable file is the source
    // of truth for what we hash. A mismatch means a torn/short flush — surface
    // it rather than silently indexing a partial block.
    if byte_len >= 0 && bytes.len() != byte_len as usize {
        panic!(
            "index_build_batch: {} is {} bytes, seal recorded byte_len={} (torn/short flush?)",
            bin_path,
            bytes.len(),
            byte_len
        );
    }

    // 2. Merkle hash — same sha2 digest as content_hash/bytes_hash.
    let digest = Sha256::digest(&bytes);
    let merkle: String = digest.iter().map(|b| format!("{:02x}", b)).collect();

    // 3. Succinct structure — SEAM STUB. Real builder is separate M1 work.
    let succinct = build_succinct_stub(&bytes);

    // 4. Write the index artifact durably BEFORE flipping the status cell.
    let idx_dir = format!("{}/index", flush_dir);
    std::fs::create_dir_all(&idx_dir)
        .unwrap_or_else(|e| panic!("index_build_batch: mkdir {}: {}", idx_dir, e));
    let idx_path = format!("{}/block-{}.idx", idx_dir, block_seq);
    // IDX1 <tab> merkle <tab> len <tab> succinct-stub — one line, self-describing.
    let artifact = format!("IDX1\tsha256:{}\t{}\t{}\n", merkle, bytes.len(), succinct);
    std::fs::write(&idx_path, artifact.as_bytes())
        .unwrap_or_else(|e| panic!("index_build_batch: write {}: {}", idx_path, e));

    // 5. SINGLE-PASS atomic flip Unindexed(0) -> Indexed(1), LAST. Same
    //    address-by-value discipline as rawmem::cell_cas_raw. A failed CAS means
    //    the cell was already Indexed (duplicate descriptor) — no third state.
    let cell = unsafe { &*(idx_cell as *const AtomicI64) };
    let _ = cell.compare_exchange(UNINDEXED, INDEXED, Ordering::SeqCst, Ordering::SeqCst);
}

/// SEAM STUB for the succinct structure. The real rank/select succinct index is
/// separate M1 work, explicitly out of scope for AXVERITY_INDEXER_THREADING_V1;
/// this placeholder records only the block length so the artifact is
/// self-describing and the seam is exercised end-to-end. When the real builder
/// lands, only this function's body changes — the handler, artifact path, and
/// status-cell protocol are stable.
fn build_succinct_stub(bytes: &[u8]) -> String {
    format!("SUCCINCT_STUB len={}", bytes.len())
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
        let d = std::env::temp_dir().join(format!("axv-idx-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.to_string_lossy().into_owned()
    }

    fn descriptor(block_seq: i64, flush_dir: &str, byte_len: i64, idx_cell: i64) -> Value {
        Value::Tuple(vec![
            Value::Int(block_seq),
            Value::Str(super::super::value::intern_str(flush_dir)),
            Value::Int(byte_len),
            Value::Int(idx_cell),
        ])
    }

    fn expected_sha256(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        digest.iter().map(|b| format!("{:02x}", b)).collect()
    }

    #[test]
    fn indexes_one_block_writes_artifact_and_flips_cell() {
        let dir = unique_dir("one");
        let payload = b"PAYLOAD-block-0-contents".to_vec();
        std::fs::write(format!("{}/block-0.bin", dir), &payload).unwrap();
        let cell = mint_cell(UNINDEXED);

        let out = index_build_batch(Value::List(vec![descriptor(
            0,
            &dir,
            payload.len() as i64,
            cell,
        )]));
        assert_eq!(out, Value::Unit);

        // artifact written with the correct sha256 (== bytes_hash's digest)
        let idx = std::fs::read_to_string(format!("{}/index/block-0.idx", dir)).unwrap();
        assert!(idx.starts_with("IDX1\t"), "artifact header: {:?}", idx);
        assert!(
            idx.contains(&format!("sha256:{}", expected_sha256(&payload))),
            "artifact merkle mismatch: {:?}",
            idx
        );
        assert!(idx.contains(&format!("SUCCINCT_STUB len={}", payload.len())));

        // status cell flipped Unindexed(0) -> Indexed(1), single pass
        assert_eq!(cell_load(cell), INDEXED);
    }

    #[test]
    fn accepts_ctor_shape_descriptor_the_m1_runtime_path() {
        // M1's Value(Int,Text,Int,Int)(..) yields Value::Ctor, not Value::Tuple.
        let dir = unique_dir("ctor");
        let payload = b"ctor-path-body".to_vec();
        std::fs::write(format!("{}/block-7.bin", dir), &payload).unwrap();
        let cell = mint_cell(UNINDEXED);
        let desc = Value::Ctor {
            tag: 0,
            fields: vec![
                Value::Int(7),
                Value::Str(super::super::value::intern_str(&dir)),
                Value::Int(payload.len() as i64),
                Value::Int(cell),
            ],
        };
        index_build_batch(Value::List(vec![desc]));
        assert!(std::path::Path::new(&format!("{}/index/block-7.idx", dir)).exists());
        assert_eq!(cell_load(cell), INDEXED);
    }

    #[test]
    fn batch_indexes_all_descriptors() {
        let dir = unique_dir("batch");
        let mut items = Vec::new();
        let mut cells = Vec::new();
        for seq in 0..5i64 {
            let payload = format!("block-{}-body", seq).into_bytes();
            std::fs::write(format!("{}/block-{}.bin", dir, seq), &payload).unwrap();
            let cell = mint_cell(UNINDEXED);
            cells.push(cell);
            items.push(descriptor(seq, &dir, payload.len() as i64, cell));
        }
        index_build_batch(Value::List(items));
        for (seq, cell) in cells.iter().enumerate() {
            assert!(std::path::Path::new(&format!("{}/index/block-{}.idx", dir, seq)).exists());
            assert_eq!(cell_load(*cell), INDEXED, "block {} not flipped", seq);
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
        // second delivery: artifact rewritten identically, CAS fails, still Indexed
        index_build_batch(d());
        assert_eq!(cell_load(cell), INDEXED);
        let idx = std::fs::read_to_string(format!("{}/index/block-0.idx", dir)).unwrap();
        assert!(idx.contains(&format!("sha256:{}", expected_sha256(&payload))));
    }

    #[test]
    #[should_panic(expected = "torn/short flush")]
    fn short_flush_is_surfaced_not_silently_indexed() {
        let dir = unique_dir("short");
        std::fs::write(format!("{}/block-0.bin", dir), b"only-8b?").unwrap(); // 8 bytes
        let cell = mint_cell(UNINDEXED);
        // seal recorded byte_len=999 but the durable file is 8 bytes -> panic
        index_build_batch(Value::List(vec![descriptor(0, &dir, 999, cell)]));
    }
}

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
/// Also accepts a BARE descriptor (Ctor or Tuple, not wrapped in a List): the
/// direct-call path used by the reindex/recovery CLI (`src/index_one.m1`),
/// which invokes this fn as an ordinary registered bridge fn rather than as a
/// wait-handler. Crash recovery = re-run every block through this path; the
/// artifact write is idempotent, so re-indexing survivors is harmless.
///
/// Returns Int(n) = number of blocks indexed this call (Unit-compatible for
/// existing callers, which all discard the result).
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
    let n = items.len();
    for item in items {
        index_one(item);
    }
    Value::Int(n as i64)
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
        assert_eq!(out, Value::Int(1));

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

    // ── Battle-hardening suite (AXVERITY_INDEXER_THREADING_V1 load/edge) ────

    #[test]
    fn bare_descriptor_direct_call_path() {
        // The reindex/recovery CLI path: a single descriptor, no List wrapper.
        let dir = unique_dir("bare");
        let payload = b"bare-descriptor-body".to_vec();
        std::fs::write(format!("{}/block-0.bin", dir), &payload).unwrap();
        let cell = mint_cell(UNINDEXED);
        let out = index_build_batch(descriptor(0, &dir, payload.len() as i64, cell));
        assert_eq!(out, Value::Int(1));
        assert_eq!(cell_load(cell), INDEXED);
        let idx = std::fs::read_to_string(format!("{}/index/block-0.idx", dir)).unwrap();
        assert!(idx.contains(&format!("sha256:{}", expected_sha256(&payload))));
    }

    #[test]
    fn concurrent_hammer_8_threads_200_blocks_each() {
        // Real-world load shape: N workers indexing disjoint blocks
        // concurrently. No shared bridge state on this path — must scale
        // without corruption or lost flips.
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
                let chunk: Vec<(i64, i64, usize)> = all
                    [t * 200..(t + 1) * 200]
                    .iter()
                    .map(|(s, p, c)| (*s, *c, p.len()))
                    .collect();
                let d = dir.clone();
                thread::spawn(move || {
                    for (seq, cell, len) in chunk {
                        index_build_batch(descriptor(seq, &d, len as i64, cell));
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        for (seq, payload, cell) in &all {
            assert_eq!(cell_load(*cell), INDEXED, "block {} not flipped", seq);
            let idx =
                std::fs::read_to_string(format!("{}/index/block-{}.idx", dir, seq)).unwrap();
            assert!(
                idx.contains(&format!("sha256:{}", expected_sha256(payload))),
                "block {} artifact merkle wrong",
                seq
            );
        }
    }

    #[test]
    fn duplicate_race_8_threads_same_block() {
        // Duplicate-descriptor delivery race: 8 threads all indexing the SAME
        // block simultaneously. Exactly-one-CAS-wins; artifact identical from
        // every writer; final state Indexed with a valid artifact.
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
        let idx = std::fs::read_to_string(format!("{}/index/block-0.idx", dir)).unwrap();
        assert!(idx.contains(&format!("sha256:{}", expected_sha256(&payload))));
    }

    #[test]
    fn binary_non_utf8_block_indexes_correctly() {
        // Real-world: block bytes are arbitrary binary, not UTF-8 text. The
        // handler is byte-based end to end (std::fs::read + sha2) — no
        // bytes_to_text anywhere on the indexing path.
        let dir = unique_dir("binary");
        let payload: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        assert!(String::from_utf8(payload.clone()).is_err(), "fixture must be non-UTF8");
        std::fs::write(format!("{}/block-0.bin", dir), &payload).unwrap();
        let cell = mint_cell(UNINDEXED);
        index_build_batch(descriptor(0, &dir, payload.len() as i64, cell));
        assert_eq!(cell_load(cell), INDEXED);
        let idx = std::fs::read_to_string(format!("{}/index/block-0.idx", dir)).unwrap();
        assert!(idx.contains(&format!("sha256:{}", expected_sha256(&payload))));
    }

    #[test]
    fn empty_block_indexes_with_empty_sha() {
        // Edge: a sealed-but-empty block (0 bytes written before rotation).
        let dir = unique_dir("empty");
        std::fs::write(format!("{}/block-0.bin", dir), b"").unwrap();
        let cell = mint_cell(UNINDEXED);
        index_build_batch(descriptor(0, &dir, 0, cell));
        assert_eq!(cell_load(cell), INDEXED);
        let idx = std::fs::read_to_string(format!("{}/index/block-0.idx", dir)).unwrap();
        // sha256 of the empty byte sequence
        assert!(idx.contains("sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"));
        assert!(idx.contains("SUCCINCT_STUB len=0"));
    }

    #[test]
    fn spike_sized_4mib_and_oversize_32mib_blocks() {
        // The hotwrite spike's real block size (4 MiB) and an oversize case
        // (32 MiB — §13's oversize-object precedent: never split, never reject).
        let dir = unique_dir("large");
        for (seq, mib) in [(0i64, 4usize), (1, 32)] {
            let payload: Vec<u8> = (0..mib * 1024 * 1024).map(|i| (i % 251) as u8).collect();
            std::fs::write(format!("{}/block-{}.bin", dir, seq), &payload).unwrap();
            let cell = mint_cell(UNINDEXED);
            index_build_batch(descriptor(seq, &dir, payload.len() as i64, cell));
            assert_eq!(cell_load(cell), INDEXED, "{}MiB block not flipped", mib);
            let idx =
                std::fs::read_to_string(format!("{}/index/block-{}.idx", dir, seq)).unwrap();
            assert!(
                idx.contains(&format!("sha256:{}", expected_sha256(&payload))),
                "{}MiB block merkle wrong",
                mib
            );
        }
    }

    #[test]
    fn ten_thousand_descriptor_batch() {
        // WAIT_ALWAYS_LIST drains everything pending into ONE handler call —
        // a deep backlog arrives as one huge batch. 10k blocks, one call.
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
        for (seq, cell) in cells.iter().enumerate() {
            assert_eq!(cell_load(*cell), INDEXED, "block {} not flipped", seq);
        }
    }

    #[test]
    #[should_panic(expected = "read")]
    fn missing_bin_is_fail_stop() {
        // A descriptor for a block whose .bin never became durable (or was
        // deleted) panics — fail-stop, never a silent skip that would leave
        // the cell honestly Unindexed but the worker convinced it's done.
        let dir = unique_dir("missing");
        let cell = mint_cell(UNINDEXED);
        index_build_batch(descriptor(42, &dir, 10, cell));
    }

    // ── Spike-4 fan-in contention sweep (intent's REQUIRED next-test) ───────
    //
    // Measures the ONE shared coordination point in the seal->indexer path:
    // the channels.rs registry mutex + global condvar behind channel_send/wait.
    // P producer threads flood-send realistic descriptor-shaped messages into
    // one consumer wait-loop; aggregate msgs/sec per P is printed.
    //
    // PASS/FAIL bar (from AXVERITY_INDEXER_THREADING_V1 r2): NO regression
    // toward the Spike-4 thread-path signature — aggregate throughput
    // DEGRADING as producers are added (Spike-4: peak ~273k rec/s @ 8 cores,
    // then FALLING; vs the 1.53M mutex-free process baseline). The hard assert
    // here is the egregious collapse only (agg(16) > 0.5 * agg(1)); the full
    // table is printed for human judgment against the softer bar.
    //
    // Run explicitly:
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
            Value::Str(super::super::value::intern_str(s))
        }

        const TOTAL: usize = 200_000;
        let mut results: Vec<(usize, f64)> = Vec::new();

        for p in [1usize, 4, 16] {
            let chan = format!("bat-fan-{}", p);
            RECEIVED.store(0, Ordering::SeqCst);

            // Dedicated consumer thread: subscribe, drain until TOTAL seen.
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
                            // Descriptor-shaped payload: same Ctor arity the
                            // real seal tap sends.
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
    //
    // Measures the CURRENT pipeline cost per sealed block: disk read of the
    // durable .bin + sha256 (Merkle) + succinct stub + artifact write + CAS
    // flip. Spike-sized 4MiB blocks, warm page cache (each block written just
    // before the timed pass — matches the real seal->index window, where the
    // flush is seconds old and still cached). Single-thread and 8-thread.
    //   cargo test --release --lib index_throughput_probe -- --ignored --nocapture
    #[test]
    #[ignore = "explicit perf probe — run with --ignored --nocapture"]
    fn index_throughput_probe() {
        use std::thread;
        use std::time::Instant;
        const MIB: usize = 1024 * 1024;
        const BLOCK: usize = 4 * MIB;
        const N: usize = 64; // 256 MiB per pass

        let dir = unique_dir("tput");
        let mut work: Vec<(i64, i64)> = Vec::new(); // (seq, cell)
        for seq in 0..N as i64 {
            let payload: Vec<u8> = (0..BLOCK).map(|i| ((i as i64 + seq) % 251) as u8).collect();
            std::fs::write(format!("{}/block-{}.bin", dir, seq), &payload).unwrap();
            work.push((seq, mint_cell(UNINDEXED)));
        }

        // single-thread pass
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

        // 8-thread pass (fresh cells, same cached files)
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

        // isolate the hash cost (the floor a smarter pipeline cannot beat
        // without incremental hashing): sha256 over the same bytes, no I/O.
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

    #[test]
    fn unicode_and_space_flush_dir() {
        // Real-world paths: spaces + non-ASCII directory names.
        let base = unique_dir("uni");
        let dir = format!("{}/flush dir — bloc κ", base);
        std::fs::create_dir_all(&dir).unwrap();
        let payload = b"unicode-dir-body".to_vec();
        std::fs::write(format!("{}/block-0.bin", dir), &payload).unwrap();
        let cell = mint_cell(UNINDEXED);
        index_build_batch(descriptor(0, &dir, payload.len() as i64, cell));
        assert_eq!(cell_load(cell), INDEXED);
        let idx = std::fs::read_to_string(format!("{}/index/block-0.idx", dir)).unwrap();
        assert!(idx.contains(&format!("sha256:{}", expected_sha256(&payload))));
    }
}

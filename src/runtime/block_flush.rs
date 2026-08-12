//! BLOCK_FLUSH_V1 — the async seal-flush worker's `wait()` handler.
//!
//! AXVERITY_PG_HOTBLK_REWIRE_V1 (D039, graphcore/DECISIONS.log): re-pointed
//! off files onto postgres. This is the OFF-THREAD half of graphcore's
//! live-path seal: the sealing producer (graphcore/lib/pg_hotblk_seal_mint.m1
//! for the LOG family, graphcore/lib/gr_obj_flush.m1 for the OBJECT family)
//! does only cheap in-memory work inline (Active->Sealed CAS, a bounded
//! memcpy of the block out of the arena), then fires the block's bytes at
//! THIS worker as a fire-and-forget `channel_send("hotmem-frame", ...)`.
//! graphcore's `gcore_flush_worker` entry (graphcore/src/gcore_flush_worker.m1)
//! is the sole `wait()` caller draining that channel.
//!
//! The worker owns the two expensive / order-sensitive steps:
//!
//!   1. The durable postgres commit — `pg_log_append` for a LOG job,
//!      `pg_obj_block_put` for an OBJECT job (see `super::pg_store`) — moved
//!      off the request thread so a client write never blocks on it
//!      (ASYNC_FLUSH_COMMIT).
//!   2. AFTER that commit succeeds, advance the anchor hash chain
//!      (`advance_anchor`, replicating graphcore/lib/gr_anchor_advance.m1's
//!      logic in Rust — see that function's doc for why). This ordering is
//!      LOAD-BEARING (durable-write-before-anchor-advance): the M1 side no
//!      longer calls gr_anchor_advance itself for the hot path, since doing
//!      so synchronously with the fire-and-forget send would race ahead of
//!      the actual commit.
//!
//! NO_FILE_IO_RETURN (D039) — SUPERSEDED for the OBJECT family by D048
//! (OBJSEG_V1): `pg_obj_block_put` now appends the sealed block's bytes to a
//! fixed-size preallocated segment file (`objseg.rs`) as one pwrite+fsync;
//! `gcore_objects` in postgres holds only the `(segment_id, offset, len)`
//! pointer, never content. This is NOT a reversion to the pre-D039 disease
//! (no per-object files, no `.axverity/hotblocks` shard machinery, no
//! `fs_*` calls from M1) — the file I/O lives entirely in the Rust bridge,
//! one write per flushed BLOCK (many objects), never one per object. The
//! LOG family (`pg_log_append`) and the anchor (`pg_anchor_set`) are
//! unchanged, still pure postgres rows. The index-frame seal->indexer notify
//! is RETIRED (not
//! reinvented): graphcore runs no indexer (INDEXER_NOTIFY, deferred), so
//! there is nothing to notify.
//!
//! ## Why this is a Rust builtin (not M1)
//!
//! It is a `wait()` handler, and `wait`'s handler slot is a raw
//! `fn(Value) -> Value` at the Rust ABI — an M1 composite cannot fill it.
//! This is I/O glue (job-parse + durable commit + anchor advance), not seal
//! logic — all seal logic stays in the M1 producers.
//!
//! ## Job encoding (two shapes, dispatched by field count)
//!
//!   * 5-field `Value::Ctor`/`Tuple` `(Int, Text, Int, Int, Bytes)` — the
//!     shape `pg_hotblk_job` (identity-pinned, unchanged from
//!     AXVERITY_FRONTEND_WRITEPATH_INTEGRATION_V1) still constructs, used
//!     for the LOG family. Routed to `pg_log_append`. AXVERITY_HOTBLK_POOL_
//!     WIRE_V1 (D044 Phase 1) + AXVERITY_HOTBLK_TIMED_FLUSH_V1 (D044 Phase
//!     2) repurpose three of the four previously-inert leading fields — the
//!     TYPE signature (and therefore the identity-pinned ABI, §6: identity
//!     is sha256(name) only, never contract-aware) is unchanged, only what
//!     the values MEAN:
//!       field 0 (Int)  — the sealed block's arena `ptr` (was inert Int(0))
//!       field 1 (Text) — the hotblk_pool shard name this block belongs to,
//!                        "" if the producer isn't pool-managed (was inert
//!                        Text(""))
//!       field 2 (Int)  — Phase 2: the block's generation (hotblk register
//!                        slot 7 at seal time), used to dedup against
//!                        whatever the timer thread already checkpointed
//!                        for this same generation (was inert Int(0))
//!       field 3        — still inert (Int(0))
//!       field 4 (Bytes)— the sealed block's FULL bytes (from offset 0),
//!                        unchanged — `commit_job` slices off whatever
//!                        prefix a checkpoint already committed, see below
//!     See graphcore/lib/pg_hotblk_job.m1's header.
//!   * 2-field `Value::Ctor`/`Tuple` `(Bytes, Text)` — `(block, index)`, the
//!     OBJECT family's own shape (graphcore/lib/gr_obj_flush.m1 builds it
//!     directly; there is no identity-pinned producer for this family — see
//!     that file's header for why). Routed to `pg_obj_block_put`. Not
//!     pool-managed (D044 Phase 1 scopes the LOG family only).
//!   * 3-field `Value::Ctor`/`Tuple` `(Int, Int, Int)` — AXVERITY_HOTBLK_
//!     TIMED_FLUSH_V1 (D044 Phase 2): `pg_hotblk_checkpoint`'s shape,
//!     `(generation, ptr, to)`. Sent by the independent timer thread
//!     (`graphcore/src/gcore_timer.m1`) — a "flush roughly up to here" ask,
//!     never the sole truth; see the watermark section below for how it's
//!     reconciled against the LOG family's own seal jobs.
//!
//! Identities are sha256(name_utf8), the bridge-wide convention.
//!
//! ## AXVERITY_HOTBLK_TIMED_FLUSH_V1 (D044 Phase 2) — the checkpoint watermark
//!
//! `CHECKPOINT_WATERMARK` tracks `(generation, bytes_already_committed)`
//! for the LOG family's single active block (see `hotblk.rs`'s own doc
//! comment: there is exactly one such block today, gcore_serve is
//! single-threaded, D007). Both a seal's LOG job (field 2 = its
//! generation) and a timer's Checkpoint job (field 0 = its generation)
//! reconcile against the SAME tracker via `advance_watermark`, so whichever
//! trigger reaches the flush worker first — volume or time — the other
//! never double-commits the overlap:
//!   - `generation` older than the tracked one: entirely stale (the writer
//!     has since rotated past it, its own seal already committed
//!     everything that could ever matter) — dropped, untouched.
//!   - `generation` equal to the tracked one: commit only the bytes past
//!     the tracked watermark; if the requested `to` doesn't exceed it,
//!     there's nothing new — dropped.
//!   - `generation` newer than the tracked one: this is the first job this
//!     watermark tracker has seen for it — commit from 0.
//!
//! Safe against races because `commit_job` runs on the SOLE flush-worker
//! thread, processing the "hotmem-frame" channel strictly in enqueue
//! order (FIFO), and a block only ever returns to `hotblk_pool` — making
//! its `ptr` eligible for a NEW mint to reuse — as part of THIS SAME
//! thread finishing that generation's own seal job. So by the time this
//! thread could possibly process a job for generation G+1, generation G's
//! seal job (enqueued strictly earlier — the writer cannot observe/publish
//! G+1 until AFTER G's seal_mint call, which is what enqueues G's seal
//! job, already ran) must already have been processed, and no other
//! thread ever touches `ptr` in between. A checkpoint job that loses this
//! race against its own seal isn't unsafe, just redundant — the
//! `to`-vs-watermark check on the SAME generation catches it too.
//!
//! ## AXVERITY_HOTBLK_POOL_WIRE_V1 (D044 Phase 1) — the free-marker protocol
//!
//! A pool-managed block (non-empty `shard`) is only eligible for reuse once
//! ITS bytes are durably committed here. `commit_job`'s LOG arm, after
//! `pg_log_append` + `advance_anchor` succeed, mints a fresh Active-state
//! cell (`rawmem::cell_new_raw`, same call `pg_hotblk_mint.m1` used to make
//! inline) and returns `(ptr, cell)` to `hotblk_pool::pool_return(shard,
//! ...)` — the NON-BLOCKING return path (`pool_put`, the allocator's own
//! blocking-at-cap push, is deliberately NOT used here: the allocator races
//! ahead and re-fills the queue to cap almost immediately after every
//! `pool_take`, so a same-cap blocking return routinely finds the queue
//! full and deadlocks the sole flush-worker thread inside this very call —
//! reproduced directly via graphcore's tests/run.sh P3 stale-detection
//! case before this fix; see `pool_return`'s own doc for the full trace).
//! Nothing else in this codebase ever calls `pool_return`/`pool_put` for
//! the LOG family — so a block genuinely cannot be handed back out to a new
//! writer (`hotblk_pool_take`) while a flush against it is still in
//! flight; the only path back into circulation runs through this exact
//! commit succeeding first. The old (now-Sealed) cell is left leaked — cells are
//! permanent for the process lifetime by explicit prior decision
//! (rawmem.rs's own doc comment on `cell_new_raw`), and this doesn't change
//! that leak rate: today's pre-Phase-1 seal_mint already leaked one cell
//! per rotate synchronously; this moves the same one leak to the same one
//! rotate, just after the flush instead of before.

use std::sync::Mutex;

use super::hotblk_pool;
use super::pg_store;
use super::rawmem;
use super::value::{get_str, intern_str, Value};

/// D044 Phase 2: `(generation, bytes_already_committed)` for the LOG
/// family's single active block. Private to the sole flush-worker thread's
/// own sequential processing — the `Mutex` is never contended (there is
/// nothing else to contend with), it's just the safe way to hold mutable
/// process-wide state without `unsafe`. See this module's own "checkpoint
/// watermark" doc section for the reconciliation rules.
static CHECKPOINT_WATERMARK: Mutex<(i64, i64)> = Mutex::new((0, 0));

/// Reconcile a job's `(generation, to)` against the tracked watermark.
/// Returns `Some(from)` — the byte offset this job should commit starting
/// from — and advances the tracker to `(generation, to)`. Returns `None`
/// (nothing new, or genuinely stale) without touching the tracker's
/// watermark, except adopting a strictly-newer `generation` even on a
/// no-op call so a LATER job for that same generation computes `from`
/// relative to `to`, not incorrectly re-derives 0.
fn advance_watermark(generation: i64, to: i64) -> Option<i64> {
    let mut wm = CHECKPOINT_WATERMARK.lock().unwrap();
    let (cur_gen, cur_wm) = *wm;
    if generation < cur_gen {
        return None; // strictly stale -- the writer has moved on
    }
    let from = if generation == cur_gen { cur_wm } else { 0 };
    if to <= from {
        if generation > cur_gen {
            *wm = (generation, from);
        }
        return None; // nothing new since the tracked watermark
    }
    *wm = (generation, to);
    Some(from)
}

fn as_bytes(field: &'static str, v: Value) -> Vec<u8> {
    match v {
        Value::Bytes(b) => b,
        other => panic!("block_flush_write: {} expected Bytes, got {:?}", field, other),
    }
}

fn as_text(field: &'static str, v: Value) -> String {
    match v {
        Value::Str(h) => get_str(&h),
        other => panic!("block_flush_write: {} expected Text, got {:?}", field, other),
    }
}

fn as_int(field: &'static str, v: Value) -> i64 {
    match v {
        Value::Int(n) => n,
        other => panic!("block_flush_write: {} expected Int, got {:?}", field, other),
    }
}

/// One parsed, ready-to-commit flush job.
enum Job {
    /// LOG family: the sealed block's FULL bytes (from offset 0), one
    /// `pg_log_append` row after watermark-slicing. `ptr`/`shard` identify
    /// the arena block for the free-marker protocol (D044 Phase 1) —
    /// `shard` empty means "not pool-managed, don't return anything, don't
    /// watermark-slice either" (Phase 2 only reconciles pool-managed
    /// blocks, since only those can ever collide with a timer checkpoint).
    /// `generation` is Phase 2's dedup key (D044 Phase 2).
    Log { ptr: i64, shard: String, generation: i64, bytes: Vec<u8> },
    /// OBJECT family: a sealed arena block plus its pending index
    /// (`"<addr>\t<off>\t<len>\n"` lines), committed as one
    /// `pg_obj_block_put` transaction.
    Obj { block: Vec<u8>, index: String },
    /// D044 Phase 2: a timer-triggered "flush roughly up to here" ask —
    /// `pg_hotblk_checkpoint`'s shape, `(generation, ptr, to)`.
    Checkpoint { generation: i64, ptr: i64, to: i64 },
}

/// Parse one drained channel item into a [`Job`]. Dispatches on field count
/// — 5 fields is a LOG job (pg_hotblk_job's identity-pinned shape), 2
/// fields is an OBJECT job (gr_obj_flush's own shape), 3 fields is a D044
/// Phase 2 Checkpoint job (pg_hotblk_checkpoint's shape). Panics on any
/// other shape: unlike the old framed-Bytes fallback this replaces, there
/// is no defensively-unreachable encoding here to tolerate — every
/// producer is graphcore's own M1, in this same repo, so a malformed job
/// is a genuine producer bug and should fail loud.
fn parse_job(item: Value) -> Job {
    match item {
        Value::Ctor { fields, .. } | Value::Tuple(fields) if fields.len() == 5 => {
            let mut it = fields.into_iter();
            let ptr = as_int("ptr", it.next().unwrap());
            let shard = as_text("shard", it.next().unwrap());
            let generation = as_int("generation", it.next().unwrap());
            let _f3 = it.next().unwrap();
            let bytes = as_bytes("bytes", it.next().unwrap());
            Job::Log { ptr, shard, generation, bytes }
        }
        Value::Ctor { fields, .. } | Value::Tuple(fields) if fields.len() == 2 => {
            let mut it = fields.into_iter();
            let block = as_bytes("block", it.next().unwrap());
            let index = as_text("index", it.next().unwrap());
            Job::Obj { block, index }
        }
        Value::Ctor { fields, .. } | Value::Tuple(fields) if fields.len() == 3 => {
            let mut it = fields.into_iter();
            let generation = as_int("generation", it.next().unwrap());
            let ptr = as_int("ptr", it.next().unwrap());
            let to = as_int("to", it.next().unwrap());
            Job::Checkpoint { generation, ptr, to }
        }
        other => panic!(
            "block_flush_write: expected a 5-field LOG job, a 2-field OBJECT job, or a 3-field Checkpoint job, got {:?}",
            other
        ),
    }
}

/// Replicate graphcore/lib/gr_anchor_advance.m1's chain-hash logic in Rust,
/// called AFTER the durable commit succeeds so the ordering the M1 fn used
/// to guarantee synchronously (durable-write-before-anchor-advance) still
/// holds now that the commit itself runs off the request thread. The M1
/// gr_anchor_advance fn is no longer called from the hot path (calling it
/// synchronously with the fire-and-forget send would race ahead of the
/// actual commit) — it is intentionally left in place, unreferenced, for
/// any other caller.
///
/// Chain rule (byte-identical to gr_anchor_advance.m1):
///   prev_hex = current anchor's hex prefix, up to the first '\n' ("" pre-
///              first-flush, matching pg_anchor_get's own "" convention)
///   new_hex  = sha256(prev_hex_bytes ++ committed_block_bytes), the hex
///              digits after "sha256:"
///   content  = new_hex ++ "\n", committed via pg_anchor_set
fn advance_anchor(committed_block: &[u8]) {
    let anchor_raw = match pg_store::pg_anchor_get(Value::Unit) {
        Value::Str(h) => get_str(&h),
        other => panic!("block_flush_write: pg_anchor_get returned non-Text: {:?}", other),
    };
    let prev_hex = anchor_raw.split('\n').next().unwrap_or("");
    let mut combined = prev_hex.as_bytes().to_vec();
    combined.extend_from_slice(committed_block);
    let hashed = super::bytes_io::bytes_hash(Value::Bytes(combined));
    let hex_with_prefix = match hashed {
        Value::Str(h) => get_str(&h),
        other => panic!("block_flush_write: bytes_hash returned non-Text: {:?}", other),
    };
    let new_hex = hex_with_prefix.strip_prefix("sha256:").unwrap_or(&hex_with_prefix);
    let content = format!("{}\n", new_hex);
    pg_store::pg_anchor_set(Value::Str(intern_str(&content)));
}

/// Commit one job to postgres, then advance the anchor over the bytes that
/// just became durable.
fn commit_job(job: Job) {
    match job {
        Job::Log { ptr, shard, generation, bytes } => {
            // D044 Phase 2: for a pool-managed block, only commit whatever
            // a timer checkpoint hasn't already durably flushed for this
            // generation (see this module's "checkpoint watermark" doc
            // section). A non-pool-managed job (empty shard) never
            // reconciles against the tracker at all -- Phase 2 only ever
            // exists for pool-managed blocks, so there is nothing for it
            // to have collided with.
            let to_commit: &[u8] = if shard.is_empty() {
                &bytes[..]
            } else {
                match advance_watermark(generation, bytes.len() as i64) {
                    Some(from) => &bytes[from as usize..],
                    None => &[],
                }
            };
            if !to_commit.is_empty() {
                let text = String::from_utf8(to_commit.to_vec()).unwrap_or_else(|e| {
                    panic!("block_flush_write: log block is not valid UTF-8: {}", e)
                });
                pg_store::pg_log_append(Value::Str(intern_str(&text)));
                advance_anchor(to_commit);
            }
            // Free-marker protocol (D044 Phase 1): only now, after the
            // commit above (if any) is durable, is this block eligible for
            // reuse. Unconditional on to_commit being non-empty -- the
            // block is DONE being written to regardless of whether a
            // checkpoint already covered all of it.
            if !shard.is_empty() {
                let cell = as_int("fresh cell", rawmem::cell_new_raw(Value::Int(1)));
                hotblk_pool::pool_return(&shard, ptr, cell);
            }
        }
        Job::Obj { block, index } => {
            pg_store::pg_obj_block_put(Value::Tuple(vec![
                Value::Bytes(block.clone()),
                Value::Str(intern_str(&index)),
            ]));
            advance_anchor(&block);
        }
        Job::Checkpoint { generation, ptr, to } => {
            let Some(from) = advance_watermark(generation, to) else {
                return; // stale or redundant -- drop silently, per design
            };
            let len = to - from;
            if len <= 0 {
                return;
            }
            let delta = match rawmem::mem_read_raw(Value::Tuple(vec![
                Value::Int(ptr),
                Value::Int(from),
                Value::Int(len),
            ])) {
                Value::Bytes(b) => b,
                other => panic!("block_flush_write: mem_read_raw returned non-Bytes: {:?}", other),
            };
            let text = String::from_utf8(delta.clone()).unwrap_or_else(|e| {
                panic!("block_flush_write: checkpoint delta is not valid UTF-8: {}", e)
            });
            pg_store::pg_log_append(Value::Str(intern_str(&text)));
            advance_anchor(&delta);
        }
    }
}

/// `block_flush_write(arg: Value) -> Value` — the `wait()` handler.
///
/// Drains a `List` of seal-flush jobs (or a bare job on the direct-call
/// path), commits each to postgres, advancing the anchor chain after each
/// commit. Returns `Int(n)` = jobs committed this call.
#[track_caller]
pub fn block_flush_write(arg: Value) -> Value {
    let items = match arg {
        Value::List(items) => items,
        Value::Unit => return Value::Unit,
        bare @ (Value::Ctor { .. } | Value::Tuple(_)) => vec![bare],
        other => panic!(
            "block_flush_write: expected List of jobs or a bare job, got {:?}",
            other
        ),
    };
    let mut n = 0i64;
    for item in items {
        commit_job(parse_job(item));
        n += 1;
    }
    Value::Int(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctor_log_job(bytes: &[u8]) -> Value {
        Value::Ctor {
            tag: 0,
            fields: vec![
                Value::Int(0),
                Value::Str(intern_str("")),
                Value::Int(0),
                Value::Int(0),
                Value::Bytes(bytes.to_vec()),
            ],
        }
    }

    fn tuple_obj_job(block: &[u8], index: &str) -> Value {
        Value::Tuple(vec![Value::Bytes(block.to_vec()), Value::Str(intern_str(index))])
    }

    #[test]
    fn empty_drain_is_noop() {
        assert_eq!(block_flush_write(Value::Unit), Value::Unit);
        assert_eq!(block_flush_write(Value::List(vec![])), Value::Int(0));
    }

    // The remaining tests hit the local postgres (peer-auth superuser on the
    // unix socket; pg_store's auto-provision path gives each test process its
    // own fresh scratch DB — but ALL tests in this crate's test binary share
    // ONE process, and therefore ONE scratch DB (pg_store::conn() is a
    // process-wide OnceLock). Unique content/addresses make these resilient
    // to OTHER concurrent siblings of their own kind, but pg_store.rs's own
    // `round_trips` test (read-only this turn — not ours to change) asserts
    // EXACT log-scan content ("L1\nL2\n"), which a concurrently-running log
    // commit from here corrupts (verified directly: running the full `cargo
    // test --release --lib` turned that assertion red the first time these
    // were added, unignored). #[ignore] here, same as this codebase's other
    // tests with an irreducible shared-state hazard (e.g.
    // indexer.rs::contention_fanin_sweep) — run explicitly and individually
    // (`cargo test --release --lib block_flush -- --ignored --nocapture`),
    // which all three were verified to pass under.

    #[test]
    #[ignore = "shares pg_store's process-wide scratch DB with pg_store::tests::round_trips — run explicitly, not as part of the default suite"]
    fn log_job_commits_and_advances_anchor() {
        let marker = format!("block_flush_log_test_marker_{}\n", std::process::id());
        let before = match pg_store::pg_anchor_get(Value::Unit) {
            Value::Str(h) => get_str(&h),
            other => panic!("expected Text, got {:?}", other),
        };
        let out = block_flush_write(Value::List(vec![ctor_log_job(marker.as_bytes())]));
        assert_eq!(out, Value::Int(1));
        let scan = match pg_store::pg_log_scan(Value::Unit) {
            Value::Str(h) => get_str(&h),
            other => panic!("expected Text, got {:?}", other),
        };
        assert!(scan.contains(&marker), "log scan should contain the committed marker");
        let after = match pg_store::pg_anchor_get(Value::Unit) {
            Value::Str(h) => get_str(&h),
            other => panic!("expected Text, got {:?}", other),
        };
        assert_ne!(before, after, "anchor must advance after a durable commit");
    }

    #[test]
    #[ignore = "shares pg_store's process-wide scratch DB with pg_store::tests::round_trips — run explicitly, not as part of the default suite"]
    fn log_job_with_shard_returns_block_to_pool() {
        // D044 Phase 1 free-marker protocol: a non-empty shard field means
        // this job's block is pool-managed, and committing it must return
        // (ptr, a freshly-minted cell) to that shard's pool -- a subsequent
        // pool_take on the SAME shard must succeed immediately (not block)
        // and hand back the ptr this job carried.
        let shard = format!("block_flush_pool_test_shard_{}", std::process::id());
        let marker = format!("block_flush_pool_test_marker_{}\n", std::process::id());
        let ptr = 0x1234_5678i64; // synthetic -- never dereferenced by this test
        let job = Value::Ctor {
            tag: 0,
            fields: vec![
                Value::Int(ptr),
                Value::Str(intern_str(&shard)),
                Value::Int(0),
                Value::Int(0),
                Value::Bytes(marker.into_bytes()),
            ],
        };
        let out = block_flush_write(Value::List(vec![job]));
        assert_eq!(out, Value::Int(1));
        let (taken_ptr, taken_cell) = hotblk_pool::pool_take(&shard);
        assert_eq!(taken_ptr, ptr, "pool must hand back the same ptr the committed job carried");
        assert!(taken_cell != 0, "returned cell must be a real minted AtomicI64 address");
    }

    #[test]
    #[ignore = "shares pg_store's process-wide scratch DB (incl. the single global anchor row) with pg_store::tests::round_trips — run explicitly, not as part of the default suite"]
    fn obj_job_commits_sliced_objects_and_advances_anchor() {
        let addr_x = format!("sha256:blockflushtestx{}", std::process::id());
        let addr_y = format!("sha256:blockflushtesty{}", std::process::id());
        let block = b"AAAthequickBBBbrownfox".to_vec();
        let index = format!("{}\t3\t8\n{}\t14\t8\n", addr_x, addr_y);
        let before = match pg_store::pg_anchor_get(Value::Unit) {
            Value::Str(h) => get_str(&h),
            other => panic!("expected Text, got {:?}", other),
        };
        let out = block_flush_write(Value::List(vec![tuple_obj_job(&block, &index)]));
        assert_eq!(out, Value::Int(1));
        match pg_store::pg_bytes_get(Value::Str(intern_str(&addr_x))) {
            Value::Bytes(b) => assert_eq!(b, b"thequick"),
            other => panic!("expected Bytes, got {:?}", other),
        }
        match pg_store::pg_bytes_get(Value::Str(intern_str(&addr_y))) {
            Value::Bytes(b) => assert_eq!(b, b"brownfox"),
            other => panic!("expected Bytes, got {:?}", other),
        }
        let after = match pg_store::pg_anchor_get(Value::Unit) {
            Value::Str(h) => get_str(&h),
            other => panic!("expected Text, got {:?}", other),
        };
        assert_ne!(before, after, "anchor must advance after a durable commit");
    }

    #[test]
    #[ignore = "shares pg_store's process-wide scratch DB with pg_store::tests::round_trips — run explicitly, not as part of the default suite"]
    fn mixed_batch_commits_both_kinds() {
        let marker = format!("block_flush_mixed_test_marker_{}\n", std::process::id());
        let addr = format!("sha256:blockflushmixed{}", std::process::id());
        let block = b"mixedbatchpayload".to_vec();
        let index = format!("{}\t0\t17\n", addr);
        let out = block_flush_write(Value::List(vec![
            ctor_log_job(marker.as_bytes()),
            tuple_obj_job(&block, &index),
        ]));
        assert_eq!(out, Value::Int(2));
        let scan = match pg_store::pg_log_scan(Value::Unit) {
            Value::Str(h) => get_str(&h),
            other => panic!("expected Text, got {:?}", other),
        };
        assert!(scan.contains(&marker));
        match pg_store::pg_bytes_get(Value::Str(intern_str(&addr))) {
            Value::Bytes(b) => assert_eq!(b, b"mixedbatchpayload"),
            other => panic!("expected Bytes, got {:?}", other),
        }
    }

    // D044 Phase 2 tests. CHECKPOINT_WATERMARK is process-wide global state
    // (same reasoning as pg_store's scratch DB above, and hotblk.rs's
    // ACTIVE_BLOCK) -- #[ignore], run explicitly, one at a time.

    fn ctor_log_job_g(ptr: i64, shard: &str, generation: i64, bytes: &[u8]) -> Value {
        Value::Ctor {
            tag: 0,
            fields: vec![
                Value::Int(ptr),
                Value::Str(intern_str(shard)),
                Value::Int(generation),
                Value::Int(0),
                Value::Bytes(bytes.to_vec()),
            ],
        }
    }

    fn ctor_checkpoint_job(generation: i64, ptr: i64, to: i64) -> Value {
        Value::Tuple(vec![Value::Int(generation), Value::Int(ptr), Value::Int(to)])
    }

    #[test]
    #[ignore = "shares CHECKPOINT_WATERMARK (process-wide) and pg_store's scratch DB — run explicitly, not as part of the default suite"]
    fn checkpoint_then_seal_does_not_duplicate() {
        // A real arena so the Checkpoint job's mem_read_raw has something
        // legitimate to read (unlike the LOG-job tests above, which never
        // exercise mem_read_raw since they hand bytes straight through).
        let full = format!("checkpoint-then-seal-payload-{}\n", std::process::id());
        let full = full.as_bytes();
        let cap = rawmem::mem_reserve_raw(Value::Int(full.len() as i64 + 16));
        let ptr = match &cap {
            Value::Tuple(es) => match &es[0] {
                Value::Int(p) => *p,
                other => panic!("expected Int ptr, got {:?}", other),
            },
            other => panic!("expected Tuple, got {:?}", other),
        };
        rawmem::mem_write_raw(Value::Tuple(vec![
            Value::Int(ptr),
            Value::Int(0),
            Value::Bytes(full.to_vec()),
        ]));

        let generation = 900_000_000 + std::process::id() as i64;
        let split = full.len() as i64 / 2;

        // Timer checkpoints the first half.
        let out = block_flush_write(Value::List(vec![ctor_checkpoint_job(generation, ptr, split)]));
        assert_eq!(out, Value::Int(1));

        // Seal arrives with the FULL bytes (as pg_hotblk_seal_mint.m1
        // always sends them, from offset 0) for the SAME generation.
        let shard = format!("checkpoint-test-shard-{}", std::process::id());
        let out = block_flush_write(Value::List(vec![ctor_log_job_g(ptr, &shard, generation, full)]));
        assert_eq!(out, Value::Int(1));

        // The durable log must contain the marker text exactly ONCE, not
        // duplicated by the seal re-committing what the checkpoint already
        // flushed.
        let scan = match pg_store::pg_log_scan(Value::Unit) {
            Value::Str(h) => get_str(&h),
            other => panic!("expected Text, got {:?}", other),
        };
        let marker = std::str::from_utf8(full).unwrap().trim_end();
        assert_eq!(
            scan.matches(marker).count(),
            1,
            "checkpoint + its own seal must commit the marker exactly once, not duplicate it"
        );

        // The block was still returned to the pool despite the checkpoint
        // having covered part of it -- the seal's own free-marker step is
        // unconditional (see commit_job's comment).
        let (taken_ptr, _cell) = hotblk_pool::pool_take(&shard);
        assert_eq!(taken_ptr, ptr, "seal must still return the block to its pool");
    }

    #[test]
    #[ignore = "shares CHECKPOINT_WATERMARK (process-wide) — run explicitly, not as part of the default suite"]
    fn stale_checkpoint_is_dropped_without_committing() {
        let generation_new = 900_100_000 + std::process::id() as i64;
        let generation_old = generation_new - 1; // strictly older, distinct range

        // Advance the tracker to the NEWER generation first (simulates the
        // writer having already rotated past what a late checkpoint still
        // thinks is current).
        let shard = format!("stale-checkpoint-shard-{}", std::process::id());
        let marker = format!("stale-checkpoint-marker-{}\n", std::process::id());
        let out = block_flush_write(Value::List(vec![ctor_log_job_g(
            0, // ptr unused by this test -- no checkpoint here ever reads it
            &shard,
            generation_new,
            marker.as_bytes(),
        )]));
        assert_eq!(out, Value::Int(1));

        let before = match pg_store::pg_log_scan(Value::Unit) {
            Value::Str(h) => get_str(&h),
            other => panic!("expected Text, got {:?}", other),
        };

        // A checkpoint for the OLDER generation, arriving late, must be
        // dropped -- silently, no panic, and critically no commit (it
        // carries a bogus ptr on purpose; if this were mistakenly treated
        // as live, mem_read_raw on it would be a real memory violation).
        let out = block_flush_write(Value::List(vec![ctor_checkpoint_job(
            generation_old,
            i64::MAX / 2, // deliberately bogus -- must never be dereferenced
            999_999,
        )]));
        assert_eq!(out, Value::Int(1), "block_flush_write still counts the job as processed, even when dropped");

        let after = match pg_store::pg_log_scan(Value::Unit) {
            Value::Str(h) => get_str(&h),
            other => panic!("expected Text, got {:?}", other),
        };
        assert_eq!(before, after, "a stale checkpoint must commit nothing");
    }
}

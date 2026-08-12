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
//! NO_FILE_IO_RETURN: no `.axverity/hotblocks` paths, no `block-<seq>.bin`,
//! no `flush_dir`, no shard machinery, no `fs_*` in the store path — the
//! only durable writes are `pg_obj_block_put` / `pg_log_append` /
//! `pg_anchor_set`. The index-frame seal->indexer notify is RETIRED (not
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
//!     WIRE_V1 (D044 Phase 1) repurposes two of the four previously-inert
//!     leading fields — the TYPE signature (and therefore the identity-
//!     pinned ABI, §6: identity is sha256(name) only, never contract-aware)
//!     is unchanged, only what the values MEAN:
//!       field 0 (Int)  — the sealed block's arena `ptr` (was inert Int(0))
//!       field 1 (Text) — the hotblk_pool shard name this block belongs to,
//!                        "" if the producer isn't pool-managed (was inert
//!                        Text(""))
//!       fields 2,3     — still inert (Int(0), Int(0))
//!       field 4 (Bytes)— the sealed block's bytes, unchanged
//!     See graphcore/lib/pg_hotblk_job.m1's header.
//!   * 2-field `Value::Ctor`/`Tuple` `(Bytes, Text)` — `(block, index)`, the
//!     OBJECT family's own shape (graphcore/lib/gr_obj_flush.m1 builds it
//!     directly; there is no identity-pinned producer for this family — see
//!     that file's header for why). Routed to `pg_obj_block_put`. Not
//!     pool-managed (D044 Phase 1 scopes the LOG family only).
//!
//! Identities are sha256(name_utf8), the bridge-wide convention.
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

use super::hotblk_pool;
use super::pg_store;
use super::rawmem;
use super::value::{get_str, intern_str, Value};

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
    /// LOG family: the accumulated ledger bytes, committed as one
    /// `pg_log_append` row. `ptr`/`shard` identify the arena block this
    /// came from for the free-marker protocol (D044 Phase 1) — `shard`
    /// empty means "not pool-managed, don't return anything."
    Log { ptr: i64, shard: String, bytes: Vec<u8> },
    /// OBJECT family: a sealed arena block plus its pending index
    /// (`"<addr>\t<off>\t<len>\n"` lines), committed as one
    /// `pg_obj_block_put` transaction.
    Obj { block: Vec<u8>, index: String },
}

/// Parse one drained channel item into a [`Job`]. Dispatches on field count
/// — 5 fields is always a LOG job (pg_hotblk_job's identity-pinned shape),
/// 2 fields is always an OBJECT job (gr_obj_flush's own shape). Panics on
/// any other shape: unlike the old framed-Bytes fallback this replaces,
/// there is no defensively-unreachable encoding here to tolerate — both
/// producers are graphcore's own M1, in this same repo, so a malformed job
/// is a genuine producer bug and should fail loud.
fn parse_job(item: Value) -> Job {
    match item {
        Value::Ctor { fields, .. } | Value::Tuple(fields) if fields.len() == 5 => {
            let mut it = fields.into_iter();
            let ptr = as_int("ptr", it.next().unwrap());
            let shard = as_text("shard", it.next().unwrap());
            let _f2 = it.next().unwrap();
            let _f3 = it.next().unwrap();
            let bytes = as_bytes("bytes", it.next().unwrap());
            Job::Log { ptr, shard, bytes }
        }
        Value::Ctor { fields, .. } | Value::Tuple(fields) if fields.len() == 2 => {
            let mut it = fields.into_iter();
            let block = as_bytes("block", it.next().unwrap());
            let index = as_text("index", it.next().unwrap());
            Job::Obj { block, index }
        }
        other => panic!(
            "block_flush_write: expected a 5-field LOG job or a 2-field OBJECT job, got {:?}",
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
        Job::Log { ptr, shard, bytes } => {
            let text = String::from_utf8(bytes.clone())
                .unwrap_or_else(|e| panic!("block_flush_write: log block is not valid UTF-8: {}", e));
            pg_store::pg_log_append(Value::Str(intern_str(&text)));
            advance_anchor(&bytes);
            // Free-marker protocol (D044 Phase 1): only now, after the
            // commit above is durable, is this block eligible for reuse.
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
}

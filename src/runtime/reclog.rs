//! BRIDGE_RECLOG_V1 (AXVERITY_HOTPATH_PARALLEL_DISPATCH_V1) — the recovery-log
//! group-commit writer: the batched, durable, ack-backpath INSERT write path
//! that replaces the caller's two sequential fsyncs (payload WAL + name-log
//! bind) with ONE batched barrier per window, gating the client ack.
//!
//! ## The topology this realizes (from the intent's through-line)
//!
//!   worker thread (pg_exec_insert):
//!       reclog_submit(frame, name_log_path, bind_line) -> oneshot id     [enqueue+mint]
//!       oneshot_wait(id)                                                 [block until durable]
//!       ack
//!   reclog janitor thread (loop):
//!       reclog_flush_once()   ── drain a batch, write payload + bind,
//!                                fsync, signal every caller's oneshot
//!
//! `reclog_submit` blocks the worker only on backpressure (the bounded channel
//! full); `oneshot_wait` blocks it until THIS submission's batch is durable.
//! Nothing else gates the ack — not hotmem (a separate fire-and-forget
//! `channel_send` the worker issued first), not the coreir-disk janitor
//! (ACK_GATES_ON_RECOVERY_LOG_ONLY).
//!
//! ## bind_record folds in here (Chris's decision — one channel, one janitor,
//!    one oneshot; NOT a third writer)
//!
//! Each submission carries BOTH the payload WAL frame AND the PK→name BIND line.
//! The janitor writes the frame to a reader-visible WAL segment and the bind
//! line to `.axverity/names/<slug>.log`, then fsyncs, then signals. So the ack
//! implies the row's content AND its PK binding are both durable — the whole
//! INSERT, not half of it. The on-disk formats are byte-identical to what
//! `wal_put` / `bind_record` write (the frame is `wal_frame_bytes`'d in M1; the
//! bind line is the same `"<ts>\tBIND\t<hash>\n"`), so every frozen reader
//! (SELECT's field-index WAL rebuild, `pull_object`'s WAL tier, `resolve_name`'s
//! `.log` fallback) sees an INSERT exactly as before — only the durability
//! TIMING and the writing THREAD change. `wal_put`/`bind_record` themselves are
//! untouched and still serve every other caller (CLI push, UPDATE/DELETE).
//!
//! ## Durability (Spike-3 STRONG, preserved by construction)
//!
//! Per batch: append every frame to the janitor's WAL segment then ONE
//! `sync_all`; append every bind line to its name-log then `sync_all` per
//! distinct name; fsync each touched parent directory once so a newly-created
//! segment/name-log's directory entry is durable (power-loss-safe, not merely
//! crash-safe). Only AFTER all of that do we signal the oneshots. We never
//! signal-then-fsync. A frame is never split across a segment (the batch total
//! drives one `wal_write_seg` rotation decision, and `wal_write_seg` rotates
//! *before* an overflowing frame).
//!
//! ## Shard & concurrency
//!
//! The janitor runs on ONE `--entries` thread bound (via `wal_shard_set`) to a
//! dedicated WAL shard disjoint from the worker shards and the CLI's shard 0, so
//! its segments never alias another writer's. It is the SOLE writer of both its
//! WAL segment and (for INSERTs) the name-logs, so those appends are
//! single-writer and never interleave — strictly safer than the pre-existing
//! per-worker name-log path. The reader fan-out count (`AXVERITY_WAL_SHARDS`)
//! must include this shard; the pg_server build sets it.
//!
//! ## Tuning (NO_HARDCODED_TUNING_VALUES)
//!
//! Batch-size cap (`AXVERITY_RECLOG_BATCH`) and window (`AXVERITY_RECLOG_WINDOW_MS`)
//! are read from env here; the bounded channel capacity is `AXVERITY_RECLOG_CAP`
//! (channels.rs). The defaults are explicit PLACEHOLDERS marked TODO-TUNE — this
//! build assigns no tuned value; the window/cap sweep is the separate later
//! empirical step the intent names. Correctness (durable-before-ack,
//! block-not-drop) does not depend on the specific values.
//!
//! Identities are sha256(name_utf8), the bridge-wide convention.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

use super::channels::{bounded_drain_batch, bounded_send_blocking};
use super::oneshot::{new_oneshot, signal_oneshot};
use super::value::{get_str, intern_str, Value};

/// The single recovery-log bounded channel name (internal; not an M1-level
/// `channel_send` target, so CHANNELS_STATIC does not apply — no topology block
/// needed). One channel, one janitor (Chris's decision).
const RECLOG_CHAN: &str = "reclog-batch";

/// Placeholder batch-size cap — TODO-TUNE. Overridden by `AXVERITY_RECLOG_BATCH`.
const BATCH_CAP_DEFAULT: usize = 256;
/// Placeholder accumulation window in ms — TODO-TUNE. Overridden by
/// `AXVERITY_RECLOG_WINDOW_MS`.
const WINDOW_MS_DEFAULT: u64 = 2;

fn batch_cap_env() -> usize {
    std::env::var("AXVERITY_RECLOG_BATCH")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(BATCH_CAP_DEFAULT)
}

fn window_ms_env() -> u64 {
    std::env::var("AXVERITY_RECLOG_WINDOW_MS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(WINDOW_MS_DEFAULT)
}

/// `reclog_submit(frame: Bytes, name_log_path: Text, bind_line: Bytes) -> Int`
///
/// Mint a oneshot, package the work item, and enqueue it onto the recovery-log
/// bounded channel — BLOCKING the caller only if the channel is full
/// (backpressure). Returns the oneshot id; the caller then `oneshot_wait`s it.
#[track_caller]
pub fn reclog_submit(args: Value) -> Value {
    let (frame, logp, bind_line) = match args {
        Value::Tuple(es) if es.len() == 3 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("reclog_submit: expected Tuple(Bytes, Text, Bytes), got {:?}", other),
    };
    // Validate shapes up front so a malformed submit fails at the producer, not
    // deep in the janitor.
    match &frame {
        Value::Bytes(_) => {}
        other => panic!("reclog_submit: arg 0 (frame) expected Bytes, got {:?}", other),
    }
    match &logp {
        Value::Str(_) => {}
        other => panic!("reclog_submit: arg 1 (name_log_path) expected Text, got {:?}", other),
    }
    match &bind_line {
        Value::Bytes(_) => {}
        other => panic!("reclog_submit: arg 2 (bind_line) expected Bytes, got {:?}", other),
    }
    let id = new_oneshot();
    let item = Value::Tuple(vec![Value::Int(id), frame, logp, bind_line]);
    stream_submit(STREAM_FUSED, item);
    Value::Int(id)
}

/// Open `path` for append (creating if absent). Returns the file plus whether it
/// had to be created (so the caller can fsync the parent directory once for a
/// new file's directory-entry durability).
fn open_append(path: &str) -> (File, bool) {
    let existed = Path::new(path).exists();
    let f = OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .unwrap_or_else(|e| panic!("reclog: open_append({}): {}", path, e));
    (f, !existed)
}

/// Fsync the parent directory of `path` so a newly-created file's directory
/// entry is durable (mirrors logbuf_open's parent-dir fsync — the STRONG
/// discipline). Best-effort: a directory that cannot be opened for fsync is a
/// hard error, matching the rest of the durable path.
fn fsync_parent_dir(path: &str) {
    if let Some(parent) = Path::new(path).parent() {
        let dir = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
        let f = File::open(dir)
            .unwrap_or_else(|e| panic!("reclog: open parent dir {:?} for fsync: {}", dir, e));
        f.sync_all()
            .unwrap_or_else(|e| panic!("reclog: fsync parent dir {:?}: {}", dir, e));
    }
}

// ── Stream seam (AXVERITY_UNIFIED_DURABLE_STREAMS_V1 — phase 1) ──────────────
// The group-commit engine is parameterized by a compile-time `stream_id` via
// MATCH DISPATCH (two fixed streams ever, not a runtime registry — the stream
// definitions are hand-written code, known at compile time; a registry would add
// indirection and a slot for a wrong write_fn/channel pairing the compiler can't
// catch). Phase 1 registers exactly ONE stream, FUSED, whose write path is the
// pre-phase-1 payload-WAL + name-log flush VERBATIM (including its per-name fsync
// loop). This is a PURE REFACTOR with zero behavior change: same channel, same
// cap/window, same fsyncs (payload 1, name N), same ack ordering. Phase 2 adds a
// NAME arm; phase 3 removes the name-write from FUSED; phase 4 handles the
// payload .bin index. The match is exhaustive — a new stream is one arm.
const STREAM_FUSED: &str = "fused";

/// The bounded channel a stream drains. Match-dispatched; exhaustive.
fn stream_chan(stream_id: &str) -> &'static str {
    match stream_id {
        STREAM_FUSED => RECLOG_CHAN,
        other => panic!("stream_chan: unknown stream_id {:?}", other),
    }
}

/// A stream's size-OR-timer release rule = (batch-size cap, window ms).
fn stream_release(stream_id: &str) -> (usize, u64) {
    match stream_id {
        STREAM_FUSED => (batch_cap_env(), window_ms_env()),
        other => panic!("stream_release: unknown stream_id {:?}", other),
    }
}

/// Enqueue one work item onto a stream's channel (blocking on full = honest
/// backpressure). Stream-agnostic apart from the channel selection.
fn stream_submit(stream_id: &str, item: Value) {
    bounded_send_blocking(stream_chan(stream_id), item);
}

/// Durably write one drained batch for `stream_id`, returning the oneshot ids to
/// ack. This is the ONLY per-stream step; the drain/ack skeleton around it
/// (`stream_flush_once`) is generic. Match-dispatched; exhaustive.
fn write_batch(stream_id: &str, batch: Vec<Value>) -> Vec<i64> {
    match stream_id {
        STREAM_FUSED => write_batch_fused(batch),
        other => panic!("write_batch: unknown stream_id {:?}", other),
    }
}

/// GENERIC group-commit skeleton: drain one size-OR-timer batch, durably write it
/// via the stream's `write_batch`, then ack every caller. Stream-agnostic. §3
/// (signal) lives HERE, AFTER `write_batch` returns (all writes durable) — the
/// identical batch-level ack-after-all-durable position the pre-refactor flush
/// used (prior grounding: signal_oneshot already fired after the entire §1+§2).
fn stream_flush_once(stream_id: &str) -> i64 {
    let (cap, window_ms) = stream_release(stream_id);
    let batch = bounded_drain_batch(stream_chan(stream_id), cap, window_ms);
    if batch.is_empty() {
        return 0;
    }
    let ids = write_batch(stream_id, batch);
    // ── 3. Durable barrier passed — release every caller's ack ──
    for id in &ids {
        signal_oneshot(*id);
    }
    ids.len() as i64
}

/// FUSED stream write path — payload WAL frames + per-name bind logs, VERBATIM
/// from the pre-phase-1 `reclog_flush_once` (its §1 + §2, incl. the audit marks).
/// The per-name fsync loop is intentionally UNCHANGED in phase 1; phase 3 removes
/// the name-write from this arm once the dedicated name stream passes its gate.
/// Returns the batch's oneshot ids for the skeleton to ack (§3 moved out).
fn write_batch_fused(batch: Vec<Value>) -> Vec<i64> {
    // Decode the batch into (oneshot id, frame bytes, name-log path, bind line).
    let mut ids: Vec<i64> = Vec::with_capacity(batch.len());
    let mut frames: Vec<Vec<u8>> = Vec::with_capacity(batch.len());
    // Preserve per-name append order: Vec of (path, lines) plus an index map.
    let mut name_order: Vec<String> = Vec::new();
    let mut name_lines: HashMap<String, Vec<Vec<u8>>> = HashMap::new();

    for item in batch {
        let mut fields = match item {
            Value::Tuple(es) if es.len() == 4 => es.into_iter(),
            other => panic!("write_batch_fused: malformed item {:?}", other),
        };
        let id = match fields.next().unwrap() {
            Value::Int(n) => n,
            other => panic!("write_batch_fused: item field 0 (id) expected Int, got {:?}", other),
        };
        let frame = match fields.next().unwrap() {
            Value::Bytes(b) => b.to_vec(),
            other => panic!("write_batch_fused: item field 1 (frame) expected Bytes, got {:?}", other),
        };
        let logp = match fields.next().unwrap() {
            Value::Str(h) => get_str(h),
            other => panic!("write_batch_fused: item field 2 (path) expected Text, got {:?}", other),
        };
        let line = match fields.next().unwrap() {
            Value::Bytes(b) => b.to_vec(),
            other => panic!("write_batch_fused: item field 3 (bind) expected Bytes, got {:?}", other),
        };
        ids.push(id);
        frames.push(frame);
        if !name_lines.contains_key(&logp) {
            name_order.push(logp.clone());
        }
        name_lines.entry(logp).or_default().push(line);
    }

    // ── 1. Payload WAL: one segment for the whole batch, one data fsync ──
    let shard = match super::walshard::wal_shard_get(Value::Unit) {
        Value::Str(h) => get_str(h),
        _ => String::from("0"),
    };
    let prefix = format!(".axverity/wal/{}-", shard);
    let total: i64 = frames.iter().map(|f| f.len() as i64).sum();
    let seq = match super::prealloc::wal_write_seg(Value::Tuple(vec![
        Value::Str(intern_str(&prefix)),
        Value::Int(total),
    ])) {
        Value::Int(n) => n,
        other => panic!("reclog_flush_once: wal_write_seg returned {:?}", other),
    };
    let seg_path = format!("{}{}.log", prefix, seq);
    let (mut seg_file, seg_new) = open_append(&seg_path);
    for frame in &frames {
        seg_file
            .write_all(frame)
            .unwrap_or_else(|e| panic!("reclog: write frame to {}: {}", seg_path, e));
    }
    super::tsmark::mark(70); // AUDIT (uncommitted): before payload WAL fsync
    seg_file
        .sync_all()
        .unwrap_or_else(|e| panic!("reclog: fsync WAL segment {}: {}", seg_path, e));
    super::tsmark::mark(71); // AUDIT (uncommitted): after payload WAL fsync
    if seg_new {
        fsync_parent_dir(&seg_path);
    }

    // ── 2. Name-log binds: append per name, one data fsync per distinct name ──
    super::tsmark::mark(72); // AUDIT (uncommitted): before name-log fsync loop
    let mut any_name_new = false;
    for logp in &name_order {
        let (mut nf, nf_new) = open_append(logp);
        for line in &name_lines[logp] {
            nf.write_all(line)
                .unwrap_or_else(|e| panic!("reclog: write bind to {}: {}", logp, e));
        }
        nf.sync_all()
            .unwrap_or_else(|e| panic!("reclog: fsync name-log {}: {}", logp, e));
        if nf_new {
            any_name_new = true;
        }
    }
    if any_name_new {
        // Every name-log lives under .axverity/names; one dir fsync covers all
        // the batch's newly-created ones.
        fsync_parent_dir(".axverity/names/x");
    }
    super::tsmark::mark(73); // AUDIT (uncommitted): after name-log fsync loop

    ids
}

/// `reclog_flush_once(Unit) -> Int` — the FUSED stream's flush; the M1 janitor
/// entry point. ABI UNCHANGED (reclog_janitor_step calls it verbatim); delegates
/// to the generic skeleton with stream_id = FUSED. Returns the item count.
#[track_caller]
pub fn reclog_flush_once(_: Value) -> Value {
    Value::Int(stream_flush_once(STREAM_FUSED))
}

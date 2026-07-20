//! AXVERITY_TSMARK_FLUSH_WIRING_V1 — Candidate A harness (DESIGN-PHASE ARTIFACT).
//!
//! Zero-mutation measurement of write_batch_fused's tsmark(70/71/72/73) deltas
//! (payload-WAL fsync vs per-distinct-name fsync loop) via the PUBLIC API only
//! (`reclog_submit`, `reclog_flush_once`, `tsmark::ts_flush`). `write_batch_fused`
//! itself is private (reclog.rs:234) and is NOT touched, called directly, or
//! modified by this file.
//!
//! Grounding for why a single-threaded harness is representative (not assumed —
//! confirmed by reading reclog.rs/tsmark.rs before writing this):
//!   - `mark(id)` (reclog.rs) and `ts_flush` (tsmark.rs) both go through the
//!     THREAD-LOCAL `MARKS` vec — "thread-local so N worker threads never
//!     contend" (tsmark.rs doc comment). stream_flush_once's I/O work is the
//!     same code path regardless of which thread calls it; there is no
//!     thread-identity branch anywhere in write_batch_fused.
//!   - `reclog_submit` only enqueues onto a bounded channel (`stream_submit` ->
//!     `bounded_send_blocking`) and returns; it does not spawn a flusher. So a
//!     single thread calling submit N times then `reclog_flush_once()` once is
//!     exactly the real worker-thread / janitor-thread relationship, just
//!     unified onto one call stack — nothing about the write path depends on
//!     those being two different OS threads.
//!   - `MARKS` only clears on `ts_flush`; repeated `reclog_flush_once()` calls
//!     safely accumulate multiple 70/71/72/73 quads before a single end-of-run
//!     flush, so one .tsv file can hold the whole cardinality sweep.
//!
//! GATED OFF: `#[ignore]` so `cargo test` never runs this by default. Per the
//! governing intent (AXVERITY_TSMARK_FLUSH_WIRING_V1), mode = (design allowed,
//! execution prohibited), status = (draft, execution-allowed false). This file
//! is written per that intent's boundary ("writing a standalone harness binary"
//! is explicitly allowed) but has NOT been compiled or run. Do not remove
//! `#[ignore]` or execute this test without Chris's explicit authorization to
//! flip the intent to an execution-allowed state.
//!
//! All I/O happens under a throwaway `std::env::temp_dir()` subdirectory,
//! never the real `.axverity/` tree.

use axis_codegen_bridge::runtime::reclog::{reclog_flush_once, reclog_submit};
use axis_codegen_bridge::runtime::tsmark::{ts_flush, ts_mark_val};
use axis_codegen_bridge::runtime::value::{get_str, intern_str, Value};

/// Batch cardinalities to sweep — number of DISTINCT name-log paths in one
/// batch. 1 = every item shares a name (worst-case amortization); N = every
/// item has its own name (worst-case fsync count, one per item).
const CARDINALITIES: &[usize] = &[1, 2, 4, 8, 16, 32, 64, 128, 256];

/// Items per batch (<= AXVERITY_RECLOG_BATCH default of 256 so one
/// `reclog_flush_once()` drains the whole thing in a single batch).
const ITEMS_PER_BATCH: usize = 256;

/// Marker id for our own `ts_mark_val(BATCH_MARK, cardinality)` calls, chosen
/// well outside the 70-73 range reclog.rs uses so the two are unambiguous when
/// parsing the flushed .tsv.
const BATCH_MARK: i64 = 200;

fn synth_frame(i: usize) -> Value {
    // Small fixed-size payload; frame CONTENT is irrelevant to the fsync-cost
    // question, only frame COUNT/total bytes matter for the WAL segment write.
    Value::Bytes(format!("frame-payload-{:06}", i).into_bytes())
}

fn synth_bind_line(i: usize) -> Value {
    Value::Bytes(format!("{}\tBIND\tsha256:{:064x}\n", i, i).into_bytes())
}

/// Deterministic name-log path for slot `i` out of `cardinality` distinct names.
fn synth_name_path(root: &std::path::Path, cardinality: usize, i: usize) -> Value {
    let slot = i % cardinality;
    let p = root.join(format!(".axverity/names/audit-{:04}.log", slot));
    Value::Str(intern_str(&p.to_string_lossy()))
}

#[test]
#[ignore = "AXVERITY_TSMARK_FLUSH_WIRING_V1: execution-allowed=false until Chris authorizes"]
fn candidate_a_reclog_fsync_split_sweep() {
    let root = std::env::temp_dir().join(format!(
        "axv-tsmark-audit-{}-{}",
        std::process::id(),
        CARDINALITIES.len()
    ));
    std::fs::create_dir_all(root.join(".axverity/names")).unwrap();
    std::fs::create_dir_all(root.join(".axverity/wal")).unwrap();

    // write_batch_fused uses paths RELATIVE to CWD (".axverity/wal/...", see
    // reclog.rs write_batch_fused) — so this harness must run from `root`.
    let prev_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&root).unwrap();

    for &cardinality in CARDINALITIES {
        // Correlate the next 70/71/72/73 quad with this cardinality.
        let _ = ts_mark_val(Value::Tuple(vec![
            Value::Int(BATCH_MARK),
            Value::Int(cardinality as i64),
        ]));

        let mut ids = Vec::with_capacity(ITEMS_PER_BATCH);
        for i in 0..ITEMS_PER_BATCH {
            let args = Value::Tuple(vec![
                synth_frame(i),
                synth_name_path(&root, cardinality, i),
                synth_bind_line(i),
            ]);
            match reclog_submit(args) {
                Value::Int(id) => ids.push(id),
                other => panic!("reclog_submit: unexpected return {:?}", other),
            }
        }

        // Drain + durably write + fsync the whole batch, then signal every
        // oneshot. We don't need the oneshots themselves (no client is
        // actually waiting) — only the fact that flush ran and marks landed.
        match reclog_flush_once(Value::Unit) {
            Value::Int(n) => assert_eq!(n as usize, ITEMS_PER_BATCH, "flush should drain the whole batch in one call"),
            other => panic!("reclog_flush_once: unexpected return {:?}", other),
        }
    }

    let dir = root.join("ts-out");
    let dir_str = get_str(intern_str(&dir.to_string_lossy()));
    let count = match ts_flush(Value::Str(intern_str(&dir_str))) {
        Value::Int(n) => n,
        other => panic!("ts_flush: unexpected return {:?}", other),
    };
    // 4 marks (70/71/72/73) + 1 batch-correlation mark per cardinality swept.
    assert_eq!(count as usize, CARDINALITIES.len() * 5);

    std::env::set_current_dir(&prev_cwd).unwrap();

    eprintln!(
        "AXVERITY_TSMARK_FLUSH_WIRING_V1: wrote {} marks under {}",
        count,
        dir.display()
    );
    eprintln!("root (not cleaned up — inspect then rm -rf): {}", root.display());
}

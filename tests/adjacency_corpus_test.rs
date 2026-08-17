//! AXVERITY_GRAPH_INTERFACE_V1 — full-corpus verification of the adjacency
//! PROJECTION (`runtime/adjacency.rs`) against independent ground truth.
//!
//! `FULL_CORPUS_NOT_SLICE` is a hard limit of the governing intent, so this
//! sweeps EVERY distinct source and EVERY distinct target, not a sample. It
//! drives the real `adj_build`/`adj_get` entry points in one process (the
//! per-node CLI would rebuild the projection once per invocation).
//!
//! Ground truth is the client's own database, exported to TSV — deliberately a
//! DIFFERENT source than the object store the projection is built from, so the
//! two cannot agree by construction.
//!
//! Skipped (not failed) when the corpus env vars are absent: the 12,906-edge
//! reference corpus is not committed to any repo. Run it with:
//!
//!   AXVERITY_ADJ_CORPUS_DIR=<store root> \
//!   AXVERITY_ADJ_CORPUS_TRUTH=<tsv: source \t verb \t target> \
//!   cargo test --release --test adjacency_corpus_test -- --nocapture

use std::collections::HashMap;
use std::fs;

use axis_codegen_bridge::runtime::adjacency::{adj_build, adj_get, adj_stats};
use axis_codegen_bridge::runtime::value::{get_str, intern_str, Value};

fn text(s: &str) -> std::sync::Arc<str> {
    intern_str(s)
}

fn as_text(v: Value) -> String {
    match v {
        Value::Str(h) => get_str(&h),
        other => panic!("expected Text, got {:?}", other),
    }
}

/// An LF-TERMINATED posting list; "" means absent. Counts real entries.
fn posting_len(s: &str) -> usize {
    if s.is_empty() {
        return 0;
    }
    s.trim_end_matches('\n').split('\n').count()
}

#[test]
fn projection_matches_ground_truth_across_the_full_corpus() {
    let (dir, truth_path) = match (
        std::env::var("AXVERITY_ADJ_CORPUS_DIR"),
        std::env::var("AXVERITY_ADJ_CORPUS_TRUTH"),
    ) {
        (Ok(d), Ok(t)) => (d, t),
        _ => {
            eprintln!("SKIP: corpus env vars unset (see file header)");
            return;
        }
    };

    // ── ground truth, from the client DB export ───────────────────────────
    let raw = fs::read_to_string(&truth_path).expect("truth tsv readable");
    let mut truth_out: HashMap<String, usize> = HashMap::new();
    let mut truth_in: HashMap<String, usize> = HashMap::new();
    let mut truth_edges = 0usize;
    for line in raw.lines() {
        if line.is_empty() {
            continue;
        }
        let mut f = line.split('\t');
        let (s, _v, t) = match (f.next(), f.next(), f.next()) {
            (Some(a), Some(b), Some(c)) => (a, b, c),
            _ => panic!("malformed truth line: {:?}", line),
        };
        *truth_out.entry(s.to_string()).or_default() += 1;
        *truth_in.entry(t.to_string()).or_default() += 1;
        truth_edges += 1;
    }

    // ── build the projection from the object store (loose + pack) ─────────
    let built = match adj_build(text(&dir)) {
        Value::Int(n) => n as usize,
        other => panic!("adj_build returned {:?}", other),
    };
    let stats = as_text(adj_stats(Value::Unit));
    eprintln!("projection: {}", stats);
    eprintln!(
        "truth:      edges={} sources={} targets={}",
        truth_edges,
        truth_out.len(),
        truth_in.len()
    );

    assert_eq!(
        built, truth_edges,
        "edge count: projection {} vs truth {}",
        built, truth_edges
    );

    // ── EVERY source, EVERY target — no sampling ──────────────────────────
    let mut out_mismatch = Vec::new();
    for (node, want) in &truth_out {
        let got = posting_len(&as_text(adj_get(text("OUT"), text(node))));
        if got != *want {
            out_mismatch.push((node.clone(), *want, got));
        }
    }
    let mut in_mismatch = Vec::new();
    for (node, want) in &truth_in {
        let got = posting_len(&as_text(adj_get(text("IN"), text(node))));
        if got != *want {
            in_mismatch.push((node.clone(), *want, got));
        }
    }

    for (n, w, g) in out_mismatch.iter().take(5) {
        eprintln!("  OUT mismatch {}: want {} got {}", n, w, g);
    }
    for (n, w, g) in in_mismatch.iter().take(5) {
        eprintln!("  IN  mismatch {}: want {} got {}", n, w, g);
    }
    eprintln!(
        "swept {} sources, {} targets — mismatches out={} in={}",
        truth_out.len(),
        truth_in.len(),
        out_mismatch.len(),
        in_mismatch.len()
    );
    assert!(out_mismatch.is_empty(), "out-degree mismatches");
    assert!(in_mismatch.is_empty(), "in-degree mismatches");

    // A node the corpus never mentions must come back empty, not wrong.
    assert_eq!(
        as_text(adj_get(text("OUT"), text("sha256:no-such-node"))),
        ""
    );
}

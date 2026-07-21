// AXVERITY_BRIDGE_LOCKFREE_EXPERIMENT_V1 — concurrency measurement for the
// sharded-Mutex index candidates (#5 bindidx, #6 contentidx).
//
// These two indexes CANNOT go thread-local (contract: worker-A's write must be
// visible to worker-B's read — the INSERT→UPDATE/DELETE and INSERT→pull_object
// read-after-write windows). So the question the intent wants a measured fact
// for is NOT "does a thread-local variant help" (forbidden) but "is the current
// 256-shard Mutex actually a live contention point under real concurrent load,
// or has the sharding already removed the collapse the single-lock tag interner
// shows?" This binary measures exactly that, matched to each index's own access
// pattern.
//
// Metric mirrors interner_contention.rs: fixed TOTAL ops split across P threads;
// report wall / throughput / speedup vs P=1. Positive scaling ⇒ the sharded lock
// is not a contention point (a lock-free-shared port would target a bottleneck
// that isn't there). Negative scaling ⇒ it collapses like the tag interner.
//
// Run: cargo run --release --bin lock_candidates_bench

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Instant;

use axis_codegen_bridge::runtime::value::{intern_str, init_runtime, Value};
use axis_codegen_bridge::runtime::bindidx::{bindidx_get, bindidx_put};
use axis_codegen_bridge::runtime::contentidx::{contentidx_get, contentidx_put};

const TOTAL_OPS: usize = 8_000_000;

fn thread_counts() -> Vec<usize> {
    let n = thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
    let mut v = vec![1usize, 2, 4, 8, 16];
    if !v.contains(&n) { v.push(n); }
    v.retain(|&p| p <= n.max(16));
    v.sort_unstable();
    v.dedup();
    v
}

#[inline]
fn s(v: &str) -> Value { Value::Str(intern_str(v)) }

/// bindidx, DISTINCT-key pattern (realistic: concurrent connections mutate
/// distinct PK rows). Each op = one put (bind) + one get (resolve) of a key in
/// this thread's own private range → shards spread by fnv1a(name).
fn bind_distinct(ops_total: usize, p: usize) -> f64 {
    let per = ops_total / p;
    let sink = Arc::new(AtomicUsize::new(0));
    let t0 = Instant::now();
    let handles: Vec<_> = (0..p).map(|tid| {
        let sink = Arc::clone(&sink);
        thread::spawn(move || {
            let mut acc = 0usize;
            for i in 0..per {
                let name = format!("t{}:row{}", tid, i & 0xffff); // per-thread key space
                bindidx_put(Value::Tuple(vec![s(&name), s("sha256:abcdef0123")]));
                if let Value::Str(h) = bindidx_get(s(&name)) { acc = acc.wrapping_add(h.len()); }
            }
            sink.fetch_add(acc, Ordering::Relaxed);
        })
    }).collect();
    for h in handles { h.join().unwrap(); }
    std::hint::black_box(sink.load(Ordering::Relaxed));
    t0.elapsed().as_secs_f64()
}

/// bindidx, HOT-key pattern (worst case: all threads hammer a tiny shared key
/// set → repeated same-shard collisions, the same-PK contended-UPDATE case).
fn bind_hotkey(ops_total: usize, p: usize) -> f64 {
    const HOT: usize = 8;
    let per = ops_total / p;
    let sink = Arc::new(AtomicUsize::new(0));
    let t0 = Instant::now();
    let handles: Vec<_> = (0..p).map(|tid| {
        let sink = Arc::clone(&sink);
        thread::spawn(move || {
            let mut acc = 0usize;
            for i in 0..per {
                let name = format!("hot:row{}", (tid + i) % HOT); // shared across threads
                bindidx_put(Value::Tuple(vec![s(&name), s("sha256:abcdef0123")]));
                if let Value::Str(h) = bindidx_get(s(&name)) { acc = acc.wrapping_add(h.len()); }
            }
            sink.fetch_add(acc, Ordering::Relaxed);
        })
    }).collect();
    for h in handles { h.join().unwrap(); }
    std::hint::black_box(sink.load(Ordering::Relaxed));
    t0.elapsed().as_secs_f64()
}

/// contentidx, DISTINCT-hash pattern (the only realistic case: content-addressed
/// hashes are ~always distinct). Each op = one put (publish bytes) + one get.
fn content_distinct(ops_total: usize, p: usize) -> f64 {
    let per = ops_total / p;
    let sink = Arc::new(AtomicUsize::new(0));
    let payload: Vec<u8> = vec![0x5au8; 96]; // a small RECORD-sized blob
    let t0 = Instant::now();
    let handles: Vec<_> = (0..p).map(|tid| {
        let sink = Arc::clone(&sink);
        let payload = payload.clone();
        thread::spawn(move || {
            let mut acc = 0usize;
            for i in 0..per {
                let hash = format!("sha256:t{}_{:08x}", tid, i & 0x3fff); // per-thread distinct
                contentidx_put(Value::Tuple(vec![s(&hash), Value::Bytes(payload.clone())]));
                if let Value::Bytes(b) = contentidx_get(s(&hash)) { acc = acc.wrapping_add(b.len()); }
            }
            sink.fetch_add(acc, Ordering::Relaxed);
        })
    }).collect();
    for h in handles { h.join().unwrap(); }
    std::hint::black_box(sink.load(Ordering::Relaxed));
    t0.elapsed().as_secs_f64()
}

fn report(label: &str, f: impl Fn(usize, usize) -> f64, ps: &[usize]) {
    println!("\n== {label} ==  (fixed total = {} ops, each op = put+get)", TOTAL_OPS);
    println!("  {:>3}  {:>10}  {:>14}  {:>9}  {:>8}", "P", "wall(s)", "ops/sec", "speedup", "eff%");
    let base = f(TOTAL_OPS, 1);
    for &p in ps {
        let dt = f(TOTAL_OPS, p);
        let tp = TOTAL_OPS as f64 / dt;
        let speedup = base / dt;
        println!("  {:>3}  {:>10.3}  {:>14.0}  {:>8.2}x  {:>7.0}%",
                 p, dt, tp, speedup, 100.0 * speedup / p as f64);
    }
}

fn main() {
    init_runtime();
    let ps = thread_counts();
    println!("sharded-Mutex index contention — cores={}, P set={:?}",
             thread::available_parallelism().map(|n| n.get()).unwrap_or(0), ps);
    report("bindidx (#5) — DISTINCT keys (realistic)", bind_distinct, &ps);
    report("bindidx (#5) — HOT keys (worst case, 8 shared)", bind_hotkey, &ps);
    report("contentidx (#6) — DISTINCT hashes (realistic)", content_distinct, &ps);
}

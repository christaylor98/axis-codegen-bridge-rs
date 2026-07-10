// M1_VALUE_STR_ARC_IMPLEMENTATION_V1 — str-load vs tag-load contention isolation.
//
// Resolves the open sub-question in decl:m1-interner-wip-orphan-tradeoff-v2 and
// the unknown in the intent: was Spike 4's P>=8-core contention finding a
// str-load problem, a tag-load problem, or both?
//
// After this turn's migration the two string mechanisms differ by construction:
//   * STR path  — Value::Str is Arc<str>; `intern_str` = Arc::from(&str).
//                 No shared, mutable, lock-guarded structure. (migrated)
//   * TAG path  — `intern_tag` still uses the global Mutex<Vec<String>> +
//                 Mutex<HashMap<String,u32>> interner. (untouched, by constraint)
//
// The TAG path is therefore a faithful in-tree replica of the OLD str-interner
// mechanism (same Mutex<HashMap>+Mutex<Vec> dedup-under-lock shape). Measuring
// TAG scaling in isolation reconstructs how the pre-migration str path would
// have scaled, and STR scaling shows the post-migration path. If TAG collapses
// at P>=8 while STR scales, Spike 4's finding is attributable to the
// Mutex-interner mechanism — which this turn removed from the str path and, by
// the INTERN_TAG_UNTOUCHED constraint, left in place on the tag path.
//
// Metric: keep TOTAL ops fixed and split them across P threads; report wall
// time, aggregate throughput, and speedup vs P=1. A lock-free mechanism gives
// near-linear speedup; a single-lock mechanism plateaus or regresses (negative
// scaling) once cores contend on the mutex.
//
// Run: cargo run --release --bin interner_contention

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Instant;

use axis_codegen_bridge::runtime::value::{intern_str, intern_tag, get_str, init_runtime, Value};

const TOTAL_OPS: usize = 24_000_000; // fixed total work, split across threads
const KEYSET: usize = 64;            // distinct keys → mostly dedup-hit (read-under-lock) path

fn thread_counts() -> Vec<usize> {
    let n = thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
    let mut v = vec![1usize, 2, 4, 8, 16];
    // include the machine's core count if it isn't already represented
    if !v.contains(&n) { v.push(n); }
    v.retain(|&p| p <= n.max(16));
    v.sort_unstable();
    v.dedup();
    v
}

/// Pre-populate both interners so every op in the measured loop takes the
/// dedup-HIT path (lock + hashmap get), i.e. steady-state, not first-insert.
fn warm() {
    for i in 0..KEYSET {
        let s = format!("key_{:04}", i);
        let _ = intern_str(&s);
        let _ = intern_tag(&s);
    }
}

/// Returns (wall_seconds, ops) for `ops_total` STR interns split across `p` threads.
fn run_str(ops_total: usize, p: usize) -> f64 {
    let per = ops_total / p;
    let sink = Arc::new(AtomicUsize::new(0));
    let t0 = Instant::now();
    let handles: Vec<_> = (0..p).map(|tid| {
        let sink = Arc::clone(&sink);
        thread::spawn(move || {
            let mut acc = 0usize;
            for i in 0..per {
                let k = format!("key_{:04}", (tid + i) % KEYSET);
                let v = Value::Str(intern_str(&k)); // full construction path
                if let Value::Str(a) = &v { acc = acc.wrapping_add(a.len()); }
                let _ = get_str(&k); // read shim on the hot path too
            }
            sink.fetch_add(acc, Ordering::Relaxed);
        })
    }).collect();
    for h in handles { h.join().unwrap(); }
    std::hint::black_box(sink.load(Ordering::Relaxed));
    t0.elapsed().as_secs_f64()
}

/// Returns wall_seconds for `ops_total` TAG interns split across `p` threads.
fn run_tag(ops_total: usize, p: usize) -> f64 {
    let per = ops_total / p;
    let sink = Arc::new(AtomicUsize::new(0));
    let t0 = Instant::now();
    let handles: Vec<_> = (0..p).map(|tid| {
        let sink = Arc::clone(&sink);
        thread::spawn(move || {
            let mut acc = 0usize;
            for i in 0..per {
                let k = format!("key_{:04}", (tid + i) % KEYSET);
                acc = acc.wrapping_add(intern_tag(&k) as usize); // Mutex interner path
            }
            sink.fetch_add(acc, Ordering::Relaxed);
        })
    }).collect();
    for h in handles { h.join().unwrap(); }
    std::hint::black_box(sink.load(Ordering::Relaxed));
    t0.elapsed().as_secs_f64()
}

/// Mixed: half the threads hammer STR, half hammer TAG, concurrently.
fn run_mixed(ops_total: usize, p: usize) -> f64 {
    let per = ops_total / p;
    let sink = Arc::new(AtomicUsize::new(0));
    let t0 = Instant::now();
    let handles: Vec<_> = (0..p).map(|tid| {
        let sink = Arc::clone(&sink);
        thread::spawn(move || {
            let mut acc = 0usize;
            for i in 0..per {
                let k = format!("key_{:04}", (tid + i) % KEYSET);
                if tid % 2 == 0 {
                    if let Value::Str(a) = Value::Str(intern_str(&k)) { acc = acc.wrapping_add(a.len()); }
                } else {
                    acc = acc.wrapping_add(intern_tag(&k) as usize);
                }
            }
            sink.fetch_add(acc, Ordering::Relaxed);
        })
    }).collect();
    for h in handles { h.join().unwrap(); }
    std::hint::black_box(sink.load(Ordering::Relaxed));
    t0.elapsed().as_secs_f64()
}

/// Single-thread Arc<str> clone throughput — the outcome test's clone metric.
fn clone_throughput() {
    let v = Value::Str(intern_str("a moderately sized interned string value"));
    let n = 50_000_000usize;
    let t0 = Instant::now();
    let mut acc = 0usize;
    for _ in 0..n {
        let c = v.clone();
        if let Value::Str(a) = &c { acc = acc.wrapping_add(a.len()); }
        std::hint::black_box(&c);
    }
    let dt = t0.elapsed().as_secs_f64();
    std::hint::black_box(acc);
    println!("  clone (1 thread): {:>10.0} clones/sec  ({:.2} ns/clone)",
             n as f64 / dt, dt * 1e9 / n as f64);
}

fn report(label: &str, f: impl Fn(usize, usize) -> f64, ps: &[usize]) {
    println!("\n== {label} ==  (fixed total = {} ops)", TOTAL_OPS);
    println!("  {:>3}  {:>10}  {:>14}  {:>9}  {:>8}", "P", "wall(s)", "ops/sec", "speedup", "eff%");
    let base = f(TOTAL_OPS, 1);
    let base_tp = TOTAL_OPS as f64 / base;
    for &p in ps {
        let dt = f(TOTAL_OPS, p);
        let tp = TOTAL_OPS as f64 / dt;
        let speedup = base / dt;
        println!("  {:>3}  {:>10.3}  {:>14.0}  {:>8.2}x  {:>7.0}%",
                 p, dt, tp, speedup, 100.0 * speedup / p as f64);
    }
    std::hint::black_box(base_tp);
}

fn main() {
    init_runtime();
    warm();
    let ps = thread_counts();
    println!("interner contention isolation — cores={}, P set={:?}",
             thread::available_parallelism().map(|n| n.get()).unwrap_or(0), ps);
    clone_throughput();
    report("STR load (migrated: Arc<str>, lock-free)", run_str, &ps);
    report("TAG load (untouched: Mutex interner = old-str mechanism)", run_tag, &ps);
    report("MIXED load (half STR, half TAG)", run_mixed, &ps);
}

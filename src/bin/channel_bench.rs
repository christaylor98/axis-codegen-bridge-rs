// AXVERITY_BRIDGE_LOCKFREE_EXPERIMENT_V1 — candidate #2 (channels.rs).
//
// Measures whether the per-channel `Mutex<VecDeque>` message-queue lock is a live
// contention point under the real producer pattern: MANY producer threads
// `channel_send` to ONE channel (the seal-tap → "index-frame" fan-in), ONE
// consumer drains via wait(). If aggregate send+drain throughput collapses as
// producers rise (like the single-lock tag interner), the lock-free
// mpsc_intrusive swap would matter; if it scales/plateaus, the swap targets a
// non-bottleneck. Reported as a measured current-mutex fact (the lock-free
// variant is NOT built this turn — see the doc for why: mpsc_intrusive is
// single-CONSUMER, so the swap would narrow channels.rs's contract, a semantic
// change flagged not silently made).
//
// Run: cargo run --release --bin channel_bench

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;

use axis_codegen_bridge::runtime::value::{intern_str, init_runtime, Value};
use axis_codegen_bridge::runtime::channels::{channel_send, event_subscribe, wait};

const TOTAL: usize = 4_000_000;
static RECVD: AtomicUsize = AtomicUsize::new(0);

/// wait() handler: count how many messages this drain pulled. Bare fn, no capture.
fn count_handler(v: Value) -> Value {
    if let Value::List(items) = v {
        RECVD.fetch_add(items.len(), Ordering::Relaxed);
    }
    Value::Unit
}

fn run(producers: usize) -> f64 {
    RECVD.store(0, Ordering::Relaxed);
    let name = format!("bench-chan-{producers}"); // fresh channel per run
    let per = TOTAL / producers;
    let real_total = per * producers;
    let start = Arc::new(Barrier::new(producers + 1));

    // Consumer: subscribe + drain until all received.
    let consumer = {
        let name = name.clone();
        thread::spawn(move || {
            event_subscribe(Value::Str(intern_str(&name)));
            while RECVD.load(Ordering::Relaxed) < real_total {
                wait(count_handler);
            }
        })
    };

    // Producers: fan-in to the one channel.
    let mut prod = Vec::new();
    for _ in 0..producers {
        let start = Arc::clone(&start);
        let name = name.clone();
        prod.push(thread::spawn(move || {
            let nm = intern_str(&name);
            start.wait();
            for i in 0..per {
                channel_send(Value::Tuple(vec![Value::Str(nm.clone()), Value::Int(i as i64)]));
            }
        }));
    }

    start.wait();
    let t0 = Instant::now();
    for h in prod { h.join().unwrap(); }
    consumer.join().unwrap();
    let dt = t0.elapsed().as_secs_f64();
    assert_eq!(RECVD.load(Ordering::Relaxed), real_total, "lost messages");
    dt
}

fn main() {
    init_runtime();
    println!("channel fan-in bench — cores={}, total≈{} msgs, 1 consumer",
             thread::available_parallelism().map(|n| n.get()).unwrap_or(0), TOTAL);
    println!("  {:>3}  {:>10}  {:>14}  {:>9}", "P", "wall(s)", "msgs/sec", "speedup");
    let base = run(1);
    for &p in &[1usize, 2, 4, 8, 16] {
        let dt = run(p);
        let tp = TOTAL as f64 / dt;
        println!("  {:>3}  {:>10.3}  {:>14.0}  {:>8.2}x", p, dt, tp, base / dt);
    }
}

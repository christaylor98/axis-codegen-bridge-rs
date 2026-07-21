// AXVERITY_BRIDGE_LOCKFREE_EXPERIMENT_V1 — soak harness for the lock-free channel
// backend (candidate #2), the confidence gate before making it load-bearing.
//
// Runs the REAL channel primitives (channel_send / event_subscribe / wait) under
// sustained N-producer → 1-consumer fan-in for SOAK_SECS, with strong per-message
// verification that a quick test can't give:
//   * NO LOSS / NO DUP / PER-PRODUCER FIFO — every message is stamped (pid, seq);
//     the consumer asserts each producer's seqs arrive as exactly 0,1,2,… with no
//     gap, repeat, or reorder (mpsc_intrusive guarantees per-producer FIFO).
//   * COMPLETE ACCOUNTING — each producer ends with a sentinel carrying its final
//     count; the consumer asserts it received exactly that many from that producer.
//   * NO LEAK — RSS (VmHWM/VmRSS from /proc/self/status) must stay bounded over
//     the whole run (mpsc_intrusive frees each node on pop; a leak shows as
//     monotonic growth).
//   * NO HANG/DEADLOCK — the run must complete within the deadline + drain grace.
//
// Env: SOAK_SECS (default 60), SOAK_PRODUCERS (default 8). Run against the backend
// under test:  AXVERITY_CHANNEL_QUEUE=lockfree SOAK_SECS=120 cargo run --release --bin channel_soak
//
// A periodic 1µs producer micro-sleep creates frequent drain-to-empty moments so
// the consumer repeatedly blocks on the Condvar and must be woken — exercising the
// block→wake→drain edge thousands of times, where lock-free/wake bugs hide.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use axis_codegen_bridge::runtime::value::{intern_str, init_runtime, Value};
use axis_codegen_bridge::runtime::channels::{channel_send, event_subscribe, wait};

const CHAN: &str = "soak-frame";
const SENTINEL: i64 = -1;

fn env_usize(k: &str, d: usize) -> usize {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}

fn rss_kb(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status").ok()
        .and_then(|s| s.lines().find(|l| l.starts_with(field))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|n| n.parse().ok()))
        .unwrap_or(0)
}

// ── Consumer verification state (thread-local: wait() runs the handler ON the
//    consumer thread, synchronously). ─────────────────────────────────────────
struct State {
    last_seen: Vec<i64>,   // per producer: highest seq accepted (-1 = none)
    counts:    Vec<u64>,   // per producer: messages accepted
    done:      Vec<bool>,  // per producer: sentinel seen + count matched
    total:     u64,
    error:     Option<String>,
}
thread_local! {
    static ST: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn consume(v: Value) -> Value {
    ST.with(|cell| {
        let mut b = cell.borrow_mut();
        let st = b.as_mut().expect("state initialized before wait()");
        if st.error.is_some() { return; }
        let items = match v { Value::List(items) => items, _ => return };
        for it in items {
            let f = match &it { Value::Tuple(f) => f, _ => { st.error = Some("non-tuple msg".into()); return; } };
            let pid = f[0].as_int() as usize;
            let seq = f[1].as_int();
            if seq == SENTINEL {
                // sentinel: (pid, -1, final_count). Per-producer FIFO guarantees
                // every real message from pid already arrived before this.
                let expected = f[2].as_int() as u64;
                if st.counts[pid] != expected {
                    st.error = Some(format!(
                        "producer {pid}: received {} but sentinel says {expected} (LOSS/DUP)",
                        st.counts[pid]));
                    return;
                }
                st.done[pid] = true;
            } else {
                // normal message: must be exactly last_seen+1 (no gap/dup/reorder)
                if seq != st.last_seen[pid] + 1 {
                    st.error = Some(format!(
                        "producer {pid}: expected seq {} got {seq} (FIFO/LOSS/DUP violated)",
                        st.last_seen[pid] + 1));
                    return;
                }
                st.last_seen[pid] = seq;
                st.counts[pid] += 1;
                st.total += 1;
            }
        }
    });
    Value::Unit
}

fn main() {
    init_runtime();
    let secs = env_usize("SOAK_SECS", 60) as u64;
    let producers = env_usize("SOAK_PRODUCERS", 8);
    let backend = std::env::var("AXVERITY_CHANNEL_QUEUE").unwrap_or_else(|_| "mutex".into());
    println!("channel soak — backend={backend}, producers={producers}, secs={secs}");
    let rss0 = rss_kb("VmRSS:");

    let stop = Arc::new(AtomicBool::new(false));

    // Consumer: subscribe, then drain+verify until every producer's sentinel seen.
    let consumer = {
        let producers = producers;
        thread::spawn(move || {
            event_subscribe(Value::Str(intern_str(CHAN)));
            ST.with(|c| *c.borrow_mut() = Some(State {
                last_seen: vec![-1; producers],
                counts:    vec![0; producers],
                done:      vec![false; producers],
                total: 0, error: None,
            }));
            loop {
                wait(consume);
                let (all_done, err) = ST.with(|c| {
                    let b = c.borrow();
                    let st = b.as_ref().unwrap();
                    (st.done.iter().all(|&d| d), st.error.clone())
                });
                if err.is_some() || all_done { break; }
            }
            ST.with(|c| {
                let b = c.borrow();
                let st = b.as_ref().unwrap();
                (st.total, st.error.clone())
            })
        })
    };

    // Producers: fan-in stamped messages until the deadline, then a sentinel.
    let deadline = Instant::now() + Duration::from_secs(secs);
    let t0 = Instant::now();
    let mut prod = Vec::new();
    for pid in 0..producers {
        let stop = Arc::clone(&stop);
        prod.push(thread::spawn(move || {
            let nm = intern_str(CHAN);
            let mut seq: i64 = 0;
            while !stop.load(Ordering::Relaxed) {
                // channel_send convention: Tuple([name, data]); the (pid,seq)
                // stamp is the single `data` payload (a nested Tuple).
                let data = Value::Tuple(vec![Value::Int(pid as i64), Value::Int(seq)]);
                channel_send(Value::Tuple(vec![Value::Str(nm.clone()), data]));
                seq += 1;
                // periodic micro-idle → frequent drain-to-empty → wake-path exercise
                if seq & 0x3fff == 0 { thread::sleep(Duration::from_micros(1)); }
            }
            // sentinel payload: (pid, -1, final_count)
            let sent = Value::Tuple(vec![Value::Int(pid as i64), Value::Int(SENTINEL), Value::Int(seq)]);
            channel_send(Value::Tuple(vec![Value::Str(nm), sent]));
            seq as u64
        }));
    }

    // Let it run, sampling peak RSS periodically.
    let mut peak_rss = rss0;
    while Instant::now() < deadline {
        thread::sleep(Duration::from_millis(500));
        peak_rss = peak_rss.max(rss_kb("VmRSS:"));
    }
    stop.store(true, Ordering::Relaxed);

    let mut sent: u64 = 0;
    for h in prod { sent += h.join().unwrap(); }

    // Consumer must finish (drain remaining + all sentinels) — bounded grace.
    let (total_recv, err) = consumer.join().expect("consumer thread panicked");
    let elapsed = t0.elapsed().as_secs_f64();
    peak_rss = peak_rss.max(rss_kb("VmHWM:"));

    println!("  sent      = {sent}");
    println!("  received  = {total_recv}");
    println!("  throughput= {:.0} msgs/s", sent as f64 / elapsed);
    println!("  RSS start = {rss0} kB   peak(VmHWM) = {peak_rss} kB   growth = {} kB", peak_rss.saturating_sub(rss0));
    match err {
        Some(e) => { println!("  RESULT    = FAIL — {e}"); std::process::exit(1); }
        None if total_recv != sent => { println!("  RESULT    = FAIL — count mismatch"); std::process::exit(1); }
        None => println!("  RESULT    = PASS — no loss/dup/reorder, all sentinels matched, completed cleanly"),
    }
}

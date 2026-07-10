//! Wake-protocol battle for `mpsc_intrusive::Queue` — measures the idle edge
//! and the hot path of `pop_blocking`, so the Condvar+`wait_timeout(2ms)` wake
//! (old) and the park/unpark parked-consumer-slot wake (new) can be compared
//! by running this same file against each implementation.
//!
//! * `idle_block_voluntary_context_switches` — the headline metric. A consumer
//!   blocked on an empty queue for ~1s should context-switch ~once with a
//!   park-based wake; the 2ms-timeout Condvar wakes it ~500 times. Read from
//!   `/proc/thread-self/status` (Linux-only, like the /proc reads elsewhere in
//!   this suite's spirit — gated accordingly).
//! * `hot_throughput_producers_to_blocking_consumer` — guards against the wake
//!   protocol regressing the hot path (it shouldn't touch it at all: when the
//!   queue is never empty, neither wake design is ever reached).
//! * `park_transition_storm_no_lost_wakeup` — hammers the exact empty→sleep
//!   transition the lost-wakeup race lives in: producers deliberately let the
//!   consumer drain to empty and begin sleeping before pushing again. A lost
//!   wakeup shows up as a hang (test harness timeout), not a wrong value.

use axis_codegen_bridge::runtime::mpsc_intrusive::Queue;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Voluntary context switches of the CALLING thread (Linux).
#[cfg(target_os = "linux")]
fn my_voluntary_ctxt_switches() -> u64 {
    let status = std::fs::read_to_string("/proc/thread-self/status")
        .expect("read /proc/thread-self/status");
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("voluntary_ctxt_switches:") {
            return rest.trim().parse().expect("parse voluntary_ctxt_switches");
        }
    }
    panic!("voluntary_ctxt_switches not found in /proc/thread-self/status");
}

/// One consumer blocks on an empty queue for ~1s before the single item
/// arrives. Report how many voluntary context switches the block cost.
/// Condvar+wait_timeout(2ms) ≈ 500/s; park ≈ 1.
#[cfg(target_os = "linux")]
#[test]
fn idle_block_voluntary_context_switches() {
    let q = Arc::new(Queue::<u32>::new());

    let consumer = {
        let q = Arc::clone(&q);
        thread::spawn(move || {
            let before = my_voluntary_ctxt_switches();
            let v = q.pop_blocking();
            let after = my_voluntary_ctxt_switches();
            (v, after - before)
        })
    };

    thread::sleep(Duration::from_secs(1));
    q.push(42);

    let (v, switches) = consumer.join().unwrap();
    assert_eq!(v, 42);
    eprintln!("idle 1s block: {} voluntary context switches", switches);
    // Park-based wake measures 1 here; the retired Condvar+wait_timeout(2ms)
    // wake measured 488. The ceiling is set well above park's number but far
    // below timeout-poll territory, so a regression back to periodic-wakeup
    // sleeping fails loudly rather than shipping silently.
    assert!(switches < 50, "idle block context-switched {} times — timeout-poll regression?", switches);
}

/// Hot path: queue is kept non-empty, so the wake protocol should be
/// irrelevant. Reports throughput so a regression here is visible.
#[test]
fn hot_throughput_producers_to_blocking_consumer() {
    const PRODUCERS: usize = 8;
    const PER_PRODUCER: usize = 100_000;

    let q = Arc::new(Queue::<usize>::new());
    let start = Instant::now();

    let handles: Vec<_> = (0..PRODUCERS)
        .map(|pid| {
            let q = Arc::clone(&q);
            thread::spawn(move || {
                for i in 0..PER_PRODUCER {
                    q.push(pid * PER_PRODUCER + i);
                }
            })
        })
        .collect();

    let mut got = 0usize;
    let mut sum = 0usize;
    while got < PRODUCERS * PER_PRODUCER {
        sum = sum.wrapping_add(q.pop_blocking());
        got += 1;
    }
    for h in handles {
        h.join().unwrap();
    }

    let elapsed = start.elapsed();
    let total = PRODUCERS * PER_PRODUCER;
    let expected_sum: usize = (0..total).sum();
    assert_eq!(sum, expected_sum);
    eprintln!(
        "hot path: {} items in {:?} ({:.1}M items/s)",
        total,
        elapsed,
        total as f64 / elapsed.as_secs_f64() / 1e6
    );
}

/// Force the consumer through the empty→sleep transition as many times as
/// possible: each producer pushes one item, then sleeps long enough for the
/// consumer to drain and commit to sleeping before the next push. A lost
/// wakeup hangs this test; correct wake protocols finish it.
#[test]
fn park_transition_storm_no_lost_wakeup() {
    const ROUNDS: usize = 400;
    const PRODUCERS: usize = 4;

    let q = Arc::new(Queue::<usize>::new());

    let handles: Vec<_> = (0..PRODUCERS)
        .map(|pid| {
            let q = Arc::clone(&q);
            thread::spawn(move || {
                for i in 0..ROUNDS {
                    q.push(pid * ROUNDS + i);
                    // Long enough for the consumer to drain to empty and begin
                    // its sleep before the next item exists — maximizing hits
                    // on the check-then-sleep window.
                    thread::sleep(Duration::from_micros(300));
                }
            })
        })
        .collect();

    let mut seen = std::collections::HashSet::new();
    for _ in 0..PRODUCERS * ROUNDS {
        let v = q.pop_blocking();
        assert!(seen.insert(v), "duplicate item {}", v);
    }
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(seen.len(), PRODUCERS * ROUNDS);
}

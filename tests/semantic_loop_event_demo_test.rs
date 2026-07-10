//! Semantic-loop event-emission demo — three lanes, each on its correct
//! scheduling discipline, wired together with the two swap-ownership
//! primitives this crate now proves out:
//!
//! * **Sensor lane (Ring 1 shape, ~10ms)** — publishes immutable snapshots
//!   into a `LatestSlot`, the single-slot atomic-swap publish-latest buffer
//!   (the "blocker #3" primitive from the axAporia semantic-loop chronicle
//!   note). Publishing never blocks and never takes a lock.
//! * **Fast lane (Ring 2 shape, 1ms period)** — a PERIODIC POLL loop:
//!   sleeps to a fixed deadline, reads the latest snapshot, assesses it,
//!   and on an envelope violation emits an event via
//!   `mpsc_intrusive::Queue::push` — fire-and-forget, one atomic swap plus
//!   a waiter-slot load, no lock, no syscall unless a sleeper exists. The
//!   fast lane's timing must stay deadline-driven: it never waits on data
//!   (that is the semantic-loop rule — determinism comes from the period,
//!   not from arrival).
//! * **Event lane** — blocks on `pop_blocking`, i.e. parked at ZERO cpu
//!   cost while idle (no timeout poll), woken by a direct futex wake only
//!   when an event actually exists.
//!
//! What the test asserts is the paradigm's promise in miniature:
//! every violation produces exactly one event; the idle event lane's
//! context-switch count is proportional to the EVENTS, not to the clock
//! (a 2ms timeout-poll wake would context-switch ~250 times over this
//! run's 500ms regardless of traffic); and fast-lane tick jitter stays
//! bounded while all of this happens around it.

use axis_codegen_bridge::runtime::mpsc_intrusive::Queue;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

// ── LatestSlot: single-slot publish-latest mailbox ──────────────────────────
//
// The publish-latest handoff from the semantic-loop notes: the publisher
// swaps a fresh boxed snapshot in and frees whatever it displaced; the (one)
// reader swaps the slot empty and owns what it got, keeping its previous
// snapshot when the slot is empty ("read latest, or keep last known" — LET
// buffering semantics). Ownership transfers by atomic swap, so exactly one
// side ever frees a given box — the same discipline as `mpsc_intrusive`'s
// waiter slot. A missed intermediate snapshot is not a bug: latest-wins is
// the entire contract.
struct LatestSlot<T> {
    slot: AtomicPtr<T>,
}

// SAFETY: the slot only ever hands a given Box to exactly one thread (the
// swap winner), and T: Send makes moving it across threads sound.
unsafe impl<T: Send> Send for LatestSlot<T> {}
unsafe impl<T: Send> Sync for LatestSlot<T> {}

impl<T> LatestSlot<T> {
    fn new() -> Self {
        LatestSlot { slot: AtomicPtr::new(std::ptr::null_mut()) }
    }

    /// Publisher side: never blocks, never locks. Displaced (stale,
    /// never-read) snapshots are freed here by the publisher itself.
    fn publish(&self, v: T) {
        let fresh = Box::into_raw(Box::new(v));
        let stale = self.slot.swap(fresh, Ordering::AcqRel);
        if !stale.is_null() {
            // SAFETY: the swap gave us exclusive ownership of `stale`.
            unsafe { drop(Box::from_raw(stale)) };
        }
    }

    /// Reader side (single reader): take the latest snapshot if a fresh one
    /// was published since the last take, else `None` (caller keeps its
    /// previous snapshot — the poll loop's "last known state").
    fn take_latest(&self) -> Option<T> {
        let p = self.slot.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if p.is_null() {
            None
        } else {
            // SAFETY: the swap gave us exclusive ownership of `p`.
            Some(*unsafe { Box::from_raw(p) })
        }
    }
}

impl<T> Drop for LatestSlot<T> {
    fn drop(&mut self) {
        let p = self.slot.load(Ordering::Relaxed);
        if !p.is_null() {
            // SAFETY: Drop means no concurrent access remains.
            unsafe { drop(Box::from_raw(p)) };
        }
    }
}

// ── Lane payloads ────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Snapshot {
    seq: u64,
    reading: f64,
}

enum Event {
    EnvelopeViolation { snapshot_seq: u64, detected_at: Instant },
    Shutdown,
}

#[cfg(target_os = "linux")]
fn my_voluntary_ctxt_switches() -> u64 {
    let status = std::fs::read_to_string("/proc/thread-self/status")
        .expect("read /proc/thread-self/status");
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("voluntary_ctxt_switches:") {
            return rest.trim().parse().expect("parse voluntary_ctxt_switches");
        }
    }
    panic!("voluntary_ctxt_switches not found");
}

#[test]
fn ring_lanes_emit_events_and_event_lane_parks_idle() {
    const FAST_PERIOD: Duration = Duration::from_millis(1);
    const FAST_TICKS: u64 = 500; // ~500ms run
    const SENSOR_PERIOD: Duration = Duration::from_millis(10);
    const ENVELOPE_LIMIT: f64 = 10.0;
    // Sensor injects a fault reading on these snapshot seqs (~every 100ms).
    const FAULT_SEQS: [u64; 4] = [10, 20, 30, 40];

    let slot = Arc::new(LatestSlot::<Snapshot>::new());
    let events = Arc::new(Queue::<Event>::new());
    let stop = Arc::new(AtomicBool::new(false));

    // ── Sensor lane (Ring 1, ~10ms): publish-latest, never blocks ──────────
    let sensor = {
        let slot = Arc::clone(&slot);
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            let mut seq = 0u64;
            while !stop.load(Ordering::Relaxed) {
                seq += 1;
                let reading = if FAULT_SEQS.contains(&seq) { 99.0 } else { 0.5 };
                slot.publish(Snapshot { seq, reading });
                thread::sleep(SENSOR_PERIOD);
            }
        })
    };

    // ── Event lane: parked while idle, woken per event ──────────────────────
    let event_lane = {
        let events = Arc::clone(&events);
        thread::spawn(move || {
            #[cfg(target_os = "linux")]
            let switches_before = my_voluntary_ctxt_switches();
            let mut seen: Vec<(u64, Duration)> = Vec::new();
            loop {
                match events.pop_blocking() {
                    Event::EnvelopeViolation { snapshot_seq, detected_at } => {
                        seen.push((snapshot_seq, detected_at.elapsed()));
                    }
                    Event::Shutdown => break,
                }
            }
            #[cfg(target_os = "linux")]
            let switches = my_voluntary_ctxt_switches() - switches_before;
            #[cfg(not(target_os = "linux"))]
            let switches = 0u64;
            (seen, switches)
        })
    };

    // ── Fast lane (Ring 2, 1ms period): deadline poll + event emission ─────
    // Runs on the test thread. Sleeps to each fixed deadline, reads latest,
    // assesses, emits. It never blocks on data — `take_latest` returning
    // None just means "keep last known state", and the tick completes on
    // schedule regardless.
    let start = Instant::now();
    let mut last_known: Option<Snapshot> = None;
    let mut alerted_up_to_seq = 0u64;
    let mut max_jitter = Duration::ZERO;

    for tick in 1..=FAST_TICKS {
        let deadline = start + FAST_PERIOD * tick as u32;
        if let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            thread::sleep(remaining);
        }
        max_jitter = max_jitter.max(Instant::now().duration_since(deadline));

        if let Some(fresh) = slot.take_latest() {
            last_known = Some(fresh);
        }
        if let Some(s) = last_known {
            // Envelope assessment on the latest known snapshot; alert once
            // per offending snapshot, not once per tick that re-sees it.
            if s.reading > ENVELOPE_LIMIT && s.seq > alerted_up_to_seq {
                alerted_up_to_seq = s.seq;
                events.push(Event::EnvelopeViolation {
                    snapshot_seq: s.seq,
                    detected_at: Instant::now(),
                });
            }
        }
    }

    stop.store(true, Ordering::Relaxed);
    events.push(Event::Shutdown);
    sensor.join().unwrap();
    let (seen, switches) = event_lane.join().unwrap();

    // Every injected fault produced exactly one event, in order.
    let seen_seqs: Vec<u64> = seen.iter().map(|(s, _)| *s).collect();
    assert_eq!(seen_seqs, FAULT_SEQS, "each fault snapshot alerts exactly once");

    let worst_latency = seen.iter().map(|(_, l)| *l).max().unwrap();
    eprintln!(
        "fast lane: {} ticks @ {:?}, max deadline jitter {:?}",
        FAST_TICKS, FAST_PERIOD, max_jitter
    );
    eprintln!(
        "event lane: {} events, worst wake-to-handle latency {:?}, {} voluntary context switches over ~{:?} run",
        seen.len(),
        worst_latency,
        switches,
        start.elapsed()
    );

    // The paradigm assertions (generous bounds — the printed numbers are
    // the metric; these exist to fail loudly on a regression in kind):
    //
    // Event lane cost tracks EVENTS, not the clock. A 2ms timeout-poll wake
    // would context-switch ~250 times over this 500ms run while handling
    // the same 4 events.
    #[cfg(target_os = "linux")]
    assert!(
        switches < 60,
        "idle event lane context-switched {} times — timeout-poll regression?",
        switches
    );
    // Direct futex wake, not a poll-quantized one.
    assert!(
        worst_latency < Duration::from_millis(50),
        "event latency {:?} — wake path degraded?",
        worst_latency
    );
    // Fast-lane ticks stayed deadline-driven while emission/waking happened
    // around them (loose: CI schedulers are noisy).
    assert!(
        max_jitter < Duration::from_millis(20),
        "fast-lane jitter {:?} — periodic lane lost its timing discipline?",
        max_jitter
    );
}

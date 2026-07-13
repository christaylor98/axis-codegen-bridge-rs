//! BRIDGE_SLABSHADOW_V1 (AXVERITY_RECLOG_SLA_BLOCK_SHADOW_VALIDATION_V1) — the
//! shadow tap that runs slablock (Phase A, slablock.rs) ALONGSIDE the live
//! reclog write path, purely for measurement. Closes C2's open half: does the
//! system absorb slablock's tick-fsyncs under real concurrent INSERT load, or
//! do they contend with the reclog janitor's fsyncs on the same journal.
//!
//! ## The four hard limits, and where each is enforced STRUCTURALLY
//!
//! * ACK_GATES_ON_RECLOG_ONLY_UNCHANGED — the tap call sits AFTER
//!   `pg_reply_tag` in pg_exec_insert.m1 (post-ack placement): the client's
//!   reply bytes are already on the socket before `slab_shadow_submit` runs.
//!   Nothing here touches reclog.rs.
//! * NO_SYNCHRONOUS_LATENCY_ON_WORKER — `slab_shadow_submit` is a bounded
//!   TRY-send (`bounded_try_send`, channels.rs): one mutex lock + len check +
//!   push, NEVER a blocking wait. Worst case (queue full) is the same lock +
//!   len check and an atomic drop-count increment.
//! * SHADOW_FAILURE_ISOLATED — the janitor runs on its own `--entries` thread,
//!   which main.rs spawns wrapped in `std::panic::catch_unwind` (rust_05.rs
//!   emitter): a panic here (ENOSPC, anything) kills ONLY the janitor. After
//!   janitor death the bounded queue fills ONCE to cap, then every submit
//!   drops in O(1) — bounded memory, measurement stops, worker never notices.
//!   This is why the tap channel is BOUNDED-drop, not the unbounded
//!   `channel_send` (whose grow-without-bound semantics would turn a dead
//!   janitor into a live-process memory leak — the hotmem-pattern assumption
//!   in the intent, evaluated and corrected).
//! * SHADOW_DATA_NEVER_AUTHORITATIVE — everything is written under
//!   `AXVERITY_SLAB_SHADOW_DIR` (default `.axverity-shadow`, a SIBLING of
//!   `.axverity/` on the same filesystem — same journal, which the contention
//!   question requires, but no query path, GC walk, scrub, or index reader
//!   ever looks there). Disposable; delete freely.
//!
//! ## Enablement
//!
//! Default OFF. The tap is compiled into pg_exec_insert but `slab_shadow_submit`
//! returns immediately (-1) unless the server is launched with
//! `AXVERITY_SLAB_SHADOW=1` (read once, OnceLock). This gives the A/B windows:
//! identical binary, env-toggled shadow.
//!
//! ## Measurement output (janitor thread, buffered writes, never fsynced)
//!
//! `<dir>/measure-<pid>.tsv`, two record kinds:
//!   `R\t<arrival_mono_ns>\t<drain_mono_ns>\t<name>`  — one per shadowed row
//!     (arrival stamped on the worker at submit: ~one clock read, the row's
//!     queue latency AND the cardinality/arrival-rate profile in one line).
//!   `T\t<mono_ns>\t<fsyncs>\t<tick_us>\t<submitted>\t<dropped>` — one per
//!     fired tick sweep (slab_tick >= 0), with cumulative counters so drop
//!     coverage is always visible in the data itself.
//!
//! Tunables (env, read once per janitor thread): AXVERITY_SLAB_SHADOW_SLA_US
//! (default 5000 — the 5ms tier), AXVERITY_SLAB_SHADOW_BATCH / _WINDOW_MS
//! (default 256 / 2, mirroring the reclog janitor's own drain cadence).
//!
//! Identities are sha256(name_utf8), the bridge-wide convention.

use std::cell::RefCell;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use super::channels::{bounded_drain_batch, bounded_try_send};
use super::slablock::{slab_append, slab_open, slab_tick};
use super::value::{get_str, intern_str, Value};

/// The shadow hand-off channel (bounded registry; internal — not an M1-level
/// channel_send target, so CHANNELS_STATIC does not apply, same as reclog's).
const SHADOW_CHAN: &str = "slab-shadow";

static SUBMITTED: AtomicU64 = AtomicU64::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);

fn enabled() -> bool {
    static E: OnceLock<bool> = OnceLock::new();
    *E.get_or_init(|| std::env::var("AXVERITY_SLAB_SHADOW").map(|v| v == "1").unwrap_or(false))
}

fn mono_nanos() -> i64 {
    static BASE: OnceLock<Instant> = OnceLock::new();
    BASE.get_or_init(Instant::now).elapsed().as_nanos() as i64
}

fn env_i64(key: &str, default: i64) -> i64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

/// `slab_shadow_submit(bytes: Bytes, name: Text) -> Int`
///
/// The worker-side tap: stamp arrival, TRY-enqueue onto the bounded shadow
/// channel. Returns 1 = enqueued, 0 = dropped (queue full — shadow lagging or
/// janitor dead; counted, never blocking), -1 = shadow disabled. The caller
/// (pg_exec_insert, post-ack) discards the return either way.
#[track_caller]
pub fn slab_shadow_submit(args: Value) -> Value {
    let (bytes, name) = match args {
        Value::Tuple(mut es) if es.len() == 2 => {
            let name = es.pop().unwrap();
            let bytes = es.pop().unwrap();
            (bytes, name)
        }
        other => panic!("slab_shadow_submit: expected Tuple(Bytes, Text), got {:?}", other),
    };
    if !enabled() {
        return Value::Int(-1);
    }
    match &bytes {
        Value::Bytes(_) => {}
        other => panic!("slab_shadow_submit: arg 0 expected Bytes, got {:?}", other),
    }
    match &name {
        Value::Str(_) => {}
        other => panic!("slab_shadow_submit: arg 1 expected Text, got {:?}", other),
    }
    let item = Value::Tuple(vec![Value::Int(mono_nanos()), bytes, name]);
    if bounded_try_send(SHADOW_CHAN, item) {
        SUBMITTED.fetch_add(1, Ordering::Relaxed);
        Value::Int(1)
    } else {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        Value::Int(0)
    }
}

/// The janitor thread's lazily-minted state: its thread-local slab handle plus
/// the buffered measurement writer. Thread-local — shared-nothing, exactly the
/// slablock/logbuf discipline; the ONLY cross-thread structure in this module
/// is the bounded hand-off queue itself (the same role reclog-batch plays for
/// the real path).
struct ShadowState {
    slab_h: i64,
    out: std::io::BufWriter<std::fs::File>,
}

thread_local! {
    static SHADOW: RefCell<Option<ShadowState>> = const { RefCell::new(None) };
}

fn init_state() -> ShadowState {
    let dir = std::env::var("AXVERITY_SLAB_SHADOW_DIR")
        .unwrap_or_else(|_| String::from(".axverity-shadow"));
    let sla_us = env_i64("AXVERITY_SLAB_SHADOW_SLA_US", 5_000);
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("slab_shadow: mkdir {}: {}", dir, e));
    let slab_h = match slab_open(Value::Tuple(vec![
        Value::Str(intern_str(&format!("{}/slab", dir))),
        Value::Int(sla_us),
        Value::Int(0),
    ])) {
        Value::Int(h) => h,
        other => panic!("slab_shadow: slab_open returned {:?}", other),
    };
    let path = format!("{}/measure-{}.tsv", dir, std::process::id());
    let f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .unwrap_or_else(|e| panic!("slab_shadow: open {}: {}", path, e));
    ShadowState { slab_h, out: std::io::BufWriter::new(f) }
}

/// `slab_shadow_flush_once(Unit) -> Int`
///
/// One janitor round: block in `bounded_drain_batch` until ≥1 shadowed row,
/// append each to this thread's slab (positional, never fsyncing), run the
/// SLA-gated `slab_tick`, and log measurement lines. Returns the batch size.
/// Panics propagate — by design: the entries wrapper catches them and the
/// drop-on-full submit keeps the worker unharmed (see module doc).
#[track_caller]
pub fn slab_shadow_flush_once(_: Value) -> Value {
    let cap = env_i64("AXVERITY_SLAB_SHADOW_BATCH", 256) as usize;
    let window = env_i64("AXVERITY_SLAB_SHADOW_WINDOW_MS", 2) as u64;
    let batch = bounded_drain_batch(SHADOW_CHAN, cap, window);
    let n = batch.len() as i64;
    SHADOW.with(|s| {
        let mut slot = s.borrow_mut();
        let state = slot.get_or_insert_with(init_state);
        let drain_ns = mono_nanos();
        for item in batch {
            let mut fields = match item {
                Value::Tuple(es) if es.len() == 3 => es.into_iter(),
                other => panic!("slab_shadow_flush_once: malformed item {:?}", other),
            };
            let arrival = match fields.next().unwrap() {
                Value::Int(ns) => ns,
                other => panic!("slab_shadow_flush_once: field 0 expected Int, got {:?}", other),
            };
            let bytes = fields.next().unwrap();
            let name = match fields.next().unwrap() {
                Value::Str(h) => get_str(&h),
                other => panic!("slab_shadow_flush_once: field 2 expected Text, got {:?}", other),
            };
            match slab_append(Value::Tuple(vec![Value::Int(state.slab_h), bytes])) {
                Value::Int(_) => {}
                other => panic!("slab_shadow_flush_once: slab_append returned {:?}", other),
            }
            writeln!(state.out, "R\t{}\t{}\t{}", arrival, drain_ns, name)
                .unwrap_or_else(|e| panic!("slab_shadow: measure write: {}", e));
        }
        let t0 = Instant::now();
        let ticked = match slab_tick(Value::Int(state.slab_h)) {
            Value::Int(r) => r,
            other => panic!("slab_shadow_flush_once: slab_tick returned {:?}", other),
        };
        if ticked >= 0 {
            let tick_us = t0.elapsed().as_micros() as i64;
            writeln!(
                state.out,
                "T\t{}\t{}\t{}\t{}\t{}",
                mono_nanos(),
                ticked,
                tick_us,
                SUBMITTED.load(Ordering::Relaxed),
                DROPPED.load(Ordering::Relaxed)
            )
            .unwrap_or_else(|e| panic!("slab_shadow: measure write: {}", e));
        }
        state
            .out
            .flush()
            .unwrap_or_else(|e| panic!("slab_shadow: measure flush: {}", e));
    });
    Value::Int(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::channels::bounded_len;
    use std::time::Duration;

    // NOTE on env: `enabled()` latches on first read. These tests set the var
    // before any submit call in this process; the drill tests that need the
    // tap DISABLED live in the integration harness (separate process).
    fn force_enabled() {
        std::env::set_var("AXVERITY_SLAB_SHADOW", "1");
        assert!(enabled());
    }

    /// These tests share one process-global bounded channel and env vars —
    /// serialize them so the default parallel test runner can't interleave.
    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        static L: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
        L.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    /// Drain everything currently queued WITHOUT blocking on empty
    /// (`bounded_drain_batch` blocks until ≥1 item, so guard on depth first).
    fn drain_all() {
        while bounded_len(SHADOW_CHAN) > 0 {
            let _ = bounded_drain_batch(SHADOW_CHAN, 100_000, 0);
        }
    }

    fn scratch(tag: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("axv-shadow-{}-{}-{}", tag, std::process::id(), nanos))
            .to_string_lossy()
            .into_owned()
    }

    fn submit(bytes: &[u8], name: &str) -> i64 {
        match slab_shadow_submit(Value::Tuple(vec![
            Value::Bytes(bytes.to_vec()),
            Value::Str(intern_str(name)),
        ])) {
            Value::Int(r) => r,
            other => panic!("submit returned {:?}", other),
        }
    }

    /// DRILL 1 (SHADOW_FAILURE_ISOLATED / NO_SYNCHRONOUS_LATENCY): with NO
    /// janitor draining, submits fill the bounded queue once, then every
    /// further submit returns 0 (dropped) WITHOUT blocking — the dead-janitor
    /// steady state. Also bounds the per-submit worst case empirically.
    #[test]
    fn drill_drop_on_full_never_blocks() {
        let _g = test_lock();
        force_enabled();
        drain_all();
        let row = vec![0x41u8; 120];
        let mut enq = 0u64;
        let mut drop = 0u64;
        let t0 = Instant::now();
        let total = 5_000; // well past any cap
        for i in 0..total {
            match submit(&row, &format!("t:{}", i)) {
                1 => enq += 1,
                0 => drop += 1,
                r => panic!("unexpected submit return {}", r),
            }
        }
        let elapsed = t0.elapsed();
        assert!(enq > 0, "some submits must enqueue before the queue fills");
        assert!(drop > 0, "queue must fill and drop with no janitor");
        assert_eq!(enq + drop, total as u64);
        // 5000 submits with zero draining must complete in far under a second —
        // i.e. no submit ever parked on a condvar. Generous bound for CI noise.
        assert!(
            elapsed < Duration::from_millis(500),
            "submits blocked: {} submits took {:?}",
            total,
            elapsed
        );
        let per = elapsed / total;
        eprintln!(
            "drill 1: {} enqueued, {} dropped, {:?} total (~{:?}/submit, worst-case path)",
            enq, drop, elapsed, per
        );
        assert!(per < Duration::from_micros(58), "per-submit cost exceeds the Phase A ceiling");
        drain_all(); // leave the shared channel clean for the next test
    }

    /// Round trip: submit K rows, one flush round appends all K to the shadow
    /// slab, writes K R-lines, and the tick fires (1µs tier).
    #[test]
    fn flush_round_trip_appends_and_measures() {
        let _g = test_lock();
        force_enabled();
        let dir = scratch("roundtrip");
        // Janitor state is thread-local — run the janitor round on a dedicated
        // thread with its own env-derived dir.
        std::env::set_var("AXVERITY_SLAB_SHADOW_DIR", &dir);
        std::env::set_var("AXVERITY_SLAB_SHADOW_SLA_US", "1");
        drain_all();
        for i in 0..10 {
            assert_eq!(submit(format!("row-{:04}", i).as_bytes(), "orders:A-1"), 1);
        }
        let dir2 = dir.clone();
        let n = std::thread::spawn(move || {
            std::env::set_var("AXVERITY_SLAB_SHADOW_DIR", &dir2);
            match slab_shadow_flush_once(Value::Unit) {
                Value::Int(n) => n,
                other => panic!("flush returned {:?}", other),
            }
        })
        .join()
        .unwrap();
        assert_eq!(n, 10);
        let slab_bin = std::fs::read(format!("{}/slab/blk-0.bin", dir)).unwrap();
        assert_eq!(slab_bin.len(), 8 * 10, "all 10 rows appended to the shadow block");
        let measure = std::fs::read_to_string(
            std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .find(|e| e.file_name().to_string_lossy().starts_with("measure-"))
                .expect("measure file")
                .path(),
        )
        .unwrap();
        assert_eq!(measure.lines().filter(|l| l.starts_with("R\t")).count(), 10);
        assert_eq!(measure.lines().filter(|l| l.starts_with("T\t")).count(), 1, "tick fired");
        assert!(measure.contains("orders:A-1"), "cardinality profile carries the name");
    }

    /// DRILL 2 (SHADOW_FAILURE_ISOLATED): a janitor round that hits an
    /// unwritable shadow dir panics — and the panic is CONFINED to the janitor
    /// thread (catch_unwind, the same wrapper main.rs puts around --entries
    /// threads). The submitting side keeps working in drop mode.
    #[test]
    fn drill_janitor_panic_is_confined() {
        let _g = test_lock();
        force_enabled();
        drain_all();
        assert_eq!(submit(b"doomed", "t:x"), 1);
        let result = std::thread::spawn(|| {
            std::env::set_var("AXVERITY_SLAB_SHADOW_DIR", "/proc/axv-cannot-create-this");
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                slab_shadow_flush_once(Value::Unit)
            }))
        })
        .join()
        .unwrap();
        assert!(result.is_err(), "janitor round must panic on unwritable dir");
        // The process (this test) is alive; submits still work (drop or enqueue).
        let r = submit(b"survivor", "t:y");
        assert!(r == 0 || r == 1, "worker-side submit unaffected by janitor panic");
        drain_all();
    }
}

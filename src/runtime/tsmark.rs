//! TS_MARK — TEMPORARY per-segment timing instrumentation for
//! AXVERITY_INSERT_PATH_TIMING_AUDIT_V1. NOT a production primitive: it captures
//! timestamps only and has zero effect on any functional path (a mark records
//! (id, monotonic_nanos) on a thread-local Vec and returns Unit). Thread-local so
//! N worker threads never contend and the probe stays cheap; monotonic clock
//! (Instant since a process-global baseline) so deltas are jump-free.
//!
//! ts_mark(id) is called at each named INSERT-path segment boundary; ts_flush(dir)
//! is called ONCE at connection close (off the measured path) to dump this
//! thread's marks to a unique file and clear them. Deltas between consecutive
//! marks are the per-segment times. This module is expected to be reverted after
//! the audit — it is instrumentation, not a feature.
//!
//! ## AXVERITY_WRITEPATH_PERF_DECOMPOSITION_V1 extension ("writeprobe")
//!
//! Gated, disableable, additive. When `AXVERITY_WRITEPROBE` is unset (the
//! default), every `ts_mark` / `ts_mark_val` / `mark()` call does EXACTLY what
//! it did before this turn — push `(id, wall_ns)` onto `MARKS`, nothing else;
//! `ts_flush` writes exactly the same `ts-<pid>-<seq>.tsv` it always has. The
//! ONLY per-call addition when disabled is one relaxed `OnceLock<bool>` load
//! (mirrors coldprobe.rs's own `enabled()` gate).
//!
//! When `AXVERITY_WRITEPROBE=1`, the SAME ~30 existing call sites across the
//! write path (pg_accept_one, pg_serve_conn, pg_query_step, pg_dispatch,
//! pg_handle_query, pg_route, pg_exec_insert, pg_hotblk_seal_mint,
//! pg_hotblk_mint, reclog_janitor_step, and reclog.rs's own Rust-side
//! `mark(70..73)` calls bracketing the payload-WAL and name-log fsyncs)
//! ADDITIONALLY capture, per mark:
//!   * `cpu_ns`            — this thread's on-CPU time (CLOCK_THREAD_CPUTIME_ID)
//!   * `tid`               — the REAL OS thread id (gettid(2)), so which thread
//!                           executes a stage is measured, never inferred
//!   * `rss_kb`/`minflt`/`majflt` — `getrusage(RUSAGE_THREAD, ..)`: this
//!                           thread's peak RSS (KB, monotonic high-water mark)
//!                           and minor/major page-fault counts (a major fault
//!                           is a strong, independent cross-check for genuine
//!                           disk-wait, distinct from the wall-minus-cpu split)
//!   * allocator counters  — `allocprobe::snapshot()`, four PROCESS-WIDE
//!                           cumulative counters (alloc/dealloc bytes, alloc/
//!                           dealloc counts). Per-stage deltas are computed
//!                           offline by diffing consecutive marks; because the
//!                           counters are process-wide, a delta is only cleanly
//!                           attributable to the marking thread's own stage
//!                           under single-writer concurrency — see the turn's
//!                           report for how multi-writer runs are handled.
//! into a SEPARATE thread-local buffer (`PMARKS`), dumped by the SAME
//! `ts_flush` call to a NEW file `<dir>/wp-<pid>-<seq>.tsv` alongside the
//! existing `ts-<pid>-<seq>.tsv` (which is untouched — old file, old format,
//! zero risk to anything that already parses it).
//!
//! No new call sites were added anywhere in M1 or Rust source for this
//! extension — it reuses the existing instrumentation verbatim, per the
//! turn's hard constraint.

use std::cell::RefCell;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use super::allocprobe;
use super::value::{get_str, Value};

static BASE: OnceLock<Instant> = OnceLock::new();
static FLUSH_SEQ: AtomicU64 = AtomicU64::new(0);

/// One extended-capture record. Field order matches the `wp-*.tsv` column order.
#[derive(Clone, Copy)]
struct ProbeRec {
    id: i64,
    wall_ns: i64,
    cpu_ns: i64,
    tid: i64,
    rss_kb: i64,
    minflt: i64,
    majflt: i64,
    alloc_bytes: i64,
    dealloc_bytes: i64,
    alloc_count: i64,
    dealloc_count: i64,
}

thread_local! {
    /// This thread's timing marks: (segment id, monotonic nanos since BASE).
    static MARKS: RefCell<Vec<(i64, i64)>> = RefCell::new(Vec::with_capacity(4096));
    /// This thread's writeprobe-extended marks (only populated when the
    /// AXVERITY_WRITEPROBE gate is on).
    static PMARKS: RefCell<Vec<ProbeRec>> = RefCell::new(Vec::with_capacity(4096));
}

fn mono_nanos() -> i64 {
    let base = BASE.get_or_init(Instant::now);
    base.elapsed().as_nanos() as i64
}

/// Read the writeprobe gate ONCE per process. Unset / `0` / `off` / `false` →
/// disabled (the default, zero-overhead beyond the one bool load). `1` / `on` /
/// `true` → enabled.
#[inline]
fn writeprobe_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        matches!(
            std::env::var("AXVERITY_WRITEPROBE")
                .ok()
                .as_deref()
                .map(|s| s.to_ascii_lowercase())
                .as_deref(),
            Some("1") | Some("on") | Some("true")
        )
    })
}

#[inline]
fn clock_ns(clk: libc::clockid_t) -> i64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid, initialized timespec; clock_gettime only writes it.
    unsafe {
        libc::clock_gettime(clk, &mut ts);
    }
    (ts.tv_sec as i64) * 1_000_000_000 + (ts.tv_nsec as i64)
}

#[inline]
fn cpu_ns() -> i64 {
    clock_ns(libc::CLOCK_THREAD_CPUTIME_ID)
}

/// The real OS thread id (Linux `gettid(2)`) — NOT Rust's `std::thread::ThreadId`
/// (which is a process-local allocator counter, not a kernel-visible id). This is
/// what makes thread attribution MEASURED rather than inferred.
#[inline]
fn os_tid() -> i64 {
    unsafe { libc::syscall(libc::SYS_gettid) as i64 }
}

/// `(maxrss_kb, minflt, majflt)` for THIS thread via `getrusage(RUSAGE_THREAD)`.
/// `ru_maxrss` is a monotonic high-water mark (KB) since thread start, not a
/// per-stage delta a stage can shrink — documented, not silently treated as
/// exact per-stage retained memory. `ru_majflt` (major faults, page-ins that
/// blocked on disk) is an independent cross-check for genuine disk-wait,
/// distinct from the wall-minus-cpu split.
#[inline]
fn thread_rusage() -> (i64, i64, i64) {
    unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_THREAD, &mut ru);
        (ru.ru_maxrss as i64, ru.ru_minflt as i64, ru.ru_majflt as i64)
    }
}

/// Capture the writeprobe-extended record for `id` at `wall_ns`, IF the gate is
/// on. This is the only new work added to every ts_mark/ts_mark_val/mark() call;
/// when the gate is off it is a single relaxed load and nothing else.
#[inline]
fn capture_probe(id: i64, wall_ns: i64) {
    if !writeprobe_enabled() {
        return;
    }
    let cpu = cpu_ns();
    let tid = os_tid();
    let (rss_kb, minflt, majflt) = thread_rusage();
    let (ab, db, ac, dc) = allocprobe::snapshot();
    PMARKS.with(|m| {
        m.borrow_mut().push(ProbeRec {
            id,
            wall_ns,
            cpu_ns: cpu,
            tid,
            rss_kb,
            minflt,
            majflt,
            alloc_bytes: ab,
            dealloc_bytes: db,
            alloc_count: ac,
            dealloc_count: dc,
        })
    });
}

/// `ts_mark(id: Int) -> Unit` — record (id, monotonic_nanos) on the calling
/// thread. The whole body is: read the clock, push a pair, return Unit.
#[track_caller]
pub fn ts_mark(arg: Value) -> Value {
    let id = match arg {
        Value::Int(n) => n,
        other => panic!("ts_mark: expected Int id, got {:?}", other),
    };
    let n = mono_nanos();
    MARKS.with(|m| m.borrow_mut().push((id, n)));
    capture_probe(id, n);
    Value::Unit
}

/// Rust-callable mark — records (id, monotonic_nanos) on the CALLING thread.
/// Used to bracket bridge-internal work (e.g. the janitor fsync in reclog.rs) on
/// the actual thread executing it, per the audit's on-the-executing-thread rule.
pub fn mark(id: i64) {
    let n = mono_nanos();
    MARKS.with(|m| m.borrow_mut().push((id, n)));
    capture_probe(id, n);
}

/// `ts_mark_val(id: Int, val: Int) -> Unit` — record (id, val) VERBATIM, where
/// val is a measured quantity (queue depth, batch size, byte count), NOT a
/// timestamp. The reader distinguishes value-marks from time-marks by id.
#[track_caller]
pub fn ts_mark_val(args: Value) -> Value {
    let (id, val) = match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("ts_mark_val: expected Tuple(Int, Int), got {:?}", other),
    };
    let id = match id { Value::Int(n) => n, other => panic!("ts_mark_val: id not Int: {:?}", other) };
    let val = match val { Value::Int(n) => n, other => panic!("ts_mark_val: val not Int: {:?}", other) };
    MARKS.with(|m| m.borrow_mut().push((id, val)));
    // Deliberately NOT captured into PMARKS: `val` is not a wall timestamp (it's
    // a queue depth / batch size), and capture_probe's cpu_ns/tid/rss fields are
    // only meaningful bracketing a REAL clock reading. ts_mark_val's `val` still
    // lands in the plain MARKS vec (and hence ts-*.tsv) exactly as before.
    Value::Unit
}

/// `ts_flush(dir: Text) -> Int` — write this thread's marks to a unique file
/// `<dir>/ts-<pid>-<seq>.tsv` as `"<id>\t<nanos>\n"` lines, clear them, return
/// the count. Called at connection close, off the measured segment path.
///
/// ALSO (writeprobe extension): if this thread accumulated any PMARKS records
/// (i.e. AXVERITY_WRITEPROBE was on for at least one mark since the last
/// flush), dump them to a NEW file `<dir>/wp-<pid>-<seq>.tsv` using the SAME
/// pid-seq stamp, then clear. The pre-existing `ts-*.tsv` output and return
/// value are completely unchanged by this addition.
#[track_caller]
pub fn ts_flush(arg: Value) -> Value {
    let dir = match arg {
        Value::Str(h) => get_str(&h),
        other => panic!("ts_flush: expected Text dir, got {:?}", other),
    };
    let count = MARKS.with(|m| {
        let mut marks = m.borrow_mut();
        if marks.is_empty() {
            return 0i64;
        }
        let _ = std::fs::create_dir_all(&dir);
        let seq = FLUSH_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = format!("{}/ts-{}-{}.tsv", dir, std::process::id(), seq);
        let mut s = String::with_capacity(marks.len() * 16);
        for (id, n) in marks.iter() {
            s.push_str(&format!("{}\t{}\n", id, n));
        }
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(s.as_bytes()));
        let count = marks.len() as i64;
        marks.clear();
        // Reuse the SAME pid-seq stamp for this flush's writeprobe file so the
        // two files can be correlated by filename if ever needed.
        PMARKS.with(|p| {
            let mut pm = p.borrow_mut();
            if !pm.is_empty() {
                let wp_path = format!("{}/wp-{}-{}.tsv", dir, std::process::id(), seq);
                let mut ws = String::with_capacity(pm.len() * 64);
                for r in pm.iter() {
                    ws.push_str(&format!(
                        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                        r.id, r.wall_ns, r.cpu_ns, r.tid, r.rss_kb, r.minflt, r.majflt,
                        r.alloc_bytes, r.dealloc_bytes, r.alloc_count, r.dealloc_count
                    ));
                }
                let _ = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&wp_path)
                    .and_then(|mut f| f.write_all(ws.as_bytes()));
                pm.clear();
            }
        });
        count
    });
    Value::Int(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_accumulate_and_flush_clears() {
        for id in [10i64, 11, 12] {
            ts_mark(Value::Int(id));
        }
        let dir = std::env::temp_dir().join(format!("axv-ts-{}", std::process::id()));
        let dir = dir.to_string_lossy().into_owned();
        let c = ts_flush(Value::Str(super::super::value::intern_str(&dir)));
        assert_eq!(c, Value::Int(3));
        // second flush with no marks -> 0
        assert_eq!(ts_flush(Value::Str(super::super::value::intern_str(&dir))), Value::Int(0));
    }

    #[test]
    fn nanos_monotonic_nondecreasing() {
        let a = mono_nanos();
        let b = mono_nanos();
        assert!(b >= a);
    }

    #[test]
    fn writeprobe_disabled_by_default_pmarks_stay_empty() {
        // Gate reads env once per process; in the test binary it is unset, so
        // capture_probe must be a no-op and PMARKS must never populate.
        for id in [900i64, 901, 902] {
            ts_mark(Value::Int(id));
        }
        let had_any = PMARKS.with(|m| !m.borrow().is_empty());
        assert!(!had_any, "PMARKS should stay empty when AXVERITY_WRITEPROBE is unset");
    }
}

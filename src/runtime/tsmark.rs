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

use std::cell::RefCell;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use super::value::{get_str, Value};

static BASE: OnceLock<Instant> = OnceLock::new();
static FLUSH_SEQ: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// This thread's timing marks: (segment id, monotonic nanos since BASE).
    static MARKS: RefCell<Vec<(i64, i64)>> = RefCell::new(Vec::with_capacity(4096));
}

fn mono_nanos() -> i64 {
    let base = BASE.get_or_init(Instant::now);
    base.elapsed().as_nanos() as i64
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
    Value::Unit
}

/// Rust-callable mark — records (id, monotonic_nanos) on the CALLING thread.
/// Used to bracket bridge-internal work (e.g. the janitor fsync in reclog.rs) on
/// the actual thread executing it, per the audit's on-the-executing-thread rule.
pub fn mark(id: i64) {
    let n = mono_nanos();
    MARKS.with(|m| m.borrow_mut().push((id, n)));
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
    Value::Unit
}

/// `ts_flush(dir: Text) -> Int` — write this thread's marks to a unique file
/// `<dir>/ts-<pid>-<seq>.tsv` as `"<id>\t<nanos>\n"` lines, clear them, return
/// the count. Called at connection close, off the measured segment path.
#[track_caller]
pub fn ts_flush(arg: Value) -> Value {
    let dir = match arg {
        Value::Str(h) => get_str(&h),
        other => panic!("ts_flush: expected Text dir, got {:?}", other),
    };
    MARKS.with(|m| {
        let mut marks = m.borrow_mut();
        if marks.is_empty() {
            return Value::Int(0);
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
        Value::Int(count)
    })
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
}

//! BRIDGE_CURSOR_V1 (AXVERITY_LOOPSTATE_CURSOR_BUILD_V1) — a mutable append
//! cursor: a per-thread growable buffer reached by an opaque integer handle,
//! used to move a loop's growing OUTPUT accumulator OFF the M1 capture-free
//! loop state (a single packed `Text` that `loop_while`/`loop_count` re-parses
//! and re-concatenates every step → O(M²)). Appending to the cursor is O(1)
//! amortized and the loop state only carries the small integer handle, so the
//! accumulation term drops from O(M²) to O(M). Validated shape:
//! AXVERITY_PRE_IMPLEMENTATION_SPIKE_V1 Spike B.
//!
//! ## Storage model — THREAD-OWNED, NO shared registry
//!   (the same nothing-shared, nothing-locked rule as `walindex.rs` /
//!    `fieldidx.rs` / `logbuf.rs`)
//!
//! Each open cursor lives in **thread-local storage** (`CURS`), reachable ONLY
//! by the thread that opened it, keyed by an opaque integer handle. There is no
//! process-global handle→buffer registry and no lock anywhere on the
//! append/get/len path: two worker threads share NOTHING and can never contend.
//! The handle counter (`NEXT`) is thread-local too, starting at 1 per thread.
//! This matches the pg_server worker-pool model exactly: the thread that runs a
//! GROUP BY query opens, appends to, reads, and closes its own cursor — the
//! cursor is never handed to another worker (cross-worker sharing is explicitly
//! out of scope, AXVERITY_LOOPSTATE_CURSOR_BUILD_V1).
//!
//! ## The buffer is `Vec<String>` chunks
//!
//! `cursor_append` pushes one chunk (O(1) amortized — no re-copy of prior
//! chunks). `cursor_get` materializes the whole buffer by concatenating the
//! chunks in append order (O(total), called ONCE at the end of the loop).
//! `cursor_len` returns the number of appended chunks. `cursor_close` frees the
//! buffer — required because a long-lived worker thread opens one cursor PER
//! query; without a free the thread-local map would grow unbounded.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::OnceLock;

use super::value::{get_str, intern_str, Value};

/// `groupby_cursor_enabled(_: Unit) -> Bool`
///
/// The ship-behind-a-flag selector (AXVERITY_LOOPSTATE_CURSOR_BUILD_V1, same
/// runtime-env discipline as `AXVERITY_QHM_VARIANT`): the GROUP BY row builder
/// (`pg_gb_rows`) dispatches to the cursor accumulator path when this is true,
/// and to the preserved string-state path otherwise. **Default OFF** — the flag
/// is only flipped to default-on after Chris reviews the measured results (the
/// qhm ship discipline). Env `AXVERITY_GROUPBY_CURSOR` ∈ {`1`,`on`,`true`} turns
/// it on; anything else (incl. unset) is off. `OnceLock`-cached so every call in
/// a query pays the env read at most once per process.
#[track_caller]
pub fn groupby_cursor_enabled(_arg: Value) -> Value {
    static ON: OnceLock<bool> = OnceLock::new();
    let on = *ON.get_or_init(|| {
        matches!(
            std::env::var("AXVERITY_GROUPBY_CURSOR").ok().as_deref(),
            Some("1") | Some("on") | Some("true") | Some("ON") | Some("TRUE")
        )
    });
    Value::Bool(on)
}

thread_local! {
    /// Per-thread cursor table, keyed by integer handle. THREAD-LOCAL, never
    /// shared — the append/get/len path touches only the calling thread's own
    /// map, so no two worker threads ever contend and no lock is taken.
    static CURS: RefCell<HashMap<i64, Vec<String>>> = RefCell::new(HashMap::new());

    /// Per-thread handle counter (first `cursor_open` returns 1, like logbuf).
    static NEXT: Cell<i64> = const { Cell::new(1) };
}

fn next_handle() -> i64 {
    NEXT.with(|c| {
        let n = c.get();
        c.set(n + 1);
        n
    })
}

fn arg_int(v: &Value, who: &str, i: usize) -> i64 {
    match v {
        Value::Int(n) => *n,
        other => panic!("{}: arg {} expected Int, got {:?}", who, i, other),
    }
}
fn arg_str(v: &Value, who: &str, i: usize) -> String {
    match v {
        Value::Str(h) => get_str(h),
        other => panic!("{}: arg {} expected Text, got {:?}", who, i, other),
    }
}

/// `cursor_open(label: Text) -> Int`
///
/// Allocate a fresh empty cursor in THIS thread's table and return its handle.
/// The `label` argument is accepted for call-site readability and provenance
/// (mirroring `walidx_open(shard)`); it is not otherwise used. Handles are never
/// reused within a thread.
#[track_caller]
pub fn cursor_open(arg: Value) -> Value {
    // Accept (and ignore) any label; only its presence matters for the ABI.
    let _label = match arg {
        Value::Str(h) => get_str(h),
        Value::Unit => String::new(),
        other => panic!("cursor_open: expected Text label (or Unit), got {:?}", other),
    };
    let h = next_handle();
    CURS.with(|c| {
        c.borrow_mut().insert(h, Vec::new());
    });
    Value::Int(h)
}

/// `cursor_append(h: Int, chunk: Text) -> Unit`
///
/// Push `chunk` onto the calling thread's cursor `h` — an O(1) amortized `Vec`
/// push, no re-copy of the prior contents, no syscall, no lock. This is the hot
/// per-step op that replaces `str_concat(accumulator, chunk)` in the loop state.
#[track_caller]
pub fn cursor_append(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 2 => es,
        other => panic!("cursor_append: expected Tuple(Int, Text), got {:?}", other),
    };
    let h = arg_int(&es[0], "cursor_append", 0);
    let chunk = arg_str(&es[1], "cursor_append", 1);
    CURS.with(|c| {
        let mut c = c.borrow_mut();
        let buf = c
            .get_mut(&h)
            .unwrap_or_else(|| panic!("cursor_append: unknown handle {} (not opened on this thread)", h));
        buf.push(chunk);
    });
    Value::Unit
}

/// `cursor_get(h: Int) -> Text`
///
/// Materialize the whole cursor `h`: concatenate its chunks in append order.
/// O(total length), called ONCE at the end of the accumulation loop. Does NOT
/// free the buffer (`cursor_close` does); a caller may `cursor_get` more than
/// once. An unknown handle yields "" (a closed/never-opened cursor reads empty)
/// rather than panicking, so a caller need not special-case the empty result.
#[track_caller]
pub fn cursor_get(arg: Value) -> Value {
    let h = arg_int(&arg, "cursor_get", 0);
    CURS.with(|c| {
        let c = c.borrow();
        let out = match c.get(&h) {
            Some(buf) => buf.concat(),
            None => String::new(),
        };
        Value::Str(intern_str(&out))
    })
}

/// `cursor_len(h: Int) -> Int`
///
/// Number of chunks appended to cursor `h` (i.e. the number of `cursor_append`
/// calls), or 0 for an unknown/closed handle. Note this is the CHUNK count, not
/// the byte length — `str_len(cursor_get(h))` gives the byte length.
#[track_caller]
pub fn cursor_len(arg: Value) -> Value {
    let h = arg_int(&arg, "cursor_len", 0);
    CURS.with(|c| {
        let c = c.borrow();
        let n = c.get(&h).map(|buf| buf.len() as i64).unwrap_or(0);
        Value::Int(n)
    })
}

/// `cursor_close(h: Int) -> Unit`
///
/// Free cursor `h` from the calling thread's table. Idempotent (closing an
/// already-closed/unknown handle is a no-op). REQUIRED for production use: a
/// long-lived pg_server worker opens one cursor per query, so without this the
/// thread-local map would grow unbounded across queries.
#[track_caller]
pub fn cursor_close(arg: Value) -> Value {
    let h = arg_int(&arg, "cursor_close", 0);
    CURS.with(|c| {
        c.borrow_mut().remove(&h);
    });
    Value::Unit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> Value {
        Value::Str(intern_str(x))
    }
    fn h(v: &Value) -> i64 {
        match v {
            Value::Int(n) => *n,
            _ => panic!("expected Int handle"),
        }
    }

    #[test]
    fn open_append_get_len_close() {
        let c = cursor_open(s("t"));
        let hd = h(&c);
        assert_eq!(cursor_len(Value::Int(hd)), Value::Int(0));
        cursor_append(Value::Tuple(vec![Value::Int(hd), s("a\n")]));
        cursor_append(Value::Tuple(vec![Value::Int(hd), s("bb\n")]));
        assert_eq!(cursor_len(Value::Int(hd)), Value::Int(2));
        match cursor_get(Value::Int(hd)) {
            Value::Str(sh) => assert_eq!(get_str(&sh), "a\nbb\n"),
            _ => panic!(),
        }
        // get is non-destructive
        match cursor_get(Value::Int(hd)) {
            Value::Str(sh) => assert_eq!(get_str(&sh), "a\nbb\n"),
            _ => panic!(),
        }
        cursor_close(Value::Int(hd));
        // after close: empty read, zero len, idempotent close
        match cursor_get(Value::Int(hd)) {
            Value::Str(sh) => assert_eq!(get_str(&sh), ""),
            _ => panic!(),
        }
        assert_eq!(cursor_len(Value::Int(hd)), Value::Int(0));
        cursor_close(Value::Int(hd));
    }

    #[test]
    fn handles_are_distinct_and_isolated() {
        let a = h(&cursor_open(s("a")));
        let b = h(&cursor_open(s("b")));
        assert_ne!(a, b);
        cursor_append(Value::Tuple(vec![Value::Int(a), s("A")]));
        cursor_append(Value::Tuple(vec![Value::Int(b), s("B")]));
        match cursor_get(Value::Int(a)) {
            Value::Str(sh) => assert_eq!(get_str(&sh), "A"),
            _ => panic!(),
        }
        match cursor_get(Value::Int(b)) {
            Value::Str(sh) => assert_eq!(get_str(&sh), "B"),
            _ => panic!(),
        }
        cursor_close(Value::Int(a));
        cursor_close(Value::Int(b));
    }

    #[test]
    fn cross_thread_handles_are_independent() {
        // Each thread has its own NEXT starting at 1 and its own table — a
        // handle from one thread is meaningless in another (thread-owned).
        let t = std::thread::spawn(|| h(&cursor_open(s("x"))));
        let other_first = t.join().unwrap();
        let this_first = h(&cursor_open(s("y")));
        assert_eq!(other_first, 1);
        assert_eq!(this_first, 1); // independent counters
        cursor_close(Value::Int(this_first));
    }
}

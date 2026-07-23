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

// agg_cursor_enabled (the AXVERITY_AGG_CURSOR dial) was DELETED in
// AXVERITY_WAY_BACK_CONSOLIDATION_V1 — §30 measured the input-consuming aggregate cursor
// gives no wire-path memory bound and no query-level win, so agg_eval always uses the string
// path (agg_eval_cur/agg_eval_cur_step deleted too). cursor_load/cursor_line/cursor_sort below
// stay — GROUP BY (the shipped winner) still uses them.

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

/// `groupby_cursor_mode(_: Unit) -> Text`
///
/// The finer-grained selector added by AXVERITY_READPATH_CLOSEOUT_V1 (Part 1),
/// superseding the boolean `groupby_cursor_enabled` for the GROUP BY row builder
/// so the two independent loop-state levers — the input-consuming cursor and the
/// native `cursor_sort` — can be enabled SEPARATELY and measured incrementally
/// against the real query path (the direct lesson of the first cursor attempt,
/// which shipped a zero-win output-only accumulator). Env `AXVERITY_GROUPBY_CURSOR`:
///   - unset / `0` / `off` / `false`        → `"off"`  (string-state + selection
///                                                       sort — the shipped default,
///                                                       byte-identical fallback)
///   - `1` / `on` / `true`                  → `"out"`  (the §26 append-only OUTPUT
///                                                       cursor — kept for continuity;
///                                                       measured no-win)
///   - `in`                                 → `"in"`   (input-consuming cursor for
///                                                       keyed + fold; selection sort
///                                                       still — lever 1 alone)
///   - `sort`                               → `"sort"` (cursor_sort replaces the
///                                                       selection sort; string keyed
///                                                       + fold — lever 2 alone)
///   - `full`                               → `"full"` (both levers)
///   - anything else                        → `"off"`  (safe default)
///
/// `OnceLock`-cached: a query pays the env read at most once per process (each
/// measurement launches a fresh server with the env pinned, so per-process
/// caching is exactly right). **Default OFF** — the flag is only flipped to
/// default-on after Chris reviews the measured results (the qhm ship discipline).
#[track_caller]
pub fn groupby_cursor_mode(_arg: Value) -> Value {
    static MODE: OnceLock<&'static str> = OnceLock::new();
    // AXVERITY_WAY_BACK_CONSOLIDATION_V1: the GROUPBY_CURSOR switch is removed. "full" won
    // decisively (§28/§30: cursor_sort is the GROUP BY lever, 25–122×; input-cursor + sort
    // combined) and is byte-identical to the string path; off/out/in/sort had no advantage over
    // full. Always "full". AXVERITY_GROUPBY_CURSOR is no longer read. (The now-unreachable
    // internal off/out entry points are deleted; the deeper pg_gb_rows_fast in/sort dead-branch
    // collapse is staged.)
    let m = *MODE.get_or_init(|| "full");
    Value::Str(intern_str(m))
}

/// `cursor_load(text: Text) -> Int`
///
/// Allocate a fresh cursor on THIS thread and populate it by splitting `text`
/// into LF-terminated lines (the trailing `\n` of the last line is a terminator,
/// not a separator, so an LF-terminated input yields exactly one chunk per line
/// with NO empty trailing chunk; an empty input yields an empty cursor). This is
/// the INPUT-CONSUMING half of the cursor primitive (AXVERITY_READPATH_CLOSEOUT_V1
/// Part 1): a loop that would otherwise `str_before/str_after` the shrinking
/// O(M) remaining-input `Text` every step (→ O(M²)) instead loads it ONCE here
/// and reads line `i` in O(1) via `cursor_line`, carrying only the small integer
/// index + handle in the loop state. Splitting matches the M1
/// `line = str_before(rem, LF); rem = str_after(rem, LF)` idiom exactly (each
/// stored chunk is the line WITHOUT its `\n`), so a cursor walk is byte-identical
/// to the threaded walk.
#[track_caller]
pub fn cursor_load(arg: Value) -> Value {
    let text = arg_str(&arg, "cursor_load", 0);
    let lines: Vec<String> = text.split_terminator('\n').map(|l| l.to_string()).collect();
    let h = next_handle();
    CURS.with(|c| {
        c.borrow_mut().insert(h, lines);
    });
    Value::Int(h)
}

/// `cursor_line(h: Int, i: Int) -> Text`
///
/// Return the `i`-th line (0-indexed) of a `cursor_load`ed cursor `h`, WITHOUT
/// its trailing `\n` (matching `str_before(rem, LF)`). O(1) `Vec` index — this is
/// what turns the input walk from O(M²) into O(M). An out-of-range index or an
/// unknown/closed handle yields "" (so a walk that overruns reads empty rather
/// than panicking).
#[track_caller]
pub fn cursor_line(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 2 => es,
        other => panic!("cursor_line: expected Tuple(Int, Int), got {:?}", other),
    };
    let h = arg_int(&es[0], "cursor_line", 0);
    let i = arg_int(&es[1], "cursor_line", 1);
    CURS.with(|c| {
        let c = c.borrow();
        let out = match c.get(&h) {
            Some(buf) if i >= 0 && (i as usize) < buf.len() => buf[i as usize].clone(),
            _ => String::new(),
        };
        Value::Str(intern_str(&out))
    })
}

/// `cursor_sort(keyed: Text, dir: Text) -> Text`
///
/// A native, STABLE sort that is a byte-identical drop-in for the M1 selection
/// sort `pg_sort` (AXVERITY_READPATH_CLOSEOUT_V1 Part 1, lever 2). Input is an
/// LF-terminated list of `<key>\t<payload>` lines; the sort key is the substring
/// before the FIRST `\t` (the whole line if there is none), compared by Rust
/// `str` Ord = UTF-8 byte order — EXACTLY `text_lt` (`str_ops.rs`), which is what
/// `pg_ext_step` uses. `dir` "0" = ascending (min first), "1" = descending (max
/// first). Output is the sorted full lines, each re-terminated with `\n` (empty
/// input → ""), matching `pg_sort` byte-for-byte.
///
/// TIE-BREAK: `sort_by` is stable, so equal keys keep their INPUT order — exactly
/// `pg_ext_step`'s rule (a strict `text_lt` keeps the first-encountered `best` on
/// an equal key, so equal keys are extracted in input order, ascending AND
/// descending). This also RETIRES the `pg_sort` selection-sort bug class outright
/// (`gap:axverity-pgsort-desc-nonterminating-hang`): there is no string line
/// removal here — the whole `str_replace`-boundary hazard cannot recur.
///
/// Genuinely pure (no thread-local, no env, no I/O) → declared `effect pure`,
/// so the compiler may legitimately CSE two identical calls (harmless: same
/// input → same sorted output).
#[track_caller]
pub fn cursor_sort(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 2 => es,
        other => panic!("cursor_sort: expected Tuple(Text, Text), got {:?}", other),
    };
    let keyed = arg_str(&es[0], "cursor_sort", 0);
    let dir = arg_str(&es[1], "cursor_sort", 1);
    let mut lines: Vec<&str> = keyed.split_terminator('\n').collect();
    // Key = text before the first TAB (whole line if none), compared bytewise.
    fn key(l: &str) -> &str {
        l.split_once('\t').map(|(k, _)| k).unwrap_or(l)
    }
    if dir == "1" {
        lines.sort_by(|a, b| key(b).cmp(key(a))); // DESC, stable → ties keep input order
    } else {
        lines.sort_by(|a, b| key(a).cmp(key(b))); // ASC, stable → ties keep input order
    }
    let mut out = String::with_capacity(keyed.len());
    for l in lines {
        out.push_str(l);
        out.push('\n');
    }
    Value::Str(intern_str(&out))
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

    fn gs(v: Value) -> String {
        match v {
            Value::Str(sh) => get_str(&sh),
            _ => panic!("expected Str"),
        }
    }

    #[test]
    fn load_line_count_close() {
        // LF-terminated: trailing \n is a terminator, not a separator.
        let hd = h(&cursor_load(s("a\nbb\nccc\n")));
        assert_eq!(cursor_len(Value::Int(hd)), Value::Int(3));
        assert_eq!(gs(cursor_line(Value::Tuple(vec![Value::Int(hd), Value::Int(0)]))), "a");
        assert_eq!(gs(cursor_line(Value::Tuple(vec![Value::Int(hd), Value::Int(1)]))), "bb");
        assert_eq!(gs(cursor_line(Value::Tuple(vec![Value::Int(hd), Value::Int(2)]))), "ccc");
        // out of range and unknown handle read empty (no panic)
        assert_eq!(gs(cursor_line(Value::Tuple(vec![Value::Int(hd), Value::Int(3)]))), "");
        assert_eq!(gs(cursor_line(Value::Tuple(vec![Value::Int(hd), Value::Int(-1)]))), "");
        assert_eq!(gs(cursor_line(Value::Tuple(vec![Value::Int(99999), Value::Int(0)]))), "");
        cursor_close(Value::Int(hd));
    }

    #[test]
    fn load_empty_and_no_trailing_newline() {
        let e = h(&cursor_load(s("")));
        assert_eq!(cursor_len(Value::Int(e)), Value::Int(0));
        cursor_close(Value::Int(e));
        let nt = h(&cursor_load(s("x\ny"))); // no trailing \n
        assert_eq!(cursor_len(Value::Int(nt)), Value::Int(2));
        assert_eq!(gs(cursor_line(Value::Tuple(vec![Value::Int(nt), Value::Int(1)]))), "y");
        cursor_close(Value::Int(nt));
    }

    fn sort(keyed: &str, dir: &str) -> String {
        gs(cursor_sort(Value::Tuple(vec![s(keyed), s(dir)])))
    }

    #[test]
    fn sort_matches_pg_sort_semantics() {
        // ASC / DESC on a simple keyed list "<key>\t<hash>".
        assert_eq!(sort("b\th2\na\th1\nc\th3\n", "0"), "a\th1\nb\th2\nc\th3\n");
        assert_eq!(sort("b\th2\na\th1\nc\th3\n", "1"), "c\th3\nb\th2\na\th1\n");
        // Empty in → empty out (pg_sort returns "" for an empty working set).
        assert_eq!(sort("", "0"), "");
        assert_eq!(sort("", "1"), "");
        // Single line.
        assert_eq!(sort("k\tv\n", "0"), "k\tv\n");
    }

    #[test]
    fn sort_ties_preserve_input_order_both_directions() {
        // Equal keys keep INPUT order (stable), ASC and DESC — matching
        // pg_ext_step's first-encountered-wins tie rule.
        assert_eq!(
            sort("a\tx\nb\ty\na\tz\n", "0"),
            "a\tx\na\tz\nb\ty\n"
        );
        // DESC: b group first, then the two a-key lines still in input order.
        assert_eq!(
            sort("a\tx\nb\ty\na\tz\n", "1"),
            "b\ty\na\tx\na\tz\n"
        );
    }

    #[test]
    fn sort_bytewise_and_suffix_colliding_keys() {
        // Non-zero-padded / suffix-colliding group values ("1","19","9","2") —
        // the exact class that broke pg_sort's str_replace removal. Native sort
        // orders them bytewise (COLLATE "C"): "1" < "19" < "2" < "9".
        assert_eq!(
            sort("9\ta\n1\tb\n19\tc\n2\td\n", "0"),
            "1\tb\n19\tc\n2\td\n9\ta\n"
        );
        assert_eq!(
            sort("9\ta\n1\tb\n19\tc\n2\td\n", "1"),
            "9\ta\n2\td\n19\tc\n1\tb\n"
        );
    }
}

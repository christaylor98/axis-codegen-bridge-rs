//! BRIDGE_NAMEPTR_V1 (AXVERITY_INSERT_PATH_FASTPATH, Landing 2) — realizes
//! intent:axverity-req-immutable-pointer (the double-buffered toggle cell)
//! for the name-binding "current" pointer intent:axverity-req-name-gitref
//! describes: "a volatile double-buffered toggle head-pointer... rebuilt on
//! recovery from the WAL last-write-per-name."
//!
//! ## The cell, exactly as specified
//!
//! Fixed two slots plus a valid indicator. A writer fills the IDLE slot (the
//! one the indicator does NOT currently point at), then atomically flips the
//! indicator. A reader reads the indicator, then that slot. Because the
//! indicator only ever points at a slot holding a COMPLETE, previously
//! published value, a reader can never observe a torn/partial write — the
//! slot it lands on was fully written before the flip that made it current.
//!
//! ## Why thread-local (NO_SHARED_REGISTRY), and what that costs
//!
//! This cell is instantiated PER NAME PER THREAD (thread-local, keyed by
//! name-slug) — the same storage model as `logbuf.rs`/`walindex.rs`/
//! `fieldidx.rs`: no `Mutex`/`Arc`/process-global registry anywhere. Because
//! only the owning thread ever touches its own cell, the double-buffer's
//! concurrency-safety property is trivially satisfied here (there is only
//! ever one accessor) — the mechanism is realized faithfully per the pinned
//! spec, deployed in the safe topology NO_SHARED_REGISTRY demands, ready to
//! generalize to a genuinely cross-thread pointer later without a redesign.
//!
//! Consequence: a read on a DIFFERENT thread than the writer never sees this
//! cell (thread-local) and must fall back to the durable log — `resolve_name`
//! (lib/resolve_name.m1) checks this cell FIRST as a same-thread fast path,
//! then falls back to `fs_read_last_line` over the name's append-only `.log`
//! (still durably fsync'd via `logbuf_open/append/sync` in
//! lib/bind_record.m1 — the "rebuilt on recovery from the WAL last-write-
//! per-name" half of req-name-gitref). This cell is a pure, disposable
//! optimization: never the only place a value lives.

use std::cell::RefCell;
use std::collections::HashMap;

use std::sync::Arc;

use super::value::Value;

/// Slots hold `Arc<str>` rather than `String`
/// (AXVERITY_BRIDGE_GLUE_OPT_SWEEP_V1). A `Value::Str` already IS an
/// `Arc<str>` (`value.rs:152-154`), so filling a slot from a caller's Text and
/// handing that Text back out are both refcount bumps — where a `String` slot
/// had to copy the bytes in and copy them out again. The flip below is
/// unchanged in kind: a slot store followed by an indicator store, on one
/// thread with no yield between them.
struct ToggleCell {
    slots: [Arc<str>; 2],
    current: usize, // which slot the indicator points at (0 or 1)
}

thread_local! {
    static CELLS: RefCell<HashMap<String, ToggleCell>> = RefCell::new(HashMap::new());

    /// The empty Text, minted once per thread — see `nameptr_get`, which
    /// returns it for a slug this thread never set.
    static EMPTY: Arc<str> = Arc::from("");
}

/// `nameptr_set(slug: Text, line: Text) -> Unit` — fill the idle slot, then
/// flip the indicator to it.
///
/// The steady-state path (a slug whose cell already exists) allocates nothing:
/// the slug is borrowed for the lookup and the line is a refcount bump. Only
/// the first set for a slug mints a `String`, and that `String` is the map key
/// it keeps.
#[track_caller]
pub fn nameptr_set(slug: std::sync::Arc<str>, line: std::sync::Arc<str>) -> Value {
    CELLS.with(|c| {
        let mut c = c.borrow_mut();
        match c.get_mut(&*slug) {
            Some(cell) => {
                let idle = 1 - cell.current;
                cell.slots[idle] = line; // fill idle slot
                cell.current = idle; // atomically (single-threaded, no yield) flip
            }
            None => {
                let empty = EMPTY.with(|e| e.clone());
                c.insert(
                    slug.to_string(),
                    ToggleCell { slots: [line, empty], current: 0 },
                );
            }
        }
    });
    Value::Unit
}

/// `nameptr_get(slug: Text) -> Text` — read the indicator, then that slot.
/// Returns "" if this thread never set a cell for `slug`.
///
/// Allocation-free on both paths.
#[track_caller]
pub fn nameptr_get(slug: std::sync::Arc<str>) -> Value {
    CELLS.with(|c| {
        let c = c.borrow();
        match c.get(&*slug) {
            Some(cell) => Value::Str(cell.slots[cell.current].clone()),
            None => EMPTY.with(|e| Value::Str(e.clone())),
        }
    })
}

#[cfg(test)]
mod bench {
    //! AXVERITY_BRIDGE_GLUE_OPT_SWEEP_V1 / phase 1. `#[ignore]`d; run as
    //!
    //!   CARGO_TARGET_DIR=<isolated> cargo test --release --lib nameptr -- \
    //!       --ignored --nocapture --test-threads=1
    //!
    //! Add --features allocprobe for allocation counts. Same shape as the
    //! u32v.rs and scratch.rs benches.
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    fn alloc_probe<F: FnOnce()>(body: F) -> Option<(i64, i64)> {
        let (b0, _, c0, _) = crate::runtime::allocprobe::snapshot();
        body();
        let (b1, _, c1, _) = crate::runtime::allocprobe::snapshot();
        if crate::runtime::allocprobe::ENABLED {
            Some((c1 - c0, b1 - b0))
        } else {
            None
        }
    }

    fn report_allocs(label: &str, n: i64, probe: Option<(i64, i64)>) {
        match probe {
            Some((count, bytes)) => println!(
                "{}: allocprobe ENABLED  allocs={} ({:.4}/call)  bytes={} ({:.2}/call)",
                label,
                count,
                count as f64 / n as f64,
                bytes,
                bytes as f64 / n as f64
            ),
            None => println!(
                "{}: allocprobe DISABLED — no allocation counts, wall-clock only",
                label
            ),
        }
    }

    #[test]
    #[ignore]
    fn bench_nameptr_1m() {
        const N: i64 = 1_000_000;
        let slug: Arc<str> = Arc::from("bind:some/name");
        let absent: Arc<str> = Arc::from("bind:never/set");
        let line: Arc<str> = Arc::from("sha256:0123456789abcdef0123456789abcdef");

        // SET, steady state — the cell exists, so this is the flip path.
        nameptr_set(slug.clone(), line.clone());
        let t0 = Instant::now();
        for _ in 0..N {
            nameptr_set(slug.clone(), line.clone());
        }
        let set_t = t0.elapsed();

        // GET hit.
        let t1 = Instant::now();
        let mut hits = 0i64;
        for _ in 0..N {
            if let Value::Str(s) = nameptr_get(slug.clone()) {
                if !s.is_empty() {
                    hits += 1;
                }
            }
        }
        let get_hit = t1.elapsed();
        assert_eq!(hits, N, "keep the loop from being optimised away");

        // GET miss — a slug this thread never set.
        let t2 = Instant::now();
        let mut misses = 0i64;
        for _ in 0..N {
            if let Value::Str(s) = nameptr_get(absent.clone()) {
                if s.is_empty() {
                    misses += 1;
                }
            }
        }
        let get_miss = t2.elapsed();
        assert_eq!(misses, N, "keep the loop from being optimised away");

        println!(
            "P1n nameptr: n={} set_ns_per_call={:.2} get_hit_ns_per_call={:.2} \
             get_miss_ns_per_call={:.2}",
            N,
            set_t.as_nanos() as f64 / N as f64,
            get_hit.as_nanos() as f64 / N as f64,
            get_miss.as_nanos() as f64 / N as f64
        );

        let p_set = alloc_probe(|| {
            for _ in 0..N {
                nameptr_set(slug.clone(), line.clone());
            }
        });
        report_allocs("P1n nameptr_set", N, p_set);

        let p_get = alloc_probe(|| {
            for _ in 0..N {
                nameptr_get(slug.clone());
            }
        });
        report_allocs("P1n nameptr_get hit", N, p_get);

        let p_miss = alloc_probe(|| {
            for _ in 0..N {
                nameptr_get(absent.clone());
            }
        });
        report_allocs("P1n nameptr_get miss", N, p_miss);
    }
}

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

use super::value::{get_str, intern_str, Value};

struct ToggleCell {
    slots: [String; 2],
    current: usize, // which slot the indicator points at (0 or 1)
}

thread_local! {
    static CELLS: RefCell<HashMap<String, ToggleCell>> = RefCell::new(HashMap::new());
}

fn arg_str(v: &Value, who: &str, i: usize) -> String {
    match v {
        Value::Str(h) => get_str(h),
        other => panic!("{}: arg {} expected Text, got {:?}", who, i, other),
    }
}

/// `nameptr_set(slug: Text, line: Text) -> Unit` — fill the idle slot, then
/// flip the indicator to it.
#[track_caller]
pub fn nameptr_set(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 2 => es,
        other => panic!("nameptr_set: expected Tuple(Text, Text), got {:?}", other),
    };
    let slug = arg_str(&es[0], "nameptr_set", 0);
    let line = arg_str(&es[1], "nameptr_set", 1);
    CELLS.with(|c| {
        let mut c = c.borrow_mut();
        match c.get_mut(&slug) {
            Some(cell) => {
                let idle = 1 - cell.current;
                cell.slots[idle] = line; // fill idle slot
                cell.current = idle; // atomically (single-threaded, no yield) flip
            }
            None => {
                c.insert(
                    slug,
                    ToggleCell { slots: [line, String::new()], current: 0 },
                );
            }
        }
    });
    Value::Unit
}

/// `nameptr_get(slug: Text) -> Text` — read the indicator, then that slot.
/// Returns "" if this thread never set a cell for `slug`.
#[track_caller]
pub fn nameptr_get(arg: Value) -> Value {
    let slug = match arg {
        Value::Str(h) => get_str(h),
        other => panic!("nameptr_get: expected Text slug, got {:?}", other),
    };
    CELLS.with(|c| {
        let c = c.borrow();
        let out = match c.get(&slug) {
            Some(cell) => cell.slots[cell.current].clone(),
            None => String::new(),
        };
        Value::Str(intern_str(&out))
    })
}

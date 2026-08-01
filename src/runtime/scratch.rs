//! BRIDGE_STRCMP_SETMAP_V1 — NAMED SCRATCH SETS AND MAPS: process-scoped,
//! thread-local, Text-keyed collections for M1's no-capture loops. NEVER
//! durable, never shared across threads, cleared by their owner.
//!
//! ## Why this module exists (the fold precedent)
//!
//! M1's only data structure is Text, so membership and dedup are
//! `str_contains` over LF-wrapped key lists — O(n) per probe, O(n²) per
//! scan. That is the `loop_while` accumulator disease (AXSEM_W2_ACCUM_
//! PERF_V1) in a new costume: `fold` fixed O(n²) STATE-CLONING by letting
//! Rust own the iteration; this module fixes O(n²) MEMBERSHIP by letting
//! Rust own the lookup structure. The workload that proved the need:
//! axSemantica-working2's depth traversal, where visited/seen scans
//! dominate an 81s full-component answer.
//!
//! ## Design — names, not handles
//!
//! Collections are keyed by a caller-chosen Text NAME (`set_add("bfs:vis",
//! k)`), not by allocated handles: a name is a constant an M1 no-capture
//! step can carry in its accumulator or its body, there is nothing to leak
//! or free, and re-running a query is deterministic because the owner
//! clears its names up front. Same storage model as adjacency.rs /
//! logbuf.rs: thread-local, no Mutex, no process-global registry.
//!
//! ## Semantics
//!
//! * `set_add` returns whether the key was NEWLY added — membership test
//!   and insertion in one call, which is exactly the shape dedup-in-fold
//!   needs.
//! * `map_get` returns "" for an absent key (the store convention:
//!   absence is empty, and the caller decides what empty means).
//! * `*_clear` clears ONE name; names are otherwise independent.
//! * Keys and values are Text. Nothing here touches the filesystem.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use super::value::{get_str, intern_str, Value};

thread_local! {
    static SETS: RefCell<HashMap<String, HashSet<String>>> = RefCell::new(HashMap::new());
    static MAPS: RefCell<HashMap<String, HashMap<String, String>>> = RefCell::new(HashMap::new());
}

fn two(args: &Value, who: &str) -> (String, String) {
    match args {
        Value::Tuple(es) if es.len() == 2 => {
            let a = match &es[0] {
                Value::Str(h) => get_str(h),
                other => panic!("{}: arg 0 expected Text, got {:?}", who, other),
            };
            let b = match &es[1] {
                Value::Str(h) => get_str(h),
                other => panic!("{}: arg 1 expected Text, got {:?}", who, other),
            };
            (a, b)
        }
        other => panic!("{}: expected Tuple(Text, Text), got {:?}", who, other),
    }
}

fn one(arg: &Value, who: &str) -> String {
    match arg {
        Value::Str(h) => get_str(h),
        other => panic!("{}: expected Text, got {:?}", who, other),
    }
}

/// `set_add(name, key) -> Bool` — insert; true iff the key was newly added.
#[track_caller]
pub fn set_add(args: Value) -> Value {
    let (name, key) = two(&args, "set_add");
    SETS.with(|s| Value::Bool(s.borrow_mut().entry(name).or_default().insert(key)))
}

/// `set_has(name, key) -> Bool` — membership, no mutation.
#[track_caller]
pub fn set_has(args: Value) -> Value {
    let (name, key) = two(&args, "set_has");
    SETS.with(|s| {
        Value::Bool(
            s.borrow()
                .get(&name)
                .map(|set| set.contains(&key))
                .unwrap_or(false),
        )
    })
}

/// `set_len(name) -> Int` — cardinality; 0 for an unknown name.
#[track_caller]
pub fn set_len(arg: Value) -> Value {
    let name = one(&arg, "set_len");
    SETS.with(|s| Value::Int(s.borrow().get(&name).map(|set| set.len() as i64).unwrap_or(0)))
}

/// `set_clear(name) -> Unit` — drop one named set entirely.
#[track_caller]
pub fn set_clear(arg: Value) -> Value {
    let name = one(&arg, "set_clear");
    SETS.with(|s| {
        s.borrow_mut().remove(&name);
    });
    Value::Unit
}

/// `map_put(name, key, value) -> Bool` — insert/overwrite; true iff the key
/// was newly added (false = overwrite of an existing key).
#[track_caller]
pub fn map_put(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 3 => es,
        other => panic!("map_put: expected Tuple(Text, Text, Text), got {:?}", other),
    };
    let name = one(&es[0], "map_put");
    let key = one(&es[1], "map_put");
    let val = one(&es[2], "map_put");
    MAPS.with(|m| {
        Value::Bool(
            m.borrow_mut()
                .entry(name)
                .or_default()
                .insert(key, val)
                .is_none(),
        )
    })
}

/// `map_get(name, key) -> Text` — the value, or "" when absent.
#[track_caller]
pub fn map_get(args: Value) -> Value {
    let (name, key) = two(&args, "map_get");
    MAPS.with(|m| {
        let out = m
            .borrow()
            .get(&name)
            .and_then(|map| map.get(&key).cloned())
            .unwrap_or_default();
        Value::Str(intern_str(&out))
    })
}

/// `map_len(name) -> Int` — entry count; 0 for an unknown name.
#[track_caller]
pub fn map_len(arg: Value) -> Value {
    let name = one(&arg, "map_len");
    MAPS.with(|m| Value::Int(m.borrow().get(&name).map(|map| map.len() as i64).unwrap_or(0)))
}

/// `map_clear(name) -> Unit` — drop one named map entirely.
#[track_caller]
pub fn map_clear(arg: Value) -> Value {
    let name = one(&arg, "map_clear");
    MAPS.with(|m| {
        m.borrow_mut().remove(&name);
    });
    Value::Unit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Value {
        Value::Str(intern_str(s))
    }

    #[test]
    fn set_add_reports_newness_and_names_are_independent() {
        set_clear(t("a"));
        set_clear(t("b"));
        assert_eq!(set_add(Value::Tuple(vec![t("a"), t("k")])), Value::Bool(true));
        assert_eq!(set_add(Value::Tuple(vec![t("a"), t("k")])), Value::Bool(false));
        assert_eq!(set_has(Value::Tuple(vec![t("b"), t("k")])), Value::Bool(false));
        assert_eq!(set_len(t("a")), Value::Int(1));
    }

    #[test]
    fn clear_scopes_to_one_name() {
        set_clear(t("x"));
        set_clear(t("y"));
        set_add(Value::Tuple(vec![t("x"), t("1")]));
        set_add(Value::Tuple(vec![t("y"), t("1")]));
        set_clear(t("x"));
        assert_eq!(set_len(t("x")), Value::Int(0));
        assert_eq!(set_len(t("y")), Value::Int(1));
    }

    #[test]
    fn map_absent_is_empty_and_put_reports_newness() {
        map_clear(t("m"));
        assert_eq!(map_get(Value::Tuple(vec![t("m"), t("k")])), t(""));
        assert_eq!(map_put(Value::Tuple(vec![t("m"), t("k"), t("v1")])), Value::Bool(true));
        assert_eq!(map_put(Value::Tuple(vec![t("m"), t("k"), t("v2")])), Value::Bool(false));
        assert_eq!(map_get(Value::Tuple(vec![t("m"), t("k")])), t("v2"));
    }
}

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

use super::value::{intern_str, Value};

thread_local! {
    static SETS: RefCell<HashMap<String, HashSet<String>>> = RefCell::new(HashMap::new());
    static MAPS: RefCell<HashMap<String, HashMap<String, String>>> = RefCell::new(HashMap::new());
}

/// `set_add(name, key) -> Bool` — insert; true iff the key was newly added.
#[track_caller]
pub fn set_add(name: std::sync::Arc<str>, key: std::sync::Arc<str>) -> Value {
    let (name, key) = (name.to_string(), key.to_string());
    SETS.with(|s| Value::Bool(s.borrow_mut().entry(name).or_default().insert(key)))
}

/// `set_has(name, key) -> Bool` — membership, no mutation.
#[track_caller]
pub fn set_has(name: std::sync::Arc<str>, key: std::sync::Arc<str>) -> Value {
    let (name, key) = (name.to_string(), key.to_string());
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
pub fn set_len(name: std::sync::Arc<str>) -> Value {
    let name = name.to_string();
    SETS.with(|s| Value::Int(s.borrow().get(&name).map(|set| set.len() as i64).unwrap_or(0)))
}

/// `set_clear(name) -> Unit` — drop one named set entirely.
#[track_caller]
pub fn set_clear(name: std::sync::Arc<str>) -> Value {
    let name = name.to_string();
    SETS.with(|s| {
        s.borrow_mut().remove(&name);
    });
    Value::Unit
}

/// `map_put(name, key, value) -> Bool` — insert/overwrite; true iff the key
/// was newly added (false = overwrite of an existing key).
#[track_caller]
pub fn map_put(name: std::sync::Arc<str>, key: std::sync::Arc<str>, val: std::sync::Arc<str>) -> Value {
    let (name, key, val) = (name.to_string(), key.to_string(), val.to_string());
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
pub fn map_get(name: std::sync::Arc<str>, key: std::sync::Arc<str>) -> Value {
    let (name, key) = (name.to_string(), key.to_string());
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
pub fn map_len(name: std::sync::Arc<str>) -> Value {
    let name = name.to_string();
    MAPS.with(|m| Value::Int(m.borrow().get(&name).map(|map| map.len() as i64).unwrap_or(0)))
}

/// `map_clear(name) -> Unit` — drop one named map entirely.
#[track_caller]
pub fn map_clear(name: std::sync::Arc<str>) -> Value {
    let name = name.to_string();
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
        set_clear(intern_str("a"));
        set_clear(intern_str("b"));
        assert_eq!(set_add(intern_str("a"), intern_str("k")), Value::Bool(true));
        assert_eq!(set_add(intern_str("a"), intern_str("k")), Value::Bool(false));
        assert_eq!(set_has(intern_str("b"), intern_str("k")), Value::Bool(false));
        assert_eq!(set_len(intern_str("a")), Value::Int(1));
    }

    #[test]
    fn clear_scopes_to_one_name() {
        set_clear(intern_str("x"));
        set_clear(intern_str("y"));
        set_add(intern_str("x"), intern_str("1"));
        set_add(intern_str("y"), intern_str("1"));
        set_clear(intern_str("x"));
        assert_eq!(set_len(intern_str("x")), Value::Int(0));
        assert_eq!(set_len(intern_str("y")), Value::Int(1));
    }

    #[test]
    fn map_absent_is_empty_and_put_reports_newness() {
        map_clear(intern_str("m"));
        assert_eq!(map_get(intern_str("m"), intern_str("k")), t(""));
        assert_eq!(map_put(intern_str("m"), intern_str("k"), intern_str("v1")), Value::Bool(true));
        assert_eq!(map_put(intern_str("m"), intern_str("k"), intern_str("v2")), Value::Bool(false));
        assert_eq!(map_get(intern_str("m"), intern_str("k")), t("v2"));
    }
}

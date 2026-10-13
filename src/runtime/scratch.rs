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
//!
//! ## Lookup borrows; only STORED text is allocated
//!
//! AXVERITY_BRIDGE_GLUE_OPT_SWEEP_V1 removed the per-call allocations that
//! were minted and discarded inside a call. Every entry point below indexes
//! its collection with `&*name` / `&*key` (an `&str` reborrow of the caller's
//! `Arc<str>`), relying on `String: Borrow<str>`, so indexing costs nothing.
//! A `String` is now allocated in exactly one place: a key that is genuinely
//! being STORED for the first time.
//!
//! This is the same discipline `u32v.rs` documents, arrived at from the same
//! measurement. That module's header used to hold up this one as the
//! counter-example that allocates per call; it no longer does, and the two
//! files now describe a single shared pattern rather than a divergence.
//!
//! `set_clear` and `map_clear` are deliberately UNCHANGED and still allocate.
//! They index via `HashMap::remove`, for which no measurement exists; see the
//! note on each. (Line numbers are deliberately omitted here — the previous
//! citation of these two sites went stale the moment this comment was added.)
//!
//! ## Map values are `Arc<str>`, not `String`
//!
//! `MAPS` stores `Arc<str>` because that is what a `Value::Str` already is
//! (`value.rs:152-154` — `intern_str` is `Arc::from`, and despite the name
//! there is no interning). Storing the caller's `Arc` is a refcount bump, and
//! returning it from `map_get` is another, so a put/get round trip allocates
//! NOTHING. The previous `String` store had to copy the text in on the way
//! past and mint a fresh `Arc` on the way out — two copies of a value that was
//! already in the right representation when it arrived.
//!
//! Sharing the buffer rather than copying it is unobservable: `Arc<str>` is
//! immutable, and no `Arc::get_mut`/`make_mut` exists anywhere in this crate,
//! so no holder can mutate what another holder sees. `Arc<str>` is `Send +
//! Sync`, so `VALUE_MUST_STAY_SEND_SYNC` (`value.rs:38-41`) is preserved —
//! though nothing here crosses a thread anyway, the store being thread-local.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::value::Value;

thread_local! {
    static SETS: RefCell<HashMap<String, HashSet<String>>> = RefCell::new(HashMap::new());
    static MAPS: RefCell<HashMap<String, HashMap<String, Arc<str>>>> = RefCell::new(HashMap::new());

    /// The empty Text, minted once per thread.
    ///
    /// `map_get` on an absent key is not an error path — absence-is-empty IS
    /// the store convention, so a miss is as hot as a hit. Handing back a
    /// clone of one cached `Arc` keeps a miss at zero allocations; the
    /// obvious `intern_str("")` would allocate an `Arc` header per call to
    /// represent nothing.
    static EMPTY: Arc<str> = Arc::from("");
}

/// A `Value::Str` holding the empty Text, without allocating.
fn empty_text() -> Value {
    EMPTY.with(|e| Value::Str(e.clone()))
}

/// `set_add(name, key) -> Bool` — insert; true iff the key was newly added.
///
/// `contains` then `insert`, rather than `insert` alone: `HashSet::insert`
/// demands an owned `String` on EVERY call, new key or not, so the one-hash
/// form cannot avoid the allocation. This trades a second hash on the
/// newly-added path for zero allocations on the already-present path — which
/// is the path dedup-in-fold actually runs, since a dedup that keeps finding
/// new keys is not deduplicating anything.
#[track_caller]
pub fn set_add(name: std::sync::Arc<str>, key: std::sync::Arc<str>) -> Value {
    SETS.with(|s| {
        let mut store = s.borrow_mut();
        match store.get_mut(&*name) {
            Some(set) => {
                if set.contains(&*key) {
                    Value::Bool(false)
                } else {
                    set.insert(key.to_string());
                    Value::Bool(true)
                }
            }
            None => {
                let mut set = HashSet::new();
                set.insert(key.to_string());
                store.insert(name.to_string(), set);
                Value::Bool(true)
            }
        }
    })
}

/// `set_has(name, key) -> Bool` — membership, no mutation.
///
/// Stores nothing, so allocates nothing.
#[track_caller]
pub fn set_has(name: std::sync::Arc<str>, key: std::sync::Arc<str>) -> Value {
    SETS.with(|s| {
        Value::Bool(
            s.borrow()
                .get(&*name)
                .map(|set| set.contains(&*key))
                .unwrap_or(false),
        )
    })
}

/// `set_len(name) -> Int` — cardinality; 0 for an unknown name.
#[track_caller]
pub fn set_len(name: std::sync::Arc<str>) -> Value {
    SETS.with(|s| {
        Value::Int(
            s.borrow()
                .get(&*name)
                .map(|set| set.len() as i64)
                .unwrap_or(0),
        )
    })
}

/// `set_clear(name) -> Unit` — drop one named set entirely.
///
/// DELIBERATELY UNALTERED by AXVERITY_BRIDGE_GLUE_OPT_SWEEP_V1, and still
/// allocates its name. This is one of the two `HashMap::remove` sites; the
/// borrowed-lookup change was measured on `get`/`get_mut`/`entry` and no
/// measurement covers `remove`. Left as a reported finding rather than taken.
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
///
/// The OVERWRITE path allocates nothing: the key is already owned by the map
/// and the value is a refcount bump onto the caller's `Arc`. Only a genuinely
/// new key mints a `String`, and that `String` is what the map keeps.
#[track_caller]
pub fn map_put(name: std::sync::Arc<str>, key: std::sync::Arc<str>, val: std::sync::Arc<str>) -> Value {
    put_in(&name, key, val)
}

fn put_in(name: &str, key: std::sync::Arc<str>, val: std::sync::Arc<str>) -> Value {
    MAPS.with(|m| {
        let mut store = m.borrow_mut();
        match store.get_mut(name) {
            Some(map) => match map.get_mut(&*key) {
                Some(slot) => {
                    *slot = val;
                    Value::Bool(false)
                }
                None => {
                    map.insert(key.to_string(), val);
                    Value::Bool(true)
                }
            },
            None => {
                let mut map = HashMap::new();
                map.insert(key.to_string(), val);
                store.insert(name.to_string(), map);
                Value::Bool(true)
            }
        }
    })
}

/// `map_get(name, key) -> Text` — the value, or "" when absent.
///
/// Allocation-free on both the hit and the miss path. A hit clones the stored
/// `Arc` (refcount bump); a miss clones the thread's cached empty `Arc`.
#[track_caller]
pub fn map_get(name: std::sync::Arc<str>, key: std::sync::Arc<str>) -> Value {
    match find_in(&name, &key) {
        Some(v) => Value::Str(v),
        None => empty_text(),
    }
}

fn find_in(name: &str, key: &str) -> Option<Arc<str>> {
    MAPS.with(|m| {
        let store = m.borrow();
        let map = store.get(name)?;
        #[cfg(test)]
        if MUTANT.load(std::sync::atomic::Ordering::Relaxed) == 1 {
            return map.iter().find(|(ek, _)| ek.starts_with(key) || key.starts_with(ek.as_str())).map(|(_, v)| v.clone());
        }
        map.get(key).cloned()
    })
}

// ── KIND 2 MEMORY (r185): LOCAL MUTABLE MAP HANDLES ──────────────────────────────────────────────────────────────────
//
// A map owned by ONE loop, written in place, dropped by `hmap_free`. It is the `map_*` storage above with the NAME taken
// out of the program's hands: `hmap_new` mints a fresh, never-reused name for the thread and returns a handle, and every
// later call spells that name itself, on the stack. The handle is a `Ctor` with the reserved tag HANDLE_TAG: it is a Value
// because the owner loop carries it in its loop state, which is a Value (a typed handle could not come back out of
// `ctor_field`). It is not Text, so the verifier refuses it as a key or a map_* name; `refuse_handle` makes the cross-thread
// doors (spawn, channel_send, bchan_send) panic on it; ids are unique process-wide, so a handle used on another thread is
// "not live", never another thread's map. The storage is thread-local like the rest of this module.
//   hmap_new(Unit) -> Value               a new empty map; no two handles ever share contents
//   hmap_put(m, k, v) -> Bool             set k to v in place; true iff k was new
//   hmap_has(m, k) -> Bool                exact key membership (never a prefix)
//   hmap_get(m, k) -> Text                "Ok:" + value, or "Unknown" (no sentinel value)
//   hmap_free(m) -> Unit                  drop the map; any later use of the handle panics
static NEXT_HANDLE: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);
const HANDLE_TAG: u32 = u32::MAX;

/// Panic if `v` is, or holds anywhere inside it, a map handle. Called at the doors by which a Value leaves its thread.
pub fn refuse_handle(v: &Value, who: &str) {
    #[cfg(test)]
    if MUTANT.load(std::sync::atomic::Ordering::Relaxed) == 3 {
        return;
    }
    match v {
        Value::Ctor { tag, fields } => {
            if *tag == HANDLE_TAG {
                panic!("{who}: a local map handle cannot be shared (it belongs to the loop that made it)");
            }
            fields.iter().for_each(|f| refuse_handle(f, who));
        }
        Value::Tuple(t) => t.iter().for_each(|f| refuse_handle(f, who)),
        Value::List(l) => l.iter().for_each(|f| refuse_handle(f, who)),
        _ => {}
    }
}

#[cfg(test)]
pub static MUTANT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0); // 0 real, 1 prefix compare, 2 one shared name

/// The internal name of handle `id`, spelled into `buf` (no allocation). A leading control byte keeps it out of reach of any
/// name a program can write for the map_* family.
fn handle_name(id: i64, buf: &mut [u8; 24]) -> &str {
    #[cfg(test)]
    if MUTANT.load(std::sync::atomic::Ordering::Relaxed) == 2 {
        return "\u{1}h0";
    }
    let mut digits = [0u8; 20];
    let (mut n, mut at) = (id as u64, digits.len());
    loop {
        at -= 1;
        digits[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 { break; }
    }
    buf[0] = 1;
    buf[1] = b'h';
    let len = digits.len() - at;
    buf[2..2 + len].copy_from_slice(&digits[at..]);
    std::str::from_utf8(&buf[..2 + len]).unwrap()
}

fn handle_id(h: &Value, who: &str) -> i64 {
    match h {
        Value::Ctor { tag, fields } if *tag == HANDLE_TAG && fields.len() == 1 => match &fields[0] {
            Value::Int(id) => *id,
            _ => panic!("{who}: not a map handle"),
        },
        _ => panic!("{who}: not a map handle"),
    }
}

fn live(name: &str, who: &str) {
    if !MAPS.with(|m| m.borrow().contains_key(name)) {
        panic!("{who}: map handle is not live (freed, or made on another thread)");
    }
}

#[track_caller]
pub fn hmap_new(_: Value) -> Value {
    let id = NEXT_HANDLE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut buf = [0u8; 24];
    let name = handle_name(id, &mut buf).to_string();
    MAPS.with(|m| { m.borrow_mut().entry(name).or_insert_with(HashMap::new); });
    Value::Ctor { tag: HANDLE_TAG, fields: vec![Value::Int(id)] }
}

#[track_caller]
pub fn hmap_put(h: Value, k: Arc<str>, v: Arc<str>) -> Value {
    let mut buf = [0u8; 24];
    let name = handle_name(handle_id(&h, "hmap_put"), &mut buf);
    live(name, "hmap_put");
    put_in(name, k, v)
}

#[track_caller]
pub fn hmap_has(h: Value, k: Arc<str>) -> Value {
    let mut buf = [0u8; 24];
    let name = handle_name(handle_id(&h, "hmap_has"), &mut buf);
    live(name, "hmap_has");
    Value::Bool(find_in(name, &k).is_some())
}

#[track_caller]
pub fn hmap_get(h: Value, k: Arc<str>) -> Value {
    let mut buf = [0u8; 24];
    let name = handle_name(handle_id(&h, "hmap_get"), &mut buf);
    live(name, "hmap_get");
    match find_in(name, &k) {
        Some(v) => Value::Str(Arc::from(format!("Ok:{}", v))),
        None => Value::Str(Arc::from("Unknown")),
    }
}

#[track_caller]
pub fn hmap_free(h: Value) -> Value {
    let mut buf = [0u8; 24];
    let name = handle_name(handle_id(&h, "hmap_free"), &mut buf).to_string();
    MAPS.with(|m| { m.borrow_mut().remove(&name); });
    Value::Unit
}

/// `map_len(name) -> Int` — entry count; 0 for an unknown name.
#[track_caller]
pub fn map_len(name: std::sync::Arc<str>) -> Value {
    MAPS.with(|m| {
        Value::Int(
            m.borrow()
                .get(&*name)
                .map(|map| map.len() as i64)
                .unwrap_or(0),
        )
    })
}

/// `map_clear(name) -> Unit` — drop one named map entirely.
///
/// DELIBERATELY UNALTERED, for the same reason as `set_clear` above: this is
/// the second `HashMap::remove` site and no measurement covers `remove`.
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
    use crate::runtime::value::intern_str;
    use std::sync::Arc;
    use std::time::Instant;

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
        // An overwrite REPLACES, it does not accumulate. This pins the one
        // branch AXVERITY_BRIDGE_GLUE_OPT_SWEEP_V1 genuinely restructured:
        // `entry().or_default().insert(k, v)` became `get_mut(&*key)` then
        // `*slot = val`. The two are equivalent only because `HashMap::insert`
        // replaces the VALUE and keeps the ORIGINAL key — a property the old
        // code relied on silently and nothing in the diff states. If a future
        // edit ever inserts a second entry for an equal key, `map_get` would
        // still return "v2" and only this assertion would notice.
        assert_eq!(map_len(intern_str("m")), Value::Int(1));
    }

    // ── benches — #[ignore]d; run explicitly under --release ──────────────────
    //
    // AXVERITY_BRIDGE_GLUE_OPT_SWEEP_V1 / phase 0. These exist to give the
    // scratch.rs name-allocation change a BEFORE number at the map_get /
    // map_put level. The published 1,810 / 437 ns/op are readtier's M1-level
    // `name-index insert` / `name-index lookup`, which are COMPOSITIONS over
    // these calls plus M1 glue — they are not comparable to the figures below
    // and must not be subtracted from them.
    //
    //   CARGO_TARGET_DIR=<isolated> cargo test --release --lib scratch -- \
    //       --ignored --nocapture --test-threads=1
    //
    // Add --features allocprobe for per-allocation counting (allocprobe.rs);
    // with it off `alloc_probe` reports DISABLED rather than reporting zeros
    // as if they were measurements. Same shape as u32v.rs's benches.
    //
    // TIMING VALIDITY. Cross-session ns/op figures from this file are NOT
    // comparable: the same unchanged `map_put` fresh path measured 476.66 ns
    // in one session and 568.30 ns in another, ~19% apart. A before/after
    // claim must come from a single back-to-back run with both
    // implementations spliced against these same benches. The ALLOCATION
    // COUNTS are the primary evidence — they are exact and immune to that
    // drift; the ns/op figures corroborate.
    //
    // `map_clear` / `set_clear` appear below as FIXTURE SETUP only. They are
    // the two sites AXVERITY_BRIDGE_GLUE_OPT_SWEEP_V1 deliberately left
    // allocating, they are never inside a timed or probed region, and NOTHING
    // here measures them. No figure in this file says anything about `remove`.

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

    /// Pre-built `Arc<str>` keys. Built OUTSIDE every timed and probed region,
    /// so neither the wall clock nor the allocation count includes the cost of
    /// minting them — the call under test is what is measured, not the fixture.
    /// Key shape matches readtier's name index: a short distinct Text per entry.
    fn keys(prefix: &str, n: i64) -> Vec<Arc<str>> {
        (0..n)
            .map(|i| Arc::from(format!("{}{}", prefix, i).as_str()))
            .collect()
    }

    // ── S3a: map_put ─────────────────────────────────────────────────────────

    #[test]
    #[ignore]
    fn bench_map_put_1m() {
        const N: i64 = 1_000_000;
        let name: Arc<str> = Arc::from("b:put");
        let ks = keys("k", N);
        let val: Arc<str> = Arc::from("v");

        // FRESH-KEY insert — the shape readtier's name index actually runs:
        // every call grows the map, so HashMap growth reallocations are
        // included exactly as a real ingest pays them.
        map_clear(name.clone());
        let t0 = Instant::now();
        let mut newly = 0i64;
        for k in &ks {
            if let Value::Bool(true) = map_put(name.clone(), k.clone(), val.clone()) {
                newly += 1;
            }
        }
        let fresh = t0.elapsed();
        assert_eq!(newly, N, "every key was distinct, so every put was an insert");

        // OVERWRITE — same keys again. No growth, no new entries: this
        // isolates the per-call constant from the table's growth cost.
        let t1 = Instant::now();
        let mut over = 0i64;
        for k in &ks {
            if let Value::Bool(false) = map_put(name.clone(), k.clone(), val.clone()) {
                over += 1;
            }
        }
        let overwrite = t1.elapsed();
        assert_eq!(over, N, "second pass over the same keys is all overwrites");

        println!(
            "S3a map_put: n={} fresh_total_ms={:.3} fresh_ns_per_call={:.2} \
             overwrite_total_ms={:.3} overwrite_ns_per_call={:.2}",
            N,
            fresh.as_secs_f64() * 1e3,
            fresh.as_nanos() as f64 / N as f64,
            overwrite.as_secs_f64() * 1e3,
            overwrite.as_nanos() as f64 / N as f64
        );

        // Allocation counts: a separate map for the fresh path so growth
        // reallocations are counted once and not charged twice.
        let pname: Arc<str> = Arc::from("b:put:probe");
        map_clear(pname.clone());
        let probe_fresh = alloc_probe(|| {
            for k in &ks {
                map_put(pname.clone(), k.clone(), val.clone());
            }
        });
        report_allocs("S3a map_put fresh", N, probe_fresh);

        let probe_over = alloc_probe(|| {
            for k in &ks {
                map_put(pname.clone(), k.clone(), val.clone());
            }
        });
        report_allocs("S3a map_put overwrite", N, probe_over);
    }

    // ── S3b: map_get ─────────────────────────────────────────────────────────

    #[test]
    #[ignore]
    fn bench_map_get_1m() {
        const N: i64 = 1_000_000;
        let name: Arc<str> = Arc::from("b:get");
        let ks = keys("k", N);
        let val: Arc<str> = Arc::from("v");

        map_clear(name.clone());
        for k in &ks {
            map_put(name.clone(), k.clone(), val.clone());
        }

        // HIT — every key present. This is the `name-index lookup` shape.
        let t0 = Instant::now();
        let mut hits = 0i64;
        for k in &ks {
            if let Value::Str(s) = map_get(name.clone(), k.clone()) {
                if !s.is_empty() {
                    hits += 1;
                }
            }
        }
        let hit = t0.elapsed();
        assert_eq!(hits, N, "keep the loop from being optimised away");

        // MISS — absent keys on a populated map. Absence-is-empty IS the store
        // convention, not an error path, so a miss is as hot as a hit and is
        // measured as its own shape. It is now allocation-free: `map_get`
        // returns a clone of the thread's cached empty `Arc` rather than
        // minting one, which is why the miss count went 3.00 -> 0.00.
        let miss_ks = keys("absent", N);
        let t1 = Instant::now();
        let mut misses = 0i64;
        for k in &miss_ks {
            if let Value::Str(s) = map_get(name.clone(), k.clone()) {
                if s.is_empty() {
                    misses += 1;
                }
            }
        }
        let miss = t1.elapsed();
        assert_eq!(misses, N, "keep the loop from being optimised away");

        println!(
            "S3b map_get: n={} hit_total_ms={:.3} hit_ns_per_call={:.2} \
             miss_total_ms={:.3} miss_ns_per_call={:.2}",
            N,
            hit.as_secs_f64() * 1e3,
            hit.as_nanos() as f64 / N as f64,
            miss.as_secs_f64() * 1e3,
            miss.as_nanos() as f64 / N as f64
        );

        let probe_hit = alloc_probe(|| {
            let mut h = 0i64;
            for k in &ks {
                if let Value::Str(s) = map_get(name.clone(), k.clone()) {
                    if !s.is_empty() {
                        h += 1;
                    }
                }
            }
            assert_eq!(h, N);
        });
        report_allocs("S3b map_get hit", N, probe_hit);

        let probe_miss = alloc_probe(|| {
            let mut m = 0i64;
            for k in &miss_ks {
                if let Value::Str(s) = map_get(name.clone(), k.clone()) {
                    if s.is_empty() {
                        m += 1;
                    }
                }
            }
            assert_eq!(m, N);
        });
        report_allocs("S3b map_get miss", N, probe_miss);
    }

    // ── S3c: set_add / set_has — the same eight-site pattern, for contrast ────

    #[test]
    #[ignore]
    fn bench_set_add_has_1m() {
        const N: i64 = 1_000_000;
        let name: Arc<str> = Arc::from("b:set");
        let ks = keys("k", N);

        set_clear(name.clone());
        let t0 = Instant::now();
        let mut newly = 0i64;
        for k in &ks {
            if let Value::Bool(true) = set_add(name.clone(), k.clone()) {
                newly += 1;
            }
        }
        let add = t0.elapsed();
        assert_eq!(newly, N);

        // ALREADY-PRESENT — the same keys again. This is the path set_add
        // exists for: a dedup that keeps finding new keys is not deduplicating.
        // Measured separately because the fresh path and this one moved in
        // OPPOSITE directions under the borrowed-lookup change, and the
        // all-fresh figure alone would misrepresent the call.
        let t2 = Instant::now();
        let mut repeat_new = 0i64;
        for k in &ks {
            if let Value::Bool(true) = set_add(name.clone(), k.clone()) {
                repeat_new += 1;
            }
        }
        let repeat = t2.elapsed();
        assert_eq!(repeat_new, 0, "second pass adds nothing — all keys present");

        let t1 = Instant::now();
        let mut present = 0i64;
        for k in &ks {
            if let Value::Bool(true) = set_has(name.clone(), k.clone()) {
                present += 1;
            }
        }
        let has = t1.elapsed();
        assert_eq!(present, N);

        println!(
            "S3c set: n={} add_total_ms={:.3} add_ns_per_call={:.2} \
             repeat_total_ms={:.3} repeat_ns_per_call={:.2} \
             has_total_ms={:.3} has_ns_per_call={:.2}",
            N,
            add.as_secs_f64() * 1e3,
            add.as_nanos() as f64 / N as f64,
            repeat.as_secs_f64() * 1e3,
            repeat.as_nanos() as f64 / N as f64,
            has.as_secs_f64() * 1e3,
            has.as_nanos() as f64 / N as f64
        );

        let probe_repeat = alloc_probe(|| {
            for k in &ks {
                set_add(name.clone(), k.clone());
            }
        });
        report_allocs("S3c set_add already-present", N, probe_repeat);

        let probe_has = alloc_probe(|| {
            let mut p = 0i64;
            for k in &ks {
                if let Value::Bool(true) = set_has(name.clone(), k.clone()) {
                    p += 1;
                }
            }
            assert_eq!(p, N);
        });
        report_allocs("S3c set_has", N, probe_has);
    }

    // ── kind 2 (local map handles); the trie spike's tests, kept: interference, prefix, many keys. Run with --test-threads=1
    // because MUTANT is global.
    use std::sync::atomic::Ordering::Relaxed;
    fn k(x: &str) -> Arc<str> { intern_str(x) }
    fn hnew() -> Value { hmap_new(Value::Unit) }
    fn hget(m: &Value, key: &str) -> String { match hmap_get(m.clone(), k(key)) { Value::Str(x) => x.to_string(), _ => panic!() } }
    fn hhas(m: &Value, key: &str) -> bool { hmap_has(m.clone(), k(key)) == Value::Bool(true) }

    fn interference() -> bool {
        let a = hnew();
        let b = hnew();
        hmap_put(a.clone(), k("k1"), k("v1"));
        hmap_put(b.clone(), k("k1"), k("v1"));
        hmap_put(a.clone(), k("k2"), k("v2"));
        let ok = !hhas(&b, "k2") && hhas(&a, "k2") && !hhas(&hnew(), "k1");
        hmap_free(a);
        hmap_free(b);
        ok
    }
    fn prefix() -> bool {
        let m = hnew();
        hmap_put(m.clone(), k("sha256:abcdef"), k("v"));
        let ok = !hhas(&m, "sha256:abc") && !hhas(&m, "sha256:abcdefg") && hget(&m, "sha256:abc") == "Unknown" && hget(&m, "sha256:abcdef") == "Ok:v";
        hmap_free(m);
        ok
    }

    #[test]
    fn kind2_real_keeps_both() {
        MUTANT.store(0, Relaxed);
        assert!(interference() && prefix());
    }

    #[test]
    fn kind2_many_keys_overwrite_and_free() {
        MUTANT.store(0, Relaxed);
        let m = hnew();
        for i in 0..5000 { assert_eq!(hmap_put(m.clone(), k(&format!("k{i}")), k(&format!("v{i}"))), Value::Bool(true)); }
        for i in 0..5000 { assert_eq!(hget(&m, &format!("k{i}")), format!("Ok:v{i}")); }
        assert_eq!(hget(&m, "k5000"), "Unknown");
        assert_eq!(hmap_put(m.clone(), k("k7"), k("new")), Value::Bool(false));
        assert_eq!(hget(&m, "k7"), "Ok:new");
        hmap_free(m);
    }

    #[test]
    #[should_panic(expected = "not live")]
    fn kind2_use_after_free_panics() {
        MUTANT.store(0, Relaxed);
        let m = hnew();
        hmap_free(m.clone());
        hmap_has(m, k("x"));
    }

    // a handle placed into a shared value: alone, inside a Ctor, inside a List, through the real doors
    fn shared_refused() -> bool {
        let h = hnew();
        let inside = Value::Ctor { tag: 0, fields: vec![Value::Int(1), Value::List(crate::runtime::value::ListBuf::from(vec![h.clone()]))] };
        let refused = |v: &Value| std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| refuse_handle(v, "t"))).is_err();
        let door = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::runtime::channels::channel_send(k("kind2_door"), inside.clone()))).is_err();
        let plain = !refused(&Value::Ctor { tag: 0, fields: vec![Value::Int(1)] }) && !refused(&Value::Tuple(vec![Value::Int(1), Value::Unit]));
        let ok = refused(&h) && refused(&inside) && door && plain;
        hmap_free(h);
        ok
    }

    #[test]
    fn kind2_handle_is_not_shareable_and_not_foreign() {
        MUTANT.store(0, Relaxed);
        assert!(shared_refused());
        // a handle used on another thread is not live there (ids are process-wide, so it never lands on another map)
        let m = hnew();
        hmap_put(m.clone(), k("a"), k("1"));
        let m2 = m.clone();
        let r = std::thread::spawn(move || std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hmap_has(m2, k("a")))).is_err()).join().unwrap();
        assert!(r);
        hmap_free(m);
    }

    #[test]
    fn kind2_mutants_fail() {
        MUTANT.store(3, Relaxed);
        assert!(!shared_refused(), "mutant 3 (handle accepted into a shared value) was not caught");
        MUTANT.store(1, Relaxed);
        assert!(!prefix(), "mutant 1 (prefix compare) was not caught");
        MUTANT.store(2, Relaxed);
        assert!(!interference(), "mutant 2 (two handles share contents) was not caught");
        MUTANT.store(0, Relaxed);
    }
}

//! BRIDGE_GCIDX_V1 (D044 Phase 3, AXVERITY_HOTBLK_TIMED_FLUSH_V1) — a
//! dedicated resident cache for graphcore's verb-index lookup artifacts.
//!
//! ## Why this exists (grounded, not assumed)
//!
//! Checked directly against pg_store.rs: `gr_index_lookup` (via `gr_get` ->
//! `gr_get_from_index`) resolved through `pg_bytes_get` on EVERY call — a
//! live `SELECT content FROM gcore_objects WHERE addr = $1`, serialized
//! through ONE `Mutex`-guarded connection — with zero residency. A hot
//! verb-index artifact paid the full round-trip every lookup, same as a
//! cold one-off fetch. D038/D039 addressed durability/write-path only;
//! read residency for graphcore was never part of that work.
//!
//! ## Why NOT contentidx (the existing general RAM cache)
//!
//! `contentidx` is fed by every `gr_put` across the whole system — edges,
//! records, everything — and FIFO-evicts under that shared, general churn
//! (own cap, default 65536). An index artifact, once built, competes with
//! all of that unrelated traffic for the same eviction budget: under real
//! load it can get evicted long before the NEXT lookup, even though
//! keeping it resident is cheap and specifically valuable (this was the
//! motivating ask that scoped this phase: indexes should get their OWN
//! allocation, staying hot independent of what else is happening — not
//! share a budget with every other object in the system). This cache is
//! scoped EXCLUSIVELY to `gr_index_lookup`'s artifact fetch via the new
//! `gcidx_get` builtin — `gr_get`/`gr_put`/contentidx's OWN behavior is
//! unmodified, and nothing else in the bridge ever calls INTO this module.
//!
//! `gcidx_get` DOES read contentidx on its own miss path, though — not an
//! exception to "own allocation," a correctness requirement: `gr_put`
//! (which `gr_index_build` calls) buffers into the OBJECT family's
//! hot-block arena and returns before the artifact is durably flushed to
//! postgres; contentidx is what closes that read-after-write gap for
//! EVERY object in the system, this cache included. A first-pass version
//! of this module skipped straight to `pg_bytes_get` on its own miss and
//! made a just-built index artifact briefly, reproducibly invisible
//! (`tests/run.sh`'s P3 section: 107/107 -> 100/107, "lookup all: 3
//! postings, got 0" immediately after `index`) — fixed by falling through
//! to contentidx first, exactly where `gr_get` already checks it, before
//! `pg_bytes_get`. `gcidx_get`'s own cache stays the FIRST tier checked
//! either way, ahead of both.
//!
//! ## Shape: qhm.rs's pattern, not contentidx's
//!
//! Many concurrent readers (`gr_index_lookup` from every request) against
//! one writer path (populate-on-miss; content is immutable once
//! addressed, so there is never a legitimate "update" to an existing
//! entry) is exactly `qhm.rs`'s "lock-free `BridgedCell` reads, brief
//! `Mutex<Writer>` writes, readers never lock" fit — see that module's own
//! doc comment for why it chose this over a plain sharded mutex.
//!
//! Deliberately SIMPLER than qhm.rs's own internals, though: qhm shards
//! (256-way) and batches/seals writes because it is built for a
//! high-THROUGHPUT write workload (every INSERT). This cache's writes are
//! rare — at most one per distinct verb-index artifact ever created, since
//! content is immutable once addressed — so there is no need for
//! per-shard sealing machinery. A single whole-map copy-on-write
//! `BridgedCell`, `Arc`-wrapped values so a "clone the whole map" write
//! only clones cheap `Arc` handles, never artifact bytes.
//!
//! Readers use the plain `BridgedCell::read()` — the "materialises an
//! owned clone, pins nothing" path (`hotmem.rs`'s own naming for it), not
//! `read_ref()`. That is a deliberate, load-bearing choice, not an
//! oversight: because no caller here EVER acquires a `ReaderHandle`,
//! `ReaderRegistry::has_any_reader()` is always false, so every write's
//! superseded map is freed IMMEDIATELY via `write_lazy`'s Level B fast
//! path (`non_blocking_memory.rs`) — zero retained memory, ever, with none
//! of `BridgedCell`'s epoch-floor reclamation complexity actually needed
//! for this workload.
//!
//! ## NOT a bypass of D032's staleness check
//!
//! This cache stores bytes keyed by content ADDRESS, which never change
//! meaning (content-addressed — the whole point). `gr_index_lookup.m1`'s
//! own anchor comparison (unchanged — see graphcore/lib/gr_index_lookup.m1)
//! runs against whatever bytes it receives, cached or not; caching only
//! ever saves the network round-trip when the fetched artifact would have
//! been identical anyway. It never skips the check itself.
//!
//! ## Tuning
//!   AXVERITY_GCIDX_CAP   total resident artifact-address cap (default
//!                        4096; own knob, distinct from AXVERITY_QHM_CAP —
//!                        indexes staying hot is a distinct concern from
//!                        general query-hotmem residency, not the same
//!                        pool)

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

use super::non_blocking_memory::{BridgedCell, ReadResult, ReaderRegistry, Writer};
use super::pg_store;
use super::value::{get_str, intern_str, Value};

const CAP_DEFAULT: usize = 4096;

fn cap() -> usize {
    static CAP: OnceLock<usize> = OnceLock::new();
    *CAP.get_or_init(|| {
        std::env::var("AXVERITY_GCIDX_CAP")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|n| *n >= 1)
            .unwrap_or(CAP_DEFAULT)
    })
}

/// `(addr -> bytes, FIFO insertion order)` — published as ONE value so a
/// write clones the map cheaply (values are `Arc<Vec<u8>>`) without ever
/// cloning artifact bytes.
type Map = (HashMap<String, Arc<Vec<u8>>>, VecDeque<String>);

struct Registry {
    cell: Arc<BridgedCell<Arc<Map>>>,
    writer: Mutex<Writer<Arc<Map>>>,
    /// Never issues a handle — exists only so write_lazy's
    /// has_any_reader() check sees "no reader ever", taking the
    /// immediate-free fast path every time (see module doc comment).
    readers: ReaderRegistry,
}

fn registry() -> &'static Registry {
    static REG: OnceLock<Registry> = OnceLock::new();
    REG.get_or_init(|| {
        let (cell, writer) = BridgedCell::new();
        Registry { cell, writer: Mutex::new(writer), readers: ReaderRegistry::new() }
    })
}

fn get_cached(addr: &str) -> Option<Arc<Vec<u8>>> {
    match registry().cell.read(0) {
        ReadResult::Empty => None,
        ReadResult::Value { value, .. } => value.0.get(addr).cloned(),
    }
}

fn put_cached(addr: &str, bytes: Arc<Vec<u8>>) {
    let reg = registry();
    let mut w = reg.writer.lock().unwrap();
    let (mut map, mut order) = match reg.cell.read(0) {
        ReadResult::Empty => (HashMap::new(), VecDeque::new()),
        ReadResult::Value { value, .. } => (*value).clone(),
    };
    if map.contains_key(addr) {
        return; // already populated (raced with another miss) -- content-addressed, nothing to update
    }
    map.insert(addr.to_string(), bytes);
    order.push_back(addr.to_string());
    while map.len() > cap() {
        match order.pop_front() {
            Some(oldest) => {
                map.remove(&oldest);
            }
            None => break,
        }
    }
    // SAFETY: this is the sole writer for this cell (guarded by `writer`'s
    // Mutex — no second Writer for this cell exists anywhere).
    unsafe {
        w.write_lazy(Arc::new((map, order)), &reg.readers);
    }
}

/// `gcidx_get(addr: Text) -> Text` — same F/A-enveloped contract as
/// `gr_get`/`gr_get_from_index` (`"F" GS <content>` / `"A" GS`). The SOLE
/// caller is `graphcore/lib/gr_index_lookup.m1`, which uses this in place
/// of `gr_get` for its own artifact fetch — everything else in graphcore
/// keeps using `gr_get`/contentidx exactly as before, untouched.
#[track_caller]
pub fn gcidx_get(arg: Value) -> Value {
    let addr = match arg {
        Value::Str(h) => get_str(&h),
        other => panic!("gcidx_get: expected Text addr, got {:?}", other),
    };
    let gs = "\u{1d}";

    if let Some(bytes) = get_cached(&addr) {
        let text = String::from_utf8((*bytes).clone())
            .unwrap_or_else(|e| panic!("gcidx_get: cached artifact is not valid UTF-8: {}", e));
        return Value::Str(intern_str(&format!("F{}{}", gs, text)));
    }

    // Miss in our own cache -- fall through to the SAME contentidx tier
    // gr_get already checks, BEFORE pg_bytes_get. This is load-bearing,
    // not an optional optimization: gr_put (which gr_index_build calls)
    // buffers into the OBJECT family's hot-block arena and returns before
    // it's durably flushed to postgres (gr_put.m1's own header: "a crash
    // between flushes can lose up to one buffer's worth of already-
    // returned gr_put calls") -- contentidx is what closes that
    // read-after-write gap. Skipping straight to pg_bytes_get, as an
    // earlier version of this fn did, made a just-built index artifact
    // briefly (and reproducibly) invisible: caught by tests/run.sh's own
    // P3 section going from 107/107 to 100/107 the first time this
    // landed, "lookup all: 3 postings, got 0" straight after `index`.
    let from_contentidx = match super::contentidx::contentidx_get(Value::Str(intern_str(&addr))) {
        Value::Bytes(b) => b,
        other => panic!("gcidx_get: contentidx_get returned non-Bytes: {:?}", other),
    };
    let content = if !from_contentidx.is_empty() {
        from_contentidx
    } else {
        match pg_store::pg_bytes_get(Value::Str(intern_str(&addr))) {
            Value::Bytes(b) => b,
            other => panic!("gcidx_get: pg_bytes_get returned non-Bytes: {:?}", other),
        }
    };
    if content.is_empty() {
        return Value::Str(intern_str(&format!("A{}", gs)));
    }
    put_cached(&addr, Arc::new(content.clone()));
    let text = String::from_utf8(content)
        .unwrap_or_else(|e| panic!("gcidx_get: fetched artifact is not valid UTF-8: {}", e));
    Value::Str(intern_str(&format!("F{}{}", gs, text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(seed: &str) -> String {
        format!("sha256:gcidx-test-{}-{}", std::process::id(), seed)
    }

    #[test]
    #[ignore = "shares pg_store's process-wide scratch DB — run explicitly, not as part of the default suite"]
    fn miss_then_hit_skips_the_second_postgres_round_trip() {
        let a = addr("mth");
        let content = b"gcidx round-trip test content".to_vec();
        pg_store::pg_bytes_put(Value::Tuple(vec![
            Value::Str(intern_str(&a)),
            Value::Bytes(content.clone()),
        ]));

        // First call: cache miss, must hit postgres and return the right bytes.
        let r1 = gcidx_get(Value::Str(intern_str(&a)));
        let text1 = match &r1 {
            Value::Str(h) => get_str(h),
            other => panic!("expected Text, got {:?}", other),
        };
        assert!(text1.starts_with('F'), "expected found envelope, got {:?}", text1);
        assert!(text1.ends_with("gcidx round-trip test content"));

        // PROOF, not assumption: delete the row out from under the cache.
        // If the second gcidx_get call still returns the correct content,
        // it MUST have come from the cache -- postgres has nothing left to
        // return (a direct pg_bytes_get confirms the row is really gone).
        {
            let mut c = pg_store::conn().lock().unwrap();
            c.execute("DELETE FROM gcore_objects WHERE addr = $1", &[&a])
                .unwrap_or_else(|e| panic!("test setup: DELETE failed: {}", e));
        }
        let gone = pg_store::pg_bytes_get(Value::Str(intern_str(&a)));
        assert_eq!(gone, Value::Bytes(Vec::new()), "row must be genuinely absent from postgres now");

        let r2 = gcidx_get(Value::Str(intern_str(&a)));
        assert_eq!(r1, r2, "cache-served hit must be identical even though postgres no longer has the row");
    }

    #[test]
    fn cap_eviction_is_fifo_and_correctness_preserving() {
        // Pure in-memory: exercises get_cached/put_cached directly, no
        // postgres needed, and no #[ignore] -- this part touches no shared
        // process-wide postgres state, only this module's own cache
        // (itself process-wide, but self-consistent regardless of other
        // tests' addresses since every key here is uniquely prefixed).
        let prefix = format!("gcidx-evict-test-{}-", std::process::id());
        let n = cap() + 5;
        for i in 0..n {
            put_cached(&format!("{}{}", prefix, i), Arc::new(format!("body-{}", i).into_bytes()));
        }
        // The first 5 inserted (oldest) must have been evicted...
        for i in 0..5 {
            assert!(
                get_cached(&format!("{}{}", prefix, i)).is_none(),
                "entry {} should have been FIFO-evicted",
                i
            );
        }
        // ...while the most recent `cap()` entries remain resident.
        for i in 5..n {
            let got = get_cached(&format!("{}{}", prefix, i));
            assert_eq!(
                got.map(|b| (*b).clone()),
                Some(format!("body-{}", i).into_bytes()),
                "entry {} should still be resident and correct",
                i
            );
        }
    }
}

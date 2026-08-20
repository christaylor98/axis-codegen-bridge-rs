//! AXVERITY_U32V_BRIDGE_PRIMITIVE_V1 — NAMED, GROWABLE, APPEND-ONLY u32
//! VECTORS: process-scoped, thread-local, Text-keyed dense integer arrays with
//! a single sealed in-place sort. Never durable, never shared across threads.
//!
//! ## Why this module exists
//!
//! M1 has no integer-keyed mutable storage primitive. `scratch.rs` gave it
//! Text-keyed sets and maps, which fixed O(n²) MEMBERSHIP; this fixes the
//! adjacent gap — a DENSE ORDERED array of small integers, which is what a
//! stride-3 projection of `(source, verb, target)` ids wants to be. Held as
//! `Vec<u32>` rather than `Value::List`, so a push is an integer store rather
//! than a `Value` construction, and the array is never named-and-cloned at an
//! M1 call site (cf. the `ref_clone` O(N²) fold finding in CLAUDE.md).
//!
//! ## Design — names, not handles
//!
//! Same storage model as `scratch.rs` / `adjacency.rs` / `logbuf.rs`:
//! thread-local, no Mutex, no process-global registry. A vector is keyed by a
//! caller-chosen Text NAME, because a name is a constant an M1 no-capture step
//! can carry in its accumulator, there is nothing to leak or free, and
//! re-running a build is deterministic because the owner calls `u32v_new` up
//! front.
//!
//! THREAD-LOCAL IS LOAD-BEARING AND VISIBLE: a vector built on one thread does
//! not exist on another. Nothing here crosses a thread boundary, and no
//! `Value` ever holds a reference to the storage — arguments arrive as
//! scalars and results leave as `Value::Int` / `Value::Unit` — so
//! `VALUE_MUST_STAY_SEND_SYNC` (`value.rs:38-41`) is satisfied by
//! construction rather than by care.
//!
//! ## Lookup borrows the name; it does not copy it
//!
//! Every entry point indexes `VECS` with `&*name` (an `&str` reborrow of the
//! caller's `Arc<str>`), relying on `String: Borrow<str>`. A `String` is
//! allocated ONLY on the genuine insert path — the first `u32v_push` or
//! `u32v_new` for a name that does not exist yet.
//!
//! This deliberately DIVERGES from `scratch.rs`, which opens every entry point
//! with `let name = name.to_string()` (`scratch.rs:48, 55, 69, 76, 87, 102,
//! 116, 123`) and so heap-allocates once per call purely to index a
//! `HashMap<String, _>`. That is not a stylistic preference: measured at 1M
//! calls, the copy was 23.31 ns of a 46.66 ns `push` — **50.0% of the call**.
//! `u32v_get` is on the path of an M1-side binary search, which pays it ~20
//! times per range lookup, so the divergence is load-bearing rather than
//! cosmetic.
//!
//! ## The surface is APPEND-ONLY PLUS A SEALED SORT
//!
//! Five calls: `new`, `push`, `get`, `len`, `sort3`. There is deliberately no
//! `set`, `insert`, `remove`, `truncate` or `clear`. The ONLY in-place
//! mutation of existing elements lives inside `u32v_sort3`, which is a
//! one-shot reordering of the whole array — the "seal" step after which the
//! array is expected to be read, not grown. (`u32v_push` after a `sort3` is
//! not rejected; the array simply stops being sorted. Enforcing a sealed flag
//! would be policy, and policy is the consumer's, not the bridge's.)
//!
//! ## What is NOT here, on purpose
//!
//! No triple semantics beyond the stride the caller declares at `sort3`. No
//! rotation logic — a rotation is a KEY ORDER, which the caller passes as
//! `(k0, k1, k2)`. No dedup policy. No binary search. No range lookup. Those
//! are compositions over `get`/`len`, and they belong in M1.

use std::cell::RefCell;
use std::collections::HashMap;

use super::value::Value;

thread_local! {
    /// Per-thread named u32 arrays. THREAD-LOCAL, never shared: reachable only
    /// by the thread that created the name. No lock is taken anywhere.
    static VECS: RefCell<HashMap<String, Vec<u32>>> = RefCell::new(HashMap::new());
}

/// The u32 domain, as an `i64` bound. `Int` is M1's only integer type, so
/// every value crossing the bridge is checked against this rather than
/// truncated into it.
const U32_MAX_I64: i64 = u32::MAX as i64;

/// `u32v_new(name) -> Unit` — establish `name` as an EMPTY array.
///
/// Idempotent in the registry sense and destructive in the storage sense: if
/// the name already exists its contents are dropped. That is the same
/// owner-clears-up-front discipline `scratch.rs` documents — a build step that
/// begins with `u32v_new` produces the same array whether or not the process
/// ran it before, which is what makes a re-run deterministic.
///
/// `*vec = Vec::new()` on the re-establish path, NOT `vec.clear()`: clear would
/// retain the old capacity, which is invisible through the five-call surface
/// but would make a rebuild's reallocation count depend on whether the process
/// had used that name before. Dropping the buffer keeps capacity at 0 either
/// way, so the array a caller gets is a function of its own calls and nothing
/// else.
#[track_caller]
pub fn u32v_new(name: std::sync::Arc<str>) -> Value {
    VECS.with(|v| {
        let mut store = v.borrow_mut();
        match store.get_mut(&*name) {
            Some(vec) => *vec = Vec::new(),
            None => {
                store.insert(name.to_string(), Vec::new());
            }
        }
    });
    Value::Unit
}

/// `u32v_push(name, n) -> Unit` — append one element.
///
/// `n` MUST be in `0..=4294967295`. Out of range PANICS rather than
/// truncating, for the same reason `bytes_push` does (`bytes_codec.rs:129-139`):
/// a silent wrap would turn an out-of-domain id into a plausible-looking one,
/// and an id that is wrong-but-plausible is undetectable downstream.
///
/// An unknown name is created empty, matching `set_add`'s
/// `entry(name).or_default()` (`scratch.rs:49`) in effect — but NOT in
/// mechanism: `entry()` requires an owned `String` key on every call, whether
/// or not the name is new. See the module note on borrowed lookup.
#[track_caller]
pub fn u32v_push(name: std::sync::Arc<str>, n: i64) -> Value {
    if n < 0 || n > U32_MAX_I64 {
        panic!("u32v_push: value {} out of range for u32 (0..=4294967295)", n);
    }
    VECS.with(|v| {
        let mut store = v.borrow_mut();
        match store.get_mut(&*name) {
            Some(vec) => vec.push(n as u32),
            None => {
                store.insert(name.to_string(), vec![n as u32]);
            }
        }
    });
    Value::Unit
}

/// `u32v_get(name, idx) -> Int` — element at `idx`.
///
/// STRICT: an unknown name or an out-of-range index panics. Unlike `map_get`,
/// where absence is meaningfully the empty Text, there is no u32 that means
/// "absent" — every value in `0..=4294967295` is a legitimate id. So this
/// follows `bytes_get` (`bytes_codec.rs:120-125`) rather than the store
/// convention.
#[track_caller]
pub fn u32v_get(name: std::sync::Arc<str>, idx: i64) -> Value {
    VECS.with(|v| {
        let store = v.borrow();
        let vec = store
            .get(&*name)
            .unwrap_or_else(|| panic!("u32v_get: unknown name {:?}", &*name));
        if idx < 0 || idx as usize >= vec.len() {
            panic!(
                "u32v_get: index {} out of range for {:?} of len {}",
                idx,
                &*name,
                vec.len()
            );
        }
        Value::Int(vec[idx as usize] as i64)
    })
}

/// `u32v_len(name) -> Int` — element count; 0 for an unknown name.
///
/// Permissive where `u32v_get` is strict, matching `set_len`
/// (`scratch.rs:68-71`): a length of zero and an absent name are the same
/// thing to a caller about to iterate, so there is nothing to distinguish.
#[track_caller]
pub fn u32v_len(name: std::sync::Arc<str>) -> Value {
    VECS.with(|v| {
        Value::Int(
            v.borrow()
                .get(&*name)
                .map(|vec| vec.len() as i64)
                .unwrap_or(0),
        )
    })
}

/// `u32v_sort3(name, k0, k1, k2) -> Unit` — sort in place as stride-3 records.
///
/// The array is read as `len/3` consecutive `[u32; 3]` records and reordered
/// by comparing slot `k0` first, then `k1`, then `k2`. `(k0, k1, k2)` MUST be
/// a permutation of `(0, 1, 2)`.
///
/// THE KEY ORDER IS THE ROTATION. A caller wanting `(verb, target, source)`
/// order over `(s, v, t)`-packed records passes `(1, 2, 0)`. There is no
/// rotation logic in this module and no rotation table — a rotation is a
/// choice the caller declares, which is why it is an argument rather than a
/// family of five more entry points.
///
/// Requiring a full permutation is a pre-condition, not a convenience: with
/// all three slots in the key the comparison is total over the record, so two
/// records compare equal only when they are byte-identical and an UNSTABLE
/// sort is therefore indistinguishable from a stable one. A partial key
/// (a repeated slot) would make the output order depend on the sort's internal
/// choices, which is exactly the kind of undeclared nondeterminism a
/// projection must not inherit.
///
/// PANICS on an unknown name, on a length that is not a multiple of 3, and on
/// a key triple that is not a permutation of `(0, 1, 2)`.
#[track_caller]
pub fn u32v_sort3(name: std::sync::Arc<str>, k0: i64, k1: i64, k2: i64) -> Value {
    for (label, k) in [("k0", k0), ("k1", k1), ("k2", k2)] {
        if !(0..=2).contains(&k) {
            panic!("u32v_sort3: {} = {} is not a slot in 0..=2", label, k);
        }
    }
    if k0 == k1 || k0 == k2 || k1 == k2 {
        panic!(
            "u32v_sort3: ({}, {}, {}) is not a permutation of (0, 1, 2) — \
             a repeated slot leaves the record order undeclared",
            k0, k1, k2
        );
    }
    let (a, b, c) = (k0 as usize, k1 as usize, k2 as usize);

    VECS.with(|v| {
        let mut store = v.borrow_mut();
        let vec = store
            .get_mut(&*name)
            .unwrap_or_else(|| panic!("u32v_sort3: unknown name {:?}", &*name));
        if vec.len() % 3 != 0 {
            panic!(
                "u32v_sort3: {:?} has len {}, not a multiple of 3",
                &*name,
                vec.len()
            );
        }
        let (records, rest) = vec.as_chunks_mut::<3>();
        debug_assert!(rest.is_empty(), "len % 3 == 0 was checked above");
        records.sort_unstable_by(|x, y| {
            (x[a], x[b], x[c]).cmp(&(y[a], y[b], y[c]))
        });
    });
    Value::Unit
}

/// Capacity of a named array, for the allocation bench ONLY.
///
/// NOT an M1 surface call: absent from `symbol_map()` and from every registry,
/// so no M1 source can reach it. It exists because "amortised allocation-free"
/// is a claim about realloc COUNT, and a count cannot be observed from the
/// five-call surface.
#[cfg(test)]
fn capacity_for_bench(name: &str) -> usize {
    VECS.with(|v| v.borrow().get(name).map(|vec| vec.capacity()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    fn n(s: &str) -> Arc<str> {
        Arc::from(s)
    }

    fn collect(name: &str) -> Vec<i64> {
        let len = match u32v_len(n(name)) {
            Value::Int(i) => i,
            other => panic!("u32v_len returned {:?}", other),
        };
        (0..len)
            .map(|i| match u32v_get(n(name), i) {
                Value::Int(v) => v,
                other => panic!("u32v_get returned {:?}", other),
            })
            .collect()
    }

    // ── new ──────────────────────────────────────────────────────────────────

    #[test]
    fn new_establishes_empty_and_resets_existing() {
        u32v_new(n("t:new"));
        u32v_push(n("t:new"), 7);
        assert_eq!(u32v_len(n("t:new")), Value::Int(1));
        u32v_new(n("t:new"));
        assert_eq!(u32v_len(n("t:new")), Value::Int(0));
    }

    #[test]
    fn names_are_independent() {
        u32v_new(n("t:ind:a"));
        u32v_new(n("t:ind:b"));
        u32v_push(n("t:ind:a"), 1);
        assert_eq!(u32v_len(n("t:ind:a")), Value::Int(1));
        assert_eq!(u32v_len(n("t:ind:b")), Value::Int(0));
    }

    // ── push ─────────────────────────────────────────────────────────────────

    #[test]
    fn push_appends_in_order_and_creates_on_demand() {
        u32v_new(n("t:push"));
        for i in 0..5 {
            u32v_push(n("t:push"), i * 10);
        }
        assert_eq!(collect("t:push"), vec![0, 10, 20, 30, 40]);

        // unknown name is created empty, per set_add's entry().or_default()
        u32v_new(n("t:push:auto"));
        VECS.with(|v| v.borrow_mut().remove("t:push:auto"));
        u32v_push(n("t:push:auto"), 3);
        assert_eq!(collect("t:push:auto"), vec![3]);
    }

    #[test]
    fn push_accepts_the_full_u32_domain() {
        u32v_new(n("t:dom"));
        u32v_push(n("t:dom"), 0);
        u32v_push(n("t:dom"), U32_MAX_I64);
        assert_eq!(collect("t:dom"), vec![0, 4_294_967_295]);
    }

    #[test]
    #[should_panic(expected = "out of range for u32")]
    fn push_rejects_negative() {
        u32v_new(n("t:neg"));
        u32v_push(n("t:neg"), -1);
    }

    #[test]
    #[should_panic(expected = "out of range for u32")]
    fn push_rejects_above_u32_max_rather_than_truncating() {
        u32v_new(n("t:over"));
        u32v_push(n("t:over"), U32_MAX_I64 + 1);
    }

    // ── get ──────────────────────────────────────────────────────────────────

    #[test]
    #[should_panic(expected = "unknown name")]
    fn get_rejects_unknown_name() {
        u32v_get(n("t:nope"), 0);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn get_rejects_index_past_end() {
        u32v_new(n("t:oob"));
        u32v_push(n("t:oob"), 1);
        u32v_get(n("t:oob"), 1);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn get_rejects_negative_index() {
        u32v_new(n("t:negidx"));
        u32v_push(n("t:negidx"), 1);
        u32v_get(n("t:negidx"), -1);
    }

    // ── len ──────────────────────────────────────────────────────────────────

    #[test]
    fn len_of_unknown_name_is_zero() {
        assert_eq!(u32v_len(n("t:absent")), Value::Int(0));
    }

    // ── sort3 ────────────────────────────────────────────────────────────────

    fn push_triples(name: &str, triples: &[[i64; 3]]) {
        u32v_new(n(name));
        for t in triples {
            for slot in t {
                u32v_push(n(name), *slot);
            }
        }
    }

    #[test]
    fn sort3_orders_by_declared_key_order() {
        // (s, v, t) records. Key order (0,1,2) is source-major.
        let triples = [[2, 1, 9], [1, 3, 4], [1, 2, 8], [2, 0, 5]];
        push_triples("t:s012", &triples);
        u32v_sort3(n("t:s012"), 0, 1, 2);
        assert_eq!(
            collect("t:s012"),
            vec![1, 2, 8, 1, 3, 4, 2, 0, 5, 2, 1, 9]
        );
    }

    #[test]
    fn sort3_rotation_is_a_key_order_not_a_different_algorithm() {
        // Same records, key order (1,2,0) — verb-major, the rt:B rotation.
        let triples = [[2, 1, 9], [1, 3, 4], [1, 2, 8], [2, 0, 5]];
        push_triples("t:s120", &triples);
        u32v_sort3(n("t:s120"), 1, 2, 0);
        // by v then t then s: (2,0,5) (2,1,9) (1,2,8) (1,3,4)
        assert_eq!(
            collect("t:s120"),
            vec![2, 0, 5, 2, 1, 9, 1, 2, 8, 1, 3, 4]
        );

        // And the third rotation, (2,0,1) — target-major.
        push_triples("t:s201", &triples);
        u32v_sort3(n("t:s201"), 2, 0, 1);
        // by t then s then v: (1,3,4) (2,0,5) (1,2,8) (2,1,9)
        assert_eq!(
            collect("t:s201"),
            vec![1, 3, 4, 2, 0, 5, 1, 2, 8, 2, 1, 9]
        );
    }

    #[test]
    fn sort3_is_a_total_order_over_the_record_so_duplicates_survive() {
        let triples = [[1, 1, 1], [0, 9, 9], [1, 1, 1]];
        push_triples("t:dup", &triples);
        u32v_sort3(n("t:dup"), 0, 1, 2);
        assert_eq!(collect("t:dup"), vec![0, 9, 9, 1, 1, 1, 1, 1, 1]);
        // No dedup happened — dedup is the consumer's policy, not the bridge's.
        assert_eq!(u32v_len(n("t:dup")), Value::Int(9));
    }

    #[test]
    fn sort3_on_empty_is_a_no_op() {
        u32v_new(n("t:empty"));
        u32v_sort3(n("t:empty"), 0, 1, 2);
        assert_eq!(u32v_len(n("t:empty")), Value::Int(0));
    }

    #[test]
    #[should_panic(expected = "not a multiple of 3")]
    fn sort3_rejects_a_ragged_array() {
        u32v_new(n("t:ragged"));
        u32v_push(n("t:ragged"), 1);
        u32v_push(n("t:ragged"), 2);
        u32v_sort3(n("t:ragged"), 0, 1, 2);
    }

    #[test]
    #[should_panic(expected = "not a permutation")]
    fn sort3_rejects_a_repeated_slot() {
        u32v_new(n("t:perm"));
        u32v_sort3(n("t:perm"), 0, 0, 1);
    }

    #[test]
    #[should_panic(expected = "is not a slot in 0..=2")]
    fn sort3_rejects_an_out_of_range_slot() {
        u32v_new(n("t:slot"));
        u32v_sort3(n("t:slot"), 0, 1, 3);
    }

    #[test]
    #[should_panic(expected = "unknown name")]
    fn sort3_rejects_unknown_name() {
        u32v_sort3(n("t:sortnope"), 0, 1, 2);
    }

    // ── benches — #[ignore]d; run explicitly under --release ──────────────────
    //
    //   CARGO_TARGET_DIR=<isolated> cargo test --release --lib u32v -- \
    //       --ignored --nocapture --test-threads=1
    //
    // Add --features allocprobe to the same command to turn on per-allocation
    // counting (allocprobe.rs). It is a compile-time feature and needs no
    // source change; with it off, `alloc_probe` below reports DISABLED rather
    // than reporting zeros as if they were measurements.

    /// Run `body` and report the global allocation delta across it, or `None`
    /// when the allocprobe feature is compiled out.
    ///
    /// Counts are process-global, so this is only attributable because the
    /// benches run under `--test-threads=1` and the loop body is the only
    /// thing allocating.
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
            None => println!("{}: allocprobe DISABLED — no allocation counts, wall-clock only", label),
        }
    }

    // ── U3: push ─────────────────────────────────────────────────────────────

    #[test]
    #[ignore]
    fn bench_u32v_push_1m() {
        const N: i64 = 1_000_000;

        // Pass 1 — TIMED. Nothing else touches the store, so the figure is the
        // call plus the push.
        u32v_new(n("b:push"));
        let name = n("b:push");
        let t0 = Instant::now();
        for i in 0..N {
            u32v_push(name.clone(), i & U32_MAX_I64);
        }
        let elapsed = t0.elapsed();
        assert_eq!(u32v_len(n("b:push")), Value::Int(N));

        // Pass 2 — UNTIMED, counts capacity transitions. Polling capacity per
        // push would dominate the timing above, which is why this is separate.
        u32v_new(n("b:cap"));
        let mut reallocs = 0usize;
        let mut cap = capacity_for_bench("b:cap");
        let mut caps: Vec<usize> = Vec::new();
        for i in 0..N {
            u32v_push(n("b:cap"), i & U32_MAX_I64);
            let c = capacity_for_bench("b:cap");
            if c != cap {
                reallocs += 1;
                caps.push(c);
                cap = c;
            }
        }

        println!(
            "U3 push: n={} total_ms={:.3} ns_per_element={:.2} growth_reallocs={} final_capacity={}",
            N,
            elapsed.as_secs_f64() * 1e3,
            elapsed.as_nanos() as f64 / N as f64,
            reallocs,
            cap
        );
        println!("U3 push: capacity_sequence={:?}", caps);

        // Pass 3 — allocation count (do-5). Separate array so growth
        // reallocations are included exactly once and are not double-counted
        // against the two passes above.
        u32v_new(n("b:push:probe"));
        let probe_name = n("b:push:probe");
        let probe = alloc_probe(|| {
            for i in 0..N {
                u32v_push(probe_name.clone(), i & U32_MAX_I64);
            }
        });
        report_allocs("U3 push", N, probe);
    }

    // ── U3c: get — the number U6 turns on ────────────────────────────────────

    #[test]
    #[ignore]
    fn bench_u32v_get_1m() {
        const N: i64 = 1_000_000;

        u32v_new(n("b:get"));
        let name = n("b:get");
        for i in 0..N {
            u32v_push(name.clone(), i & U32_MAX_I64);
        }

        // Sequential sweep. Not a binary-search access pattern, but it is the
        // per-call floor: whatever a binary search pays per probe, it pays at
        // least this.
        let t0 = Instant::now();
        let mut acc: i64 = 0;
        for i in 0..N {
            match u32v_get(name.clone(), i) {
                Value::Int(v) => acc = acc.wrapping_add(v),
                other => panic!("u32v_get returned {:?}", other),
            }
        }
        let elapsed = t0.elapsed();
        assert_ne!(acc, 0, "keep the loop from being optimised away");

        println!(
            "U3c get: n={} total_ms={:.3} ns_per_call={:.2}",
            N,
            elapsed.as_secs_f64() * 1e3,
            elapsed.as_nanos() as f64 / N as f64
        );

        // Binary-search-shaped access: ~20 probes per lookup over a 1M array,
        // scattered rather than sequential, which is the pattern U6 will pay.
        const LOOKUPS: i64 = 50_000;
        let probes_per_lookup = (N as f64).log2().ceil() as i64; // 20
        let t1 = Instant::now();
        let mut acc2: i64 = 0;
        for k in 0..LOOKUPS {
            let mut lo: i64 = 0;
            let mut hi: i64 = N - 1;
            let target = (k * 7919) & U32_MAX_I64;
            while lo <= hi {
                let mid = lo + (hi - lo) / 2;
                let v = match u32v_get(name.clone(), mid) {
                    Value::Int(v) => v,
                    other => panic!("u32v_get returned {:?}", other),
                };
                acc2 = acc2.wrapping_add(v);
                if v < target % N {
                    lo = mid + 1;
                } else {
                    hi = mid - 1;
                }
            }
        }
        let elapsed_bs = t1.elapsed();
        assert_ne!(acc2, 0, "keep the loop from being optimised away");
        println!(
            "U3c get: binary_search lookups={} probes_per_lookup~{} total_ms={:.3} \
             ns_per_probe={:.2} ns_per_lookup={:.2}",
            LOOKUPS,
            probes_per_lookup,
            elapsed_bs.as_secs_f64() * 1e3,
            elapsed_bs.as_nanos() as f64 / (LOOKUPS * probes_per_lookup) as f64,
            elapsed_bs.as_nanos() as f64 / LOOKUPS as f64
        );

        let probe = alloc_probe(|| {
            let mut a: i64 = 0;
            for i in 0..N {
                if let Value::Int(v) = u32v_get(name.clone(), i) {
                    a = a.wrapping_add(v);
                }
            }
            assert_ne!(a, 0);
        });
        report_allocs("U3c get", N, probe);
    }

    // ── U3d: len ─────────────────────────────────────────────────────────────

    #[test]
    #[ignore]
    fn bench_u32v_len_1m() {
        const N: i64 = 1_000_000;

        u32v_new(n("b:len"));
        let name = n("b:len");
        for i in 0..1000 {
            u32v_push(name.clone(), i);
        }

        let t0 = Instant::now();
        let mut acc: i64 = 0;
        for _ in 0..N {
            match u32v_len(name.clone()) {
                Value::Int(v) => acc = acc.wrapping_add(v),
                other => panic!("u32v_len returned {:?}", other),
            }
        }
        let elapsed = t0.elapsed();
        assert_ne!(acc, 0, "keep the loop from being optimised away");

        println!(
            "U3d len: calls={} total_ms={:.3} ns_per_call={:.2}",
            N,
            elapsed.as_secs_f64() * 1e3,
            elapsed.as_nanos() as f64 / N as f64
        );

        let probe = alloc_probe(|| {
            let mut a: i64 = 0;
            for _ in 0..N {
                if let Value::Int(v) = u32v_len(name.clone()) {
                    a = a.wrapping_add(v);
                }
            }
            assert_ne!(a, 0);
        });
        report_allocs("U3d len", N, probe);
    }

    // ── U4: sort3 — each rotation from INSERTION ORDER (do-4) ────────────────

    #[test]
    #[ignore]
    fn bench_u32v_sort3_1m_triples_per_rotation() {
        const TRIPLES: i64 = 1_000_000;

        // The rt:B shape (readtier/lib/rt_pop_step.m1:19-27), packed (s, v, t):
        //   s = i / 4        each source has exactly 4 triples
        //   v = i % 7        SEVEN distinct verbs at any population
        //   t = i % (n / 4)  each target appears exactly 4 times
        let tcard = (TRIPLES / 4).max(1);

        // Each rotation gets its OWN array, freshly built in insertion order.
        // Phase 1 sorted rt:A over data rt:B had already reordered, which mixed
        // key cardinality with input distribution and made the contrast
        // unreadable. Every figure below starts from the same input state.
        let rotations: [(&str, i64, i64, i64, &str, i64); 3] = [
            ("rt:A(s,v,t)", 0, 1, 2, "s", TRIPLES / 4),
            ("rt:B(v,t,s)", 1, 2, 0, "v", 7),
            ("rt:C(t,s,v)", 2, 0, 1, "t", tcard),
        ];

        for (label, k0, k1, k2, key0_name, key0_card) in rotations {
            let arr = format!("b:sort:{}", label);
            let name: std::sync::Arc<str> = Arc::from(arr.as_str());
            u32v_new(name.clone());
            for i in 0..TRIPLES {
                u32v_push(name.clone(), i / 4);
                u32v_push(name.clone(), i % 7);
                u32v_push(name.clone(), i % tcard);
            }
            assert_eq!(u32v_len(name.clone()), Value::Int(TRIPLES * 3));

            let t0 = Instant::now();
            u32v_sort3(name.clone(), k0, k1, k2);
            let elapsed = t0.elapsed();

            println!(
                "U4 sort3: {} key_order=({},{},{}) key0={} key0_cardinality={} \
                 triples={} total_ms={:.3} ns_per_triple={:.2}",
                label,
                k0,
                k1,
                k2,
                key0_name,
                key0_card,
                TRIPLES,
                elapsed.as_secs_f64() * 1e3,
                elapsed.as_nanos() as f64 / TRIPLES as f64
            );

            // Verify it actually sorted — a fast wrong answer is not a result.
            let mut prev = (0u32, 0u32, 0u32);
            for r in 0..TRIPLES {
                let g = |slot: i64| match u32v_get(name.clone(), r * 3 + slot) {
                    Value::Int(v) => v as u32,
                    other => panic!("u32v_get returned {:?}", other),
                };
                let key = (g(k0), g(k1), g(k2));
                assert!(key >= prev, "{}: output not ordered at record {}", label, r);
                prev = key;
            }

            // Release the 12 MB before the next rotation builds its own.
            u32v_new(name.clone());
        }
    }
}

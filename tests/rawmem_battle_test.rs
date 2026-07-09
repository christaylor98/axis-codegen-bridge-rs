//! Concurrency stress tests for AXVERITY_MEM_FOREIGN_FNS_V1's seven raw
//! memory/atomic-cell primitives (src/runtime/rawmem.rs). Styled after
//! tests/mem_battle_test.rs. These are the tests to also run once under
//! Miri (`cargo +nightly miri test --test rawmem_battle_test`) and under
//! ASan/TSan (see plan/CLAUDE.md-adjacent notes) as a separate manual pass —
//! not wired into any sanitizer-specific CI here, since no such tooling
//! exists elsewhere in this crate today.

use axis_codegen_bridge::runtime::rawmem::{
    cell_cas_raw, cell_load_raw, cell_new_raw, mem_free_raw, mem_read_raw, mem_reserve_raw,
    mem_write_raw,
};
use axis_codegen_bridge::runtime::value::Value;
use std::collections::HashSet;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::thread;

fn i(n: i64) -> Value {
    Value::Int(n)
}
fn tup(vs: Vec<Value>) -> Value {
    Value::Tuple(vs)
}
fn int_of(v: Value) -> i64 {
    match v {
        Value::Int(n) => n,
        other => panic!("expected Int, got {:?}", other),
    }
}

// ── 1. N threads, N distinct mem regions: write/read/free concurrently ─────

#[test]
fn concurrent_distinct_regions_write_read_free_never_corrupt() {
    // Miri's interpretation overhead makes the full native iteration count
    // impractical; native cargo test already exercises the full counts
    // below. Miri's value-add here is UB/data-race detection on the same
    // code paths, which a much lower iteration count still exercises.
    #[cfg(miri)]
    const THREADS: usize = 8;
    #[cfg(miri)]
    const ROUNDS: usize = 4;
    #[cfg(not(miri))]
    const THREADS: usize = 32;
    #[cfg(not(miri))]
    const ROUNDS: usize = 200;

    let handles: Vec<_> = (0..THREADS)
        .map(|tid| {
            thread::spawn(move || {
                for round in 0..ROUNDS {
                    let cap = 64i64;
                    let reserved = mem_reserve_raw(i(cap));
                    let ptr = match reserved {
                        Value::Tuple(ref es) => int_of(es[0].clone()),
                        _ => unreachable!(),
                    };
                    let payload: Vec<u8> = (0..32)
                        .map(|k| ((tid * 7 + round * 13 + k) % 256) as u8)
                        .collect();
                    mem_write_raw(tup(vec![i(ptr), i(0), Value::Bytes(payload.clone())]));
                    let read_back = mem_read_raw(tup(vec![i(ptr), i(0), i(32)]));
                    assert_eq!(
                        read_back,
                        Value::Bytes(payload),
                        "thread {} round {}: read-back mismatch — cross-thread corruption",
                        tid,
                        round
                    );
                    mem_free_raw(tup(vec![i(ptr), i(cap)]));
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
}

// ── 2. N threads, N distinct cells: new/load/cas concurrently ──────────────

#[test]
fn concurrent_distinct_cells_new_load_cas_never_corrupt() {
    #[cfg(miri)]
    const THREADS: usize = 8;
    #[cfg(miri)]
    const ROUNDS: usize = 4;
    #[cfg(not(miri))]
    const THREADS: usize = 32;
    #[cfg(not(miri))]
    const ROUNDS: usize = 200;

    let handles: Vec<_> = (0..THREADS)
        .map(|tid| {
            thread::spawn(move || {
                for round in 0..ROUNDS {
                    let init = (tid * 1000 + round) as i64;
                    let addr = int_of(cell_new_raw(i(init)));
                    assert_eq!(int_of(cell_load_raw(i(addr))), init);

                    let new_val = init + 1;
                    assert_eq!(
                        cell_cas_raw(tup(vec![i(addr), i(init), i(new_val)])),
                        Value::Bool(true)
                    );
                    assert_eq!(int_of(cell_load_raw(i(addr))), new_val);

                    // Stale CAS against this thread's OWN now-superseded value must fail.
                    assert_eq!(
                        cell_cas_raw(tup(vec![i(addr), i(init), i(9999)])),
                        Value::Bool(false)
                    );
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
}

// ── 3. N threads racing CAS against the SAME cell: proper atomic coordination
//    (not raw same-address access — this is exactly what cell_cas_raw exists
//    to make safe) must account for every increment exactly once. ──────────

#[test]
fn concurrent_same_cell_cas_increment_loses_no_updates() {
    #[cfg(miri)]
    const THREADS: usize = 8;
    #[cfg(miri)]
    const INCREMENTS_PER_THREAD: i64 = 20;
    #[cfg(not(miri))]
    const THREADS: usize = 16;
    #[cfg(not(miri))]
    const INCREMENTS_PER_THREAD: i64 = 2_000;

    let addr = int_of(cell_new_raw(i(0)));

    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            thread::spawn(move || {
                for _ in 0..INCREMENTS_PER_THREAD {
                    loop {
                        let current = int_of(cell_load_raw(i(addr)));
                        if cell_cas_raw(tup(vec![i(addr), i(current), i(current + 1)]))
                            == Value::Bool(true)
                        {
                            break;
                        }
                    }
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(
        int_of(cell_load_raw(i(addr))),
        THREADS as i64 * INCREMENTS_PER_THREAD,
        "lost update under concurrent same-cell CAS — atomic coordination failed"
    );
}

// ── 4. Sanity: concurrent allocation/cell minting never hands back
//    overlapping or duplicate addresses (precondition for tests 1-3 above
//    actually proving anything about DISTINCT addresses). ───────────────────

#[test]
fn concurrent_minting_never_aliases() {
    #[cfg(miri)]
    const THREADS: usize = 16;
    #[cfg(not(miri))]
    const THREADS: usize = 64;

    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            thread::spawn(|| {
                let ptr = match mem_reserve_raw(i(16)) {
                    Value::Tuple(es) => int_of(es[0].clone()),
                    _ => unreachable!(),
                };
                let cell_addr = int_of(cell_new_raw(i(0)));
                (ptr, cell_addr)
            })
        })
        .collect();

    let mut mem_ranges = Vec::new();
    let mut cell_addrs = HashSet::new();
    for h in handles {
        let (ptr, cell_addr) = h.join().unwrap();
        mem_ranges.push((ptr, ptr + 16));
        assert!(cell_addrs.insert(cell_addr), "duplicate cell address {}", cell_addr);
        mem_free_raw(tup(vec![i(ptr), i(16)]));
    }
    mem_ranges.sort();
    for w in mem_ranges.windows(2) {
        assert!(w[0].1 <= w[1].0, "overlapping mem_reserve_raw regions: {:?}", w);
    }
}

// ── 5. Reference oracle: a plain Arc<AtomicI64> under the same increment
//    workload, to confirm the workload/assertion shape itself is sound
//    (isolates "my test is wrong" from "the primitive is wrong"). ──────────

#[test]
fn reference_oracle_arc_atomic_matches_expected_shape() {
    #[cfg(miri)]
    const THREADS: usize = 8;
    #[cfg(miri)]
    const INCREMENTS_PER_THREAD: i64 = 20;
    #[cfg(not(miri))]
    const THREADS: usize = 16;
    #[cfg(not(miri))]
    const INCREMENTS_PER_THREAD: i64 = 2_000;

    let counter = Arc::new(AtomicI64::new(0));
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let counter = Arc::clone(&counter);
            thread::spawn(move || {
                for _ in 0..INCREMENTS_PER_THREAD {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(counter.load(Ordering::SeqCst), THREADS as i64 * INCREMENTS_PER_THREAD);
}

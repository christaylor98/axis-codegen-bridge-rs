//! AXVERITY_MEM_FOREIGN_FNS_V1 — seven brutally minimal, self-describing-handle
//! raw memory and atomic-cell primitives.
//!
//! `cell_new_raw`/`cell_load_raw`/`cell_cas_raw` operate on a single leaked
//! `AtomicI64`, addressed by its own address (the `Int` an M1 caller holds
//! *is* the address, never a table key). `mem_reserve_raw`/`mem_write_raw`/
//! `mem_read_raw`/`mem_free_raw` operate on a raw byte allocation, addressed
//! the same way — `mem_reserve_raw` returns `(ptr, capacity)` as a plain
//! `Value::Tuple`, and every later call carries everything the bridge needs.
//! This module holds **no static shared struct, no handle table, no
//! `UnsafeCell`, no `unsafe impl Sync`** — every raw pointer used here is a
//! bare `i64` cast, scoped to one call, used for exactly one transient
//! pointer operation, then dropped. That is a materially different (and
//! sound) shape from `hotmem.rs`'s known-unsound `UnsafeCell`-wrapped
//! long-lived struct: there is no persistent Rust reference here for two
//! threads to alias against in the first place.
//!
//! ## Concurrency contract
//!
//! Concurrent calls into this module are sound as long as they target
//! **non-overlapping byte ranges of the same live allocation** (or distinct
//! allocations/cells entirely). Raw pointers (`*mut u8`/`*const u8`) carry no
//! aliasing contract in Rust's memory model — unlike `&mut T`/`&T`, they are
//! not subject to noalias/uniqueness rules — so two threads targeting
//! genuinely disjoint ranges are not racing at the hardware or LLVM level:
//! there is nothing shared to race on. `AtomicI64` is `Sync` by construction,
//! so `cell_load_raw`/`cell_cas_raw` against a live cell are sound regardless
//! of what else is concurrently touching *other* cells.
//!
//! Nothing here proves or enforces non-overlap — that is entirely the
//! caller's coordination responsibility, same posture as the two hard UB
//! obligations below:
//!
//!   1. **`mem_free_raw`'s `capacity` must exactly match** the value returned
//!      by that `ptr`'s own `mem_reserve_raw` call (Rust's allocator `Layout`
//!      must match between `alloc` and `dealloc` — this is a property of the
//!      allocator contract, not a gap to check away).
//!   2. **`mem_read_raw` must not read past what was actually written** to a
//!      region, even if the read stays within the allocation's `capacity` —
//!      reading uninitialized bytes via `slice::from_raw_parts` is UB
//!      independent of, and not caught by, any bounds check against
//!      `capacity`. Bounds-vs-capacity and bounds-vs-initialized are two
//!      distinct caller obligations.
//!
//! Concurrent calls against the SAME address/handle (e.g. one thread writing
//! while another reads or frees the same region) are, per the above, caller
//! coordination failures — not races this module tries to detect or prevent.
//!
//! All bounds/validity checking (offset+length vs. capacity) is deliberately
//! **absent** from every `_raw` fn here — that checking lives in the M1-side
//! checked-wrapper composites layered on top of these primitives, not in the
//! bridge. The argument-shape/range panics below (wrong `Value` shape,
//! negative offset/length, non-positive capacity) are ordinary defensive
//! dispatch matching every other bridge leaf's convention — they are not the
//! bounds-checking this module deliberately omits.
//!
//! Identities are `sha256(name_utf8)` — same convention as the rest of the
//! bridge leaf primitives. See `registry/axis-mem-raw.axreg`.

use std::alloc::{alloc, dealloc, Layout};
use std::ptr;
use std::sync::atomic::{AtomicI64, Ordering};

use super::value::Value;

const ALIGN: usize = 8;

fn unpack2(fn_name: &'static str, args: Value) -> (Value, Value) {
    match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("{}: expected a 2-Tuple, got {:?}", fn_name, other),
    }
}

fn unpack3(fn_name: &'static str, args: Value) -> (Value, Value, Value) {
    match args {
        Value::Tuple(es) if es.len() == 3 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("{}: expected a 3-Tuple, got {:?}", fn_name, other),
    }
}

fn as_int(fn_name: &'static str, arg_ix: usize, v: Value) -> i64 {
    match v {
        Value::Int(n) => n,
        other => panic!("{}: arg {} expected Int, got {:?}", fn_name, arg_ix, other),
    }
}

fn as_bytes(fn_name: &'static str, arg_ix: usize, v: Value) -> Vec<u8> {
    match v {
        Value::Bytes(b) => b,
        other => panic!("{}: arg {} expected Bytes, got {:?}", fn_name, arg_ix, other),
    }
}

// ── cell_new_raw / cell_load_raw / cell_cas_raw ─────────────────────────────

/// `cell_new_raw(initial: Int) -> Int`
///
/// Leak a fresh `AtomicI64` initialized to `initial`; return its address as
/// an `Int`. The returned `Int` *is* the address, not a table key — there is
/// no per-cell bookkeeping anywhere in this module.
///
/// **No `cell_free_raw` exists — cells are permanent for the process
/// lifetime, by explicit decision (Chris, 2026-07-09), not oversight.**
/// V1 names exactly `cell_new_raw`/`cell_load_raw`/`cell_cas_raw` as the
/// allowed cell surface; an eighth reclaim fn would exceed that named
/// boundary, so it was not added during
/// AXVERITY_MEM_FOREIGN_FNS_BUILD_V1. If a reclaim path is ever needed,
/// that is new scope for a future intent, not a silent addition here.
#[track_caller]
pub fn cell_new_raw(args: Value) -> Value {
    let initial = as_int("cell_new_raw", 0, args);
    let leaked: &'static AtomicI64 = Box::leak(Box::new(AtomicI64::new(initial)));
    Value::Int(leaked as *const AtomicI64 as i64)
}

/// `cell_load_raw(addr: Int) -> Int`
///
/// Atomically load the `AtomicI64` living at `addr` (as returned by a prior
/// `cell_new_raw`). Sound for concurrent calls against distinct cells by
/// construction; concurrent calls against the same `addr` as a racing
/// `cell_cas_raw` are fine too — that's exactly what `AtomicI64` is for.
/// Caller must pass an `addr` that is actually a live cell — dereferencing a
/// stale/bogus address is UB, not checked here.
#[track_caller]
pub fn cell_load_raw(args: Value) -> Value {
    let addr = as_int("cell_load_raw", 0, args);
    let cell = unsafe { &*(addr as *const AtomicI64) };
    Value::Int(cell.load(Ordering::SeqCst))
}

/// `cell_cas_raw(addr: Int, expected: Int, new: Int) -> Bool`
///
/// Atomic compare-and-swap on the `AtomicI64` at `addr`: if its current value
/// equals `expected`, store `new` and return `true`; otherwise leave it
/// unchanged and return `false`.
#[track_caller]
pub fn cell_cas_raw(args: Value) -> Value {
    let (addr, expected, new) = unpack3("cell_cas_raw", args);
    let addr = as_int("cell_cas_raw", 0, addr);
    let expected = as_int("cell_cas_raw", 1, expected);
    let new = as_int("cell_cas_raw", 2, new);
    let cell = unsafe { &*(addr as *const AtomicI64) };
    let ok = cell
        .compare_exchange(expected, new, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok();
    Value::Bool(ok)
}

// ── mem_reserve_raw / mem_write_raw / mem_read_raw / mem_free_raw ──────────

/// `mem_reserve_raw(capacity: Int) -> Value::Tuple([Int(ptr), Int(capacity)])`
///
/// Allocate a fresh `capacity`-byte, 8-byte-aligned region via the global
/// allocator; return its address and capacity as a self-describing pair.
/// This module holds no record of the allocation — the pair the caller gets
/// back is the *only* record, anywhere, that this allocation exists.
///
/// Panics if `capacity <= 0` (a zero-sized `Layout` is UB per the allocator
/// API contract — this is argument-shape validation, not bounds-checking)
/// or if the allocator returns null (OOM).
#[track_caller]
pub fn mem_reserve_raw(args: Value) -> Value {
    let capacity = as_int("mem_reserve_raw", 0, args);
    if capacity <= 0 {
        panic!("mem_reserve_raw: capacity must be > 0, got {}", capacity);
    }
    let layout = Layout::from_size_align(capacity as usize, ALIGN)
        .unwrap_or_else(|e| panic!("mem_reserve_raw({}): bad layout: {}", capacity, e));
    let ptr = unsafe { alloc(layout) };
    if ptr.is_null() {
        panic!("mem_reserve_raw({}): allocator returned null (OOM)", capacity);
    }
    Value::Tuple(vec![Value::Int(ptr as i64), Value::Int(capacity)])
}

/// `mem_write_raw(ptr: Int, offset: Int, data: Bytes) -> Unit`
///
/// Copy `data` into `[ptr+offset, ptr+offset+data.len())`. Performs **no**
/// bounds check against any capacity — offset/length correctness relative to
/// the reservation is entirely the caller's responsibility; that checking
/// lives in the M1 checked-wrapper, not here. Panics if `offset < 0`
/// (argument-shape validation: a negative offset cast to `usize` would wrap
/// to a huge value and turn into out-of-bounds pointer arithmetic).
#[track_caller]
pub fn mem_write_raw(args: Value) -> Value {
    let (ptr_v, offset_v, data_v) = unpack3("mem_write_raw", args);
    let ptr = as_int("mem_write_raw", 0, ptr_v);
    let offset = as_int("mem_write_raw", 1, offset_v);
    let data = as_bytes("mem_write_raw", 2, data_v);
    if offset < 0 {
        panic!("mem_write_raw: offset must be >= 0, got {}", offset);
    }
    unsafe {
        let dst = (ptr as *mut u8).add(offset as usize);
        ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
    }
    Value::Unit
}

/// `mem_read_raw(ptr: Int, offset: Int, length: Int) -> Bytes`
///
/// Copy `length` bytes starting at `ptr+offset` into a fresh `Bytes` value.
/// Performs **no** bounds check against any capacity, and does not (cannot)
/// verify the read range was actually written — both are caller obligations,
/// see the module doc comment. Panics if `offset < 0 || length < 0`.
#[track_caller]
pub fn mem_read_raw(args: Value) -> Value {
    let (ptr_v, offset_v, length_v) = unpack3("mem_read_raw", args);
    let ptr = as_int("mem_read_raw", 0, ptr_v);
    let offset = as_int("mem_read_raw", 1, offset_v);
    let length = as_int("mem_read_raw", 2, length_v);
    if offset < 0 || length < 0 {
        panic!(
            "mem_read_raw: offset and length must be >= 0, got offset={}, length={}",
            offset, length
        );
    }
    let bytes = unsafe {
        let src = (ptr as *const u8).add(offset as usize);
        std::slice::from_raw_parts(src, length as usize).to_vec()
    };
    Value::Bytes(bytes)
}

/// `mem_free_raw(ptr: Int, capacity: Int) -> Unit`
///
/// Deallocate the region at `ptr`. `capacity` **must** exactly match the
/// value returned by that `ptr`'s original `mem_reserve_raw` call — the
/// `Layout` passed to `dealloc` must match the `Layout` passed to `alloc`,
/// per Rust's allocator contract. A mismatched capacity is undefined
/// behavior, not a checked error.
#[track_caller]
pub fn mem_free_raw(args: Value) -> Value {
    let (ptr_v, capacity_v) = unpack2("mem_free_raw", args);
    let ptr = as_int("mem_free_raw", 0, ptr_v);
    let capacity = as_int("mem_free_raw", 1, capacity_v);
    let layout = Layout::from_size_align(capacity as usize, ALIGN)
        .unwrap_or_else(|e| panic!("mem_free_raw({}, {}): bad layout: {}", ptr, capacity, e));
    unsafe {
        dealloc(ptr as *mut u8, layout);
    }
    Value::Unit
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::thread;

    fn i(n: i64) -> Value {
        Value::Int(n)
    }

    fn tup(vs: Vec<Value>) -> Value {
        Value::Tuple(vs)
    }

    /// V1's `unacceptable`-severity risk is exactly this: `mem_free_raw`
    /// called with a capacity that doesn't match its ptr's original
    /// `mem_reserve_raw` call. This is real, deliberately-triggered
    /// undefined behavior — not a Rust panic (the allocator contract
    /// violation isn't something `std::panic` catches) — so this test is
    /// `#[ignore]`d by default (same convention as `hotmem.rs`'s
    /// `uaf_isolation_probe`) and must only be run under Miri, which is
    /// built to catch exactly this class of allocator-contract violation.
    /// Verification command:
    ///   `cargo +nightly miri test --lib rawmem::tests::mem_free_raw_mismatched_capacity_is_ub -- --ignored --exact`
    /// Expected: Miri reports an error (deallocating with a Layout that does
    /// not match the one used for allocation), proving the documented UB is
    /// real and is exactly what Miri is expected to catch — this is the
    /// adversarial proof V1's outcome test asks for, not a passing test.
    #[test]
    #[ignore = "deliberately triggers real UB (mismatched dealloc Layout) — run only under Miri"]
    fn mem_free_raw_mismatched_capacity_is_ub() {
        let reserved = mem_reserve_raw(i(16));
        let ptr_n = match reserved {
            Value::Tuple(es) => match es[0] {
                Value::Int(n) => n,
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };
        // Deliberately wrong: reserved at 16, freed as if it were 32.
        mem_free_raw(tup(vec![i(ptr_n), i(32)]));
    }

    #[test]
    fn mem_reserve_write_read_free_roundtrip() {
        let reserved = mem_reserve_raw(i(16));
        let (ptr, capacity) = match reserved {
            Value::Tuple(es) if es.len() == 2 => (es[0].clone(), es[1].clone()),
            other => panic!("expected Tuple(Int, Int), got {:?}", other),
        };
        assert_eq!(capacity, i(16));

        let ptr_n = match ptr {
            Value::Int(n) => n,
            _ => unreachable!(),
        };

        let payload = Value::Bytes(vec![1, 2, 3, 4, 5]);
        assert_eq!(
            mem_write_raw(tup(vec![i(ptr_n), i(0), payload.clone()])),
            Value::Unit
        );

        let read_back = mem_read_raw(tup(vec![i(ptr_n), i(0), i(5)]));
        assert_eq!(read_back, payload);

        assert_eq!(mem_free_raw(tup(vec![i(ptr_n), i(16)])), Value::Unit);
    }

    #[test]
    fn mem_write_read_at_nonzero_offset() {
        let reserved = mem_reserve_raw(i(32));
        let ptr_n = match reserved {
            Value::Tuple(es) => match es[0] {
                Value::Int(n) => n,
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };
        let payload = Value::Bytes(vec![9, 9, 9]);
        mem_write_raw(tup(vec![i(ptr_n), i(10), payload.clone()]));
        assert_eq!(mem_read_raw(tup(vec![i(ptr_n), i(10), i(3)])), payload);
        mem_free_raw(tup(vec![i(ptr_n), i(32)]));
    }

    #[test]
    #[should_panic(expected = "capacity must be > 0")]
    fn mem_reserve_raw_rejects_zero_capacity() {
        mem_reserve_raw(i(0));
    }

    #[test]
    #[should_panic(expected = "capacity must be > 0")]
    fn mem_reserve_raw_rejects_negative_capacity() {
        mem_reserve_raw(i(-1));
    }

    #[test]
    #[should_panic(expected = "offset must be >= 0")]
    fn mem_write_raw_rejects_negative_offset() {
        let reserved = mem_reserve_raw(i(8));
        let ptr_n = match reserved {
            Value::Tuple(es) => match es[0] {
                Value::Int(n) => n,
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };
        mem_write_raw(tup(vec![i(ptr_n), i(-1), Value::Bytes(vec![1])]));
    }

    #[test]
    #[should_panic(expected = "offset and length must be >= 0")]
    fn mem_read_raw_rejects_negative_length() {
        let reserved = mem_reserve_raw(i(8));
        let ptr_n = match reserved {
            Value::Tuple(es) => match es[0] {
                Value::Int(n) => n,
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };
        mem_read_raw(tup(vec![i(ptr_n), i(0), i(-1)]));
    }

    #[test]
    fn cell_new_load_cas_roundtrip() {
        let addr = match cell_new_raw(i(42)) {
            Value::Int(n) => n,
            other => panic!("expected Int, got {:?}", other),
        };
        assert_eq!(cell_load_raw(i(addr)), i(42));

        // CAS success: current value (42) matches expected.
        assert_eq!(cell_cas_raw(tup(vec![i(addr), i(42), i(100)])), Value::Bool(true));
        assert_eq!(cell_load_raw(i(addr)), i(100));

        // CAS failure: current value (100) does not match stale expected (42).
        assert_eq!(cell_cas_raw(tup(vec![i(addr), i(42), i(999)])), Value::Bool(false));
        assert_eq!(cell_load_raw(i(addr)), i(100));
    }

    /// Sanity precondition for the concurrency stress test in
    /// `tests/rawmem_battle_test.rs`: N threads each minting a reservation (or
    /// a cell) concurrently must get N distinct, non-overlapping addresses —
    /// if the global allocator (or `Box::leak`) ever handed back overlapping
    /// regions, every later "distinct addresses never race" claim would be
    /// vacuous.
    #[test]
    fn concurrent_reservations_and_cells_are_distinct_and_non_overlapping() {
        let handles: Vec<_> = (0..64)
            .map(|_| {
                thread::spawn(|| {
                    let reserved = mem_reserve_raw(i(8));
                    let ptr_n = match reserved {
                        Value::Tuple(es) => match es[0] {
                            Value::Int(n) => n,
                            _ => unreachable!(),
                        },
                        _ => unreachable!(),
                    };
                    let cell_addr = match cell_new_raw(i(0)) {
                        Value::Int(n) => n,
                        _ => unreachable!(),
                    };
                    (ptr_n, cell_addr)
                })
            })
            .collect();

        let mut mem_ranges = Vec::new();
        let mut cell_addrs = HashSet::new();
        for h in handles {
            let (ptr_n, cell_addr) = h.join().unwrap();
            mem_ranges.push((ptr_n, ptr_n + 8));
            assert!(cell_addrs.insert(cell_addr), "duplicate cell address {}", cell_addr);
            mem_free_raw(tup(vec![i(ptr_n), i(8)]));
        }
        mem_ranges.sort();
        for w in mem_ranges.windows(2) {
            assert!(w[0].1 <= w[1].0, "overlapping mem_reserve_raw regions: {:?}", w);
        }
    }
}

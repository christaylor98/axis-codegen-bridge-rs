//! allocprobe.rs — per-allocation counting, OFF BY DEFAULT AT COMPILE TIME.
//!
//! ## Why this file no longer installs a global allocator by default
//!
//! Introduced by AXVERITY_WRITEPATH_PERF_DECOMPOSITION_V1 as "measurement
//! scaffolding for one turn", the `CountingAlloc` below was declared
//! `#[global_allocator]` in `lib.rs` UNCONDITIONALLY — so it was the allocator
//! of every binary linking `axis_codegen_bridge`, INCLUDING the production
//! `axverity-pg_server`. AXVERITY_HOTPATH_MEASUREMENT_V1 measured what that
//! cost, and it was not small:
//!
//!   * The four `AtomicU64` statics shared ONE 32-byte cacheline. Every
//!     allocation did two lock-prefixed read-modify-writes on that single line,
//!     so every allocating thread contended with every other allocating thread
//!     on one line — textbook false sharing, except the sharing was not even
//!     false: they were genuinely the same four addresses.
//!   * `perf annotate` on `__rust_alloc`: the two `lock` instructions were
//!     63% + 36.4% of the function; `call *malloc` was 0.00%.
//!   * Its share of pg_server CPU grew 4.62% (K=1) -> 22.56% (K=16).
//!   * On the SELECT path it flattened scaling outright: 298 ops/s at 1 core,
//!     262 ops/s at 16 cores, with instruction counts per query IDENTICAL
//!     (35.7M vs 35.9M, 0.6%) but cycles per query 15.9M vs 277.7M — 17.5x.
//!     Same work, 17.5x the cycles: pure stall.
//!
//! The original design note argued that "a `#[global_allocator]` is a
//! process-wide compile-time choice — there is no way to make it conditional
//! without an extra branch on every allocation". That reasoning was wrong in
//! two ways. First, a *compile-time* feature makes it conditional with no
//! runtime branch at all (this file). Second, even the rejected runtime branch
//! — a predictable load-and-test on a hot line — would have cost incomparably
//! less than turning every uncontended allocation into a contended atomic RMW.
//!
//! ## The contract now
//!
//! Nothing here is compiled into a normal build. Enable with:
//!
//! ```text
//! cargo build --release --features allocprobe
//! ```
//!
//! and the `#[global_allocator]` in `lib.rs` comes back. With the feature off,
//! `snapshot()` returns zeros and `ENABLED` is `false` — `tsmark.rs` stamps
//! `ENABLED` into the WRITEPROBE dump header so a reader can never mistake
//! "counting was compiled out" for "nothing allocated".
//!
//! If the feature IS enabled, the counters are each padded to their own
//! cacheline (`Counter`, `#[repr(align(128))]`). False sharing on a shared line
//! was the measured defect — counting per se was not — so a future measurement
//! turn that needs these numbers pays for the atomics but not for the
//! cross-thread line ping-pong. 128 bytes, not 64: x86-64 L2 adjacent-line
//! prefetch pulls cachelines in pairs, so 64-byte padding still lets two
//! counters share a prefetch unit.

/// Whether per-allocation counting is compiled in. Stamped into the WRITEPROBE
/// dump so a zero column is never read as a measurement.
pub const ENABLED: bool = cfg!(feature = "allocprobe");

#[cfg(feature = "allocprobe")]
mod imp {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// One counter on its own cacheline pair. See the module docs: the measured
    /// defect was four counters sharing a single 32-byte line.
    #[repr(align(128))]
    struct Counter(AtomicU64);

    static ALLOC_BYTES: Counter = Counter(AtomicU64::new(0));
    static DEALLOC_BYTES: Counter = Counter(AtomicU64::new(0));
    static ALLOC_COUNT: Counter = Counter(AtomicU64::new(0));
    static DEALLOC_COUNT: Counter = Counter(AtomicU64::new(0));

    pub struct CountingAlloc;

    unsafe impl GlobalAlloc for CountingAlloc {
        #[inline]
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            ALLOC_BYTES.0.fetch_add(layout.size() as u64, Ordering::Relaxed);
            ALLOC_COUNT.0.fetch_add(1, Ordering::Relaxed);
            System.alloc(layout)
        }

        #[inline]
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            DEALLOC_BYTES.0.fetch_add(layout.size() as u64, Ordering::Relaxed);
            DEALLOC_COUNT.0.fetch_add(1, Ordering::Relaxed);
            System.dealloc(ptr, layout)
        }

        #[inline]
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            ALLOC_BYTES.0.fetch_add(layout.size() as u64, Ordering::Relaxed);
            ALLOC_COUNT.0.fetch_add(1, Ordering::Relaxed);
            System.alloc_zeroed(layout)
        }

        #[inline]
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            // Accounted as one dealloc(old-size) + one alloc(new-size).
            DEALLOC_BYTES.0.fetch_add(layout.size() as u64, Ordering::Relaxed);
            DEALLOC_COUNT.0.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.0.fetch_add(new_size as u64, Ordering::Relaxed);
            ALLOC_COUNT.0.fetch_add(1, Ordering::Relaxed);
            System.realloc(ptr, layout, new_size)
        }
    }

    #[inline]
    pub fn snapshot() -> (i64, i64, i64, i64) {
        (
            ALLOC_BYTES.0.load(Ordering::Relaxed) as i64,
            DEALLOC_BYTES.0.load(Ordering::Relaxed) as i64,
            ALLOC_COUNT.0.load(Ordering::Relaxed) as i64,
            DEALLOC_COUNT.0.load(Ordering::Relaxed) as i64,
        )
    }
}

#[cfg(feature = "allocprobe")]
pub use imp::CountingAlloc;

/// (cumulative alloc_bytes, dealloc_bytes, alloc_count, dealloc_count) as of
/// now, or `(0, 0, 0, 0)` when the `allocprobe` feature is off. Check `ENABLED`
/// before attributing meaning to the values.
#[cfg(feature = "allocprobe")]
#[inline]
pub fn snapshot() -> (i64, i64, i64, i64) {
    imp::snapshot()
}

/// See the enabled variant. Zeros, because counting is compiled out.
#[cfg(not(feature = "allocprobe"))]
#[inline]
pub fn snapshot() -> (i64, i64, i64, i64) {
    (0, 0, 0, 0)
}

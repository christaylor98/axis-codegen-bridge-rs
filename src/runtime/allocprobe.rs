//! allocprobe.rs — AXVERITY_WRITEPATH_PERF_DECOMPOSITION_V1.
//!
//! A process-wide COUNTING global allocator wrapping `System`. Every `alloc` /
//! `dealloc` / `alloc_zeroed` / `realloc` call increments a handful of
//! `AtomicU64` counters (cumulative since process start) before delegating to
//! `System`. No thread-local state, no lazy statics beyond `AtomicU64::new(0)`
//! (a `const` initializer) — this deliberately avoids the classic
//! hook-a-global-allocator hazard of touching lazily-initialized thread-local
//! storage from inside `alloc`, which can itself allocate and reenter.
//!
//! `snapshot()` reads the four counters (allocated bytes, deallocated bytes,
//! allocation count, deallocation count) as of NOW. Callers (tsmark.rs's
//! `capture_probe`) record a snapshot at every existing `ts_mark` call site;
//! the PER-STAGE delta is computed offline by diffing two consecutive
//! snapshots. Because the counters are process-wide (not per-thread), a delta
//! is only cleanly attributable to one thread's stage when no OTHER thread is
//! concurrently allocating during that window — true for single-writer runs,
//! and explicitly caveated for concurrent-writer runs in the turn's report.
//!
//! This is measurement scaffolding for one turn (same spirit as tsmark.rs
//! itself), not a shipped feature — it is compiled into every binary that
//! links `axis_codegen_bridge` (the `#[global_allocator]` declaration lives in
//! `lib.rs`), so its own overhead is measured directly (see the turn's report)
//! rather than assumed negligible.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
static DEALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
static DEALLOC_COUNT: AtomicU64 = AtomicU64::new(0);

pub struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        DEALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        DEALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        System.dealloc(ptr, layout)
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        System.alloc_zeroed(layout)
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // Accounted as one dealloc(old-size) + one alloc(new-size).
        DEALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        DEALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        System.realloc(ptr, layout, new_size)
    }
}

/// (cumulative alloc_bytes, dealloc_bytes, alloc_count, dealloc_count) as of now.
#[inline]
pub fn snapshot() -> (i64, i64, i64, i64) {
    (
        ALLOC_BYTES.load(Ordering::Relaxed) as i64,
        DEALLOC_BYTES.load(Ordering::Relaxed) as i64,
        ALLOC_COUNT.load(Ordering::Relaxed) as i64,
        DEALLOC_COUNT.load(Ordering::Relaxed) as i64,
    )
}

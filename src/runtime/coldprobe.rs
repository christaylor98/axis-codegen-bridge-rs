//! coldprobe.rs — AXVERITY_COLDREAD_DEEP_DIVE_V1 Part A instrumentation.
//!
//! GATED, DISABLEABLE, ZERO-OVERHEAD-WHEN-OFF per-stage timing of the cold-read
//! path. This is ADDITIVE instrumentation only — it changes NO production
//! behavior. When `AXVERITY_COLDPROBE` is unset (the default), `enabled()`
//! resolves once to `false` and every call site collapses to a single
//! relaxed `OnceLock` bool load with no clock reads, no allocation, and no
//! output. It is a measurement scaffold for this one turn, not a shipped feature.
//!
//! ## The disk-wait vs CPU split (the central question the turn must answer)
//!
//! For every instrumented stage we capture TWO clocks around the same region:
//!   * WALL   = `CLOCK_MONOTONIC`        (elapsed real time)
//!   * CPU    = `CLOCK_THREAD_CPUTIME_ID` (this thread's on-CPU time only)
//! Then, for that stage:
//!   * `cpu_ns`            is time the thread was actually executing on a core
//!                         (hashing, parsing, memcpy of already-resident bytes)
//!   * `wall_ns - cpu_ns`  is time the thread was OFF-CPU during the stage —
//!                         i.e. blocked (a cold `read`/`pread` waiting on the
//!                         disk) plus any scheduler delay. On an otherwise-idle
//!                         serialized benchmark host this off-CPU time is
//!                         dominated by disk-I/O wait.
//! So a stage with `wall >> cpu` is DISK-WAIT-bound and a stage with
//! `wall ~= cpu` is axVerity-CPU-bound. That is exactly the structural-vs-
//! closeable distinction Part A exists to produce.
//!
//! ## Output
//!
//! When enabled, each instrumented stage emits ONE tab-separated line per
//! invocation to stderr (the server log), captured OUTSIDE the timed region so
//! the emit cost never contaminates a stage number:
//!   `COLDPROBE\tstage=<name>\t<k=v>...`
//! The harness sums these per stage offline. Per-invocation emit keeps the
//! attribution exact without any query-boundary hook or M1 change.

use std::sync::OnceLock;

/// Read the gate ONCE per process. Unset / `0` / `off` / `false` → disabled
/// (the default, zero-overhead). `1` / `on` / `true` → enabled.
#[inline]
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        matches!(
            std::env::var("AXVERITY_COLDPROBE")
                .ok()
                .as_deref()
                .map(|s| s.to_ascii_lowercase())
                .as_deref(),
            Some("1") | Some("on") | Some("true")
        )
    })
}

#[inline]
fn clock_ns(clk: libc::clockid_t) -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid, initialized timespec; clock_gettime only writes it.
    unsafe {
        libc::clock_gettime(clk, &mut ts);
    }
    (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64)
}

/// Elapsed real time (`CLOCK_MONOTONIC`), nanoseconds.
#[inline]
pub fn wall_ns() -> u64 {
    clock_ns(libc::CLOCK_MONOTONIC)
}

/// This thread's on-CPU time (`CLOCK_THREAD_CPUTIME_ID`), nanoseconds.
#[inline]
pub fn cpu_ns() -> u64 {
    clock_ns(libc::CLOCK_THREAD_CPUTIME_ID)
}

/// Emit one probe record. `extra` is a pre-formatted `\t`-joined `k=v` tail.
/// Called only when `enabled()`; kept out of the timed region by every caller.
#[inline]
pub fn emit(stage: &str, extra: &str) {
    eprintln!("COLDPROBE\tstage={}\t{}", stage, extra);
}

//! SLICE4_V1 (AXVERITY_SLICE4_BLOCK_DURABILITY_V2) — the single flag gating the
//! whole six-change bundle (per-record block framing, per-record indexer
//! signalling, reclog removal from the INSERT path, fill-or-timeout seal,
//! ack-after-fsync dispatch, arena pre-allocation). One dial, not six, so the
//! six changes can never be independently half-enabled (they are "shipped
//! together because none is independently correct" — the intent's own words).
//!
//! `slice4_mode()`, env `AXVERITY_SLICE4_BLOCK_DURABILITY`:
//!   unset / `on` / `1` / `true`   → `"on"`  (the framed-block path — DEFAULT)
//!   `off` / `0` / `false`         → `"off"` (the legacy reclog-based path,
//!                                            byte-identical to pre-V2 — the
//!                                            preserved fallback)
//!
//! ## DEFAULT FLIPPED off -> on by AXVERITY_HOTPATH_UNBLOCK_V1, item 4
//!
//! V2 shipped this dial default-OFF with "do not flip the default; Chris
//! decides". Chris decided: AXVERITY_HOTPATH_UNBLOCK_V1 authorises the flip
//! explicitly, because leaving it off meant the singular-janitor bottleneck
//! V2 was created to fix was still what a default-configured server ran —
//! V2's own bug class, still shipping.
//!
//! Measured on a clean post-CountingAlloc-removal baseline, A/B'd within ONE
//! run on one machine state (scripts/hotpath-ab-alloc.py, INSERT, 16-worker
//! pool, fresh store per instance, variant order flipped every K):
//!
//!   K       off (was default)      on (now default)     on/off
//!   1        475.9 ops/s  1.00x     201.3 ops/s  1.00x    0.42
//!   2        472.8        0.99x     221.6        1.10x    0.47
//!   4        476.4        1.00x     437.7        2.17x    0.92
//!   8        471.8        0.99x     850.1        4.22x    1.80
//!  16        474.0        1.00x    1404.8        6.98x    2.96
//!
//! `off` is FLAT to three digits from K=1 to K=16 — the textbook shared-
//! serialization signature, and the single `pg_reclog_janitor` thread is the
//! point. `on` reaches 6.98x. The crossover is near K≈4-5.
//!
//! HONEST COST OF THE FLIP, stated rather than buried: at K=1 the new default
//! is 2.4x SLOWER (201 vs 476 ops/s). A single-connection writer pays for
//! per-record framing and a block-flush round trip that the batched reclog
//! path amortised. This is a deliberate trade of single-connection latency for
//! concurrency that actually scales; a single-connection-bound workload should
//! set `AXVERITY_SLICE4_BLOCK_DURABILITY=off`, which remains fully supported.
//!
//! Durability is not weakened by the flip: the kill-9 crash-recovery gate
//! (scripts/verify-slice4-kill9.sh) exercises this path and now exercises it as
//! the DEFAULT, with no env forcing.
//!
//! Read once per process (OnceLock-cached), the same pattern as
//! `walshard`/`fieldidx`'s env-driven dials.

use std::sync::OnceLock;

use super::value::{intern_str, Value};

fn mode() -> &'static str {
    static MODE: OnceLock<&'static str> = OnceLock::new();
    MODE.get_or_init(|| match std::env::var("AXVERITY_SLICE4_BLOCK_DURABILITY") {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            // Explicit opt-OUT is the only way to get the legacy path now; an
            // empty/unparseable value falls through to the default like unset.
            if v == "off" || v == "0" || v == "false" {
                "off"
            } else {
                "on"
            }
        }
        // AXVERITY_HOTPATH_UNBLOCK_V1 item 4 — default flipped off -> on. See
        // the module docs for the measured justification and the honest K=1 cost.
        Err(_) => "on",
    })
}

/// `slice4_mode(Unit) -> Text` — the M1-facing dial. Returns `"on"` or `"off"`.
pub fn slice4_mode(_: Value) -> Value {
    Value::Str(intern_str(mode()))
}

/// Item 5's ack-wait timeout, env `AXVERITY_SLICE4_ACK_TIMEOUT_MS`, default 0
/// (ms) — MEASURED, not assumed. Since a hotblk block is thread-local (one per
/// pg_server worker) and a worker processes one connection synchronously, a
/// request thread waiting on its OWN block's natural fill can never be woken
/// by a DIFFERENT connection's writes, and cannot fill it further itself while
/// blocked — so under the simple query protocol the timeout NEVER helps; it
/// is pure added latency before "write the delta now" (Chris's framing).
/// Confirmed empirically post-channel-sharding (8 concurrent connections,
/// 200 rows/worker): 0ms -> 820.5 rows/s, 2ms -> 696.1 rows/s, 5ms -> 582.8
/// rows/s -- monotonically worse as the timeout grows, so 0 is the measured
/// optimum for this design, not a placeholder.
fn ack_timeout_ms_raw() -> i64 {
    static MS: OnceLock<i64> = OnceLock::new();
    *MS.get_or_init(|| {
        std::env::var("AXVERITY_SLICE4_ACK_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.trim().parse::<i64>().ok())
            .filter(|n| *n >= 0)
            .unwrap_or(0)
    })
}

/// `slice4_ack_timeout_ms(Unit) -> Int`
pub fn slice4_ack_timeout_ms(_: Value) -> Value {
    Value::Int(ack_timeout_ms_raw())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_on_when_unset() {
        // AXVERITY_HOTPATH_UNBLOCK_V1 item 4 flipped this default off -> on.
        // NOTE: relies on the env var being unset in the test process; other
        // tests in this binary never set AXVERITY_SLICE4_BLOCK_DURABILITY.
        if std::env::var("AXVERITY_SLICE4_BLOCK_DURABILITY").is_err() {
            assert_eq!(mode(), "on");
        }
    }
}

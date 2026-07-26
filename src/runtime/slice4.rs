//! SLICE4_V1 (AXVERITY_SLICE4_BLOCK_DURABILITY_V2) — the single flag gating the
//! whole six-change bundle (per-record block framing, per-record indexer
//! signalling, reclog removal from the INSERT path, fill-or-timeout seal,
//! ack-after-fsync dispatch, arena pre-allocation). One dial, not six, so the
//! six changes can never be independently half-enabled (they are "shipped
//! together because none is independently correct" — the intent's own words).
//!
//! `slice4_mode()`, env `AXVERITY_SLICE4_BLOCK_DURABILITY`:
//!   unset / `off` / `0` / `false` → `"off"` (today's reclog-based path,
//!                                            byte-identical to pre-turn — the
//!                                            preserved fallback)
//!   `on` / `1` / `true`           → `"on"`  (the new framed-block path)
//! Default OFF (hard-limit: do not flip the default; Chris decides).
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
            if v == "on" || v == "1" || v == "true" {
                "on"
            } else {
                "off"
            }
        }
        Err(_) => "off",
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
    fn defaults_off_when_unset() {
        // NOTE: relies on the env var being unset in the test process; other
        // tests in this binary never set AXVERITY_SLICE4_BLOCK_DURABILITY.
        if std::env::var("AXVERITY_SLICE4_BLOCK_DURABILITY").is_err() {
            assert_eq!(mode(), "off");
        }
    }
}

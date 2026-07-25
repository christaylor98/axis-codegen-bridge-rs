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

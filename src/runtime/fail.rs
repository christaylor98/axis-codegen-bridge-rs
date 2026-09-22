//! `fail(Text)` — deliberate, in-language failure.
//!
//! WHY THIS EXISTS
//! ---------------
//! The ~520 internal `panic!`s in this crate are assertions: they say the
//! compiler emitted a well-typed call and the registry declared the right
//! signature. They are unreachable from correct M1 and no caller could act on
//! them, so they stay panics.
//!
//! This is the other thing panicking is used for — a caller deciding that a
//! pre-condition has been violated and that continuing is worse than stopping —
//! and until now M1 could not say it. That is a real gap, not a stylistic one:
//! when a bridge fn is repatriated into an M1 composite, the composite cannot
//! port the input validation the bridge fn performed, because it has no way to
//! fail. The validation was silently dropped at every such rewrite.
//!
//! `fail` closes that. It is the deliberate half of panicking, promoted to
//! something the language can call.
//!
//! NOT A SUBSTITUTE FOR Result
//! ---------------------------
//! Reach for `result_err` (`result.rs`) when a caller could plausibly handle
//! the failure — then the reason travels as data and the process survives.
//! Reach for `fail` when there is no sensible continuation: a violated
//! pre-condition, an unreachable branch, a contract the caller broke. The test
//! is whether any caller could do something other than stop.
//!
//! TYPE
//! ----
//! `fail` never returns, so it has no result type. The registry type language
//! is monomorphic and has no bottom type, so `out` is declared `Value` and the
//! VERIFIER carries the real rule: a `CIf` arm that calls `fail` is exempt from
//! branch-type agreement and the sibling arm's type becomes the `CIf`'s type
//! (`axis-lang-lab-working` `validation/core_ir.rs`, keyed on `fail`'s identity
//! the same way the `seq` pass-through is). Without that, the natural use —
//!
//!     if int_lt(n, Int(0)) { fail(Text("negative")) } else { n }
//!
//! — would be rejected as `CIfBranchDivergence`, which is exactly the shape
//! input validation wants.
//!
//! EFFECT
//! ------
//! Declared `fullIo` and `deterministic false`. Stopping the process is as
//! observable as an effect gets, and the pair keeps `fail` out of the CSE gate
//! and out of branch sinking (`EFFECT_ORDER_V1`), so a `fail` can never be
//! shared with another or moved across an effect it was written to precede.

use super::value::Value;
use super::value::get_str;

/// Stop with `msg`. Never returns.
///
/// The panic message is prefixed so a reader can tell an M1-authored stop from
/// one of this crate's internal assertions — the two mean different things and
/// want different fixes. `#[track_caller]` keeps the generated glue's call site
/// in the message, matching every other bridge fn.
#[track_caller]
pub fn fail(msg: std::sync::Arc<str>) -> Value {
    panic!("fail: {}", get_str(&msg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::value::intern_str;

    #[test]
    #[should_panic(expected = "fail: negative input")]
    fn fail_stops_with_the_callers_message() {
        fail(intern_str("negative input"));
    }

    /// The prefix is part of the contract: it is what separates a deliberate
    /// in-language stop from this crate's internal assertions.
    #[test]
    fn panic_message_is_prefixed() {
        let p = std::panic::catch_unwind(|| fail(intern_str("boom"))).unwrap_err();
        let msg = p
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
            .expect("string panic payload");
        assert!(msg.starts_with("fail: "), "got {:?}", msg);
        assert!(msg.contains("boom"));
    }
}

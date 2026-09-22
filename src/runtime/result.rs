//! Untyped Result over `Value` — a failure that carries its reason.
//!
//! WHY UNTYPED, AND WHY THIS IS NOT THE OLD `ResultText`
//! ----------------------------------------------------
//! `ResultText` / `ResultBytes` / `ResultUnit` were registry-level `sum` types,
//! one per payload type, and they were removed
//! (`IS_REMOVE_RESULT_TYPES_v0.1.md`). Two things killed them: the type system
//! has no generic `Result<T>`, so the set grew one entry per payload; and every
//! call site unwrapped immediately, so the `Err` arm was dead. The migration in
//! that spec is the evidence — it is `result_text_unwrap(fs_read_text(p))`
//! becoming `fs_read_text(p)` at every site.
//!
//! This is the other encoding, the one `option.rs` already uses and
//! `int_div_checked` already ships: a `Value::Ctor` tagged `Ok` / `Err` inside
//! a plain `Value` return. No new registry type, so no per-payload family and
//! nothing for the type vocabulary to grow. It also keeps the declared return
//! type plain, which is what the "no Result wrappers" convention in CLAUDE.md
//! actually forbids — the wrapper TYPE, not a failure encoded as data.
//!
//! The price is the same one `Option` pays and
//! `axAI-axlang-gen-working/gen-working.axreg` already records: the verifier
//! cannot tell `Result(Text)` from `Result(Int)`. Both are `Value`.
//!
//! WHEN TO REACH FOR THIS RATHER THAN A PANIC OR AN OPTION
//! ------------------------------------------------------
//! A panic is right when failure is a bug and no caller could do anything with
//! it. `Option` is right when the only thing a caller needs is the FACT of
//! failure — `int_div_checked`, `list_get_at`. This is for when the caller
//! needs the REASON: which file, which parse error, which OS message. That is
//! the case `am_bless` currently gets by shelling out to a subprocess and
//! parsing `rc` plus captured stdout, because a branchable failure with a
//! message had nowhere else to come from.
//!
//! The tags are `Ok` and `Err`, matching the sum arms the removed types used,
//! so a reader coming from that vocabulary finds what they expect.

use super::value::{Value, intern_tag, get_tag_name};

/// Wrap a success value: `Ok(v)`.
#[track_caller]
pub fn result_ok(v: Value) -> Value {
    Value::Ctor { tag: intern_tag("Ok"), fields: vec![v] }
}

/// Wrap a failure reason: `Err(e)`. `e` is conventionally a `Text` message,
/// but nothing here requires that — it is whatever the caller can use.
#[track_caller]
pub fn result_err(e: Value) -> Value {
    Value::Ctor { tag: intern_tag("Err"), fields: vec![e] }
}

#[track_caller]
pub fn result_is_ok(r: Value) -> Value {
    match r {
        Value::Ctor { tag, ref fields } if get_tag_name(tag) == "Ok" && fields.len() == 1 =>
            Value::Bool(true),
        _ => Value::Bool(false),
    }
}

#[track_caller]
pub fn result_is_err(r: Value) -> Value {
    match r {
        Value::Ctor { tag, ref fields } if get_tag_name(tag) == "Err" && fields.len() == 1 =>
            Value::Bool(true),
        _ => Value::Bool(false),
    }
}

/// The `Ok` payload.
///
/// Panics on `Err`, and puts the reason in the message — which is the whole
/// reason to carry one. `option_unwrap` can only ever say "called on None".
#[track_caller]
pub fn result_unwrap(r: Value) -> Value {
    match r {
        Value::Ctor { tag, fields } if get_tag_name(tag) == "Ok" && fields.len() == 1 =>
            fields.into_iter().next().unwrap(),
        Value::Ctor { tag, fields } if get_tag_name(tag) == "Err" && fields.len() == 1 =>
            panic!("result_unwrap: called on Err({:?})", fields[0]),
        other => panic!("result_unwrap: not a result value: {:?}", other),
    }
}

/// The `Err` payload — the reason, for a caller that wants to report or match
/// on it rather than die. Panics on `Ok`, symmetrically with `result_unwrap`.
#[track_caller]
pub fn result_unwrap_err(r: Value) -> Value {
    match r {
        Value::Ctor { tag, fields } if get_tag_name(tag) == "Err" && fields.len() == 1 =>
            fields.into_iter().next().unwrap(),
        Value::Ctor { tag, .. } if get_tag_name(tag) == "Ok" =>
            panic!("result_unwrap_err: called on Ok"),
        other => panic!("result_unwrap_err: not a result value: {:?}", other),
    }
}

// ── Combinators (native multi-arg, Fn-position callback) ────────────────────
//
// M1 has no `?`. Without something here, a fallible pipeline is a branch
// ladder: every nested call needs its own `if result_is_ok(..)`, and in
// practice callers write `result_unwrap(..)` at each node instead — which
// panics, putting back exactly what the Result was for. That is not a
// prediction; `IS_REMOVE_RESULT_TYPES_v0.1.md` Part 5 records it happening to
// the old typed Results, where every call site was `result_text_unwrap(f(x))`.
//
// These chain instead, so a pipeline reads as a chain. They are the nearest
// thing to `?` that needs no grammar change — ordinary bridge fns taking a
// callback in a `Fn` slot, like `foreach` and `loop_count`.
//
// Native multi-arg Rust signature: the callee is a bare fn path the emitter
// resolves from a `Fn`-typed pool entry at translation time.
//
// EFFECT: these are declared `pure` in the registry and that is correct ONLY
// because the lowering builder now colours a higher-order call by its callback
// (core_ir_05.rs `push_node`). A `result_map` over an effectful callback is
// not shareable; over a pure one it is. Declaring them `fullIo` instead would
// be the old blunt fix and would cost CSE on every pure pipeline.

/// `result_map(r, f)` — `Ok(v)` becomes `Ok(f(v))`; `Err` passes through
/// untouched. The failure reason survives the whole chain without any call
/// site naming it.
#[track_caller]
pub fn result_map(r: Value, f: fn(Value) -> Value) -> Value {
    match r {
        Value::Ctor { tag, fields } if fields.len() == 1 => match get_tag_name(tag).as_str() {
            "Ok" => result_ok(f(fields.into_iter().next().unwrap())),
            "Err" => Value::Ctor { tag, fields },
            _ => panic!("result_map: not a result value"),
        },
        other => panic!("result_map: not a result value: {:?}", other),
    }
}

/// `result_and_then(r, f)` — `Ok(v)` becomes `f(v)`, which must itself be a
/// Result; `Err` passes through. This is the chaining step: it composes two
/// fallible operations without unwrapping between them.
///
/// `f`'s return is NOT re-wrapped, which is the whole difference from
/// `result_map`: `f` here is itself fallible and says so. A callback that
/// forgets to say so is a bug in the callback, and is rejected here rather
/// than allowed to produce a doubly-wrapped or bare value that the next stage
/// of the chain would misread.
#[track_caller]
pub fn result_and_then(r: Value, f: fn(Value) -> Value) -> Value {
    match r {
        Value::Ctor { tag, fields } if fields.len() == 1 => match get_tag_name(tag).as_str() {
            "Ok" => {
                let out = f(fields.into_iter().next().unwrap());
                match &out {
                    Value::Ctor { tag, fields }
                        if matches!(get_tag_name(*tag).as_str(), "Ok" | "Err")
                            && fields.len() == 1 => out,
                    other => panic!(
                        "result_and_then: callback must return a result value, got {:?}",
                        other
                    ),
                }
            }
            "Err" => Value::Ctor { tag, fields },
            _ => panic!("result_and_then: not a result value"),
        },
        other => panic!("result_and_then: not a result value: {:?}", other),
    }
}

/// `result_or_else(r, f)` — `Ok(v)` yields `v`; `Err(e)` yields `f(e)`.
///
/// The end of a chain: it discharges the Result into a plain value by giving
/// the caller somewhere to put the failure. This is the fn that makes the
/// whole encoding worth having rather than a slower panic — the reason reaches
/// a handler the caller wrote, in the language, without leaving the process.
#[track_caller]
pub fn result_or_else(r: Value, f: fn(Value) -> Value) -> Value {
    match r {
        Value::Ctor { tag, fields } if fields.len() == 1 => match get_tag_name(tag).as_str() {
            "Ok" => fields.into_iter().next().unwrap(),
            "Err" => f(fields.into_iter().next().unwrap()),
            _ => panic!("result_or_else: not a result value"),
        },
        other => panic!("result_or_else: not a result value: {:?}", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::value::intern_str;

    fn text(s: &str) -> Value { Value::Str(intern_str(s)) }

    #[test]
    fn ok_round_trips() {
        let r = result_ok(Value::Int(7));
        assert_eq!(result_is_ok(r.clone()), Value::Bool(true));
        assert_eq!(result_is_err(r.clone()), Value::Bool(false));
        assert_eq!(result_unwrap(r), Value::Int(7));
    }

    #[test]
    fn err_round_trips() {
        let r = result_err(text("no such file"));
        assert_eq!(result_is_err(r.clone()), Value::Bool(true));
        assert_eq!(result_is_ok(r.clone()), Value::Bool(false));
        assert_eq!(result_unwrap_err(r), text("no such file"));
    }

    /// Ok and Err are distinct tags, not two shapes of one. A Result is never
    /// mistaken for the other arm just because the payload count matches.
    #[test]
    fn ok_and_err_do_not_alias() {
        let ok = result_ok(text("x"));
        let err = result_err(text("x"));
        assert_ne!(ok, err);
        assert_eq!(result_is_ok(err.clone()), Value::Bool(false));
        assert_eq!(result_is_err(ok.clone()), Value::Bool(false));
    }

    /// The predicates answer `false` for a non-result rather than panicking, so
    /// a caller can test before it commits — the same contract `option_is_some`
    /// has.
    #[test]
    fn predicates_are_total() {
        assert_eq!(result_is_ok(Value::Int(1)), Value::Bool(false));
        assert_eq!(result_is_err(Value::Unit), Value::Bool(false));
        assert_eq!(result_is_ok(super::super::option::option_none()), Value::Bool(false));
    }

    /// The reason reaches the panic message. This is the difference from
    /// Option, so it is asserted rather than assumed.
    #[test]
    #[should_panic(expected = "no such file")]
    fn unwrap_on_err_reports_the_reason() {
        result_unwrap(result_err(text("no such file")));
    }

    #[test]
    #[should_panic(expected = "result_unwrap_err: called on Ok")]
    fn unwrap_err_on_ok_panics() {
        result_unwrap_err(result_ok(Value::Int(1)));
    }

    #[test]
    #[should_panic(expected = "not a result value")]
    fn unwrap_on_a_non_result_panics() {
        result_unwrap(Value::Int(1));
    }

    // ── combinators ────────────────────────────────────────────────────────

    fn double(v: Value) -> Value {
        match v { Value::Int(n) => Value::Int(n * 2), o => o }
    }
    fn fallible_ok(v: Value) -> Value { result_ok(v) }
    fn fallible_err(_: Value) -> Value { result_err(text("second stage failed")) }
    fn bare(_: Value) -> Value { Value::Int(0) }
    fn reason_len(e: Value) -> Value {
        match e { Value::Str(s) => Value::Int(s.len() as i64), _ => Value::Int(-1) }
    }

    #[test]
    fn map_transforms_ok_and_passes_err_through() {
        assert_eq!(result_map(result_ok(Value::Int(4)), double), result_ok(Value::Int(8)));
        let e = result_err(text("boom"));
        assert_eq!(result_map(e.clone(), double), e);
    }

    /// The reason survives an arbitrarily long chain untouched, which is the
    /// property that makes chaining worth anything.
    #[test]
    fn err_survives_a_chain_unchanged() {
        let e = result_err(text("no such file: /nope"));
        let out = result_and_then(result_map(e.clone(), double), fallible_ok);
        assert_eq!(out, e);
        assert_eq!(result_unwrap_err(out), text("no such file: /nope"));
    }

    #[test]
    fn and_then_does_not_rewrap() {
        // f already returns a Result; and_then must not produce Ok(Ok(..)).
        assert_eq!(
            result_and_then(result_ok(Value::Int(1)), fallible_ok),
            result_ok(Value::Int(1))
        );
    }

    /// A later stage can fail even though the earlier one succeeded, and its
    /// reason is what comes out.
    #[test]
    fn and_then_can_introduce_a_failure() {
        let out = result_and_then(result_ok(Value::Int(1)), fallible_err);
        assert_eq!(result_is_err(out.clone()), Value::Bool(true));
        assert_eq!(result_unwrap_err(out), text("second stage failed"));
    }

    #[test]
    #[should_panic(expected = "callback must return a result value")]
    fn and_then_rejects_a_callback_that_forgets_to_wrap() {
        result_and_then(result_ok(Value::Int(1)), bare);
    }

    /// `or_else` is the discharge point: the failure reaches a handler the
    /// caller wrote instead of killing the process.
    #[test]
    fn or_else_discharges_both_arms_to_a_plain_value() {
        assert_eq!(result_or_else(result_ok(Value::Int(9)), reason_len), Value::Int(9));
        assert_eq!(result_or_else(result_err(text("abcd")), reason_len), Value::Int(4));
    }

    /// The whole point, end to end: a failing pipeline produces a value the
    /// caller chose, and never panics.
    #[test]
    fn a_failing_pipeline_is_handled_without_panicking() {
        let start = result_err(text("open failed"));
        let out = result_or_else(
            result_and_then(result_map(start, double), fallible_ok),
            reason_len,
        );
        assert_eq!(out, Value::Int(11)); // "open failed".len()
    }

    /// An Option is not a Result: `Some` is not `Ok`. Keeping them separate is
    /// what lets a caller tell "no value" from "failed, and here is why".
    #[test]
    fn option_is_not_a_result() {
        let some = super::super::option::option_some(Value::Int(1));
        assert_eq!(result_is_ok(some.clone()), Value::Bool(false));
        assert_eq!(result_is_err(some), Value::Bool(false));
    }
}

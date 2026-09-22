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

    /// An Option is not a Result: `Some` is not `Ok`. Keeping them separate is
    /// what lets a caller tell "no value" from "failed, and here is why".
    #[test]
    fn option_is_not_a_result() {
        let some = super::super::option::option_some(Value::Int(1));
        assert_eq!(result_is_ok(some.clone()), Value::Bool(false));
        assert_eq!(result_is_err(some), Value::Bool(false));
    }
}

//! `fail(Text)` -- the program's own "I cannot continue here": an explicit Unknown, never a panic.
//!
//! FAULT_AS_UNKNOWN: Ok and Err need explicit conditions; everything else is Unknown, and Unknown must be ejected
//! explicitly. `fail` is how a program ejects it: the branch where a tag could still be "unknown" ends in
//! `fail(Text(...))` (verifier rule R2). It returns an Unknown{fail} carrying the message and call site, which is
//! sticky like any Unknown and stops later effects in the entry; a result that is Unknown exits 3.
//!
//! Declared `fullIo`, `deterministic false` in the registry, so it is never shared (CSE) or sunk across an effect,
//! and the verifier lets a `fail` arm agree with any sibling type (axis-lang-lab `fail: a diverging CIf arm ...`).

use super::value::{get_str, Value};

#[track_caller]
pub fn fail(msg: std::sync::Arc<str>) -> Value {
    let loc = std::panic::Location::caller();
    super::fault::raise(&get_str(&msg), &format!("{}:{}", loc.file(), loc.line()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{fault, value::intern_str};

    #[test]
    fn fail_is_an_explicit_unknown_carrying_the_message() {
        let v = fail(intern_str("negative input"));
        assert!(fault::is_unknown(&v));
        let d = fault::describe(&v);
        assert!(d.contains("fail") && d.contains("negative input"), "{}", d);
    }
}

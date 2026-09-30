//! FAULT_AS_UNKNOWN (prototype, fault-as-unknown-v1): a panic inside a builtin is a FAULT of the bridge -- it met
//! a case it has no classification for -- not a reason to abort the program. It becomes a value:
//!
//!   Unknown { kind, builtin, message, location }      (a reserved Ctor tag; Value itself is unchanged)
//!
//! and is sticky: a builtin given an Unknown argument is not called, the first Unknown is returned (provenance of
//! the FIRST cause survives), and a `CIf` whose condition is Unknown yields that Unknown instead of choosing a
//! branch. Every fault is also appended to $AX_FAULT_LOG (a JSON line), the bridge's work list.
use super::value::Value;
use std::cell::RefCell;
use std::sync::{Arc, Once};

/// Reserved Ctor tag for Unknown. No registry type uses it.
pub const UNKNOWN_TAG: u32 = 0xFFFF_FFF0;

thread_local! {
    static LAST_LOC: RefCell<Option<String>> = const { RefCell::new(None) };
}
static HOOK: Once = Once::new();

fn install_hook() {
    HOOK.call_once(|| {
        // Records where the panic happened (for the evidence) instead of printing it: a caught fault is data.
        std::panic::set_hook(Box::new(|info| {
            let loc = info.location().map(|l| format!("{}:{}", l.file(), l.line()));
            LAST_LOC.with(|c| *c.borrow_mut() = loc);
        }));
    });
}

fn s(x: &str) -> Value {
    Value::Str(Arc::from(x))
}

pub fn unknown(kind: &str, builtin: &str, message: &str, location: &str) -> Value {
    Value::Ctor { tag: UNKNOWN_TAG, fields: vec![s(kind), s(builtin), s(message), s(location)] }
}

pub fn is_unknown(v: &Value) -> bool {
    matches!(v, Value::Ctor { tag, .. } if *tag == UNKNOWN_TAG)
}

/// The evidence as one line: kind builtin: message (at location).
pub fn describe(v: &Value) -> String {
    match v {
        Value::Ctor { tag, fields } if *tag == UNKNOWN_TAG => {
            let f = |i: usize| match fields.get(i) {
                Some(Value::Str(x)) => x.to_string(),
                _ => String::new(),
            };
            format!("UNKNOWN {} in {}: {} (at {})", f(0), f(1), f(2), f(3))
        }
        other => format!("{:?}", other),
    }
}

fn record(builtin: &str, message: &str, location: &str) {
    if let Ok(path) = std::env::var("AX_FAULT_LOG") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let esc = |x: &str| x.replace('\\', "\\\\").replace('"', "\\\"");
            let _ = writeln!(f, "{{\"kind\":\"bridge_fault\",\"builtin\":\"{}\",\"message\":\"{}\",\"location\":\"{}\"}}",
                             esc(builtin), esc(message), esc(location));
        }
    }
}

/// Every builtin call goes through here. `args` are the call's data arguments (before any native conversion).
#[inline]
pub fn guard<F: FnOnce() -> Value>(builtin: &str, args: &[&Value], call: F) -> Value {
    for a in args {
        if is_unknown(a) {
            return (*a).clone(); // sticky: the builtin never sees it, the first cause's evidence is kept
        }
    }
    install_hook();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)) {
        Ok(v) => v,
        Err(p) => {
            let msg = p.downcast_ref::<&str>().map(|x| x.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<non-string panic payload>".to_string());
            let loc = LAST_LOC.with(|c| c.borrow_mut().take()).unwrap_or_default();
            record(builtin, &msg, &loc);
            unknown("bridge_fault", builtin, &msg, &loc)
        }
    }
}

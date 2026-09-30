//! FAULT_AS_UNKNOWN (prototype, fault-as-unknown-v1): a panic inside a builtin is a FAULT of the bridge -- it met
//! a case it has no classification for -- not a reason to abort the program. It becomes a value:
//!
//!   Unknown { kind, builtin, message, location }      (a reserved Ctor tag; Value itself is unchanged)
//!
//! and is sticky: a builtin given an Unknown argument is not called, the first Unknown is returned (provenance of
//! the FIRST cause survives), and a `CIf` whose condition is Unknown yields that Unknown instead of choosing a
//! branch. Every fault is also appended to $AX_FAULT_LOG (a JSON line), the bridge's work list.
//!
//! $AX_FAULT_MODE picks how LOUD a fault is -- never whether it is visible (there is no silent mode):
//!   abort    (bridge dev / CI) print the evidence and fail hard at once, exit 70: stop and fix the bridge
//!   unknown  (default)          the fault becomes an Unknown value, and every fault is still printed to stderr
//!                               as it happens and counted in a summary when the process ends
//!
//! $AX_FAULT_MAP is the USER's own classification, for a bridge nobody maintains any more: lines of
//! `builtin<TAB>message fragment<TAB>ErrName`. A fault matching one becomes a declared `Err{name, builtin, message}`
//! (reserved tag ERR_TAG) -- known, quiet, and handled by the program. Anything unmatched stays Unknown and loud.
use super::value::Value;
use std::cell::RefCell;
use std::sync::{Arc, Once};

/// Reserved Ctor tag for Unknown. No registry type uses it.
pub const UNKNOWN_TAG: u32 = 0xFFFF_FFF0;
/// Reserved Ctor tag for a declared Err produced by the user's fault map.
pub const ERR_TAG: u32 = 0xFFFF_FFF1;

fn classify_by_user_map(builtin: &str, message: &str) -> Option<String> {
    let path = std::env::var("AX_FAULT_MAP").ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().filter(|l| !l.trim_start().starts_with('#')).find_map(|l| {
        let f: Vec<&str> = l.split('\t').collect();
        (f.len() == 3 && f[0] == builtin && message.contains(f[1])).then(|| f[2].trim().to_string())
    })
}

thread_local! {
    static LAST_LOC: RefCell<Option<String>> = const { RefCell::new(None) };
}
static HOOK: Once = Once::new();
static FAULTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn abort_mode() -> bool {
    std::env::var("AX_FAULT_MODE").map(|m| m == "abort").unwrap_or(false)
}

extern "C" fn summary() {
    let n = FAULTS.load(std::sync::atomic::Ordering::Relaxed);
    if n > 0 {
        eprintln!("axis: {} bridge fault(s) this run -- each is a bridge bug to fix (AX_FAULT_LOG has them)", n);
    }
}
extern "C" { fn atexit(f: extern "C" fn()) -> i32; }

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

pub fn is_err(v: &Value) -> bool {
    matches!(v, Value::Ctor { tag, .. } if *tag == ERR_TAG)
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
        Value::Ctor { tag, fields } if *tag == ERR_TAG => {
            let f = |i: usize| match fields.get(i) { Some(Value::Str(x)) => x.to_string(), _ => String::new() };
            format!("ERR {} from {}: {}", f(0), f(1), f(2))
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
        if is_unknown(a) || is_err(a) {
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
            if let Some(name) = classify_by_user_map(builtin, &msg) {
                // the user named this fault: it is a declared Err now, not an unknown
                return Value::Ctor { tag: ERR_TAG, fields: vec![s(&name), s(builtin), s(&msg)] };
            }
            record(builtin, &msg, &loc);
            eprintln!("axis: BRIDGE FAULT in {}: {} (at {})", builtin, msg, loc);
            if abort_mode() {
                std::process::exit(70);                      // loud and hard: the bridge dev's mode
            }
            if FAULTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0 {
                unsafe { atexit(summary); }
            }
            unknown("bridge_fault", builtin, &msg, &loc)
        }
    }
}

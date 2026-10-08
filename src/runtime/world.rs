//! W1 WORLD LAYER (axMachina v2 codegen layer, axm-req-r99, 2026-10-09).
//!
//! Effects are slots to the outer world; the code between them stays pure. Every `w_` fn reaches the outer world
//! through one switch, chosen once per process by `AX_WORLD`:
//!
//!   real            (default) the call happens.
//!   record:<file>   the call happens, and one line is appended to <file>: fn, args, result.
//!   replay:<file>   nothing outside the process is touched. Reads and runs are answered from <file> by
//!                   (fn, args) in order of occurrence (the k-th identical call gets the k-th recorded answer); a
//!                   call the trace does not hold answers `Err:replay_miss` (false / 0 / exit -1 for the other
//!                   result kinds). Writes are not performed: each is appended to <file>.out, and a file written
//!                   during replay is read back from that write (an overlay), so a read after a write sees it.
//!
//! Results are values, never panics: `Ok:<payload>` / `Err:<reason>` for fallible Text results; `w_run` returns a
//! record (exit code, stdout, stderr). `w_get` / `w_set` are in-process state (never recorded): the kernel threads them
//! inside the pure part. Wired by `axRegistry-working/axis-bridge-world.dispatch.toml` and declared in
//! `axis-bridge-world.axreg`; a build that does not pass those files never sees these names. No legacy fn changes.
//!
//! Trace line: `fn \t args \t result`, args joined by U+001F, each field escaped (`\\`, `\t`, `\n`, `\u`). A result
//! is typed: `T:<text>`, `B:0|1`, `I:<n>`, `U:`, `R:<exit>\u{1f}<stdout>\u{1f}<stderr>`.

use super::value::{intern_str, intern_tag, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};

enum Mode {
    Real,
    Record,
    Replay,
}

struct World {
    mode: Mode,
    trace: String,
    answers: HashMap<String, Vec<String>>,
    used: HashMap<String, usize>,
    overlay: HashMap<String, String>,
}

static WORLD: OnceLock<Mutex<World>> = OnceLock::new();

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n").replace('\u{1f}', "\\u")
}

fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('u') => out.push('\u{1f}'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn key(f: &str, args: &[&str]) -> String {
    format!("{}\t{}", esc(f), esc(&args.join("\u{1f}")))
}

fn load(spec: &str) -> World {
    let (mode, trace) = match spec.split_once(':') {
        Some(("record", p)) => (Mode::Record, p.to_string()),
        Some(("replay", p)) => (Mode::Replay, p.to_string()),
        _ => (Mode::Real, String::new()),
    };
    let mut answers: HashMap<String, Vec<String>> = HashMap::new();
    if let Mode::Replay = mode {
        let text = std::fs::read_to_string(&trace).unwrap_or_default();
        for line in text.lines() {
            let mut parts = line.splitn(3, '\t');
            if let (Some(f), Some(a), Some(r)) = (parts.next(), parts.next(), parts.next()) {
                answers.entry(format!("{}\t{}", f, a)).or_default().push(unesc(r));
            }
        }
        let _ = std::fs::write(format!("{}.out", trace), "");
    }
    World { mode, trace, answers, used: HashMap::new(), overlay: HashMap::new() }
}

fn world() -> &'static Mutex<World> {
    WORLD.get_or_init(|| Mutex::new(load(&std::env::var("AX_WORLD").unwrap_or_default())))
}

/// Switch the world in-process (tests only; AI3 sets it with AX_WORLD).
pub fn world_set_mode(spec: &str) {
    *world().lock().unwrap() = load(spec);
}

fn append(path: &str, line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Read,
    Write,
    Run,
}

/// The one switch. `real` performs the call and returns its typed result; `miss` is the replay answer when the trace
/// holds none.
fn slot(f: &str, args: &[&str], kind: Kind, miss: &str, real: impl FnOnce() -> String) -> String {
    let mut w = world().lock().unwrap();
    let k = key(f, args);
    match w.mode {
        Mode::Real => {
            drop(w);
            real()
        }
        Mode::Record => {
            let trace = w.trace.clone();
            drop(w);
            let r = real();
            append(&trace, &format!("{}\t{}\n", k, esc(&r)));
            r
        }
        Mode::Replay => {
            let n = *w.used.get(&k).unwrap_or(&0);
            let recorded = w.answers.get(&k).and_then(|v| v.get(n)).cloned();
            w.used.insert(k.clone(), n + 1);
            let r = recorded.unwrap_or_else(|| miss.to_string());
            if kind == Kind::Write {
                let out = format!("{}.out", w.trace);
                append(&out, &format!("{}\t{}\n", k, esc(&r)));
            }
            r
        }
    }
}

fn is_replay() -> bool {
    matches!(world().lock().unwrap().mode, Mode::Replay)
}

fn overlay_get(path: &str) -> Option<String> {
    world().lock().unwrap().overlay.get(path).cloned()
}

fn overlay_put(path: &str, content: String) {
    world().lock().unwrap().overlay.insert(path.to_string(), content);
}

fn text(r: &str) -> Value {
    Value::Str(intern_str(r.strip_prefix("T:").unwrap_or(r)))
}

fn io_err(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "T:Err:missing".to_string(),
        std::io::ErrorKind::PermissionDenied => "T:Err:denied".to_string(),
        other => format!("T:Err:{:?}", other).to_lowercase(),
    }
}

// ---- reads ----

/// `w_read(path: Text) -> Text` — `Ok:<content>` | `Err:missing` | `Err:denied` | `Err:<kind>`.
pub fn w_read(path: Arc<str>) -> Value {
    if let Some(c) = if is_replay() { overlay_get(&path) } else { None } {
        return Value::Str(intern_str(&format!("Ok:{}", c)));
    }
    text(&slot("w_read", &[&path], Kind::Read, "T:Err:replay_miss", || match std::fs::read_to_string(path.as_ref()) {
        Ok(c) => format!("T:Ok:{}", c),
        Err(e) => io_err(&e),
    }))
}

/// `w_exists(path: Text) -> Bool` — a file or a directory is there.
pub fn w_exists(path: Arc<str>) -> Value {
    if is_replay() && overlay_get(&path).is_some() {
        return Value::Bool(true);
    }
    let r = slot("w_exists", &[&path], Kind::Read, "B:0", || {
        if std::path::Path::new(path.as_ref()).exists() { "B:1".to_string() } else { "B:0".to_string() }
    });
    Value::Bool(r == "B:1")
}

/// `w_list(path: Text) -> Text` — `Ok:` + the entries sorted, one per line, a directory ending in "/" |
/// `Err:missing` | `Err:not_a_dir`.
pub fn w_list(path: Arc<str>) -> Value {
    text(&slot("w_list", &[&path], Kind::Read, "T:Err:replay_miss", || {
        let p = std::path::Path::new(path.as_ref());
        if p.is_file() {
            return "T:Err:not_a_dir".to_string();
        }
        match std::fs::read_dir(p) {
            Ok(rd) => {
                let mut names: Vec<String> = rd
                    .filter_map(|e| e.ok())
                    .map(|e| {
                        let n = e.file_name().to_string_lossy().to_string();
                        if e.path().is_dir() { format!("{}/", n) } else { n }
                    })
                    .collect();
                names.sort();
                format!("T:Ok:{}", names.join("\n"))
            }
            Err(e) => io_err(&e),
        }
    }))
}

/// `w_args(Unit) -> Text` — the program's arguments (not its own name), one per line.
pub fn w_args(_: Value) -> Value {
    text(&slot("w_args", &[], Kind::Read, "T:", || format!("T:{}", std::env::args().skip(1).collect::<Vec<_>>().join("\n"))))
}

/// `w_env(name: Text) -> Text` — the variable's value, "" when unset.
pub fn w_env(name: Arc<str>) -> Value {
    text(&slot("w_env", &[&name], Kind::Read, "T:", || format!("T:{}", std::env::var(name.as_ref()).unwrap_or_default())))
}

/// `w_now(Unit) -> Int` — wall-clock nanoseconds since the Unix epoch.
pub fn w_now(_: Value) -> Value {
    let r = slot("w_now", &[], Kind::Read, "I:0", || {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        format!("I:{}", n.min(i64::MAX as u128))
    });
    Value::Int(r.strip_prefix("I:").and_then(|s| s.parse().ok()).unwrap_or(0))
}

// ---- writes ----

/// `w_write(path: Text, content: Text) -> Text` — `Ok:` | `Err:<reason>`.
pub fn w_write(path: Arc<str>, content: Arc<str>) -> Value {
    if is_replay() {
        overlay_put(&path, content.to_string());
    }
    text(&slot("w_write", &[&path, &content], Kind::Write, "T:Ok:", || match std::fs::write(path.as_ref(), content.as_ref()) {
        Ok(()) => "T:Ok:".to_string(),
        Err(e) => io_err(&e),
    }))
}

/// `w_append(path: Text, content: Text) -> Text` — `Ok:` | `Err:<reason>`; creates the file when missing.
pub fn w_append(path: Arc<str>, content: Arc<str>) -> Value {
    if let Some(c) = if is_replay() { overlay_get(&path) } else { None } {
        overlay_put(&path, format!("{}{}", c, content));
    }
    text(&slot("w_append", &[&path, &content], Kind::Write, "T:Ok:", || {
        match std::fs::OpenOptions::new().create(true).append(true).open(path.as_ref()) {
            Ok(mut f) => match f.write_all(content.as_bytes()) {
                Ok(()) => "T:Ok:".to_string(),
                Err(e) => io_err(&e),
            },
            Err(e) => io_err(&e),
        }
    }))
}

/// `w_mkdir(path: Text) -> Text` — the directory and its parents; `Ok:` | `Err:<reason>`.
pub fn w_mkdir(path: Arc<str>) -> Value {
    text(&slot("w_mkdir", &[&path], Kind::Write, "T:Ok:", || match std::fs::create_dir_all(path.as_ref()) {
        Ok(()) => "T:Ok:".to_string(),
        Err(e) => io_err(&e),
    }))
}

/// `w_print(text: Text) -> Unit` — to stdout, no newline added.
pub fn w_print(s: Arc<str>) -> Value {
    slot("w_print", &[&s], Kind::Write, "U:", || {
        print!("{}", s);
        let _ = std::io::stdout().flush();
        "U:".to_string()
    });
    Value::Unit
}

/// `w_eprint(text: Text) -> Unit` — to stderr, no newline added.
pub fn w_eprint(s: Arc<str>) -> Value {
    slot("w_eprint", &[&s], Kind::Write, "U:", || {
        eprint!("{}", s);
        "U:".to_string()
    });
    Value::Unit
}

/// `w_exit(code: Int) -> Unit` — ends the process with the code (recorded first; in replay too).
pub fn w_exit(code: i64) -> Value {
    let c = code.to_string();
    slot("w_exit", &[&c], Kind::Write, "U:", || "U:".to_string());
    let _ = std::io::stdout().flush();
    std::process::exit(code as i32);
}

// ---- runs ----

/// `w_run(command: Text) -> Value` — `/bin/sh -c command`; a record (exit code, stdout, stderr). Exit -1 when it could
/// not start (stderr says why), -2 when a signal ended it.
pub fn w_run(cmd: Arc<str>) -> Value {
    let r = slot("w_run", &[&cmd], Kind::Run, "R:-1\u{1f}\u{1f}replay_miss", || {
        match std::process::Command::new("/bin/sh").arg("-c").arg(cmd.as_ref()).output() {
            Ok(o) => format!(
                "R:{}\u{1f}{}\u{1f}{}",
                o.status.code().unwrap_or(-2),
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            ),
            Err(e) => format!("R:-1\u{1f}\u{1f}{}", e),
        }
    });
    let body = r.strip_prefix("R:").unwrap_or(&r);
    let mut f = body.splitn(3, '\u{1f}');
    let code = f.next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(-1);
    let out = f.next().unwrap_or("");
    let err = f.next().unwrap_or("");
    Value::Ctor {
        tag: intern_tag("Value"),
        fields: vec![Value::Int(code), Value::Str(intern_str(out)), Value::Str(intern_str(err))],
    }
}

// ---- state: in-process, never recorded ----

thread_local! {
    static STATE: RefCell<HashMap<String, HashMap<String, String>>> = RefCell::new(HashMap::new());
}

/// `w_get(map: Text, key: Text) -> Text` — "" when unset.
pub fn w_get(map: Arc<str>, k: Arc<str>) -> Value {
    let v = STATE.with(|s| s.borrow().get(map.as_ref()).and_then(|m| m.get(k.as_ref())).cloned().unwrap_or_default());
    Value::Str(intern_str(&v))
}

/// `w_set(map: Text, key: Text, value: Text) -> Unit`.
pub fn w_set(map: Arc<str>, k: Arc<str>, v: Arc<str>) -> Value {
    STATE.with(|s| {
        s.borrow_mut().entry(map.to_string()).or_default().insert(k.to_string(), v.to_string());
    });
    Value::Unit
}

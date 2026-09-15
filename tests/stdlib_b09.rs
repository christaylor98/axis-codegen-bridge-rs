//! stdlib(B09-T): io / fs / env completions (AXIS_STDLIB_DESIGN_V1).
//!
//! Written from the B09 spec table BEFORE reading the A-stage implementation
//! (T-stage rule: expectations come from the spec, not the code). Eight new
//! `fullIo` leaf fns:
//!   - io_eprintln(Value) -> Unit        — io_eprint plus a trailing newline, flushed
//!   - io_read_all(Unit) -> Text         — read stdin to EOF as UTF-8; panics on invalid UTF-8
//!   - fs_read_lines(Text) -> TextList   — fs_read_text then str_lines semantics; panics on read error
//!   - fs_remove_file(Text) -> Unit      — delete a regular file; panics if missing or on error
//!   - fs_is_dir(Text) -> Bool           — path exists and is a directory
//!   - env_get(Text) -> Text             — value of the variable; panics if unset
//!   - env_has(Text) -> Bool             — variable is set
//!   - env_get_or(Text, Text) -> Text    — value of the variable, or the fallback if unset
//!
//! `io_read_all` reads the real process stdin (fd 0), which the test harness
//! does not close or feed — an unguarded call hangs the whole run (verified).
//! Tests redirect fd 0 to a controlled tempfile via `libc::dup2` for the
//! duration of the call and restore it via a `Drop` guard so a panicking
//! `#[should_panic]` case still restores stdin. This is safe: `std::io::stdin()`
//! reads the raw OS fd directly and is not intercepted by the libtest output
//! capture layer (unlike stdout/stderr — see below).
//!
//! `io_eprintln` cannot be verified by redirecting fd 2 the same way: libtest's
//! output-capture layer intercepts `eprint!`/`eprintln!` before the write ever
//! reaches the OS file descriptor (verified empirically — a `dup2`-redirected
//! fd 2 sees nothing; the text shows up in the *captured test stdout* instead).
//! So the exact-bytes assertion runs in a subprocess: a helper `#[test]` that
//! only fires under an env-var flag calls `io_eprintln` directly, and the real
//! test re-execs the test binary with `--exact <helper> --nocapture`, which
//! restores raw-fd semantics in the child and lets `Command::output()` capture
//! the true bytes.
//!
//! All fs/env tests use per-test `TempDir`s or an env-var/fd `Drop` guard so
//! parallel test threads (which share one process's fd table and environment)
//! do not interfere with each other.

use axis_codegen_bridge::runtime::io::{
    fs_is_dir, fs_read_lines, fs_read_text, fs_remove_file, fs_write_text, io_eprintln,
    io_read_all,
};
use axis_codegen_bridge::runtime::process::{env_get, env_get_or, env_has};
use axis_codegen_bridge::runtime::str_ops::str_lines;
use axis_codegen_bridge::runtime::value::{get_str, intern_str, Value};

use std::io::{Seek, SeekFrom, Write};
use std::os::unix::io::AsRawFd;
use std::process::Command;
use std::sync::Mutex;

fn t(s: &str) -> Value {
    Value::Str(intern_str(s))
}

fn as_text(v: &Value) -> String {
    match v {
        Value::Str(s) => get_str(s),
        other => panic!("expected Text, got {:?}", other),
    }
}

// ── fd 0 (stdin) redirection for io_read_all ────────────────────────────────

static STDIN_LOCK: Mutex<()> = Mutex::new(());

struct FdGuard {
    fd: i32,
    saved: i32,
}

impl FdGuard {
    fn redirect_to(fd: i32, new_fd: i32) -> Self {
        let saved = unsafe { libc::dup(fd) };
        assert!(saved >= 0, "dup({}) failed", fd);
        let rc = unsafe { libc::dup2(new_fd, fd) };
        assert_eq!(rc, fd, "dup2 onto fd {} failed", fd);
        FdGuard { fd, saved }
    }
}

impl Drop for FdGuard {
    fn drop(&mut self) {
        unsafe {
            libc::dup2(self.saved, self.fd);
            libc::close(self.saved);
        }
    }
}

/// Redirects real stdin to `content` for the duration of `io_read_all`'s call,
/// then restores it. Serialized against other stdin-redirecting tests via
/// `STDIN_LOCK`, since fd 0 is process-wide, not per-thread.
fn read_all_with_stdin(content: &[u8]) -> Value {
    let _lock = STDIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut tmp = tempfile::tempfile().expect("tempfile");
    tmp.write_all(content).expect("write tmp stdin content");
    tmp.seek(SeekFrom::Start(0)).expect("seek tmp stdin");
    let _guard = FdGuard::redirect_to(0, tmp.as_raw_fd());
    io_read_all(Value::Unit)
}

// ── io_eprintln ──────────────────────────────────────────────────────────

const EPRINTLN_HELPER_ENV: &str = "AXIS_STDLIB_B09_EPRINTLN_HELPER";

/// Not a real test on its own: only emits to stderr when re-exec'd by
/// `io_eprintln_appends_newline_and_flushes` with the helper env var set and
/// `--exact ... --nocapture`, so its stderr reaches the parent's piped
/// `Command::output()` uncaptured by libtest.
#[test]
fn io_eprintln_helper_process() {
    if std::env::var(EPRINTLN_HELPER_ENV).is_ok() {
        io_eprintln(t("hello"));
        io_eprintln(Value::Int(42));
        io_eprintln(Value::Bool(true));
    }
}

#[test]
fn io_eprintln_appends_newline_and_flushes() {
    let exe = std::env::current_exe().expect("current_exe");
    let out = Command::new(exe)
        .args(["--exact", "io_eprintln_helper_process", "--nocapture"])
        .env(EPRINTLN_HELPER_ENV, "1")
        .output()
        .expect("spawn self as subprocess");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(stderr, "hello\n42\ntrue\n");
}

#[test]
fn io_eprintln_returns_unit() {
    assert_eq!(io_eprintln(t("side-effect-only")), Value::Unit);
}

// ── io_read_all ──────────────────────────────────────────────────────────

#[test]
fn io_read_all_reads_full_multiline_content_to_eof() {
    let v = read_all_with_stdin(b"line one\nline two\nline three (no trailing newline)");
    assert_eq!(v, t("line one\nline two\nline three (no trailing newline)"));
}

#[test]
fn io_read_all_empty_stdin_is_empty_text() {
    let v = read_all_with_stdin(b"");
    assert_eq!(v, t(""));
}

#[test]
#[should_panic]
fn io_read_all_invalid_utf8_panics() {
    read_all_with_stdin(&[0xff, 0xfe, 0xfd]);
}

// ── fs_read_lines ────────────────────────────────────────────────────────

#[test]
fn fs_read_lines_matches_fs_read_text_then_str_lines() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("lines.txt");
    let path_str = path.to_str().unwrap();
    std::fs::write(&path, "alpha\nbeta\ngamma\n").expect("seed file");

    let text = fs_read_text(intern_str(path_str));
    let text_arc = match &text {
        Value::Str(s) => s.clone(),
        other => panic!("fs_read_text returned non-Text: {:?}", other),
    };
    let expected = str_lines(text_arc);

    assert_eq!(fs_read_lines(intern_str(path_str)), expected);
    assert_eq!(
        fs_read_lines(intern_str(path_str)),
        Value::List(vec![t("alpha"), t("beta"), t("gamma")])
    );
}

#[test]
fn fs_read_lines_no_trailing_newline_still_yields_last_line() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("no_trailing_nl.txt");
    let path_str = path.to_str().unwrap();
    std::fs::write(&path, "only-line-no-newline").expect("seed file");

    assert_eq!(
        fs_read_lines(intern_str(path_str)),
        Value::List(vec![t("only-line-no-newline")])
    );
}

#[test]
fn fs_read_lines_empty_file_is_empty_list() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("empty.txt");
    let path_str = path.to_str().unwrap();
    std::fs::write(&path, "").expect("seed file");

    assert_eq!(fs_read_lines(intern_str(path_str)), Value::List(vec![]));
}

#[test]
#[should_panic]
fn fs_read_lines_missing_file_panics() {
    fs_read_lines(intern_str("/nonexistent/path/does-not-exist-b09.txt"));
}

// ── fs_remove_file ───────────────────────────────────────────────────────

#[test]
fn fs_remove_file_deletes_a_regular_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("to_remove.txt");
    let path_str = path.to_str().unwrap();
    fs_write_text(intern_str(path_str), intern_str("content"));
    assert!(path.exists());

    assert_eq!(fs_remove_file(intern_str(path_str)), Value::Unit);
    assert!(!path.exists());
}

#[test]
#[should_panic]
fn fs_remove_file_missing_file_panics() {
    fs_remove_file(intern_str("/nonexistent/path/does-not-exist-b09.txt"));
}

// ── fs_is_dir ────────────────────────────────────────────────────────────

#[test]
fn fs_is_dir_true_for_a_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert_eq!(
        fs_is_dir(intern_str(dir.path().to_str().unwrap())),
        Value::Bool(true)
    );
}

#[test]
fn fs_is_dir_false_for_a_regular_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("plain_file.txt");
    let path_str = path.to_str().unwrap();
    fs_write_text(intern_str(path_str), intern_str("x"));

    assert_eq!(fs_is_dir(intern_str(path_str)), Value::Bool(false));
}

#[test]
fn fs_is_dir_false_for_a_nonexistent_path() {
    assert_eq!(
        fs_is_dir(intern_str("/nonexistent/path/does-not-exist-b09")),
        Value::Bool(false)
    );
}

// ── env_get / env_has / env_get_or ──────────────────────────────────────

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvVarGuard {
    name: String,
    previous: Option<String>,
}

impl EnvVarGuard {
    fn set(name: &str, value: &str) -> Self {
        let previous = std::env::var(name).ok();
        std::env::set_var(name, value);
        EnvVarGuard {
            name: name.to_string(),
            previous,
        }
    }

    fn unset(name: &str) -> Self {
        let previous = std::env::var(name).ok();
        std::env::remove_var(name);
        EnvVarGuard {
            name: name.to_string(),
            previous,
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(v) => std::env::set_var(&self.name, v),
            None => std::env::remove_var(&self.name),
        }
    }
}

#[test]
fn env_get_returns_value_of_set_variable() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _guard = EnvVarGuard::set("AXIS_STDLIB_B09_TEST_VAR", "test-value-123");

    assert_eq!(
        env_get(intern_str("AXIS_STDLIB_B09_TEST_VAR")),
        t("test-value-123")
    );
}

#[test]
#[should_panic]
fn env_get_unset_variable_panics() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _guard = EnvVarGuard::unset("AXIS_STDLIB_B09_TEST_VAR_UNSET");

    env_get(intern_str("AXIS_STDLIB_B09_TEST_VAR_UNSET"));
}

#[test]
fn env_has_true_for_set_variable() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _guard = EnvVarGuard::set("AXIS_STDLIB_B09_TEST_VAR", "present");

    assert_eq!(
        env_has(intern_str("AXIS_STDLIB_B09_TEST_VAR")),
        Value::Bool(true)
    );
}

#[test]
fn env_has_false_for_unset_variable() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _guard = EnvVarGuard::unset("AXIS_STDLIB_B09_TEST_VAR_UNSET");

    assert_eq!(
        env_has(intern_str("AXIS_STDLIB_B09_TEST_VAR_UNSET")),
        Value::Bool(false)
    );
}

#[test]
fn env_get_or_returns_variable_value_when_set() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _guard = EnvVarGuard::set("AXIS_STDLIB_B09_TEST_VAR", "actual-value");

    assert_eq!(
        as_text(&env_get_or(
            intern_str("AXIS_STDLIB_B09_TEST_VAR"),
            intern_str("dflt")
        )),
        "actual-value"
    );
}

#[test]
fn env_get_or_returns_fallback_when_unset() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _guard = EnvVarGuard::unset("AXIS_STDLIB_B09_TEST_VAR_UNSET");

    assert_eq!(
        env_get_or(intern_str("AXIS_STDLIB_B09_TEST_VAR_UNSET"), intern_str("dflt")),
        t("dflt")
    );
}

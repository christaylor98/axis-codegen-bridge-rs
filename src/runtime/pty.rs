//! BRIDGE_PTY_V1 — a generic pseudo-terminal capability
//! (IS_BRIDGE_PTY_PRIMITIVES_v0.1.md).
//!
//!   * `pty_open(program: Text, argv: TextList, rows: Int, cols: Int) -> Int`
//!   * `pty_read(handle: Int, timeout_ms: Int) -> Bytes`
//!   * `pty_write(handle: Int, data: Bytes) -> Unit`
//!   * `pty_resize(handle: Int, rows: Int, cols: Int) -> Unit`
//!   * `pty_status(handle: Int) -> Int`
//!   * `pty_close(handle: Int) -> Int`
//!
//! Start any program as the session leader of a fresh pty and drive it. The
//! shape is proc_run's (process.rs) and tcp_*'s (net.rs): synchronous,
//! blocking `fullIo` leaves, integer handles, plain returns. Nothing here
//! knows what program is being driven — no expect/match, no parsing, no env or
//! cwd control. Those are decisions, and decisions live in M1/AI3.
//!
//! ## Telling "nothing yet" from "gone"
//!
//! `pty_read` returns empty Bytes both when nothing arrived within the timeout
//! and when the child side is closed (Linux reports EIO on the master). The
//! two are told apart by `pty_status`, never by the read: a polling loop stops
//! on `empty && pty_status(h) != PTY_RUNNING`. It cannot spin forever on a dead
//! child, and never mistakes a slow child for a dead one.
//!
//! ## Status bands
//!
//! proc_run's, verbatim, plus one disjoint value:
//!
//! ```text
//!      0 ..= 255   exited; this is its exit code
//!     -1 ..= -64   killed by a signal; this is -signum
//!          -256    could not be started (pty_open only)
//!          -257    ended with no reason reported (unreachable on unix)
//!          -258    still running (pty_status only)
//! ```

use std::collections::HashMap;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use super::process::{terminating_signal, NO_START};
use super::value::{get_str, Value};

/// `pty_status`'s "the child has not ended". Disjoint from every proc_run band.
pub const PTY_RUNNING: i64 = -258;

/// Most bytes one `pty_read` returns.
const READ_CHUNK: usize = 64 * 1024;

/// How long `pty_close` waits after the hangup before it kills.
const CLOSE_GRACE: Duration = Duration::from_millis(1000);

struct Pty {
    master: OwnedFd,
    pid: libc::pid_t,
    child: Mutex<Child>,
    /// The final status once reaped; a child is reaped once.
    ended: Mutex<Option<i64>>,
}

fn table() -> MutexGuard<'static, HashMap<i64, Arc<Pty>>> {
    static T: OnceLock<Mutex<HashMap<i64, Arc<Pty>>>> = OnceLock::new();
    // Recover from poisoning: every panic in this module fires after its map
    // operation has completed, so the map is never left half-mutated (net.rs's
    // registry() makes the same argument).
    T.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|e| e.into_inner())
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

static NEXT: AtomicI64 = AtomicI64::new(1);

/// The entry for `handle`, the table lock released before the caller does
/// (possibly blocking) I/O on it. Panics on an unknown handle: that is a bug.
fn get(handle: i64, who: &str) -> Arc<Pty> {
    match table().get(&handle) {
        Some(p) => p.clone(),
        None => panic!("{}: unknown pty handle {}", who, handle),
    }
}

fn winsize(rows: i64, cols: i64, who: &str) -> libc::winsize {
    let ok = |n: i64| (0..=u16::MAX as i64).contains(&n);
    if !ok(rows) || !ok(cols) {
        panic!("{}: rows and cols must be 0..=65535, got {}x{}", who, rows, cols);
    }
    libc::winsize { ws_row: rows as u16, ws_col: cols as u16, ws_xpixel: 0, ws_ypixel: 0 }
}

fn os_err(who: &str, what: &str) -> ! {
    panic!("{}: {}: {}", who, what, std::io::Error::last_os_error())
}

/// A fresh master/slave pair, both close-on-exec, the slave not yet anyone's
/// controlling terminal.
fn open_pair(ws: &libc::winsize) -> (OwnedFd, OwnedFd) {
    unsafe {
        let m = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        if m < 0 {
            os_err("pty_open", "posix_openpt");
        }
        let master = OwnedFd::from_raw_fd(m);
        if libc::grantpt(m) != 0 {
            os_err("pty_open", "grantpt");
        }
        if libc::unlockpt(m) != 0 {
            os_err("pty_open", "unlockpt");
        }
        let mut name = [0 as libc::c_char; 128];
        if libc::ptsname_r(m, name.as_mut_ptr(), name.len()) != 0 {
            os_err("pty_open", "ptsname_r");
        }
        let s = libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        if s < 0 {
            os_err("pty_open", "open slave");
        }
        let slave = OwnedFd::from_raw_fd(s);
        if (ws.ws_row, ws.ws_col) != (0, 0) && libc::ioctl(m, libc::TIOCSWINSZ, ws) != 0 {
            os_err("pty_open", "TIOCSWINSZ");
        }
        (master, slave)
    }
}

/// `pty_open(program: Text, argv: TextList, rows: Int, cols: Int) -> Int`
///
/// Start `program` with `argv` (the exact list; argv[0] is not included, no
/// shell) on a fresh pty sized `rows` x `cols` (0 x 0 leaves the kernel
/// default). The pty slave is its stdin, stdout and stderr, and its controlling
/// terminal in a new session. Environment and cwd are inherited. Returns a
/// handle >= 1, or -256 when the program could not be started.
#[track_caller]
pub fn pty_open(program: Arc<str>, argv: Value, rows: i64, cols: i64) -> Value {
    let args: Vec<String> = match argv {
        Value::List(items) => items
            .iter()
            .map(|v| match v {
                Value::Str(h) => get_str(h),
                other => panic!("pty_open: argv element must be Text, got {:?}", other),
            })
            .collect(),
        Value::Unit => Vec::new(),
        other => panic!("pty_open: argv must be a TextList, got {:?}", other),
    };
    let ws = winsize(rows, cols, "pty_open");
    let (master, slave) = open_pair(&ws);
    let dup = |fd: &OwnedFd| -> Stdio {
        Stdio::from(fd.try_clone().unwrap_or_else(|e| panic!("pty_open: dup slave: {}", e)))
    };

    let mut cmd = Command::new(program.as_ref());
    cmd.args(&args).stdin(dup(&slave)).stdout(dup(&slave)).stderr(dup(&slave));
    unsafe {
        // Runs in the child after stdio is in place: lead a new session and
        // take the slave (now fd 0) as its controlling terminal.
        cmd.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return Value::Int(NO_START),
    };
    drop(slave); // the parent's copies; the child holds its own

    let handle = NEXT.fetch_add(1, Ordering::Relaxed);
    let pid = child.id() as libc::pid_t;
    let pty = Pty { master, pid, child: Mutex::new(child), ended: Mutex::new(None) };
    table().insert(handle, Arc::new(pty));
    Value::Int(handle)
}

/// `pty_read(handle: Int, timeout_ms: Int) -> Bytes`
///
/// Wait up to `timeout_ms` (0 = don't wait) for output and return what is
/// available, at most 64 KiB. Empty = nothing within the timeout, or the child
/// side is closed; `pty_status` tells which. Output written before the child
/// exited is returned until drained.
#[track_caller]
pub fn pty_read(handle: i64, timeout_ms: i64) -> Value {
    if timeout_ms < 0 {
        panic!("pty_read: timeout_ms must be >= 0, got {}", timeout_ms);
    }
    let pty = get(handle, "pty_read");
    let fd = pty.master.as_raw_fd();
    let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    let timeout = timeout_ms.min(i32::MAX as i64) as libc::c_int;
    let ready = unsafe { libc::poll(&mut pfd, 1, timeout) };
    if ready <= 0 {
        // 0 = timed out; < 0 = interrupted (EINTR) -- nothing read either way
        return Value::Bytes(Vec::new());
    }
    let mut buf = vec![0u8; READ_CHUNK];
    let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
    if n > 0 {
        buf.truncate(n as usize);
        return Value::Bytes(buf);
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        // n == 0 or EIO: the child side is closed. EAGAIN/EINTR: nothing now.
        _ if n == 0 => Value::Bytes(Vec::new()),
        Some(libc::EIO) | Some(libc::EAGAIN) | Some(libc::EINTR) => Value::Bytes(Vec::new()),
        _ => panic!("pty_read({}): {}", handle, err),
    }
}

/// `pty_write(handle: Int, data: Bytes) -> Unit`
///
/// Write all of `data` to the child's input. The child side being gone (EIO)
/// is a no-op, as a vanished TCP peer is; `pty_status` reports it.
#[track_caller]
pub fn pty_write(handle: i64, data: Vec<u8>) -> Value {
    let pty = get(handle, "pty_write");
    let fd = pty.master.as_raw_fd();
    let mut rest = &data[..];
    while !rest.is_empty() {
        let n = unsafe { libc::write(fd, rest.as_ptr() as *const libc::c_void, rest.len()) };
        if n > 0 {
            rest = &rest[n as usize..];
            continue;
        }
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::EIO) => break,
            _ => panic!("pty_write({}): {}", handle, err),
        }
    }
    Value::Unit
}

/// `pty_resize(handle: Int, rows: Int, cols: Int) -> Unit`
///
/// Set the window size; the kernel sends SIGWINCH to the child's foreground
/// process group.
#[track_caller]
pub fn pty_resize(handle: i64, rows: i64, cols: i64) -> Value {
    let ws = winsize(rows, cols, "pty_resize");
    let pty = get(handle, "pty_resize");
    if unsafe { libc::ioctl(pty.master.as_raw_fd(), libc::TIOCSWINSZ, &ws) } != 0 {
        os_err("pty_resize", "TIOCSWINSZ");
    }
    Value::Unit
}

fn status_of(st: std::process::ExitStatus) -> i64 {
    match st.code() {
        Some(c) => c as i64,
        None => terminating_signal(&st),
    }
}

/// The child's status without blocking: its final one once it has ended.
fn poll_status(pty: &Pty, who: &str) -> i64 {
    let mut ended = lock(&pty.ended);
    if let Some(s) = *ended {
        return s;
    }
    match lock(&pty.child).try_wait() {
        Ok(Some(st)) => {
            let s = status_of(st);
            *ended = Some(s);
            s
        }
        Ok(None) => PTY_RUNNING,
        Err(e) => panic!("{}: wait: {}", who, e),
    }
}

/// `pty_status(handle: Int) -> Int`
///
/// Non-blocking. An exit code (0..=255), -signum (-1..=-64), or -258 while the
/// child is still running. Once the child has ended the answer is fixed.
#[track_caller]
pub fn pty_status(handle: i64) -> Value {
    Value::Int(poll_status(&get(handle, "pty_status"), "pty_status"))
}

/// `pty_close(handle: Int) -> Int`
///
/// Release the handle: hang up the child's session (SIGHUP), wait up to
/// 1000 ms for the child to end, then SIGKILL its process group and wait.
/// Returns the final status (never -258). The handle is dead afterwards.
#[track_caller]
pub fn pty_close(handle: i64) -> Value {
    let pty = match table().remove(&handle) {
        Some(p) => p,
        None => panic!("pty_close: unknown pty handle {}", handle),
    };
    let mut s = poll_status(&pty, "pty_close");
    if s == PTY_RUNNING {
        // The child leads its own session, so its pid is its process group.
        unsafe { libc::killpg(pty.pid, libc::SIGHUP) };
        let deadline = Instant::now() + CLOSE_GRACE;
        while s == PTY_RUNNING && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            s = poll_status(&pty, "pty_close");
        }
    }
    if s == PTY_RUNNING {
        unsafe { libc::killpg(pty.pid, libc::SIGKILL) };
        let st = lock(&pty.child).wait().unwrap_or_else(|e| panic!("pty_close: wait: {}", e));
        s = status_of(st);
        *lock(&pty.ended) = Some(s);
    }
    // The master fd closes when the last Arc drops -- here, unless another
    // thread is mid-read on it.
    Value::Int(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::value::intern_str;

    fn open(program: &str, args: &[&str], rows: i64, cols: i64) -> i64 {
        let argv = Value::List(args.iter().map(|a| Value::Str(intern_str(a))).collect());
        match pty_open(intern_str(program), argv, rows, cols) {
            Value::Int(h) => h,
            other => panic!("pty_open returned {:?}", other),
        }
    }

    fn sh(script: &str) -> i64 {
        open("/bin/sh", &["-c", script], 24, 80)
    }

    fn status(h: i64) -> i64 {
        match pty_status(h) {
            Value::Int(s) => s,
            other => panic!("pty_status returned {:?}", other),
        }
    }

    fn read(h: i64, ms: i64) -> Vec<u8> {
        match pty_read(h, ms) {
            Value::Bytes(b) => b,
            other => panic!("pty_read returned {:?}", other),
        }
    }

    /// The canonical loop: read until empty AND ended. Returns (output, status).
    fn drain(h: i64) -> (String, i64) {
        let mut out = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let chunk = read(h, 50);
            if chunk.is_empty() && status(h) != PTY_RUNNING {
                break;
            }
            out.extend(chunk);
            assert!(Instant::now() < deadline, "child never ended; got {:?}", String::from_utf8_lossy(&out));
        }
        (String::from_utf8_lossy(&out).into_owned(), status(h))
    }

    #[test]
    fn child_sees_window_size_and_controlling_tty() {
        let h = open("/bin/sh", &["-c", "stty size; tty >/dev/null && echo TTY"], 31, 97);
        let (out, st) = drain(h);
        assert!(out.contains("31 97"), "stty size: {:?}", out);
        assert!(out.contains("TTY"), "no controlling tty: {:?}", out);
        assert_eq!(st, 0);
        assert_eq!(pty_close(h), Value::Int(0));
    }

    #[test]
    fn exit_code_comes_back_in_its_band() {
        let h = sh("exit 7");
        assert_eq!(drain(h).1, 7);
        pty_close(h);
    }

    #[test]
    fn signal_comes_back_as_minus_signum() {
        let h = sh("kill -TERM $$");
        assert_eq!(drain(h).1, -(libc::SIGTERM as i64));
        pty_close(h);
    }

    #[test]
    fn missing_program_is_no_start_not_a_panic() {
        let argv = Value::List(vec![]);
        assert_eq!(pty_open(intern_str("/nonexistent/program"), argv, 24, 80), Value::Int(NO_START));
    }

    #[test]
    fn silent_live_child_reads_empty_after_the_timeout_and_is_running() {
        let h = sh("sleep 5");
        let t = Instant::now();
        assert!(read(h, 200).is_empty());
        let waited = t.elapsed();
        assert!(waited >= Duration::from_millis(150), "returned early: {:?}", waited);
        assert_eq!(status(h), PTY_RUNNING);
        pty_close(h);
    }

    #[test]
    fn output_written_just_before_exit_is_still_read() {
        let h = sh("printf 'last words'");
        std::thread::sleep(Duration::from_millis(300)); // the child has exited by now
        let (out, st) = drain(h);
        assert!(out.contains("last words"), "lost output: {:?}", out);
        assert_eq!(st, 0);
        pty_close(h);
    }

    #[test]
    fn write_reaches_the_child() {
        let h = sh("read line; echo \"got:$line\"");
        pty_write(h, b"hello\n".to_vec());
        let (out, st) = drain(h);
        assert!(out.contains("got:hello"), "{:?}", out);
        assert_eq!(st, 0);
        pty_close(h);
    }

    #[test]
    fn late_input_after_a_wait_is_still_received() {
        let h = sh("read line; echo \"got:$line\"");
        assert!(read(h, 300).is_empty());
        assert_eq!(status(h), PTY_RUNNING);
        pty_write(h, b"late\n".to_vec());
        let (out, _) = drain(h);
        assert!(out.contains("got:late"), "{:?}", out);
        pty_close(h);
    }

    #[test]
    fn resize_is_seen_by_the_child() {
        let h = sh("read x; stty size");
        pty_resize(h, 40, 120);
        pty_write(h, b"\n".to_vec());
        let (out, _) = drain(h);
        assert!(out.contains("40 120"), "{:?}", out);
        pty_close(h);
    }

    #[test]
    fn write_after_exit_is_a_no_op() {
        let h = sh("exit 0");
        drain(h);
        assert_eq!(pty_write(h, b"anyone?\n".to_vec()), Value::Unit);
        pty_close(h);
    }

    #[test]
    fn close_hangs_up_a_live_child() {
        let h = sh("sleep 30");
        let t = Instant::now();
        assert_eq!(pty_close(h), Value::Int(-(libc::SIGHUP as i64)));
        assert!(t.elapsed() < CLOSE_GRACE, "waited for the kill: {:?}", t.elapsed());
    }

    #[test]
    fn close_kills_a_child_that_ignores_hangup() {
        let h = sh("trap '' HUP; echo ready; while :; do sleep 1; done");
        let mut out = Vec::new();
        while !String::from_utf8_lossy(&out).contains("ready") {
            out.extend(read(h, 1000));
        }
        let t = Instant::now();
        assert_eq!(pty_close(h), Value::Int(-(libc::SIGKILL as i64)));
        assert!(t.elapsed() < CLOSE_GRACE + Duration::from_millis(500), "{:?}", t.elapsed());
    }

    #[test]
    #[should_panic(expected = "unknown pty handle")]
    fn unknown_handle_panics() {
        pty_status(i64::MAX);
    }

    #[test]
    #[should_panic(expected = "rows and cols must be 0..=65535")]
    fn out_of_range_size_panics() {
        open("/bin/true", &[], -1, 80);
    }
}

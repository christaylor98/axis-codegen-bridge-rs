//! Raw-terminal control for a full-screen keypress-driven client
//! (AXVERITY_GC_TUI_V1). D040: this is exactly the OS-level nastiness the
//! bridge owns so M1 can stay declarative — dispatch on a byte, not
//! termios/ioctl/signal plumbing.
//!
//! `tty_raw_on(vtime_tenths)` puts stdin (fd 0) into ICANON/ECHO/ISIG-off
//! raw mode with VMIN=0, VTIME=vtime_tenths. That VMIN/VTIME combination is
//! the load-bearing trick: `tty_read_key` becomes a SINGLE primitive that is
//! both the keypress reader AND the refresh-tick source — a read() that got
//! a byte returns it, a read() that timed out with no byte returns -1,
//! which the M1 fold interprets as "no key this tick, redraw anyway". No
//! second polling/timeout primitive needed.
//!
//! ISIG is turned OFF deliberately: with it on, Ctrl-C during raw mode
//! raises a real SIGINT, and terminating the process via a signal from
//! inside its own raw-mode session is exactly the "left the terminal
//! broken" failure raw-mode tools are notorious for. With ISIG off, Ctrl-C
//! is just byte 0x03 through tty_read_key — the M1 key-dispatch loop quits
//! on it the same as 'q', through the normal tty_raw_off path. SIGTERM/
//! SIGHUP (e.g. `kill` from another shell) are NOT gated by ISIG though, so
//! tty_raw_on also installs a best-effort handler that restores the saved
//! termios and _exit()s before dying — the async-signal-safety bar here is
//! the same pragmatic one unified_wait.rs's self-pipe handler already
//! accepts for this codebase (raw syscalls only, no allocation).
//!
//! Residual, accepted risk: SIGKILL cannot be intercepted by anyone, so a
//! `kill -9` during raw mode still leaves the terminal broken (as it would
//! for any raw-mode program — vim, htop, etc.); the user's remedy is the
//! same one those tools rely on: `stty sane` / `reset` from the shell.

use super::value::Value;
use std::sync::atomic::{AtomicBool, Ordering};

static RAW_ACTIVE: AtomicBool = AtomicBool::new(false);

// Written once by the main thread in `tty_raw_on`, strictly before
// `install_restore_handlers` arms the signal handler that reads it — so by
// the time any signal could invoke `restore_and_exit`, this write has
// already happened-before it (sigaction() is a full syscall boundary).
// Single-threaded caller (the gc client); never mutated concurrently.
static mut SAVED_TERMIOS: libc::termios = unsafe { std::mem::zeroed() };

extern "C" fn restore_and_exit(_signum: libc::c_int) {
    unsafe {
        libc::tcsetattr(0, libc::TCSANOW, std::ptr::addr_of!(SAVED_TERMIOS));
        libc::_exit(130);
    }
}

fn install_restore_handlers() {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = restore_and_exit as usize;
        sa.sa_flags = 0;
        libc::sigemptyset(&mut sa.sa_mask);
        for signum in [libc::SIGTERM, libc::SIGHUP] {
            libc::sigaction(signum, &sa, std::ptr::null_mut());
        }
    }
}

fn reset_default_handlers() {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = libc::SIG_DFL;
        sa.sa_flags = 0;
        libc::sigemptyset(&mut sa.sa_mask);
        for signum in [libc::SIGTERM, libc::SIGHUP] {
            libc::sigaction(signum, &sa, std::ptr::null_mut());
        }
    }
}

/// `tty_raw_on(vtime_tenths: Int) -> Unit`
#[track_caller]
pub fn tty_raw_on(n: i64) -> Value {
    let vtime = if (0..=255).contains(&n) { n as u8 } else {
        panic!("tty_raw_on: expected Int 0..=255, got {}", n)
    };
    if RAW_ACTIVE.swap(true, Ordering::SeqCst) {
        panic!("tty_raw_on: already active (missing a tty_raw_off?)");
    }
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        assert_eq!(libc::tcgetattr(0, &mut t), 0, "tty_raw_on: tcgetattr failed (not a tty?)");
        SAVED_TERMIOS = t;
        let mut raw = t;
        raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG);
        raw.c_cc[libc::VMIN] = 0;
        raw.c_cc[libc::VTIME] = vtime;
        assert_eq!(libc::tcsetattr(0, libc::TCSANOW, &raw), 0, "tty_raw_on: tcsetattr failed");
    }
    install_restore_handlers();
    Value::Unit
}

/// `tty_raw_off(Unit) -> Unit` — restores the termios `tty_raw_on` saved.
/// A no-op (not a panic) if raw mode isn't active, so an outer's
/// unconditional cleanup call is always safe to make.
#[track_caller]
pub fn tty_raw_off(_: Value) -> Value {
    if !RAW_ACTIVE.swap(false, Ordering::SeqCst) {
        return Value::Unit;
    }
    unsafe {
        libc::tcsetattr(0, libc::TCSANOW, std::ptr::addr_of!(SAVED_TERMIOS));
    }
    reset_default_handlers();
    Value::Unit
}

/// `tty_read_key(Unit) -> Int` — one raw byte from stdin (0..255), or -1 on
/// the VTIME timeout (no key pressed within `tty_raw_on`'s tick window) or
/// EOF. Bypasses Rust's buffered Stdin deliberately: a direct read(2) on
/// fd 0 is what VMIN/VTIME's tenths-of-a-second semantics apply to.
#[track_caller]
pub fn tty_read_key(_: Value) -> Value {
    let mut buf = [0u8; 1];
    let n = unsafe { libc::read(0, buf.as_mut_ptr() as *mut libc::c_void, 1) };
    if n == 1 { Value::Int(buf[0] as i64) } else { Value::Int(-1) }
}

fn winsize() -> (u16, u16) {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) == 0 {
            (ws.ws_row, ws.ws_col)
        } else {
            (0, 0)
        }
    }
}

/// `tty_rows(Unit) -> Int` — current terminal height via ioctl(TIOCGWINSZ)
/// on stdout (fd 1). 0 if not a terminal (e.g. redirected) — callers treat
/// 0 as "unknown", not a fault.
#[track_caller]
pub fn tty_rows(_: Value) -> Value {
    Value::Int(winsize().0 as i64)
}

/// `tty_cols(Unit) -> Int` — current terminal width, same convention as
/// `tty_rows`.
#[track_caller]
pub fn tty_cols(_: Value) -> Value {
    Value::Int(winsize().1 as i64)
}

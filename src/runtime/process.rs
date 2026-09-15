use super::value::{Value, intern_str, get_process_args};

#[track_caller]
pub fn proc_args(_: Value) -> Value {
    let args = get_process_args();
    Value::List(args.iter().map(|s| Value::Str(intern_str(s))).collect())
}

#[track_caller]
pub fn proc_exit(code: Value) -> Value {
    let c = match code {
        Value::Int(n) => n as i32,
        _ => 0,
    };
    std::process::exit(c);
}

#[track_caller]
pub fn proc_sleep(n: i64) -> Value {
    let secs = if n >= 0 { n as u64 } else { 0 };
    std::thread::sleep(std::time::Duration::from_secs(secs));
    Value::Unit
}

#[track_caller]
pub fn sleep(n: i64) -> Value {
    let ms = if n >= 0 { n as u64 } else { 0 };
    std::thread::sleep(std::time::Duration::from_millis(ms));
    Value::Unit
}

/// `now_unix_nanos(Unit) -> Int`
///
/// Wall-clock nanoseconds since the Unix epoch, as an Int (i64). This is the
/// clock primitive the M1 surface previously lacked: prior axVerity turns had
/// to take a `date +%s%N` timestamp from a wrapping shell script and pass it in
/// via argv (CLAUDE.md §10 — the bind/ledger log's event time). With this, an
/// M1 program (e.g. the Postgres server's INSERT->push+bind path) can stamp its
/// own ledger/name-log events natively, removing one of the last reasons a
/// write path needed a shell driver. Non-deterministic, fullIo. i64 nanoseconds
/// cover dates through ~2262, far beyond any demo horizon.
#[track_caller]
pub fn now_unix_nanos(_: Value) -> Value {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_else(|e| panic!("now_unix_nanos: system clock before Unix epoch: {}", e))
        .as_nanos();
    // as_nanos() is u128; clamp into i64 range (a real wall clock is ~1.7e18
    // now, far below i64::MAX ~9.2e18 — the min is a belt for the year-2262 tail).
    Value::Int(nanos.min(i64::MAX as u128) as i64)
}

#[track_caller]
pub fn argv(idx: Value) -> Value {
    let i = match idx { Value::Int(n) => n as usize, _ => 0 };
    let args = get_process_args();
    match args.get(i) {
        Some(s) => Value::Str(intern_str(s)),
        None => Value::Str(intern_str("")),
    }
}

#[track_caller]
pub fn argv_get(idx: i64) -> Value {
    let i = idx as usize;
    let args = get_process_args();
    match args.get(i) {
        Some(s) => Value::Str(intern_str(s)),
        None => Value::Str(intern_str("")),
    }
}

#[track_caller]
pub fn argv_int(idx: Value) -> Value {
    let i = match idx { Value::Int(n) => n as usize, _ => 0 };
    let args = get_process_args();
    match args.get(i) {
        Some(s) => Value::Int(s.parse::<i64>().unwrap_or(0)),
        None => Value::Int(0),
    }
}

#[track_caller]
pub fn argv_count(_: Value) -> Value {
    let args = get_process_args();
    Value::Int(args.len().saturating_sub(1) as i64)
}

#[track_caller]
pub fn argv_or(args_val: Value) -> Value {
    match args_val {
        Value::Tuple(ref es) if es.len() >= 2 => {
            let i = match &es[0] { Value::Int(n) => *n as usize, _ => 0 };
            let default = es[1].clone();
            let args = get_process_args();
            match args.get(i) {
                Some(s) => Value::Str(intern_str(s)),
                None => default,
            }
        }
        Value::Int(n) => {
            let args = get_process_args();
            match args.get(n as usize) {
                Some(s) => Value::Str(intern_str(s)),
                None => Value::Str(intern_str("")),
            }
        }
        _ => Value::Str(intern_str("")),
    }
}

// ── AXVERITY_SHIM_BRIDGE_PRIMS_V1 ────────────────────────────────────────────
//
// gap:axverity-shim-admit-needs-process-primitive.
//
// `proc_run(program: Text, argv: TextList) -> Int`
//
// Start `program` with `argv` as its arguments, block until it exits, return
// what the OS said about how it exited. That is the whole capability. It is the
// process analogue of `tcp_listen`/`tcp_accept` (net.rs, BRIDGE_TCP_SOCKET_V1),
// added for the same reason and carrying the same shape: a synchronous,
// blocking, `fullIo` leaf fn with no coupling to the channels layer.
//
// ## What this deliberately does NOT do (hard limit NO_ORCHESTRATION_IN_RUST)
//
// No retry. No timeout. No shell. No argv assembly — the caller passes the
// exact list. No environment manipulation. No output capture or parsing. No
// classification of the exit code into anybody's error taxonomy. No working
// directory: the child inherits this process's cwd, which is precisely why the
// shim needs no chdir primitive (hard limit NO_CHDIR) — one process, one store,
// and the store is where the process already stands. Every one of those is a
// decision, and decisions live in M1.
//
// ## The return value is a report, not a judgement
//
// A `wait` can end three ways and an `Int` has to carry all three, so the bands
// are disjoint and documented rather than collapsed:
//
//     0 ..= 255    the child exited; this is its exit code
//    -1 ..= -64    the child was killed by a signal; this is -signum
//         -256     the child could not be started at all
//         -257     the child ended and the OS reported NEITHER an exit code nor
//                  a signal. Unreachable on unix — `status()` waits for
//                  termination, so `WIFEXITED` or `WIFSIGNALED` always holds —
//                  and it is the whole of the non-unix fallback. It exists as
//                  its own value rather than as a shrug because an encoding
//                  with an undocumented case is an encoding that lies: the
//                  first version of this fn returned -255, which is inside no
//                  band, one away from -256, and would have reached a caller
//                  as "killed by signal 255".
//
// -65 ..= -255 is RESERVED and never returned. Signals are 1..=64 and exit
// codes 0..=255, so no band overlaps another and every returned value is in
// exactly one of the four. Mapping these onto "out-of-contract" versus
// "transient" versus "the server is broken" is the caller's business and is
// not done here.
//
// Not panicking on a failed spawn follows `tcp_connect`, which returns `-1`
// rather than aborting for exactly this reason: a peer that is not there is an
// ordinary outcome for the caller to answer, not a bridge-level fault. A
// missing writer binary is the same class of fact, and panicking on it would
// hand the shim back the crash that round 2 spent its whole budget removing.
#[track_caller]
pub fn proc_run(program: std::sync::Arc<str>, argv: Value) -> Value {
    let args: Vec<String> = match argv {
        Value::List(items) => items
            .iter()
            .map(|v| match v {
                Value::Str(h) => super::value::get_str(h),
                other => panic!("proc_run: argv element must be Text, got {:?}", other),
            })
            .collect(),
        Value::Unit => Vec::new(),
        other => panic!("proc_run: argv must be a TextList, got {:?}", other),
    };

    let mut cmd = std::process::Command::new(program.as_ref());
    cmd.args(&args);

    match cmd.status() {
        Ok(st) => match st.code() {
            Some(c) => Value::Int(c as i64),
            None => Value::Int(terminating_signal(&st)),
        },
        Err(_) => Value::Int(NO_START),
    }
}

/// The value `proc_run` returns when `ExitStatus::code()` was `None`: `-signum`
/// if the OS names a signal, [`NO_REASON`] if it names nothing. Split out so the
/// `cfg` is one expression rather than smeared through `proc_run`'s control flow.
#[cfg(unix)]
fn terminating_signal(st: &std::process::ExitStatus) -> i64 {
    use std::os::unix::process::ExitStatusExt;
    match st.signal() {
        Some(s) => -(s as i64),
        None => NO_REASON,
    }
}

#[cfg(not(unix))]
fn terminating_signal(_st: &std::process::ExitStatus) -> i64 {
    NO_REASON
}

/// Ended, and the OS gave no reason. See `proc_run`'s band table.
const NO_REASON: i64 = -257;

/// Could not be started at all. See `proc_run`'s band table.
const NO_START: i64 = -256;

#[cfg(all(test, unix))]
mod proc_run_tests {
    use super::*;

    fn code(v: Value) -> i64 {
        match v {
            Value::Int(n) => n,
            other => panic!("expected Int, got {:?}", other),
        }
    }

    fn argv(parts: &[&str]) -> Value {
        Value::List(parts.iter().map(|s| Value::Str(intern_str(s))).collect())
    }

    /// The three result bands are disjoint by construction; this pins each one
    /// to a concrete observation so a later refactor cannot quietly collapse
    /// two of them into a shared sentinel.
    #[test]
    fn exit_code_is_reported_verbatim() {
        assert_eq!(code(proc_run("/bin/sh".into(), argv(&["-c", "exit 0"]))), 0);
        assert_eq!(code(proc_run("/bin/sh".into(), argv(&["-c", "exit 1"]))), 1);
        assert_eq!(code(proc_run("/bin/sh".into(), argv(&["-c", "exit 42"]))), 42);
        assert_eq!(code(proc_run("/bin/sh".into(), argv(&["-c", "exit 255"]))), 255);
    }

    #[test]
    fn a_signal_death_is_negative_signum_not_an_exit_code() {
        // SIGKILL = 9. Without this band it would arrive as some exit code and
        // be indistinguishable from a writer that chose to exit that way.
        let v = code(proc_run("/bin/sh".into(), argv(&["-c", "kill -9 $$"])));
        assert_eq!(v, -9, "expected -9 for SIGKILL, got {}", v);
    }

    #[test]
    fn a_program_that_does_not_exist_returns_no_start_and_does_not_panic() {
        let v = code(proc_run(
            "/nonexistent/definitely/not/a/binary".into(),
            argv(&[]),
        ));
        assert_eq!(v, NO_START);
        assert_eq!(v, -256);
    }

    /// The four bands must be mutually exclusive and each must be documented.
    /// This is the arm the first version of this fn would have failed: it
    /// returned -255 for "no reason given", which is inside the reserved gap,
    /// one away from NO_START, and would have been read as signal 255.
    #[test]
    fn the_four_bands_do_not_overlap_and_nothing_lands_in_the_reserved_gap() {
        let exit_codes = 0..=255i64;
        let signals: Vec<i64> = (1..=64).map(|s| -s).collect();
        let reserved: Vec<i64> = (65..=255).map(|s| -s).collect();

        assert!(!exit_codes.clone().any(|c| signals.contains(&c)));
        assert!(!exit_codes.clone().any(|c| c == NO_START || c == NO_REASON));
        assert!(!signals.contains(&NO_START) && !signals.contains(&NO_REASON));
        assert!(!reserved.contains(&NO_START), "NO_START is in the reserved gap");
        assert!(!reserved.contains(&NO_REASON), "NO_REASON is in the reserved gap");
        assert_ne!(NO_START, NO_REASON);
        // -255 specifically: the value the bug used. It must be reachable by
        // nothing, so that a future edit reintroducing it fails here.
        assert_ne!(NO_START, -255);
        assert_ne!(NO_REASON, -255);
        assert!(reserved.contains(&-255), "-255 must stay in the reserved gap");
    }

    #[test]
    fn argv_is_passed_through_untouched_not_shell_interpreted() {
        // If proc_run went through a shell, `*` would glob and `;` would split.
        // `[ "$1" = ... ]` is the assertion; sh's $0 is the -c arg name.
        let v = code(proc_run(
            "/bin/sh".into(),
            argv(&["-c", r#"[ "$1" = 'a * b; c' ] && exit 7 || exit 8"#, "x", "a * b; c"]),
        ));
        assert_eq!(v, 7, "argv was mangled — a shell or a re-quote is in the path");
    }

    #[test]
    fn the_child_inherits_the_callers_working_directory() {
        // This is the property that makes a chdir primitive unnecessary
        // (NO_CHDIR): one process, one store, and the store is where the
        // process already stands.
        let here = std::env::current_dir().unwrap();
        let probe = format!(r#"[ "$PWD" = "{}" ] && exit 3 || exit 4"#, here.display());
        assert_eq!(code(proc_run("/bin/sh".into(), argv(&["-c", &probe]))), 3);
    }
}

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
pub fn proc_sleep(v: Value) -> Value {
    let secs = match v {
        Value::Int(n) if n >= 0 => n as u64,
        Value::Int(_) => 0,
        _ => panic!("proc_sleep: expected Int, got {:?}", v),
    };
    std::thread::sleep(std::time::Duration::from_secs(secs));
    Value::Unit
}

#[track_caller]
pub fn sleep(v: Value) -> Value {
    let ms = match v {
        Value::Int(n) if n >= 0 => n as u64,
        Value::Int(_) => 0,
        _ => panic!("sleep: expected Int, got {:?}", v),
    };
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
pub fn argv_get(idx: Value) -> Value {
    let i = match idx { Value::Int(n) => n as usize, _ => 0 };
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

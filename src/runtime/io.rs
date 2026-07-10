use std::io::{BufRead, Write};
use super::value::{Value, intern_str, get_str};

#[track_caller]
pub fn io_print(val: Value) -> Value {
    match &val {
        Value::Str(h) => print!("{}", get_str(h)),
        Value::Int(n) => print!("{}", n),
        Value::Bool(b) => print!("{}", b),
        Value::Unit    => print!("()"),
        other          => print!("{}", other),
    }
    std::io::stdout().flush().ok();
    Value::Unit
}

#[track_caller]
pub fn io_println(val: Value) -> Value {
    match &val {
        Value::Str(h) => println!("{}", get_str(h)),
        Value::Int(n) => println!("{}", n),
        Value::Bool(b) => println!("{}", b),
        Value::Unit    => println!("()"),
        other          => println!("{}", other),
    }
    Value::Unit
}

#[track_caller]
pub fn io_eprint(val: Value) -> Value {
    match &val {
        Value::Str(h) => eprint!("{}", get_str(h)),
        Value::Int(n) => eprint!("{}", n),
        Value::Bool(b) => eprint!("{}", b),
        Value::Unit    => eprint!("()"),
        other          => eprint!("{}", other),
    }
    std::io::stderr().flush().ok();
    Value::Unit
}

#[track_caller]
pub fn io_read_line(_: Value) -> Value {
    let stdin = std::io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line).unwrap_or(0);
    Value::Str(intern_str(&line))
}

#[track_caller]
pub fn fs_read_text(path: Value) -> Value {
    let path_str = match path {
        Value::Str(h) => get_str(h),
        _ => panic!("fs_read_text: expected Str path"),
    };
    match std::fs::read_to_string(&path_str) {
        Ok(content) => Value::Str(intern_str(&content)),
        Err(e) => panic!("fs_read_text({}): {}", path_str, e),
    }
}

/// `fs_read_last_line(path: Text) -> Text` — AXVERITY_INSERT_PATH_FASTPATH
/// Landing 2. Returns the last non-empty line of `path`, or "" if the file
/// is missing or has no non-empty line — MISSING/EMPTY = EMPTY, NOT A PANIC
/// (the same guarded-read convention as field_lookup/wal_has), since a name
/// with no binding history yet is an expected, correct query result, not an
/// error. Used to resolve a name's "current" binding directly from its
/// append-only `.log` (lib/resolve_name.m1), replacing the read of a
/// separately-maintained `.current` cache file.
#[track_caller]
pub fn fs_read_last_line(path: Value) -> Value {
    let path_str = match path {
        Value::Str(h) => get_str(h),
        _ => panic!("fs_read_last_line: expected Str path"),
    };
    let content = match std::fs::read_to_string(&path_str) {
        Ok(c) => c,
        Err(_) => return Value::Str(intern_str("")),
    };
    let last = content.lines().rev().find(|l| !l.is_empty()).unwrap_or("");
    Value::Str(intern_str(last))
}

#[track_caller]
pub fn fs_write_text(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Str(path_h), Value::Str(content_h)) => {
                let path    = get_str(path_h);
                let content = get_str(content_h);
                if let Err(e) = std::fs::write(&path, &content) {
                    panic!("fs_write_text({}): {}", path, e);
                }
                Value::Unit
            }
            _ => panic!("fs_write_text: expected Tuple(Str, Str)"),
        },
        _ => panic!("fs_write_text: expected Tuple(path, content)"),
    }
}

#[track_caller]
pub fn fs_append_text(args: Value) -> Value {
    use std::io::Write as IoWrite;
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Str(path_h), Value::Str(content_h)) => {
                let path    = get_str(path_h);
                let content = get_str(content_h);
                let result = std::fs::OpenOptions::new()
                    .append(true).create(true).open(&path)
                    .and_then(|mut f| f.write_all(content.as_bytes()));
                if let Err(e) = result {
                    panic!("fs_append_text({}): {}", path, e);
                }
                Value::Unit
            }
            _ => panic!("fs_append_text: expected Tuple(Str, Str)"),
        },
        _ => panic!("fs_append_text: expected Tuple(path, content)"),
    }
}

#[track_caller]
pub fn fs_file_exists(path: Value) -> Value {
    let path_str = match path {
        Value::Str(h) => get_str(h),
        _ => panic!("fs_file_exists: expected Str path"),
    };
    Value::Bool(std::path::Path::new(&path_str).exists())
}

#[track_caller]
pub fn fs_list_dir(path: Value) -> Value {
    let path_str = match path {
        Value::Str(h) => get_str(h),
        _ => panic!("fs_list_dir: expected Str path"),
    };
    let mut entries: Vec<Value> = std::fs::read_dir(&path_str)
        .unwrap_or_else(|e| panic!("fs_list_dir: {}", e))
        .filter_map(|e| e.ok())
        .map(|e| Value::Str(intern_str(&e.file_name().to_string_lossy())))
        .collect();
    entries.sort_by(|a, b| match (a, b) {
        (Value::Str(ah), Value::Str(bh)) => get_str(ah).cmp(&get_str(bh)),
        _ => std::cmp::Ordering::Equal,
    });
    Value::List(entries)
}

/// Observational trace. Controlled by AXIS_TRACE=1. No semantic effect.
#[track_caller]
pub fn debug_trace(val: Value) -> Value {
    if std::env::var("AXIS_TRACE").ok().as_deref() == Some("1") {
        eprintln!("[trace] {}", val);
    }
    Value::Unit
}

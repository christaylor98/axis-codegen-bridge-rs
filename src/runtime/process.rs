use super::value::{Value, intern_str, get_process_args};

pub fn proc_args(_: Value) -> Value {
    let args = get_process_args();
    Value::List(args.iter().map(|s| Value::Str(intern_str(s))).collect())
}

pub fn proc_exit(code: Value) -> Value {
    let c = match code {
        Value::Int(n) => n as i32,
        _ => 0,
    };
    std::process::exit(c);
}

pub fn argv(idx: Value) -> Value {
    let i = match idx { Value::Int(n) => n as usize, _ => 0 };
    let args = get_process_args();
    match args.get(i) {
        Some(s) => Value::Str(intern_str(s)),
        None => Value::Str(intern_str("")),
    }
}

pub fn argv_int(idx: Value) -> Value {
    let i = match idx { Value::Int(n) => n as usize, _ => 0 };
    let args = get_process_args();
    match args.get(i) {
        Some(s) => Value::Int(s.parse::<i64>().unwrap_or(0)),
        None => Value::Int(0),
    }
}

pub fn argv_count(_: Value) -> Value {
    let args = get_process_args();
    Value::Int(args.len().saturating_sub(1) as i64)
}

pub fn argv_or(idx: Value) -> Value {
    let i = match idx { Value::Int(n) => n as usize, _ => 0 };
    let args = get_process_args();
    match args.get(i) {
        Some(s) => Value::Str(intern_str(s)),
        None => Value::Str(intern_str("")),
    }
}

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

pub fn proc_sleep(v: Value) -> Value {
    let secs = match v {
        Value::Int(n) if n >= 0 => n as u64,
        Value::Int(_) => 0,
        _ => panic!("proc_sleep: expected Int, got {:?}", v),
    };
    std::thread::sleep(std::time::Duration::from_secs(secs));
    Value::Unit
}

pub fn sleep(v: Value) -> Value {
    let ms = match v {
        Value::Int(n) if n >= 0 => n as u64,
        Value::Int(_) => 0,
        _ => panic!("sleep: expected Int, got {:?}", v),
    };
    std::thread::sleep(std::time::Duration::from_millis(ms));
    Value::Unit
}

pub fn argv(idx: Value) -> Value {
    let i = match idx { Value::Int(n) => n as usize, _ => 0 };
    let args = get_process_args();
    match args.get(i) {
        Some(s) => Value::Str(intern_str(s)),
        None => Value::Str(intern_str("")),
    }
}

pub fn argv_get(idx: Value) -> Value {
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

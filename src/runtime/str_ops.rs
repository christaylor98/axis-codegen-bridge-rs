use super::value::{Value, intern_str, get_str};

pub fn str_len(s: Value) -> Value {
    match s {
        Value::Str(h) => Value::Int(get_str(h).chars().count() as i64),
        _ => panic!("str_len: expected Str"),
    }
}

pub fn str_concat(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Str(a), Value::Str(b)) => {
                Value::Str(intern_str(&format!("{}{}", get_str(*a), get_str(*b))))
            }
            _ => panic!("str_concat: expected two Str values"),
        },
        _ => panic!("str_concat: expected Tuple(Str, Str)"),
    }
}

/// Checked character access. Returns Option(Str).
pub fn str_char_at(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => {
            let h = match &es[0] { Value::Str(h) => *h, _ => panic!("str_char_at: expected Str") };
            let idx = match &es[1] { Value::Int(n) => *n, _ => panic!("str_char_at: expected Int index") };
            if idx < 0 { return super::option::option_none(); }
            let chars: Vec<char> = get_str(h).chars().collect();
            match chars.get(idx as usize) {
                Some(c) => super::option::option_some(Value::Str(intern_str(&c.to_string()))),
                None    => super::option::option_none(),
            }
        }
        _ => panic!("str_char_at: expected Tuple(Str, Int)"),
    }
}

/// Unchecked character access. Panics on out-of-bounds.
pub fn str_char(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => {
            let h   = match &es[0] { Value::Str(h) => *h, _ => panic!("str_char: expected Str") };
            let idx = match &es[1] { Value::Int(n) => *n as usize, _ => panic!("str_char: expected Int") };
            let chars: Vec<char> = get_str(h).chars().collect();
            Value::Str(intern_str(&chars[idx].to_string()))
        }
        _ => panic!("str_char: expected Tuple(Str, Int)"),
    }
}

pub fn str_char_code(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => {
            let h   = match &es[0] { Value::Str(h) => *h, _ => panic!("str_char_code: expected Str") };
            let idx = match &es[1] { Value::Int(n) => *n as usize, _ => panic!("str_char_code: expected Int") };
            let chars: Vec<char> = get_str(h).chars().collect();
            Value::Int(chars[idx] as u32 as i64)
        }
        _ => panic!("str_char_code: expected Tuple(Str, Int)"),
    }
}

pub fn str_slice(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 3 => {
            let h     = match &es[0] { Value::Str(h) => *h, _ => panic!("str_slice: expected Str") };
            let start = match &es[1] { Value::Int(n) => *n as usize, _ => panic!("str_slice: expected Int start") };
            let end   = match &es[2] { Value::Int(n) => *n as usize, _ => panic!("str_slice: expected Int end") };
            let s = get_str(h);
            let chars: Vec<char> = s.chars().collect();
            let end = end.min(chars.len());
            let slice: String = chars[start..end].iter().collect();
            Value::Str(intern_str(&slice))
        }
        _ => panic!("str_slice: expected Tuple(Str, Int, Int)"),
    }
}

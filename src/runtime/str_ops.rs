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

pub fn str_split(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Str(content_h), Value::Str(delim_h)) => {
                let content = get_str(*content_h);
                let delim   = get_str(*delim_h);
                let parts: Vec<Value> = content.split(delim.as_str())
                    .map(|s| Value::Str(intern_str(s)))
                    .collect();
                Value::List(parts)
            }
            _ => panic!("str_split: expected two Str values"),
        },
        _ => panic!("str_split: expected Tuple(Str, Str)"),
    }
}

pub fn str_starts_with(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Str(hay_h), Value::Str(pre_h)) => {
                Value::Bool(get_str(*hay_h).starts_with(get_str(*pre_h).as_str()))
            }
            _ => panic!("str_starts_with: expected two Str values"),
        },
        _ => panic!("str_starts_with: expected Tuple(Str, Str)"),
    }
}

pub fn str_ends_with(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Str(hay_h), Value::Str(suf_h)) => {
                Value::Bool(get_str(*hay_h).ends_with(get_str(*suf_h).as_str()))
            }
            _ => panic!("str_ends_with: expected two Str values"),
        },
        _ => panic!("str_ends_with: expected Tuple(Str, Str)"),
    }
}

pub fn str_trim(s: Value) -> Value {
    match s {
        Value::Str(h) => Value::Str(intern_str(get_str(h).trim())),
        _ => panic!("str_trim: expected Str"),
    }
}

pub fn str_contains(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Str(hay_h), Value::Str(need_h)) => {
                Value::Bool(get_str(*hay_h).contains(get_str(*need_h).as_str()))
            }
            _ => panic!("str_contains: expected two Str values"),
        },
        _ => panic!("str_contains: expected Tuple(Str, Str)"),
    }
}

/// Returns the char-index of the first occurrence of needle in haystack, or -1 if not found.
pub fn str_index_of(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Str(hay_h), Value::Str(need_h)) => {
                let hay    = get_str(*hay_h);
                let needle = get_str(*need_h);
                let idx = hay.find(needle.as_str())
                    .map(|byte_pos| hay[..byte_pos].chars().count() as i64)
                    .unwrap_or(-1);
                Value::Int(idx)
            }
            _ => panic!("str_index_of: expected two Str values"),
        },
        _ => panic!("str_index_of: expected Tuple(Str, Str)"),
    }
}

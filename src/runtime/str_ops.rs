use super::value::{Value, intern_str, get_str};

#[track_caller]
pub fn str_len(s: Value) -> Value {
    match s {
        Value::Str(h) => Value::Int(get_str(h).chars().count() as i64),
        _ => panic!("str_len: expected Str"),
    }
}

#[track_caller]
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
#[track_caller]
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
#[track_caller]
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

#[track_caller]
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

#[track_caller]
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

#[track_caller]
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

#[track_caller]
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

#[track_caller]
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

#[track_caller]
pub fn str_trim(s: Value) -> Value {
    match s {
        Value::Str(h) => Value::Str(intern_str(get_str(h).trim())),
        _ => panic!("str_trim: expected Str"),
    }
}

#[track_caller]
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

#[track_caller]
pub fn str_eq(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Str(a), Value::Str(b)) => Value::Bool(get_str(*a) == get_str(*b)),
            _ => panic!("str_eq: expected two Str values"),
        },
        _ => panic!("str_eq: expected Tuple(Str, Str)"),
    }
}

/// text_eq(Text, Text) -> Bool. axis-canonical alias for str_eq (different
/// registry name, identical semantics — Text is the canonical type label).
#[track_caller]
pub fn text_eq(args: Value) -> Value { str_eq(args) }

/// text_lt(Text, Text) -> Bool — lexicographic less-than. The canonical
/// registry declares this; the surface str_* family doesn't (no str_lt today).
#[track_caller]
pub fn text_lt(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Str(a), Value::Str(b)) => Value::Bool(get_str(*a) < get_str(*b)),
            _ => panic!("text_lt: expected two Str values"),
        },
        _ => panic!("text_lt: expected Tuple(Str, Str)"),
    }
}

/// Returns the char-index of the first occurrence of needle in haystack, or -1 if not found.
#[track_caller]
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

#[track_caller]
pub fn str_before(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Str(sh), Value::Str(dh)) => {
                let s = get_str(*sh);
                let d = get_str(*dh);
                let result = s.split_once(d.as_str())
                    .map(|(before, _)| before)
                    .unwrap_or(s.as_str());
                Value::Str(intern_str(result))
            }
            _ => panic!("str_before: expected two Str values"),
        },
        _ => panic!("str_before: expected Tuple(Str, Str)"),
    }
}

#[track_caller]
pub fn str_after(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Str(sh), Value::Str(dh)) => {
                let s = get_str(*sh);
                let d = get_str(*dh);
                let result = s.split_once(d.as_str())
                    .map(|(_, after)| after)
                    .unwrap_or("");
                Value::Str(intern_str(result))
            }
            _ => panic!("str_after: expected two Str values"),
        },
        _ => panic!("str_after: expected Tuple(Str, Str)"),
    }
}

#[track_caller]
pub fn str_between(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 3 => match (&es[0], &es[1], &es[2]) {
            (Value::Str(sh), Value::Str(start_h), Value::Str(end_h)) => {
                let s = get_str(*sh);
                let start = get_str(*start_h);
                let end = get_str(*end_h);
                let after_start = s.split_once(start.as_str())
                    .map(|(_, after)| after)
                    .unwrap_or(s.as_str());
                let result = after_start.split_once(end.as_str())
                    .map(|(before, _)| before)
                    .unwrap_or(after_start);
                Value::Str(intern_str(result))
            }
            _ => panic!("str_between: expected three Str values"),
        },
        _ => panic!("str_between: expected Tuple(Str, Str, Str)"),
    }
}

/// bool_to_str: Bool → Text. Returns "true" or "false".
#[track_caller]
pub fn bool_to_str(v: Value) -> Value {
    match v {
        Value::Bool(b) => Value::Str(intern_str(if b { "true" } else { "false" })),
        _ => panic!("bool_to_str: expected Bool, got {:?}", v),
    }
}

/// chr: takes Int (Unicode code point), returns single-char Str.
#[track_caller]
pub fn chr(v: Value) -> Value {
    match v {
        Value::Int(n) => {
            let c = char::from_u32(n as u32).unwrap_or('\0');
            Value::Str(intern_str(&c.to_string()))
        }
        _ => panic!("chr: expected Int, got {:?}", v),
    }
}

/// `str_join(list, sep) -> Text` — join a `ValueList(Text)` with `sep`.
#[track_caller]
pub fn str_join(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() == 2 => {
            let sep = match &es[1] {
                Value::Str(h) => get_str(*h),
                other => panic!("str_join: expected Str for sep, got {:?}", other),
            };
            let parts: Vec<String> = match &es[0] {
                Value::List(items) => items
                    .iter()
                    .map(|v| match v {
                        Value::Str(h) => get_str(*h),
                        other => panic!(
                            "str_join: ValueList element must be Str, got {:?}",
                            other
                        ),
                    })
                    .collect(),
                other => panic!("str_join: expected ValueList of Str, got {:?}", other),
            };
            Value::Str(intern_str(&parts.join(&sep)))
        }
        _ => panic!("str_join: expected Tuple(ValueList, Text), got {:?}", args),
    }
}

// ── Phase 3 — text emit helpers (BRIDGE_FOREIGN_FN_FNREF_M1) ─────────────────

/// `str_replace(s, from, to) -> Text` — replace every occurrence of `from`
/// in `s` with `to`.
#[track_caller]
pub fn str_replace(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() == 3 => match (&es[0], &es[1], &es[2]) {
            (Value::Str(s), Value::Str(from), Value::Str(to)) => {
                let result = get_str(*s).replace(&get_str(*from), &get_str(*to));
                Value::Str(intern_str(&result))
            }
            (a, b, c) => panic!(
                "str_replace: expected three Str values, got ({:?}, {:?}, {:?})",
                a, b, c
            ),
        },
        _ => panic!("str_replace: expected Tuple(Str, Str, Str), got {:?}", args),
    }
}

/// `str_repeat(s, n) -> Text` — `n` copies of `s` concatenated.
#[track_caller]
pub fn str_repeat(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() == 2 => match (&es[0], &es[1]) {
            (Value::Str(s), Value::Int(n)) => {
                let count = if *n > 0 { *n as usize } else { 0 };
                Value::Str(intern_str(&get_str(*s).repeat(count)))
            }
            (a, b) => panic!(
                "str_repeat: expected Tuple(Str, Int), got ({:?}, {:?})",
                a, b
            ),
        },
        _ => panic!("str_repeat: expected Tuple(Str, Int), got {:?}", args),
    }
}

/// `str_to_upper(s) -> Text` — ASCII / Unicode uppercase. Idempotent.
#[track_caller]
pub fn str_to_upper(v: Value) -> Value {
    match v {
        Value::Str(h) => Value::Str(intern_str(&get_str(h).to_uppercase())),
        other => panic!("str_to_upper: expected Str, got {:?}", other),
    }
}

/// `str_to_lower(s) -> Text` — ASCII / Unicode lowercase. Idempotent.
#[track_caller]
pub fn str_to_lower(v: Value) -> Value {
    match v {
        Value::Str(h) => Value::Str(intern_str(&get_str(h).to_lowercase())),
        other => panic!("str_to_lower: expected Str, got {:?}", other),
    }
}

/// `str_pad_left(s, width, pad) -> Text` — left-pad `s` with `pad` to total
/// `width` chars. If `s` is already at least `width` chars long, returns `s`
/// unchanged. `pad` is repeated and truncated as needed.
#[track_caller]
pub fn str_pad_left(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() == 3 => match (&es[0], &es[1], &es[2]) {
            (Value::Str(s), Value::Int(width), Value::Str(pad)) => {
                let s_str = get_str(*s);
                let pad_str = get_str(*pad);
                let cur = s_str.chars().count() as i64;
                if cur >= *width || pad_str.is_empty() {
                    return Value::Str(*s);
                }
                let need = (*width - cur) as usize;
                let mut prefix = String::new();
                let mut iter = pad_str.chars().cycle();
                for _ in 0..need {
                    prefix.push(iter.next().unwrap());
                }
                Value::Str(intern_str(&format!("{}{}", prefix, s_str)))
            }
            (a, b, c) => panic!(
                "str_pad_left: expected Tuple(Str, Int, Str), got ({:?}, {:?}, {:?})",
                a, b, c
            ),
        },
        _ => panic!("str_pad_left: expected Tuple(Str, Int, Str), got {:?}", args),
    }
}

/// `str_pad_right(s, width, pad) -> Text` — right-pad mirror of `str_pad_left`.
#[track_caller]
pub fn str_pad_right(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() == 3 => match (&es[0], &es[1], &es[2]) {
            (Value::Str(s), Value::Int(width), Value::Str(pad)) => {
                let s_str = get_str(*s);
                let pad_str = get_str(*pad);
                let cur = s_str.chars().count() as i64;
                if cur >= *width || pad_str.is_empty() {
                    return Value::Str(*s);
                }
                let need = (*width - cur) as usize;
                let mut suffix = String::new();
                let mut iter = pad_str.chars().cycle();
                for _ in 0..need {
                    suffix.push(iter.next().unwrap());
                }
                Value::Str(intern_str(&format!("{}{}", s_str, suffix)))
            }
            (a, b, c) => panic!(
                "str_pad_right: expected Tuple(Str, Int, Str), got ({:?}, {:?}, {:?})",
                a, b, c
            ),
        },
        _ => panic!("str_pad_right: expected Tuple(Str, Int, Str), got {:?}", args),
    }
}

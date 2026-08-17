use super::value::{Value, intern_str, get_str};

#[track_caller]
pub fn str_len(s: std::sync::Arc<str>) -> Value {
    Value::Int(get_str(&s).chars().count() as i64)
}

#[track_caller]
pub fn str_concat(a: std::sync::Arc<str>, b: std::sync::Arc<str>) -> Value {
    Value::Str(intern_str(&format!("{}{}", a, b)))
}

/// Checked character access. Returns Option(Str).
#[track_caller]
pub fn str_char_at(s: std::sync::Arc<str>, idx: i64) -> Value {
    if idx < 0 { return super::option::option_none(); }
    let chars: Vec<char> = get_str(&s).chars().collect();
    match chars.get(idx as usize) {
        Some(c) => super::option::option_some(Value::Str(intern_str(&c.to_string()))),
        None    => super::option::option_none(),
    }
}

/// Unchecked character access. Panics on out-of-bounds.
#[track_caller]
pub fn str_char(s: std::sync::Arc<str>, idx: i64) -> Value {
    let chars: Vec<char> = get_str(&s).chars().collect();
    Value::Str(intern_str(&chars[idx as usize].to_string()))
}

#[track_caller]
pub fn str_char_code(s: std::sync::Arc<str>, idx: i64) -> Value {
    let chars: Vec<char> = get_str(&s).chars().collect();
    Value::Int(chars[idx as usize] as u32 as i64)
}

#[track_caller]
pub fn str_slice(s: std::sync::Arc<str>, start: i64, end: i64) -> Value {
    let start = start as usize;
    let text = get_str(&s);
    let chars: Vec<char> = text.chars().collect();
    let end = (end as usize).min(chars.len());
    let slice: String = chars[start..end].iter().collect();
    Value::Str(intern_str(&slice))
}

#[track_caller]
pub fn str_split(content: std::sync::Arc<str>, delim: std::sync::Arc<str>) -> Value {
    let content = get_str(&content);
    let delim   = get_str(&delim);
    let parts: Vec<Value> = content.split(delim.as_str())
        .map(|s| Value::Str(intern_str(s)))
        .collect();
    Value::List(parts)
}

#[track_caller]
pub fn str_starts_with(hay: std::sync::Arc<str>, pre: std::sync::Arc<str>) -> Value {
    Value::Bool(get_str(&hay).starts_with(get_str(&pre).as_str()))
}

#[track_caller]
pub fn str_ends_with(hay: std::sync::Arc<str>, suf: std::sync::Arc<str>) -> Value {
    Value::Bool(get_str(&hay).ends_with(get_str(&suf).as_str()))
}

#[track_caller]
pub fn str_trim(s: std::sync::Arc<str>) -> Value {
    Value::Str(intern_str(get_str(&s).trim()))
}

#[track_caller]
pub fn str_contains(hay: std::sync::Arc<str>, need: std::sync::Arc<str>) -> Value {
    Value::Bool(get_str(&hay).contains(get_str(&need).as_str()))
}

#[track_caller]
pub fn str_eq(a: std::sync::Arc<str>, b: std::sync::Arc<str>) -> Value {
    Value::Bool(get_str(&a) == get_str(&b))
}

/// text_eq(Text, Text) -> Bool. axis-canonical alias for str_eq (different
/// registry name, identical semantics — Text is the canonical type label).
#[track_caller]
pub fn text_eq(a: std::sync::Arc<str>, b: std::sync::Arc<str>) -> Value { str_eq(a, b) }

/// text_lt/text_lte/text_gt/text_gte(Text, Text) -> Bool — lexicographic
/// ordering by Unicode scalar value (Rust's `str` Ord, i.e. UTF-8 byte order).
/// Canonical registry names; the surface str_* family has no ordered compares.
macro_rules! text_cmp_op {
    ($name:ident, $op:tt) => {
        #[track_caller]
        pub fn $name(a: std::sync::Arc<str>, b: std::sync::Arc<str>) -> Value {
            Value::Bool(get_str(&a) $op get_str(&b))
        }
    };
}

text_cmp_op!(text_lt,  <);
text_cmp_op!(text_lte, <=);
text_cmp_op!(text_gt,  >);
text_cmp_op!(text_gte, >=);

/// str_* ordered-comparison aliases of the canonical text_* fns (identical
/// runtime), mirroring the str_eq/text_eq pairing — str_ is the surface name,
/// text_ the canonical registry name.
#[track_caller]
pub fn str_lt(a: std::sync::Arc<str>, b: std::sync::Arc<str>) -> Value { text_lt(a, b) }
#[track_caller]
pub fn str_lte(a: std::sync::Arc<str>, b: std::sync::Arc<str>) -> Value { text_lte(a, b) }
#[track_caller]
pub fn str_gt(a: std::sync::Arc<str>, b: std::sync::Arc<str>) -> Value { text_gt(a, b) }
#[track_caller]
pub fn str_gte(a: std::sync::Arc<str>, b: std::sync::Arc<str>) -> Value { text_gte(a, b) }

/// Returns the char-index of the first occurrence of needle in haystack, or -1 if not found.
#[track_caller]
pub fn str_index_of(hay: std::sync::Arc<str>, need: std::sync::Arc<str>) -> Value {
    let hay    = get_str(&hay);
    let needle = get_str(&need);
    let idx = hay.find(needle.as_str())
        .map(|byte_pos| hay[..byte_pos].chars().count() as i64)
        .unwrap_or(-1);
    Value::Int(idx)
}

#[track_caller]
pub fn str_before(s: std::sync::Arc<str>, d: std::sync::Arc<str>) -> Value {
    let s = get_str(&s);
    let d = get_str(&d);
    let result = s.split_once(d.as_str())
        .map(|(before, _)| before)
        .unwrap_or(s.as_str());
    Value::Str(intern_str(result))
}

#[track_caller]
pub fn str_after(s: std::sync::Arc<str>, d: std::sync::Arc<str>) -> Value {
    let s = get_str(&s);
    let d = get_str(&d);
    let result = s.split_once(d.as_str())
        .map(|(_, after)| after)
        .unwrap_or("");
    Value::Str(intern_str(result))
}

#[track_caller]
pub fn str_between(s: std::sync::Arc<str>, start: std::sync::Arc<str>, end: std::sync::Arc<str>) -> Value {
    let s = get_str(&s);
    let start = get_str(&start);
    let end = get_str(&end);
    let after_start = s.split_once(start.as_str())
        .map(|(_, after)| after)
        .unwrap_or(s.as_str());
    let result = after_start.split_once(end.as_str())
        .map(|(before, _)| before)
        .unwrap_or(after_start);
    Value::Str(intern_str(result))
}

/// bool_to_str: Bool → Text. Returns "true" or "false".
#[track_caller]
pub fn bool_to_str(b: bool) -> Value {
    Value::Str(intern_str(if b { "true" } else { "false" }))
}

/// chr: takes Int (Unicode code point), returns single-char Str.
#[track_caller]
pub fn chr(n: i64) -> Value {
    let c = char::from_u32(n as u32).unwrap_or('\0');
    Value::Str(intern_str(&c.to_string()))
}

/// `str_cmp(a, b) -> Int` — BYTE-order comparison (memcmp semantics, the
/// same order SQLite's BINARY collation and the store's pinned autoindex
/// orders use): -1 when a < b, 0 when equal, 1 when a > b. Added under the
/// fold precedent (BRIDGE_STRCMP_SETMAP_V1): the workload that proved the
/// need was axSemantica-working2's query-seam sorts, which had to build
/// per-char comparison loops over an alphabet text because no ordering
/// primitive existed.
#[track_caller]
pub fn str_cmp(a: std::sync::Arc<str>, b: std::sync::Arc<str>) -> Value {
    let a = get_str(&a);
    let b = get_str(&b);
    let r = match a.as_bytes().cmp(b.as_bytes()) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    };
    Value::Int(r)
}

/// `ord(s) -> Int` — the Unicode code point of the FIRST char of `s`; -1 for
/// the empty string. The inverse of `chr` (same precedent as str_cmp).
#[track_caller]
pub fn ord(s: std::sync::Arc<str>) -> Value {
    let s = get_str(&s);
    match s.chars().next() {
        Some(c) => Value::Int(c as i64),
        None => Value::Int(-1),
    }
}

/// `str_join(list, sep) -> Text` — join a `ValueList(Text)` with `sep`.
#[track_caller]
pub fn str_join(list: Value, sep: std::sync::Arc<str>) -> Value {
    let sep = get_str(&sep);
    let parts: Vec<String> = match list {
        Value::List(items) => items
            .iter()
            .map(|v| match v {
                Value::Str(h) => get_str(h),
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

// ── Phase 3 — text emit helpers (BRIDGE_FOREIGN_FN_FNREF_M1) ─────────────────

/// `str_replace(s, from, to) -> Text` — replace every occurrence of `from`
/// in `s` with `to`.
#[track_caller]
pub fn str_replace(s: std::sync::Arc<str>, from: std::sync::Arc<str>, to: std::sync::Arc<str>) -> Value {
    let result = get_str(&s).replace(&get_str(&from), &get_str(&to));
    Value::Str(intern_str(&result))
}

/// `str_repeat(s, n) -> Text` — `n` copies of `s` concatenated.
#[track_caller]
pub fn str_repeat(s: std::sync::Arc<str>, n: i64) -> Value {
    let count = if n > 0 { n as usize } else { 0 };
    Value::Str(intern_str(&get_str(&s).repeat(count)))
}

/// `str_to_upper(s) -> Text` — ASCII / Unicode uppercase. Idempotent.
#[track_caller]
pub fn str_to_upper(s: std::sync::Arc<str>) -> Value {
    Value::Str(intern_str(&get_str(&s).to_uppercase()))
}

/// `str_to_lower(s) -> Text` — ASCII / Unicode lowercase. Idempotent.
#[track_caller]
pub fn str_to_lower(s: std::sync::Arc<str>) -> Value {
    Value::Str(intern_str(&get_str(&s).to_lowercase()))
}

/// `str_pad_left(s, width, pad) -> Text` — left-pad `s` with `pad` to total
/// `width` chars. If `s` is already at least `width` chars long, returns `s`
/// unchanged. `pad` is repeated and truncated as needed.
#[track_caller]
pub fn str_pad_left(s: std::sync::Arc<str>, width: i64, pad: std::sync::Arc<str>) -> Value {
    let s_str = get_str(&s);
    let pad_str = get_str(&pad);
    let cur = s_str.chars().count() as i64;
    if cur >= width || pad_str.is_empty() {
        return Value::Str(s);
    }
    let need = (width - cur) as usize;
    let mut prefix = String::new();
    let mut iter = pad_str.chars().cycle();
    for _ in 0..need {
        prefix.push(iter.next().unwrap());
    }
    Value::Str(intern_str(&format!("{}{}", prefix, s_str)))
}

/// `str_pad_right(s, width, pad) -> Text` — right-pad mirror of `str_pad_left`.
#[track_caller]
pub fn str_pad_right(s: std::sync::Arc<str>, width: i64, pad: std::sync::Arc<str>) -> Value {
    let s_str = get_str(&s);
    let pad_str = get_str(&pad);
    let cur = s_str.chars().count() as i64;
    if cur >= width || pad_str.is_empty() {
        return Value::Str(s);
    }
    let need = (width - cur) as usize;
    let mut suffix = String::new();
    let mut iter = pad_str.chars().cycle();
    for _ in 0..need {
        suffix.push(iter.next().unwrap());
    }
    Value::Str(intern_str(&format!("{}{}", s_str, suffix)))
}

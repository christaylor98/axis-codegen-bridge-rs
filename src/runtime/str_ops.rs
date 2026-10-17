use super::value::{Value, intern_str, get_str};

// ── O(1) code-point indexing (AX146 step A) ──────────────────────────────────────────────────────────────────────
//
// `str_len` / `str_char` / `str_char_at` / `str_char_code` count in code points. They used to copy the whole string
// (`get_str`) and then collect it into a Vec<char> on EVERY call, so a character loop was quadratic. Now:
//   - a string of at most SHORT bytes is read directly (a scan of <= SHORT bytes);
//   - a longer one (up to CACHE_MAX_BYTES) gets an index built once and kept in a small thread-local cache keyed by
//     the Arc's pointer: its ASCII flag, its code-point count, and (non-ASCII only) the byte offset of every
//     CKPT-th code point. An ASCII string then indexes by byte, a non-ASCII one by checkpoint + <= CKPT steps;
//   - a string over CACHE_MAX_BYTES is not cached (so the cache pins at most SLOTS * CACHE_MAX_BYTES per thread) and is
//     scanned per call, which is still no copy.
// Soundness of the pointer key: the cache holds a STRONG clone of each cached Arc, so an address in the cache cannot
// be freed and reused for another string while its entry lives; the text behind an Arc<str> is immutable.
const SHORT: usize = 32;
const CACHE_MAX_BYTES: usize = 32 << 20;
const CKPT: usize = 32;
const SLOTS: usize = 4;

struct Indexed {
    text: std::sync::Arc<str>,
    ascii: bool,
    nchars: usize,
    /// byte offset of code point k * CKPT (empty when ascii)
    ckpt: Vec<usize>,
}

impl Indexed {
    fn build(text: std::sync::Arc<str>) -> Indexed {
        if text.is_ascii() {
            let nchars = text.len();
            return Indexed { text, ascii: true, nchars, ckpt: Vec::new() };
        }
        let (mut ckpt, mut n) = (Vec::new(), 0usize);
        for (b, _) in text.char_indices() {
            if n % CKPT == 0 { ckpt.push(b); }
            n += 1;
        }
        Indexed { text, ascii: false, nchars: n, ckpt }
    }

    fn char_at(&self, idx: usize) -> Option<char> {
        if idx >= self.nchars { return None; }
        if self.ascii { return Some(self.text.as_bytes()[idx] as char); }
        self.text[self.ckpt[idx / CKPT]..].chars().nth(idx % CKPT)
    }
}

struct IndexCache {
    slots: [Option<Indexed>; SLOTS],
    next: usize,
}

thread_local! {
    static INDEX_CACHE: std::cell::RefCell<IndexCache> =
        const { std::cell::RefCell::new(IndexCache { slots: [None, None, None, None], next: 0 }) };
}

/// Run `cached` on the string's index (built and cached on first sight), or `direct` on the bare text when the
/// string is too short or too long to be worth caching. Neither closure may re-enter this module.
fn with_index<R>(s: &std::sync::Arc<str>, direct: impl FnOnce(&str) -> R, cached: impl FnOnce(&Indexed) -> R) -> R {
    let n = s.len();
    if n <= SHORT || n > CACHE_MAX_BYTES { return direct(s); }
    INDEX_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        let key = std::sync::Arc::as_ptr(s) as *const u8;
        let hit = c.slots.iter().position(|e| {
            e.as_ref().is_some_and(|ix| std::ptr::eq(std::sync::Arc::as_ptr(&ix.text) as *const u8, key) && ix.text.len() == n)
        });
        let slot = match hit {
            Some(i) => i,
            None => {
                let i = c.next;
                c.next = (i + 1) % SLOTS;
                c.slots[i] = Some(Indexed::build(s.clone()));
                i
            }
        };
        cached(c.slots[slot].as_ref().expect("a filled slot"))
    })
}

fn n_chars(s: &std::sync::Arc<str>) -> usize {
    with_index(s, |t| t.chars().count(), |ix| ix.nchars)
}

fn char_at_idx(s: &std::sync::Arc<str>, idx: usize) -> Option<char> {
    with_index(
        s,
        |t| if t.is_ascii() { t.as_bytes().get(idx).map(|b| *b as char) } else { t.chars().nth(idx) },
        |ix| ix.char_at(idx),
    )
}

/// The one-code-point Text for `c`. The 128 ASCII ones are preallocated and shared (a refcount bump, no allocation);
/// sharing is unobservable: an Arc<str> is immutable and nothing here compares by address.
fn char_text(c: char) -> std::sync::Arc<str> {
    static ASCII: std::sync::OnceLock<Vec<std::sync::Arc<str>>> = std::sync::OnceLock::new();
    if c.is_ascii() {
        let t = ASCII.get_or_init(|| (0u8..128).map(|b| std::sync::Arc::<str>::from((b as char).encode_utf8(&mut [0; 4]) as &str)).collect());
        return t[c as usize].clone();
    }
    intern_str(c.encode_utf8(&mut [0; 4]))
}

#[track_caller]
pub fn str_len(s: std::sync::Arc<str>) -> Value {
    Value::Int(n_chars(&s) as i64)
}

#[track_caller]
pub fn str_concat(a: std::sync::Arc<str>, b: std::sync::Arc<str>) -> Value {
    Value::Str(intern_str(&format!("{}{}", a, b)))
}

/// Checked character access. Returns Option(Str).
#[track_caller]
pub fn str_char_at(s: std::sync::Arc<str>, idx: i64) -> Value {
    if idx < 0 { return super::option::option_none(); }
    match char_at_idx(&s, idx as usize) {
        Some(c) => super::option::option_some(Value::Str(char_text(c))),
        None    => super::option::option_none(),
    }
}

/// Unchecked character access. Panics on out-of-bounds.
#[track_caller]
pub fn str_char(s: std::sync::Arc<str>, idx: i64) -> Value {
    if let Some(c) = usize::try_from(idx).ok().and_then(|i| char_at_idx(&s, i)) {
        return Value::Str(char_text(c));
    }
    // out of range: the original Vec<char> index, so the panic message is exactly what it always was
    let chars: Vec<char> = s.chars().collect();
    Value::Str(intern_str(&chars[idx as usize].to_string()))
}

#[track_caller]
pub fn str_char_code(s: std::sync::Arc<str>, idx: i64) -> Value {
    if let Some(c) = usize::try_from(idx).ok().and_then(|i| char_at_idx(&s, i)) {
        return Value::Int(c as u32 as i64);
    }
    let chars: Vec<char> = s.chars().collect();
    Value::Int(chars[idx as usize] as u32 as i64)
}

#[track_caller]
pub fn str_slice(s: std::sync::Arc<str>, start: i64, end: i64) -> Value {
    // Same contract as before (code points; end clamps to the length; start past the clamped end panics), but one
    // walk to `end` over the borrowed text: no copy of the whole string, no Vec<char>.
    let text: &str = &s;
    let (start, end) = (start as usize, end as usize);
    let (sb, eb) = char_bytes(text, start, end);
    match sb {
        Some(sb) if sb <= eb => Value::Str(intern_str(&text[sb..eb])),
        _ => panic!("str_slice: range start index {} out of range for slice of length {}", start, text.chars().count()),
    }
}

/// Byte offsets of code point `start` (None past the end) and of code point `end` clamped to the length, in one walk.
fn char_bytes(text: &str, start: usize, end: usize) -> (Option<usize>, usize) {
    let (mut sb, mut n) = (None, 0usize);
    for (b, _) in text.char_indices() {
        if n == start { sb = Some(b); }
        if n == end { return (sb, b); }
        n += 1;
    }
    if n == start { sb = Some(text.len()); }
    (sb, text.len())
}

/// `str_find_from(hay, needle, from) -> Int` — the code-point index of the first occurrence of `needle` that starts
/// at or after code point `from` (Python's `hay.find(needle, from)` for from >= 0); -1 when there is none, when `from`
/// is past the end, or when `from` is negative. Overlapping occurrences count. Borrows the text: no copy.
#[track_caller]
pub fn str_find_from(hay: std::sync::Arc<str>, needle: std::sync::Arc<str>, from: i64) -> Value {
    if from < 0 { return Value::Int(-1); }
    let text: &str = &hay;
    let (fb, _) = char_bytes(text, from as usize, from as usize);
    let idx = fb.and_then(|fb| text[fb..].find(&*needle).map(|p| from + text[fb..fb + p].chars().count() as i64));
    Value::Int(idx.unwrap_or(-1))
}

/// `str_starts_with_at(hay, needle, pos) -> Bool` — `needle` occurs in `hay` at code point `pos` (Python's
/// `hay.startswith(needle, pos)` for pos >= 0); false when `pos` is negative or past the end. Borrows the text.
#[track_caller]
pub fn str_starts_with_at(hay: std::sync::Arc<str>, needle: std::sync::Arc<str>, pos: i64) -> Value {
    if pos < 0 { return Value::Bool(false); }
    let text: &str = &hay;
    let (pb, _) = char_bytes(text, pos as usize, pos as usize);
    Value::Bool(pb.map(|pb| text[pb..].starts_with(&*needle)).unwrap_or(false))
}

#[track_caller]
pub fn str_split(content: std::sync::Arc<str>, delim: std::sync::Arc<str>) -> Value {
    let content = get_str(&content);
    let delim   = get_str(&delim);
    let parts: Vec<Value> = content.split(delim.as_str())
        .map(|s| Value::Str(intern_str(s)))
        .collect();
    Value::List(super::value::ListBuf::from(parts))
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
    Value::Bool(*a == *b)
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

#[cfg(test)]
mod index_tests {
    use super::*;
    use std::sync::Arc;

    fn t(s: &str) -> Arc<str> { Arc::from(s) }
    fn txt(v: Value) -> String { match v { Value::Str(s) => s.to_string(), o => panic!("{:?}", o) } }
    /// the reference: what the original Vec<char> implementation answered
    fn oracle(s: &str, i: i64) -> Option<char> { if i < 0 { None } else { s.chars().nth(i as usize) } }

    fn long_ascii() -> String { "abcdefgh".repeat(2000) }                         // 16000 bytes, over SHORT
    fn long_mixed() -> String { "aé日😀z".repeat(500) }                           // 1-, 2-, 3-, 4-byte chars, over SHORT
    const GRAB: &[&str] = &["", "a", "héllo", "ab😀cd"];

    #[test]
    fn every_index_agrees_with_the_char_vec_oracle() {
        for s in GRAB.iter().map(|s| s.to_string()).chain([long_ascii(), long_mixed(), "é".repeat(33), "x".repeat(33)]) {
            let a = t(&s);
            let n = s.chars().count() as i64;
            assert_eq!(str_len(a.clone()), Value::Int(n), "len of {:?}", &s[..s.len().min(12)]);
            // every in-range index, the CKPT boundaries and their neighbours included, plus both ends out of range
            for i in (-2..n + 3).filter(|i| n < 200 || i % 7 == 0 || i % CKPT as i64 <= 1 || *i >= n - 2) {
                let want = oracle(&s, i);
                let got = str_char_at(a.clone(), i);
                match want {
                    Some(c) => {
                        assert_eq!(got, super::super::option::option_some(Value::Str(Arc::from(c.to_string().as_str()))), "str_char_at {}", i);
                        assert_eq!(txt(str_char(a.clone(), i)), c.to_string(), "str_char {}", i);
                        assert_eq!(str_char_code(a.clone(), i), Value::Int(c as u32 as i64), "str_char_code {}", i);
                    }
                    None => assert_eq!(got, super::super::option::option_none(), "str_char_at {} out of range", i),
                }
            }
        }
    }

    #[test]
    fn out_of_range_panics_keep_their_message() {
        let msg = |f: Box<dyn FnOnce() + std::panic::UnwindSafe>| {
            let e = std::panic::catch_unwind(f).expect_err("must panic");
            e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap()
        };
        let a = t(&long_ascii());
        let (a1, a2, a3) = (a.clone(), a.clone(), a.clone());
        assert_eq!(msg(Box::new(move || { str_char(a1, 16000); })), "index out of bounds: the len is 16000 but the index is 16000");
        assert_eq!(msg(Box::new(move || { str_char_code(a2, 99999); })), "index out of bounds: the len is 16000 but the index is 99999");
        assert_eq!(msg(Box::new(move || { str_char(a3, -1); })), format!("index out of bounds: the len is 16000 but the index is {}", usize::MAX));
    }

    #[test]
    fn two_strings_with_equal_text_but_different_allocations_do_not_alias() {
        // the cache is keyed by address: a different Arc with the SAME length must not read another's index
        let (x, y) = (t(&"ab".repeat(100)), t(&"cd".repeat(100)));
        assert_eq!(txt(str_char(x.clone(), 0)), "a");
        assert_eq!(txt(str_char(y.clone(), 0)), "c");
        assert_eq!(txt(str_char(x, 1)), "b");
        assert_eq!(txt(str_char(y, 1)), "d");
    }

    #[test]
    fn more_strings_than_slots_still_answer_correctly() {
        let strs: Vec<Arc<str>> = (0..SLOTS * 3).map(|k| t(&format!("{}{}", "é".repeat(40), k))).collect();
        for round in 0..3 {
            for (k, s) in strs.iter().enumerate() {
                assert_eq!(txt(str_char(s.clone(), 40)), k.to_string().chars().next().unwrap().to_string(), "round {} string {}", round, k);
                assert_eq!(str_len(s.clone()), Value::Int(40 + k.to_string().len() as i64));
            }
        }
    }

    #[test]
    fn ascii_chars_share_one_allocation_and_others_do_not_break() {
        let (a, b) = (txt_arc(str_char(t("xyz"), 0)), txt_arc(str_char(t("x"), 0)));
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(&*txt_arc(str_char(t("é"), 0)), "é");
    }
    fn txt_arc(v: Value) -> Arc<str> { match v { Value::Str(s) => s, o => panic!("{:?}", o) } }

    #[test]
    fn str_eq_compares_content() {
        assert_eq!(str_eq(t("abc"), t("abc")), Value::Bool(true));
        assert_eq!(str_eq(t("abc"), t("abd")), Value::Bool(false));
        assert_eq!(str_eq(t(""), t("")), Value::Bool(true));
    }
}

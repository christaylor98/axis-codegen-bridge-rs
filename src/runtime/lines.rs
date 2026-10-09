//! G1 LINE OPS (axMachina v2 codegen layer, 2026-10-09): pure ops over line-oriented text, groomed for code
//! generation. The corpus does most of its pure work on files as lines (split on "\n", filter, fold back with a
//! newline after each); as a chain of str_split / text_list_filter / fold with a callback that is beyond the search's
//! reach, as one call each it is a size-2 program. Declared in `axRegistry-working/axis-bridge-world.axreg` next to
//! the world layer and wired by `axis-bridge-world.dispatch.toml`; no legacy fn changes.
//!
//! A text is read as lines: split on "\n", a final "" after a trailing "\n" dropped ("" is no lines). A result is
//! written as lines, each followed by "\n" (the corpus's `join_nl` fold). Columns are tab-separated, 0-based. Every
//! op is total: no panics, an out-of-range column is absent.

use super::value::{intern_str, Value};
use std::sync::Arc;

fn read(t: &str) -> Vec<&str> {
    if t.is_empty() {
        return Vec::new();
    }
    let mut v: Vec<&str> = t.split('\n').collect();
    if t.ends_with('\n') {
        v.pop();
    }
    v
}

fn write<'a, I: Iterator<Item = &'a str>>(lines: I) -> Value {
    let mut out = String::new();
    for l in lines {
        out.push_str(l);
        out.push('\n');
    }
    Value::Str(intern_str(&out))
}

fn col(line: &str, k: i64) -> Option<&str> {
    if k < 0 {
        return None;
    }
    line.split('\t').nth(k as usize)
}

/// The lines that are not blank (trimmed non-empty).
pub fn lines_nonblank(t: Arc<str>) -> Value {
    write(read(&t).into_iter().filter(|l| !l.trim().is_empty()))
}

/// The lines in reverse order.
pub fn lines_reverse(t: Arc<str>) -> Value {
    write(read(&t).into_iter().rev())
}

/// The number of lines.
pub fn lines_count(t: Arc<str>) -> Value {
    Value::Int(read(&t).len() as i64)
}

/// The lines that start with `p`.
pub fn lines_with_prefix(t: Arc<str>, p: Arc<str>) -> Value {
    write(read(&t).into_iter().filter(|l| l.starts_with(p.as_ref())))
}

/// The lines that contain `s`.
pub fn lines_containing(t: Arc<str>, s: Arc<str>) -> Value {
    write(read(&t).into_iter().filter(|l| l.contains(s.as_ref())))
}

/// The lines whose column `k` equals `v`.
pub fn lines_col_eq(t: Arc<str>, k: i64, v: Arc<str>) -> Value {
    write(read(&t).into_iter().filter(|l| col(l, k) == Some(v.as_ref())))
}

/// The lines whose column `k` is absent or differs from `v`.
pub fn lines_col_ne(t: Arc<str>, k: i64, v: Arc<str>) -> Value {
    write(read(&t).into_iter().filter(|l| col(l, k) != Some(v.as_ref())))
}

/// Each line cut to its first `n` columns (fewer when it has fewer; n <= 0 -> an empty line).
pub fn lines_head_cols(t: Arc<str>, n: i64) -> Value {
    let cut: Vec<String> = read(&t)
        .into_iter()
        .map(|l| if n <= 0 { String::new() } else { l.split('\t').take(n as usize).collect::<Vec<_>>().join("\t") })
        .collect();
    write(cut.iter().map(|s| s.as_str()))
}

/// Column `k` of the first line whose column 0 equals `key` ("" when no line does, or it has no column `k`).
pub fn lines_col_of(t: Arc<str>, key: Arc<str>, k: i64) -> Value {
    let hit = read(&t).into_iter().find(|l| col(l, 0) == Some(key.as_ref())).and_then(|l| col(l, k)).unwrap_or("");
    Value::Str(intern_str(hit))
}

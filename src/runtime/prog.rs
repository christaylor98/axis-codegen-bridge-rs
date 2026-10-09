//! Groomed program tokenizer (axMachina codegen layer, 2026-10-10). The drivers render programs as
//! `p<k>` | `c(<n>:<typed value>)` | `<op>(<arg>,...)` (n = the typed value's length in code points), and read them
//! back character by character with `str_slice`, which is O(position) per call: quadratic on the multi-kilobyte
//! programs a ladder grows into. One pass here instead. Syntax only: no op is known or evaluated.

use super::value::{intern_str, ListBuf, Value};

/// `prog_tokens(p: Text) -> TextList` — the tokens of a rendered program, in order:
///   `F<name>`  an identifier ([a-z0-9_]+) followed by "(" (a call; the "(" is consumed), other than a literal
///   `L<typed>` a literal: the identifier `c`, "(", up to 9 digits n, ":", n code points, ")"
///   `I<name>`  an identifier not followed by "("
///   `)` and `,` as themselves
///   `!`        anything else, or a malformed literal (the tokens stop there)
pub fn prog_tokens(p: std::sync::Arc<str>) -> Value {
    let cs: Vec<char> = p.chars().collect();
    let n = cs.len();
    let isid = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_';
    let mut out: Vec<Value> = Vec::new();
    let mut i = 0;
    let push = |out: &mut Vec<Value>, s: String| out.push(Value::Str(intern_str(&s)));
    while i < n {
        let c = cs[i];
        if c == ')' || c == ',' { push(&mut out, c.to_string()); i += 1; continue; }
        if !isid(c) { push(&mut out, "!".into()); break; }
        let mut e = i;
        while e < n && isid(cs[e]) { e += 1; }
        let id: String = cs[i..e].iter().collect();
        let call = e < n && cs[e] == '(';
        if call && id == "c" {
            let mut d = e + 1;
            while d < n && cs[d].is_ascii_digit() { d += 1; }
            let digits: String = cs[e + 1..d].iter().collect();
            let len = if !digits.is_empty() && digits.len() < 10 { digits.parse::<usize>().unwrap_or(0) } else { 0 };
            let ok = d < n && cs[d] == ':' && d + 1 + len < n && cs[d + 1 + len] == ')';
            if !ok { push(&mut out, "!".into()); break; }
            let tv: String = cs[d + 1..d + 1 + len].iter().collect();
            push(&mut out, format!("L{tv}"));
            i = d + 2 + len;
        } else if call {
            push(&mut out, format!("F{id}"));
            i = e + 1;
        } else {
            push(&mut out, format!("I{id}"));
            i = e;
        }
    }
    Value::List(ListBuf::from(out))
}

/// `prog_spans(p: Text) -> TextList` — one entry per `prog_tokens` token, in the same order: for a call token
/// `F<name>` "<start>,<end>,<close>" (the call's code-point span, name to its closing ")" inclusive, and the token
/// index of that ")"); "" for every other token. Lets a caller key a subtree by its text and jump over it.
pub fn prog_spans(p: std::sync::Arc<str>) -> Value {
    let toks = match prog_tokens(p.clone()) { Value::List(l) => l.iter().map(|v| match v { Value::Str(s) => s.to_string(), _ => String::new() }).collect::<Vec<_>>(), _ => vec![] };
    let mut spans = vec![String::new(); toks.len()];
    let mut stack: Vec<(usize, usize)> = Vec::new(); // (token index, start char)
    let mut pos = 0usize;
    for (k, t) in toks.iter().enumerate() {
        let (kind, body) = t.split_at(t.char_indices().nth(1).map(|(i, _)| i).unwrap_or(t.len()));
        match kind {
            "F" => { stack.push((k, pos)); pos += body.chars().count() + 1; }
            "L" => { let n = body.chars().count(); pos += 2 + n.to_string().len() + 1 + n + 1; }
            "I" => { pos += body.chars().count(); }
            ")" => { pos += 1; if let Some((f, s)) = stack.pop() { spans[f] = format!("{s},{pos},{k}"); } }
            "," => { pos += 1; }
            _ => break,
        }
    }
    Value::List(ListBuf::from(spans.into_iter().map(|s| Value::Str(intern_str(&s))).collect::<Vec<_>>()))
}

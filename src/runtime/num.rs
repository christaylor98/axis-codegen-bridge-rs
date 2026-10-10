//! Groomed number parsing (axMachina codegen layer, 2026-10-09): what the AI3 drivers spelled as ten `str_replace`
//! passes to validate digits before `str_to_int` (the hottest code in the P5 search's element ops).

use super::value::Value;

/// `str_nat_or(t: Text, d: Int) -> Int` — `t` read as a natural number when it is 1 to 18 ASCII digits (no sign, no
/// spaces), else `d`. Total: never panics (18 digits always fit an i64).
pub fn str_nat_or(t: std::sync::Arc<str>, d: i64) -> Value {
    let b = t.as_bytes();
    if b.is_empty() || b.len() > 18 || !b.iter().all(|c| c.is_ascii_digit()) { return Value::Int(d); }
    Value::Int(b.iter().fold(0i64, |n, c| n * 10 + (c - b'0') as i64))
}

/// `str_sim_ratio(a: Text, b: Text) -> Int` — Python difflib `SequenceMatcher(None, a, b).ratio()` (code points,
/// autojunk on: in b of 200+ elements, an element seen more than n/100 + 1 times is popular and never anchors a
/// match), as round(ratio * 10^12); both empty -> 10^12. For readers that rank rewrites by similarity (LU2's
/// replace-chain reader), so they agree with the reference without floats.
pub fn str_sim_ratio(a: std::sync::Arc<str>, b: std::sync::Arc<str>) -> Value {
    use std::collections::HashMap;
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let t = a.len() + b.len();
    if t == 0 { return Value::Int(1_000_000_000_000); }
    let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
    for (j, c) in b.iter().enumerate() { b2j.entry(*c).or_default().push(j); }
    let n = b.len();
    if n >= 200 {
        let ntest = n / 100 + 1;
        b2j.retain(|_, v| v.len() <= ntest);
    }
    let longest = |alo: usize, ahi: usize, blo: usize, bhi: usize| -> (usize, usize, usize) {
        let (mut bi, mut bj, mut bs) = (alo, blo, 0usize);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for i in alo..ahi {
            let mut nj: HashMap<usize, usize> = HashMap::new();
            if let Some(js) = b2j.get(&a[i]) {
                for &j in js {
                    if j < blo { continue; }
                    if j >= bhi { break; }
                    let k = j2len.get(&(j.wrapping_sub(1))).copied().unwrap_or(0) + 1;
                    let k = if j == 0 { 1 } else { k };
                    nj.insert(j, k);
                    if k > bs { bi = i + 1 - k; bj = j + 1 - k; bs = k; }
                }
            }
            j2len = nj;
        }
        // no junk (isjunk None): extend with equal elements on both sides (popular ones included)
        while bi > alo && bj > blo && a[bi - 1] == b[bj - 1] { bi -= 1; bj -= 1; bs += 1; }
        while bi + bs < ahi && bj + bs < bhi && a[bi + bs] == b[bj + bs] { bs += 1; }
        (bi, bj, bs)
    };
    let mut queue = vec![(0usize, a.len(), 0usize, b.len())];
    let mut m = 0usize;
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = longest(alo, ahi, blo, bhi);
        if k > 0 {
            m += k;
            if alo < i && blo < j { queue.push((alo, i, blo, j)); }
            if i + k < ahi && j + k < bhi { queue.push((i + k, ahi, j + k, bhi)); }
        }
    }
    let r = (2 * m) as f64 / t as f64;
    Value::Int((r * 1e12).round() as i64)
}

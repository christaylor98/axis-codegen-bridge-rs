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

use super::value::Value;
use rust_decimal::{Decimal, RoundingStrategy};

macro_rules! int_bin_op {
    ($name:ident, $op:tt) => {
        #[track_caller]
        pub fn $name(args: Value) -> Value {
            match args {
                Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
                    (Value::Int(x), Value::Int(y)) => Value::Int(x $op y),
                    _ => panic!(concat!(stringify!($name), ": expected two Int values")),
                },
                _ => panic!(concat!(stringify!($name), ": expected Tuple(Int, Int)")),
            }
        }
    };
}

/// Typed ordered comparison over one Value variant, returning Bool. Used for the
/// int_/dec_/float_ lt/lte/gt/gte families — each stays type-monomorphic (mixed
/// operands panic) so the emitter can pick the right one by operand type.
/// Float comparisons follow IEEE-754: any comparison with NaN is false.
macro_rules! cmp_op {
    ($name:ident, $variant:ident, $tyname:literal, $op:tt) => {
        #[track_caller]
        pub fn $name(args: Value) -> Value {
            match args {
                Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
                    (Value::$variant(x), Value::$variant(y)) => Value::Bool(x $op y),
                    _ => panic!(concat!(stringify!($name), ": expected two ", $tyname, " values")),
                },
                _ => panic!(concat!(stringify!($name), ": expected Tuple(", $tyname, ", ", $tyname, ")")),
            }
        }
    };
}

// int_add/int_sub/int_mul/int_lt are hand-written below with native params
// (AXVERITY_RAWMEM_CALL_CONVENTION_V1 convergence) instead of macro-generated
// — everything else in this family (int_lte/gt/gte, dec_*, float_*) stays on
// the macro/boxed convention, untouched. Same `+`/`-`/`*`/`<` operators as
// the macro would have generated — no semantic change, only calling
// convention.
#[track_caller]
pub fn int_add(x: i64, y: i64) -> Value { Value::Int(x + y) }
#[track_caller]
pub fn int_sub(x: i64, y: i64) -> Value { Value::Int(x - y) }
#[track_caller]
pub fn int_mul(x: i64, y: i64) -> Value { Value::Int(x * y) }
#[track_caller]
pub fn int_lt(x: i64, y: i64) -> Value { Value::Bool(x < y) }

#[track_caller]
pub fn int_lte(x: i64, y: i64) -> Value { Value::Bool(x <= y) }
#[track_caller]
pub fn int_gt(x: i64, y: i64) -> Value { Value::Bool(x > y) }
#[track_caller]
pub fn int_gte(x: i64, y: i64) -> Value { Value::Bool(x >= y) }

cmp_op!(dec_lt,  Dec, "Dec", <);
cmp_op!(dec_lte, Dec, "Dec", <=);
cmp_op!(dec_gt,  Dec, "Dec", >);
cmp_op!(dec_gte, Dec, "Dec", >=);
cmp_op!(float_lt,  Float, "Float", <);
cmp_op!(float_lte, Float, "Float", <=);
cmp_op!(float_gt,  Float, "Float", >);
cmp_op!(float_gte, Float, "Float", >=);

// ── int_div / int_mod: EUCLIDEAN, not truncated ───────────────────────────
// AXVERITY_FORMAT_LAND_AND_WIRE_V1 / P0 (hard-limit FIX_INT_MOD_FIRST).
//
// These were `/` and `%`, which truncate toward zero and therefore return a
// NEGATIVE remainder for a negative left operand. That is a live hazard for
// every M1 binary decoder, because M1 has no byte-width load: the only way to
// read a byte is `mem_read_int_raw` (an 8-byte SIGNED i64 read) followed by
// arithmetic. Any byte with the top bit set at a record boundary makes that
// word negative, `int_mod(v, 256)` then returns a negative "byte", it is added
// to a cursor as a varint length, and the cursor walks BACKWARDS off the front
// of the buffer — surfacing as `mem_read_int_raw: offset must be >= 0, got
// -83` (0xAD decoded as 173-256). It surfaced only because one probe corpus
// had random payload bytes; an ASCII structure stream never sets the top bit
// and never triggers it.
//
// Euclidean semantics fix this at the primitive rather than at a call site:
// `rem_euclid` is always in [0, |y|), so a decoded byte is always a byte.
//
// BOTH are changed, and that is deliberate. For a power-of-two divisor
// Euclidean division is exactly an arithmetic shift and Euclidean remainder is
// exactly a bit mask, so `int_mod(int_div(v, 256^k), 256)` extracts byte k of
// v's two's-complement representation correctly at EVERY k — which is what a
// decoder needs. Fixing only the remainder would leave `int_div` rounding
// toward zero, so extracting any byte above the lowest would still be silently
// wrong on a negative word: the same hazard, half-fixed, which is the failure
// mode the fix exists to prevent.
//
// Blast radius, checked rather than assumed: every existing `int_div`/`int_mod`
// call site in both M1 trees takes a non-negative left operand (loop indices,
// lengths, elapsed-time deltas, and a hash kept in range by construction), and
// Euclidean and truncated agree exactly on non-negative operands. No current
// caller's behaviour changes.
//
// Reproduction: experiments/intmod-m1/ sweeps all 256 byte values through a
// real record boundary in real raw memory. Before this change: 128 failures,
// first at byte 128, and 0xAD decodes to -83. After: 0 failures.
#[track_caller]
pub fn int_div(x: i64, y: i64) -> Value {
    if y == 0 { panic!("int_div: division by zero") }
    Value::Int(x.div_euclid(y))
}

#[track_caller]
pub fn int_div_checked(x: i64, y: i64) -> Value {
    if y == 0 {
        super::option::option_none()
    } else {
        super::option::option_some(Value::Int(x.div_euclid(y)))
    }
}

#[track_caller]
pub fn int_mod(x: i64, y: i64) -> Value {
    if y == 0 { panic!("int_mod: division by zero") }
    Value::Int(x.rem_euclid(y))
}

// value_eq has a second registered alias `__eq__` -> the SAME Rust symbol
// (src/emit/rust_05.rs). Any native_call_fn_arg_types entry for value_eq
// MUST be mirrored under "__eq__" too, or a CCall targeting __eq__ falls
// back to the stale boxed convention against this native signature.
#[track_caller]
pub fn value_eq(a: Value, b: Value) -> Value {
    Value::Bool(a == b)
}

#[track_caller]
pub fn int_to_str(n: i64) -> Value {
    Value::Str(super::value::intern_str(&n.to_string()))
}

#[track_caller]
pub fn str_to_int(s: std::sync::Arc<str>) -> Value {
    Value::Int(s.parse().unwrap_or(0))
}

#[track_caller]
pub fn int_abs(n: i64) -> Value {
    Value::Int(n.abs())
}

#[track_caller]
pub fn int_min(a: i64, b: i64) -> Value {
    Value::Int(a.min(b))
}

#[track_caller]
pub fn int_max(a: i64, b: i64) -> Value {
    Value::Int(a.max(b))
}

#[track_caller]
pub fn int_clamp(v: i64, lo: i64, hi: i64) -> Value {
    Value::Int(v.max(lo).min(hi))
}

#[track_caller]
pub fn celsius_to_fahrenheit(c: i64) -> Value {
    Value::Int((c * 9 / 5) + 32)
}

#[track_caller]
pub fn fahrenheit_to_celsius(f: i64) -> Value {
    Value::Int((f - 32) * 5 / 9)
}

#[track_caller]
pub fn is_positive(n: i64) -> Value {
    Value::Bool(n > 0)
}

#[track_caller]
pub fn int_eq(x: i64, y: i64) -> Value {
    Value::Bool(x == y)
}

/// dec_eq(Dec, Dec) -> Bool. Typed exact equality on rust_decimal::Decimal —
/// the Dec-typed counterpart of int_eq. Decimal equality is exact (no scaling
/// surprises: 1.0 == 1.00 is true, matching Decimal's PartialEq).
#[track_caller]
pub fn dec_eq(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Dec(x), Value::Dec(y)) => Value::Bool(x == y),
            _ => panic!("dec_eq: expected two Dec values"),
        },
        _ => panic!("dec_eq: expected Tuple(Dec, Dec)"),
    }
}

/// dec_div(Dec, Dec) -> Dec. Decimal division scaled to 16 fractional digits,
/// rounded half-away-from-zero — chosen to reproduce real Postgres 15's numeric
/// AVG output byte-for-byte (e.g. AVG(1,2) -> "1.5000000000000000", AVG(5,3) ->
/// "1.6666666666666667"). round_dp fixes the scale to exactly 16 (padding trailing
/// zeros), so to_string matches PG. Division by zero panics (AVG's finalizer never
/// calls this with count 0 — an empty group short-circuits to NULL before here).
/// AXVERITY_PGWIRE_SUM_AVG_MIN_MAX_V1.
#[track_caller]
pub fn dec_div(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Dec(x), Value::Dec(y)) => {
                if *y == Decimal::ZERO { panic!("dec_div: division by zero") }
                // round_dp fixes the rounding at 16 dp (half-away-from-zero, matching
                // PG); rescale then pads trailing zeros to a fixed scale of 16 so
                // to_string yields PG's "1.5000000000000000" rather than "1.5".
                let mut q = (x / y).round_dp_with_strategy(16, RoundingStrategy::MidpointAwayFromZero);
                q.rescale(16);
                Value::Dec(q)
            }
            _ => panic!("dec_div: expected two Dec values"),
        },
        _ => panic!("dec_div: expected Tuple(Dec, Dec)"),
    }
}

/// dec_to_text(Dec) -> Text. Render a Decimal to its canonical decimal string via
/// Decimal::to_string (scale-preserving, so a scale-16 value keeps its trailing
/// zeros). The Dec-typed counterpart of int_to_str; the seam that puts a computed
/// AVG on the wire as a numeric text value. AXVERITY_PGWIRE_SUM_AVG_MIN_MAX_V1.
#[track_caller]
pub fn dec_to_text(d: Value) -> Value {
    match d {
        Value::Dec(x) => Value::Str(super::value::intern_str(&x.to_string())),
        _ => panic!("dec_to_text: expected Dec"),
    }
}

/// float_eq(Float, Float) -> Bool. Typed IEEE-754 f64 equality — the Float-typed
/// counterpart of int_eq. Uses the standard `==`, so NaN != NaN and +0.0 == -0.0,
/// identical to how value_eq already compares Value::Float. Exact bit-equality is
/// a footgun for computed floats; callers wanting a tolerance must compose it in
/// M1.
#[track_caller]
pub fn float_eq(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Float(x), Value::Float(y)) => Value::Bool(x == y),
            _ => panic!("float_eq: expected two Float values"),
        },
        _ => panic!("float_eq: expected Tuple(Float, Float)"),
    }
}

/// Identity for unit: discards input, returns Unit.
#[track_caller]
pub fn unit_id(_args: Value) -> Value {
    Value::Unit
}

/// Sequence two unit-producing computations: takes Tuple(Unit, Unit), returns Unit.
#[track_caller]
pub fn seq_unit(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => {
            match (&es[0], &es[1]) {
                (Value::Unit, Value::Unit) => Value::Unit,
                _ => panic!("seq_unit: expected Tuple(Unit, Unit)"),
            }
        }
        Value::Unit => Value::Unit,
        _ => panic!("seq_unit: expected Tuple(Unit, Unit) or Unit"),
    }
}

/// Sequence a computation before a result of any type: `seq(Tuple(a, b)) -> b`.
///
/// The first argument is evaluated purely for its ordering/effect (it is already
/// materialised as its own `let node_N` by the time `seq` runs) and the second is
/// returned unchanged. The M1 compiler injects `seq` when lowering a discarded
/// side-effecting binding inside an `if` arm (`let _ = eff(); tail`), so that the
/// effect becomes a data-dependency of the arm's result and the branch-scoping
/// emitter keeps it inside that arm (BRANCH_SCOPING_V1). Unlike `seq_unit` this is
/// type-agnostic in both positions, since a branch result may be any Value.
// CAUTION (flagged for review): `seq` is compiler-injected by
// nf_lowering.rs's seq_scope_arm_effects for BRANCH_SCOPING_V1 — always
// called with exactly 2 args by construction, so native conversion is
// arg-count-safe, but this fn is correctness-critical for branch-effect
// scoping (a prior silent-wrong-behavior bug). Recommend extra scrutiny /
// a real `if`/`else` branch-effect test before trusting this conversion.
#[track_caller]
pub fn seq(_eff: Value, result: Value) -> Value {
    result
}

#[cfg(test)]
mod dec_agg_tests {
    use super::*;
    use super::super::coerce::int_to_dec;

    fn div(a: i64, b: i64) -> String {
        let q = dec_div(Value::Tuple(vec![int_to_dec(Value::Int(a)), int_to_dec(Value::Int(b))]));
        match dec_to_text(q) { Value::Str(s) => s.to_string(), _ => panic!("expected Str") }
    }

    #[test]
    fn avg_matches_pg15_scale_and_rounding() {
        // real PG-15 numeric AVG(int): scale 16, half-away-from-zero
        assert_eq!(div(3, 2), "1.5000000000000000");   // AVG(1,2)
        assert_eq!(div(6, 3), "2.0000000000000000");   // AVG(1,2,3)
        assert_eq!(div(5, 3), "1.6666666666666667");   // repeating -> rounds up
        assert_eq!(div(2, 3), "0.6666666666666667");
        assert_eq!(div(1, 1), "1.0000000000000000");
    }
}

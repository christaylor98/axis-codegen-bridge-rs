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

int_bin_op!(int_add, +);
int_bin_op!(int_sub, -);
int_bin_op!(int_mul, *);
cmp_op!(int_lt,  Int, "Int", <);
cmp_op!(int_lte, Int, "Int", <=);
cmp_op!(int_gt,  Int, "Int", >);
cmp_op!(int_gte, Int, "Int", >=);
cmp_op!(dec_lt,  Dec, "Dec", <);
cmp_op!(dec_lte, Dec, "Dec", <=);
cmp_op!(dec_gt,  Dec, "Dec", >);
cmp_op!(dec_gte, Dec, "Dec", >=);
cmp_op!(float_lt,  Float, "Float", <);
cmp_op!(float_lte, Float, "Float", <=);
cmp_op!(float_gt,  Float, "Float", >);
cmp_op!(float_gte, Float, "Float", >=);

#[track_caller]
pub fn int_div(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Int(x), Value::Int(y)) => {
                if *y == 0 { panic!("int_div: division by zero") }
                Value::Int(x / y)
            }
            _ => panic!("int_div: expected two Int values"),
        },
        _ => panic!("int_div: expected Tuple(Int, Int)"),
    }
}

#[track_caller]
pub fn int_div_checked(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Int(x), Value::Int(y)) => {
                if *y == 0 {
                    super::option::option_none()
                } else {
                    super::option::option_some(Value::Int(x / y))
                }
            }
            _ => panic!("int_div_checked: expected two Int values"),
        },
        _ => panic!("int_div_checked: expected Tuple(Int, Int)"),
    }
}

#[track_caller]
pub fn int_mod(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Int(x), Value::Int(y)) => {
                if *y == 0 { panic!("int_mod: division by zero") }
                Value::Int(x % y)
            }
            _ => panic!("int_mod: expected two Int values"),
        },
        _ => panic!("int_mod: expected Tuple(Int, Int)"),
    }
}

#[track_caller]
pub fn value_eq(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => Value::Bool(es[0] == es[1]),
        _ => panic!("value_eq: expected Tuple with 2 elements"),
    }
}

#[track_caller]
pub fn int_to_str(n: Value) -> Value {
    match n {
        Value::Int(i) => Value::Str(super::value::intern_str(&i.to_string())),
        _ => panic!("int_to_str: expected Int"),
    }
}

#[track_caller]
pub fn str_to_int(s: Value) -> Value {
    match s {
        Value::Str(h) => {
            let text = super::value::get_str(h);
            Value::Int(text.parse().unwrap_or(0))
        }
        _ => panic!("str_to_int: expected Str"),
    }
}

#[track_caller]
pub fn int_abs(n: Value) -> Value {
    match n {
        Value::Int(i) => Value::Int(i.abs()),
        _ => panic!("int_abs: expected Int"),
    }
}

#[track_caller]
pub fn int_min(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Int(a), Value::Int(b)) => Value::Int((*a).min(*b)),
            _ => panic!("int_min: expected two Int values"),
        },
        _ => panic!("int_min: expected Tuple(Int, Int)"),
    }
}

#[track_caller]
pub fn int_max(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Int(a), Value::Int(b)) => Value::Int((*a).max(*b)),
            _ => panic!("int_max: expected two Int values"),
        },
        _ => panic!("int_max: expected Tuple(Int, Int)"),
    }
}

#[track_caller]
pub fn int_clamp(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 3 => match (&es[0], &es[1], &es[2]) {
            (Value::Int(v), Value::Int(lo), Value::Int(hi)) => {
                Value::Int((*v).max(*lo).min(*hi))
            }
            _ => panic!("int_clamp: expected three Int values"),
        },
        _ => panic!("int_clamp: expected Tuple(Int, Int, Int)"),
    }
}

#[track_caller]
pub fn celsius_to_fahrenheit(c: Value) -> Value {
    match c {
        Value::Int(n) => Value::Int((n * 9 / 5) + 32),
        _ => panic!("celsius_to_fahrenheit: expected Int"),
    }
}

#[track_caller]
pub fn fahrenheit_to_celsius(f: Value) -> Value {
    match f {
        Value::Int(n) => Value::Int((n - 32) * 5 / 9),
        _ => panic!("fahrenheit_to_celsius: expected Int"),
    }
}

#[track_caller]
pub fn is_positive(n: Value) -> Value {
    match n {
        Value::Int(i) => Value::Bool(i > 0),
        _ => panic!("is_positive: expected Int"),
    }
}

#[track_caller]
pub fn int_eq(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Int(x), Value::Int(y)) => Value::Bool(x == y),
            _ => panic!("int_eq: expected two Int values"),
        },
        _ => panic!("int_eq: expected Tuple(Int, Int)"),
    }
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
#[track_caller]
pub fn seq(args: Value) -> Value {
    match args {
        Value::Tuple(mut es) if es.len() >= 2 => es.swap_remove(1),
        _ => panic!("seq: expected Tuple(_, _)"),
    }
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

use super::value::Value;

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

macro_rules! int_cmp_op {
    ($name:ident, $op:tt) => {
        #[track_caller]
        pub fn $name(args: Value) -> Value {
            match args {
                Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
                    (Value::Int(x), Value::Int(y)) => Value::Bool(x $op y),
                    _ => panic!(concat!(stringify!($name), ": expected two Int values")),
                },
                _ => panic!(concat!(stringify!($name), ": expected Tuple(Int, Int)")),
            }
        }
    };
}

int_bin_op!(int_add, +);
int_bin_op!(int_sub, -);
int_bin_op!(int_mul, *);
int_cmp_op!(int_lt,  <);
int_cmp_op!(int_lte, <=);
int_cmp_op!(int_gt,  >);
int_cmp_op!(int_gte, >=);

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

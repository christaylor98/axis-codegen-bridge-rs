use super::value::Value;

macro_rules! int_bin_op {
    ($name:ident, $op:tt) => {
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

pub fn value_eq(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => Value::Bool(es[0] == es[1]),
        _ => panic!("value_eq: expected Tuple with 2 elements"),
    }
}

pub fn int_to_str(n: Value) -> Value {
    match n {
        Value::Int(i) => Value::Str(super::value::intern_str(&i.to_string())),
        _ => panic!("int_to_str: expected Int"),
    }
}

pub fn str_to_int(s: Value) -> Value {
    match s {
        Value::Str(h) => {
            let text = super::value::get_str(h);
            Value::Int(text.parse().unwrap_or(0))
        }
        _ => panic!("str_to_int: expected Str"),
    }
}

pub fn int_abs(n: Value) -> Value {
    match n {
        Value::Int(i) => Value::Int(i.abs()),
        _ => panic!("int_abs: expected Int"),
    }
}

pub fn int_eq(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Int(x), Value::Int(y)) => Value::Bool(x == y),
            _ => panic!("int_eq: expected two Int values"),
        },
        _ => panic!("int_eq: expected Tuple(Int, Int)"),
    }
}

/// Identity for unit: discards input, returns Unit.
pub fn unit_id(_args: Value) -> Value {
    Value::Unit
}

/// Sequence two unit-producing computations: takes Tuple(Unit, Unit), returns Unit.
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

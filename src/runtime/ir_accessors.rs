use super::value::{Value, get_tag_name, intern_str};

fn expect_ctor(name: &str, v: Value) -> (String, Vec<Value>) {
    match v {
        Value::Ctor { tag, fields } => (get_tag_name(tag), fields),
        other => panic!("{}: expected Ctor term, got {:?}", name, other),
    }
}

/// Returns lowercase kind name: 'int' | 'bool' | 'unit' | 'var' | 'lam' | 'let' | 'if' | 'app'
pub fn ir_get_kind(v: Value) -> Value {
    let (kind, _) = expect_ctor("ir_get_kind", v);
    let short = match kind.as_str() {
        "IntLit"  => "int",
        "BoolLit" => "bool",
        "UnitLit" => "unit",
        "Var"     => "var",
        "Lam"     => "lam",
        "Let"     => "let",
        "If"      => "if",
        "App"     => "app",
        "Call"    => "call",
        other     => other,
    };
    Value::Str(intern_str(short))
}

/// Returns var name (Var), binding name (Let), or param name (Lam).
pub fn ir_get_name(v: Value) -> Value {
    let (kind, fields) = expect_ctor("ir_get_name", v);
    match kind.as_str() {
        "Var" | "Lam" | "Let" => match &fields[0] {
            Value::Str(h) => Value::Str(*h),
            other => panic!("ir_get_name: name field is not Str, got {:?}", other),
        },
        other => panic!("ir_get_name: term kind '{}' has no name field", other),
    }
}

/// Returns the integer value of an IntLit term.
pub fn ir_get_int_val(v: Value) -> Value {
    let (kind, fields) = expect_ctor("ir_get_int_val", v);
    if kind != "IntLit" {
        panic!("ir_get_int_val: expected IntLit, got '{}'", kind);
    }
    match &fields[0] {
        Value::Int(n) => Value::Int(*n),
        other => panic!("ir_get_int_val: IntLit field not Int, got {:?}", other),
    }
}

/// Returns the fn (function) field of an App term.
pub fn ir_get_fn(v: Value) -> Value {
    let (kind, mut fields) = expect_ctor("ir_get_fn", v);
    if kind != "App" {
        panic!("ir_get_fn: expected App, got '{}'", kind);
    }
    fields.remove(0)
}

/// Returns the arg field of an App term.
pub fn ir_get_arg(v: Value) -> Value {
    let (kind, mut fields) = expect_ctor("ir_get_arg", v);
    if kind != "App" {
        panic!("ir_get_arg: expected App, got '{}'", kind);
    }
    fields.remove(1)
}

/// Returns the body field of a Lam or Let term.
pub fn ir_get_body(v: Value) -> Value {
    let (kind, mut fields) = expect_ctor("ir_get_body", v);
    match kind.as_str() {
        "Lam" => fields.remove(1),
        "Let" => fields.remove(2),
        other => panic!("ir_get_body: expected Lam or Let, got '{}'", other),
    }
}

/// Returns the value field of a Let term.
pub fn ir_get_value(v: Value) -> Value {
    let (kind, mut fields) = expect_ctor("ir_get_value", v);
    if kind != "Let" {
        panic!("ir_get_value: expected Let, got '{}'", kind);
    }
    fields.remove(1)
}

/// Returns the cond field of an If term.
pub fn ir_get_cond(v: Value) -> Value {
    let (kind, mut fields) = expect_ctor("ir_get_cond", v);
    if kind != "If" {
        panic!("ir_get_cond: expected If, got '{}'", kind);
    }
    fields.remove(0)
}

/// Returns the then field of an If term.
pub fn ir_get_then(v: Value) -> Value {
    let (kind, mut fields) = expect_ctor("ir_get_then", v);
    if kind != "If" {
        panic!("ir_get_then: expected If, got '{}'", kind);
    }
    fields.remove(1)
}

/// Returns the else field of an If term.
pub fn ir_get_else(v: Value) -> Value {
    let (kind, mut fields) = expect_ctor("ir_get_else", v);
    if kind != "If" {
        panic!("ir_get_else: expected If, got '{}'", kind);
    }
    fields.remove(2)
}

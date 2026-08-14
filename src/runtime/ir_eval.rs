/// Substitution-based evaluator for Value::Ctor Core IR terms.
///
/// Evaluation strategy: call-by-value with capture-avoiding substitution.
/// Lam is a value (returned as-is). App substitutes the argument into the body
/// rather than extending an environment, so closures are not needed — the
/// self-application recursion pattern works correctly.
///
/// CCall dispatch is a runtime table independent of emit/rust.rs.

use super::value::{Value, get_str, get_tag_name, truthy};
use super::ir_constructors::subst_value;
use std::collections::HashMap;
use std::sync::OnceLock;

type PrimFn = fn(Value) -> Value;

// AXVERITY_RAWMEM_CALL_CONVENTION_V1: this dispatch table needs a uniform
// `fn(Value) -> Value` per PrimFn, but the underlying fns below were
// converted to native positional params (for the emit/rust_05.rs codegen
// path this module's own doc comment says it's independent of). These
// thin wrappers unpack the same `Value::Tuple`/single-`Value` shape the
// originals matched and delegate to the native fn — this evaluator's own
// calling convention is unaffected either way.
fn int_add_w(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Int(x), Value::Int(y)) => super::arith::int_add(*x, *y),
            _ => panic!("int_add: expected two Int values"),
        },
        _ => panic!("int_add: expected Tuple(Int, Int)"),
    }
}
fn int_sub_w(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Int(x), Value::Int(y)) => super::arith::int_sub(*x, *y),
            _ => panic!("int_sub: expected two Int values"),
        },
        _ => panic!("int_sub: expected Tuple(Int, Int)"),
    }
}
fn int_mul_w(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Int(x), Value::Int(y)) => super::arith::int_mul(*x, *y),
            _ => panic!("int_mul: expected two Int values"),
        },
        _ => panic!("int_mul: expected Tuple(Int, Int)"),
    }
}
fn int_lt_w(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Int(x), Value::Int(y)) => super::arith::int_lt(*x, *y),
            _ => panic!("int_lt: expected two Int values"),
        },
        _ => panic!("int_lt: expected Tuple(Int, Int)"),
    }
}
fn int_to_str_w(n: Value) -> Value {
    match n {
        Value::Int(i) => super::arith::int_to_str(i),
        _ => panic!("int_to_str: expected Int"),
    }
}
fn str_to_int_w(s: Value) -> Value {
    match s {
        Value::Str(h) => super::arith::str_to_int(h),
        _ => panic!("str_to_int: expected Str"),
    }
}
fn str_concat_w(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => match (&es[0], &es[1]) {
            (Value::Str(a), Value::Str(b)) => super::str_ops::str_concat(a.clone(), b.clone()),
            _ => panic!("str_concat: expected two Str values"),
        },
        _ => panic!("str_concat: expected Tuple(Str, Str)"),
    }
}
fn list_get_w(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => {
            let idx = match &es[1] { Value::Int(n) => *n, _ => panic!("list_get: expected Int index") };
            super::list::list_get(es[0].clone(), idx)
        }
        _ => panic!("list_get: expected Tuple(List, Int)"),
    }
}
fn list_append_w(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => super::list::list_append(es[0].clone(), es[1].clone()),
        _ => panic!("list_append: expected Tuple(List, elem)"),
    }
}
fn tuple_field_w(args: Value) -> Value {
    match args {
        Value::Tuple(ref es) if es.len() >= 2 => {
            let idx = match &es[1] { Value::Int(n) => *n, other => panic!("tuple_field: arg 1 expected Int, got {:?}", other) };
            super::tuple::tuple_field(es[0].clone(), idx)
        }
        other => panic!("tuple_field: expected Tuple(Value, Int), got {:?}", other),
    }
}
fn argv_get_w(idx: Value) -> Value {
    match idx {
        Value::Int(n) => super::process::argv_get(n),
        _ => super::process::argv_get(0),
    }
}

fn dispatch_table() -> &'static HashMap<&'static str, PrimFn> {
    static TABLE: OnceLock<HashMap<&'static str, PrimFn>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut m: HashMap<&'static str, PrimFn> = HashMap::new();

        // Arithmetic
        m.insert("int_add",         int_add_w);
        m.insert("int_sub",         int_sub_w);
        m.insert("int_mul",         int_mul_w);
        m.insert("int_div",         super::arith::int_div);
        m.insert("int_div_checked", super::arith::int_div_checked);
        m.insert("int_mod",         super::arith::int_mod);
        m.insert("int_to_str",      int_to_str_w);
        m.insert("str_to_int",      str_to_int_w);
        m.insert("int_lt",          int_lt_w);
        m.insert("int_lte",         super::arith::int_lte);
        m.insert("int_gt",          super::arith::int_gt);
        m.insert("int_gte",         super::arith::int_gte);
        m.insert("value_eq",        super::arith::value_eq);

        // Boolean
        m.insert("bool_and",        super::bool_ops::bool_and);
        m.insert("bool_or",         super::bool_ops::bool_or);
        m.insert("bool_not",        super::bool_ops::bool_not);

        // String
        m.insert("str_len",         super::str_ops::str_len);
        m.insert("str_concat",      str_concat_w);
        m.insert("str_char",        super::str_ops::str_char);
        m.insert("str_char_at",     super::str_ops::str_char_at);
        m.insert("str_char_code",   super::str_ops::str_char_code);
        m.insert("str_slice",       super::str_ops::str_slice);
        m.insert("str_split",       super::str_ops::str_split);
        m.insert("str_starts_with", super::str_ops::str_starts_with);
        m.insert("str_ends_with",   super::str_ops::str_ends_with);
        m.insert("str_trim",        super::str_ops::str_trim);
        m.insert("str_contains",    super::str_ops::str_contains);
        m.insert("str_index_of",    super::str_ops::str_index_of);
        m.insert("chr",             super::str_ops::chr);

        // List
        m.insert("list_nil",        super::list::list_nil);
        m.insert("list_cons",       super::list::list_cons);
        m.insert("list_len",        super::list::list_len);
        m.insert("list_get",        list_get_w);
        m.insert("list_get_at",             super::list::list_get_at);
        m.insert("list_get_println_if_some",   super::list::list_get_println_if_some);
        m.insert("list_str_len_lte_if_some",   super::list::list_str_len_lte_if_some);
        m.insert("list_append",             list_append_w);
        m.insert("list_concat",     super::list::list_concat);
        m.insert("list_reverse",    super::list::list_reverse);
        m.insert("list_head",       super::list::list_head);
        m.insert("list_tail",       super::list::list_tail);
        m.insert("list_is_empty",   super::list::list_is_empty);

        // Tuple / Ctor
        m.insert("tuple_field",     tuple_field_w);
        m.insert("ctor_field",      super::tuple::ctor_field);

        // Option
        m.insert("option_none",     super::option::option_none_fn);
        m.insert("option_some",     super::option::option_some);
        m.insert("option_is_none",  super::option::option_is_none);
        m.insert("option_is_some",  super::option::option_is_some);
        m.insert("option_unwrap",   super::option::option_unwrap);

        // IO
        m.insert("io_print",        super::io::io_print);
        m.insert("io_println",      super::io::io_println);
        m.insert("io_eprint",       super::io::io_eprint);
        m.insert("io_read_line",    super::io::io_read_line);
        m.insert("fs_read_text",    super::io::fs_read_text);
        m.insert("fs_write_text",   super::io::fs_write_text);
        m.insert("fs_append_text",  super::io::fs_append_text);
        m.insert("fs_append_text_durable", super::io::fs_append_text_durable);
        m.insert("fs_file_exists",  super::io::fs_file_exists);
        m.insert("debug_trace",     super::io::debug_trace);

        // TTY (AXVERITY_GC_TUI_V1)
        m.insert("tty_raw_on",      super::tty::tty_raw_on);
        m.insert("tty_raw_off",     super::tty::tty_raw_off);
        m.insert("tty_read_key",    super::tty::tty_read_key);
        m.insert("tty_rows",        super::tty::tty_rows);
        m.insert("tty_cols",        super::tty::tty_cols);

        // Process
        m.insert("proc_args",       super::process::proc_args);
        m.insert("proc_exit",       super::process::proc_exit);
        m.insert("proc_sleep",      super::process::proc_sleep);
        m.insert("now_unix_nanos",  super::process::now_unix_nanos);
        m.insert("hotwrite_batch_run",   super::hotwrite_batch::hotwrite_batch_run);
        m.insert("hotwrite_batch_run_c", super::hotwrite_batch::hotwrite_batch_run_c);
        m.insert("hotwrite_batch_run_c_durable", super::hotwrite_batch::hotwrite_batch_run_c_durable);
        m.insert("argv",            super::process::argv);
        m.insert("argv_get",        argv_get_w);
        m.insert("argv_int",        super::process::argv_int);
        m.insert("argv_count",      super::process::argv_count);
        m.insert("argv_or",         super::process::argv_or);

        // IR Accessors
        m.insert("ir_get_kind",     super::ir_accessors::ir_get_kind);
        m.insert("ir_get_name",     super::ir_accessors::ir_get_name);
        m.insert("ir_get_int_val",  super::ir_accessors::ir_get_int_val);
        m.insert("ir_get_fn",       super::ir_accessors::ir_get_fn);
        m.insert("ir_get_arg",      super::ir_accessors::ir_get_arg);
        m.insert("ir_get_body",     super::ir_accessors::ir_get_body);
        m.insert("ir_get_value",    super::ir_accessors::ir_get_value);
        m.insert("ir_get_cond",     super::ir_accessors::ir_get_cond);
        m.insert("ir_get_then",     super::ir_accessors::ir_get_then);
        m.insert("ir_get_else",     super::ir_accessors::ir_get_else);

        m
    })
}

/// Evaluate a Core IR term (Value::Ctor) to a runtime Value.
/// Non-Ctor values (Int, Bool, List, etc.) are runtime values and returned as-is.
fn eval(term: Value) -> Value {
    match term {
        Value::Ctor { tag, mut fields } => {
            let kind = get_tag_name(tag);
            match kind.as_str() {
                "IntLit"  => fields.remove(0),
                "BoolLit" => fields.remove(0),
                "UnitLit" => Value::Unit,

                "Var" => {
                    let name = match fields.first() {
                        Some(Value::Str(h)) => get_str(h),
                        _ => "<unknown>".to_string(),
                    };
                    panic!("ir_eval: unbound variable: {}", name)
                }

                // Lam is a value — returned without evaluating the body.
                "Lam" => Value::Ctor { tag, fields },

                "Let" => {
                    let name = match fields.remove(0) {
                        Value::Str(h) => get_str(h),
                        other => panic!("ir_eval: Let: name is not Str, got {:?}", other),
                    };
                    let val  = eval(fields.remove(0));
                    let body = fields.remove(0);
                    eval(subst_value(&name, &val, body))
                }

                "If" => {
                    let cond = eval(fields.remove(0));
                    let then = fields.remove(0);
                    let els  = fields.remove(0);
                    if truthy(&cond) { eval(then) } else { eval(els) }
                }

                "App" => {
                    let fn_val  = eval(fields.remove(0));
                    let arg_val = eval(fields.remove(0));
                    apply(fn_val, arg_val)
                }

                "Call" => {
                    let target = match fields.remove(0) {
                        Value::Str(h) => get_str(h),
                        other => panic!("ir_eval: Call: target not Str, got {:?}", other),
                    };
                    let args: Vec<Value> = match fields.remove(0) {
                        Value::List(args) => args.into_iter().map(eval).collect(),
                        other => panic!("ir_eval: Call: args not List, got {:?}", other),
                    };
                    let tbl = dispatch_table();
                    let f = tbl.get(target.as_str())
                        .unwrap_or_else(|| panic!("ir_eval: unknown function: {}", target));
                    match args.len() {
                        0 => f(Value::Unit),
                        1 => f(args.into_iter().next().unwrap()),
                        _ => f(Value::Tuple(args)),
                    }
                }

                // Unknown Ctor tag — treat as an opaque data value.
                _ => Value::Ctor { tag, fields },
            }
        }
        // Already a runtime value (Int, Bool, Str, List, Tuple, or data Ctor).
        other => other,
    }
}

/// Apply a Lam Ctor to an argument via substitution.
fn apply(fn_val: Value, arg_val: Value) -> Value {
    match fn_val {
        Value::Ctor { tag, mut fields } if get_tag_name(tag) == "Lam" => {
            let param = match fields.remove(0) {
                Value::Str(h) => get_str(h),
                other => panic!("ir_eval: apply: Lam param not Str, got {:?}", other),
            };
            let body = fields.remove(0);
            eval(subst_value(&param, &arg_val, body))
        }
        _ => panic!("ir_eval: App: expected Lam, got {:?}", fn_val),
    }
}

/// Pre-substitute bindings into term then evaluate.
/// bindings is List of Tuple(Str name, Value val).
fn apply_bindings(term: Value, bindings_val: &Value) -> Value {
    match bindings_val {
        Value::List(pairs) => pairs.iter().fold(term, |t, pair| {
            match pair {
                Value::Tuple(kv) if kv.len() == 2 => {
                    let name = match &kv[0] {
                        Value::Str(h) => get_str(h),
                        _ => panic!("ir_eval: binding key not Str"),
                    };
                    subst_value(&name, &kv[1], t)
                }
                _ => panic!("ir_eval: binding entry not Tuple([Str, Value])"),
            }
        }),
        _ => panic!("ir_eval: bindings not a List"),
    }
}

/// ir_eval: takes Tuple(term, bindings) where bindings is List of Tuple(Str, Value).
/// Pre-substitutes bindings, then evaluates the closed term.
#[track_caller]
pub fn ir_eval(v: Value) -> Value {
    match v {
        Value::Tuple(mut fields) if fields.len() == 2 => {
            let bindings_val = fields.pop().unwrap();
            let term         = fields.pop().unwrap();
            eval(apply_bindings(term, &bindings_val))
        }
        _ => panic!("ir_eval: expected Tuple([term, bindings]), got {:?}", v),
    }
}

/// ir_apply: takes Tuple(lam_term, arg). Applies lam to arg via substitution.
#[track_caller]
pub fn ir_apply(v: Value) -> Value {
    match v {
        Value::Tuple(mut fields) if fields.len() == 2 => {
            let arg = fields.pop().unwrap();
            let lam = fields.pop().unwrap();
            apply(lam, arg)
        }
        _ => panic!("ir_apply: expected Tuple([lam, arg]), got {:?}", v),
    }
}

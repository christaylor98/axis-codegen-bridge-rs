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

fn dispatch_table() -> &'static HashMap<&'static str, PrimFn> {
    static TABLE: OnceLock<HashMap<&'static str, PrimFn>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut m: HashMap<&'static str, PrimFn> = HashMap::new();

        // Arithmetic
        m.insert("int_add",         super::arith::int_add);
        m.insert("int_sub",         super::arith::int_sub);
        m.insert("int_mul",         super::arith::int_mul);
        m.insert("int_div",         super::arith::int_div);
        m.insert("int_div_checked", super::arith::int_div_checked);
        m.insert("int_mod",         super::arith::int_mod);
        m.insert("int_to_str",      super::arith::int_to_str);
        m.insert("str_to_int",      super::arith::str_to_int);
        m.insert("int_lt",          super::arith::int_lt);
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
        m.insert("str_concat",      super::str_ops::str_concat);
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

        // List
        m.insert("list_nil",        super::list::list_nil);
        m.insert("list_cons",       super::list::list_cons);
        m.insert("list_len",        super::list::list_len);
        m.insert("list_get",        super::list::list_get);
        m.insert("list_get_at",     super::list::list_get_at);
        m.insert("list_append",     super::list::list_append);
        m.insert("list_concat",     super::list::list_concat);
        m.insert("list_reverse",    super::list::list_reverse);
        m.insert("list_head",       super::list::list_head);
        m.insert("list_tail",       super::list::list_tail);
        m.insert("list_is_empty",   super::list::list_is_empty);

        // Tuple / Ctor
        m.insert("tuple_field",     super::tuple::tuple_field);
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
        m.insert("debug_trace",     super::io::debug_trace);

        // Process
        m.insert("proc_args",       super::process::proc_args);
        m.insert("proc_exit",       super::process::proc_exit);
        m.insert("argv",            super::process::argv);
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
                        Some(Value::Str(h)) => get_str(*h),
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
                        Value::Str(h) => get_str(*h),
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

/// Emit Rust source from a Core IR term.
///
/// Generated code links against axis_codegen_bridge and calls runtime
/// functions by their canonical names. No foreign stubs are generated here —
/// all foreign functions are implemented in the bridge library.

use crate::core_ir::CoreTerm;
use std::collections::HashMap;

/// Canonical symbol → Rust path in axis_codegen_bridge.
fn symbol_map() -> HashMap<&'static str, &'static str> {
    let mut m = HashMap::new();

    // Arithmetic
    m.insert("int_add",         "axis_codegen_bridge::runtime::arith::int_add");
    m.insert("int_sub",         "axis_codegen_bridge::runtime::arith::int_sub");
    m.insert("int_mul",         "axis_codegen_bridge::runtime::arith::int_mul");
    m.insert("int_div",         "axis_codegen_bridge::runtime::arith::int_div");
    m.insert("int_div_checked", "axis_codegen_bridge::runtime::arith::int_div_checked");
    m.insert("int_mod",         "axis_codegen_bridge::runtime::arith::int_mod");
    m.insert("int_to_str",      "axis_codegen_bridge::runtime::arith::int_to_str");
    m.insert("str_to_int",      "axis_codegen_bridge::runtime::arith::str_to_int");

    // Comparison
    m.insert("int_lt",   "axis_codegen_bridge::runtime::arith::int_lt");
    m.insert("int_lte",  "axis_codegen_bridge::runtime::arith::int_lte");
    m.insert("int_gt",   "axis_codegen_bridge::runtime::arith::int_gt");
    m.insert("int_gte",  "axis_codegen_bridge::runtime::arith::int_gte");
    m.insert("value_eq", "axis_codegen_bridge::runtime::arith::value_eq");

    // Boolean
    m.insert("bool_and", "axis_codegen_bridge::runtime::bool_ops::bool_and");
    m.insert("bool_or",  "axis_codegen_bridge::runtime::bool_ops::bool_or");
    m.insert("bool_not", "axis_codegen_bridge::runtime::bool_ops::bool_not");

    // String
    m.insert("str_len",       "axis_codegen_bridge::runtime::str_ops::str_len");
    m.insert("str_concat",    "axis_codegen_bridge::runtime::str_ops::str_concat");
    m.insert("str_char",      "axis_codegen_bridge::runtime::str_ops::str_char");
    m.insert("str_char_at",   "axis_codegen_bridge::runtime::str_ops::str_char_at");
    m.insert("str_char_code", "axis_codegen_bridge::runtime::str_ops::str_char_code");
    m.insert("str_slice",     "axis_codegen_bridge::runtime::str_ops::str_slice");

    // List
    m.insert("list_nil",     "axis_codegen_bridge::runtime::list::list_nil");
    m.insert("list_cons",    "axis_codegen_bridge::runtime::list::list_cons");
    m.insert("list_len",     "axis_codegen_bridge::runtime::list::list_len");
    m.insert("list_get",     "axis_codegen_bridge::runtime::list::list_get");
    m.insert("list_get_at",  "axis_codegen_bridge::runtime::list::list_get_at");
    m.insert("list_append",  "axis_codegen_bridge::runtime::list::list_append");
    m.insert("list_concat",  "axis_codegen_bridge::runtime::list::list_concat");
    m.insert("list_reverse", "axis_codegen_bridge::runtime::list::list_reverse");

    // Tuple / constructor
    m.insert("tuple_field", "axis_codegen_bridge::runtime::tuple::tuple_field");
    m.insert("ctor_field",  "axis_codegen_bridge::runtime::tuple::ctor_field");

    // Option
    m.insert("option_none",    "axis_codegen_bridge::runtime::option::option_none");
    m.insert("option_some",    "axis_codegen_bridge::runtime::option::option_some");
    m.insert("option_is_none", "axis_codegen_bridge::runtime::option::option_is_none");
    m.insert("option_is_some", "axis_codegen_bridge::runtime::option::option_is_some");
    m.insert("option_unwrap",  "axis_codegen_bridge::runtime::option::option_unwrap");

    // Equality — curried call site: App(App(Var("__eq__"), a), b) → value_eq(Tuple[a,b])
    m.insert("__eq__",      "axis_codegen_bridge::runtime::arith::value_eq");

    // IO
    m.insert("io_print",    "axis_codegen_bridge::runtime::io::io_print");
    m.insert("io_println",  "axis_codegen_bridge::runtime::io::io_println");
    m.insert("io_eprint",   "axis_codegen_bridge::runtime::io::io_eprint");
    m.insert("io_read_line","axis_codegen_bridge::runtime::io::io_read_line");
    m.insert("fs_read_text","axis_codegen_bridge::runtime::io::fs_read_text");
    m.insert("fs_write_text","axis_codegen_bridge::runtime::io::fs_write_text");
    m.insert("fs_append_text","axis_codegen_bridge::runtime::io::fs_append_text");
    m.insert("debug_trace", "axis_codegen_bridge::runtime::io::debug_trace");

    // Process
    m.insert("proc_args", "axis_codegen_bridge::runtime::process::proc_args");
    m.insert("proc_exit", "axis_codegen_bridge::runtime::process::proc_exit");

    // Registry
    m.insert("registry_has_entry",    "axis_codegen_bridge::runtime::registry::registry_has_entry");
    m.insert("registry_lookup",       "axis_codegen_bridge::runtime::registry::registry_lookup");
    m.insert("registry_get_provenance","axis_codegen_bridge::runtime::registry::registry_get_provenance");
    m.insert("registry_get_contract", "axis_codegen_bridge::runtime::registry::registry_get_contract");
    m.insert("registry_get_effect_sig","axis_codegen_bridge::runtime::registry::registry_get_effect_sig");
    m.insert("registry_all_entries",  "axis_codegen_bridge::runtime::registry::registry_all_entries");
    m.insert("registry_insert",       "axis_codegen_bridge::runtime::registry::registry_insert");
    m.insert("registry_verify_chain", "axis_codegen_bridge::runtime::registry::registry_verify_chain");
    m.insert("registry_compound_id",  "axis_codegen_bridge::runtime::registry::registry_compound_id");

    m
}

/// Collect all arguments from a nested App chain (uncurrying).
/// App(App(f, a), b) → (f, [a, b])
fn collect_app_args<'a>(term: &'a CoreTerm) -> (&'a CoreTerm, Vec<&'a CoreTerm>) {
    let mut args = Vec::new();
    let mut current = term;
    while let CoreTerm::App(func, arg, _) = current {
        args.push(arg.as_ref());
        current = func.as_ref();
    }
    args.reverse();
    (current, args)
}

pub fn emit_rust_from_core(root: &CoreTerm, _source_path: &str, _entrypoint: &str) -> String {
    let sym = symbol_map();
    let mut out = String::new();

    out.push_str("extern crate axis_codegen_bridge;\n");
    out.push_str("use axis_codegen_bridge::runtime::value::{Value, init_runtime};\n\n");
    out.push_str("fn main() {\n");
    out.push_str("    init_runtime();\n");
    // Top-level Lam: emit as a closure but don't try to Display it — closures
    // have no Display impl. Print "<function>" and exit 0, matching the
    // axis-rust-bridge convention that tests rely on.
    if matches!(root, CoreTerm::Lam(..)) {
        out.push_str("    let _closure = ");
        emit_term(root, &sym, &mut out, 1);
        out.push_str(";\n");
        out.push_str("    println!(\"<function>\");\n");
    } else {
        out.push_str("    let result = ");
        emit_term(root, &sym, &mut out, 1);
        out.push_str(";\n");
        out.push_str("    println!(\"{}\", result);\n");
    }
    out.push_str("}\n");

    out
}

fn emit_term(term: &CoreTerm, sym: &HashMap<&str, &str>, out: &mut String, depth: usize) {
    let indent = "    ".repeat(depth);
    match term {
        CoreTerm::IntLit(n, _)  => out.push_str(&format!("Value::Int({})", n)),
        CoreTerm::BoolLit(b, _) => out.push_str(&format!("Value::Bool({})", b)),
        CoreTerm::UnitLit(_)    => out.push_str("Value::Unit"),
        CoreTerm::Var(n, _)     => out.push_str(&sanitise(n)),

        CoreTerm::Let(name, val, body, _) => {
            out.push_str(&format!("{{\n{}    let {} = ", indent, sanitise(name)));
            emit_term(val, sym, out, depth + 1);
            out.push_str(&format!(";\n{}    ", indent));
            emit_term(body, sym, out, depth + 1);
            out.push_str(&format!("\n{}}}", indent));
        }

        CoreTerm::Lam(param, body, _) => {
            out.push_str(&format!("|{}: Value| {{ ", sanitise(param)));
            emit_term(body, sym, out, depth);
            out.push_str(" }");
        }

        CoreTerm::App(_, _, _) => {
            // Uncurry nested App chains: App(App(f, a), b) → f(Tuple[a, b])
            // This is the UNARY INVARIANT: all known runtime functions take exactly
            // one Value argument; multi-arg calls are packed as Value::Tuple.
            let (base, all_args) = collect_app_args(term);
            if let CoreTerm::Var(sym_name, _) = base {
                if let Some(&path) = sym.get(sym_name.as_str()) {
                    if all_args.len() > 1 {
                        // Multi-arg: pack into Tuple
                        out.push_str(path);
                        out.push_str("(Value::Tuple(vec![");
                        for (i, a) in all_args.iter().enumerate() {
                            if i > 0 { out.push_str(", "); }
                            emit_term(a, sym, out, depth);
                            if matches!(a, CoreTerm::Var(..)) { out.push_str(".clone()"); }
                        }
                        out.push_str("]))");
                    } else {
                        // Single arg: call with full qualified path
                        out.push_str(path);
                        out.push('(');
                        emit_term(all_args[0], sym, out, depth);
                        if matches!(all_args[0], CoreTerm::Var(..)) { out.push_str(".clone()"); }
                        out.push(')');
                    }
                    return;
                }
            }
            // Fallback: curried emit for local closures and unknown bases
            if let CoreTerm::App(f, arg, _) = term {
                out.push('(');
                emit_term(f, sym, out, depth);
                out.push_str(")(");
                emit_term(arg, sym, out, depth);
                out.push(')');
            }
        }

        CoreTerm::If(cond, then, els, _) => {
            out.push_str("if axis_codegen_bridge::runtime::value::truthy(&(");
            emit_term(cond, sym, out, depth);
            out.push_str(")) {\n");
            out.push_str(&format!("{}    ", indent));
            emit_term(then, sym, out, depth + 1);
            out.push_str(&format!("\n{}}} else {{\n{}    ", indent, indent));
            emit_term(els, sym, out, depth + 1);
            out.push_str(&format!("\n{}}}", indent));
        }

        CoreTerm::Call(target, args, _) => {
            let rust_fn = sym.get(target.as_str()).copied().unwrap_or(target.as_str());
            if args.is_empty() {
                out.push_str(&format!("{}(Value::Unit)", rust_fn));
            } else if args.len() == 1 {
                out.push_str(&format!("{}(", rust_fn));
                emit_term(&args[0], sym, out, depth);
                out.push(')');
            } else {
                out.push_str(&format!("{}(Value::Tuple(vec![", rust_fn));
                for (i, a) in args.iter().enumerate() {
                    if i > 0 { out.push_str(", "); }
                    emit_term(a, sym, out, depth);
                }
                out.push_str("]))");
            }
        }
    }
}

/// Make a name safe as a Rust identifier.
fn sanitise(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{}", s)
    } else {
        s
    }
}

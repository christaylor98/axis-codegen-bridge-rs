/// Emit Rust source from a Core IR term.
///
/// Generated code links against axis_codegen_bridge and calls runtime
/// functions by their canonical names. No foreign stubs are generated here —
/// all foreign functions are implemented in the bridge library.

use crate::core_ir::CoreTerm;
use std::collections::{HashMap, HashSet};

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
    m.insert("str_slice",       "axis_codegen_bridge::runtime::str_ops::str_slice");
    m.insert("str_split",       "axis_codegen_bridge::runtime::str_ops::str_split");
    m.insert("str_starts_with", "axis_codegen_bridge::runtime::str_ops::str_starts_with");
    m.insert("str_ends_with",   "axis_codegen_bridge::runtime::str_ops::str_ends_with");
    m.insert("str_trim",        "axis_codegen_bridge::runtime::str_ops::str_trim");
    m.insert("str_contains",    "axis_codegen_bridge::runtime::str_ops::str_contains");
    m.insert("str_index_of",    "axis_codegen_bridge::runtime::str_ops::str_index_of");
    m.insert("str_eq",          "axis_codegen_bridge::runtime::str_ops::str_eq");
    m.insert("chr",             "axis_codegen_bridge::runtime::str_ops::chr");

    // List
    m.insert("list_nil",     "axis_codegen_bridge::runtime::list::list_nil");
    m.insert("list_cons",    "axis_codegen_bridge::runtime::list::list_cons");
    m.insert("list_len",     "axis_codegen_bridge::runtime::list::list_len");
    m.insert("list_get",     "axis_codegen_bridge::runtime::list::list_get");
    m.insert("list_get_at",              "axis_codegen_bridge::runtime::list::list_get_at");
    m.insert("list_get_println_if_some",   "axis_codegen_bridge::runtime::list::list_get_println_if_some");
    m.insert("list_str_len_lte_if_some",   "axis_codegen_bridge::runtime::list::list_str_len_lte_if_some");
    m.insert("list_append",              "axis_codegen_bridge::runtime::list::list_append");
    m.insert("list_concat",  "axis_codegen_bridge::runtime::list::list_concat");
    m.insert("list_reverse",   "axis_codegen_bridge::runtime::list::list_reverse");
    m.insert("list_head",      "axis_codegen_bridge::runtime::list::list_head");
    m.insert("list_tail",      "axis_codegen_bridge::runtime::list::list_tail");
    m.insert("list_is_empty",  "axis_codegen_bridge::runtime::list::list_is_empty");
    m.insert("list_of_1",      "axis_codegen_bridge::runtime::list::list_of_1");
    m.insert("list_of_2",      "axis_codegen_bridge::runtime::list::list_of_2");
    m.insert("list_of_3",      "axis_codegen_bridge::runtime::list::list_of_3");

    // Tuple / constructor
    m.insert("tuple_field", "axis_codegen_bridge::runtime::tuple::tuple_field");
    m.insert("ctor_field",  "axis_codegen_bridge::runtime::tuple::ctor_field");
    m.insert("ctor_is_ok",  "axis_codegen_bridge::runtime::tuple::ctor_is_ok");
    m.insert("result_text_unwrap", "axis_codegen_bridge::runtime::tuple::result_text_unwrap");

    // Option
    m.insert("option_none",    "axis_codegen_bridge::runtime::option::option_none_fn");
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
    m.insert("proc_args",   "axis_codegen_bridge::runtime::process::proc_args");
    m.insert("proc_exit",   "axis_codegen_bridge::runtime::process::proc_exit");
    m.insert("proc_sleep",  "axis_codegen_bridge::runtime::process::proc_sleep");
    m.insert("sleep",       "axis_codegen_bridge::runtime::process::sleep");
    m.insert("argv",        "axis_codegen_bridge::runtime::process::argv");
    m.insert("argv_int",    "axis_codegen_bridge::runtime::process::argv_int");
    m.insert("argv_count",  "axis_codegen_bridge::runtime::process::argv_count");
    m.insert("argv_or",     "axis_codegen_bridge::runtime::process::argv_or");

    // Transitions (one per catalog entry; stubs matching src/*.ai2 lam bodies)
    m.insert("introduce_let_binding",       "axis_codegen_bridge::runtime::transitions::introduce_let_binding");
    m.insert("introduce_lambda",            "axis_codegen_bridge::runtime::transitions::introduce_lambda");
    m.insert("apply_function",              "axis_codegen_bridge::runtime::transitions::apply_function");
    m.insert("extract_subterm_to_function", "axis_codegen_bridge::runtime::transitions::extract_subterm_to_function");
    m.insert("inline_let_binding",          "axis_codegen_bridge::runtime::transitions::inline_let_binding");
    m.insert("rename_bound_variable",       "axis_codegen_bridge::runtime::transitions::rename_bound_variable");
    m.insert("reference_registry_function", "axis_codegen_bridge::runtime::transitions::reference_registry_function");
    m.insert("verify_foreign_reference",    "axis_codegen_bridge::runtime::transitions::verify_foreign_reference");

    // IR constructors
    m.insert("ir_make_int_lit",  "axis_codegen_bridge::runtime::ir_constructors::ir_make_int_lit");
    m.insert("ir_make_bool_lit", "axis_codegen_bridge::runtime::ir_constructors::ir_make_bool_lit");
    m.insert("ir_make_unit_lit", "axis_codegen_bridge::runtime::ir_constructors::ir_make_unit_lit");
    m.insert("ir_make_var",      "axis_codegen_bridge::runtime::ir_constructors::ir_make_var");
    m.insert("ir_make_lam",      "axis_codegen_bridge::runtime::ir_constructors::ir_make_lam");
    m.insert("ir_make_let",      "axis_codegen_bridge::runtime::ir_constructors::ir_make_let");
    m.insert("ir_make_if",       "axis_codegen_bridge::runtime::ir_constructors::ir_make_if");
    m.insert("ir_make_app",      "axis_codegen_bridge::runtime::ir_constructors::ir_make_app");
    m.insert("ir_make_call",     "axis_codegen_bridge::runtime::ir_constructors::ir_make_call");
    m.insert("ir_term_kind",     "axis_codegen_bridge::runtime::ir_constructors::ir_term_kind");
    m.insert("ir_to_string",     "axis_codegen_bridge::runtime::ir_constructors::ir_to_string");
    m.insert("ir_to_h1_string",  "axis_codegen_bridge::runtime::ir_constructors::ir_to_h1_string");
    m.insert("ir_write_bundle",  "axis_codegen_bridge::runtime::ir_constructors::ir_write_bundle");
    m.insert("ir_read_bundle",   "axis_codegen_bridge::runtime::ir_constructors::ir_read_bundle");
    m.insert("ir_subst",         "axis_codegen_bridge::runtime::ir_constructors::ir_subst");
    m.insert("ir_rename",        "axis_codegen_bridge::runtime::ir_constructors::ir_rename");
    m.insert("ir_free_vars",     "axis_codegen_bridge::runtime::ir_constructors::ir_free_vars");
    m.insert("ir_build_program_from_spec", "axis_codegen_bridge::runtime::ir_constructors::ir_build_program_from_spec");
    m.insert("ir_build_fold_from_spec",   "axis_codegen_bridge::runtime::ir_constructors::ir_build_fold_from_spec");
    m.insert("ir_bundle_view",            "axis_codegen_bridge::runtime::ir_constructors::ir_bundle_view");
    m.insert("ir_eval",          "axis_codegen_bridge::runtime::ir_eval::ir_eval");
    m.insert("ir_apply",         "axis_codegen_bridge::runtime::ir_eval::ir_apply");

    // IR Accessors
    m.insert("ir_get_kind",     "axis_codegen_bridge::runtime::ir_accessors::ir_get_kind");
    m.insert("ir_get_name",     "axis_codegen_bridge::runtime::ir_accessors::ir_get_name");
    m.insert("ir_get_int_val",  "axis_codegen_bridge::runtime::ir_accessors::ir_get_int_val");
    m.insert("ir_get_fn",       "axis_codegen_bridge::runtime::ir_accessors::ir_get_fn");
    m.insert("ir_get_arg",      "axis_codegen_bridge::runtime::ir_accessors::ir_get_arg");
    m.insert("ir_get_body",     "axis_codegen_bridge::runtime::ir_accessors::ir_get_body");
    m.insert("ir_get_value",    "axis_codegen_bridge::runtime::ir_accessors::ir_get_value");
    m.insert("ir_get_cond",     "axis_codegen_bridge::runtime::ir_accessors::ir_get_cond");
    m.insert("ir_get_then",     "axis_codegen_bridge::runtime::ir_accessors::ir_get_then");
    m.insert("ir_get_else",     "axis_codegen_bridge::runtime::ir_accessors::ir_get_else");

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

    // Frontend
    m.insert("frontend_lookup_shape", "axis_codegen_bridge::runtime::frontend::frontend_lookup_shape");
    m.insert("frontend_walk",         "axis_codegen_bridge::runtime::frontend::frontend_walk");

    m
}

/// Collect all CCall target names from a term (recursive).
fn collect_ccall_targets(term: &CoreTerm, out: &mut Vec<String>) {
    match term {
        CoreTerm::Call(target, args, _) => {
            out.push(target.clone());
            for a in args { collect_ccall_targets(a, out); }
        }
        CoreTerm::Let(_, val, body, _) => {
            collect_ccall_targets(val, out);
            collect_ccall_targets(body, out);
        }
        CoreTerm::Lam(_, body, _) => collect_ccall_targets(body, out),
        CoreTerm::App(f, a, _) => {
            collect_ccall_targets(f, out);
            collect_ccall_targets(a, out);
        }
        CoreTerm::If(cond, then, els, _) => {
            collect_ccall_targets(cond, out);
            collect_ccall_targets(then, out);
            collect_ccall_targets(els, out);
        }
        _ => {}
    }
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

/// Emit Rust source from a Core IR term plus optional library bundles.
///
/// Each entry in `libs` is `(fn_name, root_term)` — the entrypoint name and
/// root term of a library bundle. Library functions are emitted as top-level
/// Rust functions before `fn main`.
///
/// CCall targets are validated against: the bridge symbol map, library
/// function names, and `registry_names`. An unresolved target is an error.
pub fn emit_rust_from_core(
    root: &CoreTerm,
    _source_path: &str,
    _entrypoint: &str,
    libs: &[(String, CoreTerm)],
    registry_names: &HashSet<String>,
) -> Result<String, String> {
    let sym = symbol_map();

    // Build the complete set of known CCall targets.
    let lib_fn_names: HashSet<&str> = libs.iter().map(|(n, _)| n.as_str()).collect();
    let known: HashSet<&str> = sym.keys().copied()
        .chain(lib_fn_names.iter().copied())
        .chain(registry_names.iter().map(|s| s.as_str()))
        .collect();

    // Validate all CCall targets in main and all libs.
    let mut targets: Vec<String> = Vec::new();
    collect_ccall_targets(root, &mut targets);
    for (_, lib_term) in libs {
        collect_ccall_targets(lib_term, &mut targets);
    }
    for target in &targets {
        if !known.contains(target.as_str()) {
            return Err(format!(
                "unresolved: {} not found in libs, bridge, or registry",
                target
            ));
        }
    }

    // lib_sym: maps lib function names → _lib_-prefixed Rust identifiers for emit.
    // The _lib_ prefix keeps lib helpers in a separate namespace from export symbols,
    // preventing collisions when an export shares a name with a lib helper.
    let lib_sanitised: Vec<(String, String)> = libs.iter()
        .map(|(n, _)| (n.clone(), format!("_lib_{}", sanitise(n))))
        .collect();
    let lib_sym: HashMap<&str, &str> = lib_sanitised.iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    let mut out = String::new();
    out.push_str("extern crate axis_codegen_bridge;\n");
    out.push_str("use axis_codegen_bridge::runtime::value::{Value, init_runtime};\n\n");

    // Emit library functions before main.
    for (fn_name, lib_term) in libs {
        let safe_name = format!("_lib_{}", sanitise(fn_name));
        match lib_term {
            CoreTerm::Lam(param, body, _) => {
                out.push_str(&format!(
                    "#[allow(dead_code, unused_variables)]\nfn {}({}: Value) -> Value {{\n",
                    safe_name,
                    sanitise(param)
                ));
                out.push_str("    ");
                emit_term(body, &sym, &lib_sym, &mut out, 1);
                out.push_str("\n}\n\n");
            }
            _ => {
                out.push_str(&format!("#[allow(dead_code)]\nfn {}(_: Value) -> Value {{\n", safe_name));
                out.push_str("    ");
                emit_term(lib_term, &sym, &lib_sym, &mut out, 1);
                out.push_str("\n}\n\n");
            }
        }
    }

    out.push_str("fn main() {\n");
    out.push_str("    init_runtime();\n");
    // Top-level Lam: emit as a closure but don't try to Display it — closures
    // have no Display impl. Print "<function>" and exit 0, matching the
    // axis-rust-bridge convention that tests rely on.
    if matches!(root, CoreTerm::Lam(..)) {
        out.push_str("    let _closure = ");
        emit_term(root, &sym, &lib_sym, &mut out, 1);
        out.push_str(";\n");
        out.push_str("    println!(\"<function>\");\n");
    } else {
        out.push_str("    let result = ");
        emit_term(root, &sym, &lib_sym, &mut out, 1);
        out.push_str(";\n");
        out.push_str("    if !matches!(result, Value::Unit) { println!(\"{}\", result); }\n");
    }
    out.push_str("}\n");

    Ok(out)
}

fn emit_term(term: &CoreTerm, sym: &HashMap<&str, &str>, lib_sym: &HashMap<&str, &str>, out: &mut String, depth: usize) {
    let indent = "    ".repeat(depth);
    match term {
        CoreTerm::IntLit(n, _)  => out.push_str(&format!("Value::Int({})", n)),
        CoreTerm::BoolLit(b, _) => out.push_str(&format!("Value::Bool({})", b)),
        CoreTerm::UnitLit(_)    => out.push_str("Value::Unit"),
        CoreTerm::Var(n, _)     => out.push_str(&sanitise(n)),

        CoreTerm::Let(name, val, body, _) => {
            out.push_str(&format!("{{\n{}    let {} = ", indent, sanitise(name)));
            emit_term(val, sym, lib_sym, out, depth + 1);
            out.push_str(&format!(";\n{}    ", indent));
            emit_term(body, sym, lib_sym, out, depth + 1);
            out.push_str(&format!("\n{}}}", indent));
        }

        CoreTerm::Lam(param, body, _) => {
            out.push_str(&format!("|{}: Value| {{ ", sanitise(param)));
            emit_term(body, sym, lib_sym, out, depth);
            out.push_str(" }");
        }

        CoreTerm::App(_, _, _) => {
            // Uncurry nested App chains: App(App(f, a), b) → f(Tuple[a, b])
            // This is the UNARY INVARIANT: all known runtime functions take exactly
            // one Value argument; multi-arg calls are packed as Value::Tuple.
            // Applies to both bridge primitives (sym) and lib functions (lib_sym).
            let (base, all_args) = collect_app_args(term);
            if let CoreTerm::Var(sym_name, _) = base {
                let found_path = sym.get(sym_name.as_str()).copied()
                    .or_else(|| lib_sym.get(sym_name.as_str()).copied());
                if let Some(path) = found_path {
                    if all_args.len() > 1 {
                        // Multi-arg: pack into Tuple
                        out.push_str(path);
                        out.push_str("(Value::Tuple(vec![");
                        for (i, a) in all_args.iter().enumerate() {
                            if i > 0 { out.push_str(", "); }
                            emit_term(a, sym, lib_sym, out, depth);
                            if matches!(a, CoreTerm::Var(..)) { out.push_str(".clone()"); }
                        }
                        out.push_str("]))");
                    } else {
                        // Single arg: call with resolved path
                        out.push_str(path);
                        out.push('(');
                        emit_term(all_args[0], sym, lib_sym, out, depth);
                        if matches!(all_args[0], CoreTerm::Var(..)) { out.push_str(".clone()"); }
                        out.push(')');
                    }
                    return;
                }
            }
            // Fallback: curried emit for local closures and unknown bases
            if let CoreTerm::App(f, arg, _) = term {
                out.push('(');
                emit_term(f, sym, lib_sym, out, depth);
                out.push_str(")(");
                emit_term(arg, sym, lib_sym, out, depth);
                out.push(')');
            }
        }

        CoreTerm::If(cond, then, els, _) => {
            out.push_str("if axis_codegen_bridge::runtime::value::truthy(&(");
            emit_term(cond, sym, lib_sym, out, depth);
            out.push_str(")) {\n");
            out.push_str(&format!("{}    ", indent));
            emit_term(then, sym, lib_sym, out, depth + 1);
            out.push_str(&format!("\n{}}} else {{\n{}    ", indent, indent));
            emit_term(els, sym, lib_sym, out, depth + 1);
            out.push_str(&format!("\n{}}}", indent));
        }

        CoreTerm::Call(target, args, _) => {
            let rust_fn = sym.get(target.as_str()).copied()
                .or_else(|| lib_sym.get(target.as_str()).copied())
                .unwrap_or(target.as_str());
            if args.is_empty() {
                out.push_str(&format!("{}(Value::Unit)", rust_fn));
            } else if args.len() == 1 {
                out.push_str(&format!("{}(", rust_fn));
                emit_term(&args[0], sym, lib_sym, out, depth);
                if matches!(&args[0], CoreTerm::Var(..)) { out.push_str(".clone()"); }
                out.push(')');
            } else {
                out.push_str(&format!("{}(Value::Tuple(vec![", rust_fn));
                for (i, a) in args.iter().enumerate() {
                    if i > 0 { out.push_str(", "); }
                    emit_term(a, sym, lib_sym, out, depth);
                    if matches!(a, CoreTerm::Var(..)) { out.push_str(".clone()"); }
                }
                out.push_str("]))");
            }
        }
    }
}

/// Make a name safe as a Rust identifier.
pub fn sanitise(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{}", s)
    } else {
        s
    }
}

/// Emit Rust source in lib mode: one `#[no_mangle] pub extern "C"` function per export.
///
/// Each entry in `exports` is `(fn_name, root_term)`. All are compiled into the same
/// static library source. Library helper functions (from `libs`) are emitted first as
/// internal (non-exported) Rust fns.
pub fn emit_rust_lib_from_core(
    exports: &[(String, CoreTerm)],
    libs: &[(String, CoreTerm)],
    registry_names: &HashSet<String>,
) -> Result<String, String> {
    let sym = symbol_map();

    let lib_fn_names: HashSet<&str> = libs.iter().map(|(n, _)| n.as_str()).collect();
    let export_fn_names: HashSet<&str> = exports.iter().map(|(n, _)| n.as_str()).collect();
    let known: HashSet<&str> = sym.keys().copied()
        .chain(lib_fn_names.iter().copied())
        .chain(export_fn_names.iter().copied())
        .chain(registry_names.iter().map(|s| s.as_str()))
        .collect();

    let mut targets: Vec<String> = Vec::new();
    for (_, term) in exports { collect_ccall_targets(term, &mut targets); }
    for (_, lib_term) in libs  { collect_ccall_targets(lib_term, &mut targets); }
    for target in &targets {
        if !known.contains(target.as_str()) {
            return Err(format!(
                "unresolved: {} not found in libs, bridge, or registry",
                target
            ));
        }
    }

    // lib_sym: maps lib function names → _lib_-prefixed Rust identifiers for emit.
    // The _lib_ prefix keeps lib helpers in a separate namespace from export symbols,
    // preventing collisions when an export shares a name with a lib helper.
    let lib_sanitised: Vec<(String, String)> = libs.iter()
        .map(|(n, _)| (n.clone(), format!("_lib_{}", sanitise(n))))
        .collect();
    let lib_sym: HashMap<&str, &str> = lib_sanitised.iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    let mut out = String::new();
    out.push_str("extern crate axis_codegen_bridge;\n");
    out.push_str("use axis_codegen_bridge::runtime::value::{Value, init_runtime};\n\n");

    for (lib_fn, lib_term) in libs {
        let safe_name = format!("_lib_{}", sanitise(lib_fn));
        match lib_term {
            CoreTerm::Lam(param, body, _) => {
                out.push_str(&format!(
                    "#[allow(dead_code, unused_variables)]\nfn {}({}: Value) -> Value {{\n",
                    safe_name, sanitise(param)
                ));
                out.push_str("    ");
                emit_term(body, &sym, &lib_sym, &mut out, 1);
                out.push_str("\n}\n\n");
            }
            _ => {
                out.push_str(&format!("#[allow(dead_code)]\nfn {}(_: Value) -> Value {{\n", safe_name));
                out.push_str("    ");
                emit_term(lib_term, &sym, &lib_sym, &mut out, 1);
                out.push_str("\n}\n\n");
            }
        }
    }

    for (fn_name, root) in exports {
        let safe_fn = sanitise(fn_name);
        out.push_str("#[allow(improper_ctypes_definitions)]\n");
        out.push_str("#[no_mangle]\n");
        out.push_str(&format!("pub extern \"C\" fn {}(args: Value) -> Value {{\n", safe_fn));
        out.push_str("    init_runtime();\n");
        match root {
            CoreTerm::Lam(param, body, _) => {
                let safe_param = sanitise(param);
                out.push_str(&format!("    #[allow(unused_variables)] let {} = args;\n    ", safe_param));
                emit_term(body, &sym, &lib_sym, &mut out, 1);
                out.push('\n');
            }
            _ => {
                out.push_str("    let _args = args;\n    ");
                emit_term(root, &sym, &lib_sym, &mut out, 1);
                out.push('\n');
            }
        }
        out.push_str("}\n\n");
        // Shim-callable alias: _ax_exe_<name> avoids C stdlib symbol collisions at link time.
        let shim_name = format!("_ax_exe_{}", safe_fn);
        out.push_str("#[allow(improper_ctypes_definitions)]\n");
        out.push_str("#[no_mangle]\n");
        out.push_str(&format!(
            "pub extern \"C\" fn {}(args: Value) -> Value {{ {}(args) }}\n\n",
            shim_name, safe_fn
        ));
    }

    Ok(out)
}

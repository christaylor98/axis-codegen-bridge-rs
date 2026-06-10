/// Emit Rust source from a Core IR 0.5 bundle.
///
/// The 0.5 model is a flat indexed table (constant pool + node list) where
/// every CCall references its target by a 256-bit identity token (sha256 of
/// the function name for §5b bootstrap functions). The emitter:
///   1. Resolves each CCall identity → bridge runtime path or registry name.
///   2. Decodes pool entries by matching def_hash against known primitive types.
///   3. Emits one `let pool_N` declaration per pool entry.
///   4. Emits one `let node_N` declaration per node in topological order.
///   5. Returns the last node (or first pool entry if no nodes) as the result.

use std::collections::HashMap;

use crate::core_ir_05::{
    bool_type_hash, decode_bool_payload, decode_int_payload, decode_text_payload,
    hash256_to_hex, int_type_hash, sha256_bytes, text_type_hash, unit_type_hash,
    ConstantPoolEntry, CoreBundle, Hash256, Node, NodeRef,
};

// ── Symbol map (name → bridge path) ─────────────────────────────────────────

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
    m.insert("int_abs",         "axis_codegen_bridge::runtime::arith::int_abs");
    m.insert("int_min",         "axis_codegen_bridge::runtime::arith::int_min");
    m.insert("int_max",         "axis_codegen_bridge::runtime::arith::int_max");
    m.insert("int_clamp",       "axis_codegen_bridge::runtime::arith::int_clamp");
    m.insert("celsius_to_fahrenheit", "axis_codegen_bridge::runtime::arith::celsius_to_fahrenheit");
    m.insert("fahrenheit_to_celsius", "axis_codegen_bridge::runtime::arith::fahrenheit_to_celsius");
    m.insert("is_positive",     "axis_codegen_bridge::runtime::arith::is_positive");

    // Comparison
    m.insert("int_lt",   "axis_codegen_bridge::runtime::arith::int_lt");
    m.insert("int_lte",  "axis_codegen_bridge::runtime::arith::int_lte");
    m.insert("int_gt",   "axis_codegen_bridge::runtime::arith::int_gt");
    m.insert("int_gte",  "axis_codegen_bridge::runtime::arith::int_gte");
    m.insert("int_eq",   "axis_codegen_bridge::runtime::arith::int_eq");
    m.insert("value_eq", "axis_codegen_bridge::runtime::arith::value_eq");

    // Unit / sequence helpers (§5b bootstrap functions)
    m.insert("unit_id",    "axis_codegen_bridge::runtime::arith::unit_id");
    m.insert("const_unit", "axis_codegen_bridge::runtime::arith::unit_id");
    m.insert("seq_unit",   "axis_codegen_bridge::runtime::arith::seq_unit");

    // Boolean
    m.insert("bool_and", "axis_codegen_bridge::runtime::bool_ops::bool_and");
    m.insert("bool_or",  "axis_codegen_bridge::runtime::bool_ops::bool_or");
    m.insert("bool_not", "axis_codegen_bridge::runtime::bool_ops::bool_not");

    // String
    m.insert("str_len",         "axis_codegen_bridge::runtime::str_ops::str_len");
    m.insert("str_concat",      "axis_codegen_bridge::runtime::str_ops::str_concat");
    m.insert("str_char",        "axis_codegen_bridge::runtime::str_ops::str_char");
    m.insert("str_char_at",     "axis_codegen_bridge::runtime::str_ops::str_char_at");
    m.insert("str_char_code",   "axis_codegen_bridge::runtime::str_ops::str_char_code");
    m.insert("str_slice",       "axis_codegen_bridge::runtime::str_ops::str_slice");
    m.insert("str_split",       "axis_codegen_bridge::runtime::str_ops::str_split");
    m.insert("str_starts_with", "axis_codegen_bridge::runtime::str_ops::str_starts_with");
    m.insert("str_ends_with",   "axis_codegen_bridge::runtime::str_ops::str_ends_with");
    m.insert("str_trim",        "axis_codegen_bridge::runtime::str_ops::str_trim");
    m.insert("str_contains",    "axis_codegen_bridge::runtime::str_ops::str_contains");
    m.insert("str_index_of",    "axis_codegen_bridge::runtime::str_ops::str_index_of");
    m.insert("str_eq",          "axis_codegen_bridge::runtime::str_ops::str_eq");
    m.insert("str_before",      "axis_codegen_bridge::runtime::str_ops::str_before");
    m.insert("str_after",       "axis_codegen_bridge::runtime::str_ops::str_after");
    m.insert("str_between",     "axis_codegen_bridge::runtime::str_ops::str_between");
    m.insert("chr",             "axis_codegen_bridge::runtime::str_ops::chr");

    // List
    m.insert("list_nil",      "axis_codegen_bridge::runtime::list::list_nil");
    m.insert("list_cons",     "axis_codegen_bridge::runtime::list::list_cons");
    m.insert("list_len",      "axis_codegen_bridge::runtime::list::list_len");
    m.insert("list_get",      "axis_codegen_bridge::runtime::list::list_get");
    m.insert("list_get_at",              "axis_codegen_bridge::runtime::list::list_get_at");
    m.insert("list_get_println_if_some",   "axis_codegen_bridge::runtime::list::list_get_println_if_some");
    m.insert("list_str_len_lte_if_some",   "axis_codegen_bridge::runtime::list::list_str_len_lte_if_some");
    m.insert("list_append",              "axis_codegen_bridge::runtime::list::list_append");
    m.insert("list_concat",   "axis_codegen_bridge::runtime::list::list_concat");
    m.insert("list_reverse",  "axis_codegen_bridge::runtime::list::list_reverse");
    m.insert("list_head",     "axis_codegen_bridge::runtime::list::list_head");
    m.insert("list_tail",     "axis_codegen_bridge::runtime::list::list_tail");
    m.insert("list_is_empty", "axis_codegen_bridge::runtime::list::list_is_empty");
    m.insert("list_of_1",     "axis_codegen_bridge::runtime::list::list_of_1");
    m.insert("list_of_2",     "axis_codegen_bridge::runtime::list::list_of_2");
    m.insert("list_of_3",     "axis_codegen_bridge::runtime::list::list_of_3");

    // Tuple / constructor
    m.insert("tuple_field", "axis_codegen_bridge::runtime::tuple::tuple_field");
    m.insert("ctor_field",  "axis_codegen_bridge::runtime::tuple::ctor_field");
    m.insert("ctor_is_ok",  "axis_codegen_bridge::runtime::tuple::ctor_is_ok");
    m.insert("result_text_unwrap", "axis_codegen_bridge::runtime::tuple::result_text_unwrap");

    // M1 compound-value constructors / accessors
    m.insert("value_make", "axis_codegen_bridge::runtime::tuple::value_make");
    m.insert("value_0",    "axis_codegen_bridge::runtime::tuple::value_0");
    m.insert("value_1",    "axis_codegen_bridge::runtime::tuple::value_1");
    m.insert("value_2",    "axis_codegen_bridge::runtime::tuple::value_2");
    m.insert("list_make",  "axis_codegen_bridge::runtime::list::list_make");

    // Option
    m.insert("option_none",    "axis_codegen_bridge::runtime::option::option_none_fn");
    m.insert("option_some",    "axis_codegen_bridge::runtime::option::option_some");
    m.insert("option_is_none", "axis_codegen_bridge::runtime::option::option_is_none");
    m.insert("option_is_some", "axis_codegen_bridge::runtime::option::option_is_some");
    m.insert("option_unwrap",  "axis_codegen_bridge::runtime::option::option_unwrap");

    // Equality
    m.insert("__eq__", "axis_codegen_bridge::runtime::arith::value_eq");

    // IO
    m.insert("io_print",      "axis_codegen_bridge::runtime::io::io_print");
    m.insert("io_println",    "axis_codegen_bridge::runtime::io::io_println");
    m.insert("io_eprint",     "axis_codegen_bridge::runtime::io::io_eprint");
    m.insert("io_read_line",  "axis_codegen_bridge::runtime::io::io_read_line");
    m.insert("fs_read_text",  "axis_codegen_bridge::runtime::io::fs_read_text");
    m.insert("fs_write_text", "axis_codegen_bridge::runtime::io::fs_write_text");
    m.insert("fs_append_text","axis_codegen_bridge::runtime::io::fs_append_text");
    m.insert("debug_trace",   "axis_codegen_bridge::runtime::io::debug_trace");

    // Process
    m.insert("proc_args",  "axis_codegen_bridge::runtime::process::proc_args");
    m.insert("proc_exit",  "axis_codegen_bridge::runtime::process::proc_exit");
    m.insert("proc_sleep", "axis_codegen_bridge::runtime::process::proc_sleep");
    m.insert("sleep",      "axis_codegen_bridge::runtime::process::sleep");
    m.insert("argv",       "axis_codegen_bridge::runtime::process::argv");
    m.insert("argv_get",   "axis_codegen_bridge::runtime::process::argv_get");
    m.insert("argv_int",   "axis_codegen_bridge::runtime::process::argv_int");
    m.insert("argv_count", "axis_codegen_bridge::runtime::process::argv_count");
    m.insert("argv_or",    "axis_codegen_bridge::runtime::process::argv_or");

    // IR constructors / accessors (kept for backward compat)
    m.insert("ir_make_int_lit",  "axis_codegen_bridge::runtime::ir_constructors::ir_make_int_lit");
    m.insert("ir_make_bool_lit", "axis_codegen_bridge::runtime::ir_constructors::ir_make_bool_lit");
    m.insert("ir_make_unit_lit", "axis_codegen_bridge::runtime::ir_constructors::ir_make_unit_lit");
    m.insert("ir_make_var",      "axis_codegen_bridge::runtime::ir_constructors::ir_make_var");
    m.insert("ir_make_lam",      "axis_codegen_bridge::runtime::ir_constructors::ir_make_lam");
    m.insert("ir_make_let",      "axis_codegen_bridge::runtime::ir_constructors::ir_make_let");
    m.insert("ir_make_if",       "axis_codegen_bridge::runtime::ir_constructors::ir_make_if");
    m.insert("ir_make_app",      "axis_codegen_bridge::runtime::ir_constructors::ir_make_app");
    m.insert("ir_make_call",     "axis_codegen_bridge::runtime::ir_constructors::ir_make_call");
    m.insert("ir_write_bundle",  "axis_codegen_bridge::runtime::ir_constructors::ir_write_bundle");
    m.insert("ir_read_bundle",   "axis_codegen_bridge::runtime::ir_constructors::ir_read_bundle");
    m.insert("ir_bundle_view",   "axis_codegen_bridge::runtime::ir_constructors::ir_bundle_view");
    m.insert("ir_subst",         "axis_codegen_bridge::runtime::ir_constructors::ir_subst");
    m.insert("ir_rename",        "axis_codegen_bridge::runtime::ir_constructors::ir_rename");
    m.insert("ir_free_vars",     "axis_codegen_bridge::runtime::ir_constructors::ir_free_vars");
    m.insert("ir_eval",          "axis_codegen_bridge::runtime::ir_eval::ir_eval");
    m.insert("ir_apply",         "axis_codegen_bridge::runtime::ir_eval::ir_apply");
    m.insert("ir_get_kind",      "axis_codegen_bridge::runtime::ir_accessors::ir_get_kind");
    m.insert("ir_get_name",      "axis_codegen_bridge::runtime::ir_accessors::ir_get_name");
    m.insert("ir_get_int_val",   "axis_codegen_bridge::runtime::ir_accessors::ir_get_int_val");
    m.insert("ir_get_fn",        "axis_codegen_bridge::runtime::ir_accessors::ir_get_fn");
    m.insert("ir_get_arg",       "axis_codegen_bridge::runtime::ir_accessors::ir_get_arg");
    m.insert("ir_get_body",      "axis_codegen_bridge::runtime::ir_accessors::ir_get_body");
    m.insert("ir_get_value",     "axis_codegen_bridge::runtime::ir_accessors::ir_get_value");
    m.insert("ir_get_cond",      "axis_codegen_bridge::runtime::ir_accessors::ir_get_cond");
    m.insert("ir_get_then",      "axis_codegen_bridge::runtime::ir_accessors::ir_get_then");
    m.insert("ir_get_else",      "axis_codegen_bridge::runtime::ir_accessors::ir_get_else");

    m
}

/// Build the identity → bridge path map.
/// Identity = sha256(canonical_name) for all §5b bridge built-ins.
fn bridge_builtin_map() -> HashMap<Hash256, &'static str> {
    let sym = symbol_map();
    let mut map = HashMap::new();
    for (name, path) in sym {
        let identity = sha256_bytes(name.as_bytes());
        map.insert(identity, path);
    }
    map
}

// ── Registry loading ─────────────────────────────────────────────────────────

/// Parse `--reg` files (axis 0.5 registry format) and return identity → name map.
///
/// Accepts both the axis registry format (`fn <name> / identity 0x<hex> / end`) and
/// the older bridge registry format (`fn <name> / arity / end`). If no explicit
/// `identity` line is present, computes sha256(name) per the §5b rule.
pub fn load_registry_identity_map(paths: &[String]) -> HashMap<Hash256, String> {
    let mut map = HashMap::new();
    for path in paths {
        let content = match std::fs::read_to_string(path) {
            Ok(c)  => c,
            Err(e) => { eprintln!("warning: could not read --reg {}: {}", path, e); continue; }
        };
        let mut current_name: Option<String> = None;
        let mut current_identity: Option<Hash256> = None;
        for line in content.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("fn ") {
                let name = rest.split_whitespace().next().unwrap_or("").to_string();
                if !name.is_empty() {
                    current_name = Some(name.clone());
                    current_identity = Some(sha256_bytes(name.as_bytes())); // §5b default
                }
            } else if let Some(rest) = t.strip_prefix("identity ") {
                let hex = rest.trim().trim_start_matches("0x");
                if let Ok(id) = crate::core_ir_05::hex_to_hash256(hex) {
                    current_identity = Some(id);
                }
            } else if t == "end" {
                if let (Some(name), Some(id)) = (current_name.take(), current_identity.take()) {
                    map.insert(id, name);
                }
            }
        }
        // handle files that end without "end" (old format)
        if let (Some(name), Some(id)) = (current_name, current_identity) {
            map.insert(id, name);
        }
    }
    map
}

// ── Pool constant decoding ────────────────────────────────────────────────────

fn decode_pool_entry(entry: &ConstantPoolEntry) -> Result<String, String> {
    let dh = &entry.def_hash;
    if dh == &[0u8; 32] {
        return Err(
            "all-zero def_hash (UNKNOWN-gate sentinel): pool entry has no resolved \
             type identity. This is an upstream lowering gap — e.g. a lambda or \
             fn-reference that was not assigned a Fn type hash — not a bridge defect."
                .to_string(),
        );
    }
    if dh == &unit_type_hash() {
        return Ok("Value::Unit".to_string());
    }
    if dh == &bool_type_hash() {
        let v = decode_bool_payload(&entry.payload)?;
        return Ok(format!("Value::Bool({})", v));
    }
    if dh == &int_type_hash() {
        let v = decode_int_payload(&entry.payload)?;
        return Ok(format!("Value::Int({})", v));
    }
    if dh == &text_type_hash() {
        let s = decode_text_payload(&entry.payload)?;
        return Ok(format!("Value::Str(axis_codegen_bridge::runtime::value::intern_str({:?}))", s));
    }
    Err(format!(
        "unknown pool entry type hash: {} (only Unit/Bool/Int/Text supported)",
        hash256_to_hex(dh)
    ))
}

// ── Node reference expression ────────────────────────────────────────────────

fn ref_expr(r: &NodeRef) -> String {
    match r {
        NodeRef::Node(i) => format!("node_{}", i),
        NodeRef::Pool(i) => format!("pool_{}", i),
    }
}

fn ref_clone(r: &NodeRef) -> String {
    format!("{}.clone()", ref_expr(r))
}

// ── Node emission ─────────────────────────────────────────────────────────────

fn emit_node(
    node: &Node,
    builtin: &HashMap<Hash256, &'static str>,
    registry: &HashMap<Hash256, String>,
    name_to_path: &HashMap<&'static str, &'static str>,
) -> Result<String, String> {
    match node {
        Node::CCall { target_identity, args, .. } => {
            // Resolve: try builtin map first, then registry → name → builtin map
            let path: String = if let Some(&p) = builtin.get(target_identity) {
                p.to_string()
            } else if let Some(name) = registry.get(target_identity) {
                // registry name → look up bridge path
                if let Some(&p) = name_to_path.get(name.as_str()) {
                    p.to_string()
                } else {
                    return Err(format!(
                        "CCall identity {} resolves to registry name '{}' but \
                         that name has no bridge implementation",
                        hash256_to_hex(target_identity),
                        name
                    ));
                }
            } else {
                return Err(format!(
                    "unresolved CCall identity: {} — not in bridge built-ins or --reg files",
                    hash256_to_hex(target_identity)
                ));
            };

            let call = match args.len() {
                0 => format!("{}(Value::Unit)", path),
                1 => format!("{}({})", path, ref_clone(&args[0])),
                _ => {
                    let arg_exprs: Vec<String> = args.iter().map(ref_clone).collect();
                    format!("{}(Value::Tuple(vec![{}]))", path, arg_exprs.join(", "))
                }
            };
            Ok(call)
        }
        Node::CIf { cond, then_, else_ } => {
            Ok(format!(
                "if axis_codegen_bridge::runtime::value::truthy(&{}) {{ {} }} else {{ {} }}",
                ref_expr(cond),
                ref_clone(then_),
                ref_clone(else_)
            ))
        }
        // A determinacy gate has no operands and yields a Unit discharge token.
        Node::CDeterminate => Ok("Value::Unit".to_string()),
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Sanitise a name into a valid Rust identifier.
pub fn sanitise(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    if s.is_empty() {
        s = "_bundle".to_string();
    }
    s
}

/// Generate a Rust library source file from a 0.5 CoreBundle.
///
/// `fn_name` is the public symbol name for the generated Rust function.
/// `registry_identity_map` maps CCall target identities (from `--reg` files) to function names.
///
/// The generated library exposes:
///   `#[no_mangle] pub extern "C" fn <fn_name>(args: Value) -> Value`
///   `#[no_mangle] pub extern "C" fn _ax_exe_<fn_name>(args: Value) -> Value`
pub fn emit_rust_lib_from_bundle(
    bundle: &CoreBundle,
    fn_name: &str,
    registry_identity_map: &HashMap<Hash256, String>,
) -> Result<String, String> {
    let builtin = bridge_builtin_map();
    let name_to_path = symbol_map();
    let safe_name = sanitise(fn_name);

    let mut out = String::new();
    out.push_str("extern crate axis_codegen_bridge;\n");
    out.push_str("#[allow(unused_imports)]\n");
    out.push_str(
        "use axis_codegen_bridge::runtime::value::{Value, truthy, intern_str, init_runtime};\n\n",
    );

    // Validate: check all CCall targets are resolvable before generating any code
    let mut unresolved: Vec<String> = Vec::new();
    for node in &bundle.nodes {
        if let Node::CCall { target_identity, .. } = node {
            if !builtin.contains_key(target_identity) {
                if let Some(name) = registry_identity_map.get(target_identity) {
                    if !name_to_path.contains_key(name.as_str()) {
                        unresolved.push(format!(
                            "registry name '{}' (identity {}…)",
                            name,
                            &hash256_to_hex(target_identity)[..16]
                        ));
                    }
                } else {
                    unresolved.push(format!(
                        "identity {}…",
                        &hash256_to_hex(target_identity)[..16]
                    ));
                }
            }
        }
    }
    if !unresolved.is_empty() {
        return Err(format!(
            "unresolved CCall targets: {}",
            unresolved.join(", ")
        ));
    }

    // Emit main function
    out.push_str(&format!(
        "#[no_mangle]\npub extern \"C\" fn {}(args: Value) -> Value {{\n",
        safe_name
    ));
    out.push_str("    init_runtime();\n");

    // Pool entries
    for (i, entry) in bundle.constant_pool.iter().enumerate() {
        let value_expr = decode_pool_entry(entry)
            .map_err(|e| format!("pool[{}]: {}", i, e))?;
        out.push_str(&format!("    let pool_{}: Value = {};\n", i, value_expr));
    }
    // Suppress unused-variable warnings for args if pool entries reference it
    if !bundle.constant_pool.is_empty() || !bundle.nodes.is_empty() {
        out.push_str("    let _ = &args;\n");
    }

    // Nodes
    for (i, node) in bundle.nodes.iter().enumerate() {
        let expr =
            emit_node(node, &builtin, registry_identity_map, &name_to_path)
                .map_err(|e| format!("node[{}]: {}", i, e))?;
        out.push_str(&format!("    let node_{}: Value = {};\n", i, expr));
    }

    // Result: last node, or first pool entry, or Unit
    let result = if !bundle.nodes.is_empty() {
        format!("node_{}", bundle.nodes.len() - 1)
    } else if !bundle.constant_pool.is_empty() {
        "pool_0".to_string()
    } else {
        "Value::Unit".to_string()
    };
    out.push_str(&format!("    {}\n", result));
    out.push_str("}\n\n");

    // Exe shim
    out.push_str(&format!(
        "#[no_mangle]\npub extern \"C\" fn _ax_exe_{}(args: Value) -> Value {{\n",
        safe_name
    ));
    out.push_str(&format!("    {}(args)\n", safe_name));
    out.push_str("}\n");

    Ok(out)
}

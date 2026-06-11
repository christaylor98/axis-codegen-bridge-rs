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
    fn_type_hash, hash256_to_hex, int_type_hash, sha256_bytes, text_type_hash,
    unit_type_hash, ConstantPoolEntry, CoreBundle, Hash256, Node, NodeRef,
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

    // Test assertion (identity = sha256("assert") — BRIDGE_TESTKIT_FINALIZE_V1)
    m.insert("assert",   "axis_codegen_bridge::runtime::bool_ops::ax_assert");

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

    // M1 iteration / list-builder primitives (BRIDGE_FOREIGN_FN_FNREF_M1).
    // `foreach` and `loop_count` use the native multi-arg Rust calling
    // convention — the callee is a bare fn path resolved from a Fn-typed
    // pool entry. `range` is data-only and uses the unary Tuple convention.
    m.insert("range",       "axis_codegen_bridge::runtime::iter::range");
    m.insert("foreach",     "axis_codegen_bridge::runtime::iter::foreach");
    m.insert("loop_count",  "axis_codegen_bridge::runtime::iter::loop_count");
    m.insert("str_join",    "axis_codegen_bridge::runtime::str_ops::str_join");

    // Phase 2 — P1 iteration / list vocabulary.
    m.insert("flat_map",    "axis_codegen_bridge::runtime::iter::flat_map");
    m.insert("any",         "axis_codegen_bridge::runtime::iter::any");
    m.insert("all",         "axis_codegen_bridge::runtime::iter::all");
    m.insert("find_index",  "axis_codegen_bridge::runtime::iter::find_index");
    m.insert("count",       "axis_codegen_bridge::runtime::iter::count");
    m.insert("loop_while",  "axis_codegen_bridge::runtime::iter::loop_while");
    m.insert("range_step",  "axis_codegen_bridge::runtime::iter::range_step");
    m.insert("repeat",      "axis_codegen_bridge::runtime::iter::repeat");
    m.insert("enumerate",   "axis_codegen_bridge::runtime::iter::enumerate");
    m.insert("zip",         "axis_codegen_bridge::runtime::iter::zip");
    m.insert("take",        "axis_codegen_bridge::runtime::iter::take");
    m.insert("drop",        "axis_codegen_bridge::runtime::iter::drop");
    m.insert("slice",       "axis_codegen_bridge::runtime::iter::slice");
    m.insert("flatten",     "axis_codegen_bridge::runtime::iter::flatten");

    // Phase 3 — P1 text emit helpers.
    m.insert("str_replace",  "axis_codegen_bridge::runtime::str_ops::str_replace");
    m.insert("str_repeat",   "axis_codegen_bridge::runtime::str_ops::str_repeat");
    m.insert("str_to_upper", "axis_codegen_bridge::runtime::str_ops::str_to_upper");
    m.insert("str_to_lower", "axis_codegen_bridge::runtime::str_ops::str_to_lower");
    m.insert("str_pad_left", "axis_codegen_bridge::runtime::str_ops::str_pad_left");
    m.insert("str_pad_right","axis_codegen_bridge::runtime::str_ops::str_pad_right");

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

    // Signal ping-pong (signals.rs)
    m.insert("ping_loop", "axis_codegen_bridge::runtime::signals::ping_loop");
    m.insert("pong_loop", "axis_codegen_bridge::runtime::signals::pong_loop");

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

// ── Arg-kind metadata (Phase 0: HOF callee-slot signatures) ──────────────────

/// Per-arg kind for a bridge fn.
///
/// `Data`  — an ordinary `Value` argument.
/// `FnRef` — a callee/predicate slot. Must be a Pool ref to a `Fn`-typed entry;
///           emits as a bare Rust fn path (resolved at emit time from the
///           pool entry's 32-byte identity payload).
///
/// Any bridge fn NOT listed in [`fn_arg_kinds`] defaults to all-`Data` — a
/// `Fn`-typed pool ref handed to such a fn fails the type gate. This is the
/// "Fn-as-data is unrepresentable" invariant (intent
/// BRIDGE_FOREIGN_FN_FNREF_M1, FN_REF_IS_CALLEE_ONLY).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArgKind {
    Data,
    FnRef,
}

/// Higher-order primitives — the *only* fns with Fn-typed arg slots.
///
/// A fn here gets emitted as a native multi-arg Rust call (e.g.
/// `foreach(pool_0.clone(), io_println)`) instead of the unary
/// `Value::Tuple`-packed call used for data-only fns.
fn fn_arg_kinds() -> HashMap<&'static str, Vec<ArgKind>> {
    use ArgKind::*;
    let mut m: HashMap<&'static str, Vec<ArgKind>> = HashMap::new();
    m.insert("foreach",    vec![Data, FnRef]);
    m.insert("flat_map",   vec![Data, FnRef]);
    m.insert("any",        vec![Data, FnRef]);
    m.insert("all",        vec![Data, FnRef]);
    m.insert("find_index", vec![Data, FnRef]);
    m.insert("count",      vec![Data, FnRef]);
    m.insert("loop_count", vec![Data, Data, FnRef]);
    m.insert("loop_while", vec![Data, FnRef, FnRef, Data]);
    m
}

// ── Pool constant classification ─────────────────────────────────────────────

/// What a pool entry resolves to under Core IR 0.5.
///
/// `Data(expr)`  — emits `let pool_N: Value = <expr>;` and may be cloned into
///                 any data position.
/// `FnRef(path)` — a bridge symbol path. NO `let pool_N` is emitted. The path
///                 may appear ONLY in a CCall callee/predicate slot (an
///                 `ArgKind::FnRef` position). A reference from any data
///                 position is a HARD ERROR.
#[derive(Debug, Clone)]
enum PoolKind {
    Data(String),
    FnRef(String),
}

fn classify_pool_entry(
    entry: &ConstantPoolEntry,
    builtin: &HashMap<Hash256, &'static str>,
    registry: &HashMap<Hash256, String>,
    name_to_path: &HashMap<&'static str, &'static str>,
) -> Result<PoolKind, String> {
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
        return Ok(PoolKind::Data("Value::Unit".to_string()));
    }
    if dh == &bool_type_hash() {
        let v = decode_bool_payload(&entry.payload)?;
        return Ok(PoolKind::Data(format!("Value::Bool({})", v)));
    }
    if dh == &int_type_hash() {
        let v = decode_int_payload(&entry.payload)?;
        return Ok(PoolKind::Data(format!("Value::Int({})", v)));
    }
    if dh == &text_type_hash() {
        let s = decode_text_payload(&entry.payload)?;
        return Ok(PoolKind::Data(format!(
            "Value::Str(axis_codegen_bridge::runtime::value::intern_str({:?}))",
            s
        )));
    }
    if dh == &fn_type_hash() {
        if entry.payload.len() != 32 {
            return Err(format!(
                "Fn-typed pool entry has malformed payload: expected 32-byte identity, got {} bytes",
                entry.payload.len()
            ));
        }
        let mut id: Hash256 = [0u8; 32];
        id.copy_from_slice(&entry.payload);
        if let Some(&path) = builtin.get(&id) {
            return Ok(PoolKind::FnRef(path.to_string()));
        }
        if let Some(name) = registry.get(&id) {
            if let Some(&path) = name_to_path.get(name.as_str()) {
                return Ok(PoolKind::FnRef(path.to_string()));
            }
            return Err(format!(
                "Fn-typed pool entry resolves to registry name '{}' (identity {}) but \
                 that name has no bridge implementation",
                name,
                hash256_to_hex(&id)
            ));
        }
        return Err(format!(
            "Fn-typed pool entry references unknown identity {} — \
             not a bridge built-in, not in --reg files",
            hash256_to_hex(&id)
        ));
    }
    Err(format!(
        "unknown pool entry type hash: {} (only Unit/Bool/Int/Text/Fn supported)",
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
    pool_kinds: &[PoolKind],
    arg_kind_table: &HashMap<&'static str, Vec<ArgKind>>,
    builtin: &HashMap<Hash256, &'static str>,
    registry: &HashMap<Hash256, String>,
    name_to_path: &HashMap<&'static str, &'static str>,
    xbundle: &HashMap<Hash256, String>,
) -> Result<String, String> {
    match node {
        Node::CCall { target_identity, args, target_name } => {
            // Resolve target → (name, callable_path, is_extern). target_name is
            // mandatory per Core IR 0.5 §"Human Display Format" / CCall.
            let (name, path, is_extern): (String, String, bool) =
                if let Some(&p) = builtin.get(target_identity) {
                    (target_name.clone(), p.to_string(), false)
                } else if let Some(n) = registry.get(target_identity) {
                    match name_to_path.get(n.as_str()) {
                        Some(&p) => (n.clone(), p.to_string(), false),
                        None => return Err(format!(
                            "CCall identity {} resolves to registry name '{}' but \
                             that name has no bridge implementation",
                            hash256_to_hex(target_identity),
                            n
                        )),
                    }
                } else if let Some(sym) = xbundle.get(target_identity) {
                    (target_name.clone(), sym.clone(), true)
                } else {
                    return Err(format!(
                        "unresolved CCall identity: {} — not in bridge built-ins, --reg files, or --lib providers",
                        hash256_to_hex(target_identity)
                    ));
                };

            // Per-arg kind. Default = all Data; a fn with any Fn-slot MUST
            // appear in [`fn_arg_kinds`].
            let declared = arg_kind_table.get(name.as_str());
            if let Some(kinds) = declared {
                if kinds.len() != args.len() {
                    return Err(format!(
                        "CCall '{}' arg count mismatch: declared {} arg-kinds, got {} args",
                        name, kinds.len(), args.len()
                    ));
                }
            }
            let kinds_owned: Vec<ArgKind> =
                declared.cloned().unwrap_or_else(|| vec![ArgKind::Data; args.len()]);

            // Type gate + per-arg expression.
            let mut arg_exprs: Vec<String> = Vec::with_capacity(args.len());
            let mut any_fn_ref = false;
            for (i, arg) in args.iter().enumerate() {
                match kinds_owned[i] {
                    ArgKind::FnRef => {
                        any_fn_ref = true;
                        match arg {
                            NodeRef::Pool(pi) => {
                                let pi_us = *pi as usize;
                                match pool_kinds.get(pi_us) {
                                    Some(PoolKind::FnRef(path)) => arg_exprs.push(path.clone()),
                                    Some(PoolKind::Data(_)) => return Err(format!(
                                        "type gate: CCall '{}' arg[{}] expects Fn but pool[{}] is Data",
                                        name, i, pi
                                    )),
                                    None => return Err(format!(
                                        "CCall '{}' arg[{}]: pool[{}] out of range",
                                        name, i, pi
                                    )),
                                }
                            }
                            NodeRef::Node(j) => return Err(format!(
                                "type gate: CCall '{}' arg[{}] expects Fn but got node[{}] result — \
                                 Fn refs originate from pool entries only",
                                name, i, j
                            )),
                        }
                    }
                    ArgKind::Data => {
                        if let NodeRef::Pool(pi) = arg {
                            let pi_us = *pi as usize;
                            if let Some(PoolKind::FnRef(_)) = pool_kinds.get(pi_us) {
                                return Err(format!(
                                    "type gate: CCall '{}' arg[{}] is a Data slot but pool[{}] is \
                                     Fn-typed — Fn refs are callee-only, never data \
                                     (FN_REF_IS_CALLEE_ONLY)",
                                    name, i, pi
                                ));
                            }
                        }
                        arg_exprs.push(ref_clone(arg));
                    }
                }
            }

            // Calling convention:
            //   any FnRef arg → native multi-arg Rust call (`f(a, b, c)`)
            //   else          → existing Value::Tuple-packed call (data UNARY_INVARIANT)
            let body = if any_fn_ref {
                format!("{}({})", path, arg_exprs.join(", "))
            } else {
                match arg_exprs.len() {
                    0 => format!("{}(Value::Unit)", path),
                    1 => format!("{}({})", path, arg_exprs[0]),
                    _ => format!("{}(Value::Tuple(vec![{}]))", path, arg_exprs.join(", ")),
                }
            };
            Ok(if is_extern { format!("unsafe {{ {} }}", body) } else { body })
        }
        Node::CIf { cond, then_, else_ } => {
            // cond / then / else are Data positions. A Fn-typed pool ref here
            // would be "Fn as data" — reject.
            for (label, r) in &[("cond", cond), ("then", then_), ("else", else_)] {
                if let NodeRef::Pool(pi) = r {
                    let pi_us = *pi as usize;
                    if let Some(PoolKind::FnRef(_)) = pool_kinds.get(pi_us) {
                        return Err(format!(
                            "type gate: CIf {} slot is Data but pool[{}] is Fn-typed — \
                             Fn refs are callee-only, never condition or branch value \
                             (FN_REF_IS_CALLEE_ONLY)",
                            label, pi
                        ));
                    }
                }
            }
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

/// Return true if `identity` resolves to a bridge built-in (not a §5b user fn).
pub fn is_bridge_builtin(identity: &Hash256) -> bool {
    bridge_builtin_map().contains_key(identity)
}

/// Return the bridge runtime path for a built-in identity, or None if not a built-in.
pub fn builtin_path_for_identity(identity: &Hash256) -> Option<&'static str> {
    bridge_builtin_map().get(identity).copied()
}

/// Parse `--reg` files and return identity → in-clause string (e.g. "(TextList)").
///
/// Used at build time to validate the ABI of foreign-fn entries (ENTRY_ABI_MISMATCH check).
/// Falls back to computing sha256(name) as the identity when no explicit `identity` line is present.
pub fn load_registry_in_map(paths: &[String]) -> HashMap<Hash256, String> {
    let mut map = HashMap::new();
    for path in paths {
        let content = match std::fs::read_to_string(path) {
            Ok(c)  => c,
            Err(e) => { eprintln!("warning: could not read --reg {}: {}", path, e); continue; }
        };
        let mut current_identity: Option<Hash256> = None;
        for line in content.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("fn ") {
                let name = rest.split_whitespace().next().unwrap_or("").to_string();
                if !name.is_empty() {
                    current_identity = Some(sha256_bytes(name.as_bytes()));
                }
            } else if let Some(rest) = t.strip_prefix("identity ") {
                let hex = rest.trim().trim_start_matches("0x");
                if let Ok(id) = crate::core_ir_05::hex_to_hash256(hex) {
                    current_identity = Some(id);
                }
            } else if let Some(rest) = t.strip_prefix("in ") {
                if let Some(id) = current_identity {
                    map.insert(id, rest.trim().to_string());
                }
            } else if t == "end" {
                current_identity = None;
            }
        }
    }
    map
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
/// `xbundle_providers` maps §5b target identities to their identity-derived extern symbol names
///   (`ax_fn_<64hex>`). Populated from `--lib` / `--lib-dir` bundles by the driver.
///
/// The generated library exposes:
///   `#[no_mangle] pub extern "C" fn <fn_name>(args: Value) -> Value`
///   `#[no_mangle] pub extern "C" fn ax_fn_<hex>(args: Value) -> Value`  ← identity export
///   `#[no_mangle] pub extern "C" fn _ax_exe_<fn_name>(args: Value) -> Value`
pub fn emit_rust_lib_from_bundle(
    bundle: &CoreBundle,
    fn_name: &str,
    registry_identity_map: &HashMap<Hash256, String>,
    xbundle_providers: &HashMap<Hash256, String>,
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
    let mut errors: Vec<String> = Vec::new();
    for node in &bundle.nodes {
        if let Node::CCall { target_identity, target_name, .. } = node {
            if builtin.contains_key(target_identity) {
                // OK: bridge built-in
            } else if let Some(name) = registry_identity_map.get(target_identity) {
                if !name_to_path.contains_key(name.as_str()) {
                    errors.push(format!(
                        "registry name '{}' (identity {}…) has no bridge implementation",
                        name,
                        &hash256_to_hex(target_identity)[..16]
                    ));
                }
            } else if xbundle_providers.contains_key(target_identity) {
                // OK: §5b extern — provider supplied via --lib
            } else if !target_name.is_empty()
                && sha256_bytes(target_name.as_bytes()) == *target_identity
            {
                errors.push(format!(
                    "UNRESOLVED_XBUNDLE: '{}' (identity {}…) — no provider in --lib set",
                    target_name,
                    &hash256_to_hex(target_identity)[..16]
                ));
            } else {
                errors.push(format!(
                    "UNKNOWN_GATE: identity {}… — not a bridge built-in, not a §5b identity",
                    &hash256_to_hex(target_identity)[..16]
                ));
            }
        }
    }
    if !errors.is_empty() {
        return Err(format!("unresolved CCall targets:\n  {}", errors.join("\n  ")));
    }

    // Collect the distinct §5b extern symbols this bundle calls (for the extern block)
    let mut extern_syms: Vec<String> = Vec::new();
    let mut seen_extern: std::collections::HashSet<Hash256> = std::collections::HashSet::new();
    for node in &bundle.nodes {
        if let Node::CCall { target_identity, .. } = node {
            if let Some(sym) = xbundle_providers.get(target_identity) {
                if seen_extern.insert(*target_identity) {
                    extern_syms.push(sym.clone());
                }
            }
        }
    }

    // Emit extern block for §5b cross-bundle symbols.
    // "C-unwind" allows Rust panics to propagate across the ABI boundary so
    // catch_unwind in the multi-entry driver can isolate per-entry failures.
    if !extern_syms.is_empty() {
        out.push_str("#[allow(improper_ctypes)]\nextern \"C-unwind\" {\n");
        for sym in &extern_syms {
            out.push_str(&format!("    fn {}(args: Value) -> Value;\n", sym));
        }
        out.push_str("}\n\n");
    }

    // Emit main function
    out.push_str(&format!(
        "#[no_mangle]\npub extern \"C-unwind\" fn {}(args: Value) -> Value {{\n",
        safe_name
    ));
    out.push_str("    init_runtime();\n");

    // Classify all pool entries up front. Fn-typed entries are NOT emitted as
    // `let pool_N` — they are resolved to bare Rust fn paths inside CCall.
    let mut pool_kinds: Vec<PoolKind> = Vec::with_capacity(bundle.constant_pool.len());
    for (i, entry) in bundle.constant_pool.iter().enumerate() {
        let kind =
            classify_pool_entry(entry, &builtin, registry_identity_map, &name_to_path)
                .map_err(|e| format!("pool[{}]: {}", i, e))?;
        pool_kinds.push(kind);
    }

    // Pool entries — Data only.
    for (i, kind) in pool_kinds.iter().enumerate() {
        if let PoolKind::Data(expr) = kind {
            out.push_str(&format!("    let pool_{}: Value = {};\n", i, expr));
        }
    }
    // Suppress unused-variable warnings for args if pool entries reference it
    if !bundle.constant_pool.is_empty() || !bundle.nodes.is_empty() {
        out.push_str("    let _ = &args;\n");
    }

    // Nodes
    let arg_kind_table = fn_arg_kinds();
    for (i, node) in bundle.nodes.iter().enumerate() {
        let expr = emit_node(
            node,
            &pool_kinds,
            &arg_kind_table,
            &builtin,
            registry_identity_map,
            &name_to_path,
            xbundle_providers,
        )
        .map_err(|e| format!("node[{}]: {}", i, e))?;
        out.push_str(&format!("    let node_{}: Value = {};\n", i, expr));
    }

    // Result: last node, or first Data pool entry, or Unit.
    // Fn-typed pool entries are skipped — they have no `let pool_N` binding.
    let result = if !bundle.nodes.is_empty() {
        format!("node_{}", bundle.nodes.len() - 1)
    } else if let Some(i) =
        pool_kinds.iter().position(|k| matches!(k, PoolKind::Data(_)))
    {
        format!("pool_{}", i)
    } else {
        "Value::Unit".to_string()
    };
    out.push_str(&format!("    {}\n", result));
    out.push_str("}\n\n");

    // Identity-derived export: ax_fn_<hex(sha256(fn_name))>
    // Callers in other bundles link against this symbol (LINK_BY_IDENTITY).
    // Uses "C-unwind" so panics can propagate through the call chain and be
    // caught by the multi-entry driver's catch_unwind (BRIDGE_ENTRY_POINTS_V1).
    let fn_identity = sha256_bytes(fn_name.as_bytes());
    let identity_sym = format!("ax_fn_{}", hash256_to_hex(&fn_identity));
    out.push_str(&format!(
        "#[no_mangle]\npub extern \"C-unwind\" fn {}(args: Value) -> Value {{\n",
        identity_sym
    ));
    out.push_str(&format!("    {}(args)\n", safe_name));
    out.push_str("}\n\n");

    // Exe shim
    out.push_str(&format!(
        "#[no_mangle]\npub extern \"C-unwind\" fn _ax_exe_{}(args: Value) -> Value {{\n",
        safe_name
    ));
    out.push_str(&format!("    {}(args)\n", safe_name));
    out.push_str("}\n");

    Ok(out)
}

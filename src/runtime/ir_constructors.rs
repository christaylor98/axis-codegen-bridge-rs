use crate::core_ir::{CoreTerm, Provenance, EffectClass, write_core_bundle_to_file, write_core_bundle_multi_to_file, load_core_bundle};
use crate::runtime::value::{Value, intern_str, intern_tag, get_str, get_tag_name};
use std::rc::Rc;

fn make_ctor(tag: &str, fields: Vec<Value>) -> Value {
    Value::Ctor { tag: intern_tag(tag), fields }
}

#[track_caller]
pub fn ir_make_int_lit(v: Value) -> Value {
    match v {
        Value::Int(n) => make_ctor("IntLit", vec![Value::Int(n)]),
        _ => panic!("ir_make_int_lit: expected Int, got {:?}", v),
    }
}

#[track_caller]
pub fn ir_make_bool_lit(v: Value) -> Value {
    match v {
        Value::Bool(b) => make_ctor("BoolLit", vec![Value::Bool(b)]),
        _ => panic!("ir_make_bool_lit: expected Bool, got {:?}", v),
    }
}

#[track_caller]
pub fn ir_make_unit_lit(v: Value) -> Value {
    match v {
        Value::Unit => make_ctor("UnitLit", vec![]),
        _ => panic!("ir_make_unit_lit: expected Unit, got {:?}", v),
    }
}

#[track_caller]
pub fn ir_make_var(v: Value) -> Value {
    match v {
        Value::Str(s) => make_ctor("Var", vec![Value::Str(s)]),
        _ => panic!("ir_make_var: expected Str, got {:?}", v),
    }
}

#[track_caller]
pub fn ir_make_lam(v: Value) -> Value {
    match v {
        Value::Tuple(mut fields) if fields.len() == 2 => {
            let body  = fields.pop().unwrap();
            let param = fields.pop().unwrap();
            if !matches!(param, Value::Str(_)) {
                panic!("ir_make_lam: expected Str param, got {:?}", param);
            }
            make_ctor("Lam", vec![param, body])
        }
        _ => panic!("ir_make_lam: expected Tuple([param, body]), got {:?}", v),
    }
}

#[track_caller]
pub fn ir_make_let(v: Value) -> Value {
    match v {
        Value::Tuple(mut fields) if fields.len() == 3 => {
            let body = fields.pop().unwrap();
            let val  = fields.pop().unwrap();
            let name = fields.pop().unwrap();
            if !matches!(name, Value::Str(_)) {
                panic!("ir_make_let: expected Str name, got {:?}", name);
            }
            make_ctor("Let", vec![name, val, body])
        }
        _ => panic!("ir_make_let: expected Tuple([name, val, body]), got {:?}", v),
    }
}

#[track_caller]
pub fn ir_make_if(v: Value) -> Value {
    match v {
        Value::Tuple(fields) if fields.len() == 3 => make_ctor("If", fields),
        _ => panic!("ir_make_if: expected Tuple([cond, then, else]), got {:?}", v),
    }
}

#[track_caller]
pub fn ir_make_app(v: Value) -> Value {
    match v {
        Value::Tuple(fields) if fields.len() == 2 => make_ctor("App", fields),
        _ => panic!("ir_make_app: expected Tuple([fn, arg]), got {:?}", v),
    }
}

#[track_caller]
pub fn ir_make_call(v: Value) -> Value {
    match v {
        Value::Tuple(mut fields) if fields.len() == 2 => {
            let args   = fields.pop().unwrap();
            let target = fields.pop().unwrap();
            if !matches!(target, Value::Str(_)) {
                panic!("ir_make_call: expected Str target, got {:?}", target);
            }
            if !matches!(args, Value::List(_)) {
                panic!("ir_make_call: expected List args, got {:?}", args);
            }
            make_ctor("Call", vec![target, args])
        }
        _ => panic!("ir_make_call: expected Tuple([target, args]), got {:?}", v),
    }
}

fn term_to_str(v: &Value) -> String {
    match v {
        Value::Ctor { tag, fields } => {
            let kind = get_tag_name(*tag);
            match kind.as_str() {
                "IntLit" => {
                    if let [Value::Int(n)] = fields.as_slice() {
                        format!("int(value = {})", n)
                    } else { kind }
                }
                "BoolLit" => {
                    if let [Value::Bool(b)] = fields.as_slice() {
                        format!("bool(value = {})", b)
                    } else { kind }
                }
                "UnitLit" => "unit()".to_string(),
                "Var" => {
                    if let [Value::Str(s)] = fields.as_slice() {
                        format!("var(name = {})", get_str(*s))
                    } else { kind }
                }
                "Lam" => {
                    if let [Value::Str(param), body] = fields.as_slice() {
                        format!("lam(param = {}, body = {})", get_str(*param), term_to_str(body))
                    } else { kind }
                }
                "Let" => {
                    if let [Value::Str(name), val, body] = fields.as_slice() {
                        format!("let(name = {}, value = {},\nbody = {})",
                            get_str(*name), term_to_str(val), term_to_str(body))
                    } else { kind }
                }
                "If" => {
                    if let [cond, then, els] = fields.as_slice() {
                        format!("if(cond = {}, then = {}, else = {})",
                            term_to_str(cond), term_to_str(then), term_to_str(els))
                    } else { kind }
                }
                "App" => {
                    if let [func, arg] = fields.as_slice() {
                        format!("app(fn = {}, arg = {})", term_to_str(func), term_to_str(arg))
                    } else { kind }
                }
                "Call" => {
                    if let [Value::Str(target), Value::List(args)] = fields.as_slice() {
                        let args_str: Vec<String> = args.iter().map(term_to_str).collect();
                        format!("call(target = {}, args = [{}])", get_str(*target), args_str.join(", "))
                    } else { kind }
                }
                other => other.to_string(),
            }
        }
        other => format!("{:?}", other),
    }
}

#[track_caller]
pub fn ir_to_string(v: Value) -> Value {
    Value::Str(intern_str(&term_to_str(&v)))
}

#[track_caller]
pub fn ir_term_kind(v: Value) -> Value {
    match v {
        Value::Ctor { tag, .. } => Value::Str(intern_str(&get_tag_name(tag))),
        _ => panic!("ir_term_kind: expected Ctor, got {:?}", v),
    }
}

// ── H1 resurfacing ───────────────────────────────────────────────────────────

// Flatten a Let chain into `let x = val;\n...rest` — the H1 block body form.
fn h1_block_body(v: &Value, depth: usize) -> String {
    let ind = "  ".repeat(depth);
    if let Value::Ctor { tag, fields } = v {
        if get_tag_name(*tag) == "Let" {
            if let [Value::Str(name_h), val, body] = fields.as_slice() {
                let name = get_str(*name_h);
                let val_str = h1_expr(val, depth);
                let rest = h1_block_body(body, depth);
                return format!("let {} = {};\n{}{}", name, val_str, ind, rest);
            }
        }
    }
    h1_expr(v, depth)
}

fn h1_app_collect<'a>(v: &'a Value) -> (Vec<&'a Value>, &'a Value) {
    if let Value::Ctor { tag, fields } = v {
        if get_tag_name(*tag) == "App" {
            if let [func, arg] = fields.as_slice() {
                let (mut args, base) = h1_app_collect(func);
                args.push(arg);
                return (args, base);
            }
        }
    }
    (vec![], v)
}

fn h1_app(v: &Value, depth: usize) -> String {
    let (args, base) = h1_app_collect(v);
    let func_str = if let Value::Ctor { tag, fields } = base {
        if get_tag_name(*tag) == "Var" {
            if let [Value::Str(s)] = fields.as_slice() { get_str(*s) }
            else { h1_expr(base, depth) }
        } else {
            format!("({})", h1_expr(base, depth))
        }
    } else {
        h1_expr(base, depth)
    };
    let args_strs: Vec<String> = args.iter().map(|a| h1_expr(a, depth)).collect();
    format!("{}({})", func_str, args_strs.join(", "))
}

fn h1_expr(v: &Value, depth: usize) -> String {
    let ind = "  ".repeat(depth);
    let ind1 = "  ".repeat(depth + 1);
    if let Value::Ctor { tag, fields } = v {
        let kind = get_tag_name(*tag);
        match kind.as_str() {
            "IntLit" => {
                if let [Value::Int(n)] = fields.as_slice() { return format!("{}", n); }
            }
            "BoolLit" => {
                if let [Value::Bool(b)] = fields.as_slice() { return format!("{}", b); }
            }
            "UnitLit" => return "()".to_string(),
            "Var" => {
                if let [Value::Str(s)] = fields.as_slice() { return get_str(*s); }
            }
            "Lam" => {
                if let [Value::Str(param), body] = fields.as_slice() {
                    return format!("fn({}) {{\n{}{}\n{}}}",
                        get_str(*param), ind1, h1_block_body(body, depth + 1), ind);
                }
            }
            "Let" => {
                // Let used as expression — wrap in a block
                return format!("{{\n{}{}\n{}}}", ind1, h1_block_body(v, depth + 1), ind);
            }
            "If" => {
                if let [cond, then, els] = fields.as_slice() {
                    return format!("if {} {{\n{}{}\n{}}} else {{\n{}{}\n{}}}",
                        h1_expr(cond, depth),
                        ind1, h1_block_body(then, depth + 1), ind,
                        ind1, h1_block_body(els, depth + 1), ind);
                }
            }
            "App" => return h1_app(v, depth),
            "Call" => {
                if let [Value::Str(target), Value::List(args)] = fields.as_slice() {
                    let args_strs: Vec<String> = args.iter().map(|a| h1_expr(a, depth)).collect();
                    return format!("{}({})", get_str(*target), args_strs.join(", "));
                }
            }
            other => return other.to_string(),
        }
    }
    format!("{:?}", v)
}

#[track_caller]
pub fn ir_to_h1_string(v: Value) -> Value {
    Value::Str(intern_str(&h1_block_body(&v, 0)))
}

// ── Substitution and renaming ────────────────────────────────────────────────

pub(crate) fn subst_value(name: &str, replacement: &Value, term: Value) -> Value {
    match term {
        Value::Ctor { tag, mut fields } => {
            let kind = get_tag_name(tag);
            match kind.as_str() {
                "Var" => {
                    if fields.len() == 1 {
                        if let Value::Str(n) = &fields[0] {
                            if get_str(*n) == name {
                                return replacement.clone();
                            }
                        }
                    }
                    Value::Ctor { tag, fields }
                }
                "Lam" => {
                    if fields.len() == 2 {
                        let shadowed = if let Value::Str(p) = &fields[0] {
                            get_str(*p) == name
                        } else { false };
                        if shadowed {
                            return Value::Ctor { tag, fields };
                        }
                        let body      = fields.pop().unwrap();
                        let param_val = fields.pop().unwrap();
                        let new_body  = subst_value(name, replacement, body);
                        make_ctor("Lam", vec![param_val, new_body])
                    } else {
                        Value::Ctor { tag, fields }
                    }
                }
                "Let" => {
                    if fields.len() == 3 {
                        let shadowed = if let Value::Str(b) = &fields[0] {
                            get_str(*b) == name
                        } else { false };
                        let body  = fields.pop().unwrap();
                        let val   = fields.pop().unwrap();
                        let bound = fields.pop().unwrap();
                        let new_val = subst_value(name, replacement, val);
                        if shadowed {
                            make_ctor("Let", vec![bound, new_val, body])
                        } else {
                            let new_body = subst_value(name, replacement, body);
                            make_ctor("Let", vec![bound, new_val, new_body])
                        }
                    } else {
                        Value::Ctor { tag, fields }
                    }
                }
                "App" => {
                    if fields.len() == 2 {
                        let arg  = fields.pop().unwrap();
                        let func = fields.pop().unwrap();
                        let nf = subst_value(name, replacement, func);
                        let na = subst_value(name, replacement, arg);
                        make_ctor("App", vec![nf, na])
                    } else {
                        Value::Ctor { tag, fields }
                    }
                }
                "If" => {
                    if fields.len() == 3 {
                        let els  = fields.pop().unwrap();
                        let then = fields.pop().unwrap();
                        let cond = fields.pop().unwrap();
                        let nc = subst_value(name, replacement, cond);
                        let nt = subst_value(name, replacement, then);
                        let ne = subst_value(name, replacement, els);
                        make_ctor("If", vec![nc, nt, ne])
                    } else {
                        Value::Ctor { tag, fields }
                    }
                }
                "Call" => {
                    if fields.len() == 2 {
                        let args_val = fields.pop().unwrap();
                        let target   = fields.pop().unwrap();
                        let new_args = match args_val {
                            Value::List(args) => Value::List(
                                args.into_iter().map(|a| subst_value(name, replacement, a)).collect()
                            ),
                            other => other,
                        };
                        make_ctor("Call", vec![target, new_args])
                    } else {
                        Value::Ctor { tag, fields }
                    }
                }
                // IntLit, BoolLit, UnitLit — no variable positions
                _ => Value::Ctor { tag, fields },
            }
        }
        other => other,
    }
}

/// ir_subst: takes Tuple([name_str, replacement_term, target_term]).
/// Substitutes all free occurrences of name in target with replacement.
/// Respects shadowing: does not descend into Lam/Let that rebind name.
#[track_caller]
pub fn ir_subst(v: Value) -> Value {
    match v {
        Value::Tuple(mut fields) if fields.len() == 3 => {
            let target      = fields.pop().unwrap();
            let replacement = fields.pop().unwrap();
            let name_val    = fields.pop().unwrap();
            let name = match &name_val {
                Value::Str(h) => get_str(*h),
                _ => panic!("ir_subst: expected Str name, got {:?}", name_val),
            };
            subst_value(&name, &replacement, target)
        }
        _ => panic!("ir_subst: expected Tuple([name, replacement, target]), got {:?}", v),
    }
}

/// ir_rename: takes Tuple([old_name_str, new_name_str, lam_term]).
/// Replaces the Lam's param with new_name and substitutes old_name → Var(new_name) in body.
#[track_caller]
pub fn ir_rename(v: Value) -> Value {
    match v {
        Value::Tuple(mut fields) if fields.len() == 3 => {
            let lam_term = fields.pop().unwrap();
            let new_name = fields.pop().unwrap();
            let old_name = fields.pop().unwrap();
            let old_str = match &old_name {
                Value::Str(h) => get_str(*h),
                _ => panic!("ir_rename: expected Str old_name, got {:?}", old_name),
            };
            let new_str = match &new_name {
                Value::Str(h) => get_str(*h),
                _ => panic!("ir_rename: expected Str new_name, got {:?}", new_name),
            };
            match lam_term {
                Value::Ctor { tag, mut fields } if get_tag_name(tag) == "Lam" && fields.len() == 2 => {
                    let body   = fields.pop().unwrap();
                    let _param = fields.pop().unwrap();
                    let new_name_h = intern_str(&new_str);
                    let new_var    = make_ctor("Var", vec![Value::Str(new_name_h)]);
                    let new_body   = subst_value(&old_str, &new_var, body);
                    make_ctor("Lam", vec![Value::Str(new_name_h), new_body])
                }
                other => panic!("ir_rename: expected Ctor(Lam), got {:?}", other),
            }
        }
        _ => panic!("ir_rename: expected Tuple([old_name, new_name, lam_term]), got {:?}", v),
    }
}

// ── Free variable analysis ───────────────────────────────────────────────────

fn free_vars_inner(term: &Value, bound: &std::collections::HashSet<String>) -> std::collections::HashSet<String> {
    match term {
        Value::Ctor { tag, fields } => {
            let kind = get_tag_name(*tag);
            match kind.as_str() {
                "Var" => {
                    if let [Value::Str(n)] = fields.as_slice() {
                        let s = get_str(*n);
                        if !bound.contains(&s) {
                            let mut set = std::collections::HashSet::new();
                            set.insert(s);
                            set
                        } else {
                            std::collections::HashSet::new()
                        }
                    } else {
                        std::collections::HashSet::new()
                    }
                }
                "Lam" => {
                    if let [Value::Str(param), body] = fields.as_slice() {
                        let mut new_bound = bound.clone();
                        new_bound.insert(get_str(*param));
                        free_vars_inner(body, &new_bound)
                    } else {
                        std::collections::HashSet::new()
                    }
                }
                "Let" => {
                    if let [Value::Str(bound_name), val, body] = fields.as_slice() {
                        let fv_val = free_vars_inner(val, bound);
                        let mut new_bound = bound.clone();
                        new_bound.insert(get_str(*bound_name));
                        let fv_body = free_vars_inner(body, &new_bound);
                        fv_val.union(&fv_body).cloned().collect()
                    } else {
                        std::collections::HashSet::new()
                    }
                }
                "App" => {
                    if let [func, arg] = fields.as_slice() {
                        let fv1 = free_vars_inner(func, bound);
                        let fv2 = free_vars_inner(arg,  bound);
                        fv1.union(&fv2).cloned().collect()
                    } else {
                        std::collections::HashSet::new()
                    }
                }
                "If" => {
                    if let [cond, then, els] = fields.as_slice() {
                        let fc = free_vars_inner(cond, bound);
                        let ft = free_vars_inner(then, bound);
                        let fe = free_vars_inner(els,  bound);
                        let tmp: std::collections::HashSet<_> = fc.union(&ft).cloned().collect();
                        tmp.union(&fe).cloned().collect()
                    } else {
                        std::collections::HashSet::new()
                    }
                }
                "Call" => {
                    if let [_target, Value::List(args)] = fields.as_slice() {
                        args.iter().flat_map(|a| free_vars_inner(a, bound)).collect()
                    } else {
                        std::collections::HashSet::new()
                    }
                }
                _ => std::collections::HashSet::new(),
            }
        }
        _ => std::collections::HashSet::new(),
    }
}

/// ir_free_vars: takes any IR Ctor term.
/// Returns a sorted List of Str — all free variable names in the term.
#[track_caller]
pub fn ir_free_vars(v: Value) -> Value {
    let fvs = free_vars_inner(&v, &std::collections::HashSet::new());
    let mut result: Vec<Value> = fvs.into_iter()
        .map(|s| Value::Str(intern_str(&s)))
        .collect();
    result.sort_by(|a, b| {
        let sa = if let Value::Str(h) = a { get_str(*h) } else { String::new() };
        let sb = if let Value::Str(h) = b { get_str(*h) } else { String::new() };
        sa.cmp(&sb)
    });
    Value::List(result)
}

/// Write a Core IR bundle to a file.
///
/// Single-export form (backward compatible):
///   Tuple([term, Str path, Str effectClass, Bool idempotent])
///
/// Multi-export form:
///   Tuple([List(Tuple([Str name, term, Str effectSig])), Str path, Bool idempotent])
#[track_caller]
pub fn ir_write_bundle(v: Value) -> Value {
    match v {
        // Multi-export: Tuple([List(Tuple([name, term, effectSig])), path, idempotent])
        Value::Tuple(ref fields) if fields.len() == 3 => {
            let exports_val  = &fields[0];
            let path_val     = &fields[1];
            let idempotent_val = &fields[2];
            let path = match path_val {
                Value::Str(s) => get_str(*s),
                _ => panic!("ir_write_bundle(multi): expected Str path, got {:?}", path_val),
            };
            let idempotent = match idempotent_val {
                Value::Bool(b) => *b,
                _ => panic!("ir_write_bundle(multi): expected Bool idempotent, got {:?}", idempotent_val),
            };
            let export_list = match exports_val {
                Value::List(items) => items,
                _ => panic!("ir_write_bundle(multi): expected List exports, got {:?}", exports_val),
            };
            let mut entries: Vec<(String, CoreTerm, String)> = Vec::new();
            for item in export_list {
                match item {
                    Value::Tuple(parts) if parts.len() == 3 => {
                        let name = match &parts[0] {
                            Value::Str(s) => get_str(*s).to_string(),
                            other => panic!("ir_write_bundle(multi): export name must be Str, got {:?}", other),
                        };
                        let term = value_to_core_term(&parts[1])
                            .unwrap_or_else(|e| panic!("ir_write_bundle(multi): {}", e));
                        let effect_sig = match &parts[2] {
                            Value::Str(s) => get_str(*s).to_string(),
                            other => panic!("ir_write_bundle(multi): effectSig must be Str, got {:?}", other),
                        };
                        entries.push((name, term, effect_sig));
                    }
                    other => panic!("ir_write_bundle(multi): each export must be Tuple([name,term,effectSig]), got {:?}", other),
                }
            }
            let refs: Vec<(&str, &CoreTerm, &str)> = entries.iter()
                .map(|(n, t, e)| (n.as_str(), t, e.as_str()))
                .collect();
            write_core_bundle_multi_to_file(&refs, Provenance::Mechanical, idempotent, &path)
                .unwrap_or_else(|e| panic!("ir_write_bundle(multi) write failed: {}", e));
            Value::Unit
        }
        // Single-export: Tuple([term, path, effectClass, idempotent])
        Value::Tuple(mut fields) if fields.len() == 4 => {
            let idempotent_val   = fields.pop().unwrap();
            let effect_class_val = fields.pop().unwrap();
            let path_val         = fields.pop().unwrap();
            let term_val         = fields.pop().unwrap();
            let path = match &path_val {
                Value::Str(s) => get_str(*s),
                _ => panic!("ir_write_bundle: expected Str path, got {:?}", path_val),
            };
            let effect_class_str = match &effect_class_val {
                Value::Str(s) => get_str(*s),
                _ => panic!("ir_write_bundle: expected Str effectClass, got {:?}", effect_class_val),
            };
            let idempotent = match idempotent_val {
                Value::Bool(b) => b,
                _ => panic!("ir_write_bundle: expected Bool idempotent, got {:?}", idempotent_val),
            };
            let effect_class = match effect_class_str.as_str() {
                "pure"    => EffectClass::Pure,
                "reads"   => EffectClass::Reads,
                "writes"  => EffectClass::Writes,
                "full_io" => EffectClass::FullIo,
                other     => panic!("ir_write_bundle: unknown effectClass {:?}", other),
            };
            let term = value_to_core_term(&term_val)
                .unwrap_or_else(|e| panic!("ir_write_bundle: {}", e));
            write_core_bundle_to_file(&term, "bundle", Provenance::Mechanical, effect_class, idempotent, &path)
                .unwrap_or_else(|e| panic!("ir_write_bundle write failed: {}", e));
            Value::Unit
        }
        _ => panic!("ir_write_bundle: expected Tuple([term,path,effect,idem]) or Tuple([exports_list,path,idem]), got {:?}", v),
    }
}

#[track_caller]
pub fn ir_read_bundle(v: Value) -> Value {
    let path = match v {
        Value::Str(s) => get_str(s),
        _ => panic!("ir_read_bundle: expected Str path, got {:?}", v),
    };

    // Try 0.4 first to preserve existing semantics on 0.3/0.4 bundles.
    // On failure (including version mismatch), fall back to the 0.5 loader and
    // lift its flat node graph into the CoreTerm-shaped Value tree that
    // ir_to_string/ir_to_h1_string expect.
    match load_core_bundle(&path) {
        Ok(prog) => core_term_to_value(&prog.root_term),
        Err(e04) => match crate::core_ir_05::load_core_bundle(&path) {
            Ok(bundle) => value_from_bundle_05(&bundle),
            Err(e05) => panic!(
                "ir_read_bundle: failed as 0.4 ({}) and as 0.5 ({})",
                e04, e05
            ),
        },
    }
}

fn value_from_bundle_05(bundle: &crate::core_ir_05::CoreBundle) -> Value {
    // 0.5 convention: program result is the last node. When there are no nodes
    // (e.g. a bundle that resolves to a single pool literal), the result is the
    // last pool entry.
    use crate::core_ir_05::NodeRef;
    if !bundle.nodes.is_empty() {
        return walk_node_05(bundle, bundle.nodes.len() - 1);
    }
    if !bundle.constant_pool.is_empty() {
        return walk_ref_05(bundle, &NodeRef::Pool((bundle.constant_pool.len() - 1) as u32));
    }
    make_ctor("UnitLit", vec![])
}

fn walk_node_05(bundle: &crate::core_ir_05::CoreBundle, idx: usize) -> Value {
    use crate::core_ir_05::{hash256_to_hex, Node};
    match &bundle.nodes[idx] {
        Node::CCall { target_identity, args, .. } => {
            let hex = hash256_to_hex(target_identity);
            let target = format!("#{}", &hex[..16]);
            let arg_vals: Vec<Value> =
                args.iter().map(|r| walk_ref_05(bundle, r)).collect();
            make_ctor("Call", vec![
                Value::Str(intern_str(&target)),
                Value::List(arg_vals),
            ])
        }
        Node::CIf { cond, then_, else_ } => make_ctor("If", vec![
            walk_ref_05(bundle, cond),
            walk_ref_05(bundle, then_),
            walk_ref_05(bundle, else_),
        ]),
    }
}

fn walk_ref_05(bundle: &crate::core_ir_05::CoreBundle, r: &crate::core_ir_05::NodeRef) -> Value {
    use crate::core_ir_05::{
        bool_type_hash, decode_bool_payload, decode_int_payload, decode_text_payload,
        hash256_to_hex, int_type_hash, text_type_hash, unit_type_hash, NodeRef,
    };
    match r {
        NodeRef::Node(i) => walk_node_05(bundle, *i as usize),
        NodeRef::Pool(i) => {
            let entry = match bundle.constant_pool.get(*i as usize) {
                Some(e) => e,
                None => return make_ctor("Var", vec![Value::Str(
                    intern_str(&format!("<pool#{} oob>", i))
                )]),
            };
            let dh = entry.def_hash;
            if dh == unit_type_hash() {
                make_ctor("UnitLit", vec![])
            } else if dh == bool_type_hash() {
                match decode_bool_payload(&entry.payload) {
                    Ok(b) => make_ctor("BoolLit", vec![Value::Bool(b)]),
                    Err(_) => make_ctor("Var", vec![Value::Str(intern_str("<bad-bool>"))]),
                }
            } else if dh == int_type_hash() {
                match decode_int_payload(&entry.payload) {
                    Ok(n) => make_ctor("IntLit", vec![Value::Int(n)]),
                    Err(_) => make_ctor("Var", vec![Value::Str(intern_str("<bad-int>"))]),
                }
            } else if dh == text_type_hash() {
                match decode_text_payload(&entry.payload) {
                    // No CoreTerm text literal — represent as a Call("text", [Var(s)])
                    // so term_to_str renders the contents instead of dropping them.
                    Ok(s) => make_ctor("Call", vec![
                        Value::Str(intern_str("text")),
                        Value::List(vec![
                            make_ctor("Var", vec![Value::Str(intern_str(&s))]),
                        ]),
                    ]),
                    Err(_) => make_ctor("Var", vec![Value::Str(intern_str("<bad-text>"))]),
                }
            } else {
                let hex = hash256_to_hex(&dh);
                let name = format!("<pool#{}@{}>", i, &hex[..16]);
                make_ctor("Var", vec![Value::Str(intern_str(&name))])
            }
        }
    }
}

/// ir_bundle_view: takes Str path.
/// Reads a .coreir file in either Core IR 0.4 or 0.5 format and returns a
/// human-readable string. Never panics — on error, returns the error message.
#[track_caller]
pub fn ir_bundle_view(v: Value) -> Value {
    let path = match v {
        Value::Str(s) => get_str(s),
        other => return Value::Str(intern_str(&format!("ir_bundle_view: expected Str path, got {:?}", other))),
    };

    // Try 0.5 first (ratchet cache stores 0.5 bundles).
    match crate::core_ir_05::load_core_bundle(&path) {
        Ok(bundle) => {
            let mut out = String::from("CoreIR 0.5\n");
            // Pool entries
            for (i, entry) in bundle.constant_pool.iter().enumerate() {
                use crate::core_ir_05::{
                    bool_type_hash, decode_bool_payload, decode_int_payload,
                    decode_text_payload, hash256_to_hex, int_type_hash,
                    text_type_hash, unit_type_hash,
                };
                let dh = entry.def_hash;
                let desc = if dh == unit_type_hash() {
                    "Unit".to_string()
                } else if dh == bool_type_hash() {
                    match decode_bool_payload(&entry.payload) {
                        Ok(b) => format!("Bool({})", b),
                        Err(e) => format!("Bool(<err: {}>)", e),
                    }
                } else if dh == int_type_hash() {
                    match decode_int_payload(&entry.payload) {
                        Ok(n) => format!("Int({})", n),
                        Err(e) => format!("Int(<err: {}>)", e),
                    }
                } else if dh == text_type_hash() {
                    match decode_text_payload(&entry.payload) {
                        Ok(s) => format!("Text({:?})", s),
                        Err(e) => format!("Text(<err: {}>)", e),
                    }
                } else {
                    let hex = hash256_to_hex(&dh);
                    format!("Unknown(def={})", &hex[..16])
                };
                out.push_str(&format!("pool[{}]: {}\n", i, desc));
            }
            // Graph nodes
            use crate::core_ir_05::{hash256_to_hex, Node, NodeRef};
            for (i, node) in bundle.nodes.iter().enumerate() {
                let desc = match node {
                    Node::CCall { target_identity, args, .. } => {
                        let hex = hash256_to_hex(target_identity);
                        let args_str: Vec<String> = args.iter().map(|r| match r {
                            NodeRef::Node(n) => format!("node[{}]", n),
                            NodeRef::Pool(p) => format!("pool[{}]", p),
                        }).collect();
                        format!("CCall(target={}..., args=[{}])", &hex[..16], args_str.join(", "))
                    }
                    Node::CIf { cond, then_, else_ } => {
                        let ref_str = |r: &NodeRef| match r {
                            NodeRef::Node(n) => format!("node[{}]", n),
                            NodeRef::Pool(p) => format!("pool[{}]", p),
                        };
                        format!("CIf(cond={}, then={}, else={})",
                            ref_str(cond), ref_str(then_), ref_str(else_))
                    }
                };
                out.push_str(&format!("node[{}]: {}\n", i, desc));
            }
            Value::Str(intern_str(&out))
        }
        Err(e05) => {
            // Fall back to 0.4 path and render via ir_to_string.
            match load_core_bundle(&path) {
                Ok(prog) => {
                    let term_val = core_term_to_value(&prog.root_term);
                    let body = format!("CoreIR 0.4\n{}", term_to_str(&term_val));
                    Value::Str(intern_str(&body))
                }
                Err(e04) => {
                    let msg = format!(
                        "ir_bundle_view: failed as 0.5 ({}) and as 0.4 ({})",
                        e05, e04
                    );
                    Value::Str(intern_str(&msg))
                }
            }
        }
    }
}

#[track_caller]
pub fn ir_build_program_from_spec(v: Value) -> Value {
    let path = match v {
        Value::Str(s) => get_str(s),
        _ => panic!("ir_build_program_from_spec: expected Str path, got {:?}", v),
    };

    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("ir_build_program_from_spec: cannot read '{}': {}", path, e));

    let lines: Vec<&str> = content.lines()
        .filter(|l| !l.trim().is_empty())
        .collect();

    if lines.len() < 2 {
        panic!("ir_build_program_from_spec: spec file too short ({} lines): {}", lines.len(), path);
    }

    let effect_str = lines[0].trim();
    match effect_str {
        "pure" | "reads" | "writes" | "full_io" => {}
        _ => panic!("ir_build_program_from_spec: invalid effect_class {:?} in {}", effect_str, path),
    }

    let n: usize = lines[1].trim().parse()
        .unwrap_or_else(|_| panic!("ir_build_program_from_spec: invalid N {:?} in {}", lines[1].trim(), path));
    if n < 1 || n > 8 {
        panic!("ir_build_program_from_spec: N={} out of range (must be 1-8) in {}", n, path);
    }

    // Variable-arity, cursor-based parse. Per step: name, fn, nargs, then nargs
    // (arg_type, arg_val) pairs. Final: fn, nargs, then nargs pairs. No arity cap.
    let parse_arg = |typ: &str, val: &str| -> Value {
        let typ_int: i64 = typ.trim().parse()
            .unwrap_or_else(|_| panic!("ir_build_program_from_spec: invalid arg_type {:?}", typ));
        match typ_int {
            0 => make_ctor("Var", vec![Value::Str(intern_str(val.trim()))]),
            1 => {
                let lit_n: i64 = val.trim().parse()
                    .unwrap_or_else(|_| panic!("ir_build_program_from_spec: invalid int literal {:?}", val));
                make_ctor("IntLit", vec![Value::Int(lit_n)])
            }
            _ => panic!("ir_build_program_from_spec: invalid arg_type {}", typ_int),
        }
    };

    let mut idx = 2usize; // cursor past effect + N
    let read_args = |idx: &mut usize, nargs: usize, where_: &str| -> Vec<Value> {
        if *idx + nargs * 2 > lines.len() {
            panic!("ir_build_program_from_spec: spec too short for {} args at {} in {}", nargs, where_, path);
        }
        let mut args = Vec::with_capacity(nargs);
        for _ in 0..nargs {
            args.push(parse_arg(lines[*idx], lines[*idx + 1]));
            *idx += 2;
        }
        args
    };

    // Build step call terms and collect (binding_name, call_term)
    let mut steps: Vec<(String, Value)> = Vec::with_capacity(n);
    for k in 0..n {
        if idx + 3 > lines.len() {
            panic!("ir_build_program_from_spec: spec too short at step {} in {}", k, path);
        }
        let binding_name = lines[idx].trim().to_string();
        let fn_name      = lines[idx + 1].trim().to_string();
        let nargs: usize = lines[idx + 2].trim().parse()
            .unwrap_or_else(|_| panic!("ir_build_program_from_spec: invalid nargs at step {}", k));
        if nargs < 1 {
            panic!("ir_build_program_from_spec: nargs={} invalid at step {} (must be >= 1)", nargs, k);
        }
        idx += 3;
        let args = read_args(&mut idx, nargs, &format!("step {}", k));
        let call = make_ctor("Call", vec![
            Value::Str(intern_str(&fn_name)),
            Value::List(args),
        ]);
        steps.push((binding_name, call));
    }

    // Build final call term
    if idx + 2 > lines.len() {
        panic!("ir_build_program_from_spec: spec missing final call in {}", path);
    }
    let final_fn     = lines[idx].trim().to_string();
    let final_nargs: usize = lines[idx + 1].trim().parse()
        .unwrap_or_else(|_| panic!("ir_build_program_from_spec: invalid final nargs in {}", path));
    if final_nargs < 1 {
        panic!("ir_build_program_from_spec: final nargs={} invalid (must be >= 1) in {}", final_nargs, path);
    }
    idx += 2;
    let final_args = read_args(&mut idx, final_nargs, "final");
    if idx != lines.len() {
        panic!("ir_build_program_from_spec: {} trailing line(s) in {}", lines.len() - idx, path);
    }
    let final_call = make_ctor("Call", vec![
        Value::Str(intern_str(&final_fn)),
        Value::List(final_args),
    ]);

    // Wrap steps in nested Let nodes, innermost first
    let mut term = final_call;
    for (binding_name, call) in steps.into_iter().rev() {
        term = make_ctor("Let", vec![
            Value::Str(intern_str(&binding_name)),
            call,
            term,
        ]);
    }

    Value::Tuple(vec![term, Value::Str(intern_str(effect_str))])
}

/// ir_build_fold_from_spec: takes Str spec_path.
/// Parses a fold spec (key: value lines) and builds an unrolled forEach IR.
/// Returns Tuple(term, Str effect_class).
/// Supports up to 32 list elements (TODO: replace with letrec when recursion lands).
#[track_caller]
pub fn ir_build_fold_from_spec(v: Value) -> Value {
    let path = match v {
        Value::Str(s) => get_str(s),
        _ => panic!("ir_build_fold_from_spec: expected Str path, got {:?}", v),
    };

    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("ir_build_fold_from_spec: cannot read '{}': {}", path, e));

    let mut effect              = String::new();
    let mut source_fn           = String::new();
    let mut source_nargs        = 0usize;
    let mut transform_fn        = String::new();
    let mut mode                = String::new();
    let mut threshold_str       = String::new();
    let mut source_pipe_fn      = String::new();
    let mut source_pipe_unwrap  = String::new();
    // Collect source_arg{i}_type and source_arg{i}_val keyed by 1-based index.
    let mut source_arg_types: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    let mut source_arg_vals:  std::collections::HashMap<usize, String> = std::collections::HashMap::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }
        if let Some((key, val)) = trimmed.split_once(':') {
            let k = key.trim();
            let v = val.trim().to_string();
            match k {
                "effect"              => effect             = v,
                "source_fn"           => source_fn          = v,
                "source_nargs" => {
                    source_nargs = v.parse()
                        .unwrap_or_else(|_| panic!("ir_build_fold_from_spec: invalid source_nargs {:?} in {}", v, path));
                }
                "transform_fn"        => transform_fn       = v,
                "mode"                => mode               = v,
                "threshold"           => threshold_str      = v,
                "source_pipe_fn"      => source_pipe_fn     = v,
                "source_pipe_unwrap"  => source_pipe_unwrap = v,
                _ => {
                    // source_arg{i}_type  and  source_arg{i}_val
                    if let Some(rest) = k.strip_prefix("source_arg") {
                        if let Some(idx_str) = rest.strip_suffix("_type") {
                            if let Ok(idx) = idx_str.parse::<usize>() {
                                source_arg_types.insert(idx, v);
                            }
                        } else if let Some(idx_str) = rest.strip_suffix("_val") {
                            if let Ok(idx) = idx_str.parse::<usize>() {
                                source_arg_vals.insert(idx, v);
                            }
                        }
                    }
                }
            }
        }
    }

    if effect.is_empty()    { panic!("ir_build_fold_from_spec: missing 'effect' in {}", path); }
    if source_fn.is_empty() { panic!("ir_build_fold_from_spec: missing 'source_fn' in {}", path); }
    // transform_fn is required only for forEach mode (mode == "" or mode == "forEach")
    if mode.is_empty() && transform_fn.is_empty() {
        panic!("ir_build_fold_from_spec: missing 'transform_fn' in {}", path);
    }

    match effect.as_str() {
        "pure" | "reads" | "writes" | "full_io" => {}
        _ => panic!("ir_build_fold_from_spec: invalid effect {:?}", effect),
    }

    // Build the source call argument list (shared by all modes).
    // - source_nargs == 0 (or absent): arity-1 unit function (e.g. proc_args, io_read_line)
    //   → call with a single UnitLit so the verifier sees arity 1.
    // - source_nargs >= 1: typed args supplied in the spec.
    let source_args: Vec<Value> = if source_nargs == 0 {
        vec![make_ctor("UnitLit", vec![])]
    } else {
        (1..=source_nargs).map(|i| {
            let typ_str = source_arg_types.get(&i)
                .unwrap_or_else(|| panic!("ir_build_fold_from_spec: missing source_arg{}_type in {}", i, path));
            let val_str = source_arg_vals.get(&i)
                .unwrap_or_else(|| panic!("ir_build_fold_from_spec: missing source_arg{}_val in {}", i, path));
            let typ_int: i64 = typ_str.parse()
                .unwrap_or_else(|_| panic!("ir_build_fold_from_spec: invalid source_arg{}_type {:?} in {}", i, typ_str, path));
            match typ_int {
                0 => make_ctor("Var", vec![Value::Str(intern_str(val_str.as_str()))]),
                1 => {
                    let n: i64 = val_str.parse()
                        .unwrap_or_else(|_| panic!("ir_build_fold_from_spec: invalid int literal {:?} for source_arg{}_val in {}", val_str, i, path));
                    make_ctor("IntLit", vec![Value::Int(n)])
                }
                2 => {
                    let n: i64 = val_str.parse()
                        .unwrap_or_else(|_| panic!("ir_build_fold_from_spec: source_arg{}_val must be integer for type 2 (Argv), got {:?} in {}", i, val_str, path));
                    make_ctor("Call", vec![
                        Value::Str(intern_str("argv")),
                        Value::List(vec![make_ctor("IntLit", vec![Value::Int(n)])]),
                    ])
                }
                _ => panic!("ir_build_fold_from_spec: invalid source_arg{}_type {} in {}", i, typ_int, path),
            }
        }).collect()
    };

    // If source_pipe_fn is set, wrap source_args[0] through the pipe chain before
    // passing to source_fn: source_fn(source_pipe_unwrap(source_pipe_fn(arg1)), arg2, ...)
    let source_args = if !source_pipe_fn.is_empty() && !source_args.is_empty() {
        let mut args = source_args;
        let piped = make_ctor("Call", vec![
            Value::Str(intern_str(&source_pipe_fn)),
            Value::List(vec![args[0].clone()]),
        ]);
        args[0] = if source_pipe_unwrap.is_empty() {
            piped
        } else {
            make_ctor("Call", vec![
                Value::Str(intern_str(&source_pipe_unwrap)),
                Value::List(vec![piped]),
            ])
        };
        args
    } else {
        source_args
    };

    let raw_source_call = make_ctor("Call", vec![
        Value::Str(intern_str(&source_fn)),
        Value::List(source_args),
    ]);

    // proc_args returns all argv including argv[0] (the binary name).
    // Use proc_args directly as TextList (wrapping in list_tail causes a
    // TextList/Value type mismatch in the 0.5 verifier). Instead, start loop
    // indices at 1 to skip argv[0] when the source is proc_args.
    let source_call = raw_source_call;
    let start_index: usize = if source_fn == "proc_args" { 1 } else { 0 };

    const N: usize = 32;

    let term = match mode.as_str() {
        "count_if_str_len_lte" => {
            // Unrolled count: sum list_str_len_lte_if_some over 32 slots, then println.
            let threshold: i64 = threshold_str.parse()
                .unwrap_or_else(|_| panic!("ir_build_fold_from_spec: invalid threshold {:?} in {}", threshold_str, path));

            // Innermost: io_println(int_to_str(_s30))
            let final_sum_var = format!("_s{}", N - 2);
            let mut term = make_ctor("Call", vec![
                Value::Str(intern_str("io_println")),
                Value::List(vec![
                    make_ctor("Call", vec![
                        Value::Str(intern_str("int_to_str")),
                        Value::List(vec![make_ctor("Var", vec![Value::Str(intern_str(&final_sum_var))])]),
                    ]),
                ]),
            ]);

            // Left-fold sum bindings inside-out: _s30=int_add(_s29,_b31) ... _s0=int_add(_b0,_b1)
            for j in (0..N - 1).rev() {
                let lhs_name = if j == 0 { "_b0".to_string() } else { format!("_s{}", j - 1) };
                let rhs_name = format!("_b{}", j + 1);
                let sum_name = format!("_s{}", j);
                term = make_ctor("Let", vec![
                    Value::Str(intern_str(&sum_name)),
                    make_ctor("Call", vec![
                        Value::Str(intern_str("int_add")),
                        Value::List(vec![
                            make_ctor("Var", vec![Value::Str(intern_str(&lhs_name))]),
                            make_ctor("Var", vec![Value::Str(intern_str(&rhs_name))]),
                        ]),
                    ]),
                    term,
                ]);
            }

            // Indicator bindings inside-out: _b31=lsllis(lst,31,thr) ... _b0=lsllis(lst,0,thr)
            for i in (0..N).rev() {
                let b_name = format!("_b{}", i);
                term = make_ctor("Let", vec![
                    Value::Str(intern_str(&b_name)),
                    make_ctor("Call", vec![
                        Value::Str(intern_str("list_str_len_lte_if_some")),
                        Value::List(vec![
                            make_ctor("Var", vec![Value::Str(intern_str("lst"))]),
                            make_ctor("IntLit", vec![Value::Int((i + start_index) as i64)]),
                            make_ctor("IntLit", vec![Value::Int(threshold)]),
                        ]),
                    ]),
                    term,
                ]);
            }

            make_ctor("Let", vec![Value::Str(intern_str("lst")), source_call, term])
        }
        "" => {
            // Build unrolled forEach: for i in 0..N, if list[i] exists, call transform_fn on it.
            let mut term = make_ctor("UnitLit", vec![]);

            for i in (0..N).rev() {
                let res_name = format!("_res{}", i);

                // When transform_fn is io_println, use list_get_println_if_some which
                // handles the None case atomically in Rust — no CIf or option_unwrap
                // needed. This is required for 0.5 bundles where all CCall nodes in the
                // flat list are evaluated eagerly regardless of CIf control flow.
                let iter_expr = if transform_fn == "io_println" {
                    make_ctor("Call", vec![
                        Value::Str(intern_str("list_get_println_if_some")),
                        Value::List(vec![
                            make_ctor("Var", vec![Value::Str(intern_str("lst"))]),
                            make_ctor("IntLit", vec![Value::Int((i + start_index) as i64)]),
                        ]),
                    ])
                } else {
                    // For other transforms: guard with CIf so option_unwrap is only
                    // reached when the index is in-bounds. Works in 0.4 (ir_eval) mode;
                    // 0.5 lowering will still eager-evaluate both branches.
                    let cond = make_ctor("Call", vec![
                        Value::Str(intern_str("int_gt")),
                        Value::List(vec![
                            make_ctor("Var", vec![Value::Str(intern_str("n"))]),
                            make_ctor("IntLit", vec![Value::Int((i + start_index) as i64)]),
                        ]),
                    ]);
                    let get_call = make_ctor("Call", vec![
                        Value::Str(intern_str("list_get_at")),
                        Value::List(vec![
                            make_ctor("Var", vec![Value::Str(intern_str("lst"))]),
                            make_ctor("IntLit", vec![Value::Int((i + start_index) as i64)]),
                        ]),
                    ]);
                    let unwrapped = make_ctor("Call", vec![
                        Value::Str(intern_str("option_unwrap")),
                        Value::List(vec![get_call]),
                    ]);
                    let then_branch = make_ctor("Call", vec![
                        Value::Str(intern_str(&transform_fn)),
                        Value::List(vec![unwrapped]),
                    ]);
                    make_ctor("If", vec![cond, then_branch, make_ctor("UnitLit", vec![])])
                };

                term = make_ctor("Let", vec![
                    Value::Str(intern_str(&res_name)),
                    iter_expr,
                    term,
                ]);
            }

            let len_call = make_ctor("Call", vec![
                Value::Str(intern_str("list_len")),
                Value::List(vec![make_ctor("Var", vec![Value::Str(intern_str("lst"))])]),
            ]);
            term = make_ctor("Let", vec![Value::Str(intern_str("n")), len_call, term]);
            make_ctor("Let", vec![Value::Str(intern_str("lst")), source_call, term])
        }
        other => panic!("ir_build_fold_from_spec: unknown mode {:?} in {}", other, path),
    };

    Value::Tuple(vec![term, Value::Str(intern_str(&effect))])
}

// ── Internal conversions ─────────────────────────────────────────────────────

fn value_to_core_term(v: &Value) -> Result<CoreTerm, String> {
    match v {
        Value::Ctor { tag, fields } => {
            let kind = get_tag_name(*tag);
            match kind.as_str() {
                "IntLit" => match fields.as_slice() {
                    [Value::Int(n)] => Ok(CoreTerm::IntLit(*n, None)),
                    _ => Err(format!("IntLit: expected [Int], got {:?}", fields)),
                },
                "BoolLit" => match fields.as_slice() {
                    [Value::Bool(b)] => Ok(CoreTerm::BoolLit(*b, None)),
                    _ => Err(format!("BoolLit: expected [Bool], got {:?}", fields)),
                },
                "UnitLit" => Ok(CoreTerm::UnitLit(None)),
                "Var" => match fields.as_slice() {
                    [Value::Str(s)] => Ok(CoreTerm::Var(get_str(*s), None)),
                    _ => Err(format!("Var: expected [Str], got {:?}", fields)),
                },
                "Lam" => match fields.as_slice() {
                    [Value::Str(param), body] => {
                        let b = value_to_core_term(body)?;
                        Ok(CoreTerm::Lam(get_str(*param), Rc::new(b), None))
                    }
                    _ => Err(format!("Lam: expected [Str, term], got {:?}", fields)),
                },
                "Let" => match fields.as_slice() {
                    [Value::Str(name), val, body] => {
                        let v = value_to_core_term(val)?;
                        let b = value_to_core_term(body)?;
                        Ok(CoreTerm::Let(get_str(*name), Rc::new(v), Rc::new(b), None))
                    }
                    _ => Err(format!("Let: expected [Str, term, term], got {:?}", fields)),
                },
                "If" => match fields.as_slice() {
                    [cond, then, els] => {
                        let c = value_to_core_term(cond)?;
                        let t = value_to_core_term(then)?;
                        let e = value_to_core_term(els)?;
                        Ok(CoreTerm::If(Rc::new(c), Rc::new(t), Rc::new(e), None))
                    }
                    _ => Err(format!("If: expected [term, term, term], got {:?}", fields)),
                },
                "App" => match fields.as_slice() {
                    [func, arg] => {
                        let f = value_to_core_term(func)?;
                        let a = value_to_core_term(arg)?;
                        Ok(CoreTerm::App(Rc::new(f), Rc::new(a), None))
                    }
                    _ => Err(format!("App: expected [term, term], got {:?}", fields)),
                },
                "Call" => match fields.as_slice() {
                    [Value::Str(target), Value::List(args)] => {
                        let tgt = get_str(*target);
                        let mut cargs = Vec::with_capacity(args.len());
                        for a in args {
                            cargs.push(value_to_core_term(a)?);
                        }
                        Ok(CoreTerm::Call(tgt, cargs, None))
                    }
                    _ => Err(format!("Call: expected [Str, List], got {:?}", fields)),
                },
                other => Err(format!("unknown IR term kind: {}", other)),
            }
        }
        _ => Err(format!("value_to_core_term: expected Ctor, got {:?}", v)),
    }
}

fn core_term_to_value(t: &CoreTerm) -> Value {
    match t {
        CoreTerm::IntLit(n, _)  => make_ctor("IntLit",  vec![Value::Int(*n)]),
        CoreTerm::BoolLit(b, _) => make_ctor("BoolLit", vec![Value::Bool(*b)]),
        CoreTerm::UnitLit(_)    => make_ctor("UnitLit", vec![]),
        CoreTerm::Var(name, _)  => make_ctor("Var",     vec![Value::Str(intern_str(name))]),
        CoreTerm::Lam(param, body, _) => make_ctor("Lam", vec![
            Value::Str(intern_str(param)),
            core_term_to_value(body),
        ]),
        CoreTerm::Let(name, val, body, _) => make_ctor("Let", vec![
            Value::Str(intern_str(name)),
            core_term_to_value(val),
            core_term_to_value(body),
        ]),
        CoreTerm::If(cond, then, els, _) => make_ctor("If", vec![
            core_term_to_value(cond),
            core_term_to_value(then),
            core_term_to_value(els),
        ]),
        CoreTerm::App(func, arg, _) => make_ctor("App", vec![
            core_term_to_value(func),
            core_term_to_value(arg),
        ]),
        CoreTerm::Call(target, args, _) => {
            let arg_vals: Vec<Value> = args.iter().map(core_term_to_value).collect();
            make_ctor("Call", vec![
                Value::Str(intern_str(target)),
                Value::List(arg_vals),
            ])
        }
    }
}

#[cfg(test)]
mod fold_from_spec_tests {
    use super::*;
    use std::io::Write;

    fn write_spec(name: &str, contents: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fold_spec_{}_{}.txt", std::process::id(), name
        ));
        let mut f = std::fs::File::create(&path).expect("create spec");
        f.write_all(contents.as_bytes()).expect("write spec");
        path
    }

    fn unwrap_ctor<'a>(v: &'a Value, expected_kind: &str) -> &'a [Value] {
        match v {
            Value::Ctor { tag, fields } => {
                let kind = get_tag_name(*tag);
                assert_eq!(kind, expected_kind, "expected ctor {}, got {} ({:?})", expected_kind, kind, v);
                fields.as_slice()
            }
            other => panic!("expected Ctor({}), got {:?}", expected_kind, other),
        }
    }

    fn expect_str(v: &Value) -> String {
        match v {
            Value::Str(s) => get_str(*s),
            other => panic!("expected Str, got {:?}", other),
        }
    }

    fn outer_source_call(result: Value) -> (String, Vec<Value>) {
        // ir_build_fold_from_spec returns Tuple([term, Str(effect)]).
        // The outermost Let binds `lst` to the source call.
        let tuple = match result {
            Value::Tuple(parts) => parts,
            other => panic!("expected Tuple, got {:?}", other),
        };
        let term = tuple.into_iter().next().expect("term");
        let let_fields = unwrap_ctor(&term, "Let").to_vec();
        assert_eq!(expect_str(&let_fields[0]), "lst");
        let call_fields = unwrap_ctor(&let_fields[1], "Call").to_vec();
        let target = expect_str(&call_fields[0]);
        let args = match &call_fields[1] {
            Value::List(xs) => xs.clone(),
            other => panic!("expected List args, got {:?}", other),
        };
        (target, args)
    }

    #[test]
    fn no_source_nargs_emits_unit_lit_arg() {
        // proc_args is used directly (TextList) — no list_tail wrapper.
        // Indices start at 1 to skip argv[0] (binary name).
        // outer call: proc_args([UnitLit])
        let path = write_spec("no_args", "\
effect: pure
source_fn: proc_args
transform_fn: my_t
");
        let (target, args) = outer_source_call(
            ir_build_fold_from_spec(Value::Str(intern_str(path.to_str().unwrap())))
        );
        assert_eq!(target, "proc_args");
        assert_eq!(args.len(), 1, "expected one UnitLit arg inside proc_args, got {:?}", args);
        unwrap_ctor(&args[0], "UnitLit");
    }

    #[test]
    fn two_var_args_emits_typed_call() {
        let path = write_spec("two_vars", "\
effect: reads
source_fn: str_split
source_nargs: 2
source_arg1_type: 0
source_arg1_val: content
source_arg2_type: 0
source_arg2_val: delim
transform_fn: my_t
");
        let (target, args) = outer_source_call(
            ir_build_fold_from_spec(Value::Str(intern_str(path.to_str().unwrap())))
        );
        assert_eq!(target, "str_split");
        assert_eq!(args.len(), 2);
        let var1 = unwrap_ctor(&args[0], "Var");
        assert_eq!(expect_str(&var1[0]), "content");
        let var2 = unwrap_ctor(&args[1], "Var");
        assert_eq!(expect_str(&var2[0]), "delim");
    }

    #[test]
    fn read_bundle_05_int_literal() {
        use crate::core_ir_05::{serialiser::{make_int_bundle, write_core_bundle_05_to_file}};
        let bundle = make_int_bundle(42);
        let path = std::env::temp_dir()
            .join(format!("read_bundle_05_int_{}.coreir", std::process::id()));
        write_core_bundle_05_to_file(&bundle, path.to_str().unwrap()).expect("write");
        let v = ir_read_bundle(Value::Str(intern_str(path.to_str().unwrap())));
        let lit = unwrap_ctor(&v, "IntLit");
        assert!(matches!(lit[0], Value::Int(42)));
    }

    #[test]
    fn read_bundle_05_ccall() {
        use crate::core_ir_05::{
            int_type_hash, encode_int_payload, ConstantPoolEntry, NodeRef,
            serialiser::{make_ccall_bundle, write_core_bundle_05_to_file},
        };
        let pool = vec![
            ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(7) },
            ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(35) },
        ];
        // Target identity is arbitrary — only the hex prefix gets surfaced.
        let target = [0xABu8; 32];
        let bundle = make_ccall_bundle(
            target, pool, vec![NodeRef::Pool(0), NodeRef::Pool(1)]
        );
        let path = std::env::temp_dir()
            .join(format!("read_bundle_05_ccall_{}.coreir", std::process::id()));
        write_core_bundle_05_to_file(&bundle, path.to_str().unwrap()).expect("write");
        let v = ir_read_bundle(Value::Str(intern_str(path.to_str().unwrap())));
        let call = unwrap_ctor(&v, "Call");
        // Target: "#" + first 16 hex chars of 0xAB-repeated.
        assert_eq!(expect_str(&call[0]), format!("#{}", "ab".repeat(8)));
        let args = match &call[1] {
            Value::List(xs) => xs,
            other => panic!("expected List args, got {:?}", other),
        };
        assert_eq!(args.len(), 2);
        let a0 = unwrap_ctor(&args[0], "IntLit");
        assert!(matches!(a0[0], Value::Int(7)));
        let a1 = unwrap_ctor(&args[1], "IntLit");
        assert!(matches!(a1[0], Value::Int(35)));
    }

    #[test]
    fn int_literal_source_arg() {
        let path = write_spec("int_arg", "\
effect: pure
source_fn: range
source_nargs: 1
source_arg1_type: 1
source_arg1_val: 42
transform_fn: my_t
");
        let (target, args) = outer_source_call(
            ir_build_fold_from_spec(Value::Str(intern_str(path.to_str().unwrap())))
        );
        assert_eq!(target, "range");
        assert_eq!(args.len(), 1);
        let lit = unwrap_ctor(&args[0], "IntLit");
        assert!(matches!(lit[0], Value::Int(42)));
    }

    #[test]
    fn argv_source_args_emit_argv_calls() {
        let path = write_spec("argv_args", "\
effect: full_io
source_fn: str_split
source_nargs: 2
source_arg1_type: 2
source_arg1_val: 1
source_arg2_type: 2
source_arg2_val: 2
element_var: elem
transform_fn: io_println
");
        let (target, args) = outer_source_call(
            ir_build_fold_from_spec(Value::Str(intern_str(path.to_str().unwrap())))
        );
        assert_eq!(target, "str_split");
        assert_eq!(args.len(), 2);
        // Each arg should be Call("argv", [IntLit(n)])
        let call1 = unwrap_ctor(&args[0], "Call");
        assert_eq!(expect_str(&call1[0]), "argv");
        let call1_args = match &call1[1] { Value::List(xs) => xs, other => panic!("{:?}", other) };
        assert_eq!(call1_args.len(), 1);
        let lit1 = unwrap_ctor(&call1_args[0], "IntLit");
        assert!(matches!(lit1[0], Value::Int(1)));

        let call2 = unwrap_ctor(&args[1], "Call");
        assert_eq!(expect_str(&call2[0]), "argv");
        let call2_args = match &call2[1] { Value::List(xs) => xs, other => panic!("{:?}", other) };
        assert_eq!(call2_args.len(), 1);
        let lit2 = unwrap_ctor(&call2_args[0], "IntLit");
        assert!(matches!(lit2[0], Value::Int(2)));
    }

    // Helper: unwrap Let("lst", source_call, body) and return the body.
    fn strip_lst_let(result: Value) -> Value {
        let tuple = match result { Value::Tuple(p) => p, other => panic!("{:?}", other) };
        let term = tuple.into_iter().next().expect("term");
        let let_fields = unwrap_ctor(&term, "Let").to_vec();
        assert_eq!(expect_str(&let_fields[0]), "lst");
        let_fields[2].clone()
    }

    #[test]
    fn count_if_str_len_lte_emits_indicator_and_sum_bindings() {
        let path = write_spec("count_if", "\
effect: full_io
source_fn: str_split
source_nargs: 2
source_arg1_type: 2
source_arg1_val: 1
source_arg2_type: 2
source_arg2_val: 2
mode: count_if_str_len_lte
threshold: 3
");
        let body = strip_lst_let(
            ir_build_fold_from_spec(Value::Str(intern_str(path.to_str().unwrap())))
        );

        // The body is Let("_b0", lsllis(lst, 0, 3), Let("_b1", ..., ... Let("_s30", int_add(...), println(...))))
        // Check outermost binding is _b0
        let b0_fields = unwrap_ctor(&body, "Let").to_vec();
        assert_eq!(expect_str(&b0_fields[0]), "_b0");

        // Check it calls list_str_len_lte_if_some with (lst, 0, 3)
        let lsllis_fields = unwrap_ctor(&b0_fields[1], "Call").to_vec();
        assert_eq!(expect_str(&lsllis_fields[0]), "list_str_len_lte_if_some");
        let lsllis_args = match &lsllis_fields[1] { Value::List(xs) => xs.clone(), other => panic!("{:?}", other) };
        assert_eq!(lsllis_args.len(), 3);
        // arg0: Var("lst")
        let var_lst = unwrap_ctor(&lsllis_args[0], "Var");
        assert_eq!(expect_str(&var_lst[0]), "lst");
        // arg1: IntLit(0)
        let idx_lit = unwrap_ctor(&lsllis_args[1], "IntLit");
        assert!(matches!(idx_lit[0], Value::Int(0)));
        // arg2: IntLit(3) — the threshold
        let thr_lit = unwrap_ctor(&lsllis_args[2], "IntLit");
        assert!(matches!(thr_lit[0], Value::Int(3)));

        // Walk 32 _b bindings then check the first sum binding is _s0 = int_add(_b0, _b1)
        let mut cursor = b0_fields[2].clone();
        for i in 1..32usize {
            let f = unwrap_ctor(&cursor, "Let").to_vec();
            assert_eq!(expect_str(&f[0]), format!("_b{}", i));
            cursor = f[2].clone();
        }
        // cursor is now at Let("_s0", int_add(_b0, _b1), ...)
        let s0_fields = unwrap_ctor(&cursor, "Let").to_vec();
        assert_eq!(expect_str(&s0_fields[0]), "_s0");
        let add_fields = unwrap_ctor(&s0_fields[1], "Call").to_vec();
        assert_eq!(expect_str(&add_fields[0]), "int_add");
        let add_args = match &add_fields[1] { Value::List(xs) => xs.clone(), other => panic!("{:?}", other) };
        assert_eq!(add_args.len(), 2);
        let lhs = unwrap_ctor(&add_args[0], "Var");
        assert_eq!(expect_str(&lhs[0]), "_b0");
        let rhs = unwrap_ctor(&add_args[1], "Var");
        assert_eq!(expect_str(&rhs[0]), "_b1");
    }

    #[test]
    fn source_pipe_fn_and_unwrap_wrap_arg1() {
        let path = write_spec("pipe_chain", "\
effect: full_io
source_fn: str_split
source_nargs: 2
source_arg1_type: 2
source_arg1_val: 1
source_pipe_fn: fs_read_text
source_pipe_unwrap: result_text_unwrap
source_arg2_type: 2
source_arg2_val: 2
mode: count_if_str_len_lte
threshold: 3
");
        let (target, args) = outer_source_call(
            ir_build_fold_from_spec(Value::Str(intern_str(path.to_str().unwrap())))
        );
        assert_eq!(target, "str_split");
        assert_eq!(args.len(), 2);

        // arg0 must be: result_text_unwrap(fs_read_text(argv(1)))
        let unwrap_call = unwrap_ctor(&args[0], "Call").to_vec();
        assert_eq!(expect_str(&unwrap_call[0]), "result_text_unwrap");
        let unwrap_args = match &unwrap_call[1] { Value::List(xs) => xs.clone(), other => panic!("{:?}", other) };
        assert_eq!(unwrap_args.len(), 1);

        let read_call = unwrap_ctor(&unwrap_args[0], "Call").to_vec();
        assert_eq!(expect_str(&read_call[0]), "fs_read_text");
        let read_args = match &read_call[1] { Value::List(xs) => xs.clone(), other => panic!("{:?}", other) };
        assert_eq!(read_args.len(), 1);

        let argv_call = unwrap_ctor(&read_args[0], "Call").to_vec();
        assert_eq!(expect_str(&argv_call[0]), "argv");
        let argv_args = match &argv_call[1] { Value::List(xs) => xs.clone(), other => panic!("{:?}", other) };
        let lit = unwrap_ctor(&argv_args[0], "IntLit");
        assert!(matches!(lit[0], Value::Int(1)));

        // arg1 must still be argv(2) — pipe does not affect it
        let argv2_call = unwrap_ctor(&args[1], "Call").to_vec();
        assert_eq!(expect_str(&argv2_call[0]), "argv");
        let argv2_args = match &argv2_call[1] { Value::List(xs) => xs.clone(), other => panic!("{:?}", other) };
        let lit2 = unwrap_ctor(&argv2_args[0], "IntLit");
        assert!(matches!(lit2[0], Value::Int(2)));
    }

    #[test]
    fn source_pipe_fn_alone_no_unwrap() {
        let path = write_spec("pipe_no_unwrap", "\
effect: reads
source_fn: str_split
source_nargs: 2
source_arg1_type: 2
source_arg1_val: 1
source_pipe_fn: fs_read_text
source_arg2_type: 2
source_arg2_val: 2
transform_fn: io_println
");
        let (target, args) = outer_source_call(
            ir_build_fold_from_spec(Value::Str(intern_str(path.to_str().unwrap())))
        );
        assert_eq!(target, "str_split");
        // arg0 must be fs_read_text(argv(1)) — no unwrap layer
        let read_call = unwrap_ctor(&args[0], "Call").to_vec();
        assert_eq!(expect_str(&read_call[0]), "fs_read_text");
    }

    #[test]
    fn no_source_pipe_fn_leaves_arg1_unchanged() {
        // Regression: existing specs without source_pipe_fn must pass arg1 directly.
        let path = write_spec("no_pipe", "\
effect: full_io
source_fn: str_split
source_nargs: 2
source_arg1_type: 2
source_arg1_val: 1
source_arg2_type: 2
source_arg2_val: 2
mode: count_if_str_len_lte
threshold: 3
");
        let (target, args) = outer_source_call(
            ir_build_fold_from_spec(Value::Str(intern_str(path.to_str().unwrap())))
        );
        assert_eq!(target, "str_split");
        // arg0 must be argv(1) directly — no pipe wrapping
        let argv_call = unwrap_ctor(&args[0], "Call").to_vec();
        assert_eq!(expect_str(&argv_call[0]), "argv");
    }

    #[test]
    fn list_str_len_lte_if_some_oob_returns_zero() {
        use super::super::list::list_str_len_lte_if_some;
        let list = Value::List(vec![Value::Str(intern_str("hi"))]);
        // index 5 is OOB → 0
        let result = list_str_len_lte_if_some(Value::Tuple(vec![list, Value::Int(5), Value::Int(3)]));
        assert!(matches!(result, Value::Int(0)));
    }

    #[test]
    fn list_str_len_lte_if_some_within_threshold() {
        use super::super::list::list_str_len_lte_if_some;
        let list = Value::List(vec![Value::Str(intern_str("cat"))]);
        // "cat".len() == 3 ≤ 3 → 1
        let result = list_str_len_lte_if_some(Value::Tuple(vec![list, Value::Int(0), Value::Int(3)]));
        assert!(matches!(result, Value::Int(1)));
    }

    #[test]
    fn list_str_len_lte_if_some_exceeds_threshold() {
        use super::super::list::list_str_len_lte_if_some;
        let list = Value::List(vec![Value::Str(intern_str("hello"))]);
        // "hello".len() == 5 > 3 → 0
        let result = list_str_len_lte_if_some(Value::Tuple(vec![list, Value::Int(0), Value::Int(3)]));
        assert!(matches!(result, Value::Int(0)));
    }
}

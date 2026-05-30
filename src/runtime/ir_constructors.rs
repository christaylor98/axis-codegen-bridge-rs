use crate::core_ir::{CoreTerm, Provenance, EffectClass, write_core_bundle_to_file, load_core_bundle};
use crate::runtime::value::{Value, intern_str, intern_tag, get_str, get_tag_name};
use std::rc::Rc;

fn make_ctor(tag: &str, fields: Vec<Value>) -> Value {
    Value::Ctor { tag: intern_tag(tag), fields }
}

pub fn ir_make_int_lit(v: Value) -> Value {
    match v {
        Value::Int(n) => make_ctor("IntLit", vec![Value::Int(n)]),
        _ => panic!("ir_make_int_lit: expected Int, got {:?}", v),
    }
}

pub fn ir_make_bool_lit(v: Value) -> Value {
    match v {
        Value::Bool(b) => make_ctor("BoolLit", vec![Value::Bool(b)]),
        _ => panic!("ir_make_bool_lit: expected Bool, got {:?}", v),
    }
}

pub fn ir_make_unit_lit(v: Value) -> Value {
    match v {
        Value::Unit => make_ctor("UnitLit", vec![]),
        _ => panic!("ir_make_unit_lit: expected Unit, got {:?}", v),
    }
}

pub fn ir_make_var(v: Value) -> Value {
    match v {
        Value::Str(s) => make_ctor("Var", vec![Value::Str(s)]),
        _ => panic!("ir_make_var: expected Str, got {:?}", v),
    }
}

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

pub fn ir_make_if(v: Value) -> Value {
    match v {
        Value::Tuple(fields) if fields.len() == 3 => make_ctor("If", fields),
        _ => panic!("ir_make_if: expected Tuple([cond, then, else]), got {:?}", v),
    }
}

pub fn ir_make_app(v: Value) -> Value {
    match v {
        Value::Tuple(fields) if fields.len() == 2 => make_ctor("App", fields),
        _ => panic!("ir_make_app: expected Tuple([fn, arg]), got {:?}", v),
    }
}

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

pub fn ir_term_kind(v: Value) -> Value {
    match v {
        Value::Ctor { tag, .. } => Value::Str(intern_str(&get_tag_name(tag))),
        _ => panic!("ir_term_kind: expected Ctor, got {:?}", v),
    }
}

// ── Substitution and renaming ────────────────────────────────────────────────

fn subst_value(name: &str, replacement: &Value, term: Value) -> Value {
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

pub fn ir_write_bundle(v: Value) -> Value {
    match v {
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
        _ => panic!("ir_write_bundle: expected Tuple([term, path, effectClass, idempotent]), got {:?}", v),
    }
}

pub fn ir_read_bundle(v: Value) -> Value {
    match v {
        Value::Str(s) => {
            let path = get_str(s);
            let prog = load_core_bundle(&path)
                .unwrap_or_else(|e| panic!("ir_read_bundle: {}", e));
            core_term_to_value(&prog.root_term)
        }
        _ => panic!("ir_read_bundle: expected Str path, got {:?}", v),
    }
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

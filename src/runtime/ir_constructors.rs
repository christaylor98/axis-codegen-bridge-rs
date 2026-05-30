use crate::core_ir::{CoreTerm, write_core_bundle_to_file, load_core_bundle};
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

pub fn ir_write_bundle(v: Value) -> Value {
    match v {
        Value::Tuple(mut fields) if fields.len() == 2 => {
            let path_val = fields.pop().unwrap();
            let term_val = fields.pop().unwrap();
            let path = match &path_val {
                Value::Str(s) => get_str(*s),
                _ => panic!("ir_write_bundle: expected Str path, got {:?}", path_val),
            };
            let term = value_to_core_term(&term_val)
                .unwrap_or_else(|e| panic!("ir_write_bundle: {}", e));
            write_core_bundle_to_file(&term, "bundle", &path)
                .unwrap_or_else(|e| panic!("ir_write_bundle write failed: {}", e));
            Value::Unit
        }
        _ => panic!("ir_write_bundle: expected Tuple([term, path]), got {:?}", v),
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

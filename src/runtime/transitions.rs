use super::value::{Value, intern_tag, get_tag_name};

// ── Structural substitution ───────────────────────────────────────────────────
//
// Replaces every Ctor("var", [name]) whose first field equals `name` with
// `replacement`. Recurses into Ctor fields and Tuple elements; leaves
// primitive values (Int, Bool, Str, Unit) unchanged.

fn substitute(term: Value, name: &Value, replacement: &Value) -> Value {
    match &term {
        Value::Ctor { tag, fields }
            if get_tag_name(*tag) == "var" && !fields.is_empty() && &fields[0] == name =>
        {
            replacement.clone()
        }
        Value::Ctor { tag, fields } => Value::Ctor {
            tag: *tag,
            fields: fields.iter().map(|f| substitute(f.clone(), name, replacement)).collect(),
        },
        Value::Tuple(es) => {
            Value::Tuple(es.iter().map(|e| substitute(e.clone(), name, replacement)).collect())
        }
        _ => term,
    }
}

// ── Transitions ───────────────────────────────────────────────────────────────

/// introduce_let_binding — takes Tuple(name, value_term, body_term)
/// Returns Ctor("let", [name, value_term, body_term]).
pub fn introduce_let_binding(v: Value) -> Value {
    match v {
        Value::Tuple(es) if es.len() >= 3 => Value::Ctor {
            tag: intern_tag("let"),
            fields: es,
        },
        _ => panic!("introduce_let_binding: expected Tuple(name, value_term, body_term), got {:?}", v),
    }
}

/// introduce_lambda — takes Tuple(param_name, body_term)
/// Returns Ctor("lam", [param_name, body_term]).
pub fn introduce_lambda(v: Value) -> Value {
    match v {
        Value::Tuple(es) if es.len() >= 2 => Value::Ctor {
            tag: intern_tag("lam"),
            fields: es,
        },
        _ => panic!("introduce_lambda: expected Tuple(param_name, body_term), got {:?}", v),
    }
}

/// apply_function — takes Tuple(fn_term, arg_term)
/// Returns Ctor("app", [fn_term, arg_term]).
pub fn apply_function(v: Value) -> Value {
    match v {
        Value::Tuple(es) if es.len() >= 2 => Value::Ctor {
            tag: intern_tag("app"),
            fields: es,
        },
        _ => panic!("apply_function: expected Tuple(fn_term, arg_term), got {:?}", v),
    }
}

/// extract_subterm_to_function — takes Tuple(param_name, subterm)
/// Returns Ctor("lam", [param_name, subterm]).
pub fn extract_subterm_to_function(v: Value) -> Value {
    match v {
        Value::Tuple(es) if es.len() >= 2 => Value::Ctor {
            tag: intern_tag("lam"),
            fields: es,
        },
        _ => panic!("extract_subterm_to_function: expected Tuple(param_name, subterm), got {:?}", v),
    }
}

/// inline_let_binding — takes Tuple(name, value, body)
/// Substitutes every Ctor("var",[name]) in body with value; returns the result.
pub fn inline_let_binding(v: Value) -> Value {
    match v {
        Value::Tuple(mut es) if es.len() >= 3 => {
            let body = es.remove(2);
            let value = es.remove(1);
            let name = es.remove(0);
            substitute(body, &name, &value)
        }
        _ => panic!("inline_let_binding: expected Tuple(name, value, body), got {:?}", v),
    }
}

/// rename_bound_variable — takes Tuple(old_name, new_name, lam_term)
/// If lam_term is Ctor("lam", [param, body]):
///   substitutes Ctor("var",[old_name]) → new_name throughout body,
///   then rebuilds Ctor("lam", [new_name, new_body]).
/// If lam_term is not a "lam" Ctor, returns it unchanged.
pub fn rename_bound_variable(v: Value) -> Value {
    match v {
        Value::Tuple(mut es) if es.len() >= 3 => {
            let lam_term = es.remove(2);
            let new_name = es.remove(1);
            let old_name = es.remove(0);
            if let Value::Ctor { tag, fields } = lam_term.clone() {
                if get_tag_name(tag) == "lam" && fields.len() >= 2 {
                    let body = fields[1].clone();
                    let new_body = substitute(body, &old_name, &new_name);
                    return Value::Ctor { tag, fields: vec![new_name, new_body] };
                }
            }
            lam_term
        }
        _ => panic!("rename_bound_variable: expected Tuple(old_name, new_name, lam_term), got {:?}", v),
    }
}

/// reference_registry_function — takes a name value
/// Returns Ctor("var", [name]), representing a variable reference to that name.
pub fn reference_registry_function(v: Value) -> Value {
    Value::Ctor { tag: intern_tag("var"), fields: vec![v] }
}

/// verify_foreign_reference — v0 always passes.
/// Returns Bool(true). Real verification (bridge query) comes later.
pub fn verify_foreign_reference(_v: Value) -> Value {
    Value::Bool(true)
}

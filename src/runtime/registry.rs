use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use super::value::{Value, intern_str, intern_tag, get_str};

// ── On-disk schema ───────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
struct RegistryEntry {
    qualified_name: String,
    hash: String,
    provenance: String,
    timestamp: String,
    prev_hash: Option<String>,
}

#[derive(Serialize, Deserialize, Default, Debug)]
struct RegistryStore {
    entries: Vec<RegistryEntry>,
}

// ── Path resolution ──────────────────────────────────────────────────────────

fn registry_path() -> Option<String> {
    std::env::var("AXIS_REGISTRY").ok()
}

fn load_registry() -> RegistryStore {
    let path = match registry_path() {
        None => return RegistryStore::default(),
        Some(p) => p,
    };
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => RegistryStore::default(),
    }
}

fn save_registry(store: &RegistryStore) -> Result<(), String> {
    let path = registry_path()
        .ok_or_else(|| "AXIS_REGISTRY env var not set".to_string())?;
    let content = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    std::fs::write(&path, content).map_err(|e| e.to_string())
}

// ── Hashing ──────────────────────────────────────────────────────────────────

fn compute_hash(entry: &RegistryEntry) -> String {
    let repr = format!("{}:{}:{}", entry.qualified_name, entry.provenance, entry.timestamp);
    let mut h = DefaultHasher::new();
    repr.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn current_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

// ── Entry → Value ────────────────────────────────────────────────────────────

fn entry_to_value(e: &RegistryEntry) -> Value {
    Value::Tuple(vec![
        Value::Str(intern_str(&e.qualified_name)),
        Value::Str(intern_str(&e.hash)),
        Value::Str(intern_str(&e.provenance)),
        Value::Str(intern_str(&e.timestamp)),
    ])
}

fn name_from_value(v: &Value) -> Option<String> {
    match v {
        Value::Str(h) => Some(get_str(*h)),
        Value::Unit => None,
        _ => None,
    }
}

// ── Public registry functions ────────────────────────────────────────────────

/// registry_has_entry(name: Str) → Bool
/// Returns false for unit or when name is not found.
pub fn registry_has_entry(v: Value) -> Value {
    let name = match name_from_value(&v) {
        Some(n) => n,
        None => return Value::Bool(false),
    };
    let store = load_registry();
    Value::Bool(store.entries.iter().any(|e| e.qualified_name == name))
}

/// registry_lookup(name: Str) → Ctor("Ok", entry_tuple) | Ctor("Err", reason)
pub fn registry_lookup(v: Value) -> Value {
    let name = match name_from_value(&v) {
        Some(n) => n,
        None => {
            return Value::Ctor {
                tag: intern_tag("Err"),
                fields: vec![Value::Str(intern_str("no name given"))],
            };
        }
    };
    let store = load_registry();
    match store.entries.iter().find(|e| e.qualified_name == name) {
        Some(entry) => Value::Ctor {
            tag: intern_tag("Ok"),
            fields: vec![entry_to_value(entry)],
        },
        None => Value::Ctor {
            tag: intern_tag("Err"),
            fields: vec![Value::Str(intern_str("not found"))],
        },
    }
}

/// registry_get_provenance(name: Str) → Ctor("Ok", provenance_str) | Ctor("Err", reason)
pub fn registry_get_provenance(v: Value) -> Value {
    let name = match name_from_value(&v) {
        Some(n) => n,
        None => {
            return Value::Ctor {
                tag: intern_tag("Err"),
                fields: vec![Value::Str(intern_str("no name given"))],
            };
        }
    };
    let store = load_registry();
    match store.entries.iter().find(|e| e.qualified_name == name) {
        Some(entry) => Value::Ctor {
            tag: intern_tag("Ok"),
            fields: vec![Value::Str(intern_str(&entry.provenance))],
        },
        None => Value::Ctor {
            tag: intern_tag("Err"),
            fields: vec![Value::Str(intern_str("not found"))],
        },
    }
}

/// registry_get_contract(name: Str) → Ctor("Ok", Unit) | Ctor("Err", reason)
/// v0: contract field not yet stored; returns Unit when entry exists.
pub fn registry_get_contract(v: Value) -> Value {
    let name = match name_from_value(&v) {
        Some(n) => n,
        None => {
            return Value::Ctor {
                tag: intern_tag("Err"),
                fields: vec![Value::Str(intern_str("no name given"))],
            };
        }
    };
    let store = load_registry();
    if store.entries.iter().any(|e| e.qualified_name == name) {
        Value::Ctor { tag: intern_tag("Ok"), fields: vec![Value::Unit] }
    } else {
        Value::Ctor {
            tag: intern_tag("Err"),
            fields: vec![Value::Str(intern_str("not found"))],
        }
    }
}

/// registry_get_effect_sig(name: Str) → Ctor("Ok", Unit) | Ctor("Err", reason)
/// v0: effect_sig field not yet stored; returns Unit when entry exists.
pub fn registry_get_effect_sig(v: Value) -> Value {
    let name = match name_from_value(&v) {
        Some(n) => n,
        None => {
            return Value::Ctor {
                tag: intern_tag("Err"),
                fields: vec![Value::Str(intern_str("no name given"))],
            };
        }
    };
    let store = load_registry();
    if store.entries.iter().any(|e| e.qualified_name == name) {
        Value::Ctor { tag: intern_tag("Ok"), fields: vec![Value::Unit] }
    } else {
        Value::Ctor {
            tag: intern_tag("Err"),
            fields: vec![Value::Str(intern_str("not found"))],
        }
    }
}

/// registry_all_entries(_: Unit) → List of entry tuples
pub fn registry_all_entries(_: Value) -> Value {
    let store = load_registry();
    Value::List(store.entries.iter().map(entry_to_value).collect())
}

/// registry_insert(Tuple(name, hash, provenance)) → Ctor("Ok", Unit) | Ctor("Err", reason)
/// Append-only: errors if the qualified name already exists.
pub fn registry_insert(v: Value) -> Value {
    let (qualified_name, binary_hash, provenance) = match v {
        Value::Tuple(ref es) if es.len() >= 3 => {
            match (&es[0], &es[1], &es[2]) {
                (Value::Str(nh), Value::Str(hh), Value::Str(ph)) => {
                    (get_str(*nh), get_str(*hh), get_str(*ph))
                }
                _ => {
                    return Value::Ctor {
                        tag: intern_tag("Err"),
                        fields: vec![Value::Str(intern_str("insert expects Tuple(Str, Str, Str)"))],
                    };
                }
            }
        }
        _ => {
            return Value::Ctor {
                tag: intern_tag("Err"),
                fields: vec![Value::Str(intern_str("insert expects Tuple(name, hash, provenance)"))],
            };
        }
    };

    if provenance != "Human" && provenance != "OraclePromoted" {
        return Value::Ctor {
            tag: intern_tag("Err"),
            fields: vec![Value::Str(intern_str("provenance must be Human or OraclePromoted"))],
        };
    }

    let mut store = load_registry();

    if store.entries.iter().any(|e| e.qualified_name == qualified_name) {
        return Value::Ctor {
            tag: intern_tag("Err"),
            fields: vec![Value::Str(intern_str("entry already exists (registry is append-only)"))],
        };
    }

    let prev_hash = store.entries.last().map(|e| e.hash.clone());
    let timestamp = current_timestamp();

    let mut entry = RegistryEntry {
        qualified_name,
        hash: String::new(),
        provenance,
        timestamp,
        prev_hash,
    };
    // Use provided binary_hash if non-empty; otherwise derive from content.
    entry.hash = if binary_hash.is_empty() {
        compute_hash(&entry)
    } else {
        binary_hash
    };

    store.entries.push(entry);

    match save_registry(&store) {
        Ok(()) => Value::Ctor { tag: intern_tag("Ok"), fields: vec![Value::Unit] },
        Err(e) => Value::Ctor {
            tag: intern_tag("Err"),
            fields: vec![Value::Str(intern_str(&e))],
        },
    }
}

/// registry_verify_chain(_: Unit) → Bool
/// Returns true if each entry's prev_hash matches the hash of its predecessor.
pub fn registry_verify_chain(_: Value) -> Value {
    let store = load_registry();
    let entries = &store.entries;
    if entries.is_empty() {
        return Value::Bool(true);
    }
    if entries[0].prev_hash.is_some() {
        return Value::Bool(false);
    }
    for i in 1..entries.len() {
        if entries[i].prev_hash.as_deref() != Some(&entries[i - 1].hash) {
            return Value::Bool(false);
        }
    }
    Value::Bool(true)
}

/// registry_compound_id(Tuple(module, name)) → Str
/// Returns "module.name" as a qualified identifier.
pub fn registry_compound_id(v: Value) -> Value {
    match v {
        Value::Tuple(ref es) if es.len() >= 2 => {
            match (&es[0], &es[1]) {
                (Value::Str(mh), Value::Str(nh)) => {
                    let compound = format!("{}.{}", get_str(*mh), get_str(*nh));
                    Value::Str(intern_str(&compound))
                }
                _ => Value::Str(intern_str("")),
            }
        }
        Value::Str(h) => Value::Str(h),
        _ => Value::Str(intern_str("")),
    }
}

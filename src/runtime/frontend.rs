use super::value::{Value, intern_str, get_str};
use super::registry::registry_has_entry;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn extract_two_strings(v: &Value) -> Option<(String, String)> {
    match v {
        Value::Tuple(es) if es.len() >= 2 => {
            match (&es[0], &es[1]) {
                (Value::Str(ah), Value::Str(bh)) => Some((get_str(ah), get_str(bh))),
                _ => None,
            }
        }
        _ => None,
    }
}

fn extract_three_strings(v: &Value) -> Option<(String, String, String)> {
    match v {
        Value::Tuple(es) if es.len() >= 3 => {
            match (&es[0], &es[1], &es[2]) {
                (Value::Str(ah), Value::Str(bh), Value::Str(ch)) => {
                    Some((get_str(ah), get_str(bh), get_str(ch)))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Read types_path (pipe-delimited) and return the primary shape name for
/// artifact_type, or None if not found.
fn lookup_shape_in_content(content: &str, artifact_type: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, '|');
        let field0 = parts.next().unwrap_or("").trim();
        let field1 = parts.next().unwrap_or("").trim();
        if field0 == artifact_type {
            return Some(field1.to_string());
        }
    }
    None
}

// ── Public bridge functions ───────────────────────────────────────────────────

/// frontend_lookup_shape(Tuple(Str types_path, Str artifact_type)) → Str
///
/// Reads `types_path` (pipe-delimited rows: `artifact_type|primary_shape`),
/// returns the primary shape name for `artifact_type`, or "" if not found.
pub fn frontend_lookup_shape(v: Value) -> Value {
    let (types_path, artifact_type) = match extract_two_strings(&v) {
        Some(pair) => pair,
        None => return Value::Str(intern_str("")),
    };

    let content = match std::fs::read_to_string(&types_path) {
        Ok(c) => c,
        Err(_) => return Value::Str(intern_str("")),
    };

    match lookup_shape_in_content(&content, &artifact_type) {
        Some(shape) => Value::Str(intern_str(&shape)),
        None => Value::Str(intern_str("")),
    }
}

/// frontend_walk(Tuple(Str shapes_path, Str types_path, Str artifact_type)) → Str
///
/// Resolves all holes for the given artifact_type using the shapes and types
/// files.  Returns a newline-separated WalkResult ending with DONE.
pub fn frontend_walk(v: Value) -> Value {
    let (shapes_path, types_path, artifact_type) = match extract_three_strings(&v) {
        Some(triple) => triple,
        None => return Value::Str(intern_str("WALL|1\nDONE")),
    };

    // Step 1: resolve shape name from types file
    let types_content = match std::fs::read_to_string(&types_path) {
        Ok(c) => c,
        Err(_) => return Value::Str(intern_str("WALL|1\nDONE")),
    };

    let shape_name = match lookup_shape_in_content(&types_content, &artifact_type) {
        Some(s) => s,
        None => return Value::Str(intern_str("WALL|1\nDONE")),
    };

    // Step 2: read shapes file
    let shapes_content = match std::fs::read_to_string(&shapes_path) {
        Ok(c) => c,
        Err(_) => return Value::Str(intern_str("WALL|1\nDONE")),
    };

    // Step 3: filter rows where field[0] == shape_name
    // Row format: shape_name|hole_id|type_sig|status_hint|detail
    let matching_rows: Vec<Vec<&str>> = shapes_content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let parts: Vec<&str> = line.splitn(5, '|').collect();
            if parts.len() >= 5 && parts[0].trim() == shape_name.as_str() {
                Some(parts)
            } else {
                None
            }
        })
        .collect();

    if matching_rows.is_empty() {
        return Value::Str(intern_str("WALL|1\nDONE"));
    }

    // Step 4: classify each hole
    let mut lines: Vec<String> = Vec::new();
    let mut resolved_count = 0usize;

    for row in &matching_rows {
        let hole_id     = row[1].trim();
        let type_sig    = row[2].trim();
        let status_hint = row[3].trim();
        let detail      = row[4].trim();

        match status_hint {
            "REGISTRY_CHECK" => {
                // Split detail on '|': first token is the registry key; remaining
                // tokens (columns 6+) are slot metadata passed through to output.
                let registry_key = detail.split('|').next().unwrap_or(detail);
                let check_result = registry_has_entry(Value::Str(intern_str(registry_key)));
                match check_result {
                    Value::Bool(true) => {
                        resolved_count += 1;
                        lines.push(format!("RESOLVED|{}|{}|{}", hole_id, type_sig, detail));
                    }
                    _ => {
                        let extends_closure = type_sig.contains("->");
                        let metadata = if detail.len() > registry_key.len() {
                            &detail[registry_key.len()..]  // starts with "|"
                        } else {
                            ""
                        };
                        lines.push(format!(
                            "UNKNOWN|{}|{}|{} not in registry|extends_closure={}{}",
                            hole_id, type_sig, registry_key, extends_closure, metadata
                        ));
                    }
                }
            }
            "NEED" => {
                lines.push(format!("NEED|{}|{}|{}", hole_id, type_sig, detail));
            }
            "UNKNOWN" => {
                let extends_closure = type_sig.contains("->");
                lines.push(format!(
                    "UNKNOWN|{}|{}|{}|extends_closure={}",
                    hole_id, type_sig, detail, extends_closure
                ));
            }
            _ => {
                lines.push(format!(
                    "UNKNOWN|{}|{}|{}|extends_closure=false",
                    hole_id, type_sig, detail
                ));
            }
        }
    }

    // Step 5: prepend WALL|1 if no resolved lines
    let mut output = String::new();
    if resolved_count == 0 {
        output.push_str("WALL|1\n");
    }
    output.push_str(&lines.join("\n"));
    output.push_str("\nDONE");

    Value::Str(intern_str(&output))
}

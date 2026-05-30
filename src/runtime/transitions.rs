use super::value::Value;

pub fn introduce_let_binding(v: Value) -> Value { v }
pub fn introduce_lambda(v: Value) -> Value { v }
pub fn apply_function(v: Value) -> Value { v }
pub fn extract_subterm_to_function(v: Value) -> Value { v }
pub fn inline_let_binding(v: Value) -> Value { v }
pub fn rename_bound_variable(v: Value) -> Value { v }
pub fn reference_registry_function(v: Value) -> Value { v }
pub fn verify_foreign_reference(v: Value) -> Value { v }

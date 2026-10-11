use std::fs;

use super::{ConstantPoolEntry, CoreBundle, Node, NodeRef};

pub fn create_core_bundle_05(bundle: &CoreBundle) -> Vec<u8> {
    super::serialize_axbi(bundle)
}

pub fn write_core_bundle_05_to_file(bundle: &CoreBundle, path: &str) -> Result<(), String> {
    let bytes = super::serialize_axbi(bundle);
    fs::write(path, bytes).map_err(|e| format!("write failed: {}", e))
}

// ── Convenience builders for test fixtures ───────────────────────────────────

pub fn make_unit_bundle() -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![ConstantPoolEntry {
            def_hash: super::unit_type_hash(),
            payload: vec![],
        }],
        nodes: vec![],
        result: NodeRef::Pool(0),
    }
}

pub fn make_bool_bundle(v: bool) -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![ConstantPoolEntry {
            def_hash: super::bool_type_hash(),
            payload: super::encode_bool_payload(v),
        }],
        nodes: vec![],
        result: NodeRef::Pool(0),
    }
}

pub fn make_int_bundle(v: i64) -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![ConstantPoolEntry {
            def_hash: super::int_type_hash(),
            payload: super::encode_int_payload(v),
        }],
        nodes: vec![],
        result: NodeRef::Pool(0),
    }
}

pub fn make_ccall_bundle(
    target_identity: [u8; 32],
    pool: Vec<ConstantPoolEntry>,
    args: Vec<NodeRef>,
) -> CoreBundle {
    make_ccall_bundle_named(target_identity, "", pool, args)
}

/// Same as [`make_ccall_bundle`], but sets `target_name` explicitly. Builtin
/// resolution keys `native_call_fn_arg_types`/`fn_arg_kinds` lookups off
/// `target_name` (not the resolved symbol path), so a CCall targeting a
/// builtin that has opted into the native calling convention (e.g.
/// `int_add`) MUST carry its real name here or codegen falls back to the
/// stale boxed `Value::Tuple` convention and the generated Rust fails to
/// compile against the native signature.
pub fn make_ccall_bundle_named(
    target_identity: [u8; 32],
    target_name: &str,
    pool: Vec<ConstantPoolEntry>,
    args: Vec<NodeRef>,
) -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: pool,
        nodes: vec![Node::CCall { target_identity, args, target_name: target_name.to_string() }],
        result: NodeRef::Node(0),
    }
}

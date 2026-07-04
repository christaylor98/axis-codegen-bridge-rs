//! BRIDGE_VALUE_COERCION_V1 acceptance.
//!
//! Two emit-time checks confirming the dispatchers wire up correctly:
//!   * `bridge_to_dec(x, int_to_dec, dec_id, float_to_dec)` lowers to a
//!     native four-arg Rust call with the three converter paths inlined.
//!   * Same for `bridge_to_float`.
//!
//! Runtime behaviour for the eight leaves is covered by the unit-test module
//! in `src/runtime/coerce.rs`.

use axis_codegen_bridge::core_ir_05::{
    encode_int_payload, fn_type_hash, int_type_hash, sha256_bytes,
    ConstantPoolEntry, CoreBundle, Node, NodeRef,
};
use axis_codegen_bridge::emit::rust_05::emit_rust_lib_from_bundle;
use std::collections::HashMap;

fn fn_pool(name: &str) -> ConstantPoolEntry {
    ConstantPoolEntry {
        def_hash: fn_type_hash(),
        payload: sha256_bytes(name.as_bytes()).to_vec(),
    }
}

#[test]
fn bridge_to_dec_emits_native_four_arg_call() {
    let bundle = CoreBundle {
        version: "0.5".into(),
        constant_pool: vec![
            // pool[0]: Int placeholder for the Value input.
            ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(0) },
            // pool[1..3]: three Fn-typed converter refs in Int/Dec/Float order.
            fn_pool("int_to_dec"),
            fn_pool("dec_id"),
            fn_pool("float_to_dec"),
        ],
        nodes: vec![Node::CCall {
            target_identity: sha256_bytes(b"bridge_to_dec"),
            target_name: "bridge_to_dec".into(),
            args: vec![
                NodeRef::Pool(0),
                NodeRef::Pool(1),
                NodeRef::Pool(2),
                NodeRef::Pool(3),
            ],
        }],
        result: NodeRef::Node(0),
    };

    let src = emit_rust_lib_from_bundle(&bundle, "to_dec_smoke", &HashMap::new(), &HashMap::new())
        .expect("emit should succeed for a well-typed bridge_to_dec call");

    // Fn-typed pool entries are NOT bound as `let pool_N`.
    for i in [1, 2, 3] {
        assert!(
            !src.contains(&format!("let pool_{}", i)),
            "pool[{}] is Fn-typed; must not be bound as `let pool_N`. src:\n{}",
            i, src
        );
    }
    // The CCall emits as a native four-arg call.
    let expected =
        "axis_codegen_bridge::runtime::coerce::bridge_to_dec(pool_0.clone(), \
         axis_codegen_bridge::runtime::coerce::int_to_dec, \
         axis_codegen_bridge::runtime::coerce::dec_id, \
         axis_codegen_bridge::runtime::coerce::float_to_dec)";
    assert!(
        src.contains(expected),
        "expected native four-arg call, got src:\n{}",
        src
    );
}

#[test]
fn bridge_to_float_emits_native_four_arg_call() {
    let bundle = CoreBundle {
        version: "0.5".into(),
        constant_pool: vec![
            ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(0) },
            fn_pool("int_to_float"),
            fn_pool("dec_to_float"),
            fn_pool("float_id"),
        ],
        nodes: vec![Node::CCall {
            target_identity: sha256_bytes(b"bridge_to_float"),
            target_name: "bridge_to_float".into(),
            args: vec![
                NodeRef::Pool(0),
                NodeRef::Pool(1),
                NodeRef::Pool(2),
                NodeRef::Pool(3),
            ],
        }],
        result: NodeRef::Node(0),
    };

    let src = emit_rust_lib_from_bundle(&bundle, "to_float_smoke", &HashMap::new(), &HashMap::new())
        .expect("emit should succeed for a well-typed bridge_to_float call");

    let expected =
        "axis_codegen_bridge::runtime::coerce::bridge_to_float(pool_0.clone(), \
         axis_codegen_bridge::runtime::coerce::int_to_float, \
         axis_codegen_bridge::runtime::coerce::dec_to_float, \
         axis_codegen_bridge::runtime::coerce::float_id)";
    assert!(
        src.contains(expected),
        "expected native four-arg call, got src:\n{}",
        src
    );
}

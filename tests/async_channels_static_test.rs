//! Emit-time enforcement of CHANNELS_STATIC for BRIDGE_ASYNC_PRIMITIVES_V1.
//!
//! A `channel_send` whose name is a compile-time-literal Text must target a
//! channel declared in the registry (`channel <name> … end`). An undeclared name
//! is a HARD ERROR at emit time — never a silent no-op. This mirrors the
//! emit-time hard-error discipline the bridge already uses for Fn-as-data.

use axis_codegen_bridge::core_ir_05::{
    encode_int_payload, encode_text_payload, int_type_hash, sha256_bytes, text_type_hash,
    ConstantPoolEntry, CoreBundle, Node, NodeRef,
};
use axis_codegen_bridge::emit::rust_05::emit_rust_lib_from_bundle;
use std::collections::{HashMap, HashSet};

/// A bundle whose single node is `channel_send(<name>, 7)`.
fn channel_send_bundle(name: &str) -> CoreBundle {
    CoreBundle {
        version: "0.5".into(),
        constant_pool: vec![
            // pool[0]: the channel name (Text literal).
            ConstantPoolEntry { def_hash: text_type_hash(), payload: encode_text_payload(name) },
            // pool[1]: the payload (Int).
            ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(7) },
        ],
        nodes: vec![Node::CCall {
            target_identity: sha256_bytes(b"channel_send"),
            target_name: "channel_send".into(),
            args: vec![NodeRef::Pool(0), NodeRef::Pool(1)],
        }],
        result: NodeRef::Node(0),
    }
}

#[test]
fn channel_send_to_undeclared_name_is_emit_time_hard_error() {
    let bundle = channel_send_bundle("ghost");
    // Declared set contains only "a2b" — "ghost" is undeclared.
    let declared: HashSet<String> = ["a2b".to_string()].into_iter().collect();
    let err = emit_rust_lib_from_bundle(
        &bundle,
        "undeclared_send",
        &HashMap::new(),
        &HashMap::new(),
        &declared,
    )
    .expect_err("channel_send to an undeclared channel must be rejected at emit time");
    assert!(
        err.contains("CHANNELS_STATIC") && err.contains("ghost"),
        "expected CHANNELS_STATIC rejection naming the channel, got: {}",
        err
    );
}

#[test]
fn channel_send_to_empty_declared_set_is_rejected() {
    // No channels declared at all → any literal channel_send is undeclared.
    let bundle = channel_send_bundle("a2b");
    let err = emit_rust_lib_from_bundle(
        &bundle,
        "no_decls",
        &HashMap::new(),
        &HashMap::new(),
        &HashSet::new(),
    )
    .expect_err("with no declared channels, a literal channel_send must be rejected");
    assert!(err.contains("CHANNELS_STATIC"), "expected CHANNELS_STATIC, got: {}", err);
}

#[test]
fn channel_send_to_declared_name_emits() {
    let bundle = channel_send_bundle("a2b");
    let declared: HashSet<String> = ["a2b".to_string(), "b2a".to_string()].into_iter().collect();
    let src = emit_rust_lib_from_bundle(
        &bundle,
        "declared_send",
        &HashMap::new(),
        &HashMap::new(),
        &declared,
    )
    .expect("channel_send to a declared channel must emit cleanly");
    assert!(
        src.contains("channels::channel_send"),
        "expected the emitted source to call the channel_send bridge path"
    );
}

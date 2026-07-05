//! End-to-end build/link smoke test for BRIDGE_ASYNC_PRIMITIVES_V1.
//!
//! A single bundle that calls all three primitives — `event_subscribe`,
//! `channel_send`, `wait` — must compile and link via `cmd_build` into a .a
//! archive exporting the fn symbol. The channel `a2b` is declared in a temp
//! registry so the emit-time CHANNELS_STATIC gate is satisfied.

use axis_codegen_bridge::core_ir_05::{
    encode_int_payload, encode_text_payload, fn_type_hash, int_type_hash, sha256_bytes,
    text_type_hash, ConstantPoolEntry, CoreBundle, Node, NodeRef,
};
use axis_codegen_bridge::core_ir_05::serialiser::create_core_bundle_05;
use std::process::Command;
use tempfile::TempDir;

fn bridge() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_axis-codegen-bridge"))
}

#[test]
fn build_links_all_three_primitives() {
    let dir = TempDir::new().unwrap();

    // Registry declaring the one channel this program uses.
    let reg = dir.path().join("async.axreg");
    std::fs::write(&reg, "registry async 0.1\n\nchannel a2b\n  type Value\nend\n").unwrap();

    // pool[0]=Text "a2b", pool[1]=Int 7, pool[2]=Fn ref → unit_id (a bridge
    // fn(Value)->Value used as the wait handler in this smoke test).
    let bundle = CoreBundle {
        version: "0.5".into(),
        constant_pool: vec![
            ConstantPoolEntry { def_hash: text_type_hash(), payload: encode_text_payload("a2b") },
            ConstantPoolEntry { def_hash: int_type_hash(),  payload: encode_int_payload(7) },
            ConstantPoolEntry { def_hash: fn_type_hash(),   payload: sha256_bytes(b"unit_id").to_vec() },
        ],
        nodes: vec![
            Node::CCall {
                target_identity: sha256_bytes(b"event_subscribe"),
                target_name: "event_subscribe".into(),
                args: vec![NodeRef::Pool(0)],
            },
            Node::CCall {
                target_identity: sha256_bytes(b"channel_send"),
                target_name: "channel_send".into(),
                args: vec![NodeRef::Pool(0), NodeRef::Pool(1)],
            },
            Node::CCall {
                target_identity: sha256_bytes(b"wait"),
                target_name: "wait".into(),
                args: vec![NodeRef::Pool(2)],
            },
        ],
        result: NodeRef::Node(2),
    };

    let fixture = dir.path().join("async_prog.coreir");
    std::fs::write(&fixture, create_core_bundle_05(&bundle)).unwrap();
    let out = dir.path().join("async_prog");

    let output = Command::new(bridge())
        .args([
            "build",
            fixture.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--reg",
            reg.to_str().unwrap(),
        ])
        .output()
        .expect("bridge failed to run");

    assert!(
        output.status.success(),
        "build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let lib = dir.path().join("libasync_prog.a");
    assert!(lib.exists(), "archive not produced at {:?}", lib);

    let nm = Command::new("nm").arg(&lib).output().expect("nm failed");
    let syms = String::from_utf8_lossy(&nm.stdout);
    assert!(syms.contains("async_prog"), "fn symbol not in archive:\n{}", syms);
}

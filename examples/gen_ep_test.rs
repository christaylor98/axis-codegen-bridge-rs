/// Generate 0.5 .coreir fixtures for manual BRIDGE_ENTRY_POINTS_V1 CLI testing.
/// Run: cargo run --example gen_ep_test
/// Output files land in /tmp/ep_test/

use axis_codegen_bridge::core_ir_05::{
    serialiser::create_core_bundle_05,
    ConstantPoolEntry, CoreBundle, Node, NodeRef,
    int_type_hash, unit_type_hash, sha256_bytes,
    encode_int_payload,
};
use std::fs;

fn main() {
    let out = std::path::Path::new("/tmp/ep_test");
    fs::create_dir_all(out).unwrap();

    // loop_a.coreir — returns Int(42) regardless of args
    let loop_a = CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![ConstantPoolEntry {
            def_hash: int_type_hash(),
            payload: encode_int_payload(42),
        }],
        nodes: vec![],
    };
    fs::write(out.join("loop_a.coreir"), create_core_bundle_05(&loop_a)).unwrap();
    println!("wrote loop_a.coreir  (returns 42)");

    // loop_b.coreir — returns Int(99) regardless of args
    let loop_b = CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![ConstantPoolEntry {
            def_hash: int_type_hash(),
            payload: encode_int_payload(99),
        }],
        nodes: vec![],
    };
    fs::write(out.join("loop_b.coreir"), create_core_bundle_05(&loop_b)).unwrap();
    println!("wrote loop_b.coreir  (returns 99)");

    // panicky.coreir — calls option_unwrap(Unit) → panics at runtime
    let panicky = CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![ConstantPoolEntry {
            def_hash: unit_type_hash(),
            payload: vec![],
        }],
        nodes: vec![Node::CCall {
            target_identity: sha256_bytes(b"option_unwrap"),
            target_name: "option_unwrap".to_string(),
            args: vec![NodeRef::Pool(0)],
        }],
    };
    fs::write(out.join("panicky.coreir"), create_core_bundle_05(&panicky)).unwrap();
    println!("wrote panicky.coreir (panics via option_unwrap(Unit))");

    // root.coreir — trivial unit; used as the --out root when entries drive everything
    let root = CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![ConstantPoolEntry {
            def_hash: unit_type_hash(),
            payload: vec![],
        }],
        nodes: vec![],
    };
    fs::write(out.join("root.coreir"), create_core_bundle_05(&root)).unwrap();
    println!("wrote root.coreir    (unit; root bundle placeholder)");

    // argv.axreg — registry entry that gives `argv` an in (TextList) contract
    // so it can be used as a foreign-fn entry point
    let argv_id = sha256_bytes(b"argv");
    let argv_id_hex: String = argv_id.iter().map(|b| format!("{:02x}", b)).collect();
    let reg_content = format!(
        "fn argv\n  identity 0x{}\n  kind     leaf\n  in       (TextList)\n  out      TextList\n  effect   reads\n  deterministic false\n  idempotent    false\nend\n",
        argv_id_hex
    );
    fs::write(out.join("argv.axreg"), reg_content).unwrap();
    println!("wrote argv.axreg     (argv with in (TextList) for foreign-entry test)");

    println!("\nFixtures ready in /tmp/ep_test/");
    println!("Run: cargo build -q && BRIDGE=./target/debug/axis-codegen-bridge");
}

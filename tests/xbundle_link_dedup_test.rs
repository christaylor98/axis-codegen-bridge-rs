//! Regression test for BRIDGE_XBUNDLE_LINK_DEDUP.
//!
//! Cross-bundle --exe is the normal build path. This test exercises the
//! N-bundle case (root + multiple §5b providers) and asserts:
//!   1. The build succeeds with no `duplicate symbol` linker error in stderr.
//!   2. The produced exe actually runs and prints the expected value.
//!
//! The structural invariant (SINGLE_DEFINITION_OF_DROP_GLUE) is that shared
//! upstream monomorphizations from `axis_codegen_bridge` (drop_in_place<Value>
//! etc.) exist exactly once at final link — achieved by giving each downstream
//! rlib a unique crate name and linking them as proper `--extern` crate deps,
//! NOT by `-Wl,--allow-multiple-definition` / `-z muldefs`.

use axis_codegen_bridge::core_ir_05::{
    serialiser::create_core_bundle_05, bool_type_hash, encode_bool_payload, sha256_bytes,
    ConstantPoolEntry, CoreBundle, Node, NodeRef,
};
use std::process::Command;
use tempfile::TempDir;

fn bridge() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_axis-codegen-bridge"))
}

fn write_bundle(dir: &TempDir, name: &str, bundle: &CoreBundle) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, create_core_bundle_05(bundle)).unwrap();
    path
}

/// `prov_a`: returns `bool_not(false) = true`. A pure §5b leaf.
fn make_prov_a() -> CoreBundle {
    CoreBundle {
        version: "0.5".into(),
        constant_pool: vec![ConstantPoolEntry {
            def_hash: bool_type_hash(),
            payload: encode_bool_payload(false),
        }],
        nodes: vec![Node::CCall {
            target_identity: sha256_bytes(b"bool_not"),
            args: vec![NodeRef::Pool(0)],
            target_name: "bool_not".into(),
        }],
    }
}

/// `prov_b`: calls `prov_a()` and then `bool_not` on the result =>
/// `bool_not(bool_not(false)) = false`. Forces a 3-bundle closure (caller →
/// prov_b → prov_a).
fn make_prov_b() -> CoreBundle {
    CoreBundle {
        version: "0.5".into(),
        constant_pool: vec![],
        nodes: vec![
            // node[0]: prov_a() — §5b call
            Node::CCall {
                target_identity: sha256_bytes(b"prov_a"),
                args: vec![],
                target_name: "prov_a".into(),
            },
            // node[1]: bool_not(node[0])
            Node::CCall {
                target_identity: sha256_bytes(b"bool_not"),
                args: vec![NodeRef::Node(0)],
                target_name: "bool_not".into(),
            },
        ],
    }
}

/// `caller`: `prov_b()`. Root of the 3-bundle closure.
fn make_caller() -> CoreBundle {
    CoreBundle {
        version: "0.5".into(),
        constant_pool: vec![],
        nodes: vec![Node::CCall {
            target_identity: sha256_bytes(b"prov_b"),
            args: vec![],
            target_name: "prov_b".into(),
        }],
    }
}

#[test]
fn xbundle_three_bundle_exe_links_dedups_and_runs() {
    let dir = TempDir::new().unwrap();

    let a_coreir = write_bundle(&dir, "prov_a.coreir", &make_prov_a());
    let b_coreir = write_bundle(&dir, "prov_b.coreir", &make_prov_b());
    let c_coreir = write_bundle(&dir, "caller.coreir", &make_caller());
    let exe_out  = dir.path().join("caller_exe");

    let output = Command::new(bridge())
        .args([
            "build", c_coreir.to_str().unwrap(),
            "--out",  exe_out.to_str().unwrap(),
            "--lib",  a_coreir.to_str().unwrap(),
            "--lib",  b_coreir.to_str().unwrap(),
            "--exe",
        ])
        .output()
        .expect("bridge failed to run");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Structural invariant: zero duplicate-symbol errors from the linker.
    // If this fires, the fix has regressed — we are back to N-archive
    // dedup-defeating linking.
    assert!(
        !stderr.contains("duplicate symbol"),
        "linker emitted 'duplicate symbol' — fix regressed to N-archive linking:\n{}",
        stderr
    );

    // And no flag-based mask: --allow-multiple-definition / -z muldefs are
    // explicitly forbidden by the intent (BRIDGE_XBUNDLE_LINK_DEDUP).
    assert!(
        !stderr.contains("allow-multiple-definition"),
        "build used --allow-multiple-definition (forbidden):\n{}",
        stderr
    );
    assert!(
        !stderr.contains("muldefs"),
        "build used -z muldefs (forbidden):\n{}",
        stderr
    );

    assert!(
        output.status.success(),
        "3-bundle --exe build failed; stderr:\n{}",
        stderr
    );
    assert!(exe_out.exists(), "exe not produced at {:?}", exe_out);

    let run = Command::new(&exe_out).output().expect("failed to run exe");
    let stdout = String::from_utf8_lossy(&run.stdout);
    // bool_not(prov_a()) = bool_not(true) = false
    assert_eq!(
        stdout.trim(),
        "false",
        "expected 'false', got: {:?}\nstderr: {:?}",
        stdout,
        String::from_utf8_lossy(&run.stderr)
    );
}

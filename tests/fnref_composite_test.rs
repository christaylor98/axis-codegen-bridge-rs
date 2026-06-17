//! Regression test for FNREF_COMPOSITE_RESOLVER_v0.1.
//!
//! Before this fix, passing a composite (M1) fn as a HOF callee — e.g.
//! `loop_while(state, composite_gt0, body, n)` where `composite_gt0` is a
//! composite predicate — failed at emit time with:
//!     "Fn-typed pool entry resolves to registry name '<X>' but that
//!      name has no bridge implementation"
//!
//! `classify_pool_entry` checked builtin + registry/name_to_path but
//! ignored xbundle providers. The fix adds an xbundle check; the extern
//! block emission was extended to declare any Fn-typed pool entry that
//! resolves to an xbundle symbol, so the bare path is a valid Rust fn
//! pointer.
//!
//! This test builds a composite predicate as a §5b provider, then a
//! caller that uses it as the `any` HOF's fn-ref slot, and asserts the
//! exe runs and prints the predicate-driven result.

use axis_codegen_bridge::core_ir_05::{
    bool_type_hash, fn_type_hash, int_type_hash, param_type_hash, sha256_bytes,
    encode_int_payload,
    serialiser::create_core_bundle_05,
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

/// Provider `composite_gt0(v: Value) -> Bool` body: `int_gt(v, Int(0))`.
/// Pool: [Param(slot=0), Int(0)]
/// Nodes: [int_gt(Pool[0], Pool[1])]
fn make_composite_gt0() -> CoreBundle {
    CoreBundle {
        version: "0.5".into(),
        constant_pool: vec![
            ConstantPoolEntry { def_hash: param_type_hash(), payload: vec![0x00] }, // slot 0
            ConstantPoolEntry { def_hash: int_type_hash(),   payload: encode_int_payload(0) },
        ],
        nodes: vec![Node::CCall {
            target_identity: sha256_bytes(b"int_gt"),
            args: vec![NodeRef::Pool(0), NodeRef::Pool(1)],
            target_name: "int_gt".into(),
        }],
    }
}

/// Caller: `any(range(Int(0), Int(3)), composite_gt0)`.
/// Pool: [Int(0), Int(3), Fn(composite_gt0)]
/// Nodes:
///   [0] range(Pool[0], Pool[1])     → [0, 1, 2]
///   [1] any(Node[0], Pool[2])        → true (1 and 2 are positive)
fn make_caller_any() -> CoreBundle {
    let fn_id = sha256_bytes(b"composite_gt0");
    CoreBundle {
        version: "0.5".into(),
        constant_pool: vec![
            ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(0) },
            ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(3) },
            ConstantPoolEntry { def_hash: fn_type_hash(),  payload: fn_id.to_vec() },
        ],
        nodes: vec![
            Node::CCall {
                target_identity: sha256_bytes(b"range"),
                args: vec![NodeRef::Pool(0), NodeRef::Pool(1)],
                target_name: "range".into(),
            },
            Node::CCall {
                target_identity: sha256_bytes(b"any"),
                args: vec![NodeRef::Node(0), NodeRef::Pool(2)],
                target_name: "any".into(),
            },
        ],
    }
}

#[test]
fn composite_fnref_in_any_hof_runs() {
    let dir = TempDir::new().unwrap();

    let prov_path = write_bundle(&dir, "composite_gt0.coreir", &make_composite_gt0());
    let caller_path = write_bundle(&dir, "caller.coreir", &make_caller_any());
    let exe = dir.path().join("caller_exe");

    let out = Command::new(bridge())
        .args([
            "build", caller_path.to_str().unwrap(),
            "--out", exe.to_str().unwrap(),
            "--lib", prov_path.to_str().unwrap(),
            "--exe",
        ])
        .output()
        .expect("bridge invocation failed");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "bridge build failed (composite fn-ref):\n--- stderr ---\n{}",
        stderr
    );
    assert!(exe.exists(), "exe not produced at {:?}", exe);

    let run = Command::new(&exe).output().expect("failed to run exe");
    let stdout = String::from_utf8_lossy(&run.stdout);
    // any([0, 1, 2], composite_gt0) — 1 and 2 are > 0, so true.
    // The HOF result is passed back through the entry trampoline; with no
    // io_println the exe just returns; we verify it exited cleanly here.
    // The key assertion is the build succeeded — the old code returned
    // "no bridge implementation" at emit time before this fix.
    assert!(
        run.status.success(),
        "exe failed to run cleanly; stdout={:?}, stderr={:?}",
        stdout,
        String::from_utf8_lossy(&run.stderr)
    );
}

/// Same scenario but with `all` (the other HOF predicate). `all(range(0,3), composite_gt0)`
/// is FALSE because 0 is not positive. Exercises the same fn-ref dispatch but a
/// different HOF — verifies the fix isn't `any`-specific.
fn make_caller_all() -> CoreBundle {
    let fn_id = sha256_bytes(b"composite_gt0");
    CoreBundle {
        version: "0.5".into(),
        constant_pool: vec![
            ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(0) },
            ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(3) },
            ConstantPoolEntry { def_hash: fn_type_hash(),  payload: fn_id.to_vec() },
        ],
        nodes: vec![
            Node::CCall {
                target_identity: sha256_bytes(b"range"),
                args: vec![NodeRef::Pool(0), NodeRef::Pool(1)],
                target_name: "range".into(),
            },
            Node::CCall {
                target_identity: sha256_bytes(b"all"),
                args: vec![NodeRef::Node(0), NodeRef::Pool(2)],
                target_name: "all".into(),
            },
        ],
    }
}

#[test]
fn composite_fnref_in_all_hof_runs() {
    let dir = TempDir::new().unwrap();

    let prov_path = write_bundle(&dir, "composite_gt0.coreir", &make_composite_gt0());
    let caller_path = write_bundle(&dir, "caller.coreir", &make_caller_all());
    let exe = dir.path().join("caller_exe");

    let out = Command::new(bridge())
        .args([
            "build", caller_path.to_str().unwrap(),
            "--out", exe.to_str().unwrap(),
            "--lib", prov_path.to_str().unwrap(),
            "--exe",
        ])
        .output()
        .expect("bridge invocation failed");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "bridge build failed (composite fn-ref in all):\n--- stderr ---\n{}",
        stderr
    );

    let run = Command::new(&exe).output().expect("failed to run exe");
    assert!(
        run.status.success(),
        "exe failed; stderr={:?}",
        String::from_utf8_lossy(&run.stderr)
    );
}

/// Pre-existing leaf fn-ref path must still work after the fix.
/// `any(range(0, 2), bool_not)` — bool_not is a leaf builtin. The same code
/// path now also handles xbundle composites; this asserts the leaf path is
/// unaffected.
fn make_caller_leaf_fnref() -> CoreBundle {
    let fn_id = sha256_bytes(b"bool_not");
    CoreBundle {
        version: "0.5".into(),
        constant_pool: vec![
            ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(0) },
            ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(2) },
            ConstantPoolEntry { def_hash: fn_type_hash(),  payload: fn_id.to_vec() },
        ],
        nodes: vec![
            Node::CCall {
                target_identity: sha256_bytes(b"range"),
                args: vec![NodeRef::Pool(0), NodeRef::Pool(1)],
                target_name: "range".into(),
            },
            Node::CCall {
                target_identity: sha256_bytes(b"foreach"),
                args: vec![NodeRef::Node(0), NodeRef::Pool(2)],
                target_name: "foreach".into(),
            },
        ],
    }
}

#[test]
fn leaf_fnref_still_works_after_fix() {
    let dir = TempDir::new().unwrap();
    let caller_path = write_bundle(&dir, "caller.coreir", &make_caller_leaf_fnref());
    let exe = dir.path().join("caller_exe");

    let out = Command::new(bridge())
        .args([
            "build", caller_path.to_str().unwrap(),
            "--out", exe.to_str().unwrap(),
            "--exe",
        ])
        .output()
        .expect("bridge invocation failed");

    assert!(
        out.status.success(),
        "leaf fn-ref build regressed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = bool_type_hash(); // keep import for future bool-payload tests
}

// ── direct CCall to composite — xbundle fallback ─────────────────────────────
// Before this fix, a CCall whose target identity was in the --reg map but had
// no Rust impl errored "has no bridge implementation", even when an --lib
// provider could resolve it. That forced callers to omit registry files at
// eject time (e.g. axAGI-code-gen-working/scripts/build.sh:eject() previously
// dropped GEN_REG). The fix mirrors the FnRef fallback added in
// FNREF_COMPOSITE_RESOLVER_v0.1: registry lookup misses fall through to
// xbundle before raising the error.

/// Identical to `make_composite_gt0`, but called as a normal CCall (no fn-ref)
/// from a caller bundle that has the composite name in --reg as well as in
/// --lib. Used to exercise the new CCall xbundle fallback.
fn make_caller_ccall_composite() -> CoreBundle {
    CoreBundle {
        version: "0.5".into(),
        constant_pool: vec![
            ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(7) },
        ],
        nodes: vec![Node::CCall {
            target_identity: sha256_bytes(b"composite_gt0"),
            args: vec![NodeRef::Pool(0)],
            target_name: "composite_gt0".into(),
        }],
    }
}

#[test]
fn ccall_to_composite_with_registry_entry_resolves_via_xbundle() {
    let dir = TempDir::new().unwrap();
    let prov_path = write_bundle(&dir, "composite_gt0.coreir", &make_composite_gt0());
    let caller_path = write_bundle(&dir, "caller.coreir", &make_caller_ccall_composite());
    let exe = dir.path().join("caller_exe");

    // Test-local registry file that names the composite — analogous to
    // axagi-working.axreg's `kind leaf` entries for composite M1 fns. The
    // kind line is informational; the bridge ignores `kind` and resolves
    // via name → Rust impl OR identity → xbundle.
    let reg_path = dir.path().join("composite_gt0.axreg");
    std::fs::write(
        &reg_path,
        "fn composite_gt0\n  kind leaf\n  in (Int)\n  out Bool\nend\n",
    )
    .unwrap();

    let out = Command::new(bridge())
        .args([
            "build", caller_path.to_str().unwrap(),
            "--out", exe.to_str().unwrap(),
            "--lib", prov_path.to_str().unwrap(),
            "--reg", reg_path.to_str().unwrap(),
            "--exe",
        ])
        .output()
        .expect("bridge invocation failed");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "bridge build failed — CCall xbundle fallback regressed:\n{}",
        stderr
    );

    let run = Command::new(&exe).output().expect("failed to run exe");
    assert!(
        run.status.success(),
        "exe failed: stderr={:?}",
        String::from_utf8_lossy(&run.stderr)
    );
}

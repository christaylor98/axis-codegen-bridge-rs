/// Integration tests for the Core IR 0.5 build pipeline.
///
/// Each test creates a 0.5 bundle using the serialiser, writes it to a temp
/// file, then invokes the bridge CLI and checks that the resulting rlib is
/// produced with the expected symbols.

use axis_codegen_bridge::core_ir_05::{
    serialiser::{create_core_bundle_05, make_bool_bundle, make_int_bundle, make_unit_bundle,
                 make_ccall_bundle},
    ConstantPoolEntry, CoreBundle, Node, NodeRef,
    bool_type_hash, int_type_hash, sha256_bytes,
    encode_bool_payload, encode_int_payload,
};
use std::process::Command;
use tempfile::TempDir;

fn bridge() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_axis-codegen-bridge"))
}

fn write_05_bundle(dir: &TempDir, name: &str, bundle: &CoreBundle) -> std::path::PathBuf {
    let bytes = create_core_bundle_05(bundle);
    let path = dir.path().join(name);
    std::fs::write(&path, &bytes).unwrap();
    path
}

fn rlib_path(dir: &TempDir, stem: &str) -> std::path::PathBuf {
    dir.path().join(format!("lib{}.rlib", stem))
}

// ── (1) constant pool only — unit ────────────────────────────────────────────

#[test]
fn test_05_build_unit_bundle() {
    let dir = TempDir::new().unwrap();
    let bundle = make_unit_bundle();
    let fixture = write_05_bundle(&dir, "unit_fn.coreir", &bundle);
    let out = dir.path().join("unit_fn");

    let status = Command::new(bridge())
        .args(["build", fixture.to_str().unwrap(), "--out", out.to_str().unwrap()])
        .status()
        .expect("bridge failed to run");

    assert!(status.success(), "build failed for unit bundle");
    let lib = rlib_path(&dir, "unit_fn");
    assert!(lib.exists(), "rlib not produced at {:?}", lib);

    let nm = Command::new("nm").arg(&lib).output().expect("nm failed");
    let sym = String::from_utf8_lossy(&nm.stdout);
    assert!(sym.contains("unit_fn"), "symbol 'unit_fn' not in rlib:\n{}", sym);
    assert!(sym.contains("_ax_exe_unit_fn"), "exe shim not in rlib:\n{}", sym);
}

// ── (2) constant pool only — bool ─────────────────────────────────────────────

#[test]
fn test_05_build_bool_bundle() {
    let dir = TempDir::new().unwrap();
    let bundle = make_bool_bundle(true);
    let fixture = write_05_bundle(&dir, "bool_fn.coreir", &bundle);
    let out = dir.path().join("bool_fn");

    let status = Command::new(bridge())
        .args(["build", fixture.to_str().unwrap(), "--out", out.to_str().unwrap()])
        .status()
        .expect("bridge failed to run");

    assert!(status.success(), "build failed for bool bundle");
    let lib = rlib_path(&dir, "bool_fn");
    assert!(lib.exists(), "rlib not produced");
    let nm = Command::new("nm").arg(&lib).output().expect("nm failed");
    let sym = String::from_utf8_lossy(&nm.stdout);
    assert!(sym.contains("bool_fn"), "symbol 'bool_fn' not in rlib");
}

// ── (3) constant pool only — int ─────────────────────────────────────────────

#[test]
fn test_05_build_int_bundle() {
    let dir = TempDir::new().unwrap();
    let bundle = make_int_bundle(42);
    let fixture = write_05_bundle(&dir, "int_fn.coreir", &bundle);
    let out = dir.path().join("int_fn");

    let status = Command::new(bridge())
        .args(["build", fixture.to_str().unwrap(), "--out", out.to_str().unwrap()])
        .status()
        .expect("bridge failed to run");

    assert!(status.success(), "build failed for int bundle");
    let lib = rlib_path(&dir, "int_fn");
    assert!(lib.exists(), "rlib not produced");
    let nm = Command::new("nm").arg(&lib).output().expect("nm failed");
    let sym = String::from_utf8_lossy(&nm.stdout);
    assert!(sym.contains("int_fn"), "symbol 'int_fn' not in rlib");
}

// ── (4) CCall node — bool_not ─────────────────────────────────────────────────

#[test]
fn test_05_build_ccall_bool_not() {
    let dir = TempDir::new().unwrap();
    // pool[0] = Bool(false), node[0] = CCall(bool_not, [pool[0]])
    let bundle = make_ccall_bundle(
        sha256_bytes(b"bool_not"),
        vec![ConstantPoolEntry { def_hash: bool_type_hash(), payload: encode_bool_payload(false) }],
        vec![NodeRef::Pool(0)],
    );
    let fixture = write_05_bundle(&dir, "not_fn.coreir", &bundle);
    let out = dir.path().join("not_fn");

    let status = Command::new(bridge())
        .args(["build", fixture.to_str().unwrap(), "--out", out.to_str().unwrap()])
        .status()
        .expect("bridge failed to run");

    assert!(status.success(), "build failed for bool_not CCall");
    let lib = rlib_path(&dir, "not_fn");
    assert!(lib.exists(), "rlib not produced");
    let nm = Command::new("nm").arg(&lib).output().expect("nm failed");
    let sym = String::from_utf8_lossy(&nm.stdout);
    assert!(sym.contains("not_fn"), "symbol 'not_fn' not in rlib");
}

// ── (5) CCall node — int_add ──────────────────────────────────────────────────

#[test]
fn test_05_build_ccall_int_add() {
    let dir = TempDir::new().unwrap();
    // pool[0]=Int(10), pool[1]=Int(32), node[0]=CCall(int_add, [pool[0], pool[1]])
    let bundle = make_ccall_bundle(
        sha256_bytes(b"int_add"),
        vec![
            ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(10) },
            ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(32) },
        ],
        vec![NodeRef::Pool(0), NodeRef::Pool(1)],
    );
    let fixture = write_05_bundle(&dir, "add_fn.coreir", &bundle);
    let out = dir.path().join("add_fn");

    let status = Command::new(bridge())
        .args(["build", fixture.to_str().unwrap(), "--out", out.to_str().unwrap()])
        .status()
        .expect("bridge failed to run");

    assert!(status.success(), "build failed for int_add CCall");
    let lib = rlib_path(&dir, "add_fn");
    assert!(lib.exists(), "rlib not produced");
    let nm = Command::new("nm").arg(&lib).output().expect("nm failed");
    let sym = String::from_utf8_lossy(&nm.stdout);
    assert!(sym.contains("add_fn"), "symbol 'add_fn' not in rlib");
}

// ── (6) CIf node — conditional on bool pool entry ────────────────────────────

#[test]
fn test_05_build_cif_node() {
    let dir = TempDir::new().unwrap();
    // pool[0]=Bool(true), pool[1]=Int(1), pool[2]=Int(0)
    // node[0] = CIf(cond=pool[0], then=pool[1], else=pool[2])
    let bundle = CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![
            ConstantPoolEntry { def_hash: bool_type_hash(), payload: encode_bool_payload(true) },
            ConstantPoolEntry { def_hash: int_type_hash(),  payload: encode_int_payload(1) },
            ConstantPoolEntry { def_hash: int_type_hash(),  payload: encode_int_payload(0) },
        ],
        nodes: vec![Node::CIf {
            cond:  NodeRef::Pool(0),
            then_: NodeRef::Pool(1),
            else_: NodeRef::Pool(2),
        }],
    };
    let fixture = write_05_bundle(&dir, "cif_fn.coreir", &bundle);
    let out = dir.path().join("cif_fn");

    let status = Command::new(bridge())
        .args(["build", fixture.to_str().unwrap(), "--out", out.to_str().unwrap()])
        .status()
        .expect("bridge failed to run");

    assert!(status.success(), "build failed for CIf bundle");
    let lib = rlib_path(&dir, "cif_fn");
    assert!(lib.exists(), "rlib not produced");
    let nm = Command::new("nm").arg(&lib).output().expect("nm failed");
    let sym = String::from_utf8_lossy(&nm.stdout);
    assert!(sym.contains("cif_fn"), "symbol 'cif_fn' not in rlib");
}

// ── (7) inspect subcommand on a 0.5 bundle ────────────────────────────────────

#[test]
fn test_05_inspect() {
    let dir = TempDir::new().unwrap();
    let bundle = make_int_bundle(99);
    let fixture = write_05_bundle(&dir, "inspect_test.coreir", &bundle);

    let output = Command::new(bridge())
        .args(["inspect", fixture.to_str().unwrap()])
        .output()
        .expect("bridge inspect failed");

    assert!(output.status.success(), "inspect exited with error");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("0.5"), "version 0.5 not in inspect output:\n{}", stdout);
    assert!(stdout.contains("constant_pool"), "pool info not in output:\n{}", stdout);
}

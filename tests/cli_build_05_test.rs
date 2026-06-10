/// Integration tests for the Core IR 0.5 build pipeline.
///
/// Each test creates a 0.5 bundle using the serialiser, writes it to a temp
/// file, then invokes the bridge CLI and checks that the resulting .a archive is
/// produced with the expected symbols.

use axis_codegen_bridge::core_ir_05::{
    serialiser::{create_core_bundle_05, make_bool_bundle, make_int_bundle, make_unit_bundle,
                 make_ccall_bundle},
    ConstantPoolEntry, CoreBundle, Node, NodeRef,
    bool_type_hash, int_type_hash, unit_type_hash, sha256_bytes,
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
    dir.path().join(format!("lib{}.a", stem))
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

// ── Cross-bundle fixture builders ────────────────────────────────────────────

/// Provider: always returns bool_not(false) = true (ignores runtime args).
fn make_fn_negate_bundle() -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![ConstantPoolEntry {
            def_hash: bool_type_hash(),
            payload: encode_bool_payload(false),
        }],
        nodes: vec![Node::CCall {
            target_identity: sha256_bytes(b"bool_not"),
            args: vec![NodeRef::Pool(0)],
            target_name: "bool_not".to_string(),
        }],
    }
}

/// Caller: §5b CCall to fn_negate with no args → returns fn_negate's result.
fn make_two_fn_call_bundle() -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![],
        nodes: vec![Node::CCall {
            target_identity: sha256_bytes(b"fn_negate"),
            args: vec![],
            target_name: "fn_negate".to_string(),
        }],
    }
}

// ── (8) cross-bundle: two_fn_call → fn_negate, rlib symbols ──────────────────

#[test]
fn test_05_build_xbundle_two_fn_call() {
    let dir = TempDir::new().unwrap();

    // Build fn_negate provider (single-bundle — must still work, P1 identity export)
    let negate = make_fn_negate_bundle();
    let negate_coreir = write_05_bundle(&dir, "fn_negate.coreir", &negate);
    let negate_out = dir.path().join("fn_negate");
    let status = Command::new(bridge())
        .args(["build", negate_coreir.to_str().unwrap(), "--out", negate_out.to_str().unwrap()])
        .status()
        .expect("bridge failed to run");
    assert!(status.success(), "fn_negate single-bundle build failed");

    // Confirm fn_negate rlib carries the identity export symbol
    let negate_lib = rlib_path(&dir, "fn_negate");
    assert!(negate_lib.exists(), "fn_negate rlib not produced");
    let nm = Command::new("nm").arg(&negate_lib).output().expect("nm failed");
    let sym = String::from_utf8_lossy(&nm.stdout);
    assert!(sym.contains("ax_fn_"), "identity export 'ax_fn_...' missing from fn_negate rlib:\n{}", sym);
    assert!(sym.contains("fn_negate"), "'fn_negate' symbol missing from rlib:\n{}", sym);

    // Build two_fn_call with --lib fn_negate.coreir  (cross-bundle link)
    let caller = make_two_fn_call_bundle();
    let caller_coreir = write_05_bundle(&dir, "two_fn_call.coreir", &caller);
    let caller_out = dir.path().join("two_fn_call");
    let status = Command::new(bridge())
        .args([
            "build", caller_coreir.to_str().unwrap(),
            "--out",  caller_out.to_str().unwrap(),
            "--lib",  negate_coreir.to_str().unwrap(),
        ])
        .status()
        .expect("bridge failed to run");
    assert!(status.success(), "two_fn_call cross-bundle build failed");

    let caller_lib = rlib_path(&dir, "two_fn_call");
    assert!(caller_lib.exists(), "two_fn_call rlib not produced at {:?}", caller_lib);
    let nm = Command::new("nm").arg(&caller_lib).output().expect("nm failed");
    let sym = String::from_utf8_lossy(&nm.stdout);
    assert!(sym.contains("two_fn_call"), "'two_fn_call' symbol missing:\n{}", sym);
    assert!(sym.contains("_ax_exe_two_fn_call"), "'_ax_exe_two_fn_call' symbol missing:\n{}", sym);
    assert!(sym.contains("ax_fn_"), "identity export missing from caller rlib:\n{}", sym);
}

// ── (9) cross-bundle: build exe and run it, verify output ────────────────────

#[test]
fn test_05_build_xbundle_exe_runs() {
    let dir = TempDir::new().unwrap();

    let negate = make_fn_negate_bundle();
    let negate_coreir = write_05_bundle(&dir, "fn_negate.coreir", &negate);

    let caller = make_two_fn_call_bundle();
    let caller_coreir = write_05_bundle(&dir, "two_fn_call.coreir", &caller);
    let exe_out = dir.path().join("two_fn_call_exe");

    let status = Command::new(bridge())
        .args([
            "build", caller_coreir.to_str().unwrap(),
            "--out",  exe_out.to_str().unwrap(),
            "--lib",  negate_coreir.to_str().unwrap(),
            "--exe",
        ])
        .status()
        .expect("bridge failed to run");
    assert!(status.success(), "two_fn_call --exe build failed");
    assert!(exe_out.exists(), "exe not produced at {:?}", exe_out);

    let output = Command::new(&exe_out).output().expect("failed to run exe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "true",
        "expected output 'true', got: {:?}", stdout);
}

// ── (10) FAIL_CLOSED: missing provider → UNRESOLVED_XBUNDLE error ────────────

#[test]
fn test_05_build_xbundle_missing_provider_fails() {
    let dir = TempDir::new().unwrap();

    let caller = make_two_fn_call_bundle();
    let caller_coreir = write_05_bundle(&dir, "two_fn_call.coreir", &caller);
    let caller_out = dir.path().join("two_fn_call");

    let output = Command::new(bridge())
        .args([
            "build", caller_coreir.to_str().unwrap(),
            "--out",  caller_out.to_str().unwrap(),
            // intentionally no --lib fn_negate
        ])
        .output()
        .expect("bridge failed to run");

    assert!(!output.status.success(), "build should fail when provider is missing (FAIL_CLOSED)");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("UNRESOLVED_XBUNDLE"),
        "expected UNRESOLVED_XBUNDLE in stderr, got:\n{}", stderr);
}

// ── BRIDGE_ENTRY_POINTS_V1 test fixtures ─────────────────────────────────────

/// Returns Int(42) regardless of args.
fn make_entry_a_bundle() -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(42) }],
        nodes: vec![],
    }
}

/// Returns Int(99) regardless of args.
fn make_entry_b_bundle() -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(99) }],
        nodes: vec![],
    }
}

/// Panics at runtime: calls option_unwrap(Unit) → panics inside the thread.
fn make_panicky_bundle() -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![ConstantPoolEntry { def_hash: unit_type_hash(), payload: vec![] }],
        nodes: vec![Node::CCall {
            target_identity: sha256_bytes(b"option_unwrap"),
            target_name: "option_unwrap".to_string(),
            args: vec![NodeRef::Pool(0)],
        }],
    }
}

/// Write a minimal registry entry for a built-in with a specific `in` clause.
fn write_registry(dir: &TempDir, fn_name: &str, in_clause: &str) -> std::path::PathBuf {
    let path = dir.path().join(format!("{}.axreg", fn_name));
    std::fs::write(&path, format!(
        "fn {}\n  kind     leaf\n  in       {}\n  out      TextList\n  effect   reads\n  deterministic false\n  idempotent    false\nend\n",
        fn_name, in_clause
    )).unwrap();
    path
}

// ── (12) back-compat: no --entries → output format unchanged ─────────────────

#[test]
fn test_ep_no_entries_back_compat() {
    let dir = TempDir::new().unwrap();
    let negate = make_fn_negate_bundle();
    let negate_coreir = write_05_bundle(&dir, "fn_negate.coreir", &negate);

    let caller = make_two_fn_call_bundle();
    let caller_coreir = write_05_bundle(&dir, "two_fn_call.coreir", &caller);
    let exe_out = dir.path().join("back_compat_exe");

    let status = Command::new(bridge())
        .args([
            "build", caller_coreir.to_str().unwrap(),
            "--out",  exe_out.to_str().unwrap(),
            "--lib",  negate_coreir.to_str().unwrap(),
            "--exe",
        ])
        .status()
        .expect("bridge failed to run");
    assert!(status.success(), "back-compat build failed");

    let output = Command::new(&exe_out).output().expect("failed to run exe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Without --entries, output is just the bare result — no "name: " prefix.
    assert_eq!(stdout.trim(), "true",
        "back-compat: expected bare 'true', got: {:?}", stdout);
}

// ── (13) --entries: two named bundle entries both run ─────────────────────────

#[test]
fn test_ep_two_entries_both_run() {
    let dir = TempDir::new().unwrap();

    let entry_a = make_entry_a_bundle();
    let entry_a_coreir = write_05_bundle(&dir, "entry_a.coreir", &entry_a);

    let entry_b = make_entry_b_bundle();
    let entry_b_coreir = write_05_bundle(&dir, "entry_b.coreir", &entry_b);

    // Root bundle: trivial unit (unused — entries are the entry points)
    let root = make_unit_bundle();
    let root_coreir = write_05_bundle(&dir, "root.coreir", &root);
    let exe_out = dir.path().join("two_entries_exe");

    let status = Command::new(bridge())
        .args([
            "build",   root_coreir.to_str().unwrap(),
            "--out",   exe_out.to_str().unwrap(),
            "--lib",   entry_a_coreir.to_str().unwrap(),
            "--lib",   entry_b_coreir.to_str().unwrap(),
            "--entries", "entry_a,entry_b",
            "--exe",
        ])
        .status()
        .expect("bridge failed to run");
    assert!(status.success(), "two-entry build failed");
    assert!(exe_out.exists(), "exe not produced");

    let output = Command::new(&exe_out).output().expect("failed to run exe");
    assert!(output.status.success(), "exe exited non-zero");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("entry_a: 42"), "missing 'entry_a: 42' in stdout:\n{}", stdout);
    assert!(stdout.contains("entry_b: 99"), "missing 'entry_b: 99' in stdout:\n{}", stdout);
}

// ── (14) --entry (repeatable): same as --entries but one flag per name ────────

#[test]
fn test_ep_entry_flag_repeatable() {
    let dir = TempDir::new().unwrap();

    let entry_a = make_entry_a_bundle();
    let entry_a_coreir = write_05_bundle(&dir, "entry_a.coreir", &entry_a);

    let entry_b = make_entry_b_bundle();
    let entry_b_coreir = write_05_bundle(&dir, "entry_b.coreir", &entry_b);

    let root = make_unit_bundle();
    let root_coreir = write_05_bundle(&dir, "root.coreir", &root);
    let exe_out = dir.path().join("repflag_exe");

    let status = Command::new(bridge())
        .args([
            "build", root_coreir.to_str().unwrap(),
            "--out", exe_out.to_str().unwrap(),
            "--lib", entry_a_coreir.to_str().unwrap(),
            "--lib", entry_b_coreir.to_str().unwrap(),
            "--entry", "entry_a",
            "--entry", "entry_b",
            "--exe",
        ])
        .status()
        .expect("bridge failed to run");
    assert!(status.success(), "--entry repeatable build failed");

    let output = Command::new(&exe_out).output().expect("failed to run exe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("entry_a:"), "missing entry_a in stdout:\n{}", stdout);
    assert!(stdout.contains("entry_b:"), "missing entry_b in stdout:\n{}", stdout);
}

// ── (15) panic isolation: panicking entry does not kill good entry ────────────

#[test]
fn test_ep_panic_isolation() {
    let dir = TempDir::new().unwrap();

    let entry_a = make_entry_a_bundle();
    let entry_a_coreir = write_05_bundle(&dir, "entry_a.coreir", &entry_a);

    let panicky = make_panicky_bundle();
    let panicky_coreir = write_05_bundle(&dir, "panicky.coreir", &panicky);

    let root = make_unit_bundle();
    let root_coreir = write_05_bundle(&dir, "root.coreir", &root);
    let exe_out = dir.path().join("panic_iso_exe");

    let status = Command::new(bridge())
        .args([
            "build",   root_coreir.to_str().unwrap(),
            "--out",   exe_out.to_str().unwrap(),
            "--lib",   entry_a_coreir.to_str().unwrap(),
            "--lib",   panicky_coreir.to_str().unwrap(),
            "--entries", "entry_a,panicky",
            "--exe",
        ])
        .status()
        .expect("bridge failed to run");
    assert!(status.success(), "panic-isolation build failed");

    let output = Command::new(&exe_out).output().expect("failed to run exe");
    // exit code must be non-zero (panicky thread failed)
    assert!(!output.status.success(), "expected non-zero exit due to panicky entry");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // good entry still produces output
    assert!(stdout.contains("entry_a: 42"), "missing 'entry_a: 42' in stdout:\n{}", stdout);
    // panicky entry reports PANIC in stderr
    assert!(stderr.contains("panicky: PANIC"), "missing 'panicky: PANIC' in stderr:\n{}", stderr);
}

// ── (16) UNRESOLVED_ENTRY: missing provider → hard halt ──────────────────────

#[test]
fn test_ep_unresolved_entry_fails() {
    let dir = TempDir::new().unwrap();
    let root = make_unit_bundle();
    let root_coreir = write_05_bundle(&dir, "root.coreir", &root);
    let exe_out = dir.path().join("unresolved_exe");

    let output = Command::new(bridge())
        .args([
            "build", root_coreir.to_str().unwrap(),
            "--out", exe_out.to_str().unwrap(),
            "--entries", "no_such_fn",
            "--exe",
        ])
        .output()
        .expect("bridge failed to run");

    assert!(!output.status.success(), "should fail with UNRESOLVED_ENTRY");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("UNRESOLVED_ENTRY"),
        "expected UNRESOLVED_ENTRY in stderr:\n{}", stderr);
}

// ── (17) ENTRY_ABI_MISMATCH: foreign fn with wrong `in` → rejected ────────────

#[test]
fn test_ep_foreign_entry_abi_mismatch() {
    let dir = TempDir::new().unwrap();
    let root = make_unit_bundle();
    let root_coreir = write_05_bundle(&dir, "root.coreir", &root);
    let exe_out = dir.path().join("abi_mismatch_exe");

    // Registry says `argv` has `in (Int)` — wrong ABI for an entry point.
    let reg = write_registry(&dir, "argv", "(Int)");

    let output = Command::new(bridge())
        .args([
            "build", root_coreir.to_str().unwrap(),
            "--out", exe_out.to_str().unwrap(),
            "--entry", "argv",
            "--reg", reg.to_str().unwrap(),
            "--exe",
        ])
        .output()
        .expect("bridge failed to run");

    assert!(!output.status.success(), "should fail with ENTRY_ABI_MISMATCH");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ENTRY_ABI_MISMATCH"),
        "expected ENTRY_ABI_MISMATCH in stderr:\n{}", stderr);
}

// ── (18) foreign fn entry with `in (TextList)` → accepted and runs ───────────

#[test]
fn test_ep_foreign_entry_abi_match() {
    let dir = TempDir::new().unwrap();
    let root = make_unit_bundle();
    let root_coreir = write_05_bundle(&dir, "root.coreir", &root);
    let exe_out = dir.path().join("abi_match_exe");

    // Registry says `argv` has `in (TextList)` — correct ABI.
    let reg = write_registry(&dir, "argv", "(TextList)");

    let status = Command::new(bridge())
        .args([
            "build", root_coreir.to_str().unwrap(),
            "--out", exe_out.to_str().unwrap(),
            "--entry", "argv",
            "--reg", reg.to_str().unwrap(),
            "--exe",
        ])
        .status()
        .expect("bridge failed to run");

    assert!(status.success(), "build with TextList foreign entry failed");
    assert!(exe_out.exists(), "exe not produced");

    let output = Command::new(&exe_out).output().expect("failed to run exe");
    assert!(output.status.success(), "exe exited non-zero");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // argv returns the process args as a list; output will be "argv: [...]"
    assert!(stdout.contains("argv:"), "expected 'argv:' label in stdout:\n{}", stdout);
}

// ── (19) result sink: assert-based multi-entry — PASS + FAIL via AdaptiveCell ─

/// assert(Bool(true)) → Unit (PASS via result sink).
fn make_assert_pass_bundle() -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![ConstantPoolEntry { def_hash: bool_type_hash(), payload: encode_bool_payload(true) }],
        nodes: vec![Node::CCall {
            target_identity: sha256_bytes(b"assert"),
            target_name: "assert".to_string(),
            args: vec![NodeRef::Pool(0)],
        }],
    }
}

/// assert(Bool(false)) → panic (FAIL — caught by catch_unwind in harness).
fn make_assert_fail_bundle() -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![ConstantPoolEntry { def_hash: bool_type_hash(), payload: encode_bool_payload(false) }],
        nodes: vec![Node::CCall {
            target_identity: sha256_bytes(b"assert"),
            target_name: "assert".to_string(),
            args: vec![NodeRef::Pool(0)],
        }],
    }
}

#[test]
fn test_ep_result_sink_mixed_verdicts() {
    let dir = TempDir::new().unwrap();

    let pass_coreir = write_05_bundle(&dir, "assert_pass.coreir", &make_assert_pass_bundle());
    let fail_coreir = write_05_bundle(&dir, "assert_fail.coreir", &make_assert_fail_bundle());
    let root_coreir = write_05_bundle(&dir, "root.coreir", &make_unit_bundle());
    let exe_out = dir.path().join("sink_test_exe");

    let status = Command::new(bridge())
        .args([
            "build", root_coreir.to_str().unwrap(),
            "--out",     exe_out.to_str().unwrap(),
            "--lib",     pass_coreir.to_str().unwrap(),
            "--lib",     fail_coreir.to_str().unwrap(),
            "--entries", "assert_pass,assert_fail",
            "--exe",
        ])
        .status()
        .expect("bridge failed to run");
    assert!(status.success(), "result-sink build failed");

    let output = Command::new(&exe_out).output().expect("failed to run exe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // assert_fail panics → non-zero exit
    assert!(!output.status.success(), "expected non-zero exit (assert_fail panics)");
    // assert_pass completes; sink records PASS; harness prints "assert_pass: PASS"
    assert!(stdout.contains("assert_pass: PASS"),
        "expected 'assert_pass: PASS' in stdout:\n{}", stdout);
    // assert_fail panics before writing verdict; harness prints "assert_fail: PANIC"
    assert!(stderr.contains("assert_fail: PANIC"),
        "expected 'assert_fail: PANIC' in stderr:\n{}", stderr);
}

// ── (11) inspect subcommand on a 0.5 bundle ───────────────────────────────────

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

// ── (20) ping-pong: two loops exchanging signals via shared atomics ────────────

fn make_ping_bundle() -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![ConstantPoolEntry { def_hash: unit_type_hash(), payload: vec![] }],
        nodes: vec![Node::CCall {
            target_identity: sha256_bytes(b"ping_loop"),
            target_name: "ping_loop".to_string(),
            args: vec![NodeRef::Pool(0)],
        }],
    }
}

fn make_pong_bundle() -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![ConstantPoolEntry { def_hash: unit_type_hash(), payload: vec![] }],
        nodes: vec![Node::CCall {
            target_identity: sha256_bytes(b"pong_loop"),
            target_name: "pong_loop".to_string(),
            args: vec![NodeRef::Pool(0)],
        }],
    }
}

#[test]
fn test_ping_pong_two_loops() {
    let dir = TempDir::new().unwrap();

    let ping = make_ping_bundle();
    let ping_coreir = write_05_bundle(&dir, "ping.coreir", &ping);

    let pong = make_pong_bundle();
    let pong_coreir = write_05_bundle(&dir, "pong.coreir", &pong);

    let root = make_unit_bundle();
    let root_coreir = write_05_bundle(&dir, "root.coreir", &root);
    let exe_out = dir.path().join("pingpong_exe");

    let status = Command::new(bridge())
        .args([
            "build",     root_coreir.to_str().unwrap(),
            "--out",     exe_out.to_str().unwrap(),
            "--lib",     ping_coreir.to_str().unwrap(),
            "--lib",     pong_coreir.to_str().unwrap(),
            "--entries", "ping,pong",
            "--exe",
        ])
        .status()
        .expect("bridge failed to run");
    assert!(status.success(), "ping-pong build failed");
    assert!(exe_out.exists(), "exe not produced");

    let output = Command::new(&exe_out).output().expect("failed to run exe");
    assert!(output.status.success(), "ping-pong exe exited non-zero:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);

    // All 5 rounds must complete and be reported by both sides.
    for round in 1..=5 {
        assert!(stdout.contains(&format!("ping: round {} ok", round)),
            "missing 'ping: round {} ok' in:\n{}", round, stdout);
        assert!(stdout.contains(&format!("pong: round {} done", round)),
            "missing 'pong: round {} done' in:\n{}", round, stdout);
    }

    // Verify pong computed the right answers for each payload shape.
    // Round 1: Int(7) → 7² = 49
    assert!(stdout.contains("→ 49]"), "round 1 (square) wrong in:\n{}", stdout);
    // Round 2: List(0..100) → sum = 4950
    assert!(stdout.contains("→ 4950]"), "round 2 (list sum) wrong in:\n{}", stdout);
    // Round 3: Str(240 chars) → char count = 240
    assert!(stdout.contains("→ 240]"), "round 3 (str len) wrong in:\n{}", stdout);
    // Round 4: List(0..5000) → sum = 12497500
    assert!(stdout.contains("→ 12497500]"), "round 4 (large list sum) wrong in:\n{}", stdout);
    // Round 5: List(3×List(1000)) → flatten+sum = 1498500
    assert!(stdout.contains("→ 1498500]"), "round 5 (nested sum) wrong in:\n{}", stdout);

    // Harness-level verdict from the result sink.
    assert!(stdout.contains("ping: PASS"), "missing 'ping: PASS' in:\n{}", stdout);
    assert!(stdout.contains("pong: PASS"), "missing 'pong: PASS' in:\n{}", stdout);
}

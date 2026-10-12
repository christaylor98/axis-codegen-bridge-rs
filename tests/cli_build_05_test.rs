/// Integration tests for the Core IR 0.5 build pipeline.
///
/// Each test creates a 0.5 bundle using the serialiser, writes it to a temp
/// file, then invokes the bridge CLI and checks that the resulting .a archive is
/// produced with the expected symbols.

use axis_codegen_bridge::core_ir_05::{
    serialiser::{create_core_bundle_05, make_bool_bundle, make_int_bundle, make_unit_bundle,
                 make_ccall_bundle, make_ccall_bundle_named},
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
    let bundle = make_ccall_bundle_named(
        sha256_bytes(b"int_add"),
        "int_add",
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
    // [ CEnd pool[1] ] [ CEnd pool[2] ] node[4] = CIf(cond=pool[0], then=node[1], else=node[3])
    let bundle = CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![
            ConstantPoolEntry { def_hash: bool_type_hash(), payload: encode_bool_payload(true) },
            ConstantPoolEntry { def_hash: int_type_hash(),  payload: encode_int_payload(1) },
            ConstantPoolEntry { def_hash: int_type_hash(),  payload: encode_int_payload(0) },
        ],
        nodes: vec![
            Node::CBlock,
            Node::CEnd { value: NodeRef::Pool(1) },
            Node::CBlock,
            Node::CEnd { value: NodeRef::Pool(2) },
            Node::CIf { cond: NodeRef::Pool(0), then_: NodeRef::Node(1), else_: NodeRef::Node(3) },
        ],
        result: NodeRef::Node(4),
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
        result: NodeRef::Node(0),
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
        result: NodeRef::Node(0),
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
        result: NodeRef::Pool(0),
    }
}

/// Returns Int(99) regardless of args.
fn make_entry_b_bundle() -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(99) }],
        nodes: vec![],
        result: NodeRef::Pool(0),
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
        result: NodeRef::Node(0),
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

    // Cap the respawn budget. This entry panics on EVERY attempt by
    // construction, so the driver walks its whole restart budget before
    // reporting the worker down. The default is 100 (`_ax_restart_budget`,
    // src/main.rs) with a 2s backoff cap, which is ~193s of sleeping per
    // panicking entry — the two tests that build one were, between them, the
    // bulk of this suite's wall time. Nothing here asserts on the budget
    // number: the assertions are a non-zero exit and the PANIC/PASS lines,
    // which a budget of 2 reaches by the same path, just sooner.
    let output = Command::new(&exe_out)
        .env("AXVERITY_ENTRY_RESTART_BUDGET", "2")
        .output()
        .expect("failed to run exe");
    // exit code must be non-zero (panicky thread failed)
    assert!(!output.status.success(), "expected non-zero exit due to panicky entry");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // good entry still produces output
    assert!(stdout.contains("entry_a: 42"), "missing 'entry_a: 42' in stdout:\n{}", stdout);
    // Built with no --reg, so nothing declares option_unwrap pure: an undeclared builtin is effectful, and its
    // panic is Unknown (not settled -> not a Defect). The other entry is unaffected; the run exits 3.
    assert!(stderr.contains("panicky: UNKNOWN") && stderr.contains("option_unwrap"),
        "missing 'panicky: UNKNOWN ... option_unwrap' in stderr:\n{}", stderr);
    assert_eq!(output.status.code(), Some(3), "an Unknown entry result exits 3");
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
        result: NodeRef::Node(0),
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
        result: NodeRef::Node(0),
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

    // Cap the respawn budget. This entry panics on EVERY attempt by
    // construction, so the driver walks its whole restart budget before
    // reporting the worker down. The default is 100 (`_ax_restart_budget`,
    // src/main.rs) with a 2s backoff cap, which is ~193s of sleeping per
    // panicking entry — the two tests that build one were, between them, the
    // bulk of this suite's wall time. Nothing here asserts on the budget
    // number: the assertions are a non-zero exit and the PANIC/PASS lines,
    // which a budget of 2 reaches by the same path, just sooner.
    let output = Command::new(&exe_out)
        .env("AXVERITY_ENTRY_RESTART_BUDGET", "2")
        .output()
        .expect("failed to run exe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // assert_fail panics → non-zero exit
    assert!(!output.status.success(), "expected non-zero exit (assert_fail panics)");
    // assert_pass completes; sink records PASS; harness prints "assert_pass: PASS"
    assert!(stdout.contains("assert_pass: PASS"),
        "expected 'assert_pass: PASS' in stdout:\n{}", stdout);
    // assert_fail panics before writing verdict; harness prints "assert_fail: PANIC"
    assert!(stderr.contains("assert_fail: DEFECT") || stderr.contains("assert_fail: UNKNOWN") || stderr.contains("assert_fail: PANIC"),
        "expected assert_fail to be reported as failed in stderr:\n{}", stderr);
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
    assert!(stdout.contains("0.6"), "version 0.6 not in inspect output:\n{}", stdout);
    assert!(stdout.contains("constant_pool"), "pool info not in output:\n{}", stdout);
}

// ── (20) ping-pong round-trip ─────────────────────────────────────────────────
// Retired with runtime/signals.rs (BRIDGE_ASYNC_PRIMITIVES_V1). The equivalent
// 5-round exchange is now driven by the real event_subscribe / channel_send /
// wait primitives in tests/async_roundtrip_test.rs.

// ── Fix-5: bool_to_str — CCall bundle builds and exe prints "true" / "false" ─

fn make_bool_to_str_bundle(b: bool) -> CoreBundle {
    // pool[0] = Bool(b), node[0] = CCall(bool_to_str, [pool[0]])
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![ConstantPoolEntry {
            def_hash: bool_type_hash(),
            payload: encode_bool_payload(b),
        }],
        nodes: vec![Node::CCall {
            target_identity: sha256_bytes(b"bool_to_str"),
            target_name: "bool_to_str".to_string(),
            args: vec![NodeRef::Pool(0)],
        }],
        result: NodeRef::Node(0),
    }
}

#[test]
fn test_bool_to_str_true_builds() {
    let dir = TempDir::new().unwrap();
    let bundle = make_bool_to_str_bundle(true);
    let fixture = write_05_bundle(&dir, "bool_to_str_t.coreir", &bundle);
    let out = dir.path().join("bool_to_str_t");

    let status = Command::new(bridge())
        .args(["build", fixture.to_str().unwrap(), "--out", out.to_str().unwrap()])
        .status()
        .expect("bridge failed to run");
    assert!(status.success(), "build failed for bool_to_str(true)");
}

#[test]
fn test_bool_to_str_exe_true_prints_true() {
    let dir = TempDir::new().unwrap();
    let bundle = make_bool_to_str_bundle(true);
    let fixture = write_05_bundle(&dir, "bts_true.coreir", &bundle);
    let out = dir.path().join("bts_true");

    let status = Command::new(bridge())
        .args(["build", fixture.to_str().unwrap(), "--out", out.to_str().unwrap(), "--exe"])
        .status()
        .expect("bridge failed to run");
    assert!(status.success(), "build failed");
    assert!(out.exists(), "exe not produced");

    let output = Command::new(&out).output().expect("failed to run exe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "true",
        "bool_to_str(true) must print 'true', got: {:?}", stdout);
}

#[test]
fn test_bool_to_str_exe_false_prints_false() {
    let dir = TempDir::new().unwrap();
    let bundle = make_bool_to_str_bundle(false);
    let fixture = write_05_bundle(&dir, "bts_false.coreir", &bundle);
    let out = dir.path().join("bts_false");

    let status = Command::new(bridge())
        .args(["build", fixture.to_str().unwrap(), "--out", out.to_str().unwrap(), "--exe"])
        .status()
        .expect("bridge failed to run");
    assert!(status.success(), "build failed");
    assert!(out.exists(), "exe not produced");

    let output = Command::new(&out).output().expect("failed to run exe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "false",
        "bool_to_str(false) must print 'false', got: {:?}", stdout);
}

// ── (BRANCH_SCOPING_V1) CIf must not run the untaken arm's side effects ──────
//
// Regression test for the bug where `CIf` only selected between two
// already-materialized node values instead of gating their computation: a
// `fs_append_text` call placed in the `else` arm used to fire even when the
// `then` arm was taken, because every node was emitted as an unconditional
// `let node_N = <expr>;` ahead of the runtime `if`. Runs the compiled exe
// end-to-end (not just symbol presence) and checks the side-effect file.

use axis_codegen_bridge::core_ir_05::encode_text_payload;

/// Mirrors the reported repro exactly:
///   fn side_effect() -> Unit { fs_append_text(path, msg) }
///   fn iftest()      -> Bool { if <cond> { true } else { side_effect() } }
/// `side_effect` is a separate linked bundle (§5b composite, like the
/// reported `fn side_effect(flag: Bool) -> Bool { let _ = fs_append_text(...); Bool(false) }`)
/// — `cond` is a constant pool Bool rather than a CLI param, since the exe
/// shim always passes argv as `Value::List(Str)` regardless of the target
/// function's real parameter shape.
fn make_side_effect_bundle(log_path: &str) -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![
            ConstantPoolEntry { def_hash: axis_codegen_bridge::core_ir_05::text_type_hash(), payload: encode_text_payload(log_path) },
            ConstantPoolEntry { def_hash: axis_codegen_bridge::core_ir_05::text_type_hash(), payload: encode_text_payload("side effect ran\n") },
        ],
        nodes: vec![Node::CCall {
            target_identity: sha256_bytes(b"fs_append_text"),
            target_name: "fs_append_text".to_string(),
            args: vec![NodeRef::Pool(0), NodeRef::Pool(1)],
        }],
        result: NodeRef::Node(0),
    }
}

fn make_iftest_bundle(cond: bool) -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![
            ConstantPoolEntry { def_hash: bool_type_hash(), payload: encode_bool_payload(cond) },
            ConstantPoolEntry { def_hash: bool_type_hash(), payload: encode_bool_payload(true) },
        ],
        nodes: vec![
            // then-arm: [ CEnd pool[1] (true) ]
            Node::CBlock,
            Node::CEnd { value: NodeRef::Pool(1) },
            // else-arm: [ node[3] = side_effect() — §5b call into the linked bundle above; CEnd node[3] ]
            Node::CBlock,
            Node::CCall {
                target_identity: sha256_bytes(b"side_effect"),
                target_name: "side_effect".to_string(),
                args: vec![],
            },
            Node::CEnd { value: NodeRef::Node(3) },
            // node[5]: if pool[0] then node[1] else node[4]
            Node::CIf { cond: NodeRef::Pool(0), then_: NodeRef::Node(1), else_: NodeRef::Node(4) },
        ],
        result: NodeRef::Node(5),
    }
}

fn build_iftest_exe(dir: &TempDir, cond: bool, stem: &str) -> std::path::PathBuf {
    // --lib provider identity/name is derived from the file's stem (see
    // main.rs's provider_map construction), so this must be named exactly
    // `side_effect.coreir` to match the `sha256(b"side_effect")` target
    // identity `iftest`'s CCall resolves against.
    let side_effect_coreir = write_05_bundle(dir, "side_effect.coreir", &make_side_effect_bundle(
        dir.path().join("sideeffect.log").to_str().unwrap(),
    ));
    let iftest_coreir = write_05_bundle(dir, &format!("{}_iftest.coreir", stem), &make_iftest_bundle(cond));
    let out = dir.path().join(stem);
    let status = Command::new(bridge())
        .args([
            "build", iftest_coreir.to_str().unwrap(),
            "--out", out.to_str().unwrap(),
            "--lib", side_effect_coreir.to_str().unwrap(),
            "--exe",
        ])
        .status()
        .expect("bridge failed to run");
    assert!(status.success(), "iftest/side_effect cross-bundle build failed");
    out
}

// ── An orphan in no block (Core IR 0.6) ──────────────────────────────────────
//
// A node with no consumer, written outside any block, runs once on every call in
// node-index order (Core IR 0.6: a node runs when every block around it runs, and
// this one is in none). Ordinary AI3 emits it for a top-level discarded effect
// written after the condition's binding — `let c = ...; let a = eff(); let v = if
// c {...}`. These tests pin it: the orphan effect fires EXACTLY ONCE, in BOTH
// branches, and in program order ahead of the `CIf`.

/// `fn(cond) -> Bool { let _ = fs_append_text(log, "orphan\n"); if !cond { true } else { false } }`:
/// node[0] is the `CIf`'s cond, node[1] is the orphan, then the two arm blocks and the `CIf`.
fn make_orphan_before_cif_bundle(log_path: &str, cond: bool) -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![
            ConstantPoolEntry { def_hash: bool_type_hash(), payload: encode_bool_payload(cond) },
            ConstantPoolEntry { def_hash: axis_codegen_bridge::core_ir_05::text_type_hash(), payload: encode_text_payload(log_path) },
            ConstantPoolEntry { def_hash: axis_codegen_bridge::core_ir_05::text_type_hash(), payload: encode_text_payload("orphan\n") },
            ConstantPoolEntry { def_hash: bool_type_hash(), payload: encode_bool_payload(true) },
            ConstantPoolEntry { def_hash: bool_type_hash(), payload: encode_bool_payload(false) },
        ],
        nodes: vec![
            // node[0]: bool_not(pool[0]) — the CIf's cond, a real anchor.
            Node::CCall {
                target_identity: sha256_bytes(b"bool_not"),
                target_name: "bool_not".to_string(),
                args: vec![NodeRef::Pool(0)],
            },
            // node[1]: fs_append_text(log, "orphan\n") — zero consumers.
            Node::CCall {
                target_identity: sha256_bytes(b"fs_append_text"),
                target_name: "fs_append_text".to_string(),
                args: vec![NodeRef::Pool(1), NodeRef::Pool(2)],
            },
            // [ CEnd pool[3] ] [ CEnd pool[4] ] node[6]: if node[0] then node[3] else node[5]
            Node::CBlock,
            Node::CEnd { value: NodeRef::Pool(3) },
            Node::CBlock,
            Node::CEnd { value: NodeRef::Pool(4) },
            Node::CIf { cond: NodeRef::Node(0), then_: NodeRef::Node(3), else_: NodeRef::Node(5) },
        ],
        result: NodeRef::Node(6),
    }
}

/// Builds and runs the bundle above, returning (stdout, log file contents).
fn run_orphan_before_cif(cond: bool, stem: &str) -> (String, String) {
    let dir = TempDir::new().unwrap();
    let log_path = dir.path().join("orphan.log");
    let bundle = make_orphan_before_cif_bundle(log_path.to_str().unwrap(), cond);
    let fixture = write_05_bundle(&dir, &format!("{}.coreir", stem), &bundle);
    let out = dir.path().join(stem);

    let status = Command::new(bridge())
        .args([
            "build", fixture.to_str().unwrap(),
            "--out", out.to_str().unwrap(),
            "--exe",
        ])
        .status()
        .expect("bridge failed to run");
    assert!(
        status.success(),
        "orphan-before-CIf is a legal bundle shape and must build (cond={})",
        cond
    );

    let output = Command::new(&out).output().expect("failed to run exe");
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    (String::from_utf8_lossy(&output.stdout).trim().to_string(), log)
}

#[test]
fn test_orphan_before_unanchored_cif_runs_unconditionally_then_branch() {
    // cond=false → bool_not gives true → then-arm taken.
    let (stdout, log) = run_orphan_before_cif(false, "orphan_then");
    assert_eq!(stdout, "true", "expected the then-arm value, got: {:?}", stdout);
    assert_eq!(
        log, "orphan\n",
        "the orphan effect must run exactly once even when the then-arm is taken"
    );
}

#[test]
fn test_orphan_before_unanchored_cif_runs_unconditionally_else_branch() {
    // cond=true → bool_not gives false → else-arm taken. Same orphan, same
    // single firing: it is top-level, not attributed to either arm.
    let (stdout, log) = run_orphan_before_cif(true, "orphan_else");
    assert_eq!(stdout, "false", "expected the else-arm value, got: {:?}", stdout);
    assert_eq!(
        log, "orphan\n",
        "the orphan effect must run exactly once even when the else-arm is taken"
    );
}

#[test]
fn test_cif_then_taken_does_not_run_else_side_effect() {
    let dir = TempDir::new().unwrap();
    let log_path = dir.path().join("sideeffect.log");
    let exe = build_iftest_exe(&dir, true, "iftest_true");

    let output = Command::new(&exe).output().expect("failed to run exe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "true", "expected the then-branch value, got: {:?}", stdout);

    assert!(
        !log_path.exists(),
        "BRANCH_SCOPING regression: else-branch's side_effect() ran even though \
         the then-branch was taken; log file exists at {:?}",
        log_path
    );
}

#[test]
fn test_cif_else_taken_runs_else_side_effect() {
    let dir = TempDir::new().unwrap();
    let log_path = dir.path().join("sideeffect.log");
    let exe = build_iftest_exe(&dir, false, "iftest_false");

    let output = Command::new(&exe).output().expect("failed to run exe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // side_effect() -> Unit; the exe shim only prints non-Unit results.
    assert_eq!(stdout.trim(), "", "expected Unit (no printed output), got: {:?}", stdout);

    let contents = std::fs::read_to_string(&log_path)
        .unwrap_or_else(|e| panic!("expected else-branch side effect to have run at {:?}: {}", log_path, e));
    assert_eq!(contents, "side effect ran\n");
}

// ── (BUG2_RESULT_FIELD_V1) result must not be inferred as "the last node" ────
//
// Regression test for the bug where the bridge guessed a bundle's return
// value as `node_{last}` (or, failing that, the first Data pool entry) with
// no `CoreBundle.result` field to consult. That guess is simply wrong
// whenever the source's tail expression is a bare literal or `VarRef`
// following a real call — neither pushes a node of its own, so the call's
// own value silently became the bundle's return instead. The reported repro
// was `fn se2(addr: Text) -> Bool { let _ = fs_append_text(...); Bool(false) }`,
// which printed `()` (fs_append_text's Unit) instead of `false`.
//
// Both tests build+run the real compiled binary end-to-end, not just the
// emitted Rust source, so a reintroduced guess-chain fails loudly here.

#[test]
fn test_result_points_past_discarded_call_to_literal_tail() {
    let dir = TempDir::new().unwrap();
    // pool[0]=Int(3), pool[1]=Int(4)  -- args to the discarded call
    // pool[2]=Bool(false)             -- the actual tail value
    // node[0] = int_add(pool[0], pool[1])  -- discarded; evaluates to Int(7)
    // result = pool[2]  -- NOT node[0]
    let bundle = CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![
            ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(3) },
            ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(4) },
            ConstantPoolEntry { def_hash: bool_type_hash(), payload: encode_bool_payload(false) },
        ],
        nodes: vec![Node::CCall {
            target_identity: sha256_bytes(b"int_add"),
            target_name: "int_add".to_string(),
            args: vec![NodeRef::Pool(0), NodeRef::Pool(1)],
        }],
        result: NodeRef::Pool(2),
    };
    let fixture = write_05_bundle(&dir, "discard_then_literal.coreir", &bundle);
    let out = dir.path().join("discard_then_literal");

    let status = Command::new(bridge())
        .args(["build", fixture.to_str().unwrap(), "--out", out.to_str().unwrap(), "--exe"])
        .status()
        .expect("bridge failed to run");
    assert!(status.success(), "build failed");

    let output = Command::new(&out).output().expect("failed to run exe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(), "false",
        "BUG2_RESULT_FIELD regression: expected the literal tail 'false', \
         got the discarded call's own value instead: {:?}",
        stdout
    );
}

#[test]
fn test_result_points_past_discarded_call_to_earlier_node() {
    // Mirrors the axVerity `push_object` gotcha (documented pre-fix
    // workaround: wrapping a bare trailing VarRef in an identity CCall)
    // taken one step further: the tail VarRef aliases an EARLIER node, with
    // a second, discarded call pushed after it. `result` must point at the
    // earlier node directly — not "the last node", which here is neither
    // the discarded call NOR the intended value.
    let dir = TempDir::new().unwrap();
    let bundle = CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![
            ConstantPoolEntry { def_hash: bool_type_hash(), payload: encode_bool_payload(true) },
        ],
        nodes: vec![
            // node[0]: bool_not(pool[0]) -- the real tail value (Bool(false))
            Node::CCall {
                target_identity: sha256_bytes(b"bool_not"),
                target_name: "bool_not".to_string(),
                args: vec![NodeRef::Pool(0)],
            },
            // node[1]: bool_not(node[0]) -- discarded; evaluates to Bool(true)
            Node::CCall {
                target_identity: sha256_bytes(b"bool_not"),
                target_name: "bool_not".to_string(),
                args: vec![NodeRef::Node(0)],
            },
        ],
        result: NodeRef::Node(0),
    };
    let fixture = write_05_bundle(&dir, "discard_then_earlier_node.coreir", &bundle);
    let out = dir.path().join("discard_then_earlier_node");

    let status = Command::new(bridge())
        .args(["build", fixture.to_str().unwrap(), "--out", out.to_str().unwrap(), "--exe"])
        .status()
        .expect("bridge failed to run");
    assert!(status.success(), "build failed");

    let output = Command::new(&out).output().expect("failed to run exe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(), "false",
        "BUG2_RESULT_FIELD regression: expected node[0]'s value 'false', \
         got the last-pushed node's value instead: {:?}",
        stdout
    );
}

// ── Fix-3: stale rlib regression — provider is always recompiled ──────────────
//
// Builds the same provider bundle twice into the same output directory.
// With the old code the second build would skip recompilation (if !exists).
// With fix-3 the bridge always recompiles, so both builds must succeed and
// the final exe must run correctly.

#[test]
fn test_provider_always_recompiled_second_build_succeeds() {
    let dir = TempDir::new().unwrap();

    let negate = make_fn_negate_bundle();
    let negate_coreir = write_05_bundle(&dir, "fn_negate.coreir", &negate);

    let caller = make_two_fn_call_bundle();
    let caller_coreir = write_05_bundle(&dir, "two_fn_call.coreir", &caller);
    let exe_out = dir.path().join("stale_test_exe");

    // First build
    let s1 = Command::new(bridge())
        .args(["build", caller_coreir.to_str().unwrap(),
               "--out", exe_out.to_str().unwrap(),
               "--lib", negate_coreir.to_str().unwrap(), "--exe"])
        .status().expect("bridge failed on first build");
    assert!(s1.success(), "first build failed");

    // Second build into the same dir — bridge must recompile, not silently reuse stale rlibs.
    let s2 = Command::new(bridge())
        .args(["build", caller_coreir.to_str().unwrap(),
               "--out", exe_out.to_str().unwrap(),
               "--lib", negate_coreir.to_str().unwrap(), "--exe"])
        .status().expect("bridge failed on second build");
    assert!(s2.success(), "second build failed (stale rlib reuse would cause E0460 here)");

    let output = Command::new(&exe_out).output().expect("failed to run exe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "true", "exe after second build must still return 'true'");
}

// ── M1-provider: --dispatch / --provider-crate ───────────────────────────────
//
// tests/fixtures/provider_min/ is a standalone Rust crate (lib.rs) exposing
// one fn, `prov_double(Value) -> Value`, plus a dispatch.toml naming it. It
// stands in for a real external provider (e.g. axis_stdlib) that depends on
// this crate for `Value` and so cannot be depended on BY this crate.

fn fixture_path(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(rel)
}

/// pool[0] = Int(21), node[0] = CCall(prov_double, [pool[0]]) — result 42.
fn make_prov_double_bundle() -> CoreBundle {
    make_ccall_bundle_named(
        sha256_bytes(b"prov_double"),
        "prov_double",
        vec![ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(21) }],
        vec![NodeRef::Pool(0)],
    )
}

#[test]
fn test_provider_crate_dispatch_runs() {
    let dir = TempDir::new().unwrap();
    let bundle = make_prov_double_bundle();
    let fixture = write_05_bundle(&dir, "prov_call.coreir", &bundle);
    let exe_out = dir.path().join("prov_call_exe");

    let dispatch_toml = fixture_path("provider_min/dispatch.toml");
    let provider_src  = fixture_path("provider_min/lib.rs");

    let output = Command::new(bridge())
        .args([
            "build", fixture.to_str().unwrap(),
            "--out", exe_out.to_str().unwrap(),
            "--exe",
            "--dispatch", dispatch_toml.to_str().unwrap(),
            "--provider-crate", &format!("provider_min={}", provider_src.display()),
        ])
        .output()
        .expect("bridge failed to run");
    assert!(
        output.status.success(),
        "provider-crate/dispatch build failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe_out).output().expect("failed to run exe");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert_eq!(stdout.trim(), "42", "expected prov_double(21) == 42, got: {:?}", stdout);
}

// ── (b) --dispatch entry colliding with a static bridge built-in: hard error ──

#[test]
fn test_dispatch_conflict_with_static_name_fails() {
    let dir = TempDir::new().unwrap();
    let bundle = make_int_bundle(1);
    let fixture = write_05_bundle(&dir, "int_fn.coreir", &bundle);
    let out = dir.path().join("int_fn");

    let conflict_toml = dir.path().join("conflict.toml");
    std::fs::write(&conflict_toml, "[[fn]]\nname = \"int_add\"\npath = \"whatever::int_add\"\n")
        .unwrap();

    let output = Command::new(bridge())
        .args([
            "build", fixture.to_str().unwrap(),
            "--out", out.to_str().unwrap(),
            "--dispatch", conflict_toml.to_str().unwrap(),
        ])
        .output()
        .expect("bridge failed to run");

    assert!(!output.status.success(), "build should fail when a dispatch entry conflicts with a static name");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("dispatch conflict: int_add"),
        "expected 'dispatch conflict: int_add' in stderr, got:\n{}", stderr
    );
}

// ── (c) no --dispatch/--provider-crate flags: generated glue unchanged ────────

#[test]
fn test_no_flag_build_glue_unchanged() {
    let dir = TempDir::new().unwrap();
    let bundle = make_int_bundle(42);
    let fixture = write_05_bundle(&dir, "int_fn.coreir", &bundle);
    let out = dir.path().join("int_fn");

    let status = Command::new(bridge())
        .args(["build", fixture.to_str().unwrap(), "--out", out.to_str().unwrap()])
        .status()
        .expect("bridge failed to run");
    assert!(status.success(), "build failed");

    let generated = std::fs::read_to_string(dir.path().join("generated_lib.rs"))
        .expect("generated_lib.rs not written");

    // Same bundle, run straight through the plain (no-overlay) public API —
    // the two must match exactly when no --dispatch/--provider-crate flag is
    // given, proving the M1-provider addition changed nothing on that path.
    let expected = axis_codegen_bridge::emit::rust_05::emit_rust_lib_from_bundle(
        &bundle,
        "int_fn",
        &std::collections::HashMap::new(),
        &std::collections::HashMap::new(),
        &std::collections::HashSet::new(),
        &std::collections::HashSet::new(),
    )
    .expect("direct emit failed");

    assert_eq!(
        generated, expected,
        "no-flag build's generated glue must be byte-identical to emit_rust_lib_from_bundle's output"
    );
}

// ── A pure node written in two arms runs only in them ─────────────────────────────────────────────────────
//
// Once GUARDED_SHARED_PURE_V1: node interning made the same pure expression one node wherever it appeared,
// so a partial operation (here `str_char_code` past the end of the string) used inside two `if`s was one
// node the bridge had to guard by inference. In Core IR 0.6 the compiler never shares a node across blocks,
// so each arm writes its own, in its own block, and it runs only there.
//
//   [ node1 = str_char_code("ab", 5); CEnd ] [ CEnd 0 ] node5 = if false ..   -- a Defect if it ever runs
//   [ CEnd 0 ] [ node9 = str_char_code("ab", 5); CEnd ] node11 = if true ..
//   node12 = int_add(node5, node11)             -- 0, and neither str_char_code runs
#[test]
fn test_pure_node_shared_by_two_guards_is_not_hoisted() {
    let dir = TempDir::new().unwrap();
    let bundle = CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![
            ConstantPoolEntry {
                def_hash: axis_codegen_bridge::core_ir_05::text_type_hash(),
                payload: axis_codegen_bridge::core_ir_05::encode_text_payload("ab"),
            },
            ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(5) },
            ConstantPoolEntry { def_hash: bool_type_hash(), payload: encode_bool_payload(false) },
            ConstantPoolEntry { def_hash: bool_type_hash(), payload: encode_bool_payload(true) },
            ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(0) },
        ],
        nodes: {
            let scc = || Node::CCall {
                target_identity: sha256_bytes(b"str_char_code"),
                target_name: "str_char_code".to_string(),
                args: vec![NodeRef::Pool(0), NodeRef::Pool(1)],
            };
            vec![
                Node::CBlock, scc(), Node::CEnd { value: NodeRef::Node(1) },
                Node::CBlock, Node::CEnd { value: NodeRef::Pool(4) },
                Node::CIf { cond: NodeRef::Pool(2), then_: NodeRef::Node(2), else_: NodeRef::Node(4) },
                Node::CBlock, Node::CEnd { value: NodeRef::Pool(4) },
                Node::CBlock, scc(), Node::CEnd { value: NodeRef::Node(9) },
                Node::CIf { cond: NodeRef::Pool(3), then_: NodeRef::Node(7), else_: NodeRef::Node(10) },
                Node::CCall {
                    target_identity: sha256_bytes(b"int_add"),
                    target_name: "int_add".to_string(),
                    args: vec![NodeRef::Node(5), NodeRef::Node(11)],
                },
            ]
        },
        result: NodeRef::Node(12),
    };
    let fixture = write_05_bundle(&dir, "shared_guard.coreir", &bundle);
    // The registry declares str_char_code pure: a panic in it would be a Defect (exit 4), not an Unknown.
    let reg = dir.path().join("pure.axreg");
    let ident: String = sha256_bytes(b"str_char_code").iter().map(|b| format!("{:02x}", b)).collect();
    std::fs::write(&reg, format!(
        "registry shared-guard-test 0.5\n\nfn str_char_code\n  identity 0x{ident}\n  kind leaf\n  in (Text, Int)\n  \
         out Int\n  effect pure\n  deterministic true\n  idempotent true\nend\n"
    )).unwrap();
    let out = dir.path().join("shared_guard");
    let status = Command::new(bridge())
        .args(["build", fixture.to_str().unwrap(), "--out", out.to_str().unwrap(), "--exe",
               "--reg", reg.to_str().unwrap()])
        .status()
        .expect("bridge failed to run");
    assert!(status.success(), "a pure node written in two CIf arms is a legal bundle and must build");
    let run = Command::new(&out).output().expect("failed to run exe");
    assert_eq!(
        run.status.code(),
        Some(0),
        "the str_char_code written in an arm ran although its arm was not taken; stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "0");
}

// ── BRIDGE_BUILD_PARALLEL_CACHE_V1: the rustc cache must never hand back stale code ──

fn cached_build(cache: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new(bridge())
        .env("AX_BUILD_CACHE", cache)
        .args(args)
        .output()
        .expect("bridge failed to run");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "build failed:\n{}", stderr);
    stderr
}

fn run_stdout(exe: &std::path::Path) -> String {
    let run = Command::new(exe).output().expect("failed to run exe");
    String::from_utf8_lossy(&run.stdout).trim().to_string()
}

/// Same input twice hits; a changed .coreir at the SAME path and --out misses.
#[test]
fn test_build_cache_hits_then_misses_on_changed_bundle() {
    let dir = TempDir::new().unwrap();
    let cache = dir.path().join("cache");
    let exe = dir.path().join("int_fn");
    let input = write_05_bundle(&dir, "int_fn.coreir", &make_int_bundle(7));
    let args = ["build", input.to_str().unwrap(), "--out", exe.to_str().unwrap(), "--exe"];

    let first = cached_build(&cache, &args);
    assert!(first.contains("rustc: 1 compiled, 0 cached"), "{}", first);
    assert_eq!(run_stdout(&exe), "7");

    let second = cached_build(&cache, &args);
    assert!(second.contains("rustc: 0 compiled, 1 cached"), "{}", second);
    assert_eq!(run_stdout(&exe), "7");

    write_05_bundle(&dir, "int_fn.coreir", &make_int_bundle(8));
    let third = cached_build(&cache, &args);
    assert!(third.contains("rustc: 1 compiled, 0 cached"), "{}", third);
    assert_eq!(run_stdout(&exe), "8", "cache handed back the stale bundle");
}

/// A provider crate's `mod` sub-file is not on the command line — only rustc's
/// dep-info knows it. Editing it alone must recompile the provider crate AND
/// the glue that links it (its --extern is keyed by the rlib's content).
#[test]
fn test_build_cache_tracks_provider_crate_submodule() {
    let dir = TempDir::new().unwrap();
    let cache = dir.path().join("cache");
    let prov = dir.path().join("prov");
    std::fs::create_dir_all(&prov).unwrap();
    std::fs::write(prov.join("lib.rs"),
        "use axis_codegen_bridge::runtime::value::Value;\nmod factor;\n\
         pub fn prov_double(v: Value) -> Value {\n\
             match v { Value::Int(n) => Value::Int(n * factor::F), o => panic!(\"{:?}\", o) }\n}\n").unwrap();
    std::fs::write(prov.join("factor.rs"), "pub const F: i64 = 2;\n").unwrap();
    let input = write_05_bundle(&dir, "prov_call.coreir", &make_prov_double_bundle());
    let exe = dir.path().join("prov_call_exe");
    let dispatch = fixture_path("provider_min/dispatch.toml");
    let provider = format!("provider_min={}", prov.join("lib.rs").display());
    let args = ["build", input.to_str().unwrap(), "--out", exe.to_str().unwrap(), "--exe",
                "--dispatch", dispatch.to_str().unwrap(), "--provider-crate", &provider];

    cached_build(&cache, &args);
    assert_eq!(run_stdout(&exe), "42");

    let warm = cached_build(&cache, &args);
    assert_eq!(warm.matches("compiled, 0 cached").count(), 0, "{}", warm);
    assert_eq!(run_stdout(&exe), "42");

    std::fs::write(prov.join("factor.rs"), "pub const F: i64 = 3;\n").unwrap();
    let edited = cached_build(&cache, &args);
    assert_eq!(edited.matches("rustc: 1 compiled, 0 cached").count(), 2, "{}", edited);
    assert_eq!(run_stdout(&exe), "63", "cache missed the edited provider sub-module");
}

// ── Core IR 0.6: every node runs where it was written ───────────────────────────────────────────────────
//
// Where a node runs is in the IR (axis-lang-lab core_ir_spec/axis-core-ir-0.6.md): an `if`'s arms are blocks,
// `CBlock` .. `CEnd { value }`, written directly before the `CIf`, and a node runs exactly when every block around
// it runs. The bridge emits the blocks as they are and infers nothing. These are the effect-order tests of
// EFFECT_ORDER_V1 and the r178 workaround (both deleted), the spec's eight worked examples, and its mutants.

fn eo_int(v: i64) -> ConstantPoolEntry {
    ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(v) }
}

fn eo_bool(v: bool) -> ConstantPoolEntry {
    ConstantPoolEntry { def_hash: bool_type_hash(), payload: encode_bool_payload(v) }
}

fn eo_text(s: &str) -> ConstantPoolEntry {
    ConstantPoolEntry {
        def_hash: axis_codegen_bridge::core_ir_05::text_type_hash(),
        payload: axis_codegen_bridge::core_ir_05::encode_text_payload(s),
    }
}

/// A bundle written in source order, the way the lab lowers it.
struct Eo {
    pool: Vec<ConstantPoolEntry>,
    nodes: Vec<Node>,
}

impl Eo {
    fn new(pool: Vec<ConstantPoolEntry>) -> Eo {
        Eo { pool, nodes: Vec::new() }
    }

    fn push(&mut self, n: Node) -> NodeRef {
        self.nodes.push(n);
        NodeRef::Node(self.nodes.len() as u32 - 1)
    }

    fn call(&mut self, name: &str, args: Vec<NodeRef>) -> NodeRef {
        self.push(Node::CCall { target_identity: sha256_bytes(name.as_bytes()), target_name: name.to_string(), args })
    }

    /// One block: CBlock, whatever `body` writes, CEnd { the value it returns }.
    fn block(&mut self, body: impl FnOnce(&mut Eo) -> NodeRef) -> NodeRef {
        self.push(Node::CBlock);
        let v = body(self);
        self.push(Node::CEnd { value: v })
    }

    /// `if cond { then } else { else }`: two arm blocks, then the CIf.
    fn if_(&mut self, cond: NodeRef, then_: impl FnOnce(&mut Eo) -> NodeRef, else_: impl FnOnce(&mut Eo) -> NodeRef) -> NodeRef {
        let t = self.block(then_);
        let e = self.block(else_);
        self.push(Node::CIf { cond, then_: t, else_: e })
    }

    fn bundle(self, result: NodeRef) -> CoreBundle {
        CoreBundle { version: "0.6".to_string(), constant_pool: self.pool, nodes: self.nodes, result }
    }
}

const EO_PURE: &[&str] = &["tuple_field", "int_eq", "int_lt", "int_div", "int_to_str", "str_eq", "str_concat"];

/// Build `coreir` as an exe -- with a registry declaring `pure` pure + deterministic -- and run it with `argv`.
/// Ok((stdout, generated Rust)) on a clean run, Err(build stderr) when the build fails.
fn eo_run_file(coreir: &std::path::Path, argv: &[&str], pure: &[&str]) -> Result<(String, String), String> {
    let dir = TempDir::new().unwrap();
    let mut reg = "registry effect-order-test 0.5\n\n".to_string();
    for f in pure {
        let ident: String = sha256_bytes(f.as_bytes()).iter().map(|b| format!("{:02x}", b)).collect();
        reg.push_str(&format!(
            "fn {f}\n  identity 0x{ident}\n  kind leaf\n  in (Value)\n  out Value\n  effect pure\n  \
             deterministic true\n  idempotent true\nend\n\n"
        ));
    }
    let reg_path = dir.path().join("pure.axreg");
    std::fs::write(&reg_path, reg).unwrap();
    let out = dir.path().join("eo");
    let build = Command::new(bridge())
        .args(["build", coreir.to_str().unwrap(), "--out", out.to_str().unwrap(), "--exe",
               "--reg", reg_path.to_str().unwrap()])
        .output()
        .expect("bridge failed to run");
    if !build.status.success() {
        return Err(String::from_utf8_lossy(&build.stderr).into_owned());
    }
    let run = Command::new(&out).args(argv).output().expect("failed to run exe");
    assert_eq!(run.status.code(), Some(0), "{}: stderr: {}", coreir.display(), String::from_utf8_lossy(&run.stderr));
    let generated = std::fs::read_to_string(dir.path().join("generated_lib.rs")).unwrap_or_default();
    Ok((String::from_utf8_lossy(&run.stdout).trim().to_string(), generated))
}

fn eo_run(name: &str, bundle: &CoreBundle, pure: &[&str]) -> Result<(String, String), String> {
    let dir = TempDir::new().unwrap();
    let fixture = write_05_bundle(&dir, &format!("{name}.coreir"), bundle);
    eo_run_file(&fixture, &[], pure)
}

fn eo_fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/effect_order").join(name)
}

fn ex_fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core_ir_06").join(name)
}

/// The generated Rust carries none of the inference's machinery.
fn assert_no_inference(generated: &str) {
    assert!(!generated.contains("_g: Option<Value>") && !generated.contains("EFFECT_ORDER"), "{generated}");
}

#[test]
fn test_effect_order_two_shapes_from_one_source_file() {
    // tests/fixtures/effect_order/two_shapes.m1, compiled by the lab. Shape 1 (written in a branch whose
    // condition is false) does not write slot 8; shape 2 (a top-level let only a branch uses, condition false)
    // writes slot 16 -- each where it was written.
    let (out, generated) = eo_run_file(&eo_fixture("two_shapes.coreir"), &[], EO_PURE).unwrap();
    assert_eq!(out, "0\n5", "slot 8 (written in a branch not taken), slot 16 (a top-level let)");
    assert_no_inference(&generated);
}

#[test]
fn test_effect_order_gr_cf_run_cursor_get_before_close() {
    // the original EFFECT_ORDER_V1 case, from source: cursor_get (a top-level let), cursor_close, then the `if`
    for (err, want) in [("", "Kabc"), ("bad", "Xbad")] {
        let (out, _) = eo_run_file(&eo_fixture("gr_cf_run_shape.coreir"), &[err], EO_PURE).unwrap();
        assert_eq!(out, want, "err={err:?}");
    }
}

/// nested_if_effect_hoist (worked example 1), as the lab lowers it:
///   let p = tuple_field(mem_reserve_raw(64), 0); let z = mem_read_int_raw(p, 0)
///   let f = int_eq(z, 0); let g = int_eq(z, 1); let h = int_lt(z, 5)
///   let _x = if <c1> { mem_write_int_raw(p, 8, 1) } else { if <c2> { io_print("") } else { io_print("") } }
///   let _y = if <c2> { io_print("") } else { io_print("") }
///   io_println(int_to_str(mem_read_int_raw(p, 8)))
/// with `c1 = bool_and(f, g)`, `c2 = bool_and(f, h)` (each an `if`; _y's c2 is its own node -- the one in _x's
/// else-arm is in a closed block), or -- the control -- `c1 = g`, `c2 = h`. z is 0, so g is false: prints 0.
fn eo_repro(bool_and: bool) -> CoreBundle {
    let p = NodeRef::Pool;
    let mut e = Eo::new(vec![eo_int(64), eo_int(0), eo_int(1), eo_int(5), eo_bool(false), eo_int(8), eo_text("")]);
    let r = e.call("mem_reserve_raw", vec![p(0)]);
    let ptr = e.call("tuple_field", vec![r, p(1)]);
    let z = e.call("mem_read_int_raw", vec![ptr.clone(), p(1)]);
    let f = e.call("int_eq", vec![z.clone(), p(1)]);
    let g = e.call("int_eq", vec![z.clone(), p(2)]);
    let h = e.call("int_lt", vec![z, p(3)]);
    let and = |e: &mut Eo, a: NodeRef, b: NodeRef| -> NodeRef {
        if bool_and { e.if_(a, |_| b, |_| p(4)) } else { b }
    };
    let c1 = and(&mut e, f.clone(), g);
    let (ptr2, f2, h2) = (ptr.clone(), f.clone(), h.clone());
    e.if_(
        c1,
        |e| e.call("mem_write_int_raw", vec![ptr2, p(5), p(2)]),
        |e| {
            let c2 = and(e, f2, h2);
            e.if_(c2, |e| e.call("io_print", vec![p(6)]), |e| e.call("io_print", vec![p(6)]))
        },
    );
    let c2 = and(&mut e, f, h);
    e.if_(c2, |e| e.call("io_print", vec![p(6)]), |e| e.call("io_print", vec![p(6)]));
    let rd = e.call("mem_read_int_raw", vec![ptr, p(5)]);
    let s = e.call("int_to_str", vec![rd]);
    let out = e.call("io_println", vec![s]);
    e.bundle(out)
}

#[test]
fn test_effect_order_nested_if_effect_hoist_repro_prints_0() {
    for (name, bool_and) in [("eo_repro", true), ("eo_control", false)] {
        let (out, generated) = eo_run(name, &eo_repro(bool_and), EO_PURE).unwrap();
        assert_eq!(out, "0", "{name}");
        assert!(generated.contains("mem_write_int_raw"), "{name}: generated source not found");
        assert_no_inference(&generated);
    }
}

#[test]
fn test_effect_order_nested_if_effect_hoist_repro_moved_write_keeps_its_condition() {
    // without the registry every call counts as an effect -- which no longer changes where anything runs
    assert_eq!(eo_run("eo_repro_moved", &eo_repro(true), &[]).unwrap().0, "0");
    assert_eq!(eo_run("eo_control_moved", &eo_repro(false), &[]).unwrap().0, "0");
}

/// `let h = mem_write_int_raw(p, 8, 9); if c { mem_write_int_raw(p, 8, int_div(7, x)) } else { bool_and(true, h) }`
/// with `c = int_eq(x, 1)`: the write and its input `int_div(7, x)` -- which divides by zero for x = 0 -- are in
/// the then-arm and run only when c holds. h is a top-level let and always runs first: slot 8 ends 7 when c holds,
/// 9 when it doesn't.
fn eo_moved_write(x: i64) -> CoreBundle {
    let p = NodeRef::Pool;
    let mut e = Eo::new(vec![eo_int(64), eo_int(0), eo_int(1), eo_int(7), eo_int(x), eo_int(8), eo_bool(true), eo_int(9)]);
    let r = e.call("mem_reserve_raw", vec![p(0)]);
    let ptr = e.call("tuple_field", vec![r, p(1)]);
    let c = e.call("int_eq", vec![p(4), p(2)]);
    let h = e.call("mem_write_int_raw", vec![ptr.clone(), p(5), p(7)]);
    let ptr2 = ptr.clone();
    let k = e.if_(
        c,
        |e| {
            let d = e.call("int_div", vec![p(3), p(4)]);
            e.call("mem_write_int_raw", vec![ptr2, p(5), d])
        },
        |e| e.if_(p(6), |_| h, |_| p(1)),
    );
    let rd = e.call("mem_read_int_raw", vec![ptr, p(5)]);
    let s = e.call("int_to_str", vec![rd]);
    let out = e.call("io_println", vec![s]);
    let r = e.call("seq", vec![k, out]);
    e.bundle(r)
}

#[test]
fn test_effect_order_moved_write_runs_only_when_its_condition_holds() {
    let (out, generated) = eo_run("eo_moved_t", &eo_moved_write(1), EO_PURE).unwrap();
    assert_eq!(out, "7", "condition true: the write runs, after h");
    assert_no_inference(&generated);
    assert_eq!(eo_run("eo_moved_f", &eo_moved_write(0), EO_PURE).unwrap().0, "9",
               "condition false: neither the write nor its input runs");
}

#[test]
fn test_effect_order_toplevel_let_with_an_outer_effect_between_stays_unconditional() {
    // `let c = false; let w = mem_write_int_raw(p, 8, 7); io_println(read(p, 8)); if c { w } else { 0 }`:
    // w is a top-level let, in no block, and runs where it was written, before the println.
    let p = NodeRef::Pool;
    let mut e = Eo::new(vec![eo_int(64), eo_int(0), eo_int(1), eo_int(7), eo_int(8)]);
    let r = e.call("mem_reserve_raw", vec![p(0)]);
    let ptr = e.call("tuple_field", vec![r, p(1)]);
    let c = e.call("int_eq", vec![p(1), p(2)]);
    let w = e.call("mem_write_int_raw", vec![ptr.clone(), p(4), p(3)]);
    let rd = e.call("mem_read_int_raw", vec![ptr, p(4)]);
    let s = e.call("int_to_str", vec![rd]);
    let out = e.call("io_println", vec![s]);
    let k = e.if_(c, |_| w, |_| p(1));
    let res = e.call("seq", vec![k, out]);
    let (out, generated) = eo_run("eo_toplevel", &e.bundle(res), EO_PURE).unwrap();
    assert_eq!(out, "7");
    assert_no_inference(&generated);
}

/// Two levels: the write is in the then-arm of `if c2`, which is in the else-arm of `if c1`: it runs only when
/// !c1 && c2. An earlier top-level write of 9 to the same slot sits in the inner else-arm's `bool_and`.
fn eo_moved_two_levels(c1: bool, c2: bool) -> CoreBundle {
    let p = NodeRef::Pool;
    let mut e = Eo::new(vec![eo_int(64), eo_int(0), eo_int(7), eo_int(8), eo_bool(c1), eo_bool(c2), eo_bool(true), eo_int(9)]);
    let r = e.call("mem_reserve_raw", vec![p(0)]);
    let ptr = e.call("tuple_field", vec![r, p(1)]);
    let h = e.call("mem_write_int_raw", vec![ptr.clone(), p(3), p(7)]);
    let ptr2 = ptr.clone();
    let k = e.if_(
        p(4),
        |_| p(1),
        |e| e.if_(p(5), |e| e.call("mem_write_int_raw", vec![ptr2, p(3), p(2)]), |e| e.if_(p(6), |_| h, |_| p(1))),
    );
    let rd = e.call("mem_read_int_raw", vec![ptr, p(3)]);
    let s = e.call("int_to_str", vec![rd]);
    let out = e.call("io_println", vec![s]);
    let res = e.call("seq", vec![k, out]);
    e.bundle(res)
}

#[test]
fn test_effect_order_moved_write_keeps_every_enclosing_condition() {
    for (c1, c2, want) in [(false, true, "7"), (true, true, "9"), (false, false, "9"), (true, false, "9")] {
        assert_eq!(eo_run("eo_two", &eo_moved_two_levels(c1, c2), EO_PURE).unwrap().0, want, "c1={c1} c2={c2}");
    }
}

#[test]
fn test_effect_order_contradictory_case_is_a_build_error() {
    // The shape EFFECT_ORDER_V1 could not place: a node written inside an arm (the `if` j, in the inner if's
    // else-arm) and used again after the outer `if`. Core IR 0.6 has no such program -- the compiler never shares
    // a node out of a closed block -- and the bridge refuses the bundle, naming the nodes.
    let p = NodeRef::Pool;
    let mut e = Eo::new(vec![eo_int(64), eo_int(0), eo_int(1), eo_int(7), eo_int(8), eo_bool(true)]);
    let r = e.call("mem_reserve_raw", vec![p(0)]);
    let ptr = e.call("tuple_field", vec![r, p(1)]);
    let h = e.call("mem_read_int_raw", vec![ptr.clone(), p(4)]);
    let mut j = None;
    let k = e.if_(
        p(5),
        |e| {
            let c2 = e.call("int_eq", vec![p(2), p(2)]);
            e.if_(c2, |e| e.call("mem_write_int_raw", vec![ptr, p(4), p(3)]), |e| {
                let jj = e.if_(p(5), |_| h, |_| p(1));
                j = Some(jj.clone());
                jj
            })
        },
        |_| p(1),
    );
    let res = e.call("seq", vec![j.unwrap(), k]);
    let err = eo_run("eo_refused", &e.bundle(res), EO_PURE).expect_err("a use out of a closed arm must fail the build");
    assert!(err.contains("block rule 4: node 20 uses node 13, which is inside the then arm of if node 19"), "{err}");
}

#[test]
fn test_effect_order_toplevel_let_keeps_old_placement_past_a_pure_if() {
    // `let w = mem_write_int_raw(p, 8, 7); let b = bool_and(true, int_eq(1, 1)); if c { w } else { 0 }` with c
    // false and computed after w (nv_n_step's shape): w is a top-level let and runs.
    let p = NodeRef::Pool;
    let mut e = Eo::new(vec![eo_int(64), eo_int(0), eo_int(1), eo_int(7), eo_int(8), eo_bool(true), eo_bool(false)]);
    let r = e.call("mem_reserve_raw", vec![p(0)]);
    let ptr = e.call("tuple_field", vec![r, p(1)]);
    let w = e.call("mem_write_int_raw", vec![ptr.clone(), p(4), p(3)]);
    let b = e.if_(p(5), |e| e.call("int_eq", vec![p(2), p(2)]), |_| p(6));
    let c = e.call("int_eq", vec![p(1), p(2)]);
    e.if_(c, |_| w, |_| p(1));
    let rd = e.call("mem_read_int_raw", vec![ptr, p(4)]);
    let s = e.call("int_to_str", vec![rd]);
    let out = e.call("io_println", vec![s]);
    let res = e.call("seq", vec![b, out]);
    assert_eq!(eo_run("eo_toplevel_pure_if", &e.bundle(res), EO_PURE).unwrap().0, "7");
}

#[test]
fn test_effect_order_toplevel_let_under_a_condition_chain_stays_unconditional() {
    // gfx3gate's g3_gate1: `let c1 = ..; let w = g3_check(..); bool_and(x, bool_and(c1, w))` with x false. w is a
    // top-level let and runs, although no arm that names it is taken.
    let p = NodeRef::Pool;
    let mut e = Eo::new(vec![eo_int(64), eo_int(0), eo_int(1), eo_int(7), eo_int(8), eo_bool(true), eo_bool(false), eo_int(2)]);
    let r = e.call("mem_reserve_raw", vec![p(0)]);
    let ptr = e.call("tuple_field", vec![r, p(1)]);
    let c1 = e.call("int_eq", vec![p(2), p(2)]);
    let w = e.call("mem_write_int_raw", vec![ptr.clone(), p(4), p(3)]);
    let x = e.if_(p(5), |e| e.call("int_eq", vec![p(2), p(7)]), |_| p(6));
    let k = e.if_(x, |e| e.if_(c1, |_| w, |_| p(6)), |_| p(6));
    let rd = e.call("mem_read_int_raw", vec![ptr, p(4)]);
    let s = e.call("int_to_str", vec![rd]);
    let out = e.call("io_println", vec![s]);
    let res = e.call("seq", vec![k, out]);
    assert_eq!(eo_run("eo_cond_chain", &e.bundle(res), EO_PURE).unwrap().0, "7");
}

// ── the spec's worked examples, run ─────────────────────────────────────────────────────────────────────
//
// 1-6: tests/fixtures/core_ir_06/ex*.m1 (copies of axis-lang-lab examples/surface-m1/core_ir_06), compiled by the
// lab. 7 (one-armed if) and 8 (plain grouping block): M1 writes neither, so they are built here.

fn ex_run(name: &str, argv: &[&str]) -> String {
    let (out, generated) = eo_run_file(&ex_fixture(&format!("{name}.coreir")), argv, EO_PURE).unwrap();
    assert_no_inference(&generated);
    out
}

#[test]
fn test_core_ir_06_example_1_nested_if_effect_hoist_prints_0() {
    assert_eq!(ex_run("ex1_nested_if_effect_hoist", &[]), "0");
}

#[test]
fn test_core_ir_06_example_2_toplevel_let_used_in_a_branch_runs() {
    assert_eq!(ex_run("ex2_toplevel_let_used_in_branch", &[]), "5");
}

#[test]
fn test_core_ir_06_example_3_two_deep_ifs() {
    for (c1, c2, want) in [("y", "y", "w"), ("y", "n", "p-inner"), ("n", "y", "p-outer"), ("n", "n", "p-outer")] {
        assert_eq!(ex_run("ex3_two_deep", &[c1, c2]), format!("{want}\nend"), "c1={c1} c2={c2}");
    }
}

#[test]
fn test_core_ir_06_example_4_orphan_effect_runs_with_its_arm() {
    assert_eq!(ex_run("ex4_orphan_effect", &["y"]), "top\nin-then\nx");
    assert_eq!(ex_run("ex4_orphan_effect", &["n"]), "top\ny");
}

#[test]
fn test_core_ir_06_example_5_av_admit_partial_call_stays_in_its_arm() {
    // int_div(100, 0) would be a Defect (exit 4): it is in the arm `got` selects, and got is false
    assert_eq!(ex_run("ex5_av_admit", &[]), "empty\ndone");
}

#[test]
fn test_core_ir_06_example_6_nv_n_step_lets_run_every_call() {
    assert_eq!(ex_run("ex6_nv_n_step", &[]), "1");
}

#[test]
fn test_core_ir_06_example_7_one_armed_if() {
    // if c { io_println("in") } (else = pool Unit); io_println("end")
    for (c, want) in [(true, "in\nend"), (false, "end")] {
        let p = NodeRef::Pool;
        let unit = ConstantPoolEntry { def_hash: unit_type_hash(), payload: vec![] };
        let mut e = Eo::new(vec![eo_bool(c), eo_text("in"), eo_text("end"), unit]);
        let t = e.block(|e| {
            let _ = e.call("io_println", vec![p(1)]);
            p(3)
        });
        let k = e.push(Node::CIf { cond: p(0), then_: t, else_: p(3) });
        let end = e.call("io_println", vec![p(2)]);
        let res = e.call("seq", vec![k, end]);
        let (out, generated) = eo_run("ex7", &e.bundle(res), EO_PURE).unwrap();
        assert_eq!(out, want, "c={c}");
        assert!(generated.contains("} else { Value::Unit };"), "{generated}");
    }
}

#[test]
fn test_core_ir_06_example_8_plain_grouping_block_always_runs() {
    // [ a = int_add(2, 3); b = int_mul(a, 10); CEnd b ]; io_println(int_to_str(<the block>))
    let p = NodeRef::Pool;
    let mut e = Eo::new(vec![eo_int(2), eo_int(3), eo_int(10)]);
    let blk = e.block(|e| {
        let a = e.call("int_add", vec![p(0), p(1)]);
        e.call("int_mul", vec![a, p(2)])
    });
    let s = e.call("int_to_str", vec![blk]);
    let out = e.call("io_println", vec![s]);
    let (stdout, generated) = eo_run("ex8", &e.bundle(out), EO_PURE).unwrap();
    assert_eq!(stdout, "50");
    assert!(generated.contains("let node_3: Value = {"), "{generated}");
}

// ── the spec's mutants: each bundle is refused by the bridge's loader, naming the node ──────────────────

fn refused(name: &str, bytes: Vec<u8>) -> String {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join(format!("{name}.coreir"));
    std::fs::write(&path, bytes).unwrap();
    let out = dir.path().join(name);
    let build = Command::new(bridge())
        .args(["build", path.to_str().unwrap(), "--out", out.to_str().unwrap(), "--exe"])
        .output()
        .expect("bridge failed to run");
    assert!(!build.status.success(), "{name}: the bridge built a bundle it must refuse");
    String::from_utf8_lossy(&build.stderr).into_owned()
}

/// 0 c; [ 2 w ] [ 5 p ]; 7 if 0 then 3 else 6  (+ `extra` nodes, result `result`)
fn two_arms(extra: Vec<Node>, result: u32) -> CoreBundle {
    let n = NodeRef::Node;
    let p = NodeRef::Pool;
    let call = |args| Node::CCall { target_identity: sha256_bytes(b"int_to_str"), target_name: "int_to_str".to_string(), args };
    let mut nodes = vec![
        Node::CCall { target_identity: sha256_bytes(b"int_eq"), target_name: "int_eq".to_string(), args: vec![p(0), p(0)] },
        Node::CBlock, call(vec![p(0)]), Node::CEnd { value: n(2) },
        Node::CBlock, call(vec![p(0)]), Node::CEnd { value: n(5) },
        Node::CIf { cond: n(0), then_: n(3), else_: n(6) },
    ];
    nodes.extend(extra);
    CoreBundle { version: "0.6".to_string(), constant_pool: vec![eo_int(1)], nodes, result: n(result) }
}

fn mutant(nodes_edit: impl FnOnce(&mut Vec<Node>), result: u32) -> Vec<u8> {
    let mut b = two_arms(vec![], 7);
    nodes_edit(&mut b.nodes);
    b.result = NodeRef::Node(result);
    create_core_bundle_05(&b)
}

#[test]
fn test_core_ir_06_mutant_cend_with_no_value_is_refused() {
    // header, an empty pool, two nodes: CBlock, then a CEnd tag with nothing after it
    let bytes = vec![b'A', b'X', b'C', b'I', 0, 6, 0, 2, 3, 4];
    let err = refused("no_value", bytes);
    assert!(err.contains("axbi: truncated"), "{err}");
}

#[test]
fn test_core_ir_06_mutant_arm_cend_referenced_by_two_ifs_is_refused() {
    let err = refused("two_ifs", mutant(|n| n.push(Node::CIf { cond: NodeRef::Node(0), then_: NodeRef::Node(3), else_: NodeRef::Node(6) }), 8));
    assert!(err.contains("block rule 3: node 3 closes an arm of if node 7, and if node 8 references it too"), "{err}");
}

#[test]
fn test_core_ir_06_mutant_arm_block_not_directly_before_its_if_is_refused() {
    let err = refused("not_before", mutant(|n| {
        let k = n.pop().unwrap();
        n.push(Node::CCall { target_identity: sha256_bytes(b"int_to_str"), target_name: "int_to_str".to_string(), args: vec![NodeRef::Pool(0)] });
        n.push(k);
    }, 8));
    assert!(err.contains("block rule 2: if node 8"), "{err}");
}

#[test]
fn test_core_ir_06_mutant_use_of_a_node_inside_an_arm_from_outside_is_refused() {
    let err = refused("inside_arm", mutant(|n| n.push(Node::CCall {
        target_identity: sha256_bytes(b"io_println"), target_name: "io_println".to_string(), args: vec![NodeRef::Node(2)],
    }), 8));
    assert!(err.contains("block rule 4: node 8 uses node 2, which is inside the then arm of if node 7"), "{err}");
}

#[test]
fn test_core_ir_06_mutant_unbalanced_blocks_are_refused() {
    let err = refused("unclosed", mutant(|n| n.push(Node::CBlock), 7));
    assert!(err.contains("block rule 1: the block opened at node 8 is never closed"), "{err}");
    let err = refused("stray_end", mutant(|n| n.push(Node::CEnd { value: NodeRef::Node(7) }), 7));
    assert!(err.contains("block rule 1: node 8 is a CEnd with no open block"), "{err}");
}

#[test]
fn test_core_ir_06_mutant_share_across_sibling_blocks_is_refused() {
    // the else-block reuses the then-block's node, as a compiler sharing across blocks would write it
    let err = refused("sibling_share", mutant(|n| { n[5] = Node::CCall {
        target_identity: sha256_bytes(b"int_to_str"), target_name: "int_to_str".to_string(), args: vec![NodeRef::Node(2)],
    }; }, 7));
    assert!(err.contains("block rule 4: node 5 uses node 2, which is inside the then arm of if node 7"), "{err}");
}

#[test]
fn test_core_ir_06_mutant_a_core_ir_05_bundle_is_refused() {
    let mut bytes = create_core_bundle_05(&two_arms(vec![], 7));
    bytes[5] = 5;
    let err = refused("v05", bytes);
    assert!(err.contains("Core IR 0.5 bundle") && err.contains("recompile from source"), "{err}");
}

#[test]
fn test_core_ir_06_fx_var_counts_every_row() {
    // fx_var_main's shape: `let c = seq(<count>, true)` used only as an if's condition inside a bool_and arm
    assert_eq!(ex_run("fx_var", &[""]), "b n=1");
    assert_eq!(ex_run("fx_var", &["y"]), "a n=1");
}

#[test]
fn test_core_ir_06_fx_split_counts_when_fresh_is_false() {
    // fx_split_main's shape: `let sk = if <x> { <counter> } else { true }` used only as `bool_and(fresh, sk)`
    assert_eq!(ex_run("fx_split", &[""]), "skipped=1");
    assert_eq!(ex_run("fx_split", &["y"]), "skipped=1");
}

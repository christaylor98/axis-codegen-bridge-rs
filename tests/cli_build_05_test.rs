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
        result: NodeRef::Node(0),
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
    assert!(stdout.contains("0.5"), "version 0.5 not in inspect output:\n{}", stdout);
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
            // node[0]: side_effect() — §5b call into the linked bundle above.
            Node::CCall {
                target_identity: sha256_bytes(b"side_effect"),
                target_name: "side_effect".to_string(),
                args: vec![],
            },
            // node[1]: if pool[0] { pool[1] (true) } else { node[0] }
            Node::CIf {
                cond:  NodeRef::Pool(0),
                then_: NodeRef::Pool(1),
                else_: NodeRef::Node(0),
            },
        ],
        result: NodeRef::Node(1),
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

// ── ORPHAN_IS_TOP_LEVEL_V1 ───────────────────────────────────────────────────
//
// A node with no data-dependency consumer is unconditional and top-level:
// evaluated once, on every call, in node-index order. Its position relative to
// a `CIf` means nothing — scope is carried by consumer edges and by nothing
// else (see `CoreBundle`'s docs and `compute_branch_paths` in emit/rust_05.rs).
//
// This is the shape a positional tripwire used to refuse: an orphan sitting
// between the `CIf`'s cond anchor and the `CIf`, whose `then_` is a bare pool
// ref. Ordinary AI3 emits it for a top-level discarded effect written after
// the condition's binding — `let c = ...; let a = eff(); let v = if c {...}`.
// The check was unsound in both directions (a statement reorder made the same
// program pass, and the mirror `then_`-node/`else_`-pool shape was never
// tested at all), so it was removed. These tests pin what must hold instead:
// the orphan effect fires EXACTLY ONCE, in BOTH branches, and in program order
// ahead of the `CIf`'s own effects.

/// `fn(cond) -> Bool { let _ = fs_append_text(log, "orphan\n"); if !cond { true } else { false } }`
/// laid out so the orphan lands in the window the old tripwire scanned:
/// node[0] is the `CIf`'s cond anchor, node[1] is the orphan, node[2] is the
/// `CIf` — whose `then_`/`else_` are both bare pool refs.
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
            // node[2]: if node[0] { pool[3] } else { pool[4] } — both arms bare
            // pool refs, so there is no anchor pinning a then/else boundary.
            Node::CIf {
                cond:  NodeRef::Node(0),
                then_: NodeRef::Pool(3),
                else_: NodeRef::Pool(4),
            },
        ],
        result: NodeRef::Node(2),
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

// ── GUARDED_SHARED_PURE_V1: a pure node shared by two guards runs only under a guard ──────────────────────
//
// Node interning makes the same pure expression one node wherever it appears. Used inside two different
// `if`s, its scopes share no arm, and the old common-prefix rule hoisted it to the unconditional top level:
// a partial operation (here `str_char_code` past the end of the string) then crashed on exactly the path
// both guards ruled out. Each guarded scope now gets its own copy, so it only runs where a guard allows.
//
//   node0 = str_char_code("ab", 5)             -- a Defect if it ever runs
//   node1 = if false { node0 } else { 0 }
//   node2 = if true  { 0 } else { node0 }
//   node3 = int_add(node1, node2)              -- 0, and node0 never runs
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
        nodes: vec![
            Node::CCall {
                target_identity: sha256_bytes(b"str_char_code"),
                target_name: "str_char_code".to_string(),
                args: vec![NodeRef::Pool(0), NodeRef::Pool(1)],
            },
            Node::CIf { cond: NodeRef::Pool(2), then_: NodeRef::Node(0), else_: NodeRef::Pool(4) },
            Node::CIf { cond: NodeRef::Pool(3), then_: NodeRef::Pool(4), else_: NodeRef::Node(0) },
            Node::CCall {
                target_identity: sha256_bytes(b"int_add"),
                target_name: "int_add".to_string(),
                args: vec![NodeRef::Node(1), NodeRef::Node(2)],
            },
        ],
        result: NodeRef::Node(3),
    };
    let fixture = write_05_bundle(&dir, "shared_guard.coreir", &bundle);
    // Only a node the registry declares pure + deterministic may be copied into several scopes; without
    // a --reg it counts as effectful and keeps the single common-prefix scope (EFFECT_ORDER_V1).
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
    assert!(status.success(), "a pure node shared by two CIf arms is a legal bundle and must build");
    let run = Command::new(&out).output().expect("failed to run exe");
    assert_eq!(
        run.status.code(),
        Some(0),
        "GUARDED_SHARED_PURE regression: the guarded str_char_code ran unguarded; stderr: {}",
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

// ── EFFECT_ORDER_V1: an effect moved out of its `if` keeps its condition ─────
//
// WORKAROUND (r178, replaced by r179). An effect consumed only inside an arm, with a later effect at an outer
// scope between it and its `if`, has to run before that later effect. It used to be moved out of the arm with
// no condition at all, so an arm's write ran whichever way the `if` went. Now, by node order (= source order):
// an effect after its `if`'s condition was written in that branch and keeps the condition; an effect before
// it ran before the `if` was decided (a top-level `let` only a branch uses) and stays unconditional.

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

fn eo_call(name: &str, args: Vec<NodeRef>) -> Node {
    Node::CCall { target_identity: sha256_bytes(name.as_bytes()), target_name: name.to_string(), args }
}

fn eo_if(cond: NodeRef, then_: NodeRef, else_: NodeRef) -> Node {
    Node::CIf { cond, then_, else_ }
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

#[test]
fn test_effect_order_two_shapes_from_one_source_file() {
    // tests/fixtures/effect_order/two_shapes.m1, compiled by the lab. Shape 1 (written in a branch whose
    // condition is false) must not write slot 8; shape 2 (a top-level let only a branch uses, condition false)
    // must write slot 16 -- both are moved ahead of a later outer effect.
    let (out, generated) = eo_run_file(&eo_fixture("two_shapes.coreir"), &[], EO_PURE).unwrap();
    assert_eq!(out, "0\n5", "slot 8 (guarded write), slot 16 (unconditional write)");
    assert!(generated.contains("let node_6_g: Option<Value>"), "shape 1's write must be guarded:\n{generated}");
    assert!(!generated.contains("node_16_g"), "shape 2's write must stay unconditional:\n{generated}");
}

#[test]
fn test_effect_order_gr_cf_run_cursor_get_before_close() {
    // the original EFFECT_ORDER_V1 case, from source: cursor_get (a top-level let), cursor_close, then the `if`
    for (err, want) in [("", "Kabc"), ("bad", "Xbad")] {
        let (out, _) = eo_run_file(&eo_fixture("gr_cf_run_shape.coreir"), &[err], EO_PURE).unwrap();
        assert_eq!(out, want, "err={err:?}");
    }
}

/// The lab's lowering of
///   let p = tuple_field(mem_reserve_raw(64), 0); let z = mem_read_int_raw(p, 0)
///   let f = int_eq(z, 0); let g = int_eq(z, 1); let h = int_lt(z, 5)
///   let _x = if <c1> { mem_write_int_raw(p, 8, 1) } else { if <c2> { io_print("") } else { io_print("") } }
///   let _y = if <c2> { io_print("") } else { io_print("") }
///   io_println(int_to_str(mem_read_int_raw(p, 8)))
/// with `c1 = bool_and(f, g)`, `c2 = bool_and(f, h)` (each an `if`; c2 is one node shared by `_x` and `_y`),
/// or -- the control -- `c1 = g`, `c2 = h`. z is 0, so g is false and the write must not run: prints 0.
fn eo_repro(bool_and: bool) -> CoreBundle {
    let n = NodeRef::Node;
    let p = NodeRef::Pool;
    let mut nodes = vec![
        eo_call("mem_reserve_raw", vec![p(0)]),
        eo_call("tuple_field", vec![n(0), p(1)]),
        eo_call("mem_read_int_raw", vec![n(1), p(1)]),
        eo_call("int_eq", vec![n(2), p(1)]), // 3: f
        eo_call("int_eq", vec![n(2), p(2)]), // 4: g
        eo_call("int_lt", vec![n(2), p(3)]), // 5: h
    ];
    let (c1, c2) = if bool_and {
        nodes.push(eo_if(n(3), n(4), p(4))); // 6: bool_and(f, g)
        (6, 8)
    } else {
        (4, 5)
    };
    let w = nodes.len() as u32;
    nodes.push(eo_call("mem_write_int_raw", vec![n(1), p(5), p(2)]));
    if bool_and {
        nodes.push(eo_if(n(3), n(5), p(4))); // 8: bool_and(f, h)
    }
    let k = nodes.len() as u32;
    nodes.push(eo_call("io_print", vec![p(6)]));
    nodes.push(eo_call("io_print", vec![p(6)]));
    nodes.push(eo_if(n(c2), n(k), n(k + 1)));
    nodes.push(eo_if(n(c1), n(w), n(k + 2)));
    let k = nodes.len() as u32;
    nodes.push(eo_call("io_print", vec![p(6)]));
    nodes.push(eo_call("io_print", vec![p(6)]));
    nodes.push(eo_if(n(c2), n(k), n(k + 1)));
    let r = nodes.len() as u32;
    nodes.push(eo_call("mem_read_int_raw", vec![n(1), p(5)]));
    nodes.push(eo_call("int_to_str", vec![n(r)]));
    nodes.push(eo_call("io_println", vec![n(r + 1)]));
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![eo_int(64), eo_int(0), eo_int(1), eo_int(5), eo_bool(false), eo_int(8), eo_text("")],
        nodes,
        result: NodeRef::Node(r + 2),
    }
}

#[test]
fn test_effect_order_nested_if_effect_hoist_repro_prints_0() {
    // with the registry: the bool_and ifs are pure, so they are no effect to keep in order and nothing moves
    // (EFFECT_ORDER_V1 part 2) -- no guarded wrapper is emitted at all
    for (name, bool_and) in [("eo_repro", true), ("eo_control", false)] {
        let (out, generated) = eo_run(name, &eo_repro(bool_and), EO_PURE).unwrap();
        assert_eq!(out, "0", "{name}");
        assert!(generated.contains("mem_write_int_raw"), "{name}: generated source not found");
        assert!(!generated.contains("_g: Option<Value>"), "{name}: a write moved out of its if:\n{generated}");
    }
}

#[test]
fn test_effect_order_nested_if_effect_hoist_repro_moved_write_keeps_its_condition() {
    // without the registry every call is an effect, so the then-arm write really must move ahead of the
    // shared `if` -- and must still not run, since its condition is false
    assert_eq!(eo_run("eo_repro_moved", &eo_repro(true), &[]).unwrap().0, "0");
    assert_eq!(eo_run("eo_control_moved", &eo_repro(false), &[]).unwrap().0, "0");
}

/// `if c { mem_write_int_raw(p, 8, int_div(7, x)) } else { j }` with `c = int_eq(x, 1)`, where `j` is
/// `bool_and(true, h)` over `h = mem_write_int_raw(p, 8, 9)` (a let bound earlier, so it lands in j's arm): one
/// shared `if`, used again after the outer `if` -- the shape the lab produces when the same `bool_and` appears
/// in a branch and after it. j sits between the write and its `if`, so the write must run there, after h
/// (h comes first in program order): slot 8 ends 7 when c holds; when it doesn't, neither the write nor its
/// input `int_div(7, x)` -- which divides by zero for x = 0 -- runs, and slot 8 ends 9.
fn eo_moved_write(x: i64) -> CoreBundle {
    let n = NodeRef::Node;
    let p = NodeRef::Pool;
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![eo_int(64), eo_int(0), eo_int(1), eo_int(7), eo_int(x), eo_int(8), eo_bool(true), eo_int(9)],
        nodes: vec![
            eo_call("mem_reserve_raw", vec![p(0)]),
            eo_call("tuple_field", vec![n(0), p(1)]),
            eo_call("int_eq", vec![p(4), p(2)]),                 // 2: c
            eo_call("mem_write_int_raw", vec![n(1), p(5), p(7)]), // 3: h
            eo_call("int_div", vec![p(3), p(4)]),                // 4
            eo_call("mem_write_int_raw", vec![n(1), p(5), n(4)]), // 5: the write, in 7's then-arm
            eo_if(p(6), n(3), p(1)),                             // 6: j, shared
            eo_if(n(2), n(5), n(6)),                             // 7
            eo_call("mem_read_int_raw", vec![n(1), p(5)]),
            eo_call("int_to_str", vec![n(8)]),
            eo_call("io_println", vec![n(9)]),
            eo_call("seq", vec![n(6), n(10)]),                  // 11: j's use after the outer `if`
        ],
        result: NodeRef::Node(11),
    }
}

#[test]
fn test_effect_order_moved_write_runs_only_when_its_condition_holds() {
    let (out, generated) = eo_run("eo_moved_t", &eo_moved_write(1), EO_PURE).unwrap();
    assert_eq!(out, "7", "condition true: the write runs, after h and before the shared if");
    assert!(generated.contains("let node_5_g: Option<Value>"), "{generated}");
    assert_eq!(eo_run("eo_moved_f", &eo_moved_write(0), EO_PURE).unwrap().0, "9",
               "condition false: neither the write nor its input runs");
}

#[test]
fn test_effect_order_toplevel_let_with_an_outer_effect_between_stays_unconditional() {
    // `let w = mem_write_int_raw(p, 8, 7); io_println(read(p, 8)); if c { w } else { 0 }` with c = false and
    // computed BEFORE the write: c's position alone would say "written in the branch", but the println
    // between w and the `if` is an effect the lab created outside the `if` -- so w is a top-level let and
    // runs, before the println.
    let n = NodeRef::Node;
    let p = NodeRef::Pool;
    let b = CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![eo_int(64), eo_int(0), eo_int(1), eo_int(7), eo_int(8)],
        nodes: vec![
            eo_call("mem_reserve_raw", vec![p(0)]),
            eo_call("tuple_field", vec![n(0), p(1)]),
            eo_call("int_eq", vec![p(1), p(2)]),                 // 2: c (false)
            eo_call("mem_write_int_raw", vec![n(1), p(4), p(3)]), // 3: w
            eo_call("mem_read_int_raw", vec![n(1), p(4)]),
            eo_call("int_to_str", vec![n(4)]),
            eo_call("io_println", vec![n(5)]),                   // 6
            eo_if(n(2), n(3), p(1)),
        ],
        result: NodeRef::Node(6),
    };
    let (out, generated) = eo_run("eo_toplevel", &b, EO_PURE).unwrap();
    assert_eq!(out, "7");
    assert!(!generated.contains("_g: Option<Value>"), "{generated}");
}

/// Two levels: the write sits in the then-arm of `if c2` which sits in the else-arm of `if c1`; it runs only
/// when !c1 && c2. A shared `if` over an earlier write of 9 to the same slot -- the inner `if`'s else-arm, and used
/// again after the outer one -- sits between it and both `if`s.
fn eo_moved_two_levels(c1: bool, c2: bool) -> CoreBundle {
    let n = NodeRef::Node;
    let p = NodeRef::Pool;
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![eo_int(64), eo_int(0), eo_int(7), eo_int(8), eo_bool(c1), eo_bool(c2), eo_bool(true), eo_int(9)],
        nodes: vec![
            eo_call("mem_reserve_raw", vec![p(0)]),
            eo_call("tuple_field", vec![n(0), p(1)]),
            eo_call("mem_write_int_raw", vec![n(1), p(3), p(7)]), // 2: h
            eo_call("mem_write_int_raw", vec![n(1), p(3), p(2)]), // 3
            eo_if(p(6), n(2), p(1)),                             // 4: j
            eo_if(p(5), n(3), n(4)),                             // 5
            eo_if(p(4), p(1), n(5)),                             // 6
            eo_call("mem_read_int_raw", vec![n(1), p(3)]),
            eo_call("int_to_str", vec![n(7)]),
            eo_call("io_println", vec![n(8)]),
            eo_call("seq", vec![n(4), n(9)]),
        ],
        result: NodeRef::Node(10),
    }
}

#[test]
fn test_effect_order_moved_write_keeps_every_enclosing_condition() {
    for (c1, c2, want) in [(false, true, "7"), (true, true, "9"), (false, false, "9"), (true, false, "9")] {
        assert_eq!(eo_run("eo_two", &eo_moved_two_levels(c1, c2), EO_PURE).unwrap().0, want, "c1={c1} c2={c2}");
    }
}

#[test]
fn test_effect_order_contradictory_case_is_a_build_error() {
    // The write (node 4) is in the then-arm of `if c2` (node 6), which is in the then-arm of `if true` (node 7);
    // c2 (node 3) is computed inside node 7's arm, before the write. The shared `if` j (node 5) sits between
    // the write and node 7, so the write must run before node 7's arm is entered -- where c2 does not exist yet.
    let n = NodeRef::Node;
    let p = NodeRef::Pool;
    let b = CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![eo_int(64), eo_int(0), eo_int(1), eo_int(7), eo_int(8), eo_bool(true)],
        nodes: vec![
            eo_call("mem_reserve_raw", vec![p(0)]),
            eo_call("tuple_field", vec![n(0), p(1)]),
            eo_call("mem_read_int_raw", vec![n(1), p(4)]),       // 2: h
            eo_call("int_eq", vec![p(2), p(2)]),                 // 3: c2
            eo_call("mem_write_int_raw", vec![n(1), p(4), p(3)]), // 4
            eo_if(p(5), n(2), p(1)),                             // 5: j, node 6's else-arm and used after node 7
            eo_if(n(3), n(4), n(5)),                             // 6
            eo_if(p(5), n(6), p(1)),                             // 7
            eo_call("seq", vec![n(5), n(7)]),
        ],
        result: NodeRef::Node(8),
    };
    let err = eo_run("eo_refused", &b, EO_PURE).expect_err("a contradictory placement must fail the build");
    assert!(err.contains("EFFECT_ORDER_V1") && err.contains("node 4 (mem_write_int_raw)") && err.contains("if node 6")
            && err.contains("node 3") && err.contains("Then arm of if node 7"), "{err}");
}

#[test]
fn test_effect_order_toplevel_let_keeps_old_placement_past_a_pure_if() {
    // `let w = mem_write_int_raw(p, 8, 7); let b = bool_and(true, int_eq(1, 1)); if c { w } else { 0 }` with
    // c false and computed after w (nv_n_step's shape). w is a top-level let: the old bridge ran it
    // unconditionally, because the `if` b sat between it and its consumer. b has no effect in its arms, which
    // does not let w sink into the branch it was never written in.
    let n = NodeRef::Node;
    let p = NodeRef::Pool;
    let b = CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![eo_int(64), eo_int(0), eo_int(1), eo_int(7), eo_int(8), eo_bool(true), eo_bool(false)],
        nodes: vec![
            eo_call("mem_reserve_raw", vec![p(0)]),
            eo_call("tuple_field", vec![n(0), p(1)]),
            eo_call("mem_write_int_raw", vec![n(1), p(4), p(3)]), // 2: w
            eo_call("int_eq", vec![p(2), p(2)]),
            eo_if(p(5), n(3), p(6)),                             // 4: b, pure arms
            eo_call("int_eq", vec![p(1), p(2)]),                 // 5: c (false)
            eo_if(n(5), n(2), p(1)),                             // 6
            eo_call("mem_read_int_raw", vec![n(1), p(4)]),
            eo_call("int_to_str", vec![n(7)]),
            eo_call("io_println", vec![n(8)]),
            eo_call("seq", vec![n(4), n(9)]),
        ],
        result: NodeRef::Node(10),
    };
    assert_eq!(eo_run("eo_toplevel_pure_if", &b, EO_PURE).unwrap().0, "7");
}

#[test]
fn test_effect_order_toplevel_let_under_a_condition_chain_stays_unconditional() {
    // gfx3gate's g3_gate1: `let c1 = ..; let c2 = g3_check(..); return bool_and(bool_and(w, ..), bool_and(c1, c2))`.
    // Here: c1 (node 2) is bound BEFORE the write w (node 3), and w is used only in `if c1 { w }` (node 6), which
    // sits in the then-arm of `if x` (node 7). Node order alone says w comes after c1, as if written in that
    // branch -- but `x` (nodes 4, 5: a pure bool_and) was created between w and node 6 and feeds node 7's
    // condition, not node 6's arms: node 6's arms had not been begun when w was. w is a top-level let and runs,
    // although x is false.
    let n = NodeRef::Node;
    let p = NodeRef::Pool;
    let b = CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![eo_int(64), eo_int(0), eo_int(1), eo_int(7), eo_int(8), eo_bool(true), eo_bool(false), eo_int(2)],
        nodes: vec![
            eo_call("mem_reserve_raw", vec![p(0)]),
            eo_call("tuple_field", vec![n(0), p(1)]),
            eo_call("int_eq", vec![p(2), p(2)]),                 // 2: c1
            eo_call("mem_write_int_raw", vec![n(1), p(4), p(3)]), // 3: w
            eo_call("int_eq", vec![p(2), p(7)]),                 // 4
            eo_if(p(5), n(4), p(6)),                             // 5: x = bool_and(true, 1 == 2), false
            eo_if(n(2), n(3), p(1)),                             // 6
            eo_if(n(5), n(6), p(1)),                             // 7
            eo_call("mem_read_int_raw", vec![n(1), p(4)]),
            eo_call("int_to_str", vec![n(8)]),
            eo_call("io_println", vec![n(9)]),
            eo_call("seq", vec![n(7), n(10)]),
        ],
        result: NodeRef::Node(11),
    };
    assert_eq!(eo_run("eo_cond_chain", &b, EO_PURE).unwrap().0, "7");
}

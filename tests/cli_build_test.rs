use axis_codegen_bridge::core_ir::{CoreTerm, Provenance, EffectClass, create_core_bundle, create_core_bundle_multi};
use std::process::Command;
use std::rc::Rc;
use tempfile::TempDir;

fn bridge() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_axis-codegen-bridge"))
}

fn write_fixture(dir: &TempDir, name: &str, root: CoreTerm, entrypoint: &str) -> std::path::PathBuf {
    let bytes = create_core_bundle(&root, entrypoint, Provenance::Mechanical, EffectClass::Pure, true);
    let path = dir.path().join(name);
    std::fs::write(&path, &bytes).unwrap();
    path
}

// (a) build without --exe: produces lib<stem>.a, symbol present, no binary
#[test]
fn test_build_lib_only() {
    let dir = TempDir::new().unwrap();
    let fixture = write_fixture(
        &dir, "mymod.coreir",
        CoreTerm::Lam("args".to_string(), Rc::new(CoreTerm::IntLit(42, None)), None),
        "mymod_fn",
    );
    let out = dir.path().join("mymod");

    let status = Command::new(bridge())
        .args([
            "build",
            fixture.to_str().unwrap(),
            "--out", out.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run bridge");

    assert!(status.success(), "build failed");

    let lib = dir.path().join("libmymod.a");
    assert!(lib.exists(), "lib archive not produced at {:?}", lib);
    assert!(!out.exists(), "binary should not exist without --exe");

    // Symbol must be present in the archive.
    let nm = Command::new("nm").arg(&lib).output().expect("nm failed");
    let nm_out = String::from_utf8_lossy(&nm.stdout);
    assert!(nm_out.contains("mymod_fn"), "symbol mymod_fn not found in archive:\n{}", nm_out);
}

// (b) build with --exe: produces both lib<stem>.a and a runnable binary
#[test]
fn test_build_with_exe() {
    let dir = TempDir::new().unwrap();
    let fixture = write_fixture(
        &dir, "exetest.coreir",
        CoreTerm::Lam("args".to_string(), Rc::new(CoreTerm::IntLit(77, None)), None),
        "exetest_fn",
    );
    let out = dir.path().join("exetest");

    let status = Command::new(bridge())
        .args([
            "build",
            fixture.to_str().unwrap(),
            "--out", out.to_str().unwrap(),
            "--exe",
        ])
        .status()
        .expect("failed to run bridge");

    assert!(status.success(), "build --exe failed");

    let lib = dir.path().join("libexetest.a");
    assert!(lib.exists(), "lib archive not produced");
    assert!(out.exists(), "binary not produced");

    // Run the binary and check output.
    let run = Command::new(&out).output().expect("binary failed to run");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert_eq!(stdout.trim(), "77", "binary printed wrong output: {:?}", stdout);
}

// (c) bundle: merge two archives; combined contains symbols from both
#[test]
fn test_bundle_merges() {
    let dir = TempDir::new().unwrap();

    // Build first archive.
    let fix_a = write_fixture(
        &dir, "alpha.coreir",
        CoreTerm::Lam("args".to_string(), Rc::new(CoreTerm::IntLit(1, None)), None),
        "alpha_fn",
    );
    let out_a = dir.path().join("alpha");
    Command::new(bridge())
        .args(["build", fix_a.to_str().unwrap(), "--out", out_a.to_str().unwrap()])
        .status().expect("build alpha failed");

    // Build second archive.
    let fix_b = write_fixture(
        &dir, "beta.coreir",
        CoreTerm::Lam("args".to_string(), Rc::new(CoreTerm::IntLit(2, None)), None),
        "beta_fn",
    );
    let out_b = dir.path().join("beta");
    Command::new(bridge())
        .args(["build", fix_b.to_str().unwrap(), "--out", out_b.to_str().unwrap()])
        .status().expect("build beta failed");

    let lib_a    = dir.path().join("libalpha.a");
    let lib_b    = dir.path().join("libbeta.a");
    let combined = dir.path().join("combined.a");

    assert!(lib_a.exists(), "libalpha.a not built");
    assert!(lib_b.exists(), "libbeta.a not built");

    let status = Command::new(bridge())
        .args([
            "bundle",
            "--out", combined.to_str().unwrap(),
            lib_a.to_str().unwrap(),
            lib_b.to_str().unwrap(),
        ])
        .status()
        .expect("bundle failed");

    assert!(status.success(), "bundle command failed");
    assert!(combined.exists(), "combined archive not produced");

    let nm = Command::new("nm").arg(&combined).output().expect("nm failed");
    let nm_out = String::from_utf8_lossy(&nm.stdout);
    assert!(nm_out.contains("alpha_fn"), "alpha_fn missing from bundle:\n{}", nm_out);
    assert!(nm_out.contains("beta_fn"),  "beta_fn missing from bundle:\n{}", nm_out);
}

// ── multi-export tests ───────────────────────────────────────────────────────

fn write_multi_fixture(dir: &TempDir, name: &str, exports: &[(&str, CoreTerm, &str)]) -> std::path::PathBuf {
    let refs: Vec<(&str, &CoreTerm, &str)> = exports.iter().map(|(n, t, e)| (*n, t, *e)).collect();
    let bytes = create_core_bundle_multi(&refs, Provenance::Mechanical, true);
    let path = dir.path().join(name);
    std::fs::write(&path, &bytes).unwrap();
    path
}

// (d) multi-export bundle: both symbols appear in the .a
#[test]
fn test_multi_export_lib_both_symbols() {
    let dir = TempDir::new().unwrap();

    // 'add': \x -> int_add(x, x)   (simplified: just returns 42)
    let add_term = CoreTerm::Lam("x".to_string(), Rc::new(CoreTerm::IntLit(42, None)), None);
    // 'mul': \x -> 99
    let mul_term = CoreTerm::Lam("x".to_string(), Rc::new(CoreTerm::IntLit(99, None)), None);

    let fixture = write_multi_fixture(&dir, "multi.coreir", &[
        ("add", add_term, "pure"),
        ("mul", mul_term, "pure"),
    ]);
    let out = dir.path().join("multi");

    let status = Command::new(bridge())
        .args(["build", fixture.to_str().unwrap(), "--out", out.to_str().unwrap()])
        .status()
        .expect("failed to run bridge");

    assert!(status.success(), "build failed");

    let lib = dir.path().join("libmulti.a");
    assert!(lib.exists(), "lib archive not produced at {:?}", lib);

    let nm = Command::new("nm").arg(&lib).output().expect("nm failed");
    let nm_out = String::from_utf8_lossy(&nm.stdout);
    assert!(nm_out.contains("add"), "symbol 'add' not found in archive:\n{}", nm_out);
    assert!(nm_out.contains("mul"), "symbol 'mul' not found in archive:\n{}", nm_out);
}

// (e) multi-export with --exe calls the first export
#[test]
fn test_multi_export_exe_calls_first() {
    let dir = TempDir::new().unwrap();

    // 'add' is first — should be the --exe entrypoint; returns 55
    let add_term = CoreTerm::Lam("args".to_string(), Rc::new(CoreTerm::IntLit(55, None)), None);
    let mul_term = CoreTerm::Lam("args".to_string(), Rc::new(CoreTerm::IntLit(77, None)), None);

    let fixture = write_multi_fixture(&dir, "multi_exe.coreir", &[
        ("add", add_term, "pure"),
        ("mul", mul_term, "pure"),
    ]);
    let out = dir.path().join("multi_exe");

    let status = Command::new(bridge())
        .args(["build", fixture.to_str().unwrap(), "--out", out.to_str().unwrap(), "--exe"])
        .status()
        .expect("failed to run bridge");

    assert!(status.success(), "build failed");
    assert!(out.exists(), "binary not produced");

    let run = Command::new(&out).output().expect("failed to run binary");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.trim() == "55", "expected '55', got {:?}", stdout.trim());
}

// (f) single-export bundle still works identically (backward compat)
#[test]
fn test_single_export_backward_compat() {
    let dir = TempDir::new().unwrap();
    let fixture = write_fixture(
        &dir, "single.coreir",
        CoreTerm::Lam("args".to_string(), Rc::new(CoreTerm::IntLit(13, None)), None),
        "single_fn",
    );
    let out = dir.path().join("single");

    let status = Command::new(bridge())
        .args(["build", fixture.to_str().unwrap(), "--out", out.to_str().unwrap(), "--exe"])
        .status()
        .expect("failed to run bridge");

    assert!(status.success(), "build failed");
    assert!(out.exists(), "binary not produced");

    let run = Command::new(&out).output().expect("failed to run binary");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.trim() == "13", "expected '13', got {:?}", stdout.trim());
}

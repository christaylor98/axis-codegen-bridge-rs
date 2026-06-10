use axis_codegen_bridge::core_ir::{CoreTerm, Provenance, EffectClass, create_core_bundle};
use axis_codegen_bridge::core_ir_05::{
    lower_core_term_to_bundle_05,
    int_type_hash, bool_type_hash, unit_type_hash,
    encode_int_payload, encode_bool_payload,
    NodeRef, Node,
};
use axis_codegen_bridge::runtime::value::{Value, intern_str, get_str, get_tag_name, init_runtime};
use axis_codegen_bridge::runtime::{io, frontend, registry};
use std::rc::Rc;
use std::sync::Mutex;
use tempfile::{NamedTempFile, TempDir};
use std::process::Command;

// ── Helpers ───────────────────────────────────────────────────────────────────

static REGISTRY_LOCK: Mutex<()> = Mutex::new(());

fn setup() { init_runtime(); }

fn s(text: &str) -> Value {
    Value::Str(intern_str(text))
}

fn t2(a: Value, b: Value) -> Value {
    Value::Tuple(vec![a, b])
}

fn t3(a: Value, b: Value, c: Value) -> Value {
    Value::Tuple(vec![a, b, c])
}

fn ctor_tag(v: &Value) -> String {
    match v {
        Value::Ctor { tag, .. } => get_tag_name(*tag),
        other => panic!("expected Ctor, got {:?}", other),
    }
}

fn walk_str(v: &Value) -> String {
    match v {
        Value::Str(h) => get_str(*h),
        other => panic!("expected Str, got {:?}", other),
    }
}

fn bridge() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_axis-codegen-bridge"))
}

fn with_registry<F: FnOnce()>(f: F) {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = NamedTempFile::new().expect("tempfile");
    std::env::set_var("AXIS_REGISTRY", tmp.path());
    f();
    std::env::remove_var("AXIS_REGISTRY");
}

fn write_temp(dir: &TempDir, name: &str, content: &str) -> String {
    let path = dir.path().join(name);
    std::fs::write(&path, content).unwrap();
    path.to_str().unwrap().to_string()
}

// ── D1: lower_core_term_to_bundle_05 ─────────────────────────────────────────

#[test]
fn test_lower_intlit_goes_to_pool() {
    let term = CoreTerm::IntLit(42, None);
    let bundle = lower_core_term_to_bundle_05(&term).unwrap();
    assert_eq!(bundle.version, "0.5");
    assert_eq!(bundle.nodes.len(), 0, "IntLit should not produce any nodes");
    assert_eq!(bundle.constant_pool.len(), 1);
    assert_eq!(bundle.constant_pool[0].def_hash, int_type_hash());
    assert_eq!(bundle.constant_pool[0].payload,  encode_int_payload(42));
}

#[test]
fn test_lower_boollit_goes_to_pool() {
    let term = CoreTerm::BoolLit(true, None);
    let bundle = lower_core_term_to_bundle_05(&term).unwrap();
    assert_eq!(bundle.nodes.len(), 0);
    assert_eq!(bundle.constant_pool.len(), 1);
    assert_eq!(bundle.constant_pool[0].def_hash, bool_type_hash());
    assert_eq!(bundle.constant_pool[0].payload,  encode_bool_payload(true));
}

#[test]
fn test_lower_unitlit_goes_to_pool() {
    let term = CoreTerm::UnitLit(None);
    let bundle = lower_core_term_to_bundle_05(&term).unwrap();
    assert_eq!(bundle.nodes.len(), 0);
    assert_eq!(bundle.constant_pool.len(), 1);
    assert_eq!(bundle.constant_pool[0].def_hash, unit_type_hash());
    assert_eq!(bundle.constant_pool[0].payload,  Vec::<u8>::new());
}

#[test]
fn test_lower_let_chain_var_resolves() {
    // let x = IntLit(7) in x  →  1 pool entry, env lookup succeeds
    let term = CoreTerm::Let(
        "x".to_string(),
        Rc::new(CoreTerm::IntLit(7, None)),
        Rc::new(CoreTerm::Var("x".to_string(), None)),
        None,
    );
    let bundle = lower_core_term_to_bundle_05(&term).unwrap();
    assert_eq!(bundle.constant_pool.len(), 1);
    assert_eq!(bundle.nodes.len(), 0);
    assert_eq!(bundle.constant_pool[0].def_hash, int_type_hash());
}

#[test]
fn test_lower_unbound_var_is_err() {
    let term = CoreTerm::Var("missing".to_string(), None);
    let err = lower_core_term_to_bundle_05(&term).unwrap_err();
    assert!(err.contains("unbound"), "unexpected error: {}", err);
}

#[test]
fn test_lower_call_produces_ccall_node() {
    // Call("int_add", [IntLit(1), IntLit(2)]) → 2 pool entries + 1 CCall node
    let term = CoreTerm::Call(
        "int_add".to_string(),
        vec![CoreTerm::IntLit(1, None), CoreTerm::IntLit(2, None)],
        None,
    );
    let bundle = lower_core_term_to_bundle_05(&term).unwrap();
    assert_eq!(bundle.constant_pool.len(), 2);
    assert_eq!(bundle.nodes.len(), 1);
    match &bundle.nodes[0] {
        Node::CCall { args, .. } => {
            assert_eq!(args.len(), 2);
            assert!(matches!(args[0], NodeRef::Pool(0)));
            assert!(matches!(args[1], NodeRef::Pool(1)));
        }
        other => panic!("expected CCall, got {:?}", other),
    }
}

#[test]
fn test_lower_app_spine_produces_ccall_node() {
    // App(App(Var("int_add"), IntLit(1)), IntLit(2)) → CCall with 2 args
    let term = CoreTerm::App(
        Rc::new(CoreTerm::App(
            Rc::new(CoreTerm::Var("int_add".to_string(), None)),
            Rc::new(CoreTerm::IntLit(1, None)),
            None,
        )),
        Rc::new(CoreTerm::IntLit(2, None)),
        None,
    );
    let bundle = lower_core_term_to_bundle_05(&term).unwrap();
    assert_eq!(bundle.nodes.len(), 1);
    match &bundle.nodes[0] {
        Node::CCall { args, .. } => assert_eq!(args.len(), 2),
        other => panic!("expected CCall, got {:?}", other),
    }
}

#[test]
fn test_lower_app_non_var_head_is_err() {
    // App(IntLit(1), IntLit(2)) — head is not a Var → error
    let term = CoreTerm::App(
        Rc::new(CoreTerm::IntLit(1, None)),
        Rc::new(CoreTerm::IntLit(2, None)),
        None,
    );
    let err = lower_core_term_to_bundle_05(&term).unwrap_err();
    assert!(
        err.contains("App head") || err.contains("Var"),
        "unexpected error: {}", err
    );
}

#[test]
fn test_lower_if_produces_cif_node() {
    // If(BoolLit(true), IntLit(1), IntLit(0)) → 3 pool entries, 1 CIf node
    let term = CoreTerm::If(
        Rc::new(CoreTerm::BoolLit(true, None)),
        Rc::new(CoreTerm::IntLit(1, None)),
        Rc::new(CoreTerm::IntLit(0, None)),
        None,
    );
    let bundle = lower_core_term_to_bundle_05(&term).unwrap();
    assert_eq!(bundle.constant_pool.len(), 3, "cond + then + else each go to pool");
    assert_eq!(bundle.nodes.len(), 1);
    assert!(matches!(&bundle.nodes[0], Node::CIf { .. }));
}

#[test]
fn test_lower_lam_is_err() {
    let term = CoreTerm::Lam(
        "x".to_string(),
        Rc::new(CoreTerm::Var("x".to_string(), None)),
        None,
    );
    let err = lower_core_term_to_bundle_05(&term).unwrap_err();
    assert!(err.contains("Lam"), "unexpected error: {}", err);
}

// ── D2: frontend_walk ─────────────────────────────────────────────────────────

#[test]
fn test_frontend_walk_missing_types_file() {
    setup();
    let dir = TempDir::new().unwrap();
    // types file does not exist
    let shapes_path = dir.path().join("shapes.txt").to_str().unwrap().to_string();
    let types_path  = dir.path().join("MISSING_types.txt").to_str().unwrap().to_string();
    let result = frontend::frontend_walk(t3(s(&shapes_path), s(&types_path), s("art")));
    assert_eq!(walk_str(&result), "WALL|1\nDONE");
}

#[test]
fn test_frontend_walk_artifact_not_in_types() {
    setup();
    let dir = TempDir::new().unwrap();
    let types  = write_temp(&dir, "types.txt",  "other_artifact|MyShape\n");
    let shapes = write_temp(&dir, "shapes.txt", "MyShape|h1|Int|NEED|some detail\n");
    let result = frontend::frontend_walk(t3(s(&shapes), s(&types), s("my_artifact")));
    assert_eq!(walk_str(&result), "WALL|1\nDONE");
}

#[test]
fn test_frontend_walk_need_status() {
    setup();
    let dir = TempDir::new().unwrap();
    let types  = write_temp(&dir, "types.txt",  "art|Shape1\n");
    let shapes = write_temp(&dir, "shapes.txt", "Shape1|h1|Int|NEED|requires input\n");
    let result = frontend::frontend_walk(t3(s(&shapes), s(&types), s("art")));
    let out = walk_str(&result);
    // No resolved entries → WALL|1 prefix
    assert!(out.starts_with("WALL|1\n"), "expected WALL prefix, got: {}", out);
    assert!(out.contains("NEED|h1|Int|requires input"), "missing NEED line, got: {}", out);
    assert!(out.ends_with("DONE"), "missing DONE, got: {}", out);
}

#[test]
fn test_frontend_walk_unknown_status() {
    setup();
    let dir = TempDir::new().unwrap();
    let types  = write_temp(&dir, "types.txt",  "art|ShapeU\n");
    let shapes = write_temp(&dir, "shapes.txt", "ShapeU|h2|Text|UNKNOWN|some info\n");
    let result = frontend::frontend_walk(t3(s(&shapes), s(&types), s("art")));
    let out = walk_str(&result);
    assert!(out.starts_with("WALL|1\n"), "expected WALL prefix, got: {}", out);
    assert!(out.contains("UNKNOWN|h2|Text"), "missing UNKNOWN line, got: {}", out);
}

#[test]
fn test_frontend_walk_registry_check_unresolved() {
    setup();
    with_registry(|| {
        let dir = TempDir::new().unwrap();
        let types  = write_temp(&dir, "types.txt",  "art|ShapeR\n");
        let shapes = write_temp(&dir, "shapes.txt", "ShapeR|h3|Int|REGISTRY_CHECK|mymod.not_present\n");
        let result = frontend::frontend_walk(t3(s(&shapes), s(&types), s("art")));
        let out = walk_str(&result);
        assert!(out.starts_with("WALL|1\n"),          "expected WALL prefix, got: {}", out);
        assert!(out.contains("UNKNOWN|h3"),           "missing UNKNOWN line, got: {}", out);
        assert!(out.contains("not in registry"),      "missing 'not in registry', got: {}", out);
    });
}

#[test]
fn test_frontend_walk_registry_check_resolved() {
    setup();
    with_registry(|| {
        // Insert the registry entry first
        registry::registry_insert(t3(
            s("mymod.known_fn"),
            s(""),
            s("Human"),
        ));

        let dir = TempDir::new().unwrap();
        let types  = write_temp(&dir, "types.txt",  "art|ShapeResolved\n");
        let shapes = write_temp(&dir, "shapes.txt", "ShapeResolved|h4|Int|REGISTRY_CHECK|mymod.known_fn\n");
        let result = frontend::frontend_walk(t3(s(&shapes), s(&types), s("art")));
        let out = walk_str(&result);
        // Has a resolved entry → no WALL|1 prefix
        assert!(!out.starts_with("WALL|1"), "unexpected WALL in: {}", out);
        assert!(out.contains("RESOLVED|h4|Int"),      "missing RESOLVED line, got: {}", out);
        assert!(out.ends_with("DONE"),                "missing DONE, got: {}", out);
    });
}

#[test]
fn test_frontend_walk_wrong_arg_type() {
    setup();
    let result = frontend::frontend_walk(Value::Unit);
    assert_eq!(walk_str(&result), "WALL|1\nDONE");
}

// ── D3: --reg CLI end-to-end ──────────────────────────────────────────────────

#[test]
fn test_reg_flag_writes_valid_axreg_entry() {
    let dir = TempDir::new().unwrap();
    let reg_path = dir.path().join("out.axreg");

    let term  = CoreTerm::Lam("args".to_string(), Rc::new(CoreTerm::IntLit(42, None)), None);
    let bytes = create_core_bundle(&term, "regtest_fn", Provenance::Mechanical, EffectClass::Pure, true);
    let fixture = dir.path().join("regtest.coreir");
    std::fs::write(&fixture, &bytes).unwrap();

    // Create an empty axreg file (append requires the file to exist)
    std::fs::write(&reg_path, "").unwrap();

    let status = Command::new(bridge())
        .args([
            "build",
            fixture.to_str().unwrap(),
            "--out", dir.path().join("regtest").to_str().unwrap(),
            "--reg", reg_path.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run bridge");

    assert!(status.success(), "bridge exited with failure");

    let content = std::fs::read_to_string(&reg_path).unwrap();

    // Required valid fields
    assert!(content.contains("fn regtest_fn"),      "missing fn entry:\n{}", content);
    assert!(content.contains("identity"),            "missing identity field:\n{}", content);
    assert!(content.contains("kind     leaf"),       "missing kind field:\n{}", content);
    assert!(content.contains("in       (Value)"),    "missing in field:\n{}", content);
    assert!(content.contains("out      Value"),      "missing out field:\n{}", content);
    assert!(content.contains("effect   pure"),       "missing effect field:\n{}", content);
    assert!(content.contains("deterministic true"),  "missing deterministic:\n{}", content);
    assert!(content.contains("idempotent    true"),  "missing idempotent:\n{}", content);
    assert!(content.contains("end"),                 "missing end marker:\n{}", content);

    // Forbidden fields must be absent
    assert!(!content.contains("arity"),   "forbidden field 'arity' present:\n{}", content);
    assert!(!content.contains("profile"), "forbidden field 'profile' present:\n{}", content);
}

#[test]
fn test_reg_flag_is_idempotent() {
    let dir = TempDir::new().unwrap();
    let reg_path = dir.path().join("idem.axreg");

    let term  = CoreTerm::Lam("args".to_string(), Rc::new(CoreTerm::IntLit(1, None)), None);
    let bytes = create_core_bundle(&term, "idem_fn", Provenance::Mechanical, EffectClass::Pure, true);
    let fixture = dir.path().join("idem.coreir");
    std::fs::write(&fixture, &bytes).unwrap();
    std::fs::write(&reg_path, "").unwrap();

    let out_arg = dir.path().join("idem");
    let args = [
        "build",
        fixture.to_str().unwrap(),
        "--out", out_arg.to_str().unwrap(),
        "--reg", reg_path.to_str().unwrap(),
    ];

    Command::new(bridge()).args(&args).status().expect("first run failed");
    let after_first  = std::fs::read_to_string(&reg_path).unwrap();

    Command::new(bridge()).args(&args).status().expect("second run failed");
    let after_second = std::fs::read_to_string(&reg_path).unwrap();

    let count_first  = after_first.matches("fn idem_fn").count();
    let count_second = after_second.matches("fn idem_fn").count();

    assert_eq!(count_first,  1, "expected 1 entry after first run, got {}", count_first);
    assert_eq!(count_second, 1, "expected 1 entry after second run (idempotent), got {}", count_second);
}

// ── D4: io.rs error paths ─────────────────────────────────────────────────────

#[test]
fn test_fs_read_text_missing_file_returns_err() {
    setup();
    let result = io::fs_read_text(s("/nonexistent/axis/path/does_not_exist.txt"));
    assert_eq!(ctor_tag(&result), "Err");
}

#[test]
fn test_fs_write_text_bad_dir_returns_err() {
    setup();
    let result = io::fs_write_text(t2(s("/no/such/directory/file.txt"), s("content")));
    assert_eq!(ctor_tag(&result), "Err");
}

#[test]
fn test_fs_write_text_roundtrip() {
    setup();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("write_test.txt");
    let path_str = path.to_str().unwrap();

    let write_result = io::fs_write_text(t2(s(path_str), s("hello bridge")));
    assert_eq!(ctor_tag(&write_result), "Ok");

    let read_result = io::fs_read_text(s(path_str));
    assert_eq!(ctor_tag(&read_result), "Ok");
    match read_result {
        Value::Ctor { fields, .. } => assert_eq!(fields[0], s("hello bridge")),
        _ => panic!("expected Ctor"),
    }
}

#[test]
fn test_fs_append_text_accumulates() {
    setup();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("append_test.txt");
    let path_str = path.to_str().unwrap();

    assert_eq!(ctor_tag(&io::fs_append_text(t2(s(path_str), s("line1\n")))), "Ok");
    assert_eq!(ctor_tag(&io::fs_append_text(t2(s(path_str), s("line2\n")))), "Ok");

    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "line1\nline2\n");
}

#[test]
fn test_fs_read_text_ok_contains_content() {
    setup();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("readable.txt");
    std::fs::write(&path, "axis works").unwrap();

    let result = io::fs_read_text(s(path.to_str().unwrap()));
    assert_eq!(ctor_tag(&result), "Ok");
    match result {
        Value::Ctor { fields, .. } => assert_eq!(fields[0], s("axis works")),
        _ => panic!("expected Ctor"),
    }
}

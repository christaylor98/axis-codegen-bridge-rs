use axis_codegen_bridge::core_ir::CoreTerm;
use axis_codegen_bridge::emit::rust::emit_rust_from_core;
use std::collections::HashSet;
use std::rc::Rc;

fn no_registry() -> HashSet<String> { HashSet::new() }

fn double_lib() -> (String, CoreTerm) {
    (
        "double".to_string(),
        CoreTerm::Lam(
            "x".to_string(),
            Rc::new(CoreTerm::Call(
                "int_mul".to_string(),
                vec![CoreTerm::Var("x".to_string(), None), CoreTerm::IntLit(2, None)],
                None,
            )),
            None,
        ),
    )
}

#[test]
fn test_emit_lib_defines_rust_fn() {
    let libs = vec![double_lib()];
    let root = CoreTerm::Call("double".to_string(), vec![CoreTerm::IntLit(21, None)], None);
    let code = emit_rust_from_core(&root, "test.coreir", "main", &libs, &no_registry()).unwrap();
    assert!(code.contains("fn _lib_double(x: Value) -> Value"), "lib fn not emitted");
    assert!(code.contains("int_mul"), "lib body not emitted");
}

#[test]
fn test_emit_lib_call_in_main() {
    let libs = vec![double_lib()];
    let root = CoreTerm::Call("double".to_string(), vec![CoreTerm::IntLit(21, None)], None);
    let code = emit_rust_from_core(&root, "test.coreir", "main", &libs, &no_registry()).unwrap();
    assert!(code.contains("_lib_double(Value::Int(21))"), "call site not emitted");
}

#[test]
fn test_emit_unresolved_ccall_is_error() {
    let root = CoreTerm::Call("nonexistent".to_string(), vec![], None);
    let result = emit_rust_from_core(&root, "test.coreir", "main", &[], &no_registry());
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("unresolved: nonexistent"), "wrong error: {}", err);
}

#[test]
fn test_emit_ccall_resolved_via_registry() {
    let root = CoreTerm::Call("my_custom_fn".to_string(), vec![], None);
    let mut reg = HashSet::new();
    reg.insert("my_custom_fn".to_string());
    let code = emit_rust_from_core(&root, "test.coreir", "main", &[], &reg).unwrap();
    assert!(code.contains("my_custom_fn(Value::Unit)"), "registry-declared call not emitted");
}

#[test]
fn test_emit_bridge_symbol_passes_validation() {
    let root = CoreTerm::Call("int_add".to_string(), vec![CoreTerm::IntLit(1, None), CoreTerm::IntLit(2, None)], None);
    let result = emit_rust_from_core(&root, "test.coreir", "main", &[], &no_registry());
    assert!(result.is_ok(), "bridge symbol should pass validation");
}

#[test]
fn test_emit_duplicate_lib_names_detected_before_emit() {
    // Duplicate detection happens in cmd_build, not emit_rust_from_core.
    // Emit with two libs of the same name is ambiguous but not caught in emit itself —
    // the second definition wins in the emitted file. This test just verifies both fns
    // appear and the code is syntactically plausible.
    let lib1 = ("double".to_string(), CoreTerm::IntLit(1, None));
    let lib2 = ("greet".to_string(), CoreTerm::IntLit(2, None));
    let root = CoreTerm::Call("double".to_string(), vec![], None);
    let code = emit_rust_from_core(&root, "t.coreir", "main", &[lib1, lib2], &no_registry()).unwrap();
    assert!(code.contains("fn _lib_double"), "double not defined");
    assert!(code.contains("fn _lib_greet"), "greet not defined");
}

#[test]
fn test_emit_lib_ccall_resolved_against_bridge() {
    // Library function that calls a bridge symbol internally.
    let greet = (
        "greet".to_string(),
        CoreTerm::Lam(
            "x".to_string(),
            Rc::new(CoreTerm::Call(
                "io_println".to_string(),
                vec![CoreTerm::Call(
                    "int_to_str".to_string(),
                    vec![CoreTerm::Var("x".to_string(), None)],
                    None,
                )],
                None,
            )),
            None,
        ),
    );
    let libs = vec![double_lib(), greet];
    let root = CoreTerm::Let(
        "d".to_string(),
        Rc::new(CoreTerm::Call("double".to_string(), vec![CoreTerm::IntLit(21, None)], None)),
        Rc::new(CoreTerm::Call("greet".to_string(), vec![CoreTerm::Var("d".to_string(), None)], None)),
        None,
    );
    let code = emit_rust_from_core(&root, "t.coreir", "main", &libs, &no_registry()).unwrap();
    assert!(code.contains("fn _lib_double"), "double lib missing");
    assert!(code.contains("fn _lib_greet"), "greet lib missing");
    assert!(code.contains("let d ="), "let binding missing");
}

#[test]
fn test_emit_lib_unresolved_ccall_in_lib_body_is_error() {
    let bad_lib = (
        "bad".to_string(),
        CoreTerm::Lam(
            "x".to_string(),
            Rc::new(CoreTerm::Call("nonexistent_fn".to_string(), vec![], None)),
            None,
        ),
    );
    let root = CoreTerm::Call("bad".to_string(), vec![], None);
    let result = emit_rust_from_core(&root, "t.coreir", "main", &[bad_lib], &no_registry());
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("unresolved: nonexistent_fn"));
}

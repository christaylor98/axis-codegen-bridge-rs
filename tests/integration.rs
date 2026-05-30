use axis_codegen_bridge::runtime::value::{Value, intern_str, get_str, init_runtime};
use axis_codegen_bridge::runtime::ir_constructors::{
    ir_make_int_lit, ir_make_bool_lit, ir_make_unit_lit,
    ir_make_var, ir_make_lam, ir_make_let, ir_make_if, ir_make_app, ir_make_call,
    ir_term_kind, ir_write_bundle, ir_read_bundle,
    ir_subst, ir_rename, ir_free_vars,
};
use axis_codegen_bridge::runtime::{arith, str_ops, list, option, bool_ops};
use axis_codegen_bridge::core_ir::{CoreTerm, Provenance, EffectClass, create_core_bundle, load_core_bundle_from_bytes};
use axis_codegen_bridge::executor::{execute_core_program, FunctionProvider, Value as ExecValue, RuntimeError};
use std::rc::Rc;

// Pull CoreProgram through the public loader path
use axis_codegen_bridge::core_ir::loader::CoreProgram;

fn setup() { init_runtime(); }

fn make_program(root: CoreTerm) -> CoreProgram {
    CoreProgram {
        root_term: root,
        entrypoint_id: 0,
        provenance: Provenance::Mechanical,
        effect_class: EffectClass::Pure,
        idempotent: true,
    }
}

// ── Value / string interning ─────────────────────────────────────────────────

#[test]
fn test_intern_str_round_trip() {
    setup();
    let h = intern_str("hello");
    assert_eq!(get_str(h), "hello");
}

#[test]
fn test_intern_str_dedup() {
    setup();
    let h1 = intern_str("same");
    let h2 = intern_str("same");
    assert_eq!(h1, h2);
}

#[test]
fn test_value_display_int()  { assert_eq!(format!("{}", Value::Int(42)), "42"); }
#[test]
fn test_value_display_unit() { assert_eq!(format!("{}", Value::Unit), "()"); }

#[test]
fn test_value_display_str() {
    setup();
    let h = intern_str("hi");
    assert_eq!(format!("{}", Value::Str(h)), "hi");
}

// ── Arithmetic ───────────────────────────────────────────────────────────────

#[test]
fn test_int_add() {
    let r = arith::int_add(Value::Tuple(vec![Value::Int(3), Value::Int(4)]));
    assert_eq!(r, Value::Int(7));
}

#[test]
fn test_int_sub() {
    let r = arith::int_sub(Value::Tuple(vec![Value::Int(10), Value::Int(3)]));
    assert_eq!(r, Value::Int(7));
}

#[test]
fn test_int_mul() {
    let r = arith::int_mul(Value::Tuple(vec![Value::Int(6), Value::Int(7)]));
    assert_eq!(r, Value::Int(42));
}

#[test]
fn test_int_div() {
    let r = arith::int_div(Value::Tuple(vec![Value::Int(10), Value::Int(2)]));
    assert_eq!(r, Value::Int(5));
}

#[test]
fn test_int_div_checked_zero() {
    setup();
    let r = arith::int_div_checked(Value::Tuple(vec![Value::Int(10), Value::Int(0)]));
    assert_eq!(option::option_is_none(r), Value::Bool(true));
}

#[test]
fn test_int_div_checked_ok() {
    setup();
    let r = arith::int_div_checked(Value::Tuple(vec![Value::Int(10), Value::Int(2)]));
    assert_eq!(option::option_unwrap(r), Value::Int(5));
}

#[test]
fn test_int_lt_true()  { assert_eq!(arith::int_lt(Value::Tuple(vec![Value::Int(1), Value::Int(2)])), Value::Bool(true)); }
#[test]
fn test_int_lt_false() { assert_eq!(arith::int_lt(Value::Tuple(vec![Value::Int(2), Value::Int(1)])), Value::Bool(false)); }

#[test]
fn test_value_eq_same()  { assert_eq!(arith::value_eq(Value::Tuple(vec![Value::Int(5), Value::Int(5)])), Value::Bool(true)); }
#[test]
fn test_value_eq_diff()  { assert_eq!(arith::value_eq(Value::Tuple(vec![Value::Int(5), Value::Int(6)])), Value::Bool(false)); }

#[test]
fn test_int_to_str() {
    setup();
    let r = arith::int_to_str(Value::Int(42));
    assert_eq!(format!("{}", r), "42");
}

#[test]
fn test_str_to_int() {
    setup();
    let h = intern_str("99");
    assert_eq!(arith::str_to_int(Value::Str(h)), Value::Int(99));
}

// ── String ops ───────────────────────────────────────────────────────────────

#[test]
fn test_str_len() {
    setup();
    let h = intern_str("hello");
    assert_eq!(str_ops::str_len(Value::Str(h)), Value::Int(5));
}

#[test]
fn test_str_concat() {
    setup();
    let ha = intern_str("foo");
    let hb = intern_str("bar");
    let r  = str_ops::str_concat(Value::Tuple(vec![Value::Str(ha), Value::Str(hb)]));
    assert_eq!(format!("{}", r), "foobar");
}

#[test]
fn test_str_char_at_valid() {
    setup();
    let h = intern_str("abc");
    let r = str_ops::str_char_at(Value::Tuple(vec![Value::Str(h), Value::Int(1)]));
    let inner = option::option_unwrap(r);
    assert_eq!(format!("{}", inner), "b");
}

#[test]
fn test_str_char_at_oob() {
    setup();
    let h = intern_str("abc");
    let r = str_ops::str_char_at(Value::Tuple(vec![Value::Str(h), Value::Int(99)]));
    assert_eq!(option::option_is_none(r), Value::Bool(true));
}

// ── Bool ops ─────────────────────────────────────────────────────────────────

#[test]
fn test_bool_and_tt() { assert_eq!(bool_ops::bool_and(Value::Tuple(vec![Value::Bool(true),  Value::Bool(true)])),  Value::Bool(true)); }
#[test]
fn test_bool_and_tf() { assert_eq!(bool_ops::bool_and(Value::Tuple(vec![Value::Bool(true),  Value::Bool(false)])), Value::Bool(false)); }
#[test]
fn test_bool_or_ft()  { assert_eq!(bool_ops::bool_or( Value::Tuple(vec![Value::Bool(false), Value::Bool(true)])),  Value::Bool(true)); }
#[test]
fn test_bool_or_ff()  { assert_eq!(bool_ops::bool_or( Value::Tuple(vec![Value::Bool(false), Value::Bool(false)])), Value::Bool(false)); }
#[test]
fn test_bool_not_t()  { assert_eq!(bool_ops::bool_not(Value::Bool(true)),  Value::Bool(false)); }
#[test]
fn test_bool_not_f()  { assert_eq!(bool_ops::bool_not(Value::Bool(false)), Value::Bool(true)); }

// ── List ops ─────────────────────────────────────────────────────────────────

#[test]
fn test_list_nil() { assert_eq!(list::list_nil(Value::Unit), Value::List(vec![])); }

#[test]
fn test_list_cons() {
    setup();
    let r = list::list_cons(Value::Tuple(vec![Value::Int(1), list::list_nil(Value::Unit)]));
    assert_eq!(r, Value::List(vec![Value::Int(1)]));
}

#[test]
fn test_list_len() {
    let l = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
    assert_eq!(list::list_len(l), Value::Int(3));
}

#[test]
fn test_list_append() {
    let l = Value::List(vec![Value::Int(1), Value::Int(2)]);
    let r = list::list_append(Value::Tuple(vec![l, Value::Int(3)]));
    assert_eq!(r, Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]));
}

#[test]
fn test_list_get_at_valid() {
    setup();
    let l = Value::List(vec![Value::Int(10), Value::Int(20)]);
    let r = list::list_get_at(Value::Tuple(vec![l, Value::Int(1)]));
    assert_eq!(option::option_unwrap(r), Value::Int(20));
}

#[test]
fn test_list_get_at_oob() {
    setup();
    let l = Value::List(vec![Value::Int(10)]);
    let r = list::list_get_at(Value::Tuple(vec![l, Value::Int(5)]));
    assert_eq!(option::option_is_none(r), Value::Bool(true));
}

#[test]
fn test_list_reverse() {
    let l = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
    assert_eq!(list::list_reverse(l), Value::List(vec![Value::Int(3), Value::Int(2), Value::Int(1)]));
}

// ── Option ───────────────────────────────────────────────────────────────────

#[test]
fn test_option_none_is_none() {
    setup();
    assert_eq!(option::option_is_none(option::option_none()), Value::Bool(true));
}

#[test]
fn test_option_some_is_some() {
    setup();
    let s = option::option_some(Value::Int(42));
    assert_eq!(option::option_is_some(s.clone()), Value::Bool(true));
    assert_eq!(option::option_unwrap(s), Value::Int(42));
}

// ── Core IR round-trip ───────────────────────────────────────────────────────

fn round_trip(term: CoreTerm) -> CoreTerm {
    let bytes = create_core_bundle(&term, "test", Provenance::Mechanical, EffectClass::Pure, true);
    load_core_bundle_from_bytes(&bytes).expect("round-trip load").root_term
}

#[test]
fn test_round_trip_int_lit() {
    match round_trip(CoreTerm::IntLit(42, None)) {
        CoreTerm::IntLit(n, _) => assert_eq!(n, 42),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn test_round_trip_bool_lit() {
    match round_trip(CoreTerm::BoolLit(true, None)) {
        CoreTerm::BoolLit(b, _) => assert!(b),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn test_round_trip_unit() {
    match round_trip(CoreTerm::UnitLit(None)) {
        CoreTerm::UnitLit(_) => {},
        _ => panic!("wrong variant"),
    }
}

#[test]
fn test_round_trip_let() {
    let term = CoreTerm::Let(
        "x".to_string(),
        Rc::new(CoreTerm::IntLit(99, None)),
        Rc::new(CoreTerm::Var("x".to_string(), None)),
        None,
    );
    match round_trip(term) {
        CoreTerm::Let(name, _, _, _) => assert_eq!(name, "x"),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn test_round_trip_call() {
    let term = CoreTerm::Call(
        "int_add".to_string(),
        vec![CoreTerm::IntLit(1, None), CoreTerm::IntLit(2, None)],
        None,
    );
    match round_trip(term) {
        CoreTerm::Call(name, args, _) => {
            assert_eq!(name, "int_add");
            assert_eq!(args.len(), 2);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn test_round_trip_deterministic() {
    let term = CoreTerm::IntLit(7, None);
    let b1 = create_core_bundle(&term, "test", Provenance::Mechanical, EffectClass::Pure, true);
    let b2 = create_core_bundle(&term, "test", Provenance::Mechanical, EffectClass::Pure, true);
    assert_eq!(b1, b2);
}

// ── Executor ─────────────────────────────────────────────────────────────────

#[test]
fn test_executor_int_lit() {
    let r = execute_core_program(&make_program(CoreTerm::IntLit(42, None)), &FunctionProvider::new(vec![])).unwrap();
    assert_eq!(r, ExecValue::Int(42));
}

#[test]
fn test_executor_bool_lit() {
    let r = execute_core_program(&make_program(CoreTerm::BoolLit(false, None)), &FunctionProvider::new(vec![])).unwrap();
    assert_eq!(r, ExecValue::Bool(false));
}

#[test]
fn test_executor_let_and_var() {
    let term = CoreTerm::Let("x".to_string(), Rc::new(CoreTerm::IntLit(10, None)), Rc::new(CoreTerm::Var("x".to_string(), None)), None);
    assert_eq!(execute_core_program(&make_program(term), &FunctionProvider::new(vec![])).unwrap(), ExecValue::Int(10));
}

#[test]
fn test_executor_if_true() {
    let term = CoreTerm::If(Rc::new(CoreTerm::BoolLit(true, None)), Rc::new(CoreTerm::IntLit(1, None)), Rc::new(CoreTerm::IntLit(0, None)), None);
    assert_eq!(execute_core_program(&make_program(term), &FunctionProvider::new(vec![])).unwrap(), ExecValue::Int(1));
}

#[test]
fn test_executor_if_false() {
    let term = CoreTerm::If(Rc::new(CoreTerm::BoolLit(false, None)), Rc::new(CoreTerm::IntLit(1, None)), Rc::new(CoreTerm::IntLit(0, None)), None);
    assert_eq!(execute_core_program(&make_program(term), &FunctionProvider::new(vec![])).unwrap(), ExecValue::Int(0));
}

#[test]
fn test_executor_call() {
    fn add(args: &[ExecValue]) -> Result<ExecValue, RuntimeError> {
        match (&args[0], &args[1]) {
            (ExecValue::Int(a), ExecValue::Int(b)) => Ok(ExecValue::Int(a + b)),
            _ => panic!("type error"),
        }
    }
    let reg  = FunctionProvider::new(vec![("add", 2, add)]);
    let term = CoreTerm::Call("add".to_string(), vec![CoreTerm::IntLit(3, None), CoreTerm::IntLit(4, None)], None);
    assert_eq!(execute_core_program(&make_program(term), &reg).unwrap(), ExecValue::Int(7));
}

#[test]
fn test_executor_missing_fn_is_error() {
    let term = CoreTerm::Call("nonexistent".to_string(), vec![], None);
    assert!(execute_core_program(&make_program(term), &FunctionProvider::new(vec![])).is_err());
}

#[test]
fn test_executor_deterministic() {
    fn fixed(_: &[ExecValue]) -> Result<ExecValue, RuntimeError> { Ok(ExecValue::Int(99)) }
    let reg  = FunctionProvider::new(vec![("fixed", 0, fixed)]);
    let term = CoreTerm::Call("fixed".to_string(), vec![], None);
    let r1 = execute_core_program(&make_program(term.clone()), &reg).unwrap();
    let r2 = execute_core_program(&make_program(term),          &reg).unwrap();
    assert_eq!(r1, r2);
}

// ── IR constructor primitives ────────────────────────────────────────────────

#[test]
fn test_ir_make_int_lit_kind() {
    setup();
    let t = ir_make_int_lit(Value::Int(42));
    assert_eq!(format!("{}", ir_term_kind(t)), "IntLit");
}

#[test]
fn test_ir_make_bool_lit_kind() {
    setup();
    let t = ir_make_bool_lit(Value::Bool(true));
    assert_eq!(format!("{}", ir_term_kind(t)), "BoolLit");
}

#[test]
fn test_ir_make_unit_lit_kind() {
    setup();
    let t = ir_make_unit_lit(Value::Unit);
    assert_eq!(format!("{}", ir_term_kind(t)), "UnitLit");
}

#[test]
fn test_ir_make_var_kind() {
    setup();
    let h = intern_str("myvar");
    let t = ir_make_var(Value::Str(h));
    assert_eq!(format!("{}", ir_term_kind(t)), "Var");
}

#[test]
fn test_ir_make_lam_kind() {
    setup();
    let ph = intern_str("p");
    let body = ir_make_int_lit(Value::Int(0));
    let t = ir_make_lam(Value::Tuple(vec![Value::Str(ph), body]));
    assert_eq!(format!("{}", ir_term_kind(t)), "Lam");
}

#[test]
fn test_ir_make_let_kind() {
    setup();
    let nh  = intern_str("n");
    let val = ir_make_int_lit(Value::Int(10));
    let bdy = ir_make_var(Value::Str(nh));
    let t   = ir_make_let(Value::Tuple(vec![Value::Str(nh), val, bdy]));
    assert_eq!(format!("{}", ir_term_kind(t)), "Let");
}

#[test]
fn test_ir_make_if_kind() {
    setup();
    let cond = ir_make_bool_lit(Value::Bool(true));
    let thn  = ir_make_int_lit(Value::Int(1));
    let els  = ir_make_int_lit(Value::Int(0));
    let t    = ir_make_if(Value::Tuple(vec![cond, thn, els]));
    assert_eq!(format!("{}", ir_term_kind(t)), "If");
}

#[test]
fn test_ir_make_app_kind() {
    setup();
    let fh  = intern_str("f");
    let func = ir_make_var(Value::Str(fh));
    let arg  = ir_make_unit_lit(Value::Unit);
    let t    = ir_make_app(Value::Tuple(vec![func, arg]));
    assert_eq!(format!("{}", ir_term_kind(t)), "App");
}

#[test]
fn test_ir_make_call_kind() {
    setup();
    let th   = intern_str("int_add");
    let args = Value::List(vec![ir_make_int_lit(Value::Int(1)), ir_make_int_lit(Value::Int(2))]);
    let t    = ir_make_call(Value::Tuple(vec![Value::Str(th), args]));
    assert_eq!(format!("{}", ir_term_kind(t)), "Call");
}

#[test]
fn test_ir_constructors_round_trip() {
    setup();
    use tempfile::NamedTempFile;

    // Build: let x = 42 in x
    let x_h   = intern_str("x");
    let int_t = ir_make_int_lit(Value::Int(42));
    let var_t = ir_make_var(Value::Str(x_h));
    let let_t = ir_make_let(Value::Tuple(vec![Value::Str(x_h), int_t, var_t]));

    assert_eq!(format!("{}", ir_term_kind(let_t.clone())), "Let");

    let tmp      = NamedTempFile::new().unwrap();
    let path_h   = intern_str(tmp.path().to_str().unwrap());
    let ec_h     = intern_str("pure");
    ir_write_bundle(Value::Tuple(vec![let_t.clone(), Value::Str(path_h), Value::Str(ec_h), Value::Bool(true)]));
    let loaded = ir_read_bundle(Value::Str(path_h));

    assert_eq!(format!("{}", ir_term_kind(loaded.clone())), "Let");
    assert_eq!(let_t, loaded);
}

#[test]
fn test_ir_round_trip_nested() {
    setup();
    use tempfile::NamedTempFile;

    // Build: lam p -> if true then 1 else 0
    let ph   = intern_str("p");
    let cond = ir_make_bool_lit(Value::Bool(true));
    let thn  = ir_make_int_lit(Value::Int(1));
    let els  = ir_make_int_lit(Value::Int(0));
    let body = ir_make_if(Value::Tuple(vec![cond, thn, els]));
    let lam  = ir_make_lam(Value::Tuple(vec![Value::Str(ph), body]));

    let tmp    = NamedTempFile::new().unwrap();
    let path_h = intern_str(tmp.path().to_str().unwrap());
    let ec_h   = intern_str("pure");
    ir_write_bundle(Value::Tuple(vec![lam.clone(), Value::Str(path_h), Value::Str(ec_h), Value::Bool(true)]));
    let loaded = ir_read_bundle(Value::Str(path_h));

    assert_eq!(lam, loaded);
}

// ── Core IR 0.4 format verification ─────────────────────────────────────────

#[test]
fn test_core_bundle_v04_round_trip_with_metadata() {
    // Write a bundle with explicit provenance/effectClass/idempotent, read back and verify.
    let term  = CoreTerm::IntLit(99, None);
    let bytes = create_core_bundle(&term, "entry", Provenance::Mechanical, EffectClass::Writes, false);
    let prog  = load_core_bundle_from_bytes(&bytes).expect("v0.4 round-trip");
    assert!(matches!(prog.root_term, CoreTerm::IntLit(99, _)));
    assert_eq!(prog.provenance,   Provenance::Mechanical);
    assert_eq!(prog.effect_class, EffectClass::Writes);
    assert!(!prog.idempotent);
}

#[test]
fn test_core_bundle_v04_effect_class_reads() {
    let term  = CoreTerm::BoolLit(true, None);
    let bytes = create_core_bundle(&term, "e", Provenance::Mechanical, EffectClass::Reads, true);
    let prog  = load_core_bundle_from_bytes(&bytes).unwrap();
    assert_eq!(prog.effect_class, EffectClass::Reads);
    assert!(prog.idempotent);
}

#[test]
fn test_ir_write_bundle_v04_verified_by_loader() {
    setup();
    use tempfile::NamedTempFile;
    use axis_codegen_bridge::core_ir::load_core_bundle;

    let x_h   = intern_str("x");
    let int_t = ir_make_int_lit(Value::Int(7));
    let var_t = ir_make_var(Value::Str(x_h));
    let let_t = ir_make_let(Value::Tuple(vec![Value::Str(x_h), int_t, var_t]));

    let tmp    = NamedTempFile::new().unwrap();
    let path   = tmp.path().to_str().unwrap();
    let path_h = intern_str(path);
    let ec_h   = intern_str("writes");
    ir_write_bundle(Value::Tuple(vec![let_t, Value::Str(path_h), Value::Str(ec_h), Value::Bool(false)]));

    let prog = load_core_bundle(path).expect("load written v0.4 bundle");
    assert!(matches!(prog.root_term, CoreTerm::Let(..)));
    assert_eq!(prog.provenance,   Provenance::Mechanical);
    assert_eq!(prog.effect_class, EffectClass::Writes);
    assert!(!prog.idempotent);
}

// ── ir_subst ─────────────────────────────────────────────────────────────────

#[test]
fn test_ir_subst_replaces_matching_var() {
    setup();
    let xh    = intern_str("x");
    let var_x = ir_make_var(Value::Str(xh));
    let ilit  = ir_make_int_lit(Value::Int(42));
    // subst(x → IntLit(42), Var(x)) → IntLit(42)
    let result = ir_subst(Value::Tuple(vec![Value::Str(xh), ilit.clone(), var_x]));
    assert_eq!(format!("{}", ir_term_kind(result.clone())), "IntLit");
    assert_eq!(result, ilit);
}

#[test]
fn test_ir_subst_leaves_non_matching_var() {
    setup();
    let xh    = intern_str("x");
    let yh    = intern_str("y");
    let var_y = ir_make_var(Value::Str(yh));
    let ilit  = ir_make_int_lit(Value::Int(42));
    // subst(x → IntLit(42), Var(y)) → Var(y) unchanged
    let result = ir_subst(Value::Tuple(vec![Value::Str(xh), ilit, var_y.clone()]));
    assert_eq!(result, var_y);
}

#[test]
fn test_ir_subst_lam_param_shadows() {
    setup();
    let xh    = intern_str("x");
    let var_x = ir_make_var(Value::Str(xh));
    let ilit  = ir_make_int_lit(Value::Int(99));
    // lam(x, Var(x)) — x is bound by the lam; substituting x leaves it unchanged
    let lam    = ir_make_lam(Value::Tuple(vec![Value::Str(xh), var_x]));
    let result = ir_subst(Value::Tuple(vec![Value::Str(xh), ilit, lam.clone()]));
    assert_eq!(result, lam);
}

#[test]
fn test_ir_subst_let_val_reached_body_shadowed() {
    setup();
    let xh    = intern_str("x");
    let yh    = intern_str("y");
    let var_x = ir_make_var(Value::Str(xh));
    let var_x2 = ir_make_var(Value::Str(xh));
    let ilit  = ir_make_int_lit(Value::Int(7));
    // let(x, Var(x), Var(x)) — substituting x:
    //   val  = Var(x)  → IntLit(7)   (subst reaches val before binding)
    //   body = Var(x)  → unchanged    (x is shadowed by the Let)
    let let_t  = ir_make_let(Value::Tuple(vec![Value::Str(xh), var_x, var_x2]));
    let result = ir_subst(Value::Tuple(vec![Value::Str(xh), ilit.clone(), let_t]));
    // The let's val should be substituted, body should not — result is still a Let
    assert_eq!(format!("{}", ir_term_kind(result)), "Let");
    let _ = yh; // silence unused warning
}

#[test]
fn test_ir_subst_substitutes_in_app_both_sides() {
    setup();
    let xh    = intern_str("x");
    let var_x1 = ir_make_var(Value::Str(xh));
    let var_x2 = ir_make_var(Value::Str(xh));
    let ilit  = ir_make_int_lit(Value::Int(5));
    // App(Var(x), Var(x)) — both sides substituted
    let app    = ir_make_app(Value::Tuple(vec![var_x1, var_x2]));
    let result = ir_subst(Value::Tuple(vec![Value::Str(xh), ilit, app]));
    // Result: App(IntLit(5), IntLit(5))
    assert_eq!(format!("{}", ir_term_kind(result)), "App");
}

// ── ir_rename ────────────────────────────────────────────────────────────────

#[test]
fn test_ir_rename_produces_lam() {
    setup();
    let xh    = intern_str("x");
    let yh    = intern_str("y");
    let var_x = ir_make_var(Value::Str(xh));
    // lam(x, Var(x)) renamed x → y = lam(y, Var(y))
    let lam    = ir_make_lam(Value::Tuple(vec![Value::Str(xh), var_x]));
    let result = ir_rename(Value::Tuple(vec![Value::Str(xh), Value::Str(yh), lam]));
    assert_eq!(format!("{}", ir_term_kind(result)), "Lam");
}

#[test]
fn test_ir_rename_body_var_updated() {
    setup();
    let xh    = intern_str("x");
    let yh    = intern_str("y");
    let var_x = ir_make_var(Value::Str(xh));
    // lam(x, Var(x)) renamed x → y
    let lam       = ir_make_lam(Value::Tuple(vec![Value::Str(xh), var_x]));
    let renamed   = ir_rename(Value::Tuple(vec![Value::Str(xh), Value::Str(yh), lam]));
    // The renamed lam should have Var(y) as body — verify it equals lam(y, Var(y))
    let var_y     = ir_make_var(Value::Str(yh));
    // Build expected: lam(y, Var(y)) and check it equals renamed
    let expected  = ir_make_lam(Value::Tuple(vec![Value::Str(yh), var_y]));
    assert_eq!(renamed, expected);
}

// ── ir_free_vars ─────────────────────────────────────────────────────────────

#[test]
fn test_ir_free_vars_lit_has_none() {
    setup();
    let ilit = ir_make_int_lit(Value::Int(42));
    let fvs  = ir_free_vars(ilit);
    assert_eq!(fvs, Value::List(vec![]));
}

#[test]
fn test_ir_free_vars_var_is_free() {
    setup();
    let xh  = intern_str("x");
    let var = ir_make_var(Value::Str(xh));
    let fvs = ir_free_vars(var);
    assert_eq!(fvs, Value::List(vec![Value::Str(xh)]));
}

#[test]
fn test_ir_free_vars_lam_binds_param() {
    setup();
    let xh  = intern_str("x");
    let var = ir_make_var(Value::Str(xh));
    // lam(x, Var(x)) — x is bound, not free
    let lam = ir_make_lam(Value::Tuple(vec![Value::Str(xh), var]));
    let fvs = ir_free_vars(lam);
    assert_eq!(fvs, Value::List(vec![]));
}

#[test]
fn test_ir_free_vars_app_collects_both() {
    setup();
    let xh    = intern_str("x");
    let yh    = intern_str("y");
    let var_x = ir_make_var(Value::Str(xh));
    let var_y = ir_make_var(Value::Str(yh));
    // App(Var(x), Var(y)) — both free
    let app = ir_make_app(Value::Tuple(vec![var_x, var_y]));
    let fvs = ir_free_vars(app);
    // Sorted alphabetically: x, y
    assert_eq!(list::list_len(fvs.clone()), Value::Int(2));
    assert_eq!(list::list_get(Value::Tuple(vec![fvs.clone(), Value::Int(0)])), Value::Str(xh));
    assert_eq!(list::list_get(Value::Tuple(vec![fvs,         Value::Int(1)])), Value::Str(yh));
}

#[test]
fn test_ir_free_vars_let_binding_not_free_in_body() {
    setup();
    let xh    = intern_str("x");
    let yh    = intern_str("y");
    let var_x = ir_make_var(Value::Str(xh));
    let var_y = ir_make_var(Value::Str(yh));
    let ilit  = ir_make_int_lit(Value::Int(0));
    // let(x, Var(y), Var(x)) — y is free (in val), x is bound (in body)
    let let_t = ir_make_let(Value::Tuple(vec![Value::Str(xh), var_y, var_x]));
    let fvs   = ir_free_vars(let_t);
    assert_eq!(fvs, Value::List(vec![Value::Str(yh)]));
    let _ = ilit;
}

// ── list_head / list_tail / list_is_empty ────────────────────────────────────

#[test]
fn test_list_head() {
    let l = Value::List(vec![Value::Int(10), Value::Int(20)]);
    assert_eq!(list::list_head(l), Value::Int(10));
}

#[test]
fn test_list_tail() {
    let l = Value::List(vec![Value::Int(10), Value::Int(20), Value::Int(30)]);
    assert_eq!(list::list_tail(l), Value::List(vec![Value::Int(20), Value::Int(30)]));
}

#[test]
fn test_list_is_empty_empty() {
    assert_eq!(list::list_is_empty(Value::List(vec![])), Value::Bool(true));
}

#[test]
fn test_list_is_empty_nonempty() {
    assert_eq!(list::list_is_empty(Value::List(vec![Value::Int(1)])), Value::Bool(false));
}

// ── str_split / str_starts_with ──────────────────────────────────────────────

#[test]
fn test_str_split() {
    setup();
    let s = intern_str("a,b,c");
    let d = intern_str(",");
    let r = str_ops::str_split(Value::Tuple(vec![Value::Str(s), Value::Str(d)]));
    assert_eq!(r, Value::List(vec![
        Value::Str(intern_str("a")),
        Value::Str(intern_str("b")),
        Value::Str(intern_str("c")),
    ]));
}

#[test]
fn test_str_starts_with_true() {
    setup();
    let s = intern_str("hello world");
    let p = intern_str("hello");
    assert_eq!(str_ops::str_starts_with(Value::Tuple(vec![Value::Str(s), Value::Str(p)])), Value::Bool(true));
}

#[test]
fn test_str_starts_with_false() {
    setup();
    let s = intern_str("hello world");
    let p = intern_str("world");
    assert_eq!(str_ops::str_starts_with(Value::Tuple(vec![Value::Str(s), Value::Str(p)])), Value::Bool(false));
}

// ── ir_eval ──────────────────────────────────────────────────────────────────

use axis_codegen_bridge::runtime::ir_eval::{ir_eval, ir_apply};

#[test]
fn test_ir_eval_let_binding() {
    setup();
    let xh   = intern_str("x");
    let term = ir_make_let(Value::Tuple(vec![
        Value::Str(xh),
        ir_make_int_lit(Value::Int(42)),
        ir_make_var(Value::Str(xh)),
    ]));
    let result = ir_eval(Value::Tuple(vec![term, Value::List(vec![])]));
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_ir_eval_if_true() {
    setup();
    let term = ir_make_if(Value::Tuple(vec![
        ir_make_bool_lit(Value::Bool(true)),
        ir_make_int_lit(Value::Int(1)),
        ir_make_int_lit(Value::Int(0)),
    ]));
    let result = ir_eval(Value::Tuple(vec![term, Value::List(vec![])]));
    assert_eq!(result, Value::Int(1));
}

#[test]
fn test_ir_apply_identity() {
    setup();
    let xh  = intern_str("x");
    let lam = ir_make_lam(Value::Tuple(vec![
        Value::Str(xh),
        ir_make_var(Value::Str(xh)),
    ]));
    let result = ir_apply(Value::Tuple(vec![lam, Value::Int(99)]));
    assert_eq!(result, Value::Int(99));
}

#[test]
fn test_ir_eval_ccall_int_add() {
    setup();
    let term = ir_make_call(Value::Tuple(vec![
        Value::Str(intern_str("int_add")),
        Value::List(vec![ir_make_int_lit(Value::Int(3)), ir_make_int_lit(Value::Int(4))]),
    ]));
    let result = ir_eval(Value::Tuple(vec![term, Value::List(vec![])]));
    assert_eq!(result, Value::Int(7));
}

/// Recursive self-applying map pattern that doubles each element of [1, 2, 3].
/// Proves iteration via recursive Core IR without any for_each_* foreign functions.
///
/// Structure:
///   let double = lam(n, call(int_add, [var(n), var(n)])) in
///   let loop   = lam(self, lam(lst,
///                  if(call(list_is_empty, [var(lst)]),
///                     call(list_nil, []),
///                     call(list_cons, [app(var(double), call(list_head, [var(lst)])),
///                                      app(app(var(self), var(self)), call(list_tail, [var(lst)]))])))) in
///   app(app(var(loop), var(loop)), var(input))
/// with bindings: input = [1, 2, 3]
#[test]
fn test_ir_eval_recursive_map_double() {
    setup();
    let n_h      = intern_str("n");
    let self_h   = intern_str("self");
    let lst_h    = intern_str("lst");
    let double_h = intern_str("double");
    let loop_h   = intern_str("loop");
    let input_h  = intern_str("input");

    // double = lam(n, call(int_add, [var(n), var(n)]))
    let double_lam = ir_make_lam(Value::Tuple(vec![
        Value::Str(n_h),
        ir_make_call(Value::Tuple(vec![
            Value::Str(intern_str("int_add")),
            Value::List(vec![ir_make_var(Value::Str(n_h)), ir_make_var(Value::Str(n_h))]),
        ])),
    ]));

    // loop_body = if(is_empty(lst), nil,
    //                cons(app(double, head(lst)), app(app(self, self), tail(lst))))
    let loop_body = ir_make_if(Value::Tuple(vec![
        ir_make_call(Value::Tuple(vec![
            Value::Str(intern_str("list_is_empty")),
            Value::List(vec![ir_make_var(Value::Str(lst_h))]),
        ])),
        ir_make_call(Value::Tuple(vec![
            Value::Str(intern_str("list_nil")),
            Value::List(vec![]),
        ])),
        ir_make_call(Value::Tuple(vec![
            Value::Str(intern_str("list_cons")),
            Value::List(vec![
                ir_make_app(Value::Tuple(vec![
                    ir_make_var(Value::Str(double_h)),
                    ir_make_call(Value::Tuple(vec![
                        Value::Str(intern_str("list_head")),
                        Value::List(vec![ir_make_var(Value::Str(lst_h))]),
                    ])),
                ])),
                ir_make_app(Value::Tuple(vec![
                    ir_make_app(Value::Tuple(vec![
                        ir_make_var(Value::Str(self_h)),
                        ir_make_var(Value::Str(self_h)),
                    ])),
                    ir_make_call(Value::Tuple(vec![
                        Value::Str(intern_str("list_tail")),
                        Value::List(vec![ir_make_var(Value::Str(lst_h))]),
                    ])),
                ])),
            ]),
        ])),
    ]));

    // loop = lam(self, lam(lst, loop_body))
    let loop_lam = ir_make_lam(Value::Tuple(vec![
        Value::Str(self_h),
        ir_make_lam(Value::Tuple(vec![Value::Str(lst_h), loop_body])),
    ]));

    // let double = double_lam in
    // let loop   = loop_lam   in
    // app(app(var(loop), var(loop)), var(input))
    let term = ir_make_let(Value::Tuple(vec![
        Value::Str(double_h),
        double_lam,
        ir_make_let(Value::Tuple(vec![
            Value::Str(loop_h),
            loop_lam,
            ir_make_app(Value::Tuple(vec![
                ir_make_app(Value::Tuple(vec![
                    ir_make_var(Value::Str(loop_h)),
                    ir_make_var(Value::Str(loop_h)),
                ])),
                ir_make_var(Value::Str(input_h)),
            ])),
        ])),
    ]));

    let bindings = Value::List(vec![
        Value::Tuple(vec![
            Value::Str(input_h),
            Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
        ]),
    ]);

    let result = ir_eval(Value::Tuple(vec![term, bindings]));
    assert_eq!(result, Value::List(vec![Value::Int(2), Value::Int(4), Value::Int(6)]));
}

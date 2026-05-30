use axis_codegen_bridge::runtime::value::{Value, intern_str, get_str, init_runtime};
use axis_codegen_bridge::runtime::{arith, str_ops, list, option, bool_ops};
use axis_codegen_bridge::core_ir::{CoreTerm, create_core_bundle, load_core_bundle_from_bytes};
use axis_codegen_bridge::executor::{execute_core_program, FunctionProvider, Value as ExecValue, RuntimeError};
use std::rc::Rc;

// Pull CoreProgram through the public loader path
use axis_codegen_bridge::core_ir::loader::CoreProgram;

fn setup() { init_runtime(); }

fn make_program(root: CoreTerm) -> CoreProgram {
    CoreProgram { root_term: root, entrypoint_id: 0 }
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
fn test_list_nil() { assert_eq!(list::list_nil(), Value::List(vec![])); }

#[test]
fn test_list_cons() {
    setup();
    let r = list::list_cons(Value::Tuple(vec![Value::Int(1), list::list_nil()]));
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
    let bytes = create_core_bundle(&term, "test");
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
    assert_eq!(create_core_bundle(&term, "test"), create_core_bundle(&term, "test"));
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

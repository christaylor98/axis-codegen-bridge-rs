//! FAULT_AS_UNKNOWN step 3: the cases that used to give a wrong value silently now fail loudly, so the
//! fault guard (step 4) turns each into an Unknown carrying its cause, instead of a plausible wrong answer.
use axis_codegen_bridge::runtime::arith::*;
use axis_codegen_bridge::runtime::fault;
use axis_codegen_bridge::runtime::tuple::*;
use axis_codegen_bridge::runtime::value::Value;

fn loud(name: &str, f: impl FnOnce() -> Value + std::panic::UnwindSafe) -> String {
    let r = std::panic::catch_unwind(f);
    let p = r.expect_err(&format!("{} returned a value instead of failing", name));
    p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap()
}

#[test]
fn each_silent_case_is_loud() {
    let cases: Vec<(&str, Box<dyn FnOnce() -> Value + std::panic::UnwindSafe>, &str)> = vec![
        ("int_add", Box::new(|| int_add(i64::MAX, 1)), "overflow"),
        ("int_sub", Box::new(|| int_sub(i64::MIN, 1)), "overflow"),
        ("int_mul", Box::new(|| int_mul(i64::MAX, 2)), "overflow"),
        ("int_abs", Box::new(|| int_abs(i64::MIN)), "overflow"),
        ("celsius_to_fahrenheit", Box::new(|| celsius_to_fahrenheit(i64::MAX)), "overflow"),
        ("fahrenheit_to_celsius", Box::new(|| fahrenheit_to_celsius(i64::MIN)), "overflow"),
        ("str_to_int", Box::new(|| str_to_int("abc".into())), "not an integer"),
        ("value_1", Box::new(|| value_1(Value::Tuple(vec![Value::Int(1)]))), "out of range"),
        ("value_0", Box::new(|| value_0(Value::Int(1))), "not a compound"),
        ("tuple_field", Box::new(|| tuple_field(Value::Tuple(vec![]), 0)), "out of range"),
        ("tuple_field<0", Box::new(|| tuple_field(Value::Tuple(vec![Value::Int(1)]), -1)), "out of range"),
        ("ctor_field", Box::new(|| ctor_field(Value::Ctor { tag: 1, fields: vec![] }, 3)), "out of range"),
    ];
    for (name, f, want) in cases {
        let msg = loud(name, f);
        assert!(msg.contains(want), "{}: message {:?} lacks {:?}", name, msg, want);
    }
    // in-range and in-bounds still compute
    assert!(matches!(int_add(2, 3), Value::Int(5)));
    assert!(matches!(str_to_int("-42".into()), Value::Int(-42)));
    assert!(matches!(value_0(Value::Tuple(vec![Value::Int(7)])), Value::Int(7)));
}

#[test]
fn a_pure_builtins_failure_is_a_defect_an_effectful_ones_is_unknown() {
    // settled semantics (pure): a defect -- unwinds with the DEFECT payload, never becomes a value
    let (a, b) = (Value::Int(i64::MAX), Value::Int(1));
    let p = std::panic::catch_unwind(|| fault::guard("int_add", false, &[&a, &b], || int_add(i64::MAX, 1)))
        .expect_err("an overflow must not return a value");
    assert!(fault::is_defect_payload(&*p));
    // the world (effectful): an Unknown carrying the cause
    let v = fault::guard("fs_read_text", true, &[], || panic!("fs_read_text(x): permission denied"));
    assert!(fault::is_unknown(&v), "got {:?}", v);
}

//! text_list_map2_text / text_list_map3_text (iter::map2 / map3): position-wise, in order, equal lengths only.
use axis_codegen_bridge::runtime::iter::{map2, map3};
use axis_codegen_bridge::runtime::value::{init_runtime, intern_str, ListBuf, Value};

fn tl(items: &[&str]) -> Value {
    Value::List(ListBuf::from(items.iter().map(|s| Value::Str(intern_str(s))).collect::<Vec<_>>()))
}
fn cat(v: Value) -> Value {
    match v {
        Value::Tuple(es) => Value::Str(intern_str(&es.iter().map(|e| match e { Value::Str(s) => s.to_string(), _ => panic!() }).collect::<String>())),
        _ => panic!("callee expects a Tuple"),
    }
}

#[test]
fn zips_in_order() {
    init_runtime();
    assert_eq!(map2(tl(&["a", "b", ""]), tl(&["1", "", "3"]), cat), tl(&["a1", "b", "3"]));
    assert_eq!(map3(tl(&["a", "b"]), tl(&["1", "2"]), tl(&["x", "y"]), cat), tl(&["a1x", "b2y"]));
    assert_eq!(map2(tl(&[]), tl(&[]), cat), tl(&[]));
}

#[test]
fn unequal_lengths_panic() {
    init_runtime();
    assert!(std::panic::catch_unwind(|| map2(tl(&["a"]), tl(&[]), cat)).is_err());
    assert!(std::panic::catch_unwind(|| map3(tl(&["a"]), tl(&["b"]), tl(&[]), cat)).is_err());
}

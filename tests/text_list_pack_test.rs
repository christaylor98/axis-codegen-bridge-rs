//! text_list_pack / text_list_unpack and value_to_text_list / value_to_int_list (axMachina P5's list vectors).
use axis_codegen_bridge::runtime::list::{text_list_pack, text_list_unpack, value_to_int_list, value_to_text_list};
use axis_codegen_bridge::runtime::value::{init_runtime, intern_str, ListBuf, Value};

fn tl(items: &[&str]) -> Value {
    Value::List(ListBuf::from(items.iter().map(|s| Value::Str(intern_str(s))).collect::<Vec<_>>()))
}

fn packed(items: &[&str]) -> String {
    match text_list_pack(tl(items)) { Value::Str(s) => s.to_string(), other => panic!("not Text: {other:?}") }
}

#[test]
fn pack_encoding() {
    init_runtime();
    assert_eq!(packed(&[]), "0||");
    assert_eq!(packed(&[""]), "1|0|");
    assert_eq!(packed(&["", ""]), "2|0,0|");
    assert_eq!(packed(&["ab", "c"]), "2|2,1|abc");
    // code points, not bytes
    assert_eq!(packed(&["é", "日本"]), "2|1,2|é日本");
}

#[test]
fn round_trip_any_text() {
    init_runtime();
    let cases: Vec<Vec<&str>> = vec![
        vec![], vec![""], vec!["", ""], vec!["a"], vec!["|"], vec!["1|1|x", ",", "||"], vec!["\0", "\n\t", "\u{1f}"],
        vec!["é", "日本", "🙂x"], vec!["0||"], vec!["12", "345"], vec!["", "a", ""],
    ];
    for c in &cases {
        let p = text_list_pack(tl(c));
        let s = match &p { Value::Str(s) => s.clone(), _ => unreachable!() };
        assert_eq!(text_list_unpack(s), tl(c), "case {c:?}");
    }
}

#[test]
fn pack_is_injective_on_tricky_pairs() {
    init_runtime();
    // lists a naive join would collide on
    let pairs: Vec<(Vec<&str>, Vec<&str>)> = vec![
        (vec!["a,b"], vec!["a", "b"]), (vec!["ab", ""], vec!["a", "b"]), (vec![""], vec![]), (vec!["", ""], vec![""]),
        (vec!["1|1|a"], vec!["a"]), (vec!["é"], vec!["e\u{301}"]),
    ];
    for (a, b) in &pairs { assert_ne!(packed(a), packed(b), "{a:?} vs {b:?}"); }
}

fn unpack_panics(t: &str) -> String {
    let t = intern_str(t);
    let r = std::panic::catch_unwind(move || text_list_unpack(t));
    match r {
        Ok(v) => panic!("expected a panic, got {v:?}"),
        Err(e) => e.downcast_ref::<String>().cloned().unwrap_or_default(),
    }
}

#[test]
fn unpack_refuses_non_packings() {
    init_runtime();
    for (t, why) in [
        ("", "no count"), ("x||", "count is not a number"), ("1", "no count"), ("1|", "no lengths"), ("1|x|a", "a length is not a number"),
        ("1|2|a", "items shorter"), ("1|1|ab", "items longer"), ("2|1|a", "count and lengths differ"),
        ("0|1|", "count 0 with lengths"), ("0||x", "items longer"),
    ] {
        let m = unpack_panics(t);
        assert!(m.contains(why), "{t:?}: {m}");
    }
}

#[test]
fn narrowing_list_accessors() {
    init_runtime();
    assert_eq!(value_to_text_list(tl(&["a", "b"])), tl(&["a", "b"]));
    let il = Value::List(ListBuf::from(vec![Value::Int(7), Value::Int(8)]));
    assert_eq!(value_to_int_list(il.clone()), il.clone());
    let m = std::panic::catch_unwind(|| value_to_text_list(Value::List(ListBuf::from(vec![Value::Int(7)])))).unwrap_err();
    assert!(m.downcast_ref::<String>().unwrap().contains("element 0 is Int, expected Text"));
    let m = std::panic::catch_unwind(|| value_to_int_list(Value::Int(3))).unwrap_err();
    assert!(m.downcast_ref::<String>().unwrap().contains("expected ValueList, got Int"));
}

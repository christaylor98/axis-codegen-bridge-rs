//! prog_tokens (codegen layer): the rendered-program tokenizer.
use axis_codegen_bridge::runtime::prog::*;
use axis_codegen_bridge::runtime::value::Value;
use std::sync::Arc;

fn toks(s: &str) -> Vec<String> {
    match prog_tokens(Arc::from(s)) { Value::List(l) => l.iter().map(|v| match v { Value::Str(s) => s.to_string(), o => panic!("{o:?}") }).collect(), o => panic!("{o:?}") }
}

#[test]
fn tokens() {
    assert_eq!(toks("p0"), vec!["Ip0"]);
    assert_eq!(toks("str_concat(p0,c(4:t:a,))"), vec!["Fstr_concat", "Ip0", ",", "Lt:a,", ")"]);
    assert_eq!(toks("c(2:t:)"), vec!["Lt:"]);
    assert_eq!(toks("f(c(5:t:é()))"), vec!["Ff", "Lt:é()", ")"]);
    assert_eq!(toks("c(9:t:x)"), vec!["!"]);
    assert_eq!(toks("Str(p0)"), vec!["!"]);
    assert_eq!(toks(""), Vec::<String>::new());
}

fn spans(s: &str) -> Vec<String> {
    match prog_spans(Arc::from(s)) { Value::List(l) => l.iter().map(|v| match v { Value::Str(s) => s.to_string(), o => panic!("{o:?}") }).collect(), o => panic!("{o:?}") }
}

#[test]
fn call_spans() {
    // f(c(4:t:a,),g(p0)): f spans 0..18 closing at token 6; g spans 12..17 closing at token 5
    let p = "f(c(4:t:a,),g(p0))";
    assert_eq!(spans(p), vec!["0,18,6", "", "", "12,17,5", "", "", ""]);
    let s: Vec<char> = p.chars().collect();
    assert_eq!(s[12..17].iter().collect::<String>(), "g(p0)");
    assert_eq!(spans("p0"), vec![""]);
}

//! str_nat_or (codegen layer): exactly the drivers' old digit check (non-empty, all of 0-9, under 19 chars).
use axis_codegen_bridge::runtime::num::*;
use axis_codegen_bridge::runtime::value::Value;
use std::sync::Arc;

fn n(s: &str, d: i64) -> i64 { match str_nat_or(Arc::from(s), d) { Value::Int(v) => v, o => panic!("{o:?}") } }

#[test]
fn naturals_and_everything_else() {
    assert_eq!(n("0", 7), 0);
    assert_eq!(n("042", 7), 42);
    assert_eq!(n("123456789012345678", 7), 123456789012345678);
    assert_eq!(n("1234567890123456789", 7), 7);
    assert_eq!(n("", 7), 7);
    assert_eq!(n("-1", 7), 7);
    assert_eq!(n("+1", 7), 7);
    assert_eq!(n(" 1", 7), 7);
    assert_eq!(n("1a", 7), 7);
    assert_eq!(n("١", 7), 7);
}

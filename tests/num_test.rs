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

fn r(a: &str, b: &str) -> i64 { match str_sim_ratio(Arc::from(a), Arc::from(b)) { Value::Int(v) => v, o => panic!("{o:?}") } }

#[test]
fn sim_ratio_matches_difflib() {
    // expected values from Python 3 difflib.SequenceMatcher(None, a, b).ratio(), * 1e12, rounded
    let cases = include_str!("fixtures/sim_ratio.tsv");
    let mut n = 0;
    for line in cases.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let un = |s: &str| s.replace("\\t", "\t").replace("\\n", "\n");
        assert_eq!(r(&un(f[0]), &un(f[1])), f[2].parse::<i64>().unwrap(), "{line}");
        n += 1;
    }
    assert!(n >= 200);
}

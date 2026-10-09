//! fbank_free (codegen layer): a bank used, freed, and many banks allocated and freed in turn without the process
//! keeping their pages (resident memory stays flat where leaking them would grow it by gigabytes).
use axis_codegen_bridge::runtime::fbank::*;
use axis_codegen_bridge::runtime::value::Value;
use std::sync::Arc;

fn int(v: Value) -> i64 { match v { Value::Int(n) => n, o => panic!("not Int: {o:?}") } }

fn rss() -> i64 {
    let s = std::fs::read_to_string("/proc/self/statm").unwrap();
    s.split_whitespace().nth(1).unwrap().parse::<i64>().unwrap() * 4096
}

#[test]
fn free_returns_the_pages() {
    let b = int(fbank_new(1000, 1 << 20));
    assert_eq!(int(fbank_put(b, 0, Arc::from("a"))), 0);
    assert_eq!(int(fbank_put(b, 1, Arc::from("a"))), -1);
    assert!(matches!(fbank_free(b), Value::Unit));
    let before = rss();
    for _ in 0..20 {
        let b = int(fbank_new(1 << 20, 256 << 20));
        for i in 0..200_000i64 { fbank_put(b, i, Arc::from(format!("entry-{i}-{}", "x".repeat(400)).as_str())); }
        fbank_free(b);
    }
    let grew = rss() - before;
    assert!(grew < 512 << 20, "resident memory grew {grew} bytes over 20 used-and-freed banks");
}

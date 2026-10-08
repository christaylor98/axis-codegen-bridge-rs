//! FBANK_V1: frozen bank semantics, the lowest-ordinal-wins rule under real concurrency, capacity, and
//! handle_list_make with one handle.
use axis_codegen_bridge::runtime::fbank::*;
use axis_codegen_bridge::runtime::tasks::handle_list_make;
use axis_codegen_bridge::runtime::value::{init_runtime, intern_str, Value};
use std::collections::HashMap;

fn int(v: Value) -> i64 { match v { Value::Int(n) => n, o => panic!("not Int: {o:?}") } }
fn yes(v: Value) -> bool { match v { Value::Bool(b) => b, o => panic!("not Bool: {o:?}") } }
fn text(v: Value) -> String { match v { Value::Str(s) => s.to_string(), o => panic!("not Text: {o:?}") } }

#[test]
fn put_get_dedupe_sequential() {
    init_runtime();
    let b = int(fbank_new(16, 1024));
    assert_eq!(int(fbank_put(b, 0, intern_str("abc"))), 0);
    assert_eq!(int(fbank_put(b, 1, intern_str("é日"))), 1);
    assert_eq!(int(fbank_put(b, 2, intern_str("abc"))), -1, "a higher ordinal is refused");
    assert_eq!(int(fbank_put(b, 3, intern_str(""))), 2, "empty text is content too; a refused put took no id");
    assert_eq!(int(fbank_put(b, 4, intern_str(""))), -1);
    assert_eq!(text(fbank_get(b, 0)), "abc");
    assert_eq!(text(fbank_get(b, 1)), "é日");
    assert_eq!(text(fbank_get(b, 2)), "");
    assert!(yes(fbank_holds(b, 0)) && yes(fbank_holds(b, 1)) && yes(fbank_holds(b, 2)));

    assert_eq!(int(fbank_ordinal(b, 1)), 1);
    assert_eq!(int(fbank_len(b)), 3, "refused puts take no id");
    assert_eq!(int(fbank_used(b)), 3 + 5, "nor bytes");
}

#[test]
fn lower_ordinal_displaces_a_higher_holder() {
    init_runtime();
    let b = int(fbank_new(8, 64));
    let late = int(fbank_put(b, 50, intern_str("x")));
    let early = int(fbank_put(b, 7, intern_str("x")));
    assert!(late >= 0 && early >= 0);
    assert!(!yes(fbank_holds(b, late)) && yes(fbank_holds(b, early)));
    assert_eq!(int(fbank_put(b, 9, intern_str("x"))), -1);
}

#[test]
fn full_is_minus_two() {
    init_runtime();
    let b = int(fbank_new(2, 100));
    assert!(int(fbank_put(b, 0, intern_str("a"))) >= 0);
    assert!(int(fbank_put(b, 1, intern_str("b"))) >= 0);
    assert_eq!(int(fbank_put(b, 2, intern_str("c"))), -2, "entries exhausted");
    let b = int(fbank_new(10, 4));
    assert!(int(fbank_put(b, 0, intern_str("abc"))) >= 0);
    assert_eq!(int(fbank_put(b, 1, intern_str("de"))), -2, "bytes exhausted");
}

#[test]
fn invented_id_panics() {
    init_runtime();
    let b = int(fbank_new(4, 16));
    let r = std::panic::catch_unwind(|| fbank_get(b, 0));
    assert!(r.is_err());
}

/// 32 threads put 4,000 contents x 8 copies each, every copy with a distinct ordinal, in a different scrambled
/// order per thread. Afterwards every content has exactly one holder and it is the copy with the lowest ordinal.
#[test]
fn concurrent_lowest_ordinal_wins() {
    init_runtime();
    const CONTENTS: i64 = 4000;
    const COPIES: i64 = 8;
    const THREADS: i64 = 32;
    let total = CONTENTS * COPIES;
    let b = int(fbank_new(total, total * 16));
    // ordinal o puts content (o * 7919) % CONTENTS: every content gets COPIES ordinals spread over the range
    let content = |o: i64| format!("c{}", (o * 7919) % CONTENTS);
    let ids: Vec<Vec<(i64, i64)>> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..THREADS).map(|t| s.spawn(move || {
            let mut mine: Vec<i64> = (0..total).filter(|o| o % THREADS == t).collect();
            // scramble per thread (deterministic)
            let n = mine.len();
            for i in 0..n { mine.swap(i, (i * 2654435761usize + t as usize * 97) % n); }
            mine.iter().map(|&o| (o, int(fbank_put(b, o, intern_str(&content(o)))))).collect::<Vec<_>>()
        })).collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut min_ord: HashMap<String, i64> = HashMap::new();
    for o in 0..total { let e = min_ord.entry(content(o)).or_insert(o); if o < *e { *e = o; } }
    let mut holders: HashMap<String, i64> = HashMap::new();
    for (o, id) in ids.into_iter().flatten() {
        assert!(id >= -1, "no put may be refused as full");
        if id >= 0 && yes(fbank_holds(b, id)) {
            assert_eq!(text(fbank_get(b, id)), content(o));
            assert!(holders.insert(content(o), o).is_none(), "two holders for {}", content(o));
        }
    }
    assert_eq!(holders.len() as i64, CONTENTS);
    assert_eq!(holders, min_ord, "the holder of every content is its lowest ordinal");
}

#[test]
fn handle_list_make_one_handle() {
    init_runtime();
    let one = Value::Tuple(vec![Value::Int(5), Value::Unit]);
    match handle_list_make(one.clone()) { Value::List(l) => assert_eq!(&l[..], &[one.clone()]), o => panic!("{o:?}") }
    let two = Value::Tuple(vec![one.clone(), one.clone()]);
    match handle_list_make(two) { Value::List(l) => assert_eq!(l.len(), 2), o => panic!("{o:?}") }
    match handle_list_make(Value::Unit) { Value::List(l) => assert!(l.is_empty()), o => panic!("{o:?}") }
}

#[test]
fn find_is_read_only() {
    init_runtime();
    let b = int(fbank_new(8, 64));
    assert_eq!(int(fbank_find(b, intern_str("x"))), -1);
    assert_eq!(int(fbank_len(b)), 0, "find puts nothing");
    let late = int(fbank_put(b, 9, intern_str("x")));
    assert_eq!(int(fbank_find(b, intern_str("x"))), late);
    let early = int(fbank_put(b, 2, intern_str("x")));
    assert_eq!(int(fbank_find(b, intern_str("x"))), early, "find answers the current holder");
    assert_eq!(int(fbank_find(b, intern_str("y"))), -1);
}

fn tlist(items: &[&str]) -> Value {
    Value::List(axis_codegen_bridge::runtime::value::ListBuf::from(items.iter().map(|s| Value::Str(intern_str(s))).collect::<Vec<_>>()))
}
fn texts(v: Value) -> Vec<String> {
    match v { Value::List(l) => l.iter().map(|x| match x { Value::Str(s) => s.to_string(), o => panic!("{o:?}") }).collect(), o => panic!("{o:?}") }
}

#[test]
fn intern_and_get_lists() {
    init_runtime();
    let b = int(fbank_new(16, 256));
    let ids = texts(fbank_intern_list(b, tlist(&["a", "bb", "a", "", "bb"])));
    assert_eq!(ids[0], ids[2]);
    assert_eq!(ids[1], ids[4]);
    assert!(ids[0] != ids[1] && ids[0] != ids[3] && ids[1] != ids[3]);
    assert_eq!(int(fbank_len(b)), 3, "each text stored once");
    let back = texts(fbank_get_list(b, tlist(&ids.iter().map(|s| s.as_str()).collect::<Vec<_>>())));
    assert_eq!(back, vec!["a", "bb", "a", "", "bb"]);
    let small = int(fbank_new(1, 16));
    assert_eq!(texts(fbank_intern_list(small, tlist(&["x", "y"]))), vec!["0", "-2"], "full -> -2 in place");
}

/// 32 threads intern 2,000 texts each, all drawn from the same 500: every text gets ONE id in every thread.
#[test]
fn concurrent_intern_agrees() {
    init_runtime();
    let b = int(fbank_new(4000, 64000));
    let got: Vec<Vec<(String, String)>> = std::thread::scope(|s| {
        (0..32).map(|t| s.spawn(move || {
            let xs: Vec<String> = (0..2000).map(|i| format!("v{}", (i * 7 + t * 13) % 500)).collect();
            let ids = texts(fbank_intern_list(b, tlist(&xs.iter().map(|s| s.as_str()).collect::<Vec<_>>())));
            xs.into_iter().zip(ids).collect::<Vec<_>>()
        })).collect::<Vec<_>>().into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut idof: HashMap<String, String> = HashMap::new();
    for (x, id) in got.into_iter().flatten() {
        let e = idof.entry(x.clone()).or_insert(id.clone());
        assert_eq!(*e, id, "{x} got two ids");
        assert_eq!(text(fbank_get(b, id.parse().unwrap())), x);
    }
    assert_eq!(idof.len(), 500);
    let distinct: std::collections::HashSet<_> = idof.values().collect();
    assert_eq!(distinct.len(), 500, "distinct texts, distinct ids");
}

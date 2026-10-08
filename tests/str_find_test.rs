//! str_find_from / str_starts_with_at (Python's find(s, from) / startswith(s, pos) in code points), and str_slice
//! after it stopped copying the whole text: each against a naive Vec<char> reference on every (from, needle) over
//! ASCII and non-ASCII texts, overlapping occurrences, empty needles, and positions past the end.
use axis_codegen_bridge::runtime::str_ops::{str_find_from, str_slice, str_starts_with_at};
use axis_codegen_bridge::runtime::value::Value;
use std::sync::Arc;

fn a(s: &str) -> Arc<str> { Arc::from(s) }
fn int(v: Value) -> i64 { match v { Value::Int(i) => i, o => panic!("not an Int: {:?}", o) } }
fn boolv(v: Value) -> bool { match v { Value::Bool(b) => b, o => panic!("not a Bool: {:?}", o) } }
fn text(v: Value) -> String { match v { Value::Str(s) => s.to_string(), o => panic!("not a Str: {:?}", o) } }

fn find_ref(h: &[char], n: &[char], from: usize) -> i64 {
    if from > h.len() { return -1; }
    (from..=h.len()).find(|&i| i + n.len() <= h.len() && h[i..i + n.len()] == *n).map(|i| i as i64).unwrap_or(-1)
}

const TEXTS: [&str; 6] = ["", "aaa", "abcabc", "é漢éé漢", "a\tb\tc\t", "xxyxxyxx"];
const NEEDLES: [&str; 8] = ["", "a", "aa", "bc", "é", "漢é", "\t", "xx"];

#[test]
fn find_from_matches_python_find() {
    for t in TEXTS {
        let h: Vec<char> = t.chars().collect();
        for nd in NEEDLES {
            let n: Vec<char> = nd.chars().collect();
            for from in 0..=h.len() + 2 {
                assert_eq!(int(str_find_from(a(t), a(nd), from as i64)), find_ref(&h, &n, from), "{:?}.find({:?}, {})", t, nd, from);
            }
        }
    }
    assert_eq!(int(str_find_from(a("aaa"), a("a"), -1)), -1);
    assert_eq!(int(str_find_from(a("aaa"), a("aa"), 1)), 1, "overlapping");
    assert_eq!(int(str_find_from(a("ab"), a(""), 2)), 2, "empty needle at the end");
    assert_eq!(int(str_find_from(a("ab"), a(""), 3)), -1, "past the end");
}

#[test]
fn starts_with_at_matches_python_startswith() {
    for t in TEXTS {
        let h: Vec<char> = t.chars().collect();
        for nd in NEEDLES {
            let n: Vec<char> = nd.chars().collect();
            for pos in 0..=h.len() + 2 {
                let want = pos <= h.len() && pos + n.len() <= h.len() && h[pos..pos + n.len()] == *n;
                assert_eq!(boolv(str_starts_with_at(a(t), a(nd), pos as i64)), want, "{:?}.startswith({:?}, {})", t, nd, pos);
            }
        }
    }
    assert!(!boolv(str_starts_with_at(a("abc"), a("a"), -1)));
}

#[test]
fn slice_unchanged() {
    for t in TEXTS {
        let h: Vec<char> = t.chars().collect();
        for s in 0..=h.len() {
            for e in s..h.len() + 3 {
                let want: String = h[s..e.min(h.len())].iter().collect();
                assert_eq!(text(str_slice(a(t), s as i64, e as i64)), want, "{:?}[{}..{}]", t, s, e);
            }
        }
        let all: String = h.iter().collect();
        assert_eq!(text(str_slice(a(t), 0, -1)), all, "a negative end clamps to the length, as before");
    }
}

#[test]
#[should_panic]
fn slice_start_past_end_panics() { str_slice(a("ab"), 3, 5); }

#[test]
#[should_panic]
fn slice_start_after_end_panics() { str_slice(a("abcd"), 3, 1); }

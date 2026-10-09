//! G1 line ops (codegen layer): each op on ordinary, edge (empty, no trailing newline, blank lines, missing columns)
//! inputs, and the corpus compositions they replace.
use axis_codegen_bridge::runtime::lines::*;
use axis_codegen_bridge::runtime::value::Value;
use std::sync::Arc;

fn t(s: &str) -> Arc<str> { Arc::from(s) }
fn txt(v: Value) -> String { match v { Value::Str(s) => s.to_string(), o => panic!("not Text: {o:?}") } }
fn int(v: Value) -> i64 { match v { Value::Int(n) => n, o => panic!("not Int: {o:?}") } }

#[test]
fn reading_and_writing_lines() {
    assert_eq!(int(lines_count(t(""))), 0);
    assert_eq!(int(lines_count(t("a"))), 1);
    assert_eq!(int(lines_count(t("a\n"))), 1);
    assert_eq!(int(lines_count(t("a\n\nb"))), 3);
    assert_eq!(int(lines_count(t("\n"))), 1);
    assert_eq!(txt(lines_reverse(t("a\nb\nc"))), "c\nb\na\n");
    assert_eq!(txt(lines_reverse(t(""))), "");
}

#[test]
fn filters() {
    assert_eq!(txt(lines_nonblank(t("a\n\n  \nb\n"))), "a\nb\n");
    assert_eq!(txt(lines_nonblank(t(" \n"))), "");
    assert_eq!(txt(lines_with_prefix(t("fn a\n fn b\nfn c"), t("fn "))), "fn a\nfn c\n");
    assert_eq!(txt(lines_with_prefix(t("x\ny"), t(""))), "x\ny\n");
    assert_eq!(txt(lines_containing(t("ab\ncd\nxbx"), t("b"))), "ab\nxbx\n");
}

#[test]
fn columns() {
    let s = "f\t(Int)\tInt\ng\t(Text)\tText\nh\nk\t(Int)";
    assert_eq!(txt(lines_col_eq(t(s), 1, t("(Int)"))), "f\t(Int)\tInt\nk\t(Int)\n");
    assert_eq!(txt(lines_col_ne(t(s), 1, t("(Int)"))), "g\t(Text)\tText\nh\n");
    assert_eq!(txt(lines_col_eq(t(s), -1, t("f"))), "");
    assert_eq!(txt(lines_col_eq(t(s), 9, t(""))), "");
    assert_eq!(txt(lines_head_cols(t("a\tb\tc\td\nx\ny\tz"), 3)), "a\tb\tc\nx\ny\tz\n");
    assert_eq!(txt(lines_head_cols(t("a\tb"), 0)), "\n");
}

#[test]
fn corpus_compositions() {
    // v1 copy_lines / reverse_lines / count_fns / build_slot1, as one or two calls.
    let f = "x\n\ny\n \nz";
    assert_eq!(txt(lines_nonblank(t(f))), "x\ny\nz\n");
    assert_eq!(txt(lines_reverse(lines_nonblank(t(f)).as_text())), "z\ny\nx\n");
    assert_eq!(int(lines_count(lines_with_prefix(t("fn a\nend\nfn b\n"), t("fn ")).as_text())), 2);
}

trait AsText { fn as_text(self) -> Arc<str>; }
impl AsText for Value { fn as_text(self) -> Arc<str> { match self { Value::Str(s) => s, o => panic!("not Text: {o:?}") } } }

#[test]
fn lookup() {
    let s = "f\t(Int)\tInt\ng\t(Text)\tText\nf\tx\ty";
    assert_eq!(txt(lines_col_of(t(s), t("g"), 2)), "Text");
    assert_eq!(txt(lines_col_of(t(s), t("f"), 1)), "(Int)");
    assert_eq!(txt(lines_col_of(t(s), t("h"), 1)), "");
    assert_eq!(txt(lines_col_of(t(s), t("g"), 7)), "");
    assert_eq!(txt(lines_col_of(t(s), t("g"), -1)), "");
}

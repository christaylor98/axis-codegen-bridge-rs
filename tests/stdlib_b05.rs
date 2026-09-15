//! stdlib(B05-T): typed list access — mint.py aliases (AXIS_STDLIB_DESIGN_V1).
//!
//! Written from the B05 spec table BEFORE reading the A-stage implementation
//! (T-stage rule: expectations come from the spec, not the code). `list_last`
//! is new Rust; every other fn in this brief is a mint.py alias with no Rust
//! of its own, so its test calls the `source` fn directly, one #[test] per
//! registry name (25 fns) so a failure names exactly which alias regressed
//! even though several share one underlying implementation:
//!   - int_list_get / text_list_get / bool_list_get        -> list::list_get
//!   - int_list_last / text_list_last / bool_list_last     -> list::list_last (new)
//!   - int_list_append / text_list_append / bool_list_append -> list::list_append
//!   - int_list_concat / text_list_concat / bool_list_concat -> list::list_concat
//!   - int_list_reverse / text_list_reverse / bool_list_reverse -> list::list_reverse
//!   - int_list_take / text_list_take / bool_list_take     -> iter::take
//!   - int_list_drop / text_list_drop / bool_list_drop     -> iter::drop
//!   - int_list_slice / text_list_slice / bool_list_slice  -> iter::slice
//!
//! `take`/`drop`/`slice` take a single boxed `Value::Tuple` argument (the
//! pre-existing untyped calling convention — see `native_call_fn_arg_types()`
//! in `src/emit/rust_05.rs`, which has no entry for any of the three), not
//! separate native params like `list_get`.
//!
//! `take`/`drop`/`slice` never panic on n<0, n>len, or from>to — the spec's
//! semantics column calls this out explicitly ("n is clamped to [0, len]
//! (source semantics: never panics ...)" / "both bounds clamped to [0, len]
//! and to >= from enforced by clamping"). Those cases are asserted against
//! the clamped result, not `#[should_panic]`.

use axis_codegen_bridge::runtime::iter;
use axis_codegen_bridge::runtime::list;
use axis_codegen_bridge::runtime::value::{intern_str, Value};

fn i(n: i64) -> Value {
    Value::Int(n)
}

fn t(x: &str) -> Value {
    Value::Str(intern_str(x))
}

fn int_list(xs: &[i64]) -> Value {
    Value::List(xs.iter().map(|n| i(*n)).collect())
}

fn text_list(xs: &[&str]) -> Value {
    Value::List(xs.iter().map(|x| t(x)).collect())
}

fn bool_list(xs: &[bool]) -> Value {
    Value::List(xs.iter().map(|b| Value::Bool(*b)).collect())
}

// ── list_last (new leaf fn) ─────────────────────────────────────────────

#[test]
fn list_last_basic() {
    assert_eq!(list::list_last(int_list(&[1, 2, 3])), i(3));
}

#[test]
#[should_panic]
fn list_last_empty_panics() {
    list::list_last(Value::List(vec![]));
}

// ── int_list_get / text_list_get / bool_list_get -> list::list_get ──────

#[test]
fn int_list_get_basic() {
    assert_eq!(list::list_get(int_list(&[10, 20, 30]), 1), i(20));
}

#[test]
#[should_panic]
fn int_list_get_out_of_range_panics() {
    list::list_get(int_list(&[10, 20, 30]), 99);
}

#[test]
fn text_list_get_basic() {
    assert_eq!(list::list_get(text_list(&["a", "b", "c"]), 2), t("c"));
}

#[test]
#[should_panic]
fn text_list_get_out_of_range_panics() {
    list::list_get(text_list(&["a", "b", "c"]), 99);
}

#[test]
fn bool_list_get_basic() {
    assert_eq!(list::list_get(bool_list(&[true, false, true]), 0), Value::Bool(true));
}

#[test]
#[should_panic]
fn bool_list_get_out_of_range_panics() {
    list::list_get(bool_list(&[true, false, true]), 99);
}

// ── int_list_last / text_list_last / bool_list_last -> list::list_last ──

#[test]
fn int_list_last_basic() {
    assert_eq!(list::list_last(int_list(&[10, 20, 30])), i(30));
}

#[test]
#[should_panic]
fn int_list_last_empty_panics() {
    list::list_last(Value::List(vec![]));
}

#[test]
fn text_list_last_basic() {
    assert_eq!(list::list_last(text_list(&["a", "b", "c"])), t("c"));
}

#[test]
#[should_panic]
fn text_list_last_empty_panics() {
    list::list_last(Value::List(vec![]));
}

#[test]
fn bool_list_last_basic() {
    assert_eq!(list::list_last(bool_list(&[true, false])), Value::Bool(false));
}

#[test]
#[should_panic]
fn bool_list_last_empty_panics() {
    list::list_last(Value::List(vec![]));
}

// ── int_list_append / text_list_append / bool_list_append -> list::list_append ─

#[test]
fn int_list_append_basic() {
    assert_eq!(
        list::list_append(int_list(&[1, 2]), i(3)),
        int_list(&[1, 2, 3])
    );
}

#[test]
fn text_list_append_basic() {
    assert_eq!(
        list::list_append(text_list(&["a", "b"]), t("c")),
        text_list(&["a", "b", "c"])
    );
}

#[test]
fn bool_list_append_basic() {
    assert_eq!(
        list::list_append(bool_list(&[true]), Value::Bool(false)),
        bool_list(&[true, false])
    );
}

// ── int_list_concat / text_list_concat / bool_list_concat -> list::list_concat ─

#[test]
fn int_list_concat_basic() {
    assert_eq!(
        list::list_concat(int_list(&[1, 2]), int_list(&[3, 4])),
        int_list(&[1, 2, 3, 4])
    );
}

#[test]
fn text_list_concat_basic() {
    assert_eq!(
        list::list_concat(text_list(&["a"]), text_list(&["b", "c"])),
        text_list(&["a", "b", "c"])
    );
}

#[test]
fn bool_list_concat_basic() {
    assert_eq!(
        list::list_concat(bool_list(&[true]), bool_list(&[false])),
        bool_list(&[true, false])
    );
}

// ── int_list_reverse / text_list_reverse / bool_list_reverse -> list::list_reverse ─

#[test]
fn int_list_reverse_basic() {
    assert_eq!(list::list_reverse(int_list(&[1, 2, 3])), int_list(&[3, 2, 1]));
}

#[test]
fn text_list_reverse_basic() {
    assert_eq!(
        list::list_reverse(text_list(&["a", "b", "c"])),
        text_list(&["c", "b", "a"])
    );
}

#[test]
fn bool_list_reverse_basic() {
    assert_eq!(
        list::list_reverse(bool_list(&[true, false])),
        bool_list(&[false, true])
    );
}

// ── int_list_take / text_list_take / bool_list_take -> iter::take ───────
// take(args: Tuple(List, Int)) -> List; first n elements, all if n >= len.

#[test]
fn int_list_take_basic() {
    assert_eq!(
        iter::take(Value::Tuple(vec![int_list(&[1, 2, 3, 4]), i(2)])),
        int_list(&[1, 2])
    );
}

#[test]
fn int_list_take_n_ge_len_returns_all() {
    assert_eq!(
        iter::take(Value::Tuple(vec![int_list(&[1, 2]), i(10)])),
        int_list(&[1, 2])
    );
}

#[test]
fn int_list_take_negative_n_clamps_to_empty() {
    assert_eq!(
        iter::take(Value::Tuple(vec![int_list(&[1, 2, 3]), i(-1)])),
        int_list(&[])
    );
}

#[test]
fn text_list_take_basic() {
    assert_eq!(
        iter::take(Value::Tuple(vec![text_list(&["a", "b", "c"]), i(1)])),
        text_list(&["a"])
    );
}

#[test]
fn text_list_take_negative_n_clamps_to_empty() {
    assert_eq!(
        iter::take(Value::Tuple(vec![text_list(&["a", "b"]), i(-1)])),
        text_list(&[])
    );
}

#[test]
fn bool_list_take_basic() {
    assert_eq!(
        iter::take(Value::Tuple(vec![bool_list(&[true, false, true]), i(2)])),
        bool_list(&[true, false])
    );
}

#[test]
fn bool_list_take_negative_n_clamps_to_empty() {
    assert_eq!(
        iter::take(Value::Tuple(vec![bool_list(&[true, false]), i(-1)])),
        bool_list(&[])
    );
}

// ── int_list_drop / text_list_drop / bool_list_drop -> iter::drop ───────
// drop(args: Tuple(List, Int)) -> List; all but the first n.

#[test]
fn int_list_drop_basic() {
    assert_eq!(
        iter::drop(Value::Tuple(vec![int_list(&[1, 2, 3, 4]), i(2)])),
        int_list(&[3, 4])
    );
}

#[test]
fn int_list_drop_negative_n_clamps_to_whole_list() {
    assert_eq!(
        iter::drop(Value::Tuple(vec![int_list(&[1, 2, 3]), i(-1)])),
        int_list(&[1, 2, 3])
    );
}

#[test]
fn text_list_drop_basic() {
    assert_eq!(
        iter::drop(Value::Tuple(vec![text_list(&["a", "b", "c"]), i(1)])),
        text_list(&["b", "c"])
    );
}

#[test]
fn text_list_drop_negative_n_clamps_to_whole_list() {
    assert_eq!(
        iter::drop(Value::Tuple(vec![text_list(&["a", "b"]), i(-1)])),
        text_list(&["a", "b"])
    );
}

#[test]
fn bool_list_drop_basic() {
    assert_eq!(
        iter::drop(Value::Tuple(vec![bool_list(&[true, false, true]), i(1)])),
        bool_list(&[false, true])
    );
}

#[test]
fn bool_list_drop_negative_n_clamps_to_whole_list() {
    assert_eq!(
        iter::drop(Value::Tuple(vec![bool_list(&[true, false]), i(-1)])),
        bool_list(&[true, false])
    );
}

// ── int_list_slice / text_list_slice / bool_list_slice -> iter::slice ───
// slice(args: Tuple(List, Int, Int)) -> List; elements [from, to).

#[test]
fn int_list_slice_basic() {
    assert_eq!(
        iter::slice(Value::Tuple(vec![int_list(&[1, 2, 3, 4, 5]), i(1), i(4)])),
        int_list(&[2, 3, 4])
    );
}

#[test]
fn int_list_slice_out_of_range_clamps_to_len() {
    assert_eq!(
        iter::slice(Value::Tuple(vec![int_list(&[1, 2, 3]), i(0), i(99)])),
        int_list(&[1, 2, 3])
    );
}

#[test]
fn int_list_slice_from_gt_to_clamps_to_empty() {
    assert_eq!(
        iter::slice(Value::Tuple(vec![int_list(&[1, 2, 3]), i(2), i(0)])),
        int_list(&[])
    );
}

#[test]
fn text_list_slice_basic() {
    assert_eq!(
        iter::slice(Value::Tuple(vec![text_list(&["a", "b", "c", "d"]), i(1), i(3)])),
        text_list(&["b", "c"])
    );
}

#[test]
fn text_list_slice_out_of_range_clamps_to_len() {
    assert_eq!(
        iter::slice(Value::Tuple(vec![text_list(&["a", "b"]), i(0), i(99)])),
        text_list(&["a", "b"])
    );
}

#[test]
fn text_list_slice_from_gt_to_clamps_to_empty() {
    assert_eq!(
        iter::slice(Value::Tuple(vec![text_list(&["a", "b", "c"]), i(2), i(0)])),
        text_list(&[])
    );
}

#[test]
fn bool_list_slice_basic() {
    assert_eq!(
        iter::slice(Value::Tuple(vec![bool_list(&[true, false, true, false]), i(1), i(3)])),
        bool_list(&[false, true])
    );
}

#[test]
fn bool_list_slice_out_of_range_clamps_to_len() {
    assert_eq!(
        iter::slice(Value::Tuple(vec![bool_list(&[true, false]), i(0), i(99)])),
        bool_list(&[true, false])
    );
}

#[test]
fn bool_list_slice_from_gt_to_clamps_to_empty() {
    assert_eq!(
        iter::slice(Value::Tuple(vec![bool_list(&[true, false, true]), i(2), i(0)])),
        bool_list(&[])
    );
}

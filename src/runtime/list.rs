use super::value::{Value, get_str, intern_str};

// ── M1_VALUELIST_NARROWING_V1 ─────────────────────────────────────────────
//
// Every other fn in this file is element-agnostic: `Value::List(Vec<Value>)`
// is a single untyped vector, and every op above is a match on the List tag
// that never inspects an element. A narrowing conversion is the first
// operation that must — it exists to reject a ValueList whose elements are
// not all the target tag, and NO_DEFAULTING_EVER means there is no arm that
// substitutes a value or silently drops a mismatched one: a bad element is a
// panic, not a computation.
//
// The panic must name the offending index and the actual tag found (Chris's
// binding requirement on this intent) — "expected Int" alone tells a caller
// nothing about WHERE the list went wrong, the same defect as int_div's bare
// "division by zero".

fn value_tag_name(v: &Value) -> &'static str {
    match v {
        Value::Int(_) => "Int",
        Value::Bool(_) => "Bool",
        Value::Str(_) => "Text",
        Value::Unit => "Unit",
        Value::Tuple(_) => "Tuple",
        Value::List(_) => "List",
        Value::Ctor { .. } => "Ctor",
        Value::Dec(_) => "Dec",
        Value::Float(_) => "Float",
        Value::Bytes(_) => "Bytes",
    }
}

/// Shared element-inspecting loop behind all three `value_list_to_*_list`
/// narrowing fns. An empty list narrows unconditionally and vacuously: the
/// target type comes from which of the three callers you reached, not from
/// inspecting contents, so there is no element-type ambiguity for an empty
/// list to get wrong.
#[track_caller]
fn narrow_value_list(fn_name: &str, expected: &str, list: Value, is_expected: fn(&Value) -> bool) -> Value {
    match list {
        Value::List(items) => {
            for (idx, item) in items.iter().enumerate() {
                if !is_expected(item) {
                    panic!(
                        "{fn_name}: element {idx} is {actual}, expected {expected}",
                        actual = value_tag_name(item)
                    );
                }
            }
            Value::List(items)
        }
        other => panic!("{fn_name}: expected ValueList, got {actual}", actual = value_tag_name(&other)),
    }
}

#[track_caller]
pub fn value_list_to_int_list(list: Value) -> Value {
    narrow_value_list("value_list_to_int_list", "Int", list, |v| matches!(v, Value::Int(_)))
}

#[track_caller]
pub fn value_list_to_text_list(list: Value) -> Value {
    narrow_value_list("value_list_to_text_list", "Text", list, |v| matches!(v, Value::Str(_)))
}

#[track_caller]
pub fn value_list_to_bool_list(list: Value) -> Value {
    narrow_value_list("value_list_to_bool_list", "Bool", list, |v| matches!(v, Value::Bool(_)))
}

/// Widening, the converse of the `value_list_to_*_list` narrowing: a typed list is already a `Value::List` underneath, so the TextList / IntList / BoolList
/// is the ValueList unchanged. These exist so a typed list can be handed to a fn that takes a ValueList (the generic higher-order fns: count, any, all,
/// find_index, flat_map, foreach, fold, filter, map, zip, enumerate) without the caller building a copy. Nothing is checked: every element already has the right tag.
#[track_caller]
pub fn text_list_to_value_list(list: Value) -> Value {
    list
}

#[track_caller]
pub fn int_list_to_value_list(list: Value) -> Value {
    list
}

#[track_caller]
pub fn bool_list_to_value_list(list: Value) -> Value {
    list
}

/// Scalar sibling of `narrow_value_list`: checks a single `Value`'s tag
/// instead of walking a list's elements.
#[track_caller]
fn narrow_value(fn_name: &str, expected: &str, v: Value, is_expected: fn(&Value) -> bool) -> Value {
    if is_expected(&v) {
        v
    } else {
        panic!("{fn_name}: expected {expected}, got {actual}", actual = value_tag_name(&v));
    }
}

#[track_caller]
pub fn value_to_int(v: Value) -> Value {
    narrow_value("value_to_int", "Int", v, |v| matches!(v, Value::Int(_)))
}

#[track_caller]
pub fn value_to_text(v: Value) -> Value {
    narrow_value("value_to_text", "Text", v, |v| matches!(v, Value::Str(_)))
}

/// `value_to_bytes(v)`: v when it holds Bytes (PyAx unpacks a Bytes variable from loop state with it).
#[track_caller]
pub fn value_to_bytes(v: Value) -> Value {
    narrow_value("value_to_bytes", "Bytes", v, |v| matches!(v, Value::Bytes(_)))
}

/// `value_to_text_list(v)`: v when it holds a list of Text (each element checked, like value_list_to_text_list).
/// How a TextList travels through loop state: value_make stores it, ctor_field hands back a Value.
#[track_caller]
pub fn value_to_text_list(v: Value) -> Value {
    narrow_value_list("value_to_text_list", "Text", v, |v| matches!(v, Value::Str(_)))
}

/// `value_to_int_list(v)`: v when it holds a list of Int (each element checked), the IntList sibling of value_to_text_list.
#[track_caller]
pub fn value_to_int_list(v: Value) -> Value {
    narrow_value_list("value_to_int_list", "Int", v, |v| matches!(v, Value::Int(_)))
}

/// `text_list_pack(l)`: l as one Text, "<n>|<len1>,<len2>,...|<item1><item2>..." with lengths in code points
/// ("0||" for the empty list). Any item text is safe: nothing is escaped, the lengths say where items end, so two
/// lists pack to the same Text only when they are equal. Inverse of text_list_unpack.
#[track_caller]
pub fn text_list_pack(list: Value) -> Value {
    match list {
        Value::List(items) => {
            let mut lens = String::new();
            let mut body = String::new();
            for (idx, item) in items.iter().enumerate() {
                match item {
                    Value::Str(s) => {
                        if idx > 0 { lens.push(','); }
                        lens.push_str(&s.chars().count().to_string());
                        body.push_str(s);
                    }
                    other => panic!("text_list_pack: element {idx} is {actual}, expected Text", actual = value_tag_name(other)),
                }
            }
            Value::Str(intern_str(&format!("{}|{}|{}", items.len(), lens, body)))
        }
        other => panic!("text_list_pack: expected TextList, got {actual}", actual = value_tag_name(&other)),
    }
}

/// `text_list_unpack(t)`: the TextList that text_list_pack packed into t. Panics naming the cause when t is not a
/// packing (count, lengths or items do not agree).
#[track_caller]
pub fn text_list_unpack(t: std::sync::Arc<str>) -> Value {
    let bad = |why: &str| -> ! { panic!("text_list_unpack: not a packed list ({why})") };
    let (n, rest) = t.split_once('|').unwrap_or_else(|| bad("no count"));
    let n: usize = n.parse().unwrap_or_else(|_| bad("count is not a number"));
    let (lens, mut body) = rest.split_once('|').unwrap_or_else(|| bad("no lengths"));
    let mut out = Vec::with_capacity(n);
    if n > 0 {
        for l in lens.split(',') {
            let l: usize = l.parse().unwrap_or_else(|_| bad("a length is not a number"));
            let cut = body.char_indices().nth(l).map(|(i, _)| i).unwrap_or_else(|| {
                if body.chars().count() == l { body.len() } else { bad("items shorter than the lengths") }
            });
            out.push(Value::Str(intern_str(&body[..cut])));
            body = &body[cut..];
        }
    } else if !lens.is_empty() {
        bad("count 0 with lengths");
    }
    if out.len() != n { bad("count and lengths differ"); }
    if !body.is_empty() { bad("items longer than the lengths"); }
    Value::List(super::value::ListBuf::from(out))
}

#[track_caller]
pub fn value_to_bool(v: Value) -> Value {
    narrow_value("value_to_bool", "Bool", v, |v| matches!(v, Value::Bool(_)))
}

#[track_caller]
pub fn list_nil(_: Value) -> Value {
    Value::List(super::value::ListBuf::new())
}

/// Build an M1 ValueList from its elements. Lowering target of
/// `ValueList(T)(a, b, ...)`. Variadic, same calling convention as value_make.
#[track_caller]
pub fn list_make(args: Value) -> Value {
    Value::List(super::value::ListBuf::from(super::tuple::fields_from_variadic(args)))
}

#[track_caller]
pub fn list_cons(elem: Value, tail: Value) -> Value {
    match tail {
        Value::List(tail) => {
            let mut v = vec![elem];
            v.extend(tail);
            Value::List(super::value::ListBuf::from(v))
        }
        _ => Value::List(super::value::ListBuf::from(vec![elem])),
    }
}

#[track_caller]
pub fn list_len(list: Value) -> Value {
    match list {
        Value::List(es) => Value::Int(es.len() as i64),
        _ => panic!("list_len: expected List"),
    }
}

#[track_caller]
pub fn list_get(list: Value, idx: i64) -> Value {
    match list {
        Value::List(elems) => elems[idx as usize].clone(),
        _ => panic!("list_get: expected List"),
    }
}

#[track_caller]
pub fn list_get_at(list: Value, idx: i64) -> Value {
    if idx < 0 { return super::option::option_none(); }
    match list {
        Value::List(elems) => match elems.get(idx as usize) {
            Some(v) => super::option::option_some(v.clone()),
            None    => super::option::option_none(),
        },
        _ => panic!("list_get_at: expected List"),
    }
}

#[track_caller]
pub fn list_append(list: Value, elem: Value) -> Value {
    match list {
        // SHARED_LIST_V1 + MOVE_ON_LAST_USE_V1: `list` arrives moved when this is its last use, so the push is in
        // place; if another value still shares the elements, ListBuf copies them first (copy-on-write).
        Value::List(mut elems) => {
            elems.push(elem);
            Value::List(elems)
        }
        _ => panic!("list_append: expected List as first element"),
    }
}

// ── In-place list updates (PYAX_LIST_IN_PLACE_V1) ────────────────────────────
// The list arrives owned; ListBuf copies the elements only if they are still shared. Indices are already checked
// and normalized by the caller (PyAx's prelude raises IndexError first): out of range here is a defect.

/// `list_set(xs, i, v)`: xs with element i replaced by v.
#[track_caller]
pub fn list_set(list: Value, idx: i64, elem: Value) -> Value {
    match list {
        Value::List(mut elems) => {
            let n = elems.len();
            match usize::try_from(idx).ok().filter(|i| *i < n) {
                Some(i) => elems[i] = elem,
                None => panic!("list_set: index {} out of range (length {})", idx, n),
            }
            Value::List(elems)
        }
        _ => panic!("list_set: expected List"),
    }
}

/// `list_insert(xs, i, v)`: xs with v inserted before element i (i == len appends).
#[track_caller]
pub fn list_insert(list: Value, idx: i64, elem: Value) -> Value {
    match list {
        Value::List(mut elems) => {
            let n = elems.len();
            match usize::try_from(idx).ok().filter(|i| *i <= n) {
                Some(i) => elems.insert(i, elem),
                None => panic!("list_insert: index {} out of range (length {})", idx, n),
            }
            Value::List(elems)
        }
        _ => panic!("list_insert: expected List"),
    }
}

/// `list_remove(xs, i)`: xs without element i.
#[track_caller]
pub fn list_remove(list: Value, idx: i64) -> Value {
    match list {
        Value::List(mut elems) => {
            let n = elems.len();
            match usize::try_from(idx).ok().filter(|i| *i < n) {
                Some(i) => { elems.remove(i); }
                None => panic!("list_remove: index {} out of range (length {})", idx, n),
            }
            Value::List(elems)
        }
        _ => panic!("list_remove: expected List"),
    }
}

/// `list_drop_last(xs)`: xs without its last element (the rest of a pop).
#[track_caller]
pub fn list_drop_last(list: Value) -> Value {
    match list {
        Value::List(mut elems) => {
            if elems.pop().is_none() {
                panic!("list_drop_last: empty list");
            }
            Value::List(elems)
        }
        _ => panic!("list_drop_last: expected List"),
    }
}

#[cfg(test)]
mod in_place_tests {
    use super::*;
    fn ints(xs: &[i64]) -> Value { Value::List(xs.iter().map(|x| Value::Int(*x)).collect()) }

    #[test]
    fn updates_give_the_new_list_and_never_touch_a_shared_one() {
        let xs = ints(&[1, 2, 3]);
        let keep = xs.clone();                                     // shared: the update must copy, not write through
        assert_eq!(list_set(xs.clone(), 1, Value::Int(9)), ints(&[1, 9, 3]));
        assert_eq!(list_insert(xs.clone(), 3, Value::Int(4)), ints(&[1, 2, 3, 4]));
        assert_eq!(list_insert(xs.clone(), 0, Value::Int(0)), ints(&[0, 1, 2, 3]));
        assert_eq!(list_remove(xs.clone(), 0), ints(&[2, 3]));
        assert_eq!(list_drop_last(xs.clone()), ints(&[1, 2]));
        assert_eq!(list_append(xs, Value::Int(4)), ints(&[1, 2, 3, 4]));
        assert_eq!(keep, ints(&[1, 2, 3]));
    }

    #[test]
    fn an_unshared_list_is_updated_in_place() {
        let xs = ints(&[1, 2, 3]);
        let before = match &xs { Value::List(es) => es.as_ptr(), _ => unreachable!() };
        let ys = list_set(xs, 0, Value::Int(7));
        let after = match &ys { Value::List(es) => es.as_ptr(), _ => unreachable!() };
        assert_eq!(before, after);                                 // same storage: nothing was copied
    }

    #[test]
    #[should_panic(expected = "list_set: index 3 out of range")]
    fn out_of_range_is_a_defect() {
        list_set(ints(&[1, 2, 3]), 3, Value::Int(0));
    }
}

#[track_caller]
pub fn list_concat(a: Value, b: Value) -> Value {
    match (a, b) {
        (Value::List(mut a), Value::List(b)) => {
            a.extend(b);
            Value::List(a)
        }
        _ => panic!("list_concat: expected two Lists"),
    }
}

#[track_caller]
pub fn list_reverse(list: Value) -> Value {
    match list {
        Value::List(mut es) => { es.reverse(); Value::List(es) }
        _ => panic!("list_reverse: expected List"),
    }
}

#[track_caller]
pub fn list_head(list: Value) -> Value {
    match list {
        Value::List(es) if !es.is_empty() => es[0].clone(),
        Value::List(_) => panic!("list_head: called on empty list"),
        _ => panic!("list_head: expected List"),
    }
}

#[track_caller]
pub fn list_tail(list: Value) -> Value {
    match list {
        Value::List(es) if !es.is_empty() => Value::List(super::value::ListBuf::from(es[1..].to_vec())),
        Value::List(_) => panic!("list_tail: called on empty list"),
        _ => panic!("list_tail: expected List"),
    }
}

#[track_caller]
pub fn list_is_empty(list: Value) -> Value {
    match list {
        Value::List(es) => Value::Bool(es.is_empty()),
        _ => panic!("list_is_empty: expected List"),
    }
}

#[track_caller]
pub fn list_of_1(v: Value) -> Value {
    Value::List(super::value::ListBuf::from(vec![v]))
}

#[track_caller]
pub fn list_of_2(a: Value, b: Value) -> Value {
    Value::List(super::value::ListBuf::from(vec![a, b]))
}

#[track_caller]
pub fn list_of_3(a: Value, b: Value, c: Value) -> Value {
    Value::List(super::value::ListBuf::from(vec![a, b, c]))
}

/// Returns 1 if list[index] exists and str_len(list[index]) ≤ max_len, else 0. OOB-safe.
#[track_caller]
pub fn list_str_len_lte_if_some(list: Value, idx: i64, max_len: i64) -> Value {
    if idx < 0 { return Value::Int(0); }
    match list {
        Value::List(elems) => match elems.get(idx as usize) {
            Some(Value::Str(s)) => {
                let len = get_str(s).chars().count() as i64;
                Value::Int(if len <= max_len { 1 } else { 0 })
            }
            Some(_) => panic!("list_str_len_lte_if_some: list element is not Str"),
            None    => Value::Int(0),
        },
        _ => panic!("list_str_len_lte_if_some: expected List"),
    }
}

/// Get list[i] and println the value if it exists; return Unit either way.
/// Used by the unrolled forEach loop in 0.5 bundles where CIf branches are
/// evaluated eagerly — inlining the None check into Rust avoids option_unwrap(None).
#[track_caller]
pub fn list_get_println_if_some(list: Value, idx: i64) -> Value {
    if idx < 0 { return Value::Unit; }
    match list {
        Value::List(elems) => match elems.get(idx as usize) {
            Some(v) => super::io::io_println(v.clone()),
            None    => Value::Unit,
        },
        _ => panic!("list_get_println_if_some: expected List"),
    }
}

#[cfg(test)]
mod widening_tests {
    use super::*;
    fn texts(xs: &[&str]) -> Value { Value::List(xs.iter().map(|x| Value::Str((*x).into())).collect()) }
    fn ints(xs: &[i64]) -> Value { Value::List(xs.iter().map(|x| Value::Int(*x)).collect()) }

    #[test]
    fn widening_is_the_identity_and_narrowing_brings_it_back() {
        let t = texts(&["ax", "b", ""]);
        assert_eq!(text_list_to_value_list(t.clone()), t);
        assert_eq!(value_list_to_text_list(text_list_to_value_list(t.clone())), t);
        let i = ints(&[1, 5, 9]);
        assert_eq!(value_list_to_int_list(int_list_to_value_list(i.clone())), i);
        let b = Value::List([true, false].iter().map(|x| Value::Bool(*x)).collect());
        assert_eq!(value_list_to_bool_list(bool_list_to_value_list(b.clone())), b);
        assert_eq!(value_list_to_text_list(text_list_to_value_list(texts(&[]))), texts(&[]));   // the empty list round-trips too
    }
}

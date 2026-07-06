use axis_codegen_bridge::runtime::value::{Value, intern_str, get_tag_name, init_runtime};
use axis_codegen_bridge::runtime::{arith, str_ops, list, registry};
use std::sync::Mutex;
use tempfile::NamedTempFile;

// ── Helpers ──────────────────────────────────────────────────────────────────

static REGISTRY_LOCK: Mutex<()> = Mutex::new(());

fn setup() { init_runtime(); }

fn s(text: &str) -> Value {
    Value::Str(intern_str(text))
}

fn t2(a: Value, b: Value) -> Value {
    Value::Tuple(vec![a, b])
}

fn t3(a: Value, b: Value, c: Value) -> Value {
    Value::Tuple(vec![a, b, c])
}

fn ctor_tag(v: &Value) -> String {
    match v {
        Value::Ctor { tag, .. } => get_tag_name(*tag),
        other => panic!("expected Ctor, got {:?}", other),
    }
}

/// Run f with AXIS_REGISTRY pointing to a fresh isolated tempfile.
/// Serialised via REGISTRY_LOCK to prevent env-var races across tests.
fn with_registry<F: FnOnce()>(f: F) {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = NamedTempFile::new().expect("tempfile");
    std::env::set_var("AXIS_REGISTRY", tmp.path());
    f();
    std::env::remove_var("AXIS_REGISTRY");
}

// ── arith — untested functions ────────────────────────────────────────────────

#[test]
fn test_int_mod_basic() {
    setup();
    assert_eq!(arith::int_mod(t2(Value::Int(10), Value::Int(3))), Value::Int(1));
}

#[test]
fn test_int_mod_exact_divisor() {
    setup();
    assert_eq!(arith::int_mod(t2(Value::Int(9), Value::Int(3))), Value::Int(0));
}

#[test]
fn test_int_mod_negative_dividend() {
    setup();
    assert_eq!(arith::int_mod(t2(Value::Int(-7), Value::Int(3))), Value::Int(-1));
}

#[test]
fn test_int_eq_true() {
    setup();
    assert_eq!(arith::int_eq(t2(Value::Int(42), Value::Int(42))), Value::Bool(true));
}

#[test]
fn test_int_eq_false() {
    setup();
    assert_eq!(arith::int_eq(t2(Value::Int(1), Value::Int(2))), Value::Bool(false));
}

#[test]
fn test_dec_eq() {
    use axis_codegen_bridge::runtime::value::Decimal;
    use std::str::FromStr;
    setup();
    let a = Value::Dec(Decimal::from_str("1.50").unwrap());
    let b = Value::Dec(Decimal::from_str("1.500").unwrap()); // scale-insensitive equality
    let c = Value::Dec(Decimal::from_str("2.0").unwrap());
    assert_eq!(arith::dec_eq(t2(a.clone(), b)), Value::Bool(true));
    assert_eq!(arith::dec_eq(t2(a, c)), Value::Bool(false));
}

#[test]
fn test_float_eq() {
    setup();
    assert_eq!(arith::float_eq(t2(Value::Float(1.5), Value::Float(1.5))), Value::Bool(true));
    assert_eq!(arith::float_eq(t2(Value::Float(1.5), Value::Float(2.5))), Value::Bool(false));
    // IEEE-754: NaN != NaN, +0.0 == -0.0 (matches value_eq semantics).
    assert_eq!(arith::float_eq(t2(Value::Float(f64::NAN), Value::Float(f64::NAN))), Value::Bool(false));
    assert_eq!(arith::float_eq(t2(Value::Float(0.0), Value::Float(-0.0))), Value::Bool(true));
}

#[test]
fn test_unit_id_discards_int() {
    setup();
    assert_eq!(arith::unit_id(Value::Int(99)), Value::Unit);
}

#[test]
fn test_unit_id_discards_unit() {
    setup();
    assert_eq!(arith::unit_id(Value::Unit), Value::Unit);
}

#[test]
fn test_seq_unit_tuple() {
    setup();
    assert_eq!(arith::seq_unit(t2(Value::Unit, Value::Unit)), Value::Unit);
}

#[test]
fn test_seq_unit_plain_unit() {
    setup();
    assert_eq!(arith::seq_unit(Value::Unit), Value::Unit);
}

#[test]
fn test_int_abs_positive() {
    setup();
    assert_eq!(arith::int_abs(Value::Int(5)), Value::Int(5));
}

#[test]
fn test_int_abs_negative() {
    setup();
    assert_eq!(arith::int_abs(Value::Int(-7)), Value::Int(7));
}

#[test]
fn test_int_abs_zero() {
    setup();
    assert_eq!(arith::int_abs(Value::Int(0)), Value::Int(0));
}

// ── str_ops — untested functions ──────────────────────────────────────────────

#[test]
fn test_str_char_code_ascii() {
    setup();
    assert_eq!(str_ops::str_char_code(t2(s("A"), Value::Int(0))), Value::Int(65));
}

#[test]
fn test_str_char_code_unicode() {
    setup();
    let expected = 'é' as i64;
    assert_eq!(str_ops::str_char_code(t2(s("é"), Value::Int(0))), Value::Int(expected));
}

#[test]
fn test_str_slice_mid() {
    setup();
    assert_eq!(str_ops::str_slice(t3(s("hello"), Value::Int(1), Value::Int(4))), s("ell"));
}

#[test]
fn test_str_slice_clamps_to_len() {
    setup();
    assert_eq!(str_ops::str_slice(t3(s("hi"), Value::Int(0), Value::Int(100))), s("hi"));
}

#[test]
fn test_str_slice_empty_range() {
    setup();
    assert_eq!(str_ops::str_slice(t3(s("hello"), Value::Int(2), Value::Int(2))), s(""));
}

#[test]
fn test_str_ends_with_true() {
    setup();
    assert_eq!(str_ops::str_ends_with(t2(s("hello"), s("llo"))), Value::Bool(true));
}

#[test]
fn test_str_ends_with_false() {
    setup();
    assert_eq!(str_ops::str_ends_with(t2(s("hello"), s("hel"))), Value::Bool(false));
}

#[test]
fn test_str_ends_with_empty_suffix() {
    setup();
    assert_eq!(str_ops::str_ends_with(t2(s("hello"), s(""))), Value::Bool(true));
}

#[test]
fn test_str_trim_both_sides() {
    setup();
    assert_eq!(str_ops::str_trim(s("  hello  ")), s("hello"));
}

#[test]
fn test_str_trim_no_whitespace() {
    setup();
    assert_eq!(str_ops::str_trim(s("clean")), s("clean"));
}

#[test]
fn test_str_trim_only_whitespace() {
    setup();
    assert_eq!(str_ops::str_trim(s("   ")), s(""));
}

#[test]
fn test_str_contains_true() {
    setup();
    assert_eq!(str_ops::str_contains(t2(s("foobar"), s("oba"))), Value::Bool(true));
}

#[test]
fn test_str_contains_false() {
    setup();
    assert_eq!(str_ops::str_contains(t2(s("foobar"), s("baz"))), Value::Bool(false));
}

#[test]
fn test_str_contains_empty_needle() {
    setup();
    assert_eq!(str_ops::str_contains(t2(s("anything"), s(""))), Value::Bool(true));
}

#[test]
fn test_str_index_of_found() {
    setup();
    assert_eq!(str_ops::str_index_of(t2(s("hello"), s("ll"))), Value::Int(2));
}

#[test]
fn test_str_index_of_not_found() {
    setup();
    assert_eq!(str_ops::str_index_of(t2(s("hello"), s("xyz"))), Value::Int(-1));
}

#[test]
fn test_str_index_of_at_start() {
    setup();
    assert_eq!(str_ops::str_index_of(t2(s("hello"), s("he"))), Value::Int(0));
}

#[test]
fn test_chr_ascii() {
    setup();
    assert_eq!(str_ops::chr(Value::Int(65)), s("A"));
}

#[test]
fn test_chr_unicode() {
    setup();
    assert_eq!(str_ops::chr(Value::Int('é' as i64)), s("é"));
}

// ── Fix-4: chr(Int(10)) produces a real newline (0x0A), not backslash-n ──────
//
// M1 string literals do not process escape sequences. Text("\n") compiles to
// a literal backslash + n. chr(Int(10)) is the only way to get a real newline.

#[test]
fn test_chr_newline_is_0x0a() {
    setup();
    let nl = str_ops::chr(Value::Int(10));
    match &nl {
        Value::Str(h) => {
            let text = axis_codegen_bridge::runtime::value::get_str(*h);
            assert_eq!(text.len(), 1, "chr(10) must be exactly 1 char");
            assert_eq!(text.as_bytes()[0], 0x0A,
                "chr(10) must be 0x0A (newline), got 0x{:02X}", text.as_bytes()[0]);
        }
        other => panic!("chr(10) returned non-Str: {:?}", other),
    }
}

#[test]
fn test_chr_tab_is_0x09() {
    setup();
    let tab = str_ops::chr(Value::Int(9));
    match &tab {
        Value::Str(h) => {
            let text = axis_codegen_bridge::runtime::value::get_str(*h);
            assert_eq!(text.as_bytes()[0], 0x09, "chr(9) must be 0x09 (tab)");
        }
        other => panic!("chr(9) returned non-Str: {:?}", other),
    }
}

#[test]
fn test_chr_newline_concat_roundtrip() {
    setup();
    let nl  = str_ops::chr(Value::Int(10));
    let ab  = str_ops::str_concat(Value::Tuple(vec![s("a"), nl]));
    match &ab {
        Value::Str(h) => {
            let text = axis_codegen_bridge::runtime::value::get_str(*h);
            assert_eq!(text, "a\n", "concat with chr(10) should produce literal newline");
        }
        other => panic!("str_concat returned non-Str: {:?}", other),
    }
}

// ── Fix-5: bool_to_str converts Bool → Text ──────────────────────────────────

#[test]
fn test_bool_to_str_true() {
    setup();
    assert_eq!(str_ops::bool_to_str(Value::Bool(true)), s("true"));
}

#[test]
fn test_bool_to_str_false() {
    setup();
    assert_eq!(str_ops::bool_to_str(Value::Bool(false)), s("false"));
}

// ── text_eq / text_lt (axis.axreg canonical names) ───────────────────────────

#[test]
fn test_text_eq_equal() {
    setup();
    assert_eq!(str_ops::text_eq(t2(s("abc"), s("abc"))), Value::Bool(true));
}

#[test]
fn test_text_eq_not_equal() {
    setup();
    assert_eq!(str_ops::text_eq(t2(s("abc"), s("abd"))), Value::Bool(false));
}

#[test]
fn test_text_lt_less() {
    setup();
    assert_eq!(str_ops::text_lt(t2(s("abc"), s("abd"))), Value::Bool(true));
}

#[test]
fn test_text_lt_equal() {
    setup();
    assert_eq!(str_ops::text_lt(t2(s("abc"), s("abc"))), Value::Bool(false));
}

#[test]
fn test_text_lt_greater() {
    setup();
    assert_eq!(str_ops::text_lt(t2(s("abd"), s("abc"))), Value::Bool(false));
}

// ── list — untested functions ─────────────────────────────────────────────────

#[test]
fn test_list_concat_two_non_empty() {
    setup();
    let a = Value::List(vec![Value::Int(1), Value::Int(2)]);
    let b = Value::List(vec![Value::Int(3), Value::Int(4)]);
    let result = list::list_concat(t2(a, b));
    assert_eq!(result, Value::List(vec![
        Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4),
    ]));
}

#[test]
fn test_list_concat_left_empty() {
    setup();
    let result = list::list_concat(t2(
        Value::List(vec![]),
        Value::List(vec![Value::Int(1)]),
    ));
    assert_eq!(result, Value::List(vec![Value::Int(1)]));
}

#[test]
fn test_list_concat_right_empty() {
    setup();
    let result = list::list_concat(t2(
        Value::List(vec![Value::Int(1)]),
        Value::List(vec![]),
    ));
    assert_eq!(result, Value::List(vec![Value::Int(1)]));
}

#[test]
fn test_list_str_len_lte_if_some_within_threshold() {
    setup();
    let lst = Value::List(vec![s("hi"), s("world")]);
    // "hi" has len 2, threshold 3 → 1
    assert_eq!(
        list::list_str_len_lte_if_some(t3(lst, Value::Int(0), Value::Int(3))),
        Value::Int(1)
    );
}

#[test]
fn test_list_str_len_lte_if_some_exceeds_threshold() {
    setup();
    let lst = Value::List(vec![s("hello")]);
    // "hello" has len 5, threshold 3 → 0
    assert_eq!(
        list::list_str_len_lte_if_some(t3(lst, Value::Int(0), Value::Int(3))),
        Value::Int(0)
    );
}

#[test]
fn test_list_str_len_lte_if_some_oob_index() {
    setup();
    let lst = Value::List(vec![s("x")]);
    // index 5 is out of bounds → 0
    assert_eq!(
        list::list_str_len_lte_if_some(t3(lst, Value::Int(5), Value::Int(10))),
        Value::Int(0)
    );
}

#[test]
fn test_list_str_len_lte_if_some_exact_threshold() {
    setup();
    let lst = Value::List(vec![s("abc")]);
    // "abc" has len 3, threshold 3 → 1 (≤ is inclusive)
    assert_eq!(
        list::list_str_len_lte_if_some(t3(lst, Value::Int(0), Value::Int(3))),
        Value::Int(1)
    );
}

#[test]
fn test_list_get_println_if_some_in_bounds() {
    setup();
    let lst = Value::List(vec![Value::Int(42)]);
    // prints "42" to stdout, returns Unit
    assert_eq!(list::list_get_println_if_some(t2(lst, Value::Int(0))), Value::Unit);
}

#[test]
fn test_list_get_println_if_some_oob() {
    setup();
    let lst = Value::List(vec![Value::Int(1)]);
    assert_eq!(list::list_get_println_if_some(t2(lst, Value::Int(99))), Value::Unit);
}

#[test]
fn test_list_get_println_if_some_negative_index() {
    setup();
    let lst = Value::List(vec![Value::Int(1)]);
    assert_eq!(list::list_get_println_if_some(t2(lst, Value::Int(-1))), Value::Unit);
}

// ── registry ──────────────────────────────────────────────────────────────────
//
// All registry tests run under REGISTRY_LOCK with an isolated tempfile.
// The registry uses AXIS_REGISTRY env var; without it all operations no-op safely.

#[test]
fn test_registry_has_entry_not_found() {
    with_registry(|| {
        assert_eq!(registry::registry_has_entry(s("mymod.fn1")), Value::Bool(false));
    });
}

#[test]
fn test_registry_insert_and_has_entry() {
    with_registry(|| {
        let r = registry::registry_insert(t3(s("mymod.fn1"), s(""), s("Human")));
        assert_eq!(ctor_tag(&r), "Ok");
        assert_eq!(registry::registry_has_entry(s("mymod.fn1")), Value::Bool(true));
    });
}

#[test]
fn test_registry_insert_duplicate_rejected() {
    with_registry(|| {
        registry::registry_insert(t3(s("mymod.fn1"), s(""), s("Human")));
        let r = registry::registry_insert(t3(s("mymod.fn1"), s(""), s("Human")));
        assert_eq!(ctor_tag(&r), "Err");
    });
}

#[test]
fn test_registry_insert_invalid_provenance() {
    with_registry(|| {
        let r = registry::registry_insert(t3(s("mymod.fn1"), s(""), s("NotAValidProvenance")));
        assert_eq!(ctor_tag(&r), "Err");
    });
}

#[test]
fn test_registry_insert_oracle_promoted_provenance() {
    with_registry(|| {
        let r = registry::registry_insert(t3(s("mymod.fn1"), s(""), s("OraclePromoted")));
        assert_eq!(ctor_tag(&r), "Ok");
    });
}

#[test]
fn test_registry_lookup_found() {
    with_registry(|| {
        registry::registry_insert(t3(s("mod.fn"), s(""), s("Human")));
        let r = registry::registry_lookup(s("mod.fn"));
        assert_eq!(ctor_tag(&r), "Ok");
    });
}

#[test]
fn test_registry_lookup_not_found() {
    with_registry(|| {
        let r = registry::registry_lookup(s("missing.fn"));
        assert_eq!(ctor_tag(&r), "Err");
    });
}

#[test]
fn test_registry_lookup_unit_arg() {
    with_registry(|| {
        let r = registry::registry_lookup(Value::Unit);
        assert_eq!(ctor_tag(&r), "Err");
    });
}

#[test]
fn test_registry_get_provenance_found() {
    with_registry(|| {
        registry::registry_insert(t3(s("mod.fn"), s(""), s("Human")));
        let r = registry::registry_get_provenance(s("mod.fn"));
        assert_eq!(ctor_tag(&r), "Ok");
    });
}

#[test]
fn test_registry_get_provenance_not_found() {
    with_registry(|| {
        let r = registry::registry_get_provenance(s("missing.fn"));
        assert_eq!(ctor_tag(&r), "Err");
    });
}

#[test]
fn test_registry_all_entries_empty() {
    with_registry(|| {
        assert_eq!(registry::registry_all_entries(Value::Unit), Value::List(vec![]));
    });
}

#[test]
fn test_registry_all_entries_after_two_inserts() {
    with_registry(|| {
        registry::registry_insert(t3(s("mod.a"), s(""), s("Human")));
        registry::registry_insert(t3(s("mod.b"), s(""), s("Human")));
        match registry::registry_all_entries(Value::Unit) {
            Value::List(es) => assert_eq!(es.len(), 2),
            other => panic!("expected List, got {:?}", other),
        }
    });
}

#[test]
fn test_registry_verify_chain_empty() {
    with_registry(|| {
        assert_eq!(registry::registry_verify_chain(Value::Unit), Value::Bool(true));
    });
}

#[test]
fn test_registry_verify_chain_valid_after_inserts() {
    with_registry(|| {
        registry::registry_insert(t3(s("mod.a"), s(""), s("Human")));
        registry::registry_insert(t3(s("mod.b"), s(""), s("Human")));
        assert_eq!(registry::registry_verify_chain(Value::Unit), Value::Bool(true));
    });
}

#[test]
fn test_registry_compound_id() {
    setup();
    let r = registry::registry_compound_id(t2(s("mymod"), s("myfn")));
    assert_eq!(r, s("mymod.myfn"));
}

#[test]
fn test_registry_compound_id_passthrough_str() {
    setup();
    // Single Str variant passes through unchanged
    let r = registry::registry_compound_id(s("already.qualified"));
    assert_eq!(r, s("already.qualified"));
}

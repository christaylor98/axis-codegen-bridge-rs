//! Acceptance tests for BRIDGE_FOREIGN_FN_FNREF_M1.
//!
//! Covers:
//!   * Runtime behaviour of the new M1 iteration / emit-stdlib primitives
//!     (range, str_join, foreach, loop_count, plus a sampling of Phase 2/3).
//!   * Emit-time type-gate rejection of a Fn-typed pool entry placed in a
//!     data position (FN_REF_IS_CALLEE_ONLY).

use axis_codegen_bridge::core_ir_05::{
    fn_type_hash, int_type_hash, encode_int_payload, sha256_bytes,
    ConstantPoolEntry, CoreBundle, Node, NodeRef,
};
use axis_codegen_bridge::emit::rust_05::emit_rust_lib_from_bundle;
use axis_codegen_bridge::runtime::iter;
use axis_codegen_bridge::runtime::str_ops;
use axis_codegen_bridge::runtime::value::{init_runtime, intern_str, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};

fn setup() { init_runtime(); }

fn s(text: &str) -> Value {
    Value::Str(intern_str(text))
}

// ── Phase 1 acceptance — exact one-line examples from the work manifest ──────

#[test]
fn accept_range_0_3_is_0_1_2() {
    setup();
    let result = iter::range(Value::Tuple(vec![Value::Int(0), Value::Int(3)]));
    assert_eq!(
        result,
        Value::List(vec![Value::Int(0), Value::Int(1), Value::Int(2)])
    );
}

#[test]
fn accept_str_join_ab_comma_is_a_comma_b() {
    setup();
    // ValueList(Text)("a", "b")
    let list = Value::List(vec![s("a"), s("b")]);
    let result = str_ops::str_join(Value::Tuple(vec![list, s(",")]));
    assert_eq!(result, s("a,b"));
}

/// `foreach(xs, print_it)` runs the effect per element.
///
/// We can't observe stdout cleanly across the test harness, so we use a
/// process-local counter as the "effect" — the contract being tested is that
/// `callee` is invoked once per element and the result is `Unit`.
static FOREACH_CALLS: AtomicI64 = AtomicI64::new(0);

fn count_call(v: Value) -> Value {
    let n = v.as_int();
    FOREACH_CALLS.fetch_add(n, Ordering::SeqCst);
    Value::Unit
}

#[test]
fn accept_foreach_runs_per_element() {
    setup();
    FOREACH_CALLS.store(0, Ordering::SeqCst);
    let xs = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
    let result = iter::foreach(xs, count_call);
    assert_eq!(result, Value::Unit);
    assert_eq!(FOREACH_CALLS.load(Ordering::SeqCst), 6);
}

// ── Phase 1 — loop_count ─────────────────────────────────────────────────────

fn inc1(v: Value) -> Value {
    Value::Int(v.as_int() + 1)
}

#[test]
fn loop_count_applies_step_n_times() {
    setup();
    let result = iter::loop_count(Value::Int(5), Value::Int(0), inc1);
    assert_eq!(result, Value::Int(5));
}

#[test]
fn loop_count_zero_returns_init() {
    setup();
    let result = iter::loop_count(Value::Int(0), Value::Int(42), inc1);
    assert_eq!(result, Value::Int(42));
}

// ── Phase 2 — sampled HOFs ───────────────────────────────────────────────────

fn is_positive_pred(v: Value) -> Value {
    Value::Bool(v.as_int() > 0)
}

#[test]
fn any_finds_truthy() {
    setup();
    let xs = Value::List(vec![Value::Int(-1), Value::Int(0), Value::Int(7)]);
    assert_eq!(iter::any(xs, is_positive_pred), Value::Bool(true));
}

#[test]
fn all_rejects_zero() {
    setup();
    let xs = Value::List(vec![Value::Int(1), Value::Int(0), Value::Int(7)]);
    assert_eq!(iter::all(xs, is_positive_pred), Value::Bool(false));
}

#[test]
fn find_index_returns_minus_one_when_none() {
    setup();
    let xs = Value::List(vec![Value::Int(-1), Value::Int(-2)]);
    assert_eq!(
        iter::find_index(xs, is_positive_pred),
        Value::Int(-1)
    );
}

#[test]
fn count_counts_truthy() {
    setup();
    let xs = Value::List(vec![
        Value::Int(-1), Value::Int(0), Value::Int(1), Value::Int(2),
    ]);
    assert_eq!(iter::count(xs, is_positive_pred), Value::Int(2));
}

// ── Phase 2 — sampled data fns ───────────────────────────────────────────────

#[test]
fn enumerate_pairs_index_with_value() {
    setup();
    let xs = Value::List(vec![s("a"), s("b")]);
    let out = iter::enumerate(xs);
    match out {
        Value::List(pairs) => {
            assert_eq!(pairs.len(), 2);
            // Each element must be a `Value` Ctor with [Int(i), v].
            for (i, p) in pairs.iter().enumerate() {
                match p {
                    Value::Ctor { fields, .. } => {
                        assert_eq!(fields[0], Value::Int(i as i64));
                    }
                    other => panic!("expected Ctor pair, got {:?}", other),
                }
            }
        }
        other => panic!("expected List, got {:?}", other),
    }
}

#[test]
fn zip_truncates_to_shorter() {
    setup();
    let xs = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
    let ys = Value::List(vec![s("a"), s("b")]);
    let out = iter::zip(Value::Tuple(vec![xs, ys]));
    if let Value::List(pairs) = out {
        assert_eq!(pairs.len(), 2);
    } else {
        panic!("zip did not return List");
    }
}

#[test]
fn take_and_drop_partition() {
    setup();
    let xs = Value::List(vec![
        Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4),
    ]);
    let head = iter::take(Value::Tuple(vec![xs.clone(), Value::Int(2)]));
    let tail = iter::drop(Value::Tuple(vec![xs, Value::Int(2)]));
    assert_eq!(head, Value::List(vec![Value::Int(1), Value::Int(2)]));
    assert_eq!(tail, Value::List(vec![Value::Int(3), Value::Int(4)]));
}

#[test]
fn flatten_concats_inner_lists() {
    setup();
    let xs = Value::List(vec![
        Value::List(vec![Value::Int(1), Value::Int(2)]),
        Value::List(vec![Value::Int(3)]),
    ]);
    assert_eq!(
        iter::flatten(xs),
        Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
    );
}

// ── Phase 3 — text helpers ───────────────────────────────────────────────────

#[test]
fn str_replace_substitutes_all_occurrences() {
    setup();
    let r = str_ops::str_replace(Value::Tuple(vec![
        s("a_b_c"),
        s("_"),
        s("-"),
    ]));
    assert_eq!(r, s("a-b-c"));
}

#[test]
fn str_to_upper_and_lower_are_idempotent() {
    setup();
    let u = str_ops::str_to_upper(s("Hello"));
    assert_eq!(u, s("HELLO"));
    assert_eq!(str_ops::str_to_upper(u.clone()), u);

    let l = str_ops::str_to_lower(s("Hello"));
    assert_eq!(l, s("hello"));
    assert_eq!(str_ops::str_to_lower(l.clone()), l);
}

#[test]
fn str_pad_left_and_right_match_width() {
    setup();
    assert_eq!(
        str_ops::str_pad_left(Value::Tuple(vec![s("7"), Value::Int(3), s("0")])),
        s("007")
    );
    assert_eq!(
        str_ops::str_pad_right(Value::Tuple(vec![s("7"), Value::Int(3), s("0")])),
        s("700")
    );
}

// ── Emit-time type gate — FN_REF_IS_CALLEE_ONLY ──────────────────────────────

/// A CCall to `int_add` where one of its arg slots references a Fn-typed pool
/// entry must be rejected at emit time. `int_add` is a Data-only fn so this
/// is the "Fn-as-data" illegal state.
#[test]
fn type_gate_rejects_fn_pool_ref_in_data_slot() {
    let int_add_id = sha256_bytes(b"int_add");
    let io_println_id = sha256_bytes(b"io_println");

    let bundle = CoreBundle {
        version: "0.5".into(),
        constant_pool: vec![
            // pool[0]: Int(1) — fine, Data slot
            ConstantPoolEntry {
                def_hash: int_type_hash(),
                payload: encode_int_payload(1),
            },
            // pool[1]: Fn ref to io_println — Fn-typed
            ConstantPoolEntry {
                def_hash: fn_type_hash(),
                payload: io_println_id.to_vec(),
            },
        ],
        nodes: vec![
            // int_add(pool[0], pool[1])  ← pool[1] is Fn-typed but slot is Data
            Node::CCall {
                target_identity: int_add_id,
                target_name: "int_add".into(),
                args: vec![NodeRef::Pool(0), NodeRef::Pool(1)],
            },
        ],
        result: NodeRef::Node(0),
    };

    let err = emit_rust_lib_from_bundle(
        &bundle,
        "type_gate_smoke",
        &HashMap::new(),
        &HashMap::new(),
        &std::collections::HashSet::new(),
    )
    .expect_err("emit should reject Fn-typed pool ref in a Data slot");
    assert!(
        err.contains("type gate") && err.contains("Fn"),
        "expected type-gate error mentioning Fn, got: {}",
        err
    );
}

/// A CCall to a HOF (`foreach`) where the callee slot gets a Data pool entry
/// must also be rejected — the slot expects Fn.
#[test]
fn type_gate_rejects_data_pool_ref_in_fn_slot() {
    let foreach_id = sha256_bytes(b"foreach");

    let bundle = CoreBundle {
        version: "0.5".into(),
        constant_pool: vec![
            // pool[0]: Int — used as the "list" (wrong type but type gate is
            // structural; the runtime would panic. We're only checking the
            // FnRef-vs-Data classification here.)
            ConstantPoolEntry {
                def_hash: int_type_hash(),
                payload: encode_int_payload(0),
            },
            // pool[1]: Int again — sitting in the Fn-slot
            ConstantPoolEntry {
                def_hash: int_type_hash(),
                payload: encode_int_payload(1),
            },
        ],
        nodes: vec![Node::CCall {
            target_identity: foreach_id,
            target_name: "foreach".into(),
            args: vec![NodeRef::Pool(0), NodeRef::Pool(1)],
        }],
        result: NodeRef::Node(0),
    };

    let err = emit_rust_lib_from_bundle(
        &bundle,
        "type_gate_smoke_fn_slot",
        &HashMap::new(),
        &HashMap::new(),
        &std::collections::HashSet::new(),
    )
    .expect_err("emit should reject Data pool ref in a Fn slot");
    assert!(
        err.contains("expects Fn") || err.contains("type gate"),
        "expected type-gate error about Fn slot, got: {}",
        err
    );
}

/// Happy path: a HOF call (`foreach(pool_0, io_println)`) with a Fn-typed
/// pool ref in the Fn slot lowers to a native multi-arg Rust call.
#[test]
fn fn_slot_emits_native_multi_arg_call() {
    let foreach_id = sha256_bytes(b"foreach");
    let io_println_id = sha256_bytes(b"io_println");

    let bundle = CoreBundle {
        version: "0.5".into(),
        constant_pool: vec![
            // pool[0]: Int — placeholder "list" (would normally be a ValueList
            // built via a prior node; for emit-shape check we just need the
            // slot to be Data-typed).
            ConstantPoolEntry {
                def_hash: int_type_hash(),
                payload: encode_int_payload(0),
            },
            // pool[1]: Fn ref to io_println
            ConstantPoolEntry {
                def_hash: fn_type_hash(),
                payload: io_println_id.to_vec(),
            },
        ],
        nodes: vec![Node::CCall {
            target_identity: foreach_id,
            target_name: "foreach".into(),
            args: vec![NodeRef::Pool(0), NodeRef::Pool(1)],
        }],
        result: NodeRef::Node(0),
    };

    let src = emit_rust_lib_from_bundle(
        &bundle,
        "foreach_smoke",
        &HashMap::new(),
        &HashMap::new(),
        &std::collections::HashSet::new(),
    )
    .expect("emit should succeed for a well-typed foreach call");

    // Pool[1] (Fn-typed) gets NO `let pool_1 = ...;` binding.
    assert!(
        !src.contains("let pool_1"),
        "Fn-typed pool entry must not be bound as a `let pool_N`, got src:\n{}",
        src
    );
    // The CCall emits as a native multi-arg call with io_println as a bare
    // Rust fn path, not wrapped in Value::Tuple.
    assert!(
        src.contains(
            "axis_codegen_bridge::runtime::iter::foreach(pool_0.clone(), \
             axis_codegen_bridge::runtime::io::io_println)"
        ),
        "expected native multi-arg foreach call, got src:\n{}",
        src
    );
    // No fn-as-data wrapping anywhere.
    assert!(
        !src.contains("Value::Tuple(vec![pool_0.clone(), axis_codegen_bridge"),
        "Fn ref must not be tuple-packed, got src:\n{}",
        src
    );
}

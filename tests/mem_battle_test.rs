use axis_codegen_bridge::runtime::value::{Value, intern_str, init_runtime};
use axis_codegen_bridge::runtime::{arith, str_ops, list, bool_ops};
use axis_codegen_bridge::runtime::non_blocking_memory::{AdaptiveCell, AdaptiveRegistry};
use std::panic;
use std::sync::{Arc, Mutex};
use std::thread;

fn setup() { init_runtime(); }

fn s(text: &str) -> Value { Value::Str(intern_str(text)) }
fn t2(a: Value, b: Value) -> Value { Value::Tuple(vec![a, b]) }

// ── 1. Value deep-clone stress ────────────────────────────────────────────────

fn deep_list(depth: usize) -> Value {
    if depth == 0 {
        Value::Int(0)
    } else {
        Value::List(vec![deep_list(depth - 1)])
    }
}

fn deep_tuple(depth: usize) -> Value {
    if depth == 0 {
        Value::Bool(true)
    } else {
        Value::Tuple(vec![deep_tuple(depth - 1), Value::Int(depth as i64)])
    }
}

#[test]
fn test_deep_clone_list_100_levels() {
    setup();
    let v = deep_list(100);
    for _ in 0..1000 {
        let _ = v.clone();
    }
}

#[test]
fn test_deep_clone_tuple_100_levels() {
    setup();
    let v = deep_tuple(100);
    for _ in 0..1000 {
        let _ = v.clone();
    }
}

#[test]
fn test_value_clone_drop_tight_loop() {
    setup();
    for i in 0..50_000 {
        let v = Value::List(vec![
            Value::Int(i),
            Value::Str(intern_str("x")),
            Value::Bool(i % 2 == 0),
            Value::Tuple(vec![Value::Int(i * 2), Value::Unit]),
        ]);
        let c = v.clone();
        drop(v);
        drop(c);
    }
}

#[test]
fn test_wide_list_clone_and_drop() {
    setup();
    let big = Value::List((0..10_000).map(Value::Int).collect());
    for _ in 0..100 {
        let _ = big.clone();
    }
}

#[test]
fn test_ctor_deep_nested_clone() {
    setup();
    let mut v = Value::Ctor { tag: 0, fields: vec![Value::Int(42)] };
    for _ in 0..50 {
        v = Value::Ctor { tag: 1, fields: vec![v.clone(), Value::Bool(true)] };
    }
    for _ in 0..500 {
        let _ = v.clone();
    }
}

// ── 2. String intern under concurrent load ────────────────────────────────────

#[test]
fn test_intern_concurrent_dedup() {
    setup();
    let handles: Vec<_> = (0..16).map(|tid| {
        thread::spawn(move || {
            for i in 0..500 {
                let key = format!("thread_{}_key_{}", tid, i % 50);
                let h1 = intern_str(&key);
                let h2 = intern_str(&key);
                assert_eq!(h1, h2, "same string must intern to same handle");
            }
        })
    }).collect();
    for h in handles { h.join().unwrap(); }
}

#[test]
fn test_intern_large_volume() {
    setup();
    let handles: Vec<u32> = (0..5_000).map(|i| {
        intern_str(&format!("unique_string_battle_{}", i))
    }).collect();
    for (i, &h) in handles.iter().enumerate() {
        let expected = format!("unique_string_battle_{}", i);
        let h2 = intern_str(&expected);
        assert_eq!(h, h2);
    }
}

#[test]
fn test_intern_empty_and_unicode() {
    setup();
    let h_empty1 = intern_str("");
    let h_empty2 = intern_str("");
    assert_eq!(h_empty1, h_empty2);

    let h_uni1 = intern_str("こんにちは世界🦀");
    let h_uni2 = intern_str("こんにちは世界🦀");
    assert_eq!(h_uni1, h_uni2);
}

#[test]
fn test_intern_concurrent_mixed_shared_unique() {
    setup();
    let shared_keys: Arc<Vec<String>> = Arc::new(
        (0..20).map(|i| format!("shared_{}", i)).collect()
    );
    let handles: Vec<_> = (0..8).map(|tid| {
        let keys = Arc::clone(&shared_keys);
        thread::spawn(move || {
            for round in 0..200 {
                let shared_h = intern_str(&keys[round % keys.len()]);
                let unique = format!("t{}_{}", tid, round);
                let _ = intern_str(&unique);
                let shared_h2 = intern_str(&keys[round % keys.len()]);
                assert_eq!(shared_h, shared_h2);
            }
        })
    }).collect();
    for h in handles { h.join().unwrap(); }
}

// ── 3. List operations — pathological sizes ───────────────────────────────────

#[test]
fn test_list_cons_chain_10k() {
    setup();
    let mut acc = Value::List(vec![]);
    for i in 0..10_000 {
        acc = list::list_cons(t2(Value::Int(i), acc));
    }
    assert_eq!(list::list_len(acc), Value::Int(10_000));
}

#[test]
fn test_list_append_10k() {
    setup();
    let mut acc = Value::List(vec![]);
    for i in 0..10_000 {
        acc = list::list_append(t2(acc, Value::Int(i)));
    }
    assert_eq!(list::list_len(acc.clone()), Value::Int(10_000));
    assert_eq!(list::list_get(t2(acc, Value::Int(9_999))), Value::Int(9_999));
}

#[test]
fn test_list_concat_large() {
    setup();
    let a: Value = Value::List((0..5_000).map(Value::Int).collect());
    let b: Value = Value::List((5_000..10_000).map(Value::Int).collect());
    let c = list::list_concat(t2(a, b));
    assert_eq!(list::list_len(c.clone()), Value::Int(10_000));
    assert_eq!(list::list_head(c.clone()), Value::Int(0));
    assert_eq!(list::list_get(t2(c, Value::Int(9_999))), Value::Int(9_999));
}

#[test]
fn test_list_reverse_large() {
    setup();
    let lst = Value::List((0..10_000).map(Value::Int).collect());
    let rev = list::list_reverse(lst);
    assert_eq!(list::list_head(rev.clone()), Value::Int(9_999));
    assert_eq!(list::list_get(t2(rev, Value::Int(9_999))), Value::Int(0));
}

#[test]
fn test_list_head_tail_chain() {
    setup();
    let mut lst = Value::List((0..1_000).map(Value::Int).collect());
    for expected in 0..1_000i64 {
        assert_eq!(list::list_head(lst.clone()), Value::Int(expected));
        lst = list::list_tail(lst);
    }
    assert_eq!(lst, Value::List(vec![]));
}

#[test]
fn test_list_get_at_oob_returns_none() {
    setup();
    let lst = Value::List(vec![Value::Int(1), Value::Int(2)]);
    let result = list::list_get_at(t2(lst, Value::Int(99)));
    assert!(matches!(result, Value::Ctor { tag, .. } if {
        use axis_codegen_bridge::runtime::value::get_tag_name;
        get_tag_name(tag) == "None"
    }));
}

#[test]
fn test_list_nested_lists_large() {
    setup();
    let inner = Value::List((0..100).map(Value::Int).collect());
    let outer = Value::List(vec![inner.clone(); 500]);
    assert_eq!(list::list_len(outer.clone()), Value::Int(500));
    let first = list::list_head(outer);
    assert_eq!(first, inner);
}

// ── 4. String operations under stress ────────────────────────────────────────

#[test]
fn test_str_concat_chain_large() {
    setup();
    let mut acc = s("start");
    for i in 0..500 {
        acc = str_ops::str_concat(t2(acc, s(&format!("_{}", i))));
    }
    // Verify at least the length grew
    assert!(matches!(str_ops::str_len(acc), Value::Int(n) if n > 500));
}

#[test]
fn test_str_char_at_walk_unicode() {
    setup();
    let text = s("Hello, world! こんにちは🦀");
    let len = match str_ops::str_len(text.clone()) { Value::Int(n) => n, _ => panic!() };
    for i in 0..len {
        let r = str_ops::str_char_at(t2(text.clone(), Value::Int(i)));
        assert!(matches!(r, Value::Ctor { .. }), "expected Some at index {}", i);
    }
    let oob = str_ops::str_char_at(t2(text, Value::Int(len + 100)));
    assert!(matches!(oob, Value::Ctor { tag, .. } if {
        use axis_codegen_bridge::runtime::value::get_tag_name;
        get_tag_name(tag) == "None"
    }));
}

#[test]
fn test_str_slice_full_and_empty() {
    setup();
    let text = s("battlehardening");
    let full = str_ops::str_slice(Value::Tuple(vec![text.clone(), Value::Int(0), Value::Int(15)]));
    assert_eq!(full, text);
    let empty = str_ops::str_slice(Value::Tuple(vec![text, Value::Int(5), Value::Int(5)]));
    assert_eq!(empty, s(""));
}

#[test]
fn test_str_slice_beyond_end_clamped() {
    setup();
    let text = s("hello");
    let result = str_ops::str_slice(Value::Tuple(vec![text.clone(), Value::Int(0), Value::Int(9999)]));
    assert_eq!(result, text);
}

// ── 5. Panic / unwind cleanup ─────────────────────────────────────────────────

#[test]
fn test_panic_during_list_op_no_leak() {
    setup();
    // Each iteration allocates Values and then panics inside catch_unwind.
    // The test verifies we don't get a process-level double-free or abort.
    for i in 0..200 {
        let result = panic::catch_unwind(|| {
            let lst = Value::List((0..100).map(Value::Int).collect());
            let _ = lst.clone();
            if i % 2 == 0 {
                // trigger a real panic path through the runtime
                list::list_head(Value::List(vec![]));
            }
            lst
        });
        if i % 2 == 0 {
            assert!(result.is_err());
        } else {
            assert!(result.is_ok());
        }
    }
}

#[test]
fn test_assert_true_no_panic() {
    setup();
    let result = panic::catch_unwind(|| bool_ops::ax_assert(Value::Bool(true)));
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), Value::Unit);
}

#[test]
fn test_assert_false_panics() {
    setup();
    let result = panic::catch_unwind(|| bool_ops::ax_assert(Value::Bool(false)));
    assert!(result.is_err());
}

#[test]
fn test_int_div_by_zero_panics_then_resumes() {
    setup();
    for _ in 0..100 {
        let err = panic::catch_unwind(|| {
            arith::int_div(t2(Value::Int(100), Value::Int(0)))
        });
        assert!(err.is_err());
        // Normal ops still work after the panic
        assert_eq!(arith::int_div(t2(Value::Int(100), Value::Int(5))), Value::Int(20));
    }
}

#[test]
fn test_unwind_drops_nested_values() {
    setup();
    // Allocate deeply nested values inside a panicking closure; verifies drop
    // is called on unwind (no abort means drop completed without UB).
    for _ in 0..50 {
        let _ = panic::catch_unwind(|| {
            let _v = deep_list(50);
            let _w = deep_tuple(50);
            let _lst = Value::List((0..1000).map(Value::Int).collect());
            panic!("deliberate unwind");
        });
    }
}

#[test]
fn test_unwind_with_interned_strings_inside() {
    setup();
    for i in 0..200 {
        let _ = panic::catch_unwind(|| {
            let _s = s(&format!("unwind_str_{}", i));
            if i % 3 == 0 { panic!("deliberate"); }
            Value::Int(i)
        });
    }
}

// ── 6. AdaptiveCell<u8> single-threaded stress ───────────────────────────────

#[test]
fn test_adaptive_cell_write_read_1000_rounds() {
    let reg = AdaptiveRegistry::new();
    let mut cell: AdaptiveCell<u8> = AdaptiveCell::new();
    let handle = reg.acquire();

    for i in 0..=255u8 {
        let epoch = unsafe { cell.write(i, &reg) };
        assert!(epoch > 0);
        let rr = cell.read(0);
        use axis_codegen_bridge::runtime::non_blocking_memory::ReadResult;
        match rr {
            ReadResult::Value { value, .. } => assert_eq!(value, i),
            other => panic!("unexpected: {:?}", other),
        }
    }
    drop(handle);
}

#[test]
fn test_adaptive_cell_read_pinned_never_written_is_none() {
    let reg = AdaptiveRegistry::new();
    let cell: AdaptiveCell<u8> = AdaptiveCell::new();
    let handle = reg.acquire();
    assert!(cell.read_pinned(&handle, 0).is_none());
    drop(handle);
}

#[test]
fn test_adaptive_cell_epoch_increases_monotonically() {
    let reg = AdaptiveRegistry::new();
    let mut cell: AdaptiveCell<u8> = AdaptiveCell::new();
    let mut prev = 0u64;
    for v in 0..200u8 {
        let epoch = unsafe { cell.write(v, &reg) };
        assert!(epoch > prev, "epoch must increase: prev={} epoch={}", prev, epoch);
        prev = epoch;
    }
}

#[test]
fn test_adaptive_cell_read_pinned_returns_correct_value() {
    let reg = AdaptiveRegistry::new();
    let mut cell: AdaptiveCell<u8> = AdaptiveCell::new();
    unsafe { cell.write(42u8, &reg) };
    let handle = reg.acquire();
    let rref = cell.read_pinned(&handle, 0);
    assert!(rref.is_some());
    drop(rref);
    drop(handle);
}

// ── 7. AdaptiveCell<u8> multi-threaded sink stress ───────────────────────────

#[test]
fn test_adaptive_sink_32_entries_concurrent() {
    setup();
    const N: usize = 32;
    let cells: Vec<Arc<Mutex<AdaptiveCell<u8>>>> =
        (0..N).map(|_| Arc::new(Mutex::new(AdaptiveCell::new()))).collect();
    let regs: Vec<Arc<AdaptiveRegistry>> =
        (0..N).map(|_| Arc::new(AdaptiveRegistry::new())).collect();

    let handles: Vec<_> = (0..N).map(|i| {
        let cell = Arc::clone(&cells[i]);
        let reg  = Arc::clone(&regs[i]);
        thread::spawn(move || {
            // Half panic, half succeed
            let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                if i % 2 == 0 { panic!("deliberate entry panic"); }
                let _work: Value = Value::List((0..500).map(Value::Int).collect());
                unsafe { cell.lock().unwrap().write(1u8, &reg) };
            }));
            (i, result.is_ok())
        })
    }).collect();

    let results: Vec<(usize, bool)> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    for (i, ok) in results {
        if i % 2 == 0 {
            assert!(!ok, "entry {} should have panicked", i);
        } else {
            assert!(ok, "entry {} should have succeeded", i);
            // Verify cell was written
            use axis_codegen_bridge::runtime::non_blocking_memory::ReadResult;
            let rr = cells[i].lock().unwrap().read(0);
            assert!(matches!(rr, ReadResult::Value { value: 1, .. }));
        }
    }
}

#[test]
fn test_adaptive_sink_write_then_read_after_join() {
    const N: usize = 16;
    let cells: Vec<Arc<Mutex<AdaptiveCell<u8>>>> =
        (0..N).map(|_| Arc::new(Mutex::new(AdaptiveCell::new()))).collect();
    let regs: Vec<Arc<AdaptiveRegistry>> =
        (0..N).map(|_| Arc::new(AdaptiveRegistry::new())).collect();

    let handles: Vec<_> = (0..N).map(|i| {
        let cell = Arc::clone(&cells[i]);
        let reg  = Arc::clone(&regs[i]);
        thread::spawn(move || {
            unsafe { cell.lock().unwrap().write((i as u8).wrapping_add(1), &reg) };
        })
    }).collect();
    for h in handles { h.join().unwrap(); }

    use axis_codegen_bridge::runtime::non_blocking_memory::ReadResult;
    for i in 0..N {
        let handle = regs[i].acquire();
        let rref   = cells[i].lock().unwrap().read_pinned(&handle, 0);
        assert!(rref.is_some(), "entry {} should have a written value", i);
        // is_zero_copy false for u8 (inline slot, Copied variant)
        if let Some(r) = rref {
            assert!(!r.is_zero_copy());
            drop(r);
        }
        drop(handle);

        // Also verify via owned read
        let rr = cells[i].lock().unwrap().read(0);
        match rr {
            ReadResult::Value { value, .. } => {
                assert_eq!(value, (i as u8).wrapping_add(1));
            }
            other => panic!("entry {}: unexpected {:?}", i, other),
        }
    }
}

// ── 8. Mixed runtime ops stress under threads ─────────────────────────────────

#[test]
fn test_concurrent_list_ops_independent_threads() {
    setup();
    let handles: Vec<_> = (0..8).map(|tid| {
        thread::spawn(move || {
            let mut lst = Value::List(vec![]);
            for i in 0..500i64 {
                lst = list::list_append(t2(lst, Value::Int(tid * 1000 + i)));
            }
            assert_eq!(list::list_len(lst.clone()), Value::Int(500));
            let rev = list::list_reverse(lst.clone());
            assert_eq!(list::list_head(rev), Value::Int(tid * 1000 + 499));
        })
    }).collect();
    for h in handles { h.join().unwrap(); }
}

#[test]
fn test_concurrent_str_concat_independent_threads() {
    setup();
    let handles: Vec<_> = (0..8).map(|tid| {
        thread::spawn(move || {
            let mut acc = s(&format!("t{}", tid));
            for i in 0..100 {
                acc = str_ops::str_concat(t2(acc, s(&format!("_{}", i))));
            }
            assert!(matches!(str_ops::str_len(acc), Value::Int(n) if n > 100));
        })
    }).collect();
    for h in handles { h.join().unwrap(); }
}

#[test]
fn test_value_equality_under_clone_stress() {
    setup();
    let original = Value::List(vec![
        Value::Int(1),
        Value::Str(intern_str("hello")),
        Value::Bool(true),
        Value::Tuple(vec![Value::Int(99), Value::Unit]),
    ]);
    let mut clones: Vec<Value> = (0..1_000).map(|_| original.clone()).collect();
    for c in &clones {
        assert_eq!(c, &original);
    }
    // Drop in reverse to stress drop ordering
    while let Some(v) = clones.pop() {
        drop(v);
    }
}

#[test]
fn test_arith_saturating_extremes() {
    setup();
    assert_eq!(
        arith::int_add(t2(Value::Int(i64::MAX), Value::Int(0))),
        Value::Int(i64::MAX)
    );
    assert_eq!(
        arith::int_sub(t2(Value::Int(i64::MIN), Value::Int(0))),
        Value::Int(i64::MIN)
    );
    assert_eq!(
        arith::int_mul(t2(Value::Int(0), Value::Int(i64::MAX))),
        Value::Int(0)
    );
}

#[test]
fn test_bool_ops_stress() {
    setup();
    for i in 0..10_000 {
        let v = arith::int_gt(t2(Value::Int(i), Value::Int(0)));
        let _ = bool_ops::bool_not(v.clone());
        let _ = bool_ops::bool_and(t2(v.clone(), Value::Bool(true)));
        let _ = bool_ops::bool_or(t2(v, Value::Bool(false)));
    }
}

// ── 9. Drop counter: verify values don't escape panics ───────────────────────

use std::sync::atomic::{AtomicUsize, Ordering as AOrdering};

static DROP_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Tracked;
impl Drop for Tracked {
    fn drop(&mut self) { DROP_COUNTER.fetch_add(1, AOrdering::Relaxed); }
}

#[test]
fn test_tracked_drop_through_catch_unwind() {
    // Not using Value here — verifies Rust unwind safety pattern used in harness.
    DROP_COUNTER.store(0, AOrdering::Relaxed);
    let result = panic::catch_unwind(|| {
        let _t1 = Tracked;
        let _t2 = Tracked;
        let _t3 = Tracked;
        panic!("unwind");
    });
    assert!(result.is_err());
    assert_eq!(DROP_COUNTER.load(AOrdering::Relaxed), 3, "all three Tracked must be dropped");
}

#[test]
fn test_tracked_drop_no_panic_path() {
    DROP_COUNTER.store(0, AOrdering::Relaxed);
    let result = panic::catch_unwind(|| {
        let _t1 = Tracked;
        let _t2 = Tracked;
        42
    });
    assert!(result.is_ok());
    assert_eq!(DROP_COUNTER.load(AOrdering::Relaxed), 2);
}

// ── 10. Reclaim sweep — retired block list stays bounded ─────────────────────

#[test]
fn test_adaptive_cell_reclaim_keeps_retired_bounded() {
    // u8 is inline — block path never triggered, retired always 0.
    let reg = AdaptiveRegistry::new();
    let mut cell: AdaptiveCell<u8> = AdaptiveCell::new();
    for i in 0..=255u8 {
        unsafe { cell.write(i, &reg) };
    }
    let retired = cell.retired_len();
    assert_eq!(retired, 0, "u8 inline path must never add retired blocks, got {}", retired);
}

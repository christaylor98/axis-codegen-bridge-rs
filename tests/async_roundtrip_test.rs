//! Round-trip parity for BRIDGE_ASYNC_PRIMITIVES_V1.
//!
//! Reproduces the exact 5-round ping-pong exchange that `runtime/signals.rs`
//! performed with its bespoke SPSC channels, but built entirely from the three
//! real foreign primitives — `event_subscribe`, `channel_send`, `wait`. This is
//! the SIGNALS_RS_REMOVED_LAST gate: signals.rs is only deleted once this passes.
//!
//! The payload shapes, the `process` computation, and the per-round verification
//! are byte-for-byte the same logic signals.rs used, so a pass here means the
//! primitives are behaviourally equivalent to the retired prototype.

use axis_codegen_bridge::runtime::channels::{channel_send, event_subscribe, wait};
use axis_codegen_bridge::runtime::value::{get_str, intern_str, Value};

const ROUNDS: usize = 5;

fn make_payload(round: usize) -> Value {
    match round {
        0 => Value::Int(7),
        1 => Value::List((0..100i64).map(Value::Int).collect()),
        2 => Value::Str(intern_str(&"hello world ".repeat(20))),
        3 => Value::List((0..5_000i64).map(Value::Int).collect()),
        4 => Value::List(vec![
            Value::List((0..1_000i64).map(Value::Int).collect()),
            Value::List((0..1_000i64).map(Value::Int).collect()),
            Value::List((0..1_000i64).map(Value::Int).collect()),
        ]),
        _ => unreachable!(),
    }
}

fn process(v: &Value) -> Value {
    match v {
        Value::Int(n) => Value::Int(n * n),
        Value::Str(h) => Value::Int(get_str(*h).chars().count() as i64),
        Value::List(es) if es.iter().all(|e| matches!(e, Value::List(_))) => {
            let mut total: i64 = 0;
            for inner in es {
                if let Value::List(xs) = inner {
                    for x in xs {
                        if let Value::Int(n) = x {
                            total += n;
                        }
                    }
                }
            }
            Value::Int(total)
        }
        Value::List(es) => {
            let sum: i64 = es
                .iter()
                .filter_map(|x| if let Value::Int(n) = x { Some(*n) } else { None })
                .sum();
            Value::Int(sum)
        }
        _ => Value::Unit,
    }
}

// ── Bare-fn handlers (CLOSURE_RULE_HARD) ──────────────────────────────────────
//
// Both handlers are plain `fn(Value) -> Value` pointers — no captured state.
// The compiler will not let them close over `expected`/`round`, which is exactly
// why ping's per-round verification lives OUTSIDE the handler (below): `wait`
// returns the handler's result, and the caller compares it.

/// pong's handler: WAIT_ALWAYS_LIST delivers the payload as a single-element
/// list. Process it and reply on `b2a`.
fn pong_handler(msgs: Value) -> Value {
    let payload = match &msgs {
        Value::List(es) => es[0].clone(),
        other => panic!("pong_handler expected List, got {:?}", other),
    };
    let result = process(&payload);
    channel_send(Value::Tuple(vec![Value::Str(intern_str("b2a")), result]));
    Value::Unit
}

/// ping's handler: return the received result so `wait` hands it back to the
/// caller for verification.
fn ping_handler(msgs: Value) -> Value {
    match msgs {
        Value::List(mut es) => es.remove(0),
        other => panic!("ping_handler expected List, got {:?}", other),
    }
}

#[test]
fn ping_pong_roundtrip_via_primitives() {
    let pong = std::thread::spawn(|| {
        event_subscribe(Value::Str(intern_str("a2b")));
        for _ in 0..ROUNDS {
            wait(pong_handler);
        }
    });

    let ping = std::thread::spawn(|| {
        event_subscribe(Value::Str(intern_str("b2a")));
        for round in 0..ROUNDS {
            let payload = make_payload(round);
            let expected = process(&payload);
            channel_send(Value::Tuple(vec![Value::Str(intern_str("a2b")), payload]));
            let got = wait(ping_handler);
            assert_eq!(
                got,
                expected,
                "ping: round {} mismatch — expected {:?} got {:?}",
                round + 1,
                expected,
                got
            );
        }
    });

    ping.join().expect("ping thread panicked");
    pong.join().expect("pong thread panicked");
}

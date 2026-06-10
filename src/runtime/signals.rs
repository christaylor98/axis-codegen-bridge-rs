use super::value::{Value, intern_str, get_str};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

// ── SPSC channel ──────────────────────────────────────────────────────────────
//
// Sender fills slot (mutex), then bumps epoch (Release).
// Receiver spins on epoch (Acquire), then takes from slot.
// Safe for exactly one concurrent sender and one concurrent receiver.

struct Chan {
    epoch: AtomicU64,
    slot:  Mutex<Option<Value>>,
}

impl Chan {
    fn new() -> Self {
        Chan { epoch: AtomicU64::new(0), slot: Mutex::new(None) }
    }

    fn send(&self, value: Value) {
        *self.slot.lock().unwrap() = Some(value);
        self.epoch.fetch_add(1, Ordering::Release);
    }

    fn recv(&self, last_epoch: u64) -> (Value, u64) {
        loop {
            let e = self.epoch.load(Ordering::Acquire);
            if e > last_epoch {
                if let Some(v) = self.slot.lock().unwrap().take() {
                    return (v, e);
                }
            }
            std::thread::yield_now();
        }
    }
}

static A2B: OnceLock<Chan> = OnceLock::new();  // ping → pong
static B2A: OnceLock<Chan> = OnceLock::new();  // pong → ping
fn a2b() -> &'static Chan { A2B.get_or_init(Chan::new) }
fn b2a() -> &'static Chan { B2A.get_or_init(Chan::new) }

// ── Test rounds (varying payload sizes and shapes) ────────────────────────────

const ROUNDS: usize = 5;

fn make_payload(round: usize) -> Value {
    match round {
        // Round 1: scalar Int — baseline
        0 => Value::Int(7),
        // Round 2: small flat list (100 elems)
        1 => Value::List((0..100i64).map(Value::Int).collect()),
        // Round 3: medium string (240 chars)
        2 => Value::Str(intern_str(&"hello world ".repeat(20))),
        // Round 4: large flat list (5 000 elems)
        3 => Value::List((0..5_000i64).map(Value::Int).collect()),
        // Round 5: nested lists (3 × 1 000 elems) — tests deep Value traversal
        4 => Value::List(vec![
            Value::List((0..1_000i64).map(Value::Int).collect()),
            Value::List((0..1_000i64).map(Value::Int).collect()),
            Value::List((0..1_000i64).map(Value::Int).collect()),
        ]),
        _ => unreachable!(),
    }
}

// Pong's computation: varies by shape so each round exercises different logic.
fn process(v: &Value) -> Value {
    match v {
        // scalar  → square
        Value::Int(n) => Value::Int(n * n),

        // string  → char count
        Value::Str(h) => Value::Int(get_str(*h).chars().count() as i64),

        // nested list → flatten + sum all integers
        Value::List(es) if es.iter().all(|e| matches!(e, Value::List(_))) => {
            let mut total: i64 = 0;
            for inner in es {
                if let Value::List(xs) = inner {
                    for x in xs {
                        if let Value::Int(n) = x { total += n; }
                    }
                }
            }
            Value::Int(total)
        }

        // flat list → sum all integers
        Value::List(es) => {
            let sum: i64 = es.iter()
                .filter_map(|x| if let Value::Int(n) = x { Some(*n) } else { None })
                .sum();
            Value::Int(sum)
        }

        _ => Value::Unit,
    }
}

fn shape_of(v: &Value) -> String {
    match v {
        Value::Int(_)  => "Int(scalar)".to_string(),
        Value::Str(h)  => format!("Str({} chars)", get_str(*h).chars().count()),
        Value::List(es) if es.iter().all(|e| matches!(e, Value::List(_))) =>
            format!("List({}×List({}))", es.len(),
                if let Some(Value::List(inner)) = es.first() { inner.len() } else { 0 }),
        Value::List(es) => format!("List({} elems)", es.len()),
        _              => "other".to_string(),
    }
}

// ── Entry points ──────────────────────────────────────────────────────────────

pub fn ping_loop(_: Value) -> Value {
    let mut last_epoch: u64 = 0;
    for round in 0..ROUNDS {
        let payload  = make_payload(round);
        let expected = process(&payload);
        let shape    = shape_of(&payload);
        a2b().send(payload);
        let (result, e) = b2a().recv(last_epoch);
        last_epoch = e;
        if result != expected {
            panic!("ping: round {} mismatch — expected {:?} got {:?}",
                   round + 1, expected, result);
        }
        println!("ping: round {} ok   [{} → {}]", round + 1, shape, result);
    }
    Value::Unit
}

pub fn pong_loop(_: Value) -> Value {
    let mut last_epoch: u64 = 0;
    for round in 0..ROUNDS {
        let (payload, e) = a2b().recv(last_epoch);
        last_epoch = e;
        let shape  = shape_of(&payload);
        let result = process(&payload);
        b2a().send(result.clone());
        println!("pong: round {} done [{}]", round + 1, shape);
    }
    Value::Unit
}

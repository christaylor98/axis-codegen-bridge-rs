use super::value::Value;
use std::sync::atomic::{AtomicU32, Ordering};

const ROUNDS: u32 = 5;

static PING_EPOCH: AtomicU32 = AtomicU32::new(0);
static PONG_EPOCH: AtomicU32 = AtomicU32::new(0);

pub fn ping_loop(_: Value) -> Value {
    for round in 1..=ROUNDS {
        PING_EPOCH.store(round, Ordering::Release);
        while PONG_EPOCH.load(Ordering::Acquire) < round {
            std::thread::yield_now();
        }
        println!("ping: round {}", round);
    }
    Value::Unit
}

pub fn pong_loop(_: Value) -> Value {
    for round in 1..=ROUNDS {
        while PING_EPOCH.load(Ordering::Acquire) < round {
            std::thread::yield_now();
        }
        PONG_EPOCH.store(round, Ordering::Release);
        println!("pong: round {}", round);
    }
    Value::Unit
}

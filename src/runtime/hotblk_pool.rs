//! HOTBLK_POOL_V1 (AXVERITY_SLICE4_BLOCK_DURABILITY_V2, item 6) — arena
//! pre-allocation off the hot path. A per-shard bounded queue of freshly-
//! minted `(ptr, cell)` pairs; a dedicated allocator thread per shard keeps it
//! several blocks ahead, so the request thread's `pg_hotblk_mint` takes a
//! ready arena instead of calling `hrw_mint_block` (mem_reserve_raw) itself.
//!
//! Deliberately its OWN small Mutex+Condvar bounded queue, NOT a reuse of
//! `channels.rs`'s `bchan_send`/`bchan_drain` — those share ONE global
//! capacity (`AXVERITY_RECLOG_CAP`, default 1024) across every bounded-channel
//! name, which is reclog's own tuning knob; reusing it here would mean either
//! entangling this pool's depth with reclog's unrelated backpressure setting,
//! or silently pre-allocating up to 1024 * 4MiB = 4GiB per shard. This module
//! gets an independent depth knob (`AXVERITY_SLICE4_POOL_DEPTH`).
//!
//! Shared, not thread-local (unlike hotblk.rs's accumulator): this is
//! INHERENTLY cross-thread by construction (the allocator thread produces,
//! the request thread consumes), the same reasoning `oneshot.rs` and
//! `ack_registry.rs` already document for their own shared state.

use std::collections::{HashMap, VecDeque};
use std::sync::{Condvar, Mutex, OnceLock};

use super::value::{get_str, intern_str, Value};

struct Pool {
    queue: Mutex<VecDeque<(i64, i64)>>,
    cap: usize,
    not_full: Condvar,
    not_empty: Condvar,
}

fn pool_depth() -> usize {
    static DEPTH: OnceLock<usize> = OnceLock::new();
    *DEPTH.get_or_init(|| {
        std::env::var("AXVERITY_SLICE4_POOL_DEPTH")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|n| *n >= 1)
            .unwrap_or(2)
    })
}

fn registry() -> &'static Mutex<HashMap<String, std::sync::Arc<Pool>>> {
    static REG: OnceLock<Mutex<HashMap<String, std::sync::Arc<Pool>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

fn pool_for(shard: &str) -> std::sync::Arc<Pool> {
    registry()
        .lock()
        .unwrap()
        .entry(shard.to_string())
        .or_insert_with(|| {
            std::sync::Arc::new(Pool {
                queue: Mutex::new(VecDeque::new()),
                cap: pool_depth(),
                not_full: Condvar::new(),
                not_empty: Condvar::new(),
            })
        })
        .clone()
}

/// Push a freshly-minted `(ptr, cell)` into `shard`'s pool, BLOCKING while
/// already at the configured depth (so the allocator thread naturally stays
/// "several blocks ahead," never unboundedly so). Internal Rust entry — the
/// per-shard allocator thread calls this in a loop.
pub(crate) fn pool_put(shard: &str, ptr: i64, cell: i64) {
    let p = pool_for(shard);
    let mut q = p.queue.lock().unwrap();
    while q.len() >= p.cap {
        q = p.not_full.wait(q).unwrap();
    }
    q.push_back((ptr, cell));
    drop(q);
    p.not_empty.notify_one();
}

/// Pop one ready `(ptr, cell)` from `shard`'s pool, BLOCKING if empty (the
/// documented "what happens when the allocator can't keep up" case the intent
/// requires be measured, not assumed away). Internal Rust entry —
/// `pg_hotblk_mint` calls this instead of minting inline.
pub(crate) fn pool_take(shard: &str) -> (i64, i64) {
    let p = pool_for(shard);
    let mut q = p.queue.lock().unwrap();
    while q.is_empty() {
        q = p.not_empty.wait(q).unwrap();
    }
    let item = q.pop_front().unwrap();
    drop(q);
    p.not_full.notify_one();
    item
}

/// `hotblk_pool_put(shard: Text, ptr: Int, cell: Int) -> Unit`
#[track_caller]
pub fn hotblk_pool_put(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 3 => es,
        other => panic!("hotblk_pool_put: expected Tuple(Text, Int, Int), got {:?}", other),
    };
    let shard = match &es[0] {
        Value::Str(s) => get_str(s),
        other => panic!("hotblk_pool_put: arg 0 expected Text, got {:?}", other),
    };
    let ptr = match &es[1] {
        Value::Int(n) => *n,
        other => panic!("hotblk_pool_put: arg 1 expected Int, got {:?}", other),
    };
    let cell = match &es[2] {
        Value::Int(n) => *n,
        other => panic!("hotblk_pool_put: arg 2 expected Int, got {:?}", other),
    };
    pool_put(&shard, ptr, cell);
    Value::Unit
}

/// `hotblk_pool_take(shard: Text) -> Text` — `"<ptr>\t<cell>"`.
#[track_caller]
pub fn hotblk_pool_take(arg: Value) -> Value {
    let shard = match arg {
        Value::Str(s) => get_str(&s),
        other => panic!("hotblk_pool_take: expected Text shard, got {:?}", other),
    };
    let (ptr, cell) = pool_take(&shard);
    Value::Str(intern_str(&format!("{}\t{}", ptr, cell)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn put_then_take_round_trips() {
        let shard = "test-shard-a";
        pool_put(shard, 111, 222);
        assert_eq!(pool_take(shard), (111, 222));
    }

    #[test]
    fn take_blocks_until_put() {
        let shard = "test-shard-b";
        let t = thread::spawn(move || {
            thread::sleep(Duration::from_millis(15));
            pool_put(shard, 42, 43);
        });
        assert_eq!(pool_take(shard), (42, 43));
        t.join().unwrap();
    }

    #[test]
    fn put_blocks_when_at_depth() {
        // With AXVERITY_SLICE4_POOL_DEPTH unset/parsed elsewhere, exercise the
        // default depth (>=1) by filling to capacity then confirming a
        // subsequent put only completes once a take makes room.
        let shard = "test-shard-c";
        let depth = pool_depth();
        for i in 0..depth {
            pool_put(shard, i as i64, i as i64);
        }
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done2 = done.clone();
        let t = thread::spawn(move || {
            pool_put(shard, 999, 999); // blocks until a slot frees
            done2.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        thread::sleep(Duration::from_millis(20));
        assert!(!done.load(std::sync::atomic::Ordering::SeqCst), "put should still be blocked at capacity");
        let _ = pool_take(shard); // frees one slot
        t.join().unwrap();
        assert!(done.load(std::sync::atomic::Ordering::SeqCst), "put should have completed after a take");
        // drain the rest so this test doesn't leak into others sharing the shard name
        for _ in 0..depth {
            let _ = pool_take(shard);
        }
    }
}

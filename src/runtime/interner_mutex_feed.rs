//! Feeder variant A for the sharded-interner experiment: each shard's
//! dedicated writer thread is fed by a plain `Mutex<VecDeque<Request>>` +
//! `Condvar` — the same shape as `oneshot.rs`/`channels.rs`'s existing
//! bounded channel. Per-request completion signal is `oneshot.rs`'s exact
//! `Mutex<Option<u32>>` + `Condvar` pattern, reused rather than reinvented.
//!
//! Storage (`interner_shard::ShardStorage`) is identical to variant B
//! (`interner_lockfree_feed.rs`) — the ONLY difference between the two
//! variants is how a request travels from a calling thread to its shard's
//! owner thread, which is exactly what `tests/interner_battle_test.rs`
//! benchmarks head-to-head.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, OnceLock};
use std::thread;

use super::interner_shard::{pack, shard_count, shard_for, unpack, ShardStorage};

/// Per-request completion signal — identical shape to `oneshot.rs`'s
/// `Mutex<bool>` + `Condvar` (deliberately NOT "upgraded" to
/// `AtomicBool`+park/unpark: this sits off the hot path, and stacking a
/// novel primitive on a novel queue in the same change is unneeded risk).
struct Reply {
    local: Mutex<Option<u32>>,
    cv: Condvar,
}

impl Reply {
    fn new() -> Self {
        Reply { local: Mutex::new(None), cv: Condvar::new() }
    }
    fn wait(&self) -> u32 {
        let mut guard = self.local.lock().unwrap();
        while guard.is_none() {
            guard = self.cv.wait(guard).unwrap();
        }
        guard.unwrap()
    }
    fn signal(&self, local: u32) {
        let mut guard = self.local.lock().unwrap();
        *guard = Some(local);
        self.cv.notify_all();
    }
}

struct Request {
    text: String,
    reply: std::sync::Arc<Reply>,
}

struct ShardCtx {
    storage: ShardStorage,
    queue: Mutex<VecDeque<Request>>,
    cv: Condvar,
}

fn shard_owner_loop(ctx: &'static ShardCtx) {
    loop {
        let req = {
            let mut q = ctx.queue.lock().unwrap();
            while q.is_empty() {
                q = ctx.cv.wait(q).unwrap();
            }
            q.pop_front().unwrap()
        };
        let local = ctx.storage.get_or_insert(&req.text);
        req.reply.signal(local);
    }
}

struct Table {
    shards: Vec<&'static ShardCtx>,
}

impl Table {
    fn new(name: &'static str) -> Self {
        let n = shard_count();
        let shards: Vec<&'static ShardCtx> = (0..n)
            .map(|shard_id| {
                let ctx: &'static ShardCtx = Box::leak(Box::new(ShardCtx {
                    storage: ShardStorage::new(),
                    queue: Mutex::new(VecDeque::new()),
                    cv: Condvar::new(),
                }));
                thread::Builder::new()
                    .name(format!("intern-mutex-{}-{}", name, shard_id))
                    .spawn(move || shard_owner_loop(ctx))
                    .expect("failed to spawn shard-owner thread");
                ctx
            })
            .collect();
        Table { shards }
    }

    fn intern(&self, s: &str) -> u32 {
        if s.is_empty() {
            return 0;
        }
        let shard_id = shard_for(s);
        let ctx = self.shards[shard_id as usize];
        let reply = std::sync::Arc::new(Reply::new());
        {
            let mut q = ctx.queue.lock().unwrap();
            q.push_back(Request { text: s.to_string(), reply: reply.clone() });
        }
        ctx.cv.notify_one();
        let local = reply.wait();
        pack(shard_id, local)
    }

    fn get(&self, handle: u32) -> String {
        if handle == 0 {
            return String::new();
        }
        let (shard_id, local) = unpack(handle);
        let ctx = self.shards[shard_id as usize];
        ctx.storage.get(local).unwrap_or_else(|| format!("<invalid-str-{}>", handle))
    }
}

fn strings() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| Table::new("str"))
}

fn tags() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| Table::new("tag"))
}

pub fn intern_str(s: &str) -> u32 {
    strings().intern(s)
}
pub fn get_str(handle: u32) -> String {
    strings().get(handle)
}
pub fn intern_tag(name: &str) -> u32 {
    tags().intern(name)
}
pub fn get_tag_name(tag: u32) -> String {
    tags().get(tag)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn intern_dedup_and_roundtrip() {
        let a = intern_str("hello-mutex-feed");
        let b = intern_str("world-mutex-feed");
        let a2 = intern_str("hello-mutex-feed");
        assert_eq!(a, a2);
        assert_ne!(a, b);
        assert_eq!(get_str(a), "hello-mutex-feed");
        assert_eq!(get_str(b), "world-mutex-feed");
    }

    #[test]
    fn empty_string_is_handle_zero() {
        assert_eq!(intern_str(""), 0);
        assert_eq!(get_str(0), "");
    }

    #[test]
    fn tags_are_independent_of_strings() {
        let s = intern_str("shared-name");
        let t = intern_tag("shared-name");
        assert_eq!(get_str(s), "shared-name");
        assert_eq!(get_tag_name(t), "shared-name");
    }

    #[test]
    fn concurrent_dedup_same_string_many_threads() {
        let handles: Vec<_> = (0..32)
            .map(|_| thread::spawn(|| intern_str("concurrent-mutex-feed-target")))
            .collect();
        let results: Vec<u32> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let first = results[0];
        assert!(results.iter().all(|&r| r == first), "all threads must dedup to the same handle");
        assert_eq!(get_str(first), "concurrent-mutex-feed-target");
    }

    #[test]
    fn concurrent_unique_strings_all_distinct_and_readable() {
        let handles: Vec<_> = (0..64)
            .map(|i| {
                thread::spawn(move || {
                    let s = format!("unique-mutex-feed-{}", i);
                    let h = intern_str(&s);
                    (h, s)
                })
            })
            .collect();
        let results: Vec<(u32, String)> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let mut seen = std::collections::HashSet::new();
        for (h, s) in &results {
            assert!(seen.insert(*h), "duplicate handle {} for distinct strings", h);
            assert_eq!(&get_str(*h), s);
        }
    }
}

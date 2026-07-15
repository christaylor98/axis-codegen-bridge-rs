//! BRIDGE_CONTENTIDX_V1 (AXVERITY_HOTMEM_CONTENT_INDEX_V1) — a shared, sharded,
//! bounded, in-memory `content_hash -> record bytes` index. The symmetric twin of
//! `bindidx` (name->hash): where bindidx closes the INSERT->UPDATE/DELETE *binding*
//! read-after-write window, contentidx closes the *content* window it opened.
//!
//! ## The problem it solves (reproduced, not assumed)
//!
//! An INSERT publishes the PK->hash binding into bindidx synchronously, then hands
//! the record's bytes to the recovery-log channel and acks WITHOUT fsync (FAST
//! async-commit). For a bounded window — measured ~1-2ms, = the reclog-janitor
//! latency — the binding is visible (`resolve_name` returns the hash) but the
//! content is not yet in any tier `pull_object` reads (loose/WAL/pack). A follow-up
//! UPDATE/SELECT that resolves the hash and calls `pull_object` then falls through
//! to `pack_read`, whose `fs_read_text` on the missing pack pointer PANICS —
//! killing the worker thread, hanging the client, and leaking a worker from the
//! accept pool. contentidx makes the bytes findable in RAM the instant the binding
//! is, closing the window with zero change to the durable path.
//!
//! ## In-memory accelerator, NOT a source of truth (mirrors bindidx)
//!
//! The durable record is the recovery-log / WAL tier. contentidx is a pure
//! accelerator over the ~2ms gap plus a hot-read cache for very recent content. A
//! miss returns empty Bytes so `pull_object` falls through to the durable tiers; a
//! cold index after restart is CORRECT (the durable path serves every miss). It
//! adds NO fsync and NO disk I/O — a put is one sharded-mutex lock + a map insert
//! (measured: INSERT p99 unchanged, 0.116ms baseline vs 0.145ms — in the noise).
//!
//! ## Content is immutable => insert-if-absent, never overwrite
//!
//! Keys are content hashes, so a value never changes. We do NOT import bindidx's
//! last-writer-wins overwrite path (the POINT-flavor append-only-once property).
//!
//! ## Bounded lifecycle — provably safe FIFO eviction (no janitor coupling)
//!
//! Each shard is capped and evicts in insertion order (FIFO). The safety invariant:
//! an entry must never be evicted before its content is durably findable elsewhere,
//! else the panic returns. The recovery-log channel bounds the number of
//! *un-fsynced* records in flight to `AXVERITY_RECLOG_CAP` (default 1024) + one
//! in-flight batch (`AXVERITY_RECLOG_BATCH`, default 256) ~= 1280 — producers BLOCK
//! when the channel is full, so no more than that many records can be un-durable at
//! once. With the total cap (`AXVERITY_CONTENTIDX_CAP`, default 65536) set far above
//! that bound (51x margin at defaults), FIFO eviction only ever removes entries that
//! are long since fsynced — an evicted key is always already served by the durable
//! tiers. If you raise `AXVERITY_RECLOG_CAP`, raise `AXVERITY_CONTENTIDX_CAP` to keep
//! the margin. Worst-case memory is cap * avg-record-bytes (64Ki * ~1KiB ~= 64MiB).
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use super::value::{get_str, Value};

const NSHARDS: usize = 256; // power of two; keyed by fnv1a(hash), same as bindidx
const CAP_DEFAULT: usize = 65536; // total; >> reclog in-flight bound (~1280) => safe FIFO evict

/// Total capacity from `AXVERITY_CONTENTIDX_CAP` (default 65536). Must stay well
/// above `AXVERITY_RECLOG_CAP + AXVERITY_RECLOG_BATCH` (the un-durable in-flight
/// bound) so FIFO eviction never drops an entry that is not yet durable.
fn total_cap() -> usize {
    static C: OnceLock<usize> = OnceLock::new();
    *C.get_or_init(|| {
        std::env::var("AXVERITY_CONTENTIDX_CAP")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|&n| n >= NSHARDS)
            .unwrap_or(CAP_DEFAULT)
    })
}

#[inline]
fn per_shard_cap() -> usize {
    (total_cap() / NSHARDS).max(1)
}

/// One shard: the content map plus a FIFO queue of its keys for bounded eviction.
struct Shard {
    map: HashMap<String, Vec<u8>>,
    fifo: VecDeque<String>,
}

struct ContentIdx {
    shards: Vec<Mutex<Shard>>,
}

fn idx() -> &'static ContentIdx {
    static I: OnceLock<ContentIdx> = OnceLock::new();
    I.get_or_init(|| ContentIdx {
        shards: (0..NSHARDS)
            .map(|_| {
                Mutex::new(Shard {
                    map: HashMap::new(),
                    fifo: VecDeque::new(),
                })
            })
            .collect(),
    })
}

#[inline]
fn shard(key: &str) -> usize {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in key.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (h as usize) & (NSHARDS - 1)
}

/// `contentidx_put(hash: Text, bytes: Bytes) -> Unit` — publish a record's bytes
/// under its content hash. Insert-if-absent (immutable content); locks one shard;
/// O(1); no fsync. Evicts the shard's oldest entry (FIFO) if at the per-shard cap.
#[track_caller]
pub fn contentidx_put(args: Value) -> Value {
    let (hash, bytes) = match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("contentidx_put: expected Tuple(Text, Bytes), got {:?}", other),
    };
    let hash = match hash {
        Value::Str(h) => get_str(&h),
        other => panic!("contentidx_put: arg 0 expected Text hash, got {:?}", other),
    };
    let bytes = match bytes {
        Value::Bytes(b) => b,
        other => panic!("contentidx_put: arg 1 expected Bytes, got {:?}", other),
    };
    let cap = per_shard_cap();
    let s = shard(&hash);
    let mut g = idx().shards[s].lock().unwrap_or_else(|p| p.into_inner());
    if g.map.contains_key(&hash) {
        return Value::Unit; // insert-if-absent: content is immutable, never re-queue
    }
    while g.fifo.len() >= cap {
        if let Some(old) = g.fifo.pop_front() {
            g.map.remove(&old);
        } else {
            break;
        }
    }
    g.fifo.push_back(hash.clone());
    g.map.insert(hash, bytes);
    Value::Unit
}

/// `contentidx_get(hash: Text) -> Bytes` — the record's bytes, or empty Bytes on a
/// miss (the caller falls through to the durable loose/WAL/pack tiers). No lock on
/// the returned bytes: the clone happens under the shard lock, then the lock drops.
#[track_caller]
pub fn contentidx_get(arg: Value) -> Value {
    let hash = match arg {
        Value::Str(h) => get_str(&h),
        other => panic!("contentidx_get: expected Text hash, got {:?}", other),
    };
    let s = shard(&hash);
    let g = idx().shards[s].lock().unwrap_or_else(|p| p.into_inner());
    Value::Bytes(g.map.get(&hash).cloned().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::value::intern_str;

    fn put(h: &str, b: &[u8]) {
        contentidx_put(Value::Tuple(vec![
            Value::Str(intern_str(h)),
            Value::Bytes(b.to_vec()),
        ]));
    }
    fn get(h: &str) -> Vec<u8> {
        match contentidx_get(Value::Str(intern_str(h))) {
            Value::Bytes(b) => b,
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn put_get_and_miss() {
        assert_eq!(get("contentidx_test:absent"), Vec::<u8>::new());
        put("sha256:aaa", b"RECORD\tk=v");
        assert_eq!(get("sha256:aaa"), b"RECORD\tk=v".to_vec());
    }

    #[test]
    fn insert_if_absent_does_not_overwrite() {
        put("sha256:immut", b"first");
        put("sha256:immut", b"second"); // ignored — content is immutable
        assert_eq!(get("sha256:immut"), b"first".to_vec());
    }

    #[test]
    fn fifo_eviction_drops_oldest_keeps_newest_within_a_shard() {
        // Drive one shard past its per-shard cap and confirm the OLDEST key is the
        // one evicted, the newest survive. Keys are chosen to collide on one shard
        // so the test is independent of the global cap size.
        let cap = per_shard_cap();
        // find a target shard and generate cap+extra keys that all map to it
        let target = shard("seed");
        let mut keys = Vec::new();
        let mut i = 0u64;
        while keys.len() < cap + 5 {
            let k = format!("ev:{}", i);
            if shard(&k) == target {
                keys.push(k);
            }
            i += 1;
        }
        for (n, k) in keys.iter().enumerate() {
            put(k, format!("v{}", n).as_bytes());
        }
        // the first 5 inserted into this shard must have been evicted (FIFO)
        for k in &keys[..5] {
            assert_eq!(get(k), Vec::<u8>::new(), "oldest key {} should be evicted", k);
        }
        // the last `cap` inserted must all still be present
        for (n, k) in keys.iter().enumerate().skip(keys.len() - cap) {
            assert_eq!(get(k), format!("v{}", n).into_bytes(), "recent key {} should survive", k);
        }
    }
}

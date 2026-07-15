//! BRIDGE_BINDIDX_V1 (AXVERITY_STORAGE_SUBSTRATE_DURABILITY_V1 slice 3a) — a shared,
//! sharded, in-memory `name → current-value` binding index that gives
//! CROSS-CONNECTION read-after-write for `resolve_name`.
//!
//! A mutation (`bindidx_put(name, value)` at INSERT / UPDATE / DELETE) is visible to
//! `bindidx_get(name)` on ANY worker thread instantly, with no wait — closing the
//! window between an INSERT and a same-or-other-connection UPDATE/DELETE that today
//! only sees the binding after the durable name-log lands.
//!
//! ## Sharded, not single-locked (the Spike-4 distinction, measured)
//!
//! The index is 256 lock shards keyed by fnv1a(name). A bind locks exactly one shard,
//! so concurrent binds on distinct names (the realistic distinct-PK case) rarely
//! collide. This is a genuine mitigation, not the naive single lock that already
//! failed: under REAL simultaneous N-writer contention the sharded index SCALES UP
//! with writers (measured: 256 shards → 11.7M binds/s at 16 writers; hot-key worst
//! case 12.9M) whereas a single lock DEGRADES (1.26M→1.02M as writers 4→16). Shard
//! count was swept (16/64/256), not assumed; 256 was the best of the three and clears
//! the target with headroom.
//!
//! ## Rebuildable cache — NOT a source of truth
//!
//! The durable record is elsewhere: the name-log today, the mmap segments after the
//! slice-4 cutover. This index is a pure in-memory accelerator. A cold index on
//! restart is CORRECT — `resolve_name`'s existing durable fallback serves the miss —
//! and re-warms as writes land. `bindidx_get` returns "" on a miss precisely so the
//! caller falls through to that durable path; it never manufactures a NOT_FOUND.
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::value::{get_str, intern_str, Value};

const NSHARDS: usize = 256; // power of two; swept 16/64/256, 256 measured best

struct BindIdx {
    shards: Vec<Mutex<HashMap<String, String>>>,
}

fn idx() -> &'static BindIdx {
    static I: OnceLock<BindIdx> = OnceLock::new();
    I.get_or_init(|| BindIdx {
        shards: (0..NSHARDS).map(|_| Mutex::new(HashMap::new())).collect(),
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

/// `bindidx_put(name: Text, value: Text) -> Unit` — last-writer-wins insert of a
/// name's current value (a content hash for a bind, "TOMBSTONED" for a delete). Locks
/// one shard; O(1); no fsync, no cross-shard coordination.
#[track_caller]
pub fn bindidx_put(args: Value) -> Value {
    let (name, value) = match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("bindidx_put: expected Tuple(Text, Text), got {:?}", other),
    };
    let name = match name {
        Value::Str(h) => get_str(h),
        other => panic!("bindidx_put: arg 0 expected Text name, got {:?}", other),
    };
    let value = match value {
        Value::Str(h) => get_str(h),
        other => panic!("bindidx_put: arg 1 expected Text value, got {:?}", other),
    };
    let s = shard(&name);
    idx().shards[s]
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(name, value);
    Value::Unit
}

/// `bindidx_get(name: Text) -> Text` — the name's current value, or "" if the name is
/// not in the index (a miss — the caller falls through to the durable path).
#[track_caller]
pub fn bindidx_get(arg: Value) -> Value {
    let name = match arg {
        Value::Str(h) => get_str(h),
        other => panic!("bindidx_get: expected Text name, got {:?}", other),
    };
    let s = shard(&name);
    let v = idx().shards[s]
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&name)
        .cloned()
        .unwrap_or_default();
    Value::Str(intern_str(&v))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Value {
        Value::Str(intern_str(v))
    }
    fn got(v: Value) -> String {
        match v {
            Value::Str(h) => get_str(h),
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn put_get_lww_and_miss() {
        // miss -> "" (caller falls to durable path)
        assert_eq!(got(bindidx_get(s("bindidx_test:absent"))), "");
        // bind, then read
        bindidx_put(Value::Tuple(vec![s("bindidx_test:k1"), s("sha256:aaa")]));
        assert_eq!(got(bindidx_get(s("bindidx_test:k1"))), "sha256:aaa");
        // last-writer-wins (UPDATE rebinds)
        bindidx_put(Value::Tuple(vec![s("bindidx_test:k1"), s("sha256:bbb")]));
        assert_eq!(got(bindidx_get(s("bindidx_test:k1"))), "sha256:bbb");
        // tombstone (DELETE)
        bindidx_put(Value::Tuple(vec![s("bindidx_test:k1"), s("TOMBSTONED")]));
        assert_eq!(got(bindidx_get(s("bindidx_test:k1"))), "TOMBSTONED");
    }

    #[test]
    fn concurrent_writers_distinct_keys() {
        // sanity: many threads binding distinct keys don't lose writes (sharded, no
        // single-lock serialization). Correctness check, not the perf trial.
        let n = 16usize;
        let per = 2000usize;
        let hs: Vec<_> = (0..n)
            .map(|t| {
                std::thread::spawn(move || {
                    for i in 0..per {
                        bindidx_put(Value::Tuple(vec![
                            s(&format!("bindidx_conc:t{}:{}", t, i)),
                            s(&format!("sha256:{}_{}", t, i)),
                        ]));
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        for t in 0..n {
            for i in (0..per).step_by(500) {
                assert_eq!(
                    got(bindidx_get(s(&format!("bindidx_conc:t{}:{}", t, i)))),
                    format!("sha256:{}_{}", t, i)
                );
            }
        }
    }
}

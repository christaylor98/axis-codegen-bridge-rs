//! BRIDGE_WAL_SHARD_V1 (AXVERITY_ACCEPTLOOP_SHARD_DISPATCH) — a THREAD-LOCAL
//! "which WAL shard is this thread writing to" cell, plus a process-wide read
//! fan-out count. This is the write-path piece that lets the shared-listener
//! pg_server (N `--entries` worker threads, each accepting on one shared
//! listener) route each connection's WAL writes to that worker's OWN shard —
//! WITHOUT threading a shard id through the frozen pg → wal_push → wal_put call
//! chain.
//!
//! ## Thread-local, shared-nothing (NO_SHARED_REGISTRY_STILL_HOLDS)
//!
//! `SHARD` lives in thread-local storage exactly like `logbuf.rs`'s `LOGS` and
//! `walindex.rs`'s `IDX`: reachable only by the thread that set it, no lock, no
//! process-global registry on the hot append path. A worker thread calls
//! `wal_shard_set("<s>")` ONCE at startup; every subsequent `wal_put` on that
//! thread reads its shard via `wal_shard_get()`. Two worker threads share
//! nothing here — the Landing-1 append-path invariant is preserved.
//!
//! Default is `"0"`: a thread that never calls `wal_shard_set` (every CLI
//! process — `push`, `record-put`, the daemon's own tools) writes shard `"0"`,
//! byte-identical to the pre-dispatch behavior. This keeps the change additive
//! and backward-compatible.
//!
//! ## Read fan-out count
//!
//! `wal_shard_count()` reports how many shards the READ path must fan out over
//! (writes land on shards `0..N-1`, but the WAL index has no shard field — each
//! shard is an independent prefix namespace — so a reader must check each). It
//! reads `AXVERITY_WAL_SHARDS` (env), default `1` — so with the env unset a
//! reader checks only shard `"0"`, again byte-identical to pre-dispatch reads.
//! The launcher of a multi-shard pg_server sets it to N.
//!
//! Identities are sha256(name_utf8) — same convention as the rest of the bridge.

use std::cell::RefCell;

use super::value::{get_str, intern_str, Value};

thread_local! {
    /// This thread's WAL shard id (as the string used in segment paths, e.g.
    /// `.axverity/wal/<shard>-<seq>.log`). Default `"0"`. Thread-local: never
    /// shared, never locked — the append path stays shared-nothing.
    static SHARD: RefCell<String> = RefCell::new(String::from("0"));
}

/// `wal_shard_set(shard: Text) -> Unit` — bind THIS thread's WAL shard. Called
/// once per worker thread at startup, before its accept loop.
#[track_caller]
pub fn wal_shard_set(v: Value) -> Value {
    let s = match v {
        Value::Str(h) => get_str(h),
        other => panic!("wal_shard_set: expected Text shard, got {:?}", other),
    };
    SHARD.with(|c| *c.borrow_mut() = s);
    Value::Unit
}

/// `wal_shard_get(Unit) -> Text` — this thread's WAL shard (default `"0"`).
/// Read on the hot write path by `wal_put`; a thread-local read, no lock.
#[track_caller]
pub fn wal_shard_get(_: Value) -> Value {
    SHARD.with(|c| Value::Str(intern_str(&c.borrow())))
}

/// `wal_shard_count(Unit) -> Int` — number of shards the READ path fans out
/// over. `AXVERITY_WAL_SHARDS` env, default `1` (unset ⇒ shard "0" only,
/// backward-compatible). Clamped to `>= 1`.
#[track_caller]
pub fn wal_shard_count(_: Value) -> Value {
    let n = std::env::var("AXVERITY_WAL_SHARDS")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(1);
    Value::Int(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shard_defaults_to_zero_then_settable() {
        // A fresh thread sees the default.
        assert_eq!(wal_shard_get(Value::Unit), Value::Str(intern_str("0")));
        wal_shard_set(Value::Str(intern_str("3")));
        assert_eq!(wal_shard_get(Value::Unit), Value::Str(intern_str("3")));
    }

    #[test]
    fn count_defaults_to_one() {
        // Env not set in this unit context ⇒ 1 (backward-compatible).
        std::env::remove_var("AXVERITY_WAL_SHARDS");
        assert_eq!(wal_shard_count(Value::Unit), Value::Int(1));
    }
}

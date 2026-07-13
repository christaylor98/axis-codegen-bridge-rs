//! BRIDGE_PKINDEX_V1 (AXVERITY_UNIFIED_DURABLE_STREAMS_V1, phase 2) — the
//! rebuildable `(table,pk) -> current-hash` projection over the payload WAL
//! stream. The SECOND consumer of the ONE shared frame-scanner
//! (`walindex::walk_frames`); it never re-parses the frame layout itself, so it
//! can never drift from the content-hash index (hard-limit
//! FRAME_PARSERS_UPDATED_LOCKSTEP).
//!
//! ## What it is, and what it is NOT
//!
//! The binding for a row is carried IN the object's own frame envelope
//! (`H|P|V|env|payload`, env = `"<table>\t<seq>\t<pk>"`, un-hashed — object and
//! binding are ONE durable frame, one torn-tail unit). This projection is the
//! read-side index over that: replay the framed WAL forward, and for every valid
//! frame carrying a non-empty envelope, set `map["<table>:<pk>"] = "sha256:<H>"`.
//! Because the replay visits frames in append order, a later frame for the same
//! `(table,pk)` overwrites the earlier one — **last-append-wins by frame order**,
//! never by comparing the envelope's seq/timestamp (hard-limit
//! CURRENTNESS_BY_APPEND_ORDER_ONLY; seq is informational only).
//!
//! It is REBUILDABLE-ONLY and NEVER written on the durability path (hard-limit
//! PK_INDEX_REBUILDABLE_NEVER_FSYNCED): there is deliberately NO `pkidx_snapshot`
//! and NO fsync anywhere in this module. `pkidx_rebuild` is a pure forward replay
//! from offset 0 — O(WAL), off the hot INSERT path. A future non-fsynced /
//! memory-only checkpoint could accelerate it without touching the invariant; not
//! built here.
//!
//! ## Storage model — THREAD-OWNED shards, NO shared registry
//!
//! Identical to `walindex.rs`: each open shard lives in thread-local storage
//! (`PKIDX`), reachable only by its opener, keyed by an opaque handle; the handle
//! counter (`NEXT`) is thread-local, first `pkidx_open` returns 1. Nothing is
//! shared, nothing is locked. Identities are `sha256(name_utf8)`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use super::value::{get_str, intern_str, Value};
use super::walindex::walk_frames;

/// One open pk-index shard: name -> current content address ("sha256:<hex>").
struct PkShard {
    #[allow(dead_code)]
    shard: String,
    map: HashMap<String, String>,
}

thread_local! {
    /// Per-thread pk-index shard table, keyed by integer handle. THREAD-LOCAL,
    /// never shared — lookup/rebuild touch only the calling thread's own map.
    static PKIDX: RefCell<HashMap<i64, PkShard>> = RefCell::new(HashMap::new());
    /// Per-thread handle counter (first `pkidx_open` returns 1, like walindex).
    static NEXT: Cell<i64> = const { Cell::new(1) };
}

fn next_handle() -> i64 {
    NEXT.with(|c| {
        let n = c.get();
        c.set(n + 1);
        n
    })
}

fn arg_int(v: &Value, who: &str, i: usize) -> i64 {
    match v {
        Value::Int(n) => *n,
        other => panic!("{}: arg {} expected Int, got {:?}", who, i, other),
    }
}
fn arg_str(v: &Value, who: &str, i: usize) -> String {
    match v {
        Value::Str(h) => get_str(h),
        other => panic!("{}: arg {} expected Text, got {:?}", who, i, other),
    }
}

/// `pkidx_open(shard: Text) -> Int`
///
/// Register a fresh empty pk-index shard in THIS thread's table, return its handle.
#[track_caller]
pub fn pkidx_open(arg: Value) -> Value {
    let shard = match arg {
        Value::Str(h) => get_str(h),
        other => panic!("pkidx_open: expected Text shard, got {:?}", other),
    };
    let h = next_handle();
    PKIDX.with(|idx| {
        idx.borrow_mut().insert(h, PkShard { shard, map: HashMap::new() });
    });
    Value::Int(h)
}

/// `pkidx_has(h: Int, name: Text) -> Bool` — is `name` bound in the shard?
#[track_caller]
pub fn pkidx_has(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 2 => es,
        other => panic!("pkidx_has: expected Tuple(Int, Text), got {:?}", other),
    };
    let h = arg_int(&es[0], "pkidx_has", 0);
    let name = arg_str(&es[1], "pkidx_has", 1);
    PKIDX.with(|idx| {
        let idx = idx.borrow();
        let sh = idx.get(&h).unwrap_or_else(|| panic!("pkidx_has: unknown handle {}", h));
        Value::Bool(sh.map.contains_key(&name))
    })
}

/// `pkidx_get(h: Int, name: Text) -> Text`
///
/// Return the current content address `"sha256:<hex>"` bound to `name`, or `""` if
/// `name` has never been bound in this (replayed) shard.
#[track_caller]
pub fn pkidx_get(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 2 => es,
        other => panic!("pkidx_get: expected Tuple(Int, Text), got {:?}", other),
    };
    let h = arg_int(&es[0], "pkidx_get", 0);
    let name = arg_str(&es[1], "pkidx_get", 1);
    PKIDX.with(|idx| {
        let idx = idx.borrow();
        let sh = idx.get(&h).unwrap_or_else(|| panic!("pkidx_get: unknown handle {}", h));
        let out = match sh.map.get(&name) {
            Some(addr) => addr.clone(),
            None => String::new(),
        };
        Value::Str(intern_str(&out))
    })
}

/// Parse an envelope `"<table>\t<seq>\t<pk>"` into the binding key `"<table>:<pk>"`.
/// Returns `None` for an empty envelope (a plain blob — no binding) or a malformed
/// one (defensive: a valid frame always carries a well-formed env, but a
/// non-INSERT frame carries none). The seq field is ignored — currentness is
/// append order, never seq.
fn env_to_name(env: &[u8]) -> Option<String> {
    if env.is_empty() {
        return None;
    }
    let s = std::str::from_utf8(env).ok()?;
    let mut it = s.split('\t');
    let table = it.next()?;
    // Skip non-row envelopes that share the WAL stream: fact-lineage frames
    // ("FACT\t…", assert_fact) and contradicts edges ("CONTRADICTS\t…") are not
    // pk-bindings — indexing them would materialize spurious "FACT:<agent>" /
    // "CONTRADICTS:<b>" bindings (ASSERT_FACT_PROMOTE_TO_LIVE_BUILD_V1).
    if table == "FACT" || table == "CONTRADICTS" {
        return None;
    }
    let _seq = it.next()?;
    let pk = it.next()?;
    Some(format!("{}:{}", table, pk))
}

/// `pkidx_rebuild(h: Int, seg_prefix: Text) -> Int`
///
/// Reconstruct the pk-index shard `h` by a full forward replay of the framed WAL
/// `<seg_prefix><seq>.log` from offset 0 (NO snapshot — this projection is never
/// persisted, hard-limit PK_INDEX_REBUILDABLE_NEVER_FSYNCED). Every valid frame's
/// envelope binds `"<table>:<pk>" -> "sha256:<H>"`, last-append-wins by frame
/// order. Returns the number of frames scanned. NO fsync occurs.
#[track_caller]
pub fn pkidx_rebuild(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 2 => es,
        other => panic!("pkidx_rebuild: expected Tuple(Int, Text), got {:?}", other),
    };
    let h = arg_int(&es[0], "pkidx_rebuild", 0);
    let prefix = arg_str(&es[1], "pkidx_rebuild", 1);

    let scanned = PKIDX.with(|idx| {
        let mut idx = idx.borrow_mut();
        let sh = idx.get_mut(&h).unwrap_or_else(|| panic!("pkidx_rebuild: unknown handle {}", h));
        let map = &mut sh.map;
        // Full replay from (0,0); the shared scanner hash-checks every frame and
        // stops at the torn frontier, so the pk-index and the content-hash index
        // see EXACTLY the same set of committed frames (structural both-or-neither).
        let (_fr_seg, _fr_off, scanned) = walk_frames(&prefix, 0, 0, |_seg, _off, _len, env, _payload, hexh| {
            if let Some(name) = env_to_name(env) {
                map.insert(name, format!("sha256:{}", hexh)); // last-append-wins
            }
        });
        scanned
    });
    Value::Int(scanned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::walindex;
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn sha_hex(p: &[u8]) -> String {
        Sha256::digest(p).iter().map(|b| format!("{:02x}", b)).collect()
    }

    /// Build a Branch-A frame `H(64)|P(10)|V(10)|env(V)|payload(P)`. `env` empty
    /// ⇒ V=0 (a plain blob, no binding). The content hash covers PAYLOAD ONLY.
    fn frame(table: &str, seq: &str, pk: &str, payload: &[u8]) -> Vec<u8> {
        let env: Vec<u8> = if table.is_empty() && pk.is_empty() {
            Vec::new()
        } else {
            format!("{}\t{}\t{}", table, seq, pk).into_bytes()
        };
        let mut f = Vec::new();
        f.extend_from_slice(sha_hex(payload).as_bytes());
        f.extend_from_slice(format!("{:010}", payload.len()).as_bytes());
        f.extend_from_slice(format!("{:010}", env.len()).as_bytes());
        f.extend_from_slice(&env);
        f.extend_from_slice(payload);
        f
    }

    static SEG_CTR: AtomicU64 = AtomicU64::new(0);

    /// Write `bytes` as segment 0 under a unique temp prefix; return the prefix.
    fn write_seg(bytes: &[u8]) -> String {
        let n = SEG_CTR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("pkidx-test-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).unwrap();
        let prefix = format!("{}/seg-", dir.to_str().unwrap());
        std::fs::write(format!("{}0.log", prefix), bytes).unwrap();
        prefix
    }

    fn pk_get(h: i64, name: &str) -> String {
        match pkidx_get(Value::Tuple(vec![Value::Int(h), Value::Str(intern_str(name))])) {
            Value::Str(s) => get_str(s),
            _ => unreachable!(),
        }
    }
    fn wal_has(h: i64, hexh: &str) -> bool {
        match walindex::walidx_has(Value::Tuple(vec![Value::Int(h), Value::Str(intern_str(hexh))])) {
            Value::Bool(b) => b,
            _ => unreachable!(),
        }
    }

    fn rebuild_both(prefix: &str) -> (i64, i64) {
        let ph = match pkidx_open(Value::Str(intern_str("0"))) { Value::Int(n) => n, _ => unreachable!() };
        pkidx_rebuild(Value::Tuple(vec![Value::Int(ph), Value::Str(intern_str(prefix))]));
        let wh = match walindex::walidx_open(Value::Str(intern_str("0"))) { Value::Int(n) => n, _ => unreachable!() };
        walindex::walidx_rebuild(Value::Tuple(vec![
            Value::Int(wh),
            Value::Str(intern_str(prefix)),
            Value::Str(intern_str("/nonexistent-snap")),
        ]));
        (ph, wh)
    }

    #[test]
    fn last_append_wins_by_frame_order() {
        // Two binds of the same (users,42) in append order → the LATER hash wins,
        // regardless of the seq field (here the later frame has the SMALLER seq).
        let p1 = b"RECORD\tid=42\tname=old";
        let p2 = b"RECORD\tid=42\tname=new";
        let mut seg = frame("users", "999", "42", p1);
        seg.extend_from_slice(&frame("users", "001", "42", p2));
        let prefix = write_seg(&seg);
        let (ph, _wh) = rebuild_both(&prefix);
        assert_eq!(pk_get(ph, "users:42"), format!("sha256:{}", sha_hex(p2)));
    }

    #[test]
    fn torn_tail_drops_object_and_binding_together() {
        // A complete first frame + a second frame truncated by one byte. The torn
        // frame must appear in NEITHER index; the intact one in BOTH.
        let p1 = b"RECORD\tid=1\tname=alice";
        let p2 = b"RECORD\tid=2\tname=bob";
        let mut seg = frame("t", "10", "1", p1);
        let f2 = frame("t", "20", "2", p2);
        seg.extend_from_slice(&f2[..f2.len() - 1]); // truncate last byte of frame 2
        let prefix = write_seg(&seg);
        let (ph, wh) = rebuild_both(&prefix);
        // frame 1: present in both
        assert!(wal_has(wh, &sha_hex(p1)));
        assert_eq!(pk_get(ph, "t:1"), format!("sha256:{}", sha_hex(p1)));
        // frame 2 (torn): present in neither — the structural both-or-neither claim
        assert!(!wal_has(wh, &sha_hex(p2)));
        assert_eq!(pk_get(ph, "t:2"), "");
    }

    #[test]
    fn empty_envelope_blob_indexes_object_but_no_binding() {
        // A V=0 frame (CLI push / plain blob) → in walidx, but NO pk binding.
        let p = b"just some blob bytes";
        let prefix = write_seg(&frame("", "", "", p));
        let (ph, wh) = rebuild_both(&prefix);
        assert!(wal_has(wh, &sha_hex(p)));
        assert_eq!(pk_get(ph, ":"), ""); // no binding key materialized
    }

    #[test]
    fn identity_unchanged_envelope_does_not_enter_hash() {
        // Same payload, two DIFFERENT envelopes → identical content hash H (the
        // envelope is outside the hash input). Both frames validate on scan.
        let p = b"RECORD\tid=7\tv=x";
        let a = frame("ta", "1", "7", p);
        let b = frame("tb", "2", "7", p);
        assert_eq!(&a[..64], &b[..64]); // H is byte-identical
        assert_eq!(std::str::from_utf8(&a[..64]).unwrap(), sha_hex(p));
    }
}

//! BRIDGE_CONTRADICTS_V1 (ASSERT_FACT_PROMOTE_TO_LIVE_BUILD_V1, Phase 3) — the
//! native-mem `contradicts` adjacency projection over the payload WAL stream.
//!
//! A `contradicts(A,B)` is written as ONE WAL frame whose un-hashed envelope is
//! `"CONTRADICTS\t<A>\t<B>"` (object and edge are ONE torn-tail unit). This
//! projection replays the framed WAL and, for every such envelope, inserts BOTH
//! `A->B` and `B->A` (symmetric, both-or-neither — one frame projects both
//! directions, so a torn tail can never leave a one-directional contradiction, and
//! the read path stays a strictly one-directional depth-1 out-edge lookup; docs §D3).
//! The SECOND consumer of the shared `walindex::walk_frames` scanner, so it can
//! never drift from the content-hash index.
//!
//! ## PANIC-ON-COLD (AC-1, hard) — no silent-empty
//! Unlike `pkidx_get` (which returns `""` on a cold miss), a contradicts LOOKUP
//! against a shard that was NEVER rebuilt in this process PANICS. Rationale
//! (docs §D1 / routing note): this index gates an Irreversible promotion; a
//! fresh/cold process would see an empty map, read "no contradiction", and
//! silently promote a contradicted fact — the worst failure for the gate. Every
//! reader asserts `rebuilt == true` and panics otherwise. Fail closed, LOUDLY.
//! Consequence: the traversal (and the promote gate consuming it) must run inside
//! a warm daemon that rebuilt the shard at startup, never a cold fresh CLI process.
//!
//! ## THREAD-OWNED shards, NO shared registry (mirrors pkindex/walindex)
//! Each shard lives in thread-local storage keyed by an opaque handle; nothing is
//! shared, nothing is locked. Identities are `sha256(name_utf8)`.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use super::value::{get_str, intern_str, Value};
use super::walindex::walk_frames;

/// One open contradicts-adjacency shard: from-addr -> {to-addr}.
struct AdjShard {
    #[allow(dead_code)]
    shard: String,
    /// AC-1: false until contradicts_rebuild runs in THIS process. A lookup while
    /// false panics — never a silent cold/empty answer.
    rebuilt: bool,
    adj: HashMap<String, HashSet<String>>,
}

thread_local! {
    /// Per-thread adjacency table, keyed by integer handle. THREAD-LOCAL, never
    /// shared — lookup/rebuild touch only the calling thread's own map.
    static CIDX: RefCell<HashMap<i64, AdjShard>> = RefCell::new(HashMap::new());
    /// Per-thread handle counter (first contradicts_open returns 1, like walindex).
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

/// `contradicts_open(shard: Text) -> Int`
///
/// Register a fresh EMPTY, NOT-yet-rebuilt adjacency shard in THIS thread; return
/// its handle. A lookup before contradicts_rebuild panics (AC-1).
#[track_caller]
pub fn contradicts_open(arg: Value) -> Value {
    let shard = match arg {
        Value::Str(h) => get_str(h),
        other => panic!("contradicts_open: expected Text shard, got {:?}", other),
    };
    let h = next_handle();
    CIDX.with(|idx| {
        idx.borrow_mut()
            .insert(h, AdjShard { shard, rebuilt: false, adj: HashMap::new() });
    });
    Value::Int(h)
}

/// Parse an envelope `"CONTRADICTS\t<A>\t<B>"` into (A, B). Returns None for an
/// empty / non-CONTRADICTS / malformed envelope (a plain blob, a FACT frame, or a
/// pk-binding all correctly ignored).
fn env_pair(env: &[u8]) -> Option<(String, String)> {
    if env.is_empty() {
        return None;
    }
    let s = std::str::from_utf8(env).ok()?;
    let mut it = s.split('\t');
    if it.next()? != "CONTRADICTS" {
        return None;
    }
    let a = it.next()?;
    let b = it.next()?;
    Some((a.to_string(), b.to_string()))
}

/// `contradicts_rebuild(h: Int, seg_prefix: Text) -> Int`
///
/// Reconstruct shard `h` by a full forward replay of the framed WAL from offset 0
/// (NO snapshot — rebuildable-only). Every valid CONTRADICTS frame inserts BOTH
/// `A->B` and `B->A`. Sets `rebuilt = true`. Returns the number of frames scanned.
#[track_caller]
pub fn contradicts_rebuild(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 2 => es,
        other => panic!("contradicts_rebuild: expected Tuple(Int, Text), got {:?}", other),
    };
    let h = arg_int(&es[0], "contradicts_rebuild", 0);
    let prefix = arg_str(&es[1], "contradicts_rebuild", 1);

    let scanned = CIDX.with(|idx| {
        let mut idx = idx.borrow_mut();
        let sh = idx
            .get_mut(&h)
            .unwrap_or_else(|| panic!("contradicts_rebuild: unknown handle {}", h));
        let adj = &mut sh.adj;
        // The shared scanner hash-checks every frame and stops at the torn frontier,
        // so a torn CONTRADICTS frame is dropped whole — neither direction is inserted.
        let (_fr_seg, _fr_off, scanned) =
            walk_frames(&prefix, 0, 0, |_seg, _off, _len, env, _payload, _hexh| {
                if let Some((a, b)) = env_pair(env) {
                    adj.entry(a.clone()).or_default().insert(b.clone());
                    adj.entry(b).or_default().insert(a);
                }
            });
        sh.rebuilt = true;
        scanned
    });
    Value::Int(scanned)
}

/// AC-1 guard: a lookup against a never-rebuilt-this-process shard is fail-closed-loud.
fn assert_rebuilt(sh: &AdjShard, h: i64) {
    if !sh.rebuilt {
        panic!(
            "contradicts: shard {} looked up but NEVER REBUILT in this process — \
             refusing a cold/empty answer (would silently allow a contradicted \
             promotion). Call contradicts_rebuild first (run inside the warm daemon).",
            h
        );
    }
}

/// `contradicts_has(h: Int, from: Text, to: Text) -> Bool`
///
/// Is `from` recorded as contradicting `to`? PANICS if the shard was never rebuilt.
#[track_caller]
pub fn contradicts_has(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 3 => es,
        other => panic!("contradicts_has: expected Tuple(Int, Text, Text), got {:?}", other),
    };
    let h = arg_int(&es[0], "contradicts_has", 0);
    let from = arg_str(&es[1], "contradicts_has", 1);
    let to = arg_str(&es[2], "contradicts_has", 2);
    CIDX.with(|idx| {
        let idx = idx.borrow();
        let sh = idx
            .get(&h)
            .unwrap_or_else(|| panic!("contradicts_has: unknown handle {}", h));
        assert_rebuilt(sh, h);
        Value::Bool(sh.adj.get(&from).is_some_and(|s| s.contains(&to)))
    })
}

/// `contradicts_any(h: Int, from: Text) -> Bool`
///
/// Does `from` contradict ANY fact (a depth-1 out-edge existence check — the
/// bounded traversal the promote gate consumes)? PANICS if the shard was never
/// rebuilt.
#[track_caller]
pub fn contradicts_any(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 2 => es,
        other => panic!("contradicts_any: expected Tuple(Int, Text), got {:?}", other),
    };
    let h = arg_int(&es[0], "contradicts_any", 0);
    let from = arg_str(&es[1], "contradicts_any", 1);
    CIDX.with(|idx| {
        let idx = idx.borrow();
        let sh = idx
            .get(&h)
            .unwrap_or_else(|| panic!("contradicts_any: unknown handle {}", h));
        assert_rebuilt(sh, h);
        Value::Bool(sh.adj.get(&from).is_some_and(|s| !s.is_empty()))
    })
}

// ── Warm singleton (daemon fast-path) ────────────────────────────────────────
// A per-thread WARM adjacency, rebuilt ONCE at daemon-worker startup
// (`contradicts_warm`) then queried per request (`contradicts_any_warm`) with NO
// per-call rebuild — the warm host the routing note (§D1) calls for. Same
// fail-closed-loud AC-1 discipline: `contradicts_any_warm` PANICS if this thread
// never warmed.

thread_local! {
    static WARM: RefCell<Option<AdjShard>> = const { RefCell::new(None) };
}

/// `contradicts_warm(seg_prefix: Text) -> Int` — (re)build THIS thread's WARM
/// singleton by a full forward WAL replay; returns frames scanned. Call once at
/// daemon-worker startup, before serving requests.
#[track_caller]
pub fn contradicts_warm(arg: Value) -> Value {
    let prefix = match arg {
        Value::Str(h) => get_str(h),
        other => panic!("contradicts_warm: expected Text seg_prefix, got {:?}", other),
    };
    let mut adj: HashMap<String, HashSet<String>> = HashMap::new();
    let (_fs, _fo, scanned) =
        walk_frames(&prefix, 0, 0, |_seg, _off, _len, env, _payload, _hexh| {
            if let Some((a, b)) = env_pair(env) {
                adj.entry(a.clone()).or_default().insert(b.clone());
                adj.entry(b).or_default().insert(a);
            }
        });
    WARM.with(|w| {
        *w.borrow_mut() = Some(AdjShard { shard: "warm".to_string(), rebuilt: true, adj });
    });
    Value::Int(scanned)
}

/// `contradicts_any_warm(from: Text) -> Bool` — depth-1 out-edge existence check
/// against THIS thread's WARM singleton. PANICS if the thread never warmed (AC-1).
#[track_caller]
pub fn contradicts_any_warm(arg: Value) -> Value {
    let from = match arg {
        Value::Str(h) => get_str(h),
        other => panic!("contradicts_any_warm: expected Text from-addr, got {:?}", other),
    };
    WARM.with(|w| {
        let w = w.borrow();
        match w.as_ref() {
            None => panic!(
                "contradicts_any_warm: WARM singleton NEVER REBUILT in this process/thread \
                 — refusing a cold/empty answer (would silently allow a contradicted \
                 promotion). Call contradicts_warm at daemon-worker startup."
            ),
            Some(sh) => Value::Bool(sh.adj.get(&from).is_some_and(|s| !s.is_empty())),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn sha_hex(p: &[u8]) -> String {
        Sha256::digest(p).iter().map(|b| format!("{:02x}", b)).collect()
    }

    /// Build a Branch-A frame `H(64)|P(10)|V(10)|env(V)|payload(P)` with a
    /// CONTRADICTS envelope. H covers PAYLOAD ONLY (envelope is un-hashed).
    fn cframe(a: &str, b: &str, payload: &[u8]) -> Vec<u8> {
        let env = format!("CONTRADICTS\t{}\t{}", a, b).into_bytes();
        let mut f = Vec::new();
        f.extend_from_slice(sha_hex(payload).as_bytes());
        f.extend_from_slice(format!("{:010}", payload.len()).as_bytes());
        f.extend_from_slice(format!("{:010}", env.len()).as_bytes());
        f.extend_from_slice(&env);
        f.extend_from_slice(payload);
        f
    }

    static SEG_CTR: AtomicU64 = AtomicU64::new(0);
    fn write_seg(bytes: &[u8]) -> String {
        let n = SEG_CTR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("cidx-test-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).unwrap();
        let prefix = format!("{}/seg-", dir.to_str().unwrap());
        std::fs::write(format!("{}0.log", prefix), bytes).unwrap();
        prefix
    }

    fn open() -> i64 {
        match contradicts_open(Value::Str(intern_str("0"))) {
            Value::Int(n) => n,
            _ => unreachable!(),
        }
    }
    fn rebuild(h: i64, p: &str) {
        contradicts_rebuild(Value::Tuple(vec![Value::Int(h), Value::Str(intern_str(p))]));
    }
    fn has(h: i64, a: &str, b: &str) -> bool {
        match contradicts_has(Value::Tuple(vec![
            Value::Int(h),
            Value::Str(intern_str(a)),
            Value::Str(intern_str(b)),
        ])) {
            Value::Bool(x) => x,
            _ => unreachable!(),
        }
    }
    fn any(h: i64, a: &str) -> bool {
        match contradicts_any(Value::Tuple(vec![Value::Int(h), Value::Str(intern_str(a))])) {
            Value::Bool(x) => x,
            _ => unreachable!(),
        }
    }

    #[test]
    fn both_directions_from_one_frame() {
        // ONE frame ⇒ BOTH directions present (AC-3 both-or-neither, symmetric read).
        let seg = cframe("sha256:aaa", "sha256:bbb", b"payload-ab");
        let p = write_seg(&seg);
        let h = open();
        rebuild(h, &p);
        assert!(has(h, "sha256:aaa", "sha256:bbb"));
        assert!(has(h, "sha256:bbb", "sha256:aaa")); // symmetric from the single frame
        assert!(any(h, "sha256:aaa"));
        assert!(any(h, "sha256:bbb"));
        assert!(!any(h, "sha256:ccc"));
        assert!(!has(h, "sha256:aaa", "sha256:ccc"));
    }

    #[test]
    fn torn_tail_drops_both_directions_together() {
        // Complete first frame + truncated second ⇒ the torn edge is in NEITHER
        // direction (the both-or-neither guarantee under torn tail).
        let f1 = cframe("sha256:a1", "sha256:b1", b"x1");
        let f2 = cframe("sha256:a2", "sha256:b2", b"x2");
        let mut seg = f1.clone();
        seg.extend_from_slice(&f2[..f2.len() - 1]); // truncate last byte
        let p = write_seg(&seg);
        let h = open();
        rebuild(h, &p);
        assert!(has(h, "sha256:a1", "sha256:b1"));
        assert!(has(h, "sha256:b1", "sha256:a1"));
        assert!(!any(h, "sha256:a2")); // torn — neither direction
        assert!(!any(h, "sha256:b2"));
    }

    #[test]
    #[should_panic(expected = "NEVER REBUILT")]
    fn cold_any_panics_not_empty() {
        // AC-1: opened but NOT rebuilt ⇒ a lookup must PANIC, never return false.
        let h = open();
        let _ = any(h, "sha256:whatever");
    }

    #[test]
    #[should_panic(expected = "NEVER REBUILT")]
    fn cold_has_panics_not_empty() {
        let h = open();
        let _ = has(h, "sha256:a", "sha256:b");
    }

    #[test]
    fn warm_singleton_then_query() {
        // WARM is thread-local; the std test harness runs each test on its own
        // thread, so this thread's singleton starts None and is fresh.
        let seg = cframe("sha256:w1", "sha256:w2", b"wp");
        let p = write_seg(&seg);
        assert!(matches!(contradicts_warm(Value::Str(intern_str(&p))), Value::Int(_)));
        assert!(matches!(contradicts_any_warm(Value::Str(intern_str("sha256:w1"))), Value::Bool(true)));
        assert!(matches!(contradicts_any_warm(Value::Str(intern_str("sha256:w2"))), Value::Bool(true)));
        assert!(matches!(contradicts_any_warm(Value::Str(intern_str("sha256:none"))), Value::Bool(false)));
    }

    #[test]
    #[should_panic(expected = "NEVER REBUILT")]
    fn cold_warm_query_panics() {
        // Fresh thread ⇒ WARM is None ⇒ a query must PANIC, never return false (AC-1).
        let _ = contradicts_any_warm(Value::Str(intern_str("sha256:x")));
    }
}

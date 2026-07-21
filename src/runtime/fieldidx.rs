//! BRIDGE_FIELDIDX_V1 (AXVERITY_INSERT_PATH_FASTPATH) — the SQL-facing field
//! index (turn:0015's `(field,value) -> {hash}` posting-list reverse index)
//! realized with the SAME shape as `walindex.rs` (intent:axverity-req-wal-
//! index-disposable-r2): a thread-owned, in-memory shard used by the READ
//! side (`field_lookup`) to bound its WAL-replay cost, plus a disposable
//! batched watermarked snapshot as the checkpoint tier. This is an
//! ADAPTATION of that pinned pattern, not a new design — the two differences
//! from `walindex.rs` are forced by field-index's own shape (not invented
//! here): (1) the map is keyed by `(field, value)`, not by content hash, and
//! (2) each key maps to a POSTING LIST (multiple hashes), not a single
//! `(seg,off,len)` pointer.
//!
//! ## Write side
//!
//! `field_index_add` (M1, `lib/field_index_add.m1`) no longer does the
//! turn:0015 legacy per-call `fs_mkdir_p` + `fs_append_text` (two small-file
//! syscall groups, structurally the one-file-per-key pattern the whole
//! WAL/pack redesign exists to eliminate). It now does ONE call into the
//! already-proven `wal_put` primitive (one framed, fsync'd append — the same
//! ~2.3ms-class op the WAL write path already pays), framing a
//! `"FIELDIDX\t<field>\t<value>\t<hash>"` declaration line. This module never
//! sees that write directly — it only ever reads it back via WAL replay,
//! exactly as `walidx_rebuild` reads WAL-framed object bytes back.
//!
//! Every `pg_exec_insert` INSERT ALSO writes a `"RECORD\t<col>=<val>\t..."`
//! frame via `wal_push` before any field is declared (turn:0017's RECORD
//! format, byte-identical). So this module's rebuild recognizes BOTH frame
//! shapes: an implicit declaration carried for free inside a RECORD frame
//! (the common pg INSERT case — no separate FIELDIDX frame is ever written
//! for it), and an explicit standalone `FIELDIDX` frame (the
//! `axverity-field_index` CLI primitive, decoupled from any real object —
//! turn:0015's `field_index_add` was designed to accept an arbitrary hash
//! independent of what any object's content, so a synthetic/undeclared hash
//! must still resolve; only a durable frame of its own can carry that).
//!
//! ## No shared registry (NO_SHARED_REGISTRY)
//!
//! Thread-local only, same storage model as `logbuf.rs`/`walindex.rs`: no
//! `Mutex`/`RwLock`/`Arc`/process-global registry anywhere on this path.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::OnceLock;

use super::value::{get_str, intern_str, Value};

struct FieldShard {
    // "field\tvalue" -> ordered, de-duplicated posting list of hashes
    map: HashMap<String, Vec<String>>,
    fseg: i64,
    foff: i64,
}

fn bump_frontier(sh: &mut FieldShard, seg: i64, end: i64) {
    if seg > sh.fseg || (seg == sh.fseg && end > sh.foff) {
        sh.fseg = seg;
        sh.foff = end;
    }
}

fn push_dedup(list: &mut Vec<String>, hash: String) {
    if !list.iter().any(|h| h == &hash) {
        list.push(hash);
    }
}

thread_local! {
    static FIDX: RefCell<HashMap<i64, FieldShard>> = RefCell::new(HashMap::new());
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

/// `fieldidx_open(shard: Text) -> Int`
#[track_caller]
pub fn fieldidx_open(arg: Value) -> Value {
    let _shard = match arg {
        Value::Str(h) => get_str(h),
        other => panic!("fieldidx_open: expected Text shard, got {:?}", other),
    };
    let h = next_handle();
    FIDX.with(|idx| {
        idx.borrow_mut()
            .insert(h, FieldShard { map: HashMap::new(), fseg: 0, foff: 0 });
    });
    Value::Int(h)
}

/// `fieldidx_insert(h: Int, field: Text, value: Text, hash: Text) -> Unit`
/// Synchronous in-memory insert (append, de-duplicated) — no syscall, no lock.
#[track_caller]
pub fn fieldidx_insert(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 4 => es,
        other => panic!("fieldidx_insert: expected Tuple(Int, Text, Text, Text), got {:?}", other),
    };
    let h = arg_int(&es[0], "fieldidx_insert", 0);
    let field = arg_str(&es[1], "fieldidx_insert", 1);
    let value = arg_str(&es[2], "fieldidx_insert", 2);
    let hash = arg_str(&es[3], "fieldidx_insert", 3);
    FIDX.with(|idx| {
        let mut idx = idx.borrow_mut();
        let sh = idx
            .get_mut(&h)
            .unwrap_or_else(|| panic!("fieldidx_insert: unknown handle {}", h));
        let key = format!("{}\t{}", field, value);
        push_dedup(sh.map.entry(key).or_default(), hash);
    });
    Value::Unit
}

/// `fieldidx_get(h: Int, field: Text, value: Text) -> Text`
/// Returns the posting list as LF-joined hashes (append order), or "" if the
/// key was never seen by this shard.
#[track_caller]
pub fn fieldidx_get(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 3 => es,
        other => panic!("fieldidx_get: expected Tuple(Int, Text, Text), got {:?}", other),
    };
    let h = arg_int(&es[0], "fieldidx_get", 0);
    let field = arg_str(&es[1], "fieldidx_get", 1);
    let value = arg_str(&es[2], "fieldidx_get", 2);
    FIDX.with(|idx| {
        let idx = idx.borrow();
        let sh = idx
            .get(&h)
            .unwrap_or_else(|| panic!("fieldidx_get: unknown handle {}", h));
        let key = format!("{}\t{}", field, value);
        let out = match sh.map.get(&key) {
            Some(list) => list.join("\n"),
            None => String::new(),
        };
        Value::Str(intern_str(&out))
    })
}

fn write_durable(path: &str, bytes: &[u8]) {
    let p = Path::new(path);
    let parent = p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(
        ".{}.tmp",
        p.file_name().and_then(|s| s.to_str()).unwrap_or("snap")
    ));
    {
        let mut f = File::create(&tmp)
            .unwrap_or_else(|e| panic!("fieldidx_snapshot: create {:?}: {}", tmp, e));
        f.write_all(bytes).unwrap_or_else(|e| panic!("fieldidx_snapshot: write: {}", e));
        f.sync_all().unwrap_or_else(|e| panic!("fieldidx_snapshot: fsync: {}", e));
    }
    std::fs::rename(&tmp, p).unwrap_or_else(|e| panic!("fieldidx_snapshot: rename: {}", e));
    if let Ok(dir) = File::open(parent) {
        let _ = dir.sync_all();
    }
}

/// `fieldidx_snapshot(h: Int, path: Text) -> Unit`
/// ONE batched file per shard (never per key):
///   `FIELDIDX1\t<wm_seg>\t<wm_off>\t<count>\n` then `count` lines
///   `<field>\t<value>\t<hash>\n` (one line per POSTING ENTRY).
#[track_caller]
pub fn fieldidx_snapshot(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 2 => es,
        other => panic!("fieldidx_snapshot: expected Tuple(Int, Text), got {:?}", other),
    };
    let h = arg_int(&es[0], "fieldidx_snapshot", 0);
    let path = arg_str(&es[1], "fieldidx_snapshot", 1);
    let body = FIDX.with(|idx| {
        let idx = idx.borrow();
        let sh = idx
            .get(&h)
            .unwrap_or_else(|| panic!("fieldidx_snapshot: unknown handle {}", h));
        let count: usize = sh.map.values().map(|v| v.len()).sum();
        let mut s = format!("FIELDIDX1\t{}\t{}\t{}\n", sh.fseg, sh.foff, count);
        for (key, hashes) in &sh.map {
            for hash in hashes {
                s.push_str(&format!("{}\t{}\n", key, hash));
            }
        }
        s
    });
    write_durable(&path, body.as_bytes());
    Value::Unit
}

/// Load a batched snapshot, returning the watermark `(seg, off)`. Non-
/// authoritative: a missing/malformed/truncated snapshot yields `(0, 0)`
/// (full replay) and stages nothing — same discipline as `walindex.rs`.
fn load_snapshot(map: &mut HashMap<String, Vec<String>>, path: &str) -> (i64, i64) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return (0, 0),
    };
    let mut lines = content.lines();
    let hdr = match lines.next() {
        Some(h) => h,
        None => return (0, 0),
    };
    let hp: Vec<&str> = hdr.split('\t').collect();
    if hp.len() != 4 || hp[0] != "FIELDIDX1" {
        return (0, 0);
    }
    let wm_seg: i64 = hp[1].parse().unwrap_or(-1);
    let wm_off: i64 = hp[2].parse().unwrap_or(-1);
    let count: usize = hp[3].parse().unwrap_or(usize::MAX);
    if wm_seg < 0 || wm_off < 0 || count == usize::MAX {
        return (0, 0);
    }
    let mut staged: Vec<(String, String)> = Vec::new();
    for line in lines {
        let p: Vec<&str> = line.rsplitn(2, '\t').collect(); // [hash, "field\tvalue"]
        if p.len() != 2 {
            return (0, 0);
        }
        staged.push((p[1].to_string(), p[0].to_string()));
    }
    if staged.len() != count {
        return (0, 0);
    }
    for (key, hash) in staged {
        push_dedup(map.entry(key).or_default(), hash);
    }
    (wm_seg, wm_off)
}

// (read_segment + sha256_hex removed — the frame walk now lives in the ONE shared
//  scanner walindex::walk_frames, which owns segment reads and hash-checking.)

/// Extract `(field, value)` cells from a decoded RECORD/FIELDIDX payload and
/// insert them (keyed to `hash`) into `map`. `RECORD\tc1=v1\tc2=v2\t...` cells
/// are all indexed against the frame's own hash; a `FIELDIDX\tfield\tvalue\thash`
/// line declares exactly one (field,value)->hash triple explicitly, decoupled
/// from the frame's own hash (turn:0015 tier-agnostic declaration semantics).
fn index_payload(map: &mut HashMap<String, Vec<String>>, text: &str, own_hash: &str) {
    let mut cells = text.split('\t');
    match cells.next() {
        Some("RECORD") => {
            // own_hash is the frame's bare 64-hex key (wal_frame_bytes strips the
            // "sha256:" prefix before framing); every OTHER posting entry in this
            // index (FIELDIDX declarations, and every hash string field_lookup's
            // callers compare against) is the full "sha256:<hex>" address, so this
            // must match that shape or it silently fails to de-dup / never matches
            // what a caller looks for.
            let addr = format!("sha256:{}", own_hash);
            for cell in cells {
                if let Some((field, value)) = cell.split_once('=') {
                    push_dedup(
                        map.entry(format!("{}\t{}", field, value)).or_default(),
                        addr.clone(),
                    );
                }
            }
        }
        Some("FIELDIDX") => {
            let rest: Vec<&str> = cells.collect();
            if rest.len() == 3 {
                let (field, value, hash) = (rest[0], rest[1], rest[2]);
                push_dedup(map.entry(format!("{}\t{}", field, value)).or_default(), hash.to_string());
            }
        }
        _ => {}
    }
}

/// On-disk byte length of the framed WAL segment `<prefix><seq>.log`, or `None`
/// if it does not exist. Used by `do_replay`'s cheap stat-guard to decide whether
/// a frontier segment has grown at all before paying a whole-file read.
fn segment_len(prefix: &str, seq: i64) -> Option<u64> {
    std::fs::metadata(format!("{}{}.log", prefix, seq))
        .ok()
        .map(|m| m.len())
}

/// Full (re)build of shard `sh`: load the disposable snapshot (if valid), then
/// forward-replay the framed WAL from the SNAPSHOT watermark via the ONE shared
/// scanner `walindex::walk_frames`. This is the cold-start / fresh-handle cost.
fn do_rebuild(sh: &mut FieldShard, prefix: &str, snap_path: &str) -> i64 {
    let (wm_seg, wm_off) = load_snapshot(&mut sh.map, snap_path);
    bump_frontier(sh, wm_seg, wm_off);
    let map = &mut sh.map;
    let (fr_seg, fr_off, scanned) =
        super::walindex::walk_frames(prefix, wm_seg, wm_off, |_seg, _off, _len, _env, payload, hexh| {
            if let Ok(text) = std::str::from_utf8(payload) {
                index_payload(map, text, hexh);
            }
        });
    bump_frontier(sh, fr_seg, fr_off);
    scanned
}

/// GENUINELY-INCREMENTAL replay of shard `sh`: forward-replay the framed WAL from
/// the HANDLE'S OWN frontier (`sh.fseg/sh.foff`), NOT the snapshot watermark. This
/// is the whole point of the residency build (design:axverity-field-index-
/// residency-spike-results found `fieldidx_rebuild` always re-walks from the
/// snapshot, ignoring the caller's position). Returns frames scanned SINCE the
/// frontier — a coherent refresh costs O(delta), not O(whole store).
///
/// Cheap stat-guard: if the frontier segment has not grown past our offset AND no
/// later segment exists, there is nothing to replay — skip the whole-file read
/// entirely (`read_segment` reads the ENTIRE segment). This is what makes the M
/// within-query replays of a JOIN O(1) each (one/two `stat`s) in the common
/// zero-delta case, instead of O(M × segment-size) I/O that would erase the win.
fn do_replay(sh: &mut FieldShard, prefix: &str) -> i64 {
    let (from_seg, from_off) = (sh.fseg, sh.foff);
    let has_next = segment_len(prefix, from_seg + 1).is_some();
    match segment_len(prefix, from_seg) {
        Some(len) if (len as i64) <= from_off && !has_next => return 0, // no delta
        None if !has_next => return 0, // frontier segment gone, no successor
        _ => {}
    }
    let map = &mut sh.map;
    let (fr_seg, fr_off, scanned) =
        super::walindex::walk_frames(prefix, from_seg, from_off, |_seg, _off, _len, _env, payload, hexh| {
            if let Ok(text) = std::str::from_utf8(payload) {
                index_payload(map, text, hexh);
            }
        });
    bump_frontier(sh, fr_seg, fr_off);
    scanned
}

/// `fieldidx_rebuild(h: Int, seg_prefix: Text, snap_path: Text) -> Int`
/// Load the disposable snapshot, then forward-replay the framed WAL via the ONE
/// shared scanner `walindex::walk_frames` (AXVERITY_UNIFIED_DURABLE_STREAMS_V1
/// phase 2 — new 84-byte H|P|V|env|payload layout), indexing RECORD/FIELDIDX
/// payloads. The field index ignores the (table,seq,pk) envelope; it indexes the
/// payload text exactly as before. Sharing the scanner keeps this parser in
/// lockstep with the content-hash and pk indexes (hard-limit
/// FRAME_PARSERS_UPDATED_LOCKSTEP). Returns frames scanned (diagnostics).
#[track_caller]
pub fn fieldidx_rebuild(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 3 => es,
        other => panic!("fieldidx_rebuild: expected Tuple(Int, Text, Text), got {:?}", other),
    };
    let h = arg_int(&es[0], "fieldidx_rebuild", 0);
    let prefix = arg_str(&es[1], "fieldidx_rebuild", 1);
    let snap_path = arg_str(&es[2], "fieldidx_rebuild", 2);

    let scanned = FIDX.with(|idx| {
        let mut idx = idx.borrow_mut();
        let sh = idx
            .get_mut(&h)
            .unwrap_or_else(|| panic!("fieldidx_rebuild: unknown handle {}", h));
        do_rebuild(sh, &prefix, &snap_path)
    });
    Value::Int(scanned)
}

/// `fieldidx_replay(h: Int, seg_prefix: Text) -> Int`
///
/// AXVERITY_FIELDINDEX_RESIDENCY_BUILD_V1 — the genuinely-incremental primitive.
/// Replay the framed WAL into shard `h` starting from the HANDLE'S OWN frontier
/// (`sh.fseg/sh.foff`), returning the number of frames walked (the delta since the
/// last rebuild/replay of THIS handle). Unlike `fieldidx_rebuild`, it does NOT
/// reload the snapshot and does NOT restart from the snapshot watermark. The
/// returned count is the direct incrementality instrument (hard-limit
/// VERIFY_GENUINE_INCREMENTALITY): after appending K frames it returns K, not
/// base+K; with no new frames it returns 0.
#[track_caller]
pub fn fieldidx_replay(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 2 => es,
        other => panic!("fieldidx_replay: expected Tuple(Int, Text), got {:?}", other),
    };
    let h = arg_int(&es[0], "fieldidx_replay", 0);
    let prefix = arg_str(&es[1], "fieldidx_replay", 1);
    let scanned = FIDX.with(|idx| {
        let mut idx = idx.borrow_mut();
        let sh = idx
            .get_mut(&h)
            .unwrap_or_else(|| panic!("fieldidx_replay: unknown handle {}", h));
        do_replay(sh, &prefix)
    });
    Value::Int(scanned)
}

// ── Per-worker RESIDENT field-index handles (AXVERITY_FIELDINDEX_RESIDENCY_BUILD_V1)
//
// A thread-local map `shard -> resident handle` so a long-lived pg_server worker
// reuses ONE field-index shard across many `field_lookup` calls instead of the
// fresh-open+full-rebuild-per-call fallback. Thread-LOCAL only — same
// nothing-shared/nothing-locked model as `walindex`/`logbuf`/`cursor`; a
// server-lifetime handle is necessarily PER-WORKER (a shared one would need a
// lock, reintroducing the contention the design avoids — spike finding 3).
//
// Coherency is by REPLAY-BEFORE-READ: `fieldidx_res_get` replays each resident
// handle from its own frontier before every lookup, so the handle is always
// current at read time regardless of scope. The three scopes (per-query /
// per-connection / server-lifetime) therefore differ ONLY in WHEN the handle is
// dropped (amortization of the one cold rebuild), never in correctness — the
// scope-boundary reset is `fieldidx_res_scope`.

thread_local! {
    static RESIDENT: RefCell<HashMap<String, i64>> = RefCell::new(HashMap::new());
}

/// The residency scope, read once per process from `AXVERITY_FIELDIDX_RESIDENCY`:
///   unset / `off` / `0` / `false` → `"off"`    (fresh-handle-per-call fallback,
///                                                byte-identical to pre-build)
///   `query`                        → `"query"`  (resident within one query;
///                                                dropped at each query boundary)
///   `conn` / `connection`          → `"conn"`   (resident within one connection)
///   `server` / `1` / `on` / `true` → `"server"` (resident for the worker's life)
/// Default OFF — the flag is only flipped to default-on after Chris reviews the
/// measured results (the qhm / GROUP-BY-cursor ship discipline).
fn residency_mode() -> &'static str {
    static MODE: OnceLock<&'static str> = OnceLock::new();
    MODE.get_or_init(|| {
        match std::env::var("AXVERITY_FIELDIDX_RESIDENCY")
            .ok()
            .as_deref()
            .map(|s| s.to_ascii_lowercase())
            .as_deref()
        {
            Some("query") => "query",
            Some("conn") | Some("connection") => "conn",
            Some("server") | Some("1") | Some("on") | Some("true") => "server",
            // AXVERITY_READPATH_FINAL_CLOSEOUT_V1 Item 3 — default FLIPPED to "query"
            // (built, single-client-measured, correctness-verified, AND concurrency-
            // confirmed: fix:axverity-fieldindex-residency-concurrency-confirmed; the
            // data showed query≈server for the JOIN win with perfect coherency and
            // bounded memory, so per-query scope is the recommended setting). Explicit
            // off/0/false remain reachable for the byte-identical fresh-handle fallback.
            Some("off") | Some("0") | Some("false") => "off",
            _ => "query",
        }
    })
}

/// `fieldidx_residency_mode(_: Unit) -> Text` — the flag, for M1 dispatch in
/// `fieldidx_lookup_step`. `off` selects the preserved fresh-handle-per-call path.
#[track_caller]
pub fn fieldidx_residency_mode(_arg: Value) -> Value {
    Value::Str(intern_str(residency_mode()))
}

/// `fieldidx_res_get(shard: Text, seg_prefix: Text, snap_path: Text, field: Text,
///                   value: Text) -> Text`
///
/// The resident lookup used by `field_lookup`'s per-shard fan-out when residency
/// is enabled. Get-or-create the resident handle for `shard`; bring it current
/// (first touch → full `do_rebuild`; thereafter → incremental `do_replay` from its
/// own frontier); return the (field,value) posting list LF-JOINED — byte-identical
/// to what `fieldidx_get` returns on a freshly-rebuilt handle (`"" if absent`).
#[track_caller]
pub fn fieldidx_res_get(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 5 => es,
        other => panic!(
            "fieldidx_res_get: expected Tuple(shard, prefix, snap, field, value), got {:?}",
            other
        ),
    };
    let shard = arg_str(&es[0], "fieldidx_res_get", 0);
    let prefix = arg_str(&es[1], "fieldidx_res_get", 1);
    let snap = arg_str(&es[2], "fieldidx_res_get", 2);
    let field = arg_str(&es[3], "fieldidx_res_get", 3);
    let value = arg_str(&es[4], "fieldidx_res_get", 4);

    let existing = RESIDENT.with(|r| r.borrow().get(&shard).copied());
    let h = match existing {
        Some(h) => h,
        None => {
            let h = next_handle();
            FIDX.with(|idx| {
                idx.borrow_mut()
                    .insert(h, FieldShard { map: HashMap::new(), fseg: 0, foff: 0 });
            });
            RESIDENT.with(|r| {
                r.borrow_mut().insert(shard.clone(), h);
            });
            h
        }
    };

    FIDX.with(|idx| {
        let mut idx = idx.borrow_mut();
        let sh = idx
            .get_mut(&h)
            .unwrap_or_else(|| panic!("fieldidx_res_get: resident handle {} vanished", h));
        if existing.is_none() {
            do_rebuild(sh, &prefix, &snap);
        } else {
            do_replay(sh, &prefix);
        }
        let key = format!("{}\t{}", field, value);
        let out = match sh.map.get(&key) {
            Some(list) => list.join("\n"),
            None => String::new(),
        };
        Value::Str(intern_str(&out))
    })
}

/// `fieldidx_res_scope(which: Text) -> Unit`
///
/// Scope-boundary reset: drop ALL resident handles (and free the underlying FIDX
/// shards, so a long-lived worker does not leak) IFF the active residency mode
/// equals `which`. Called by the server loop with `"query"` at each query start
/// and `"conn"` at each connection start. Under `server` mode neither boundary
/// matches, so handles persist for the worker's life; under `off` nothing is
/// resident so it is always a no-op.
#[track_caller]
pub fn fieldidx_res_scope(arg: Value) -> Value {
    let which = match arg {
        Value::Str(h) => get_str(h),
        other => panic!("fieldidx_res_scope: expected Text, got {:?}", other),
    };
    if residency_mode() == which {
        RESIDENT.with(|r| {
            let mut r = r.borrow_mut();
            FIDX.with(|idx| {
                let mut idx = idx.borrow_mut();
                for h in r.values() {
                    idx.remove(h);
                }
            });
            r.clear();
        });
    }
    Value::Unit
}

#[cfg(test)]
mod residency_tests {
    //! AXVERITY_FIELDINDEX_RESIDENCY_BUILD_V1 — direct incrementality instrument
    //! (hard-limit VERIFY_GENUINE_INCREMENTALITY). Builds real 84-byte framed WAL
    //! segments (`H|P|V|env|payload`, the ONE layout `walindex::walk_frames`
    //! parses) and asserts `do_replay` walks only the DELTA (K frames), never the
    //! whole store (base+K), and that the resulting postings are correct.
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::Write as _;

    fn frame(payload: &str) -> Vec<u8> {
        let hexh: String = Sha256::digest(payload.as_bytes())
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();
        let env = ""; // empty envelope (field index ignores it)
        let mut out = Vec::with_capacity(84 + payload.len());
        out.extend_from_slice(hexh.as_bytes()); //  64  H
        out.extend_from_slice(format!("{:010}", payload.len()).as_bytes()); // 10  P
        out.extend_from_slice(format!("{:010}", env.len()).as_bytes()); // 10  V
        out.extend_from_slice(env.as_bytes());
        out.extend_from_slice(payload.as_bytes());
        out
    }

    // FIELDIDX declaration frame for a distinct (k, v{i}) -> sha256:{i}.
    fn fidx_frame(i: usize) -> Vec<u8> {
        frame(&format!("FIELDIDX\tk\tv{}\tsha256:{:064}", i, i))
    }

    fn append_frames(path: &str, range: std::ops::Range<usize>) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for i in range {
            f.write_all(&fidx_frame(i)).unwrap();
        }
        f.sync_all().unwrap();
    }

    #[test]
    fn replay_walks_only_the_delta_not_the_whole_store() {
        let dir = std::env::temp_dir().join(format!("fidx_res_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prefix = format!("{}/seg-", dir.to_str().unwrap());
        let seg0 = format!("{}0.log", prefix);

        // Seed B base frames, then a cold build over the whole store.
        let b = 500usize;
        append_frames(&seg0, 0..b);
        let mut sh = FieldShard { map: HashMap::new(), fseg: 0, foff: 0 };
        let scanned_full = do_rebuild(&mut sh, &prefix, "/nonexistent.snap");
        assert_eq!(scanned_full, b as i64, "cold build must walk the whole store");
        assert_eq!(sh.map.len(), b, "cold build indexed all base postings");

        // Append K, replay: MUST walk ONLY K (incrementality), not b+K.
        let k = 37usize;
        append_frames(&seg0, b..(b + k));
        let scanned_delta = do_replay(&mut sh, &prefix);
        assert_eq!(
            scanned_delta, k as i64,
            "replay walked {} frames; genuine incremental replay must walk exactly the {}-frame delta, not base+K={}",
            scanned_delta, k, b + k
        );
        assert_eq!(sh.map.len(), b + k, "delta postings merged in");
        // The new key must resolve.
        assert!(sh.map.contains_key(&format!("k\tv{}", b + k - 1)));

        // No new frames → replay walks 0 (stat-guard fast path).
        let scanned_none = do_replay(&mut sh, &prefix);
        assert_eq!(scanned_none, 0, "zero-delta replay must scan 0 frames");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replay_crosses_a_segment_rotation() {
        let dir = std::env::temp_dir().join(format!("fidx_res_rot_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prefix = format!("{}/seg-", dir.to_str().unwrap());

        append_frames(&format!("{}0.log", prefix), 0..10);
        let mut sh = FieldShard { map: HashMap::new(), fseg: 0, foff: 0 };
        assert_eq!(do_rebuild(&mut sh, &prefix, "/nonexistent.snap"), 10);

        // A later segment appears (a rotation). Replay must pick up its frames.
        append_frames(&format!("{}1.log", prefix), 10..25);
        assert_eq!(do_replay(&mut sh, &prefix), 15, "replay must cross into segment 1");
        assert_eq!(sh.map.len(), 25);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

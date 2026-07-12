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

        let (wm_seg, wm_off) = load_snapshot(&mut sh.map, &snap_path);
        bump_frontier(sh, wm_seg, wm_off);
        let map = &mut sh.map;
        let (fr_seg, fr_off, scanned) =
            super::walindex::walk_frames(&prefix, wm_seg, wm_off, |_seg, _off, _len, _env, payload, hexh| {
                if let Ok(text) = std::str::from_utf8(payload) {
                    index_payload(map, text, hexh);
                }
            });
        bump_frontier(sh, fr_seg, fr_off);
        scanned
    });
    Value::Int(scanned)
}

//! BRIDGE_WALINDEX_V1 (AXVERITY_WAL_ALLOCATION_AND_BLOB_PATH, Landing A) — the
//! WAL internal index as a HOT in-memory per-thread shard plus a DISPOSABLE
//! batched snapshot. This corrects the `.axverity/wal/index/` regression (one
//! tiny file per key) — see `docs/landingA-wal-index-and-segments.md`.
//!
//! ## Storage model — THREAD-OWNED shards, NO shared registry
//!   (hard-limit INDEX_SHARDED_NO_SHARED_LOCK — the same nothing-shared,
//!    nothing-locked rule that killed the Landing-1 logbuf registry mutex)
//!
//! Each open index shard lives in **thread-local storage** (`IDX`), reachable
//! ONLY by the thread that opened it, keyed by an opaque integer handle. There
//! is no process-global handle→shard registry and no lock anywhere on the
//! insert/lookup path: two writer threads share NOTHING and can never contend.
//! The handle counter (`NEXT`) is thread-local too, starting at 1 per thread —
//! so a `--entries` writer thread's first `walidx_open` returns handle 1,
//! identical to a fresh process, exactly like `logbuf.rs`. This is the literal
//! per-writer-thread in-memory shard the requirement mandates; its canonical
//! demonstration is the N-shard daemon (N threads → N thread-local shards).
//!
//! ## Authoritative vs. disposable
//!
//! The map is `key → (seg, off, len)` where `key` is the 64-hex object digest,
//! `seg` the segment sequence number, `off` the payload byte-offset within that
//! segment, `len` the payload byte-length. It is NEVER the source of truth: the
//! framed WAL segments are. `walidx_snapshot` serializes a shard to ONE batched
//! file per shard (never per key), watermarked with the WAL position it is
//! current as of; `walidx_rebuild` reconstructs a shard by loading that snapshot
//! and then replaying the framed WAL forward from the watermark, hash-checking
//! every frame. A stale, corrupt, or missing snapshot therefore degrades to more
//! WAL replay, never to a wrong answer (hard-limit INDEX_SNAPSHOT_DISPOSABLE).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

use super::value::{get_str, intern_str, Value};

/// One open index shard: the shard name (for snapshot provenance) and the
/// in-memory hash→location map. Thread-owned; reachable only by its opener.
struct IdxShard {
    #[allow(dead_code)]
    shard: String,
    map: HashMap<String, (i64, i64, i64)>, // key -> (seg, off, len)
    fseg: i64, // frontier segment: the WAL position this shard is current as of
    foff: i64, // frontier offset within fseg (next-write position)
}

/// Advance a shard's frontier watermark to `(seg, end)` if it is beyond the
/// current one. `end` is a frame's next-write position (payload offset + len).
fn bump_frontier(sh: &mut IdxShard, seg: i64, end: i64) {
    if seg > sh.fseg || (seg == sh.fseg && end > sh.foff) {
        sh.fseg = seg;
        sh.foff = end;
    }
}

thread_local! {
    /// Per-thread shard table, keyed by integer handle. THREAD-LOCAL, never
    /// shared — the insert/lookup path touches only the calling thread's own
    /// map, so no two writer threads ever contend and no lock is taken.
    static IDX: RefCell<HashMap<i64, IdxShard>> = RefCell::new(HashMap::new());

    /// Per-thread handle counter (first `walidx_open` returns 1, like logbuf).
    static NEXT: Cell<i64> = const { Cell::new(1) };
}

fn next_handle() -> i64 {
    NEXT.with(|c| {
        let n = c.get();
        c.set(n + 1);
        n
    })
}

/// `walidx_open(shard: Text) -> Int`
///
/// Register a fresh empty index shard in THIS thread's table and return its
/// handle. Handles are never reused within a thread.
#[track_caller]
pub fn walidx_open(arg: Value) -> Value {
    let shard = match arg {
        Value::Str(h) => get_str(h),
        other => panic!("walidx_open: expected Text shard, got {:?}", other),
    };
    let h = next_handle();
    IDX.with(|idx| {
        idx.borrow_mut().insert(h, IdxShard { shard, map: HashMap::new(), fseg: 0, foff: 0 });
    });
    Value::Int(h)
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

/// `walidx_insert(h: Int, key: Text, seg: Int, off: Int, len: Int) -> Unit`
///
/// Synchronous in-memory insert into the calling thread's shard — a HashMap
/// insert, no syscall, no lock. This is the hot loop the daemon runs per object.
#[track_caller]
pub fn walidx_insert(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 5 => es,
        other => panic!("walidx_insert: expected Tuple(Int, Text, Int, Int, Int), got {:?}", other),
    };
    let h = arg_int(&es[0], "walidx_insert", 0);
    let key = arg_str(&es[1], "walidx_insert", 1);
    let seg = arg_int(&es[2], "walidx_insert", 2);
    let off = arg_int(&es[3], "walidx_insert", 3);
    let len = arg_int(&es[4], "walidx_insert", 4);
    IDX.with(|idx| {
        let mut idx = idx.borrow_mut();
        let sh = idx
            .get_mut(&h)
            .unwrap_or_else(|| panic!("walidx_insert: unknown handle {} (not opened on this thread)", h));
        sh.map.insert(key, (seg, off, len));
        bump_frontier(sh, seg, off + len);
    });
    Value::Unit
}

/// `walidx_has(h: Int, key: Text) -> Bool` — is `key` present in the shard?
#[track_caller]
pub fn walidx_has(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 2 => es,
        other => panic!("walidx_has: expected Tuple(Int, Text), got {:?}", other),
    };
    let h = arg_int(&es[0], "walidx_has", 0);
    let key = arg_str(&es[1], "walidx_has", 1);
    IDX.with(|idx| {
        let idx = idx.borrow();
        let sh = idx
            .get(&h)
            .unwrap_or_else(|| panic!("walidx_has: unknown handle {}", h));
        Value::Bool(sh.map.contains_key(&key))
    })
}

/// `walidx_get(h: Int, key: Text) -> Text`
///
/// Return `"<seg>\t<off>\t<len>"` for `key`, or `""` if absent.
#[track_caller]
pub fn walidx_get(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 2 => es,
        other => panic!("walidx_get: expected Tuple(Int, Text), got {:?}", other),
    };
    let h = arg_int(&es[0], "walidx_get", 0);
    let key = arg_str(&es[1], "walidx_get", 1);
    IDX.with(|idx| {
        let idx = idx.borrow();
        let sh = idx
            .get(&h)
            .unwrap_or_else(|| panic!("walidx_get: unknown handle {}", h));
        let out = match sh.map.get(&key) {
            Some((seg, off, len)) => format!("{}\t{}\t{}", seg, off, len),
            None => String::new(),
        };
        Value::Str(intern_str(&out))
    })
}

/// Durable write: write to a temp sibling, fsync it, rename over `path`, fsync
/// the parent directory. The snapshot is disposable, but an atomic replace keeps
/// a reader from ever observing a half-written snapshot (it would otherwise be
/// rejected by the load validator and fall back to replay — still correct, but
/// the atomic rename avoids the churn).
fn write_durable(path: &str, bytes: &[u8]) {
    let p = Path::new(path);
    let parent = p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(
        ".{}.tmp",
        p.file_name().and_then(|s| s.to_str()).unwrap_or("snap")
    ));
    {
        let mut f = File::create(&tmp)
            .unwrap_or_else(|e| panic!("walidx_snapshot: create {:?}: {}", tmp, e));
        f.write_all(bytes).unwrap_or_else(|e| panic!("walidx_snapshot: write: {}", e));
        f.sync_all().unwrap_or_else(|e| panic!("walidx_snapshot: fsync: {}", e));
    }
    std::fs::rename(&tmp, p).unwrap_or_else(|e| panic!("walidx_snapshot: rename: {}", e));
    if let Ok(dir) = File::open(parent) {
        let _ = dir.sync_all();
    }
}

/// `walidx_snapshot(h: Int, path: Text) -> Unit`
///
/// Serialize the shard to ONE batched file at `path` (never per key), watermarked
/// with the shard's own tracked frontier (`fseg`, `foff`) — the WAL position it
/// is current as of, maintained by `walidx_insert`/`walidx_rebuild`. Format:
///   `WALIDX1\t<wm_seg>\t<wm_off>\t<count>\n`  then `count` lines
///   `<key>\t<seg>\t<off>\t<len>\n`.
#[track_caller]
pub fn walidx_snapshot(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 2 => es,
        other => panic!("walidx_snapshot: expected Tuple(Int, Text), got {:?}", other),
    };
    let h = arg_int(&es[0], "walidx_snapshot", 0);
    let path = arg_str(&es[1], "walidx_snapshot", 1);
    let body = IDX.with(|idx| {
        let idx = idx.borrow();
        let sh = idx
            .get(&h)
            .unwrap_or_else(|| panic!("walidx_snapshot: unknown handle {}", h));
        let mut s = format!("WALIDX1\t{}\t{}\t{}\n", sh.fseg, sh.foff, sh.map.len());
        for (k, (seg, off, len)) in &sh.map {
            s.push_str(&format!("{}\t{}\t{}\t{}\n", k, seg, off, len));
        }
        s
    });
    write_durable(&path, body.as_bytes());
    Value::Unit
}

/// Load a batched snapshot into `map`, returning the watermark `(seg, off)` it is
/// current as of. Non-authoritative: a missing, malformed, or truncated snapshot
/// yields `(0, 0)` (full replay) and leaves `map` unpopulated by it. Entries are
/// only committed to `map` if the whole file validates (magic + declared count).
fn load_snapshot(map: &mut HashMap<String, (i64, i64, i64)>, path: &str) -> (i64, i64) {
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
    if hp.len() != 4 || hp[0] != "WALIDX1" {
        return (0, 0);
    }
    let wm_seg: i64 = hp[1].parse().unwrap_or(-1);
    let wm_off: i64 = hp[2].parse().unwrap_or(-1);
    let count: usize = hp[3].parse().unwrap_or(usize::MAX);
    if wm_seg < 0 || wm_off < 0 || count == usize::MAX {
        return (0, 0);
    }
    let mut staged: Vec<(String, (i64, i64, i64))> = Vec::new();
    for line in lines {
        let p: Vec<&str> = line.split('\t').collect();
        if p.len() != 4 {
            return (0, 0);
        }
        let seg: i64 = p[1].parse().unwrap_or(-1);
        let off: i64 = p[2].parse().unwrap_or(-1);
        let len: i64 = p[3].parse().unwrap_or(-1);
        if seg < 0 || off < 0 || len < 0 {
            return (0, 0);
        }
        staged.push((p[0].to_string(), (seg, off, len)));
    }
    if staged.len() != count {
        return (0, 0); // truncated/torn snapshot — full replay
    }
    for (k, v) in staged {
        map.insert(k, v);
    }
    (wm_seg, wm_off)
}

fn read_segment(prefix: &str, seq: i64) -> Option<Vec<u8>> {
    let path = format!("{}{}.log", prefix, seq);
    match File::open(&path) {
        Ok(mut f) => {
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)
                .unwrap_or_else(|e| panic!("walidx_rebuild: read {}: {}", path, e));
            Some(buf)
        }
        Err(_) => None,
    }
}

fn sha256_hex(payload: &[u8]) -> String {
    Sha256::digest(payload).iter().map(|b| format!("{:02x}", b)).collect()
}

/// THE ONE production frame-scanner (AXVERITY_UNIFIED_DURABLE_STREAMS_V1, phase 2).
///
/// Walk the framed WAL segments `<prefix><seq>.log` forward from the watermark
/// `(wm_seg, wm_off)`, hash-checking every frame, calling `visit` for each valid
/// frame and STOPPING at the first torn/invalid one (the truncation frontier).
/// Returns `(frontier_seg, frontier_off, frames_scanned)`.
///
/// Frame layout (Branch A — envelope extension carrying (table,seq,pk) OUTSIDE the
/// content hash):
///   `H(64 hex) | P(10 dec payload-len) | V(10 dec envelope-len) | env(V) | payload(P)`
/// — an 84-byte fixed header, then the un-hashed envelope, then the payload. The
/// content hash covers the PAYLOAD ONLY (`sha256_hex(payload) == H`); the envelope
/// is never hashed, so object identity is unchanged by its presence. A frame is
/// valid iff the whole `84 + V + P` extent is present AND the payload hash-matches
/// — so a torn tail discards the object AND its binding together (env sits BEFORE
/// the payload, so a hash-valid payload transitively guarantees an intact env under
/// the truncation-only fault model). `visit` receives
/// `(seg, payload_off, payload_len, env_bytes, hexh)`.
///
/// This is the SINGLE frame parser ALL index projections share (content-hash index
/// via `walidx_rebuild`, the (table,pk)->hash projection via
/// `pkindex::pkidx_rebuild`, and the field index via `fieldidx::fieldidx_rebuild`),
/// so they can never drift on the frame layout (hard-limit
/// FRAME_PARSERS_UPDATED_LOCKSTEP). Any layout change lives HERE, once. `visit`
/// receives `(seg, payload_off, payload_len, env_bytes, payload_bytes, hexh)`.
pub(crate) fn walk_frames<F>(prefix: &str, wm_seg: i64, wm_off: i64, mut visit: F) -> (i64, i64, i64)
where
    F: FnMut(i64, i64, i64, &[u8], &[u8], &str),
{
    let (mut cur_seg, mut off) = (wm_seg, wm_off);
    let mut scanned: i64 = 0;
    let (mut fr_seg, mut fr_off) = (wm_seg, wm_off);
    loop {
        let data = match read_segment(prefix, cur_seg) {
            Some(d) => d,
            None => break, // watermark segment no longer exists — stop
        };
        let dlen = data.len();
        let mut off_us = if off < 0 { 0usize } else { off as usize };
        let mut torn = false;
        // Need the full 84-byte fixed header (H|P|V) before we can read V.
        while off_us + 84 <= dlen {
            let hexh = match std::str::from_utf8(&data[off_us..off_us + 64]) {
                Ok(s) => s,
                Err(_) => { torn = true; break; }
            };
            let plenf = match std::str::from_utf8(&data[off_us + 64..off_us + 74]) {
                Ok(s) => s,
                Err(_) => { torn = true; break; }
            };
            let vlenf = match std::str::from_utf8(&data[off_us + 74..off_us + 84]) {
                Ok(s) => s,
                Err(_) => { torn = true; break; }
            };
            let plen: usize = match plenf.trim().parse() {
                Ok(n) => n,
                Err(_) => { torn = true; break; }
            };
            let vlen: usize = match vlenf.trim().parse() {
                Ok(n) => n,
                Err(_) => { torn = true; break; }
            };
            let hdr_end = off_us + 84;
            if hdr_end + vlen + plen > dlen {
                torn = true; // envelope or payload torn at the tail
                break;
            }
            let env = &data[hdr_end..hdr_end + vlen];
            let payload = &data[hdr_end + vlen..hdr_end + vlen + plen];
            if sha256_hex(payload) != hexh {
                torn = true; // hash mismatch — torn/invalid frame
                break;
            }
            visit(cur_seg, (hdr_end + vlen) as i64, plen as i64, env, payload, hexh);
            off_us += 84 + vlen + plen;
            scanned += 1;
        }
        let clean_end = !torn && off_us == dlen;
        if clean_end && read_segment(prefix, cur_seg + 1).is_some() {
            cur_seg += 1;
            off = 0;
            continue;
        }
        fr_seg = cur_seg;
        fr_off = off_us as i64;
        break; // torn frontier, or last segment exhausted
    }
    (fr_seg, fr_off, scanned)
}

/// `walidx_rebuild(h: Int, seg_prefix: Text, snap_path: Text) -> Int`
///
/// Reconstruct the shard `h`: load the disposable snapshot at `snap_path` (if
/// valid), then replay the framed WAL segments `<seg_prefix><seq>.log` forward
/// from the snapshot watermark, hash-checking each frame — inserting valid
/// frames into the shard and STOPPING at the first torn/invalid frame (the
/// truncation frontier). This IS the crash recovery, and it is the ONE
/// frame-scanner the read path shares (cross-checked against `spike3_recover`).
/// Returns the number of frames scanned from the WAL (post-watermark), for
/// diagnostics; the reconstructed shard is the real output.
///
/// Frame layout (Branch A): `H(64 hex) | P(10 payload-len) | V(10 envelope-len) |
/// env(V) | payload(P)` — an 84-byte fixed header, then the un-hashed envelope,
/// then the payload. The per-frame walk + torn-tail rule lives in `walk_frames`
/// (the ONE shared production scanner); this fn only loads the disposable snapshot
/// and threads a content-index visitor into it. A clean short read ends a segment;
/// a hash mismatch or out-of-bounds length is a torn tail and halts the scan.
#[track_caller]
pub fn walidx_rebuild(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 3 => es,
        other => panic!("walidx_rebuild: expected Tuple(Int, Text, Text), got {:?}", other),
    };
    let h = arg_int(&es[0], "walidx_rebuild", 0);
    let prefix = arg_str(&es[1], "walidx_rebuild", 1);
    let snap_path = arg_str(&es[2], "walidx_rebuild", 2);

    let scanned = IDX.with(|idx| {
        let mut idx = idx.borrow_mut();
        let sh = idx
            .get_mut(&h)
            .unwrap_or_else(|| panic!("walidx_rebuild: unknown handle {}", h));

        let (wm_seg, wm_off) = load_snapshot(&mut sh.map, &snap_path);
        bump_frontier(sh, wm_seg, wm_off);
        // Content-hash index visitor: key = payload digest, value = (seg, payload
        // offset, payload len). The envelope is ignored here (the pk-index consumes
        // it via the same walk in pkindex.rs). One scanner, two projections.
        let map = &mut sh.map;
        let (fr_seg, fr_off, scanned) = walk_frames(&prefix, wm_seg, wm_off, |seg, off, len, _env, _payload, hexh| {
            map.insert(hexh.to_string(), (seg, off, len));
        });
        bump_frontier(sh, fr_seg, fr_off); // torn/exhausted → frontier
        scanned
    });
    Value::Int(scanned)
}

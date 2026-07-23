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
use std::sync::OnceLock;

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
    // AXVERITY_COLDREAD_DEEP_DIVE_V1 Part A: gated per-stage timing. Splits the
    // whole-segment disk read (`read_segment` = read_to_end) from the per-frame
    // parse+sha256 CPU loop, capturing wall AND thread-CPU for each so the
    // harness can compute disk-wait (wall-cpu) vs CPU (cpu). Zero cost when off.
    let probe = super::coldprobe::enabled();
    let mut seg_read_wall = 0u64;
    let mut seg_read_cpu = 0u64;
    let mut parse_wall = 0u64;
    let mut parse_cpu = 0u64;
    let mut seg_bytes = 0u64;
    loop {
        let (rw0, rc0) = if probe {
            (super::coldprobe::wall_ns(), super::coldprobe::cpu_ns())
        } else {
            (0, 0)
        };
        let data = match read_segment(prefix, cur_seg) {
            Some(d) => d,
            None => break, // watermark segment no longer exists — stop
        };
        if probe {
            seg_read_wall += super::coldprobe::wall_ns().saturating_sub(rw0);
            seg_read_cpu += super::coldprobe::cpu_ns().saturating_sub(rc0);
            seg_bytes += data.len() as u64;
        }
        let dlen = data.len();
        let mut off_us = if off < 0 { 0usize } else { off as usize };
        let mut torn = false;
        let (pw0, pc0) = if probe {
            (super::coldprobe::wall_ns(), super::coldprobe::cpu_ns())
        } else {
            (0, 0)
        };
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
        if probe {
            parse_wall += super::coldprobe::wall_ns().saturating_sub(pw0);
            parse_cpu += super::coldprobe::cpu_ns().saturating_sub(pc0);
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
    if probe {
        // `seg_read_*` = whole-segment disk read (read_to_end); `parse_*` =
        // per-frame utf8 parse + sha256 verify CPU. A short segment basename
        // tags whether this scan is a field-index anchor scan or a content pull.
        let tag = Path::new(prefix)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(prefix);
        super::coldprobe::emit(
            "walk_frames",
            &format!(
                "prefix={}\tframes={}\tseg_bytes={}\tseg_read_wall_ns={}\tseg_read_cpu_ns={}\tparse_hash_wall_ns={}\tparse_hash_cpu_ns={}",
                tag, scanned, seg_bytes, seg_read_wall, seg_read_cpu, parse_wall, parse_cpu
            ),
        );
    }
    (fr_seg, fr_off, scanned)
}

/// On-disk byte length of the framed WAL segment `<prefix><seq>.log`, or `None`
/// if it does not exist. Used by `do_replay`'s cheap stat-guard to decide whether
/// a frontier segment has grown at all before paying a whole-file read. Mirrors
/// `fieldidx::segment_len`.
fn segment_len(prefix: &str, seq: i64) -> Option<u64> {
    std::fs::metadata(format!("{}{}.log", prefix, seq)).ok().map(|m| m.len())
}

/// Full (re)build of shard `sh`: load the disposable snapshot (if valid), then
/// forward-replay the framed WAL from the SNAPSHOT watermark via the ONE shared
/// scanner `walk_frames`, keying each frame by its payload digest. This is the
/// cold-start / fresh-handle cost, factored out of `walidx_rebuild` so the
/// resident path can share it. Content-hash index visitor: key = payload digest,
/// value = (seg, payload offset, payload len); the envelope is ignored here (the
/// pk-index consumes it via the same walk in pkindex.rs). Returns frames scanned.
fn do_rebuild(sh: &mut IdxShard, prefix: &str, snap_path: &str) -> i64 {
    let (wm_seg, wm_off) = load_snapshot(&mut sh.map, snap_path);
    bump_frontier(sh, wm_seg, wm_off);
    let map = &mut sh.map;
    let (fr_seg, fr_off, scanned) =
        walk_frames(prefix, wm_seg, wm_off, |seg, off, len, _env, _payload, hexh| {
            map.insert(hexh.to_string(), (seg, off, len));
        });
    bump_frontier(sh, fr_seg, fr_off); // torn/exhausted → frontier
    scanned
}

/// GENUINELY-INCREMENTAL replay of shard `sh`: forward-replay the framed WAL from
/// the HANDLE'S OWN frontier (`sh.fseg/sh.foff`), NOT the snapshot watermark. This
/// is the whole point of AXVERITY_PULLOBJECT_RESIDENCY_BUILD_V1 — `walidx_rebuild`
/// always re-walks from the snapshot, ignoring the caller's position (the same
/// finding the field-index residency build made for `fieldidx_rebuild`). Returns
/// frames scanned SINCE the frontier — a coherent refresh costs O(delta), not
/// O(whole store).
///
/// Cheap stat-guard: if the frontier segment has not grown past our offset AND no
/// later segment exists, there is nothing to replay — skip the whole-file read
/// (`read_segment` reads the ENTIRE segment). This is what makes the M within-query
/// replays of a large aggregate O(1) each (one/two `stat`s) in the common
/// zero-delta case, instead of O(M × segment-size) I/O that would erase the win —
/// byte-for-byte the same guard `fieldidx::do_replay` uses.
fn do_replay(sh: &mut IdxShard, prefix: &str) -> i64 {
    let (from_seg, from_off) = (sh.fseg, sh.foff);
    let has_next = segment_len(prefix, from_seg + 1).is_some();
    match segment_len(prefix, from_seg) {
        Some(len) if (len as i64) <= from_off && !has_next => return 0, // no delta
        None if !has_next => return 0, // frontier segment gone, no successor
        _ => {}
    }
    let map = &mut sh.map;
    let (fr_seg, fr_off, scanned) =
        walk_frames(prefix, from_seg, from_off, |seg, off, len, _env, _payload, hexh| {
            map.insert(hexh.to_string(), (seg, off, len));
        });
    bump_frontier(sh, fr_seg, fr_off);
    scanned
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
        do_rebuild(sh, &prefix, &snap_path)
    });
    Value::Int(scanned)
}

/// `walidx_replay(h: Int, seg_prefix: Text) -> Int`
///
/// AXVERITY_PULLOBJECT_RESIDENCY_BUILD_V1 — the genuinely-incremental primitive,
/// the exact analogue of `fieldidx_replay`. Replay the framed WAL into shard `h`
/// starting from the HANDLE'S OWN frontier (`sh.fseg/sh.foff`), returning the
/// number of frames walked (the delta since the last rebuild/replay of THIS
/// handle). Unlike `walidx_rebuild`, it does NOT reload the snapshot and does NOT
/// restart from the snapshot watermark. The returned count is the direct
/// incrementality instrument (hard-limit VERIFY_GENUINE_INCREMENTALITY): after
/// appending K frames it returns K, not base+K; with no new frames it returns 0.
#[track_caller]
pub fn walidx_replay(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 2 => es,
        other => panic!("walidx_replay: expected Tuple(Int, Text), got {:?}", other),
    };
    let h = arg_int(&es[0], "walidx_replay", 0);
    let prefix = arg_str(&es[1], "walidx_replay", 1);
    let scanned = IDX.with(|idx| {
        let mut idx = idx.borrow_mut();
        let sh = idx
            .get_mut(&h)
            .unwrap_or_else(|| panic!("walidx_replay: unknown handle {}", h));
        do_replay(sh, &prefix)
    });
    Value::Int(scanned)
}

// ── Per-worker RESIDENT WAL-index handles (AXVERITY_PULLOBJECT_RESIDENCY_BUILD_V1)
//
// This is the direct analogue of `fieldidx.rs`'s resident field-index handles,
// applied to the CONTENT-HASH index that pull_object's WAL tier consumes
// (wal_has/wal_read via wal_find_shard_step). The bug it closes:
// `walidx_open` never closes, and pull_object opens ~3 fresh full-store shards
// per row (wal_has → wal_find_shard_step, then wal_read → wal_find_shard AGAIN +
// its own rebuild). During a large-M aggregate (pred_run pulls M rows) that is
// ~O(M) leaked full-store index shards — the unbounded VmHWM climb
// (gap:axverity-oom-pullobject-rescan-via-predrun-OPEN, ~7.76 GB at M=3000).
//
// A thread-local map `shard -> resident handle` lets a long-lived pg_server worker
// reuse ONE content-index shard across every pull_object instead of the fresh-open
// (leaked) + full-rebuild-per-call fallback. Thread-LOCAL only — same
// nothing-shared/nothing-locked model as `walindex`/`fieldidx`/`logbuf`/`cursor`.
// The resident shard's memory is O(distinct objects in the WAL) — ONE (seg,off,len)
// entry per object regardless of how many times each is pulled — so it is BOUNDED
// by construction: pulling the same or different objects during a large aggregate
// is a HashMap lookup, never a new allocation. (This is the central unknown the
// memory-bound test verifies rather than assumes.)
//
// Coherency is by REPLAY-BEFORE-READ: `walidx_res_get` replays each resident handle
// from its own frontier before every lookup, so it is always current at read time
// regardless of scope. The scopes (query / conn / server) differ ONLY in WHEN the
// handle is dropped (amortization of the one cold rebuild), never in correctness —
// the scope-boundary reset is `walidx_res_scope`.

thread_local! {
    static RESIDENT: RefCell<HashMap<String, i64>> = RefCell::new(HashMap::new());
}

/// The residency scope, read once per process from `AXVERITY_WALIDX_RESIDENCY`:
///   unset / `off` / `0` / `false` → `"off"`    (fresh-handle-per-call fallback,
///                                                byte-identical to pre-build)
///   `query`                        → `"query"`  (resident within one query)
///   `conn` / `connection`          → `"conn"`   (resident within one connection)
///   `server` / `1` / `on` / `true` → `"server"` (resident for the worker's life)
///   explicit `off` / `0` / `false`  → `"off"`    (preserved fresh-handle fallback)
/// Default QUERY as of AXVERITY_CONCURRENT_AND_COLDSTORE_BENCHMARK_V1 (Chris flipped
/// off→query after the cold benchmark: query scope collapses cold aggregates ~67-125×
/// with a bounded resident map (O(distinct objects per query) ⇒ ~11.5MB not 7.76GB,
/// §31) and byte-identical results verified across modes). The explicit off tokens keep
/// the pre-build fresh-rebuild path reachable. SEPARATE flag from
/// AXVERITY_FIELDIDX_RESIDENCY (the two indexes are independently gated).
fn residency_mode() -> &'static str {
    static MODE: OnceLock<&'static str> = OnceLock::new();
    MODE.get_or_init(|| {
        match std::env::var("AXVERITY_WALIDX_RESIDENCY")
            .ok()
            .as_deref()
            .map(|s| s.to_ascii_lowercase())
            .as_deref()
        {
            Some("off") | Some("0") | Some("false") => "off",
            Some("conn") | Some("connection") => "conn",
            Some("server") | Some("1") | Some("on") | Some("true") => "server",
            // unset, "query", or anything unrecognized → the new default
            _ => "query",
        }
    })
}

/// `walidx_residency_mode(_: Unit) -> Text` — the flag, for M1 dispatch in
/// `wal_read` / `wal_find_shard_step`. `off` selects the preserved
/// fresh-handle-per-call path (byte-identical to pre-build).
#[track_caller]
pub fn walidx_residency_mode(_arg: Value) -> Value {
    Value::Str(intern_str(residency_mode()))
}

/// `walidx_res_get(shard: Text, seg_prefix: Text, snap_path: Text, key: Text) -> Text`
///
/// The resident content-index lookup used by pull_object's WAL tier
/// (wal_read / wal_find_shard_step) when residency is enabled. Get-or-create the
/// resident handle for `shard`; bring it current (first touch → full `do_rebuild`;
/// thereafter → incremental `do_replay` from its own frontier); return
/// `"<seg>\t<off>\t<len>"` for `key`, or `""` if absent — byte-identical to what
/// `walidx_get` returns on a freshly-rebuilt handle. The empty-string return
/// doubles as the membership answer (wal_find_shard_step tests non-empty), so one
/// resident primitive serves both the has-fan-out and the read.
#[track_caller]
pub fn walidx_res_get(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 4 => es,
        other => panic!(
            "walidx_res_get: expected Tuple(shard, prefix, snap, key), got {:?}",
            other
        ),
    };
    let shard = arg_str(&es[0], "walidx_res_get", 0);
    let prefix = arg_str(&es[1], "walidx_res_get", 1);
    let snap = arg_str(&es[2], "walidx_res_get", 2);
    let key = arg_str(&es[3], "walidx_res_get", 3);

    let existing = RESIDENT.with(|r| r.borrow().get(&shard).copied());
    let h = match existing {
        Some(h) => h,
        None => {
            let h = next_handle();
            IDX.with(|idx| {
                idx.borrow_mut().insert(
                    h,
                    IdxShard { shard: shard.clone(), map: HashMap::new(), fseg: 0, foff: 0 },
                );
            });
            RESIDENT.with(|r| {
                r.borrow_mut().insert(shard.clone(), h);
            });
            h
        }
    };

    IDX.with(|idx| {
        let mut idx = idx.borrow_mut();
        let sh = idx
            .get_mut(&h)
            .unwrap_or_else(|| panic!("walidx_res_get: resident handle {} vanished", h));
        if existing.is_none() {
            do_rebuild(sh, &prefix, &snap);
        } else {
            do_replay(sh, &prefix);
        }
        let out = match sh.map.get(&key) {
            Some((seg, off, len)) => format!("{}\t{}\t{}", seg, off, len),
            None => String::new(),
        };
        Value::Str(intern_str(&out))
    })
}

/// `walidx_res_scope(which: Text) -> Unit`
///
/// Scope-boundary reset: drop ALL resident handles (and free the underlying IDX
/// shards, so a long-lived worker does not leak) IFF the active residency mode
/// equals `which`. Called by the server loop with `"query"` at each query start
/// and `"conn"` at each connection start. Under `server` mode neither boundary
/// matches, so handles persist for the worker's life; under `off` nothing is
/// resident so it is always a no-op. Byte-for-byte the same discipline as
/// `fieldidx_res_scope`.
#[track_caller]
pub fn walidx_res_scope(arg: Value) -> Value {
    let which = match arg {
        Value::Str(h) => get_str(h),
        other => panic!("walidx_res_scope: expected Text, got {:?}", other),
    };
    if residency_mode() == which {
        RESIDENT.with(|r| {
            let mut r = r.borrow_mut();
            IDX.with(|idx| {
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
    //! AXVERITY_PULLOBJECT_RESIDENCY_BUILD_V1 — direct incrementality instrument
    //! (hard-limit VERIFY_GENUINE_INCREMENTALITY). Builds real 84-byte framed WAL
    //! segments (`H|P|V|env|payload`, the ONE layout `walk_frames` parses) and
    //! asserts `do_replay` walks only the DELTA (K frames), never the whole store
    //! (base+K), and that the resulting (seg,off,len) map is correct. Mirrors
    //! `fieldidx::residency_tests`.
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::Write as _;

    fn frame(payload: &str) -> Vec<u8> {
        let hexh: String = Sha256::digest(payload.as_bytes()).iter().map(|b| format!("{:02x}", b)).collect();
        let env = "";
        let mut out = Vec::with_capacity(84 + payload.len());
        out.extend_from_slice(hexh.as_bytes()); // 64 H
        out.extend_from_slice(format!("{:010}", payload.len()).as_bytes()); // 10 P
        out.extend_from_slice(format!("{:010}", env.len()).as_bytes()); // 10 V
        out.extend_from_slice(env.as_bytes());
        out.extend_from_slice(payload.as_bytes());
        out
    }

    // A distinct object payload per i (its own content hash → its own key).
    fn obj_frame(i: usize) -> Vec<u8> {
        frame(&format!("RECORD\tgrp=g\tprice={:05}\tcolor=c{}", i % 100000, i))
    }
    // The hex key an object frame indexes under (payload digest, walk_frames' hexh).
    fn obj_key(i: usize) -> String {
        Sha256::digest(format!("RECORD\tgrp=g\tprice={:05}\tcolor=c{}", i % 100000, i).as_bytes())
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect()
    }

    fn append_frames(path: &str, range: std::ops::Range<usize>) {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        for i in range {
            f.write_all(&obj_frame(i)).unwrap();
        }
        f.sync_all().unwrap();
    }

    #[test]
    fn replay_walks_only_the_delta_not_the_whole_store() {
        let dir = std::env::temp_dir().join(format!("walidx_res_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prefix = format!("{}/seg-", dir.to_str().unwrap());
        let seg0 = format!("{}0.log", prefix);

        let b = 500usize;
        append_frames(&seg0, 0..b);
        let mut sh = IdxShard { shard: "0".into(), map: HashMap::new(), fseg: 0, foff: 0 };
        let scanned_full = do_rebuild(&mut sh, &prefix, "/nonexistent.snap");
        assert_eq!(scanned_full, b as i64, "cold build must walk the whole store");
        assert_eq!(sh.map.len(), b, "cold build indexed all base objects");

        let k = 37usize;
        append_frames(&seg0, b..(b + k));
        let scanned_delta = do_replay(&mut sh, &prefix);
        assert_eq!(
            scanned_delta, k as i64,
            "replay walked {} frames; genuine incremental replay must walk exactly the {}-frame delta, not base+K={}",
            scanned_delta, k, b + k
        );
        assert_eq!(sh.map.len(), b + k, "delta objects merged in");
        assert!(sh.map.contains_key(&obj_key(b + k - 1)), "the new object must resolve");

        let scanned_none = do_replay(&mut sh, &prefix);
        assert_eq!(scanned_none, 0, "zero-delta replay must scan 0 frames (stat-guard)");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replay_crosses_a_segment_rotation() {
        let dir = std::env::temp_dir().join(format!("walidx_res_rot_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prefix = format!("{}/seg-", dir.to_str().unwrap());

        append_frames(&format!("{}0.log", prefix), 0..10);
        let mut sh = IdxShard { shard: "0".into(), map: HashMap::new(), fseg: 0, foff: 0 };
        assert_eq!(do_rebuild(&mut sh, &prefix, "/nonexistent.snap"), 10);

        append_frames(&format!("{}1.log", prefix), 10..25);
        assert_eq!(do_replay(&mut sh, &prefix), 15, "replay must cross into segment 1");
        assert_eq!(sh.map.len(), 25);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn res_get_matches_a_fresh_rebuild_and_is_bounded() {
        // The resident handle must return byte-identical meta to a fresh rebuild,
        // and its map size must stay == distinct objects no matter how many times
        // we look up (bounded by construction, not by number of lookups).
        let dir = std::env::temp_dir().join(format!("walidx_res_get_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prefix = format!("{}/seg-", dir.to_str().unwrap());
        let n = 200usize;
        append_frames(&format!("{}0.log", prefix), 0..n);

        // fresh rebuild reference
        let mut ref_sh = IdxShard { shard: "s".into(), map: HashMap::new(), fseg: 0, foff: 0 };
        do_rebuild(&mut ref_sh, &prefix, "/nope.snap");

        // resident handle, keyed by shard "s"
        let shard = "s".to_string();
        let get = |i: usize| -> String {
            match walidx_res_get(Value::Tuple(vec![
                Value::Str(intern_str(&shard)),
                Value::Str(intern_str(&prefix)),
                Value::Str(intern_str("/nope.snap")),
                Value::Str(intern_str(&obj_key(i))),
            ])) {
                Value::Str(h) => get_str(&h),
                _ => panic!("res_get non-Text"),
            }
        };
        // 10× the object count of lookups (many repeats) — map must not grow.
        for _round in 0..10 {
            for i in 0..n {
                let (seg, off, len) = ref_sh.map.get(&obj_key(i)).cloned().unwrap();
                assert_eq!(get(i), format!("{}\t{}\t{}", seg, off, len), "resident meta must match fresh rebuild");
            }
        }
        let h = RESIDENT.with(|r| *r.borrow().get(&shard).unwrap());
        let sz = IDX.with(|idx| idx.borrow().get(&h).unwrap().map.len());
        assert_eq!(sz, n, "resident map size == distinct objects, not lookups");
        // absent key → ""
        assert_eq!(get_absent(&shard, &prefix), "", "absent key returns empty");

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn get_absent(shard: &str, prefix: &str) -> String {
        match walidx_res_get(Value::Tuple(vec![
            Value::Str(intern_str(shard)),
            Value::Str(intern_str(prefix)),
            Value::Str(intern_str("/nope.snap")),
            Value::Str(intern_str(&"f".repeat(64))),
        ])) {
            Value::Str(h) => get_str(&h),
            _ => panic!("res_get non-Text"),
        }
    }
}

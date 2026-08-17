//! RAWBLK_V1 (AXVERITY_MEMCPY_HOTPATH_TRIAL_V1) — the "memcpy and return"
//! INSERT hot path's frame format, its derivation, and its recovery.
//!
//! ## What this changes about the durable artifact
//!
//! Today's INSERT hot path parses the SQL, builds a `RECORD` string, hashes it,
//! publishes three in-memory indexes, and only then memcpys the *derived*
//! record frame into the hot block. Measured, parse alone is ~86.5% of that
//! span. This module is the substrate for moving ALL of it off the request
//! thread: the hot path memcpys **what the client submitted** into the block
//! and returns; a pool thread derives the record afterwards.
//!
//! So the durable artifact changes from "the record axVerity derived" to "the
//! submission axVerity received". That is a provenance decision, not a
//! performance one, and it is deliberately surfaced as such — see
//! `docs/turn-memcpy-hotpath-trial.md`. Both candidate submissions are
//! supported:
//!
//!   * kind `A` — payload is the raw SQL text, byte-for-byte as it arrived.
//!   * kind `B` — payload is `<20-digit shape id><Bind parameter section>`,
//!     i.e. the client's structured parameters verbatim plus a reference to a
//!     statement shape registered once at Parse time. No SQL text exists.
//!
//! ## Frame format (chosen deliberately, NOT the `wal_frame_env_bytes` reuse)
//!
//! ```text
//!   "RAWF1"   5  magic
//!   kind      1  'A' | 'B'
//!   ts       20  unix nanos, zero-padded ASCII decimal
//!   plen     10  payload length, zero-padded ASCII decimal
//!   payload  plen
//!   "RAWEND"  6  trailing sentinel
//! ```
//!
//! `docs/turn-slice4-frame-format.md` decided that hot-block records reuse the
//! Branch-A envelope frame `H|P|V|env|payload`, whose `H` is
//! `sha256_hex(payload)`. That frame **cannot** be used here: `H` is precisely
//! the `bytes_hash` this turn is required to move off the hot path, and the
//! `env` is `table\tseq\tpk`, which only exists after parsing. Emitting a frame
//! whose integrity field we are forbidden to compute would mean either a fake
//! hash (a lie in the durable artifact) or paying the cost we are removing.
//!
//! Integrity instead comes from the fault model the existing design already
//! assumes and states: block writes are **truncation-only**
//! (`block_flush.rs::write_bin_durable` is tmp+fsync+rename, so a block file is
//! either whole or absent, and a partially-*filled* arena is a prefix). Under
//! truncation-only, magic + declared length + trailing sentinel is sufficient:
//! a torn tail fails one of the three and parsing stops there, exactly as
//! `parse_frames_from_bytes` stops on a hash mismatch. This is a WEAKER check
//! than sha256 against bit-rot — stated plainly rather than glossed. The
//! derived record written by the pool into the canonical hot-block stream is
//! still sha256-framed, so the content-addressed tier is unchanged.
//!
//! ## Derivation lives here, in ONE place
//!
//! `derive_a` / `derive_b` reproduce `lib/pg_build_record.m1` +
//! `lib/pg_pk_name.m1` exactly (see the per-fn comments for the M1 line each
//! step mirrors). They are used by recovery and replay. The live pool path
//! calls the *M1* fns instead, deliberately: an independent reimplementation on
//! the recovery side is the same discipline `hotblk_recover.rs::env_to_name`
//! adopted — a bug shared by both sides would otherwise silently cancel out,
//! and the kill-9 gate's whole value is that it does not.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::OnceLock;

use sha2::{Digest, Sha256};

use super::value::{get_str, intern_str, Value};

pub const MAGIC: &[u8; 5] = b"RAWF1";
pub const SENTINEL: &[u8; 6] = b"RAWEND";
pub const HDR: usize = 5 + 1 + 20 + 10; // 36
pub const OVERHEAD: usize = HDR + 6; // 42

// ── the flag ─────────────────────────────────────────────────────────────
//
// `AXVERITY_MEMCPY_HOTPATH`: `off` (DEFAULT — today's path, untouched),
// `a` (ARM A), `b` (ARM B). Read once per process, OnceLock-cached, same
// pattern as `slice4::mode`. Nothing here flips a default.

fn mode() -> &'static str {
    static MODE: OnceLock<&'static str> = OnceLock::new();
    MODE.get_or_init(|| match std::env::var("AXVERITY_MEMCPY_HOTPATH") {
        Ok(v) => match v.trim().to_ascii_lowercase().as_str() {
            "a" | "arma" | "arm-a" => "a",
            "b" | "armb" | "arm-b" => "b",
            // MEASUREMENT CONTROL, not an arm: Extended Query framing over the
            // UNCHANGED write path, so ARM B's number can be separated from the
            // cost of the protocol ARM B is obliged to use. See
            // lib/pg_ext_insert_ctl.m1.
            "bctl" => "bctl",
            _ => "off",
        },
        Err(_) => "off",
    })
}

/// `memcpy_hotpath_mode(Unit) -> Text` → `"off"` | `"a"` | `"b"` | `"bctl"`.
pub fn memcpy_hotpath_mode(_: Value) -> Value {
    Value::Str(intern_str(mode()))
}

/// `memcpy_canon_mode(Unit) -> Text` → `"on"` (DEFAULT) | `"off"`.
///
/// MEASUREMENT CONFIGURATION, and it comes with a correctness caveat that must
/// travel with any number produced under it.
///
/// Both arms turned out to be ~1.8x SLOWER than the current path at K=1, and
/// `scripts/memcpy-fsync-count.sh` explains why with a count rather than a
/// story: the current path performs 1.00 durable block flushes per acked row,
/// and both arms perform 2.00 — the request thread fsyncs the RAW block before
/// acking, and the pool fsyncs the derived CANONICAL block afterwards. The
/// slowdown is 199/108 = 1.84 against a flush ratio of exactly 2.00.
///
/// But that doubling is a property of THIS implementation's choice, not of
/// memcpy-and-defer. The canonical stream is written so the read path
/// (fieldidx / walidx / pkindex, all of which scan
/// `.axverity/hotblocks/<shard>/block-*.bin`) keeps working completely
/// untouched — deliberately, because V2's first attempt at removing a
/// write-path component silently broke SELECT. `AXVERITY_MEMCPY_CANON=off`
/// measures the other fork: raw stream only, indexes published in RAM,
/// derived records reconstructible from the raw block on demand.
///
/// CAVEAT, stated because a number without it would be misleading: under `off`
/// the RAM tiers (bindidx / contentidx / qhm) still see every row, and recovery
/// still re-derives every acked row from the raw block, but the block-scanning
/// read tiers do NOT — so a `WHERE col=val` field lookup will miss rows after
/// the RAM tier is evicted or the process restarts. It is a measurement of what
/// the design fork would cost, not a working configuration.
pub fn canon_mode() -> &'static str {
    static M: OnceLock<&'static str> = OnceLock::new();
    M.get_or_init(|| match std::env::var("AXVERITY_MEMCPY_CANON") {
        Ok(v) if matches!(v.trim().to_ascii_lowercase().as_str(), "off" | "0" | "false") => "off",
        _ => "on",
    })
}

pub fn memcpy_canon_mode(_: Value) -> Value {
    Value::Str(intern_str(canon_mode()))
}

// ── frame encode ─────────────────────────────────────────────────────────

fn pad(n: u64, width: usize) -> String {
    format!("{:0width$}", n, width = width)
}

/// Build one raw frame. ONE allocation, then straight memcpys — this is the
/// whole of what the hot path is allowed to do to the client's bytes.
pub fn encode_frame(kind: u8, ts: i64, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(OVERHEAD + payload.len());
    out.extend_from_slice(MAGIC);
    out.push(kind);
    out.extend_from_slice(pad(ts.max(0) as u64, 20).as_bytes());
    out.extend_from_slice(pad(payload.len() as u64, 10).as_bytes());
    out.extend_from_slice(payload);
    out.extend_from_slice(SENTINEL);
    out
}

/// `rawblk_frame(kind: Text, ts: Int, payload: Bytes) -> Bytes`
#[track_caller]
pub fn rawblk_frame(args: Value) -> Value {
    let (kind, ts, payload) = match args {
        Value::Tuple(es) if es.len() == 3 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("rawblk_frame: expected Tuple(Text, Int, Bytes), got {:?}", other),
    };
    let k = match kind {
        Value::Str(h) => *get_str(&h).as_bytes().first().unwrap_or(&b'A'),
        other => panic!("rawblk_frame: arg 0 (kind) expected Text, got {:?}", other),
    };
    let ts = match ts {
        Value::Int(n) => n,
        other => panic!("rawblk_frame: arg 1 (ts) expected Int, got {:?}", other),
    };
    match payload {
        Value::Bytes(b) => Value::Bytes(encode_frame(k, ts, &b)),
        Value::Str(h) => Value::Bytes(encode_frame(k, ts, get_str(&h).as_bytes())),
        other => panic!("rawblk_frame: arg 2 (payload) expected Bytes|Text, got {:?}", other),
    }
}

// ── frame parse ──────────────────────────────────────────────────────────

fn dec(b: &[u8]) -> Option<u64> {
    let mut n: u64 = 0;
    for c in b {
        if !c.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add((c - b'0') as u64)?;
    }
    Some(n)
}

/// Walk every intact frame in one block buffer, in order. Stops at the first
/// frame that fails ANY of magic / declared-length-fits / trailing sentinel —
/// the torn tail. Returns the number of frames visited.
///
/// Deliberately mirrors `walindex::parse_frames_from_bytes`'s contract (stop,
/// never skip-and-continue), so a corrupt interior can never be silently
/// stepped over to reach data beyond it.
pub fn parse_raw_frames<F: FnMut(u8, i64, &[u8])>(data: &[u8], mut visit: F) -> i64 {
    let mut off = 0usize;
    let mut n = 0i64;
    while off + OVERHEAD <= data.len() {
        if &data[off..off + 5] != MAGIC {
            break;
        }
        let kind = data[off + 5];
        let ts = match dec(&data[off + 6..off + 26]) {
            Some(v) => v as i64,
            None => break,
        };
        let plen = match dec(&data[off + 26..off + 36]) {
            Some(v) => v as usize,
            None => break,
        };
        let end = match off.checked_add(OVERHEAD).and_then(|e| e.checked_add(plen)) {
            Some(e) if e <= data.len() => e,
            _ => break,
        };
        if &data[end - 6..end] != SENTINEL {
            break;
        }
        visit(kind, ts, &data[off + HDR..off + HDR + plen]);
        n += 1;
        off = end;
    }
    n
}

// ── derivation: the exact M1 semantics, reimplemented ────────────────────
//
// Mirrors, statement for statement:
//   lib/pg_insert_cols.m1   str_before(str_after(q,"("), ")")
//   lib/pg_insert_vals.m1   the same, one "(" further along
//   lib/pg_build_record.m1  "RECORD" ++ for each pair "\t<col>=<val>"
//   lib/pg_record_step.m1   col = trim(before(rem,",")), val = replace(trim(...),"'","")
//   lib/pg_zip_more.m1      loop while trim(cols_rem) != ""
//   lib/pg_pk_name.m1       trim(table) ++ ":" ++ replace(trim(rawval),"'","")
//   lib/pg_slice_between.m1 table = trim(slice(q, idx(upper,"INTO")+4, idx(upper,"(")))
//
// `str_before` returns the WHOLE string when the delimiter is absent;
// `str_after` returns "" — verified against str_ops.rs, not assumed from the
// doc comment (CLAUDE.md §2 records that those two disagree).

fn s_before<'a>(s: &'a str, d: &str) -> &'a str {
    match s.split_once(d) {
        Some((b, _)) => b,
        None => s,
    }
}

fn s_after<'a>(s: &'a str, d: &str) -> &'a str {
    match s.split_once(d) {
        Some((_, a)) => a,
        None => "",
    }
}

/// `(name, record)` from one ARM-A raw SQL submission.
pub fn derive_a(q: &str) -> Option<(String, String)> {
    let cols = s_before(s_after(q, "("), ")");
    let past_cols = s_after(s_after(q, "("), ")");
    let vals = s_before(s_after(past_cols, "("), ")");
    if cols.trim().is_empty() {
        return None;
    }

    let mut record = String::from("RECORD");
    let mut cols_rem = cols;
    let mut vals_rem = vals;
    let mut guard = 0;
    while !cols_rem.trim().is_empty() && guard < 1_000_000 {
        guard += 1;
        let col = s_before(cols_rem, ",").trim().to_string();
        let val = s_before(vals_rem, ",").trim().replace('\'', "");
        record.push('\t');
        record.push_str(&col);
        record.push('=');
        record.push_str(&val);
        cols_rem = s_after(cols_rem, ",");
        vals_rem = s_after(vals_rem, ",");
    }

    // table: pg_slice_between(q, "INTO", "(")
    let u = q.to_uppercase();
    let si = u.find("INTO")?;
    let s = si + 4;
    let ei = u.find('(')?;
    let e = if ei < s { s } else { ei };
    let table = q.get(s..e)?.trim();

    let pk = s_before(vals, ",").trim().replace('\'', "");
    Some((format!("{}:{}", table, pk), record))
}

/// `(name, record)` from an ARM-B payload: `<20-digit shape id><param section>`.
pub fn derive_b(payload: &[u8], shapes: &HashMap<u64, (String, Vec<String>)>) -> Option<(String, String)> {
    if payload.len() < 20 {
        return None;
    }
    let id = dec(&payload[..20])?;
    let (table, cols) = shapes.get(&id)?;
    let vals = decode_params(&payload[20..])?;
    let mut record = String::from("RECORD");
    for (i, c) in cols.iter().enumerate() {
        record.push('\t');
        record.push_str(c);
        record.push('=');
        record.push_str(vals.get(i).map(|s| s.as_str()).unwrap_or(""));
    }
    let pk = vals.first().cloned().unwrap_or_default();
    Some((format!("{}:{}", table, pk), record))
}

/// Decode a captured Bind parameter section: `int16 n` then n × (`int32 len`,
/// bytes). `len == -1` is SQL NULL, rendered as the empty string (the same
/// thing the Simple path's unquoted-empty-value case produces).
pub fn decode_params(sec: &[u8]) -> Option<Vec<String>> {
    if sec.len() < 2 {
        return None;
    }
    let n = u16::from_be_bytes([sec[0], sec[1]]) as usize;
    let mut off = 2usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        if off + 4 > sec.len() {
            return None;
        }
        let l = i32::from_be_bytes([sec[off], sec[off + 1], sec[off + 2], sec[off + 3]]);
        off += 4;
        if l < 0 {
            out.push(String::new());
            continue;
        }
        let l = l as usize;
        if off + l > sec.len() {
            return None;
        }
        out.push(String::from_utf8_lossy(&sec[off..off + l]).into_owned());
        off += l;
    }
    Some(out)
}

// ── the durable statement-shape log (ARM B) ──────────────────────────────
//
// ARM B's block frames reference a shape by id instead of carrying column
// names. That reference must outlive the process, or an acked row becomes
// unreadable after a crash — so the shape is appended and FSYNCED at Parse
// time, before any Execute referencing it can be acked. Once per prepared
// statement, never on the per-row path.

pub fn shapes_path(raw_root: &str) -> String {
    format!("{}/shapes.log", raw_root)
}

pub fn load_shapes(raw_root: &str) -> HashMap<u64, (String, Vec<String>)> {
    let mut m = HashMap::new();
    if let Ok(txt) = std::fs::read_to_string(shapes_path(raw_root)) {
        for line in txt.lines() {
            let mut f = line.split('\t');
            let (Some(id), Some(table), Some(cols)) = (f.next(), f.next(), f.next()) else {
                continue;
            };
            let Ok(id) = id.parse::<u64>() else { continue };
            m.insert(
                id,
                (table.to_string(), cols.split(',').map(|s| s.to_string()).collect()),
            );
        }
    }
    m
}

/// FNV-1a over `"<table>\t<c1,c2,...>"`. Content-derived so the same statement
/// always gets the same id across processes and the log dedups by construction.
pub fn shape_id(table: &str, cols: &[String]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let key = format!("{}\t{}", table, cols.join(","));
    for b in key.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h & 0x7fff_ffff_ffff_ffff
}

pub fn append_shape_durable(raw_root: &str, id: u64, table: &str, cols: &[String]) {
    let _ = std::fs::create_dir_all(raw_root);
    let path = shapes_path(raw_root);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(format!("{}\t{}\t{}\n", id, table, cols.join(",")).as_bytes());
        let _ = f.sync_all();
    }
}

// ── the one-dir-per-thread guard ─────────────────────────────────────────
//
// `hotblk.rs`'s accumulator register holds exactly ONE block per thread — one
// arena pointer, one cell, one sequence counter, one cursor. It has no field
// for which directory that block belongs to, so if a thread ever writes two
// different block streams, the second stream inherits the first's live block
// and its sequence numbering. Nothing in the type system or the M1 signatures
// prevents it: `pg_hotblk_write` and `pg_rawblk_write` differ only in a string.
//
// This turn wrote that invariant down as a comment and then violated it in the
// same build. Under `AXVERITY_MEMCPY_HOTPATH=b`, Simple-Query INSERTs were
// deliberately left on the current path while Extended-Query INSERTs took the
// raw path; a worker that served one of each straddled `hotblocks/<s>` and
// `rawblocks/<s>`. The symptom was a panic in an unrelated-looking place
// (`fs_append_text(.axverity/rawblocks/1/manifest.log): No such file or
// directory` — the mkdir only happens on a MINT, and the straddling thread
// never minted for the second dir), a respawn loop, and a measured throughput
// collapse from ~1670 to ~370 ops/s at K=16 that looked for all the world like
// a property of ARM B. It was a bug in the harness-visible behaviour of the
// mode, not a property of the arm.
//
// A comment did not prevent it, so the invariant is now ENFORCED. First dir
// wins; a mismatch is a loud fail-stop, not a silent write into the wrong
// stream — because the quiet version of this bug is a block of one stream's
// records flushed into another stream's namespace under another stream's
// sequence number, which is data corruption that recovery cannot detect.
//
// Cost is a thread-local read and a u64 compare (the dir is fingerprinted, not
// string-compared), so it is affordable on the default path too — and it has to
// be on the default path, because the default path is where a straddling thread
// binds its first dir.

thread_local! {
    static BOUND_DIR: RefCell<Option<(u64, String)>> = const { RefCell::new(None) };
}

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// `hotblk_dir_guard(flush_dir: Text) -> Unit`
#[track_caller]
pub fn hotblk_dir_guard(dir: std::sync::Arc<str>) -> Value {
    let dir = dir.to_string();
    let fp = fnv1a(&dir);
    BOUND_DIR.with(|b| {
        let mut b = b.borrow_mut();
        match &*b {
            None => *b = Some((fp, dir)),
            Some((have, _)) if *have == fp => {}
            Some((_, have_dir)) => panic!(
                "hotblk_dir_guard: this thread already owns the block stream '{}' and \
                 cannot also write '{}'. The per-thread hotblk accumulator holds ONE \
                 block with ONE sequence counter; straddling two directories would \
                 flush one stream's records into the other's namespace. Route every \
                 INSERT on a given thread through the same block stream.",
                have_dir, dir
            ),
        }
    });
    Value::Unit
}

// ── derive-pool telemetry ────────────────────────────────────────────────
//
// The intent requires an ANSWER to "does the pool keep up, or does it back up",
// and to "how large is the read-your-writes window" — measurements, not
// arguments. Neither can come from the existing WRITEPROBE mark stream: that is
// gated behind `AXVERITY_WRITEPROBE` and costs 0.8–1.7 µs per mark, which is
// itself larger than several of the stages under test, so switching it on to
// read a queue depth would distort the throughput it is being read alongside.
//
// So this is a separate, ALWAYS-ON, near-free accumulator: three thread-local
// counters updated per derived item, flushed to a small file once every
// `FLUSH_EVERY` items. One ~60-byte write per 1024 derives is below the noise
// floor of a path that already fsyncs blocks, and it runs in every mode so the
// `off` baseline is measured by the same instrument (it simply never ticks,
// because nothing publishes to the derive channel under `off`).
//
// `lat` is the interval from the hot path's own `ts` — the instant the client's
// bytes arrived — to the moment the row is published into bindidx/contentidx/
// qhm. That interval IS the read-your-writes window from the server's side: for
// exactly that long, a row that has been (or is about to be) acked is not yet
// visible to a reader that goes through the in-memory indexes.

const FLUSH_EVERY: i64 = 1024;

thread_local! {
    /// (items, depth_sum, depth_max, lat_sum_ns, lat_max_ns, since_flush)
    static POOL_STATS: RefCell<[i64; 6]> = const { RefCell::new([0; 6]) };
}

fn now_nanos() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

fn write_pool_stats(shard: &str, s: &[i64; 6]) {
    let dir = ".axverity/rawblocks";
    let _ = std::fs::create_dir_all(dir);
    let path = format!(
        "{}/pooldepth-{}-{:?}.txt",
        dir,
        shard,
        std::thread::current().id()
    );
    let line = format!(
        "items={} depth_sum={} depth_max={} lat_sum_ns={} lat_max_ns={}\n",
        s[0], s[1], s[2], s[3], s[4]
    );
    let _ = std::fs::write(path, line);
}

/// `derive_stat(shard: Text, depth: Int, ts: Int) -> Unit`
///
/// Record one derived item: the queue depth observed after it, and the
/// hot-path-to-published latency implied by its `ts`.
#[track_caller]
pub fn derive_stat(args: Value) -> Value {
    // INSERT_PATH_HONESTY_V1 Phase 3 — the single telemetry dial
    // (AXVERITY_TELEMETRY, default OFF; see tsmark::telemetry_enabled). Gates the
    // POOL_STATS counter bump AND the every-1024-items pooldepth-*.txt write.
    if !super::tsmark::telemetry_enabled() {
        return Value::Unit;
    }
    let (shard, depth, ts) = match args {
        Value::Tuple(es) if es.len() == 3 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("derive_stat: expected Tuple(Text, Int, Int), got {:?}", other),
    };
    let shard = match shard {
        Value::Str(h) => get_str(&h),
        other => panic!("derive_stat: arg 0 expected Text, got {:?}", other),
    };
    let depth = match depth {
        Value::Int(n) => n,
        other => panic!("derive_stat: arg 1 expected Int, got {:?}", other),
    };
    let ts = match ts {
        Value::Int(n) => n,
        other => panic!("derive_stat: arg 2 expected Int, got {:?}", other),
    };
    let lat = (now_nanos() - ts).max(0);
    let flush = POOL_STATS.with(|c| {
        let mut s = c.borrow_mut();
        s[0] += 1;
        s[1] += depth;
        if depth > s[2] {
            s[2] = depth;
        }
        s[3] += lat;
        if lat > s[4] {
            s[4] = lat;
        }
        s[5] += 1;
        if s[5] >= FLUSH_EVERY {
            s[5] = 0;
            Some(*s)
        } else {
            None
        }
    });
    if let Some(s) = flush {
        write_pool_stats(&shard, &s);
    }
    Value::Unit
}

/// `derive_stat_flush(shard: Text) -> Unit` — write the partial accumulator out
/// without waiting for the next 1024-item boundary. Called on the pool's idle
/// path (the same place it seals its block), so a run that ends mid-batch still
/// reports what it saw rather than losing the tail.
#[track_caller]
pub fn derive_stat_flush(arg: Value) -> Value {
    // INSERT_PATH_HONESTY_V1 Phase 3 — dial, default OFF. Not a new behaviour:
    // with the dial off derive_stat never bumps the accumulator, so s[0] == 0 and
    // the body below already wrote nothing.
    if !super::tsmark::telemetry_enabled() {
        return Value::Unit;
    }
    let shard = match arg {
        Value::Str(h) => get_str(&h),
        other => panic!("derive_stat_flush: expected Text, got {:?}", other),
    };
    POOL_STATS.with(|c| {
        let s = *c.borrow();
        if s[0] > 0 {
            write_pool_stats(&shard, &s);
        }
    });
    Value::Unit
}

// ── recovery over a raw shard ────────────────────────────────────────────
//
// The kill-9 gate's oracle for ARM A/B. Same handle/open/rebuild/dump shape as
// hotblk_recover.rs so scripts/verify-slice4-kill9.sh needs no new plumbing —
// only a different dumper binary.

struct RawShard {
    raw_dir: String,
    shapes: HashMap<u64, (String, Vec<String>)>,
    pk: HashMap<String, String>,
    hashes: HashSet<String>,
    blocks_scanned: i64,
    frames_scanned: i64,
    undecodable: i64,
}

thread_local! {
    static SHARDS: RefCell<HashMap<i64, RawShard>> = RefCell::new(HashMap::new());
    static NEXT: RefCell<i64> = const { RefCell::new(1) };
}

/// `rawblk_recover_open(raw_dir: Text) -> Int`
#[track_caller]
pub fn rawblk_recover_open(raw_dir: std::sync::Arc<str>) -> Value {
    let raw_dir = raw_dir.to_string();
    // shapes.log sits one level above the per-shard dir (it is process-wide,
    // not per-shard), but tolerate it living beside the blocks too.
    let parent = raw_dir.trim_end_matches('/').rsplit_once('/').map(|(p, _)| p.to_string())
        .unwrap_or_else(|| raw_dir.clone());
    let mut shapes = load_shapes(&parent);
    shapes.extend(load_shapes(&raw_dir));
    let h = NEXT.with(|c| {
        let mut c = c.borrow_mut();
        let n = *c;
        *c += 1;
        n
    });
    SHARDS.with(|s| {
        s.borrow_mut().insert(
            h,
            RawShard {
                raw_dir,
                shapes,
                pk: HashMap::new(),
                hashes: HashSet::new(),
                blocks_scanned: 0,
                frames_scanned: 0,
                undecodable: 0,
            },
        )
    });
    Value::Int(h)
}

/// `rawblk_recover_rebuild(h: Int) -> Int` — frames scanned.
#[track_caller]
pub fn rawblk_recover_rebuild(h: i64) -> Value {
    SHARDS.with(|s| {
        let mut map = s.borrow_mut();
        let sh = map.get_mut(&h).unwrap_or_else(|| panic!("rawblk_recover_rebuild: bad handle {}", h));
        let mut seq = 1i64;
        // Walk block-1.bin, block-2.bin, ... to the first gap — the flushed
        // frontier. A crash before rename leaves the trailing block absent,
        // which is exactly where the walk stops.
        loop {
            let path = format!("{}/block-{}.bin", sh.raw_dir, seq);
            let Ok(data) = std::fs::read(&path) else { break };
            sh.blocks_scanned += 1;
            let shapes = std::mem::take(&mut sh.shapes);
            let mut pk = std::mem::take(&mut sh.pk);
            let mut hashes = std::mem::take(&mut sh.hashes);
            let mut undec = 0i64;
            let n = parse_raw_frames(&data, |kind, _ts, payload| {
                let derived = match kind {
                    b'A' => derive_a(&String::from_utf8_lossy(payload)),
                    b'B' => derive_b(payload, &shapes),
                    _ => None,
                };
                match derived {
                    Some((name, record)) => {
                        let digest = Sha256::digest(record.as_bytes());
                        let hex: String = digest.iter().map(|b| format!("{:02x}", b)).collect();
                        pk.insert(name, format!("sha256:{}", hex));
                        hashes.insert(hex);
                    }
                    None => undec += 1,
                }
            });
            sh.shapes = shapes;
            sh.pk = pk;
            sh.hashes = hashes;
            sh.undecodable += undec;
            sh.frames_scanned += n;
            seq += 1;
        }
        Value::Int(sh.frames_scanned)
    })
}

fn with_shard<T>(h: i64, f: impl FnOnce(&RawShard) -> T) -> T {
    SHARDS.with(|s| {
        let map = s.borrow();
        let sh = map.get(&h).unwrap_or_else(|| panic!("rawblk_recover: bad handle {}", h));
        f(sh)
    })
}

/// `rawblk_recover_stats(h: Int) -> Text` → `"<blocks>\t<frames>\t<undecodable>"`
pub fn rawblk_recover_stats(h: i64) -> Value {
    with_shard(h, |sh| {
        Value::Str(intern_str(&format!(
            "{}\t{}\t{}",
            sh.blocks_scanned, sh.frames_scanned, sh.undecodable
        )))
    })
}

/// `rawblk_recover_dump_pk(h: Int) -> Text` — `"<table:pk>\t<hash>\n"` sorted.
pub fn rawblk_recover_dump_pk(h: i64) -> Value {
    with_shard(h, |sh| {
        let mut keys: Vec<_> = sh.pk.iter().collect();
        keys.sort();
        let mut out = String::new();
        for (k, v) in keys {
            out.push_str(k);
            out.push('\t');
            out.push_str(v);
            out.push('\n');
        }
        Value::Str(intern_str(&out))
    })
}

/// `rawblk_recover_dump_hashes(h: Int) -> Text` — bare hex, one per line, sorted.
pub fn rawblk_recover_dump_hashes(h: i64) -> Value {
    with_shard(h, |sh| {
        let mut hs: Vec<_> = sh.hashes.iter().collect();
        hs.sort();
        let mut out = String::new();
        for x in hs {
            out.push_str(x);
            out.push('\n');
        }
        Value::Str(intern_str(&out))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrips_and_stops_at_a_torn_tail() {
        let mut buf = encode_frame(b'A', 1234, b"INSERT INTO t (a) VALUES ('x')");
        buf.extend_from_slice(&encode_frame(b'A', 5678, b"second"));
        let mut seen: Vec<(u8, i64, Vec<u8>)> = Vec::new();
        let n = parse_raw_frames(&buf, |k, ts, p| seen.push((k, ts, p.to_vec())));
        assert_eq!(n, 2);
        assert_eq!(seen[0].1, 1234);
        assert_eq!(seen[1].2, b"second".to_vec());

        // Truncate mid-second-frame: the first still parses, the torn one does not.
        for cut in 1..(OVERHEAD + 6) {
            let t = &buf[..buf.len() - cut];
            let mut c = 0;
            parse_raw_frames(t, |_, _, _| c += 1);
            assert_eq!(c, 1, "truncating {} bytes should leave exactly 1 frame", cut);
        }
    }

    #[test]
    fn derive_a_matches_the_m1_record_shape() {
        let (name, rec) =
            derive_a("INSERT INTO k9 (id, val) VALUES ('w0_r1', 'payload1')").unwrap();
        assert_eq!(name, "k9:w0_r1");
        assert_eq!(rec, "RECORD\tid=w0_r1\tval=payload1");
    }

    #[test]
    fn derive_b_matches_derive_a_for_the_same_logical_row() {
        let cols = vec!["id".to_string(), "val".to_string()];
        let id = shape_id("k9", &cols);
        let mut shapes = HashMap::new();
        shapes.insert(id, ("k9".to_string(), cols));

        let mut sec = Vec::new();
        sec.extend_from_slice(&2u16.to_be_bytes());
        for v in ["w0_r1", "payload1"] {
            sec.extend_from_slice(&(v.len() as i32).to_be_bytes());
            sec.extend_from_slice(v.as_bytes());
        }
        let mut payload = pad(id, 20).into_bytes();
        payload.extend_from_slice(&sec);

        let (nb, rb) = derive_b(&payload, &shapes).unwrap();
        let (na, ra) =
            derive_a("INSERT INTO k9 (id, val) VALUES ('w0_r1', 'payload1')").unwrap();
        assert_eq!((nb, rb), (na, ra));
    }
}

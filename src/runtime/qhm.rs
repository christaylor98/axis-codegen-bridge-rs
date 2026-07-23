//! BRIDGE_QHM_V1 (AXVERITY_QUERY_HOTMEM_FIX_TRIALS_V1) — a query-dedicated
//! content-hash → record-bytes hot index, built as a MULTI-VARIANT TRIAL
//! ALONGSIDE `contentidx.rs`/`hotmem.rs`/`non_blocking_memory.rs`, which are
//! left completely untouched as the baseline.
//!
//! ## Why this exists (grounded, not assumed)
//!
//! Measured: aggregate reads (`sum`/`avg`/`GROUP BY`/`JOIN`) are super-linear
//! in row count while INSERT and point-lookup are flat. Root cause (traced
//! through `lib/pull_object.m1`): a settled record lives in the WAL tier, so
//! `pull_object` serves every per-matched-row pull from that disk tier —
//! `contentidx` (RAM) sits at tier-3, consulted ONLY after the loose+WAL disk
//! tiers miss, i.e. never on a settled read. So today's RAM index never
//! accelerates the hot aggregate pull. This module is consulted FIRST (tier-0)
//! for immutable content, turning the per-row pull into a RAM lookup.
//!
//! IMPORTANT, measured-not-assumed scope: `COUNT(*)` performs ZERO per-row
//! pulls (it only counts posting-list hashes), so NO record cache can speed it
//! — its cost is the field-index scan + the M1 loop-state O(M²) string rebuild,
//! both explicitly OUT OF SCOPE for this trial. Only the pull-per-row shapes
//! (`SUM`/`AVG`/`GROUP BY`/`JOIN`, and `SELECT *`) can benefit here.
//!
//! ## Variants (runtime-selected by `AXVERITY_QHM_VARIANT`, no rebuild between)
//!
//!   off    — inert: `qhm_get` always misses, `qhm_put` no-ops. The read path
//!            then falls through to the existing tiers UNCHANGED, so this IS the
//!            pre-qhm baseline. (`contentidx` remains populated at its tier-3
//!            position.) Selectable fallback; NO LONGER the default.
//!   mutex  — RAM-FIRST control: a sharded `Mutex<HashMap>` + FIFO, structurally
//!            identical to `contentidx` but consulted first and independently
//!            capped. Isolates the "RAM-first reorder" effect from the
//!            "lock-free engine" effect — NOT lock-free (writes and reads both
//!            take the shard mutex briefly). Selectable fallback; not default.
//!   lfa    — LOCK-FREE reads via `non_blocking_memory`, seal-then-reclaim with
//!            PER-SEAL reclaim (fine granularity). Selectable fallback; not default.
//!   lfb    — **SHIPPED DEFAULT.** Same lock-free engine as `lfa`, seal-then-reclaim
//!            with WATERMARK/epoch-BATCHED reclaim (`reclaim_if_watermark`, coarse
//!            granularity — fewer floor scans for identical memory at these scales).
//!
//! `lfa`/`lfb` differ ONLY in the reclaim-trigger granularity — the trial's
//! stated `unknown` (per-block vs per-shard-epoch vs per-N-blocks). They share
//! all read/write code, so a losing one is a single-line discard.
//!
//! ## Shipped default & fallbacks (AXVERITY_QHM_SHIP_LOCKFREE_V1)
//!
//! The default (env unset) is `lfb` — lock-free reads by default. The trial
//! (`docs/turn-qhm-fix-trials.md`) measured `mutex ≈ lfa ≈ lfb` in throughput at
//! the tested concurrency (K=8, 256 shards, sub-µs critical section). That
//! equivalence is precisely WHY the default is non-blocking: with no measured
//! throughput cost to lock-freedom, there is no load-bearing reason to make the
//! default read path take a lock. `lfb` over `lfa` because watermark-batched
//! reclaim does fewer floor scans for the same memory (within noise today, but
//! the strictly-cheaper reclaim policy).
//!
//! `off`/`mutex`/`lfa` are kept in-tree and remain explicitly selectable
//! (supersede-don't-delete). Reach for them only as escape hatches:
//!   * `off`   — revert to exact pre-qhm read behavior (disk-tier-only) if the
//!               hot index ever needs to be taken out of the path for triage.
//!   * `mutex` — a blocking RAM-first control; useful only to A/B whether a
//!               future regression is lock-free-engine-specific vs the reorder.
//!               NOT recommended as a running default: it is strictly a
//!               comparison/fallback artifact, equal-or-slower with no upside.
//!   * `lfa`   — per-seal reclaim, if a future workload ever shows `lfb`'s
//!               batched reclaim retaining too much transient memory.
//!
//! ## Cap sizing — REQUIRED operator input, not a silent default (see below)
//!
//! The whole win is conditional on the resident cap covering the query working
//! set. Trial finding: with `AXVERITY_QHM_CAP=512` against a ~4000-row working
//! set (≈8× over cap), the pull-heavy-aggregate win COLLAPSED from 17–33× to
//! ~13%, because most matched rows get evicted and fall back to the disk tiers.
//! Sizing rule of thumb:
//!
//!     AXVERITY_QHM_CAP  ≳  peak number of DISTINCT rows touched by a single
//!                          aggregate/JOIN/SELECT * query (its working set)
//!
//! The default (65536) comfortably covers the low-thousands-row regime these
//! shapes run at today; raise it for larger working sets, and keep headroom (the
//! cap is a hard resident ceiling — at/over it you are in the ~13% regime, not
//! the 17–33× one). Full guidance: `docs/turn-qhm-ship-lockfree.md`.
//!
//! ## Soundness (the load-bearing bit)
//!
//! The engine's reader-floor protocol protects ONLY the single HEAD block a
//! reader pins — walking the `prev`-chain to older blocks would race reclaim
//! (UAF). So the lock-free variants publish an IMMUTABLE `Arc<SegSet>` as the
//! cell's single head value. A read pins the head (one block deref — the
//! proven-sound `hotmem`/`read_ref` pattern), clones the `Arc` out UNDER the
//! pin, unpins, then walks the segment maps via ordinary `Arc` lifetimes — no
//! raw-pointer chain walk. Writes serialise through a brief per-shard
//! `Mutex<Writer>` (the engine REQUIRES a single writer per cell; the mutex
//! makes "one writer at a time" a fact even with N worker threads). Reads never
//! touch that mutex. Per-variant concurrent write/read probes (`bad == 0`, byte
//! content validated) mirror `hotmem`'s `uaf_isolation_probe` and gate
//! viability. The reused hazard-pointer protocol is separately modelled by
//! `tests/loom_arena.rs`.
//!
//! ## Tuning (all env, `OnceLock`-cached, contentidx pattern)
//!   AXVERITY_QHM_VARIANT   off|mutex|lfa|lfb           (default lfb — shipped)
//!   AXVERITY_QHM_CAP       total resident entry cap    (default 65536; size ≳ working set)
//!   AXVERITY_QHM_BLOCK     per-shard seal threshold     (default per_shard_cap/4)
//! A `qhm_flush` primitive seals every shard's pending batch — call it after a
//! seed and before timed reads so the whole working set is resident (a real
//! system would seal on idle). Between seals the just-written entries are
//! writer-private (invisible to readers) — that recent-write window is still
//! covered by the untouched loose/WAL/`contentidx` tiers, so a miss here is
//! never a wrong answer, only a slower (disk) pull.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::non_blocking_memory::{BridgedCell, ReaderHandle, ReaderRegistry, Writer};
use super::value::{get_str, Value};

const NSHARDS: usize = 256; // power of two; keyed by fnv1a(hash), same as contentidx
const CAP_DEFAULT: usize = 65536;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Variant {
    Off,
    Mutex,
    LfA,
    LfB,
}

fn variant() -> Variant {
    static V: OnceLock<Variant> = OnceLock::new();
    *V.get_or_init(|| {
        // SHIPPED DEFAULT (AXVERITY_QHM_SHIP_LOCKFREE_V1): unset/empty/unknown ->
        // `lfb` (lock-free reads, watermark-batched reclaim). This is the shipped
        // engine: the 17-33x pull-heavy-aggregate win is live on the standard
        // path with no env set. `off`/`mutex`/`lfa` remain explicitly selectable
        // as non-default fallbacks (see module doc "Shipped default & fallbacks").
        // No blocking primitive is introduced by this flip — the default arm only
        // reselects an already-present variant; `lfb`'s reads are lock-free and
        // its sole Mutex (per-shard `Mutex<Writer>`) predates this turn and is
        // justified in the LOCK-FREE backend header (single-writer contract).
        match std::env::var("AXVERITY_QHM_VARIANT")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "off" => Variant::Off,
            "mutex" => Variant::Mutex,
            "lfa" => Variant::LfA,
            "lfb" => Variant::LfB,
            _ => Variant::LfB,
        }
    })
}

fn total_cap() -> usize {
    static C: OnceLock<usize> = OnceLock::new();
    *C.get_or_init(|| {
        std::env::var("AXVERITY_QHM_CAP")
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

fn block_size() -> usize {
    static B: OnceLock<usize> = OnceLock::new();
    *B.get_or_init(|| {
        std::env::var("AXVERITY_QHM_BLOCK")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|&n| n >= 1)
            .unwrap_or_else(|| (per_shard_cap() / 4).max(1))
    })
}

#[inline]
fn shard_of(key: &str) -> usize {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in key.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (h as usize) & (NSHARDS - 1)
}

// ===========================================================================
// AXVERITY_ZEROCOPY_READPATH_BUILD_V1 — RETURN-PATH variants (sites 1/2/3 of
// design:axverity-readpath-gap-is-representation-not-guarantee). All are
// return/materialization-path only: the hash-keyed lookup, sharding, and
// seal-then-reclaim discipline are UNTOUCHED. Two independent env flags,
// OnceLock-cached, so `off`/`clone` (both unset) is byte-identical to the
// pre-turn path and remains the always-available fallback.
//
//   AXVERITY_QHM_RETURN  off (default) | revalidate | arc
//     off        — pull_object uses the Bytes path (`bytes_to_text(qhm_get)`);
//                  segments store `Stored::Bytes`. The exact baseline.
//     revalidate — pull_object uses `qhm_get_text`; segments still store
//                  `Stored::Bytes`; the read validates the Arc-wrapped bytes
//                  IN PLACE and builds one Arc<str>. REDUCED-COPY: removes the
//                  segment->Vec deep copy (qhm.rs:396) and the intermediate
//                  M1 `Value::Bytes`, but still validates per read. Always safe
//                  (no stored-content change).
//     arc        — pull_object uses `qhm_get_text`; segments store
//                  `Stored::Text(Arc<str>)`, VALIDATED ONCE at put. The read is
//                  `Arc::clone` — a genuine zero-copy SHARED borrow handed to
//                  M1 across worker threads (Arc<str> is Send+Sync + immutable).
//                  Folds in site 3 (validate-once): safe because the ONLY qhm
//                  write path is pg_exec_insert -> text_to_bytes(record), always
//                  valid UTF-8 (site-3 audit). A non-UTF-8 put (never on the
//                  audited path) falls back to Stored::Bytes defensively, so a
//                  read can never observe a wrong/torn value.
//
//   AXVERITY_QHM_KEY     clone (default) | borrow            (site 2 / get_str)
//     clone  — the baseline `get_str(&h)`: one owned String allocation of the
//              ~71-char address per get.
//     borrow — pass the Value::Str's Arc<str> as a borrowed &str straight to the
//              shard lookup: zero allocation on the key. get_str's OWN signature
//              can't cheaply change (~200 callers want an owned String), so this
//              is the reachable shape at the hot site (reported, not assumed).
// ===========================================================================

// AXVERITY_WAY_BACK_CONSOLIDATION_V1: return_mode() is hardcoded Off (QHM_RETURN switch removed);
// Revalidate/Shared are retained only by the staged-for-deletion Text-representation code.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ReturnMode {
    Off,
    Revalidate,
    Shared, // "arc": stored Arc<str>, returned by cheap clone
}

// AXVERITY_WAY_BACK_CONSOLIDATION_V1: the QHM_RETURN switch is removed — §32 measured the
// zero-copy return variants (arc/revalidate) within noise of the baseline (the record copy is
// ~0.3% of per-row cost). Always Off (baseline Bytes storage). AXVERITY_QHM_RETURN is no longer
// read. (The residual Text-representation code + its tests + the orphaned qhm_get_text/
// qhm_return_mode entry points are staged for a follow-up cleanup pass, kept intact here so this
// switch removal does not refactor the qhm=lfb winner's storage/tests.)
fn return_mode() -> ReturnMode {
    ReturnMode::Off
}

// AXVERITY_WAY_BACK_CONSOLIDATION_V1: key_mode() is hardcoded Clone (QHM_KEY switch removed);
// Borrow is retained only by the match arms staged for deletion with the Text-representation code.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum KeyMode {
    Clone,
    Borrow,
}

// AXVERITY_WAY_BACK_CONSOLIDATION_V1: the QHM_KEY switch is removed — §32 measured the borrow
// (zero-alloc key) variant within noise. Always Clone (baseline get_str). AXVERITY_QHM_KEY is no
// longer read.
fn key_mode() -> KeyMode {
    KeyMode::Clone
}

/// An immutable stored record value in the lock-free backend. The
/// representation is chosen ONCE at put time by `return_mode()`:
///   * `Bytes(Arc<[u8]>)` — off/revalidate: an Arc-wrapped copy of the record
///     bytes. `to_bytes()` is `(*arc).to_vec()`, byte-identical and same-cost as
///     the historical `Vec<u8>` clone the reader used to do.
///   * `Text(Arc<str>)`   — arc: the record validated once as UTF-8 and shared.
///     `to_text()` is `Arc::clone` — zero-copy.
/// This is a container/representation choice for the RETURN path; the record
/// CONTENT (bytes), content-addressing, and hash-keying are unchanged.
#[derive(Clone)]
enum Stored {
    Bytes(Arc<[u8]>),
    Text(Arc<str>),
}

impl Stored {
    /// Build the stored form for `bytes` under the active return mode. In `arc`
    /// mode validate-once here (site 3); any non-UTF-8 input (never on the
    /// audited put path) falls back to Bytes so reads still serve it.
    fn build(bytes: Vec<u8>) -> Stored {
        if return_mode() == ReturnMode::Shared {
            match String::from_utf8(bytes) {
                Ok(s) => Stored::Text(Arc::from(s)),
                Err(e) => Stored::Bytes(Arc::from(e.into_bytes().into_boxed_slice())),
            }
        } else {
            Stored::Bytes(Arc::from(bytes.into_boxed_slice()))
        }
    }

    /// The record bytes as an owned Vec (the `qhm_get` / Bytes-return path). One
    /// copy — byte-identical to the historical behavior regardless of storage.
    fn to_bytes(&self) -> Vec<u8> {
        match self {
            Stored::Bytes(b) => b.to_vec(),
            Stored::Text(s) => s.as_bytes().to_vec(),
        }
    }

    /// A shared `Arc<str>` view of the record (the `qhm_get_text` path).
    ///   * `Text` => `Arc::clone` (zero-copy shared borrow).
    ///   * `Bytes` => validate UTF-8 + build one Arc<str> (reduced-copy). `None`
    ///     on invalid UTF-8 so the caller falls through to the disk tier (never
    ///     a panic, never a wrong answer).
    fn to_text(&self) -> Option<Arc<str>> {
        match self {
            Stored::Text(s) => Some(Arc::clone(s)),
            Stored::Bytes(b) => std::str::from_utf8(b).ok().map(super::value::intern_str),
        }
    }
}

// ===========================================================================
// MUTEX backend — RAM-first control. A private copy of contentidx's shape
// (sharded Mutex<HashMap> + FIFO). Deliberately NOT sharing contentidx's
// statics: this must be independently capped and consulted-first, and the
// intent forbids touching contentidx.rs.
// ===========================================================================
struct MShard {
    map: HashMap<String, Vec<u8>>,
    fifo: VecDeque<String>,
}
struct MIdx {
    shards: Vec<Mutex<MShard>>,
}
fn midx() -> &'static MIdx {
    static I: OnceLock<MIdx> = OnceLock::new();
    I.get_or_init(|| MIdx {
        shards: (0..NSHARDS)
            .map(|_| {
                Mutex::new(MShard {
                    map: HashMap::new(),
                    fifo: VecDeque::new(),
                })
            })
            .collect(),
    })
}
fn m_put(hash: String, bytes: Vec<u8>) {
    let cap = per_shard_cap();
    let s = shard_of(&hash);
    let mut g = midx().shards[s].lock().unwrap_or_else(|p| p.into_inner());
    if g.map.contains_key(&hash) {
        return; // insert-if-absent: content is immutable
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
}
fn m_get(hash: &str) -> Vec<u8> {
    let s = shard_of(hash);
    let g = midx().shards[s].lock().unwrap_or_else(|p| p.into_inner());
    g.map.get(hash).cloned().unwrap_or_default()
}

// ===========================================================================
// LOCK-FREE backend — lfa/lfb. Reads are lock-free (single head-block pin +
// Arc clone-out); writes serialise through a brief per-shard Mutex<Writer>.
// ===========================================================================

/// An immutable sealed batch of `hash -> Stored`. Never mutated after seal.
type Segment = HashMap<String, Stored>;

/// The single head value published in a shard's `BridgedCell`. Immutable; a
/// reader clones this whole `Arc` out under the head pin, then walks `segs`
/// (newest first) via ordinary `Arc` lifetimes.
struct SegSet {
    segs: Vec<Arc<Segment>>, // newest first
    total: usize,            // sum of segment lengths (for cap-driven eviction)
}

struct LfWriter {
    writer: Writer<Arc<SegSet>>,
    pending: HashMap<String, Stored>, // writer-private, unsealed
}

struct LfShard {
    cell: Arc<BridgedCell<Arc<SegSet>>>, // readers use this; lock-free
    wr: Mutex<LfWriter>,                 // writers lock this; readers never do
}

struct LfIdx {
    shards: Vec<LfShard>,
    reg: ReaderRegistry, // one global registry (worker pool << MAX_READERS=64)
    epoch: AtomicU64,    // global monotonic block epoch, unique across shards
}

fn lfidx() -> &'static LfIdx {
    static I: OnceLock<LfIdx> = OnceLock::new();
    I.get_or_init(|| {
        let shards = (0..NSHARDS)
            .map(|_| {
                let (cell, writer) = BridgedCell::<Arc<SegSet>>::new();
                LfShard {
                    cell,
                    wr: Mutex::new(LfWriter {
                        writer,
                        pending: HashMap::new(),
                    }),
                }
            })
            .collect();
        LfIdx {
            shards,
            reg: ReaderRegistry::new(),
            epoch: AtomicU64::new(0),
        }
    })
}

thread_local! {
    /// This thread's reader slot, lazily acquired. `ReaderHandle`'s Drop
    /// releases the floor on thread exit. `None` until first read.
    static QHM_READER: RefCell<Option<ReaderHandle<'static>>> = const { RefCell::new(None) };
}

/// Seal the shard's pending batch into a new immutable segment, publish a new
/// `SegSet` head, evict oldest segments past the cap, and reclaim per the
/// selected granularity. Caller MUST hold `w` (the shard write lock) — that is
/// what upholds the engine's single-writer contract.
fn lf_seal(w: &mut LfWriter, v: Variant) {
    if w.pending.is_empty() {
        return;
    }
    let seg = Arc::new(std::mem::take(&mut w.pending));
    // Owned read of the current head under the write lock. Sound per the F2
    // rule: we hold the sole writer capability and reclaim runs only under this
    // same lock, so no concurrent reclamation of this cell can occur.
    let cur = w.writer.cell().read(0).value();
    let mut segs: Vec<Arc<Segment>> = Vec::with_capacity(1 + cur.as_ref().map_or(0, |c| c.segs.len()));
    let mut total = seg.len();
    segs.push(seg);
    if let Some(cur) = cur {
        for s in &cur.segs {
            segs.push(Arc::clone(s));
            total += s.len();
        }
    }
    // Cap-driven eviction: drop oldest sealed segments while over capacity
    // (keep at least the just-sealed one). Dropping the Arc here is the
    // "reclaim" of evicted content; live readers holding an older SegSet keep
    // their copy alive independently (ordinary Arc lifetime).
    let cap = per_shard_cap();
    while total > cap && segs.len() > 1 {
        if let Some(old) = segs.pop() {
            total -= old.len();
        }
    }
    let epoch = lfidx().epoch.fetch_add(1, Ordering::SeqCst) + 1; // strictly increasing, > 0
    let new_set = Arc::new(SegSet { segs, total });
    // SAFETY: single writer per cell — guaranteed by holding `w` (the shard
    // write mutex). `epoch` is globally monotonic, hence strictly greater than
    // any epoch previously published for THIS cell, satisfying
    // write_with_epoch's contract.
    unsafe {
        w.writer.write_with_epoch(new_set, epoch);
    }
    match v {
        Variant::LfA => {
            // Fine-grained: attempt to free the just-retired container block
            // immediately on every seal.
            w.writer.reclaim(&lfidx().reg);
        }
        Variant::LfB => {
            // Coarse: let retired blocks accumulate to the engine's watermark,
            // then sweep in a batch with exponential back-off on starved floors.
            w.writer.reclaim_if_watermark(&lfidx().reg);
        }
        _ => {}
    }
}

fn lf_put(hash: String, bytes: Vec<u8>, v: Variant) {
    let s = shard_of(&hash);
    let sh = &lfidx().shards[s];
    let mut w = sh.wr.lock().unwrap_or_else(|p| p.into_inner());
    // Insert-if-absent within the pending batch (content is immutable). We do
    // NOT scan published segments here to keep the write O(1); a re-insert of
    // an already-sealed hash costs one duplicate entry (harmless: reads are
    // newest-first) and is vanishingly rare for content-addressed INSERTs.
    if w.pending.contains_key(&hash) {
        return;
    }
    w.pending.insert(hash, Stored::build(bytes));
    if w.pending.len() >= block_size() {
        lf_seal(&mut w, v);
    }
}

/// Pin the head, clone the `Arc<SegSet>` out UNDER the pin, then unpin, and walk
/// the (now Arc-owned) segments newest-first applying `f` to the found `Stored`.
/// The pin/clone/unpin shape is IDENTICAL to the proven baseline — only what we
/// extract from the `Stored` differs (bytes vs shared text). Returns `f`'s
/// result, or `miss` when the key is absent / the cell is uninitialised.
#[inline]
fn lf_lookup<T>(hash: &str, miss: T, f: impl FnOnce(&Stored) -> T) -> T {
    let s = shard_of(hash);
    let sh = &lfidx().shards[s];
    QHM_READER.with(|r| {
        if r.borrow().is_none() {
            // Worker pool (~10 threads) << MAX_READERS (64); acquire will not
            // exhaust in practice. Documented bound.
            *r.borrow_mut() = Some(lfidx().reg.acquire());
        }
        let borrowed = r.borrow();
        let handle = borrowed.as_ref().expect("qhm reader handle just acquired");
        // Single-block deref — the only sound shape (see module doc).
        let set: Arc<SegSet> = match sh.cell.read_ref(handle, 0) {
            None => return miss,
            Some(read_ref) => (*read_ref).clone(),
        };
        for seg in &set.segs {
            if let Some(st) = seg.get(hash) {
                return f(st);
            }
        }
        miss
    })
}

fn lf_get(hash: &str, _v: Variant) -> Vec<u8> {
    lf_lookup(hash, Vec::new(), Stored::to_bytes)
}

/// Shared-text read (`qhm_get_text`). `Text` storage => `Arc::clone` (zero-copy);
/// `Bytes` storage => validate + one Arc<str>. `None` on miss OR invalid UTF-8.
fn lf_get_text(hash: &str) -> Option<Arc<str>> {
    lf_lookup(hash, None, Stored::to_text)
}

fn lf_flush(v: Variant) {
    for sh in &lfidx().shards {
        let mut w = sh.wr.lock().unwrap_or_else(|p| p.into_inner());
        lf_seal(&mut w, v);
    }
}

// ===========================================================================
// M1-facing bridge entry points (dispatched by AXVERITY_QHM_VARIANT)
// ===========================================================================

/// `qhm_put(hash: Text, bytes: Bytes) -> Unit` — publish a record's bytes under
/// its content hash into the selected variant's index. Insert-if-absent (content
/// immutable). No fsync, no disk I/O. `off` no-ops.
#[track_caller]
pub fn qhm_put(args: Value) -> Value {
    let v = variant();
    if v == Variant::Off {
        return Value::Unit;
    }
    let (hash, bytes) = match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("qhm_put: expected Tuple(Text, Bytes), got {:?}", other),
    };
    let hash = match hash {
        Value::Str(h) => get_str(&h),
        other => panic!("qhm_put: arg 0 expected Text hash, got {:?}", other),
    };
    let bytes = match bytes {
        Value::Bytes(b) => b,
        other => panic!("qhm_put: arg 1 expected Bytes, got {:?}", other),
    };
    match v {
        Variant::Off => {}
        Variant::Mutex => m_put(hash, bytes),
        Variant::LfA | Variant::LfB => lf_put(hash, bytes, v),
    }
    Value::Unit
}

/// Dispatch a bytes-returning lookup by variant.
#[inline]
fn get_bytes_by(hash: &str, v: Variant) -> Vec<u8> {
    match v {
        Variant::Off => Vec::new(),
        Variant::Mutex => m_get(hash),
        Variant::LfA | Variant::LfB => lf_get(hash, v),
    }
}

/// `qhm_get(hash: Text) -> Bytes` — the record's bytes, or empty Bytes on a miss
/// (caller falls through to the existing durable tiers). `off` always misses.
/// The key (address) is read per `AXVERITY_QHM_KEY`: `clone` (baseline
/// `get_str`, one owned String) or `borrow` (the Arc<str>'s &str, zero alloc).
#[track_caller]
pub fn qhm_get(arg: Value) -> Value {
    let v = variant();
    if v == Variant::Off {
        return Value::Bytes(Vec::new());
    }
    let bytes = match arg {
        Value::Str(h) => match key_mode() {
            KeyMode::Borrow => get_bytes_by(h.as_ref(), v),
            KeyMode::Clone => get_bytes_by(&get_str(&h), v),
        },
        other => panic!("qhm_get: expected Text hash, got {:?}", other),
    };
    Value::Bytes(bytes)
}

/// `qhm_get_text(hash: Text) -> Text` — the record's bytes AS TEXT (a shared
/// Arc<str>), or "" on a miss / invalid-UTF-8 (caller routes to the disk tiers).
/// This is the RETURN-PATH variant entry (AXVERITY_ZEROCOPY_READPATH_BUILD_V1):
/// it fuses the old `bytes_to_text(qhm_get(..))` two-hop into one, and under
/// `AXVERITY_QHM_RETURN=arc` returns a zero-copy `Arc::clone` of the once-
/// validated stored text. `off` never routes here (pull_object uses the Bytes
/// path); it is defined for all variants so a direct caller is well-behaved.
#[track_caller]
pub fn qhm_get_text(arg: Value) -> Value {
    let v = variant();
    let text: Option<Arc<str>> = match arg {
        Value::Str(h) => {
            let get = |k: &str| -> Option<Arc<str>> {
                match v {
                    Variant::Off => None,
                    Variant::Mutex => {
                        let b = m_get(k);
                        if b.is_empty() {
                            None
                        } else {
                            std::str::from_utf8(&b).ok().map(super::value::intern_str)
                        }
                    }
                    Variant::LfA | Variant::LfB => lf_get_text(k),
                }
            };
            match key_mode() {
                KeyMode::Borrow => get(h.as_ref()),
                KeyMode::Clone => get(&get_str(&h)),
            }
        }
        other => panic!("qhm_get_text: expected Text hash, got {:?}", other),
    };
    // "" is the established miss sentinel (records are never empty; pull_object
    // routes an empty result to the durable tiers).
    Value::Str(text.unwrap_or_else(|| super::value::intern_str("")))
}

/// `qhm_flush(_: Unit) -> Unit` — seal every shard's pending batch so the whole
/// written working set is resident before timed reads. No-op for off/mutex.
#[track_caller]
pub fn qhm_flush(_arg: Value) -> Value {
    let v = variant();
    if let Variant::LfA | Variant::LfB = v {
        lf_flush(v);
    }
    Value::Unit
}

/// `qhm_return_mode(_: Unit) -> Text` — "off" | "revalidate" | "arc". Lets
/// `pull_object` choose the Bytes path (off, exact baseline) vs the fused
/// `qhm_get_text` path (revalidate/arc). AXVERITY_QHM_RETURN, read once.
#[track_caller]
pub fn qhm_return_mode(_arg: Value) -> Value {
    let s = match return_mode() {
        ReturnMode::Off => "off",
        ReturnMode::Revalidate => "revalidate",
        ReturnMode::Shared => "arc",
    };
    Value::Str(super::value::intern_str(s))
}

/// `qhm_stats(_: Unit) -> Text` — diagnostic snapshot for the reclaim-pressure
/// micro-benchmark: `variant=<v> retired=<sum> segs=<sum> entries=<sum>`.
/// `retired` is the total across shards of engine blocks retired-but-not-freed
/// (the memory the reclaim policy has not yet returned).
#[track_caller]
pub fn qhm_stats(_arg: Value) -> Value {
    let v = variant();
    let s = match v {
        Variant::Off => "variant=off".to_string(),
        Variant::Mutex => {
            let mut entries = 0usize;
            for sh in &midx().shards {
                let g = sh.lock().unwrap_or_else(|p| p.into_inner());
                entries += g.map.len();
            }
            format!("variant=mutex retired=0 segs=0 entries={}", entries)
        }
        Variant::LfA | Variant::LfB => {
            let name = if v == Variant::LfA { "lfa" } else { "lfb" };
            let (mut retired, mut segs, mut entries) = (0usize, 0usize, 0usize);
            for sh in &lfidx().shards {
                let w = sh.wr.lock().unwrap_or_else(|p| p.into_inner());
                retired += w.writer.retired_len();
                entries += w.pending.len();
                if let Some(cur) = w.writer.cell().read(0).value() {
                    segs += cur.segs.len();
                    entries += cur.total;
                }
            }
            format!(
                "variant={} retired={} segs={} entries={}",
                name, retired, segs, entries
            )
        }
    };
    Value::Str(super::value::intern_str(&s))
}

// Tests share the process-global `static` lf arena, and `cargo test` runs test
// fns in parallel. Exact-presence assertions (the deterministic seal test, the
// stored_* round-trips) can be spuriously evicted by a concurrent probe churning
// cap-eviction on the same shard — a pre-existing shared-static hazard (the arena
// is a static), worsened by this turn's added arc probes. This lock serialises
// only the ARENA-SENSITIVE tests against each other; it changes no assertion and
// preserves each probe's OWN internal reader/writer thread concurrency.
#[cfg(test)]
static ARENA_TEST_SERIAL: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::value::intern_str;

    // NOTE: variant() is process-global (OnceLock) and env-driven, so these
    // tests exercise the backends DIRECTLY (m_*/lf_* functions) rather than
    // through the env dispatch — that keeps them independent of the ambient
    // AXVERITY_QHM_VARIANT and runnable in one `cargo test` process.

    #[test]
    fn mutex_put_get_and_miss() {
        assert_eq!(m_get("qhm_mtest:absent"), Vec::<u8>::new());
        m_put("sha256:m_aaa".into(), b"RECORD\tk=v".to_vec());
        assert_eq!(m_get("sha256:m_aaa"), b"RECORD\tk=v".to_vec());
    }

    #[test]
    fn mutex_insert_if_absent() {
        m_put("sha256:m_immut".into(), b"first".to_vec());
        m_put("sha256:m_immut".into(), b"second".to_vec());
        assert_eq!(m_get("sha256:m_immut"), b"first".to_vec());
    }

    #[test]
    fn lf_put_flush_get_and_miss() {
        let _serial = ARENA_TEST_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        assert_eq!(lf_get("qhm_lftest:absent", Variant::LfA), Vec::<u8>::new());
        lf_put("sha256:lf_aaa".into(), b"RECORD\tk=v".to_vec(), Variant::LfA);
        // Not yet sealed (pending < block_size) — still findable via flush.
        lf_flush(Variant::LfA);
        assert_eq!(lf_get("sha256:lf_aaa", Variant::LfA), b"RECORD\tk=v".to_vec());
    }

    #[test]
    fn lf_seals_at_block_threshold_without_flush() {
        // Drive one shard past block_size so it auto-seals, then confirm the
        // sealed entries are readable without an explicit flush.
        let _serial = ARENA_TEST_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let target = shard_of("seedkey");
        let bs = block_size();
        let mut keys = Vec::new();
        let mut i = 0u64;
        while keys.len() < bs {
            let k = format!("lfseal:{}", i);
            if shard_of(&k) == target {
                keys.push(k);
            }
            i += 1;
        }
        for (n, k) in keys.iter().enumerate() {
            lf_put(k.clone(), format!("v{}", n).into_bytes(), Variant::LfB);
        }
        // The bs-th insert triggers a seal; all bs entries must be readable.
        for (n, k) in keys.iter().enumerate() {
            assert_eq!(
                lf_get(k, Variant::LfB),
                format!("v{}", n).into_bytes(),
                "sealed key {} should be readable",
                k
            );
        }
    }

    #[test]
    fn stored_text_roundtrip_bytes_and_text() {
        let _serial = ARENA_TEST_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        // Force Text storage (the arc-mode representation), independent of the
        // ambient AXVERITY_QHM_RETURN env: a Text-stored entry reads back
        // zero-copy via lf_get_text AND byte-identically via lf_get.
        let k = "sha256:qhm_text_rt";
        {
            let shard = shard_of(k);
            let sh = &lfidx().shards[shard];
            let mut w = sh.wr.lock().unwrap_or_else(|p| p.into_inner());
            w.pending.insert(k.to_string(), Stored::Text(Arc::from("RECORD\tk=v")));
            lf_seal(&mut w, Variant::LfB);
        }
        assert_eq!(lf_get_text(k).as_deref(), Some("RECORD\tk=v"));
        assert_eq!(lf_get(k, Variant::LfB), b"RECORD\tk=v".to_vec());
    }

    #[test]
    fn stored_bytes_revalidate_to_text_and_bad_utf8() {
        let _serial = ARENA_TEST_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        // Default build() => Bytes storage (ambient env off); lf_get_text must
        // validate + return text (the revalidate variant).
        let k = "sha256:qhm_bytes_reval";
        lf_put(k.to_string(), b"RECORD\tc=1".to_vec(), Variant::LfB);
        lf_flush(Variant::LfB);
        assert_eq!(lf_get_text(k).as_deref(), Some("RECORD\tc=1"));
        // Invalid UTF-8 stored as Bytes => text read is a clean None (caller
        // routes to the disk tier), while the bytes read still serves it.
        let kb = "sha256:qhm_bad_utf8";
        lf_put(kb.to_string(), vec![0xff, 0xfe, 0x00], Variant::LfB);
        lf_flush(Variant::LfB);
        assert_eq!(lf_get_text(kb), None);
        assert_eq!(lf_get(kb, Variant::LfB), vec![0xff, 0xfe, 0x00]);
    }

    #[test]
    fn intern_roundtrip_helper_available() {
        // sanity: value interner is reachable (used by qhm_stats)
        let h = intern_str("sha256:x");
        assert_eq!(get_str(&h), "sha256:x");
    }
}

// ===========================================================================
// SOUNDNESS PROBES — the viability gate for the lock-free variants.
//
// Mirrors hotmem.rs::uaf_isolation_probe: pure concurrent write/read against
// this module's own arena, content-validated (bad == 0). A variant that is
// fast but trips these is a FAILED variant, not a fast option with a caveat.
// The reused hazard-pointer reclamation protocol is additionally modelled by
// tests/loom_arena.rs.
// ===========================================================================
#[cfg(test)]
mod soundness_probe {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtoOrd};
    use std::sync::Arc as StdArc;
    use std::thread;

    // Bounded, Miri-friendly: fixed iteration counts, content-validated. One
    // writer thread seals repeatedly; several reader threads pin/clone/walk
    // concurrently. Any freed-under-a-live-read block would surface as a
    // corrupted (wrong-shape or absent-when-present) read.
    fn concurrent_probe(v: Variant) {
        let _serial = super::ARENA_TEST_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        const WRITES: u64 = if cfg!(miri) { 60 } else { 40_000 };
        const NKEYS: u64 = 64; // small keyspace so reads frequently hit live entries
        let done = StdArc::new(AtomicBool::new(false));
        let bad = StdArc::new(AtomicUsize::new(0));

        let w_done = done.clone();
        let writer = thread::spawn(move || {
            for i in 0..WRITES {
                let k = format!("probe:{}:{}", v as u8, i % NKEYS);
                // value uniquely encodes the key so a torn/aliased read is
                // detectable: "V-<key>-END".
                let val = format!("V-{}-END", k).into_bytes();
                match v {
                    Variant::LfA | Variant::LfB => lf_put(k, val, v),
                    _ => unreachable!(),
                }
                if i % 8 == 0 {
                    // periodic flush so readers see fresh entries and seals
                    // interleave with reads.
                    lf_flush(v);
                }
            }
            lf_flush(v);
            w_done.store(true, AtoOrd::Release);
        });

        let mut readers = Vec::new();
        for _ in 0..3 {
            let r_done = done.clone();
            let r_bad = bad.clone();
            readers.push(thread::spawn(move || loop {
                for kk in 0..NKEYS {
                    let k = format!("probe:{}:{}", v as u8, kk);
                    let b = lf_get(&k, v);
                    if !b.is_empty() {
                        // must be exactly "V-<k>-END" for THIS key
                        let ok = match String::from_utf8(b) {
                            Ok(s) => s == format!("V-{}-END", k),
                            Err(_) => false,
                        };
                        if !ok {
                            r_bad.fetch_add(1, AtoOrd::Relaxed);
                        }
                    }
                }
                if r_done.load(AtoOrd::Acquire) {
                    break;
                }
            }));
        }

        writer.join().unwrap();
        for r in readers {
            r.join().unwrap();
        }
        assert_eq!(
            bad.load(AtoOrd::Relaxed),
            0,
            "corrupted/aliased reads observed for {:?}",
            v
        );
    }

    // The genuinely SHARED (not thread-local) zero-copy pattern this turn adds:
    // the writer seals `Stored::Text` entries; readers clone the `Arc<str>` out
    // ACROSS threads under concurrent seal/reclaim. A freed-under-live-read block
    // or a torn Arc would surface as wrong-content text. This is the per-variant
    // concurrency gate the intent requires for the shared-borrow (arc) variant —
    // it is NOT assumed sound by analogy to the bytes probe.
    fn arc_text_probe(v: Variant) {
        let _serial = super::ARENA_TEST_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        const WRITES: u64 = if cfg!(miri) { 60 } else { 40_000 };
        const NKEYS: u64 = 64;
        let done = StdArc::new(AtomicBool::new(false));
        let bad = StdArc::new(AtomicUsize::new(0));

        let w_done = done.clone();
        let writer = thread::spawn(move || {
            for i in 0..WRITES {
                let k = format!("arcprobe:{}:{}", v as u8, i % NKEYS);
                let val = format!("V-{}-END", k);
                let shard = shard_of(&k);
                {
                    let sh = &lfidx().shards[shard];
                    let mut w = sh.wr.lock().unwrap_or_else(|p| p.into_inner());
                    if !w.pending.contains_key(&k) {
                        w.pending.insert(k.clone(), Stored::Text(Arc::from(val.as_str())));
                    }
                    if w.pending.len() >= block_size() {
                        lf_seal(&mut w, v);
                    }
                }
                if i % 8 == 0 {
                    lf_flush(v);
                }
            }
            lf_flush(v);
            w_done.store(true, AtoOrd::Release);
        });

        let mut readers = Vec::new();
        for _ in 0..3 {
            let r_done = done.clone();
            let r_bad = bad.clone();
            readers.push(thread::spawn(move || loop {
                for kk in 0..NKEYS {
                    let k = format!("arcprobe:{}:{}", v as u8, kk);
                    if let Some(s) = lf_get_text(&k) {
                        if &*s != format!("V-{}-END", k) {
                            r_bad.fetch_add(1, AtoOrd::Relaxed);
                        }
                    }
                }
                if r_done.load(AtoOrd::Acquire) {
                    break;
                }
            }));
        }

        writer.join().unwrap();
        for r in readers {
            r.join().unwrap();
        }
        assert_eq!(
            bad.load(AtoOrd::Relaxed),
            0,
            "corrupted/aliased shared-text reads observed for {:?}",
            v
        );
    }

    #[test]
    fn lfb_arc_shared_text_never_corrupts() {
        arc_text_probe(Variant::LfB);
    }

    #[test]
    fn lfa_arc_shared_text_never_corrupts() {
        arc_text_probe(Variant::LfA);
    }

    #[test]
    fn lfa_concurrent_write_read_never_corrupts() {
        concurrent_probe(Variant::LfA);
    }

    #[test]
    fn lfb_concurrent_write_read_never_corrupts() {
        concurrent_probe(Variant::LfB);
    }
}

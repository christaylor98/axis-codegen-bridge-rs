//! BRIDGE_SLABLOCK_V1 (AXVERITY_RECLOG_SLA_BLOCK_BUILD_PHASE_A_V1) — the
//! SLA-tiered adaptive block durability writer, PHASE A: an ISOLATED module,
//! deliberately wired into NO live path. It implements the design minted as
//! AXVERITY_RECLOG_SLA_TIERED_BLOCK_DURABILITY_V1 (rust-bridge, PROPOSED):
//! eliminate the recovery-log/WAL tier as a durability primitive — write
//! directly into content blocks, fsync every block with unflushed data on a
//! single SLA tick, positional identity until seal, content hash only at seal.
//!
//! ## ISOLATION (hard constraint of the governing intent)
//!
//! This module has ZERO call edges into `reclog.rs` (`reclog_submit` /
//! `write_batch_fused`), `pg_exec_insert.m1`, or any existing dispatch. The old
//! path remains live and authoritative. Nothing here is reachable from an
//! INSERT. Cutover is a SEPARATE future intent — do not wire this module into a
//! live path without that intent's explicit authorization.
//!
//! ## Shared-nothing topology (SHARED_NOTHING_PRESERVED)
//!
//! All state lives in `thread_local!` storage keyed by handle — the exact
//! `logbuf.rs` / `walindex.rs` pattern: no `Mutex`, no `Arc`, no atomics, no
//! process-global registry. The design's "global SLA tick sweep" is realized
//! PER THREAD: every block is owned by exactly one thread (single writer), so
//! the union of per-thread sweeps IS the global sweep — there exists no block a
//! per-thread tick cannot reach, and no cross-thread coordination primitive is
//! introduced. (This confirms the governing intent's tentative assumption
//! during design-to-code translation, as it required.)
//!
//! ## Positional identity until seal; content hash at seal (criterion 1)
//!
//! An accumulating block is identified by `<dir>/blk-<seq>.bin` — segment path
//! + sequence number, the same positional pattern as `prealloc.rs::seg_path` /
//! reclog's WAL segments. A record's identity within it is `(seq, offset,
//! len)`, returned by `slab_append`. The content hash is computed ONCE, at
//! seal, by an incremental `sha2::Sha256` fed exactly the appended bytes in
//! order — byte-identical in algorithm and `"sha256:<64 hex>"` rendering to
//! `bytes_io.rs::bytes_hash` (verified by a test that cross-checks the two).
//! There is no dual durability primitive: the ONLY durable structure is the
//! block file itself, at every SLA tier.
//!
//! ## The SLA tick (criteria 5–7)
//!
//! `slab_open(dir, sla_us, block_bytes)` fixes the tier EXPLICITLY — a
//! user-facing parameter, not a hidden env default (the design's constraint
//! that the durability-vs-latency tradeoff be explicit). `slab_tick(h)` is the
//! sweep: if less than `sla_us` has elapsed since the last sweep it does
//! NOTHING and returns -1 (callers may invoke it as often as they like — the
//! tier alone controls fsync cadence). When it fires, it sweeps ALL of this
//! handle's blocks with unflushed data:
//!
//!   1. FULL blocks (rotated out by an append that would have overflowed them)
//!      are the early-exit INSIDE the sweep: final fsync + seal (hash
//!      finalized), no separate per-block trigger, no per-block deadline
//!      timers.
//!   2. The ACTIVE block, if dirty, gets `sync_all` — the SAME file object is
//!      re-fsynced and grows until full (no fragmentation: partial flushes
//!      never mint a new object).
//!
//! At the physical fsync-latency floor the mechanism degenerates to
//! block-of-1-append per fsync — same single structure, no separate
//! small-append log (criterion 7 / the no-disguised-WAL constraint).
//!
//! ## Durability mapping (must map 1:1 onto the current guarantees)
//!
//! The current path acks a row after its batch's fsync barrier. In this
//! topology the explicit point is: an append is durable at the first
//! `slab_tick` sweep that returns ≥ 0 after it (the sweep fsyncs every block
//! holding unflushed bytes before returning). `slab_append` NEVER fsyncs — an
//! ack must gate on the covering tick, never on append return. STRONG
//! (power-loss) discipline is kept: a newly created block file's parent
//! directory is fsynced once, at that block's first data fsync (the
//! `fsync_parent_dir` discipline from reclog/logbuf); block extents are
//! pre-allocated via `prealloc::fs_prealloc` (fallocate KEEP_SIZE), so EOF
//! still marks the true data frontier — the Landing-A rationale, reused not
//! reinvented.
//!
//! Diagnosis gate (governing intent, completed before this file was written):
//! `block_flush::block_flush_write` (block_flush.rs:182; tmp+fsync+rename, a
//! whole-block one-shot — its durable-write DISCIPLINE is reused, its
//! tmp+rename shape is not, because an SLA block grows in place and rename
//! semantics don't apply to a growing file), `prealloc::wal_write_seg`
//! (prealloc.rs:110; positional seq + prealloc — pattern adopted verbatim),
//! `hotmem_write/read` (hotmem.rs:121/206; not reused — the arena solves
//! hand-off, not durability), `bytes_hash` (bytes_io.rs:172; algorithm +
//! rendering adopted exactly).
//!
//! Identities are sha256(name_utf8), the bridge-wide convention.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::Instant;

use sha2::{Digest, Sha256};

use super::value::{get_str, intern_str, Value};

/// Default block capacity when `slab_open` is passed `block_bytes = 0` —
/// matches the existing 4 MiB seal-block size (block_flush.rs / hotblk path).
const DEFAULT_BLOCK_BYTES: i64 = 4 * 1024 * 1024;

/// One accumulating (or rotated-out-but-unsealed) block. Positional identity
/// only: `path` = `<dir>/blk-<seq>.bin`. The hash exists only as the running
/// hasher state until seal.
struct Block {
    seq: i64,
    path: String,
    file: File,
    len: i64,
    /// Bytes appended since this block's last fsync.
    unflushed: i64,
    /// Running content hash — finalized exactly once, at seal.
    hasher: Sha256,
    /// Whether this block's parent-dir entry has been made durable (fsynced
    /// alongside this block's first data fsync).
    dir_synced: bool,
}

/// One per-thread SLA block writer.
struct Slab {
    dir: String,
    /// SLA tick period in microseconds — the user-selected tier.
    sla_us: i64,
    block_bytes: i64,
    next_seq: i64,
    /// The block currently accepting appends (minted lazily on first append).
    active: Option<Block>,
    /// Blocks rotated out full, awaiting their sweep early-exit (fsync+seal).
    full_pending: Vec<Block>,
    /// Sealed blocks: (seq, content hash, len).
    sealed: Vec<(i64, String, i64)>,
    last_sweep: Option<Instant>,
    // Counters (exposed via slab_stats — the harness's fsync meter).
    appends: i64,
    data_fsyncs: i64,
    dir_fsyncs: i64,
    sweeps: i64,
    gated_ticks: i64,
}

thread_local! {
    /// This thread's slab writers. Thread-local so N writer threads share
    /// NOTHING — same pattern as logbuf.rs LOGS / walindex.rs shards.
    static SLABS: RefCell<HashMap<i64, Slab>> = RefCell::new(HashMap::new());
    /// Per-thread handle counter. Handles are meaningless on other threads,
    /// exactly like logbuf handles.
    static NEXT: Cell<i64> = const { Cell::new(1) };
}

fn blk_path(dir: &str, seq: i64) -> String {
    // Same positional pattern as prealloc.rs::seg_path — path + sequence.
    format!("{}/blk-{}.bin", dir, seq)
}

/// Fsync the parent directory of `path` — the reclog/logbuf STRONG discipline
/// for a newly created file's directory entry.
fn fsync_parent_dir(path: &str) {
    if let Some(parent) = Path::new(path).parent() {
        let dir = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
        let f = File::open(dir)
            .unwrap_or_else(|e| panic!("slablock: open parent dir {:?} for fsync: {}", dir, e));
        f.sync_all()
            .unwrap_or_else(|e| panic!("slablock: fsync parent dir {:?}: {}", dir, e));
    }
}

impl Slab {
    /// Mint the next positional block: `<dir>/blk-<next_seq>.bin`, extent
    /// pre-allocated (KEEP_SIZE) via the Landing-A primitive so appends are
    /// pure data writes and EOF stays the true frontier.
    fn mint_block(&mut self) -> Block {
        let seq = self.next_seq;
        self.next_seq += 1;
        let path = blk_path(&self.dir, seq);
        super::prealloc::fs_prealloc(Value::Tuple(vec![
            Value::Str(intern_str(&path)),
            Value::Int(self.block_bytes),
        ]));
        let file = OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap_or_else(|e| panic!("slablock: open {} for append: {}", path, e));
        Block {
            seq,
            path,
            file,
            len: 0,
            unflushed: 0,
            hasher: Sha256::new(),
            dir_synced: false,
        }
    }

    /// Fsync one block's data (and, first time only, its directory entry).
    fn fsync_block(block: &mut Block, data_fsyncs: &mut i64, dir_fsyncs: &mut i64) {
        block
            .file
            .sync_all()
            .unwrap_or_else(|e| panic!("slablock: fsync {}: {}", block.path, e));
        *data_fsyncs += 1;
        block.unflushed = 0;
        if !block.dir_synced {
            fsync_parent_dir(&block.path);
            *dir_fsyncs += 1;
            block.dir_synced = true;
        }
    }

    /// Seal a block: it MUST already be fsynced (unflushed == 0). Finalizes the
    /// content hash — the one and only point content identity is computed.
    fn seal_block(&mut self, block: Block) -> String {
        debug_assert_eq!(block.unflushed, 0, "seal requires a flushed block");
        let digest = block.hasher.finalize();
        let hex: String = digest.iter().map(|b| format!("{:02x}", b)).collect();
        let hash = format!("sha256:{}", hex);
        self.sealed.push((block.seq, hash.clone(), block.len));
        hash
    }

    /// The SLA sweep body: every block with unflushed data gets its fsync.
    /// Full blocks are the early-exit — fsync + seal inside the sweep, no
    /// separate trigger. Returns data fsyncs performed.
    fn sweep(&mut self) -> i64 {
        let before = self.data_fsyncs;
        let mut data = self.data_fsyncs;
        let mut dirs = self.dir_fsyncs;
        // 1. Full blocks: flush (if needed) then seal — early-exit in the sweep.
        let pending = std::mem::take(&mut self.full_pending);
        for mut block in pending {
            if block.unflushed > 0 {
                Self::fsync_block(&mut block, &mut data, &mut dirs);
            }
            self.data_fsyncs = data;
            self.dir_fsyncs = dirs;
            self.seal_block(block);
        }
        // 2. Active block: re-fsync in place — same object grows until full.
        if let Some(active) = self.active.as_mut() {
            if active.unflushed > 0 {
                Self::fsync_block(active, &mut data, &mut dirs);
            }
        }
        self.data_fsyncs = data;
        self.dir_fsyncs = dirs;
        self.sweeps += 1;
        self.data_fsyncs - before
    }
}

fn with_slab<R>(h: i64, f: impl FnOnce(&mut Slab) -> R) -> R {
    SLABS.with(|s| {
        let mut map = s.borrow_mut();
        let slab = map
            .get_mut(&h)
            .unwrap_or_else(|| panic!("slablock: unknown handle {} on this thread", h));
        f(slab)
    })
}

/// `slab_open(dir: Text, sla_us: Int, block_bytes: Int) -> Int`
///
/// Open a per-thread SLA block writer rooted at `dir`. `sla_us` is the SLA
/// tick period in microseconds — the EXPLICIT user-selected durability tier
/// (e.g. 100 / 5_000 / 100_000 / 2_000_000). `block_bytes <= 0` selects the
/// 4 MiB default. Returns a handle valid only on the calling thread.
#[track_caller]
pub fn slab_open(args: Value) -> Value {
    let (dir, sla_us, block_bytes) = match args {
        Value::Tuple(es) if es.len() == 3 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("slab_open: expected Tuple(Text, Int, Int), got {:?}", other),
    };
    let dir = match dir {
        Value::Str(hh) => get_str(&hh),
        other => panic!("slab_open: arg 0 (dir) expected Text, got {:?}", other),
    };
    let sla_us = match sla_us {
        Value::Int(n) if n > 0 => n,
        Value::Int(n) => panic!("slab_open: sla_us must be > 0 (explicit tier), got {}", n),
        other => panic!("slab_open: arg 1 (sla_us) expected Int, got {:?}", other),
    };
    let block_bytes = match block_bytes {
        Value::Int(n) if n > 0 => n,
        Value::Int(_) => DEFAULT_BLOCK_BYTES,
        other => panic!("slab_open: arg 2 (block_bytes) expected Int, got {:?}", other),
    };
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("slab_open: mkdir {}: {}", dir, e));
    let h = NEXT.with(|n| {
        let h = n.get();
        n.set(h + 1);
        h
    });
    SLABS.with(|s| {
        s.borrow_mut().insert(
            h,
            Slab {
                dir,
                sla_us,
                block_bytes,
                next_seq: 0,
                active: None,
                full_pending: Vec::new(),
                sealed: Vec::new(),
                last_sweep: None,
                appends: 0,
                data_fsyncs: 0,
                dir_fsyncs: 0,
                sweeps: 0,
                gated_ticks: 0,
            },
        )
    });
    Value::Int(h)
}

/// `slab_append(h: Int, bytes: Bytes) -> Int`
///
/// Append record bytes to the handle's active block; returns the record's
/// byte offset within that block (positional identity `(active seq, offset,
/// len)`). NEVER fsyncs — durability comes only from the tick sweep; an ack
/// must gate on the covering `slab_tick`, not on this returning. Rotation: an
/// append that would overflow a non-empty active block moves it to the
/// full-pending list (sealed at the next sweep) and mints `seq+1`; a record is
/// never split across blocks. A single record larger than `block_bytes` lands
/// alone in a fresh block, which may exceed the capacity by that record — the
/// pack-tier oversize rule.
#[track_caller]
pub fn slab_append(args: Value) -> Value {
    let (h, bytes) = match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("slab_append: expected Tuple(Int, Bytes), got {:?}", other),
    };
    let h = match h {
        Value::Int(n) => n,
        other => panic!("slab_append: arg 0 (handle) expected Int, got {:?}", other),
    };
    let bytes = match bytes {
        Value::Bytes(b) => b,
        other => panic!("slab_append: arg 1 expected Bytes, got {:?}", other),
    };
    let offset = with_slab(h, |slab| {
        // Rotate a non-empty active block the incoming record would overflow.
        let needs_rotation = match slab.active.as_ref() {
            Some(a) => a.len > 0 && a.len + bytes.len() as i64 > slab.block_bytes,
            None => false,
        };
        if needs_rotation {
            let full = slab.active.take().expect("checked Some above");
            slab.full_pending.push(full);
        }
        if slab.active.is_none() {
            let block = slab.mint_block();
            slab.active = Some(block);
        }
        let active = slab.active.as_mut().expect("just minted above");
        let offset = active.len;
        active
            .file
            .write_all(&bytes)
            .unwrap_or_else(|e| panic!("slab_append: write {}: {}", active.path, e));
        active.hasher.update(&bytes);
        active.len += bytes.len() as i64;
        active.unflushed += bytes.len() as i64;
        slab.appends += 1;
        offset
    });
    Value::Int(offset)
}

/// `slab_tick(h: Int) -> Int`
///
/// The SLA tick. If fewer than `sla_us` microseconds have elapsed since this
/// handle's last sweep, does NOTHING (no I/O) and returns -1 — call it as
/// often as you like, the tier alone controls fsync cadence. Otherwise sweeps
/// every block with unflushed data (full-pending first — fsync+seal, the
/// early-exit — then the active block's in-place re-fsync) and returns the
/// number of data fsyncs performed (0 = fired but nothing was dirty).
#[track_caller]
pub fn slab_tick(arg: Value) -> Value {
    let h = match arg {
        Value::Int(n) => n,
        other => panic!("slab_tick: expected Int handle, got {:?}", other),
    };
    let n = with_slab(h, |slab| {
        let now = Instant::now();
        if let Some(last) = slab.last_sweep {
            if now.duration_since(last).as_micros() < slab.sla_us as u128 {
                slab.gated_ticks += 1;
                return -1;
            }
        }
        slab.last_sweep = Some(now);
        slab.sweep()
    });
    Value::Int(n)
}

/// `slab_seal(h: Int) -> Text`
///
/// Explicit end-of-stream seal (shutdown / handoff): sweeps any full-pending
/// blocks, then final-fsyncs and seals the active block regardless of the SLA
/// gate. Returns the active block's content hash (`"sha256:<hex>"`), or
/// `"EMPTY"` if no bytes were ever appended to the current active block. After
/// this the handle has no active block; a later append mints the next seq.
#[track_caller]
pub fn slab_seal(arg: Value) -> Value {
    let h = match arg {
        Value::Int(n) => n,
        other => panic!("slab_seal: expected Int handle, got {:?}", other),
    };
    let hash = with_slab(h, |slab| {
        // Drain rotated-out blocks first so seal order matches block order.
        let pending = std::mem::take(&mut slab.full_pending);
        let mut data = slab.data_fsyncs;
        let mut dirs = slab.dir_fsyncs;
        for mut block in pending {
            if block.unflushed > 0 {
                Slab::fsync_block(&mut block, &mut data, &mut dirs);
            }
            slab.data_fsyncs = data;
            slab.dir_fsyncs = dirs;
            slab.seal_block(block);
        }
        match slab.active.take() {
            None => String::from("EMPTY"),
            Some(mut block) => {
                if block.len == 0 {
                    // Never-written block: nothing durable to seal.
                    slab.active = Some(block);
                    return String::from("EMPTY");
                }
                if block.unflushed > 0 {
                    Slab::fsync_block(&mut block, &mut data, &mut dirs);
                }
                slab.data_fsyncs = data;
                slab.dir_fsyncs = dirs;
                slab.seal_block(block)
            }
        }
    });
    Value::Str(intern_str(&hash))
}

/// `slab_stats(h: Int) -> Text`
///
/// One TSV line of counters:
/// `appends=N\tdata_fsyncs=N\tdir_fsyncs=N\tsweeps=N\tgated=N\tsealed=N\tactive_seq=N\tactive_len=N\tactive_unflushed=N\tsla_us=N`
/// (`active_seq=-1` when no active block). Read-only.
#[track_caller]
pub fn slab_stats(arg: Value) -> Value {
    let h = match arg {
        Value::Int(n) => n,
        other => panic!("slab_stats: expected Int handle, got {:?}", other),
    };
    let s = with_slab(h, |slab| {
        let (aseq, alen, aunf) = match slab.active.as_ref() {
            Some(a) => (a.seq, a.len, a.unflushed),
            None => (-1, 0, 0),
        };
        format!(
            "appends={}\tdata_fsyncs={}\tdir_fsyncs={}\tsweeps={}\tgated={}\tsealed={}\tactive_seq={}\tactive_len={}\tactive_unflushed={}\tsla_us={}",
            slab.appends,
            slab.data_fsyncs,
            slab.dir_fsyncs,
            slab.sweeps,
            slab.gated_ticks,
            slab.sealed.len(),
            aseq,
            alen,
            aunf,
            slab.sla_us,
        )
    });
    Value::Str(intern_str(&s))
}

/// `slab_sealed(h: Int) -> Text`
///
/// The sealed-block ledger: one `"<seq>\t<sha256:hex>\t<len>"` line per sealed
/// block, LF-joined, in seal order. Empty Text if nothing sealed. Read-only —
/// this is Phase-A observability, NOT an on-disk manifest (whether a durable
/// manifest accompanies seal is a cutover-intent question).
#[track_caller]
pub fn slab_sealed(arg: Value) -> Value {
    let h = match arg {
        Value::Int(n) => n,
        other => panic!("slab_sealed: expected Int handle, got {:?}", other),
    };
    let s = with_slab(h, |slab| {
        slab.sealed
            .iter()
            .map(|(seq, hash, len)| format!("{}\t{}\t{}", seq, hash, len))
            .collect::<Vec<_>>()
            .join("\n")
    });
    Value::Str(intern_str(&s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::bytes_io::bytes_hash;

    fn scratch(tag: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!("axv-slab-{}-{}-{}", tag, std::process::id(), nanos));
        d.to_string_lossy().into_owned()
    }

    fn open(dir: &str, sla_us: i64, block_bytes: i64) -> i64 {
        match slab_open(Value::Tuple(vec![
            Value::Str(intern_str(dir)),
            Value::Int(sla_us),
            Value::Int(block_bytes),
        ])) {
            Value::Int(h) => h,
            other => panic!("slab_open returned {:?}", other),
        }
    }

    fn append(h: i64, b: &[u8]) -> i64 {
        match slab_append(Value::Tuple(vec![Value::Int(h), Value::Bytes(b.to_vec())])) {
            Value::Int(off) => off,
            other => panic!("slab_append returned {:?}", other),
        }
    }

    fn stats(h: i64) -> String {
        match slab_stats(Value::Int(h)) {
            Value::Str(s) => get_str(&s),
            other => panic!("slab_stats returned {:?}", other),
        }
    }

    fn stat_field(h: i64, key: &str) -> i64 {
        let s = stats(h);
        s.split('\t')
            .find_map(|kv| kv.strip_prefix(&format!("{}=", key)))
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("stat {} missing in {}", key, s))
    }

    #[test]
    fn append_never_fsyncs_tick_does() {
        let dir = scratch("tick");
        let h = open(&dir, 1, 0); // 1us tier: first tick always fires
        append(h, b"row-one");
        append(h, b"row-two");
        assert_eq!(stat_field(h, "data_fsyncs"), 0, "append must not fsync");
        let n = match slab_tick(Value::Int(h)) {
            Value::Int(n) => n,
            other => panic!("slab_tick returned {:?}", other),
        };
        assert_eq!(n, 1, "one dirty active block -> one data fsync");
        assert_eq!(stat_field(h, "data_fsyncs"), 1);
        assert_eq!(stat_field(h, "active_unflushed"), 0);
    }

    #[test]
    fn sla_gate_blocks_early_ticks() {
        let dir = scratch("gate");
        let h = open(&dir, 60_000_000, 0); // 60s tier: second tick can't fire
        append(h, b"x");
        assert_eq!(slab_tick(Value::Int(h)), Value::Int(1)); // first tick fires
        append(h, b"y");
        assert_eq!(slab_tick(Value::Int(h)), Value::Int(-1), "gated inside SLA window");
        assert_eq!(stat_field(h, "gated"), 1);
        assert_eq!(stat_field(h, "data_fsyncs"), 1);
    }

    #[test]
    fn same_block_grows_in_place_no_fragmentation() {
        let dir = scratch("grow");
        let h = open(&dir, 1, 0);
        for i in 0..5 {
            append(h, format!("row-{}", i).as_bytes());
            slab_tick(Value::Int(h));
            std::thread::sleep(std::time::Duration::from_micros(5));
        }
        // 5 partial flushes, still exactly ONE block file, grown in place.
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(files.len(), 1, "partial flushes must not mint new objects");
        assert_eq!(stat_field(h, "active_seq"), 0);
        assert!(stat_field(h, "data_fsyncs") >= 5);
    }

    #[test]
    fn rotation_seals_inside_sweep_with_bytes_hash_identical_hash() {
        let dir = scratch("seal");
        let h = open(&dir, 1, 16); // tiny 16-byte blocks force rotation
        let a = b"0123456789"; // 10 bytes
        let b = b"abcdefghij"; // 10 bytes -> would overflow, rotates
        append(h, a);
        append(h, b);
        assert_eq!(stat_field(h, "sealed"), 0, "seal happens in the sweep, not at append");
        let n = match slab_tick(Value::Int(h)) {
            Value::Int(n) => n,
            other => panic!("slab_tick returned {:?}", other),
        };
        assert_eq!(n, 2, "full block + active block");
        assert_eq!(stat_field(h, "sealed"), 1);
        // Sealed hash must be byte-identical to bytes_hash over the same bytes.
        let expect = match bytes_hash(Value::Bytes(a.to_vec())) {
            Value::Str(s) => get_str(&s),
            other => panic!("bytes_hash returned {:?}", other),
        };
        let sealed = match slab_sealed(Value::Int(h)) {
            Value::Str(s) => get_str(&s),
            other => panic!("slab_sealed returned {:?}", other),
        };
        let mut cols = sealed.split('\t');
        assert_eq!(cols.next(), Some("0"));
        assert_eq!(cols.next(), Some(expect.as_str()));
        assert_eq!(cols.next(), Some("10"));
        // Block 0 on disk is exactly the sealed bytes; block 1 holds the rest.
        assert_eq!(std::fs::read(format!("{}/blk-0.bin", dir)).unwrap(), a);
        assert_eq!(std::fs::read(format!("{}/blk-1.bin", dir)).unwrap(), b);
    }

    #[test]
    fn explicit_seal_returns_hash_and_offsets_are_positional() {
        let dir = scratch("xseal");
        let h = open(&dir, 60_000_000, 0);
        assert_eq!(append(h, b"aaaa"), 0);
        assert_eq!(append(h, b"bb"), 4, "offset = prior bytes in block");
        let all = b"aaaabb";
        let expect = match bytes_hash(Value::Bytes(all.to_vec())) {
            Value::Str(s) => get_str(&s),
            other => panic!("bytes_hash returned {:?}", other),
        };
        let got = match slab_seal(Value::Int(h)) {
            Value::Str(s) => get_str(&s),
            other => panic!("slab_seal returned {:?}", other),
        };
        assert_eq!(got, expect, "seal-time hash == bytes_hash of appended bytes");
        assert_eq!(slab_seal(Value::Int(h)), Value::Str(intern_str("EMPTY")));
    }

    #[test]
    fn handles_are_thread_local_shared_nothing() {
        let dirs: Vec<String> = (0..4).map(|i| scratch(&format!("thr{}", i))).collect();
        let mut joins = Vec::new();
        for dir in dirs {
            joins.push(std::thread::spawn(move || {
                let h = open(&dir, 1, 0);
                assert_eq!(h, 1, "per-thread handle counter: every thread mints 1");
                for i in 0..50 {
                    append(h, format!("t-{}-{}", dir, i).as_bytes());
                }
                slab_tick(Value::Int(h));
                let sealed = match slab_seal(Value::Int(h)) {
                    Value::Str(s) => get_str(&s),
                    other => panic!("slab_seal returned {:?}", other),
                };
                assert!(sealed.starts_with("sha256:"));
                stat_field(h, "data_fsyncs")
            }));
        }
        for j in joins {
            let fsyncs = j.join().unwrap();
            assert!(fsyncs >= 1);
        }
    }

    #[test]
    fn oversize_record_lands_alone_in_fresh_block() {
        let dir = scratch("oversize");
        let h = open(&dir, 1, 64);
        append(h, b"small");
        let big = vec![0x5au8; 200]; // > block_bytes
        assert_eq!(append(h, &big), 0, "oversize record starts its own block");
        slab_tick(Value::Int(h));
        assert_eq!(stat_field(h, "sealed"), 1, "the small block sealed at rotation sweep");
        assert_eq!(std::fs::read(format!("{}/blk-1.bin", dir)).unwrap(), big);
    }
}

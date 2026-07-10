//! Shared shard/slot storage for the sharded-interner experiment
//! (mutex-audit follow-up; see `interner_mutex_feed.rs` / `interner_lockfree_feed.rs`
//! for the two feeder variants built on top of this).
//!
//! This module is ONLY the storage: handle encoding, per-shard dedup map,
//! and the growable slot array readers pull from. It says nothing about how
//! a request gets from a calling thread to a shard's owner thread — that's
//! the feeder modules' job, and it's the only axis those two variants
//! differ on. Keeping storage identical between variants is what makes the
//! benchmark comparison fair.
//!
//! ## Handle encoding
//!
//! `u32` handle = `(shard_id << LOCAL_BITS) | local_index`. Handle `0` is
//! globally reserved for `""` and bypasses shard storage entirely — neither
//! `intern` nor `get` for the empty string ever touches a shard.
//!
//! Confirmed via full-codebase search (before this module existed) that
//! every consumer of a `value.rs` string/tag handle outside `value.rs`
//! itself treats it as a fully opaque token — no arithmetic, no
//! sequential-index assumption, no serialization — so this encoding change
//! is invisible to 414 existing call sites.
//!
//! ## Per-shard structure
//!
//! - **Private dedup map** (`HashMap<String, u32>`): touched ONLY by that
//!   shard's one dedicated writer thread. Zero synchronization needed, by
//!   construction — this is the same single-writer-per-shard discipline
//!   already adopted for Hot Mem's write path.
//! - **Public slot storage**: growable as immutable, fixed-size blocks
//!   (`BLOCK_LEN` slots each) rather than one giant preallocation, so an
//!   empty program pays close to zero memory/startup cost and there is no
//!   hard exhaustion ceiling below the 24-bit local-index budget.
//!
//! ## Publish discipline — write-once, NOT a seqlock
//!
//! This is deliberately NOT the same soundness argument as
//! `non_blocking_memory::SeqCell` (a retry-based seqlock for a
//! repeatedly-overwritten `Copy` value). Every slot here is written
//! EXACTLY ONCE and never touched again after publish, so there is nothing
//! to retry against. The argument is the simpler one `Cell<T>`'s Block path
//! already relies on in this same crate: the writer fully initializes a
//! slot's `String`, THEN does `committed.store(index + 1, Release)`; a
//! reader does `committed.load(Acquire)` FIRST and only indexes a slot if
//! `local_index < committed`. That single Release/Acquire pair on
//! `committed` is sufficient to make everything the writer did before it
//! (including allocating the slot's block, if it was newly allocated)
//! visible to the reader — the same "release sequence" reasoning
//! `Cell<T>`'s own doc comment spells out for its Block path. Do NOT
//! "simplify" this to look like `SeqCell` — `SeqCell` allows overwrite,
//! this does not, and porting its retry logic here would be solving a
//! problem this design doesn't have while missing the one it does.
//!
//! Both loads (`committed` and the block pointer) use `Acquire` here even
//! though the block-pointer load could be proven `Relaxed`-safe given the
//! reasoning above — matching this crate's existing preference (see
//! `rawmem.rs`'s blanket `SeqCst` choice) for a stronger-than-strictly-
//! required ordering when the cost is negligible and the alternative saves
//! nothing measurable.
//!
//! ## Lifetime
//!
//! Shards are process-lifetime singletons (like `value.rs`'s existing
//! `STRING_TABLE`/`TAG_TABLE`): nothing here is ever freed before process
//! exit. Miri's leak checker will flag this as an intentional leak, same as
//! `rawmem.rs`'s `cell_new_raw` — run with `MIRIFLAGS=-Zmiri-ignore-leaks`.

use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
use std::sync::OnceLock;

/// Bits of the handle reserved for the shard id (max 256 shards).
const SHARD_ID_BITS: u32 = 8;
/// Bits of the handle reserved for the per-shard local index.
pub const LOCAL_BITS: u32 = 32 - SHARD_ID_BITS;
const LOCAL_MASK: u32 = (1 << LOCAL_BITS) - 1;

/// Slots per immutable block. Chosen so a freshly-touched shard allocates
/// one small block (a few dozen KB), not a huge up-front reservation.
const BLOCK_LEN: usize = 4096;
/// Blocks per shard — bounds the local-index space to `LOCAL_BITS`.
const MAX_BLOCKS: usize = (1usize << LOCAL_BITS) / BLOCK_LEN;

/// Pack a (shard, local_index) pair into a handle.
///
/// Encodes `local + 1`, never bare `local` — `pack(0, 0)` would otherwise
/// equal exactly `0`, colliding with the globally-reserved empty-string
/// handle (a real bug caught by `concurrent_unique_strings_all_distinct_and_readable`:
/// the first unique string landing on shard 0's first slot silently
/// resolved to `""`). Never called with `local == LOCAL_MASK` —
/// `get_or_insert`'s exhaustion check bounds `local < LOCAL_MASK` for
/// exactly this reason, so `local + 1` never wraps.
pub fn pack(shard: u32, local: u32) -> u32 {
    debug_assert!(local < LOCAL_MASK, "local index {} exceeds per-shard budget", local);
    debug_assert!(shard < (1 << SHARD_ID_BITS), "shard id {} exceeds SHARD_ID_BITS", shard);
    (shard << LOCAL_BITS) | ((local + 1) & LOCAL_MASK)
}

/// Unpack a handle into (shard, local_index). Never called with `handle ==
/// 0` — that's the globally-reserved empty-string sentinel, short-circuited
/// by every caller before reaching here.
pub fn unpack(handle: u32) -> (u32, u32) {
    debug_assert_ne!(handle, 0, "handle 0 is the empty-string sentinel, never a packed (shard, local)");
    (handle >> LOCAL_BITS, (handle & LOCAL_MASK) - 1)
}

/// Number of shards, frozen at first use. Must never change after the first
/// handle is minted — it's baked into every handle's bit layout via
/// `pack`/`unpack`'s caller (shard selection), not into `pack` itself, but
/// changing shard COUNT mid-run would route the same string to a different
/// shard than one already interned it under, which would break dedup.
pub fn shard_count() -> u32 {
    static N: OnceLock<u32> = OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("AXVERITY_INTERN_SHARDS")
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .filter(|&n| n >= 1 && n <= (1 << SHARD_ID_BITS))
            .unwrap_or(8)
    })
}

/// Which shard a string routes to. Stable for a given `shard_count()` — the
/// only thing that matters is that the same string always maps to the same
/// shard within one process run.
pub fn shard_for(s: &str) -> u32 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    (h.finish() as u32) % shard_count()
}

/// One immutable-once-filled block of slots. `UnsafeCell` because the
/// owning shard's single writer thread writes each slot exactly once before
/// any reader can observe it (see module doc comment); `MaybeUninit`
/// because slots start empty.
struct Block {
    slots: [UnsafeCell<MaybeUninit<String>>; BLOCK_LEN],
}

impl Block {
    fn new_boxed() -> Box<Block> {
        Box::new(Block {
            slots: std::array::from_fn(|_| UnsafeCell::new(MaybeUninit::uninit())),
        })
    }
}

// SAFETY: `Block` is shared read-only (via raw pointer) across threads once
// published; the single owning writer thread is the only one that ever
// writes a slot, and only writes each slot once, before it is reachable by
// any reader (see `ShardStorage::get_or_insert`/`ShardStorage::get`).
unsafe impl Sync for Block {}

/// Public slot storage + private dedup map for one shard.
///
/// `dedup` is genuinely single-writer (only that shard's owner thread ever
/// touches it) — `UnsafeCell` here is a bare "no runtime check" cell, not a
/// concurrency primitive; nothing about it is `Sync`-safe for multiple
/// writers, and nothing else in this module ever assumes otherwise.
pub struct ShardStorage {
    blocks: Box<[AtomicPtr<Block>]>,
    committed: AtomicU32,
    dedup: UnsafeCell<HashMap<String, u32>>,
    /// Writer-only running count — avoids the writer ever needing to read
    /// back its own `committed` store.
    next: UnsafeCell<u32>,
}

// SAFETY: `dedup`/`next` are touched only by this shard's single owning
// writer thread (enforced by construction in the feeder modules, not by
// this type) — the `Sync` bound here is required so `ShardStorage` can live
// behind a `&'static` shared reference reachable from both the writer
// thread and every reader thread, exactly like `ReaderRegistry`.
unsafe impl Sync for ShardStorage {}

impl ShardStorage {
    pub fn new() -> Self {
        ShardStorage {
            blocks: (0..MAX_BLOCKS).map(|_| AtomicPtr::new(std::ptr::null_mut())).collect(),
            committed: AtomicU32::new(0),
            dedup: UnsafeCell::new(HashMap::new()),
            next: UnsafeCell::new(0),
        }
    }

    /// Get-or-insert `s`, returning its LOCAL index (not yet packed into a
    /// handle — the feeder knows its own shard id and packs it).
    ///
    /// SAFETY / CONTRACT: must be called only from this shard's single
    /// owning writer thread. Never called concurrently with itself.
    pub fn get_or_insert(&self, s: &str) -> u32 {
        let dedup = unsafe { &mut *self.dedup.get() };
        if let Some(&local) = dedup.get(s) {
            return local;
        }
        let next = unsafe { &mut *self.next.get() };
        let local = *next;
        // Strictly `<`, not `<=`: `local == LOCAL_MASK` is reserved so
        // `pack`'s `local + 1` can never wrap back to 0 (see `pack`'s doc
        // comment) — costs exactly one slot out of ~16.7M per shard.
        assert!(
            local < LOCAL_MASK,
            "interner_shard: shard exhausted its {}-slot budget",
            LOCAL_MASK
        );
        let block_id = (local as usize) / BLOCK_LEN;
        let slot_in_block = (local as usize) % BLOCK_LEN;
        debug_assert!(block_id < MAX_BLOCKS);

        if slot_in_block == 0 {
            let raw = Box::into_raw(Block::new_boxed());
            self.blocks[block_id].store(raw, Ordering::Release);
        }
        // Relaxed is provably sufficient here (this thread just stored it,
        // or a prior call on this same single-writer thread did) — Acquire
        // costs nothing extra and removes any doubt.
        let block_ptr = self.blocks[block_id].load(Ordering::Acquire);
        unsafe {
            (*(*block_ptr).slots[slot_in_block].get()).write(s.to_string());
        }
        // Publish AFTER the string is fully written — this Release pairs
        // with readers' Acquire on `committed` and is what makes the write
        // above (and the block-pointer store above it, if any) visible.
        self.committed.store(local + 1, Ordering::Release);
        *next = local + 1;
        dedup.insert(s.to_string(), local);
        local
    }

    /// Read the string at `local` index. Returns `None` if not yet
    /// committed (caller passed a handle from a shard/local combination
    /// this storage never produced — treated as "unknown", matching
    /// `value.rs::get_str`'s existing `<invalid-str-N>` fallback contract
    /// at the feeder layer, not here).
    pub fn get(&self, local: u32) -> Option<String> {
        let committed = self.committed.load(Ordering::Acquire);
        if local >= committed {
            return None;
        }
        let block_id = (local as usize) / BLOCK_LEN;
        let slot_in_block = (local as usize) % BLOCK_LEN;
        let block_ptr = self.blocks[block_id].load(Ordering::Acquire);
        debug_assert!(!block_ptr.is_null(), "committed index with unpublished block");
        // SAFETY: `local < committed` (Acquire-loaded above) guarantees,
        // via the Release/Acquire pair on `committed`, that this slot's
        // write (and its block's publish, if this is the block's first
        // slot) happened-before this read.
        let s = unsafe { (*(*block_ptr).slots[slot_in_block].get()).assume_init_ref().clone() };
        Some(s)
    }
}

impl Default for ShardStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_unpack_roundtrip() {
        // LOCAL_MASK itself is reserved (see pack's doc comment) — max
        // valid local is LOCAL_MASK - 1.
        for shard in [0u32, 1, 7, 255] {
            for local in [0u32, 1, 12345, LOCAL_MASK - 1] {
                let h = pack(shard, local);
                assert_eq!(unpack(h), (shard, local));
            }
        }
    }

    #[test]
    fn pack_shard_zero_local_zero_never_collides_with_empty_string_sentinel() {
        // Regression test: pack(0, 0) must never equal 0 (the globally
        // reserved empty-string handle) — this was a real bug caught by
        // interner_mutex_feed/interner_lockfree_feed's concurrent-unique-
        // strings test (the first unique string landing on shard 0's first
        // slot silently resolved to "").
        assert_ne!(pack(0, 0), 0);
    }

    #[test]
    fn single_shard_get_or_insert_dedup_and_roundtrip() {
        let storage = ShardStorage::new();
        let a = storage.get_or_insert("hello");
        let b = storage.get_or_insert("world");
        let a2 = storage.get_or_insert("hello");
        assert_eq!(a, a2, "same string must dedup to same local index");
        assert_ne!(a, b);
        assert_eq!(storage.get(a).as_deref(), Some("hello"));
        assert_eq!(storage.get(b).as_deref(), Some("world"));
    }

    #[test]
    fn get_before_commit_returns_none() {
        let storage = ShardStorage::new();
        assert_eq!(storage.get(0), None);
        storage.get_or_insert("x");
        assert_eq!(storage.get(0).as_deref(), Some("x"));
        assert_eq!(storage.get(1), None);
    }

    #[test]
    fn block_boundary_crossing() {
        let storage = ShardStorage::new();
        // Force at least one block rollover.
        for i in 0..(BLOCK_LEN + 10) {
            let s = format!("s{}", i);
            let local = storage.get_or_insert(&s);
            assert_eq!(local as usize, i);
        }
        assert_eq!(storage.get(0).as_deref(), Some("s0"));
        assert_eq!(storage.get(BLOCK_LEN as u32 - 1).as_deref(), Some(format!("s{}", BLOCK_LEN - 1)).as_deref());
        assert_eq!(storage.get(BLOCK_LEN as u32).as_deref(), Some(format!("s{}", BLOCK_LEN)).as_deref());
        assert_eq!(storage.get(BLOCK_LEN as u32 + 9).as_deref(), Some(format!("s{}", BLOCK_LEN + 9)).as_deref());
    }

    #[test]
    fn shard_for_is_stable() {
        let a = shard_for("consistent");
        let b = shard_for("consistent");
        assert_eq!(a, b);
        assert!(a < shard_count());
    }
}

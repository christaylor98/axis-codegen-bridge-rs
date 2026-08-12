//! HOTBLK_V1 (AXVERITY_FRONTEND_WRITEPATH_INTEGRATION_V1) — a thin, thread-local
//! register file holding a pg_server worker thread's CURRENT hot-block
//! accumulator across INSERT calls. DUMB PERSISTENCE ONLY: it stores six i64
//! fields and hands them back. It contains NO mint/seal/write logic — every bit
//! of that stays in the spike's already-proven M1 fns (hrw_mint_block /
//! hrw_seal_current / hrw_seal_flush_reclaim), which read and write these fields
//! via hotblk_get/hotblk_set. The seal logic is ported, not reinvented
//! (hard-limit SEAL_LOGIC_PORTED_NOT_REINVENTED); this file is only where the
//! accumulator lives between calls.
//!
//! ## Why a thread-local register instead of loop-state threading
//!
//! The spike (hrw_step.m1) carries its accumulator through its `loop_while`
//! state tuple — fine for a single batch loop. But the live pg_server threads
//! only a bare `Int` through BOTH its loops: the per-connection query loop
//! (`loop_while(conn, ...)`, pg_serve_conn.m1) and the accept loop
//! (`loop_count(1e6, listener, pg_accept_one)`, src/pg_server.m1). A 4 MiB block
//! spans many INSERTs across many short connections, so the accumulator must
//! persist across both — and widening either loop's state would rewrite the
//! frozen request/protocol layer (hard-limit PG_SERVER_LAYER_UNTOUCHED).
//!
//! A per-thread register sidesteps that: it is the SAME shared-nothing,
//! thread-owned pattern logbuf.rs (Landing 1) established — reachable only by
//! the worker thread that owns the block, with no Mutex/RwLock/Arc/atomic and
//! nothing shared on the path. Each pg_server worker thread has its own block;
//! N workers accumulate N disjoint blocks with zero contention, exactly like N
//! thread-owned logbufs.
//!
//! ## Field layout — MUST match lib/pg_hotblk_write.m1's constants exactly
//!
//!   0  ptr            active block arena ptr        (0 == NO live block sentinel)
//!   1  cell           active block state cell address (Free/Active/Sealed)
//!   2  block_seq      active block sequence number
//!   3  cursor         bytes written so far in the active block
//!   4  block_start_i  record ordinal the active block started at (manifest)
//!   5  idx_cell       active block index-status cell address (Unindexed/Indexed)
//!   6  capacity       active block's arena capacity in BYTES, as returned by
//!                     mem_reserve_raw. Added by BENCHMARK_UB_AND_PROTOCOL_FIX_V1
//!                     follow-up so the seal/free path stops reconstructing it from
//!                     a hardcoded Int(4194304): mem_free_raw requires the capacity
//!                     to match the original alloc Layout EXACTLY, and a mismatch is
//!                     undefined behaviour, not a checked error. Read by
//!                     pg_hotblk_write/commit and pg_{hotblk,derive}_seal_mint.
//!
//! An untouched slot reads all-zero; `ptr == 0` is the "no live block yet"
//! sentinel M1 keys on to mint the first block on the thread's first INSERT.
//!
//! ## ABI shapes (deliberately the proven ones)
//!
//! hotblk_get is 1-arg (`Value::Int` in, like logbuf_sync) and hotblk_set is
//! 2-arg (`Value::Tuple(2)` in, like logbuf_append) — the exact shapes logbuf.rs
//! already exercises, so this leans on no unverified N-arg packing. Identities
//! are sha256(name_utf8), the bridge-wide convention.
//!
//! ## AXVERITY_HOTBLK_TIMED_FLUSH_V1 (D044 Phase 2) — slot 7 + cross-thread publish
//!
//! Slot 7 (`generation`) is new: a monotonic counter, LOG-family-only
//! (bumped only on a slot-0/`ptr` write — the OBJECT family never touches
//! slot 0, see the disjoint-slots note above, so this can never fire for
//! it), incremented every time this thread's active block changes identity
//! (mint or rotate). It exists because `ptr` values get RECYCLED under
//! D044 Phase 1's pool-based free-list — the same numeric address WILL
//! recur later in the process's life, so `ptr` alone cannot serve as its
//! own "is this still the block I think it is" check; `generation` can.
//!
//! `hotblk_set` ALSO transparently publishes `(ptr, cursor, generation)` to
//! a process-wide `SeqCell` (`non_blocking_memory`, the established no-CAS
//! discipline — see `interner_shard.rs`/`hotmem.rs`/`qhm.rs`) whenever slot
//! 0 or slot 3 changes, so the independent timer thread
//! (`graphcore/src/gcore_timer.m1`) can read this thread's current block
//! state lock-free, without blocking the writer and without the writer
//! knowing anything changed (D040: the M1 write path stays exactly as it
//! was before this phase — pg_hotblk_write.m1/pg_hotblk_commit.m1 are
//! byte-for-byte unchanged; only pg_hotblk_seal_mint.m1 gained a single
//! extra `hotblk_get(Int(7))` read, to hand the flush worker the
//! generation its full-block job belongs to — see block_flush.rs).
//!
//! SINGLE GLOBAL CELL, not per-thread: correct today because
//! hotblk_get/hotblk_set have exactly one live caller family (graphcore's
//! pg_hotblk_* path), and gcore_serve is verified single-threaded (D007,
//! one serial accept loop) — so there is only ever one thread's worth of
//! state to publish. If hotblk_get/hotblk_set ever gets a second,
//! multi-threaded consumer, this cell needs to become per-thread-keyed;
//! not built now (YAGNI — no such consumer exists to design against).

use std::cell::RefCell;
use std::sync::OnceLock;

use super::non_blocking_memory::SeqCell;
use super::value::{get_str, intern_str, Value};

// 8 slots: 0..5 as documented above, slot 6 the active block's capacity,
// slot 7 the D044 Phase 2 generation counter (LOG-family-only, see above).
const NFIELDS: usize = 8;
const FIELD_PTR: usize = 0;
const FIELD_CURSOR: usize = 3;
const FIELD_GENERATION: usize = 7;

thread_local! {
    /// This thread's active hot-block accumulator. THREAD-LOCAL, never shared:
    /// reachable only by the worker thread that owns the block. No lock, no
    /// atomic, no registry — the same "thread-owned, no shared registry" model
    /// as logbuf.rs's LOGS.
    static HOTBLK: RefCell<[i64; NFIELDS]> = const { RefCell::new([0; NFIELDS]) };
}

/// D044 Phase 2's cross-thread publish target: `(ptr, cursor, generation)`.
/// `OnceLock` because `SeqCell::new()` isn't `const fn` (matches
/// `hotmem.rs`'s own `ARENA_SHARED: OnceLock<...>` pattern for the same
/// reason).
static ACTIVE_BLOCK: OnceLock<SeqCell<(i64, i64, i64)>> = OnceLock::new();

fn active_block() -> &'static SeqCell<(i64, i64, i64)> {
    ACTIVE_BLOCK.get_or_init(SeqCell::new)
}

/// `hotblk_timer_interval_ms(Unit) -> Int` — D044 Phase 2's timer tick
/// interval, `AXVERITY_HOTBLK_TIMER_MS` (default 250, within D044's
/// required 100-500ms bound), same env-knob-with-`OnceLock`-cache shape as
/// `hotblk_pool.rs`'s own `pool_depth()`. The sole caller is
/// `graphcore/src/gcore_timer.m1`.
#[track_caller]
pub fn hotblk_timer_interval_ms(_arg: Value) -> Value {
    static MS: OnceLock<i64> = OnceLock::new();
    let ms = *MS.get_or_init(|| {
        std::env::var("AXVERITY_HOTBLK_TIMER_MS")
            .ok()
            .and_then(|v| v.trim().parse::<i64>().ok())
            .filter(|n| *n >= 1)
            .unwrap_or(250)
    });
    Value::Int(ms)
}

/// `hotblk_get(field: Int) -> Int`
///
/// Read one accumulator field of the calling thread's active block. Panics on a
/// non-Int arg or an out-of-range field index.
#[track_caller]
pub fn hotblk_get(arg: Value) -> Value {
    let f = match arg {
        Value::Int(n) => n,
        other => panic!("hotblk_get: expected Int field, got {:?}", other),
    };
    if f < 0 || f as usize >= NFIELDS {
        panic!("hotblk_get: field {} out of range 0..{}", f, NFIELDS);
    }
    HOTBLK.with(|s| Value::Int(s.borrow()[f as usize]))
}

/// `hotblk_set(field: Int, val: Int) -> Unit`
///
/// Write one accumulator field of the calling thread's active block. Panics on a
/// non-Tuple(2) arg, a non-Int field/value, or an out-of-range field index.
#[track_caller]
pub fn hotblk_set(args: Value) -> Value {
    let (f, v) = match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("hotblk_set: expected Tuple(Int, Int), got {:?}", other),
    };
    let f = match f {
        Value::Int(n) => n,
        other => panic!("hotblk_set: arg 0 expected Int field, got {:?}", other),
    };
    let v = match v {
        Value::Int(n) => n,
        other => panic!("hotblk_set: arg 1 expected Int val, got {:?}", other),
    };
    if f < 0 || f as usize >= NFIELDS {
        panic!("hotblk_set: field {} out of range 0..{}", f, NFIELDS);
    }
    let published = HOTBLK.with(|s| {
        let mut r = s.borrow_mut();
        r[f as usize] = v;
        // D044 Phase 2: a slot-0 (ptr) write is a mint/rotate — bump this
        // thread's generation and publish (new ptr, cursor=0, new gen).
        // A slot-3 (cursor) write is a plain accumulate — republish with
        // the unchanged ptr/generation. Both read the REST of the fields
        // from the same already-locked borrow, so this can't observe a
        // torn combination of its own thread's fields.
        match f as usize {
            FIELD_PTR => {
                r[FIELD_GENERATION] += 1;
                Some((r[FIELD_PTR], 0i64, r[FIELD_GENERATION]))
            }
            FIELD_CURSOR => Some((r[FIELD_PTR], r[FIELD_CURSOR], r[FIELD_GENERATION])),
            _ => None,
        }
    });
    if let Some(triple) = published {
        // SAFETY: hotblk_set has exactly one live caller family
        // (graphcore's pg_hotblk_* path) and gcore_serve is verified
        // single-threaded (D007) — see this module's doc comment.
        unsafe {
            active_block().write(triple);
        }
    }
    Value::Unit
}

/// `hotblk_active_block(Unit) -> Text` — D044 Phase 2. Lock-free cross-
/// thread read of the LOG-family accumulator's current `(ptr, cursor,
/// generation)`, as `"<ptr>\t<cursor>\t<generation>"`. The ONLY caller is
/// `graphcore/src/gcore_timer.m1`'s timer thread; safe from any thread,
/// any number of readers (`SeqCell::read`'s own contract). `"0\t0\t0"`
/// before the first-ever write (nothing to checkpoint yet — matches the
/// existing `ptr == 0` "no live block" sentinel every other reader of this
/// register already keys on).
#[track_caller]
pub fn hotblk_active_block(_arg: Value) -> Value {
    let (ptr, cursor, generation) = active_block().read(0).value().unwrap_or((0, 0, 0));
    Value::Str(intern_str(&format!("{}\t{}\t{}", ptr, cursor, generation)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_reads_zero_then_roundtrips() {
        // A fresh thread's slot is all-zero (ptr==0 sentinel).
        for f in 0..NFIELDS as i64 {
            assert_eq!(hotblk_get(Value::Int(f)), Value::Int(0));
        }
        // set/get round-trips each field independently.
        let vals = [111, 222, 333, 444, 555, 666];
        for (f, v) in vals.iter().enumerate() {
            hotblk_set(Value::Tuple(vec![Value::Int(f as i64), Value::Int(*v)]));
        }
        for (f, v) in vals.iter().enumerate() {
            assert_eq!(hotblk_get(Value::Int(f as i64)), Value::Int(*v));
        }
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn get_out_of_range_panics() {
        let _ = hotblk_get(Value::Int(NFIELDS as i64));
    }

    /// D044 Phase 2: hotblk_active_block reflects ptr/cursor writes, and
    /// generation bumps ONLY on a ptr (field 0) write, never on a plain
    /// cursor (field 3) accumulate.
    ///
    /// ACTIVE_BLOCK is a deliberate single PROCESS-WIDE cell (see this
    /// module's own doc comment: correct because production has exactly
    /// one thread ever calling hotblk_set). That means, unlike HOTBLK
    /// itself, it is NOT isolated by running this test on a fresh thread —
    /// every OTHER test in this same binary that calls hotblk_set (e.g.
    /// unset_reads_zero_then_roundtrips) publishes to the SAME cell.
    /// cargo test's default parallel runner interleaves them for real:
    /// confirmed directly, twice — first an absolute "0\t0\t0" starting
    /// assumption failed with "111\t444\t1" (a prior test's leftover
    /// state), then even a RELATIVE g0/g0+1 check failed once fixed,
    /// because another test's write landed on the SAME cell between two
    /// of THIS test's own reads. Neither is a production bug (production
    /// never has a second writer to race). #[ignore] here, same pattern
    /// block_flush.rs's tests already use for their own shared
    /// process-wide-state hazard — run explicitly, alone.
    #[test]
    #[ignore = "shares the process-wide ACTIVE_BLOCK cell with every other hotblk_set caller in this test binary — run explicitly, not as part of the default parallel suite"]
    fn active_block_reflects_ptr_and_cursor_generation_only_on_ptr() {
        std::thread::spawn(|| {
            let read = || match hotblk_active_block(Value::Unit) {
                Value::Str(h) => get_str(&h),
                other => panic!("expected Text, got {:?}", other),
            };
            let gen_of = |s: &str| s.rsplit('\t').next().unwrap().parse::<i64>().unwrap();
            let g0 = gen_of(&read());

            hotblk_set(Value::Tuple(vec![Value::Int(0), Value::Int(4096)]));
            let r1 = read();
            assert_eq!(r1, format!("4096\t0\t{}", g0 + 1), "first ptr write: cursor resets, generation +1");

            hotblk_set(Value::Tuple(vec![Value::Int(3), Value::Int(500)]));
            assert_eq!(read(), format!("4096\t500\t{}", g0 + 1), "cursor write: ptr/generation unchanged");

            hotblk_set(Value::Tuple(vec![Value::Int(3), Value::Int(900)]));
            assert_eq!(read(), format!("4096\t900\t{}", g0 + 1), "second cursor write: still same generation");

            hotblk_set(Value::Tuple(vec![Value::Int(0), Value::Int(8192)]));
            assert_eq!(read(), format!("8192\t0\t{}", g0 + 2), "rotate: new ptr, cursor resets, generation +1 again");
        })
        .join()
        .unwrap();
    }
}

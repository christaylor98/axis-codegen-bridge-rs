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

use std::cell::RefCell;

use super::value::Value;

const NFIELDS: usize = 6;

thread_local! {
    /// This thread's active hot-block accumulator. THREAD-LOCAL, never shared:
    /// reachable only by the worker thread that owns the block. No lock, no
    /// atomic, no registry — the same "thread-owned, no shared registry" model
    /// as logbuf.rs's LOGS.
    static HOTBLK: RefCell<[i64; NFIELDS]> = const { RefCell::new([0; NFIELDS]) };
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
    HOTBLK.with(|s| s.borrow_mut()[f as usize] = v);
    Value::Unit
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
}

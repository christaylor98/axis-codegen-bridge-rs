//! ACK_REGISTRY_V1 (AXVERITY_SLICE4_BLOCK_DURABILITY_V2, item 5) — maps a
//! sealed hotblk block's identity `(flush_dir, block_seq)` to the oneshot
//! tokens waiting on ITS durability. This is the missing piece `oneshot.rs`
//! doesn't provide on its own: oneshot is a bare id→cell rendezvous with no
//! notion of "which block": something has to remember which waiters belong
//! to which block so `block_flush_write` can signal exactly the right ones
//! after ITS fsync, never a different block's.
//!
//! Deliberately a SHARED, Mutex-protected registry (unlike hotblk.rs's
//! thread-local accumulator) — this is inherently cross-thread by
//! construction: the request thread registers and waits, the block-flush
//! worker thread signals after fsync. Contention is bounded and rare: one
//! lock+push per INSERT (registration) and one lock+drain per block SEAL
//! (not per record), never on the fsync itself.
//!
//! `ack_signal_block` is called from `block_flush.rs` immediately after
//! `write_bin_durable` returns — i.e. AFTER fsync, never after the earlier
//! in-memory `write()`. That ordering is the whole point of this module.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::oneshot::{new_oneshot, signal_oneshot};
use super::value::{get_str, Value};

fn registry() -> &'static Mutex<HashMap<(String, i64), Vec<i64>>> {
    static REG: OnceLock<Mutex<HashMap<(String, i64), Vec<i64>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Mint a fresh oneshot token and register it as waiting on `(flush_dir,
/// block_seq)`'s eventual durable seal. Returns the token id.
pub(crate) fn register(flush_dir: &str, block_seq: i64) -> i64 {
    let id = new_oneshot();
    registry()
        .lock()
        .unwrap()
        .entry((flush_dir.to_string(), block_seq))
        .or_default()
        .push(id);
    id
}

/// Signal every token waiting on `(flush_dir, block_seq)` (removing the
/// entry) and return how many were signaled. Called AFTER that block's fsync
/// completes. A block with zero registered waiters (e.g. slice4 off, or the
/// old fill-triggered path with no ack-wait callers yet) is a harmless no-op.
pub(crate) fn signal_block(flush_dir: &str, block_seq: i64) -> usize {
    let ids = registry().lock().unwrap().remove(&(flush_dir.to_string(), block_seq));
    match ids {
        Some(ids) => {
            let n = ids.len();
            for id in ids {
                signal_oneshot(id);
            }
            n
        }
        None => 0,
    }
}

/// `ack_register(flush_dir: Text, block_seq: Int) -> Int`
#[track_caller]
pub fn ack_register(flush_dir: std::sync::Arc<str>, block_seq: i64) -> Value {
    Value::Int(register(&flush_dir, block_seq))
}

/// `ack_signal_block(flush_dir: Text, block_seq: Int) -> Int` (count signaled)
#[track_caller]
pub fn ack_signal_block(flush_dir: std::sync::Arc<str>, block_seq: i64) -> Value {
    Value::Int(signal_block(&flush_dir, block_seq) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::oneshot::wait_oneshot_timeout;

    #[test]
    fn registered_waiters_are_signaled_by_matching_block_only() {
        let a1 = register("/tmp/dirA", 1);
        let a2 = register("/tmp/dirA", 1);
        let b1 = register("/tmp/dirB", 1); // different flush_dir, same seq
        let a_other_seq = register("/tmp/dirA", 2); // same flush_dir, different seq

        let n = signal_block("/tmp/dirA", 1);
        assert_eq!(n, 2);
        assert!(wait_oneshot_timeout(a1, 50));
        assert!(wait_oneshot_timeout(a2, 50));
        // Unrelated registrations must NOT have been signaled.
        assert!(!wait_oneshot_timeout(b1, 5));
        assert!(!wait_oneshot_timeout(a_other_seq, 5));

        // Clean up the still-pending ones so they don't leak across tests.
        signal_block("/tmp/dirB", 1);
        signal_block("/tmp/dirA", 2);
    }

    #[test]
    fn signaling_an_empty_block_is_a_harmless_noop() {
        assert_eq!(signal_block("/tmp/never-registered", 999), 0);
    }
}

//! Loom model of the Hot Mem arena's hazard-pointer reclamation protocol
//! (AXVERITY_HOTMEM_ARENA_SOUNDNESS_FIX_V1, bug 2 — the missing StoreLoad
//! barrier).
//!
//! Run with:
//!   RUSTFLAGS="--cfg loom" cargo test --test loom_arena --release
//!
//! This is a faithful MINI-MODEL of the production protocol
//! (`BridgedCell::read_ref` pin + `Writer::write_lazy` / `reclaim` floor scan +
//! free), not the production types — those use `std` atomics, whereas loom
//! requires `loom::sync::atomic`. The shapes match exactly:
//!
//!   reader:  slot.store(pin, Release); [FENCE]; head.load(Acquire);
//!            tighten pin; DEREFERENCE the block it read.
//!   writer:  head.store(new, Release); [FENCE]; floor = slot.load(Acquire);
//!            if floor > retired_epoch { FREE the retired block }.
//!
//! A use-after-free is modelled the way loom actually catches it: the block's
//! payload is a `loom::cell::UnsafeCell` accessed NON-atomically. The reader's
//! dereference is a non-atomic read; the writer's free is a non-atomic write
//! (memory reuse). If the protocol ever lets the writer free a block a reader
//! is dereferencing, those two accesses race, and loom reports it as a data
//! race across ALL interleavings — including the store-buffering (SB) outcome
//! that plain Release/Acquire permits and only a SeqCst fence forbids.
//!
//! `fenced_protocol_has_no_uaf` proves the fixed protocol is race-free.
//! `unfenced_protocol_exhibits_uaf` proves the model has teeth: without the
//! fences, loom DOES find the SB race — i.e. the fence is load-bearing, not
//! decoration.
#![cfg(loom)]

use loom::cell::UnsafeCell;
use loom::sync::atomic::{fence, AtomicU64, Ordering};
use loom::sync::Arc;
use loom::thread;

const IDLE: u64 = u64::MAX;

/// One reader, one writer, two blocks (epochs 1 and 2). The writer publishes
/// block 1, supersedes it with block 2 (retiring block 1 at epoch 1), then
/// tries to reclaim block 1. The reader pins, reads the head, and dereferences
/// whichever block it observed. Only block 1 is ever freed, so only a read of
/// block 1 concurrent with its free is a UAF.
fn run_protocol(use_fence: bool) {
    loom::model(move || {
        let head = Arc::new(AtomicU64::new(0)); // 0 = empty, else epoch
        let slot = Arc::new(AtomicU64::new(IDLE)); // reader floor pin
        let block1 = Arc::new(UnsafeCell::new(1u64)); // block 1 payload

        let writer = {
            let head = head.clone();
            let slot = slot.clone();
            let block1 = block1.clone();
            thread::spawn(move || {
                head.store(1, Ordering::Release); // publish block 1
                head.store(2, Ordering::Release); // supersede -> retire block 1 (epoch 1)
                if use_fence {
                    fence(Ordering::SeqCst); // writer-side hazard barrier
                }
                let floor = slot.load(Ordering::Acquire); // floor scan
                if floor > 1 {
                    // Protocol says: no live reader pins epoch <= 1, so block 1
                    // is safe to free. Model the free as memory reuse: a
                    // non-atomic write. If the protocol is wrong, this races the
                    // reader's dereference below.
                    block1.with_mut(|p| unsafe { *p = 0xDEAD });
                }
            })
        };

        let reader = {
            let head = head.clone();
            let slot = slot.clone();
            let block1 = block1.clone();
            thread::spawn(move || {
                slot.store(1, Ordering::Release); // conservative pin (>= 1)
                if use_fence {
                    fence(Ordering::SeqCst); // reader-side hazard barrier
                }
                let h = head.load(Ordering::Acquire);
                if h == 1 {
                    slot.store(1, Ordering::Release); // tighten to epoch 1
                    // Dereference block 1 (non-atomic read). Must not race a free.
                    block1.with(|p| unsafe {
                        let _ = *p;
                    });
                    slot.store(IDLE, Ordering::Release); // release pin
                }
                // h == 2: reader observed the new head; it would pin block 2,
                // which is never freed in this model — no access to block 1.
            })
        };

        writer.join().unwrap();
        reader.join().unwrap();
    });
}

#[test]
fn fenced_protocol_has_no_uaf() {
    // The production fix: SeqCst fences on both sides. loom must find NO
    // interleaving in which the writer's free races the reader's dereference.
    run_protocol(true);
}

#[test]
fn unfenced_protocol_exhibits_uaf() {
    // Without the fences the reader's (pin; load head) and the writer's
    // (store head; load floor) form a store-buffering pattern: loom MUST find
    // an interleaving where the writer frees block 1 while the reader is
    // dereferencing it. We expect loom to abort the model with a race/UAF,
    // which surfaces as a panic. If it does NOT, the model has no teeth and the
    // fenced result would be meaningless — so we assert the failure occurred.
    let found_bug = std::panic::catch_unwind(|| run_protocol(false)).is_err();
    assert!(
        found_bug,
        "loom did NOT find the expected store-buffering UAF without the fence — \
         model lacks teeth; the fenced pass cannot be trusted"
    );
}

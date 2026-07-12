//! AXVERITY_HOTMEM_CONSUMER_IMPLEMENTATION_V1 — hot mem arena, first slice.
//!
//! Exposes `non_blocking_memory::BridgedCell`/`ReaderRegistry` (already
//! built, already tested — 66 tests, ASan-clean, see that module's own doc
//! comment) to M1 as a single process-wide, epoch-gated, best-effort cache
//! fed asynchronously from the WAL write path. This module owns NO new
//! reclamation logic — it only wires the existing engine to the bridge's
//! `fn(Value) -> Value` dispatch ABI and picks the concurrency discipline
//! that makes doing so sound.
//!
//! ## SOUNDNESS — resolved by the Writer/shared-cell split
//! (AXVERITY_HOTMEM_ARENA_SOUNDNESS_FIX_V1)
//!
//! This module previously wrapped the arena in `UnsafeCell<BridgedCell>` with a
//! manually-asserted `unsafe impl Sync`, synthesising a `&mut BridgedCell` for
//! the writer thread while reader threads held `&BridgedCell` into the SAME
//! allocation. That is textbook `noalias` UB — a live `&mut T` on one thread
//! aliasing a live `&T` on another licenses the compiler to miscompile,
//! regardless of how correct the underlying atomic head/floor protocol is. It
//! reproduced as invalid-UTF8 reads escalating to SIGSEGV within seconds under
//! pure concurrent write/read (see `uaf_isolation_probe` below).
//!
//! The fix (in `non_blocking_memory.rs`): `BridgedCell` was split so the shared
//! half holds ONLY the atomic head (`Cell<T>`) and auto-derives `Sync` with no
//! `unsafe` and no `UnsafeCell`, while all writer-private state (the retired
//! list, sweep counters) and every mutating/reclaiming method moved to a
//! separate `Writer<T>`. `Writer` is `!Clone` + `!Sync` (so a second writer, or
//! a shared `&Writer`, is a *type error*) and `Send` (so the one handle can be
//! moved onto the janitor thread). The mutators take `&mut self` on the
//! `Writer`, so the widest `&mut` ever formed is `&mut Vec<RetiredEntry>` —
//! memory no reader references. `&mut BridgedCell` is never formed anywhere, so
//! the aliasing UB is structurally impossible.
//!
//! Consequently THIS module no longer needs any `unsafe impl`: the shared cell
//! lives behind `Arc<BridgedCell>` (auto-`Sync`) in `ARENA_SHARED`; readers
//! hold `&BridgedCell` via the `Arc`; the single `Writer` lives in the janitor
//! thread's TLS (it is `!Sync`, so cannot sit in a `static`). No `Mutex` — the
//! rejected direction ("no mutex, ever", Chris 2026-07-09) — is used or
//! reintroduced.
//!
//! Acceptance: `uaf_isolation_probe` below runs with ZERO changes to its test
//! body and must now PASS (it is no longer `#[ignore]`d). That was the
//! pre-registered acceptance check for this redesign.
//!
//! Earlier bug (also fixed, kept for the record): `hotmem_read` once called the
//! owned, pins-nothing `BridgedCell::read()`, so `write_lazy`'s immediate-free
//! optimisation always believed no reader existed and freed the superseded
//! Block right away — a plain use-after-free (`free(): unaligned chunk detected
//! in tcache 2` within ~3s under concurrent INSERT load). `hotmem_read` uses
//! `read_ref()`, which pins the reader's floor before dereferencing and
//! releases it only after the byte copy is made.
//!
//! **Exactly one thread ever calls `hotmem_write`: the dedicated
//! "hotmem-frame" channel janitor thread.** Every producer (pg_server workers,
//! CLI one-shots) only ever calls `channel_send` — fire-and-forget, never
//! touches the arena directly. This invariant is now BACKED by the type system
//! for the shared cell (`Writer` is the sole mutate capability and is `!Clone`
//! `!Sync`); the one residual `unsafe` is the narrow `unsafe impl Send for
//! Writer` (exclusive-ownership move), audited in `non_blocking_memory.rs`.
//!
//! The arena is explicitly NOT durable and NOT critical — the WAL is still
//! the durability boundary (see `wal_put.m1`'s `channel_send` hook); losing
//! an arena update, or the whole arena on process death, is by design a
//! no-op for correctness. That is what makes the best-effort `channel_send`
//! hand-off (rather than a blocking/acked path) the right choice here.

use std::cell::{Cell as StdCell, RefCell};
use std::sync::{Arc, OnceLock};

use super::non_blocking_memory::{BridgedCell, ReaderHandle, ReaderRegistry, Writer};
use super::value::Value;

/// The arena payload: a shared reference to the exact byte buffer the WAL
/// write already built. Moving an `Arc` into `write_lazy` is a refcount
/// bump, never a copy of the underlying bytes — the arena never owns a
/// second copy of anything a writer produced.
type Payload = Arc<Vec<u8>>;

/// The reader-facing shared arena cell. `None` until the sole writer thread's
/// first `hotmem_write` mints the `(Arc<BridgedCell>, Writer)` pair and
/// publishes the `Arc` here. Readers only ever observe it through a shared
/// `&BridgedCell` — no `&mut`, no `UnsafeCell`. `Arc<BridgedCell<Payload>>` is
/// auto-`Sync`, so the old `unsafe impl Sync for Arena` is gone entirely.
static ARENA_SHARED: OnceLock<Arc<BridgedCell<Payload>>> = OnceLock::new();
static READERS: OnceLock<ReaderRegistry> = OnceLock::new();

fn readers() -> &'static ReaderRegistry {
    READERS.get_or_init(ReaderRegistry::new)
}

thread_local! {
    /// This thread's reader slot, lazily acquired. `ReaderHandle`'s Drop
    /// releases the floor the instant the thread exits (normal or panic
    /// unwind) — see `ReaderRegistry::acquire`'s own doc comment.
    static READER: RefCell<Option<ReaderHandle<'static>>> = const { RefCell::new(None) };

    /// (epoch, missed) from this thread's most recent `hotmem_read` — M1
    /// has no tuples, so `hotmem_epoch`/`hotmem_missed` split this out.
    static LAST: StdCell<(i64, i64)> = const { StdCell::new((0, 0)) };

    /// The sole writer capability, lazily minted on this thread's first
    /// `hotmem_write`. Lives in TLS because `Writer` is `!Sync` and cannot sit
    /// in a plain `static`; the single-janitor-thread invariant means exactly
    /// one thread ever populates this. The paired `Arc` is published to
    /// `ARENA_SHARED` at the same moment (first-writer-wins).
    static WRITER: RefCell<Option<Writer<Payload>>> = const { RefCell::new(None) };
}

/// `hotmem_write(arg: Value) -> Value`   —   List(Bytes) -> Unit
///
/// The `wait()` handler for the "hotmem-frame" channel. `wait()` drains
/// every pending message into one batch per call (see `channels.rs`) and
/// hands the whole batch to ONE handler invocation — mirrors
/// `wal_fast_batch_write` in `logbuf.rs` exactly, same reason: the batch
/// already arrived as owned, final values, so writing each straight into
/// the arena needs no intermediate accumulator.
///
/// SAFETY: must only ever be reachable as the wait-handler on the single
/// dedicated hotmem-janitor thread. See module doc comment.
#[track_caller]
pub fn hotmem_write(arg: Value) -> Value {
    let items = match arg {
        Value::List(items) => items,
        other => panic!("hotmem_write: expected List, got {:?}", other),
    };
    if items.is_empty() {
        return Value::Unit;
    }
    let reg = readers();
    WRITER.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            // First write on this (the sole janitor) thread: mint the paired
            // (shared cell, writer) and publish the Arc for readers. First
            // writer wins; a stray second writer thread (a contract violation)
            // would get its own orphan cell no reader observes — harmless
            // rather than UB, which is strictly better than the old shared
            // `&mut` scheme.
            let (cell, writer) = BridgedCell::new();
            let _ = ARENA_SHARED.set(cell);
            *w = Some(writer);
        }
        let writer = w.as_mut().expect("writer just initialised above");
        for item in items {
            let bytes = match item {
                Value::Bytes(b) => b,
                other => panic!("hotmem_write: expected Bytes item, got {:?}", other),
            };
            // SAFETY: sole `&mut` owner of `writer` on the single writer
            // thread — `write_lazy`'s single-writer contract is upheld.
            unsafe {
                writer.write_lazy(Arc::new(bytes), reg);
            }
        }
        // Opportunistic, watermark-gated — bounds retired-list growth without
        // scanning reader floors on every single write. See
        // `Writer::reclaim_if_watermark`'s own doc comment.
        writer.reclaim_if_watermark(reg);
    });
    Value::Unit
}

/// `hotmem_reader_start(arg: Value) -> Value`   —   Unit -> Unit
///
/// Lazily acquires this thread's `ReaderHandle`, idempotent. Call once per
/// consumer thread before the first `hotmem_read`.
pub fn hotmem_reader_start(_arg: Value) -> Value {
    READER.with(|r| {
        if r.borrow().is_none() {
            *r.borrow_mut() = Some(readers().acquire());
        }
    });
    Value::Unit
}

/// `hotmem_read(arg: Value) -> Value`   —   Int(last_epoch) -> Bytes
///
/// Pinned read (`BridgedCell::read_ref`, NOT `BridgedCell::read`). Still
/// materializes an owned `Bytes` for the M1/Rust FFI boundary (M1's `Bytes`
/// ABI is an owned buffer), but does so through the floor-pinning path so
/// the writer's `write_lazy` immediate-free optimization cannot free the
/// Block out from under a read in progress.
///
/// FIXED (use-after-free): this previously called the "owned, pins-nothing"
/// `BridgedCell::read()` fast path. That path's own doc comment
/// (non_blocking_memory.rs, `BridgedCell::read`'s "F2 HAZARD" note) warns it
/// is sound ONLY when the single-writer contract structurally prevents
/// concurrent reclamation — NOT the case here, since this module
/// deliberately shares the arena across threads via `UnsafeCell` so one
/// janitor thread writes while other threads read concurrently. Because
/// `hotmem_read` never pinned a floor, `write_lazy` (in `hotmem_write`,
/// above) always saw `floor_min() == u64::MAX` and freed the superseded
/// Block immediately — racing a concurrent `read()` that had already loaded
/// the (now-dangling) pointer but not yet finished cloning out of it.
/// Reproduced empirically as glibc-detected heap corruption
/// (`free(): unaligned chunk detected in tcache 2`) within seconds under
/// concurrent INSERT load. `read_ref` pins the reader's floor BEFORE the
/// pointer is dereferenced, so `write_lazy` retires (rather than frees) any
/// Block a live read might still reach; the pin is released only after this
/// function has already copied the bytes out.
///
/// Auto-acquires a reader handle if `hotmem_reader_start` was not called
/// first, so callers who only ever read from one thread can skip the
/// explicit start call. Returns empty `Bytes` if the cell has never been
/// written.
pub fn hotmem_read(arg: Value) -> Value {
    let last_epoch = match arg {
        Value::Int(n) => n.max(0) as u64,
        other => panic!("hotmem_read: expected Int, got {:?}", other),
    };
    // Shared `&BridgedCell` — never `&mut`, never `UnsafeCell`. `None` means no
    // writer has published yet (arena empty).
    let cell = match ARENA_SHARED.get() {
        Some(cell) => cell,
        None => {
            LAST.with(|l| l.set((0, 0)));
            return Value::Bytes(Vec::new());
        }
    };
    READER.with(|r| {
        if r.borrow().is_none() {
            *r.borrow_mut() = Some(readers().acquire());
        }
        let borrowed = r.borrow();
        let handle = borrowed
            .as_ref()
            .expect("hotmem_read: reader handle just acquired above");
        match cell.read_ref(handle, last_epoch) {
            None => {
                LAST.with(|l| l.set((0, 0)));
                Value::Bytes(Vec::new())
            }
            Some(read_ref) => {
                let epoch = read_ref.epoch;
                let missed = read_ref.missed;
                // Copy out while the floor is still pinned; `read_ref`'s
                // Drop (end of this arm) releases the pin only after this
                // clone is done — this ordering is what closes the race.
                let bytes = (**read_ref).clone();
                LAST.with(|l| l.set((epoch as i64, missed as i64)));
                Value::Bytes(bytes)
            }
        }
    })
}

/// `hotmem_epoch(arg: Value) -> Value`   —   Unit -> Int
///
/// Epoch from this thread's most recent `hotmem_read`.
pub fn hotmem_epoch(_arg: Value) -> Value {
    Value::Int(LAST.with(|l| l.get().0))
}

/// `hotmem_missed(arg: Value) -> Value`   —   Unit -> Int
///
/// Writes missed since the previous `hotmem_read`'s `last_epoch`, as of
/// this thread's most recent `hotmem_read` call. 0 == fully current.
pub fn hotmem_missed(_arg: Value) -> Value {
    Value::Int(LAST.with(|l| l.get().1))
}

// KNOWN-FAILING regression probe for the open UnsafeCell-aliasing soundness
// issue documented at the top of this module. Isolates hotmem_write/
// hotmem_read from SQL parsing, the wire protocol, and reclog entirely —
// pure concurrent write/read against this module's own arena. `#[ignore]`d
// so a normal `cargo test` run stays green; run explicitly
// (`cargo test --release --lib uaf_isolation_probe -- --ignored --nocapture`)
// to reproduce the corruption against the current UnsafeCell scheme, or to
// confirm a future lock-free redesign actually closes it (this test should
// go from failing to passing with ZERO changes to the test itself once the
// redesign lands — that's the acceptance check).
#[cfg(test)]
mod uaf_isolation_probe {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtoOrd};
    use std::sync::Arc as StdArc;
    use std::thread;
    use std::time::Duration;

    // ACCEPTANCE TEST (AXVERITY_HOTMEM_ARENA_SOUNDNESS_FIX_V1): formerly
    // `#[ignore]`d as known-failing against the UnsafeCell scheme. The redesign
    // must make it pass with ZERO changes to the body below — that is the
    // pre-registered gate. Un-ignored deliberately.
    #[test]
    fn concurrent_write_read_never_corrupts() {
        let stop = StdArc::new(AtomicBool::new(false));
        let bad = StdArc::new(AtomicUsize::new(0));
        let writes = StdArc::new(AtomicUsize::new(0));
        let reads = StdArc::new(AtomicUsize::new(0));
        let nonempty_reads = StdArc::new(AtomicUsize::new(0));

        const TAG_LEN: usize = "PAYLOAD-00000000000000000000-END".len();

        let w_stop = stop.clone();
        let w_writes = writes.clone();
        let writer = thread::spawn(move || {
            let mut i: u64 = 0;
            while !w_stop.load(AtoOrd::Relaxed) {
                let payload = format!("PAYLOAD-{:020}-END", i).into_bytes();
                hotmem_write(Value::List(vec![Value::Bytes(payload)]));
                w_writes.fetch_add(1, AtoOrd::Relaxed);
                i += 1;
            }
        });

        let r_stop = stop.clone();
        let r_bad = bad.clone();
        let r_reads = reads.clone();
        let r_nonempty = nonempty_reads.clone();
        let reader = thread::spawn(move || {
            let mut last_epoch: i64 = 0;
            while !r_stop.load(AtoOrd::Relaxed) {
                let v = hotmem_read(Value::Int(last_epoch));
                r_reads.fetch_add(1, AtoOrd::Relaxed);
                if let Value::Bytes(b) = v {
                    if !b.is_empty() {
                        r_nonempty.fetch_add(1, AtoOrd::Relaxed);
                        match String::from_utf8(b) {
                            Ok(s) => {
                                let ok = s.len() == TAG_LEN
                                    && s.starts_with("PAYLOAD-")
                                    && s.ends_with("-END");
                                if !ok {
                                    eprintln!("MALFORMED (valid utf8 but wrong shape): {:?}", s);
                                    r_bad.fetch_add(1, AtoOrd::Relaxed);
                                }
                            }
                            Err(e) => {
                                eprintln!("INVALID UTF-8: {}", e);
                                r_bad.fetch_add(1, AtoOrd::Relaxed);
                            }
                        }
                    }
                } else {
                    eprintln!("hotmem_read returned non-Bytes: {:?}", v);
                    r_bad.fetch_add(1, AtoOrd::Relaxed);
                }
                last_epoch = LAST.with(|l| l.get().0);
            }
        });

        thread::sleep(Duration::from_secs(4));
        stop.store(true, AtoOrd::Relaxed);
        writer.join().unwrap();
        reader.join().unwrap();

        let w = writes.load(AtoOrd::Relaxed);
        let r = reads.load(AtoOrd::Relaxed);
        let ne = nonempty_reads.load(AtoOrd::Relaxed);
        let b = bad.load(AtoOrd::Relaxed);
        eprintln!("writes={w} reads={r} nonempty_reads={ne} bad={b}");
        assert_eq!(b, 0, "corrupted/malformed reads observed: {b}");
    }

    // Bounded, Miri-friendly variant of the probe above: fixed iteration
    // counts, no wall-clock sleep, content-validated. Small enough to run
    // under `cargo +nightly miri test` (tree-borrows) as an authoritative
    // UB / data-race check on the concurrent write/read path.
    #[test]
    fn concurrent_write_read_bounded() {
        const WRITES: u64 = if cfg!(miri) { 40 } else { 20_000 };
        const TAG_LEN: usize = "PAYLOAD-00000000000000000000-END".len();
        let done = StdArc::new(AtomicBool::new(false));
        let bad = StdArc::new(AtomicUsize::new(0));

        let w_done = done.clone();
        let writer = thread::spawn(move || {
            for i in 0..WRITES {
                let payload = format!("PAYLOAD-{:020}-END", i).into_bytes();
                hotmem_write(Value::List(vec![Value::Bytes(payload)]));
            }
            w_done.store(true, AtoOrd::Release);
        });

        let r_done = done.clone();
        let r_bad = bad.clone();
        let reader = thread::spawn(move || {
            let mut last_epoch: i64 = 0;
            loop {
                let v = hotmem_read(Value::Int(last_epoch));
                if let Value::Bytes(b) = v {
                    if !b.is_empty() {
                        let ok = match String::from_utf8(b) {
                            Ok(s) => {
                                s.len() == TAG_LEN
                                    && s.starts_with("PAYLOAD-")
                                    && s.ends_with("-END")
                            }
                            Err(_) => false,
                        };
                        if !ok {
                            r_bad.fetch_add(1, AtoOrd::Relaxed);
                        }
                    }
                }
                last_epoch = LAST.with(|l| l.get().0);
                if r_done.load(AtoOrd::Acquire) {
                    break;
                }
            }
        });

        writer.join().unwrap();
        reader.join().unwrap();
        assert_eq!(bad.load(AtoOrd::Relaxed), 0, "corrupted reads observed");
    }
}

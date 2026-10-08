//! FBANK_V1 — a frozen bank: append-only, deduplicating, shared by every thread, with no lock and no shared refcount.
//!
//! Why it exists: an `Arc`-shared value is immutable, but every by-value read still WRITES its refcount, so threads
//! reading the same strings contend on the same cache lines. Measured for axMachina P5 (2026-10-08, 16 cores / 32
//! threads): apply+pack over one Arc-shared TextList scaled 5.1x at 32 tasks; the same work on a private copy per task
//! scaled 15.9x. Here a reader copies the entry's bytes out into its own fresh Text and writes nothing shared.
//!
//! Shape:
//!   - `fbank_new(max_entries, max_bytes)` reserves everything up front, zeroed (the OS commits pages only as they are
//!     touched). The bank is never freed (like `cell_new_raw`'s cells); its Int is its address, valid in any thread.
//!   - `fbank_put(b, ordinal, text)` appends `text` and dedupes it on content. Space comes from two fetch-adds (entry
//!     id, byte cursor); the bytes go into the writer's own range; the entry is published by a CAS into an
//!     open-addressing table. When equal content is put twice, the LOWER ordinal holds it, whatever the thread
//!     timing: a higher-ordinal holder is displaced by CAS, a higher-ordinal newcomer is refused. So once every put
//!     has finished (a `join`), who holds each content is exactly what a sequential first-seen pass in ordinal order
//!     would have kept.
//!   - Answers: the new id (it holds the content now; `fbank_holds` says whether it still does once puts finish),
//!     -1 (a lower ordinal already holds equal content), decided by a read-only probe when it can be (no id, no bytes), -2 (the bank is full: `max_entries` or `max_bytes` reached).
//!   - `fbank_get(b, id)` is a private copy of the entry's text.
//!
//! After a -2 the bank is full for good (the refused put still used an id); a caller treats it as Unknown(memory).
//!
//! Lock-free, not wait-free: a CAS that loses re-examines its slot. Ids are only meaningful when obtained from
//! `fbank_put` (or `fbank_len` after a join); an invented id panics when out of range.
//!
//! Memory: max_bytes + 32 * max_entries + 8 * table (table = smallest power of two >= 2 * max_entries).

use std::alloc::{alloc_zeroed, Layout};
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};

use super::value::{intern_str, Value};

struct Bank {
    max_entries: usize,
    max_bytes: usize,
    bytes: *mut u8,
    cursor: AtomicUsize,
    count: AtomicUsize,
    off: &'static [AtomicU64],
    len: &'static [AtomicU64],
    hash: &'static [AtomicU64],
    ord: &'static [AtomicI64],
    table: &'static [AtomicU64], // entry id + 1; 0 = empty
    mask: usize,
}

// SAFETY: `bytes` is written only in ranges handed out by `cursor.fetch_add` (disjoint per writer) and read only
// for entries published through `table` with Release/Acquire, so no byte is read while it is written.
unsafe impl Send for Bank {}
unsafe impl Sync for Bank {}

/// A zeroed, leaked slice of `n` atomics (all-zero bytes are a valid atomic zero).
fn zeroed<T>(n: usize) -> &'static [T] {
    let n = n.max(1);
    let layout = Layout::array::<T>(n).unwrap_or_else(|_| panic!("fbank_new: {n} entries do not fit in memory"));
    let p = unsafe { alloc_zeroed(layout) } as *mut T;
    if p.is_null() { panic!("fbank_new: could not reserve {} bytes", layout.size()); }
    unsafe { std::slice::from_raw_parts(p, n) }
}

fn bank(b: i64) -> &'static Bank {
    if b == 0 { panic!("fbank: 0 is not a bank"); }
    unsafe { &*(b as *const Bank) }
}

fn hash_of(data: &[u8]) -> u64 {
    // Fixed keys: deterministic across runs and threads.
    let mut h = std::hash::BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default().build_hasher();
    h.write(data);
    h.finish()
}

impl Bank {
    fn data(&self, id: usize) -> &[u8] {
        let off = self.off[id].load(Ordering::Relaxed) as usize;
        let len = self.len[id].load(Ordering::Relaxed) as usize;
        unsafe { std::slice::from_raw_parts(self.bytes.add(off), len) }
    }
    fn published(&self) -> usize { self.count.load(Ordering::Acquire).min(self.max_entries) }
    fn check(&self, fname: &str, id: i64) -> usize {
        if id < 0 || id as usize >= self.published() {
            panic!("{fname}: id {id} is not an entry of this bank ({} entries)", self.published());
        }
        id as usize
    }
}

/// `fbank_new(max_entries: Int, max_bytes: Int) -> Int` — a new empty bank; its address.
#[track_caller]
pub fn fbank_new(max_entries: i64, max_bytes: i64) -> Value {
    if max_entries < 1 || max_bytes < 0 {
        panic!("fbank_new: max_entries must be >= 1 and max_bytes >= 0 (got {max_entries}, {max_bytes})");
    }
    let n = max_entries as usize;
    let slots = (2 * n).next_power_of_two();
    let b = Bank {
        max_entries: n,
        max_bytes: max_bytes as usize,
        bytes: zeroed::<u8>(max_bytes as usize).as_ptr() as *mut u8,
        cursor: AtomicUsize::new(0),
        count: AtomicUsize::new(0),
        off: zeroed(n),
        len: zeroed(n),
        hash: zeroed(n),
        ord: zeroed(n),
        table: zeroed(slots),
        mask: slots - 1,
    };
    Value::Int(Box::leak(Box::new(b)) as *const Bank as i64)
}

/// `fbank_put(b: Int, ordinal: Int, text: Text) -> Int` — id | -1 (a lower ordinal holds equal content) | -2 (full).
#[track_caller]
pub fn fbank_put(b: i64, ordinal: i64, text: std::sync::Arc<str>) -> Value {
    let bk = bank(b);
    let data = text.as_bytes();
    let h = hash_of(data);
    // Read-only probe first: a put a lower ordinal already refuses costs no id and no bytes.
    let mut i = (h as usize) & bk.mask;
    loop {
        let cur = bk.table[i].load(Ordering::Acquire);
        if cur == 0 { break; }
        let c = (cur - 1) as usize;
        if bk.hash[c].load(Ordering::Relaxed) == h && bk.data(c) == data {
            if bk.ord[c].load(Ordering::Relaxed) <= ordinal { return Value::Int(-1); }
            break;
        }
        i = (i + 1) & bk.mask;
    }
    let id = bk.count.fetch_add(1, Ordering::AcqRel);
    if id >= bk.max_entries { return Value::Int(-2); }
    let off = bk.cursor.fetch_add(data.len(), Ordering::AcqRel);
    if off + data.len() > bk.max_bytes { return Value::Int(-2); }
    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), bk.bytes.add(off), data.len()); }
    bk.off[id].store(off as u64, Ordering::Relaxed);
    bk.len[id].store(data.len() as u64, Ordering::Relaxed);
    bk.hash[id].store(h, Ordering::Relaxed);
    bk.ord[id].store(ordinal, Ordering::Relaxed);
    let mine = id as u64 + 1;
    let mut i = (h as usize) & bk.mask;
    loop {
        let cur = bk.table[i].load(Ordering::Acquire);
        if cur == 0 {
            match bk.table[i].compare_exchange(0, mine, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Value::Int(id as i64),
                Err(_) => continue, // someone took this slot: look at what they put there
            }
        }
        let c = (cur - 1) as usize;
        if bk.hash[c].load(Ordering::Relaxed) == h && bk.data(c) == data {
            if bk.ord[c].load(Ordering::Relaxed) <= ordinal { return Value::Int(-1); }
            match bk.table[i].compare_exchange(cur, mine, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Value::Int(id as i64),
                Err(_) => continue, // displaced meanwhile: compare against the new holder
            }
        }
        i = (i + 1) & bk.mask;
    }
}

/// `fbank_holds(b: Int, id: Int) -> Bool` — whether entry `id` holds its content (true for at most one entry per
/// content; final once every put has finished).
#[track_caller]
pub fn fbank_holds(b: i64, id: i64) -> Value {
    let bk = bank(b);
    let e = bk.check("fbank_holds", id);
    let h = bk.hash[e].load(Ordering::Relaxed);
    let data = bk.data(e);
    let mut i = (h as usize) & bk.mask;
    loop {
        let cur = bk.table[i].load(Ordering::Acquire);
        if cur == 0 { return Value::Bool(false); }
        let c = (cur - 1) as usize;
        if bk.hash[c].load(Ordering::Relaxed) == h && bk.data(c) == data { return Value::Bool(c == e); }
        i = (i + 1) & bk.mask;
    }
}

/// `fbank_get(b: Int, id: Int) -> Text` — entry `id`'s text, as a private copy.
#[track_caller]
pub fn fbank_get(b: i64, id: i64) -> Value {
    let bk = bank(b);
    let e = bk.check("fbank_get", id);
    Value::Str(intern_str(unsafe { std::str::from_utf8_unchecked(bk.data(e)) }))
}

/// `fbank_ordinal(b: Int, id: Int) -> Int` — the ordinal entry `id` was put with.
#[track_caller]
pub fn fbank_ordinal(b: i64, id: i64) -> Value {
    let bk = bank(b);
    let e = bk.check("fbank_ordinal", id);
    Value::Int(bk.ord[e].load(Ordering::Relaxed))
}

/// `fbank_len(b: Int) -> Int` — entries put so far (holders, refused and displaced alike); ids are 0 .. len-1.
#[track_caller]
pub fn fbank_len(b: i64) -> Value {
    Value::Int(bank(b).published() as i64)
}

/// `fbank_used(b: Int) -> Int` — bytes of text stored so far (at most max_bytes).
#[track_caller]
pub fn fbank_used(b: i64) -> Value {
    let bk = bank(b);
    Value::Int(bk.cursor.load(Ordering::Acquire).min(bk.max_bytes) as i64)
}

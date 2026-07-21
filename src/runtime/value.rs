use std::sync::{OnceLock, Mutex, Arc};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::collections::HashMap;
pub use rust_decimal::Decimal;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    // M1_VALUE_STR_ARC_IMPLEMENTATION_V1: Str now carries the string inline as
    // Arc<str> (cheap clone via atomic refcount, no shared interner). Replaces
    // the former Str(u32) handle into the global Mutex-guarded STRING_TABLE —
    // the Spike-4 P>=8 contention source on the str path. Arc<str> is Send+Sync
    // and derives Clone/Debug/PartialEq (PartialEq compares str contents, which
    // preserves the interner's equal-string => equal-value semantics).
    Str(Arc<str>),
    Unit,
    Tuple(Vec<Value>),
    List(Vec<Value>),
    Ctor { tag: u32, fields: Vec<Value> },
    // BRIDGE_VALUE_COERCION_V1: numeric tags for the to_dec / to_float family.
    // Dec is rust_decimal::Decimal (128-bit fixed decimal, ~28 significant digits).
    // Float is IEEE 754 f64. Float-to-Dec via `Decimal::from_f64_retain` preserves
    // significant digits within Decimal's range; values outside the range panic.
    Dec(Decimal),
    Float(f64),
    // BRIDGE_BYTES_IO_M1: opaque byte blob (PrimCode::Bytes=4). Carrier for
    // fs_read_bytes / fs_write_bytes / text_to_bytes. NOT a List<Int> — kept
    // as Vec<u8> so the bridge can pass blobs without per-element overhead.
    Bytes(Vec<u8>),
}

// M1_VALUE_STR_ARC_IMPLEMENTATION_V1 hard invariant (VALUE_MUST_STAY_SEND_SYNC):
// Value crosses thread boundaries via main.rs's `--entries` thread::spawn
// closures and channels.rs's Mutex<VecDeque<Value>>. Arc<str> is Send + Sync,
// so Value remains Send + Sync. This assertion makes that a compile-time gate:
// if any future field breaks it, the crate fails to build here.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Value>();
};

impl Value {
    #[track_caller]
    pub fn as_int(&self) -> i64 {
        match self {
            Value::Int(n) => *n,
            _ => panic!("expected Int, got {:?}", self),
        }
    }

    #[track_caller]
    pub fn as_bool(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            _ => panic!("expected Bool, got {:?}", self),
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Int(n)    => write!(f, "{}", n),
            Value::Bool(b)   => write!(f, "{}", b),
            Value::Str(h)    => write!(f, "{}", get_str(h)),
            Value::Unit      => write!(f, "()"),
            Value::Dec(d)    => write!(f, "{}", d),
            Value::Float(x)  => write!(f, "{}", x),
            Value::Tuple(es) => {
                write!(f, "(")?;
                for (i, e) in es.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", e)?;
                }
                write!(f, ")")
            }
            Value::List(es) => {
                write!(f, "[")?;
                for (i, e) in es.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", e)?;
                }
                write!(f, "]")
            }
            Value::Ctor { tag, fields } => {
                write!(f, "{}(", get_tag_name(*tag))?;
                for (i, fld) in fields.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", fld)?;
                }
                write!(f, ")")
            }
            Value::Bytes(bs) => write!(f, "<{} bytes>", bs.len()),
        }
    }
}

#[track_caller]
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b)    => *b,
        Value::Int(n)     => *n != 0,
        Value::Str(h)     => !get_str(h).is_empty(),
        Value::Unit       => false,
        Value::Tuple(es)  => !es.is_empty(),
        Value::List(es)   => !es.is_empty(),
        Value::Ctor { .. } => true,
        Value::Dec(d)     => !d.is_zero(),
        Value::Float(x)   => *x != 0.0,
        Value::Bytes(bs)  => !bs.is_empty(),
    }
}

// ── String values (M1_VALUE_STR_ARC_IMPLEMENTATION_V1) ───────────────────────
//
// Str values are carried inline as Arc<str>. There is no longer a global
// STRING_TABLE / STRING_MAP interner — that Mutex-guarded shared structure was
// the str-path contention source and is removed entirely (no shared, mutable,
// lock-guarded structure remains here). `intern_str` and `get_str` are retained
// only as thin constructor/accessor shims so the ~200 existing call sites keep
// compiling; the names are historical (there is no interning anymore).
//
// `intern_str` allocates a fresh Arc<str> from the given string. Callers that
// build a Value::Str do `Value::Str(intern_str(s))`, unchanged from before.

#[track_caller]
pub fn intern_str(s: &str) -> Arc<str> {
    Arc::from(s)
}

// `get_str` returns an owned String from anything str-like — an owned
// `Arc<str>` (from a by-value `Value::Str(h)` match), a `&Arc<str>` (from a
// by-reference match), a `&str`, or a `String`. The clone-on-read behavior of
// the old handle-based implementation is preserved: callers still receive a
// fresh owned String, so no aliasing/isolation assumptions change.
#[track_caller]
pub fn get_str<S: AsRef<str>>(s: S) -> String {
    s.as_ref().to_string()
}

// ── Tag table ────────────────────────────────────────────────────────────────
//
// The tag interner maps a Ctor's name (compile-time-ish literal like "Some",
// "CIf", "ChannelMsg") to a small dense u32 that is embedded IN the Value and
// crosses thread boundaries (see VALUE_MUST_STAY_SEND_SYNC above + channels.rs's
// Value queue). So the u32->name mapping MUST be globally consistent across all
// threads: a thread-local table would resolve a Value received from another
// thread to the wrong (or no) name. Thread-local is therefore a CONTRACT
// VIOLATION here, not merely a slower option — the interner must stay SHARED.
//
// AXVERITY_BRIDGE_LOCKFREE_EXPERIMENT_V1 candidate #3: two SHARED implementations
// selected by the `AXVERITY_TAG_INTERNER` env flag, current Mutex path the
// default fallback. Both stores are process-global and independent; the flag is
// read once (OnceLock) so a process uses exactly one, never a mix.
//
//   * `mutex`    (default) — the original `Mutex<Vec>` + `Mutex<HashMap>` dedup.
//                Every intern/get takes a lock, INCLUDING the common
//                already-interned dedup-HIT read path. This is the exact shape
//                of the pre-migration STRING_TABLE that Spike-4 found collapses
//                at P>=8 (interner_contention.rs measures it via `run_tag`).
//   * `lockfree` — RCU: an immutable `TagSnapshot { names, map }` published via
//                an `AtomicPtr`. Reads (`get_tag_name`, and `intern_tag`'s
//                dedup-HIT fast path) load the pointer Acquire and never lock.
//                A genuinely-new tag takes the (uncontended: new tags are rare,
//                write-once, ~dozens per process) `LF_WRITE` mutex, re-checks,
//                clones the snapshot + appends, and publishes the new pointer
//                Release. The prior snapshot is INTENTIONALLY LEAKED (never
//                freed) so a reader holding a raw `&` can never hit a UAF — the
//                no-reclaim RCU guarantee. Bounded: leaks are one small snapshot
//                per DISTINCT tag ever seen (a tiny, write-once set), zero on the
//                steady-state read path.

// ---- shared flag (read once) ----
fn tag_interner_lockfree() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| matches!(std::env::var("AXVERITY_TAG_INTERNER").as_deref(), Ok("lockfree")))
}

#[track_caller]
pub fn intern_tag(name: &str) -> u32 {
    if tag_interner_lockfree() { intern_tag_lockfree(name) } else { intern_tag_mutex(name) }
}

#[track_caller]
pub fn get_tag_name(tag: u32) -> String {
    if tag_interner_lockfree() { get_tag_name_lockfree(tag) } else { get_tag_name_mutex(tag) }
}

// ---- variant `mutex` (default, preserved verbatim) ----
static TAG_TABLE: OnceLock<Mutex<Vec<String>>>         = OnceLock::new();
static TAG_MAP:   OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();

#[track_caller]
pub fn intern_tag_mutex(name: &str) -> u32 {
    let map = TAG_MAP.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = map.lock().unwrap();
    if let Some(&t) = map.get(name) { return t; }
    let tbl = TAG_TABLE.get_or_init(|| Mutex::new(Vec::new()));
    let mut tbl = tbl.lock().unwrap();
    let t = tbl.len() as u32;
    tbl.push(name.to_string());
    map.insert(name.to_string(), t);
    t
}

#[track_caller]
pub fn get_tag_name_mutex(tag: u32) -> String {
    let tbl = TAG_TABLE.get_or_init(|| Mutex::new(Vec::new()));
    let tbl = tbl.lock().unwrap();
    tbl.get(tag as usize).cloned().unwrap_or_else(|| "Unknown".to_string())
}

// ---- variant `lockfree` (RCU: lock-free reads, publish-on-new-tag) ----
struct TagSnapshot {
    names: Vec<String>,
    map:   HashMap<String, u32>,
}

/// The published head. Always non-null after first access (an empty snapshot is
/// installed on init). Swapped Release on each new-tag publish; loaded Acquire
/// on every read. Snapshots are leaked, never freed (RCU no-reclaim).
fn lf_head() -> &'static AtomicPtr<TagSnapshot> {
    static H: OnceLock<AtomicPtr<TagSnapshot>> = OnceLock::new();
    H.get_or_init(|| {
        let empty = Box::new(TagSnapshot { names: Vec::new(), map: HashMap::new() });
        AtomicPtr::new(Box::into_raw(empty))
    })
}

/// Serializes ONLY the new-tag publish (never the read path). Uncontended in
/// steady state — a genuinely new tag is a rare, ~startup-time event.
static LF_WRITE: Mutex<()> = Mutex::new(());

#[track_caller]
pub fn intern_tag_lockfree(name: &str) -> u32 {
    // Fast path: lock-free dedup-HIT (the common case the bench hammers).
    // SAFETY: the head pointer is non-null after init and points at a snapshot
    // that is never mutated in place and never freed, so the deref is sound.
    let snap = unsafe { &*lf_head().load(Ordering::Acquire) };
    if let Some(&t) = snap.map.get(name) { return t; }

    // Slow path: a new tag. Serialize the publish and re-check under the lock
    // (another thread may have installed `name` between the fast-path miss and
    // acquiring the lock).
    let _g = LF_WRITE.lock().unwrap_or_else(|p| p.into_inner());
    let cur = unsafe { &*lf_head().load(Ordering::Acquire) };
    if let Some(&t) = cur.map.get(name) { return t; }
    let t = cur.names.len() as u32;
    let mut names = cur.names.clone();
    let mut map = cur.map.clone();
    names.push(name.to_string());
    map.insert(name.to_string(), t);
    let raw = Box::into_raw(Box::new(TagSnapshot { names, map }));
    // Publish the new immutable snapshot; the old one is intentionally leaked.
    lf_head().store(raw, Ordering::Release);
    t
}

#[track_caller]
pub fn get_tag_name_lockfree(tag: u32) -> String {
    // SAFETY: as intern_tag_lockfree's fast path — non-null, immutable, never freed.
    let snap = unsafe { &*lf_head().load(Ordering::Acquire) };
    snap.names.get(tag as usize).cloned().unwrap_or_else(|| "Unknown".to_string())
}

// ── Process args (captured once at startup) ──────────────────────────────────

static PROCESS_ARGS: OnceLock<Vec<String>> = OnceLock::new();

#[track_caller]
pub fn init_runtime() {
    PROCESS_ARGS.get_or_init(|| std::env::args().collect());
    // STRING_TABLE / STRING_MAP removed: Value::Str is inline Arc<str> now.
}

#[track_caller]
pub fn get_process_args() -> &'static Vec<String> {
    PROCESS_ARGS.get_or_init(|| std::env::args().collect())
}

// ── Tag interner correctness (AXVERITY_BRIDGE_LOCKFREE_EXPERIMENT_V1, cand #3) ──
//
// Both variants share a process-global store that persists across tests in one
// binary, so assertions are RELATIVE (idempotence, injectivity, round-trip),
// never absolute u32 values, and each test uses a unique name prefix.
#[cfg(test)]
mod tag_interner_tests {
    use super::*;
    use std::collections::HashMap as Map;

    /// lock-free path: interning is idempotent, injective, and round-trips —
    /// the exact behavioral contract the mutex path provides.
    #[test]
    fn lockfree_idempotent_injective_roundtrip() {
        let names: Vec<String> = (0..40).map(|i| format!("lf_basic_{:03}", i)).collect();
        let ids: Vec<u32> = names.iter().map(|n| intern_tag_lockfree(n)).collect();
        // idempotent: re-intern returns the same id
        for (n, &id) in names.iter().zip(&ids) {
            assert_eq!(intern_tag_lockfree(n), id, "re-intern of {n} changed id");
        }
        // injective: distinct names -> distinct ids
        let uniq: std::collections::HashSet<u32> = ids.iter().copied().collect();
        assert_eq!(uniq.len(), ids.len(), "two names collided on one id");
        // round-trip: get_tag_name(id) == name
        for (n, &id) in names.iter().zip(&ids) {
            assert_eq!(&get_tag_name_lockfree(id), n);
        }
        // out-of-range -> "Unknown", matching the mutex path
        assert_eq!(get_tag_name_lockfree(u32::MAX), "Unknown");
    }

    /// mutex path: same behavioral contract (parity anchor for the lock-free one).
    #[test]
    fn mutex_idempotent_injective_roundtrip() {
        let names: Vec<String> = (0..40).map(|i| format!("mx_basic_{:03}", i)).collect();
        let ids: Vec<u32> = names.iter().map(|n| intern_tag_mutex(n)).collect();
        for (n, &id) in names.iter().zip(&ids) {
            assert_eq!(intern_tag_mutex(n), id);
        }
        let uniq: std::collections::HashSet<u32> = ids.iter().copied().collect();
        assert_eq!(uniq.len(), ids.len());
        for (n, &id) in names.iter().zip(&ids) {
            assert_eq!(&get_tag_name_mutex(id), n);
        }
        assert_eq!(get_tag_name_mutex(u32::MAX), "Unknown");
    }

    /// The candidate's OWN access pattern: N threads concurrently intern an
    /// overlapping set of NEW tags (racing the slow-path publish) while reading
    /// back. Global consistency must hold — every observation of a name agrees
    /// on one id, every id maps back to exactly one name, and get_tag_name
    /// round-trips. This is the cross-thread contract a thread-local table
    /// would violate.
    #[test]
    fn lockfree_concurrent_interning_globally_consistent() {
        use std::thread;
        let names: Vec<String> = (0..50).map(|i| format!("lf_cc_{:03}", i)).collect();
        let p = 16usize;
        let handles: Vec<_> = (0..p).map(|_| {
            let names = names.clone();
            thread::spawn(move || {
                let mut got = Vec::with_capacity(names.len() * 200);
                for _ in 0..200 {
                    for n in &names {
                        got.push((n.clone(), intern_tag_lockfree(n)));
                    }
                }
                got
            })
        }).collect();

        let mut name_to_id: Map<String, u32> = Map::new();
        let mut id_to_name: Map<u32, String> = Map::new();
        for h in handles {
            for (n, id) in h.join().unwrap() {
                if let Some(&prev) = name_to_id.get(&n) {
                    assert_eq!(prev, id, "name {n} observed with two ids ({prev} vs {id})");
                }
                name_to_id.insert(n.clone(), id);
                if let Some(prev) = id_to_name.get(&id) {
                    assert_eq!(prev, &n, "id {id} maps to two names ({prev} vs {n})");
                }
                id_to_name.insert(id, n);
            }
        }
        assert_eq!(name_to_id.len(), 50, "not all distinct tags interned");
        for (n, &id) in &name_to_id {
            assert_eq!(&get_tag_name_lockfree(id), n, "round-trip failed for {n}");
        }
    }
}

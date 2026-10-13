//! r185 KIND 2 + FREEZE: a local mutable map owned by its handle, and the immutable map it freezes into. NO LOCKS, no atomics
//! on a map operation, no `unsafe`.
//!
//! LOCAL MAP (`Value::Local`). The map lives in a THREAD-LOCAL table on the thread that made it; the handle value carries only
//! (owner thread, slot, generation). Value therefore stays Send + Sync with nothing added to satisfy it. Another thread can hold
//! the handle but finds nothing: its own table has no such slot, and the owner check refuses first, by name. The handle counts
//! its copies in the table (Clone on the owner adds one, Drop on the owner takes one); when the last copy drops, the slot is
//! emptied and reused. There is no free call and no global name.
//!   hmap_new(Unit) -> Value               a new empty local map
//!   hmap_put(m, k, v) -> Bool             set k to v in place; true iff k was new (local maps only)
//!   hmap_has(m, k) -> Bool                exact key membership (never a prefix)
//!   hmap_get(m, k) -> Text                "Ok:" + value, or "Unknown" (no sentinel value)
//!   hmap_freeze(m) -> Value               moves the map out of the table into a frozen map (no copy); the handle is dead after
//! FROZEN MAP (`Value::Frozen`). An `Arc<HashMap>`: immutable, readable from any thread without a lock (has / get accept it).
//!   hmap_hash(f) -> Text                  "sha256:<hex>" of the entries in key order; the ONLY thing that hashes. Equality of two
//!                                         frozen maps compares the entries directly, so it hashes nothing either. Not cached
//!                                         (a cache is an atomic init); a caller that needs it often keeps the Text.
//!
//! LIMITS, stated: a copy of a handle that is DROPPED on a thread other than its owner cannot reach the owner's table without a
//! lock, so it leaves the count high and the slot lives until its owner thread ends (the table is dropped with the thread).
//! The doors by which a Value changes thread (spawn, channel_send, bchan_send) refuse a handle, so this needs a path that
//! does not exist today. A copy made off the owner thread is inert (it is not counted and does nothing on drop).

use super::value::{intern_str, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::thread::ThreadId;

type Map = HashMap<String, Arc<str>>;

enum State {
    Free,
    Live(Map),
    Dead,
}

struct Entry {
    gen: u32,
    count: u32,
    state: State,
}

pub struct Table {
    slots: Vec<Entry>,
    free: Vec<usize>,
}

thread_local! {
    static ME: ThreadId = std::thread::current().id();
    static TABLE: RefCell<Table> = RefCell::new(Table { slots: Vec::new(), free: Vec::new() });
    #[cfg(test)]
    static HASHED: std::cell::Cell<u64> = std::cell::Cell::new(0);
}

fn me() -> ThreadId {
    ME.with(|m| *m)
}

/// Run `f` on this thread's table.
fn with_table<R>(f: impl FnOnce(&mut Table) -> R) -> R {
    #[cfg(test)]
    if mutants::mode() == 5 {
        return mutants::with_shared(f);
    }
    TABLE.with(|t| f(&mut t.borrow_mut()))
}

/// The handle. `counted` is true for copies made on the owner thread.
pub struct LocalMap {
    owner: ThreadId,
    index: u32,
    gen: u32,
    counted: bool,
}

impl Clone for LocalMap {
    fn clone(&self) -> Self {
        let counted = self.owner == me() && with_table(|t| match t.slots.get_mut(self.index as usize) {
            Some(e) if e.gen == self.gen && !matches!(e.state, State::Free) => { e.count += 1; true }
            _ => false,
        });
        LocalMap { owner: self.owner, index: self.index, gen: self.gen, counted }
    }
}

impl Drop for LocalMap {
    fn drop(&mut self) {
        #[cfg(test)]
        if mutants::mode() == 4 {
            return;
        }
        if !self.counted || self.owner != me() {
            return;
        }
        let (index, gen) = (self.index as usize, self.gen);
        let _ = TABLE.try_with(|t| {
            if let Ok(mut t) = t.try_borrow_mut() {
                let freed = match t.slots.get_mut(index) {
                    Some(e) if e.gen == gen && !matches!(e.state, State::Free) => {
                        e.count -= 1;
                        if e.count == 0 { e.state = State::Free; e.gen = e.gen.wrapping_add(1); true } else { false }
                    }
                    _ => false,
                };
                if freed { t.free.push(index); }
            }
        });
    }
}

impl PartialEq for LocalMap {
    fn eq(&self, o: &Self) -> bool { self.owner == o.owner && self.index == o.index && self.gen == o.gen }
}

impl std::fmt::Debug for LocalMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "<local map>") }
}

/// A frozen map: immutable behind a reference count.
#[derive(Clone)]
pub struct FrozenMap(Arc<Map>);

impl PartialEq for FrozenMap {
    fn eq(&self, o: &Self) -> bool { Arc::ptr_eq(&self.0, &o.0) || *self.0 == *o.0 }
}

impl std::fmt::Debug for FrozenMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "<frozen map>") }
}

fn owner_check(h: &LocalMap, who: &str) {
    #[cfg(test)]
    if mutants::mode() == 5 {
        return;
    }
    let here = me();
    if h.owner != here {
        panic!("{who}: {:?} belongs to thread {:?}; this is thread {:?}", h, h.owner, here);
    }
}

/// Run `f` on the live map of `h`; refuses by name a foreign thread, a dead handle or a handle with no slot here.
fn on_live<R>(h: &LocalMap, who: &str, f: impl FnOnce(&mut Map) -> R) -> R {
    owner_check(h, who);
    let found = with_table(|t| match t.slots.get_mut(h.index as usize) {
        Some(e) if e.gen == h.gen => match &mut e.state {
            State::Live(m) => Ok(f(m)),
            State::Dead => Err("is dead (it was frozen)"),
            State::Free => Err("has no map here"),
        },
        _ => Err("has no map here"),
    });
    match found {
        Ok(r) => r,
        Err(why) => panic!("{who}: {:?} {why}", h),
    }
}

fn same_key(a: &str, b: &str) -> bool {
    #[cfg(test)]
    if mutants::mode() == 1 {
        return a.starts_with(b) || b.starts_with(a);
    }
    a == b
}

fn find(m: &Map, key: &str) -> Option<Arc<str>> {
    #[cfg(test)]
    if mutants::mode() == 1 {
        return m.iter().find(|(k, _)| same_key(k, key)).map(|(_, v)| v.clone());
    }
    m.get(key).cloned()
}

fn lookup(h: &Value, key: &str, who: &str) -> Option<Arc<str>> {
    match h {
        Value::Local(l) => on_live(l, who, |m| find(m, key)),
        Value::Frozen(f) => find(&f.0, key),
        other => panic!("{who}: expected a map, got {:?}", other),
    }
}

#[track_caller]
pub fn hmap_new(_: Value) -> Value {
    let owner = me();
    #[cfg(test)]
    if mutants::mode() == 2 {
        // every handle is the first map
        let first = with_table(|t| match t.slots.first_mut() { Some(e) if matches!(e.state, State::Live(_)) => { e.count += 1; Some(e.gen) } _ => None });
        if let Some(gen) = first {
            return Value::Local(LocalMap { owner, index: 0, gen, counted: true });
        }
    }
    let (index, gen) = with_table(|t| match t.free.pop() {
        Some(i) => { let e = &mut t.slots[i]; e.count = 1; e.state = State::Live(HashMap::new()); (i, e.gen) }
        None => { t.slots.push(Entry { gen: 0, count: 1, state: State::Live(HashMap::new()) }); (t.slots.len() - 1, 0) }
    });
    Value::Local(LocalMap { owner, index: index as u32, gen, counted: true })
}

#[track_caller]
pub fn hmap_put(h: Value, k: Arc<str>, v: Arc<str>) -> Value {
    match &h {
        Value::Local(l) => Value::Bool(on_live(l, "hmap_put", |m| match m.get_mut(&*k) {
            Some(slot) => { *slot = v; false }
            None => { m.insert(k.to_string(), v); true }
        })),
        other => panic!("hmap_put: {:?} cannot be written (only a local map can)", other),
    }
}

#[track_caller]
pub fn hmap_has(h: Value, k: Arc<str>) -> Value {
    Value::Bool(lookup(&h, &k, "hmap_has").is_some())
}

#[track_caller]
pub fn hmap_get(h: Value, k: Arc<str>) -> Value {
    match lookup(&h, &k, "hmap_get") {
        Some(v) => Value::Str(intern_str(&format!("Ok:{}", v))),
        None => Value::Str(intern_str("Unknown")),
    }
}

#[track_caller]
pub fn hmap_freeze(h: Value) -> Value {
    match &h {
        Value::Local(l) => {
            #[cfg(test)]
            if mutants::mode() == 6 {
                return h.clone(); // the "frozen" map is still the live one
            }
            let taken = on_live(l, "hmap_freeze", |m| std::mem::take(m));
            with_table(|t| { t.slots[l.index as usize].state = State::Dead; });
            let frozen = FrozenMap(Arc::new(taken));
            #[cfg(test)]
            if mutants::mode() == 7 {
                let _ = hash_of(&frozen.0); // hashed although nothing asked
            }
            Value::Frozen(frozen)
        }
        other => panic!("hmap_freeze: {:?} is not a local map", other),
    }
}

fn hash_of(m: &Map) -> String {
    #[cfg(test)]
    HASHED.with(|c| c.set(c.get() + 1));
    use sha2::{Digest, Sha256};
    let mut keys: Vec<&String> = m.keys().collect();
    keys.sort();
    let mut h = Sha256::new();
    for k in keys {
        let v = &m[k];
        h.update((k.len() as u64).to_le_bytes());
        h.update(k.as_bytes());
        h.update((v.len() as u64).to_le_bytes());
        h.update(v.as_bytes());
    }
    let hex: String = h.finalize().iter().map(|b| format!("{:02x}", b)).collect();
    format!("sha256:{}", hex)
}

#[track_caller]
pub fn hmap_hash(f: Value) -> Value {
    match &f {
        Value::Frozen(fm) => Value::Str(intern_str(&hash_of(&fm.0))),
        other => panic!("hmap_hash: {:?} is not a frozen map (freeze it first)", other),
    }
}

/// Panic if `v` is, or holds anywhere inside it, a local map. Called at the doors by which a Value changes thread. A frozen map
/// is shareable and passes.
pub fn refuse_handle(v: &Value, who: &str) {
    #[cfg(test)]
    if mutants::mode() == 3 {
        return;
    }
    match v {
        Value::Local(l) => panic!("{who}: {:?} cannot be shared (it belongs to the loop on thread {:?} that made it)", l, l.owner),
        Value::Ctor { fields, .. } => fields.iter().for_each(|f| refuse_handle(f, who)),
        Value::Tuple(t) => t.iter().for_each(|f| refuse_handle(f, who)),
        Value::List(l) => l.iter().for_each(|f| refuse_handle(f, who)),
        _ => {}
    }
}

// ===== TEST-ONLY BELOW (the no-lock scan reads only the text above this line) =====
#[cfg(test)]
pub mod mutants {
    use super::*;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::Mutex;

    /// 0 real; 1 prefix compare; 2 handles share one map; 3 doors accept a handle; 4 drop leaks; 5 one table shared by all
    /// threads (an operation from a non-owner thread succeeds); 6 freeze leaves the map live; 7 freeze hashes eagerly.
    pub static MODE: AtomicU8 = AtomicU8::new(0);
    pub fn mode() -> u8 { MODE.load(Ordering::Relaxed) }
    static SHARED: Mutex<Table> = Mutex::new(Table { slots: Vec::new(), free: Vec::new() });
    pub fn with_shared<R>(f: impl FnOnce(&mut Table) -> R) -> R { f(&mut SHARED.lock().unwrap()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::mutants::{mode, MODE};
    use std::sync::atomic::Ordering::Relaxed;

    fn k(x: &str) -> Arc<str> { intern_str(x) }
    fn new() -> Value { hmap_new(Value::Unit) }
    fn get(m: &Value, key: &str) -> String { match hmap_get(m.clone(), k(key)) { Value::Str(x) => x.to_string(), _ => panic!() } }
    fn has(m: &Value, key: &str) -> bool { hmap_has(m.clone(), k(key)) == Value::Bool(true) }
    fn live() -> usize { TABLE.with(|t| t.borrow().slots.iter().filter(|e| !matches!(e.state, State::Free)).count()) }
    fn hashed() -> u64 { HASHED.with(|c| c.get()) }
    fn msg<F: FnOnce() + std::panic::UnwindSafe>(f: F) -> Option<String> {
        std::panic::catch_unwind(f).err().map(|e| e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default())
    }

    // ── properties, each run under every mutant that should break it ──────────────────────────────────────────────────
    fn interference() -> bool {
        let a = new();
        let b = new();
        hmap_put(a.clone(), k("k1"), k("v1"));
        hmap_put(b.clone(), k("k1"), k("v1"));
        hmap_put(a.clone(), k("k2"), k("v2"));
        !has(&b, "k2") && has(&a, "k2") && !has(&new(), "k1")
    }
    fn prefix() -> bool {
        let m = new();
        hmap_put(m.clone(), k("sha256:abcdef"), k("v"));
        !has(&m, "sha256:abc") && !has(&m, "sha256:abcdefg") && get(&m, "sha256:abc") == "Unknown" && get(&m, "sha256:abcdef") == "Ok:v"
    }
    fn reader_does_not_see_later_change() -> bool {
        let m = new();
        hmap_put(m.clone(), k("a"), k("1"));
        let f = hmap_freeze(m.clone());
        let late = msg(std::panic::AssertUnwindSafe(|| { hmap_put(m.clone(), k("b"), k("2")); }));
        late.map_or(false, |t| t.contains("local map")) && !has(&f, "b") && get(&f, "a") == "Ok:1"
    }
    fn frozen_hashed_only_on_use() -> bool {
        let m = new();
        hmap_put(m.clone(), k("a"), k("1"));
        let before = hashed();
        let f = hmap_freeze(m);
        let g = f.clone();
        let quiet = has(&f, "a") && get(&g, "a") == "Ok:1" && f == g && hashed() == before;
        let _ = hmap_hash(f);
        quiet && hashed() == before + 1
    }
    fn drop_frees() -> bool {
        let base = live();
        let m = new();
        hmap_put(m.clone(), k("a"), k("1"));
        let c = m.clone();
        let up = live() == base + 1;
        drop(m);
        let still = live() == base + 1; // a copy remains
        drop(c);
        let f = hmap_freeze(new());
        drop(f);
        up && still && live() == base
    }
    fn foreign_thread_refused() -> bool {
        let m = new();
        hmap_put(m.clone(), k("a"), k("1"));
        let m2 = m.clone();
        let t = std::thread::spawn(move || msg(std::panic::AssertUnwindSafe(|| { hmap_has(m2.clone(), k("a")); })));
        let refused = t.join().unwrap().map_or(false, |t| t.contains("local map") && t.contains("belongs to thread"));
        refused
    }

    #[test]
    fn real_keeps_every_property() {
        MODE.store(0, Relaxed);
        assert!(interference(), "interference");
        assert!(prefix(), "prefix");
        assert!(reader_does_not_see_later_change(), "later change");
        assert!(frozen_hashed_only_on_use(), "hash on use");
        assert!(drop_frees(), "drop frees");
        assert!(foreign_thread_refused(), "foreign thread");
    }

    #[test]
    fn many_keys_overwrite_and_slot_reuse() {
        MODE.store(0, Relaxed);
        let m = new();
        for i in 0..5000 { assert_eq!(hmap_put(m.clone(), k(&format!("k{i}")), k(&format!("v{i}"))), Value::Bool(true)); }
        for i in 0..5000 { assert_eq!(get(&m, &format!("k{i}")), format!("Ok:v{i}")); }
        assert_eq!(get(&m, "k5000"), "Unknown");
        assert_eq!(hmap_put(m.clone(), k("k7"), k("new")), Value::Bool(false));
        assert_eq!(get(&m, "k7"), "Ok:new");
        drop(m);
        let a = new(); let ai = if let Value::Local(l) = &a { l.index } else { 99 };
        drop(a);
        let b = new(); // reuses the slot with a new generation
        assert!(if let Value::Local(l) = &b { l.index == ai && l.gen >= 1 } else { false });
        assert!(!has(&b, "k7"));
    }

    #[test]
    fn dead_and_frozen_refuse_by_name() {
        MODE.store(0, Relaxed);
        let m = new();
        let f = hmap_freeze(m.clone());
        for (who, r) in [
            ("put", msg(std::panic::AssertUnwindSafe(|| { hmap_put(m.clone(), k("x"), k("y")); }))),
            ("has", msg(std::panic::AssertUnwindSafe(|| { hmap_has(m.clone(), k("x")); }))),
            ("freeze again", msg(std::panic::AssertUnwindSafe(|| { hmap_freeze(m.clone()); }))),
        ] {
            let t = r.unwrap_or_else(|| panic!("{who} on a dead handle succeeded"));
            assert!(t.contains("<local map>") && t.contains("dead"), "{who}: {t}");
        }
        let t = msg(std::panic::AssertUnwindSafe(|| { hmap_put(f.clone(), k("x"), k("y")); })).unwrap();
        assert!(t.contains("<frozen map>"), "{t}");
        // a frozen map reads from any thread with no lock
        let f2 = f.clone();
        assert!(std::thread::spawn(move || hmap_has(f2, k("zz")) == Value::Bool(false)).join().unwrap());
    }

    #[test]
    fn doors_refuse_a_handle_even_nested_and_pass_a_frozen_map() {
        MODE.store(0, Relaxed);
        let h = new();
        let inside = Value::Ctor { tag: 0, fields: vec![Value::Int(1), Value::List(crate::runtime::value::ListBuf::from(vec![h.clone()]))] };
        let t = msg(std::panic::AssertUnwindSafe(|| refuse_handle(&inside, "t"))).unwrap();
        assert!(t.contains("<local map>"));
        let door = msg(std::panic::AssertUnwindSafe(|| { crate::runtime::channels::channel_send(k("hmap_door"), inside.clone()); })).unwrap();
        assert!(door.contains("<local map>"), "{door}");
        refuse_handle(&Value::Ctor { tag: 0, fields: vec![Value::Int(1)] }, "t");
        refuse_handle(&hmap_freeze(h), "t");
    }

    #[test]
    fn accessors_refuse_a_map_by_name() {
        MODE.store(0, Relaxed);
        let l = new();
        let f = hmap_freeze(new());
        for (name, v) in [("local map", l), ("frozen map", f)] {
            let a = msg(std::panic::AssertUnwindSafe(|| { crate::runtime::list::list_get(v.clone(), 0); })).unwrap();
            let b = msg(std::panic::AssertUnwindSafe(|| { crate::runtime::tuple::ctor_field(v.clone(), 0); })).unwrap();
            let c = msg(std::panic::AssertUnwindSafe(|| { crate::runtime::tuple::tuple_field(v.clone(), 0); })).unwrap();
            for (who, t) in [("list_get", a), ("ctor_field", b), ("tuple_field", c)] {
                assert!(t.contains(who) && t.contains(name), "{who} on a {name}: {t}");
            }
        }
    }

    /// The gate for "no lock": the non-test text of this file names no Mutex, RwLock, atomic or unsafe impl.
    fn lock_free(src: &str) -> bool {
        let real = src.split("// ===== TEST-ONLY BELOW").next().unwrap();
        let code: String = real.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        !["Mutex", "RwLock", "Atomic", "unsafe", "Condvar", "OnceLock", "LazyLock", "static mut"].iter().any(|w| code.contains(w))
    }

    #[test]
    fn no_lock_in_the_real_code_and_the_gate_bites() {
        let src = include_str!("hmap.rs");
        assert!(lock_free(src), "hmap.rs takes a lock");
        // mutant 8: the same file with a lock taken on a map operation
        let bad = src.replacen("fn me() -> ThreadId {", "fn me() -> ThreadId {\n    let _g = std::sync::Mutex::new(0).lock();", 1);
        assert!(!lock_free(&bad), "mutant 8 (a lock on an operation) was not caught");
        let bad2 = src.replacen("pub struct LocalMap {", "unsafe impl Sync for LocalMap {}\npub struct LocalMap {", 1);
        assert!(!lock_free(&bad2), "mutant 8b (unsafe impl) was not caught");
    }

    #[test]
    fn mutants_fail() {
        for (m, name) in [(1u8, "prefix"), (2, "interference"), (3, "doors"), (4, "leak"), (5, "foreign"), (6, "reader sees change"), (7, "eager hash")] {
            MODE.store(m, Relaxed);
            let caught = match m {
                1 => !prefix(),
                2 => !interference(),
                3 => msg(std::panic::AssertUnwindSafe(|| refuse_handle(&new(), "t"))).is_none(),
                4 => !drop_frees(),
                5 => !foreign_thread_refused(),
                6 => !reader_does_not_see_later_change(),
                _ => !frozen_hashed_only_on_use(),
            };
            MODE.store(0, Relaxed);
            assert!(m != 3 || caught, "mutant {m} ({name}) was not caught");
            if m != 3 { assert!(caught, "mutant {m} ({name}) was not caught"); }
        }
        MODE.store(0, Relaxed);
        assert_eq!(mode(), 0);
    }
}

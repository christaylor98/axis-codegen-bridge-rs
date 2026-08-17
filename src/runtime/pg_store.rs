//! pg_store — postgres-backed durable store for the graphcore store layer
//! (AXVERITY_POSTGRES_STORE_SWAP_V1).
//!
//! The append-only ledger and the state anchor live in a real postgres
//! database; postgres's WAL + group commit own their durability. Object
//! CONTENT (D048, OBJSEG_V1) lives in fixed-size preallocated segment files
//! on local disk (see `objseg.rs`) — `gcore_objects` holds only the index
//! (`addr -> segment_id, seg_offset, len`), never the bytes. Everything
//! here is under the surface: the wire ABI and the store-op semantics
//! (put/get/append/bind + log replay + anchor chain) are unchanged; the
//! 107-test battery sees the same addresses, log lines, and wire format.
//!
//! The builtins (bridge leaf fns, declared in axis-bridge.axreg):
//!   * `pg_bytes_put(addr: Text, content: Bytes) -> Unit`
//!         content-addressed object upsert (ON CONFLICT DO NOTHING). Bytes
//!         go through `objseg::seg_append`; only the resulting pointer is
//!         a postgres row.
//!   * `pg_bytes_get(addr: Text) -> Bytes`
//!         object read; empty Bytes on absent (ZERO_ROWS_NEVER_MEANS_UNKNOWN
//!         at the wire stays in M1 — the seam returns Bytes either way).
//!         Looks up the pointer, then `objseg::seg_read`s the bytes.
//!   * `pg_obj_block_put(block: Bytes, index: Text) -> Unit`
//!         parse the arena's pending index ("<addr>\t<off>\t<len>\n" lines),
//!         append the WHOLE sealed `block` to the current segment in ONE
//!         `pwrite`+`fsync` (D048 — this is the actual fix: the block used
//!         to get exploded into one postgres row per object here; now it's
//!         one contiguous disk write and postgres gets only per-object
//!         pointers into it), then insert every object's index row in ONE
//!         committed transaction.
//!   * `pg_log_append(line: Text) -> Unit`
//!         append one ledger block as a row (seq = append order).
//!   * `pg_log_scan(Unit) -> Text`
//!         the whole ledger, rows concatenated in append order — byte-equal
//!         to the old `.gcore/log` file content.
//!   * `pg_anchor_get(Unit) -> Text`
//!         the current anchor ("" before the first flush).
//!   * `pg_anchor_set(value: Text) -> Unit`
//!         commit the anchor as a single-row upsert (key 'anchor').
//!
//! Connection lifecycle: ONE lazily-initialized shared connection. gcore_serve
//! is a single serial accept loop, so one connection is correct; a Mutex keeps
//! the global safe if a future consumer multithreads the server. The target
//! database comes from `AXVERITY_PG_DSN` (libpq-style, e.g.
//! `host=/var/run/postgresql port=5434 user=chris dbname=foo`) when set;
//! otherwise the process AUTO-PROVISIONS a fresh per-PID scratch database via
//! the local peer-auth superuser socket and bootstraps the schema — so a
//! freshly started server always owns an EMPTY store, giving the "isolated
//! temp store" the tests assume with zero plumbing. Panics (never silently
//! degrades) on any connection/statement error: durability is postgres's job
//! now, and a failing store must be loud.

use std::sync::{Mutex, OnceLock};

use postgres::{Client, NoTls};

use super::value::{Value, intern_str};

// Schema (idempotent bootstrap; run on first connect). gcore_objects is
// handled separately by `migrate_gcore_objects` (D048 changed its shape
// from content-holding to index-only; see that fn).
const BOOTSTRAP: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS gcore_log (seq bigserial PRIMARY KEY, line text NOT NULL)",
    "CREATE TABLE IF NOT EXISTS gcore_anchor (k text PRIMARY KEY, v text NOT NULL)",
    "CREATE TABLE IF NOT EXISTS gcore_meta (k text PRIMARY KEY, v text NOT NULL)",
    "INSERT INTO gcore_meta(k, v) VALUES ('schema_version', '2') ON CONFLICT (k) DO NOTHING",
];

/// D048 (OBJSEG_V1): `gcore_objects` changed from `(addr, content bytea)` to
/// `(addr, segment_id, seg_offset, len)` — an index into `objseg.rs`'s
/// segment files, never the content itself. Pre-D048 data is disposable
/// (Chris, 2026-08-12 turn: "existing data disposable" was the accepted
/// default) — detected by the old `content` column's presence and dropped,
/// not migrated row-by-row (there is nowhere for the old bytes to migrate
/// TO without re-deriving segment offsets from scratch, and the store this
/// runs against is either a fresh per-PID test scratch DB or a dev DB with
/// nothing durable riding on it).
fn migrate_gcore_objects(client: &mut Client) {
    let has_old_col = client
        .query_opt(
            "SELECT 1 FROM information_schema.columns \
             WHERE table_name = 'gcore_objects' AND column_name = 'content'",
            &[],
        )
        .unwrap_or_else(|e| panic!("pg_store: migrate_gcore_objects check: {e}"))
        .is_some();
    if has_old_col {
        client
            .execute("DROP TABLE gcore_objects", &[])
            .unwrap_or_else(|e| panic!("pg_store: migrate_gcore_objects drop: {e}"));
    }
    client
        .execute(
            "CREATE TABLE IF NOT EXISTS gcore_objects ( \
                addr text PRIMARY KEY, \
                segment_id bigint NOT NULL, \
                seg_offset bigint NOT NULL, \
                len bigint NOT NULL \
             )",
            &[],
        )
        .unwrap_or_else(|e| panic!("pg_store: migrate_gcore_objects create: {e}"));
}

static PG: OnceLock<Mutex<Client>> = OnceLock::new();

// pub(crate), not private: gcidx.rs's own tests need direct row-level
// control (DELETE) to prove cache-only serving rather than assuming it
// from code shape (D044 Phase 3's acceptance criteria — see that
// module's tests).
pub(crate) fn conn() -> &'static Mutex<Client> {
    PG.get_or_init(|| Mutex::new(connect_and_bootstrap()))
}

fn connect_and_bootstrap() -> Client {
    let dsn = match std::env::var("AXVERITY_PG_DSN") {
        Ok(d) => d,
        Err(_) => provision_scratch_db(),
    };
    let mut client = Client::connect(&dsn, NoTls)
        .unwrap_or_else(|e| panic!("pg_store: connect failed ({dsn}): {e}"));
    migrate_gcore_objects(&mut client);
    for stmt in BOOTSTRAP {
        client.execute(*stmt, &[]).unwrap_or_else(|e| {
            panic!("pg_store: bootstrap failed ({stmt}): {e}")
        });
    }
    client
}

/// Auto-provision a fresh per-PID scratch database so a freshly started server
/// owns an empty store with no environment plumbing. Peer-auth superuser on
/// the local socket can create/drop databases; the OS user is the pg user.
fn provision_scratch_db() -> String {
    let port = std::env::var("AXVERITY_PG_PORT").unwrap_or_else(|_| "5434".into());
    let user = std::env::var("USER").unwrap_or_else(|_| "chris".into());
    let socket = std::env::var("AXVERITY_PG_SOCKET")
        .unwrap_or_else(|_| "/var/run/postgresql".into());
    let db = format!("axv_gcore_{}", std::process::id());

    let maint = format!("host={socket} port={port} user={user} dbname=postgres");
    let mut c = Client::connect(&maint, NoTls)
        .unwrap_or_else(|e| panic!("pg_store: cannot reach maintenance db ({maint}): {e}"));
    // Fresh empty store per process: drop any leftover from a prior run that
    // reused this pid, then create. CREATE DATABASE must run outside a
    // transaction — the sync client executes each statement on its own.
    c.execute(&format!("DROP DATABASE IF EXISTS {db}"), &[])
        .unwrap_or_else(|e| panic!("pg_store: drop {db}: {e}"));
    c.execute(&format!("CREATE DATABASE {db}"), &[])
        .unwrap_or_else(|e| panic!("pg_store: create {db}: {e}"));
    drop(c);
    format!("host={socket} port={port} user={user} dbname={db}")
}

// ── pg_bytes_put / pg_bytes_get ─────────────────────────────────────────────

/// `pg_bytes_put(addr: Text, content: Bytes) -> Unit` — content-addressed,
/// idempotent upsert. Bytes are appended to the current segment (D048,
/// `objseg::seg_append` — one pwrite + one fsync); the postgres row is only
/// the pointer.
#[track_caller]
pub fn pg_bytes_put(addr: std::sync::Arc<str>, content: Vec<u8>) -> Value {
    let addr = addr.to_string();
    let (seg_id, off) = super::objseg::seg_append(&content);
    let len = content.len() as i64;
    let mut c = conn().lock().unwrap();
    c.execute(
        "INSERT INTO gcore_objects(addr, segment_id, seg_offset, len) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (addr) DO NOTHING",
        &[&addr, &seg_id, &off, &len],
    )
    .unwrap_or_else(|e| panic!("pg_bytes_put({addr}): {e}"));
    Value::Unit
}

/// `pg_bytes_get(addr: Text) -> Bytes` — empty Bytes on absent; the M1 seam
/// maps empty -> miss (same convention as contentidx_get). Looks up the
/// segment pointer, then reads the bytes off disk (D048).
#[track_caller]
pub fn pg_bytes_get(addr: std::sync::Arc<str>) -> Value {
    let addr = addr.to_string();
    let mut c = conn().lock().unwrap();
    let row = c
        .query_opt(
            "SELECT segment_id, seg_offset, len FROM gcore_objects WHERE addr = $1",
            &[&addr],
        )
        .unwrap_or_else(|e| panic!("pg_bytes_get({addr}): {e}"));
    drop(c);
    match row {
        Some(row) => {
            let seg_id: i64 = row.get(0);
            let off: i64 = row.get(1);
            let len: i64 = row.get(2);
            Value::Bytes(super::objseg::seg_read(seg_id, off, len))
        }
        None => Value::Bytes(Vec::new()),
    }
}

/// `pg_obj_block_put(block: Bytes, index: Text) -> Unit` — parse the pending
/// index lines, append the WHOLE `block` to the current segment in ONE
/// pwrite+fsync (D048 — this used to explode into one postgres row per
/// object; now it's one contiguous disk write), then insert every object's
/// pointer row in ONE transaction. A malformed index line is a panic (loud
/// corruption, never silent drop); the index commit is atomic or not at
/// all — strictly stronger than the old write-block-then-append-index
/// two-step crash window, and the segment append itself is durable
/// (fsync'd) before any pointer row can reference it.
#[track_caller]
pub fn pg_obj_block_put(block: Vec<u8>, index: std::sync::Arc<str>) -> Value {
    let index = index.to_string();

    let mut objs: Vec<(String, i64, i64)> = Vec::new(); // (addr, off-in-block, len)
    for line in index.lines() {
        if line.is_empty() {
            continue;
        }
        let mut f = line.split('\t');
        let addr = f.next().unwrap_or_else(|| panic!("pg_obj_block_put: bad index line: {line:?}"));
        let off: usize = f.next().unwrap_or_else(|| panic!("pg_obj_block_put: bad index line: {line:?}"))
            .parse()
            .unwrap_or_else(|e| panic!("pg_obj_block_put: bad offset in {line:?}: {e}"));
        let len: usize = f.next().unwrap_or_else(|| panic!("pg_obj_block_put: bad index line: {line:?}"))
            .parse()
            .unwrap_or_else(|e| panic!("pg_obj_block_put: bad length in {line:?}: {e}"));
        let end = off.checked_add(len)
            .unwrap_or_else(|| panic!("pg_obj_block_put: overflow in {line:?}"));
        if block.get(off..end).is_none() {
            panic!("pg_obj_block_put: slice out of range in {line:?} (block={})", block.len());
        }
        objs.push((addr.to_string(), off as i64, len as i64));
    }
    if objs.is_empty() {
        return Value::Unit;
    }

    // One contiguous, fsync'd write for the whole block -- the fix.
    let (seg_id, base_off) = super::objseg::seg_append(&block);

    let mut c = conn().lock().unwrap();
    let mut tx = c.transaction().unwrap_or_else(|e| panic!("pg_obj_block_put: begin: {e}"));
    for (addr, off, len) in &objs {
        let seg_offset = base_off + off;
        tx.execute(
            "INSERT INTO gcore_objects(addr, segment_id, seg_offset, len) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (addr) DO NOTHING",
            &[addr, &seg_id, &seg_offset, len],
        )
        .unwrap_or_else(|e| panic!("pg_obj_block_put({addr}): {e}"));
    }
    tx.commit().unwrap_or_else(|e| panic!("pg_obj_block_put: commit: {e}"));
    Value::Unit
}

// ── pg_log_append / pg_log_scan ─────────────────────────────────────────────

/// `pg_log_append(line: Text) -> Unit` — append one ledger block. seq is
/// bigserial, so scan order == append order == old file order.
#[track_caller]
pub fn pg_log_append(line: std::sync::Arc<str>) -> Value {
    let line = line.to_string();
    let mut c = conn().lock().unwrap();
    c.execute("INSERT INTO gcore_log(line) VALUES ($1)", &[&line])
        .unwrap_or_else(|e| panic!("pg_log_append: {e}"));
    Value::Unit
}

/// `pg_log_scan(Unit) -> Text` — the whole ledger, rows concatenated in
/// append order. Byte-equal to the old `.gcore/log` file content.
#[track_caller]
pub fn pg_log_scan(_u: Value) -> Value {
    let mut c = conn().lock().unwrap();
    let rows = c
        .query("SELECT line FROM gcore_log ORDER BY seq", &[])
        .unwrap_or_else(|e| panic!("pg_log_scan: {e}"));
    let mut out = String::new();
    for row in &rows {
        let line: String = row.get(0);
        out.push_str(&line);
    }
    Value::Str(intern_str(&out))
}

// ── pg_anchor_get / pg_anchor_set ───────────────────────────────────────────

/// `pg_anchor_get(Unit) -> Text` — "" before the first flush (no committed
/// state yet), else the stored anchor text (64-hex + LF, same as the old
/// anchor file).
#[track_caller]
pub fn pg_anchor_get(_u: Value) -> Value {
    let mut c = conn().lock().unwrap();
    match c
        .query_opt("SELECT v FROM gcore_anchor WHERE k = 'anchor'", &[])
        .unwrap_or_else(|e| panic!("pg_anchor_get: {e}"))
    {
        Some(row) => {
            let v: String = row.get(0);
            Value::Str(intern_str(&v))
        }
        None => Value::Str(intern_str("")),
    }
}

/// `pg_anchor_set(value: Text) -> Unit` — commit the anchor (single-row
/// upsert on key 'anchor').
#[track_caller]
pub fn pg_anchor_set(value: std::sync::Arc<str>) -> Value {
    let value = value.to_string();
    let mut c = conn().lock().unwrap();
    c.execute(
        "INSERT INTO gcore_anchor(k, v) VALUES ('anchor', $1) \
         ON CONFLICT (k) DO UPDATE SET v = EXCLUDED.v",
        &[&value],
    )
    .unwrap_or_else(|e| panic!("pg_anchor_set: {e}"));
    Value::Unit
}

#[cfg(test)]
mod tests {
    use super::*;

    // Direct smoke of the store builtins against the local postgres
    // (peer-auth superuser on the unix socket; the auto-provision path makes
    // each test run its own fresh scratch DB — the "isolated temp store").
    // Must be run from an account with create-database rights (chris).
    #[test]
    fn round_trips() {
        let a1 = intern_str("sha256:aa");
        let res = pg_bytes_put(a1.clone(), b"hello".to_vec());
        assert_eq!(res, Value::Unit);

        let got = pg_bytes_get(a1.clone());
        match got {
            Value::Bytes(b) => assert_eq!(b, b"hello"),
            other => panic!("expected Bytes, got {:?}", other),
        }

        // absent -> empty Bytes (the seam's miss convention)
        match pg_bytes_get(intern_str("sha256:nope")) {
            Value::Bytes(b) => assert!(b.is_empty()),
            other => panic!("expected empty Bytes, got {:?}", other),
        }

        // log append is ordered and byte-faithful
        assert_eq!(pg_log_append(intern_str("L1\n")), Value::Unit);
        assert_eq!(pg_log_append(intern_str("L2\n")), Value::Unit);
        assert_eq!(
            pg_log_scan(Value::Unit),
            Value::Str(intern_str("L1\nL2\n"))
        );

        // anchor upsert: set, read back, overwrite
        assert_eq!(pg_anchor_set(intern_str("abc\n")), Value::Unit);
        assert_eq!(pg_anchor_get(Value::Unit), Value::Str(intern_str("abc\n")));
        assert_eq!(pg_anchor_set(intern_str("def\n")), Value::Unit);
        assert_eq!(pg_anchor_get(Value::Unit), Value::Str(intern_str("def\n")));

        // block put: two objects from one block via index lines
        let block = b"AAAthequickBBBbrownfox".to_vec();
        let index = "sha256:x\t3\t8\nsha256:y\t14\t8\n";
        assert_eq!(
            pg_obj_block_put(block, intern_str(index)),
            Value::Unit
        );
        match pg_bytes_get(intern_str("sha256:x")) {
            Value::Bytes(b) => assert_eq!(b, b"thequick"),
            other => panic!("expected Bytes, got {:?}", other),
        }
        match pg_bytes_get(intern_str("sha256:y")) {
            Value::Bytes(b) => assert_eq!(b, b"brownfox"),
            other => panic!("expected Bytes, got {:?}", other),
        }
    }
}

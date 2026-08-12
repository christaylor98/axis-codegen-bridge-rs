//! pg_store — postgres-backed durable store for the graphcore store layer
//! (AXVERITY_POSTGRES_STORE_SWAP_V1).
//!
//! Objects, the append-only ledger, and the state anchor live in a real
//! postgres database. Postgres's WAL + group commit OWN durability — one
//! fdatasync per commit batch, crash-safe ordering for free — so the M1
//! store layer stops issuing its own fsync barriers. Everything here is
//! under the surface: the wire ABI and the store-op semantics (put/get/
//! append/bind + log replay + anchor chain) are unchanged; the 107-test
//! battery sees the same addresses, log lines, and wire format.
//!
//! The builtins (bridge leaf fns, declared in axis-bridge.axreg):
//!   * `pg_bytes_put(addr: Text, content: Bytes) -> Unit`
//!         content-addressed object upsert (ON CONFLICT DO NOTHING).
//!   * `pg_bytes_get(addr: Text) -> Bytes`
//!         object read; empty Bytes on absent (ZERO_ROWS_NEVER_MEANS_UNKNOWN
//!         at the wire stays in M1 — the seam returns Bytes either way).
//!   * `pg_obj_block_put(block: Bytes, index: Text) -> Unit`
//!         parse the arena's pending index ("<addr>\t<off>\t<len>\n" lines),
//!         slice `block` per line, and insert every object in ONE committed
//!         transaction — one commit (one group-commit fdatasync) per flushed
//!         block, preserving the block-granular durability model.
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

use super::value::{Value, get_str, intern_str};

// Schema (idempotent bootstrap; run on first connect).
const BOOTSTRAP: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS gcore_objects (addr text PRIMARY KEY, content bytea NOT NULL)",
    "CREATE TABLE IF NOT EXISTS gcore_log (seq bigserial PRIMARY KEY, line text NOT NULL)",
    "CREATE TABLE IF NOT EXISTS gcore_anchor (k text PRIMARY KEY, v text NOT NULL)",
    "CREATE TABLE IF NOT EXISTS gcore_meta (k text PRIMARY KEY, v text NOT NULL)",
    "INSERT INTO gcore_meta(k, v) VALUES ('schema_version', '1') ON CONFLICT (k) DO NOTHING",
];

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

// ── tuple / scalar unpacking ────────────────────────────────────────────────

fn unpack2(args: Value, who: &str) -> (Value, Value) {
    match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("{who}: expected Tuple(Text, Bytes/Text), got {:?}", other),
    }
}

fn to_str(v: Value, who: &str, what: &str) -> String {
    match v {
        Value::Str(h) => get_str(h),
        other => panic!("{who}: {what} expected Text, got {:?}", other),
    }
}

fn to_bytes(v: Value, who: &str, what: &str) -> Vec<u8> {
    match v {
        Value::Bytes(b) => b,
        other => panic!("{who}: {what} expected Bytes, got {:?}", other),
    }
}

// ── pg_bytes_put / pg_bytes_get ─────────────────────────────────────────────

/// `pg_bytes_put(addr: Text, content: Bytes) -> Unit` — content-addressed,
/// idempotent upsert. One committed row; postgres's group commit owns the
/// fsync.
#[track_caller]
pub fn pg_bytes_put(args: Value) -> Value {
    let (a, b) = unpack2(args, "pg_bytes_put");
    let addr = to_str(a, "pg_bytes_put", "addr");
    let content = to_bytes(b, "pg_bytes_put", "content");
    let mut c = conn().lock().unwrap();
    c.execute(
        "INSERT INTO gcore_objects(addr, content) VALUES ($1, $2) \
         ON CONFLICT (addr) DO NOTHING",
        &[&addr, &content],
    )
    .unwrap_or_else(|e| panic!("pg_bytes_put({addr}): {e}"));
    Value::Unit
}

/// `pg_bytes_get(addr: Text) -> Bytes` — empty Bytes on absent; the M1 seam
/// maps empty -> miss (same convention as contentidx_get).
#[track_caller]
pub fn pg_bytes_get(addr: Value) -> Value {
    let addr = to_str(addr, "pg_bytes_get", "addr");
    let mut c = conn().lock().unwrap();
    match c.query_opt("SELECT content FROM gcore_objects WHERE addr = $1", &[&addr])
        .unwrap_or_else(|e| panic!("pg_bytes_get({addr}): {e}"))
    {
        Some(row) => {
            let content: Vec<u8> = row.get(0);
            Value::Bytes(content)
        }
        None => Value::Bytes(Vec::new()),
    }
}

/// `pg_obj_block_put(block: Bytes, index: Text) -> Unit` — parse the pending
/// index lines and insert every object in ONE transaction. A malformed index
/// line is a panic (loud corruption, never silent drop); the whole block
/// commits atomically or not at all — strictly stronger than the old
/// write-block-then-append-index two-step crash window.
#[track_caller]
pub fn pg_obj_block_put(args: Value) -> Value {
    let (b, i) = unpack2(args, "pg_obj_block_put");
    let block = to_bytes(b, "pg_obj_block_put", "block");
    let index = to_str(i, "pg_obj_block_put", "index");

    let mut objs: Vec<(String, Vec<u8>)> = Vec::new();
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
        let content = block
            .get(off..end)
            .unwrap_or_else(|| panic!("pg_obj_block_put: slice out of range in {line:?} (block={})", block.len()));
        objs.push((addr.to_string(), content.to_vec()));
    }
    if objs.is_empty() {
        return Value::Unit;
    }

    let mut c = conn().lock().unwrap();
    let mut tx = c.transaction().unwrap_or_else(|e| panic!("pg_obj_block_put: begin: {e}"));
    for (addr, content) in &objs {
        tx.execute(
            "INSERT INTO gcore_objects(addr, content) VALUES ($1, $2) \
             ON CONFLICT (addr) DO NOTHING",
            &[addr, content],
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
pub fn pg_log_append(line: Value) -> Value {
    let line = to_str(line, "pg_log_append", "line");
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
pub fn pg_anchor_set(value: Value) -> Value {
    let value = to_str(value, "pg_anchor_set", "value");
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
        let a1 = Value::Str(intern_str("sha256:aa"));
        let hello = Value::Str(intern_str("hello"));
        let res = pg_bytes_put(Value::Tuple(vec![
            a1.clone(),
            text_to_bytes_for_test(hello),
        ]));
        assert_eq!(res, Value::Unit);

        let got = pg_bytes_get(a1.clone());
        match got {
            Value::Bytes(b) => assert_eq!(b, b"hello"),
            other => panic!("expected Bytes, got {:?}", other),
        }

        // absent -> empty Bytes (the seam's miss convention)
        match pg_bytes_get(Value::Str(intern_str("sha256:nope"))) {
            Value::Bytes(b) => assert!(b.is_empty()),
            other => panic!("expected empty Bytes, got {:?}", other),
        }

        // log append is ordered and byte-faithful
        assert_eq!(pg_log_append(Value::Str(intern_str("L1\n"))), Value::Unit);
        assert_eq!(pg_log_append(Value::Str(intern_str("L2\n"))), Value::Unit);
        assert_eq!(
            pg_log_scan(Value::Unit),
            Value::Str(intern_str("L1\nL2\n"))
        );

        // anchor upsert: set, read back, overwrite
        assert_eq!(pg_anchor_set(Value::Str(intern_str("abc\n"))), Value::Unit);
        assert_eq!(pg_anchor_get(Value::Unit), Value::Str(intern_str("abc\n")));
        assert_eq!(pg_anchor_set(Value::Str(intern_str("def\n"))), Value::Unit);
        assert_eq!(pg_anchor_get(Value::Unit), Value::Str(intern_str("def\n")));

        // block put: two objects from one block via index lines
        let block = b"AAAthequickBBBbrownfox".to_vec();
        let index = "sha256:x\t3\t8\nsha256:y\t14\t8\n";
        assert_eq!(
            pg_obj_block_put(Value::Tuple(vec![
                Value::Bytes(block),
                Value::Str(intern_str(index)),
            ])),
            Value::Unit
        );
        match pg_bytes_get(Value::Str(intern_str("sha256:x"))) {
            Value::Bytes(b) => assert_eq!(b, b"thequick"),
            other => panic!("expected Bytes, got {:?}", other),
        }
        match pg_bytes_get(Value::Str(intern_str("sha256:y"))) {
            Value::Bytes(b) => assert_eq!(b, b"brownfox"),
            other => panic!("expected Bytes, got {:?}", other),
        }
    }

    fn text_to_bytes_for_test(v: Value) -> Value {
        super::super::bytes_io::text_to_bytes(v)
    }
}

//! PGBSHAPE_V1 (AXVERITY_MEMCPY_HOTPATH_TRIAL_V1, ARM B) — the Extended Query
//! Protocol side of "there is no SQL text to parse at all".
//!
//! ## The scoping finding this module exists because of
//!
//! `decl:pgwire-extended-query-protocol-v1` is marked COMPLETE, and the intent
//! took that to mean ARM B was "a wiring question rather than a
//! build-from-scratch one". Probed against a live server before writing any
//! code, it is not:
//!
//! ```text
//!   EXTENDED, zero-param INSERT      -> ErrorResponse 42601 "not supported
//!   EXTENDED, parameterised INSERT   ->  over extended query protocol in v1"
//! ```
//!
//! `lib/pg_ext_run.m1` routes every INSERT/UPDATE/DELETE to
//! `pg_ext_unsupported`. Worse for ARM B's premise: `lib/pg_ext_bind.m1`
//! implements Bind by **textually substituting** `$1..$n` into the statement
//! SQL (`pg_bind_subst`), so even on the paths EQP does serve, parameters
//! become SQL text and are re-parsed. EQP as shipped does not preserve client
//! structure; it destroys it. ARM B therefore had to be built, not wired.
//!
//! ## What this module does
//!
//! Splits the per-statement work from the per-row work, which is the whole
//! claim ARM B makes:
//!
//!   * **Parse (once per prepared statement)** — `pgb_parse_shape` extracts the
//!     table and column list from the INSERT, derives a content-based shape id,
//!     and appends it to a durable, fsynced `shapes.log`. This is the only SQL
//!     parsing ARM B ever does. It is fsynced *here* rather than lazily because
//!     an acked row whose shape reference did not survive the crash is an
//!     unreadable row — the shape must be durable strictly before any Execute
//!     that references it can be acked.
//!
//!   * **Bind (per row)** — `pgb_bind_capture` copies the Bind message's
//!     parameter section **verbatim**, one memcpy, no decoding. It does not
//!     build a string, substitute anything, or look at the SQL.
//!
//!   * **Execute (per row, the hot path)** — `pgb_payload` emits
//!     `<20-digit shape id><captured parameter section>`: one allocation and
//!     two memcpys, no parsing.
//!
//! ## Bounds, stated rather than discovered later
//!
//! * **Text format only.** If any Bind format code is binary (1),
//!   `pgb_bind_capture` returns 0 and the connection falls back to the normal
//!   path. Storing binary parameters is representable, but deriving a `RECORD`
//!   from them would need per-type decoding that does not exist here, and
//!   guessing would put wrong bytes under a content hash.
//! * **One live statement per connection.** The shape is held thread-locally,
//!   matching the existing EQP state machine, which already keeps exactly one
//!   `stmt` per connection (`lib/pg_ext_parse.m1`). Parse-A/Parse-B/Bind-A is
//!   not supported here and is not supported by the shipped state machine
//!   either.
//! * Worker threads serve one connection at a time and Bind is immediately
//!   followed by Execute on that same thread, so the thread-local carry is the
//!   same shared-nothing discipline as `hotblk.rs`'s accumulator register.

use std::cell::RefCell;
use std::collections::HashMap;

use super::rawblk::{
    append_shape_durable, decode_params, load_shapes, shape_id,
};
use super::value::{get_str, intern_str, Value};

thread_local! {
    /// (shape id, table, cols) for this thread's currently-parsed statement.
    static SHAPE: RefCell<Option<(u64, String, Vec<String>)>> = const { RefCell::new(None) };
    /// The most recent Bind's parameter section, copied verbatim.
    static PARAMS: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    /// Derive-pool side: shape id -> (table, cols), populated lazily from the
    /// durable log. A map rather than a single slot because a pool thread
    /// interleaves rows from every connection on its shard, so a one-entry
    /// cache would re-read shapes.log on every alternation between two tables.
    static SHAPE_CACHE: RefCell<HashMap<u64, (String, Vec<String>)>> =
        RefCell::new(HashMap::new());
}

fn raw_root() -> String {
    ".axverity/rawblocks".to_string()
}

fn as_text(field: &'static str, v: &Value) -> String {
    match v {
        Value::Str(h) => get_str(h),
        other => panic!("pgbshape: {} expected Text, got {:?}", field, other),
    }
}

fn as_bytes(field: &'static str, v: &Value) -> Vec<u8> {
    match v {
        Value::Bytes(b) => b.clone(),
        Value::Str(h) => get_str(h).into_bytes(),
        other => panic!("pgbshape: {} expected Bytes, got {:?}", field, other),
    }
}

/// `pgb_parse_shape(sql: Text) -> Int`
///
/// Register the shape of an `INSERT INTO <t> (c1, ...) VALUES (...)` statement.
/// Returns the shape id, or 0 if the statement is not an INSERT of that shape
/// (in which case the caller leaves the connection on its normal path).
#[track_caller]
pub fn pgb_parse_shape(arg: Value) -> Value {
    let sql = as_text("sql", &arg);
    let u = sql.trim_start().to_uppercase();
    if !u.starts_with("INSERT") {
        SHAPE.with(|s| *s.borrow_mut() = None);
        return Value::Int(0);
    }
    // table = trim(slice(sql, idx("INTO")+4, idx("("))) — the same span
    // lib/pg_slice_between.m1 takes.
    let uu = sql.to_uppercase();
    let (Some(si), Some(ei)) = (uu.find("INTO"), uu.find('(')) else {
        SHAPE.with(|s| *s.borrow_mut() = None);
        return Value::Int(0);
    };
    let s = si + 4;
    if ei < s {
        SHAPE.with(|s| *s.borrow_mut() = None);
        return Value::Int(0);
    }
    let table = match sql.get(s..ei) {
        Some(t) => t.trim().to_string(),
        None => {
            SHAPE.with(|s| *s.borrow_mut() = None);
            return Value::Int(0);
        }
    };
    let cols: Vec<String> = sql[ei + 1..]
        .split_once(')')
        .map(|(c, _)| c)
        .unwrap_or("")
        .split(',')
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();
    if table.is_empty() || cols.is_empty() {
        SHAPE.with(|s| *s.borrow_mut() = None);
        return Value::Int(0);
    }

    let id = shape_id(&table, &cols);
    // Idempotent: only append when this id is not already on the log. The log
    // is read once per registration, which happens once per prepared
    // statement, never on the per-row path.
    let root = raw_root();
    if !load_shapes(&root).contains_key(&id) {
        append_shape_durable(&root, id, &table, &cols);
    }
    SHAPE.with(|s| *s.borrow_mut() = Some((id, table, cols)));
    Value::Int(id as i64)
}

/// Locate the parameter section of a Bind message. `msg` is the FULL message
/// as `pg_dispatch_ext` slices it — type byte + 4-byte length + body.
/// Returns `(start, end)` spanning `int16 nparams` through the last value.
fn param_section(msg: &[u8]) -> Option<(usize, usize, bool)> {
    let mut off = 5usize; // skip type byte + length
    for _ in 0..2 {
        // portal name, then statement name — both NUL-terminated
        let nul = msg[off..].iter().position(|b| *b == 0)?;
        off += nul + 1;
    }
    if off + 2 > msg.len() {
        return None;
    }
    let nfmt = u16::from_be_bytes([msg[off], msg[off + 1]]) as usize;
    off += 2;
    let mut binary = false;
    for _ in 0..nfmt {
        if off + 2 > msg.len() {
            return None;
        }
        if u16::from_be_bytes([msg[off], msg[off + 1]]) != 0 {
            binary = true;
        }
        off += 2;
    }
    let start = off;
    if off + 2 > msg.len() {
        return None;
    }
    let nparams = u16::from_be_bytes([msg[off], msg[off + 1]]) as usize;
    off += 2;
    for _ in 0..nparams {
        if off + 4 > msg.len() {
            return None;
        }
        let l = i32::from_be_bytes([msg[off], msg[off + 1], msg[off + 2], msg[off + 3]]);
        off += 4;
        if l > 0 {
            off = off.checked_add(l as usize)?;
            if off > msg.len() {
                return None;
            }
        }
    }
    Some((start, off, binary))
}

/// `pgb_bind_capture(msg: Bytes) -> Int`
///
/// Copy the Bind message's parameter section verbatim and return the shape id
/// this connection's statement was registered under. Returns 0 — meaning "not
/// an ARM-B row, use the normal path" — when there is no registered shape, the
/// message will not parse, or any parameter is in binary format.
#[track_caller]
pub fn pgb_bind_capture(arg: Value) -> Value {
    let msg = as_bytes("msg", &arg);
    let Some((id, _, _)) = SHAPE.with(|s| s.borrow().clone()) else {
        return Value::Int(0);
    };
    let Some((start, end, binary)) = param_section(&msg) else {
        return Value::Int(0);
    };
    if binary {
        return Value::Int(0);
    }
    PARAMS.with(|p| {
        let mut p = p.borrow_mut();
        p.clear();
        p.extend_from_slice(&msg[start..end]);
    });
    Value::Int(id as i64)
}

/// `pgb_payload(Unit) -> Bytes` — `<20-digit shape id><parameter section>`.
/// One allocation, two memcpys. This is everything ARM B's hot path does to
/// the client's bytes before they enter the block.
pub fn pgb_payload(_: Value) -> Value {
    let id = SHAPE.with(|s| s.borrow().as_ref().map(|(i, _, _)| *i).unwrap_or(0));
    PARAMS.with(|p| {
        let p = p.borrow();
        let mut out = Vec::with_capacity(20 + p.len());
        out.extend_from_slice(format!("{:020}", id).as_bytes());
        out.extend_from_slice(&p);
        Value::Bytes(out)
    })
}

/// `pgb_record(payload: Bytes) -> Text` → `"<table:pk>\t<RECORD…>"`.
///
/// The pool-side derivation for ARM B. Returns `""` when the shape id is not
/// resolvable, which the caller reports rather than silently dropping.
#[track_caller]
pub fn pgb_record(arg: Value) -> Value {
    let payload = as_bytes("payload", &arg);
    if payload.len() < 20 {
        return Value::Str(intern_str(""));
    }
    let id: u64 = match std::str::from_utf8(&payload[..20]).ok().and_then(|s| s.parse().ok()) {
        Some(v) => v,
        None => return Value::Str(intern_str("")),
    };
    // Resolve from this thread's shape cache; on a miss, reload the durable log
    // once and re-check. The pool thread never saw the Parse (that ran on a
    // request thread), so the first row of each shape pays one file read.
    let cached = SHAPE_CACHE.with(|c| c.borrow().get(&id).cloned());
    let (table, cols) = match cached {
        Some(v) => v,
        None => {
            let loaded = load_shapes(&raw_root());
            SHAPE_CACHE.with(|c| c.borrow_mut().extend(loaded.iter().map(|(k, v)| (*k, v.clone()))));
            match loaded.get(&id) {
                Some(v) => v.clone(),
                None => return Value::Str(intern_str("")),
            }
        }
    };
    let Some(vals) = decode_params(&payload[20..]) else {
        return Value::Str(intern_str(""));
    };
    let mut record = String::from("RECORD");
    for (i, c) in cols.iter().enumerate() {
        record.push('\t');
        record.push_str(c);
        record.push('=');
        record.push_str(vals.get(i).map(|s| s.as_str()).unwrap_or(""));
    }
    let pk = vals.first().cloned().unwrap_or_default();
    Value::Str(intern_str(&format!("{}:{}\t{}", table, pk, record)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bind_msg(portal: &str, stmt: &str, params: &[&str], fmt: u16) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(portal.as_bytes());
        body.push(0);
        body.extend_from_slice(stmt.as_bytes());
        body.push(0);
        body.extend_from_slice(&1u16.to_be_bytes());
        body.extend_from_slice(&fmt.to_be_bytes());
        body.extend_from_slice(&(params.len() as u16).to_be_bytes());
        for p in params {
            body.extend_from_slice(&(p.len() as i32).to_be_bytes());
            body.extend_from_slice(p.as_bytes());
        }
        body.extend_from_slice(&0u16.to_be_bytes()); // result format codes
        let mut msg = vec![b'B'];
        msg.extend_from_slice(&((body.len() + 4) as u32).to_be_bytes());
        msg.extend_from_slice(&body);
        msg
    }

    #[test]
    fn param_section_spans_exactly_the_values() {
        let msg = bind_msg("", "s1", &["abc", "de"], 0);
        let (start, end, binary) = param_section(&msg).unwrap();
        assert!(!binary);
        // int16 n + (4+3) + (4+2)
        assert_eq!(end - start, 2 + 7 + 6);
        let vals = decode_params(&msg[start..end]).unwrap();
        assert_eq!(vals, vec!["abc".to_string(), "de".to_string()]);
    }

    #[test]
    fn binary_format_is_declined_not_guessed() {
        let msg = bind_msg("", "s1", &["abc"], 1);
        let (_, _, binary) = param_section(&msg).unwrap();
        assert!(binary, "binary format code must be detected so capture declines");
    }
}

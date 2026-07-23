//! BRIDGE_TCP_SOCKET_V1 — tcp_listen, tcp_accept, tcp_read, tcp_write, tcp_close.
//!
//! Synchronous, blocking TCP server primitives for the M1 surface, added to
//! unblock the Postgres wire-protocol milestone (gap:axverity-postgres-wire-
//! needs-socket-primitive). These are ordinary `fullIo` leaf fns — they do NOT
//! touch the channels.rs async/event layer.
//!
//!   * `tcp_listen(port: Int) -> (handle: Int, port: Int)`
//!         Bind `0.0.0.0:port` and start listening. `port` 0 requests an
//!         OS-assigned ephemeral port. Returns a `Value::Tuple([handle, port])`
//!         where `port` is the actually-bound port — destructure it with
//!         `tuple_field` (slot 0 = handle, slot 1 = port), the established
//!         multi-value-return precedent (tuple.rs). Panics on bind error.
//!
//!   * `tcp_accept(listener: Int) -> Int`
//!         Block until a peer connects; return a handle for the accepted
//!         stream. Panics on accept error or if `listener` is not a listener.
//!
//!   * `tcp_read(stream: Int) -> Bytes`
//!         Block until ≥1 byte is available, then return one chunk (up to
//!         64 KiB). Returns empty `Bytes` at end-of-stream (peer closed).
//!         Panics on I/O error or if `stream` is not a stream.
//!
//!   * `tcp_write(stream: Int, data: Bytes) -> Unit`
//!         Write all of `data` and flush. Panics on I/O error or if `stream`
//!         is not a stream.
//!
//!   * `tcp_close(handle: Int) -> Unit`
//!         Drop the listener or stream. Panics on an unknown handle (a
//!         double-close is a bug, not a silent no-op).
//!
//! ## Handle registry
//!
//! Sockets live in a process-global `HashMap<i64, Arc<Sock>>` keyed by an
//! integer handle (an `AtomicI64` counter). Handles are opaque `Int`s to M1.
//! Blocking calls (`accept`/`read`) clone the `Arc` out of the map and then
//! release the map lock **before** doing I/O, so one blocked reader never
//! stalls another socket's operations. Read/write go through `&TcpStream`
//! (std implements `Read`/`Write` for shared refs), so a stream can be read and
//! written concurrently without an extra fd.
//!
//! Identities are sha256(name_utf8) — same convention as the rest of the bridge.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::value::{get_str, Value};

/// One registered socket: either a listener or an accepted/connected stream.
enum Sock {
    Listener(TcpListener),
    Stream(TcpStream),
}

// ── AXVERITY_BRIDGE_LOCKFREE_EXPERIMENT_V1 candidate #1 ──────────────────────
//
// The `registry()` Mutex is a SINGLE process-global lock taken on every
// insert_sock / get_sock / tcp_close — i.e. on every accept, read, write, and
// close, across all N `--entries` accept-pool workers. Grounding split its
// contract in two:
//   * STREAM sockets (tcp_connect / tcp_accept → read/write/close) are created
//     AND used on ONE thread for their whole lifetime (accept→serve→close in
//     one pg_accept_one iteration). ⇒ thread-local-safe.
//   * LISTENER sockets (tcp_listen_shared) are created once at startup by one
//     worker and then shared by all N workers calling tcp_accept on them. ⇒
//     must stay shared, but are immutable + touched only at accept.
//
// So the `threadlocal` variant (flag `AXVERITY_NET_REGISTRY=threadlocal`, the
// `shared` Mutex path the default) moves STREAM handles into a thread-local map
// (no lock on the read/write/close hot path) while LISTENERS stay in the shared
// registry. Handle-space split so get_sock/close can route with only the i64:
//   * stream handles  — POSITIVE, from a thread-local counter (per-thread 1,2,…)
//   * listener handles — NEGATIVE, from the global counter negated
// Never 0 (the pg loop-state done-sentinel), never colliding across the sign
// boundary. In `shared` mode BOTH kinds get positive handles in the shared
// registry — byte-identical to the original behavior.
fn net_threadlocal() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| matches!(std::env::var("AXVERITY_NET_REGISTRY").as_deref(), Ok("threadlocal")))
}

thread_local! {
    /// Thread-owned stream table (threadlocal mode only). Reachable only by the
    /// worker that accepted/connected the stream — the same thread that serves
    /// and closes it. No lock, no Arc-registry, nothing shared. (logbuf.rs shape.)
    static TL_STREAMS: RefCell<HashMap<i64, Arc<Sock>>> = RefCell::new(HashMap::new());
    /// Per-thread positive handle counter; first stream on a thread is 1.
    static TL_NEXT: Cell<i64> = const { Cell::new(1) };
}

/// Process-global socket table, keyed by integer handle. Holds every socket in
/// `shared` mode; only listeners (negative handles) in `threadlocal` mode.
fn registry() -> &'static Mutex<HashMap<i64, Arc<Sock>>> {
    static REG: OnceLock<Mutex<HashMap<i64, Arc<Sock>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Allocate the next global opaque handle (never reused within a process run).
fn next_handle() -> i64 {
    static COUNTER: AtomicI64 = AtomicI64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Register a LISTENER and return its handle. Always shared (cross-thread by
/// contract). Negative handle in threadlocal mode so get_sock routes it here.
fn insert_listener(sock: Sock) -> i64 {
    let h = if net_threadlocal() { -next_handle() } else { next_handle() };
    registry().lock().unwrap().insert(h, Arc::new(sock));
    h
}

/// Register a STREAM and return its handle. Thread-local (lock-free) in
/// threadlocal mode; shared registry (original behavior) in shared mode.
fn insert_stream(sock: Sock) -> i64 {
    if net_threadlocal() {
        TL_NEXT.with(|c| {
            let h = c.get();
            c.set(h + 1);
            TL_STREAMS.with(|m| m.borrow_mut().insert(h, Arc::new(sock)));
            h
        })
    } else {
        let h = next_handle();
        registry().lock().unwrap().insert(h, Arc::new(sock));
        h
    }
}

/// Clone the `Arc` for `handle`, releasing any lock before the caller does
/// (possibly blocking) I/O on it. Routes to the thread-local stream table for a
/// positive handle in threadlocal mode, else the shared registry. Panics on
/// unknown handle.
fn get_sock(handle: i64, who: &str) -> Arc<Sock> {
    if net_threadlocal() && handle > 0 {
        TL_STREAMS
            .with(|m| m.borrow().get(&handle).cloned())
            .unwrap_or_else(|| panic!("{}: unknown socket handle {}", who, handle))
    } else {
        registry()
            .lock()
            .unwrap()
            .get(&handle)
            .cloned()
            .unwrap_or_else(|| panic!("{}: unknown socket handle {}", who, handle))
    }
}

/// Remove (close) `handle` from whichever table owns it.
fn remove_sock(handle: i64) -> Option<Arc<Sock>> {
    if net_threadlocal() && handle > 0 {
        TL_STREAMS.with(|m| m.borrow_mut().remove(&handle))
    } else {
        registry().lock().unwrap().remove(&handle)
    }
}

/// Extract an `Int` handle from the single-arg calling convention.
fn handle_arg(v: &Value, who: &str) -> i64 {
    match v {
        Value::Int(n) => *n,
        other => panic!("{}: expected Int handle, got {:?}", who, other),
    }
}

// ── AXVERITY_SLAB_TO_WIRE_BUILD_V1 — paired response batching + slab-to-wire ──
//
// One flag, `AXVERITY_SLAB_TO_WIRE` (off by default), gates BOTH halves of the
// paired build so they can never be enabled independently (the discovery turn
// proved the slab-to-wire half ALONE reproduces a near-null 1.02x, because the
// per-row socket write dominates; batching is what surfaces the win):
//
//   * RESPONSE BATCHING (this section): backend messages are coalesced into a
//     per-connection thread-local buffer instead of one `write_all` per message.
//     Flush policy = PostgreSQL's own model (pqcomm.c `pq_flush` before every
//     socket wait): FLUSH BEFORE EVERY `tcp_read`. A server never blocks on a
//     read with unsent output, so this is correct for the SSL reply (a bare 'N'
//     then a read), the extended-protocol Flush/Sync, and simple-protocol
//     request/response alike — with NO added latency for a POINT query (the
//     buffer is drained at the top of the next accept-loop read, microseconds
//     after the response is written) and NO M1 change. A hard byte cap
//     (`BATCH_CAP`) bounds memory for a huge result set (TCP is a stream, so a
//     mid-response flush is transparent to the client), and `tcp_close` drains
//     any tail. Since axVerity materialises the full posting list before it
//     emits, the row count is known up-front — there is never a "waiting for
//     rows that never arrive" partial batch, so no timeout policy is needed.
//
//   * SLAB-TO-WIRE (see `pg_emit_datarow1` below + future shape primitives):
//     the DataRow bytes are formatted in Rust straight from the value bytes and
//     appended to the SAME per-conn buffer, with NO intermediate `Value`
//     materialisation of the frame (the M1 path builds ~8 `Value::Bytes` per row
//     via pg_data_row_1∘pg_frame; this builds one Vec and appends it).
//
// When the flag is off, `tcp_write`/`tcp_read`/`tcp_close` are byte-for-byte the
// original per-message-flush path (the preserved fallback).

fn slab_to_wire_on() -> bool {
    // AXVERITY_WAY_BACK_CONSOLIDATION_V1: DROPPED. The response-batching / slab-to-wire variant
    // was measured a NO-WIN — it is slightly SLOWER on the common single-row shapes (point
    // 25→27ms, count 61→63ms warm A/B) because those return one small response with nothing to
    // coalesce; the win only exists for large multi-row results, which are not the dominant
    // shape. So the switch is removed: always the byte-for-byte per-message-flush fallback.
    // AXVERITY_SLAB_TO_WIRE is no longer read. (Deleting the now-inert batching branches below is
    // wire-hot-path surgery, staged for its own gated pass; this keeps the drop zero-risk.)
    false
}

/// Flush the per-conn coalescing buffer once it reaches this many bytes, so a
/// large result set does not grow the buffer without bound. A mid-response flush
/// is transparent to the client (TCP is a byte stream; frame boundaries are
/// self-describing via the length prefix).
const BATCH_CAP: usize = 256 * 1024;

thread_local! {
    /// Per-connection coalescing buffer, keyed by socket handle. A worker serves
    /// one connection at a time but many connections over its lifetime, so the
    /// buffer is keyed by handle and removed on flush-to-empty / close.
    static BATCH: RefCell<HashMap<i64, Vec<u8>>> = RefCell::new(HashMap::new());
}

/// The actual socket write (the original `tcp_write` body). Writes ALL of `data`
/// and flushes the OS socket. Panics on I/O error / wrong handle kind.
fn raw_write(handle: i64, data: &[u8]) {
    let sock = get_sock(handle, "tcp_write");
    let mut stream: &TcpStream = match &*sock {
        Sock::Stream(s) => s,
        Sock::Listener(_) => panic!("tcp_write: handle {} is a listener, not a stream", handle),
    };
    stream
        .write_all(data)
        .unwrap_or_else(|e| panic!("tcp_write({}): {}", handle, e));
    stream
        .flush()
        .unwrap_or_else(|e| panic!("tcp_write({}): flush: {}", handle, e));
}

/// Append `data` to the conn's coalescing buffer; flush if it reaches the cap.
fn buffered_append(handle: i64, data: &[u8]) {
    let over_cap = BATCH.with(|b| {
        let mut m = b.borrow_mut();
        let buf = m.entry(handle).or_default();
        buf.extend_from_slice(data);
        buf.len() >= BATCH_CAP
    });
    if over_cap {
        flush_conn(handle);
    }
}

/// Drain the conn's buffer to the socket (no-op if empty / absent). Takes the
/// bytes out from under the borrow BEFORE the (possibly blocking) socket write.
fn flush_conn(handle: i64) {
    let bytes = BATCH.with(|b| {
        let mut m = b.borrow_mut();
        match m.get_mut(&handle) {
            Some(buf) if !buf.is_empty() => Some(std::mem::take(buf)),
            _ => None,
        }
    });
    if let Some(bytes) = bytes {
        raw_write(handle, &bytes);
    }
}

/// `pg_emit_datarow1(conn: Int, val: Text) -> Unit` — the slab-to-wire emitter
/// for a single-text-column DataRow (POINT / filtered SELECT / the hash-emitting
/// plain-SELECT path). Formats the 'D' frame in Rust straight from `val`'s bytes
/// and appends it to the conn's coalescing buffer — NO `Value::Bytes` frame is
/// built (contrast the M1 pg_data_row_1∘pg_frame chain's ~8 allocations/row).
/// Byte-identical to what pg_data_row_1(val) produces. Falls back to an immediate
/// raw write when the flag is off, so a stray caller is always well-behaved.
#[track_caller]
pub fn pg_emit_datarow1(args: Value) -> Value {
    let (conn, val) = match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("pg_emit_datarow1: expected Tuple(Int, Text), got {:?}", other),
    };
    let conn = match conn {
        Value::Int(n) => n,
        other => panic!("pg_emit_datarow1: arg 0 expected Int conn, got {:?}", other),
    };
    let vb: &[u8] = match &val {
        Value::Str(s) => s.as_bytes(),
        other => panic!("pg_emit_datarow1: arg 1 expected Text, got {:?}", other),
    };
    // 'D' | int32(2+4+len+4) | int16(1) | int32(len) | val
    let total = (2 + 4 + vb.len() + 4) as u32;
    let mut frame = Vec::with_capacity(11 + vb.len());
    frame.push(b'D');
    frame.extend_from_slice(&total.to_be_bytes());
    frame.extend_from_slice(&1u16.to_be_bytes());
    frame.extend_from_slice(&(vb.len() as u32).to_be_bytes());
    frame.extend_from_slice(vb);
    if slab_to_wire_on() {
        buffered_append(conn, &frame);
    } else {
        raw_write(conn, &frame);
    }
    Value::Unit
}

/// `pg_stream_rows(conn: Int, posting: Text) -> Int` — the SELECT-streaming
/// EXECUTOR: run the entire per-row emit loop in Rust, returning the row count.
///
/// This replaces the M1 `loop_while(state, pg_row_more, pg_row_step, ..)` streaming
/// loop (pg_emit_result), whose loop-state is a Text `"<conn>\t<count>\t<remaining
/// LF-hashes>"` re-parsed AND rebuilt every iteration — `str_after(posting, LF)`
/// copies the whole remaining hash list each step, so the M1 loop is O(M²) in the
/// row count. Here `posting` is split ONCE (O(M) total) and each LF-separated,
/// non-empty hash is framed as a 1-column DataRow straight into the per-conn
/// coalescing buffer — no per-row M1 interpreter dispatch, no O(M²) re-threading.
///
/// Byte-identical to the M1 loop's output: field_lookup yields an LF-TERMINATED
/// posting, so the last split token is "" (skipped) exactly as pg_row_more stops
/// when the remaining field becomes "". An empty/blank posting emits zero rows,
/// returning 0 (the "SELECT 0" case). The M1 caller writes RowDescription before
/// and CommandComplete+ReadyForQuery after (from this returned count).
#[track_caller]
pub fn pg_stream_rows(args: Value) -> Value {
    let (conn, posting) = match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("pg_stream_rows: expected Tuple(Int, Text), got {:?}", other),
    };
    let conn = match conn {
        Value::Int(n) => n,
        other => panic!("pg_stream_rows: arg 0 expected Int conn, got {:?}", other),
    };
    let s: &str = match &posting {
        Value::Str(p) => p.as_ref(),
        other => panic!("pg_stream_rows: arg 1 expected Text posting, got {:?}", other),
    };
    let on = slab_to_wire_on();
    let mut count: i64 = 0;
    let mut frame: Vec<u8> = Vec::with_capacity(96);
    for line in s.split('\n') {
        if line.is_empty() {
            continue;
        }
        let vb = line.as_bytes();
        frame.clear();
        let total = (2 + 4 + vb.len() + 4) as u32;
        frame.push(b'D');
        frame.extend_from_slice(&total.to_be_bytes());
        frame.extend_from_slice(&1u16.to_be_bytes());
        frame.extend_from_slice(&(vb.len() as u32).to_be_bytes());
        frame.extend_from_slice(vb);
        if on {
            buffered_append(conn, &frame);
        } else {
            raw_write(conn, &frame);
        }
        count += 1;
    }
    Value::Int(count)
}

/// `slab_to_wire_enabled(_: Unit) -> Bool` — lets an M1 emit path route to the
/// Rust slab-to-wire emitters when the flag is on, else keep its exact current
/// Value-materialising body (the preserved fallback).
#[track_caller]
pub fn slab_to_wire_enabled(_arg: Value) -> Value {
    Value::Bool(slab_to_wire_on())
}

// ── tcp_listen ───────────────────────────────────────────────────────────────

#[track_caller]
pub fn tcp_listen(v: Value) -> Value {
    let port = match v {
        Value::Int(n) if (0..=65535).contains(&n) => n as u16,
        Value::Int(n) => panic!("tcp_listen: port {} out of range 0..=65535", n),
        other => panic!("tcp_listen: expected Int port, got {:?}", other),
    };
    let listener = TcpListener::bind(("0.0.0.0", port))
        .unwrap_or_else(|e| panic!("tcp_listen({}): {}", port, e));
    let bound = listener
        .local_addr()
        .unwrap_or_else(|e| panic!("tcp_listen({}): local_addr: {}", port, e))
        .port();
    let handle = insert_listener(Sock::Listener(listener));
    Value::Tuple(vec![Value::Int(handle), Value::Int(bound as i64)])
}

// ── tcp_listen_shared ──────────────────────────────────────────────────────────
//
// AXVERITY_ACCEPTLOOP_SHARD_DISPATCH — the shared-listener piece of the Model-A
// pool. N `--entries` worker threads each call `tcp_listen_shared(port)`; the
// FIRST binds a single listen socket for that port, and every subsequent caller
// gets the SAME listener handle back. All N workers then block in `tcp_accept`
// on that one socket, and the kernel hands each incoming connection to exactly
// one ready (idle-in-accept) worker — the classic pre-fork accept pool, so the
// distribution is naturally balanced with no SO_REUSEPORT 4-tuple-hash imbalance.
//
// The per-port dedup map is a process-global Mutex, but it is touched ONLY at
// worker startup (N times total), never on the accept/read/write/append hot
// path — the shared-nothing write-path invariant (NO_SHARED_REGISTRY) is intact.
// The accepted `TcpListener` is stored in the same `registry()` as `tcp_listen`;
// `TcpListener::accept(&self)` is safe to call concurrently from every worker
// because `get_sock` clones the `Arc` and drops the map lock before blocking.
//
// Backpressure is the kernel's listen backlog on that single socket: with all N
// workers busy, further connections queue in the backlog and, once it fills,
// are refused by the OS (clean `ECONNREFUSED`) rather than silently hung —
// HONEST_BACKPRESSURE, documented, not a timeout-in-userspace.

/// Per-port shared-listener dedup: port → (socket handle, bound port). Held only
/// during the brief startup registration, never on the hot path.
fn shared_listeners() -> &'static Mutex<HashMap<u16, (i64, i64)>> {
    static S: OnceLock<Mutex<HashMap<u16, (i64, i64)>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

#[track_caller]
pub fn tcp_listen_shared(v: Value) -> Value {
    let port = match v {
        Value::Int(n) if (0..=65535).contains(&n) => n as u16,
        Value::Int(n) => panic!("tcp_listen_shared: port {} out of range 0..=65535", n),
        other => panic!("tcp_listen_shared: expected Int port, got {:?}", other),
    };
    // Serialize registration so exactly one thread binds; the rest reuse it.
    let mut shared = shared_listeners().lock().unwrap();
    if let Some(&(h, bound)) = shared.get(&port) {
        return Value::Tuple(vec![Value::Int(h), Value::Int(bound)]);
    }
    let listener = TcpListener::bind(("0.0.0.0", port))
        .unwrap_or_else(|e| panic!("tcp_listen_shared({}): {}", port, e));
    let bound = listener
        .local_addr()
        .unwrap_or_else(|e| panic!("tcp_listen_shared({}): local_addr: {}", port, e))
        .port() as i64;
    let handle = insert_listener(Sock::Listener(listener));
    shared.insert(port, (handle, bound));
    Value::Tuple(vec![Value::Int(handle), Value::Int(bound)])
}

// ── tcp_connect ──────────────────────────────────────────────────────────────

#[track_caller]
pub fn tcp_connect(args: Value) -> Value {
    let (host, port) = match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("tcp_connect: expected Tuple(Text, Int), got {:?}", other),
    };
    let host = match host {
        Value::Str(h) => get_str(h),
        other => panic!("tcp_connect: arg 0 expected Text host, got {:?}", other),
    };
    let port = match port {
        Value::Int(n) if (0..=65535).contains(&n) => n as u16,
        Value::Int(n) => panic!("tcp_connect: port {} out of range 0..=65535", n),
        other => panic!("tcp_connect: arg 1 expected Int port, got {:?}", other),
    };
    let stream = TcpStream::connect((host.as_str(), port))
        .unwrap_or_else(|e| panic!("tcp_connect({}:{}): {}", host, port, e));
    // fix:axverity-pointread-floor — disable Nagle on the client socket too, so
    // the bridge's own request/response spikes don't incur the same ~40ms
    // delayed-ACK stall (parity with the accepted-stream fix above).
    let _ = stream.set_nodelay(true);
    Value::Int(insert_stream(Sock::Stream(stream)))
}

// ── tcp_accept ───────────────────────────────────────────────────────────────

#[track_caller]
pub fn tcp_accept(v: Value) -> Value {
    let handle = handle_arg(&v, "tcp_accept");
    let sock = get_sock(handle, "tcp_accept");
    let listener = match &*sock {
        Sock::Listener(l) => l,
        Sock::Stream(_) => panic!("tcp_accept: handle {} is a stream, not a listener", handle),
    };
    let (stream, _addr) = listener
        .accept()
        .unwrap_or_else(|e| panic!("tcp_accept({}): {}", handle, e));
    // fix:axverity-pointread-floor — disable Nagle on the accepted server
    // socket. Without TCP_NODELAY, a multi-write response (each pg protocol
    // message is a separate write_all) leaves a small trailing segment that
    // Nagle holds pending ACK; the client's delayed-ACK timer then stalls the
    // exchange ~40ms per request/response round-trip. This was ~97% of the
    // measured POINT-read floor (AXVERITY_POINTREAD_FLOOR_DECOMPOSITION_V1).
    // PostgreSQL sets the same option (pqcomm.c). Best-effort: a platform that
    // cannot set it still serves correctly, just with Nagle.
    let _ = stream.set_nodelay(true);
    Value::Int(insert_stream(Sock::Stream(stream)))
}

// ── tcp_read ─────────────────────────────────────────────────────────────────

#[track_caller]
pub fn tcp_read(v: Value) -> Value {
    let handle = handle_arg(&v, "tcp_read");
    // AXVERITY_SLAB_TO_WIRE_BUILD_V1: never block on a read with unsent buffered
    // output (PostgreSQL's pq_flush-before-wait). Drains this conn's coalescing
    // buffer so the peer has the full prior response before we wait for its next
    // message. No-op when the flag is off (buffer is always empty).
    if slab_to_wire_on() {
        flush_conn(handle);
    }
    let sock = get_sock(handle, "tcp_read");
    let mut stream: &TcpStream = match &*sock {
        Sock::Stream(s) => s,
        Sock::Listener(_) => panic!("tcp_read: handle {} is a listener, not a stream", handle),
    };
    let mut buf = [0u8; 65536];
    let n = stream
        .read(&mut buf)
        .unwrap_or_else(|e| panic!("tcp_read({}): {}", handle, e));
    Value::Bytes(buf[..n].to_vec())
}

// ── tcp_write ────────────────────────────────────────────────────────────────

#[track_caller]
pub fn tcp_write(args: Value) -> Value {
    let (handle, data) = match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("tcp_write: expected Tuple(Int, Bytes), got {:?}", other),
    };
    let handle = match handle {
        Value::Int(n) => n,
        other => panic!("tcp_write: arg 0 expected Int handle, got {:?}", other),
    };
    let data = match data {
        Value::Bytes(b) => b,
        other => panic!("tcp_write: arg 1 expected Bytes, got {:?}", other),
    };
    // AXVERITY_SLAB_TO_WIRE_BUILD_V1: coalesce into the per-conn buffer when the
    // flag is on (drained before the next tcp_read / on close / at the cap).
    // Off => the original immediate write+flush (preserved fallback).
    if slab_to_wire_on() {
        buffered_append(handle, &data);
    } else {
        raw_write(handle, &data);
    }
    Value::Unit
}

// ── tcp_close ────────────────────────────────────────────────────────────────

#[track_caller]
pub fn tcp_close(v: Value) -> Value {
    let handle = handle_arg(&v, "tcp_close");
    // AXVERITY_SLAB_TO_WIRE_BUILD_V1: drain any tail before closing so a response
    // that did not end at a read boundary is never lost. Then drop the buffer.
    if slab_to_wire_on() {
        flush_conn(handle);
        BATCH.with(|b| { b.borrow_mut().remove(&handle); });
    }
    match remove_sock(handle) {
        Some(_) => Value::Unit,
        None => panic!("tcp_close: unknown socket handle {}", handle),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::tuple::tuple_field;
    use crate::runtime::value::intern_str;

    fn as_int(v: Value, who: &str) -> i64 {
        match v {
            Value::Int(n) => n,
            other => panic!("{}: expected Int, got {:?}", who, other),
        }
    }

    /// tcp_listen(0) → destructure via tuple_field → full accept/read/write/close
    /// round trip on 127.0.0.1. Exercises all five primitives.
    #[test]
    fn tcp_listen_accept_read_write_close_roundtrip() {
        let bound = tcp_listen(Value::Int(0));
        // Destructure through tuple_field — the multi-value-return precedent.
        let listener = as_int(
            tuple_field(Value::Tuple(vec![bound.clone(), Value::Int(0)])),
            "handle",
        );
        let port = as_int(
            tuple_field(Value::Tuple(vec![bound.clone(), Value::Int(1)])),
            "port",
        );
        assert!(port > 0, "ephemeral bind must yield a nonzero port, got {}", port);

        let payload = b"hello postgres wire".to_vec();
        let expected = payload.clone();

        let server = std::thread::spawn(move || {
            let conn = as_int(tcp_accept(Value::Int(listener)), "conn");
            // Read until we have the whole payload (loopback rarely segments,
            // but be robust); empty Bytes means EOF.
            let mut got = Vec::new();
            while got.len() < expected.len() {
                match tcp_read(Value::Int(conn)) {
                    Value::Bytes(b) if b.is_empty() => break,
                    Value::Bytes(b) => got.extend_from_slice(&b),
                    other => panic!("tcp_read returned {:?}", other),
                }
            }
            // Reply, to exercise tcp_write from the bridge side.
            tcp_write(Value::Tuple(vec![
                Value::Int(conn),
                Value::Bytes(b"ack".to_vec()),
            ]));
            tcp_close(Value::Int(conn));
            got
        });

        // Client half uses std sockets directly.
        let mut client = std::net::TcpStream::connect(("127.0.0.1", port as u16))
            .expect("client connect");
        client.write_all(&payload).expect("client write");
        client.flush().expect("client flush");

        let mut reply = Vec::new();
        // Read the 3-byte ack (then EOF as the server closes).
        let mut chunk = [0u8; 16];
        loop {
            let n = client.read(&mut chunk).expect("client read reply");
            if n == 0 {
                break;
            }
            reply.extend_from_slice(&chunk[..n]);
        }

        let got = server.join().expect("server thread panicked");
        assert_eq!(got, payload, "server did not receive the sent payload");
        assert_eq!(reply, b"ack", "client did not receive the tcp_write reply");

        tcp_close(Value::Int(listener));
    }

    /// All-bridge loopback: the client half is now `tcp_connect` + `tcp_write` +
    /// `tcp_read` (no `std::net`), proving both endpoints via the bridge fns.
    #[test]
    fn tcp_connect_all_bridge_loopback() {
        let bound = tcp_listen(Value::Int(0));
        let listener = as_int(
            tuple_field(Value::Tuple(vec![bound.clone(), Value::Int(0)])),
            "handle",
        );
        let port = as_int(
            tuple_field(Value::Tuple(vec![bound.clone(), Value::Int(1)])),
            "port",
        );

        let payload = b"m1 client to m1 server".to_vec();
        let expected = payload.clone();

        let server = std::thread::spawn(move || {
            let conn = as_int(tcp_accept(Value::Int(listener)), "conn");
            let mut got = Vec::new();
            while got.len() < expected.len() {
                match tcp_read(Value::Int(conn)) {
                    Value::Bytes(b) if b.is_empty() => break,
                    Value::Bytes(b) => got.extend_from_slice(&b),
                    other => panic!("tcp_read returned {:?}", other),
                }
            }
            tcp_write(Value::Tuple(vec![Value::Int(conn), Value::Bytes(b"ok".to_vec())]));
            tcp_close(Value::Int(conn));
            got
        });

        // Client half: entirely bridge fns.
        let client = as_int(
            tcp_connect(Value::Tuple(vec![
                Value::Str(intern_str("127.0.0.1")),
                Value::Int(port),
            ])),
            "client",
        );
        tcp_write(Value::Tuple(vec![
            Value::Int(client),
            Value::Bytes(payload.clone()),
        ]));

        let mut reply = Vec::new();
        loop {
            match tcp_read(Value::Int(client)) {
                Value::Bytes(b) if b.is_empty() => break,
                Value::Bytes(b) => reply.extend_from_slice(&b),
                other => panic!("tcp_read returned {:?}", other),
            }
        }
        tcp_close(Value::Int(client));

        let got = server.join().expect("server thread panicked");
        assert_eq!(got, payload, "server did not receive client payload");
        assert_eq!(reply, b"ok", "client did not receive reply");
        tcp_close(Value::Int(listener));
    }

    /// N concurrent tcp_listen(0) calls must yield N distinct ephemeral ports
    /// with zero bind failures (project concurrent-race test discipline).
    #[test]
    fn concurrent_ephemeral_listen_distinct_ports() {
        const N: usize = 16;
        let threads: Vec<_> = (0..N)
            .map(|_| {
                std::thread::spawn(|| match tcp_listen(Value::Int(0)) {
                    Value::Tuple(es) => (as_int(es[0].clone(), "handle"), as_int(es[1].clone(), "port")),
                    other => panic!("tcp_listen returned {:?}", other),
                })
            })
            .collect();

        let results: Vec<(i64, i64)> =
            threads.into_iter().map(|t| t.join().expect("listener thread panicked")).collect();

        let mut ports: Vec<i64> = results.iter().map(|(_, p)| *p).collect();
        ports.sort_unstable();
        ports.dedup();
        assert_eq!(ports.len(), N, "expected {} distinct ports, got {}", N, ports.len());

        for (h, _) in &results {
            tcp_close(Value::Int(*h));
        }
    }

    // AXVERITY_SLAB_TO_WIRE_BUILD_V1 — coalescing produces byte-identical output.
    // Exercises buffered_append + flush_conn directly (the flag is env-global, so
    // the helpers are tested rather than the flag dispatch). Two appends then one
    // flush must deliver exactly the concatenation, in order.
    #[test]
    fn batch_coalesces_byte_identical() {
        let bound = tcp_listen(Value::Int(0));
        let (listener, port) = match bound {
            Value::Tuple(es) => (as_int(es[0].clone(), "h"), as_int(es[1].clone(), "p")),
            _ => unreachable!(),
        };
        let server = std::thread::spawn(move || {
            let conn = as_int(tcp_accept(Value::Int(listener)), "conn");
            // nothing on the wire yet after two buffered appends
            buffered_append(conn, b"RowDescription...");
            buffered_append(conn, b"DataRow...");
            flush_conn(conn); // now the peer gets exactly the concatenation
            // a second flush is a no-op (buffer emptied)
            flush_conn(conn);
            tcp_close(Value::Int(conn));
        });
        let mut client = std::net::TcpStream::connect(("127.0.0.1", port as u16)).unwrap();
        let mut got = Vec::new();
        client.read_to_end(&mut got).unwrap();
        server.join().unwrap();
        assert_eq!(got, b"RowDescription...DataRow...");
        tcp_close(Value::Int(listener));
    }

    // pg_emit_datarow1 frames a 1-column DataRow byte-identically to
    // pg_data_row_1(val)∘pg_frame. Flag is off in tests => it raw-writes, so we
    // read the exact frame off the wire and check the layout + value bytes,
    // including a multi-byte UTF-8 value (TAB-free boundaries).
    #[test]
    fn pg_emit_datarow1_frames_correctly() {
        let bound = tcp_listen(Value::Int(0));
        let (listener, port) = match bound {
            Value::Tuple(es) => (as_int(es[0].clone(), "h"), as_int(es[1].clone(), "p")),
            _ => unreachable!(),
        };
        let server = std::thread::spawn(move || {
            let conn = as_int(tcp_accept(Value::Int(listener)), "conn");
            pg_emit_datarow1(Value::Tuple(vec![Value::Int(conn), Value::Str(intern_str("café"))]));
            tcp_close(Value::Int(conn));
        });
        let mut client = std::net::TcpStream::connect(("127.0.0.1", port as u16)).unwrap();
        let mut got = Vec::new();
        client.read_to_end(&mut got).unwrap();
        server.join().unwrap();
        // 'café' = 5 bytes (é is 2). Frame: 'D' | int32(2+4+5+4=15) | int16(1) | int32(5) | café
        let vb = "café".as_bytes();
        let mut expect = Vec::new();
        expect.push(b'D');
        expect.extend_from_slice(&((2 + 4 + vb.len() + 4) as u32).to_be_bytes());
        expect.extend_from_slice(&1u16.to_be_bytes());
        expect.extend_from_slice(&(vb.len() as u32).to_be_bytes());
        expect.extend_from_slice(vb);
        assert_eq!(got, expect);
        tcp_close(Value::Int(listener));
    }
}

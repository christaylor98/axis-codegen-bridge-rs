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

/// Process-global socket table, keyed by integer handle.
fn registry() -> &'static Mutex<HashMap<i64, Arc<Sock>>> {
    static REG: OnceLock<Mutex<HashMap<i64, Arc<Sock>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Allocate the next opaque socket handle (never reused within a process run).
fn next_handle() -> i64 {
    static COUNTER: AtomicI64 = AtomicI64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Register `sock` and return its fresh handle.
fn insert_sock(sock: Sock) -> i64 {
    let h = next_handle();
    registry().lock().unwrap().insert(h, Arc::new(sock));
    h
}

/// Clone the `Arc` for `handle` out of the map, releasing the map lock before
/// the caller does any (possibly blocking) I/O on it. Panics on unknown handle.
fn get_sock(handle: i64, who: &str) -> Arc<Sock> {
    registry()
        .lock()
        .unwrap()
        .get(&handle)
        .cloned()
        .unwrap_or_else(|| panic!("{}: unknown socket handle {}", who, handle))
}

/// Extract an `Int` handle from the single-arg calling convention.
fn handle_arg(v: &Value, who: &str) -> i64 {
    match v {
        Value::Int(n) => *n,
        other => panic!("{}: expected Int handle, got {:?}", who, other),
    }
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
    let handle = insert_sock(Sock::Listener(listener));
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
    let handle = insert_sock(Sock::Listener(listener));
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
    Value::Int(insert_sock(Sock::Stream(stream)))
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
    Value::Int(insert_sock(Sock::Stream(stream)))
}

// ── tcp_read ─────────────────────────────────────────────────────────────────

#[track_caller]
pub fn tcp_read(v: Value) -> Value {
    let handle = handle_arg(&v, "tcp_read");
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
    let sock = get_sock(handle, "tcp_write");
    let mut stream: &TcpStream = match &*sock {
        Sock::Stream(s) => s,
        Sock::Listener(_) => panic!("tcp_write: handle {} is a listener, not a stream", handle),
    };
    stream
        .write_all(&data)
        .unwrap_or_else(|e| panic!("tcp_write({}): {}", handle, e));
    stream
        .flush()
        .unwrap_or_else(|e| panic!("tcp_write({}): flush: {}", handle, e));
    Value::Unit
}

// ── tcp_close ────────────────────────────────────────────────────────────────

#[track_caller]
pub fn tcp_close(v: Value) -> Value {
    let handle = handle_arg(&v, "tcp_close");
    match registry().lock().unwrap().remove(&handle) {
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
}

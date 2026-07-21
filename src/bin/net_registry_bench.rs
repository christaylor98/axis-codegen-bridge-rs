// AXVERITY_BRIDGE_LOCKFREE_EXPERIMENT_V1 — candidate #1 (net.rs socket registry).
//
// Real loopback accept-pool matching the pg_server shape: ONE shared listener,
// C worker threads each accept() one connection then serve M read→echo-write
// round-trips, C client threads each connect then do M write→read round-trips.
// Every stream op (accept/connect/read/write/close) goes through the bridge's
// socket registry — the exact lock the `threadlocal` variant removes from the
// per-connection hot path.
//
// Correctness: each client stamps its payload with (client_id, iter) and asserts
// the echo matches byte-for-byte. In threadlocal mode a mis-routed handle would
// echo another connection's bytes (or panic on unknown handle) — so a clean full
// run at high concurrency IS the correctness proof for the variant's own access
// pattern. Run once per mode (fresh process; the flag is OnceLock-cached):
//   AXVERITY_NET_REGISTRY=shared      cargo run --release --bin net_registry_bench
//   AXVERITY_NET_REGISTRY=threadlocal cargo run --release --bin net_registry_bench

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;

use axis_codegen_bridge::runtime::value::{intern_str, init_runtime, Value};
use axis_codegen_bridge::runtime::net::{
    tcp_accept, tcp_close, tcp_connect, tcp_listen_shared, tcp_read, tcp_write,
};

const M: usize = 4000; // round-trips per connection

fn as_int(v: Value, who: &str) -> i64 {
    match v { Value::Int(n) => n, other => panic!("{who}: expected Int, got {other:?}") }
}
fn tuple_ints(v: Value) -> (i64, i64) {
    match v {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (as_int(it.next().unwrap(), "t0"), as_int(it.next().unwrap(), "t1"))
        }
        other => panic!("expected Tuple(Int,Int), got {other:?}"),
    }
}
fn bytes(v: Value, who: &str) -> Vec<u8> {
    match v { Value::Bytes(b) => b, other => panic!("{who}: expected Bytes, got {other:?}") }
}

/// One connection's fixed-size framed payload stamped with (cid, iter).
fn payload(cid: usize, iter: usize) -> Vec<u8> {
    let mut v = format!("cid={cid} it={iter} ", ).into_bytes();
    v.resize(64, b'.'); // fixed 64-byte frame so read() returns it whole on loopback
    v
}

fn run(mode: &str, clients: usize) -> f64 {
    // Shared listener on an ephemeral port.
    let (listener, port) = tuple_ints(tcp_listen_shared(Value::Int(0)));
    let start = Arc::new(Barrier::new(clients * 2 + 1));
    let accepted = Arc::new(AtomicUsize::new(0));

    // Worker threads: each accepts one connection, serves M echo round-trips.
    let mut workers = Vec::new();
    for _ in 0..clients {
        let start = Arc::clone(&start);
        let accepted = Arc::clone(&accepted);
        workers.push(thread::spawn(move || {
            start.wait();
            let conn = as_int(tcp_accept(Value::Int(listener)), "accept");
            accepted.fetch_add(1, Ordering::Relaxed);
            for _ in 0..M {
                let req = bytes(tcp_read(Value::Int(conn)), "wread");
                // echo exactly what was read back to the client
                tcp_write(Value::Tuple(vec![Value::Int(conn), Value::Bytes(req)]));
            }
            tcp_close(Value::Int(conn));
        }));
    }

    // Client threads: connect, M write→read round-trips, verify each echo.
    let mut clients_h = Vec::new();
    for cid in 0..clients {
        let start = Arc::clone(&start);
        clients_h.push(thread::spawn(move || {
            start.wait();
            let conn = as_int(tcp_connect(Value::Tuple(vec![
                Value::Str(intern_str("127.0.0.1")), Value::Int(port),
            ])), "connect");
            for it in 0..M {
                let p = payload(cid, it);
                tcp_write(Value::Tuple(vec![Value::Int(conn), Value::Bytes(p.clone())]));
                let echo = bytes(tcp_read(Value::Int(conn)), "cread");
                assert_eq!(echo, p, "echo mismatch cid={cid} it={it} — mis-routed handle?");
            }
            tcp_close(Value::Int(conn));
        }));
    }

    start.wait();
    let t0 = Instant::now();
    for h in clients_h { h.join().unwrap(); }
    for h in workers { h.join().unwrap(); }
    t0.elapsed().as_secs_f64()
}

fn main() {
    init_runtime();
    let mode = std::env::var("AXVERITY_NET_REGISTRY").unwrap_or_else(|_| "shared".into());
    let cores = thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
    println!("net registry bench — mode={mode}, cores={cores}, M={M} round-trips/conn");
    println!("  {:>3}  {:>10}  {:>14}  {:>8}", "C", "wall(s)", "roundtrips/s", "note");
    for &c in &[1usize, 2, 4, 8, 16] {
        let dt = run(&mode, c);
        let rt = (c * M) as f64 / dt;
        println!("  {:>3}  {:>10.3}  {:>14.0}  {}", c, dt, rt, "ok(verified)");
    }
}

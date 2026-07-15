//! Real-process SIGKILL recovery harness for the mmapseg durability primitive
//! (AXVERITY_STORAGE_SUBSTRATE_DURABILITY_V1). Driven by
//! scripts/verify-mmapseg-crash.sh (in the axVerity repo).
//!
//!   mmapseg_crashtest hold  <path> <cap> <n>   append n records with NO msync,
//!                                              then hold forever for an external
//!                                              `kill -9` (genuine SIGKILL).
//!   mmapseg_crashtest count <path> <cap>       open (recover) the segment and
//!                                              print the number of intact records.
//!
//! The point: after a real SIGKILL of `hold` (no msync, no Drop, no flush), `count`
//! in a FRESH process must recover every appended record from the MAP_SHARED page
//! cache — crash-safety with zero write-path sync.
use axis_codegen_bridge::runtime::mmapseg::{
    mmapseg_append, mmapseg_frontier, mmapseg_open, mmapseg_read,
};
use axis_codegen_bridge::runtime::value::{intern_str, Value};

fn as_int(v: Value) -> i64 {
    match v {
        Value::Int(n) => n,
        other => panic!("expected Int, got {:?}", other),
    }
}

fn open(path: &str, cap: i64) -> i64 {
    as_int(mmapseg_open(Value::Tuple(vec![
        Value::Str(intern_str(path)),
        Value::Int(cap),
    ])))
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mode = a.get(1).map(|s| s.as_str()).unwrap_or("");
    let path = &a[2];
    let cap: i64 = a[3].parse().unwrap();
    let h = open(path, cap);
    match mode {
        "hold" => {
            let n: usize = a[4].parse().unwrap();
            for i in 0..n {
                let rec = format!("rec-{}", i).into_bytes();
                let off = as_int(mmapseg_append(Value::Tuple(vec![
                    Value::Int(h),
                    Value::Bytes(rec),
                ])));
                assert!(off >= 0, "segment full at record {}", i);
            }
            println!("APPENDED {} (no msync); holding for SIGKILL", n);
            use std::io::Write;
            std::io::stdout().flush().ok();
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
        "count" => {
            // open recovered the frontier; walk frames up to it and count.
            let frontier = as_int(mmapseg_frontier(Value::Int(h))) as usize;
            let mut off = 0usize;
            let mut count = 0usize;
            while off < frontier {
                let payload = match mmapseg_read(Value::Tuple(vec![
                    Value::Int(h),
                    Value::Int(off as i64),
                ])) {
                    Value::Bytes(b) => b,
                    _ => break,
                };
                if payload.is_empty() {
                    break;
                }
                off += 8 + payload.len(); // FRAME_OVERHEAD + payload
                count += 1;
            }
            println!("{}", count);
        }
        _ => {
            eprintln!("usage: mmapseg_crashtest hold|count <path> <cap> [n]");
            std::process::exit(2);
        }
    }
}

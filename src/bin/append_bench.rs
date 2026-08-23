//! append_bench — AXVERITY_EXTENT_WRITE_PATH step 4.
//!
//! Prices the per-RECORD cost of the two packing paths, calling the real
//! bridge functions rather than re-implementations.
//!
//!   arena      what axVerity-working2's packer does today: mem_copy_raw the
//!              record into the hotframe block. No syscall, no allocation, no
//!              hashing. The block is written once, later, from the raw pointer.
//!   slab       what slablock does: a Value::Bytes (Vec<u8>) per record, then
//!              slab_append -> file.write_all (one write(2) per record) plus a
//!              running Sha256 update over every byte.
//!
//! Three arms decompose the slab cost so the question "is slab_append_raw worth
//! building?" gets an answer rather than an opinion:
//!
//!   vec        the Vec<u8> allocation + copy ALONE — exactly what a raw-pointer
//!              variant of slab_append would remove, and nothing else.
//!   slab       the whole thing.
//!   slab - vec is therefore the syscall + hash floor that NO call-convention
//!              change can remove.
//!
//! Usage: append_bench <rec_bytes> <nrecords> <dir>

use axis_codegen_bridge::runtime::{rawmem, slablock};
use std::sync::Arc;
use std::time::Instant;

fn as_int(v: axis_codegen_bridge::runtime::value::Value) -> i64 {
    match v {
        axis_codegen_bridge::runtime::value::Value::Int(n) => n,
        other => panic!("expected Int, got {:?}", other),
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 4 {
        eprintln!("usage: append_bench <rec_bytes> <nrecords> <dir>");
        std::process::exit(2);
    }
    let rec: usize = a[1].parse().expect("rec_bytes");
    let n: usize = a[2].parse().expect("nrecords");
    let dir = a[3].clone();
    std::fs::create_dir_all(&dir).expect("mkdir");

    // Source: one record-shaped run of bytes, reused. Stands in for the input
    // buffer the packer copies out of.
    let src: Vec<u8> = (0..rec).map(|i| (i % 251) as u8).collect();
    let src_ptr = src.as_ptr() as i64;

    // ── arena: mem_copy_raw into a hotframe-sized block, rotating at capacity
    let cap: i64 = 16 * 1024 * 1024;
    // mem_reserve_raw returns Tuple[ptr, capacity] — the same shape M1's
    // tuple_field(res, 0) unpacks.
    let arena = match rawmem::mem_reserve_raw(cap) {
        axis_codegen_bridge::runtime::value::Value::Tuple(f) => as_int(f[0].clone()),
        other => panic!("mem_reserve_raw: expected Tuple, got {:?}", other),
    };
    let t = Instant::now();
    let mut off: i64 = 0;
    for _ in 0..n {
        if off + rec as i64 > cap {
            off = 0; // block rotation — the packer's seal, without the I/O
        }
        rawmem::mem_copy_raw(arena, off, src_ptr, 0, rec as i64);
        off += rec as i64;
    }
    let arena_ns = t.elapsed().as_nanos() as f64 / n as f64;

    // ── vec: the Value::Bytes materialisation alone
    let t = Instant::now();
    let mut sink: u64 = 0;
    for _ in 0..n {
        let v: Vec<u8> = src.clone();
        sink = sink.wrapping_add(v[0] as u64);
        std::hint::black_box(&v);
    }
    let vec_ns = t.elapsed().as_nanos() as f64 / n as f64;
    std::hint::black_box(sink);

    // ── slab: Vec + slab_append (write(2) per record + Sha256 update)
    let h = as_int(slablock::slab_open(Arc::from(dir.as_str()), 200_000, cap));
    let t = Instant::now();
    for _ in 0..n {
        slablock::slab_append(h, src.clone());
    }
    let slab_ns = t.elapsed().as_nanos() as f64 / n as f64;

    let floor = slab_ns - vec_ns;
    println!(
        "rec={} n={}  arena={:.0} ns  vec={:.0} ns  slab={:.0} ns  slab_minus_vec={:.0} ns  slab/arena={:.1}x",
        rec, n, arena_ns, vec_ns, slab_ns, floor, slab_ns / arena_ns
    );
}

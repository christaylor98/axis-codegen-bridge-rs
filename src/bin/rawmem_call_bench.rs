// AXVERITY_RAWMEM_CALL_CONVENTION_BENCH_V1 — measures how much of
// mem_copy_raw's per-call cost was the Value::Tuple boxing convention the
// emitter used to use, vs. the native positional-arg call it uses now
// (AXVERITY_RAWMEM_CALL_CONVENTION_V1). `mem_copy_raw` itself is the
// "native" arm below; `mem_copy_raw_boxed` is a frozen replica of its old
// Value::Tuple-boxed body, kept only so this comparison still has something
// to compare the current fn against after the migration.
//
//   cargo run --release --bin rawmem_call_bench

use std::hint::black_box;
use std::time::Instant;

use axis_codegen_bridge::runtime::rawmem::mem_copy_raw;
use axis_codegen_bridge::runtime::value::Value;

const REPS: usize = 7;

/// Iteration count scaled so total bytes moved per arm stays bounded — a
/// fixed iters count that's fine at len=8 would move ~1TB at len=65536.
fn iters_for(len: usize) -> usize {
    (400_000_000 / len.max(1)).clamp(20_000, 2_000_000)
}

/// Frozen replica of mem_copy_raw's pre-migration body — the Value::Tuple
/// unpack this fn used before AXVERITY_RAWMEM_CALL_CONVENTION_V1 dropped it.
#[inline(never)]
fn mem_copy_raw_boxed(args: Value) -> Value {
    let (dst_ptr_v, dst_offset_v, src_ptr_v, src_offset_v, len_v) = match args {
        Value::Tuple(es) if es.len() == 5 => {
            let mut it = es.into_iter();
            (
                it.next().unwrap(), it.next().unwrap(), it.next().unwrap(),
                it.next().unwrap(), it.next().unwrap(),
            )
        }
        other => panic!("mem_copy_raw_boxed: expected a 5-Tuple, got {:?}", other),
    };
    let as_int = |v: Value| match v { Value::Int(n) => n, other => panic!("expected Int, got {:?}", other) };
    let dst_ptr = as_int(dst_ptr_v);
    let dst_offset = as_int(dst_offset_v);
    let src_ptr = as_int(src_ptr_v);
    let src_offset = as_int(src_offset_v);
    let len = as_int(len_v);
    if dst_offset < 0 || src_offset < 0 || len < 0 {
        panic!(
            "mem_copy_raw_boxed: offsets and len must be >= 0, got dst_offset={}, src_offset={}, len={}",
            dst_offset, src_offset, len
        );
    }
    unsafe {
        let src = (src_ptr as *const u8).add(src_offset as usize);
        let dst = (dst_ptr as *mut u8).add(dst_offset as usize);
        std::ptr::copy_nonoverlapping(src, dst, len as usize);
    }
    Value::Unit
}

fn median_ns(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    samples[samples.len() / 2]
}

fn time_arm<F: FnMut()>(iters: usize, mut f: F) -> f64 {
    let mut samples = Vec::with_capacity(REPS);
    for _ in 0..REPS {
        let start = Instant::now();
        for _ in 0..iters {
            f();
        }
        let elapsed = start.elapsed();
        samples.push(elapsed.as_nanos() as f64 / iters as f64);
    }
    median_ns(samples)
}

fn run_len(len: usize) {
    let buf_len = len.max(8);
    let mut src = vec![0u8; buf_len];
    let mut dst = vec![0u8; buf_len];
    for (i, b) in src.iter_mut().enumerate() {
        *b = i as u8;
    }
    let src_ptr = src.as_mut_ptr() as i64;
    let dst_ptr = dst.as_mut_ptr() as i64;
    let len_i = len as i64;
    let iters = iters_for(len);

    // Null arm: isolates the loop/black_box floor itself, same methodology
    // as the earlier CCall measurement (null arm subtracted to isolate the
    // harness floor) — see project_ccall_cost_65ns memory.
    let null_ns = time_arm(iters, || {
        black_box(dst_ptr);
        black_box(src_ptr);
        black_box(len_i);
    });

    let boxed_ns = time_arm(iters, || {
        let args = Value::Tuple(vec![
            Value::Int(black_box(dst_ptr)),
            Value::Int(0),
            Value::Int(black_box(src_ptr)),
            Value::Int(0),
            Value::Int(black_box(len_i)),
        ]);
        black_box(mem_copy_raw_boxed(args));
    });

    let native_ns = time_arm(iters, || {
        black_box(mem_copy_raw(
            black_box(dst_ptr),
            0,
            black_box(src_ptr),
            0,
            black_box(len_i),
        ));
    });

    let boxed_net = boxed_ns - null_ns;
    let native_net = native_ns - null_ns;

    println!(
        "len={:>6}  iters={:>9}  null={:>7.1}ns  boxed={:>7.1}ns (net {:>7.1})  native={:>7.1}ns (net {:>7.1})  ratio={:>5.1}x  saved={:>7.1}ns/call",
        len, iters, null_ns, boxed_ns, boxed_net, native_ns, native_net,
        boxed_net / native_net.max(0.001),
        boxed_net - native_net,
    );

    std::mem::forget(src);
    std::mem::forget(dst);
}

fn main() {
    println!("mem_copy_raw: Value::Tuple-boxed call vs. native positional-arg call, same body");
    println!("{} reps/len, iters scaled per-len so total bytes moved stays bounded, median of reps reported\n", REPS);
    for len in [8usize, 64, 4096, 65536] {
        run_len(len);
    }
}

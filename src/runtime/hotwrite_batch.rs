//! AXVERITY_HOTWRITE_ADMISSION_MINIMAL_CAPTURE_V1 — ISOLATION MEASUREMENT ONLY.
//!
//! Single-call collapse of the full per-record iterate/capture/stamp/hash/write
//! cycle into ONE bridge invocation (vs one M1->bridge call per record — the
//! ~5-hop/record cost the size-independent 75% was measured to be). NOT wired
//! into any real write path: a throwaway measurement variant, same standard as
//! the accumulator test. Reproduces the M1 workload's EXACT SMALL/MEDIUM/LARGE
//! record bytes (20/70/10 by i%10, byte-identical to hrw_small/large_bytes.m1
//! and hotwrite_record.m1) so the generation work matches the 46k baseline;
//! `do_key`==1 additionally computes the content-hash KEY sha256(record) per
//! record (the admission cycle's "hash" step, Chris's key decision).
//!
//! Safe Rust only — NO new unsafe surface (UNSAFE_SCOPED_TO_EXISTING_HOTMEM_BRIDGE).

use super::value::Value;
use sha2::{Digest, Sha256};
use std::hint::black_box;

const HEX: &[u8; 16] = b"0123456789abcdef";

/// SMALL (pos 0,1): "S|<12-digit i>|" padded with 'x' to 64 bytes.
#[inline]
fn gen_small(i: i64, out: &mut Vec<u8>) {
    out.clear();
    out.extend_from_slice(b"S|");
    out.extend_from_slice(format!("{:012}", i).as_bytes());
    out.push(b'|');
    while out.len() < 64 { out.push(b'x'); }
}

/// LARGE (pos 9): "L|<12-digit i>|" padded with 'x' to 4096 bytes.
#[inline]
fn gen_large(i: i64, out: &mut Vec<u8>) {
    out.clear();
    out.extend_from_slice(b"L|");
    out.extend_from_slice(format!("{:012}", i).as_bytes());
    out.push(b'|');
    while out.len() < 4096 { out.push(b'x'); }
}

/// MEDIUM (pos 2..8): payload = "HW1|<12>|" padded to 100 B; record =
/// hex(sha256(payload))(64) + zeropad(len=100,10) + payload = 174 B.
/// Byte-identical to hotwrite_record.m1 (note: baseline already hashes here).
#[inline]
fn gen_medium(i: i64, payload: &mut Vec<u8>, out: &mut Vec<u8>) {
    payload.clear();
    payload.extend_from_slice(b"HW1|");
    payload.extend_from_slice(format!("{:012}", i).as_bytes());
    payload.push(b'|');
    while payload.len() < 100 { payload.push(b'x'); }
    let digest = Sha256::digest(&payload[..]);
    out.clear();
    for b in digest.iter() {
        out.push(HEX[(b >> 4) as usize]);
        out.push(HEX[(b & 0xf) as usize]);
    }
    out.extend_from_slice(format!("{:010}", payload.len()).as_bytes());
    out.extend_from_slice(payload);
}

/// `hotwrite_batch_run(n: Int, block_size: Int, do_key: Int) -> Int`
/// Returns total bytes written (also defeats dead-code elimination of the loop).
#[track_caller]
pub fn hotwrite_batch_run(args: Value) -> Value {
    let (n, block_size, do_key) = match args {
        Value::Tuple(ref es) if es.len() == 3 => {
            let g = |k: usize| match &es[k] {
                Value::Int(v) => *v,
                other => panic!("hotwrite_batch_run: arg {} must be Int, got {:?}", k, other),
            };
            (g(0), g(1), g(2))
        }
        other => panic!("hotwrite_batch_run: expected 3-tuple (Int,Int,Int), got {:?}", other),
    };
    let bs = block_size as usize;
    let mut blocks: Vec<Vec<u8>> = Vec::new();   // kept => real no-flush RAM accumulation
    let mut arena: Vec<u8> = vec![0u8; bs];
    let mut cursor: usize = 0;
    let mut total: i64 = 0;
    let mut rec: Vec<u8> = Vec::with_capacity(4096);
    let mut payload: Vec<u8> = Vec::with_capacity(128);
    for i in 0..n {
        let pos = i % 10;
        if pos < 2 { gen_small(i, &mut rec); }
        else if pos == 9 { gen_large(i, &mut rec); }
        else { gen_medium(i, &mut payload, &mut rec); }
        if do_key == 1 {
            let key = Sha256::digest(&rec[..]);   // content-hash KEY
            black_box(&key);
        }
        let len = rec.len();
        if cursor + len > bs {
            blocks.push(std::mem::replace(&mut arena, vec![0u8; bs]));
            cursor = 0;
        }
        arena[cursor..cursor + len].copy_from_slice(&rec);
        cursor += len;
        total += len as i64;
    }
    blocks.push(arena);
    black_box(&blocks);
    Value::Int(total)
}

// ── C variant: standalone isolated FFI surface (src/runtime/hotwrite_batch.c) ──
extern "C" {
    fn hotwrite_batch_c_run(n: i64, block_size: i64, do_key: i64) -> i64;
    fn hotwrite_batch_c_durable(dir: *const std::os::raw::c_char, n: i64, block_size: i64, do_key: i64) -> i64;
}

/// `hotwrite_batch_run_c_durable(dir: Text, n: Int, block_size: Int, do_key: Int) -> Int`
/// Phase A: durable full-cycle collapse — writes block-<seq>.bin + manifest.log
/// with real per-block fsync (passes hotwrite-workload-verify.py).
#[track_caller]
pub fn hotwrite_batch_run_c_durable(args: Value) -> Value {
    let (dir, n, block_size, do_key) = match args {
        Value::Tuple(ref es) if es.len() == 4 => {
            let dir = match &es[0] {
                Value::Str(s) => s.to_string(),
                other => panic!("hotwrite_batch_run_c_durable: arg 0 (dir) must be Text, got {:?}", other),
            };
            let g = |k: usize| match &es[k] {
                Value::Int(v) => *v,
                other => panic!("hotwrite_batch_run_c_durable: arg {} must be Int, got {:?}", k, other),
            };
            (dir, g(1), g(2), g(3))
        }
        other => panic!("hotwrite_batch_run_c_durable: expected 4-tuple (Text,Int,Int,Int), got {:?}", other),
    };
    let cdir = std::ffi::CString::new(dir).expect("hotwrite_batch_run_c_durable: dir has interior NUL");
    let total = unsafe { hotwrite_batch_c_durable(cdir.as_ptr(), n, block_size, do_key) };
    Value::Int(total)
}

/// `hotwrite_batch_run_c(n: Int, block_size: Int, do_key: Int) -> Int`
/// Thin Rust->C FFI shim; all per-record work happens inside the C TU.
#[track_caller]
pub fn hotwrite_batch_run_c(args: Value) -> Value {
    let (n, block_size, do_key) = match args {
        Value::Tuple(ref es) if es.len() == 3 => {
            let g = |k: usize| match &es[k] {
                Value::Int(v) => *v,
                other => panic!("hotwrite_batch_run_c: arg {} must be Int, got {:?}", k, other),
            };
            (g(0), g(1), g(2))
        }
        other => panic!("hotwrite_batch_run_c: expected 3-tuple (Int,Int,Int), got {:?}", other),
    };
    let total = unsafe { hotwrite_batch_c_run(n, block_size, do_key) };
    Value::Int(total)
}

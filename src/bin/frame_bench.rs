//! frame_bench — AXVERITY_EXTENT_WRITE_PATH: which per-record integrity frame?
//!
//! Three candidates, measured on the record sizes axVerity actually stores:
//!
//!   slice4    H(64 hex) | P(10) | V(10) | env | payload   — the decided
//!             one-way-door format. 84 bytes + env, sha256 over the payload.
//!   trailer   varint | payload | sha256(payload)[0..k]     — keep the existing
//!             LEB128 frame, append a truncated hash.
//!   fnv       varint | payload | fnv1a64                   — what mmapseg.rs
//!             already uses for torn-tail detection.
use sha2::{Digest, Sha256};
use std::time::Instant;

fn fnv1a64(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in b {
        h ^= x as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

fn main() {
    let n = 200_000usize;
    println!("{:>8} {:>12} {:>12} {:>12}   {:>10} {:>10} {:>10}",
             "rec", "sha256 ns", "fnv1a ns", "sha/fnv", "slice4 ovh", "trail-8", "fnv-8");
    for rec in [8usize, 14, 60, 349, 4096] {
        let buf: Vec<u8> = (0..rec).map(|i| (i % 251) as u8).collect();

        let t = Instant::now();
        let mut acc = 0u8;
        for _ in 0..n {
            let mut h = Sha256::new();
            h.update(&buf);
            acc ^= h.finalize()[0];
        }
        let sha = t.elapsed().as_nanos() as f64 / n as f64;
        std::hint::black_box(acc);

        let t = Instant::now();
        let mut a2 = 0u64;
        for _ in 0..n {
            a2 ^= fnv1a64(&buf);
        }
        let fnv = t.elapsed().as_nanos() as f64 / n as f64;
        std::hint::black_box(a2);

        // Space overhead as a percentage of the record, framing included.
        let vlen = if rec < 128 { 1 } else if rec < 16384 { 2 } else { 3 };
        let slice4 = 64 + 10 + 10;                    // H | P | V, env empty
        let trail8 = vlen + 8;                        // varint + 8-byte truncated sha256
        let fnv8 = vlen + 8;                          // varint + fnv1a64
        println!("{:>8} {:>12.0} {:>12.0} {:>11.1}x   {:>9.0}% {:>9.0}% {:>9.0}%",
                 rec, sha, fnv, sha / fnv,
                 100.0 * slice4 as f64 / rec as f64,
                 100.0 * trail8 as f64 / rec as f64,
                 100.0 * fnv8 as f64 / rec as f64);
    }
}

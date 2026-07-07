//! BRIDGE_CDC_V1 (AXVERITY_LANDING_B_BLOB_CHUNKER) — a standalone, reusable
//! content-defined chunker primitive.
//!
//! ## Why this is a bridge primitive (req-bridge-rule, "impossible in M1" prong)
//!
//! Content-defined chunking is an inherently per-byte scan: a rolling hash must
//! advance one byte at a time across the WHOLE payload (~10^9 steps for a 1 GB
//! blob), and the boundaries ARE a function of every byte — there is no
//! sub-linear formulation. M1's only per-byte access is `bytes_slice(b,i,i+1)`,
//! which allocates a fresh `Vec<u8>` and crosses the M1↔Rust `Value` ABI *per
//! byte*, and a `loop_count` body is a capture-free fn-ref dispatched over that
//! same ABI per iteration. So a pure-M1 scan is ~10^9 × (heap alloc + ABI
//! marshal + interpreted arithmetic) — categorically the disqualified regime,
//! the same per-byte/O(N^2) fold that already forced `logbuf`/`bytes_concat`
//! bridge-side. Hence: the scan lives here in Rust; everything above it
//! (routing, per-chunk storage, manifest assembly, dedup, hash-verified
//! reconstruction) stays in M1/drivers.
//!
//! ## Algorithm — FastCDC (chosen at ALGORITHM_CHECKPOINT by Chris)
//!
//! Gear-hash rolling function (`fp = (fp << 1) + Gear[byte]`, one table lookup +
//! shift + add per byte — the fastest per-byte step of the CDC family, which is
//! the whole justification for paying the bridge-primitive cost) with
//! *normalized chunking*: a stricter mask (more one-bits ⇒ a cut is less likely
//! ⇒ chunks grow) is used until the running length reaches the average, then a
//! looser mask after it, which tightens the chunk-size distribution around the
//! target. A min-size skip (no boundary is even tested before `MIN`) bounds the
//! smallest chunk; `MAX` bounds the largest. Boundary stability under
//! insert/delete (the property that makes dedup work: inserting bytes at the
//! front of a file re-cuts only the edited region, leaving interior chunks of
//! unchanged regions byte-identical) is a consequence of the cut decision at
//! position `i` depending ONLY on bytes `[0..i]`, never on file position.
//!
//! ## Contract — STANDALONE, no DB knowledge (CHUNKER_STANDALONE)
//!
//!   `chunk_file(path: Text) -> Text`
//!
//! Streams `path` through a bounded (~MAX-byte) working window — never loads the
//! whole file, so no RAM spike — and returns one line per chunk:
//!
//!   `<offset>\t<len>\tsha256:<64hex>\n`
//!
//! Offsets/lens are byte offsets into the file; the hash is SHA-256 of the chunk
//! bytes formatted EXACTLY as `bytes_hash`/`content_hash` do (`"sha256:"` +
//! 64 lowercase hex), so a manifest hash equals the address `push_object`
//! returns for the same chunk. The primitive knows nothing about the object
//! store, WAL, or manifests — the FUSE/filesystem track (req-file-as-manifest)
//! reuses it verbatim. Chunk BYTES never leave this module; only metadata does.
//!
//! ## Determinism / dedup stability caveat
//!
//! The gear table and the masks are derived deterministically from fixed
//! constants and the average size, so the same bytes chunk identically on every
//! machine and every run — a hard requirement for cross-file/cross-version
//! dedup. Changing `AXVERITY_CDC_AVG_BYTES` changes every boundary; the average
//! must stay stable across a store for its chunks to dedup. The provisional
//! defaults (avg 1 MiB, min 256 KiB, max 4 MiB) come from gap-allocation-defaults
//! and are tunable, not a one-way door.
//!
//! Identity is sha256("chunk_file"), the bridge-wide convention.

use std::fs::File;
use std::io::{BufReader, Read};

use sha2::{Digest, Sha256};

use super::value::{intern_str, Value};

// ── Gear table: 256 fixed pseudo-random u64s (splitmix64, const-evaluated) ─────
//
// Const-computed from a fixed seed so the table is byte-identical on every build
// and machine. A stable table is load-bearing: two machines must cut the same
// bytes the same way or dedup silently breaks.

const fn gear_table() -> [u64; 256] {
    let mut t = [0u64; 256];
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut i = 0;
    while i < 256 {
        // splitmix64
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z = z ^ (z >> 31);
        t[i] = z;
        i += 1;
    }
    t
}

const GEAR: [u64; 256] = gear_table();

// ── Parameters ────────────────────────────────────────────────────────────────

const DEFAULT_AVG: usize = 1024 * 1024; // 1 MiB
const MIN_AVG: usize = 64; // guard against absurd env values

struct Params {
    min: usize,
    avg: usize,
    max: usize,
    mask_s: u64,
    mask_l: u64,
}

fn env_usize(key: &str) -> Option<usize> {
    std::env::var(key).ok().and_then(|s| s.parse::<usize>().ok()).filter(|n| *n > 0)
}

/// floor(log2(n)) for n >= 1.
fn floor_log2(n: usize) -> u32 {
    63 - (n as u64).leading_zeros()
}

/// Spread `bits` one-bits across the 64-bit word (positions 1..=61, evenly
/// spaced) for good gear-hash mixing — testing only the noisy lowest bit gives a
/// poor chunk-size distribution, so we avoid bit 0 and cluster in the mid range.
fn spread_mask(bits: u32) -> u64 {
    let bits = bits.clamp(1, 60);
    let mut m = 0u64;
    let mut k = 0u32;
    while k < bits {
        let pos = 1 + (k as u64 * 61 / bits as u64);
        m |= 1u64 << pos;
        k += 1;
    }
    m
}

fn params() -> Params {
    let avg = env_usize("AXVERITY_CDC_AVG_BYTES").unwrap_or(DEFAULT_AVG).max(MIN_AVG);
    let min = env_usize("AXVERITY_CDC_MIN_BYTES").unwrap_or(avg / 4).max(1);
    let max = env_usize("AXVERITY_CDC_MAX_BYTES").unwrap_or(avg * 4).max(min + 1);
    let b = floor_log2(avg);
    // Normalized chunking level 2: stricter mask before the average (b+2 bits),
    // looser after it (b-2 bits).
    let mask_s = spread_mask(b + 2);
    let mask_l = spread_mask(b.saturating_sub(2).max(1));
    Params { min, avg, max, mask_s, mask_l }
}

// ── FastCDC cut: length of the first chunk within `data` ────────────────────────
//
// Returns a length in `1..=min(data.len(), MAX)`. The decision at position `i`
// uses only `data[0..=i]`. When `data.len() <= MIN` the whole remainder is one
// (final) chunk.

fn fastcdc_cut(data: &[u8], p: &Params) -> usize {
    let len = data.len();
    if len <= p.min {
        return len;
    }
    let mut n = len;
    if n > p.max {
        n = p.max;
    }
    let mut normal = p.avg;
    if normal > n {
        normal = n;
    }
    let mut fp: u64 = 0;
    let mut i = p.min;
    // Region [MIN, normal): stricter mask (reluctant to cut → avoid tiny chunks).
    while i < normal {
        fp = (fp << 1).wrapping_add(GEAR[data[i] as usize]);
        if fp & p.mask_s == 0 {
            return i;
        }
        i += 1;
    }
    // Region [normal, n): looser mask (eager to cut → avoid oversized chunks).
    while i < n {
        fp = (fp << 1).wrapping_add(GEAR[data[i] as usize]);
        if fp & p.mask_l == 0 {
            return i;
        }
        i += 1;
    }
    // No content boundary found: cut at MAX (or EOF).
    n
}

fn sha256_addr(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let hex: String = digest.iter().map(|b| format!("{:02x}", b)).collect();
    format!("sha256:{}", hex)
}

// ── Streaming driver: chunk a byte reader, emit the manifest lines ─────────────
//
// Keeps a bounded working window (fills to >= MAX, or EOF, before each cut) so a
// content boundary is only ever chosen from fully-available bytes — never at a
// buffer edge — and peak memory is ~MAX regardless of file size.

fn chunk_reader<R: Read>(mut r: R, p: &Params) -> std::io::Result<String> {
    let mut out = String::new();
    let mut buf: Vec<u8> = Vec::with_capacity(p.max + 65536);
    let mut abs: u64 = 0;
    let mut eof = false;
    let mut tmp = [0u8; 65536];
    loop {
        while !eof && buf.len() < p.max {
            let k = r.read(&mut tmp)?;
            if k == 0 {
                eof = true;
            } else {
                buf.extend_from_slice(&tmp[..k]);
            }
        }
        if buf.is_empty() {
            break;
        }
        let cut = fastcdc_cut(&buf, p);
        let addr = sha256_addr(&buf[..cut]);
        out.push_str(&format!("{}\t{}\t{}\n", abs, cut, addr));
        abs += cut as u64;
        buf.drain(..cut);
    }
    // Emit no trailing newline: each line is `\n`-terminated above, so drop the
    // final one — the CLI outer's io_println adds exactly one. Without this the
    // printed manifest ends in a blank line and a naive `cut -f3` sees a phantom
    // empty chunk hash. Empty file => "" (io_println prints a single newline).
    if out.ends_with('\n') {
        out.pop();
    }
    Ok(out)
}

// ── chunk_file ─────────────────────────────────────────────────────────────────

/// `chunk_file(path: Text) -> Text`
///
/// Content-defined-chunk a file. Returns one `"<offset>\t<len>\tsha256:<hex>\n"`
/// line per chunk (empty string for an empty file). Panics on any OS error
/// opening/reading `path` — the panic-on-OS-error discipline of the rest of the
/// fs surface.
#[track_caller]
pub fn chunk_file(arg: Value) -> Value {
    let path = match arg {
        Value::Str(h) => super::value::get_str(h),
        other => panic!("chunk_file: expected Text path, got {:?}", other),
    };
    let f = File::open(&path).unwrap_or_else(|e| panic!("chunk_file({}): open: {}", path, e));
    let p = params();
    let out = chunk_reader(BufReader::new(f), &p)
        .unwrap_or_else(|e| panic!("chunk_file({}): read: {}", path, e));
    Value::Str(intern_str(&out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Deterministic pseudo-random bytes (splitmix64) — reproducible test data.
    fn prng_bytes(seed: u64, n: usize) -> Vec<u8> {
        let mut x = seed;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z = z ^ (z >> 31);
            v.push((z & 0xff) as u8);
        }
        v
    }

    // Small params so multi-chunk behavior shows up on KB-scale test data.
    fn test_params() -> Params {
        let avg = 4096usize;
        let b = floor_log2(avg);
        Params {
            min: avg / 4,
            avg,
            max: avg * 4,
            mask_s: spread_mask(b + 2),
            mask_l: spread_mask(b.saturating_sub(2).max(1)),
        }
    }

    fn chunks(data: &[u8], p: &Params) -> Vec<(u64, usize, String)> {
        let s = chunk_reader(Cursor::new(data.to_vec()), p).unwrap();
        s.lines()
            .map(|l| {
                let mut it = l.split('\t');
                let off: u64 = it.next().unwrap().parse().unwrap();
                let len: usize = it.next().unwrap().parse().unwrap();
                let h = it.next().unwrap().to_string();
                (off, len, h)
            })
            .collect()
    }

    #[test]
    fn contiguous_covering_and_sized() {
        let p = test_params();
        let data = prng_bytes(1, 200_000);
        let cs = chunks(&data, &p);
        assert!(cs.len() > 1, "expected multiple chunks");
        let mut expect_off = 0u64;
        for (i, (off, len, h)) in cs.iter().enumerate() {
            assert_eq!(*off, expect_off, "chunk {} offset non-contiguous", i);
            assert!(*len <= p.max, "chunk {} len {} exceeds MAX {}", i, len, p.max);
            // every chunk except the last must be >= MIN
            if i + 1 < cs.len() {
                assert!(*len >= p.min, "interior chunk {} len {} below MIN {}", i, len, p.min);
            }
            // hash matches an independent recompute over the exact span
            let span = &data[*off as usize..*off as usize + *len];
            assert_eq!(*h, sha256_addr(span), "chunk {} hash mismatch", i);
            expect_off += *len as u64;
        }
        assert_eq!(expect_off as usize, data.len(), "chunks do not cover the file");
    }

    #[test]
    fn deterministic_across_runs() {
        let p = test_params();
        let data = prng_bytes(7, 150_000);
        assert_eq!(chunks(&data, &p), chunks(&data, &p));
    }

    #[test]
    fn streaming_matches_regardless_of_read_granularity() {
        // A reader that dribbles 1 byte at a time must yield the same boundaries
        // as a bulk reader — proves the fill-to-MAX window is correct.
        struct Dribble(Vec<u8>, usize);
        impl Read for Dribble {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                if self.1 >= self.0.len() || out.is_empty() {
                    return Ok(0);
                }
                out[0] = self.0[self.1];
                self.1 += 1;
                Ok(1)
            }
        }
        let p = test_params();
        let data = prng_bytes(3, 120_000);
        let bulk = chunk_reader(Cursor::new(data.clone()), &p).unwrap();
        let dribbled = chunk_reader(Dribble(data, 0), &p).unwrap();
        assert_eq!(bulk, dribbled);
    }

    #[test]
    fn boundary_stability_under_front_insertion() {
        // THE dedup property: inserting bytes at the front of a file must leave
        // interior chunks of the unchanged region byte-identical (shared hashes),
        // rather than shifting every boundary. Build B = junk-prefix ++ R and
        // A = R; assert they share most interior chunk hashes of R.
        let p = test_params();
        let r = prng_bytes(42, 300_000);
        let prefix = prng_bytes(99, 5000);
        let mut b = prefix.clone();
        b.extend_from_slice(&r);

        let a_hashes: std::collections::HashSet<String> =
            chunks(&r, &p).into_iter().map(|c| c.2).collect();
        let b_hashes: Vec<String> = chunks(&b, &p).into_iter().map(|c| c.2).collect();
        let shared = b_hashes.iter().filter(|h| a_hashes.contains(*h)).count();

        // With a 5 KB shifting prefix, only the chunks near the splice differ;
        // the bulk of R's interior chunks must reappear identically in B.
        assert!(
            shared as f64 > 0.6 * a_hashes.len() as f64,
            "expected boundary stability (shared {} of {} A-chunks)",
            shared,
            a_hashes.len()
        );
    }

    #[test]
    fn empty_file_yields_no_chunks() {
        let p = test_params();
        assert_eq!(chunk_reader(Cursor::new(Vec::<u8>::new()), &p).unwrap(), "");
    }

    #[test]
    fn identical_interior_region_dedups() {
        // Two different blobs that embed the SAME large region must produce at
        // least one byte-identical interior chunk (same hash) — the concrete
        // shape DEDUP_VERIFY checks end-to-end.
        let p = test_params();
        let shared = prng_bytes(500, 100_000);
        let mut a = prng_bytes(1, 20_000);
        a.extend_from_slice(&shared);
        a.extend_from_slice(&prng_bytes(2, 20_000));
        let mut b = prng_bytes(3, 33_000);
        b.extend_from_slice(&shared);
        b.extend_from_slice(&prng_bytes(4, 15_000));

        let ah: std::collections::HashSet<String> =
            chunks(&a, &p).into_iter().map(|c| c.2).collect();
        let bh: std::collections::HashSet<String> =
            chunks(&b, &p).into_iter().map(|c| c.2).collect();
        let shared_ct = ah.intersection(&bh).count();
        assert!(shared_ct >= 1, "expected >=1 shared chunk from the shared region");
    }
}

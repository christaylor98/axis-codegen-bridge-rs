//! GPU_P5_KERNELS_V1 — the memo side of the P5 search on the GPU (axMachina P6). Rust, compiled to PTX.
//!
//! One thread per (combination, column). A combination j of a job picks one entry from each argument pool
//! (i1 = j / (c2 c3), i2 = (j / c3) mod c2, i3 = j mod c3: itertools.product order); an entry is a B id and its value
//! at a column is `bvec[id * n + col]` (a value id). The key (op code, a, b, c) is looked up in an open-addressing
//! memo table on the device:
//!   discover  a missing key is claimed (state 0 -> 1 -> 2, value PENDING) and its slot listed: every distinct miss
//!             once, whatever the thread timing
//!   produce   every key is present and answered: the value ids of the combination's output vector
//!   insert    keys with their answers put into an empty table (a table grown on the host)
//!   setval    answers written into claimed slots
//! The host (src/runtime/gpu_p5.rs) answers the misses through AI3 and keeps everything else.
#![no_std]
#![feature(abi_ptx, stdarch_nvptx)]

use core::arch::nvptx::*;
use core::sync::atomic::{AtomicU32, Ordering};

pub const PENDING: u32 = 0xFFFF_FFFF;
pub const NONE: u32 = 0xFFFF_FFFE;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[inline(always)]
unsafe fn tid() -> u64 {
    (_block_idx_x() as u64) * (_block_dim_x() as u64) + (_thread_idx_x() as u64)
}

#[inline(always)]
unsafe fn stride() -> u64 {
    (_grid_dim_x() as u64) * (_block_dim_x() as u64)
}

#[inline(always)]
fn mix(k0: u64, k1: u64) -> u64 {
    let mut z = k0 ^ k1.rotate_left(29) ^ 0x9E37_79B9_7F4A_7C15;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The job: op code, arity, pool sizes 2 and 3, and the three pools (B ids).
#[repr(C)]
pub struct Job {
    pub code: u32,
    pub k: u32,
    pub c2: u32,
    pub c3: u32,
    pub p1: *const u32,
    pub p2: *const u32,
    pub p3: *const u32,
}

/// The memo table.
#[repr(C)]
pub struct Table {
    pub st: *const AtomicU32,
    pub k0: *mut u64,
    pub k1: *mut u64,
    pub val: *mut u32,
    pub mask: u64,
}

#[inline(always)]
unsafe fn key(job: &Job, bvec: *const u32, n: u64, j: u64, col: u64) -> (u64, u64) {
    let c3 = job.c3 as u64;
    let c2 = job.c2 as u64;
    let i3 = j % c3;
    let i2 = (j / c3) % c2;
    let i1 = j / (c3 * c2);
    let a = *bvec.add((*job.p1.add(i1 as usize) as u64 * n + col) as usize);
    let b = if job.k >= 2 { *bvec.add((*job.p2.add(i2 as usize) as u64 * n + col) as usize) } else { NONE };
    let c = if job.k >= 3 { *bvec.add((*job.p3.add(i3 as usize) as u64 * n + col) as usize) } else { NONE };
    (((job.code as u64) << 32) | a as u64, ((b as u64) << 32) | c as u64)
}

/// Combinations j0 .. j0+m of the job, n columns: every missing key claimed once, its slot and key listed in `miss`,
/// `missk0`, `missk1` (`nmiss` counts them; past `misscap` flag bit 1). A full table sets flag bit 2.
#[no_mangle]
pub unsafe extern "ptx-kernel" fn p5_discover(job: Job, bvec: *const u32, n: u64, j0: u64, m: u64, t: Table,
                                              miss: *mut u32, missk0: *mut u64, missk1: *mut u64, nmiss: *const AtomicU32,
                                              misscap: u32, flag: *const AtomicU32) {
    let total = m * n;
    let mut x = tid();
    while x < total {
        let (k0, k1) = key(&job, bvec, n, j0 + x / n, x % n);
        let mut h = mix(k0, k1) & t.mask;
        let mut probes = 0u64;
        loop {
            let st = &*t.st.add(h as usize);
            let s = st.load(Ordering::Acquire);
            if s == 0 {
                if st.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                    *t.k0.add(h as usize) = k0;
                    *t.k1.add(h as usize) = k1;
                    *t.val.add(h as usize) = PENDING;
                    st.store(2, Ordering::Release);
                    let i = (*nmiss).fetch_add(1, Ordering::AcqRel);
                    if i < misscap {
                        *miss.add(i as usize) = h as u32;
                        *missk0.add(i as usize) = k0;
                        *missk1.add(i as usize) = k1;
                    } else { (*flag).fetch_or(1, Ordering::AcqRel); }
                    break;
                }
                (*flag).fetch_or(16, Ordering::AcqRel); // lost the claim to a writer: the host reruns the pass
                break;
            }
            if s == 1 { (*flag).fetch_or(16, Ordering::AcqRel); break; } // being written: never wait (a warp can deadlock)
            if core::ptr::read_volatile(t.k0.add(h as usize)) == k0 && core::ptr::read_volatile(t.k1.add(h as usize)) == k1 { break; }
            probes += 1;
            if probes > t.mask { (*flag).fetch_or(2, Ordering::AcqRel); break; }
            h = (h + 1) & t.mask;
        }
        x += stride();
    }
}

/// Combinations j0 .. j0+m: `out[(j - j0) * n + col]` = the answer of its key. A key absent or unanswered sets flag
/// bit 4 (a bug: discover and setval came first).
#[no_mangle]
pub unsafe extern "ptx-kernel" fn p5_produce(job: Job, bvec: *const u32, n: u64, j0: u64, m: u64, t: Table,
                                             out: *mut u32, flag: *const AtomicU32) {
    let total = m * n;
    let mut x = tid();
    while x < total {
        let (k0, k1) = key(&job, bvec, n, j0 + x / n, x % n);
        let mut h = mix(k0, k1) & t.mask;
        let mut probes = 0u64;
        let mut v = PENDING;
        loop {
            let s = (*t.st.add(h as usize)).load(Ordering::Acquire);
            if s == 0 { break; }
            if *t.k0.add(h as usize) == k0 && *t.k1.add(h as usize) == k1 { v = *t.val.add(h as usize); break; }
            probes += 1;
            if probes > t.mask { break; }
            h = (h + 1) & t.mask;
        }
        if v == PENDING { (*flag).fetch_or(4, Ordering::AcqRel); }
        *out.add(x as usize) = v;
        x += stride();
    }
}

/// `cnt` distinct keys with their answers into the table (a grown table; no key is present yet).
#[no_mangle]
pub unsafe extern "ptx-kernel" fn p5_insert(cnt: u64, keys0: *const u64, keys1: *const u64, vals: *const u32, t: Table,
                                            flag: *const AtomicU32) {
    let mut x = tid();
    while x < cnt {
        let k0 = *keys0.add(x as usize);
        let k1 = *keys1.add(x as usize);
        let mut h = mix(k0, k1) & t.mask;
        let mut probes = 0u64;
        loop {
            let st = &*t.st.add(h as usize);
            if st.load(Ordering::Acquire) == 0 && st.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                *t.k0.add(h as usize) = k0;
                *t.k1.add(h as usize) = k1;
                *t.val.add(h as usize) = *vals.add(x as usize);
                st.store(2, Ordering::Release);
                break;
            }
            probes += 1;
            if probes > t.mask { (*flag).fetch_or(2, Ordering::AcqRel); break; }
            h = (h + 1) & t.mask;
        }
        x += stride();
    }
}

/// `val[slots[i]] = vals[i]` for i < cnt.
#[no_mangle]
pub unsafe extern "ptx-kernel" fn p5_setval(cnt: u64, slots: *const u32, vals: *const u32, val: *mut u32) {
    let mut x = tid();
    while x < cnt {
        *val.add(*slots.add(x as usize) as usize) = *vals.add(x as usize);
        x += stride();
    }
}

// ── filtering and dedupe (GPU_P5_V2): the host only sees the combinations that reach a bank ──────────────────────

/// A vector's 128-bit fingerprint (two independent 64-bit mixes over its value ids).
#[inline(always)]
unsafe fn vhash(v: *const u32, n: u64) -> (u64, u64) {
    let mut a = 0x243F_6A88_85A3_08D3u64 ^ n;
    let mut b = 0x1319_8A2E_0370_7344u64 ^ n.rotate_left(17);
    let mut i = 0u64;
    while i < n {
        let x = *v.add(i as usize) as u64;
        a = mix(a ^ x, i);
        b = mix(b.rotate_left(23) ^ (x << 1), !i);
        i += 1;
    }
    (a, b)
}

/// A set / min-ordinal table keyed by (tag, fingerprint): `ord` holds the lowest ordinal put.
#[repr(C)]
pub struct VTable {
    pub st: *const AtomicU32,
    pub k0: *mut u64,
    pub k1: *mut u64,
    pub ord: *const core::sync::atomic::AtomicU64,
    pub mask: u64,
}

const RETRY: u64 = u64::MAX - 1;

/// Put (k0, k1) with ordinal `o` (min kept). -> its slot, u64::MAX when the table is full, RETRY when the slot is being
/// written by another thread (never waited for: the caller sets flag bit 16 and the host reruns the pass).
#[inline(always)]
unsafe fn vput(t: &VTable, k0: u64, k1: u64, o: u64) -> u64 {
    let mut h = mix(k0, k1) & t.mask;
    let mut probes = 0u64;
    loop {
        let st = &*t.st.add(h as usize);
        let s = st.load(Ordering::Acquire);
        if s == 0 {
            if st.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                *t.k0.add(h as usize) = k0;
                *t.k1.add(h as usize) = k1;
                (*t.ord.add(h as usize)).store(o, Ordering::Release); // the claimant's ordinal; others min into it once published
                st.store(2, Ordering::Release);
                return h;
            }
            return RETRY;
        }
        if s == 1 { return RETRY; }
        if core::ptr::read_volatile(t.k0.add(h as usize)) == k0 && core::ptr::read_volatile(t.k1.add(h as usize)) == k1 {
            (*t.ord.add(h as usize)).fetch_min(o, Ordering::AcqRel);
            return h;
        }
        probes += 1;
        if probes > t.mask { return u64::MAX; }
        h = (h + 1) & t.mask;
    }
}

/// The slot holding (k0, k1), or u64::MAX (read-only).
#[inline(always)]
unsafe fn vfind(t: &VTable, k0: u64, k1: u64) -> u64 {
    let mut h = mix(k0, k1) & t.mask;
    let mut probes = 0u64;
    loop {
        let s = (*t.st.add(h as usize)).load(Ordering::Acquire);
        if s == 0 { return u64::MAX; }
        if *t.k0.add(h as usize) == k0 && *t.k1.add(h as usize) == k1 { return h; }
        probes += 1;
        if probes > t.mask { return u64::MAX; }
        h = (h + 1) & t.mask;
    }
}

/// The per-combination filter context. `mask[col]` is the expected value id at a row column, NONE where any value
/// fits; `last` = 1 at the last size; `role` tags B entries, `ltag` the last size's role-less set.
#[repr(C)]
pub struct Filt {
    pub n: u64,
    pub eid: u32,
    pub last: u32,
    pub role: u64,
    pub ltag: u64,
    pub mask: *const u32,
    pub ord0: u64,
}

const REFUSED: u32 = 0xFFFF_FFFD;

/// Pass 1 over m produced vectors (`out`, combination c = ordinal ord0 + c): skip all-E, (last) a vector not giving
/// the rows, and a vector B already holds for the role; put the rest into `vt` with their ordinal. `cand[c]` = 1 for
/// a put one. A refused value anywhere sets flag bit 8; a full table bit 2.
#[no_mangle]
pub unsafe extern "ptx-kernel" fn p5_claim(f: Filt, out: *const u32, m: u64, bseen: VTable, vt: VTable, cand: *mut u8,
                                           flag: *const AtomicU32) {
    let mut c = tid();
    while c < m {
        let v = out.add((c * f.n) as usize);
        let mut alle = true;
        let mut fits = true;
        let mut i = 0u64;
        while i < f.n {
            let x = *v.add(i as usize);
            if x == REFUSED { (*flag).fetch_or(8, Ordering::AcqRel); }
            if x != f.eid { alle = false; }
            let mk = *f.mask.add(i as usize);
            if mk != NONE && mk != x { fits = false; }
            i += 1;
        }
        let mut keep = !alle && (f.last == 0 || fits);
        if keep {
            let (h0, h1) = vhash(v, f.n);
            let rk = h0 ^ f.role.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            if vfind(&bseen, rk, h1) != u64::MAX { keep = false; }
            if keep {
                let tag = if f.last != 0 { f.ltag } else { f.role };
                let r = vput(&vt, h0 ^ tag.wrapping_mul(0x9E37_79B9_7F4A_7C15), h1, f.ord0 + c);
                if r == u64::MAX { (*flag).fetch_or(2, Ordering::AcqRel); }
                if r == RETRY { (*flag).fetch_or(16, Ordering::AcqRel); }
            }
        }
        *cand.add(c as usize) = keep as u8;
        c += stride();
    }
}

/// Pass 2 (after pass 1 over the whole size): a candidate is kept when it holds its fingerprint's lowest ordinal; its
/// index goes to `kept` (`nkept` counts) and its vector to `kvec` at the same position.
#[no_mangle]
pub unsafe extern "ptx-kernel" fn p5_keep(f: Filt, out: *const u32, m: u64, vt: VTable, cand: *const u8,
                                          kept: *mut u32, kvec: *mut u32, nkept: *const AtomicU32) {
    let mut c = tid();
    while c < m {
        if *cand.add(c as usize) != 0 {
            let v = out.add((c * f.n) as usize);
            let (h0, h1) = vhash(v, f.n);
            let tag = if f.last != 0 { f.ltag } else { f.role };
            let h = vfind(&vt, h0 ^ tag.wrapping_mul(0x9E37_79B9_7F4A_7C15), h1);
            if h != u64::MAX && (*vt.ord.add(h as usize)).load(Ordering::Acquire) == f.ord0 + c {
                let i = (*nkept).fetch_add(1, Ordering::AcqRel) as u64;
                *kept.add(i as usize) = c as u32;
                let mut k = 0u64;
                while k < f.n { *kvec.add((i * f.n + k) as usize) = *v.add(k as usize); k += 1; }
            }
        }
        c += stride();
    }
}

/// B rows from .. to (their roles in `roles`, one per row) into the B set.
#[no_mangle]
pub unsafe extern "ptx-kernel" fn p5_bseen_add(bvec: *const u32, n: u64, from: u64, to: u64, roles: *const u32, bseen: VTable,
                                               flag: *const AtomicU32) {
    let mut r = from + tid();
    while r < to {
        let (h0, h1) = vhash(bvec.add((r * n) as usize), n);
        let role = *roles.add((r - from) as usize) as u64;
        let x = vput(&bseen, h0 ^ role.wrapping_mul(0x9E37_79B9_7F4A_7C15), h1, r);
        if x == u64::MAX { (*flag).fetch_or(2, Ordering::AcqRel); }
        if x == RETRY { (*flag).fetch_or(16, Ordering::AcqRel); }
        r += stride();
    }
}

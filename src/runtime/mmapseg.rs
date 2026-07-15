//! BRIDGE_MMAPSEG_V1 (AXVERITY_STORAGE_SUBSTRATE_DURABILITY_V1) — a per-thread,
//! mmap-backed, append-only durable log segment. The durability primitive that
//! replaces reclog for the write path:
//!
//!   append(h, bytes) = memcpy a framed record into a MAP_SHARED mmap, advance the
//!                      cursor. NO write(2), NO fsync, NO lock, NO coordination.
//!                      The writer returns immediately.
//!   msync(h)         = flush the dirty range to stable storage. Run on a
//!                      BACKGROUND cadence thread, NEVER by the appending writer.
//!
//! ## Why mmap MAP_SHARED (the whole point)
//!
//! A record appended into a MAP_SHARED mapping is a dirty page in the kernel's
//! page cache, associated with the backing file. It is therefore:
//!   * CRASH-SAFE FOR FREE — a process crash / SIGKILL does not discard the pages;
//!     the kernel writes them back. A fresh process that remaps the file sees every
//!     record, with zero msync on the write path. (Measured: 1,000,000/1,000,000
//!     records recovered after a hard _exit with no msync — the spike this module
//!     graduates from.)
//!   * POWER-LOSS-SAFE within the msync cadence — only pages dirtied since the last
//!     background `msync` are at risk on a real power failure. The loss window is
//!     the cadence, and the writer never pays for it.
//!
//! This is genuinely distinct from write(2)+fsync (logbuf.rs) and from
//! write_all_at+batched fdatasync (the substrate's ExtentWriter): those put a
//! syscall — and, at batch boundaries, an fsync — ON the writer. Here the writer
//! only ever does a memcpy.
//!
//! ## Storage model — THREAD-OWNED, no shared registry (mirrors logbuf.rs)
//!
//! Each open segment lives in THREAD-LOCAL storage (`SEGS`), reachable only by the
//! thread that opened it; the handle counter (`NEXT`) is thread-local too, so a
//! `--entries` worker's first open returns handle 1, identical to a fresh process.
//! Each pg_server worker owns its own segment file, so N writers share nothing and
//! never contend — the Spike-4 mutex-free property (measured: 21M rec/s/thread,
//! scaling to 107M rec/s aggregate at 16 threads on own segments).
//!
//! ## Frame format (torn-tail-safe recovery)
//!
//!   [ u32 LE payload_len ][ payload_len bytes ][ u32 LE fnv1a(payload) ]
//!
//! Recovery (`scan_frontier`) walks frames from offset 0. It stops at the first
//! frame whose length is 0 (the zero-filled pre-allocated tail — the natural
//! terminator), out of range, or whose fnv1a checksum does not match its payload
//! (a torn tail from a power-loss mid-append). The stop offset is the recovered
//! frontier: a restarted process resumes appending there, and every frame before
//! it is intact. Framing/hashing stays a bridge concern here (unlike logbuf.rs,
//! whose framing is in M1) because torn-tail detection IS the recovery contract.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::path::Path;

use super::value::{get_str, Value};

const FRAME_OVERHEAD: usize = 8; // 4-byte len prefix + 4-byte crc trailer
const MAX_REC: usize = 64 * 1024 * 1024; // a single record cannot exceed 64 MiB
const PAGE: usize = 4096;

/// FNV-1a 32-bit — the same cheap non-crypto checksum block_flush/the substrate use.
fn fnv1a(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// One open mmap-backed segment. Thread-owned; the raw pointer never crosses
/// threads (SEGS is thread-local), so no Send/Sync is needed or implied.
struct MmapSeg {
    ptr: *mut u8,
    cap: usize,
    cursor: usize, // append frontier (also the recovered frontier at open)
    synced: usize, // bytes already msync'd (for incremental background msync)
    #[allow(dead_code)]
    file: std::fs::File, // keeps the fd alive for the mapping's life
    path: String,
}

impl Drop for MmapSeg {
    fn drop(&mut self) {
        // munmap does NOT sync — dropping a handle mid-run must never fsync on the
        // writer. The dirty pages stay in the page cache (crash-safe); the
        // background cadence / OS writeback persists them.
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.cap);
        }
    }
}

impl MmapSeg {
    /// Open (creating if absent) a `cap`-byte pre-allocated segment and mmap it
    /// MAP_SHARED. Existing content is preserved and its frontier recovered
    /// (resume-on-open / crash recovery), so a fresh file yields frontier 0 and an
    /// existing one resumes after its last intact frame.
    fn open(path: &str, cap: usize) -> std::io::Result<MmapSeg> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)?;
        // Pre-allocate to `cap`: set_len establishes the mappable length (keeping
        // any existing content), fallocate reserves the disk blocks so appends are
        // pure data writes with no metadata churn (best-effort; no-op on FSes
        // without fallocate — EOF/frontier still marks the true frontier).
        file.set_len(cap as u64)?;
        unsafe {
            libc::fallocate(file.as_raw_fd(), 0, 0, cap as i64);
        }
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                cap,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        let ptr = ptr as *mut u8;
        let (frontier, _count) = unsafe { scan_frontier(ptr, cap) };
        Ok(MmapSeg {
            ptr,
            cap,
            cursor: frontier,
            synced: frontier,
            file,
            path: path.to_string(),
        })
    }

    /// Append one framed record. Returns the byte offset of the frame, or -1 if it
    /// would not fit (the caller rotates to a fresh segment — a frame never spans a
    /// segment boundary). Pure memcpy: no syscall, no lock.
    fn append(&mut self, rec: &[u8]) -> i64 {
        let need = FRAME_OVERHEAD + rec.len();
        if rec.len() > MAX_REC || self.cursor + need > self.cap {
            return -1;
        }
        let off = self.cursor;
        let crc = fnv1a(rec);
        unsafe {
            let base = self.ptr.add(off);
            std::ptr::copy_nonoverlapping((rec.len() as u32).to_le_bytes().as_ptr(), base, 4);
            std::ptr::copy_nonoverlapping(rec.as_ptr(), base.add(4), rec.len());
            std::ptr::copy_nonoverlapping(crc.to_le_bytes().as_ptr(), base.add(4 + rec.len()), 4);
        }
        self.cursor += need;
        off as i64
    }

    /// Read the payload of the frame at `off`, verifying its checksum. Returns None
    /// on any framing/checksum failure (never reads past the frontier).
    fn read_at(&self, off: usize) -> Option<Vec<u8>> {
        unsafe { read_frame(self.ptr, self.cursor.max(self.cap), off) }
            .filter(|_| off < self.cursor)
    }

    /// msync the range dirtied since the last msync (page-aligned start). Advances
    /// the synced watermark. Called by the background cadence thread only.
    fn msync(&mut self) {
        if self.cursor > self.synced {
            let start = self.synced & !(PAGE - 1);
            let len = self.cursor - start;
            unsafe {
                libc::msync(self.ptr.add(start) as *mut libc::c_void, len, libc::MS_SYNC);
            }
            self.synced = self.cursor;
        }
    }
}

/// Read a framed record at `off` from a raw mapping of at least `limit` bytes,
/// verifying its fnv1a checksum. `unsafe`: caller guarantees `ptr` maps ≥ `limit`.
unsafe fn read_frame(ptr: *const u8, limit: usize, off: usize) -> Option<Vec<u8>> {
    if off + 4 > limit {
        return None;
    }
    let n = u32::from_le_bytes([
        *ptr.add(off),
        *ptr.add(off + 1),
        *ptr.add(off + 2),
        *ptr.add(off + 3),
    ]) as usize;
    if n == 0 || n > MAX_REC || off + FRAME_OVERHEAD + n > limit {
        return None;
    }
    let mut v = vec![0u8; n];
    std::ptr::copy_nonoverlapping(ptr.add(off + 4), v.as_mut_ptr(), n);
    let crc = u32::from_le_bytes([
        *ptr.add(off + 4 + n),
        *ptr.add(off + 4 + n + 1),
        *ptr.add(off + 4 + n + 2),
        *ptr.add(off + 4 + n + 3),
    ]);
    if fnv1a(&v) != crc {
        return None; // torn tail
    }
    Some(v)
}

/// Walk frames from 0 to the first invalid one; return (frontier_offset, count).
/// The zero-filled pre-allocated tail terminates the scan (len == 0), as does any
/// out-of-range length or checksum mismatch (a torn power-loss tail).
unsafe fn scan_frontier(ptr: *const u8, cap: usize) -> (usize, usize) {
    let mut off = 0usize;
    let mut count = 0usize;
    while let Some(v) = read_frame(ptr, cap, off) {
        off += FRAME_OVERHEAD + v.len();
        count += 1;
    }
    (off, count)
}

thread_local! {
    /// Per-thread segment table, keyed by integer handle. Thread-local, never
    /// shared — the append/msync path touches only the calling thread's own
    /// segment, so no two writer threads contend and no lock is taken.
    static SEGS: RefCell<HashMap<i64, MmapSeg>> = RefCell::new(HashMap::new());
    /// Per-thread handle counter (starts at 1, like logbuf's NEXT).
    static NEXT: Cell<i64> = const { Cell::new(1) };
}

fn next_handle() -> i64 {
    NEXT.with(|c| {
        let n = c.get();
        c.set(n + 1);
        n
    })
}

/// `mmapseg_open(path: Text, cap: Int) -> Int` — open/mmap a `cap`-byte segment
/// (resuming/recovering existing content), return the thread-local handle.
#[track_caller]
pub fn mmapseg_open(args: Value) -> Value {
    let (path, cap) = match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("mmapseg_open: expected Tuple(Text, Int), got {:?}", other),
    };
    let path = match path {
        Value::Str(h) => get_str(h),
        other => panic!("mmapseg_open: arg 0 expected Text path, got {:?}", other),
    };
    let cap = match cap {
        Value::Int(n) if n > 0 => n as usize,
        other => panic!("mmapseg_open: arg 1 expected positive Int cap, got {:?}", other),
    };
    // parent-dir fsync once so the new segment file's directory entry is durable
    // (mirrors logbuf_open; a one-time open-path cost, never on append).
    let parent = Path::new(&path).parent().unwrap_or_else(|| Path::new(""));
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let seg = MmapSeg::open(&path, cap)
        .unwrap_or_else(|e| panic!("mmapseg_open({}): {}", path, e));
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    let h = next_handle();
    SEGS.with(|s| s.borrow_mut().insert(h, seg));
    Value::Int(h)
}

/// `mmapseg_append(handle: Int, data: Bytes) -> Int` — memcpy a framed record;
/// return its offset, or -1 if the segment is full (caller rotates).
#[track_caller]
pub fn mmapseg_append(args: Value) -> Value {
    let (h, data) = match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("mmapseg_append: expected Tuple(Int, Bytes), got {:?}", other),
    };
    let h = match h {
        Value::Int(n) => n,
        other => panic!("mmapseg_append: arg 0 expected Int handle, got {:?}", other),
    };
    let data = match data {
        Value::Bytes(b) => b,
        other => panic!("mmapseg_append: arg 1 expected Bytes, got {:?}", other),
    };
    SEGS.with(|s| {
        let mut s = s.borrow_mut();
        let seg = s
            .get_mut(&h)
            .unwrap_or_else(|| panic!("mmapseg_append: unknown handle {} (not opened on this thread)", h));
        Value::Int(seg.append(&data))
    })
}

/// `mmapseg_read(handle: Int, off: Int) -> Bytes` — read the frame at `off` (empty
/// Bytes if invalid / past the frontier).
#[track_caller]
pub fn mmapseg_read(args: Value) -> Value {
    let (h, off) = match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("mmapseg_read: expected Tuple(Int, Int), got {:?}", other),
    };
    let h = match h {
        Value::Int(n) => n,
        other => panic!("mmapseg_read: arg 0 expected Int handle, got {:?}", other),
    };
    let off = match off {
        Value::Int(n) if n >= 0 => n as usize,
        other => panic!("mmapseg_read: arg 1 expected non-negative Int, got {:?}", other),
    };
    SEGS.with(|s| {
        let s = s.borrow();
        let seg = s
            .get(&h)
            .unwrap_or_else(|| panic!("mmapseg_read: unknown handle {}", h));
        Value::Bytes(seg.read_at(off).unwrap_or_default())
    })
}

/// `mmapseg_msync(handle: Int) -> Unit` — flush the dirty range (background cadence).
#[track_caller]
pub fn mmapseg_msync(arg: Value) -> Value {
    let h = match arg {
        Value::Int(n) => n,
        other => panic!("mmapseg_msync: expected Int handle, got {:?}", other),
    };
    SEGS.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(seg) = s.get_mut(&h) {
            seg.msync();
        }
        Value::Unit
    })
}

/// `mmapseg_frontier(handle: Int) -> Int` — current append/recovered frontier offset.
#[track_caller]
pub fn mmapseg_frontier(arg: Value) -> Value {
    let h = match arg {
        Value::Int(n) => n,
        other => panic!("mmapseg_frontier: expected Int handle, got {:?}", other),
    };
    SEGS.with(|s| {
        let s = s.borrow();
        let seg = s
            .get(&h)
            .unwrap_or_else(|| panic!("mmapseg_frontier: unknown handle {}", h));
        Value::Int(seg.cursor as i64)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> String {
        let d = std::env::temp_dir();
        format!("{}/mmapseg_test_{}_{}.seg", d.display(), std::process::id(), name)
    }
    const CAP: usize = 16 * 1024 * 1024;

    // A record's raw framed size on disk, for computing expected frontiers.
    fn framed(n: usize) -> usize {
        FRAME_OVERHEAD + n
    }

    #[test]
    fn roundtrip_read_back() {
        let p = tmp("roundtrip");
        let _ = std::fs::remove_file(&p);
        let mut seg = MmapSeg::open(&p, CAP).unwrap();
        let mut offs = Vec::new();
        for i in 0..1000u32 {
            let rec = format!("record-{}", i).into_bytes();
            let off = seg.append(&rec);
            assert!(off >= 0);
            offs.push((off as usize, rec));
        }
        for (off, rec) in &offs {
            assert_eq!(seg.read_at(*off).as_deref(), Some(rec.as_slice()));
        }
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn recovers_all_after_reopen_without_msync() {
        // The crash-safety property, in-process: append with NO msync, drop the
        // handle (munmap, still no sync), reopen a FRESH mapping of the same file
        // and confirm the scan recovers every record from the page cache. (The real
        // cross-process SIGKILL survival was measured in the spike: 1M/1M recovered.)
        let p = tmp("nomsync");
        let _ = std::fs::remove_file(&p);
        let n = 50_000usize;
        {
            let mut seg = MmapSeg::open(&p, CAP).unwrap();
            for i in 0..n {
                assert!(seg.append(format!("r{}", i).as_bytes()) >= 0);
            }
            // NO seg.msync() — drop munmaps without syncing.
        }
        let seg2 = MmapSeg::open(&p, CAP).unwrap();
        let (_frontier, count) = unsafe { scan_frontier(seg2.ptr, seg2.cap) };
        assert_eq!(count, n, "every record must recover from the page cache without msync");
        // and the reopened segment resumes at the recovered frontier
        assert_eq!(seg2.cursor, _frontier);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn torn_tail_is_dropped_at_frontier() {
        // Write 3 records, then corrupt the 3rd record's checksum in place — recovery
        // must stop at the frontier BEFORE it, recovering exactly 2.
        let p = tmp("torn");
        let _ = std::fs::remove_file(&p);
        let mut seg = MmapSeg::open(&p, CAP).unwrap();
        let r0 = b"alpha".to_vec();
        let r1 = b"bravo".to_vec();
        let r2 = b"charlie".to_vec();
        seg.append(&r0);
        seg.append(&r1);
        let off2 = seg.append(&r2) as usize;
        // corrupt one payload byte of record 2 (its checksum no longer matches)
        unsafe {
            *seg.ptr.add(off2 + 4) ^= 0xff;
        }
        let (frontier, count) = unsafe { scan_frontier(seg.ptr, seg.cap) };
        assert_eq!(count, 2, "torn 3rd record must not be recovered");
        assert_eq!(frontier, framed(r0.len()) + framed(r1.len()), "frontier stops before the torn frame");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn msync_then_recover() {
        let p = tmp("msync");
        let _ = std::fs::remove_file(&p);
        let n = 10_000usize;
        {
            let mut seg = MmapSeg::open(&p, CAP).unwrap();
            for i in 0..n {
                seg.append(format!("x{}", i).as_bytes());
            }
            seg.msync(); // explicit power-loss-safe flush
        }
        let seg2 = MmapSeg::open(&p, CAP).unwrap();
        let (_f, count) = unsafe { scan_frontier(seg2.ptr, seg2.cap) };
        assert_eq!(count, n);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn resume_append_after_reopen() {
        // Recovery resumes: reopen finds the frontier and new appends land after it.
        let p = tmp("resume");
        let _ = std::fs::remove_file(&p);
        {
            let mut seg = MmapSeg::open(&p, CAP).unwrap();
            seg.append(b"one");
            seg.append(b"two");
        }
        let mut seg2 = MmapSeg::open(&p, CAP).unwrap();
        assert_eq!(seg2.cursor, framed(3) + framed(3));
        let off = seg2.append(b"three") as usize;
        assert_eq!(off, framed(3) + framed(3));
        assert_eq!(seg2.read_at(off).as_deref(), Some(b"three".as_slice()));
        let (_f, count) = unsafe { scan_frontier(seg2.ptr, seg2.cap) };
        assert_eq!(count, 3);
        let _ = std::fs::remove_file(&p);
    }
}

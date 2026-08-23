//! pwrite_bench — AXVERITY_EXTENT_WRITE_PATH step 3.
//!
//! Measures the durable-write cost of the axVerity-working2 flusher's access
//! pattern: N fixed-size blocks, ejected one at a time in strict increasing
//! sequence order, each of which must be durable before the next.
//!
//! It calls the REAL bridge primitives, not a re-implementation, so the numbers
//! describe the code that would actually ship:
//!
//!   baseline   bytes_io::fs_write_raw  — one NEW file per block through
//!              write_durable: temp write + fsync(tmp) + rename + fsync(parent
//!              dir). This is what lib/wp_flusher_step.m1 does today.
//!   none|fdatasync|fsync
//!              pwrite::fs_pwrite_raw   — blocks written at offsets into ONE
//!              pre-allocated extent (fs_prealloc + one fs_sync_dir), with the
//!              named sync_mode.
//!
//! What this does NOT measure: the flusher itself. At step 3 the write path
//! still uses fs_write_raw; wiring it to the extent path is step 5. This
//! isolates the primitive under the flusher's access pattern so the sync_mode
//! default can be chosen before that wiring, not after.
//!
//! Usage: pwrite_bench <baseline|none|fdatasync|fsync> <block_bytes> <nblocks> <dir>

use axis_codegen_bridge::runtime::{bytes_io, prealloc, pwrite};
use std::sync::Arc;
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 5 {
        eprintln!("usage: pwrite_bench <baseline|none|fdatasync|fsync> <block_bytes> <nblocks> <dir>");
        std::process::exit(2);
    }
    let mode = a[1].as_str();
    let block: usize = a[2].parse().expect("block_bytes");
    let nblk: usize = a[3].parse().expect("nblocks");
    let dir = a[4].clone();

    std::fs::create_dir_all(&dir).expect("create_dir_all");

    // One arena block, refilled per iteration so the source is a live buffer
    // rather than a page the kernel has already seen — matching the hotframe
    // pool, where the block being ejected was just written by the packer.
    let mut arena = vec![0u8; block];

    let t0 = Instant::now();
    match mode {
        "baseline" => {
            for i in 0..nblk {
                arena[0] = i as u8;
                let p: Arc<str> = Arc::from(format!("{}/block-{}.bin", dir, i).as_str());
                bytes_io::fs_write_raw(p, arena.as_ptr() as i64, 0, block as i64);
            }
        }
        _ => {
            let full = mode.ends_with("-full");
            let sync_mode: i64 = match mode.trim_end_matches("-full") {
                "none" => 0,
                "fdatasync" => 1,
                "fsync" => 2,
                other => {
                    eprintln!("unknown mode {}", other);
                    std::process::exit(2);
                }
            };
            let seg: Arc<str> = Arc::from(format!("{}/seg-0.bin", dir).as_str());
            // `-full` variants set the file's LOGICAL length to the whole
            // segment up front (ftruncate), instead of fs_prealloc's
            // FALLOC_FL_KEEP_SIZE which reserves blocks but leaves st_size at
            // bytes-actually-written.
            //
            // This is the difference that decides whether fdatasync can beat
            // fsync at all. Under KEEP_SIZE every block written past the
            // previous end EXTENDS st_size — an inode metadata change — so
            // fdatasync must journal it too and collapses onto fsync. Under
            // ftruncate-to-full the size never changes after setup, so
            // fdatasync has only data to flush.
            //
            // Landing A chose KEEP_SIZE deliberately, to keep EOF marking the
            // true data frontier for recovery. Measuring both is how that
            // choice gets priced instead of assumed.
            if full {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .open(&*seg)
                    .expect("open for set_len")
                    .set_len((block * nblk) as u64)
                    .expect("set_len");
            }
            // Once per segment, not once per block — this is the cost the
            // extent design moves out of the per-block path, so it is paid
            // here, inside the timed region, rather than quietly excluded.
            prealloc::fs_prealloc(seg.clone(), (block * nblk) as i64);
            pwrite::fs_sync_dir(Arc::from(dir.as_str()));
            for i in 0..nblk {
                arena[0] = i as u8;
                pwrite::fs_pwrite_raw(
                    seg.clone(),
                    arena.as_ptr() as i64,
                    0,
                    block as i64,
                    (i * block) as i64,
                    sync_mode,
                );
            }
        }
    }
    let el = t0.elapsed();

    let bytes = (block * nblk) as f64;
    let ms_per_block = el.as_secs_f64() * 1000.0 / nblk as f64;
    let mb_s = bytes / (1024.0 * 1024.0) / el.as_secs_f64();
    println!(
        "mode={} block={} nblocks={} wall_ms={:.1} ms_per_block={:.4} mb_s={:.1}",
        mode,
        block,
        nblk,
        el.as_secs_f64() * 1000.0,
        ms_per_block,
        mb_s
    );
}

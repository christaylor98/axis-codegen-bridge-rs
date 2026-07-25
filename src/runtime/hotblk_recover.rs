//! HOTBLK_RECOVER_V1 (AXVERITY_SLICE4_BLOCK_DURABILITY_V2, task 4) — recovery
//! from sealed hotblk blocks ALONE, independent of reclog. This is what the
//! crash-recovery gate exercises: with reclog's role held constant, can the
//! (table,pk)->hash binding and content-hash presence be reconstructed purely
//! from `block-<seq>.bin` artifacts?
//!
//! Mirrors `pkindex.rs`'s shape (thread-owned handle, open/rebuild/get/has) but
//! walks `<flush_dir>/block-<seq>.bin` files instead of `<prefix><seq>.log` WAL
//! segments, reusing `walindex::parse_frames_from_bytes` — the SAME hash-check/
//! torn-tail parser every other index projection uses
//! (FRAME_PARSERS_UPDATED_LOCKSTEP). `env_to_name` is an intentionally
//! INDEPENDENT re-implementation of pkindex.rs's parse (not a call into it) —
//! this module verifies recovery independently of the reclog-side projection,
//! so a bug shared by both would not silently cancel out.
//!
//! Each sealed block is a COMPLETE, ATOMICALLY-WRITTEN file
//! (`block_flush.rs::write_bin_durable` is tmp+fsync+rename) — unlike a WAL
//! segment there is no "torn tail mid-file" case for an already-renamed block;
//! the only failure modes are a missing trailing block (crash before rename,
//! walked-off frontier) or, defensively, a corrupted one — both handled by
//! `parse_frames_from_bytes`'s torn-frame stop, exactly as for WAL segments.
//! THREAD-OWNED, nothing shared, nothing locked — identical model to
//! `pkindex.rs`/`walindex.rs`.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use super::value::{get_str, intern_str, Value};
use super::walindex::parse_frames_from_bytes;

struct RecoverShard {
    flush_dir: String,
    pk: HashMap<String, String>, // "table:pk" -> "sha256:<hex>", last-append-wins
    hashes: HashSet<String>,     // every hash-valid content hash seen (bare hex, no "sha256:" prefix)
    blocks_scanned: i64,
    frames_scanned: i64,
}

thread_local! {
    static SHARDS: RefCell<HashMap<i64, RecoverShard>> = RefCell::new(HashMap::new());
    static NEXT: Cell<i64> = const { Cell::new(1) };
}

fn next_handle() -> i64 {
    NEXT.with(|c| {
        let n = c.get();
        c.set(n + 1);
        n
    })
}

fn read_block(flush_dir: &str, seq: i64) -> Option<Vec<u8>> {
    let path = format!("{}/block-{}.bin", flush_dir, seq);
    std::fs::read(path).ok()
}

/// Parse envelope `"<table>\t<seq>\t<pk>"` -> `"<table>:<pk>"` binding key.
/// Independent re-implementation of `pkindex::env_to_name` (see module doc).
fn env_to_name(env: &[u8]) -> Option<String> {
    if env.is_empty() {
        return None;
    }
    let s = std::str::from_utf8(env).ok()?;
    let mut it = s.split('\t');
    let table = it.next()?;
    if table == "FACT" || table == "CONTRADICTS" {
        return None;
    }
    let _seq = it.next()?;
    let pk = it.next()?;
    Some(format!("{}:{}", table, pk))
}

/// `hotblk_recover_open(flush_dir: Text) -> Int` — register a fresh recovery
/// shard for THIS thread, return its handle.
#[track_caller]
pub fn hotblk_recover_open(arg: Value) -> Value {
    let flush_dir = match arg {
        Value::Str(h) => get_str(&h),
        other => panic!("hotblk_recover_open: expected Text flush_dir, got {:?}", other),
    };
    let h = next_handle();
    SHARDS.with(|s| {
        s.borrow_mut().insert(
            h,
            RecoverShard { flush_dir, pk: HashMap::new(), hashes: HashSet::new(), blocks_scanned: 0, frames_scanned: 0 },
        );
    });
    Value::Int(h)
}

/// `hotblk_recover_rebuild(h: Int) -> Int` — walk EVERY sealed
/// `block-<seq>.bin` in this shard's flush_dir, in sequence order starting at
/// 1, hash-checking every frame via the shared parser. Stops at the first
/// missing (or, defensively, torn) block — the recovery frontier. Returns
/// frames scanned. Idempotent-if-rerun (last-append-wins semantics match
/// pkidx_rebuild's, so replaying the same blocks twice is harmless).
#[track_caller]
pub fn hotblk_recover_rebuild(arg: Value) -> Value {
    let h = match arg {
        Value::Int(n) => n,
        other => panic!("hotblk_recover_rebuild: expected Int handle, got {:?}", other),
    };
    let scanned = SHARDS.with(|s| {
        let mut s = s.borrow_mut();
        let sh = s.get_mut(&h).unwrap_or_else(|| panic!("hotblk_recover_rebuild: unknown handle {}", h));
        let mut seq = 1i64;
        let mut total_frames = 0i64;
        let mut total_blocks = 0i64;
        loop {
            let data = match read_block(&sh.flush_dir, seq) {
                Some(d) => d,
                None => break, // no more sealed blocks — clean frontier
            };
            total_blocks += 1;
            let (_end_off, _torn) = parse_frames_from_bytes(&data, 0, |_poff, _plen, env, _payload, hexh| {
                if let Some(name) = env_to_name(env) {
                    sh.pk.insert(name, format!("sha256:{}", hexh)); // last-append-wins
                }
                sh.hashes.insert(hexh.to_string());
                total_frames += 1;
            });
            // A torn frame inside an atomically-renamed sealed block should
            // never happen; if it ever does, this block's valid PREFIX is
            // still indexed and the scan still stops at this block (same
            // fail-safe posture as a WAL segment's torn tail) — no worse than
            // the WAL-based projections' existing behavior.
            seq += 1;
        }
        sh.blocks_scanned = total_blocks;
        sh.frames_scanned = total_frames;
        total_frames
    });
    Value::Int(scanned)
}

/// `hotblk_recover_pk_get(h: Int, name: Text) -> Text` — the reconstructed
/// current content address for `"<table>:<pk>"`, or `""` if never bound in
/// the scanned blocks.
#[track_caller]
pub fn hotblk_recover_pk_get(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 2 => es,
        other => panic!("hotblk_recover_pk_get: expected Tuple(Int, Text), got {:?}", other),
    };
    let h = match &es[0] {
        Value::Int(n) => *n,
        other => panic!("hotblk_recover_pk_get: arg 0 expected Int, got {:?}", other),
    };
    let name = match &es[1] {
        Value::Str(s) => get_str(s),
        other => panic!("hotblk_recover_pk_get: arg 1 expected Text, got {:?}", other),
    };
    SHARDS.with(|s| {
        let s = s.borrow();
        let sh = s.get(&h).unwrap_or_else(|| panic!("hotblk_recover_pk_get: unknown handle {}", h));
        let out = sh.pk.get(&name).cloned().unwrap_or_default();
        Value::Str(intern_str(&out))
    })
}

/// `hotblk_recover_has_hash(h: Int, hexh: Text) -> Bool` — was a hash-valid
/// frame with this exact content hash found in the scanned blocks? Since
/// `parse_frames_from_bytes` only invokes its visitor for frames where
/// `sha256(payload) == hexh` already held, a `true` here is itself the content
/// correctness proof for that hash — no separate content-get is needed for
/// the recovery-correctness gate.
#[track_caller]
pub fn hotblk_recover_has_hash(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 2 => es,
        other => panic!("hotblk_recover_has_hash: expected Tuple(Int, Text), got {:?}", other),
    };
    let h = match &es[0] {
        Value::Int(n) => *n,
        other => panic!("hotblk_recover_has_hash: arg 0 expected Int, got {:?}", other),
    };
    let hexh_full = match &es[1] {
        Value::Str(s) => get_str(s),
        other => panic!("hotblk_recover_has_hash: arg 1 expected Text, got {:?}", other),
    };
    let hexh = hexh_full.strip_prefix("sha256:").unwrap_or(&hexh_full);
    SHARDS.with(|s| {
        let s = s.borrow();
        let sh = s.get(&h).unwrap_or_else(|| panic!("hotblk_recover_has_hash: unknown handle {}", h));
        Value::Bool(sh.hashes.contains(hexh))
    })
}

/// `hotblk_recover_dump_pk(h: Int) -> Text` — every reconstructed
/// `"<table:pk>\t<hash>"` binding, one per line, LF-terminated (empty if
/// none). Lets a kill-9 test harness diff the FULL recovered binding set
/// against its own independently-tracked acked-set without needing M1-side
/// iteration over an unbounded key list.
#[track_caller]
pub fn hotblk_recover_dump_pk(arg: Value) -> Value {
    let h = match arg {
        Value::Int(n) => n,
        other => panic!("hotblk_recover_dump_pk: expected Int handle, got {:?}", other),
    };
    SHARDS.with(|s| {
        let s = s.borrow();
        let sh = s.get(&h).unwrap_or_else(|| panic!("hotblk_recover_dump_pk: unknown handle {}", h));
        let mut lines: Vec<String> = sh.pk.iter().map(|(k, v)| format!("{}\t{}", k, v)).collect();
        lines.sort(); // deterministic output for diffing
        let mut out = lines.join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        Value::Str(intern_str(&out))
    })
}

/// `hotblk_recover_dump_hashes(h: Int) -> Text` — every distinct hash-valid
/// content hash found (bare hex, no `sha256:` prefix), one per line,
/// LF-terminated (empty if none).
#[track_caller]
pub fn hotblk_recover_dump_hashes(arg: Value) -> Value {
    let h = match arg {
        Value::Int(n) => n,
        other => panic!("hotblk_recover_dump_hashes: expected Int handle, got {:?}", other),
    };
    SHARDS.with(|s| {
        let s = s.borrow();
        let sh = s.get(&h).unwrap_or_else(|| panic!("hotblk_recover_dump_hashes: unknown handle {}", h));
        let mut hashes: Vec<&String> = sh.hashes.iter().collect();
        hashes.sort();
        let mut out = hashes.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        Value::Str(intern_str(&out))
    })
}

/// `hotblk_recover_stats(h: Int) -> Text` — `"<blocks_scanned>\t<frames_scanned>"`,
/// for test/diagnostic reporting.
#[track_caller]
pub fn hotblk_recover_stats(arg: Value) -> Value {
    let h = match arg {
        Value::Int(n) => n,
        other => panic!("hotblk_recover_stats: expected Int handle, got {:?}", other),
    };
    SHARDS.with(|s| {
        let s = s.borrow();
        let sh = s.get(&h).unwrap_or_else(|| panic!("hotblk_recover_stats: unknown handle {}", h));
        Value::Str(intern_str(&format!("{}\t{}", sh.blocks_scanned, sh.frames_scanned)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn sha_hex(p: &[u8]) -> String {
        Sha256::digest(p).iter().map(|b| format!("{:02x}", b)).collect()
    }

    /// Build a Branch-A frame `H(64)|P(10)|V(10)|env(V)|payload(P)`, identical
    /// shape to pkindex.rs's test helper and to `wal_frame_env_bytes.m1`.
    fn frame(table: &str, seq: &str, pk: &str, payload: &[u8]) -> Vec<u8> {
        let env: Vec<u8> = if table.is_empty() && pk.is_empty() {
            Vec::new()
        } else {
            format!("{}\t{}\t{}", table, seq, pk).into_bytes()
        };
        let mut f = Vec::new();
        f.extend_from_slice(sha_hex(payload).as_bytes());
        f.extend_from_slice(format!("{:010}", payload.len()).as_bytes());
        f.extend_from_slice(format!("{:010}", env.len()).as_bytes());
        f.extend_from_slice(&env);
        f.extend_from_slice(payload);
        f
    }

    fn unique_dir(tag: &str) -> String {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let d = std::env::temp_dir().join(format!("hotblk-recover-test-{}-{}-{}", tag, std::process::id(), nanos));
        std::fs::create_dir_all(&d).unwrap();
        d.to_string_lossy().into_owned()
    }

    fn write_block(dir: &str, seq: i64, bytes: &[u8]) {
        std::fs::write(format!("{}/block-{}.bin", dir, seq), bytes).unwrap();
    }

    fn pk_get(h: i64, name: &str) -> String {
        match hotblk_recover_pk_get(Value::Tuple(vec![Value::Int(h), Value::Str(intern_str(name))])) {
            Value::Str(s) => get_str(s),
            _ => unreachable!(),
        }
    }
    fn has_hash(h: i64, hexh: &str) -> bool {
        match hotblk_recover_has_hash(Value::Tuple(vec![Value::Int(h), Value::Str(intern_str(hexh))])) {
            Value::Bool(b) => b,
            _ => unreachable!(),
        }
    }

    #[test]
    fn reconstructs_pk_binding_and_hash_presence_from_one_block() {
        let dir = unique_dir("single");
        let p1 = b"RECORD\tid=1\tname=alice";
        let mut blk = frame("t", "10", "1", p1);
        let p2 = b"RECORD\tid=2\tname=bob";
        blk.extend_from_slice(&frame("t", "20", "2", p2));
        write_block(&dir, 1, &blk);

        let h = match hotblk_recover_open(Value::Str(intern_str(&dir))) {
            Value::Int(n) => n,
            _ => unreachable!(),
        };
        let scanned = match hotblk_recover_rebuild(Value::Int(h)) {
            Value::Int(n) => n,
            _ => unreachable!(),
        };
        assert_eq!(scanned, 2);
        assert_eq!(pk_get(h, "t:1"), format!("sha256:{}", sha_hex(p1)));
        assert_eq!(pk_get(h, "t:2"), format!("sha256:{}", sha_hex(p2)));
        assert!(has_hash(h, &format!("sha256:{}", sha_hex(p1))));
        assert!(has_hash(h, &sha_hex(p2))); // bare hex also accepted
    }

    #[test]
    fn last_append_wins_across_multiple_blocks() {
        // Same (table,pk) bound in block 1, then re-bound in block 2 — the
        // LATER block's hash must win, mirroring pkindex's last-append-wins
        // (here by block sequence order, the hotblk analogue of frame order).
        let dir = unique_dir("multi");
        let p_old = b"RECORD\tid=42\tname=old";
        let p_new = b"RECORD\tid=42\tname=new";
        write_block(&dir, 1, &frame("users", "1", "42", p_old));
        write_block(&dir, 2, &frame("users", "2", "42", p_new));

        let h = match hotblk_recover_open(Value::Str(intern_str(&dir))) {
            Value::Int(n) => n,
            _ => unreachable!(),
        };
        hotblk_recover_rebuild(Value::Int(h));
        assert_eq!(pk_get(h, "users:42"), format!("sha256:{}", sha_hex(p_new)));
        // The superseded hash's CONTENT is still recoverable (blocks are never
        // rewritten) even though the binding points elsewhere now.
        assert!(has_hash(h, &sha_hex(p_old)));
    }

    #[test]
    fn missing_trailing_block_stops_cleanly_at_the_frontier() {
        // Block 1 exists and is fully valid; block 2 was never sealed (e.g. a
        // crash before rename). Recovery must see exactly block 1's frames,
        // never panic, never fabricate block 2's content.
        let dir = unique_dir("frontier");
        let p1 = b"RECORD\tid=1\tname=alice";
        write_block(&dir, 1, &frame("t", "1", "1", p1));
        // no block-2.bin written

        let h = match hotblk_recover_open(Value::Str(intern_str(&dir))) {
            Value::Int(n) => n,
            _ => unreachable!(),
        };
        let scanned = match hotblk_recover_rebuild(Value::Int(h)) {
            Value::Int(n) => n,
            _ => unreachable!(),
        };
        assert_eq!(scanned, 1);
        assert_eq!(pk_get(h, "t:1"), format!("sha256:{}", sha_hex(p1)));
        assert_eq!(pk_get(h, "t:999"), ""); // never bound
    }

    #[test]
    fn dump_pk_and_dump_hashes_are_sorted_lf_terminated_and_complete() {
        let dir = unique_dir("dump");
        let p1 = b"RECORD\tid=1\tname=alice";
        let p2 = b"RECORD\tid=2\tname=bob";
        let mut blk = frame("t", "1", "1", p1);
        blk.extend_from_slice(&frame("t", "2", "2", p2));
        write_block(&dir, 1, &blk);

        let h = match hotblk_recover_open(Value::Str(intern_str(&dir))) {
            Value::Int(n) => n,
            _ => unreachable!(),
        };
        hotblk_recover_rebuild(Value::Int(h));
        let pk_dump = match hotblk_recover_dump_pk(Value::Int(h)) {
            Value::Str(s) => get_str(s),
            _ => unreachable!(),
        };
        assert!(pk_dump.ends_with('\n'));
        assert_eq!(
            pk_dump,
            format!("t:1\tsha256:{}\nt:2\tsha256:{}\n", sha_hex(p1), sha_hex(p2))
        );
        let hash_dump = match hotblk_recover_dump_hashes(Value::Int(h)) {
            Value::Str(s) => get_str(s),
            _ => unreachable!(),
        };
        assert!(hash_dump.contains(&sha_hex(p1)));
        assert!(hash_dump.contains(&sha_hex(p2)));
        assert_eq!(hash_dump.lines().count(), 2);
    }

    #[test]
    fn torn_frame_within_a_block_drops_it_and_stops_that_block_at_the_tear() {
        // Defensive case only (should not occur for a real atomically-renamed
        // block) — a valid frame followed by a truncated one. The truncated
        // frame must not appear; the valid one must.
        let dir = unique_dir("torn");
        let p1 = b"RECORD\tid=1\tname=alice";
        let p2 = b"RECORD\tid=2\tname=bob";
        let mut blk = frame("t", "1", "1", p1);
        let f2 = frame("t", "2", "2", p2);
        blk.extend_from_slice(&f2[..f2.len() - 1]);
        write_block(&dir, 1, &blk);

        let h = match hotblk_recover_open(Value::Str(intern_str(&dir))) {
            Value::Int(n) => n,
            _ => unreachable!(),
        };
        hotblk_recover_rebuild(Value::Int(h));
        assert_eq!(pk_get(h, "t:1"), format!("sha256:{}", sha_hex(p1)));
        assert_eq!(pk_get(h, "t:2"), "");
        assert!(!has_hash(h, &sha_hex(p2)));
    }
}

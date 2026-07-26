//! BLOCKFILE_V1 (AXVERITY_BREAK_111_BINDING_V1, Phase 1) — the completeness
//! discipline for `block-<seq>.bin`, and the one place that knows it.
//!
//! ## Why this module has to exist
//!
//! Today a sealed block becomes visible ATOMICALLY: `block_flush.rs`'s
//! `write_bin_durable` writes a `.tmp`, fsyncs it, renames it into place, then
//! fsyncs the parent directory. The rename is what makes "the file exists" and
//! "the file is complete" the same statement — so every reader can treat a
//! present `block-<seq>.bin` as whole, and `walk_hotblk_frames` /
//! `hotblk_recover` can use "first missing file" as the frontier.
//!
//! That atomicity costs the parent-directory fsync, which is HALF of axVerity's
//! measured 2.000 fsyncs per committed row (AXVERITY_PER_SHARD_GROUP_COMMIT_V1
//! §1, strace-validated). It is paid only because every block is a NEW file, so
//! a new directory entry has to be made durable before the ack. PostgreSQL does
//! not pay it per commit: it preallocates and recycles WAL segments, so a commit
//! appends into a file whose directory entry is already durable.
//!
//! Preallocating `block-<seq>.bin` gets the same saving, but it BREAKS the
//! identity above — a pre-created file exists while empty, and a file being
//! written exists while partial. Two failure modes follow, and the second is a
//! live-path bug, not merely a recovery one:
//!
//!   1. A scanner reaching a pre-created empty file would parse zero frames and,
//!      seeing no error, walk on to the next seq — advancing its persistent
//!      frontier (`walidx`'s `sh.hb_seq = next_seq`) PAST a block that has not
//!      been written yet. When it is written, that scanner never re-reads it and
//!      its rows are permanently invisible to SELECT.
//!   2. A scanner observing a block MID-WRITE can see a prefix that happens to
//!      end on a frame boundary. It parses cleanly, looks complete, and the
//!      frontier advances past a block that is about to grow. Same silent loss,
//!      and no crash is required — an ordinary concurrent SELECT is enough.
//!
//! So completeness stops being inferable from presence and has to be RECORDED.
//! This module records it as a fixed trailer written in the SAME `write_all` as
//! the body, costing no extra fsync:
//!
//! ```text
//!   <body: the framed records, byte-identical to today> | MAGIC(8) | len(16 ASCII digits)
//! ```
//!
//! `split_complete` is the single reader-side rule. Everything that reads a
//! block goes through it, so the writer and the three readers (walindex.rs,
//! hotblk_recover.rs, indexer.rs) cannot drift — the same
//! FRAME_PARSERS_UPDATED_LOCKSTEP argument that made `parse_frames_from_bytes`
//! the one frame parser.
//!
//! ## Torn-tail interaction (the case a trailer alone does not cover)
//!
//! A crash can persist the page holding the trailer while losing a page in the
//! middle of the body — the file then LOOKS complete but has a hole. The frames
//! are individually sha256-checked, so `parse_frames_from_bytes` reports `torn`
//! for that block. Callers must therefore require BOTH a valid trailer AND a
//! clean full-body parse before advancing a frontier past a block;
//! `body_is_intact` is the helper for that second half. Fail-safe: an
//! intact-looking trailer over a torn body stops the scan rather than skipping
//! the block.
//!
//! ## Mode
//!
//! `AXVERITY_BLOCK_PREALLOC` (env, default `off`) selects which writer path runs.
//! DEFAULT OFF — this build changes no default; the dial is reported, not flipped.
//!
//!   off  → today's tmp+fsync+rename+dir-fsync, NO trailer, 2 fsyncs per block.
//!   on   → pre-created file, write body+trailer in place, fsync. 1 fsync per
//!          block, plus one amortised directory fsync per preallocation batch.
//!
//! Readers accept BOTH shapes: a file carrying a valid trailer is complete by the
//! trailer, and one without is complete by rename atomicity (the legacy case).
//! The one thing that would be ambiguous — a PARTIAL file written in prealloc
//! mode, which has no trailer and is therefore indistinguishable from a legacy
//! whole file — is resolved by consulting the mode, because a store is written by
//! one server configuration at a time. **Do not switch this dial on an existing
//! store**: a partial block left by a prealloc-mode crash would be read as a
//! complete legacy block if the store were then opened with the dial off.

use std::sync::OnceLock;

/// Trailer magic. 8 bytes, ends in `\n` so a hexdump of a block file shows the
/// boundary plainly.
pub(crate) const BLOCK_MAGIC: &[u8; 8] = b"AXVBLK1\n";
/// Body length, ASCII decimal, zero-padded, fixed width.
pub(crate) const BLOCK_LEN_DIGITS: usize = 16;
/// Total trailer width.
pub(crate) const BLOCK_TRAILER_LEN: usize = 8 + BLOCK_LEN_DIGITS;

/// `AXVERITY_BLOCK_PREALLOC` — `on`/`1`/`true` enables the preallocated-file
/// write path. Default OFF. Read once per process (OnceLock), the same pattern as
/// `slice4.rs`/`walshard.rs`.
pub(crate) fn prealloc_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| match std::env::var("AXVERITY_BLOCK_PREALLOC") {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            v == "on" || v == "1" || v == "true"
        }
        Err(_) => false,
    })
}

/// How many block files to create per preallocation batch — the directory fsync
/// is paid ONCE per batch, so this is the amortisation factor: fsyncs per block
/// tend to `1 + 1/BATCH`. `AXVERITY_BLOCK_PREALLOC_BATCH`, default 64.
pub(crate) fn prealloc_batch() -> i64 {
    static N: OnceLock<i64> = OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("AXVERITY_BLOCK_PREALLOC_BATCH")
            .ok()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .filter(|&n| n >= 1)
            .unwrap_or(64)
    })
}

/// Build the trailer for a body of `body_len` bytes.
pub(crate) fn trailer_for(body_len: usize) -> Vec<u8> {
    let mut t = Vec::with_capacity(BLOCK_TRAILER_LEN);
    t.extend_from_slice(BLOCK_MAGIC);
    t.extend_from_slice(format!("{:0width$}", body_len, width = BLOCK_LEN_DIGITS).as_bytes());
    t
}

/// The reader-side completeness rule. Returns the block BODY if `data` is a
/// complete block, or `None` if it is a pre-created (empty) or partially written
/// file — in which case the caller is at its frontier and MUST NOT advance past
/// this seq.
///
/// A file bearing a valid trailer is complete regardless of mode. A file without
/// one is complete only in legacy (non-prealloc) mode, where rename atomicity
/// guarantees it; in prealloc mode an absent trailer means "not finished yet".
pub(crate) fn split_complete(data: &[u8]) -> Option<&[u8]> {
    if data.len() >= BLOCK_TRAILER_LEN {
        let split = data.len() - BLOCK_TRAILER_LEN;
        let (body, trailer) = data.split_at(split);
        if &trailer[..8] == BLOCK_MAGIC {
            if let Ok(s) = std::str::from_utf8(&trailer[8..]) {
                if let Ok(n) = s.trim().parse::<usize>() {
                    // The recorded length must agree with where the trailer
                    // actually starts, so a coincidental magic mid-body cannot
                    // pass.
                    if n == body.len() {
                        return Some(body);
                    }
                }
            }
        }
    }
    if prealloc_enabled() {
        // Pre-created (0 bytes) or mid-write: not complete, and crucially not to
        // be confused with a legacy whole file.
        None
    } else {
        // Legacy: the file was renamed into place, so its presence IS its
        // completeness — including the degenerate zero-byte block, which
        // `indexer.rs` has always indexed as the empty sha. Byte-identical to
        // pre-Phase-1 behaviour, which is what keeps the dial's OFF arm a true
        // control.
        Some(data)
    }
}

/// Second half of the advance rule: a block may only be stepped PAST if its body
/// also parsed cleanly to the end. A crash can persist the trailer's page while
/// losing an interior one, leaving a complete-looking file with a hole that the
/// per-frame sha256 check reports as torn.
pub(crate) fn body_is_intact(body_len: usize, end_off: usize, torn: bool) -> bool {
    !torn && end_off == body_len
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(body: &[u8]) -> Vec<u8> {
        let mut v = body.to_vec();
        v.extend_from_slice(&trailer_for(body.len()));
        v
    }

    #[test]
    fn complete_block_yields_its_body() {
        let body = b"some framed records".to_vec();
        let file = framed(&body);
        assert_eq!(split_complete(&file), Some(&body[..]));
    }

    #[test]
    fn empty_file_completeness_follows_the_mode() {
        // The pre-created case. In prealloc mode an empty file is NOT a block, so
        // a frontier can never step past a block that has yet to be written. In
        // legacy mode presence is completeness (rename atomicity), and a
        // zero-byte block has always indexed as the empty sha — that arm must
        // stay byte-identical or the dial's OFF side is not a control.
        if prealloc_enabled() {
            assert_eq!(split_complete(&[]), None);
        } else {
            assert_eq!(split_complete(&[]), Some(&[][..]));
        }
    }

    #[test]
    fn truncated_block_has_no_valid_trailer() {
        let body = b"aaaabbbbccccdddd".to_vec();
        let file = framed(&body);
        // Every proper prefix must fail the trailer check.
        for cut in 1..file.len() {
            let part = &file[..cut];
            if let Some(b) = split_complete(part) {
                // Only acceptable in legacy mode, where a present file is whole
                // by rename atomicity; the trailer must never validate.
                assert_eq!(b, part, "a truncated file must not parse as a trailered block");
            }
        }
    }

    #[test]
    fn length_must_match_where_the_trailer_starts() {
        let body = b"payload".to_vec();
        let mut file = body.clone();
        file.extend_from_slice(BLOCK_MAGIC);
        // Wrong recorded length — a coincidental magic must not pass.
        file.extend_from_slice(format!("{:016}", body.len() + 5).as_bytes());
        assert!(split_complete(&file).map(|b| b.len()) != Some(body.len()));
    }

    #[test]
    fn intactness_requires_clean_full_body_parse() {
        assert!(body_is_intact(100, 100, false));
        assert!(!body_is_intact(100, 100, true), "torn body must not advance");
        assert!(!body_is_intact(100, 60, false), "short parse must not advance");
    }

    #[test]
    fn trailer_roundtrips_its_length() {
        for n in [0usize, 1, 84, 4 * 1024 * 1024] {
            let t = trailer_for(n);
            assert_eq!(t.len(), BLOCK_TRAILER_LEN);
            assert_eq!(&t[..8], BLOCK_MAGIC);
            assert_eq!(std::str::from_utf8(&t[8..]).unwrap().parse::<usize>().unwrap(), n);
        }
    }
}

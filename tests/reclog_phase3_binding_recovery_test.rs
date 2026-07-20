//! AXVERITY_RECLOG_PHASE3 — empirical basis for the phase-3 name-stream decision.
//!
//! Two characterization experiments over the EXISTING, UNMODIFIED write/replay
//! path (`content_hash`, `pkidx_open/rebuild/get`, the Branch-A enveloped frame
//! format). No production `.rs` is touched; no fsync behavior is changed. Same
//! test-harness class as the Candidate-A tsmark harness. `#[ignore]` so the
//! default `cargo test` never runs them; run with `--ignored`.
//!
//! The two questions these convert from inference to fact:
//!   E1. Does an INSERT's `(table,pk) -> hash` binding survive crash-recovery via
//!       the WAL-envelope replay ALONE, with the per-name `.log` absent?
//!       (If yes, a name-log durability barrier protects no unique datum for
//!       bindings — the ack-gating decision is forced by data.)
//!   E2. Can a DELETE's tombstone survive that same WAL-envelope replay?
//!       (If no, Candidate Z — dropping the `.log` write — is provably blocked
//!       until tombstones are carried in the envelope stream.)
//!
//! Frames are built to the exact Branch-A layout
//! `H(64 hex) | P(10 dec payload-len) | V(10 dec env-len) | env(V) | payload(P)`
//! (lib/wal_frame_env_bytes.m1 / walindex.rs), with H computed by the bridge's
//! own `content_hash` so the scanner's hash-check (`sha256_hex(payload)==H`,
//! walindex.rs) accepts them exactly as a real INSERT frame.

use axis_codegen_bridge::runtime::hash::content_hash;
use axis_codegen_bridge::runtime::pkindex::{pkidx_get, pkidx_open, pkidx_rebuild};
use axis_codegen_bridge::runtime::value::{get_str, intern_str, Value};

use std::sync::atomic::{AtomicU64, Ordering};

static SEG_CTR: AtomicU64 = AtomicU64::new(0);

/// Bridge-computed sha256 hex (64 chars, no prefix) of `payload`.
fn sha_hex(payload: &[u8]) -> String {
    let list = Value::List(payload.iter().map(|&b| Value::Int(b as i64)).collect());
    let full = match content_hash(list) {
        Value::Str(s) => get_str(s),
        other => panic!("content_hash returned {:?}", other),
    };
    full.strip_prefix("sha256:").unwrap().to_string()
}

/// One Branch-A frame. `env` empty ⇒ V=0 (a plain blob, no binding).
fn frame(env: &str, payload: &[u8]) -> Vec<u8> {
    let envb = env.as_bytes();
    let mut f = Vec::new();
    f.extend_from_slice(sha_hex(payload).as_bytes()); // H (64)
    f.extend_from_slice(format!("{:010}", payload.len()).as_bytes()); // P (10)
    f.extend_from_slice(format!("{:010}", envb.len()).as_bytes()); // V (10)
    f.extend_from_slice(envb); // env (V)
    f.extend_from_slice(payload); // payload (P)
    f
}

/// Write `frames` concatenated as segment 0 under a unique temp prefix; return
/// the prefix `pkidx_rebuild` expects (`<prefix><seq>.log`). Nothing else is
/// created — critically, NO `.log` name-file is written anywhere.
fn write_wal_only(frames: &[Vec<u8>]) -> String {
    let n = SEG_CTR.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("reclog-p3-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).unwrap();
    let prefix = format!("{}/wal-", dir.to_str().unwrap());
    let mut seg = Vec::new();
    for f in frames {
        seg.extend_from_slice(f);
    }
    std::fs::write(format!("{}0.log", prefix), &seg).unwrap();
    prefix
}

fn rebuild_and_get(prefix: &str, name: &str) -> (i64, String) {
    let h = match pkidx_open(Value::Str(intern_str("0"))) {
        Value::Int(h) => h,
        other => panic!("pkidx_open returned {:?}", other),
    };
    let scanned = match pkidx_rebuild(Value::Tuple(vec![
        Value::Int(h),
        Value::Str(intern_str(prefix)),
    ])) {
        Value::Int(n) => n,
        other => panic!("pkidx_rebuild returned {:?}", other),
    };
    let addr = match pkidx_get(Value::Tuple(vec![
        Value::Int(h),
        Value::Str(intern_str(name)),
    ])) {
        Value::Str(s) => get_str(s),
        other => panic!("pkidx_get returned {:?}", other),
    };
    (scanned, addr)
}

/// E1 — the INSERT binding is fully recoverable from the WAL envelope with NO
/// `.log` present. Faithful INSERT frame: env = "orders\t<seq>\tA-1", payload =
/// the RECORD bytes; segment written, no name-log written.
#[test]
#[ignore = "phase-3 characterization; run with --ignored"]
fn e1_insert_binding_survives_wal_replay_without_namelog() {
    let payload = b"RECORD\tpk=A-1\tqty=5";
    let want = format!("sha256:{}", sha_hex(payload));
    let f = frame("orders\t1700000000000000000\tA-1", payload);
    let prefix = write_wal_only(&[f]);

    let (scanned, got) = rebuild_and_get(&prefix, "orders:A-1");

    assert_eq!(scanned, 1, "exactly one committed frame replayed");
    assert_eq!(
        got, want,
        "binding orders:A-1 must resolve to the payload hash from the WAL alone"
    );
    eprintln!("E1 PASS: orders:A-1 -> {} recovered from WAL envelope, no .log", got);
}

/// E2 — a DELETE's tombstone CANNOT be expressed in the WAL-envelope stream, so
/// it cannot survive WAL replay.
///
/// Faithful model of the real path: a real DELETE (lib/pg_delete_apply.m1 ->
/// tombstone -> bind_record) appends a TOMBSTONE line to the per-name `.log`
/// and writes NO WAL frame. So from the WAL's view a deleted row is
/// indistinguishable from a live one. We demonstrate the structural gap two
/// ways in one run:
///   (a) after an INSERT, with the `.log` (where the tombstone would live)
///       absent, WAL replay still reports the row BOUND — a deleted row looks
///       live;
///   (b) there is NO envelope value that encodes "deleted": `pkidx_rebuild`'s
///       replay only ever `map.insert`s (pkindex.rs `env_to_name` +
///       rebuild has no delete branch), so appending a would-be "tombstone
///       envelope" frame for the same pk just rebinds it to that frame's
///       payload hash — a live binding to different bytes, never a removal.
#[test]
#[ignore = "phase-3 characterization; run with --ignored"]
fn e2_tombstone_cannot_survive_wal_replay() {
    // (a) INSERT then "logical DELETE" (which, in the real path, only touches
    //     .log). WAL has just the insert frame; .log is absent.
    let ins = b"RECORD\tpk=B-2\tqty=9";
    let ins_hash = format!("sha256:{}", sha_hex(ins));
    let f_ins = frame("orders\t1700000000000000001\tB-2", ins);
    let prefix_a = write_wal_only(&[f_ins.clone()]);
    let (_s, got_a) = rebuild_and_get(&prefix_a, "orders:B-2");
    assert_eq!(
        got_a, ins_hash,
        "with .log absent, the deleted row still resolves live from the WAL — \
         the tombstone lives only in .log and is invisible to WAL replay"
    );

    // (b) Try to encode a tombstone AS a second envelope frame for the same pk.
    //     There is no delete kind in the envelope grammar; whatever we put, the
    //     rebuild produces a LIVE binding (last-append-wins), never "deleted".
    let tomb_payload = b"TOMBSTONE"; // a naive "delete marker" object
    let tomb_hash = format!("sha256:{}", sha_hex(tomb_payload));
    let f_tomb = frame("orders\t1700000000000000002\tB-2", tomb_payload);
    let prefix_b = write_wal_only(&[f_ins, f_tomb]);
    let (scanned_b, got_b) = rebuild_and_get(&prefix_b, "orders:B-2");
    assert_eq!(scanned_b, 2, "both frames committed");
    assert_eq!(
        got_b, tomb_hash,
        "a would-be tombstone envelope just rebinds the pk to its own payload \
         hash — the pk-index has no delete semantics, so 'deleted' is unreachable"
    );
    assert_ne!(
        got_b, "",
        "there is NO WAL-replay outcome that reports orders:B-2 as deleted"
    );
    eprintln!(
        "E2 PASS: tombstone is unrepresentable in WAL replay \
         (deleted row resolves live: {} then {})",
        ins_hash, tomb_hash
    );
}

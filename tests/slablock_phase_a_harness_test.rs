//! AXVERITY_RECLOG_SLA_BLOCK_BUILD_PHASE_A_V1 — Phase A validation harness.
//!
//! Same class as the Candidate A harness (reclog_tsmark_wiring_audit_test.rs):
//! a cargo integration test OUTSIDE runtime/, all I/O under a throwaway
//! `std::env::temp_dir()` subtree, NEVER the real `.axverity/`, driving the
//! module through its PUBLIC bridge API only.
//!
//! Unlike Candidate A, the governing intent here is (phase implementation,
//! execution allowed, CRITERIA_VALIDATED_BY_HARNESS_NOT_ASSUMED) — this
//! harness is MEANT to run. The `#[ignore]` gate exists only to keep a
//! ~1-minute timed measurement out of default `cargo test`; run it explicitly:
//!
//!   cargo test --release --test slablock_phase_a_harness_test -- --ignored --nocapture
//!
//! ## Isolation note (OLD_PATH_UNTOUCHED)
//!
//! The per-name-fsync-loop baseline (~1 fsync/row today, E1/E2: cardinality ≈
//! batch size) is EMULATED here (append+fsync per row on a scratch file) —
//! this harness deliberately does NOT call `reclog_submit`/`reclog_flush_once`,
//! so the excluded path is never touched, not even read-only. The emulation is
//! mechanism-faithful: the baseline's per-row cost IS one data fsync, which is
//! exactly what the emulation performs and times (it also yields this rig's
//! physical fsync-latency floor, needed for the tier floor-bounding check).
//!
//! ## What each design criterion maps to below
//!
//!   C1 positional-until-seal / hash-at-seal  -> unit tests in slablock.rs
//!      (offset returns, blk-<seq>.bin naming, bytes_hash cross-check) + the
//!      sealed-ledger checks here.
//!   C2 no throughput cost                    -> appends never fsync: mean
//!      append latency measured in-sweep must be orders below the fsync floor
//!      (the extra partial-block fsyncs ride the tick, off the append path).
//!      Full end-to-end (26-idle-cores) absorption remains untested-prediction
//!      until a cutover intent wires a real load — reported, not overclaimed.
//!   C3 strictly fewer fsyncs than baseline   -> measured fsync count vs
//!      rows-per-run at every load > 1 row/tick, every tier.
//!   C4 tombstone prerequisite                -> out of scope (excluded by the
//!      governing intent); nothing here removes or claims it.
//!   C5 single tick sweeps all dirty blocks   -> full-block seal observed only
//!      at the sweep (never at append), partial + full flushed by one tick.
//!   C6 no fragmentation                      -> file count per run == blocks
//!      the data minimally needs (partial flushes grow blk-0 in place).
//!   C7 floor degenerates to same mechanism   -> 0.1ms tier (below the fsync
//!      floor) produces only blk-*.bin files, fsyncs == sweeps, no auxiliary
//!      structure — block-of-1(ish), one durability primitive.

use std::io::Write;
use std::time::{Duration, Instant};

use axis_codegen_bridge::runtime::slablock::{
    slab_append, slab_open, slab_seal, slab_sealed, slab_stats, slab_tick,
};
use axis_codegen_bridge::runtime::value::{get_str, intern_str, Value};

/// SLA tiers from the design intent's example set.
const TIERS: &[(&str, i64)] = &[
    ("0.1ms", 100),
    ("5ms", 5_000),
    ("100ms", 100_000),
    ("2000ms", 2_000_000),
];

/// Rows arriving per SLA window — the same 1..256 range as the tsmark audit's
/// cardinality sweep (Candidate A CARDINALITIES).
const LOADS: &[usize] = &[1, 2, 4, 8, 16, 32, 64, 128, 256];

/// Synthetic row: ~120 bytes, the small-INSERT shape.
fn synth_row(i: usize) -> Vec<u8> {
    format!(
        "ROW\t{:08}\tcol_a=value-{:08}\tcol_b=payload-{:032}\tcol_c={:016x}\n",
        i, i, i, i
    )
    .into_bytes()
}

fn scratch_root() -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = std::env::temp_dir().join(format!("axv-slab-phase-a-{}-{}", std::process::id(), nanos));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn open(dir: &str, sla_us: i64) -> i64 {
    match slab_open(Value::Tuple(vec![
        Value::Str(intern_str(dir)),
        Value::Int(sla_us),
        Value::Int(0),
    ])) {
        Value::Int(h) => h,
        other => panic!("slab_open returned {:?}", other),
    }
}

fn stat_field(h: i64, key: &str) -> i64 {
    let s = match slab_stats(Value::Int(h)) {
        Value::Str(s) => get_str(&s),
        other => panic!("slab_stats returned {:?}", other),
    };
    s.split('\t')
        .find_map(|kv| kv.strip_prefix(&format!("{}=", key)))
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("stat {} missing in {}", key, s))
}

/// The per-row-fsync baseline, emulated: append one row + sync_all, per row.
/// Returns per-row fsync latencies. This is the mechanism the design replaces
/// (1 fsync/row) and doubles as this rig's physical fsync floor.
fn baseline_per_row_fsync(root: &std::path::Path, rows: usize) -> Vec<Duration> {
    let path = root.join("baseline-per-row.log");
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .unwrap();
    let mut lats = Vec::with_capacity(rows);
    for i in 0..rows {
        let row = synth_row(i);
        let t0 = Instant::now();
        f.write_all(&row).unwrap();
        f.sync_all().unwrap();
        lats.push(t0.elapsed());
    }
    lats
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

struct ComboResult {
    tier: &'static str,
    sla_us: i64,
    load: usize,
    rows: usize,
    data_fsyncs: i64,
    sweeps: i64,
    sealed: i64,
    block_files: usize,
    baseline_fsyncs: usize,
    mean_append_us: f64,
    mean_durable_ms: f64,
    max_durable_ms: f64,
}

/// Drive one (tier, load) combo: `windows` SLA windows, each appending `load`
/// rows then polling the tick until the sweep fires; per-row durability
/// latency = sweep-complete time − append time.
fn run_combo(root: &std::path::Path, tier: &'static str, sla_us: i64, load: usize) -> ComboResult {
    let dir = root.join(format!("slab-{}-{}", tier, load));
    let dir_s = dir.to_string_lossy().into_owned();
    let h = open(&dir_s, sla_us);

    let windows: usize = std::cmp::max(3, std::cmp::min(8, (1_500_000 / sla_us) as usize));
    let poll = Duration::from_micros(std::cmp::min(std::cmp::max(sla_us / 10, 20), 2_000) as u64);

    let mut rows = 0usize;
    let mut append_lat = Vec::new();
    let mut durable_lat: Vec<Duration> = Vec::new();

    for w in 0..windows {
        let mut pending: Vec<Instant> = Vec::with_capacity(load);
        for i in 0..load {
            let row = synth_row(w * load + i);
            let t0 = Instant::now();
            let off = slab_append(Value::Tuple(vec![Value::Int(h), Value::Bytes(row)]));
            append_lat.push(t0.elapsed());
            match off {
                Value::Int(_) => {}
                other => panic!("slab_append returned {:?}", other),
            }
            pending.push(t0);
            rows += 1;
        }
        // Poll the tick until the sweep fires (gate makes extra calls free).
        loop {
            match slab_tick(Value::Int(h)) {
                Value::Int(-1) => std::thread::sleep(poll),
                Value::Int(_) => break,
                other => panic!("slab_tick returned {:?}", other),
            }
        }
        let durable_at = Instant::now();
        for t in pending {
            durable_lat.push(durable_at.duration_since(t));
        }
    }

    // End-of-stream: explicit seal (final fsync + hash) so the ledger and the
    // on-disk block set are complete for the checks below.
    match slab_seal(Value::Int(h)) {
        Value::Str(_) => {}
        other => panic!("slab_seal returned {:?}", other),
    }

    let sealed_ledger = match slab_sealed(Value::Int(h)) {
        Value::Str(s) => get_str(&s),
        other => panic!("slab_sealed returned {:?}", other),
    };
    let sealed = if sealed_ledger.is_empty() { 0 } else { sealed_ledger.lines().count() as i64 };
    assert!(sealed >= 1, "a run that appended rows must seal at least one block");
    for line in sealed_ledger.lines() {
        let hash = line.split('\t').nth(1).unwrap_or("");
        assert!(hash.starts_with("sha256:") && hash.len() == 71, "malformed seal hash {:?}", line);
    }

    let block_files = std::fs::read_dir(&dir)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("blk-")
        })
        .count();
    // C6/C7: nothing but blk-<seq>.bin may exist — no auxiliary structure.
    let all_files = std::fs::read_dir(&dir).unwrap().count();
    assert_eq!(
        all_files, block_files,
        "only blk-*.bin allowed in a slab dir (single durability mechanism)"
    );

    let mean_us =
        append_lat.iter().map(|d| d.as_secs_f64() * 1e6).sum::<f64>() / append_lat.len() as f64;
    let mean_ms =
        durable_lat.iter().map(|d| d.as_secs_f64() * 1e3).sum::<f64>() / durable_lat.len() as f64;
    let max_ms = durable_lat
        .iter()
        .map(|d| d.as_secs_f64() * 1e3)
        .fold(0.0f64, f64::max);

    ComboResult {
        tier,
        sla_us,
        load,
        rows,
        data_fsyncs: stat_field(h, "data_fsyncs"),
        sweeps: stat_field(h, "sweeps"),
        sealed,
        block_files,
        baseline_fsyncs: rows, // per-name loop: 1 fsync/row (E1/E2)
        mean_append_us: mean_us,
        mean_durable_ms: mean_ms,
        max_durable_ms: max_ms,
    }
}

#[test]
#[ignore = "timed ~1-minute measurement sweep; run explicitly with -- --ignored --nocapture (execution authorized by AXVERITY_RECLOG_SLA_BLOCK_BUILD_PHASE_A_V1)"]
fn phase_a_sla_tier_sweep() {
    let root = scratch_root();
    assert!(
        !root.to_string_lossy().contains(".axverity"),
        "harness must never touch .axverity/"
    );

    // ── Physical fsync floor + per-row baseline mechanism, on this rig ──
    let base = baseline_per_row_fsync(&root, 64);
    let floor = median(base);
    let floor_ms = floor.as_secs_f64() * 1e3;
    eprintln!(
        "fsync floor (median of 64 per-row append+fsync): {:.3}ms — the baseline costs this PER ROW",
        floor_ms
    );

    let mut results = Vec::new();
    for &(tier, sla_us) in TIERS {
        for &load in LOADS {
            results.push(run_combo(&root, tier, sla_us, load));
        }
    }

    // ── Report table ──
    eprintln!(
        "\n{:>7} {:>5} {:>5} {:>7} {:>9} {:>7} {:>7} {:>6} {:>11} {:>12} {:>11}",
        "tier", "load", "rows", "fsyncs", "baseline", "sweeps", "sealed", "files",
        "append(us)", "durable(ms)", "max(ms)"
    );
    let mut tsv = String::from(
        "tier\tsla_us\tload\trows\tdata_fsyncs\tbaseline_fsyncs\tsweeps\tsealed\tblock_files\tmean_append_us\tmean_durable_ms\tmax_durable_ms\n",
    );
    for r in &results {
        eprintln!(
            "{:>7} {:>5} {:>5} {:>7} {:>9} {:>7} {:>7} {:>6} {:>11.2} {:>12.3} {:>11.3}",
            r.tier, r.load, r.rows, r.data_fsyncs, r.baseline_fsyncs, r.sweeps, r.sealed,
            r.block_files, r.mean_append_us, r.mean_durable_ms, r.max_durable_ms
        );
        tsv.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.2}\t{:.3}\t{:.3}\n",
            r.tier, r.sla_us, r.load, r.rows, r.data_fsyncs, r.baseline_fsyncs, r.sweeps,
            r.sealed, r.block_files, r.mean_append_us, r.mean_durable_ms, r.max_durable_ms
        ));
    }
    let tsv_path = root.join("phase-a-results.tsv");
    std::fs::write(&tsv_path, tsv).unwrap();
    eprintln!("\nresults TSV: {}", tsv_path.display());
    eprintln!("scratch root (inspect then rm -rf): {}", root.display());

    // ── Criterion assertions ──
    for r in &results {
        // C3: strictly fewer fsyncs than the per-row baseline wherever more
        // than one row arrives per tick; parity(+seal) allowed at load 1.
        if r.load > 1 {
            assert!(
                (r.data_fsyncs as usize) < r.baseline_fsyncs,
                "[{} load {}] {} fsyncs not < baseline {}",
                r.tier, r.load, r.data_fsyncs, r.baseline_fsyncs
            );
        } else {
            assert!(
                (r.data_fsyncs as usize) <= r.baseline_fsyncs + 1,
                "[{} load 1] degenerate case must not exceed baseline+seal: {} vs {}",
                r.tier, r.data_fsyncs, r.baseline_fsyncs
            );
        }
        // At most 1 data fsync per sweep here (all data fits one 4MiB block,
        // +1 for the explicit end seal).
        assert!(
            r.data_fsyncs <= r.sweeps + 1,
            "[{} load {}] fsyncs {} exceed sweeps {} + seal",
            r.tier, r.load, r.data_fsyncs, r.sweeps
        );
        // C6: no fragmentation — total data (windows*load*~120B) < 4MiB, so
        // exactly ONE block object, grown in place across partial flushes.
        assert_eq!(
            r.block_files, 1,
            "[{} load {}] partial flushes must not scatter blocks",
            r.tier, r.load
        );
        assert_eq!(r.sealed, 1, "[{} load {}] one block -> one seal", r.tier, r.load);
        // C2 (append path): appends never fsync — mean append latency must sit
        // far below the fsync floor (fsyncs ride the tick, not the append).
        assert!(
            r.mean_append_us / 1e3 < floor_ms / 4.0,
            "[{} load {}] append path looks fsync-bound: {:.1}us vs floor {:.3}ms",
            r.tier, r.load, r.mean_append_us, floor_ms
        );
        // SLA semantics: durability latency is bounded by roughly one SLA
        // window + the physical fsync cost (generous 3x slack for scheduler
        // noise; the 0.1ms tier is floor-bounded by design, so its bound is
        // the floor, not the SLA).
        let sla_ms = r.sla_us as f64 / 1e3;
        let bound_ms = sla_ms + (floor_ms * 3.0).max(3.0) + sla_ms.max(floor_ms) * 2.0;
        assert!(
            r.max_durable_ms <= bound_ms,
            "[{} load {}] max durability latency {:.3}ms breaches tier bound {:.3}ms",
            r.tier, r.load, r.max_durable_ms, bound_ms
        );
    }

    // C7 at the floor tier: same single mechanism — fsync cadence == sweep
    // cadence, no auxiliary files (already asserted per-combo above).
    for r in results.iter().filter(|r| r.tier == "0.1ms") {
        assert!(
            r.data_fsyncs <= r.sweeps + 1,
            "floor tier must remain 1 fsync per sweep (block-of-N degenerate), no side structure"
        );
    }

    eprintln!("\nPHASE A SWEEP: all criterion assertions passed");
}

/// Shared-nothing under real parallelism: P threads, each its own slab +
/// tick loop, no cross-thread state to contend on (mirrors the P-sharded
/// topology). Quick enough to leave un-ignored? Kept behind the same gate to
/// keep default `cargo test` timing-free.
#[test]
#[ignore = "timed measurement; run with -- --ignored --nocapture (authorized by AXVERITY_RECLOG_SLA_BLOCK_BUILD_PHASE_A_V1)"]
fn phase_a_parallel_shared_nothing() {
    let root = scratch_root();
    let threads = 4;
    let rows_per_thread = 2_000usize;
    let mut joins = Vec::new();
    for t in 0..threads {
        let dir = root.join(format!("par-{}", t)).to_string_lossy().into_owned();
        joins.push(std::thread::spawn(move || {
            let h = open(&dir, 5_000);
            let t0 = Instant::now();
            for i in 0..rows_per_thread {
                slab_append(Value::Tuple(vec![
                    Value::Int(h),
                    Value::Bytes(synth_row(i)),
                ]));
                if i % 64 == 0 {
                    slab_tick(Value::Int(h));
                }
            }
            // final flush + seal
            loop {
                match slab_tick(Value::Int(h)) {
                    Value::Int(-1) => std::thread::sleep(Duration::from_micros(200)),
                    _ => break,
                }
            }
            slab_seal(Value::Int(h));
            let secs = t0.elapsed().as_secs_f64();
            (stat_field(h, "data_fsyncs"), rows_per_thread as f64 / secs)
        }));
    }
    let mut total_rows_s = 0.0;
    for j in joins {
        let (fsyncs, rows_s) = j.join().unwrap();
        assert!(fsyncs >= 1);
        assert!((fsyncs as usize) < rows_per_thread, "ticked fsyncs must undercut 1/row");
        total_rows_s += rows_s;
    }
    eprintln!(
        "parallel shared-nothing: {} threads, aggregate ~{:.0} rows/s, per-thread fsyncs << rows (no cross-thread stalls possible: zero shared state)",
        threads, total_rows_s
    );
}

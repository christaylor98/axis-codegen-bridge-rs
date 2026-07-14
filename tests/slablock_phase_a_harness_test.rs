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

fn open(dir: &str, sla_us: i64, block_bytes: i64) -> i64 {
    match slab_open(Value::Tuple(vec![
        Value::Str(intern_str(dir)),
        Value::Int(sla_us),
        Value::Int(block_bytes),
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
    let h = open(&dir_s, sla_us, 0);

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
            let h = open(&dir, 5_000, 0);
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

/// AXVERITY_SLABLOCK_CAP_TIER_CONCURRENCY_SWEEP_V1 — extends the tier sweep
/// (single-thread, cap fixed at default) and the shared-nothing parallel test
/// (single tier/cap, fixed 4 threads) above into one 3-dimensional matrix: SLA
/// tier x block-size cap x thread count. Same shared-nothing topology as
/// `phase_a_parallel_shared_nothing` (thread-local slab per thread, no
/// cross-thread coordination introduced) — reuses `open()`/`stat_field()`/
/// `synth_row()`; no slablock.rs mechanism change (the cap sweep only varies
/// `slab_open`'s existing third argument, already load-bearing before this
/// test existed).
///
/// TOTAL_ROWS is held FIXED across the concurrency dimension and divided
/// across threads (`rows_per_thread = TOTAL_ROWS / threads`) rather than
/// holding rows-per-thread fixed. This is deliberate and is what makes the
/// sweep answer the intent's actual question: for a FIXED aggregate workload,
/// does splitting it across more shared-nothing shards inflate block count
/// (each shard's smaller row share leaves a proportionally larger trailing
/// partial block)? Holding rows-per-thread fixed instead would make total
/// data volume scale with thread count and could never show this effect.
///
/// Cap values: three FIXED (tier-independent) caps, plus one TIER-SCALED
/// CANDIDATE. IMPORTANT — the governing intent's own text refers to
/// "tier-scaled proposals per V2's 8th criterion"
/// (AXVERITY_RECLOG_SLA_TIERED_BLOCK_DURABILITY_V2). That document/intent does
/// NOT exist: checked `mcp__axsemantica-intent__list_intents` on instance
/// `db-live` (prefix `intent:`, and a targeted `intent:axverity-reclog-sla*`
/// search — zero hits), grepped `docs/`, `specs/`, and this whole bridge repo
/// for "TIERED_BLOCK_DURABILITY_V2" / "8th criterion" / "tier-scaled" — zero
/// hits anywhere. Only V1 (Phase A, commit dae35f1, 7 criteria C1-C7) is a
/// real, built artifact. So there is no existing formula to sweep against.
/// `tier_scaled_cap_candidate` below is a CLAUDE-AUTHORED CANDIDATE ONLY, not
/// V2's (because V2 isn't real) — reported in its own labeled column, never
/// conflated with the three measured fixed-cap columns. Per this intent's own
/// `authority AI_PROPOSE_ONLY` / `may-decide false`, whether this candidate
/// (or any tier-scaled cap at all) is worth carrying forward is Chris's call,
/// not assumed here.
const FIXED_CAPS: &[(&str, i64)] = &[
    ("64KiB", 64 * 1024),
    ("1MiB", 1024 * 1024),
    ("4MiB-default", 4 * 1024 * 1024),
];

/// CANDIDATE ONLY (see test doc comment above — no V2 source exists). Linear
/// in the tier's SLA window relative to the fastest tier (100us), clamped to
/// [64KiB, 4MiB]: rationale is that a slower tier accumulates more data
/// between ticks, so a bigger cap avoids splitting one SLA-window's worth of
/// data across multiple blocks (fragmentation), while a faster tier bounds
/// the fsync'd size per tick with a smaller cap.
fn tier_scaled_cap_candidate(sla_us: i64) -> i64 {
    let scaled = (64 * 1024_i64).saturating_mul(sla_us / 100);
    scaled.clamp(64 * 1024, 4 * 1024 * 1024)
}

const CONCURRENCY: &[usize] = &[1, 2, 4, 8, 16, 32];

/// Fixed aggregate workload for the whole sweep — see doc comment above for
/// why this is held constant (not per-thread) across the concurrency axis.
/// Divisible by every value in CONCURRENCY.
const TOTAL_ROWS: usize = 640_000;

struct SweepResult {
    tier: &'static str,
    sla_us: i64,
    cap_label: String,
    cap_bytes: i64,
    threads: usize,
    rows_per_thread: usize,
    elapsed_s: f64,
    aggregate_rows_s: f64,
    total_data_fsyncs: i64,
    total_blocks: usize,
    max_blocks_per_thread: usize,
    min_blocks_per_thread: usize,
}

fn run_sweep_combo(
    root: &std::path::Path,
    tier: &'static str,
    sla_us: i64,
    cap_label: &str,
    cap_bytes: i64,
    threads: usize,
    rows_per_thread: usize,
) -> SweepResult {
    let t0 = Instant::now();
    let mut joins = Vec::new();
    for t in 0..threads {
        let dir = root
            .join(format!("sw-{}-{}-{}t-{}", tier, cap_label, threads, t))
            .to_string_lossy()
            .into_owned();
        joins.push(std::thread::spawn(move || {
            let h = open(&dir, sla_us, cap_bytes);
            for i in 0..rows_per_thread {
                slab_append(Value::Tuple(vec![Value::Int(h), Value::Bytes(synth_row(i))]));
                if i % 64 == 0 {
                    slab_tick(Value::Int(h));
                }
            }
            loop {
                match slab_tick(Value::Int(h)) {
                    Value::Int(-1) => std::thread::sleep(Duration::from_micros(200)),
                    _ => break,
                }
            }
            slab_seal(Value::Int(h));
            let fsyncs = stat_field(h, "data_fsyncs");
            let all = std::fs::read_dir(&dir).unwrap().count();
            let blocks = std::fs::read_dir(&dir)
                .unwrap()
                .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with("blk-"))
                .count();
            assert_eq!(all, blocks, "only blk-*.bin allowed in a slab dir");
            (fsyncs, blocks)
        }));
    }
    let mut total_fsyncs = 0i64;
    let mut total_blocks = 0usize;
    let mut max_blocks = 0usize;
    let mut min_blocks = usize::MAX;
    for j in joins {
        let (fsyncs, blocks) = j.join().unwrap();
        total_fsyncs += fsyncs;
        total_blocks += blocks;
        max_blocks = max_blocks.max(blocks);
        min_blocks = min_blocks.min(blocks);
    }
    let elapsed = t0.elapsed().as_secs_f64();
    let total_rows = threads * rows_per_thread;
    SweepResult {
        tier,
        sla_us,
        cap_label: cap_label.to_string(),
        cap_bytes,
        threads,
        rows_per_thread,
        elapsed_s: elapsed,
        aggregate_rows_s: total_rows as f64 / elapsed,
        total_data_fsyncs: total_fsyncs,
        total_blocks,
        max_blocks_per_thread: max_blocks,
        min_blocks_per_thread: min_blocks,
    }
}

fn result_header() {
    eprintln!(
        "{:>7} {:>22} {:>9} {:>7} {:>12} {:>12} {:>9} {:>9} {:>9} {:>9}",
        "tier", "cap", "cap_B", "threads", "rows/thread", "aggr rows/s", "fsyncs",
        "tot_blk", "max/thr", "min/thr"
    );
}

fn print_result_line(r: &SweepResult) {
    eprintln!(
        "{:>7} {:>22} {:>9} {:>7} {:>12} {:>12.0} {:>9} {:>9} {:>9} {:>9}  ({:.2}s)",
        r.tier, r.cap_label, r.cap_bytes, r.threads, r.rows_per_thread, r.aggregate_rows_s,
        r.total_data_fsyncs, r.total_blocks, r.max_blocks_per_thread, r.min_blocks_per_thread,
        r.elapsed_s
    );
}

/// Shared report/assert tail for both the full sweep and the smoke variant
/// below — prints the table, writes the TSV, and runs the sanity-only
/// assertions (this is exploratory measurement, not a correctness-criteria
/// sweep like `phase_a_sla_tier_sweep`'s C1-C7 assertions).
fn report_sweep_results(root: &std::path::Path, tsv_name: &str, results: &[SweepResult]) {
    eprintln!();
    result_header();
    let mut tsv = String::from(
        "tier\tsla_us\tcap_label\tcap_bytes\tthreads\trows_per_thread\telapsed_s\taggregate_rows_s\ttotal_data_fsyncs\ttotal_blocks\tmax_blocks_per_thread\tmin_blocks_per_thread\n",
    );
    for r in results {
        print_result_line(r);
        tsv.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{:.6}\t{:.1}\t{}\t{}\t{}\t{}\n",
            r.tier, r.sla_us, r.cap_label, r.cap_bytes, r.threads, r.rows_per_thread,
            r.elapsed_s, r.aggregate_rows_s, r.total_data_fsyncs, r.total_blocks,
            r.max_blocks_per_thread, r.min_blocks_per_thread
        ));
    }
    let tsv_path = root.join(tsv_name);
    std::fs::write(&tsv_path, tsv).unwrap();
    eprintln!("\nresults TSV: {}", tsv_path.display());
    eprintln!("scratch root (inspect then rm -rf): {}", root.display());

    for r in results {
        assert!(r.total_data_fsyncs >= r.threads as i64, "[{} {} {}t] expected >=1 fsync/thread", r.tier, r.cap_label, r.threads);
        assert!(r.max_blocks_per_thread >= 1, "[{} {} {}t] every thread must seal >=1 block", r.tier, r.cap_label, r.threads);
    }
}

#[test]
#[ignore = "timed multi-minute sweep matrix; run with -- --ignored --nocapture (authorized by AXVERITY_SLABLOCK_CAP_TIER_CONCURRENCY_SWEEP_V1)"]
fn cap_tier_concurrency_sweep() {
    let root = scratch_root();
    assert!(
        !root.to_string_lossy().contains(".axverity"),
        "harness must never touch .axverity/"
    );

    let mut results = Vec::new();
    for &(tier, sla_us) in TIERS {
        let mut caps: Vec<(String, i64)> =
            FIXED_CAPS.iter().map(|(l, b)| (l.to_string(), *b)).collect();
        caps.push((
            "tier-scaled-candidate".to_string(),
            tier_scaled_cap_candidate(sla_us),
        ));
        for (cap_label, cap_bytes) in &caps {
            for &threads in CONCURRENCY {
                let rows_per_thread = TOTAL_ROWS / threads;
                results.push(run_sweep_combo(
                    &root,
                    tier,
                    sla_us,
                    cap_label,
                    *cap_bytes,
                    threads,
                    rows_per_thread,
                ));
            }
        }
    }

    report_sweep_results(&root, "cap-tier-concurrency-results.tsv", &results);
    eprintln!("\nCAP x TIER x CONCURRENCY SWEEP: complete, {} combos measured", results.len());
}

/// Fast smoke variant of the sweep above — same mechanism, far fewer points,
/// for a quick sanity read before committing to the full multi-minute matrix:
/// tiers capped at <=1s SLA (drops the 2000ms tier), only the two FIXED_CAPS
/// extremes (64KiB and 4MiB-default — the pair most likely to show a
/// cap-driven block-count effect), concurrency pushed straight to the two
/// extremes {1, 32} (skips the middle of CONCURRENCY) so the max-load point
/// is exercised without paying for every intermediate P. 3 tiers x 2 caps x 2
/// concurrency = 12 combos.
#[test]
#[ignore = "smoke sweep; run with -- --ignored --nocapture (authorized by AXVERITY_SLABLOCK_CAP_TIER_CONCURRENCY_SWEEP_V1)"]
fn cap_tier_concurrency_sweep_smoke() {
    let root = scratch_root();
    assert!(
        !root.to_string_lossy().contains(".axverity"),
        "harness must never touch .axverity/"
    );

    let smoke_tiers: Vec<(&str, i64)> =
        TIERS.iter().copied().filter(|&(_, sla_us)| sla_us <= 1_000_000).collect();
    let smoke_caps: Vec<(String, i64)> =
        FIXED_CAPS.iter().map(|(l, b)| (l.to_string(), *b)).collect();
    let smoke_concurrency: &[usize] = &[1, 32];

    let mut results = Vec::new();
    for &(tier, sla_us) in &smoke_tiers {
        for (cap_label, cap_bytes) in &smoke_caps {
            for &threads in smoke_concurrency {
                let rows_per_thread = TOTAL_ROWS / threads;
                results.push(run_sweep_combo(
                    &root,
                    tier,
                    sla_us,
                    cap_label,
                    *cap_bytes,
                    threads,
                    rows_per_thread,
                ));
            }
        }
    }

    report_sweep_results(&root, "cap-tier-concurrency-smoke-results.tsv", &results);
    eprintln!("\nCAP x TIER x CONCURRENCY SMOKE: complete, {} combos measured", results.len());
}

/// RAMP-TO-CEILING — find where the machine can accept no more, don't just
/// sample endpoints. The smoke run above (TOTAL_ROWS=640_000, DIVIDED across
/// threads) measured real numbers but never stressed the machine: per its own
/// TSV, the nine P=1 combos consumed 206 of 222.87 total seconds while the
/// nine P=32 combos finished in 0.2-3s each — CPU stayed near-idle simply
/// because >90% of wall-clock was single-core work, not because P=32 itself
/// is cheap or because the system was ever pushed hard.
///
/// This test instead holds ROWS PER THREAD FIXED (not divided down as
/// concurrency rises) and ramps thread count 1 -> 2 -> 4 -> ... -> 64,
/// INCLUDING beyond the 32 physical cores (oversubscription) — each step
/// offers strictly more aggregate work than the last. If throughput scales
/// linearly with thread count, there is no contention yet. Where per-step
/// aggregate rows/s stops climbing (or drops) is the actual ceiling — fsync/
/// journal contention, CPU saturation, or (per this repo's prior findings,
/// e.g. project_wal_spike memory) a shared kernel/interner resource, not
/// assumed here, reported from the numbers as they land.
///
/// Results print step-by-step AS EACH CONCURRENCY LEVEL COMPLETES (not just
/// in one final table), with the delta vs. the previous step, so a plateau
/// or reversal is visible live, not only in hindsight. Cross-check against an
/// external monitor (`mpstat -P ALL 2` / `vmstat 2` in another terminal)
/// while this runs — don't rely solely on the harness's self-reported rows/s.
///
/// Scoped to ONE representative (tier, cap) pair per concurrency ladder to
/// keep the ramp tractable within a few minutes — the earlier smoke/full
/// sweep already cover tier/cap variation at fixed concurrency; this is the
/// concurrency axis in isolation, pushed to its limit.
const CEILING_ROWS_PER_THREAD: usize = 500_000;
const CEILING_CONCURRENCY: &[usize] = &[1, 2, 4, 8, 16, 24, 32, 48, 64];
const CEILING_COMBOS: &[(&str, i64, &str, i64)] = &[
    ("5ms", 5_000, "4MiB-default", 4 * 1024 * 1024),
    ("5ms", 5_000, "64KiB", 64 * 1024),
];

/// Per-call timing breakdown for one ceiling step — FACTS about where wall
/// time actually goes, not inference from aggregate throughput alone. Every
/// field is a direct measurement (Instant::elapsed around the actual call),
/// summed/maxed across all threads in the step. This is what let the P=1->P=2
/// collapse (43,658 -> 17,851 rows/s, 11.45s -> 56.02s, fsyncs 2212 -> 14960)
/// get diagnosed instead of guessed at.
#[derive(Default, Clone, Copy)]
struct CeilingStepDebug {
    threads: usize,
    rows_per_thread: usize,
    step_wall_s: f64,
    aggregate_rows_s: f64,
    total_data_fsyncs: i64,
    // Hot-loop append() calls (the never-fsyncs append path).
    append_calls: u64,
    append_total_us: f64,
    append_max_us: f64,
    // Hot-loop tick() calls (checked every 64 appends) that did NOT trigger a
    // real fsync (data_fsyncs unchanged) — should be near-free.
    hot_tick_noop_calls: u64,
    hot_tick_noop_total_us: f64,
    // Hot-loop tick() calls that DID trigger a real fsync sweep.
    hot_tick_flush_calls: u64,
    hot_tick_flush_total_us: f64,
    hot_tick_flush_max_us: f64,
    // Final drain loop: repeated tick() polls (some no-op -> sleep 200us,
    // one final successful flush) after the append loop finishes.
    drain_noop_calls: u64,
    drain_sleep_total_us: f64,
    drain_flush_calls: u64,
    drain_flush_total_us: f64,
    drain_flush_max_us: f64,
    // Sum of each thread's own measured wall time (cross-check: this summed
    // across buckets above should ~match; any gap is unaccounted-for time —
    // e.g. scheduling, the sha256/Vec work inside append not isolated below).
    thread_wall_total_us: f64,
}

fn run_ceiling_step_debug(
    root: &std::path::Path,
    tier: &'static str,
    sla_us: i64,
    cap_label: &str,
    cap_bytes: i64,
    threads: usize,
    rows_per_thread: usize,
) -> CeilingStepDebug {
    let t0 = Instant::now();
    let mut joins = Vec::new();
    for t in 0..threads {
        let dir = root
            .join(format!("dbg-{}-{}-{}t-{}", tier, cap_label, threads, t))
            .to_string_lossy()
            .into_owned();
        joins.push(std::thread::spawn(move || {
            let thread_t0 = Instant::now();
            let h = open(&dir, sla_us, cap_bytes);
            let mut append_calls = 0u64;
            let mut append_total_us = 0.0f64;
            let mut append_max_us = 0.0f64;
            let mut hot_noop_calls = 0u64;
            let mut hot_noop_total_us = 0.0f64;
            let mut hot_flush_calls = 0u64;
            let mut hot_flush_total_us = 0.0f64;
            let mut hot_flush_max_us = 0.0f64;
            let mut last_fsyncs = 0i64;

            for i in 0..rows_per_thread {
                let a0 = Instant::now();
                slab_append(Value::Tuple(vec![Value::Int(h), Value::Bytes(synth_row(i))]));
                let a_us = a0.elapsed().as_secs_f64() * 1e6;
                append_calls += 1;
                append_total_us += a_us;
                if a_us > append_max_us {
                    append_max_us = a_us;
                }
                if i % 64 == 0 {
                    let tk0 = Instant::now();
                    slab_tick(Value::Int(h));
                    let tk_us = tk0.elapsed().as_secs_f64() * 1e6;
                    let now_fsyncs = stat_field(h, "data_fsyncs");
                    if now_fsyncs > last_fsyncs {
                        hot_flush_calls += 1;
                        hot_flush_total_us += tk_us;
                        if tk_us > hot_flush_max_us {
                            hot_flush_max_us = tk_us;
                        }
                        last_fsyncs = now_fsyncs;
                    } else {
                        hot_noop_calls += 1;
                        hot_noop_total_us += tk_us;
                    }
                }
            }

            let mut drain_noop = 0u64;
            let mut drain_sleep_us = 0.0f64;
            let mut drain_flush_calls = 0u64;
            let mut drain_flush_total_us = 0.0f64;
            let mut drain_flush_max_us = 0.0f64;
            loop {
                let tk0 = Instant::now();
                let res = slab_tick(Value::Int(h));
                let tk_us = tk0.elapsed().as_secs_f64() * 1e6;
                match res {
                    Value::Int(-1) => {
                        drain_noop += 1;
                        let s0 = Instant::now();
                        std::thread::sleep(Duration::from_micros(200));
                        drain_sleep_us += s0.elapsed().as_secs_f64() * 1e6;
                    }
                    _ => {
                        drain_flush_calls += 1;
                        drain_flush_total_us += tk_us;
                        if tk_us > drain_flush_max_us {
                            drain_flush_max_us = tk_us;
                        }
                        break;
                    }
                }
            }
            slab_seal(Value::Int(h));
            let fsyncs = stat_field(h, "data_fsyncs");
            let thread_wall_us = thread_t0.elapsed().as_secs_f64() * 1e6;

            (
                fsyncs, append_calls, append_total_us, append_max_us,
                hot_noop_calls, hot_noop_total_us, hot_flush_calls, hot_flush_total_us, hot_flush_max_us,
                drain_noop, drain_sleep_us, drain_flush_calls, drain_flush_total_us, drain_flush_max_us,
                thread_wall_us,
            )
        }));
    }

    let mut d = CeilingStepDebug { threads, rows_per_thread, ..Default::default() };
    for j in joins {
        let (
            fsyncs, append_calls, append_total_us, append_max_us,
            hot_noop_calls, hot_noop_total_us, hot_flush_calls, hot_flush_total_us, hot_flush_max_us,
            drain_noop, drain_sleep_us, drain_flush_calls, drain_flush_total_us, drain_flush_max_us,
            thread_wall_us,
        ) = j.join().unwrap();
        d.total_data_fsyncs += fsyncs;
        d.append_calls += append_calls;
        d.append_total_us += append_total_us;
        d.append_max_us = d.append_max_us.max(append_max_us);
        d.hot_tick_noop_calls += hot_noop_calls;
        d.hot_tick_noop_total_us += hot_noop_total_us;
        d.hot_tick_flush_calls += hot_flush_calls;
        d.hot_tick_flush_total_us += hot_flush_total_us;
        d.hot_tick_flush_max_us = d.hot_tick_flush_max_us.max(hot_flush_max_us);
        d.drain_noop_calls += drain_noop;
        d.drain_sleep_total_us += drain_sleep_us;
        d.drain_flush_calls += drain_flush_calls;
        d.drain_flush_total_us += drain_flush_total_us;
        d.drain_flush_max_us = d.drain_flush_max_us.max(drain_flush_max_us);
        d.thread_wall_total_us += thread_wall_us;
    }
    d.step_wall_s = t0.elapsed().as_secs_f64();
    d.aggregate_rows_s = (threads * rows_per_thread) as f64 / d.step_wall_s;
    d
}

fn print_ceiling_debug(d: &CeilingStepDebug) {
    eprintln!(
        "  P={:<3} wall={:>7.2}s  aggr={:>10.0} rows/s  fsyncs={:>7}",
        d.threads, d.step_wall_s, d.aggregate_rows_s, d.total_data_fsyncs
    );
    eprintln!(
        "        append: {:>7} calls, {:>9.1}ms total, {:>8.1}us max/call",
        d.append_calls, d.append_total_us / 1000.0, d.append_max_us
    );
    eprintln!(
        "        hot-tick noop:  {:>7} calls, {:>9.1}ms total",
        d.hot_tick_noop_calls, d.hot_tick_noop_total_us / 1000.0
    );
    eprintln!(
        "        hot-tick FLUSH: {:>7} calls, {:>9.1}ms total, {:>8.1}us max/call",
        d.hot_tick_flush_calls, d.hot_tick_flush_total_us / 1000.0, d.hot_tick_flush_max_us
    );
    eprintln!(
        "        drain: {:>7} noop-polls ({:>8.1}ms slept), {:>3} final-flush calls, {:>9.1}ms total, {:>8.1}us max/call",
        d.drain_noop_calls, d.drain_sleep_total_us / 1000.0,
        d.drain_flush_calls, d.drain_flush_total_us / 1000.0, d.drain_flush_max_us
    );
    let accounted_us = d.append_total_us + d.hot_tick_noop_total_us + d.hot_tick_flush_total_us
        + d.drain_sleep_total_us + d.drain_flush_total_us;
    eprintln!(
        "        sum-of-threads wall={:>9.1}ms, accounted-for={:>9.1}ms ({:>5.1}%), unaccounted={:>9.1}ms",
        d.thread_wall_total_us / 1000.0,
        accounted_us / 1000.0,
        100.0 * accounted_us / d.thread_wall_total_us,
        (d.thread_wall_total_us - accounted_us) / 1000.0
    );
}

#[test]
#[ignore = "ramps concurrency 1->64 to find the real saturation ceiling, several minutes, with per-call debug timing; run with -- --ignored --nocapture, and watch `iostat -x 2` / `mpstat -P ALL 2` in a second terminal (authorized by AXVERITY_SLABLOCK_CAP_TIER_CONCURRENCY_SWEEP_V1)"]
fn cap_tier_concurrency_ceiling() {
    let root = scratch_root();
    assert!(
        !root.to_string_lossy().contains(".axverity"),
        "harness must never touch .axverity/"
    );
    eprintln!("scratch root: {}", root.display());

    for &(tier, sla_us, cap_label, cap_bytes) in CEILING_COMBOS {
        eprintln!("\n=== ramp: tier={} cap={} rows/thread={} (fixed) ===", tier, cap_label, CEILING_ROWS_PER_THREAD);
        let mut prev_rows_s: Option<f64> = None;
        for &threads in CEILING_CONCURRENCY {
            let d = run_ceiling_step_debug(
                &root, tier, sla_us, cap_label, cap_bytes, threads, CEILING_ROWS_PER_THREAD,
            );
            print_ceiling_debug(&d);
            let delta = match prev_rows_s {
                Some(prev) => format!("{:+.0}% vs prev step", (d.aggregate_rows_s - prev) / prev * 100.0),
                None => "baseline".to_string(),
            };
            eprintln!("    -> {}", delta);
            prev_rows_s = Some(d.aggregate_rows_s);
        }
    }

    eprintln!("\nCAP x TIER x CONCURRENCY CEILING RAMP (debug): complete");
}

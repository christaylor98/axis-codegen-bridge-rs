# AXVERITY_POST_BRIDGE_PERF_VALIDATION_V1 — report

**Intent:** `AXVERITY_POST_BRIDGE_PERF_VALIDATION_V1`
**Runner:** ClaudeCode (bounded; propose-only on any reinstatement)
**Date:** 2026-07-30
**Derived from:** BRIDGE_SURFACE_AUDIT_V1, INSERT_PATH_HONESTY_V1, BRIDGE_CONSOLIDATION_V1,
BYTE_INT_CODEC_COLLAPSE_V1, BENCHMARK_UB_AND_PROTOCOL_FIX_V1

---

## Verdict up front

**Safe as-is. No primitive reinstatement is proposed.**

| Phase | Result |
|---|---|
| 0 DOC-FIX | Done; comment-only, block balance re-verified. **The intent's own description of this fix was wrong** — see below. |
| 1 FIELD-RANGE-AUDIT | **5 field-6 sites, all safe.** Zero breaking dependencies on the old 0..5 boundary. 3 doc/coverage drift items found, none a correctness break. |
| 2 REBUILD-VERIFY | **smoke 15/17, SLT 6/6** — exact baseline match. |
| 3 MEASURE | **fsyncs/row 2.000 (exact match).** Rate +0.46%, inside the 2.9% variance spread. **NULL RESULT**, as predicted. |
| 4 REPORT | This document. No reinstatement proposed. |

No STOP condition fired.

---

## Phase 0 — DOC-FIX

Target: `axis-codegen-bridge-rs/registry/axis-codegen-bridge.axreg:197-200`.

### The intent's instruction was partly wrong, and I did not follow it literally

The intent says to correct the block so it stops "describing `int16/int32_be_decode/encode`
as existing." Checked against ground truth before editing:

| fn | registry decl | Rust impl | live M1 callers | status |
|---|---|---|---|---|
| `int16_be_decode` | absent | absent | 0 | **RETIRED** — line 198 was stale |
| `int32_be_decode` | absent | absent | 0 | **RETIRED** — line 200 was stale |
| `int16_be_encode` | `axis-bridge.axreg:3112` | `bytes_codec.rs:208` | present | **ALIVE** |
| `int32_be_encode` | `axis-bridge.axreg:3131` | `bytes_codec.rs:229` | present | **ALIVE** |

The two **encoders still exist** and are referenced by 23 live `lib/*.m1` files.
Deleting all four lines as instructed would have replaced one doc drift with a worse
one — a registry that fails to document two functions that are on the hot path.

**Applied instead:** removed the two stale decode lines, added the retirement note
(mirroring the canonical wording already in `axis-bridge.axreg:3123`), documented the
two `bytes_get`/`bytes_push` atoms that *do* exist but were undocumented here, and
left both encoder lines intact.

### Block-balance re-verification

| file | `fn` blocks | `end` markers | note |
|---|---|---|---|
| `axis-codegen-bridge.axreg` (edited) | **0** | **0** | types + comments only — it has *no* fn blocks at all |
| `axis-bridge.axreg` (untouched) | **367** | **367** | the 367/367 the intent refers to lives **here**, not in the edited file |

`git diff` on the edited file contains **zero non-comment changed lines** (verified by
filtering the diff for lines not starting with `//`). The intent's "re-verify 367/367
after the edit" premise pointed at the wrong file; both are recorded above.

---

## Phase 1 — FIELD-RANGE-AUDIT (T28)

`hotblk` widened `NFIELDS` 6 → 7 (`hotblk.rs:62`), i.e. valid field range 0..5 → 0..6,
with slot 6 = the active block's allocator capacity.

### T28: field-6 sites in axVerity — 5 found, 5 safe

| # | site | direction | use | verdict |
|---|---|---|---|---|
| 1 | `lib/pg_hotblk_mint.m1:59` | **write** | `hotblk_set(Int(6), capacity)` from `mem_reserve_raw`/`hotblk_block_bytes` | **SAFE** — the write that makes slot 6 meaningful |
| 2 | `lib/pg_hotblk_write.m1:49` | read | `cap0` → rotate/overflow decision | **SAFE** |
| 3 | `lib/pg_hotblk_commit.m1:16` | read | `cap1` → `mem_write_checked` bound | **SAFE** |
| 4 | `lib/pg_hotblk_seal_mint.m1:54` | read | `capacity` → `mem_free_checked` | **SAFE** — this *is* the UB fix |
| 5 | `lib/pg_derive_seal_mint.m1:35` | read | `capacity` → `mem_free_checked` | **SAFE** — this *is* the UB fix |

`mem_free_checked` is called at exactly **two** sites (4 and 5), and both now free with
the threaded capacity. `mem_reserve_raw` (`rawmem.rs:181`) returns the *requested*
capacity verbatim, so slot 6 is exact by construction, not approximate.

### Breaking dependencies on the old 0..5 boundary: **none**

Checked and found clear:
- **No axVerity-side Rust** reads or writes hotblk fields at all (only M1 does).
- **No range check, loop bound, or array-size assumption** over the hotblk field set
  anywhere in `lib/`, `src/`, or the registries.
- `axis-bridge.axreg:2135` documents the field indices by **reference**
  ("Field indices match `lib/pg_hotblk_write.m1`") rather than hardcoding a range, so
  it did not go stale.
- `hotblk_recover.rs` reconstructs from sealed block *files*; it never touches the
  accumulator register, so it has no field-count coupling.
- `hotblk.rs`'s own `get_out_of_range_panics` test asserts against `NFIELDS`, so it
  tracked the widening automatically.

### Three drift items found — reported, not fixed (none is a correctness break)

1. **`lib/pg_rawblk_write.m1:34`** — the **only** remaining literal `Int(4194304)` in
   M1. Its sibling `pg_hotblk_write.m1:50` reads `cap0 = hotblk_get(Int(6))` for the
   same rotate decision; `pg_rawblk_write` still restates the constant.
   **Safe today** — `hotblk_block_bytes.m1` is the single, non-configurable definition
   (`4194304`, no env dial), and both branches of `pg_hotblk_mint` source capacity from
   it, so the literal is numerically equal. It is a **latent drift site** of exactly the
   kind `hotblk_block_bytes.m1`'s own header warns about. **Not touched**: this file is
   `AXVERITY_MEMCPY_HOTPATH_TRIAL_V1` bake-off apparatus, and
   `BAKEOFF_APPARATUS_UNTOUCHABLE` forbids it. Note the block still gets a correct
   *bound* on this path — `pg_hotblk_commit` reads slot 6 for `mem_write_checked` — so
   only the rotate threshold duplicates the constant.
2. **`hotblk.rs:3`** — module doc still says "it stores **six** i64 fields". Stale; it
   stores seven. Bridge-side comment, outside this intent's Phase 0 edit scope.
3. **`hotblk.rs` `unset_reads_zero_then_roundtrips`** — its `vals` array has 6 entries,
   so the set/get round-trip covers fields 0..5 only; **field 6 is not round-trip
   tested**. The all-zero loop above it does cover 0..6. A coverage gap that appeared
   silently when NFIELDS widened, not a failure.

---

## Phase 2 — REBUILD-VERIFY (T29)

Full chain rebuilt in order: `axis-lang-lab-working` (`cargo build --release`, exit 0)
→ `axis-codegen-bridge-rs` (exit 0) → `scripts/build.sh` (exit 0) →
`scripts/pg-server-build.sh` (exit 0, 4 workers on shards 1..4). Orphaned
`axverity-pg_server` processes cleared before and after.

| harness | baseline | this run | verdict |
|---|---|---|---|
| `scripts/axv-smoke.sh` | 15/17 | **15/17** | **match** |
| `scripts/slt-run.sh` | 6/6 | **6/6** (simple + extended legs) | **match** |

The two smoke failures are S16 `PREPARE` and S17 `EXECUTE`, both
`ERROR: axVerity: unsupported statement`, byte-identical to the documented baseline
signature — the same unimplemented feature, not a regression. Verified against the
recorded failure signature rather than assumed pre-existing, per the stop condition.

**T29: pass rate did not regress. No STOP condition.**

---

## Phase 3 — MEASURE (T30)

Method reproduces INSERT_PATH_HONESTY_V1's T7 verbatim for comparability: fresh store,
300 autocommitted single-row INSERTs (one statement per transaction, one connection),
fsyncs via `strace -f -c -e trace=fsync,fdatasync`, throughput measured **separately
without strace**. Harness in the session scratchpad; **no repo script modified**.

Comparison point is INSERT_PATH_HONESTY_V1's recorded pre-cleanup baseline, which T9
established as still comparable.

### fsyncs per committed row

| arm | fsyncs | rows | **fsyncs/row** |
|---|---|---|---|
| pre-cleanup baseline (T7, dial OFF) | 600 | 300 | 2.000 |
| **post-cleanup (this run)** | **600** | **300** | **2.000** |

Exact match. This is the expected structural result: every function this session
repatriated is `effect pure`, and a pure function cannot issue an fsync. Confirmed
empirically rather than argued.

A sanity gate (`SELECT count(*) WHERE a='r1'` → 1) guards the number — an earlier
harness revision silently produced a false `0.000` when a stale server held the port
and the workload never committed. That failure mode is now fail-fast (port pre-flight
+ row-count gate), and the reported figure passed both.

### Insert rate (no strace, 4 reps, rows/s)

| arm | mean | median | min | max | spread |
|---|---|---|---|---|---|
| pre-cleanup baseline (T7, dial OFF) | 107.8 | 107.9 | 106.2 | 109.3 | 2.9% |
| **post-cleanup (this run)** | **108.3** | 108.85 | 106.4 | 109.1 | 2.49% |

**Delta: +0.46% (post-cleanup marginally faster).** Comfortably inside the 2.9% spread,
and the ranges overlap almost entirely (106.4–109.1 vs 106.2–109.3).

> ### T30 verdict: **NULL RESULT.** No measurable regression.
> The intent's prediction — "this session's bridge changes produce no measurable
> regression… delta stays within the 2.9%/7.8% variance spread… null result is a valid,
> expected outcome" — is **upheld**.

### Isolating the individual changes (CCall accounting)

The aggregate above cannot by itself attribute a sub-noise delta, so each change is
isolated by CCall count — grounded in counts read from the emitted Rust and in T13's
independently measured **~65 ns per CCall**.

CCall dispatches counted from `build/generated_*_xb.rs` (excluding `CIf` branch nodes),
**independently reproducing T13's table exactly**:

| composite | CCalls | retired primitive | delta |
|---|---|---|---|
| `be16_decode` | 6 | 1 | +5 |
| `be32_decode` | 12 | 1 | +11 |
| `be16_encode` | 7 | (still a primitive) | n/a |
| `be32_encode` | 13 | (still a primitive) | n/a |

**Codec collapse, in context.** Only the *decoders* were switched; both encoders remain
Rust primitives. Three `be32_decode` sites are reachable per INSERT statement —
`pg_frame_buf_incomplete`, `pg_dispatch_ext`, and `pg_query_text` (via `pg_ext_simple`):

- 3 × +11 = **+33 CCalls/row** ≈ **2.15 µs/row**

**Capacity threading, in context.** From the `70b5b90` diff, the live per-row cost is
exactly two added `hotblk_get(Int(6))` calls (`pg_hotblk_commit`, `pg_hotblk_write`),
each replacing an `Int(4194304)` constant-pool entry. `hotblk_get` is declared
`effect fullIo, deterministic false`, so it is not CSE-shareable and each is a real
dispatch:

- **+2 CCalls/row** ≈ **0.13 µs/row**
- The `pg_hotblk_mint` additions (`hotblk_set` slot 6 + a third tab field to parse) are
  **per 4 MiB block**, not per row — amortised to ≈ 0.

| change | CCalls/row | µs/row | share of the 9.23 ms row budget |
|---|---|---|---|
| codec collapse (decoders) | +33 | 2.15 | 0.023% |
| capacity threading | +2 | 0.13 | 0.0014% |
| **combined** | **+35** | **2.27** | **0.025%** |

The combined predicted cost is **~118× smaller than the 2.9% run-to-run variance
floor**. That is why the aggregate measurement shows nothing: not because the effect is
unmeasured, but because it is **bounded two orders of magnitude below** what this
harness can resolve.

### On T31 — the unattributed 8.6 ms/row (carried forward, still open)

The intent's stop condition requires saying explicitly whether T9's unattributed mass
confounds this verdict. Split answer, honestly:

- **It does confound the aggregate.** 91% of the row budget remains unattributed, so the
  +0.46% rate delta cannot be causally assigned to anything this session did. Read that
  number as "no regression detected," **not** "these changes are +0.46% faster."
- **It does not confound the isolation.** The CCall accounting does not depend on the
  aggregate — it is derived from counts in the emitted Rust and an independently
  measured per-CCall constant. It yields an **upper bound** on this session's cost that
  holds regardless of what the other 8.6 ms turns out to be.

**T31 remains open and is not resolved here.** Nothing in this intent investigated the
8.6 ms; it is carried forward unchanged.

---

## Phase 4 — Reintegration check

| axis | check |
|---|---|
| identity | All work maps to Phases 0–4 of `AXVERITY_POST_BRIDGE_PERF_VALIDATION_V1`. |
| `PERFORMANCE_NOW_IN_SCOPE` | **Discharged** — Phase 3 was run, not skipped. fsyncs/row and rate both measured against the recorded pre-cleanup baseline. |
| `NO_BEHAVIOR_CHANGE_UNAUDITED` | **Discharged** — all 5 field-6 sites enumerated with a per-site verdict from reading each call site's logic; the "no old-boundary dependency" claim is backed by the negative sweeps listed in Phase 1, not assumed from the unchanged type signature. |
| `RETURN_PATH_IS_ADD_PRIMITIVE_NOT_REVERT` | **Not exercised** — no regression found, so no reinstatement and no revert is proposed. |
| `M1_LANG_TEST_OUT_OF_SCOPE` | **Honoured** — not read, not built, not touched. |
| `BAKEOFF_APPARATUS_UNTOUCHABLE` | **Honoured** — `pg_rawblk_write.m1` was *found* by the audit and is *reported only*; no dial was read as a variable or changed. `memcpy_*`/`slice4_*`/`slab_shadow_*` untouched. |
| `MEASURE_BEFORE_REINSTATE` | **Honoured** — measurement completed; verdict is "no reinstatement warranted." |
| `AI_PROPOSE_ONLY` | **Honoured** — nothing reinstated. The three Phase 1 drift items are reported for Chris's decision, not fixed. |
| priority order | correctness (Phase 2 green) > behaviour-change auditing (Phase 1, 5/5 sites) > evidence-grounding (counts re-derived, not inherited) > performance measurement (Phase 3) > reinstatement speed (moot). |
| epistemics | T28/T29/T30 reported directly; T30's null result recorded as a legitimate outcome, not softened; T31 carried forward as still-open. |

### Risk register, as it actually played out

| risk | outcome |
|---|---|
| field-6 dependency missed (high) | **Did not materialise.** Zero dependencies on the old boundary; the audit did surface 3 real drift items it would otherwise have missed. |
| small regression inside the 2.9%/7.8% spread producing a false positive (medium) | **Avoided** — the delta is *positive* (+0.46%) and the CCall bound independently caps the true cost at 0.025%, so there is no ambiguous middle to misread. |
| T9's mass confounds the measurement (high) | **Partly materialised.** It confounds the aggregate; it does not confound the CCall isolation. Stated explicitly above rather than papered over. |
| momentum treats the doc-fix/audit as busywork (medium) | **Avoided** — the doc-fix caught the intent's own factual error about the encoders, and the audit caught 3 drift items. Neither would have surfaced from a literal, fast execution. |

---

## Deliverables and open items for Chris

**Changed:** `axis-codegen-bridge-rs/registry/axis-codegen-bridge.axreg` — comment lines
only (Phase 0). Nothing else in any repo was modified.

**No authorization is being requested** — no primitive reinstatement is warranted.

Three items reported for your decision, all currently harmless:

1. `lib/pg_rawblk_write.m1:34` — last literal `Int(4194304)`; should read
   `hotblk_get(Int(6))` like its sibling. **Blocked by `BAKEOFF_APPARATUS_UNTOUCHABLE`**,
   so it needs its own intent (or an explicit exemption) to fix.
2. `hotblk.rs:3` — "six i64 fields" → seven. One-line comment fix.
3. `hotblk.rs` round-trip test — extend `vals` to 7 entries so field 6 is covered.

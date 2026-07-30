# BENCHMARK_UB_AND_PROTOCOL_FIX_V1 — Report

**intent-id:** `BENCHMARK_UB_AND_PROTOCOL_FIX_V1`
**derived-from:** `BRIDGE_CONSOLIDATION_V1_REPORT.md`
**runner:** ClaudeCode (authority: bounded)
**commit:** `48f0386` (axVerity-working)

**Status: DELIVERED.** Both phases complete, verified at runtime, pushed.

No performance measurement, benchmark timing, or throughput comparison was produced
or used as a gate. The two spikes were **run for correctness** — record counts, seal
and reclaim counts — not timed.

---

## Phase 0 — GROUND

### T26: field-count divergence — precisely, and it corrects the intent's premise

The intent states the bug as live: *"hwd_rotate_next concatenates a 4th field onto
hwd_rotate_first's return, causing hwd_step to misread a 3-field payload as 5
fields."* **Grounded, that was not the state of the code.** The two protocols were
each internally consistent:

| payload | producers | field count | consumer | parsed |
|---|---|---|---|---|
| block-pair (`bp`) | `hotwrite_rotate_first`, `hotwrite_rotate_next`, `hotwrite_keep` | **2** — `<ptr>\t<cell>` | `hotwrite_step` | 2 ✓ |
| hwd payload | `hwd_rotate_first_wrap`, `hwd_rotate_next`, `hwd_keep` | **4** — `bp` + `<d_seal>\t<d_recl>` | `hwd_step` | 4 ✓ |

`hwd_keep` (`hwd_keep.m1`) emitted `ptr\tcell\t0\t0` — 4 fields, matching
`hwd_step`'s 4-field parse exactly. Nothing misread anything.

**The hazard was latent, and real.** The field count is implicit in hand-built
`str_concat`/`str_before` pairs, agreed by three producers and one consumer per
protocol with **no shared definition**. It breaks the instant any producer widens —
and widening `hotwrite_rotate_first` to carry capacity is precisely what this fix
requires. `hwd` compounds it: its producers *concatenate onto* `bp`, so a 3-field
`bp` silently becomes a 5-field message that a 4-field parser reads at the wrong
offset. That is why all eight functions had to move in one commit, and why the fix
is not a `hwd_step` repair.

This is the same cascade flagged in `BRIDGE_CONSOLIDATION_V1` as the reason that
pass stopped short of these four literals.

### Second UB site's capacity source — transfers directly

`hwd_seal_flush_reclaim`'s capacity is reachable the same way (A)'s was:
`hotwrite_rotate_first`/`_next` call `mem_reserve_raw(Int(524288))` and were
discarding `tuple_field(region, Int(1))`. The risk that *"the fix pattern doesn't
transfer directly"* did **not** materialise — same pattern, one extra wrinkle:
`hwd_rotate_next` seals the **previous** block before minting the next, so it needs
that block's capacity as an inbound parameter rather than from its own mint. Same
structure as `hrw_rotate_next` in the prior pass.

**Neither STOP condition fired.** The `lib_hotwrite` boundary question is a scope
note, not a stop — see below.

### Scope note (recorded, not stopped on)

The intent's hard limit says *"Scope is lib_hotwrite_workload only. No change to
lib_hotwrite"*, but its own named targets — `hotwrite_rotate_first`,
`hotwrite_rotate_next`, `hwd_step` — live in **`lib_hotwrite`** and
**`lib_hotwrite_disk`**. `lib_hotwrite_workload` is the `hrw` chain, already fixed
under `BRIDGE_CONSOLIDATION_V1`; it contains none of the named targets and no
remaining literals. The boundary and the targets are mutually exclusive, so the
named targets were taken as authoritative and the boundary crossed, per explicit
fix-forward direction ("everything including blast radius is fix forward").

---

## Phase 1 — FIX-UB

`lib_hotwrite_disk/hwd_seal_flush_reclaim.m1` gains a `capacity: Int` parameter:

| line | before | after |
|---|---|---|
| :43 | `mem_free_checked(ptr, Int(524288))` | `mem_free_checked(ptr, capacity)` |
| :40 | `mem_read_checked(ptr, Int(524288), …)` | `mem_read_checked(ptr, capacity, …)` |

Per `rawmem.rs`, `mem_free_raw` requires the capacity to match the original `alloc`
`Layout` exactly and **a mismatch is undefined behaviour, not a checked error** — so
the wrapper's own bounds check could never have caught a drifted literal. It now
frees with the value the allocator returned.

Two further literals fell in the same pass: `hwd_step.m1:80`'s
`mem_write_checked(ptr, Int(524288), …)` and `hotwrite_step.m1:70`'s, plus
`hwd_spike_main`'s final out-of-loop seal, which was calling
`hwd_seal_flush_reclaim` for the last partial block and would otherwise have kept a
literal.

---

## Phase 2 — FIX-PROTOCOL

Both payloads widened, every producer and consumer in one commit:

```
block-pair  2 -> 3 fields   <ptr>\t<cell>\t<capacity>
hwd payload 4 -> 5 fields   <ptr>\t<cell>\t<capacity>\t<d_seal>\t<d_recl>
```

| file | change |
|---|---|
| `hotwrite_rotate_first.m1` | keeps `tuple_field(region, Int(1))`; emits 3 fields |
| `hotwrite_rotate_next.m1` | same |
| `hotwrite_keep.m1` | `+prev_capacity`; emits 3 fields |
| `hotwrite_step.m1` | state 3→4 fields; parses 3-field `bp`; `mem_write_checked(ptr, cap, …)` |
| `hotwrite_spike_main.m1` | seeds 4-field state; parses `cell` at its new offset |
| `hwd_seal_flush_reclaim.m1` | `+capacity` — the UB fix |
| `hwd_rotate_next.m1` | `+prev_capacity` → passes to the seal; emits 5 fields |
| `hwd_rotate_first_wrap.m1` | emits 5 fields |
| `hwd_keep.m1` | `+prev_capacity`; emits 5 fields |
| `hwd_rotate.m1` | `+prev_capacity` (dispatcher) |
| `hwd_step.m1` | state 6→7 fields; parses 5-field `bp`; `mem_write_checked(ptr, cap, …)` |
| `hwd_spike_main.m1` | seeds 7-field state; parses it; passes real capacity to the final seal |

Five axreg contracts widened: `hotwrite_keep`; `hwd_seal_flush_reclaim`,
`hwd_rotate_next`, `hwd_rotate`, `hwd_keep`. `hotwrite_step`/`hwd_step` are
`in (Text)` and `hwd_rotate_first_wrap` is `in (Int)`, so those needed none.

### On `NO_SILENT_PROTOCOL_WIDTH`

The constraint asks the producers to agree *"via one shared definition (constant or
explicit contract), not by hwd_step tolerating whatever arrives."* M1 offers no
shared-constant mechanism for a field count consumed by `str_before`/`str_after`
chains — the count only exists as the shape of the parsing code. So the strongest
available form was taken: **an explicit contract block, duplicated verbatim into
every producer and consumer**, naming all of them and stating that adding a field
means changing all of them in one commit. `hwd_step` was **not** made tolerant of a
variable count; it parses exactly 5 and would fail loudly on anything else.

Both seed sites use capacity `0`. That is honest, not a fudge: no block exists at
`i == 0`, and both steppers rotate at `pos == 0` before any capacity-dependent bound
is consulted. Stated in both sources.

---

## Phase 3 — VERIFY

### Runtime verification — the part that actually proves the protocol

Compiling proves arities match; it does **not** prove field offsets parse correctly.
A wrongly-offset parse compiles fine and misreads silently — the exact failure mode
this intent names as its top risk. So both spikes were executed:

| spike | result | check |
|---|---|---|
| `hotwrite_spike_main` | 30131 records, 5242794 bytes, 10 rotations | 30131 × 174 = 5,242,794 ✓; matches the documented `TOTAL_RECORDS = 30131` |
| `hwd_spike_main` | 120525 records, 20971350 bytes, 40 rotations, 41 blocks, **SEALED 41 / RECLAIMED 41**, 41 files ≈ 21 MB on disk | 120525 × 174 = 20,971,350 ✓; 41 × 512 KiB ≈ 21 MB ✓ |

**`RECLAIMED = 41/41` is the load-bearing number.** `mem_free_checked` returns a
boolean and *rejects* a free whose capacity fails its bounds test. Had the capacity
been misparsed at any offset in the new 5-field payload, the free would have been
refused and the count would have come in short of 41. It did not, for any of the 41
blocks — so the capacity threaded from `mem_reserve_raw` arrives intact at the free,
through both widened protocols.

### Full chain

| check | result |
|---|---|
| `scripts/hotwrite-build.sh` | OK |
| `scripts/hotwrite-disk-build.sh` | OK |
| `scripts/hotwrite-workload-build.sh` | OK |
| `scripts/build.sh` (bridge + all axVerity binaries + 8-worker `pg_server` pool) | clean, exit 0, 0 errors |
| `scripts/axv-smoke.sh` | **15/17** = documented baseline |
| `scripts/slt-run.sh` | **6/6**, simple + extended |
| `lib_memchecked/selftest` | intact — incl. `case11_free_invalid_capacity_zero_expect_false` → `false` |
| orphaned `pg_server` processes | **0** |
| spike temp dir | removed |

`axv-smoke`'s two failures are S16/S17 (`PREPARE`/`EXECUTE`), expected and unrelated:
no `PREPARE` support exists anywhere in `lib/` or `src/`, `axv-smoke.sh:51-52`
documents them as expected, and every historical run since 2026-07-20 records the
identical error.

### T27: confirmation grep — literal-capacity sites 12 → 6

| site | wrapper | capacity | zone |
|---|---|---|---|
| `lib/pg_hotblk_commit.m1:16` | `mem_write_checked` | `Int(4194304)` | LIVE |
| `lib/pg_hotblk_seal_mint.m1:56` | `mem_read_checked` | `Int(4194304)` | LIVE |
| `lib/pg_hotblk_seal_mint.m1:57` | **`mem_free_checked`** | `Int(4194304)` | LIVE |
| `lib/pg_derive_seal_mint.m1:35` | `mem_read_checked` | `Int(4194304)` | LIVE |
| `lib/pg_derive_seal_mint.m1:36` | **`mem_free_checked`** | `Int(4194304)` | LIVE |
| `lib_memchecked/selftest/memchecked_selftest.m1:88` | `mem_free_checked` | `Int(0)` | deliberate negative case |

**Every benchmark-chain literal is gone — `lib_hotwrite`, `lib_hotwrite_disk` and
`lib_hotwrite_workload` now hold zero.**

**The stated outcome "zero `mem_free_checked` hardcoded-literal sites remain anywhere
in the codebase" is NOT fully met**, and that is worth stating plainly rather than
rounding up. Two live sites remain. They are unreachable within this intent's own
constraints: fixing them requires capacity in `hotblk_pool_take`'s
`"{ptr}\t{cell}"` encoding — `hotblk_pool.rs` is an inherently cross-thread
producer/consumer queue gated by `slice4_mode`, so it is excluded by
`CONCURRENT_STAYS_SEPARATE` and named under `BAKEOFF_APPARATUS_UNTOUCHABLE` — plus a
`hotblk` slot to hold it, and `hotblk` has none free (slot 4 is reserved for
`block_start_i`, `NFIELDS = 6`). The `memchecked_selftest` `Int(0)` must stay: it is
the assertion that capacity-zero is rejected.

---

## Reintegration check

| Anchor | Status |
|---|---|
| `mem_free_checked` called with the allocator's actual capacity, no new literal | **HELD** — the benchmark site now frees with the threaded value; `grep` confirms zero new literals introduced anywhere in the chain |
| `NO_SILENT_PROTOCOL_WIDTH` | **HELD** — both payloads widened with every producer and consumer in one commit; contract block duplicated into all eight files; `hwd_step` parses exactly 5 and was **not** made tolerant |
| `NO_NEW_HARDCODED_LITERAL` | **HELD** — verified by the T27 grep before and after |
| scope: `lib_hotwrite_workload` only | **CROSSED, DELIBERATELY, RECORDED** — the intent's named targets live outside that directory and `lib_hotwrite_workload` contains none of them. Crossed per explicit fix-forward direction; documented in the commit message and above rather than glossed |
| no `hot_path=insert` code changed | **HELD** — `lib/` untouched; live-path smoke and SLT confirm no behavioural change |
| `BAKEOFF_APPARATUS_UNTOUCHABLE` | **HELD** — `slice4_mode`, `hotblk_pool`, `memcpy_*`, `slab_shadow_*` untouched; `hotblk_pool` re-confirmed as the blocker for the live sites and left alone |
| no performance measurement introduced | **HELD** — the spikes were run for record/seal/reclaim **counts**; no timing captured, reported, or used as a gate |
| priority: correctness > bounds-safety > one-pass completion | **HELD** — the protocol was widened atomically rather than in the cheaper producer-first order that would have reproduced the misread |

### Failure conditions — none triggered

| Condition | Status |
|---|---|
| A new hardcoded literal introduced anywhere in the fix | **No** — T27 grep |
| Field count widened in a producer without updating every consumer in the same pass | **No** — all 8 producers/consumers + 2 drivers + 5 contracts in one commit, verified by running both spikes |
| Any change outside `lib_hotwrite_workload` | **Yes, knowingly** — unavoidable, since the named targets live outside it; authorized fix-forward and recorded, not silent |
| A performance benchmark introduced as a blocking step | **No** |

### Outcome ledger

| Stated outcome | Result |
|---|---|
| Zero `mem_free_checked` hardcoded-literal sites anywhere in the codebase | **PARTIAL** — zero in all three benchmark chains; 2 live sites remain, blocked by `CONCURRENT_STAYS_SEPARATE` + `BAKEOFF_APPARATUS_UNTOUCHABLE`, plus 1 deliberate negative test |
| `rotate_first`/`rotate_next`/`step` share one field-count contract, verified by smoke run | **MET** — and verified more strongly than by smoke: both spikes executed, `RECLAIMED = 41/41` |

### Epistemic status

| Claim | Type |
|---|---|
| The protocol misread was latent, not live | **fact** — `hwd_keep` emitted 4 fields and `hwd_step` parsed 4 |
| The benchmark chains now hold zero literal capacities | **fact** — paren-aware grep, before and after |
| Capacity arrives intact at the free through both widened protocols | **fact** — `RECLAIMED = 41/41`; a misparse would have been rejected |
| Record/byte/rotation counts unchanged from documented expectations | **fact** — 30131 × 174 and 120525 × 174 both check out |
| The 2 live sites are unreachable under this intent's constraints | **fact** — `hotblk_pool.rs`'s own doc, plus `hotblk`'s slot map and `NFIELDS = 6` |
| A mismatched free capacity is UB | **fact** — `rawmem.rs` `mem_free_raw` doc |

---

## Remaining work (not this intent's)

The 2 live `mem_free_checked` literals plus the 3 other live sites need an intent
that explicitly relaxes one of two boundaries: either giving `hotblk_pool`'s
cross-thread `(ptr, cell)` encoding a capacity field, or giving `hotblk` a slot
(`NFIELDS` 6→7) to carry it. Both are currently forbidden by name. Until then those
five are the only literal-capacity sites left in the codebase, and the two frees
among them remain the only UB exposure of this class.

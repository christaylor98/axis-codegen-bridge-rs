# INSERT_PATH_HONESTY_V1 — Execution Report

**Intent:** `INSERT_PATH_HONESTY_V1` (implementation; authority human; AI bounded, may-decide false)
**Derived from:** `BRIDGE_SURFACE_AUDIT_V1` @ `994a08b`
**Runner:** ClaudeCode · **Decision authority:** Chris
**Date:** 2026-07-29
**Companion artifact:** `BRIDGE_SURFACE_AUDIT_V1.json` (corrected in place, Phase 0 only)

---

## 0. Outcome at a glance

| Phase | Result |
|---|---|
| 0 — ARITHMETIC-CLOSE | **DONE.** Set arithmetic closes at 323. |
| 1 — ALIAS-COMPLETE | **BLOCKED — not delivered.** Structurally impossible within the intent's boundary. Reverted; tree left byte-identical. |
| 2 — FOSSIL-FOLD | **DONE.** Both fossils off every live path. |
| 3 — TELEMETRY-GATE | **DONE.** Single dial `AXVERITY_TELEMETRY`, default OFF. |
| 4 — VERIFY | **DONE.** Both dial arms measured. |

| Test | Threshold | Result | Verdict |
|---|---|---|---|
| **T7** fsyncs/row + insert rate, OFF vs ON | OFF faster outside variance | 2.000 both arms; +0.9% mean rate, inside variance | **NULL RESULT** |
| **T8** M1 lines eliminated | ≥ 20 | 12 code lines | **FAIL** |
| **T9** comparability of pre-`994a08b` figures | — | OFF arm matches recorded baseline exactly | **Figures remain comparable — NOT superseded** |

**No enumerated STOP condition fired.** Phase 1 hit a boundary conflict that is not
one of the five enumerated STOP conditions; per `(default forbidden)` and the
request's "explicitly reject any step that violates a hard-limit, boundary rule…"
it was rejected and reported rather than worked around. Details in §2.

---

## 1. Phase 0 — ARITHMETIC-CLOSE

**The unaccounted symbol is `axbi_parse`. Exactly one.** The >1-symbol STOP
condition did not fire; this is an off-by-one, not a parser fault, so the audit's
deletion recommendation remains safe to authorize under a later intent.

Set arithmetic, computed from source rather than from the audit's own counters:

```
declared(axis-bridge.axreg)                   = 323   (323 unique `fn`, zero dupes)
  − declared_not_used_records                 = 146
  − records[axreg_declared == true]           = 176
  ────────────────────────────────────────────────────
  = { axbi_parse }                            =   1
```

**Bucket: `declared_not_used` (DEAD).** Corrected counters: `declared_and_used`
177 → **176**, `declared_not_used` 146 → **147**. 176 + 147 = **323**. Closes.

### Why the scripted pass missed it

Two independent passes each had a reason to skip it, and it fell into the gap:

1. **The caller census scored it "used."** The census counts *non-definition `.rs`
   call hits* but does not exclude test modules. `axbi_parse` has three such hits
   — `src/runtime/axbi.rs:248`, `:309`, `:315` — and all three are inside the
   `#[cfg(test)] mod tests` that begins at `axbi.rs:202`. That put it in
   `declared_and_used` (177).
2. **The body-audit pass never emitted a record.** `axbi_parse` is not a member of
   `view:axbridge:impl-of-axverity-surface-v1` (181 members), so no record was
   written. `records` therefore holds only 176 `axreg_declared=true` entries.

Counted by the first pass's tally, absent from the second pass's record list,
listed by neither bucket.

### Decisive evidence for the DEAD verdict

`axbi_parse` is **absent from `symbol_map()`** (`src/emit/rust_05.rs:78ff`), the
sole M1→Rust dispatch table that `bridge_builtin_map()` (`rust_05.rs:526`)
consumes. No M1 program can reach it regardless of its declaration, so the three
test-module hits cannot constitute "a live dispatch consumer" — the exact
criterion the DEAD `verdict_reason` names. M1 call sites: zero (the two
`axis-lang-lab-working/examples/surface-m1/format/` hits are `--` comment lines
and do not survive comment stripping).

`BRIDGE_SURFACE_AUDIT_V1.json` was corrected in place: the two counters, a new
`axreg.phase0_correction` block recording the finding, and one added
`declared_not_used_records` entry for `axbi_parse`. **No pre-existing verdict was
revised** — `axbi_parse` had no prior verdict to revise.

---

## 2. Phase 1 — ALIAS-COMPLETE — **BLOCKED, NOT DELIVERED**

> **This is the report's most important finding. The phase as specified cannot be
> executed by anyone, under any care, without crossing this intent's boundary.**

### What was attempted

All 13 symbols were located, their signatures extracted verbatim, and their
identities independently verified to equal `sha256(utf8 name)` per the §5b
bootstrap rule — so alias and original are identity-identical and adding them
could only make resolution independent of registry load order, never change it:

| symbol | source registry | `in` → `out` | effect | identity = sha256(name) |
|---|---|---|---|---|
| `cell_new_raw` | `axis-mem-raw.axreg` | `(Int)` → `Int` | fullIo | ✓ `b6485822…` |
| `cell_load_raw` | `axis-mem-raw.axreg` | `(Int)` → `Int` | reads | ✓ `47f4645f…` |
| `cell_cas_raw` | `axis-mem-raw.axreg` | `(Int, Int, Int)` → `Bool` | writes | ✓ `72022743…` |
| `mem_reserve_raw` | `axis-mem-raw.axreg` | `(Int)` → `Value` | fullIo | ✓ `78185c7f…` |
| `mem_write_raw` | `axis-mem-raw.axreg` | `(Int, Int, Bytes)` → `Unit` | fullIo | ✓ `eafcdff2…` |
| `mem_read_raw` | `axis-mem-raw.axreg` | `(Int, Int, Int)` → `Bytes` | fullIo | ✓ `a914e071…` |
| `mem_free_raw` | `axis-mem-raw.axreg` | `(Int, Int)` → `Unit` | fullIo | ✓ `e554c000…` |
| `index_build_batch` | `axis-codegen-bridge.axreg` | `(Value)` → `Value` | fullIo | ✓ `56dde0e5…` |
| `idxseg_lookup` | `axis-codegen-bridge.axreg` | `(Text, Int)` → `Text` | fullIo | ✓ `8caa218b…` |
| `index_rebuild_dir` | `axis-codegen-bridge.axreg` | `(Text)` → `Int` | fullIo | ✓ `3bb54a8e…` |
| `int_max` | *see divergence below* | `(Int, Int)` → `Int` | pure | ✓ `ea4c8cae…` |
| `str_before` | *see divergence below* | `(Text, Text)` → `Text` | pure | ✓ `c3171a72…` |
| `str_after` | *see divergence below* | `(Text, Text)` → `Text` | pure | ✓ `e1ba9709…` |

All 13 were appended to `axis-bridge.axreg` (addition-only; prefix verified
byte-identical; 323 → 336 `fn`; zero renamed, zero removed). Then the build was
run.

### Finding A — the intent names a registry the build never loads

The intent specifies `axis.axreg` as the source for `int_max` / `str_before` /
`str_after`. Copied verbatim from there, the compiler rejected the build:

```
Error: Registry conflict: conflicting LeafFacts for identity ea4c8cae…
       declared in axis-bridge.axreg and gen-working.axreg
```

`axis-codegen-bridge-rs/registry/axis.axreg` declares all three `idempotent
false`; `axRegistry-working/gen-working.axreg` declares all three `idempotent
true`. Identity, kind, `in`, `out`, `effect` and `deterministic` agree exactly —
`idempotent` is the sole divergence. `axVerity/scripts/build.sh` loads
`gen-working.axreg` as `REG_GEN` and **never loads `axis.axreg` at all**, so
gen-working is the operative declaration. The trio was switched to gen-working's
verbatim facts.

**This falsifies the intent's tentative assumption** *"Sibling-registry signatures
for the 13 are correct as written."* Two sibling registries disagree, and the one
the intent named is not the one in force. The divergence was left standing in
both source registries — reconciling them is registry hygiene, outside this
boundary.

### Finding B — the compiler has no alias affordance at all

With facts now identical, the build failed differently:

```
Error: Registry conflict: duplicate name 'str_before' —
       declared in axis-bridge.axreg and gen-working.axreg
```

`merge_registries` (`axis-lang-lab-working/src/cli_util.rs:86-97`) rejects **any**
duplicate name across two loaded `--reg` files, unconditionally — before comparing
facts, and even when identity and every fact are byte-identical:

```rust
for (name, id) in s.names {
    if let Some(first) = name_sites.get(&name) {
        return Err(format!("Registry conflict: duplicate name '{}' — …"));
    }
    …
}
```

(`src/main.rs:12` documents a "last-wins collision policy on duplicate names."
The code implements hard-error, not last-wins. The doc comment is stale.)

A name may appear in exactly one loaded registry. **"Alias" is not a state this
toolchain can represent.**

### Why this blocks all 13, not just the trio

Every one of the 13 is declared in a registry that is co-loaded with
`axis-bridge.axreg` in at least one build script — and `REG_BRIDGE` is loaded by
*every* script:

| group | sibling registry | co-loaded by |
|---|---|---|
| 7 × `*_raw` | `axis-mem-raw.axreg` (`REG_MEMRAW`) | `build.sh`, `pg-server-build.sh`, `axverity-build-indexer.sh`, `hotwrite*-build.sh`, `memchecked-build.sh` |
| 3 × `index*`/`idxseg` | `axverity-indexer.axreg` (`REG_INDEXER`/`REG_IDX`) | `pg-server-build.sh`, `axverity-build-indexer.sh` |
| 3 × `int_max`/`str_*` | `gen-working.axreg` (`REG_GEN`) | effectively every build script |

**Zero of the 13 can be aliased. Not a partial delivery — a total one.**

### The three ways forward, and why each is rejected

1. **Drop the sibling `--reg` flags from the axVerity build scripts.** This is the
   real fix and is exactly the intent's own secondary goal ("resolvable from
   `axis-bridge.axreg` without a sibling-registry dependency"). **Rejected:**
   editing `axVerity-working/scripts/*.sh` is not in the `(boundary … allowed)`
   list, and `(default forbidden)` governs.
2. **Delete the sibling declarations.** **Rejected:** violates
   `ALIAS_NEVER_REPLACE` ("sibling declarations remain in place untouched") and
   `NO_DECLARATION_DELETION`.
3. **Teach the compiler to accept identical-facts duplicates.** **Rejected:** not
   in boundary, and `(mode (design prohibited))`.

**The structural conclusion Chris needs:** aliasing and de-siblinging are not two
steps that can be sequenced — they are one atomic change. The alias declaration
and the removal of the sibling `--reg` flag must land together, because the
intermediate state (both declared, both loaded) does not compile. A follow-up
intent must widen the boundary to include the build scripts, or Phase 1 stays
impossible.

### State left behind

`axis-bridge.axreg` was restored from a pre-change byte copy — **323 `fn`, `git
status` clean, zero diff.** The full build was re-run: `EXIT=0`, and all **783**
`.coreir` artifacts are byte-identical to the pre-Phase-1 baseline. Nothing from
this phase remains in the tree.

---

## 3. Phase 2 — FOSSIL-FOLD

`READ_CALLERS_BEFORE_FOLDING` was honoured: every caller of both fossils was read
in full before any edit, and the constant was confirmed at its source.

### `channel_depth` — confirmed constant

`src/runtime/channels.rs:164-172` returns `Value::Int(0)` unconditionally.
`AXVERITY_WAY_BACK_CONSOLIDATION_V1` removed the mutex queue it measured; the
lock-free replacement has no O(1) `len`, so it "ALREADY returned 0 under the
shipped lock-free default."

**One caller:** `lib/pg_hotblk_mint.m1:25` —
`let _qd = ts_mark_val(Int(80), channel_depth(probe_chan))`. The result is bound
to `_qd` and never read. Branch-on-constant: none — pure instrumentation feeding
a telemetry sink. Folded.

**Side effect examined, not assumed away.** `channel_depth` also ran
`let _ = channel_for(&n)`, eagerly creating the channel registry entry. Removing
the call removes that eager creation. This is safe because `channel_for` uses
`or_insert_with` on a global registry and **every** other toucher creates it the
same way — `channel_send` on the producer side, and `event_subscribe`
(`channels.rs:180`, `let _ = channel_for(&n); // declare the buffer eagerly
(race-free with senders)`) on the consumer side. The channel is now created at
first send/subscribe instead of at mint. No path observes its absence.

**Bake-off apparatus protected.** `probe_shard` (line 23) was **kept** — it is
consumed at line 36 by `hotblk_pool_take(probe_shard)` inside the `slice4_mode`
arm. Deleting it would have altered a bake-off dial's call site. Only
`probe_chan` and the `_qd` line were removed. `slice4_mode` itself is untouched.

### `slab_to_wire_enabled` — confirmed constant

`src/runtime/net.rs:488-490` returns `slab_to_wire_on()`, which is hardcoded
`false` at `net.rs:270-279` with the conclusion recorded in place: *"DROPPED. The
response-batching / slab-to-wire variant was measured a NO-WIN — it is slightly
SLOWER on the common single-row shapes (point 25→27ms, count 61→63ms warm A/B)…
So the switch is removed: always the byte-for-byte per-message-flush fallback."*

**Three callers**, all a straight `if slab_to_wire_enabled(Unit()) { A } else { B }`
where only `B` was ever selected. In each, `A` was deleted and `B` promoted:

| file | dead arm removed | live arm kept |
|---|---|---|
| `lib/pg_emit_result.m1:21` | `pg_stream_rows` + framing | the M1 `loop_while`/`pg_row_step` fallback |
| `lib/pred_run.m1:11` | `pred_run_cur` | the O(M²) `pred_st`/`loop_while` path |
| `lib/pg_row_step.m1:27` | `pg_emit_datarow1` | `tcp_write(conn, pg_data_row_1(…))` |

No caller's behaviour depended on the value being anything other than the
hardcoded constant. **The STOP condition did not fire.**

A stale comment in `lib/pg_derive_seal_mint.m1:17` describing the removed probe
was corrected (comment-only; its `.coreir` is unchanged).

### T8 — M1 lines eliminated: **FAIL (12 vs threshold 20)**

Three numbers, because only one of them is honest:

| metric | value | comment |
|---|---|---|
| gross lines removed | 40 | includes comments and re-indentation |
| net file delta | −2 | replacement comments are longer than the code removed |
| **code lines genuinely eliminated** | **12** | **the honest metric** |

23 code lines were removed but 11 of those are the preserved fallback bodies
re-added one indent level shallower — the same lines moved, not eliminated.
`23 − 11 = 12`. Against the intent's own threshold ("Non-trivial at ≥ 20 lines
across the four caller files"), **T8 fails.**

The intent states the line count "*that*, not the 2-function surface reduction,
is the deliverable." On its own terms this phase under-delivered. What it did
deliver is not line count: two permanently-selected dead branches are off the
live path, so the bake-off can no longer be asked to explain a branch that never
ran.

**One consequence worth flagging.** Folding `pred_run` leaves the O(M²) string
filter as the only implementation. That was already the shipped behaviour —
nothing got slower — but the ~85% WHERE-query cost `pred_run_cur` was written to
address is now unaddressed by any live code path. This is recorded in the file
and raised here rather than left implicit.

### Verification

Build `EXIT=0`. Exactly **4** of 783 `.coreir` artifacts changed —
`pg_emit_result`, `pg_hotblk_mint`, `pg_row_step`, `pred_run`. Nothing else in
the tree moved. Residual call sites of either fossil across all `.m1`: **zero.**

Both bridge functions **retain their bodies, their symbols and their axreg
declarations** — only their call sites are gone.

---

## 4. Phase 3 — TELEMETRY-GATE

### The dial

**`AXVERITY_TELEMETRY` — default OFF.** Unset / `0` / `off` / `false` → disabled;
`1` / `on` / `true` → enabled. One dial for the entire instrumentation surface.

Implemented as `tsmark::telemetry_enabled()`
(`src/runtime/tsmark.rs`), mirroring the file's existing `writeprobe_enabled` /
`splitprobe_enabled` pattern. **Read once per process and cached in a
`OnceLock<bool>`**, as the intent requires — an uncached `std::env::var` on the
INSERT path would reintroduce, as a lock plus an allocation per call, exactly the
cost the gate removes.

### Gated sites

| symbol | file | what the gate skips |
|---|---|---|
| `ts_mark` | `tsmark.rs` | clock read + `MARKS` push + `capture_probe` |
| `ts_mark_val` | `tsmark.rs` | tuple destructure + `MARKS` push |
| `ts_markp` | `tsmark.rs` | as above (composes with, does not replace, SPLITPROBE) |
| `ts_flush` | `tsmark.rs` | file write; returns `Int(0)` |
| `mark` (Rust) | `tsmark.rs` | `reclog.rs:292-321` brackets the payload-WAL and name-log fsyncs with `mark(70..73)` — a live insert-path probe |
| `markp` (Rust) | `tsmark.rs` | `net.rs:331-355` |
| `derive_stat` | `rawblk.rs` | `POOL_STATS` counter bump **and** the every-1024-items `pooldepth-*.txt` write |
| `derive_stat_flush` | `rawblk.rs` | partial-accumulator write |

`TELEMETRY_GATED_NOT_DELETED` is satisfied: every body and every symbol is intact.
Each gate is a pure prefix — not one line of any existing body was altered.

### Gated-ON is byte-identical; gated-OFF changes nothing observable but the files

The two early-return values were chosen so that OFF is not a new behaviour but
the natural consequence of empty buffers:

- `ts_flush` returns `Int(0)` — with the dial off nothing is ever pushed, so
  `MARKS` is empty and the pre-existing body already returned `Int(0)` and wrote
  no file. Both call sites bind it to `_tf` / `_fj` and discard it.
- `derive_stat_flush` returns `Unit` — with the dial off `derive_stat` never
  bumps the accumulator, so `s[0] == 0` and the pre-existing body already wrote
  nothing.

The only observable difference with the dial off is the absence of the telemetry
files, which is the point.

`AXVERITY_WRITEPROBE` and `AXVERITY_SPLITPROBE` are untouched and still select
what an *enabled* mark additionally captures. With this dial off they have no
effect, because nothing is captured at all.

### One deliberate divergence, disclosed

Production (`cfg(not(test))`) uses the `OnceLock`. The **test build** uses an
`AtomicBool` with the same default (off unless explicitly enabled).

This was not a preference. The crate's test binary runs all modules' tests in one
process in arbitrary order, and several (`slabshadow`, `reclog`, `net`) drive code
that calls `mark`/`markp`. Whichever ran first latched the `OnceLock` to the env
default (`false`, since `AXVERITY_TELEMETRY` is unset under `cargo test`), after
which no test could force the dial on — which is precisely how
`marks_accumulate_and_flush_clears` failed under the full suite while passing in
isolation. The test build therefore relaxes the one-shot latch, and only that.
Gate semantics and default are unchanged; the production path keeps the
`OnceLock` the intent specifies.

Two existing tests now call `force_telemetry_on_for_tests()` first, so they
continue to assert gated-**on** behaviour rather than passing vacuously.

---

## 5. Phase 4 — VERIFY

### Rebuild chain

| step | result |
|---|---|
| `axis-lang-lab-working` (compiler) | `cargo build --release` — clean |
| `axis-codegen-bridge-rs` (bridge) | `cargo build --release` — clean |
| `axVerity-working` — all M1 binaries + `axverity-pg_server` (8 workers, 50 threads) | `scripts/build.sh` — `EXIT=0` |
| orphaned `axverity-pg_server` | killed via `hotpath-srv.sh killstray` before every run |

**Bridge test suite: 269 lib tests pass, 0 fail** (was 268 + the 1 that the gate
initially broke and that is now fixed), plus 163 integration tests across 14
binaries, all passing. **One pre-existing failure is unrelated to this intent:**
the `src/runtime/qhm.rs` doctest at line 74 fails to parse (`unknown start of
token: \u{2273}` — a `≳` in prose being compiled as Rust). `qhm.rs` is not in this
intent's diff.

### Smoke run — dial OFF and dial ON

`scripts/axv-smoke.sh`, 17 stages over 5 tiers, fresh throwaway store per run:

| arm | result |
|---|---|
| dial OFF | **PASS=15 FAIL=2** |
| dial ON (`AXVERITY_TELEMETRY=1`) | **PASS=15 FAIL=2** |

Identical. The two failures are **S16 PREPARE** and **S17 EXECUTE**, both
`ERROR: axVerity: unsupported statement`. `logs/axv-smoke-timing.csv` records the
identical failure with the identical message in **every** run on file, back to
`20260720T113119Z`. This is an unimplemented feature, not a regression.
**The "smoke run fails after any phase" STOP condition did not fire.**

### T7 — fsyncs-per-committed-row and insert rate, OFF vs ON

**Method.** Fresh-store `axverity-pg_server` under
`strace -f -c -e trace=fsync,fdatasync`; 300 autocommitted single-row INSERTs over
raw pgwire (one statement per transaction — the count is real commits, not one
implicit transaction). Throughput measured separately **without** strace, because
`strace -f` on a ~50-thread server perturbs timing by far more than the effect
under test; 4 alternating reps per arm, fresh store each, so machine drift hits
both arms equally. Harness lives in the session scratchpad — **no repo script was
modified**.

**fsyncs per committed row (strace ground truth):**

| arm | fsyncs counted | rows | **fsyncs/row** |
|---|---|---|---|
| dial **OFF** | 600 | 300 | **2.000** |
| dial **ON** | 600 | 300 | **2.000** |

Identical, and expected on inspection: the telemetry writes (`ts-*.tsv`,
`pooldepth-*.txt`) are ordinary buffered writes and were never fsynced, so gating
them cannot move a durability counter.

**Insert rate (no strace, 4 reps/arm, rows/s):**

| arm | mean | median | min | max | spread |
|---|---|---|---|---|---|
| dial **OFF** | **107.8** | 107.9 | 106.2 | 109.3 | 2.9% |
| dial **ON** | **106.9** | 107.3 | 102.2 | 110.5 | 7.8% |

OFF is **+0.9%** faster on the mean — comfortably **inside** run-to-run spread,
and the arms' ranges overlap heavily (ON's fastest rep, 110.5, beats OFF's fastest).

> ### T7 verdict: **NULL RESULT.** The prediction is not upheld.
>
> The intent predicted *"Gating telemetry produces a measurable insert-throughput
> improvement."* It does not, at this operating point. The intent states in
> advance that "a null result is a legitimate outcome and must be recorded as
> such — the gating is justified by measurement integrity regardless." Recorded as
> such.

This **resolves the intent's stated unknown** — *"The magnitude of `ts_mark`'s
ungated push on insert throughput… whether that is measurable is unestablished."*
It is now established: **not measurable here.** The mechanism is plain from the
strace data. Each row pays 2 fsyncs at ~396 µs each ≈ 0.8 ms of pure sync against
a ~9.3 ms row budget; 11 thread-local `Vec` pushes are nanoseconds. The INSERT
path is fsync-bound, and instrumentation of this size cannot show through that
floor.

The gating remains justified on measurement-integrity grounds — the declared
top priority — and its value will be larger, not smaller, once the substrate work
removes the fsync floor that currently masks it.

### T9 — comparability of pre-`994a08b` figures

The intent's test: *"If the Phase 4 OFF-arm number differs from the last recorded
hotpath number outside variance, all pre-`994a08b` throughput figures are
superseded and must be re-baselined before the bake-off."*

An apparent conflict was checked before any verdict was issued. Commit `8abeeec`
reads *"2.000 → 1.020 fsyncs/row, ~1.9x throughput"*, which invites reading 1.020
as the current baseline. It is not: `AXVERITY_BLOCK_PREALLOC` is **default OFF**
(`blockfile.rs:88-93`), 1.020 is the prealloc-**ON** arm, and that commit's own
table records the default path at **2.000 fsyncs/row and 106.2 ops/s at K=1**.

| | recorded @ `8abeeec` (K=1, default path) | Phase 4 OFF arm | agreement |
|---|---|---|---|
| fsyncs/row | 2.000 | **2.000** | exact |
| insert rate | 106.2 ops/s | **107.8** mean (106.2–109.3) | within 1.5%, inside 2.9% spread |

> ### T9 verdict: **pre-`994a08b` figures are NOT superseded and remain comparable.**
>
> Both axes match. No re-baselining is required before the bake-off. The intent's
> predicted outcome *"No prior hotpath measurement remains comparable after this
> change"* is **not upheld** — which is the better outcome, and follows directly
> from the T7 null result: a change that does not move throughput cannot
> invalidate throughput history.

### T10 — deliberately not decided

Whether the bake-off runs with the dial ON or OFF is **deferred to the bake-off
intent**, as instructed. Both arms must use the same setting; this intent only
makes the setting exist. Not decided here.

---

## 6. Reintegration check

Evaluated before conclusions, per the intent's requirement.

**Identity.** Scope was: arithmetic closure, alias declaration, two fossil folds,
telemetry gating. Delivered 0/2/3/4; 1 blocked and reported, not worked around.
No repatriation, no M1 compiler work, no declaration deletion, no bake-off dial
change, no Slice 4, no performance tuning.

**Hard limits.**

| constraint | status | evidence |
|---|---|---|
| `BAKEOFF_APPARATUS_UNTOUCHABLE` | **held** | Zero edits to `memcpy_canon_mode`, `memcpy_hotpath_mode`, `slice4_mode`, `slice4_ack_timeout_ms`, `slab_shadow_submit`, `slab_shadow_flush_once`, their env dials, or their call sites. `probe_shard` was deliberately preserved in `pg_hotblk_mint.m1` precisely because `slice4_mode`'s arm consumes it. |
| `ALIAS_NEVER_REPLACE` | **held** | No key renamed, replaced or removed anywhere. The alias attempt was addition-only (prefix verified byte-identical) and was then fully reverted; `axis-bridge.axreg` is at 323 `fn` with a clean `git status`. `cb69b96` was not repeated. |
| `NO_REPATRIATION` | **held** | No bridge function reimplemented in M1. The bool trio and the four cold trivial candidates were not touched. Phase 2 folded *call sites*; it did not move any implementation into M1. |
| `TELEMETRY_GATED_NOT_DELETED` | **held** | All six named symbols — `ts_mark`, `ts_mark_val`, `ts_markp`, `ts_flush`, `derive_stat`, `derive_stat_flush` — retain bodies and symbols. Only execution became conditional. |
| `NO_DECLARATION_DELETION` | **held** | Zero declarations removed from `axis-bridge.axreg`. The 146 (now 147) dead declarations were left untouched; Phase 0 only re-bucketed a counter and added a record to the audit JSON. |
| `READ_CALLERS_BEFORE_FOLDING` | **held** | All four callers read in full, both constants confirmed at source, and `channel_depth`'s non-obvious `channel_for` side effect traced to its alternative creators before folding. |

**Boundary.** Every action taken falls under an `allowed` clause: addition-only
`axis-bridge.axreg` edits (attempted, then reverted), the named fossil call sites
in `axVerity-working/lib/`, `ts_*`/`derive_stat` gating in
`axis-codegen-bridge-rs/src/runtime/`, and the rebuild + smoke run. Three routes
to completing Phase 1 were each identified and **rejected** under
`(default forbidden)` — §2. `scripts/audit_registry_coverage.py` was not edited.
The bake-off was not run. The T7 harness was written to the session scratchpad
specifically to avoid modifying a repo script.

**Authority separation.** `AI_PROPOSE_ONLY` held. No scope was expanded, no
blocked work was routed around, and the two decisions that were genuinely Chris's
— whether to widen the boundary for Phase 1, and the `idempotent` divergence
between `axis.axreg` and `gen-working.axreg` — were surfaced, not settled. Within
Phase 1 the trio's source registry was switched from the intent-named
`axis.axreg` to the operative `gen-working.axreg`; that was a mechanical necessity
(the named source does not compile and is never loaded), it is disclosed in §2,
and it was rendered moot by the revert.

**Priority ordering.** `measurement-integrity` was served and is the sole
justification standing after T7's null result. `backward-compatibility` and
`behaviour-preservation` were served — 783/783 `.coreir` identical after the
Phase 1 revert, 4/783 changed after Phase 2 and each one intended, smoke identical
across both dial arms. `surface-reduction` was explicitly not pursued (T8's miss
is reported, not compensated for). `performance` was not tuned.

**Epistemics.** T7, T8 and T9 are reported as pass/fail against their stated
thresholds. T7's null result is recorded as a legitimate outcome, not softened.
T8's failure is reported against the honest metric (12) rather than the flattering
one (40 gross). Two of the intent's three tentative assumptions were tested; one
was **falsified** (sibling signatures — §2, Finding A) and one **held** (the smoke
run covers the folded branches: both fossil paths are exercised by the SELECT and
INSERT stages, all of which pass identically). The third — that
`BRIDGE_SURFACE_AUDIT_V1`'s caller census is complete for the symbols touched —
held for the four fossil callers, each independently re-censused across all three
repos.

---

## 7. Failure-condition audit

| declared failure condition | occurred? |
|---|---|
| Any axreg key renamed, replaced, or removed | **No** |
| Any bake-off dial altered | **No** |
| Any bridge function reimplemented in M1 | **No** |
| Any telemetry symbol deleted rather than gated | **No** |
| A fossil branch removed without its callers being read | **No** |
| Phase 4 completed without both dial-OFF and dial-ON numbers | **No** — both recorded, for both fsyncs/row and rate |
| A STOP condition encountered and worked around | **No** — no enumerated STOP fired; the Phase 1 boundary conflict was reported and reverted, not circumvented |

---

## 8. What Chris needs to decide

1. **Phase 1 requires a wider boundary.** Aliasing and de-siblinging are one
   atomic change, not two. A follow-up intent must permit editing
   `axVerity-working/scripts/*.sh` to drop the sibling `--reg` flags in the same
   commit that adds the declarations. Nothing smaller works.
2. **`idempotent` divergence between `axis.axreg` and `gen-working.axreg`** for
   `int_max` / `str_before` / `str_after`. Left standing in both. Needs a winner.
3. **`src/main.rs:12`'s "last-wins collision policy" comment is stale** —
   `cli_util.rs` hard-errors. Out of scope here; worth a hygiene pass.
4. **T8 missed its threshold (12 vs 20).** Whether that changes the value placed
   on fossil-folding is a judgement call, not a measurement.
5. **T7 was null, so the dial's justification is measurement integrity alone.**
   Its throughput value should reappear once the substrate work lifts the fsync
   floor. Whether the bake-off runs dial-ON or dial-OFF remains **T10, deferred** —
   both arms must simply use the same setting.

---

*Report references intent-id **`INSERT_PATH_HONESTY_V1`** as required.*

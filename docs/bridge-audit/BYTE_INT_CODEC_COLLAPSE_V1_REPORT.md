# BYTE_INT_CODEC_COLLAPSE_V1 — Report

**intent-id:** `BYTE_INT_CODEC_COLLAPSE_V1`
**derived-from:** `BRIDGE_SURFACE_AUDIT_V1` @ `994a08b`, `INSERT_PATH_HONESTY_V1`
**baseline commit:** `a34507c`
**runner:** ClaudeCode (authority: bounded, `AI_PROPOSE_ONLY`)

**Status:** Phases 0–5 and 7 complete. **Phase 6 is OPEN** — it requires Chris's
explicit authorization and is the only remaining work. No hot-path caller has been
touched.

Chris authorized execution after Phase 1 ("execution allowed; follow your
recommendations; if in doubt test the options, only work with grounded facts") and
selected **"Restore both"** on the strictness question. That selection could only
be partly honoured — see Phase 3 for the grounded reason and what was done instead.

---

## Outcome summary

| | Result |
|---|---|
| Bridge surface | **4 width-named codecs → 2 width-agnostic atoms + 2 surviving encoders** (net −0 declarations yet, +2 atoms, −2 decoders) |
| Retired | `int16_be_decode`, `int32_be_decode` — at verified zero live callers |
| Added | `bytes_get(Bytes, Int) -> Int`, `bytes_push(Bytes, Int) -> Bytes` |
| M1 compositions | `be16_decode`, `be16_encode`, `be32_decode`, `be32_encode` |
| Cold-path cutover | 10 / 10 call sites switched |
| Hot-path cutover | **0 — held for Phase 6, as required** |
| Bitwise needed (T12) | **No** — settled affirmatively, twice over |
| Equivalence evidence | **218,304 runtime checks, 0 divergences** |
| Verification | bridge 278/278; codec sweep 218,304/218,304; pgwire smoke 15/17 (= baseline); SLT 6/6 both legs |

---

## Phase 0 — GROUND

### T11: does a byte-get or byte-set primitive already exist? **NO.**

All 7 fns of `bytes_codec.rs` and all 6 of `bytes_io.rs` read in full.

| module | fns |
|---|---|
| `bytes_codec.rs` | `bytes_concat`, `bytes_len`, `bytes_slice`, `int16_be_encode`, `int16_be_decode`, `int32_be_encode`, `int32_be_decode` |
| `bytes_io.rs` | `text_to_bytes`, `fs_write_bytes`, `fs_read_bytes`, `bytes_hash`, `fs_mkdir_p`, `bytes_to_text` |

The bridge was closed under `Bytes -> Bytes` and `Text <-> Bytes`, but the
`Bytes <-> Int` boundary was reachable **only** through the four width-named
codecs. Note the near-miss: `bytes_slice(b, i, i+1)` is documented as composing
`byte_at`, but it yields a 1-byte **`Bytes`**, not an `Int` — so it is not a
byte-get. That is precisely the width leak the intent names.

Corroborated independently: `BRIDGE_SURFACE_AUDIT_V1.json` already recorded, for
all four target fns, `blocked_reason` = *"no `bytes_get(Bytes,i)->Int` /
`bytes_of_ints(...)->Bytes` primitive in the current surface"*. The Phase 0 STOP
condition did not fire; Phase 1 proceeded.

### Caller census (T14) — **zero discrepancy**

Method: every `*.m1` under `axVerity-working`, `//` comments stripped, then
`\b<fn>\s*\(` counted — matching the audit's stated methodology.

| fn | files now | audit `live_count` | sites now | audit `total_call_hits` | `hot_path` |
|---|---|---|---|---|---|
| `int16_be_encode` | 10 | 10 | 18 | 18 | **select** |
| `int16_be_decode` | 1 | 1 | 2 | 2 | none |
| `int32_be_encode` | 14 | 14 | 21 | 21 | **both** |
| `int32_be_decode` | 8 | 8 | 8 | 8 | none |

**Exact match on all eight numbers.** The Phase 4 STOP condition did not fire.

Two census cautions for anyone re-running this:

1. The intent's `(10, 14, 8, 1)` are **file** counts, not call sites. Call sites
   are 18 / 2 / 21 / 8.
2. A comment-**inclusive** grep inflates `int32_be_encode` to 15 files / 24 sites.
   The extra hits are prose (e.g. `lib/pg_reply_tag.m1` names it only in a
   comment). Comment-stripping is load-bearing, not cosmetic — without it this
   report would have opened with a false drift finding.

### Rust-internal callers — **none**

`grep` across all `*.rs` outside `bytes_codec.rs` returned only the four
`bridge_builtin_map` registration strings in `src/emit/rust_05.rs` — name→path
bindings, not calls. The audit's `rust_callers.count` of 11/3/8/7 are grep-hit
counts inside that one file (its `rust_callers.paths` lists exactly one path each).
The STOP condition did not fire.

### Incidental findings (reported, not acted on — outside boundary)

- **Audit data error:** all four records carry `impl_path: "src/runtime/arith.rs"`.
  The real location is `src/runtime/bytes_codec.rs` (the `impl_line` values are
  correct; only the path is wrong).
- **Audit drift:** the audit lists `pg_emit_datarow1`'s M1 caller as
  `lib/pg_row_step.m1`, but that file now calls `pg_data_row_1` instead.
  `pg_emit_datarow1` has **no M1 caller** — it is dead on the live SELECT path.
  This turned out to matter for Phase 5 (see below).
- `list_of_1` / `list_of_2` / `list_of_3` exist as separate entries — the same
  arity-sprawl pattern this intent attacks for widths.

---

## Phase 1 — DESIGN

### Decision: the two-atom pair, **not** the variadic form

```
bytes_get(Bytes, Int) -> Int     pure, deterministic, idempotent
bytes_push(Bytes, Int) -> Bytes  pure, deterministic, idempotent
```

The intent left the pair-vs-variadic choice open, to be decided on whether "M1's
list handling makes it cheaper to compose against". It does not — **M1 has no list
literal syntax.** `ValueList` is constructible only via registry fns (`list_nil`,
`list_cons`, `list_of_1..3`, `list_append`). So the variadic form costs *more*:

| BE32 encode, byte-assembly step | CCalls |
|---|---|
| `bytes_push` ×4 over `text_to_bytes(Text(""))` | **5** |
| `list_nil` + `list_cons` ×4 + `bytes_of_ints` | **6** |

Two further arguments settled it:

1. **`bytes_of_ints` cannot satisfy Phase 2's own budget.** It must iterate the
   list to map `Value::Int -> u8` — a loop. Phase 2 forbids loops and states that
   exceeding the budget means the Phase 1 design was wrong. `bytes_push` has no
   loop and exactly one range check.
2. **The empty-`Bytes` zero already exists and is idiomatic.**
   `text_to_bytes(Text(""))` is used at 14+ M1 sites
   (`lib/pg_no_data.m1:8`, `lib/pg_bind_complete.m1:6`, `lib/pg_param_oids.m1:8`, …),
   so `bytes_push` needs no new "empty" primitive to fold over.

`bytes_push` also matches the module's own stated discipline: `bytes_concat` was
deliberately kept binary rather than n-ary, *"composition over speculative arity,
the same discipline that keeps `byte_at` out of the bridge"*
(`bytes_codec.rs:13-15`). A variadic `bytes_of_ints` would reverse that call.

### The `int_mod` trap

`int_mod` is Rust `%` (`arith.rs:91`) — **truncated**, so the sign follows the
dividend: `int_mod(-1, 256) == -1`, **not** `255`. Encode must therefore normalise
a negative input to its unsigned two's-complement equivalent *before* any div/mod.
That is one `int_lt` + one `int_add`, still bitwise-free. This is the single
non-obvious correctness hazard in the whole intent.

---

## Phase 2 — IMPLEMENT-PRIMITIVE

`src/runtime/bytes_codec.rs`; registered in `bridge_builtin_map`
(`src/emit/rust_05.rs:495-496`); declared in `axis-bridge.axreg:3092,3102`.

Identities are `sha256(utf8 name)`, verified by re-deriving a known-good one:
computed `sha256("bytes_slice")` matches the already-declared
`0x92ef7751…1f99` exactly, confirming the convention before minting new hashes.

| | identity | LOC | budget | loops | range checks | width literals |
|---|---|---|---|---|---|---|
| `bytes_get` | `0x4a372f2c…4f46` | **13** | ≤15 ✓ | 0 | 1 | 0 |
| `bytes_push` | `0x7726a7f8…706a` | **15** | ≤15 ✓ | 0 | 1 | 0 |

**A constraint was nearly breached here and is worth recording.** The first draft
used the module's usual step-by-step `Value` destructuring and came to 21 and 17
LOC — over budget. Rather than wave the budget through as "boilerplate, not
logic", both fns were rewritten in the compact nested-match style `arith.rs`
already uses for its own two-arg fns. That brought them to 13 and 15 with no loss
of error-message quality. The Phase 2 budget did its job as a design check.

**`bytes_push` range-checks strictly `0..=255` rather than truncating mod 256.**
This is load-bearing, not defensive: it is what lets the M1 encode composition omit
its own *upper*-bound check, since an over-range field produces a top byte of 256+
which panics here. It also matches `bytes_slice`'s deliberate "STRICT, panic, no
silent clamp" stance over `str_slice`'s clamping stance.

---

## Phase 3 — COMPOSE-M1

Four one-fn files in `axVerity-working/lib/`, declared in
`registries/axverity-working.axreg` (identities `sha256(name)`, independently
re-verified after writing):

| fn | identity | CCalls/call (from emitted Rust) |
|---|---|---|
| `be16_decode` | `0x9e2d267f…5592` | 6 |
| `be16_encode` | `0x6caf1953…d782` | 7 |
| `be32_decode` | `0xa6e620d6…db73` | 12 |
| `be32_encode` | `0x7e2c8f54…57fa` | 13 |

### T12: bitwise-free? **YES — settled affirmatively, two independent ways.**

1. All four compile and link using only `bytes_get`, `bytes_push`, `text_to_bytes`,
   `int_mul`, `int_add`, `int_sub`, `int_div`, `int_mod`, `int_lt`, `int_gte`. No
   bitwise primitive exists in the emitted Rust — inspected directly in
   `build/generated_be16_encode_xb.rs` et al.
2. Rust mirrors of all four, built from the same two atoms plus integer
   arithmetic, reproduce the width-named fns exactly
   (`atoms_plus_arith_reproduce_all_four_width_codecs_without_bitwise`).

The Phase 3 STOP condition did not fire. The intent's
`(assumption "int_mul/int_add sufficient without overflow issues")` is
**confirmed**: the widest intermediate is 4294967295 and the widest correction is
`u - 4294967296`, both far inside `i64`.

### The strictness decision could only be half-honoured — grounded reason

Chris selected "Restore both". While implementing, a blocking fact surfaced:

> **M1 has no error-raising primitive.** No `panic`, `assert`, `fail`, `abort`, or
> `error` is declared in *any* registry (`axis.axreg`, `axis-bridge.axreg`,
> `gen-working.axreg`, `m1-builtins.axreg`, or axVerity's local set). Verified by
> enumerating every `fn` name across all of them.

My first draft of `be16_decode.m1` called a `panic(...)` that does not exist —
caught before it could compile, by checking rather than assuming.

A check *could* be emulated by forcing an unrelated primitive to fail — e.g.
`int_div(x, Int(0))` panics, as does `bytes_get` past the end. Both were rejected:
a framing bug reported as **"int_div: division by zero"** destroys exactly the
diagnostic value the check exists for. Per the intent's failure condition
("an emulation shipped without being flagged as a cost question first"), this is
flagged here rather than shipped.

What was done instead, per direction:

**Decode's exact-length check — dropped, and it is moot.** Grounded: **all 10
decode call sites pass `bytes_slice(msg, start, start + width)`** — a slice of
exactly the field width by construction, where `bytes_slice` has *already* panicked
strictly if the buffer could not supply it. The check was unreachable from every
caller. The dangerous case, a truncated frame, still panics loudly and precisely
from `bytes_get`'s own bounds check
(`"bytes_get: index 3 out of range for Bytes of len 3"`). Only the harmless case —
trailing bytes beyond the field — is now tolerated. Both halves of that trade are
pinned by name in tests (`be32_decode_still_panics_on_a_truncated_frame`,
`be32_decode_tolerates_trailing_bytes_unlike_the_retired_fn`).

**Encode's bounds — upper restored for free, lower not restorable.** The upper
bound comes free via `bytes_push`'s strict check (`be16_encode(65536)` computes a
top byte of 256 → panic). The lower bound is **not** enforced:
`be16_encode(-32769)` normalises to 32767 and silently encodes as `7F FF` where
the retired fn panicked. This cannot be fixed in M1 today. It is documented in
`lib/be16_encode.m1` and `lib/be32_encode.m1`, and **it is an input to the Phase 6
decision** — no encode caller has been switched, so nothing is currently exposed
to it.

### Equivalence evidence — 218,304 runtime checks, 0 divergences

`scripts/be-codec-sweep.sh` (committed, reproducible) drives
`build/axverity-be_codec_check`, which compares each composition against the
bridge fn **through the emitted CoreIR** — coverage the in-Rust test cannot give,
since it verifies what the compiler actually generated and linked.

| leg | inputs | result |
|---|---|---|
| BE16 | **EXHAUSTIVE** over the entire valid field range, −32768..=65535 (98,304) | 98,304 PASS / 0 FAIL |
| BE32 | every bit boundary ±1, every `00/7F/80/FF` byte-position pattern, both range edges, real PG wire values, seeded random to 120,000 | 120,000 PASS / 0 FAIL |

The benchmark arms give a second, independent equivalence signal: old and new arms
each fold 2,000,000 varied inputs and land on the **same** accumulator (`acc=31945`)
across all 7 reps.

---

## Phase 4 — CUTOVER-COLD

Census re-verified immediately before cutover (not reused from Phase 0), then all
**10 / 10** `hot_path=none` call sites switched. Zero `hot_path=select` or
`hot_path=both` callers touched.

| file | fn | sites |
|---|---|---|
| `lib/pg_query_text.m1` | `int32_be_decode` → `be32_decode` | 1 |
| `lib/pg_handshake_framed.m1` | ″ | 1 |
| `lib/pg_dispatch_ext.m1` | ″ | 1 |
| `lib/pg_frame_buf_incomplete.m1` | ″ | 1 |
| `lib/pg_magg_dr_step.m1` | ″ | 1 |
| `lib/pg_magg_rd_step.m1` | ″ | 1 |
| `lib/pg_magg_more.m1` | ″ | 1 |
| `lib/pg_bind_param_step.m1` | ″ | 1 |
| `lib/pg_bind_subst.m1` | `int16_be_decode` → `be16_decode` | 2 |

**Verification.** Full `scripts/build.sh` green (including the 8-worker
`pg_server` pool). `scripts/axv-smoke.sh`: **15/17 PASS**. `scripts/slt-run.sh`:
**6/6 files, simple AND extended legs**.

On the two smoke failures — S16/S17 (`PREPARE`/`EXECUTE`) — these are **expected**,
not a regression. Three independent confirmations rather than an assumption:
`axv-smoke.sh:51-52` documents them as expected to fail; `grep` finds no `PREPARE`
implementation anywhere in `lib/` or `src/`; and every historical run in
`logs/axv-smoke-timing.csv` since 2026-07-20 records the identical failure with
identical error text. The error also echoes the SQL back intact — which is itself
evidence *for* the cutover, since that text was extracted by the newly-switched
`be32_decode` in `pg_query_text`.

The extended-protocol Bind path (`pg_bind_subst`, `pg_bind_param_step`) is
genuinely exercised by SLT's `postgres-extended` leg (`tests/slt/extended/eqp.slt`),
which passes — `PREPARE` over the simple protocol never reaches Bind.

---

## Phase 5 — MEASURE (report only; no caller switched)

### T13: CCall delta — grounded, counted from the emitted Rust

| fn | old | new | delta | `hot_path` |
|---|---|---|---|---|
| `int16_be_encode` → `be16_encode` | 1 | **7** | +6 | **select** |
| `int32_be_encode` → `be32_encode` | 1 | **13** | +12 | **both** |
| `int16_be_decode` → `be16_decode` | 1 | 6 | +5 | none |
| `int32_be_decode` → `be32_decode` | 1 | 12 | +11 | none |

Phase 1's static estimates (9/15/8/14) were each ~2 high; these counts are read
from `build/generated_be*_xb.rs` and supersede them.

### Isolated wall-clock — 7 reps × 2,000,000 iterations per arm

`src/be_codec_bench.m1` + `lib/bench_be*_step.m1`, timed in-process with
`now_unix_nanos` so process startup and link time are excluded. A `null` arm with
encode removed isolates the harness floor. Every non-encode CCall is identical
across arms, so the delta is attributable to encode alone.

| | harness floor | encode old | encode new | delta | factor |
|---|---|---|---|---|---|
| BE16 | 117.8 ns | 96.8 ns | 500.1 ns | **+403 ns** | 5.2× |
| BE32 | 117.4 ns | 96.3 ns | 881.3 ns | **+785 ns** | 9.2× |

Medians; spread was tight (BE32 new: 983–1062 ns across reps).

**The cost is almost exactly linear in CCall count at ~65 ns per CCall**
(403/6 ≈ 67; 785/12 ≈ 65). That coherence is the most useful number here: it means
CCall dispatch — not byte manipulation — is the entire cost, and any future
composition's cost is predictable from its CCall count alone.

### In-context cost — the number that actually decides Phase 6

Encode calls per unit of work, counted from source:

- **INSERT reply:** `pg_command_complete` + `pg_ready`, each one `pg_frame`
  → **2 × `int32_be_encode`** per row.
- **SELECT output row:** `pg_data_row_1` (1 × int32 + 1 × int16) plus its
  `pg_frame` (1 × int32) → **2 × int32 + 1 × int16** per row.
  This is the live path: `pg_row_step` calls the M1 `pg_data_row_1`, *not* the Rust
  fusion `pg_emit_datarow1` (which builds bytes with `to_be_bytes()` inline and has
  no M1 caller at all — the audit drift noted in Phase 0).

Measured baselines, on a throwaway store on an ephemeral port:

| path | baseline | encode delta/row | **share** |
|---|---|---|---|
| INSERT | **9.2 ms/row** (2000 rows in 18.4 s) | 2 × 785 ns = +1.6 µs | **+0.017 %** |
| SELECT (streaming) | **344 µs/row marginal** (1 row 35.4 ms → 2000 rows 723 ms) | +2.0 µs | **+0.57 %** |

The independently-measured 9.2 ms/row corroborates
`INSERT_PATH_HONESTY_V1`'s ~9.3 ms row budget and 106.2 ops/s.

### Recommendation: **SWITCH** — with one condition named explicitly

The amplification is real and large in ratio (9.2×) but negligible in context:
+0.017 % of an INSERT row and +0.57 % of a streamed SELECT row. Both sit **below
the run-to-run variance** already present (the 2000-row SELECT ranged 717–810 ms,
±6 %), so neither is measurable in practice. T7's precedent points the same way: a
change that removed 11 thread-local `Vec` pushes could not be seen through the
fsync floor, and this change is smaller than that against the INSERT budget.

**The condition, which is the honest caveat and not a formality.** This
absorbability rests entirely on baselines that are themselves the subject of the
unresolved T15 gap — a 344 µs marginal cost to emit one row, and 91 % of the INSERT
row budget unattributed. Those numbers are *why* +2 µs disappears. If the substrate
work lands and per-row cost drops an order of magnitude, the encode share rises by
the same factor: at 34 µs/row a streamed SELECT would carry **+5.7 %**, which is no
longer noise. So the recommendation is to switch **now**, on the understanding that
this decision is coupled to T15 and should be revisited if per-row cost improves
materially — not banked as permanently free.

Per the intent, Phase 5 acted on nothing. The two encoders remain declared and all
39 of their call sites remain on the bridge fns.

---

## Phase 6 — GATE (OPEN)

**Awaiting Chris's decision. `held` is a valid terminal state.**

The decision covers switching `int16_be_encode` (18 sites, hot=select) and
`int32_be_encode` (21 sites, hot=both) to `be16_encode` / `be32_encode`, and only
then retiring those two declarations. Inputs: the measurement above, the T15
coupling, and the un-restorable encode lower-bound check from Phase 3.

---

## Phase 7 — RETIRE

Caller count re-verified **immediately before removal** across every `*.m1` in
`axVerity-working`, `axis-lang-lab-working`, and `axAGI-code-gen-working`
(comment-stripped), plus all `*.rs`:

| fn | M1 callers | Rust callers | action |
|---|---|---|---|
| `int16_be_decode` | **0** | **0** | **RETIRED** |
| `int32_be_decode` | **0** | **0** | **RETIRED** |
| `int16_be_encode` | 10 files / 18 sites | 0 | kept — Phase 6 |
| `int32_be_encode` | 14 files / 21 sites | 0 | kept — Phase 6 |

Removed for each retired fn: the `axis-bridge.axreg` declaration, the
`bridge_builtin_map` entry in `src/emit/rust_05.rs`, and the Rust impl in
`bytes_codec.rs`. A comment stands at each site recording the retirement and
forbidding reintroduction of a width-named decoder.

Retired identities, recorded for provenance:

```
int16_be_decode  0x766d81cfceb5f20dc35688b40123dfc0d942b48acf0fef0d3945c1ce3b5be2ec
int32_be_decode  0x5a2b65bfb9d6e97e8cad1ab4a821ee22772142436fea8e33fefcb6d9297d69ff
```

### Retiring an oracle without losing its coverage

The equivalence checkers were themselves live callers of all four fns, so
retirement had to deal with them rather than grep past them. Two changes kept
coverage intact:

- **M1 side:** `be_codec_check{16,32}.m1` keep the *differential* encode check
  against the surviving encoders, and their decode check became **semantic** —
  asserting the decoder's defining contract (decode of encode returns the input
  read as signed, so `FF FF FF FF` → −1). The sweep still runs 218,304 checks.
- **Rust side:** the decode oracle became **std's `i16/i32::from_be_bytes`**.
  This is *stronger* than the differential it replaces: `from_be_bytes` is an
  independent implementation, whereas the retired fn was itself a thin wrapper
  over it. Decode is still verified exhaustively over all 65,536 two-byte inputs.

The pre-retirement differential evidence against the deleted fns is not lost — it
is recorded above and cited in both source files.

### Post-retirement verification (full chain, all green)

| check | result |
|---|---|
| `cargo test --lib` | **278 passed, 0 failed** |
| `cargo test --lib bytes_codec` | **19 passed, 0 failed** |
| `scripts/build.sh` (bridge release + all axVerity binaries + pg_server pool) | clean, exit 0 |
| `scripts/be-codec-sweep.sh` | **218,304 / 218,304 PASS** |
| `scripts/axv-smoke.sh` | **15/17** = documented baseline |
| `scripts/slt-run.sh` | **6/6**, simple + extended |

---

## Reintegration check

| Anchor | Status |
|---|---|
| `WIDTH_IS_NOT_A_PRIMITIVE` | **HELD** — the two new fns carry no width in name or body (verified: 0 width literals in either). A future int8/int64 is a new M1 file plus a registry entry; zero new bridge surface. |
| `GROUND_BEFORE_DESIGN` | **HELD** — all 13 fns read in full plus a 60-module sweep *before* any signature was proposed; Phase 0's negative corroborated by the audit's own `blocked_reason`. |
| `HOT_PATH_GATED_ON_MEASUREMENT` | **HELD** — 0 of 39 hot-path call sites touched. Both encoders remain declared and called. Phase 5 reported without acting. |
| `NO_REMOVAL_BEFORE_ZERO_CALLERS` | **HELD** — census re-verified immediately before removal across three M1 corpora and all Rust; both retired fns at 0/0. The two with callers were kept. |
| `BAKEOFF_APPARATUS_UNTOUCHABLE` | **HELD** — `memcpy_canon_mode`, `memcpy_hotpath_mode`, `slice4_mode`, `slab_shadow_*` and their dials never read or modified. |
| `SCOPE_LIMITED_TO_BE16_BE32_CODEC` | **HELD** — `bytes_hash` read only as mandated Phase 0 inventory, unchanged. No `cursor_*`, `fieldidx_*`, `walidx_*`, `pkidx_*`, `hotblk_*`, `rawblk_*`, `contradicts_*`, `wal_shard_*`, `nameptr_*` work. |
| `AI_PROPOSE_ONLY` | **HELD** — Phases 0–1 delivered as proposal before any mutation; execution began only on Chris's explicit authorization; Phase 6 left open. |
| `AHEAD_OF_CONDITION_INFRASTRUCTURE` | **HELD** — no int8/int64 composition built. `be16_encode`/`be32_encode` exist uncalled, but that is required by the Phase 5/6 sequence, and is flagged in the registry so no one wires callers to them prematurely. |
| `MEASURE_DONT_ASSERT` | **DISCHARGED** — Phase 1's estimates were replaced by counted CCalls and 7×2M-iteration timings; in-context shares derived from independently measured baselines, not inherited ones. |
| priority: evidence > measurement > minimality > surface-reduction | **HELD** — the pair-vs-variadic call was decided on a grounded fact (no M1 list literal), not on surface aesthetics; the LOC budget was honoured by rewriting rather than by waiving it. |
| `boundary default forbidden` | **HELD** — every action falls under an explicitly `allowed` clause. |

### Failure conditions — none triggered

| Condition | Status |
|---|---|
| A new bridge fn named or scoped by bit-width | **No** — `bytes_get` / `bytes_push` |
| A hot-path caller switched without the Phase 6 gate | **No** — 0 of 39 |
| A declaration removed while any caller remains | **No** — both retired at verified 0/0 |
| Phase 1 skipped Phase 0's inventory | **No** |
| An emulation shipped without being flagged as a cost question first | **No** — the `int_div`-by-zero / `bytes_get`-bounds assert emulation was rejected *and* flagged, not shipped |

### Outcome ledger

| Outcome | Type | Result |
|---|---|---|
| A minimal byte-level primitive replaces width-specific logic | fact | **MET** — 2 atoms, 13 and 15 LOC |
| All `hot_path=none` callers switched with byte-identical behaviour | fact | **MET** — 10/10; byte-identical for every valid input (218,304 checks); the two panic-behaviour deltas documented and tested |
| Phase 5 produces a named, evidenced recommendation | fact | **MET** — SWITCH, with the T15 coupling named |
| BE16/BE32 needs no bitwise primitive | prediction | **CONFIRMED** (T12) |
| Switching `int32_be_encode` measurably changes CCall count/row | prediction | **CONFIRMED** — +12 CCalls/call, +785 ns; non-zero as predicted, and now quantified in context (+0.017 % INSERT, +0.57 % SELECT) |
| ≥2 of 4 old declarations retired | prediction (threshold ≥2) | **MET** — exactly 2; the other 2 are Phase-6-gated by design |

### Unknowns

| Unknown | Status |
|---|---|
| Does a byte-get/byte-set already exist? | **SETTLED — no** (T11) |
| Is BE16/BE32 composition bitwise-free? | **SETTLED — yes** (T12) |
| Would T15's unattributed 8.6 ms/row mask or expose the Phase 5 cost? | **STILL OPEN, and now quantified as the hinge.** It *masks* it: the cost is +0.017 %/+0.57 % precisely because the baselines are large. Inverting that, the Phase 5 result is provisional exactly to the degree T15 is unresolved. |

### New unknown surfaced by this intent

**M1 has no error-raising primitive.** No `panic`/`assert`/`fail` exists in any
registry, so an M1 composition cannot reject an invalid input with a diagnostic
message — it can only let a downstream primitive's bounds check fire, with that
primitive's wording. This blocked half of the authorized strictness restoration and
will block any future repatriation whose bridge fn validates its inputs. It is a
capability gap of the same kind as `bytes_hash`'s bitwise gap, and it deserves its
own intent rather than a workaround.

---

## Artifacts

**`axis-codegen-bridge-rs`**
- `src/runtime/bytes_codec.rs` — `bytes_get`, `bytes_push`; both decoders removed; test module reworked onto composition mirrors + std oracle
- `src/emit/rust_05.rs` — 2 map entries added, 2 removed
- `docs/bridge-audit/BYTE_INT_CODEC_COLLAPSE_V1_REPORT.md` — this report

**`axRegistry-working`**
- `axis-bridge.axreg` — `bytes_get`/`bytes_push` declared; `int16_be_decode`/`int32_be_decode` removed with a standing note

**`axVerity-working`**
- `lib/be{16,32}_{de,en}code.m1` — the four compositions
- `lib/be_codec_check{16,32}.m1`, `src/be_codec_check.m1`, `scripts/be-codec-sweep.sh` — equivalence harness
- `lib/bench_be{16,32}_{old,new}_step.m1`, `lib/bench_be_null_step.m1`, `src/be_codec_bench.m1` — Phase 5 harness
- 9 `lib/pg_*.m1` files — 10 cold-path call sites switched
- `registries/axverity-working.axreg`, `scripts/build.sh` — declarations and build wiring

# BRIDGE_CONSOLIDATION_V1 — Report

**intent-id:** `BRIDGE_CONSOLIDATION_V1`
**derived-from:** `BRIDGE_SURFACE_AUDIT_V1` @ `994a08b`, `INSERT_PATH_HONESTY_V1`,
`HOTPATH_STATE_TO_OFFSET_V1`
**runner:** ClaudeCode (authority: bounded, `AI_PROPOSE_ONLY`)

**Status: (A) DELIVERED. (B)–(E) STRUCTURALLY BLOCKED — one shared root cause.**

No performance measurement, benchmark, CCall count, or throughput comparison was
produced, requested, or used as a gate anywhere in this pass, per the hard limit.

---

## Headline

Four of the five workstreams — (B), (C), (D), (E) — cannot be done as specified,
and they all fail for the **same single reason**:

> **Every function named in (B), (C), (D) and (E) is Rust-only, holds thread-local
> mutable state (a `handle → shard` map), and has no M1 source. M1 cannot hold that
> state, so none of them can be "rewritten in M1" or "merged into shared M1
> function(s)".**

This is not a new discovery and not a pacing objection. It is the exact
`blocked_reason` `BRIDGE_SURFACE_AUDIT_V1` recorded for every one of these
functions:

> *"M1 has no mutable cross-call state; repatriation requires threading the state
> through the CoreIR dataflow"*

Dropping the performance gate does not unblock any of it — the blocker was never
performance. The intent's own note instructs *"If HOTPATH_STATE_TO_OFFSET_V1 has
already produced Phase 0-1 artifacts, reuse them; do not re-derive"*, and those
artifacts say precisely this.

**(A) was the one feasible workstream, and dropping the performance gate genuinely
did unblock it.** It is delivered: the UB-risk `mem_free_checked` is fixed with the
allocator's real capacity.

---

## Phase 0 — GROUND

### T21: `mem_free_checked(ptr, Int(<literal>))` grep — 4 real sites, 2 live

Complete paren-aware audit of every `*.m1` (a first pass using `[^)]*` silently
truncated `Int(4194304)` and reported the exact opposite; the corrected parse is
below).

**12 literal-capacity sites existed before this pass**, none previously catalogued.
Of the `mem_free_checked` subset — the UB class, because `mem_free_raw` requires the
capacity to match the original `alloc` `Layout` and a mismatch is **undefined
behaviour, not a checked error**:

| site | capacity | zone | outcome this pass |
|---|---|---|---|
| `lib_hotwrite_workload/hrw_seal_flush_reclaim.m1:34` | `Int(4194304)` | benchmark | **FIXED** |
| `lib/pg_hotblk_seal_mint.m1:57` | `Int(4194304)` | **LIVE** | blocked — see below |
| `lib/pg_derive_seal_mint.m1:36` | `Int(4194304)` | **LIVE** | blocked — see below |
| `lib_hotwrite_disk/hwd_seal_flush_reclaim.m1:43` | `Int(524288)` | benchmark | not done — see below |

Two block sizes (`524288`, `4194304`) were maintained by hand across four modules.
The codebase had already been bitten: `hrw_seal_flush_reclaim.m1:9` documents that
it exists as a **clone** of the 512 KiB version purely because that one *"hardcodes
capacity=Int(524288)"*. Cloning a function to change a constant is the drift this
defect produces.

### T22: is fieldidx/walidx's resident table thread-local? **YES — but that was never the blocker**

`fieldidx.rs:461` and `walindex.rs:717` both declare
`RESIDENT: RefCell<HashMap<String, i64>>` inside `thread_local!`. No `Mutex`,
`RwLock`, or `Arc`; the `OnceLock`s are read-only env-mode caches.
`fieldidx.rs:39` asserts it in its own doc.

So the Phase 4 precondition is **satisfied** — and Phase 4 is still impossible,
for a different reason given under (C) below. The intent made thread-locality the
gate; statefulness is the actual obstacle.

### T23: nameptr's toggle atomicity mechanism — identified

`nameptr.rs:67-82`:

```rust
let idle = 1 - cell.current;
cell.slots[idle] = line;   // fill idle slot
cell.current = idle;       // "atomically (single-threaded, no yield) flip"
```

`CELLS` is `thread_local!`, so **no concurrent reader is reachable** — the only
reader, `nameptr_get`, runs on the same thread and neither function yields. The
toggle protects against a reader seeing a half-written slot; that reader cannot
exist. The guarantee is therefore **vestigial with respect to concurrency**: there
is no cross-thread invariant for a rewrite to lose.

Per the hard limit this is stated rather than assumed — but it is moot, because (B)
does not proceed (see below), so no rewrite exists that could drop it.

### (iv) dump_hashes / dump_pk / stats — pure formatting, confirmed

`hotblk_recover.rs:208-261` and the `rawblk.rs` counterparts are sort → `join("\n")`
→ LF-terminate over an already-built shard, plus a `format!` for `stats`. **No frame
parsing, no filesystem access.** The Phase 0 STOP condition (frame-format
dependency) does **not** fire — the logic really is duplicated formatting.

It is nevertheless not extractable to M1: each body begins
`SHARDS.with(|s| s.borrow().get(&h) ...)`, a thread-local handle lookup.

### The structural finding that governs (B)–(E)

| target | actual state | M1 source? |
|---|---|---|
| `hotblk_get/set` | `RefCell<[i64; 6]>`, indexed 0..5, already bounds-checked | none |
| `wal_shard_get/set` | `RefCell<String>` | none |
| `nameptr_get/set` | `RefCell<HashMap<String, ToggleCell>>` of `String`s | none |
| `logbuf_append/len/open` | `RefCell<HashMap<i64, LogBuf>>`; `LogBuf` owns a live `std::fs::File` | none |
| `cursor_append/len/get/open/close` | `RefCell<HashMap<i64, Vec<String>>>` | none |
| `fieldidx_res_get`, `walidx_res_get` | stateful resident-handle caches over `RESIDENT` + `FIDX`/`IDX`, calling `do_rebuild`/`do_replay` | none |
| `fieldidx_snapshot`, `walidx_snapshot` | read thread-local shard by handle, format, `write_durable` | none |
| `hotblk_recover_*`, `rawblk_recover_*` dumps | read thread-local `SHARDS` by handle | none |

Verified mechanically: `grep -rl "^fn <name>(" --include=*.m1` returns nothing for
all eleven representative targets. The intent's boundary authorizes editing their
*"M1 sources"* — **those files do not exist.**

---

## Phase 1 — (A) CAPACITY-THREAD — DELIVERED

Capacity from `mem_reserve_raw` now rides the whole `hrw` chain from mint to free.
**All three `hrw` literals eliminated**, including the UB-risk free:

| site | before | after |
|---|---|---|
| `hrw_step.m1:107` | `mem_write_checked(ptr1, Int(4194304), …)` | `capacity1` |
| `hrw_seal_flush_reclaim.m1:31` | `mem_read_checked(ptr, Int(4194304), …)` | `capacity` |
| `hrw_seal_flush_reclaim.m1:34` | `mem_free_checked(ptr, Int(4194304))` | `capacity` — **UB risk closed** |

`hrw_step`'s overflow test also moved from `int_sub(Int(4194304), cursor0)` to the
threaded capacity, so the rotate decision and the write bound can no longer
disagree about the block size — previously two independent literals.

**The chain**, and why each link was necessary:

```
hrw_mint_block            capacity = tuple_field(region, Int(1))  → ctor field 2
hrw_rotate_first          reads mb field 2                        → ctor 9 → 10 fields
hrw_rotate_next           takes capacity0 (it seals the PREVIOUS block
                          before minting the next, so the old block's
                          capacity must be in hand)               → ctor 9 → 10
hrw_rotate                dispatcher, threads capacity0
hrw_keep                  same block continues → passes capacity through
hrw_step                  state 16 → 17 fields; uses capacity for BOTH the
                          overflow test and mem_write_checked
hrw_seal_current          new capacity param → queue
hrw_queue_push            queue entry 6 → 7 comma fields
hrw_flush_oldest          parses field 7
hrw_seal_flush_reclaim    new capacity param → read + free
```

Six axreg `in` contracts widened to match (`hrw_rotate`, `hrw_rotate_next`,
`hrw_keep`, `hrw_seal_current`, `hrw_queue_push`, `hrw_seal_flush_reclaim`).
`hrw_step` is `in (Value)`, which carries no arity, so its state widening needed no
contract change.

The selftest seeds capacity `0`. That is safe and not a fudge: at `i == 0`,
`hrw_step` sets `need_new` via `is_first`, so the capacity-based overflow test is
not consulted before the first mint supplies the real value. Stated in the source.

**Result: 12 literal-capacity sites → 9 real** (plus one deliberate negative case,
`memchecked_selftest.m1:88`'s `mem_free_checked(ptr, Int(0))`, which asserts
capacity-zero rejection and must stay).

### The 9 remaining, and why each was not done

**5 LIVE sites — boundary-blocked, unchanged from the prior intent's finding.**
`pg_hotblk_commit.m1:16`, `pg_hotblk_seal_mint.m1:56,57`,
`pg_derive_seal_mint.m1:35,36`. `pg_hotblk_mint` has two branches; the
`slice4_mode`-on branch takes its pointer from `hotblk_pool_take`, which returns
`Value::Str("{ptr}\t{cell}")` (`hotblk_pool.rs:126`) — no capacity field. Threading
real capacity requires widening that encoding, and `hotblk_pool.rs` is *"INHERENTLY
cross-thread by construction (the allocator thread produces…)"* — a real
cross-thread queue, which **`CONCURRENT_STAYS_SEPARATE` says to report and exclude,
not modify**. `slice4_mode` is separately named in
`BAKEOFF_APPARATUS_UNTOUCHABLE`. Compounding it, `hotblk` has nowhere to store a
capacity: slot 4 is documented as `block_start_i` (reserved, currently unused — so
squatting would be wrong) and `NFIELDS = 6` leaves no slot 6.

Fixing only the non-`slice4` branch would leave capacity correct on one path and
wrong on the other — worse than a consistent literal, since `mem_free_checked`
would then free with a value that is right or wrong depending on a dial.

**4 benchmark sites — not attempted, and I stopped deliberately rather than
half-finish.** `hotwrite_step.m1:70`, `hwd_step.m1:80`,
`hwd_seal_flush_reclaim.m1:40,43` (including a second `mem_free_checked` UB site).
These hang off `hotwrite_rotate_first`/`hotwrite_rotate_next`, which return a
**tab-delimited `Text`** `"<ptr>\t<cell>"`. Adding a third field breaks every
`str_after(bp, tab)` parser, and `hwd_rotate_next` / `hwd_rotate_first_wrap` build a
*4-field* protocol by concatenating onto that string — so a 3-field `bp` silently
becomes a 5-field message that `hwd_step`'s parser misreads. The cascade is
~8 files plus contracts across two module families, on top of the 13 already
changed in this pass. With the tree green and the UB site closed, I committed
rather than start a second protocol migration I could not finish and verify to the
same standard. This is the honest remaining work, not a hidden gap.

---

## Phases 2–3 — (B) HELPER + OFFSET-REWRITE — NOT DONE

**Phase 2's helper already exists.** `lib_memchecked/mem_read_checked.m1` +
`mem_write_checked.m1` are exactly the specified `memblk_read`/`memblk_write`, and
enforce **more** than asked: `offset >= 0`, `length >= 1`,
`offset + length <= capacity` (the specified check), **plus**
`offset + length <= written_hwm`, an uninitialized-read guard the intent does not
mention. Building a second pair would duplicate a shipped, selftested module.

**`ONE_CHECKED_PATH` already holds.** Zero callers of `mem_read_raw`/`mem_write_raw`
outside `lib_memchecked/` — in M1 or Rust. This pass added none.

**One correction to the intent's Phase 2 spec.** It directs a failure mode
*"matching mem_read_raw/mem_write_raw's existing panic-on-invalid-access
convention"*. Those fns panic only on **argument shape** (`offset < 0`); they perform
no capacity check at all, and `mem_read_raw`'s doc says the capacity check *"lives in
the M1 checked-wrapper, not here"*. The shipped wrapper deliberately returns a
defined empty-`Bytes` sentinel and documents why a panic is unavailable: **M1 has no
error-raising primitive**. So "panic" was not an option, and the existing convention
is a sentinel.

**Phase 3 is not possible.** The five families hold no raw memory and no address —
see the Phase 0 table. `wal_shard_get()` returns a `String`; `logbuf_append` extends
a `Vec<u8>` inside a struct owning a live `File`; `hotblk_get(2)` is an index into a
6-element `i64` array that is *already* bounds-checked more strictly than a capacity
comparison. There is no `(ptr, capacity)` to pass and no byte block to read. A
`memblk_read` rewrite of these would have to invent a representation that does not
exist.

`FAT_POINTER_ALWAYS` is therefore **vacuously held** for these families — no M1
function under this intent holds a bare address, because none of them holds an
address at all. Reported rather than claimed as a win.

---

## Phase 4 — (C) RES-MERGE — NOT DONE

T22 confirmed thread-local, so the intent's stated precondition passes. The merge is
still impossible: `walidx_res_get` (`walindex.rs:760-807`) is a **stateful resident-
handle cache**. It reads and mutates thread-local `RESIDENT`, allocates handles via
`next_handle()`, inserts into the thread-local `IDX` shard map, and dispatches to
`do_rebuild`/`do_replay` over WAL segments on the filesystem. That is mutable
cross-call state plus frame-format-aware replay — not expressible in M1.

Two further obstacles worth recording: the two fns have **different arities**
(`fieldidx_res_get` takes a 5-tuple, `walidx_res_get` a 4-tuple), and each closes
over a *different* thread-local pair (`FIDX`/`RESIDENT` vs `IDX`/`RESIDENT`), so
even a Rust-side merge needs the state passed in as a parameter.

A genuine consolidation **is** available, just not in M1: `fieldidx_res_scope` and
`walidx_res_scope` are near-identical — `walindex.rs:817` says *"Byte-for-byte the
same discipline as `fieldidx_res_scope`"* — and could share one Rust helper
parameterized by the two thread-locals. Not done: the boundary authorizes writing
*M1* helpers, and the spine marks these Rust files read-mostly.

---

## Phase 5 — (D) DURABLE-HELPER — NOT DONE

`write_durable` is genuinely triplicated: `fieldidx.rs:168`, `walindex.rs:184`,
`bytes_io.rs:111`. Real duplication — but **all three are Rust**, and the M1-side
durable-write helper the intent asks for **already exists**: `fs_write_bytes`
performs exactly that skeleton (write tmp → fsync tmp → atomic rename → fsync
parent dir).

The part that cannot move to M1 is the body construction:
`walidx_snapshot` (`walindex.rs:211-231`) looks up a shard by handle in the
thread-local `IDX` map and serializes `sh.map`. So the split the intent describes —
one shared M1 `write_durable` plus two thin format functions — would leave the two
"thin" functions still needing to reach into thread-local state, i.e. still Rust.

The available win is a Rust-side dedup of the two module-local `write_durable`
copies against `bytes_io.rs`'s. Not done, same boundary reason as (C).

---

## Phase 6 — (E) REPORT-FORMATTER — NOT DONE

Phase 0 confirmed these six bodies are pure formatting with no frame-format
dependency, so the STOP condition did not fire and the duplication is real —
`hotblk_recover_dump_pk`/`dump_hashes` and the `rawblk_recover` counterparts share
an identical sort → `join("\n")` → LF-terminate shape.

But `dump_report(shard, format) -> Text` cannot be an M1 function: every body
starts with a thread-local handle lookup (`SHARDS.with(|s| s.borrow().get(&h))`).
M1 can only reach that data through a bridge call, so an M1 formatter would need the
shard's entire contents marshalled across the boundary first — which is a new bridge
primitive, not a consolidation.

`open`/`rebuild` were not touched for either family.

---

## Phase 7 — RETIRE — NOTHING RETIRED

No function was fully replaced, so nothing reached zero callers and no declaration
was removed. `NO_REMOVAL_BEFORE_ZERO_CALLERS` is held trivially. (A) changed
signatures but retired no function.

---

## Phase 8 — VERIFY

| check | result |
|---|---|
| `scripts/build.sh` (bridge + all axVerity binaries + 8-worker `pg_server` pool) | clean, exit 0, 0 errors |
| `scripts/hotwrite-workload-build.sh` (the changed chain) | OK |
| `scripts/hotwrite-build.sh` | OK |
| `scripts/hotwrite-disk-build.sh` | OK |
| `scripts/axv-smoke.sh` | **15/17** = documented baseline (S16/S17 `PREPARE`/`EXECUTE` are expected failures; no `PREPARE` support exists, and every historical run since 2026-07-20 shows the identical error) |
| `scripts/slt-run.sh` | **6/6**, simple + extended legs |
| `lib_memchecked/selftest` | bounds enforcement intact |
| orphaned `pg_server` processes | **0** |

No performance figure is reported, per the hard limit.

---

## Reintegration check

| Anchor | Status |
|---|---|
| `FAT_POINTER_ALWAYS` | **ADVANCED where applicable; vacuous where not.** The `hrw` chain now carries the allocator's real `(ptr, capacity)` from mint to free. The (B) families hold no address at all, so there is nothing to bind — reported, not claimed. |
| `ONE_CHECKED_PATH` | **HELD** — still zero callers of `mem_read_raw`/`mem_write_raw` outside `lib_memchecked/`. This pass added none. |
| `CONCURRENT_STAYS_SEPARATE` | **HELD, and it was load-bearing.** `bindidx`/`contentidx` untouched and unmerged. Grounding surfaced a second case — `hotblk_pool.rs`, an inherently cross-thread producer/consumer queue — and it was reported and excluded exactly as the constraint prescribes, not modified to unblock the live sites. |
| bounds check never omitted or weakened | **HELD** — no check was removed. Three literal capacities were *replaced by the real value*, which strengthens them. The `mem_free_checked` UB exposure is closed for `hrw`. |
| `GROUND_BEFORE_DESIGN` (for C) | **HELD** — T22 confirmed before any merge was attempted; the merge was then blocked on a *different*, also-grounded fact rather than on assumption. |
| nameptr atomicity preserved | **HELD trivially** — mechanism identified (T23) and shown vestigial under thread-local scoping; no rewrite shipped, so nothing could drop it. |
| `NO_REMOVAL_BEFORE_ZERO_CALLERS` | **HELD** — nothing removed. |
| `BAKEOFF_APPARATUS_UNTOUCHABLE` | **HELD** — `slice4_mode` / `hotblk_pool` identified as the live-path blocker and deliberately not modified; `memcpy_*`, `slab_shadow_*` untouched. |
| no performance measurement introduced | **HELD** — none produced, none used as a gate. The one prior CCall figure is not cited as a decision input anywhere in this pass. |
| `AI_PROPOSE_ONLY` | **HELD** — delivered inside the authorized scope; the four infeasible workstreams are reported with evidence rather than approximated with something weaker. |
| priority: correctness > bounds-safety > concurrency-preservation > one-pass completion | **HELD, and this ordering decided the pass.** Completion-in-one-pass ranks *below* correctness and concurrency-preservation, which is why the live sites stayed blocked and the second protocol migration was stopped rather than rushed. |

### Failure conditions — none triggered

| Condition | Status |
|---|---|
| Any M1 fn holding a bare address Int | **No** |
| Any direct M1 call to `mem_read_raw`/`mem_write_raw` outside the helper | **No** |
| Any bounds check omitted or weaker than the original | **No** — three strengthened |
| `bindidx`/`contentidx` merged, partially or fully | **No** — untouched |
| A shadow-verification divergence shipped rather than halted on | **No** — no rewrite existed to verify (Phase 3 not possible); (A) verified by full build + smoke + SLT |
| nameptr atomicity silently dropped | **No** — named, and no rewrite shipped |
| Any performance benchmark introduced as a blocking step | **No** |
| A declaration removed before zero-caller re-confirmation | **No** — none removed |

### Outcome ledger

| Stated outcome | Result |
|---|---|
| ~6 capacity literals eliminated; `mem_free_checked` no longer hardcoded on a confirmed-live path | **PARTIAL** — 3 eliminated and the `hrw` UB free fixed. The live sites are boundary-blocked by `CONCURRENT_STAYS_SEPARATE` + `BAKEOFF_APPARATUS_UNTOUCHABLE`; 4 benchmark sites remain, enumerated. |
| `memblk_read`/`memblk_write` exist as sole raw callers with bounds checks | **ALREADY TRUE** before this intent, with a stronger check than specified |
| Five families rewritten in M1, shadow-verified | **NOT POSSIBLE** — no raw memory, no address, thread-local state, no M1 source |
| res_get/res_scope merged iff thread-local | **NOT DONE** — thread-local confirmed, but they are stateful caches; M1 cannot hold the state |
| snapshot fns share one `write_durable` | **NOT DONE** — the M1 helper already exists (`fs_write_bytes`); the remaining duplication is Rust-side |
| dump fns share one formatter, open/rebuild untouched | **NOT DONE** — duplication real and confirmed, but handle lookup is thread-local; open/rebuild untouched |
| `bindidx`/`contentidx` unmerged and unmodified | **HELD** |
| No performance figure reported or blocking | **HELD** |

### Epistemic status

| Claim | Type |
|---|---|
| All 11 representative (B)–(E) targets are Rust-only with no M1 source | **fact** — mechanical `grep` over every `*.m1` |
| Each holds thread-local mutable state entangled with the logic to be shared | **fact** — bodies read in full |
| 12 literal-capacity sites existed; 3 eliminated; 9 remain | **fact** — paren-aware audit, before and after |
| A mismatched `mem_free_checked` capacity is UB | **fact** — `rawmem.rs` `mem_free_raw` doc |
| `hrw` now frees with the allocator's returned capacity | **fact** — source + clean build of the chain |
| `hotblk_pool` is inherently cross-thread | **fact** — its own module doc |
| The 4 remaining benchmark sites need a ~8-file protocol migration | **estimate** — derived from reading the parsers and the 4-field `hwd` protocol; not attempted |

---

## Recommended follow-on

1. **The 4 benchmark literals** (incl. a second `mem_free_checked` UB site). Migrate
   `hotwrite_rotate_first`/`_next` to a 3-field Text, updating `hotwrite_keep`,
   `hotwrite_rotate`, `hotwrite_step`, `hwd_rotate_next`, `hwd_rotate_first_wrap`,
   `hwd_step`, `hwd_seal_flush_reclaim`, `hwd_spike_main` and their contracts
   together. Self-contained; no live path, no dial.
2. **The 5 live literals.** Needs a decision on giving the `slice4` pool encoding a
   capacity field — which means touching an excluded cross-thread module — and a
   `hotblk` slot to hold it. Both currently forbidden, so this needs its own intent
   that explicitly relaxes one of those boundaries.
3. **(C)/(D)/(E) as Rust-side dedup.** All three duplications are real and
   confirmed: two `res_scope` bodies described as "byte-for-byte the same
   discipline", three `write_durable` copies, six dump bodies with identical
   sort/join/LF shape. A Rust-side consolidation is straightforward and safe; it
   just is not the M1 merge this intent specified. Worth an intent that says Rust.
4. **The real (B) question.** If the goal is genuinely to get these families out of
   Rust, the prerequisite is a way for M1 to hold mutable cross-call state — the
   audit's own recorded blocker. That is a language/runtime question, not a
   refactor, and `logbuf_append` owning a live `File` may make it unreachable for
   at least one family regardless.

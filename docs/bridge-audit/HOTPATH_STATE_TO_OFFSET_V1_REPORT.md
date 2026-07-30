# HOTPATH_STATE_TO_OFFSET_V1 — Report

**intent-id:** `HOTPATH_STATE_TO_OFFSET_V1`
**derived-from:** `BRIDGE_SURFACE_AUDIT_V1` @ `994a08b`, `INSERT_PATH_HONESTY_V1`
**runner:** ClaudeCode (authority: bounded, `AI_PROPOSE_ONLY`)

**Status: original intent HALTED AT PHASE 0; re-scoped adoption pass DELIVERED.**

This document has two parts, written in sequence:

1. **Phase 0 as originally specified — halted.** Grounding falsified the intent's
   central premise about its own named targets, so no Phase 1 design was written.
   Nothing was modified at that point. One claim in Finding 2 was later found to be
   wrong and is corrected inline below.
2. **[Adoption pass](#adoption-pass--re-scoped-authorized-delivered) — delivered.**
   Chris re-scoped to the real defect the grounding uncovered and authorized it.
   One file changed (`lib_hotwrite_workload/hrw_mint_block.m1`); the remaining 12
   capacity-literal sites are enumerated and logged, 5 of them boundary-blocked.

The measurement gate on the live path (options 2 and 3) remains closed.

---

## Executive summary

| Finding | Consequence |
|---|---|
| **1.** The six named functions hold Rust data structures behind `thread_local RefCell` — an i64 array, a `String`, two `HashMap`s. **None holds an address. None calls `rawmem` at all.** | `FAT_POINTER_ALWAYS` has nothing to bind to. A `(ptr, capacity)` pair cannot represent a `String`, a `HashMap`, or a live `File`. |
| **2.** The requested Phase 2 deliverable **already exists** as `lib_memchecked/` (`AXVERITY_MEM_CHECKED_WRAPPERS_V1`), with a *stronger* check than specified, and `ONE_CHECKED_PATH` is **already satisfied**. | Phases 1–2 as written would rebuild a shipped module. |
| **3.** Phase 2's instruction to panic on bounds failure **contradicts** the convention it cites. The existing wrappers deliberately return a defined sentinel and document why a panic is unavailable. | Following Phase 2 literally would break the established convention, not match it. |
| **4.** There *is* a real bare-address defect — but at **different functions** than the six named: the allocation sites discard capacity. | A re-scoped intent has a genuine, well-founded target. See "What the real defect is". |

---

## Phase 0 — GROUND

### Finding 1: the six named functions are not address-based

Read in full: `hotblk.rs`, `walshard.rs`, `nameptr.rs`, `logbuf.rs` (`logbuf_append`
+ `LogBuf`), plus `rawmem.rs`'s `mem_reserve_raw` / `mem_read_raw` /
`mem_write_raw` / `mem_free_raw`.

| fn | actual state | addressed by | "capacity" | bounds check today |
|---|---|---|---|---|
| `hotblk_get` / `hotblk_set` | `RefCell<[i64; 6]>` — a fixed Rust array | **field index 0..5** | compile-time `const NFIELDS: usize = 6` | explicit: `f < 0 \|\| f >= NFIELDS` → panic (`hotblk.rs:72,99`) |
| `wal_shard_get` / `wal_shard_set` | `RefCell<String>` | **nothing** — `wal_shard_get` takes `Unit` | n/a | none needed |
| `nameptr_get` / `nameptr_set` | `RefCell<HashMap<String, ToggleCell>>`, `ToggleCell { slots: [String; 2], current: usize }` | **`String` slug** | n/a (hash map) | none; missing key → `""` |
| `logbuf_append` | `RefCell<HashMap<i64, LogBuf>>`, `LogBuf { path: String, file: File, buf: Vec<u8>, file_len: u64 }` | **opaque i64 handle** | `Vec` grows | unknown handle → panic |

**`grep` for `mem_read_raw` / `mem_write_raw` / `mem_reserve_raw` / `rawmem` across
all four modules returns zero hits.** These functions do not use raw memory in any
form.

The intent's scope says the work is *"replacing bare-Int addresses with an M1-held
(ptr, capacity) tuple"*. **There are no bare-Int addresses in this family to
replace.** `hotblk_get(2)` is an array index, already bounds-checked more strictly
than a capacity comparison would be. `nameptr_get("some-slug")` is a hash lookup.
`logbuf_append` owns a live `std::fs::File` — not representable as bytes in a raw
block at any capacity.

**What the audit actually said.** All six carry `blocked_reason`:

> *"M1 has no mutable cross-call state; repatriation requires th[reading the state
> through the CoreIR dataflow]"*

and `m1_form`: *"pure data structure threaded through the CoreIR dataflow as a
Value"*. That is a **value-threading** blocker, not an addressing one. The intent's
premise — *"This removes the value-threading cost that made the family
PRIMITIVE-BLOCKED — an Int pair is native and cheap to pass"* — assumes the state
*is* an Int pair. It is not: it is 6 i64s, or a `String`, or a `HashMap` of
`String`s, or a `HashMap` of structs owning file descriptors.

### Finding 2: the requested helper already exists, and `ONE_CHECKED_PATH` already holds

`axVerity-working/lib_memchecked/` (built under `AXVERITY_MEM_CHECKED_WRAPPERS_V1`):

```
mem_read_checked.m1          mem_write_checked.m1          mem_free_checked.m1
mem_write_checked_commit.m1  mem_free_checked_commit.m1    selftest/
```

`mem_read_checked.m1` implements **exactly** the bounds-check expression Phase 1
was to specify, and three checks beyond it:

```m1
fn mem_read_checked(ptr: Int, capacity: Int, offset: Int, length: Int, written_hwm: Int) -> Bytes {
  let end       = int_add(offset, length)
  let offset_ok = int_gte(offset, Int(0))
  let length_ok = int_gte(length, Int(1))
  let cap_ok    = int_lte(end, capacity)      // <-- the intent's specified check
  let init_ok   = int_lte(end, written_hwm)   // <-- uninitialized-read guard
  ...
  if ok { mem_read_raw(ptr, offset, length) } else { text_to_bytes(Text("")) }
}
```

The `written_hwm` check closes a gap the intent does not mention at all: capacity
alone does not catch a read of **in-bounds-but-never-written** memory. The module
also documents its own limitation explicitly (single-writer monotonic-append
assumption; scattered concurrent writes to one allocation would need per-range
tracking) — i.e. the hard part has already been thought through and bounded.

**`ONE_CHECKED_PATH` is already satisfied for read/write:**

- Rust callers of `mem_read_raw` / `mem_write_raw` outside `rawmem.rs`: **zero**.
- M1 callers outside `lib_memchecked/`: **zero**.

`mem_reserve_raw` is called directly from M1 (`hrw_mint_block.m1:31`,
`hotwrite_rotate_first.m1:26`, `hotwrite_rotate_next.m1:21`), but `ONE_CHECKED_PATH`
as written constrains only `mem_read_raw`/`mem_write_raw`, and reserve is the
function that *produces* the fat pointer rather than consuming it.

**CORRECTION (added during the adoption pass).** The sentence that stood here —
that the gap is adoption because the module is *"never wired into pg_server"* —
was **wrong**. It repeated `mem_read_checked.m1`'s own header, and **that header is
stale**. The wrappers *are* wired into the live path:

| site | call |
|---|---|
| `lib/pg_hotblk_commit.m1:16` | `mem_write_checked(ptr1, Int(4194304), cursor1, bytes)` — **live insert path** |
| `lib/pg_hotblk_seal_mint.m1:56,57` | `mem_read_checked` / `mem_free_checked` |
| `lib/pg_derive_seal_mint.m1:35,36` | `mem_read_checked` / `mem_free_checked` |

So the gap is neither construction nor adoption. It is the **capacity argument**:
every one of those calls passes a hardcoded literal instead of the capacity the
allocator returned. See "What the real defect is" below, which the adoption pass
then narrowed further.

### Finding 3: Phase 2's failure-mode instruction contradicts the cited convention

Phase 2 directs: *"must reject any offset+width > capacity with a defined failure
behaviour (panic, per project's existing raw-primitive convention — confirm this
matches mem_read_raw's own failure mode in Phase 0, don't invent a new one)."*

Confirmed in Phase 0, and the answer inverts the instruction:

- `mem_read_raw` **does** panic — but only on *argument-shape* validation
  (`offset < 0 || length < 0`). It performs **no** capacity check at all; its doc
  states the capacity check *"lives in the M1 checked-wrapper, not here."*
- The existing M1 wrapper **deliberately does not panic** on a bounds failure. It
  returns an empty-`Bytes` sentinel, and documents why at length: `Bytes` has no
  literal syntax in M1, `Value`'s sum type carries no `Bytes` variant, so an
  `Option`/`Result` return is unavailable — and a new bridge type was forbidden by
  its governing intent.

This is the same constraint that blocked half of `BYTE_INT_CODEC_COLLAPSE_V1`'s
strictness work earlier today: **M1 has no error-raising primitive** — no `panic`,
`assert`, or `fail` is declared in any registry. So "panic" is not available to an
M1-side helper at all, and the established convention is a defined sentinel.
Following Phase 2 literally would either force the helper into Rust (contradicting
Phase 2's own stated preference for M1-side) or invent a failure mode that
diverges from the shipped wrappers.

### T16 — are `cursor_*` / `fieldidx_open` thread-local? **YES, both.**

| module | state |
|---|---|
| `cursor.rs` | `thread_local! { static CURS: RefCell<HashMap<i64, Vec<String>>>; static NEXT: Cell<i64> }` |
| `fieldidx.rs` | `thread_local! { static FIDX: RefCell<HashMap<i64, FieldShard>>; static NEXT: Cell<i64> }`, plus `thread_local! { static RESIDENT: RefCell<HashMap<String, i64>> }` |

Same shape as `logbuf.rs`. The `OnceLock`s in both are read-only env-var config
caches, not mutable state. `fieldidx.rs:39` asserts in its own doc: *"no
`Mutex`/`RwLock`/`Arc`/process-global registry anywhere on this path."*

**The corresponding STOP condition does not fire.** Reported per instruction rather
than silently dropped — but note they remain boundary-`forbidden` for
implementation regardless, and Finding 1 applies to them equally: they are
handle-addressed `HashMap`s, not memory blocks.

### T17 — does `nameptr_set` rely on cross-CCall atomicity? **NO.**

`nameptr.rs:67-82`:

```rust
let idle = 1 - cell.current;
cell.slots[idle] = line;   // fill idle slot
cell.current = idle;       // atomically (single-threaded, no yield) flip
```

`CELLS` is `thread_local!`, so **no concurrent reader can exist** — the only reader
is `nameptr_get` on the same thread, and neither function yields. The toggle
protects against a reader observing a half-written slot, but that reader is
unreachable by construction. The guarantee is therefore *vestigial* with respect to
concurrency: it costs nothing and breaks nothing, but there is no cross-thread
invariant for an M1 redesign to lose.

**The corresponding STOP condition does not fire** — the toggle is replicable by a
multi-CCall M1 sequence, *if* the state were reachable from M1 at all (per Finding
1, it is not).

### Assumptions checked

| Intent assumption | Verdict |
|---|---|
| `hotblk`'s accumulator array is fixed at 6 slots, no runtime growth | **CONFIRMED** — `const NFIELDS: usize = 6` (`hotblk.rs:52`) |
| `mem_read_raw`/`mem_write_raw`'s panic-on-invalid-access is an acceptable failure mode for the helper to match | **FALSIFIED as stated** — they panic only on argument shape, never on capacity; the shipped wrapper's convention is a sentinel, and M1 cannot panic. See Finding 3. |

---

## What the real defect is

The intent's instinct is sound — there **is** a live bare-address problem. It sits
at different functions than the six named, and Phase 0 located it precisely:

```m1
// lib_hotwrite_workload/hrw_mint_block.m1:31-32   (also hotwrite_rotate_first.m1:26-27,
let region = mem_reserve_raw(Int(4194304))       //  hotwrite_rotate_next.m1:21)
let ptr    = tuple_field(region, Int(0))         // <-- capacity DISCARDED here
```

`mem_reserve_raw` hands back `(ptr, capacity)` — the self-describing pair its doc
calls *"the **only** record, anywhere, that this allocation exists"* — and the
caller immediately keeps field 0 and drops field 1. The bare `ptr` is then stashed
into the hot-block scratchpad by `lib/pg_hotblk_mint.m1`:

```m1
let ptr = str_to_int(str_before(ptrcell, tab))
let _s0 = hotblk_set(Int(0), ptr)     // bare address, capacity long gone
```

So `hotblk` is implicated only as the *container* someone chose for a pointer — it
is a general 6-slot i64 scratchpad, not a memory abstraction. The defect is at the
allocation sites that throw the capacity away, and the fix is to thread the pair and
route reads/writes through the **already-built** `lib_memchecked` wrappers.

That is an **adoption** task on a *different, smaller* function set —
`hrw_mint_block`, `hotwrite_rotate_first`, `hotwrite_rotate_next`, `pg_hotblk_mint`
— and it does not require a new helper, a new primitive, or touching
`nameptr`/`walshard`/`logbuf` at all.

Two cautions for whoever scopes that: `pg_hotblk_mint` is on the live insert path
and passes the pointer through a **tab-delimited `Text`** round-trip
(`str_to_int(str_before(ptrcell, tab))`), so threading a real pair changes that
encoding; and `slice4_mode` gates an alternate branch through
`hotblk_pool_take` — `slice4_mode` is a **bake-off dial and is
`BAKEOFF_APPARATUS_UNTOUCHABLE`**, so any such intent must state how it avoids
disturbing it.

---

## Reintegration check

| Anchor | Status |
|---|---|
| `GROUND_BEFORE_DESIGN` | **HELD — and it is what caught this.** Every named target's state-establishment point was read before any signature was proposed. The premise failed at that step, so no design was written. |
| `FAT_POINTER_ALWAYS` | **NOT APPLICABLE as scoped** — no M1 function in the named family holds an address. Reported rather than satisfied vacuously. A real violation was found elsewhere and named. |
| `ONE_CHECKED_PATH` | **ALREADY SATISFIED** — zero callers of `mem_read_raw`/`mem_write_raw` outside `lib_memchecked/`, in M1 or Rust. |
| bounds check present and testable before any correctness claim | **N/A — no correctness claim made.** No design proposed, so nothing relies on an unvalidated offset. |
| `NO_UNMEASURED_HOTPATH_CUTOVER` | **HELD** — nothing switched, nothing measured, no production source touched. |
| `BAKEOFF_APPARATUS_UNTOUCHABLE` | **HELD** — `slice4_mode` observed only as a read-only branch condition while reading `pg_hotblk_mint`; no dial read, written, or modified. Flagged as a hazard for any follow-on. |
| no work on `bytes_hash`, `cursor_close/load/open`, `fieldidx_open`, `*_rebuild` | **HELD** — `cursor.rs`/`fieldidx.rs` read only for T16's thread-locality verdict, which Phase 0 explicitly mandates; no design or change. |
| `AI_PROPOSE_ONLY` | **HELD** — halted for Chris's decision rather than re-scoping the intent unilaterally. |
| `MEASURE_DONT_ASSERT` | **HELD** — every claim here is a citation to a read line, not an inference. |
| `boundary default forbidden` | **HELD** — only reading and report authoring, both explicitly allowed. |
| priority: evidence-grounding > safety-preservation > measurement-integrity > generality | **HELD** — evidence-grounding is exactly the priority that stopped this at Phase 0 rather than producing a plausible design against a false premise. |

### Failure conditions — none triggered

| Condition | Status |
|---|---|
| Any M1 fn under this intent holding a bare address Int | **No** — none written |
| Any direct M1 call to `mem_read_raw`/`mem_write_raw` outside the helper | **No** — pre-existing state already clean |
| A bounds check omitted or silently weaker than the original | **No** — no composition written |
| Any hot-path caller switched without Phase 6 authorization | **No** — zero sources modified |
| `cursor_*`/`fieldidx_get` included without confirming thread-locality | **No** — confirmed thread-local, and still excluded |
| Phase 4 skipped or its divergence check not run | **N/A** — halted at Phase 0; Phase 4 is not reached, not skipped |

### Epistemic status

| Claim | Type |
|---|---|
| The six named fns contain no address and never call `rawmem` | **fact** — all four modules read in full; grep returns zero hits |
| `lib_memchecked/` already implements the Phase 2 deliverable with a stronger check | **fact** — source read in full |
| `ONE_CHECKED_PATH` already holds | **fact** — exhaustive grep, M1 and Rust |
| `cursor_*`/`fieldidx` are thread-local (T16) | **fact** — `thread_local!` declarations read |
| `nameptr_set` needs no cross-CCall atomicity (T17) | **fact** — `CELLS` is `thread_local!`, so no concurrent reader is reachable |
| `hotblk` capacity is fixed at 6 | **fact** — `const NFIELDS: usize = 6` |
| The real bare-address defect is capacity-discard at the three `mem_reserve_raw` sites | **fact** — the `tuple_field(region, Int(0))` lines are cited |
| A re-scoped adoption intent would improve safety | **untested prediction** — plausible but unmeasured; no CCall or throughput work was done, since Phase 5 was never reached |

### Unknowns

| Unknown | Status |
|---|---|
| `nameptr_set` cross-CCall atomicity (T17) | **SETTLED — moot**, thread-local scoping makes the question empty |
| `cursor_*`/`fieldidx_get` state model (T16) | **SETTLED — thread-local**, same shape as `logbuf` |
| Whether CCall overhead is visible against T9's unexplained 91% | **UNADDRESSED** — Phase 5 not reached. Note that `BYTE_INT_CODEC_COLLAPSE_V1` Phase 5 has since measured the raw input to this question: **a CCall costs ~65 ns**, and cost is linear in CCall count. That figure is reusable by any re-scoped version of this intent to predict a delta before building anything. |

---

## Recommendation

Do not proceed to Phase 1 as written — it would design a `(ptr, capacity)`
representation for state that has no pointer, and rebuild a module that already
ships.

Three coherent ways forward, for Chris to choose:

1. **Re-scope to adoption** (recommended). New intent targeting the three
   capacity-discarding `mem_reserve_raw` sites plus `pg_hotblk_mint`: thread the
   real pair, route reads/writes through the existing `lib_memchecked` wrappers, and
   wire that module in for the first time. Keeps `FAT_POINTER_ALWAYS` and
   `ONE_CHECKED_PATH` as the anchors — they simply bind to the right functions.
   Must state up front how it avoids `slice4_mode`.
2. **Re-scope to the actual blocker.** If the goal was really to repatriate the
   thread-local family, the blocker is *"M1 has no mutable cross-call state"* — a
   value-threading problem needing a completely different design (threading state
   through the CoreIR dataflow as a `Value`), and one that must reckon with
   `logbuf_append` owning a live `File`, which cannot be threaded as a value at all.
3. **Close as already-satisfied.** If the aim was the `ONE_CHECKED_PATH` discipline
   itself, it is already met; the only outstanding work is wiring `lib_memchecked`
   into the live path, which is option 1.

---

# ADOPTION PASS — re-scoped, authorized, delivered

Chris authorized the non-live adoption package: *"build the non-live threading now,
report the live-path blocker precisely as a logged finding, don't try to solve it in
the same pass"*, plus a folded-in request to grep for other
`mem_free_checked(ptr, Int(<literal>))` sites and report them in the same output.
Options 2 and 3 (touching the live path / bumping `NFIELDS`) remain closed and still
require the measurement gate.

## The capacity-literal audit (complete, comment-stripped, paren-aware)

12 call sites pass a hardcoded literal capacity. None was previously catalogued.

| zone | site | wrapper | capacity |
|---|---|---|---|
| **LIVE — blocked** | `lib/pg_hotblk_commit.m1:16` | `mem_write_checked` | `Int(4194304)` |
| **LIVE — blocked** | `lib/pg_hotblk_seal_mint.m1:56` | `mem_read_checked` | `Int(4194304)` |
| **LIVE — blocked** | `lib/pg_hotblk_seal_mint.m1:57` | `mem_free_checked` | `Int(4194304)` |
| **LIVE — blocked** | `lib/pg_derive_seal_mint.m1:35` | `mem_read_checked` | `Int(4194304)` |
| **LIVE — blocked** | `lib/pg_derive_seal_mint.m1:36` | `mem_free_checked` | `Int(4194304)` |
| benchmark | `lib_hotwrite/hotwrite_step.m1:70` | `mem_write_checked` | `Int(524288)` |
| benchmark | `lib_hotwrite_disk/hwd_step.m1:80` | `mem_write_checked` | `Int(524288)` |
| benchmark | `lib_hotwrite_disk/hwd_seal_flush_reclaim.m1:40` | `mem_read_checked` | `Int(524288)` |
| benchmark | `lib_hotwrite_disk/hwd_seal_flush_reclaim.m1:43` | `mem_free_checked` | `Int(524288)` |
| benchmark | `lib_hotwrite_workload/hrw_step.m1:107` | `mem_write_checked` | `Int(4194304)` |
| benchmark | `lib_hotwrite_workload/hrw_seal_flush_reclaim.m1:31` | `mem_read_checked` | `Int(4194304)` |
| benchmark | `lib_hotwrite_workload/hrw_seal_flush_reclaim.m1:34` | `mem_free_checked` | `Int(4194304)` |

Everything inside `lib_memchecked/` (the wrappers themselves and their 12-case
selftest) is correctly parameterised — capacity is a parameter or a local, never a
literal. The single literal there, `mem_free_checked(ptr, Int(0))` at
`selftest/memchecked_selftest.m1:88`, is a deliberate negative case (case 11,
"free_invalid_capacity_zero_expect_false").

Two block sizes — `524288` and `4194304` — are maintained by hand across these 12
sites in four modules. The codebase has already been bitten:
`hrw_seal_flush_reclaim.m1:9` documents that it exists as a **clone** of the
512 KiB version specifically because that one *"hardcodes capacity=Int(524288)"*.
Cloning a function to change a constant is the drift this defect produces.

### `mem_free_checked` with a literal capacity — the folded-in grep

**4 real sites, and 2 of them are live.** This is the sharpest class: per
`rawmem.rs`, `mem_free_raw` requires the capacity to match the original `alloc`
`Layout` exactly, and a mismatch is **undefined behaviour, not a checked error** —
so `mem_free_checked`'s bounds check cannot save a caller whose literal has drifted.

| site | capacity | zone |
|---|---|---|
| `lib/pg_hotblk_seal_mint.m1:57` | `Int(4194304)` | **LIVE — blocked, logged** |
| `lib/pg_derive_seal_mint.m1:36` | `Int(4194304)` | **LIVE — blocked, logged** |
| `lib_hotwrite_disk/hwd_seal_flush_reclaim.m1:43` | `Int(524288)` | benchmark |
| `lib_hotwrite_workload/hrw_seal_flush_reclaim.m1:34` | `Int(4194304)` | benchmark |

Per instruction, the two live ones are logged here and **not** taken into scope on
this pass.

## What was shipped

**One file: `lib_hotwrite_workload/hrw_mint_block.m1`.**

```m1
fn hrw_mint_block(next_block_seq: Int) -> Value(Int, Int, Int) {
  let region   = mem_reserve_raw(Int(4194304))
  let ptr      = tuple_field(region, Int(0))
  let capacity = tuple_field(region, Int(1))   // was DISCARDED
  let cell     = cell_new_raw(Int(0))
  let _act     = cell_cas_raw(cell, Int(0), Int(1))
  Value(Int, Int, Int)(ptr, cell, capacity)
}
```

Why this is the load-bearing one-line change and not a token gesture:

- It is **the only one of the three named allocation sites that the main
  `scripts/build.sh` compiles** (line 74) — i.e. the only one on the live-path
  build. `hotwrite_rotate_first` / `hotwrite_rotate_next` are reached solely by the
  three dedicated benchmark build scripts.
- It is the allocation site feeding the live path: `pg_hotblk_mint.m1:38` calls it
  on the non-`slice4` branch.
- It makes the allocator's real capacity **available at the live path's mint point
  for the first time**. Every future fix to the five blocked live sites requires
  that value to exist there; without this, that pass is impossible.
- Capacity is **appended** as field 2, so all four existing readers
  (`pg_hotblk_mint`, `hrw_rotate_first`, `hrw_rotate_next`,
  `hotblk_allocator_step` — each verified to read only `ctor_field` 0 and 1) are
  behaviourally untouched. The registry contract is `out Value`, which carries no
  arity, so no axreg change was needed.
- Cost: one extra `Int` in a ctor built **once per 4 MiB block**. Nothing per-record.

Confirmed in the emitted Rust (`build/generated_hrw_mint_block_xb.rs:13-15`): two
`tuple_field` dispatches now, on pool constants 0 and 1.

Three now-stale comments asserting the old `Value(Int, Int)` arity were corrected in
`hrw_mint_block.m1`, `hrw_rotate_first.m1`, `hrw_rotate_next.m1`.

## Why the remaining 12 sites were not fixed on this pass

**The five live sites are boundary-blocked.** `pg_hotblk_mint` has two branches. The
`slice4_mode`-on branch takes its pointer from `hotblk_pool_take`, which returns
`Value::Str("{ptr}\t{cell}")` (`hotblk_pool.rs:126`) — a Text encoding with no
capacity field, owned by `AXVERITY_SLICE4_BLOCK_DURABILITY_V2` and gated by
`slice4_mode`, which is named in `BAKEOFF_APPARATUS_UNTOUCHABLE`. Threading real
capacity through the live path requires widening that encoding. Compounding it:
`hotblk` has nowhere to *store* a capacity — slot 4 is documented as `block_start_i`
(reserved, currently unused, so squatting on it would be wrong) and `NFIELDS = 6`
leaves no slot 6.

**The seven benchmark sites need per-record state widened.** This is the finding
that changed the shape of the authorized package, so it is worth stating precisely:
capacity cannot reach a seal/free site without travelling with the pointer through
the per-record loop.

- `hrw`: `hrw_mint_block` → `hrw_rotate_first`/`hrw_rotate_next` → **`hrw_step`'s
  16-field `Value` ctor state** → `hrw_seal_current` → `hrw_queue_push` (6-field
  comma-delimited queue entry) → `hrw_flush_oldest` → `hrw_seal_flush_reclaim`.
  `hrw_rotate_next` seals the *previous* block using `ptr0` taken from that state,
  so the old block's capacity must have been carried in it. Threading requires
  widening the state to 17 fields and touching ~9 files.
- `hotwrite` / `hwd`: pointer and cell travel as a tab-delimited `Text`
  (`"<ptr>\t<cell>"`) parsed **every record** by `hotwrite_step` / `hwd_step`.
  Appending a third field breaks every `str_after(bp, tab)` parser, so it cannot be
  done without editing those per-record paths.

Either way the edit lands in a throughput benchmark's inner loop and adds per-record
CCalls, which would **move the very numbers these modules exist to produce**. At the
~65 ns/CCall measured under `BYTE_INT_CODEC_COLLAPSE_V1`, ~2–4 added CCalls is
~130–260 ns per record against a benchmark whose records cost ~1–2 µs — a 7–25 %
perturbation of published spike figures. That is a decision about measurement
integrity, not a mechanical refactor, and it is not this pass's call to make.

## Recommended follow-on, in dependency order

1. **Live path (needs the gate).** Give the `slice4` pool encoding a capacity field
   and `hotblk` a slot to hold it, then switch the five live literals — chiefly the
   two `mem_free_checked` UB sites. Requires touching `slice4` apparatus and
   `hotblk.rs` (`NFIELDS` 6→7), so it needs explicit authorization and a
   measurement gate. `hrw_mint_block` now supplies the capacity this depends on.
2. **Benchmark chains (needs a perturbation decision).** Widen the `hrw` state and
   the `hotwrite`/`hwd` Text protocols, accepting a re-baseline of the spike
   numbers — or decide the drift risk in non-production benchmark code does not
   justify invalidating their published figures, and leave them literal with a
   comment pointing at this report.
3. **Stale header.** `lib_memchecked/mem_read_checked.m1`'s "never wired into
   pg_server" claim is false and misled this report's own Phase 0. Worth correcting
   at the source.

## Verification

| check | result |
|---|---|
| `scripts/build.sh` (bridge + all axVerity binaries + 8-worker `pg_server` pool) | clean, exit 0, 0 errors |
| `scripts/hotwrite-workload-build.sh` | OK |
| `scripts/hotwrite-build.sh` | OK |
| `scripts/hotwrite-disk-build.sh` | OK |
| `lib_memchecked/selftest` — 12 cases incl. bounds rejection, exact-boundary accept, uninitialized-gap, capacity-zero reject | **all pass** |
| `scripts/axv-smoke.sh` (live path; `pg_hotblk_mint` reads the changed fn) | **15/17** = documented baseline |
| `scripts/slt-run.sh` | **6/6**, simple + extended |

## Reintegration check — adoption pass

| Anchor | Status |
|---|---|
| `FAT_POINTER_ALWAYS` | **ADVANCED, not yet satisfied.** The live path's mint point now carries the allocator's real capacity instead of discarding it. 12 downstream sites still reconstruct capacity from a literal; all 12 are enumerated above rather than left implicit. |
| `ONE_CHECKED_PATH` | **HELD** — still zero callers of `mem_read_raw`/`mem_write_raw` outside `lib_memchecked/`. This pass added none. |
| `NO_UNMEASURED_HOTPATH_CUTOVER` | **HELD** — no hot-path caller switched. The one change is behaviourally invisible to every existing reader and costs one `Int` per 4 MiB block. Options 2 and 3 remain closed. |
| `BAKEOFF_APPARATUS_UNTOUCHABLE` | **HELD** — `slice4_mode` / `hotblk_pool` identified as the live-path blocker and deliberately **not** modified. `memcpy_*`, `slab_shadow_*` untouched. Benchmark inner loops left unperturbed, which is the same principle applied to measurement rather than to dials. |
| `GROUND_BEFORE_DESIGN` | **HELD — and it changed the package twice.** Grounding corrected a stale "never wired in" claim, then revealed that "three allocation sites" resolves into one live-relevant site plus two benchmark-only ones whose consumers sit in per-record loops. |
| `AI_PROPOSE_ONLY` | **HELD** — shipped only inside the authorized non-live scope; everything requiring a gate is logged, not done. |
| scope discipline | **HELD** — the folded-in `mem_free_checked` grep found 2 live sites; per instruction they went into the blocker report, not into scope. |

### Epistemic status

| Claim | Type |
|---|---|
| 12 sites pass a literal capacity; 4 are `mem_free_checked`, 2 of those live | **fact** — paren-aware audit over every `*.m1`, output reproduced above |
| `lib_memchecked` is wired into the live path; its header saying otherwise is stale | **fact** — `pg_hotblk_commit.m1:16` read directly |
| `hrw_mint_block` is the only one of the three allocation sites on the main build | **fact** — `scripts/build.sh:74` |
| All four readers take only fields 0 and 1, so appending is safe | **fact** — each call site read |
| Capacity now comes from the allocator, not a constant | **fact** — `generated_hrw_mint_block_xb.rs:13-15` |
| A mismatched `mem_free_checked` capacity is UB | **fact** — `rawmem.rs` `mem_free_raw` doc |
| Threading the benchmark chains would perturb published spike numbers by ~7–25 % | **estimate** — derived from the measured ~65 ns/CCall and stated per-record costs; not measured on these binaries, which were deliberately not re-run |

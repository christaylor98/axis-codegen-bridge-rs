# HOTPATH_STATE_TO_OFFSET_V1 — Report

**intent-id:** `HOTPATH_STATE_TO_OFFSET_V1`
**derived-from:** `BRIDGE_SURFACE_AUDIT_V1` @ `994a08b`, `INSERT_PATH_HONESTY_V1`
**runner:** ClaudeCode (authority: bounded, `AI_PROPOSE_ONLY`)

**Status: HALTED AT PHASE 0.** Phase 1 not started.

Phase 0 falsified the intent's central premise about its own named targets. Under
`GROUND_BEFORE_DESIGN`, and per the request's *"Halt and report on any STOP condition
rather than working around it"*, no design is proposed here. Three grounded findings
require a re-scope decision from Chris before any further phase runs.

Nothing was modified. This report is the only artifact.

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

The real gap is **adoption, not construction**: `mem_read_checked.m1`'s own header
states it is *"Standalone module … **never wired into pg_server or
axVerity-working's existing lib/**"*.

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

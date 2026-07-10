# M1_VALUE_STR_ARC_IMPLEMENTATION_V1 — implementation & measurement report

Intent: `M1_VALUE_STR_ARC_IMPLEMENTATION_V1` (governance, authority human,
AI bounded). Implements the DECIDED representation change
`decl:m1-value-str-default-representation-v3`: `Value::Str(u32)` →
`Value::Str(Arc<str>)`, migrates all call sites, keeps the test suite green,
and runs the str-load-vs-tag-load contention isolation measurement.

Status: **implementation complete, all gates green.** Recommendations for Chris
below are recommendations, not decisions.

---

## 1. What was changed

### Core representation (`src/runtime/value.rs`)
- `Value::Str(u32)` → `Value::Str(Arc<str>)`. The string is now carried inline;
  cheap clone is an atomic refcount bump.
- The global interner is **removed**: `static STRING_TABLE: OnceLock<Mutex<Vec<String>>>`
  and `static STRING_MAP: OnceLock<Mutex<HashMap<String,u32>>>` are gone, along
  with their `init_runtime()` initialization. This is the Mutex-guarded shared
  structure that was the str-path contention source. **No new shared, mutable,
  lock-guarded structure was introduced anywhere** (NO_NEW_SHARED_STRUCTURE).
- `intern_str` and `get_str` are **retained as thin shims** (names kept to avoid
  churn at ~200 call sites; there is no interning anymore):
  - `intern_str(s: &str) -> Arc<str>` = `Arc::from(s)`. Every construction site
    `Value::Str(intern_str(x))` — including the code emitted by
    `emit/rust_05.rs` — compiles unchanged.
  - `get_str<S: AsRef<str>>(s: S) -> String` — accepts an owned `Arc<str>`, a
    `&Arc<str>`, a `&str`, or a `String`, and returns a fresh owned `String`.
    The old clone-on-read behavior is preserved by construction (see §5, the
    aliasing unknown).
- Added a permanent compile-time gate making VALUE_MUST_STAY_SEND_SYNC a static
  invariant: `assert_send_sync::<Value>()` in a `const _: fn()`. Arc<str> is
  Send+Sync, so Value stays Send+Sync; if a future field breaks it, the crate
  fails to build at that line.
- `#[derive(Clone, Debug, PartialEq)]` on Value is unchanged and still
  typechecks (DERIVES_MUST_KEEP_WORKING). Note PartialEq on `Arc<str>` compares
  **str contents**, which preserves the interner's equal-string ⇒ equal-value
  semantics exactly.

### Call-site migration (18 files, mechanical)
Fresh grep found ~271 `Value::Str` sites across 28 files (more than the
report's "~15+"; the assumption that the enumerated set was exhaustive was
correct to flag as tentative). The migration reduced to a few mechanical
classes:
- **Construction** (`Value::Str(intern_str(x))`) — unchanged; `intern_str` now
  returns `Arc<str>`.
- **Reads** `get_str(*h)` → `get_str(h)` (drop the deref; `Arc<str>` is not
  `Copy`, so `*h` moved out of a shared reference). 82 sites.
- **Handle extraction** `Value::Str(h) => *h` → `Value::Str(h) => h.clone()`.
  4 sites (str_ops char/slice).
- **Deref-clone passthrough** `Value::Str(*h)` → `Value::Str(h.clone())`. 3 sites.
- **Move-twice** `ir_constructors.rs` `ir_rename` used one handle in two
  `Value::Str(...)` — first use now `.clone()`.
- **Sentinel** `coerce.rs` test `Value::Str(0)` → `Value::Str("".into())`.
- **Tests**: `mem_battle_test.rs` `let handles: Vec<u32>` → `Vec<Arc<str>>` and
  a `for (i, &h)` → `for (i, h)` (both literally encoded the old u32 repr, so
  both are call sites being migrated). See the honesty note in §2.

### Untouched, by constraint
- `intern_tag` / `TAG_TABLE` / `TAG_MAP`: **zero diff lines** (INTERN_TAG_UNTOUCHED,
  verified with `git diff`).
- `interner_lockfree_feed.rs`, `interner_mutex_feed.rs`, `interner_shard.rs`,
  `mpsc_intrusive.rs`: **not deleted and not edited** — still `??` (untracked)
  in git (WIP_FILES_NOT_DELETED_THIS_PASS). They turned out to be fully
  self-contained experiments that define their *own* `intern_str`/`get_str`
  (returning u32) and reference `Value::` zero times, so the migration neither
  broke them nor needed to remove any "str-side caller" (there were none to
  remove — they have no external callers except the `pub mod` lines in the
  pre-existing uncommitted `mod.rs`).

### New (measurement only)
- `src/bin/interner_contention.rs` — the isolation-measurement harness (§3).

---

## 2. What passed

- **Full test suite: 363 passed, 0 failed, 2 ignored (pre-existing), across 15
  binaries.** Baseline before the change was the same suite, all green; the
  count is identical modulo nothing. This satisfies FULL_TEST_SUITE_MUST_PASS.
- The interner-semantics tests in `mem_battle_test.rs` (concurrent dedup, large
  volume, empty/unicode, mixed shared/unique) **all pass** — their `assert_eq!`
  on interned values now compares `Arc<str>` contents, which yields the same
  equal-string ⇒ equal result the handle comparison did.
- Release build clean; the emitted-code path (`emit/rust_05.rs` →
  `Value::Str(intern_str(...))`) recompiles and links.
- **Honesty note on "unmodified suite":** two lines in `mem_battle_test.rs`
  (`Vec<u32>` and `&h`) literally hard-coded the *old* representation and could
  not survive the type change; they were migrated as call sites (the intent's
  boundary explicitly allows call-site migration, and the failure-test demands
  *zero* remaining references to the old u32-handle path). No test assertion or
  behavior was weakened. Flagging because the success narrative said "unmodified."

---

## 3. What the isolation measurement showed

Harness: fix total work (24M ops), split across P threads, report speedup vs
P=1. A lock-free mechanism → near-linear speedup; a single-mutex mechanism →
plateau/negative scaling once cores contend. The **TAG path is a faithful
in-tree replica of the OLD str-interner mechanism** (identical
`Mutex<HashMap>`+`Mutex<Vec>` dedup-under-lock shape), which is exactly why
measuring it in isolation reconstructs the pre-migration str behavior. Machine:
32 cores. Two runs, stable; representative numbers:

| P  | STR (migrated, Arc, lock-free) | TAG (untouched Mutex = old-str mechanism) | MIXED |
|----|-------------------------------:|------------------------------------------:|------:|
| 1  | 1.00×   (19.0 M ops/s)         | 1.00×  (18.6 M ops/s)                     | 1.00× |
| 2  | 1.97×                          | **0.41×**                                 | 1.95× |
| 4  | 3.77×                          | **0.27×**                                 | 0.94× |
| 8  | 7.73×                          | **0.24×**                                 | 0.58× |
| 16 | 8.10×                          | 0.24×                                     | 0.49× |
| 32 | 12.68×  (243 M ops/s)          | 0.22×  (4.2 M ops/s)                      | 0.47× |

- **STR (migrated):** near-linear to P=8 (7.7×), throughput keeps *rising* to
  P=32, **no collapse** — no new P≥8 contention was introduced on the str path.
- **TAG (the old mechanism, untouched):** **negative scaling from P=2** (32
  cores end up ~4.5× *slower* than one), fully saturated by P=8. This is the
  classic single-mutex collapse — and it is precisely the Spike-4 P≥8 pattern.
- **MIXED:** fine at P=2 (only one TAG thread), then the TAG halves serialize on
  the mutex and drag the whole workload down from P≥4.

**Conclusion (decisive, reproducible):** Spike 4's P≥8 contention is
attributable to the **Mutex-interner mechanism**, not to string-ness. The
migrated str path (Arc<str>, no shared lock) scales cleanly. The tag path, which
retains the identical mechanism by constraint, reproduces the collapse in
isolation. This resolves the intent's unknown: it was a *mechanism* problem that
lived on the str path (now removed) and still lives, latent, on the tag path.

### Clone throughput + end-to-end regression check
- **Clone (single thread): ~117 M clones/s, ~8.5 ns/clone** (Arc atomic bump),
  contention-free. Honest nuance: an *isolated* clone is marginally *more*
  expensive than the old `u32` Copy (~1 ns) — but clone never hit the interner;
  construction did, and construction is where the win is.
- **No-regression vs AXVERITY_HOTWRITE_RAM_THROUGHPUT_SPIKE_V1** (end-to-end M1
  binary rebuilt against the migrated bridge, 7 trials each, byte-identical
  workload output — 30131 records / 5242794 bytes / 10 rotations):
  - **Before (pre-migration bridge): mean 80,716 rec/s** (median 80,326, CV 1.5%)
    — matches the recorded ~81.6k baseline.
  - **After (Arc<str> bridge): mean 128,271 rec/s** (median 128,326, CV 1.7%).
  - **+59%.** Not merely no regression — a large improvement, because the write
    path constructs many *unique* Str values per record; the old path paid
    lock + hashmap-insert + double String-alloc into an ever-growing table per
    string, the new path is one Arc allocation with no lock and no shared table.

---

## 4. Recommendation on tag-side WIP scaffolding (for Chris — a recommendation, not a decision)

The measurement gives evidence, so the disposition is no longer a coin-flip:

1. **The str-side WIP experiments** (`interner_shard.rs`,
   `interner_lockfree_feed.rs`, `interner_mutex_feed.rs`) were built to fix the
   *str* interner's contention. That interner no longer exists — `Value::Str` is
   `Arc<str>`, which needs no interner — so as **str-side** code these files are
   now dead. Retirement is justified. (Deletion remains a separate, not-yet-
   authorized pass, per the intent; they are left in place, untouched.)

2. **But the contention they were built to solve is real and still present — on
   the tag path.** The isolation run shows `intern_tag`'s Mutex interner
   collapses from P=2 and saturates by P=8. So the *design* those experiments
   prototyped (a lock-free / sharded interner) is **not worthless — it is aimed
   at the wrong table.** My recommendation: **retain the WIP as a repurpose
   template for a future tag-interner fix rather than discard the design**, and
   treat "give `intern_tag` a lock-free/sharded treatment" as a real, evidence-
   backed follow-on — *gated on whether tag interning is ever on a hot multi-core
   path* (today `intern_tag` fires on Ctor-tag construction; how hot that is
   under the demo's multi-writer load is the scoping question, and it is yours).

In short: the "tag-side scaffolding survival" sub-question resolves to **survive
as a design template, because the tag path demonstrably still carries the P≥8
contention** — not "keep the str-side files running" (they're dead as str-side)
and not "discard the idea" (the idea is now the tag path's fix).

---

## 5. Reintegration check (before final conclusions)

- **identity M1_VALUE_STR_ARC_IMPLEMENTATION_V1** — implemented exactly the
  decided representation; scope respected (tag mechanism excluded; WIP files not
  deleted; accumulator-shape spike not touched).
- **VALUE_MUST_STAY_SEND_SYNC** — Arc<str> is Send+Sync; enforced permanently by
  a compile-time assertion; the Send-critical sites (main.rs `--entries`
  thread::spawn, channels.rs `Mutex<VecDeque<Value>>`) compile and their tests
  pass. ✔
- **INTERN_TAG_UNTOUCHED** — zero diff lines touching intern_tag/TAG_TABLE/
  TAG_MAP. ✔
- **NO_NEW_SHARED_STRUCTURE** — one shared lock-guarded structure removed, none
  added. ✔
- **DERIVES_MUST_KEEP_WORKING** — `#[derive(Clone, Debug, PartialEq)]` unchanged,
  compiles, semantics preserved (content equality). ✔
- **WIP_FILES_NOT_DELETED_THIS_PASS** — all four files still `??` in git,
  unmodified. ✔
- **FULL_TEST_SUITE_MUST_PASS** — 363/363 (2 pre-existing ignored). ✔
- **priority correctness/soundness/test-integrity > perf** — honored; the missed-
  call-site risk (dominant failure mode) was ruled out by a fresh full-tree grep
  showing zero remaining live u32-handle references. ✔
- **authority AI_BOUNDED** — implementation done; the tag-scaffolding disposition
  and WIP-file deletion are left as recommendations/decisions for Chris. Nothing
  was committed (the bridge repo also carries pre-existing uncommitted WIP; a
  commit is Chris's call and would need to be scoped to exclude that WIP). ✔

**Final conclusion:** the representation change is implemented across every call
site with the suite green; the isolation measurement decisively attributes Spike
4's P≥8 contention to the Mutex-interner mechanism (str path fixed, tag path
latent); and the end-to-end write path is not just regression-free but 59%
faster.

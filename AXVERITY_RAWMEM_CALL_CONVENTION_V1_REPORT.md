# AXVERITY_RAWMEM_CALL_CONVENTION_V1 — crate-wide native calling-convention conversion

**Date:** 2026-08-14/15
**Scope:** `axis-codegen-bridge-rs` (this crate), verified against its three
live downstream consumers: `axVerity-working/graphcore`, `axVerity-working2`,
`axSemantica-v2` (confirmed to have no dependency on this crate — nothing to
verify there).

## What changed

256 bridge builtin functions were converted from the original boxed calling
convention (every fn `fn(args: Value) -> Value`, multi-arg calls packed into
a `Value::Tuple` at the call site, unpacked via an internal `match`/`panic!`)
to native positional Rust parameters (`fn(x: i64, y: Arc<str>, ...) -> Value`,
with `Value::as_int()`/`as_text()`/`as_bytes()`/`as_bool()` applied at the
*call site* instead).

This started as a narrow, explicitly-scoped decision — converge the ~23
functions `axVerity-working2`'s write-path slice depends on, after a
benchmark (`rawmem_call_bench`) showed the boxed convention's `Value::Tuple`
Vec allocation costs ~30-40ns/call, 7-9x the cost of a primitive like
`mem_copy_raw`'s own body at small argument sizes. Chris then directed
"apply it crate-wide" — not because the perf win generalizes (it doesn't;
every downstream write path here is fsync-dominated, where a few dozen
nanoseconds per call is noise), but because the native signature is a
**strictly clearer interface** (arity and types visible without reading the
body) and **compiles-checks misuse** that the uniform `fn(Value) -> Value`
shape could only catch at runtime.

## Method

Three batches, each: parallel per-file conversion (subagents editing
disjoint file sets only — never the shared dispatch tables, to avoid edit
races), then a single sequential integration pass (shared-table entries +
`ir_eval.rs` dispatch-table wrappers + any cross-file call-site fixes),
then a full `cargo test --release --lib --tests` run, then a rebuild +
test of every live downstream consumer, before moving to the next batch.

- **Batch 1** (76 fns) — pure utility primitives: `str_ops.rs`, `list.rs`,
  `tuple.rs`, `option.rs`, `arith.rs`, `bool_ops.rs`, `hash.rs`, `coerce.rs`
  (6 of 6 correctly left boxed — see below), `bytes_codec.rs`, `bytes_io.rs`,
  `process.rs`, `channels.rs`.
- **Batch 2** (~128 fns) — the rest of the non-frozen runtime: IR
  constructors/accessors, io/net/tty, registry/cursor/transitions/scratch,
  contradicts/hotmem/qhm/pkindex/pgbshape/oneshot/walshard/rawmem remainder,
  walindex/fieldidx/logbuf/slablock/mmapseg, indexer/hotwrite_batch/adjacency
  + 14 smaller files.
- **Batch 3** (22 fns) — graphcore's CLAUDE.md-designated "frozen bridge
  surface": `pg_store.rs`, `block_flush.rs`/`hotblk.rs`/`hotblk_pool.rs`,
  `hotblk_recover.rs`, `rawblk.rs`/`tsmark.rs`. Converted under stricter
  rules (precision over coverage, skip on any uncertainty) and gated on
  graphcore's own adversarial stress scripts, not just the standard suite.

`native_call_fn_arg_types()` (`src/emit/rust_05.rs`) now has 256 entries,
one per converted fn, each a `Vec<NativeArgType>` naming the exact
accessor the codegen applies per argument position.

## Bugs found and fixed along the way

1. **`make_ccall_bundle`'s `target_name: String::new()`** — the codegen's
   builtin resolver keys `native_call_fn_arg_types` lookups off `target_name`
   (not the resolved symbol path). An empty name silently fell back to the
   stale boxed convention while the underlying Rust fn was already native —
   a real, silent miscompile, caught by `test_05_build_ccall_int_add`
   actually failing (not just failing to compile — the *test itself* was the
   signal). Fixed by adding `make_ccall_bundle_named` and giving the two raw
   test bundles (`int_add`, `bool_not`) their real names.
2. **`loop_count`'s `native_call_fn_arg_types` entry was 2 long, not 3** —
   `loop_count(n, init, step)` is 3-ary; the short entry meant the arg-count
   gate rejected every `loop_count` call in `axVerity-working2`. Caught by
   rebuilding that repo, not by the bridge's own suite (a reminder that a
   library's own green suite doesn't prove a downstream consumer is fine).
3. **`value_eq`'s registered alias `__eq__`** — same Rust symbol, separate
   registry name; needed its own `native_call_fn_arg_types` entry (same
   failure class as #1). Caught by a subagent auditing its own work before
   reporting, not by a build failure — the alias has no current call site
   anywhere, so it would have shipped silently broken and only surfaced the
   day something finally called `__eq__`.

## What was deliberately left boxed, and why

Not every fn was eligible, and the reasons form a real taxonomy, not just
"didn't get to it":

- **Genuinely polymorphic single-arg fns** — `io_println` (must accept
  Int/Bool/Text/Unit interchangeably), `coerce.rs`'s 6 dispatchers, all 10
  of `ir_accessors.rs` (destructure a generic `Value::Ctor` IR term whose
  shape depends on which node kind it is), `registry.rs`'s `name_from_value`
  (accepts `Str` or `Unit`), all 8 of `transitions.rs` (bare identity
  passthroughs by design).
- **FnRef/HOF callees** — anything ever passed as a bare `fn(Value) -> Value`
  into `foreach`/`fold`/`loop_while`/`wait`/`bridge_to_dec`/`bridge_to_float`
  MUST keep the uniform shape: `coerce.rs`'s 6 converters, `block_flush.rs`'s
  `block_flush_write` (`wait()`'s handler), `logbuf.rs`'s
  `wal_fast_batch_write`. Native params are heterogeneous — you cannot hold
  them behind one fn-pointer type.
- **Lenient/graceful-fallback fns** — anything that returns a soft
  fallback (an `Err` `Ctor`, an empty string, a default) instead of
  panicking on off-shape input. Converting would silently turn a recoverable
  path into a hard panic — a real behavior change, not a reshape.
  `registry_insert`, `registry_compound_id`, `ir_bundle_view`,
  `frontend_lookup_shape`/`frontend_walk`, `rawblk_frame`.
- **Gated-telemetry fns with a documented off-equivalence contract** — all
  4 of `tsmark.rs` and `rawblk.rs`'s `derive_stat`/`derive_stat_flush` check
  `!telemetry_enabled()` and return *before* touching their argument at all.
  With telemetry off (the default), a wrong-typed call today silently
  no-ops; a native accessor would panic unconditionally regardless of the
  gate. `tsmark.rs`'s own doc comment states byte-identical off-behavior as
  a hard invariant — left entirely untouched.
- **Discarded-argument fns** — `_: Value`/`Unit` params with nothing to
  convert to and zero benefit (a long tail across most files).

`native_call_fn_arg_types` also documents this in its own header comment;
see `src/emit/rust_05.rs`.

## Verification

Each batch was verified independently, not just at the end:

| Gate | Batch 1 | Batch 2 | Batch 3 |
|---|---|---|---|
| `cargo build --release` (lib+bins) | clean | clean | clean |
| `cargo test --release --lib --tests` | 0 failed | 0 failed | 0 failed |
| graphcore `tests/run.sh` (107 cases) | 107/107 | 107/107 | 107/107 |
| graphcore targeted checks | — | `cursor_sort` ASC/DESC/stability tests, `pgbshape` bind/record tests | `hotblk-pool-stress.sh`, `hotblk-timer-flush.sh` (adversarial D044 probes) |
| `axVerity-working2` slice test | all PASS | all PASS | all PASS |

The batch-2 and batch-3 targeted checks weren't generic re-runs — they were
picked because those specific functions had documented prior-incident
history (`cursor_sort`: a past silent ASC/DESC regression) or sat on the
live concurrency surface a generic pass/fail count wouldn't stress
(`hotblk_pool`'s free-marker protocol under churn, the independent flush
timer thread).

## What's still open

- `docs/turn-history.md`-style write-up for `axVerity-working`/graphcore
  itself was not created — this report lives only in this crate, since
  graphcore's own source was not modified (only rebuilt against this crate).
- All three repos (`axis-codegen-bridge-rs`, `axVerity-working`,
  `axVerity-working2`) are currently **uncommitted**. Awaiting explicit
  go-ahead to commit (one commit per repo, straight to `main`, no push).
- The remaining ~120 boxed fns are not a backlog — see the taxonomy above.
  Any *new* fn added to this crate should default to native unless it falls
  into one of those categories.

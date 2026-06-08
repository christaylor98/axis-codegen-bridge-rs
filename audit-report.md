# Quality & Test Audit — axis-codegen-bridge-rs

**Date:** 2026-06-08  
**Rustc:** 1.96.0 (stable)  
**Auditor:** Claude (automated, multi-phase)

---

## Summary

The codebase is a Rust library/CLI that compiles Axis Core IR (0.4 and 0.5) into Rust static libraries via `rustc` invocation. Overall health is **moderate**. The test suite is broad (153 tests, all passing after a clean build) and covers the primary compilation pipelines well. However, there are three significant quality gaps: (1) the `append_fn_to_registry` function in `main.rs` writes **forbidden axreg fields** (`arity`, `profile`) that violate the project's own `CLAUDE.md` spec; (2) the 0.4→0.5 lowering path (`core_ir_05::lower`) has zero test coverage; (3) the registry module (`runtime/registry.rs`) — a stateful, file-backed system — has zero tests. The runtime is also pervasively `panic!`-on-bad-input with no graceful degradation, which is acceptable for generated code today but becomes a hardening concern as the surface grows. The test infrastructure has a fragile rlib-staging requirement that causes all CLI integration tests to fail out of a clean checkout without a documented workaround.

---

## Test Results

| Status  | Count |
|---------|-------|
| Passed  | 153   |
| Failed  | 0     |
| Skipped | 0     |

Tests are distributed across four harnesses:

| File                        | Tests |
|-----------------------------|-------|
| `src/runtime/ir_constructors.rs` (inline) | 13 |
| `tests/integration.rs`      | 118   |
| `tests/cli_build_test.rs`   | 7     |
| `tests/cli_build_05_test.rs`| 7     |
| `tests/link_test.rs`        | 8     |

### Pre-condition required to pass

`cargo clean` + a fresh build leaves `target/debug/libaxis_codegen_bridge.rlib` absent. The bridge binary locates its rlib via `std::env::current_exe().parent()`, resolving to `target/debug/`. The actual rlib is hashed into `target/debug/deps/libaxis_codegen_bridge-<hash>.rlib`. All six `cli_build_05_test` tests and all seven `cli_build_test` tests fail until the rlib is staged:

```sh
# Required before running tests against a fresh build:
cp $(find target/debug/deps -name 'libaxis_codegen_bridge*.rlib' | head -1) \
   target/debug/libaxis_codegen_bridge.rlib
```

This is not documented and is not automated. See Recommended Action #4.

---

## Flow Coverage Matrix

| Flow | Coverage | Notes |
|------|----------|-------|
| **0.4 CoreIR build → .a** (`cmd_build`, lib-only) | Full | `test_build_lib_only`, `test_single_export_backward_compat` |
| **0.4 CoreIR build → .a + binary** (`--exe`) | Full | `test_build_with_exe`, `test_stem_abs_calls_lib_int_abs` |
| **0.4 multi-export bundle** | Full | `test_multi_export_lib_both_symbols`, `test_multi_export_exe_calls_first` |
| **Archive bundle merge** (`cmd_bundle`) | Full | `test_bundle_merges` |
| **0.5 CoreBundle build → .a** | Full | `cli_build_05_test` covers unit/bool/int/ccall/cif/inspect |
| **0.4 → 0.5 lowering** (`cmd_build05`, `lower_core_term_to_bundle_05`) | **None** | Zero tests. No CLI test exercises `build05`; the lowering function is unreachable in the standard `build` path for 0.4 input |
| **Core IR round-trip serialisation (0.4)** | Full | `integration.rs` round-trip tests cover all node types |
| **CCall validation / emit** | Full | `link_test.rs` covers resolved, unresolved, registry-declared targets |
| **IR evaluation** (`ir_eval`, `ir_apply`) | Full | `integration.rs` covers let-binding, if/else, closures, recursive map |
| **Registry CRUD** (`runtime/registry.rs`) | **None** | `registry_insert`, `registry_verify_chain`, `registry_has_entry` — all untested |
| **Frontend walk / shape lookup** | **None** | `frontend_walk`, `frontend_lookup_shape` — both untested |
| **Error paths: missing file / rustc absent** | **None** | `cmd_build` error exits not exercised |
| **`--lib-dir` expansion and dedup** | Partial | `test_stem_abs_calls_lib_int_abs` exercises `--lib-dir`; duplicate function name error path untested |
| **Registry auto-append on build** (`append_fn_to_registry`) | **None** | The function is called in `cmd_build` but no test verifies the written file |

---

## Dead Code

- **`src/runtime/value.rs:17–27` — `Value::as_int()` and `Value::as_bool()`**  
  Both are `pub` but have zero callers anywhere in `src/` or the test suite. They panic on wrong input type. Likely dead API surface from an earlier design.

- **`src/runtime/transitions.rs` — all 8 functions**  
  All are identity stubs: `fn introduce_let_binding(v: Value) -> Value { v }`, etc. They are listed in `emit/rust.rs`'s symbol map (so are technically reachable if a generated program calls them), but have no meaningful implementation and are never called by any test. Likely placeholder stubs awaiting real implementation.

- **`src/main.rs:63` — `cmd_build05` subcommand**  
  The `build05` subcommand (`axis-codegen-bridge build05 <in> <out>`) lowers 0.4 → 0.5 and writes a bundle. It is documented in `usage()` but has no test. The more common `build` path auto-detects 0.5 bundles, making `build05` mostly redundant.

- **`src/executor.rs` — `FunctionProvider`, `execute_core_program`**  
  This is a tree-walking interpreter used only in `tests/integration.rs`. It is exposed as `pub` library API (`pub mod executor`) but has no external consumer. It could be gated behind `#[cfg(test)]`.

---

## Hardening Risks (HIGH → LOW)

### HIGH

**1. `append_fn_to_registry` writes FORBIDDEN axreg fields — `main.rs:96–104`**  
The function writes entries containing `arity` and `profile`, which are explicitly forbidden by `CLAUDE.md` ("`arity` — not a real axreg field"; "`profile` — wrong keyword; correct keyword is `effect`"). Any `.axreg` file auto-populated by `--reg` during `build` will fail axreg validation.  
*Fix:* Replace with valid fields: `effect`, `deterministic`, `idempotent`, `kind`, `in`, `out`. Generate an `identity` hash via `sha256(name)` per §5b convention. Do not write `arity` or `profile`.

**2. Pervasive `panic!` on type mismatch across all runtime bridge functions**  
All ~50 runtime functions in `arith.rs`, `str_ops.rs`, `list.rs`, `io.rs`, `ir_constructors.rs` panic immediately on a wrong-type argument (e.g., `panic!("int_add: expected Tuple(Int, Int)")`). These functions are called from `extern "C"` entry points in generated programs with no `catch_unwind`. A single type mismatch in user-generated code crashes the entire host process with no error message propagation.  
*Fix:* Accept that this is a type-safe internal ABI (generated code controls types) for now, but add `#[track_caller]` to improve diagnostics. For any user-facing path, consider wrapping with `std::panic::catch_unwind` in the `extern "C"` shim.

**3. `str_to_int` silently returns 0 on parse failure — `arith.rs:101`**  
`text.parse().unwrap_or(0)` gives no error signal. A generated program calling `str_to_int("abc")` receives `Value::Int(0)` silently.  
*Fix:* Return an `Option`-wrapped value or match `int_div_checked` pattern; or at minimum document the silent-zero contract.

**4. `int_div` and `int_mod` panic on divide-by-zero — `arith.rs:43,72`**  
Unlike `int_div_checked`, the unchecked variants terminate the process.  
*Fix:* Document clearly; add a compile-time note encouraging use of `int_div_checked` where the divisor is user-controlled.

### MEDIUM

**5. Registry chain integrity uses `DefaultHasher` — `registry.rs:compute_hash`**  
`DefaultHasher` is randomised per-process and not stable across Rust versions. The `registry_verify_chain` function's integrity guarantee is meaningless. The project already depends on `sha2`.  
*Fix:* Use `sha2::Sha256` for chain hashing.

**6. Mutex `unwrap()` in string table — `value.rs:85,88,97,108,111,120`**  
All `STRING_TABLE` and `STRING_MAP` lock acquisitions use `.unwrap()`. If any thread panics while holding the lock, the mutex is poisoned and all subsequent calls to `intern_str` / `get_str` will also panic.  
*Fix:* Use `.unwrap_or_else(|e| e.into_inner())` (recover from poisoned mutex) or restructure to avoid shared mutable state.

**7. Cap'n Proto traversal limit is 1 GB — `core_ir_05/loader.rs:35`**  
`opts.traversal_limit_in_words = Some(1 << 30)`. A maliciously crafted `.coreir` bundle could cause the loader to allocate up to 8 GB before hitting the limit.  
*Fix:* Reduce to a more conservative limit (e.g., 64 MB) appropriate for the expected bundle sizes.

**8. No path traversal protection in `fs_read_text`, `fs_write_text`, `fs_append_text`**  
Generated programs can call these with arbitrary paths. There is no sandboxing or allow-list.  
*Fix:* Acceptable for the current use case (generated programs are trusted); document the assumption explicitly.

### LOW

**9. `#[no_mangle] pub extern "C" fn` uses non-FFI-safe `Value` type**  
The compiler emits `improper_ctypes_definitions` warnings for all generated library functions. The `Value` enum has no `#[repr(C)]`. This causes compiler noise and is technically undefined behaviour if the `.a` is linked against non-Rust code.  
*Fix:* Add `#[allow(improper_ctypes_definitions)]` to the generated file header (already done for `#[allow(improper_ctypes)]` on `extern` blocks), or annotate `Value` with `#[repr(u8)]` / document the ABI assumption.

**10. Generated imports `truthy` and `intern_str` unconditionally — `rust_05.rs:228`**  
Every 0.5-generated file emits `use ... truthy, intern_str, init_runtime` regardless of whether these are used, producing compiler warnings on every build.  
*Fix:* Only emit the imports actually required, or add `#[allow(unused_imports)]` to the generated header.

---

## Coverage Gaps (prioritised by risk)

### Priority 1 — Business-critical untested paths

**`src/core_ir_05/lower.rs` — `lower_core_term_to_bundle_05`**  
Estimated coverage: **0%**. This is the 0.4→0.5 lowering path. Zero tests exercise any of: Let-chain lowering, App-spine flattening, CIf emission, variable resolution, or the `Lam` rejection error.  
*Recommended tests:* lower an IntLit, a Let+Var, a Call with two args, an App spine, a CIf, and confirm Lam returns an error.

**`src/runtime/registry.rs` — all 9 public functions**  
Estimated coverage: **0%**. This is a stateful, file-backed system with chain integrity. Zero tests.  
*Recommended tests:* `registry_insert` (happy path, duplicate rejection, invalid provenance), `registry_verify_chain` (valid chain, broken chain), `registry_has_entry` (found, not found, Unit input), `registry_compound_id`.

**`src/main.rs` — `append_fn_to_registry`**  
Estimated coverage: **0%** as a standalone path.  
*Recommended tests:* build with `--reg`, check that the file is written; build twice (idempotency). NOTE: this also surfaces the CLAUDE.md violation (action #1).

### Priority 2 — Runtime functions missing any test

**`src/runtime/str_ops.rs`** — 7 of 14 public functions untested: `str_char_code`, `str_slice`, `str_ends_with`, `str_trim`, `str_contains`, `str_index_of`, `chr`.

**`src/runtime/arith.rs`** — 5 of 10 public functions untested: `int_mod`, `int_abs` (tested via CLI only), `int_eq`, `unit_id`, `seq_unit`.

**`src/runtime/list.rs`** — 3 of 16 untested: `list_concat`, `list_str_len_lte_if_some`, `list_get_println_if_some`.

### Priority 3 — Infrastructure / IO

**`src/runtime/io.rs`** — All 8 functions have no direct unit tests. `io_print`, `io_println`, `io_eprint` are trivially tested (just print), but `io_read_line`, `fs_read_text`, `fs_write_text`, `fs_append_text` have meaningful behaviour including error paths.

**`src/runtime/frontend.rs`** — `frontend_walk` (complex multi-step logic with registry checks, RESOLVED/UNKNOWN/NEED classification) has **0% coverage**. This is medium-risk given its complexity.

**`emit/rust_05.rs` — `load_registry_identity_map`** — tested implicitly by CLI tests when `--reg` is supplied, but no unit test covers the parsing logic directly (no-identity-line fallback, `end`-less file, malformed hex).

### Coverage estimate by module

| Module | Est. Coverage | Gap |
|--------|--------------|-----|
| `runtime/value.rs` | ~80% | `as_int`, `as_bool`, `get_tag_name` untested |
| `runtime/arith.rs` | ~50% | 5 functions untested |
| `runtime/str_ops.rs` | ~50% | 7 functions untested |
| `runtime/list.rs` | ~80% | 3 functions untested |
| `runtime/bool_ops.rs` | 100% | — |
| `runtime/option.rs` | ~70% | `option_none_fn` shim untested |
| `runtime/io.rs` | ~20% | No unit tests |
| `runtime/process.rs` | ~40% | `proc_args`, `argv_get`, `proc_exit` untested |
| `runtime/registry.rs` | **0%** | Priority gap |
| `runtime/transitions.rs` | **0%** | Stubs, intentional |
| `runtime/ir_constructors.rs` | ~85% | `ir_bundle_view`, `ir_build_program_from_spec` coverage thin |
| `runtime/ir_accessors.rs` | ~90% | — |
| `runtime/ir_eval.rs` | ~80% | CCall error paths, deep recursion not tested |
| `runtime/frontend.rs` | **0%** | Priority gap |
| `core_ir/loader.rs` | ~70% | Load-error paths untested |
| `core_ir_05/lower.rs` | **0%** | Priority gap |
| `core_ir_05/loader.rs` | ~60% | Malformed input paths untested |
| `emit/rust.rs` | ~70% | App fallback path, sanitise edge cases untested |
| `emit/rust_05.rs` | ~65% | `load_registry_identity_map`, Text pool entry untested |
| `executor.rs` | ~80% | Lam/App rejection not tested |

---

## Recommended Next Actions

1. **(S) Fix `append_fn_to_registry` — `main.rs:96–104`.**  
   Replace `arity`/`profile` with the valid axreg field set per `CLAUDE.md`. This is a spec violation that corrupts auto-generated registry files.

2. **(M) Stage rlib correctly in CI / test setup.**  
   Add a shell script or `build.rs` post-build step that copies the hashed rlib to `target/debug/libaxis_codegen_bridge.rlib`. Without this, `cargo test` fails on a clean checkout. Document the requirement clearly.

3. **(M) Add tests for `core_ir_05::lower::lower_core_term_to_bundle_05`.**  
   Cover: literal lowering (Int, Bool, Unit), Let-chain, Call with multiple args, App spine flattening, CIf, Lam rejection, unbound variable error.

4. **(M) Add tests for `runtime/registry.rs`.**  
   Cover: happy-path insert, duplicate rejection, invalid provenance rejection, chain verification (valid + broken), `registry_has_entry`, `registry_all_entries`.

5. **(S) Replace `DefaultHasher` in `registry::compute_hash` with SHA-256.**  
   The chain integrity check is currently meaningless across Rust versions or processes.

6. **(M) Add unit tests for uncovered `str_ops`, `arith`, and `list` functions.**  
   Priority: `str_slice`, `str_trim`, `str_contains`, `str_ends_with`, `chr`, `int_mod`, `int_eq`, `list_concat`.

7. **(S) Add tests for `frontend_walk`.**  
   The REGISTRY_CHECK / NEED / UNKNOWN classification logic is complex and has no test coverage.

8. **(S) Gate `executor.rs` behind `#[cfg(test)]`.**  
   It is a test-only tree-walker exported as public library API. Removing it from the public surface reduces the API footprint.

9. **(M) Add `#[track_caller]` to all `panic!`-on-bad-input bridge functions.**  
   This makes crash diagnostics far more actionable without changing behaviour.

10. **(L) Evaluate replacing `panic!` with `Result`-returning runtime functions.**  
    Long-term hardening: generated code should be able to handle type errors gracefully rather than aborting the host process. Coordinate with the codegen pipeline before changing the ABI.

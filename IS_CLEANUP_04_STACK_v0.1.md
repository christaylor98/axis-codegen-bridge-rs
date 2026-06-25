# IS: Delete the 0.4 IR stack — one bundle format, Core IR 0.5 only
#
# Run from: axis-codegen-bridge-rs
# For: Claude Code
#
# Context: the bridge accumulated a parallel 0.4 IR track (core_ir, emit/rust.rs,
# executor.rs, core_ir_05/lower.rs) alongside the authoritative 0.5 track
# (core_ir_05, emit/rust_05.rs). Both tracks live in cmd_build as a try-0.5 /
# fallback-to-0.4 pattern. The 0.4 track is dead weight. Delete it entirely.
# One format. 0.5. Done.
#
# BEFORE TOUCHING ANYTHING: read the WARNING block below.

```lisp
(intent-spec
  (intent-id CLEANUP_04_STACK_v0.1)
  (version INTENT_SYSTEM_SPEC.v1.0)

  (scope
    (primary axis-codegen-bridge-rs)
    (test-repo /home/chris/dev/M1-lang-test)
    (includes
      axis-codegen-bridge-rs/src/core_ir/           ;; DELETE entire directory
      axis-codegen-bridge-rs/src/emit/rust.rs        ;; DELETE
      axis-codegen-bridge-rs/src/executor.rs         ;; DELETE
      axis-codegen-bridge-rs/src/core_ir_05/lower.rs ;; DELETE — 0.4→0.5 lowering, dead once 0.4 is gone
      axis-codegen-bridge-rs/src/lib.rs              ;; EDIT — remove core_ir mod + 0.4 capnp block
      axis-codegen-bridge-rs/src/emit/mod.rs         ;; EDIT — remove `pub mod rust;`
      axis-codegen-bridge-rs/src/core_ir_05/mod.rs   ;; EDIT — remove `pub mod lower;` + re-export
      axis-codegen-bridge-rs/build.rs                ;; EDIT — remove 0.4 capnp compile step
      axis-codegen-bridge-rs/src/main.rs             ;; EDIT — see (surgery-main) below
      axis-codegen-bridge-rs/src/runtime/ir_constructors.rs ;; INVESTIGATE before touching — see WARNING)
    (excludes
      core_ir_spec/axis_core_ir_0_4.capnp            ;; leave schema file on disk (history)
      src/core_ir_05/                                ;; keep — this is the live track
      src/emit/rust_05.rs                            ;; keep
      registry/*.axreg                               ;; read-only per CLAUDE.md))

  ;; ── WARNING: ir_constructors.rs ─────────────────────────────────────────────
  ;; src/runtime/ir_constructors.rs line 1 imports:
  ;;   use crate::core_ir::{CoreTerm, Provenance, EffectClass,
  ;;                         write_core_bundle_to_file, write_core_bundle_multi_to_file,
  ;;                         load_core_bundle};
  ;;
  ;; This file has ~419 functions. It is used by bin/make_test_fixtures.rs,
  ;; which currently writes 0.4-format bundles that cmd_build05 then lowers to 0.5.
  ;;
  ;; BEFORE deleting core_ir, you MUST determine:
  ;;   (a) Does M1-lang-test consume .coreir files that were produced by make_test_fixtures?
  ;;   (b) Are those .coreir files 0.4 or 0.5 format?
  ;;   (c) If 0.4: ir_constructors.rs must be rewritten to produce CoreBundle (0.5) directly
  ;;       before core_ir can be deleted. That is a SEPARATE IS.
  ;;   (d) If 0.5 (or M1 compiler outputs 0.5 directly and fixtures are unused):
  ;;       the import line can simply be removed and dead code eliminated.
  ;;
  ;; DO NOT DELETE core_ir until (a)-(d) are resolved.
  ;; Surface the answer to the human before proceeding past the baseline step.

  (phase-1-baseline
    (step "Run: cargo test --workspace 2>&1 | tee /tmp/baseline-bridge.txt")
    (step "Run tests in /home/chris/dev/M1-lang-test — check for Makefile or scripts/test.sh")
    (step "Record: how many tests pass, any failures, exact counts")
    (step "Run: git -C axis-codegen-bridge-rs status && git -C M1-lang-test status")
    (step "Commit any uncommitted work in both repos with message 'pre-cleanup baseline'")
    (gate "STOP: surface baseline results to human before phase 2. Do not proceed if any test is red."))

  (phase-2-investigate-ir-constructors
    (step "Read src/runtime/ir_constructors.rs — identify every use of CoreTerm, Provenance, EffectClass, write_core_bundle_to_file, write_core_bundle_multi_to_file, load_core_bundle")
    (step "Read bin/make_test_fixtures.rs — determine what format it writes")
    (step "Check M1-lang-test: does it call the bridge binary with .coreir files? What generates those files?")
    (gate "STOP: report (a)-(d) from WARNING block to human. Await decision before touching ir_constructors.rs or deleting core_ir."))

  (phase-3-safe-deletes
    ;; These are safe regardless of ir_constructors outcome — they have no dependents
    ;; other than what is being deleted in the same phase.
    (note "Only proceed here after human authorises based on phase-2 report.")
    (deletes
      src/emit/rust.rs
      src/core_ir_05/lower.rs)
    (edits
      (src/emit/mod.rs        "remove `pub mod rust;`")
      (src/core_ir_05/mod.rs  "remove `pub mod lower;` and `pub use lower::lower_core_term_to_bundle_05;`")
      (build.rs               "remove the first CompilerCommand block (0.4 capnp compile)"))
    (step "cargo build — must compile before continuing"))

  (surgery-main
    ;; Changes to src/main.rs after core_ir is cleared to delete
    (remove "use axis_codegen_bridge::core_ir;" — import line ~5")
    (remove "fn cmd_build05(...)" — entire function ~10 lines")
    (remove "fn append_fn_to_registry(...)" — if it uses core_ir::EffectClass and has no other callers")
    (edit   "cmd_build: remove the 0.4 fallback branch (the else arm after core_ir_05::load_core_bundle fails)")
    (replace-fallback-with
      "Err(e) => { eprintln!(\"error: input is not a valid Core IR 0.5 bundle: {}\", e); std::process::exit(1); }")
    (remove "the entire second half of cmd_build that loads libs and program via core_ir:: calls")
    (note   "cmd_build has two halves: the 0.5 arm (keep) and the 0.4 arm (delete). The 0.5 arm already handles --lib, --reg, --entries, --exe. Keep only that arm."))

  (phase-4-core-ir-delete
    ;; Only after phase-2 gate cleared AND ir_constructors.rs is clean
    (deletes
      src/core_ir/              ;; entire directory
      src/executor.rs)
    (edits
      (src/lib.rs "remove `pub mod core_ir;` and the axis_core_ir_0_4_capnp include block")))

  (constraint
    (hard-limit "DO NOT DELETE src/core_ir/ until phase-2 gate is cleared by the human.")
    (hard-limit "DO NOT modify src/runtime/ir_constructors.rs until ir_constructors dependency chain is understood.")
    (hard-limit "cargo build must pass after every phase before proceeding to the next.")
    (hard-limit "cargo test must be green at the end — same count as baseline or better.")
    (hard-limit "M1-lang-test tests must be green at the end — same count as baseline or better.")
    (hard-limit "Do not touch registry/*.axreg files.")
    (hard-limit "Do not create new versioned variants — if something needs updating, update it in place."))

  (outcome
    (baseline-captured
      (type untested-prediction)
      (test "cargo test output saved to /tmp/baseline-bridge.txt; M1-lang-test baseline captured"))
    (ir-constructors-dependency-resolved
      (type unknown)
      (test "phase-2 investigation complete; human has approved path forward"))
    (0.4-stack-deleted
      (type untested-prediction)
      (test "src/core_ir/ gone, emit/rust.rs gone, executor.rs gone, lower.rs gone; cargo build passes"))
    (single-bundle-format
      (type untested-prediction)
      (test "cmd_build no longer has a 0.4 fallback; only 0.5 path remains"))
    (tests-green-post-cleanup
      (type untested-prediction)
      (test "cargo test same count as baseline; M1-lang-test same count as baseline")))

  (next-tests
    (t1 "cargo test --workspace: same pass count as baseline")
    (t2 "M1-lang-test: all tests pass")
    (t3 "cargo build: zero errors after each phase"))

  (mode structured-design execution-allowed true)
  (status ready-for-claude-code))

(model-recommendation
  (recommended claude-sonnet-4-6)
  (tier T2)
  (floor claude-sonnet-4-6)
  (rationale "structured-design mode; contained deletion with one human gate at phase-2; no irreversible risk if cargo build is checked after each phase")
  (authority human)
  (may-decide false))
```

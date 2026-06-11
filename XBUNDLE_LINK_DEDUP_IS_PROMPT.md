You are operating under INTENT_SPEC.v0.2 (LLM-HARDENED).

(intent-id BRIDGE_XBUNDLE_LINK_DEDUP)

;; ------------------------------------------------------------
;; LONG-CONTEXT REHYDRATION ANCHOR
;; ------------------------------------------------------------
;; All constraints remain binding. Absence implies forbidden. Constraint > Priority > Goal.
;; STRUCTURAL_FIX_NOT_A_LINKER_FLAG. SINGLE_DEFINITION_OF_DROP_GLUE.
;; VALUE_ENUM_UNTOUCHED. LANG_REPO_READ_ONLY. NO_TEST_REGRESSIONS.
;; BRIDGE_XBUNDLE_LINK_DEDUP governs interpretation.

(intent

  ;; ----------------------------------------------------------
  ;; IDENTITY
  ;; ----------------------------------------------------------
  (identity
    (name "BRIDGE_XBUNDLE_LINK_DEDUP")
    (owner "Chris")
    (scope "In axis-codegen-bridge-rs: make 'multiple Core IR 0.5 bundles -> one runnable binary' the
            NORMAL, default build model — not a special case. The bridge's product is target binaries, so
            cross-bundle --exe is a primary path. Establish the model where the shared runtime — including
            the Value type and its compiler-generated drop glue (drop_in_place<Value>) — is defined ONCE
            in the shared bridge crate, and every bundle merely REFERENCES it instead of carrying its own
            copy. This eliminates the pre-existing rust-lld 'duplicate symbol: drop_in_place<Value>'.
            INCLUDES: the exe/link strategy in src/main.rs (how bundle + _xb provider rlibs are handed to
            the final rustc) and the rustc/Cargo invocation that yields a single definition of every
            shared monomorphization.
            EXCLUDES: the foreign-fn / Fn-ref work (already shipped, must stay green); any change to the
            Value type's definition; the legacy bridge; the ir_* removal work; any write to axis-lang-lab-working."))

  ;; ----------------------------------------------------------
  ;; GOAL
  ;; ----------------------------------------------------------
  (goal
    (primary "Cross-bundle --exe is the normal build path: any number of 0.5 bundles link into one
              runnable binary with the shared runtime (Value + its drop glue) defined ONCE and referenced
              by every bundle — no per-bundle copy, no duplicate symbol. The 7 cli_build_05_test
              cross-bundle failures going green is the proof, not the goal.")
    (secondary
      "The fix is STRUCTURAL — one definition of the shared drop glue across the link — not a flag
       that tolerates duplicates."
      "The produced --exe actually runs (e.g. test_05_build_xbundle_exe_runs)."
      "The link model scales to N bundles, not just 2."
      "All currently-green tests stay green, including the 19 foreign_fn_fnref_test cases.")
    (type outcome-oriented))

  ;; ----------------------------------------------------------
  ;; ACTOR
  ;; ----------------------------------------------------------
  (actor
    (Chris
      (type human) (role "Bridge owner, final authority") (authority full) (may-decide true))
    (ClaudeCode
      (type ai) (role "Implements the link fix") (authority bounded) (may-decide false)))

  ;; ----------------------------------------------------------
  ;; PRIORITY
  ;; ----------------------------------------------------------
  (priority
    (correctness high)
    (single-definition-structural high)
    (no-regressions high)
    (minimal-blast-radius medium))

  ;; ----------------------------------------------------------
  ;; CONSTRAINT — HARD LIMITS
  ;; ----------------------------------------------------------
  (constraint
    (hard-limit "STRUCTURAL FIX ONLY. The resolution must make `drop_in_place<Value>` (and other shared
                 monomorphizations) exist ONCE at final link. Do NOT resolve it with
                 -Wl,--allow-multiple-definition or -z muldefs — those tolerate the collision, they do not
                 fix it. (They are the known stopgap and are explicitly out of scope here.)"))

  (constraint
    (hard-limit "DO NOT change the Value type (src/runtime/value.rs) or add any variant. The collision is
                 a link-strategy problem, not a data-model problem."))

  (constraint
    (hard-limit "NO TEST REGRESSIONS. Every test green before this change stays green — especially
                 tests/foreign_fn_fnref_test.rs (19) and the non-failing cli_build_05_test cases.
                 Run cargo test --release and diff the pass set."))

  (constraint
    (hard-limit "DO NOT WRITE TO axis-lang-lab-working. Read-only, for reference only."))

  (constraint
    (hard-limit "Determinism is structural — the link step must stay deterministic (stable archive order,
                 stable symbol resolution). No nondeterministic dedup heuristics."))

  ;; ----------------------------------------------------------
  ;; RISK
  ;; ----------------------------------------------------------
  (risk ("Papering over with --allow-multiple-definition and calling it fixed" unacceptable))
  (risk ("The dedup picks the wrong copy / silently drops a needed symbol -> runtime miscompile" critical))
  (risk ("Breaking the single-bundle (non-xbundle) --exe path while fixing the multi-bundle one" high))
  (risk ("Regressing the §5b extern provider resolution (lib<pfn>_xb.a)" high))
  (risk ("Any edit landing in axis-lang-lab-working" unacceptable))

  ;; ----------------------------------------------------------
  ;; BOUNDARY
  ;; ----------------------------------------------------------
  (boundary
    ("Change how src/main.rs hands bundle + _xb provider rlibs to the final exe rustc invocation" allowed)
    ("Link bundle/provider archives as genuine crate deps (--extern name=rlib + -L dependency=) instead of raw -C link-arg= archives" allowed)
    ("Adjust crate-type / output kind (.rlib vs .a) of the emitted bundle + provider artifacts if that is what dedups" allowed)
    ("Add a regression test that builds an --exe from >=2 bundles and asserts no duplicate-symbol + it runs" allowed)
    ("Read tests/cli_build_05_test.rs and src/main.rs to map the current link path" allowed)
    ("Use -Wl,--allow-multiple-definition or -z muldefs as the fix" forbidden)
    ("Modify the Value type" forbidden)
    ("Write anything into axis-lang-lab-working" forbidden)
    ("Touch the legacy bridge" forbidden)
    (default forbidden))

  ;; ----------------------------------------------------------
  ;; UNKNOWN
  ;; ----------------------------------------------------------
  (unknown
    ("ROOT CAUSE (verified at planning time): the cross-bundle --exe link in src/main.rs compiles each
      bundle and each §5b provider as an rlib (--crate-type=rlib, named lib<stem>.a / lib<pfn>_xb.a) but
      then feeds them to the FINAL rustc as raw `-C link-arg=<archive>` inputs — whereas the bridge itself
      is linked properly via `--extern axis_codegen_bridge=<rlib>` + `-L dependency=`. Handed to rust-lld as
      opaque static archives, the per-rlib monomorphized `drop_in_place::<Value>` (external linkage) is NOT
      deduplicated, so two archives collide. CONFIRM this is the mechanism before fixing."))

  (unknown
    ("LEADING FIX DIRECTION to validate first: link the bundle + provider rlibs as real crate dependencies
      (`--extern <crate>=<rlib>` + `-L dependency=<dir>`) so rustc's own cross-rlib dedup keeps one copy of
      each shared monomorphization, instead of `-C link-arg=` raw archives. Verify it actually removes the
      duplicate `drop_in_place<Value>` AND still resolves the §5b extern provider symbols. If it does not
      fully dedup, fall back to: compiling the shared runtime/Value glue into a single crate all bundles
      depend on (one monomorphization site), or a shared dylib. Pick whichever provably yields one
      definition — do not guess; prove it with the failing tests."))

  ;; ----------------------------------------------------------
  ;; ASSUMPTION
  ;; ----------------------------------------------------------
  (assumption
    ("The 7 failures are pre-existing and orthogonal to the foreign-fn work (reproduced on plain main)." tentative))
  (assumption
    ("Core IR 0.5 + rust_05 emitter is the live path; the legacy 0.4 emit path is not in scope." tentative))

  ;; ----------------------------------------------------------
  ;; OUTCOME
  ;; ----------------------------------------------------------
  (outcome
    (success
      "cargo test --release: all 7 previously-failing cli_build_05_test cases pass; no
       'duplicate symbol: drop_in_place<Value>'; the multi-bundle --exe builds AND runs."
      "The pass set is a strict superset of before — zero regressions; foreign_fn_fnref_test still 19/19."
      "The fix is single-definition by construction, with no --allow-multiple-definition / -z muldefs.")
    (failure
      "Green achieved via a duplicate-tolerating linker flag."
      "Any prior-green test now failing."
      "A produced exe that links but miscompiles because the wrong symbol copy was kept."
      "Any write into axis-lang-lab-working."))

  ;; ----------------------------------------------------------
  ;; MODE
  ;; ----------------------------------------------------------
  (mode
    (phase execution-enabled)
    (design allowed)
    (execution allowed))

  ;; ----------------------------------------------------------
  ;; STATUS
  ;; ----------------------------------------------------------
  (status
    (state approved)
    (authority human)
    (execution-allowed true))
)

;; ============================================================
;; INVARIANT COMPRESSION LAYER (LONG-CONTEXT SURVIVAL)
;; ============================================================
(intent-invariants
  (hard-limit STRUCTURAL_FIX_NOT_A_LINKER_FLAG)
  (hard-limit SINGLE_DEFINITION_OF_DROP_GLUE)
  (hard-limit VALUE_ENUM_UNTOUCHED)
  (hard-limit LANG_REPO_READ_ONLY)
  (boundary   NO_TEST_REGRESSIONS)
  (authority  CLAUDECODE_BOUNDED_CHRIS_FINAL))

;; Semantic gravity anchors:
;; XBUNDLE_TO_EXE_IS_THE_NORM
;; VALUE_GLUE_DEFINED_ONCE_IN_SHARED_LIB
;; STRUCTURAL_FIX_NOT_A_LINKER_FLAG
;; SINGLE_DEFINITION_OF_DROP_GLUE
;; LINK_RLIBS_AS_CRATE_DEPS_NOT_RAW_ARCHIVES
;; VALUE_ENUM_UNTOUCHED
;; NO_TEST_REGRESSIONS

;; ============================================================
;; REINTEGRATION CHECK — run before concluding
;; ============================================================
;; Evaluate against:
;; - identity BRIDGE_XBUNDLE_LINK_DEDUP
;; - constraints STRUCTURAL_FIX_NOT_A_LINKER_FLAG, SINGLE_DEFINITION_OF_DROP_GLUE, VALUE_ENUM_UNTOUCHED,
;;   NO_TEST_REGRESSIONS, LANG_REPO_READ_ONLY
;; - priority correctness > single-definition > no-regressions > blast-radius
;; - risk duplicate-tolerating flag = unacceptable; wrong-copy-kept = critical
;; - boundary default forbidden; Value untouched; --allow-multiple-definition forbidden

;; ============================================================
;; REQUEST
;; ============================================================
;; 1. Read src/main.rs (the --exe / xbundle link path) and tests/cli_build_05_test.rs; confirm the
;;    root-cause mechanism in the first UNKNOWN (raw `-C link-arg=` archives defeating rlib dedup).
;; 2. Apply the structural fix — first validate linking bundle + provider rlibs as real crate deps
;;    (--extern + -L dependency); fall back to a single shared-runtime crate / dylib only if that does
;;    not provably yield one definition.
;; 3. Add a multi-bundle --exe regression test (no duplicate symbol + the exe runs).
;; 4. cargo test --release; confirm all 7 cli_build_05 failures now pass and the pass set is a strict
;;    superset of before (foreign_fn_fnref_test still 19/19).
;; Reject any path that uses --allow-multiple-definition / -z muldefs, touches Value, or writes the lang repo.

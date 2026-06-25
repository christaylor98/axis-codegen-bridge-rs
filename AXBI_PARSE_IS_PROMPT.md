You are operating under INTENT_SPEC.v0.2 (LLM-HARDENED).

(intent-id AXBI_PARSE_V1)

;; ------------------------------------------------------------
;; LONG-CONTEXT REHYDRATION ANCHOR
;; ------------------------------------------------------------
;; All constraints remain binding. Absence implies forbidden. Constraint > Priority > Goal.
;; ONE_BRIDGE_FN_ONLY. NO_NEW_FILES. NO_LANG_REPO_WRITES. AXREG_IMMUTABLE_IDENTITIES.
;; NO_OPAQUE_HANDLES. VALUE_TREE_IS_SELF_DESCRIBING.
;; AXBI_PARSE_V1 governs interpretation.

(intent

  ;; ----------------------------------------------------------
  ;; IDENTITY
  ;; ----------------------------------------------------------
  (identity
    (name "AXBI_PARSE_V1")
    (owner "Chris")
    (scope "In axis-codegen-bridge-rs: wire `axbi_parse` into the emitter symbol map,
            build, and make the three unit tests in src/runtime/axbi.rs pass.
            INCLUDES: one symbol_map line in src/emit/rust_05.rs; cargo build; cargo test.
            EXCLUDES: any additional bridge functions; any write to axis-lang-lab-working;
            any new source files; any opaque handle design."))

  ;; ----------------------------------------------------------
  ;; WHAT IS ALREADY DONE
  ;; ----------------------------------------------------------
  ;; - src/runtime/axbi.rs — ONE bridge function: `pub fn axbi_parse(v: Value) -> Value`
  ;;   Parses a full .axbi byte stream (ValueList of Int 0..=255).
  ;;   Returns Value::Tuple([pool_list, nodes_list]) — self-describing, no opaque handles.
  ;;   Contains three #[cfg(test)] tests: parse_structure, bad_magic_panics, too_short_panics.
  ;;
  ;; - src/runtime/mod.rs — `pub mod axbi;` added.
  ;;
  ;; - registry/axis-codegen-bridge.axreg — entry added (AXBI_PARSE_V1):
  ;;     identity 0x5493ec17ab42802b6ce813f9c9f295580ca60677a555aecbc74ebf55e5d80de7
  ;;     kind leaf | in (ValueList) | out Value | effect reads
  ;;     deterministic true | idempotent true
  ;;
  ;; WHAT REMAINS:
  ;; - src/emit/rust_05.rs symbol_map() → add one line:
  ;;     m.insert("axbi_parse", "axis_codegen_bridge::runtime::axbi::axbi_parse");
  ;; - cargo build (confirm zero errors)
  ;; - cargo test axbi (confirm parse_structure, bad_magic_panics, too_short_panics all PASS)
  ;; - Fix any unused-import warning: `use crate::runtime::value::get_str;` in axbi.rs test
  ;;   module is unused — remove if compiler flags it.

  ;; ----------------------------------------------------------
  ;; GOAL
  ;; ----------------------------------------------------------
  (goal
    (primary "axbi_parse is fully wired: symbol map, build green, three unit tests pass.")
    (secondary
      "No new bridge functions added."
      "No opaque handle types introduced."
      "Returned Value tree shape exactly matches the doc comment in axbi.rs.")
    (type outcome-oriented))

  ;; ----------------------------------------------------------
  ;; ACTOR
  ;; ----------------------------------------------------------
  (actor
    (Chris
      (type human)
      (role "Bridge owner, final authority")
      (authority full)
      (may-decide true))
    (ClaudeCode
      (type ai)
      (role "Wires and tests the bridge function")
      (authority bounded)
      (may-decide false)))

  ;; ----------------------------------------------------------
  ;; PRIORITY
  ;; ----------------------------------------------------------
  (priority
    (build-green high)
    (tests-pass high)
    (one-function-surface high)
    (opaque-handle-avoidance high))

  ;; ----------------------------------------------------------
  ;; CONSTRAINT — HARD LIMITS
  ;; ----------------------------------------------------------
  (constraint
    (hard-limit "ONE BRIDGE FUNCTION ONLY. axbi_parse is the complete bridge surface for .axbi
                 binary format. Do NOT add axbi_write, axbi_identity_hex, bundle_node_count,
                 pool_hash_hex, or any accessor function. The returned Value tree is self-describing;
                 M1 navigates it with standard list/tuple functions already registered."))

  (constraint
    (hard-limit "NO NEW SOURCE FILES. All edits land in existing files only:
                 src/emit/rust_05.rs (symbol_map line) and, if needed, src/runtime/axbi.rs
                 (fix unused-import warning). No new .rs, no new .md, no new test files."))

  (constraint
    (hard-limit "DO NOT WRITE TO axis-lang-lab-working. That repo is read-only from this context.
                 The bridge spec (core_ir_spec/axbi-m1-bridge-spec.md) lives in axis-lang-lab-working
                 and may be read for reference — emit no edits to it."))

  (constraint
    (hard-limit "AXREG IDENTITY IS FROZEN. The axreg entry for axbi_parse has identity
                 0x5493ec17ab42802b6ce813f9c9f295580ca60677a555aecbc74ebf55e5d80de7.
                 Do not recompute, replace, or remove it."))

  (constraint
    (hard-limit "NO OPAQUE HANDLES. The returned Value from axbi_parse is a plain, nested Value tree
                 (Tuple, List, Ctor, Str, Int). Do not introduce opaque handle patterns,
                 token types, or secondary parsing bridge functions."))

  ;; ----------------------------------------------------------
  ;; RISK
  ;; ----------------------------------------------------------
  (risk ("Additional bridge functions added beyond axbi_parse" high))
  (risk ("symbol_map line missing — axbi_parse unreachable via codegen" high))
  (risk ("Opaque handle design introduced" unacceptable))
  (risk ("Any write to axis-lang-lab-working" unacceptable))
  (risk ("axreg identity modified" unacceptable))
  (risk ("Build fails due to unexpected compile error in axbi.rs" low))

  ;; ----------------------------------------------------------
  ;; BOUNDARY
  ;; ----------------------------------------------------------
  (boundary
    ("Add one symbol_map line in src/emit/rust_05.rs" allowed)
    ("Remove unused `get_str` import from axbi.rs test module if compiler warns" allowed)
    ("Run cargo build and cargo test axbi" allowed)
    ("Read axis-lang-lab-working files for spec reference" allowed)
    ("Add any further bridge functions" forbidden)
    ("Add new .rs source files" forbidden)
    ("Write to axis-lang-lab-working" forbidden)
    ("Modify the axreg identity field" forbidden)
    ("Introduce opaque handles or secondary accessor functions" forbidden)
    (default forbidden))

  ;; ----------------------------------------------------------
  ;; ASSUMPTION
  ;; ----------------------------------------------------------
  (assumption
    ("symbol_map() in src/emit/rust_05.rs is the only dispatch table that needs updating;
      no separate registry/routing table exists for runtime calls." tentative))
  (assumption
    ("Value::Ctor, Value::List, Value::Tuple, intern_str, intern_tag are all pub in
      src/runtime/value.rs and accessible from src/runtime/axbi.rs." confirmed))
  (assumption
    ("The three tests in axbi.rs compile and pass once the unused import is resolved." tentative))

  ;; ----------------------------------------------------------
  ;; OUTCOME
  ;; ----------------------------------------------------------
  (outcome
    (success
      (fact "axbi_parse appears in symbol_map in src/emit/rust_05.rs")
      (untested-prediction "cargo build succeeds with zero errors"
        (test "run: cargo build -p axis-codegen-bridge-rs 2>&1 | tail -5"))
      (untested-prediction "cargo test axbi shows parse_structure, bad_magic_panics, too_short_panics: ok"
        (test "run: cargo test -p axis-codegen-bridge-rs axbi 2>&1"))
      (untested-prediction "no unused-import warnings in axbi.rs"
        (test "confirm no `warning: unused import` lines in build output")))
    (failure
      "Build fails for any reason."
      "Any test in the axbi module fails."
      "Any new bridge function added."
      "Any write to axis-lang-lab-working."))

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
;; WORK MANIFEST — two steps, in order
;; ============================================================
;;
;; STEP 1 — src/emit/rust_05.rs
;;   In the symbol_map() function, after the last existing m.insert line
;;   (currently bridge_to_float), add:
;;
;;     // AXBI_PARSE_V1 — Core IR 0.5 binary format parser
;;     m.insert("axbi_parse", "axis_codegen_bridge::runtime::axbi::axbi_parse");
;;
;;   That is the ONLY edit to rust_05.rs.
;;
;; STEP 2 — verify
;;   cargo build -p axis-codegen-bridge-rs
;;   cargo test  -p axis-codegen-bridge-rs axbi -- --nocapture
;;   Expected: 3 tests pass, 0 failures, 0 warnings.
;;
;; ============================================================
;; VALUE TREE CONTRACT (reference — do not change)
;; ============================================================
;;
;;   axbi_parse(bytes: ValueList) -> Value
;;
;;   Returns Value::Tuple([pool, nodes]) where:
;;
;;   pool  = Value::List([
;;             Value::Tuple([Str(def_hash_64hex), List([Int(byte)...])]),
;;             ...
;;           ])
;;
;;   nodes = Value::List([
;;             Ctor "CCall"        fields: [Str(name), Str(id_64hex), List([NodeRef...])]
;;             Ctor "CIf"          fields: [NodeRef(cond), NodeRef(then), NodeRef(else)]
;;             Ctor "CDeterminate" fields: []
;;             ...
;;           ])
;;
;;   NodeRef = Value::Tuple([Str("node"|"pool"), Int(index)])
;;
;;   Hard-fails (UNKNOWN gate) on: bad magic, wrong version, truncated data,
;;   non-minimal varint, forward edge, out-of-range pool ref.
;;
;; ============================================================
;; INVARIANT COMPRESSION LAYER
;; ============================================================
(intent-invariants
  (hard-limit ONE_BRIDGE_FN_ONLY)
  (hard-limit NO_NEW_FILES)
  (hard-limit NO_LANG_REPO_WRITES)
  (hard-limit AXREG_IMMUTABLE_IDENTITIES)
  (hard-limit NO_OPAQUE_HANDLES)
  (boundary   SYMBOL_MAP_ONLY_EDIT_IN_RUST_05)
  (authority  CLAUDECODE_BOUNDED_CHRIS_FINAL))

;; Semantic gravity anchors:
;; ONE_BRIDGE_FN_ONLY
;; NO_OPAQUE_HANDLES
;; VALUE_TREE_IS_SELF_DESCRIBING
;; NO_LANG_REPO_WRITES
;; AXREG_IMMUTABLE_IDENTITIES

;; ============================================================
;; REINTEGRATION CHECK — run before concluding
;; ============================================================
;; Evaluate against:
;; - identity AXBI_PARSE_V1
;; - constraints ONE_BRIDGE_FN_ONLY, NO_NEW_FILES, NO_LANG_REPO_WRITES,
;;   AXREG_IMMUTABLE_IDENTITIES, NO_OPAQUE_HANDLES
;; - outcome: build green + 3 tests pass
;; - risk: no additional bridge fns, no opaque handles, no lang-repo writes
;; - epistemics: outcomes fact-typed where confirmed; build/test outcomes typed
;;   untested-prediction with concrete test commands

;; ============================================================
;; REQUEST
;; ============================================================
;; Execute the three steps in the WORK MANIFEST above — in order.
;; Stop if any step surfaces a compiler error that is not the unused-import.
;; Report the cargo test output verbatim.
;; Do not add any bridge functions beyond axbi_parse.

(model-recommendation
  (recommended "Sonnet 4.6")
  (tier T2)
  (floor "Sonnet 4.6")
  (rationale "structured-design mode + low constraint density (5 hard-limits, bounded scope) → T2 sufficient; no governance or unacceptable-risk escalation triggers")
  (authority human)
  (may-decide false))

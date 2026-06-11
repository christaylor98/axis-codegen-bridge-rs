You are operating under INTENT_SPEC.v0.2 (LLM-HARDENED).

(intent-id BRIDGE_FOREIGN_FN_FNREF_M1)

;; ------------------------------------------------------------
;; LONG-CONTEXT REHYDRATION ANCHOR
;; ------------------------------------------------------------
;; All constraints remain binding. Absence implies forbidden. Constraint > Priority > Goal.
;; NO_LAMBDA_EVER. FN_REF_IS_NOT_A_VALUE. FN_REF_IS_CALLEE_ONLY. VALUELIST_IS_DATA_ONLY.
;; IT_IS_ALL_RUST_DOWN_HERE. LANG_REPO_READ_ONLY. AXREG_IMMUTABLE_IDENTITIES.
;; BRIDGE_FOREIGN_FN_FNREF_M1 governs interpretation.

(intent

  ;; ----------------------------------------------------------
  ;; IDENTITY
  ;; ----------------------------------------------------------
  (identity
    (name "BRIDGE_FOREIGN_FN_FNREF_M1")
    (owner "Chris")
    (scope "In axis-codegen-bridge-rs: (Phase 0) teach the bridge to accept a Fn reference as an
            argument under Core IR 0.5, resolved at emit time to a bare Rust fn path — NO Value::Fn;
            then (Phases 1-3) add the M1 iteration + emit stdlib foreign fns.
            INCLUDES: Rust impls, src/emit/rust_05.rs symbol-map + Fn-typed-arg lowering,
            registry/axis-codegen-bridge.axreg entries.
            EXCLUDES: any write to axis-lang-lab-working (read-only, for spec text only);
            re-adding map/filter/sort/reduce (M1 already ships these); the legacy bridge;
            the ir_* removal / spec-driven-generator work; the pure-vs-fullIo effect-cleanup sweep
            of pre-existing primitives; migrating existing HOF callees."))

  ;; ----------------------------------------------------------
  ;; GOAL
  ;; ----------------------------------------------------------
  (goal
    (primary "The bridge emits and runs Core IR 0.5 bundles that call the new foreign fns, including
              higher-order fns whose callee is a Fn reference — with NO lambda and NO fn-as-data anywhere.")
    (secondary
      "Lowest friction for AI gen + mech code gen — the iteration/emit vocabulary is complete."
      "Every new fn resolves through the UNKNOWN gate (has an axreg entry + a symbol-map line)."
      "Illegal states are unrepresentable: a fn ref can only sit in an HOF callee slot, never in data.")
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
      (role "Implements the bridge changes")
      (authority bounded)
      (may-decide false)))

  ;; ----------------------------------------------------------
  ;; PRIORITY
  ;; ----------------------------------------------------------
  (priority
    (correctness high)
    (no-fn-as-data high)
    (codegen-friction-reduction high)
    (purity low))

  ;; ----------------------------------------------------------
  ;; CONSTRAINT — HARD LIMITS
  ;; ----------------------------------------------------------
  (constraint
    (hard-limit "NO LAMBDA EVER. No closures, no Lam terms, no inline abstraction at the bridge.
                 A Fn reference is ONLY a 256-bit minted identity token (sha256 of the fn name). Per
                 Core IR 0.5: a Fn ref is a constant-pool entry whose defHash is the Fn type and whose
                 payload is the 32-byte identity hash — no special node. Core IR 0.5 lower.rs MUST keep
                 rejecting CoreTerm::Lam — do not relax that."))

  (constraint
    (hard-limit "A FN REFERENCE IS NOT A VALUE. Do NOT add a Value::Fn variant. `Value` models DATA
                 only (Int, Bool, Str, Unit, Tuple, List, Ctor). A fn ref is callee-only and resolved at
                 EMIT TIME to a bare Rust fn path; it may appear ONLY in the callee/predicate slot of a
                 higher-order primitive. A Fn-typed pool entry in any data position (ValueList element,
                 Value(...) compound, Ctor field, eq/sort/serialize operand) is a HARD ERROR. Make the
                 illegal state unrepresentable, not merely unverified."))

  (constraint
    (hard-limit "IT IS ALL RUST DOWN HERE. The Value::Tuple arg-packing convention is NOT load-bearing
                 — it is one way the emitter renders a call. A higher-order call is an ordinary Rust call;
                 passing a fn by name in argument position is native and free. Do NOT contort the design to
                 preserve Tuple-packing for HOFs, and do NOT treat the native multi-arg shape as a regression."))

  (constraint
    (hard-limit "DO NOT WRITE TO axis-lang-lab-working. Read core_ir_spec/axis-core-ir-0.5.md and the
                 M1 surface spec for grounding only — emit no edits, no files, no commits into the lang repo."))

  (constraint
    (hard-limit "axreg entries use the strict format in axis-codegen-bridge-rs/CLAUDE.md.
                 Identity = SHA-256 of the UTF-8 fn-name bytes; use the manifest's precomputed identities
                 verbatim. NEVER modify or remove an existing identity field. Fields exactly: identity,
                 kind, in, out, effect, deterministic, idempotent. No arity, no profile."))

  (constraint
    (hard-limit "Effects: every new fn is `pure` EXCEPT `foreach`, which is `fullIo`."))

  (constraint
    (hard-limit "Do NOT re-add map / filter / sort / reduce — M1 already ships them. Add ONLY the fns
                 enumerated in the work manifest below. Every new fn needs ALL THREE: a Rust impl, a
                 src/emit/rust_05.rs symbol-map line, and an axreg entry. A missing piece is a hard failure."))

  ;; ----------------------------------------------------------
  ;; RISK
  ;; ----------------------------------------------------------
  (risk ("A Value::Fn variant added — fn-as-data leak into ValueList / compound / eq" unacceptable))
  (risk ("A Fn ref lowered as a lambda/closure instead of an emit-time Rust path" unacceptable))
  (risk ("Any edit landing in axis-lang-lab-working" unacceptable))
  (risk ("A new fn present in Rust but absent from the axreg — UNKNOWN gate / silent miss" high))
  (risk ("Identity hash recomputed wrong and diverging from the manifest" high))
  (risk ("foreach marked pure instead of fullIo — effect lie" high))

  ;; ----------------------------------------------------------
  ;; BOUNDARY
  ;; ----------------------------------------------------------
  (boundary
    ("Add a Fn-type branch to the src/emit/rust_05.rs pool decoder (resolve identity -> Rust path)" allowed)
    ("Lower Fn-typed CCall args as a native multi-arg Rust call with the fn as a bare path" allowed)
    ("Add a new runtime module (e.g. src/runtime/iter.rs) and/or extend list.rs + str_ops.rs" allowed)
    ("Add symbol_map lines in src/emit/rust_05.rs" allowed)
    ("Append the paste-ready entries to registry/axis-codegen-bridge.axreg" allowed)
    ("Read core_ir_spec/axis-core-ir-0.5.md and the M1 surface spec for grounding" allowed)
    ("Add a Value::Fn (or any fn-as-data) variant" forbidden)
    ("Relax lower.rs Lam rejection" forbidden)
    ("Write anything into axis-lang-lab-working" forbidden)
    ("Touch the legacy bridge" forbidden)
    ("Re-add map/filter/sort/reduce or migrate their callee type" forbidden)
    (default forbidden))

  ;; ----------------------------------------------------------
  ;; UNKNOWN
  ;; ----------------------------------------------------------
  (unknown
    ("TYPE-VOCABULARY: the manifest signatures use `ValueList` and `Fn`, but
      axis-codegen-bridge-rs/CLAUDE.md lists valid axreg types as
      Int Text Bool Unit TextList ResultText ResultUnit Value — `ValueList` and `Fn` are not in it.
      Resolve before writing axreg: either the valid-type set is extended (ValueList = data-only list,
      Fn = callee-position only) or these map onto existing names. DO NOT guess — confirm with Chris.
      Under this design the separation is clean: Fn is strictly callee-position, ValueList strictly data."))

  ;; ----------------------------------------------------------
  ;; ASSUMPTION
  ;; ----------------------------------------------------------
  (assumption
    ("Core IR 0.5 is the current/authoritative schema; rust_05.rs is the live emitter." tentative))
  (assumption
    ("Higher-order callees (foreach, flat_map, any, all, find_index, count, loop_count, loop_while)
      receive their callee as a NATIVE Rust fn pointer (fn(Value)->Value), resolved at emit time from
      the Fn-typed pool entry's identity — NOT as a runtime Value. They apply it per element/step." tentative))
  (assumption
    ("'No Tuple' — paired outputs (enumerate, zip) are Value(...) compounds in a ValueList, built via
      the existing value_make / value_0/1/2 product, not a Tuple type." tentative))

  ;; ----------------------------------------------------------
  ;; OUTCOME
  ;; ----------------------------------------------------------
  (outcome
    (success
      "A Core IR 0.5 bundle calling foreach(xs, print_it) emits print_it as a bare Rust fn path in
       foreach's callee slot and runs the effect per element — zero lambda, zero Value::Fn."
      "A fn ref placed in a ValueList (or any data position) is rejected at emit time."
      "range(0,3) -> [0,1,2]; str_join(ValueList(Text)(\"a\",\"b\"), \",\") -> \"a,b\"."
      "Every new fn appears in BOTH src/emit/rust_05.rs symbol_map AND registry/axis-codegen-bridge.axreg
       with the manifest's exact identity, and the bridge builds (cargo build --release).")
    (failure
      "Any Value::Fn / fn-as-data path, any lambda/closure path."
      "Any fn missing its axreg entry or symbol-map line."
      "Any write into axis-lang-lab-working."
      "An identity hash that disagrees with the manifest."))

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
;; WORK MANIFEST  (M1 GAPS only — map/filter/sort/reduce already ship)
;; ============================================================

;; ------------------------------------------------------------
;; PHASE 0 — FN-REF SUPPORT  (FOUNDATION — core for everything else; ship before any HOF)
;; Design: callee-only, emit-time resolution. Fn is NOT a Value.
;; ------------------------------------------------------------
;;   1. src/runtime/value.rs → do NOT add Value::Fn. `Value` stays pure data. A fn ref never becomes
;;      a runtime value; it is resolved at emit time to a Rust fn path.
;;   2. src/emit/rust_05.rs pool decoder → add a Fn-type branch. When a pool entry's defHash is the Fn
;;      type hash, resolve its 32-byte payload identity -> bridge symbol path and emit the BARE Rust fn
;;      path (e.g. axis_codegen_bridge::runtime::io::io_println). This is the branch that today falls
;;      through to "unknown pool entry type hash … only Unit/Bool/Int/Text supported" — whose message
;;      already names "a fn-reference that was not assigned a Fn type hash". Per axis-core-ir-0.5.md
;;      §"Fn Reference as Argument": NO new node type — a fn ref is a typed constant.
;;   3. emit_node CCall arg lowering → when ANY arg references a Fn-typed pool entry, emit a NATIVE
;;      multi-arg Rust call (e.g. `foreach(pool_0.clone(), io_println)`). The Fn-typed arg emits as the
;;      bare fn path; data args emit as `pool_N.clone()`. Driven purely by the arg's pool-entry type
;;      (Fn-typed ⇒ path + native call), NOT by a hardcoded list of HOF names. HOF Rust signatures are
;;      native: `fn(Value, fn(Value)->Value) -> Value`.
;;   4. TYPE GATE → a Fn-typed pool entry referenced anywhere other than an HOF callee/predicate slot
;;      is a HARD ERROR. A Fn is never an element of a ValueList / Value(...) compound / Ctor, never
;;      compared, never returned as data.
;;   5. KEEP src/core_ir_05/lower.rs CoreTerm::Lam rejection EXACTLY as-is.
;;   Acceptance: foreach(xs, print_it) emits print_it as a bare Rust fn path and runs per element —
;;   zero lambda, zero Value::Fn; a fn ref in a ValueList is rejected at emit time.

;; ------------------------------------------------------------
;; PHASE 1 — P0 fns (unblock real codegen)
;; ------------------------------------------------------------
;;   foreach     (ValueList, Fn) -> Unit       fullIo   side-effect iterator (effectful peer of map)
;;   range       (Int, Int)      -> ValueList  pure     ValueList(Int), half-open [s,e)
;;   loop_count  (Int, Value, Fn)-> Value      pure     apply step(acc) n times
;;   str_join    (ValueList, Text)-> Text      pure     ValueList of Text -> joined Text

;; ------------------------------------------------------------
;; PHASE 2 — P1 fns (complete the vocabulary)
;; ------------------------------------------------------------
;;   flat_map (ValueList, Fn)->ValueList | any (ValueList, Fn)->Bool | all (ValueList, Fn)->Bool
;;   find_index (ValueList, Fn)->Int (-1 if none) | count (ValueList, Fn)->Int
;;   range_step (Int,Int,Int)->ValueList | repeat (Value,Int)->ValueList
;;   enumerate (ValueList)->ValueList(Value(Int,T)) | zip (ValueList,ValueList)->ValueList(Value(A,B))
;;   take (ValueList,Int)->ValueList | drop (ValueList,Int)->ValueList | slice (ValueList,Int,Int)->ValueList
;;   flatten (ValueList)->ValueList | loop_while (Value, Fn, Fn, Int max)->Value

;; ------------------------------------------------------------
;; PHASE 3 — P1 text emit helpers
;; ------------------------------------------------------------
;;   str_replace (Text,Text,Text)->Text | str_repeat (Text,Int)->Text
;;   str_to_upper (Text)->Text [idempotent] | str_to_lower (Text)->Text [idempotent]
;;   str_pad_left (Text,Int,Text)->Text | str_pad_right (Text,Int,Text)->Text

;; PASTE-READY AXREG ENTRIES: use the exact block in the foreign-fn manifest
;; (FOREIGN_FNS_iteration_stdlib.md) — identities are precomputed §5b; copy them verbatim.
;; Resolve the (ValueList / Fn) type-vocabulary UNKNOWN with Chris before appending.

;; ============================================================
;; INVARIANT COMPRESSION LAYER (LONG-CONTEXT SURVIVAL)
;; ============================================================
(intent-invariants
  (hard-limit NO_LAMBDA_EVER)
  (hard-limit FN_REF_IS_NOT_A_VALUE)
  (hard-limit FN_REF_IS_CALLEE_ONLY)
  (hard-limit LANG_REPO_READ_ONLY)
  (hard-limit AXREG_IMMUTABLE_IDENTITIES)
  (boundary   SYMBOL_MAP_AND_AXREG_BOTH_MANDATORY)
  (authority  CLAUDECODE_BOUNDED_CHRIS_FINAL))

;; Semantic gravity anchors:
;; NO_LAMBDA_EVER
;; FN_REF_IS_NOT_A_VALUE
;; FN_REF_IS_CALLEE_ONLY
;; VALUELIST_IS_DATA_ONLY
;; IT_IS_ALL_RUST_DOWN_HERE
;; FOREACH_IS_FULLIO

;; ============================================================
;; REINTEGRATION CHECK — run before concluding
;; ============================================================
;; Evaluate against:
;; - identity BRIDGE_FOREIGN_FN_FNREF_M1
;; - constraints NO_LAMBDA_EVER, FN_REF_IS_NOT_A_VALUE, FN_REF_IS_CALLEE_ONLY, LANG_REPO_READ_ONLY,
;;   AXREG_IMMUTABLE_IDENTITIES
;; - priority correctness > no-fn-as-data > codegen-friction > purity
;; - risk Value::Fn / fn-as-data = unacceptable; lang-repo-write = unacceptable
;; - boundary default forbidden; lower.rs Lam rejection stays
;; - unknown TYPE-VOCABULARY (ValueList/Fn) resolved with Chris before axreg is written

;; ============================================================
;; REQUEST
;; ============================================================
;; Implement Phase 0 FIRST — it is the foundation; nothing higher-order works without it.
;; Then Phase 1 (P0) -> Phase 2 (P1) -> Phase 3 (text helpers).
;; Surface the TYPE-VOCABULARY unknown and confirm with Chris BEFORE writing axreg entries.
;; For every new fn: Rust impl + src/emit/rust_05.rs symbol-map line + axreg entry (manifest identity).
;; Reject any step that adds Value::Fn / fn-as-data, introduces a lambda, or writes to the lang repo.
;; Build with cargo build --release and include the one-line tests:
;;   range(0,3) -> [0,1,2]; str_join(ValueList(Text)("a","b"), ",") -> "a,b"; foreach(xs, print_it) runs per element.

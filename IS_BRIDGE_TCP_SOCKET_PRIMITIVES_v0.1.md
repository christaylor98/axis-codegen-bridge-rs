```lisp
;; ============================================================
;; INTENT DECLARATION — AUTHORITATIVE BLOCK
;; ============================================================

You are operating under INTENT_SYSTEM_SPEC.v1.0.

(intent-id BRIDGE_TCP_SOCKET_V1)

;; ------------------------------------------------------------
;; LONG-CONTEXT REHYDRATION ANCHOR
;; ------------------------------------------------------------
;; All constraints remain binding.
;; Absence implies forbidden.
;; Constraint > Priority > Goal.
;; Authority separation must be preserved.
;; BRIDGE_TCP_SOCKET_V1 governs interpretation.


(intent-mode
  (state structured-design)
  (authority human)
  (downgrade-allowed false))


(intent

  ;; ------------------------------------------------------------
  ;; IDENTITY
  ;; ------------------------------------------------------------
  (identity
    (name "BRIDGE_TCP_SOCKET_V1")
    (owner "Chris")
    (scope "TCP listen/accept/connect/read/write/close bridge primitives for axis-codegen-bridge-rs, unblocking the Postgres wire protocol milestone. Excludes TLS, UDP, non-blocking/select, and any axlang M1-surface parser change. (client-side connect added by AMENDMENT — Chris approved.)"))


  ;; ------------------------------------------------------------
  ;; GOAL
  ;; ------------------------------------------------------------
  (goal
    (primary "Land tcp_listen/tcp_accept/tcp_connect/tcp_read/tcp_write/tcp_close as bridge leaf fns, closing gap:axverity-postgres-wire-needs-socket-primitive.")
    (secondary
      "Support explicit port parameter including 0 (ephemeral) without adding a fixed-port special case."
      "Return the bound port from tcp_listen without inventing new registry types."
      "[AMENDED] Prove both server and client halves entirely through bridge fns — no std::net in test scaffolding.")
    (type outcome-oriented))


  ;; ------------------------------------------------------------
  ;; ACTOR
  ;; ------------------------------------------------------------
  (actor

    (Chris
      (type human)
      (role "Final decision authority")
      (authority full)
      (may-decide true))

    (Claude
      (type ai)
      (role "Diagnosis, design proposal, IS authoring")
      (authority none)
      (may-decide false))

    (ClaudeCode
      (type system)
      (role "Executes approved IS: writes net.rs, registry entries, dispatch wiring, runs tests")
      (authority bounded)
      (may-decide false)))


  ;; ------------------------------------------------------------
  ;; PRIORITY
  ;; ------------------------------------------------------------
  (priority
    (precedent-consistency high)
    (correctness high)
    (demo-critical-path high)
    (performance low))


  ;; ------------------------------------------------------------
  ;; CONSTRAINT — HARD LIMITS
  ;; ------------------------------------------------------------
  (constraint
    (hard-limit "No Result/Ok-Err wrapper types — panic-only convention (IS_REMOVE_RESULT_TYPES_FINAL_v0.1.md)."))

  (constraint
    (hard-limit "No new registry `type` declaration for this feature — reuse existing Value sum type + tuple_field/ctor_field precedent (tuple.rs), matching every other compound-returning fn in axis-codegen-bridge.axreg."))

  (constraint
    (hard-limit "tcp_listen port is caller-supplied; 0 must be a valid input and must bind an OS-assigned ephemeral port."))

  (constraint
    (hard-limit "tcp_listen must return the bound port alongside the handle, in the same call."))

  (constraint
    (hard-limit "Socket primitives must not touch the channels.rs async/event layer — synchronous fullIo leaf fns only."))

  (constraint
    (hard-limit "THREE_PIECE_RULE: net.rs impl + dispatch entry (rust_05.rs) + registry entry land in the same commit."))

  (constraint
    (hard-limit "[AMENDED] Any scope change (boundary forbidden → allowed) must be annotated in-artifact with actor + may-decide, not silently applied."))


  ;; ------------------------------------------------------------
  ;; RISK
  ;; ------------------------------------------------------------
  (risk
    ("tuple_field/ctor_field return Value::Unit on shape mismatch instead of panicking — a caller destructuring tcp_listen's Tuple incorrectly gets silent Unit, not a panic, masking a bug as a value" medium))

  (risk
    ("Blocking tcp_accept/tcp_read/tcp_connect stalls the calling M1 process loop indefinitely if the peer never connects/sends — no timeout primitive exists" medium))

  (risk
    ("Ephemeral port exhaustion or bind collision under heavy concurrent-listener test load" low))


  ;; ------------------------------------------------------------
  ;; BOUNDARY
  ;; ------------------------------------------------------------
  (boundary
    ("tcp_listen / tcp_accept / tcp_connect / tcp_read / tcp_write / tcp_close" allowed)
    ("TLS" forbidden)
    ("UDP" forbidden)
    ("non-blocking / select" forbidden)
    ("client-side tcp_connect" allowed)  ;; AMENDED — Chris approved scope change (may-decide true)
    (default forbidden))


  ;; ------------------------------------------------------------
  ;; UNKNOWN
  ;; ------------------------------------------------------------
  (unknown
    ("Whether tuple_field's silent-Unit-on-mismatch behavior needs correcting to panic-on-mismatch for convention consistency — out of scope for this IS, flagged for separate decision."))


  ;; ------------------------------------------------------------
  ;; ASSUMPTION
  ;; ------------------------------------------------------------
  (assumption
    ("Bridge dispatch wraps multi-arg leaf fn calls as Value::Tuple before invocation, consistent for tcp_write(Int, Bytes) as for existing 2-arg fns." confirmed))

  (assumption
    ("Binding 0.0.0.0 (not loopback-only) is acceptable for the Postgres-wire demo." tentative))


  ;; ------------------------------------------------------------
  ;; OUTCOME — re-typed against actual test results (2026-07-06)
  ;; ------------------------------------------------------------
  (outcome

    (fact "tuple_field(Value, Int) -> Value and ctor_field(Value, Int) -> Value already exist in tuple.rs — the established pattern for multi-value bridge returns, used in place of any static Product/Tuple type across every compound-returning fn in this codebase.")

    (fact "`product` is used zero times in registry/axis-codegen-bridge.axreg — a first-of-its-kind Product-typed `out` would break precedent, not extend it.")

    (fact "Result/Ok-Err wrapper types were deliberately removed from this bridge (IS_REMOVE_RESULT_TYPES_FINAL_v0.1.md); panic-only is the live, confirmed convention.")

    (fact "tcp_listen returning Value::Tuple([Int(handle), Int(port)]) round-trips correctly through tuple_field at the bridge call site — confirmed by tests::tcp_listen_accept_read_write_close_roundtrip, passing.")

    (fact "Concurrent tcp_listen(0) across N threads yields N distinct ports, zero bind failures — confirmed by tests::concurrent_ephemeral_listen_distinct_ports, passing.")

    (fact "net.rs + dispatch wiring + registry entries build and pass clean — cargo build 0 errors, cargo test 295 passed / 0 failed, net.rs 3/3 green.")

    (fact "Identity hashes verified against sha256(name) in all three registries (axis-codegen-bridge.axreg, axRegistry-working, axAI-axlang-gen-working) — tcp_connect = f3cb7cba2165737c40329d3d8d2cd9ccae60b6dd2e43a1c918510499806e0850, independently re-verified.")

    (fact "[AMENDED] Client half (tcp_connect+tcp_write+tcp_read) and server half (tcp_listen+tcp_accept+tcp_read+tcp_write) both proven with zero raw std::net calls in test code — tests::tcp_connect_all_bridge_loopback, passing. Both roles now provably go through the bridge, not just the server half.")

    (fact "validate.sh — all registries PASS."))


  ;; ------------------------------------------------------------
  ;; MODE LOCK
  ;; ------------------------------------------------------------
  (mode
    (phase complete)
    (design allowed)
    (execution allowed))


  ;; ------------------------------------------------------------
  ;; STATUS
  ;; ------------------------------------------------------------
  (status
    (state shipped)
    (authority human)
    (execution-allowed true))
)

;; ============================================================
;; INVARIANT COMPRESSION LAYER (LONG CONTEXT SURVIVAL)
;; ============================================================

(intent-invariants
  (hard-limit PANIC_ONLY_NO_RESULT_TYPES)
  (hard-limit NO_NEW_REGISTRY_TYPE_REUSE_VALUE)
  (hard-limit PORT_PARAM_ZERO_VALID)
  (hard-limit TCP_LISTEN_RETURNS_HANDLE_AND_PORT)
  (hard-limit NO_ASYNC_LAYER_COUPLING)
  (hard-limit THREE_PIECE_RULE)
  (hard-limit SCOPE_CHANGE_MUST_BE_ANNOTATED)
  (authority AI_PROPOSE_ONLY))

;; Semantic gravity anchors:
;; PANIC_ONLY_NO_RESULT_TYPES
;; NO_NEW_REGISTRY_TYPE_REUSE_VALUE
;; TUPLE_FIELD_IS_THE_PRECEDENT
;; TCP_LISTEN_RETURNS_HANDLE_AND_PORT
;; BOTH_ROLES_THROUGH_THE_BRIDGE
;; AI_PROPOSE_ONLY


;; ============================================================
;; ARCHITECTURAL SPINE (RESPONSIBILITY ONLY — NOT DESIGN)
;; ============================================================

(spine
  "src/runtime/net.rs – TcpListener/TcpStream registry, six leaf fns, 3 tests. SHIPPED."
  "src/runtime/mod.rs – pub mod net. SHIPPED."
  "src/emit/rust_05.rs – symbol_map v3, six dispatch entries. SHIPPED."
  "registry/axis-codegen-bridge.axreg – six fn entries, no new type. SHIPPED."
  "CLAUDE.md – valid bridge fn list, six entries. SHIPPED."
  "axRegistry-working/axis-bridge.axreg – propagated, verified. SHIPPED."
  "axAI-axlang-gen-working/registries/axis-bridge.axreg – propagated, verified. SHIPPED.")


(spine-rules
  (only net.rs may hold std::net socket state)
  (net.rs registry.rs dispatch.rs land-in-same-commit)
  (no Result/Option wrapper types introduced)
  (no new type declarations)
  (scope changes annotated in-artifact, not silent))


;; ============================================================
;; REINTEGRATION CHECK TEMPLATE
;; ============================================================

;; Before any conclusions:
;; Evaluate against:
;; - identity BRIDGE_TCP_SOCKET_V1
;; - constraints PANIC_ONLY_NO_RESULT_TYPES, NO_NEW_REGISTRY_TYPE_REUSE_VALUE, THREE_PIECE_RULE, SCOPE_CHANGE_MUST_BE_ANNOTATED
;; - priority precedent-consistency > correctness > demo-critical-path > performance
;; - risk tuple_field-silent-Unit medium, blocking-calls-no-timeout medium
;; - boundary default forbidden
;; - authority AI_PROPOSE_ONLY
;; - epistemics: all outcomes fact-typed against actual test results; no predictions left unresolved


;; ============================================================
;; STATUS: CLOSED
;; ============================================================
;; gap:axverity-postgres-wire-needs-socket-primitive → LIVE (satisfied)
;; All 6 next-tests from prior revision executed and passed. No open predictions.
```

(model-recommendation
  (recommended "n/a — shipped")
  (tier n/a)
  (floor n/a)
  (rationale "Execution complete; model selection for this IS is no longer live.")
  (authority human)
  (may-decide false))

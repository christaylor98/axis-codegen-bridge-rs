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
    (scope "TCP listen/accept/read/write/close/connect bridge primitives for axis-codegen-bridge-rs, unblocking the Postgres wire protocol milestone. Excludes TLS, UDP, non-blocking/select, and any axlang M1-surface parser change. (client-side connect added by AMENDMENT — Chris approved.)"))


  ;; ------------------------------------------------------------
  ;; GOAL
  ;; ------------------------------------------------------------
  (goal
    (primary "Land tcp_listen/tcp_accept/tcp_read/tcp_write/tcp_close as bridge leaf fns, closing gap:axverity-postgres-wire-needs-socket-primitive.")
    (secondary
      "Support explicit port parameter including 0 (ephemeral) without adding a fixed-port special case."
      "Return the bound port from tcp_listen without inventing new registry types.")
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


  ;; ------------------------------------------------------------
  ;; RISK
  ;; ------------------------------------------------------------
  (risk
    ("tuple_field/ctor_field return Value::Unit on shape mismatch instead of panicking — a caller destructuring tcp_listen's Tuple incorrectly gets silent Unit, not a panic, masking a bug as a value" medium))

  (risk
    ("Blocking tcp_accept/tcp_read stalls the calling M1 process loop indefinitely if the peer never connects/sends — no timeout primitive exists" medium))

  (risk
    ("Ephemeral port exhaustion or bind collision under heavy concurrent-listener test load" low))


  ;; ------------------------------------------------------------
  ;; BOUNDARY
  ;; ------------------------------------------------------------
  (boundary
    ("tcp_listen / tcp_accept / tcp_read / tcp_write / tcp_close" allowed)
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

  (unknown
    ("Whether ephemeral port range exhaustion is a real concern at this project's actual concurrent-test parallelism — untested at scale."))


  ;; ------------------------------------------------------------
  ;; ASSUMPTION
  ;; ------------------------------------------------------------
  (assumption
    ("Bridge dispatch wraps multi-arg leaf fn calls as Value::Tuple before invocation, consistent for tcp_write(Int, Bytes) as for existing 2-arg fns." tentative))

  (assumption
    ("Binding 0.0.0.0 (not loopback-only) is acceptable for the Postgres-wire demo." tentative))


  ;; ------------------------------------------------------------
  ;; OUTCOME
  ;; ------------------------------------------------------------
  (outcome

    (fact "tuple_field(Value, Int) -> Value and ctor_field(Value, Int) -> Value already exist in tuple.rs — the established pattern for multi-value bridge returns, used in place of any static Product/Tuple type across every compound-returning fn in this codebase.")

    (fact "`product` is used zero times in registry/axis-codegen-bridge.axreg — a first-of-its-kind Product-typed `out` would break precedent, not extend it.")

    (fact "Result/Ok-Err wrapper types were deliberately removed from this bridge (IS_REMOVE_RESULT_TYPES_FINAL_v0.1.md); panic-only is the live, confirmed convention (bytes_io.rs::bytes_hash, ::fs_mkdir_p).")

    (fact "TypeShape::Product is fully parsed and lowered in axis-lang-lab-working/src/registry/core05/text.rs — available if ever needed, not required here.")

    (untested-prediction "tcp_listen returning Value::Tuple([Int(handle), Int(port)]) round-trips correctly through tuple_field at an M1 call site."
      (test "New #[test] in net.rs: tcp_listen(0) -> tuple_field(_, 0)/tuple_field(_, 1) -> spawn thread tcp_accept+tcp_read -> main thread TcpStream::connect+write -> assert bytes match."))

    (untested-prediction "Concurrent tcp_listen(0) calls across N threads yield N distinct ports with zero bind failures."
      (test "N-thread concurrent-listener test, assert distinct ports and all binds succeed — matches project's existing SIGKILL/concurrent-process-race test discipline."))

    (untested-prediction "net.rs + dispatch wiring + registry entries build and pass clean."
      (test "cargo build 2>&1 | grep ^error | wc -l == 0; cargo test tail -5 == 0 failed; validate.sh all PASS."))

    (untested-prediction "Identity hashes as computed match sha256(name) at commit time."
      (test "echo -n '{name}' | sha256sum for each of the 5 fn names, cross-check against axreg identity field.")))


  ;; ------------------------------------------------------------
  ;; MODE LOCK
  ;; ------------------------------------------------------------
  (mode
    (phase structured-design)
    (design allowed)
    (execution allowed))


  ;; ------------------------------------------------------------
  ;; STATUS
  ;; ------------------------------------------------------------
  (status
    (state proposed)
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
  (authority AI_PROPOSE_ONLY))

;; Semantic gravity anchors:
;; PANIC_ONLY_NO_RESULT_TYPES
;; NO_NEW_REGISTRY_TYPE_REUSE_VALUE
;; TUPLE_FIELD_IS_THE_PRECEDENT
;; TCP_LISTEN_RETURNS_HANDLE_AND_PORT
;; AI_PROPOSE_ONLY


;; ============================================================
;; ARCHITECTURAL SPINE (RESPONSIBILITY ONLY — NOT DESIGN)
;; ============================================================

(spine
  "src/runtime/net.rs – new module; TcpListener/TcpStream registry, five leaf fns."
  "src/runtime/mod.rs – pub mod net."
  "src/emit/rust_05.rs – dispatch map entries for the five fn names."
  "registry/axis-codegen-bridge.axreg – five fn entries, no new type."
  "CLAUDE.md – valid bridge fn list addition."
  "axRegistry-working/axis-bridge.axreg – registry propagation."
  "axAI-axlang-gen-working/registries/axis-bridge.axreg – registry propagation.")


(spine-rules
  (only net.rs may hold std::net socket state)
  (net.rs registry.rs dispatch.rs land-in-same-commit)
  (no Result/Option wrapper types introduced)
  (no new type declarations))


;; ============================================================
;; REINTEGRATION CHECK TEMPLATE
;; ============================================================

;; Before any conclusions:
;; Evaluate against:
;; - identity BRIDGE_TCP_SOCKET_V1
;; - constraints PANIC_ONLY_NO_RESULT_TYPES, NO_NEW_REGISTRY_TYPE_REUSE_VALUE, THREE_PIECE_RULE
;; - priority precedent-consistency > correctness > demo-critical-path > performance
;; - risk tuple_field-silent-Unit medium, blocking-accept-no-timeout medium
;; - boundary default forbidden
;; - authority AI_PROPOSE_ONLY
;; - epistemics: outcomes fact-typed; predictions/unknowns carry tests


;; ============================================================
;; NEXT-TESTS (aggregated forward agenda)
;; ============================================================

(next-tests
  (test "cargo build 2>&1 | grep ^error | wc -l  → expect 0")
  (test "cargo test 2>&1 | tail -5  → expect 0 failed")
  (test "echo -n '{name}' | sha256sum for tcp_listen/accept/read/write/close → cross-check axreg identity")
  (test "cd axRegistry-working && bash validate.sh → expect all PASS")
  (test "net.rs #[test]: tcp_listen(0) → tuple_field destructure → accept/read/write round trip on 127.0.0.1")
  (test "N-thread concurrent tcp_listen(0) → assert N distinct ports, zero bind failures"))


;; ============================================================
;; REQUEST SECTION
;; ============================================================

Execute BRIDGE_TCP_SOCKET_V1: implement net.rs, wire dispatch, add registry
entries, propagate registry, update CLAUDE.md, run next-tests in order.
Reject any implementation step that violates a hard-limit constraint above.
Report outcome per (outcome) entry, re-typing each untested-prediction as
fact or disconfirmed based on actual test results — do not leave predictions
unresolved in the completion report.
```

(model-recommendation
  (recommended "Opus 4.8")
  (tier T3)
  (floor "Sonnet 4.6")
  (rationale "structured-design base (T2) + 6 hard-limit constraints (≥4 dense-invariant threshold) → escalate one tier to T3")
  (authority human)
  (may-decide false))

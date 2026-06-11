# BRIDGE_XBUNDLE_LINK_V1

Authoritative IS spec: cross-bundle linking for the Core IR 0.5 build path.
Status: P1–P3 implemented (identity exports, `--lib`/`--lib-dir`, transitive
closure + cycle safety). Remaining: P4 (content-addressed cache), P5 (Tier 2
sidecar manifest).

```lisp
;; ============================================================
;; INTENT DECLARATION — AUTHORITATIVE BLOCK
;; BRIDGE_XBUNDLE_LINK_V1
;; ============================================================
;; You are operating under INTENT_SYSTEM_SPEC.v1.0.
;; (intent-id BRIDGE_XBUNDLE_LINK_V1)
;;
;; ------------------------------------------------------------
;; LONG-CONTEXT REHYDRATION ANCHOR
;; ------------------------------------------------------------
;; All constraints remain binding. Absence implies forbidden.
;; Constraint > Priority > Goal.
;; North star: semantic traceability + AI gen + mech codegen simplicity.
;;
;; GRAVITY ANCHORS (rehydrate on long context):
;;   TWO_CLASSES          — every CCall target is either a MINTED identity
;;                          (impl lives in the bridge) or a §5b identity
;;                          (sha256(target_name); impl lives in another bundle)
;;   LINK_BY_IDENTITY     — exported symbols and cross-bundle resolution key on
;;                          target_identity, never on the bare name string
;;   SEMANTIC_TRACE_KEPT  — a cross-bundle call carries its identity hash end to
;;                          end; the linked symbol is derived from that hash
;;   CLOSURE_TRANSITIVE   — link the transitive closure of §5b dependencies
;;   CONTENT_ADDRESSED    — compiled artifacts are cached keyed by identity /
;;                          bundle content hash
;;   NO_NEW_AXREG_FIELDS  — the dependency index is a SIDECAR manifest or CLI
;;                          input, never a new axreg field (bridge CLAUDE.md)
;;   BRIDGE_ONLY          — bridge codegen + driver only; do NOT touch compiler
;;                          lowering, the spec-driven path, or the capnp schema

(intent-mode (state governance) (authority human) (downgrade-allowed false))

(intent
  (identity
    (name "BRIDGE_XBUNDLE_LINK_V1")
    (owner "chris")
    (scope "Add cross-bundle linking to the Core IR 0.5 build path so a bundle
            that calls a function defined in another bundle (a §5b user fn,
            identity = sha256(name)) compiles and runs. Covers: target
            classification, the export ABI, dependency resolution (explicit and
            index-driven), transitive closure, cycle handling, and the rustc
            link line. Excludes: compiler lowering, capnp schema, the minted
            built-in set (already resolved), the M1 surface."))

  (goal
    (primary "When the 0.5 emitter meets a CCall whose target is a §5b identity
              that is not a bridge built-in, resolve it to a symbol exported by
              the providing bundle's rlib, emit the extern declaration + call,
              and put that rlib on the link line — so the program builds and
              runs with the cross-bundle call intact.")
    (secondary "Keep every cross-bundle edge semantically traced: the linked
                symbol is a deterministic function of target_identity, so a
                bundle graph remains a complete picture of system behaviour.")
    (type outcome-oriented))

  (priority
    (semantic-traceability high)
    (mech-codegen-simplicity high)
    (incrementality high)         ;; Tier 1 before Tier 2; never regress single-bundle builds
    (runtime-performance medium)
    (build-speed medium))

  ;; ----------------------------------------------------------
  ;; CONSTRAINT — hard limits
  ;; ----------------------------------------------------------
  (constraint
    (hard-limit "TARGET CLASSIFICATION IS EXACT. A CCall target is §5b iff
                 target_identity == sha256(utf8(target_name)) AND it is not in
                 the bridge built-in map. Minted identities (declared in a --reg
                 with a non-sha256(name) hash) are bridge-resident and resolve
                 as today. Never guess; an identity that is neither built-in nor
                 a resolvable §5b provider is an UNKNOWN gate — hard halt.")
    (hard-limit "LINK BY IDENTITY. Each compiled bundle exports its fn under a
                 symbol deterministically derived from its identity (e.g.
                 `ax_fn_<64hex>`). Cross-bundle externs are emitted against that
                 identity-derived symbol. Name strings are display only and MUST
                 NOT be the link key — two fns may share a name.")
    (hard-limit "NO NAME-STRING DISPATCH. The bridge never invokes a fn by name
                 string. Resolution is identity -> provider -> identity-symbol.")
    (hard-limit "NO NEW AXREG FIELDS. The dependency index is a separate link
                 manifest file and/or the --lib / --lib-dir CLI inputs. Do NOT
                 add an artifact/bundle field to any .axreg entry.")
    (hard-limit "NO REGRESSIONS. Existing single-bundle 0.5 builds and the M1
                 baseline must keep passing. Cross-bundle resolution only
                 activates for §5b targets that were previously hard errors.")
    (hard-limit "FAIL CLOSED. A §5b target with no provider in the supplied
                 --lib set / manifest is an error with the identity, the name,
                 and the searched providers — never a stub, never a default.")
    (hard-limit "CYCLES ARE LEGAL ACROSS BUNDLES. Mutual recursion across
                 bundles must link (rlib externs resolve at final link). The
                 closure walker must not assume a DAG; it computes strongly-
                 connected components and links each SCC's members together.")
    (hard-limit "BRIDGE ONLY. Do not edit the compiler, the spec-driven path,
                 the lowering YAML, or axis_core_ir_0_5.capnp."))

  ;; ----------------------------------------------------------
  ;; RISK
  ;; ----------------------------------------------------------
  (risk ("Symbol keyed on name not identity -> two same-named fns collide,
          silent wrong-callee, no compile error" critical))
  (risk ("Cross-bundle recursion treated as a DAG -> closure walk loops or
          drops an edge" high))
  (risk ("Provider compiled with a different export convention than the caller
          expects -> link error or undefined symbol at final link" high))
  (risk ("Re-compiling every dependency on every build -> build-time blowup
          without the content-addressed cache" medium))

  ;; ----------------------------------------------------------
  ;; BOUNDARY
  ;; ----------------------------------------------------------
  (boundary
    ("Add --lib / --lib-dir handling to the 0.5 build path"            allowed)
    ("Add an identity-derived export symbol to emitted bundle rlibs"   allowed)
    ("Emit extern decls + calls for §5b targets in emit_rust_lib_from_bundle" allowed)
    ("Add a sidecar link-manifest reader (identity -> bundle path)"    allowed)
    ("Compute transitive closure + SCCs over the cross-bundle call graph" allowed)
    ("Add a content-addressed rlib cache keyed by identity/content hash" allowed)
    ("Pass --link-lib / --link-search to rustc for resolved providers" allowed)
    ("Add new bridge .rs files if genuinely required"                  allowed)
    ("Edit any axreg field set"                                        forbidden)
    ("Edit the capnp schema or compiler lowering"                      forbidden)
    ("Dispatch or link by bare fn name"                                forbidden)
    ("Emit a stub for an unresolved §5b target"                        forbidden)
    (default                                                           forbidden))

  ;; ============================================================
  ;; THE DESIGN — normative
  ;; ============================================================
  ;;
  ;; PART 1 — CLASSIFY EACH CCALL (emit_rust_lib_from_bundle / emit_node)
  ;;   built-in  : target_identity in bridge_builtin_map, OR resolves via --reg
  ;;               to a name present in symbol_map  -> call runtime path (today).
  ;;   §5b extern : target_identity == sha256(target_name) and not built-in
  ;;               -> resolve to a provider bundle, emit extern call (NEW).
  ;;   unknown    : neither -> UNKNOWN gate, hard halt.
  ;;
  ;; PART 2 — EXPORT ABI (the contract between provider and caller)
  ;;   When a bundle is compiled, in addition to its public entry symbol it
  ;;   exports the body under an identity-derived symbol:
  ;;       #[no_mangle] pub extern "C" fn ax_fn_<hex>(args: Value) -> Value
  ;;   where <hex> = hex(target_identity of the fn the bundle defines).
  ;;   A caller emits, per distinct §5b target:
  ;;       extern "C" { fn ax_fn_<hex>(args: Value) -> Value; }
  ;;   and routes the CCall to ax_fn_<hex>(...). Identity in, identity out.
  ;;
  ;; PART 3 — RESOLUTION SOURCES (how the bridge finds the provider)
  ;;   TIER 1 (explicit, implement first):
  ;;     --lib <bundle.coreir> (repeatable) and --lib-dir <dir>. The bridge
  ;;     reads each provider bundle, computes the identity of the fn it defines
  ;;     (sha256 of its declared name, or its recorded identity), and builds an
  ;;     identity -> provider-bundle map. Mirrors the existing 0.4 --lib path.
  ;;   TIER 2 (index-driven, implement second):
  ;;     A sidecar manifest (e.g. link-index.json: identity_hex -> bundle path
  ;;     or prebuilt rlib). The bridge auto-resolves §5b targets from it, with
  ;;     --lib as override. No axreg change.
  ;;
  ;; PART 4 — TRANSITIVE CLOSURE + CYCLES
  ;;   Starting from the root bundle, collect §5b targets; resolve each to a
  ;;   provider; recurse into that provider's §5b targets; continue to fixpoint.
  ;;   Build the call graph over bundles, compute SCCs. A trivial SCC compiles
  ;;   to its own rlib; a non-trivial SCC (mutual recursion) has its members
  ;;   linked together (mutual extern decls resolve at final link). Every node
  ;;   in the closure is compiled (or cache-hit) before the final link.
  ;;
  ;; PART 5 — DRIVER (cmd_build, 0.5 branch) + LINK LINE
  ;;   1. Load root bundle. 2. Resolve closure (Part 3/4). 3. For each provider
  ;;   bundle, emit its rlib exporting ax_fn_<hex> (reuse emit_rust_lib_from_bundle
  ;;   with the identity-export added), cache by content hash. 4. Emit the root
  ;;   with extern decls for its §5b targets. 5. rustc once, passing the bridge
  ;;   rlib + every provider rlib via --link-lib / --link-search (already
  ;;   supported by the 0.5 path's warning).
  ;;
  ;; PART 6 — CACHE
  ;;   Key each compiled provider rlib by the bundle's content hash (== its
  ;;   identity domain). On rebuild, skip compilation on cache hit. The cache is
  ;;   sound because identity is a content hash: same identity -> same bytes.

  ;; ----------------------------------------------------------
  ;; PROCEDURE (phased — do not skip)
  ;; ----------------------------------------------------------
  ;; P1. Add the identity-export symbol to emitted rlibs (Part 2). Confirm
  ;;     existing single-bundle builds + M1 baseline still pass.
  ;; P2. Tier 1: --lib / --lib-dir on the 0.5 path; classify + emit extern for
  ;;     §5b; resolve providers from --lib; link. Target: two_fn_call PASS
  ;;     (--lib fn_negate.coreir).
  ;; P3. Transitive closure + SCC cycle handling. Add an integration test with a
  ;;     two-bundle mutual-recursion fixture.
  ;; P4. Content-addressed cache.
  ;; P5. Tier 2: sidecar manifest auto-resolution; --lib remains override.

  ;; ----------------------------------------------------------
  ;; ERROR TAXONOMY
  ;; ----------------------------------------------------------
  ;; UNRESOLVED_XBUNDLE   §5b target with no provider in --lib/manifest.
  ;;                      Report identity, target_name, searched providers. Halt.
  ;; UNKNOWN_GATE         identity neither built-in nor §5b-resolvable. Halt.
  ;; SYMBOL_MISMATCH      provider rlib lacks ax_fn_<hex> (export bug). Halt.
  ;; CYCLE_UNLINKED       SCC member missing from the link set. Halt.

  ;; ----------------------------------------------------------
  ;; DELIVERABLE + VERIFICATION
  ;; ----------------------------------------------------------
  ;; - two_fn_call.coreir builds + runs (links fn_negate.coreir), output `true`.
  ;; - ai2-app.coreir stays a CORRECT failure: its target `f` has NO providing
  ;;   bundle -> UNRESOLVED_XBUNDLE, fail closed. (Not a regression; a true gap.)
  ;; - ai2-lam.coreir unchanged (compiler lowering gap, out of scope).
  ;; - A new two-bundle mutual-recursion fixture builds + runs.
  ;; - Single-bundle 0.5 builds and the 19-file M1 baseline unchanged.
  ;; - Headline target for the coreir sweep: 29/31 (up from 28), the remaining
  ;;   two being ai2-app (no provider) and ai2-lam (compiler gap).
) ;; END BRIDGE_XBUNDLE_LINK_V1
```

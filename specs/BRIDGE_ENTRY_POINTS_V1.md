# BRIDGE_ENTRY_POINTS_V1

Authoritative IS spec: named multi-entry concurrent execution for the 0.5 exe.
Builds on BRIDGE_XBUNDLE_LINK_V1 (identity→symbol resolution). Status: not yet
implemented.

```lisp
;; ============================================================
;; INTENT DECLARATION — AUTHORITATIVE BLOCK
;; BRIDGE_ENTRY_POINTS_V1
;; ============================================================
;; You are operating under INTENT_SYSTEM_SPEC.v1.0.
;; (intent-id BRIDGE_ENTRY_POINTS_V1)
;;
;; ------------------------------------------------------------
;; LONG-CONTEXT REHYDRATION ANCHOR
;; ------------------------------------------------------------
;; All constraints remain binding. Absence implies forbidden.
;; Constraint > Priority > Goal.
;; North star: semantic traceability + AI gen + mech codegen simplicity.
;; Builds ON TOP of BRIDGE_XBUNDLE_LINK_V1 (identity-symbol resolution).
;;
;; GRAVITY ANCHORS (rehydrate on long context):
;;   NO_ASSUMED_ENTRY     — the bridge never assumes the entry point; entries
;;                          are explicitly named. No --entries => single root
;;                          (today's behaviour) is the only implicit case.
;;   UNIFORM_ENTRY_ABI    — every entry is `fn(args: Value) -> Value` where
;;                          args = List(Text) = the full argv[1..]. The SAME
;;                          list is handed to every entry.
;;   RESOLVE_BY_IDENTITY  — entry names resolve to symbols by identity, reusing
;;                          the xbundle path (built-in runtime path OR
;;                          ax_fn_<hex> from a linked provider). Never by name.
;;   THREAD_PER_ENTRY     — each entry runs in its own thread; concurrency is
;;                          bridge physics, the entries declare none.
;;   ENTRIES_INDEPENDENT  — in THIS phase entries share no mutable state. Any
;;                          cross-entry sharing is the H1 channel layer, later.
;;   BRIDGE_ONLY          — bridge codegen + driver only. No capnp, no compiler,
;;                          no axreg field changes.

(intent-mode (state governance) (authority human) (downgrade-allowed false))

(intent
  (identity
    (name "BRIDGE_ENTRY_POINTS_V1")
    (owner "chris")
    (scope "Generalise the 0.5 executable from one assumed entry to a named set
            of entry points, each launched in its own thread and handed the full
            argv as a List(Text). Covers: --entries CLI, the uniform entry ABI,
            identity-based resolution (reusing xbundle), thread-per-entry
            driver codegen with panic isolation, and allowing foreign fns as
            entries under an ABI check. Excludes: registry-declared entries
            (startup/entry_point), inter-entry shared state / channels, the H1
            async layer — all deferred and captured here as forward constraints."))

  (goal
    (primary "Let a single bridge-built executable launch N named entry points
              concurrently — one OS thread each — every one receiving the full
              argv as List(Text), so multiple process loops run side by side in
              one process. With no --entries, behaviour is byte-identical to the
              current single-root exe.")
    (secondary "Keep every entry semantically traced: an entry is dispatched by
                its identity-derived symbol, so the launched set is a complete,
                hash-keyed picture of what the program runs.")
    (type outcome-oriented))

  (priority
    (semantic-traceability high)
    (mech-codegen-simplicity high)
    (back-compat high)
    (fault-isolation high)
    (memory-footprint medium))

  ;; ----------------------------------------------------------
  ;; CONSTRAINT — hard limits
  ;; ----------------------------------------------------------
  (constraint
    (hard-limit "NO ASSUMED ENTRY. The generated main launches exactly the
                 entries named by --entries. If --entries is absent, the single
                 bundle root is the one entry (back-compat). The bridge must
                 never silently pick an entry by position or convention beyond
                 that one explicit fallback.")
    (hard-limit "UNIFORM ENTRY ABI. Every entry symbol is
                 `extern \"C\" fn(args: Value) -> Value`. The driver builds
                 args = Value::List(env::args()[1..] -> Value::Str) ONCE and
                 passes a clone to every entry. Entries receive the whole argv;
                 the bridge does no per-entry arg parsing.")
    (hard-limit "RESOLVE BY IDENTITY (reuse xbundle). Each entry name resolves
                 to identity = sha256(name) (§5b) or a minted built-in. §5b
                 entries resolve to ax_fn_<hex> from a provider linked via
                 --lib/--lib-dir, through the existing collect_xbundle_closure.
                 An entry with no provider and no built-in is UNRESOLVED_ENTRY —
                 hard halt, never a stub.")
    (hard-limit "FOREIGN FNS MAY BE ENTRIES — under an ABI check. A foreign
                 (built-in) fn is a legal entry IFF its registry contract is
                 `in (TextList)`. Validate at build time; reject a misfit as
                 ENTRY_ABI_MISMATCH. Bundle entries take the C ABI symbol and
                 are convention-enforced (the surface must give them a
                 List(Text) parameter).")
    (hard-limit "THREAD-PER-ENTRY WITH PANIC ISOLATION. Spawn one thread per
                 entry. Each entry call is wrapped in catch_unwind so one loop
                 panicking does not abort the others. The driver joins all
                 threads; an entry that never returns keeps the process alive
                 (intended for daemon loops). Exit code is non-zero if any entry
                 panicked or returned an error result.")
    (hard-limit "ENTRIES ARE INDEPENDENT THIS PHASE. The generated entries
                 share no mutable state. Do NOT add shared globals, channels, or
                 cross-entry handoff here. When shared state lands (H1), it MUST
                 use the single-writer/multi-reader substrate and carry interned
                 ids, never Values — captured as a forward constraint, not built
                 now.")
    (hard-limit "BACK-COMPAT. With no --entries, the emitted exe is behaviourally
                 identical to today: one entry = the bundle root, args = the same
                 value it gets now. Existing 0.5 builds, xbundle tests, and the
                 M1 baseline must not change.")
    (hard-limit "BRIDGE ONLY. No capnp schema, no compiler/lowering, no axreg
                 field additions."))

  ;; ----------------------------------------------------------
  ;; RISK
  ;; ----------------------------------------------------------
  (risk ("An entry whose real ABI is not (TextList) is launched -> the List is
          misread as the wrong type. Mitigate: build-time contract check for
          foreign entries; documented convention for bundle entries." high))
  (risk ("One entry panics and the default panic=abort tears down the process,
          killing every other loop. Mitigate: per-thread catch_unwind + aggregate
          exit, and ensure the build profile is not panic=abort for the exe." high))
  (risk ("Two providers export a same-named entry with different identities ->
          wrong callee launched. Mitigate: resolve by identity (inherited from
          xbundle), never by bare name." high))
  (risk ("Many entries x default ~2MB stack -> avoidable memory blow-up.
          Mitigate: configurable per-entry stack size with a sane default." medium))

  ;; ----------------------------------------------------------
  ;; BOUNDARY
  ;; ----------------------------------------------------------
  (boundary
    ("Add --entries <a,b,c> (and repeatable --entry <name>) to cmd_build"   allowed)
    ("Emit a thread-per-entry main that feeds each entry the argv List(Text)" allowed)
    ("Resolve entry names via the existing identity->symbol xbundle path"    allowed)
    ("Validate foreign-fn entries against `in (TextList)` from --reg"        allowed)
    ("Add catch_unwind isolation + aggregate exit code"                     allowed)
    ("Add an optional --entry-stack-size <bytes>"                            allowed)
    ("Add new bridge .rs files if genuinely required"                       allowed)
    ("Assume/guess an entry beyond the single-root fallback"                forbidden)
    ("Dispatch an entry by bare name"                                       forbidden)
    ("Add shared globals/channels between entries in this phase"            forbidden)
    ("Edit axreg fields, capnp, or the compiler"                            forbidden)
    (default                                                                forbidden))

  ;; ============================================================
  ;; THE DESIGN — normative
  ;; ============================================================
  ;;
  ;; PART 1 — CLI (cmd_build, 0.5 branch)
  ;;   --entries name1,name2,...   comma list of entry fn names
  ;;   --entry  name               repeatable alias (robust if names ever vary)
  ;;   (absent)                    => single entry = the bundle root (today)
  ;;   --entry-stack-size <bytes>  optional; default a modest value (e.g. 1 MiB)
  ;;
  ;; PART 2 — ENTRY ABI (uniform)
  ;;   Symbol shape:  extern "C" fn(args: Value) -> Value
  ;;   The driver builds, once:
  ;;       let argv: Vec<Value> = std::env::args().skip(1)
  ;;                                 .map(|s| Value::Str(intern_str(&s))).collect();
  ;;       let args = Value::List(argv);
  ;;   and passes args.clone() to every entry. Entries own argv interpretation.
  ;;
  ;; PART 3 — RESOLUTION (reuse BRIDGE_XBUNDLE_LINK_V1)
  ;;   For each entry name N:
  ;;     id = sha256(N)
  ;;     - built-in: id in bridge_builtin_map OR --reg name in symbol_map
  ;;                 -> runtime path (foreign entry; go to Part 4 check)
  ;;     - §5b:      provider bundle in the --lib closure exports ax_fn_<hex(id)>
  ;;                 -> extern decl + that symbol
  ;;     - else:     UNRESOLVED_ENTRY (identity, name, searched providers) -> halt
  ;;   Entries are additional roots fed to collect_xbundle_closure so their
  ;;   transitive providers link too.
  ;;
  ;; PART 4 — FOREIGN-ENTRY ABI CHECK
  ;;   If N resolves to a built-in, require its registry contract `in (TextList)`.
  ;;   Pass -> emit a call to the runtime path with the args List.
  ;;   Fail -> ENTRY_ABI_MISMATCH (name, actual `in`, expected `(TextList)`). Halt.
  ;;
  ;; PART 5 — DRIVER CODEGEN (generated main)
  ;;   fn main() {
  ;;       init_runtime();
  ;;       let args = <Part 2>;
  ;;       let mut handles = vec![];
  ;;       for (name, call) in entries {                  // call = a fn pointer/shim
  ;;           let a = args.clone();
  ;;           let h = std::thread::Builder::new()
  ;;                     .name(name).stack_size(STACK)
  ;;                     .spawn(move || {
  ;;                         std::panic::catch_unwind(|| call(a))
  ;;                     }).expect("spawn");
  ;;           handles.push((name, h));
  ;;       }
  ;;       let mut bad = false;
  ;;       for (name, h) in handles {
  ;;           match h.join() {
  ;;               Ok(Ok(v))  => println!("{name}: {v}"),
  ;;               Ok(Err(_)) => { eprintln!("{name}: PANIC"); bad = true; }
  ;;               Err(_)     => { eprintln!("{name}: thread join failed"); bad = true; }
  ;;           }
  ;;       }
  ;;       std::process::exit(if bad {1} else {0});
  ;;   }
  ;;   Notes: the single-entry (no --entries) case may keep the existing
  ;;   straight-line main; the thread path activates only with >=1 named entry.
  ;;   The exe build profile MUST NOT be panic=abort, or catch_unwind is moot.

  ;; ----------------------------------------------------------
  ;; PROCEDURE (phased — do not skip)
  ;; ----------------------------------------------------------
  ;; P1. Parse --entries/--entry; default to single-root. Confirm no-arg builds
  ;;     are byte-identical and the M1 + xbundle suites stay green.
  ;; P2. Multi-entry thread driver (Part 5): spawn, catch_unwind, join, aggregate.
  ;; P3. Foreign-entry ABI validation (Part 4).
  ;; P4. --entry-stack-size + labeled output.
  ;; Tests:
  ;;   - two named bundle entries launch concurrently, each sees full argv.
  ;;   - a foreign fn with `in (TextList)` works as an entry.
  ;;   - a foreign fn without (TextList) is rejected (ENTRY_ABI_MISMATCH).
  ;;   - one entry panics -> others complete, exit code != 0.
  ;;   - no --entries -> identical output to the pre-change single-root exe.

  ;; ----------------------------------------------------------
  ;; ERROR TAXONOMY
  ;; ----------------------------------------------------------
  ;; UNRESOLVED_ENTRY    named entry not a built-in and no provider. Halt.
  ;; ENTRY_ABI_MISMATCH  foreign entry whose `in` != (TextList). Halt.
  ;; ENTRY_PANIC         runtime, isolated per thread; surfaced in exit code.

  ;; ----------------------------------------------------------
  ;; DELIVERABLE + VERIFICATION
  ;; ----------------------------------------------------------
  ;; - `--entries loopA,loopB` runs both concurrently in one exe, each handed the
  ;;   full argv as List(Text); labeled per-entry results; non-zero exit if any
  ;;   panicked.
  ;; - A foreign fn entry with (TextList) launches; a non-(TextList) one is
  ;;   rejected at build.
  ;; - Killing one entry (panic) leaves the others running.
  ;; - No --entries => unchanged single-root behaviour; M1 baseline + xbundle
  ;;   tests untouched.
  ;; - FORWARD CONSTRAINT recorded (not built): when entries gain shared state,
  ;;   it uses the single-writer/multi-reader substrate and carries interned ids,
  ;;   never Values.
) ;; END BRIDGE_ENTRY_POINTS_V1
```

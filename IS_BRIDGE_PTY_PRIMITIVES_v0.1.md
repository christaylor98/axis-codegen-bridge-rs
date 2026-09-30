```lisp
;; ============================================================
;; INTENT DECLARATION — AUTHORITATIVE BLOCK
;; ============================================================

You are operating under INTENT_SYSTEM_SPEC.v1.0.

(intent-id BRIDGE_PTY_V1)

;; ------------------------------------------------------------
;; LONG-CONTEXT REHYDRATION ANCHOR
;; ------------------------------------------------------------
;; All constraints remain binding.
;; Absence implies forbidden.
;; Constraint > Priority > Goal.
;; Authority separation must be preserved.
;; BRIDGE_PTY_V1 governs interpretation.


(intent-mode
  (state structured-design)
  (authority human)
  (downgrade-allowed false))


(intent

  ;; ------------------------------------------------------------
  ;; IDENTITY
  ;; ------------------------------------------------------------
  (identity
    (name "BRIDGE_PTY_V1")
    (owner "Chris")
    (scope "A GENERIC pseudo-terminal capability for axis-codegen-bridge-rs: start any program as the session leader of a fresh pty, read its output with a caller-chosen timeout, write its input, resize its window, ask how it ended, close it. Any AI3/M1 program can drive any interactive program through it. Excludes: environment/cwd control, shells, output parsing, expect-style matching, signals other than close's hangup-then-kill, non-unix hosts."))


  ;; ------------------------------------------------------------
  ;; GOAL
  ;; ------------------------------------------------------------
  (goal
    (primary "Land pty_open / pty_read / pty_write / pty_resize / pty_status / pty_close as synchronous fullIo bridge leaf fns, so a program that needs a real terminal can be driven from AI3/M1 with no Python or other foreign harness.")
    (secondary
      "Every outcome a caller must act on is distinguishable from the return values: data, nothing-yet, child-gone, exit code, killed-by-signal, could-not-start."
      "Exit-status encoding is proc_run's (process.rs), extended by one disjoint value for 'still running' — one status vocabulary across the bridge, not two."
      "No new dependency: libc (already a dependency) only. No new registry type, no new Value variant.")
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
      (role "Design proposal, IS authoring, implementation of the approved IS")
      (authority bounded)
      (may-decide false)))


  ;; ------------------------------------------------------------
  ;; THE CONTRACT — six leaf fns
  ;; ------------------------------------------------------------
  ;;
  ;; pty_open(program: Text, argv: TextList, rows: Int, cols: Int) -> Int
  ;;   Allocate a pty pair, set the window to rows x cols (0 x 0 = leave the
  ;;   kernel default), start `program` with `argv` (exact list, no shell, argv[0]
  ;;   is NOT included — same as proc_run) with the pty slave as stdin, stdout
  ;;   and stderr, in a new session with the slave as controlling terminal.
  ;;   Environment and cwd are inherited. Returns a handle >= 1.
  ;;   Returns -256 (proc_run's NO_START) when the program could not be started
  ;;   (not found, not executable) — an ordinary outcome, not a panic.
  ;;   Panics when the pty itself cannot be allocated, or rows/cols are outside
  ;;   0..=65535 (a violated pre-condition).
  ;;
  ;; pty_read(handle: Int, timeout_ms: Int) -> Bytes
  ;;   Wait up to timeout_ms (0 = don't wait; must be >= 0) for output, then
  ;;   return what is available (at most 64 KiB per call). Non-empty = output.
  ;;   Empty = nothing arrived within the timeout OR the child side of the pty
  ;;   is closed (EOF; on Linux the master reports EIO). Empty never means
  ;;   "error". To tell the two empties apart, ask pty_status. Output the child
  ;;   wrote before exiting is still returned until drained.
  ;;   Bytes, not Text: a read may split a UTF-8 sequence and terminal output is
  ;;   not guaranteed UTF-8; bytes_to_text is the caller's step.
  ;;
  ;; pty_write(handle: Int, data: Bytes) -> Unit
  ;;   Write all of data to the child's input. Child side gone (EIO) is a
  ;;   no-op, like a vanished TCP peer — pty_status reports it. Other I/O errors
  ;;   panic.
  ;;
  ;; pty_resize(handle: Int, rows: Int, cols: Int) -> Unit
  ;;   Set the window size; the kernel delivers SIGWINCH to the child's
  ;;   foreground process group. Same range pre-condition as pty_open.
  ;;
  ;; pty_status(handle: Int) -> Int
  ;;   Non-blocking. proc_run's bands plus one:
  ;;        0 ..= 255   exited; this is its exit code
  ;;       -1 ..= -64   killed by a signal; this is -signum
  ;;            -258    STILL RUNNING (new; disjoint from every proc_run band)
  ;;   Once the child has ended the answer is fixed (reaped once, cached).
  ;;
  ;; pty_close(handle: Int) -> Int
  ;;   Release the handle: SIGHUP the child's process group (it leads its own
  ;;   session, so pid = pgid; explicit, not via fd close, so a concurrent read
  ;;   holding the fd cannot delay it),
  ;;   wait up to 1000 ms for the child to end, then SIGKILL its process group
  ;;   and wait. Returns the final status in pty_status's bands (never -258).
  ;;   The handle is dead afterwards.
  ;;
  ;; Every fn but pty_open panics on an unknown handle (a bug, not an outcome).
  ;;
  ;; Canonical polling shape (the reason for the design):
  ;;   loop_while(not(done), step) where step = pty_read(h, 50) ... and done =
  ;;   bytes_len(chunk) == 0 AND pty_status(h) != -258.
  ;; A caller can never spin forever on a dead child, and never mistakes a slow
  ;; child for a dead one.


  ;; ------------------------------------------------------------
  ;; PRIORITY
  ;; ------------------------------------------------------------
  (priority
    (generality high)
    (precedent-consistency high)
    (correctness high)
    (performance low))


  ;; ------------------------------------------------------------
  ;; CONSTRAINT — HARD LIMITS
  ;; ------------------------------------------------------------
  (constraint
    (hard-limit "GENERIC: no fn, parameter, default or test is shaped around one consumer. Nothing in pty.rs names axDisplay, a key code, a screen size or an escape sequence."))

  (constraint
    (hard-limit "Plain return types, no Result wrappers (CLAUDE.md plain-return-type convention). Ordinary world outcomes are values (empty Bytes, status bands, -256); bugs panic."))

  (constraint
    (hard-limit "Status encoding reuses proc_run's bands verbatim; the only addition is -258 = still running."))

  (constraint
    (hard-limit "No new registry type and no new Value variant. TextList in, Bytes in/out, Int handles."))

  (constraint
    (hard-limit "libc only — no pty crate, no nix. posix_openpt/grantpt/unlockpt/ptsname_r (all in libc proper, no -lutil) + std::process::Command with pre_exec for setsid/TIOCSCTTY."))

  (constraint
    (hard-limit "Synchronous fullIo leaves; no coupling to channels.rs or any async layer."))

  (constraint
    (hard-limit "No orchestration in Rust (proc_run's NO_ORCHESTRATION_IN_RUST): no retry, no expect/match, no output parsing, no env/cwd manipulation. Decisions live in AI3/M1."))

  (constraint
    (hard-limit "THREE_PIECE_RULE: pty.rs + dispatch rows (rust_05.rs symbol map + native arg types) + registry entries (axRegistry-working/axis-bridge.axreg, axAI-axlang-gen-working/registries/axis-bridge.axreg) + CLAUDE.md list land together. Leaf identity = sha256(utf8 name)."))


  ;; ------------------------------------------------------------
  ;; RISK
  ;; ------------------------------------------------------------
  (risk
    ("pty_close on a handle another thread is blocked in pty_read on: the fd is held by an Arc, so it stays open until that read returns — no use-after-close; the hangup is sent by killpg, so it is not delayed" low))

  (risk
    ("Linux discards pty output still queued when the LAST slave fd closes only on some older kernels; modern kernels keep it readable until drained. A test proves drain-after-exit on this host" low))

  (risk
    ("A child that ignores SIGHUP and SIGKILL cannot exist; a child stuck in uninterruptible sleep (D state) makes pty_close's final wait block" low))

  (risk
    ("pty_close's 1000 ms grace period is a fixed policy; a caller needing a different one has no knob in v1" low))


  ;; ------------------------------------------------------------
  ;; BOUNDARY
  ;; ------------------------------------------------------------
  (boundary
    ("pty_open / pty_read / pty_write / pty_resize / pty_status / pty_close" allowed)
    ("unit tests driving real programs (/bin/sh, stty, cat) through the six fns" allowed)
    ("environment or cwd parameters" forbidden)
    ("arbitrary signal delivery (pty_kill)" forbidden)
    ("expect/match helpers in Rust" forbidden)
    ("changes to proc_run or tty.rs" forbidden)
    ("migrating any consumer's harness onto these fns" forbidden)   ;; a separate, consumer-side intent
    (default forbidden))


  ;; ------------------------------------------------------------
  ;; UNKNOWN
  ;; ------------------------------------------------------------
  (unknown
    ("Whether a pty_signal(handle, signum) is wanted generically (Ctrl-C can already be sent as byte 0x03 through pty_write, which the line discipline turns into SIGINT)."))

  (unknown
    ("Whether consumers want env control; proc_run deliberately has none, so v1 matches it."))


  ;; ------------------------------------------------------------
  ;; ASSUMPTION
  ;; ------------------------------------------------------------
  (assumption
    ("native_call_fn_arg_types handles a TextList arg as Value, as proc_run's (Text, Value) row does." confirmed))

  (assumption
    ("posix_openpt/grantpt/unlockpt/ptsname_r and TIOCSCTTY/TIOCSWINSZ are exported by libc 0.2.186 for linux." confirmed))


  ;; ------------------------------------------------------------
  ;; OUTCOME — to be fact-typed against test results on completion
  ;; ------------------------------------------------------------
  (outcome
    (fact "14 unit tests in src/runtime/pty.rs pass, 20/20 repeated runs: window size + controlling tty seen by the child (stty size, tty); empty read after the timeout with status -258 while live; output written just before exit still drained; exit code / -signum / -256 each in band; write received, including late after a wait; resize observed; write after exit a no-op; close = -SIGHUP promptly, -SIGKILL within grace+0.5 s for a HUP-ignoring child; unknown handle and bad size panic.")
    (fact "End to end from AI3: a pty_e2e.ai3 using all six fns compiled and verified against axRegistry-working (axis), linked by `axis-codegen-bridge build --exe`, and ran: early status -258, child saw resize 40x120, received 'hello', exit 3 reported by both pty_status and pty_close.")
    (fact "axRegistry-working validate.sh: canonical set loads, zero conflicts. Identities = sha256(name), rule re-verified against tcp_write.")
    (fact "cargo test: all pty tests green; the only failures are pre-existing and unrelated — pg_store::round_trips (needs a live postgres) and the runtime::fail doctest (fails identically with these changes stashed).")
    (fact "process.rs: NO_START, NO_REASON and terminating_signal widened to pub(crate) for reuse; proc_run behaviour unchanged."))

  ;; ------------------------------------------------------------
  ;; MODE LOCK
  ;; ------------------------------------------------------------
  (mode
    (phase execution)
    (design allowed)
    (execution allowed))


  ;; ------------------------------------------------------------
  ;; STATUS
  ;; ------------------------------------------------------------
  (status
    (state shipped)    ;; Chris, 2026-09-30: "spec first, then implement"; "make it generic"
    (authority human)
    (execution-allowed true))
)

;; ============================================================
;; INVARIANT COMPRESSION LAYER (LONG CONTEXT SURVIVAL)
;; ============================================================

(intent-invariants
  (hard-limit GENERIC_NOT_CONSUMER_SHAPED)
  (hard-limit PLAIN_RETURNS_OUTCOMES_AS_VALUES)
  (hard-limit PROC_RUN_STATUS_BANDS_PLUS_RUNNING)
  (hard-limit NO_NEW_TYPE_NO_NEW_VARIANT)
  (hard-limit LIBC_ONLY)
  (hard-limit NO_ASYNC_LAYER_COUPLING)
  (hard-limit NO_ORCHESTRATION_IN_RUST)
  (hard-limit THREE_PIECE_RULE))

;; Semantic gravity anchors:
;; EMPTY_READ_PLUS_STATUS_DISAMBIGUATES
;; RUNNING_IS_MINUS_258
;; BYTES_NOT_TEXT


;; ============================================================
;; ARCHITECTURAL SPINE (RESPONSIBILITY ONLY — NOT DESIGN)
;; ============================================================

(spine
  "src/runtime/pty.rs – handle table, six leaf fns, tests."
  "src/runtime/mod.rs – pub mod pty."
  "src/emit/rust_05.rs – six symbol-map rows + six native_call_fn_arg_types rows."
  "CLAUDE.md – six entries in the bridge fn list."
  "axRegistry-working/axis-bridge.axreg – six fn entries."
  "axAI-axlang-gen-working/registries/axis-bridge.axreg – six fn entries.")


(spine-rules
  (only pty.rs may hold pty/child state)
  (no Result/Option wrapper types introduced)
  (no new type declarations)
  (scope changes annotated in-artifact, not silent))
```

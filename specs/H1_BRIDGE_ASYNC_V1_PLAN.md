# H1_BRIDGE_ASYNC_V1 — Implementation Plan

Scope: bridge-side execution model for H1 async/IPC/process loops. Three foreign fns
(`event_subscribe`, `wait`, `channel_send`), entry-point routing, background dispatch,
event registry, channel delivery, startup launch, and the eight bridge checking rules.
Excludes H1 surface YAML, Core IR capnp schema, M1, ratchet.

## 0. Working assumptions (override if wrong)

These mirror the recommended answers to the three forks; the plan is built on them.

1. **Registry format** — amend `CLAUDE.md` to permit the new fn fields
   (`entry_point`, `background`, `startup`, `callbacks`) plus `channel` blocks and the
   `List` / `Fn` types, then build a *structured* axreg parser. The spec's boundary block
   explicitly allows these additions, so it overrides the current "never add fields" rule.
2. **Fn references** — a handler is represented by its **registry identity hash** carried in
   an existing `Value` (no new capnp node, no `Value::Fn`). The bridge always invokes by
   hash → satisfies `SEMANTIC_TRACE_PRESERVED` and RULE 3 for free.
3. **Execution home** — the async machinery lives in **new bridge runtime modules** that the
   generated Rust links against; startup/entry-point wiring is **emitted into the generated
   driver**. This fits the existing codegen-bridge architecture (load Core IR → resolve by
   identity → emit Rust → compile).

## 1. Current state (verified)

- `src/main.rs` — CLI: `build` / `build05` / `bundle` / `inspect`. Loads `.coreir` bundles,
  validates `CCall` targets, emits Rust, compiles via rustc.
- Registry parsing is shallow: `load_registry_names` (main.rs) collects `fn` names into a
  `HashSet`; `rust_05::load_registry_identity_map` parses only `fn` / `identity` / `end`.
  **No field is parsed into structured form** — `effect`/`in`/`out`/`kind` are decorative
  to the codegen path today.
- `src/runtime/*.rs` — foreign fn implementations linked into generated programs.
  `runtime/value.rs` `Value` = `Int | Bool | Str(u32) | Unit | Tuple | List | Ctor`.
  **No `Fn` variant; no event/channel/scheduler concept anywhere.**
- `src/emit/rust.rs` + `rust_05.rs` — identity→bridge-path builtin map; `CCall` resolved to
  builtin path or registry name; unresolved target = hard error.
- `registry/*.axreg` — every `fn` already carries `in/out/effect/kind/deterministic/
  idempotent`; types declared via `type X = prim/sum/list …` (so `List`/`Fn` are addable as
  type decls, not magic).
- `src/executor.rs` — a *separate* minimal test interpreter (Unit/Int/Bool only); not the
  production path. Ignore for this work except as a unit-test harness pattern.

## 2. Governance item to resolve first (blocking)

`CLAUDE.md` currently forbids adding axreg fields and limits types to
`Int Text Bool Unit TextList ResultText ResultUnit Value`. The spec needs
`entry_point/background/startup/callbacks`, `channel … end` blocks, and `List`/`Fn`.
Action: update the AXREG section of `CLAUDE.md` to whitelist exactly these additions and
document their grammar, *before* touching any `.axreg`. (`arity`/`profile` stay forbidden.)
This keeps the codebase's own guardrails consistent with the spec.

## 3. Workstreams

### WS-1 — Structured registry parser  *(foundation; everything depends on it)*
New module `src/runtime/axreg.rs` (or `src/registry_format.rs`): parse a full axreg into
typed structs — `FnEntry { name, identity, kind, ins, out, effect, deterministic,
idempotent, entry_point: Option<String>, background: bool, startup: bool,
callbacks: Vec<usize> }` and `ChannelDecl { name, ty }`. Replace the two ad-hoc parsers in
`main.rs` and `rust_05.rs` with calls into it (keep their existing return shapes as thin
adapters so nothing downstream breaks). Reject `arity`/`profile`; validate type names
against the declared `type` set.
Tests: round-trip parse of `axis-codegen-bridge.axreg`; parse of new async fields; rejection
of forbidden fields.

### WS-2 — Bridge runtime: event registry + subscription tables
New `src/runtime/bridge/events.rs`. Per-process subscription table (RULE 4): `event_subscribe`
**replaces atomically**, never appends (`BRIDGE_OWNS_SUBSCRIPTIONS`). Event IDs are `Int`
constants; channel events map to `EVENT_CHANNEL_<name>` (WS-4). State lives bridge-side, keyed
by execution-context id — H1 holds nothing between calls.
Implements foreign fn `event_subscribe(events: List): ResultUnit`.

### WS-3 — Bridge runtime: the `wait` primitive + closure-rule enforcement
In `src/runtime/bridge/wait.rs`. `wait(handler: Fn): ResultUnit` suspends the context until any
subscribed event fires, then calls `handler` **synchronously within wait's own frame** with a
`List` of `Value(event_id: Int, data: Value)` pairs (`WAIT_ALWAYS_LIST`: empty=timeout/spurious,
one=normal, many=batched). Invoke handler **by identity hash** (assumption 2). Enforce
`callbacks [arg:0]` (RULE 1, `CLOSURE_RULE_HARD`): the handler hash must not be stored, escape
the frame, or be invoked from any timer/interrupt/async context. Encode the callback contract so
violations are structurally impossible (handler hash is a local, dropped when `wait` returns).

### WS-4 — Bridge runtime: channels
In `src/runtime/bridge/channels.rs`. `channel_send(name: Text, data: Value): ResultUnit`
(RULE 6). Channels are **static** (`CHANNELS_STATIC`): names resolved against `ChannelDecl`s
from the registry; unknown name = hard error. Bridge owns buffers; `channel_send` enqueues and
fires `EVENT_CHANNEL_<name>` to subscribers; pending messages **batched** into the `List` that
`wait` delivers. Unidirectional, best-effort unless declared persistent.

### WS-5 — Dispatch / background execution
In `src/runtime/bridge/dispatch.rs`. On dispatching an entry/handler fn, read the registry
`background` flag (`BACKGROUND_FLAG_IN_REGISTRY`, RULE 7): `false` → run inline (loop blocks);
`true` → spawn an independent execution context and return immediately. Spawned context owns its
own subscription list and obeys the closure rule independently. H1 calls the fn identically
regardless — the flag is checked only on the bridge side.

### WS-6 — Startup launch + entry-point routing (emitted driver)
Extend `src/emit/rust_05.rs` (and `rust.rs` if the 0.4 path is in scope) to emit a generated
`main`/driver that, at boot: (1) loads registry + resolves all identity hashes; (2) launches
every `startup true` fn in its own context (multiple expected — one per loop); (3) wires each
`entry_point <kind>` fn to its external source by identity hash (`STATIC_ENTRY_POINTS_ONLY` —
declared in registry, never registered at runtime); (4) creates declared channels; (5) runs the
event loop. Entry points are invoked **only with complete typed H1 values** (RULE 2,
`MEANING_TRANSFER_ONLY`) — the meaning/physics boundary check lives here.

### WS-7 — Registry entries + builtin wiring for the three fns
Add `event_subscribe`, `wait`, `channel_send` (and any `is_*` event-inspection foreign fns the
dispatch examples need) to `registry/axis-codegen-bridge.axreg` with correct
`in/out/effect/identity`, and `wait` with `callbacks [arg:0]`. Register their bridge paths in the
builtin identity→path map in `src/emit/rust.rs`. Derive each identity via
`registry_compound_id(name, contract)` — do **not** invent hashes (per CLAUDE.md rule 2/3).
Add `List`/`Fn` `type` declarations and the example `channel` blocks.

### WS-8 — Bridge checking rules as enforced invariants
Centralize the eight RULEs (PART 11) as explicit checks with tests, not prose:
closure rule (WS-3), meaning transfer (WS-6), identity-hash routing (already structural via
assumption 2 + existing resolver), subscription ownership (WS-2), wait-as-List (WS-3),
channel delivery + unknown-name rejection (WS-4), background dispatch (WS-5), static topology /
reject undeclared processes & channels (WS-6/WS-4).

## 4. Risk → mitigation (from spec RISK block)

- *Fn ref stored/escapes & called async* (critical) → WS-3 makes the handler hash a frame-local
  with no storage path; add a test asserting no `wait` handler survives its frame.
- *Entry point gets partial/raw data* (critical) → WS-6 boundary check: refuse to invoke unless
  args express as clean H1 types.
- *wait delivers scalar not List* (critical) → WS-3 type makes `List` the only handler arg shape.
- *Invoke by name string* (high) → assumption 2 + existing identity resolver; no name-string path.
- *Dynamic channel / dynamic spawn* (high) → WS-4/WS-6 reject anything not in the registry.

## 5. Suggested sequence

WS-1 → (WS-2, WS-4 parser-dependent) → WS-3 → WS-5 → WS-6 → WS-7 → WS-8, with the `CLAUDE.md`
amendment (§2) landed before WS-7 touches any `.axreg`. WS-1 unblocks everything; WS-7/WS-8 are
the integration close-out.

## 6. Verification

Per-WS unit tests (parser round-trip, atomic subscription replace, List-always delivery,
unknown-channel rejection, background spawn returns immediately, closure-rule escape rejected).
End-to-end: build the PART-10 kernel-boot topology (three startup loops + two entry points + two
channels) from a fixture bundle and assert the generated driver launches all three contexts,
wires both entry points by hash, and routes a `channel_send` to its subscriber as a batched List.
Confirm `cargo test` stays green and the bridge still compiles after each WS.

## 7. Open questions for Chris

- Confirm the three §0 assumptions (registry amendment, identity-hash Fn refs, runtime-lib +
  emitted driver). Any "no" reshapes the affected workstream.
- Is the 0.4 emit path (`rust.rs`) in scope for startup/entry-point wiring, or 0.5 (`rust_05.rs`)
  only?
- Persistent channels: needed in v1, or defer (best-effort only for now)?
- Are the `is_*` event-inspection fns (e.g. `is_packet_event`) bridge foreign fns to add now, or
  authored later in H1?

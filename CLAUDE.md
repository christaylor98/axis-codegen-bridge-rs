## AXREG FORMAT — strict rules, no exceptions

Every `fn` entry in an axreg file has exactly these fields in this order:

```
fn <name>
  identity <0xHASH>
  kind     leaf | composite
  in       (<Type>, <Type>, ...)   ← comma-separated, parenthesised
  out      <Type>
  effect   pure | reads | writes | fullIo
  deterministic true | false
  idempotent    true | false
end
```

### VALID TYPE NAMES — only these, no others

```
Int  Text  Bool  Unit  Dec  Float  Bytes  TextList  Value  ValueList  Fn
```

`ResultText` / `ResultUnit` / `ResultBytes` no longer exist — every bridge fn
returns a plain type and panics on failure. See "Plain-return-type convention"
below.

`Dec` is `rust_decimal::Decimal` (128-bit fixed decimal, ~28 significant digits;
PrimCode 7). `Float` is IEEE 754 f64 (PrimCode 3). Both are runtime `Value`
variants (`Value::Dec`, `Value::Float`) — added by BRIDGE_VALUE_COERCION_V1.

`Bytes` is an opaque byte blob (`Value::Bytes(Vec<u8>)`, PrimCode 4) — added by
BRIDGE_BYTES_IO_M1. Not a `List<Int>` — kept as `Vec<u8>` so the bridge can pass
blobs without per-element overhead.

### Plain-return-type convention (universal, no exceptions)

Bridge functions return plain types. **No Result wrappers.** A type mismatch is
a compile-time error; runtime failures panic with a clear message. Pre-conditions
own the rest. Examples:

- `fs_read_text(Text) -> Text`        — panics on read error
- `fs_read_bytes(Text) -> Bytes`      — panics on read error
- `fs_write_bytes(Text, Bytes) -> Unit` — panics on write error
- `fs_mkdir_p(Text) -> Unit`          — panics on mkdir error
- `bytes_to_text(Bytes) -> Text`      — panics on invalid UTF-8
- `hash256_parse(Text) -> Text`       — panics on invalid hash format
- `ir_write_bundle(Value, Text) -> Unit` — panics on IO/encode error
- `tcp_listen(Int) -> Value` — bind `0.0.0.0:port` (0 = ephemeral); returns
  `Value::Tuple([handle, bound_port])`, destructured with `tuple_field`. Panics
  on bind error.
- `tcp_accept(Int) -> Int` — block for a peer; returns a stream handle. Panics
  on accept error.
- `tcp_connect(Text, Int) -> Int` — dial `host:port` as a client; returns a
  stream handle usable with `tcp_read`/`tcp_write`/`tcp_close`. Panics on
  connect error.
- `tcp_read(Int) -> Bytes` — block, return one chunk (empty `Bytes` at EOF).
  Panics on I/O error.
- `tcp_write(Int, Bytes) -> Unit` — write all + flush. Panics on I/O error.
- `tcp_close(Int) -> Unit` — drop the listener/stream. Panics on unknown handle.

The TCP socket fns (BRIDGE_TCP_SOCKET_V1, `net.rs`) are synchronous blocking
`fullIo` leaves — they do NOT use the `channels.rs` async layer. `tcp_listen`
returns its `(handle, port)` pair as a `Value::Tuple` reusing the existing
`Value` type + `tuple_field` precedent, not a new registry `type`.

Use `fs_file_exists(Text) -> Bool` for existence checks rather than probing with
a read-and-catch pattern. The `ResultText` / `ResultUnit` / `ResultBytes` sum
types no longer exist — never introduce a new fn that returns them.

`ValueList` is the homogeneous list-of-Value data type
(`sha256([0x01, 0x03, value_type_hash])` per Core IR 0.5 — `PrimCode::Value=6`).
It is **data-only**: every element is a `Value`.

`Fn` is the foreign-fn reference type (`sha256([0x01, 0x00, 8])` per Core IR 0.5
`PrimCode::Fn=8`). It is **callee-position only**: a `Fn` may appear only in the
callee/predicate slot of a higher-order primitive (e.g. `foreach(ValueList, Fn)`).
A `Fn` is NEVER a `Value`, NEVER a list element, NEVER a compound field,
NEVER compared, NEVER returned as data. The emitter resolves a `Fn` pool entry's
identity payload to a bare Rust fn path at translation time. The illegal state
(`Fn` in a data position) is rejected at emit time as a HARD ERROR.

### FORBIDDEN FIELDS — never add these

| Field    | Why forbidden |
|----------|--------------|
| `arity`  | Not a real axreg field. Arity is derived from `in (...)` by counting types. |
| `profile`| Wrong keyword. The correct keyword is `effect`. |

### FORBIDDEN ACTIONS on axreg files

- Never remove or modify the `identity` field of any entry.
- Never add fields not in the list above.
- Never use type names outside the valid list.
- Never use `profile` — use `effect`.
- Never add `arity` — it is not a valid field.

### When adding a new function

1. Add `in (...)`, `out`, `effect` using types from the valid list only.
2. Derive the identity hash:
   - **Leaf bridge fns** (`kind leaf`): `identity = sha256(utf8_name_bytes)` of
     the function name string. This matches `bridge_builtin_map()` in
     `src/emit/rust_05.rs` and every existing entry in
     `axis-codegen-bridge.axreg` (verified: `content_hash`, `hash256_parse`,
     `int_add`, `str_len`, …).
   - **Composite fns** (`kind composite`): use
     `registry_compound_id(name, contract)`.
3. Do not invent an identity hash.
4. If the correct type cannot be determined from the Rust source,
   leave the entry without `in`/`out` and report it as a gap.

## `Value::List` clone cost — the O(N²) fold, and what is load-bearing

Recorded by `M1_LIST_FOLD_FINDING_CLOSEOUT_V1` (2026-08-17), closing
`M1_VALUE_ALLOCATION_STRATEGY_BAKEOFF_V1`. Nothing below was changed by
that intent — these are the sites a future change must not break.

### The mechanism

`ref_clone` (`src/emit/rust_05.rs:1314`) emits a `.clone()` at **every**
call site that names a list. `Value::List` is `Vec<Value>` with no
structural sharing (`src/runtime/value.rs:19`), so each of those clones
is O(N) in the list length.

An M1 fold written as `loop_count` + channel-peek names the list **twice
per iteration** — hence 2N deep copies, hence O(N²). The clone-count
model predicted a 2× ratio between the two-clone and one-clone probes;
the measurement came in at **1.85× at both 10k and 100k**. That
agreement is what makes the call-site argument clone the single root
cause rather than one contributing factor.

`list_get` is **not** the cause: it indexes directly
(`src/runtime/list.rs:36-41`, `elems[idx].clone()` — O(1) plus one
element clone). An earlier claim that it was O(i) was retracted.

`foreach` (`src/runtime/iter.rs:41-48`) is the remedy and is already
correct: it destructures `Value::List(items)` and moves each element out
of the owned `Vec`, so the list is never named inside the loop body. No
runtime change is needed to get linear behaviour.

### `VALUE_MUST_STAY_SEND_SYNC` is load-bearing (`value.rs:38-41`)

The `assert_send_sync::<Value>()` compile-time gate is not decorative and
not merely satisfied by accident. On the axVerity write path a
**4-element `Value::List` crosses three OS threads per iteration** —
built on `mem_controller`, received on `disk_controller`, received again
on `flusher`, each a separate thread spawned per `--entries` name. It is
also structurally required at `src/runtime/channels.rs:266` and `:68`.

Consequence: swapping the payload for `Rc<Vec<Value>>` to make cloning
cheap is **undefined behaviour here, not merely slower**. Any shared
payload must be atomically refcounted.

### In-place mutation on owned move — deliberate, three sites

- `src/runtime/list.rs:63` — `list_append`, `Value::List(mut elems)`
- `src/runtime/list.rs:74` — `list_concat`, `(Value::List(mut a), ...)`
- `src/runtime/list.rs:85` — `list_reverse`, `Value::List(mut es)`

These take the payload by owned move and mutate it in place, on purpose:
the native call site already handed over an owned clone, so pushing /
extending / reversing directly avoids a second copy. Any future move to
a **shared** payload representation must address all three — writing
through a shared buffer would be observable by other holders of the same
allocation, which is a semantic change, not an optimisation.

### Rejected: the cheap-to-clone payload

A shared, root-owned, never-freed `Value::List` buffer was built and
measured. It removes the quadratic (**1,902× at 100k**, flat to 1M on
unchanged M1 source) but disqualified itself on the shape the write path
actually uses: **+70.1% at 4 elements, +108.2% at 8**, and **250.5 bytes
leaked per list construction**, unbounded in construction count. It has
been reverted out of this tree. Do not re-propose a shared-payload
candidate without addressing the small-list construction regression, the
three mutation sites above, and the `Send + Sync` requirement.

## `int_div` / `int_mod` are EUCLIDEAN — this is shared infrastructure

Changed by `AXVERITY_FORMAT_LAND_AND_WIRE_V1` / P0 (2026-08-18) in
`src/runtime/arith.rs`. **This crate is used by both `axVerity-working`
and `axVerity-working2`**, so the change is stated here as well as in the
consuming repo.

`int_div` was `x / y` and `int_mod` was `x % y`, which truncate toward
zero and return a **negative remainder** for a negative left operand.
They are now `x.div_euclid(y)` and `x.rem_euclid(y)`: a remainder is
always in `[0, |y|)`.

**Why.** M1 has no byte-width load. `mem_read_int_raw` is an 8-byte
**signed** read, so a decoder at a record boundary pulls in the following
seven bytes, and any of them setting the top bit makes the word negative.
`int_mod(v, 256)` then returned a negative "byte", which was added to a
read cursor as a length and drove it below zero —
`mem_read_int_raw: offset must be >= 0, got -83`. Fixing it at the call
site would have left the hazard in place for the next decoder.

**Both, not just the remainder.** For a power-of-two divisor, Euclidean
division *is* an arithmetic shift and Euclidean remainder *is* a bit
mask, so `int_mod(int_div(v, 256^k), 256)` yields byte `k` of `v`'s
two's-complement representation for every `k`. Leaving `int_div`
truncating would make every byte above the lowest silently wrong on a
negative word.

**Compatibility.** Euclidean and truncated agree exactly on non-negative
operands. Every pre-existing `int_div`/`int_mod` call site in both M1
trees was checked and takes a non-negative left operand, so no existing
caller changed behaviour. `tests/unit_runtime.rs` carries the semantics:
`test_int_mod_negative_dividend` (now `-7 mod 3 == 2`, with the
`x == div*y + mod` identity asserted alongside),
`test_int_mod_byte_extraction_is_unsigned`, and
`test_int_div_negative_is_arithmetic_shift`.

If you need C-style truncated remainder, it is **not** available as a
primitive and should not be added without a measured caller that needs
it — the decoding hazard above is the reason.

## Named-handle modules borrow their lookups — shared infrastructure

Landed by `AXVERITY_BRIDGE_GLUE_OPT_SWEEP_V1` (2026-08-20) in
`scratch.rs`, `nameptr.rs`, `adjacency.rs`. **The write path links these**,
so the rules are stated here as well as in the consuming repos.

### Index with `&*name`, never `name.to_string()`

Every named-handle entry point indexes its collection with an `&str`
reborrow of the caller's `Arc<str>` (`String: Borrow<str>`). A `String`
is allocated **only** where one is genuinely stored — a key being kept
for the first time. `u32v.rs` established the pattern; the rest now
follow it. It is the house pattern, not one module's exception.

### Map values and slots are `Arc<str>`, and that is load-bearing

`scratch.rs`'s `MAPS` and `nameptr.rs`'s `ToggleCell.slots` hold
`Arc<str>`, not `String`, because a `Value::Str` **already is** an
`Arc<str>` (`value.rs:152-154` — `intern_str` is `Arc::from`; despite
the name there is no interning). Storing the caller's `Arc` and handing
it back are both refcount bumps, so a put/get round trip allocates
nothing.

**The safety argument is structural and must be re-checked if it ever
stops holding:** sharing the buffer is unobservable only because
`Arc<str>` is immutable and **no `Arc::get_mut` or `Arc::make_mut`
exists anywhere in this crate**. Introducing one would make these stores
aliasing-visible. `Arc<str>` is `Send + Sync`, so
`VALUE_MUST_STAY_SEND_SYNC` (`value.rs:38-41`) is preserved.

### `set_clear` / `map_clear` still copy, deliberately

They index via `HashMap::remove`, which no measurement covers. A
borrowed form compiles and is a pure borrow with no insert path — it was
reported and **not taken**. Do not "finish the job" without a
measurement.

### Measurement discipline for this crate

**Allocation counts are primary evidence; ns/op corroborates.** The
unchanged `map_put` fresh path measured 476.66 ns and 568.30 ns in two
sessions — **~19% apart on identical code**. A cross-session ns
comparison in this repo is not a comparison. Use a back-to-back run with
both implementations spliced against the same benches; the bench headers
in `scratch.rs` and `u32v.rs` say so.

Measured result, per-call allocations, before → after: `map_get`
hit 4→0, miss 3→0; `map_put` overwrite 3→0, fresh 3→1 (the stored key);
`set_has` 2→0; `set_add` already-present 2→0; `nameptr_get` hit 3→0.

End to end on the axVerity read tier at 1M triples: rebuild
**34,562 → 24,708 ms (−28.5%)**, footprint **2,423 → 2,121 B/triple
(−12.5%, 288 MiB resident)**. Attribution was separated with a third
build: the allocation work owns **100%** of the footprint gain and ~24%
of the time gain; `-C opt-level=3` on the generated glue owns ~76% of the
time gain and **zero** of the footprint gain.

## M1-generated glue compiles at `-C opt-level=3`

The three `rustc` invocations in `src/main.rs` that compile M1-generated
glue — provider rlib, bundle rlib, and shim+final-link — carry
`-C opt-level=3`. They previously defaulted to **opt-level 0**, which
meant every published M1 timing measured the build configuration rather
than the architecture.

Cost: glue build time **+40% (write path) / +42% (readtier)**, almost
entirely the rlib stage. It repays inside a single 1M readtier rebuild.
Small bundles are unaffected — `cli_build_05_test` is unchanged, because
its time is rustc startup and linking, not optimisation.

`-C embed-bitcode=no` is on all three sites. On the two rlib sites it
earns its place. On the shim+link site it does **nothing** — that site
emits an executable, which has no downstream consumer to LTO against, so
no bitcode is embedded either way (verified: byte-identical artefacts).
It is kept for consistency. **Do not test this flag by looking for a
rustc diagnostic** — it is a size-and-time flag and is silent when
absent.

## ORPHAN_IS_TOP_LEVEL_V1 — orphan scope is an edge, never a position

Landed 2026-09-13 in `src/emit/rust_05.rs`, `src/core_ir_05/mod.rs`, and
(doc only) `axis-lang-lab-working/src/lowering/nf_lowering.rs`. **Both
axVerity trees link this crate**, so the rule is stated here too.

### The rule

A node with **no consumer edge** — not `result`, not a `CCall` arg, not a
`CIf`'s cond/then_/else_ — is **unconditional and top-level**: evaluated
once, on every call, in node-index order. A node's index position
relative to a `CIf` says nothing about branch membership.

This is a **producer contract**. A producer that wants a discarded effect
gated by a branch MUST give it a consumer edge inside that arm, by
threading it through the arm's result with `seq(eff, result) -> result`.
M1/AI3 does this in `nf_lowering.rs` `seq_scope_arm_effects`, called on
both arms at `:495`/`:498`; `branch_scoping_tests` (5 tests, nested `if`
included) is what holds branch effect scoping up. There is no emitter-side
fallback and there cannot be one — see below.

### The removed tripwire, and why it must not come back

`compute_branch_paths` used to refuse to build when an orphan sat in the
index window `(cond_anchor+1)..k` of a `CIf` whose `then_` was a bare pool
ref. Its doc comment claimed the shape was "not reachable from M1 today".
**It was reachable from ordinary AI3**, by writing a top-level discarded
effect after the condition's binding:

```
let c = str_eq(Text("p"), Text("p"))
let a = fs_write_text(Text("/tmp/x"), Text("1"))   // top-level, not in an arm
let v = if c { Text("y") } else { Text("n") }
```

Two independent defects, both measured before removal:

- **False positives.** Moving the `let a` above the `let c`, or making the
  then-arm a call instead of a literal, compiled the identical program.
  It was testing positional adjacency, not branch membership.
- **False negatives.** Only `then_` was tested for being a bare pool ref.
  `then_` pool + `else_` node was rejected; `then_` node + `else_` pool was
  accepted — same ambiguity, opposite verdicts.

It can't be repaired by narrowing. An arm-local orphan and a top-level
orphan lower to **byte-identical** node sequences (cond, orphan, `CIf`),
so no positional or structural signal separates them. Reachability is
exactly the information an orphan lacks. Do not reintroduce a positional
form of this check — the guarantee belongs in the producer, where the arm
structure still exists.

The check only ever *rejected*; it never altered codegen. Removing it is
strictly build-permitting: every bundle that built before builds
identically after, and only the reject set shrinks.

`tests/cli_build_05_test.rs` carries the replacement —
`test_orphan_before_unanchored_cif_runs_unconditionally_{then,else}_branch`
build the once-refused shape end to end and assert the orphan effect fires
exactly once with either arm taken. The real branch-scoping tests
(`test_cif_then_taken_does_not_run_else_side_effect` and siblings) are
unchanged and still pass.

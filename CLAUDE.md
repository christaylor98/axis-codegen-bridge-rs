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

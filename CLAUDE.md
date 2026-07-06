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

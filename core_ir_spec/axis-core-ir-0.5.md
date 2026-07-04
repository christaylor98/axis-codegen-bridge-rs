# Core IR 0.5 — Working Draft

**Status:** Working draft — unproven against a full implementation. The versioning
ceremony of earlier versions ("CANONICAL", "MUST conform") is withheld until the
model survives contact with a build. Supersedes Core IR 0.4.

---

## Overview

Core IR 0.5 is a radical simplification of Core IR 0.4. The node set shrinks
from nine to three. The recursive tree structure becomes a flat indexed table.
Lambda abstraction and variable binding are removed from the canonical form
entirely. Every constant is a pool entry, not a node.

The result is a format where every edge is an integer index, cycles are
unrepresentable by construction, and the full program graph is a single
forward pass over an array. Complexity that belonged to the surface
(lambda, let, variable names) is pushed to the registry layer or to the
lowering phase, where it belongs.

---

## Summary of Changes from 0.4 → 0.5

### Node set: 9 → 3

| Removed | Reason |
|---|---|
| `CIntLit`, `CBoolLit`, `CUnitLit` | Constants are pool entries `(def_hash, bytes)`, not nodes |
| `CLam`, `CApp` | All abstraction is registry-resident; no inline lambda at this layer |
| `CLet`, `CVar` | No named binding; sharing is structural (two edges to same index) |

| Remaining | Role |
|---|---|
| `CCall` | Every call to a named function — the only node with a side effect |
| `CIf` | A semantic fork into two mutually exclusive meanings |
| `CDeterminate` | A structural domination gate for irreversibility checking |

### Structure: recursive tree → flat indexed table

0.4 encoded a `CoreTerm` as a recursive struct. 0.5 encodes a `CoreBundle` as
two flat arrays — `constantPool` and `nodes` — plus a single `result: NodeRef`
naming the bundle's semantic value (see §Bundle Result). Edges are integer
indices (`NodeRef`), not nested structs.

**Topological invariant** (checked by the Verifier): for every `NodeRef.node(i)`
appearing in the argument list of the node at index `j`, `i < j`. This makes
cycles structurally unrepresentable. Deserialisation builds a random-access
array with no pointer chasing.

### CCall: name string added alongside identity hash

0.4 dispatched by `targetName: Text`. 0.5 replaced this with
`targetIdentity: Hash256`. This spec adds `targetName: Text` back alongside the
hash. See §CCall below.

### Bundle header: simplified

The `provenance`, `effectClass`, and `idempotent` header fields from 0.4 are
removed. Bundle identity is the content hash of the canonical encoding — no
declared identity field. `version` is a wire-format transport marker only.

---

## Constant Pool

All literal values live in `CoreBundle.constantPool` as `(defHash, payload)`
entries. There are no literal nodes.

A pool entry's `defHash` is the content hash of the `TypeDef` that describes
the payload's layout, resolved against the active registry scope at validation
time. The payload is the canonical bytes of the value, encoded per the Registry
Format Specification.

**Standard primitives:**

| Type | defHash resolves to | Payload |
|---|---|---|
| `Int` | `TypeDef` for prim int | LEB128 signed integer |
| `Text` | `TypeDef` for prim text | varint length + UTF-8 bytes |
| `Bool` | `TypeDef` for prim bool | `0x00` or `0x01` |
| `Unit` | `TypeDef` for prim unit | zero bytes |
| `Float` | `TypeDef` for prim float | raw UTF-8 decimal string (e.g. `"3.14"`) — never binary float |
| `Decimal` | `TypeDef` for prim dec | raw UTF-8 decimal string (e.g. `"15.60"`) — trailing zeros significant |

**Float and Decimal are always raw strings.** They are never parsed to binary
floating-point in the pool payload. `Decimal(15.60)` and `Decimal(15.6)` are
distinct values — the pool preserves the string as written. The bridge is
responsible for interpreting the string at execution time.

---

## Fn Reference as Argument

When a function is passed as an argument to another function (e.g.
`sort(list, comparator_fn)`), the fn reference is a constant pool entry:

- `defHash` resolves to the `Fn` type in the registry
- `payload` is the 32-byte minted identity hash of the referenced function

No special node type is required. A fn reference is a typed constant. The bridge
resolves the payload hash to a concrete implementation at translation time.

---

## Nodes

### CCall

```
CCall {
  targetIdentity : Hash256   // 256-bit minted identity — authoritative
  args           : NodeRef[] // ordered arguments
  targetName     : Text      // human-readable fn name — mandatory
}
```

**`targetIdentity`** is the authoritative dispatch key. The Verifier, the UNKNOWN
gate, and the bridge all key on this. If `targetIdentity` does not resolve in the
active scope: **UNKNOWN gate — hard halt, never a default, never a guess.**

**Zero identity (all-zero `Hash256`) is also UNKNOWN gate.** Hard halt. No
exceptions.

**`targetName`** is mandatory and must match the name in the registry entry for
`targetIdentity`. It is the primary human-readable identifier. Function names are
unique across all domains — a bundle displayed as text is fully understandable by
reading names alone, without resolving hashes. The compiler emits both; tools and
humans read the name; the bridge dispatches on the hash.

**`args`** are ordered `NodeRef`s. Each refers to either a prior node result (by
topological index) or a pool entry. The def_hash of each argument's value must
match the expected def_hash in the function's frozen genesis signature. Type
mismatch is a hard FAIL.

CCall is the only node the Verifier validates against the registry. It is the
only node that can cause a side effect.

---

### CIf

```
CIf {
  cond : NodeRef   // must resolve to Bool type
  then : NodeRef   // result when condition is true
  else : NodeRef   // result when condition is false
}
```

Declares that the program's meaning diverges into two mutually exclusive paths.
Not a runtime evaluator — a semantic declaration. For static analysis (blast
radius, origin tracing) both branches are treated as reachable.

---

### CDeterminate

```
CDeterminate {
  // no fields
}
```

A pure structural domination gate. No operands, no edges.

**Soundness invariant:** an Irreversible CCall must be dominated by a
CDeterminate on every control-flow path from the bundle entry to that node.
Verification fails if any Irreversible CCall is not dominated.

CDeterminate produces a value of type `Unit` (a discharge token). Surface
lowering does not currently emit CDeterminate — tests construct gated/ungated
bundles directly. Lowering integration is a deferred phase.

---

## Bundle Result

Every `CoreBundle` carries a single `result: NodeRef` — the ref whose value
*is* the bundle's meaning. A consumer executing or interpreting a bundle must
emit/return exactly this ref's value; it must never be inferred positionally.

`result` is unconstrained by kind: it may point at a `CCall`, a `CIf`, a
`CDeterminate` (whose value is `Unit`), or directly at a bare pool entry —
the last of these is the common case whenever the source's tail expression is
a literal or an unadorned variable reference, since neither pushes a node of
its own.

**Why this is a dedicated field and not "the last node in `nodes`":** a bare
literal or variable-reference tail following at least one earlier node (e.g.
`let _ = some_call(...); Bool(false)`) never gets a node slot of its own —
`some_call`'s node remains the last entry in `nodes` even though it isn't
what the program returns. Treating "the last node" as the result silently
returns the wrong value whenever this shape occurs. `result` removes the
ambiguity by recording the answer explicitly, exactly as the producer
computed it — lowering always knows the true tail ref; only the flat table
representation risked losing it.

`result` is subject to the same range check as any other `NodeRef`:
`Pool(i)` requires `i < pool_count`; `Node(i)` requires `i < node_count`. It
is not otherwise ordering-constrained — nothing in the bundle depends on
`result` itself, so it may point at any node or pool entry regardless of
position.

---

## Human Display Format

A bundle displayed for human inspection MUST lead with `targetName` on each
CCall. The hash is secondary — shown as an abbreviated suffix for verification
purposes only. Example:

```
pool[0]  Int        42
pool[1]  Text       "hello"
pool[2]  FnRef      compare_int_lt   0x1a2b3c4d…
node[0]  CCall      int_add          0xabcd1234…   [pool[0], pool[1]]
node[1]  CCall      compare_int_lt   0x1a2b3c4d…   [node[0], pool[0]]
node[2]  CIf        cond=node[1]  then=pool[0]  else=pool[1]
node[3]  CDeterminate
result:  node[2]
```

A bundle where any CCall displays only a hash and no name is non-conforming
display output. Tools that emit such output must be updated.

---

## Topological Invariant

For every `NodeRef.node(i)` inside the argument list (or cond/then/else) of the
node at index `j`: `i < j`.

This invariant:
- Makes cycles unrepresentable by construction
- Makes serialisation a single forward pass
- Makes deserialisation a random-access array build (no pointer chasing)
- Makes blast-radius and origin-trace analysis linear array passes

The Verifier checks this invariant on every bundle. A bundle that violates it
is rejected before any other check.

`result` (see §Bundle Result) is checked the same way, treated as if it were
one more edge trailing the node table: `NodeRef.node(i)` requires
`i < node_count`; `NodeRef.pool(i)` requires `i < pool_count`.

---

## Bundle Identity

The bundle's identity is the content hash of its canonical encoding. There is
no declared identity field — it is computed by the consumer. The `version` field
is a wire-format transport marker only; it is not part of the meaning hash.

The canonical encoding includes `constant_pool`, `nodes`, **and** `result` —
two bundles with identical pool/node tables but different `result` refs are
different programs, and must hash differently.

---

## Wire Formats

Three formats exist for a `CoreBundle`. They are semantically equivalent and
fully roundtrippable. The **canonical binary** form is authoritative: bundle
identity is the SHA-256 of its canonical bytes. The other two formats are
transport/interop conveniences.

| Format | Extension | Use |
|---|---|---|
| Canonical binary | `.axbi` | On-disk storage, cross-language embedding, identity hashing |
| JSON | `.axbi.json` | Debugging, tooling, web consumers, human authoring |
| Cap'n Proto | — | **Deprecated.** Kept for backward compatibility only. Do not use in new consumers. |

---

### Primitive Encoding Rules

All binary formats use these building blocks. Decoders MUST reject non-canonical
encodings.

| Primitive | Encoding |
|---|---|
| `varint(n)` | Unsigned LEB128, minimal form. Non-minimal encodings (redundant continuation bytes) are a hard error. |
| `hash256(h)` | 32 raw bytes, big-endian, no length prefix. |
| `bytes(b)` | `varint(len)` followed by `len` raw bytes. |
| `NodeRef` | Single `varint`. Low bit selects space: `node(i)` → `i << 1`; `pool(i)` → `(i << 1) \| 1`. |

---

### Canonical Binary Layout

This is the layout produced and consumed by `serialize_canonical` /
`deserialize_canonical`. It is the ONLY input to the bundle identity hash.
The `version` field is excluded.

```
varint(pool_count)
for each pool entry:
    hash256(def_hash)          // 32 bytes — type identity
    bytes(payload)             // varint(len) + payload bytes

varint(node_count)
for each node:
    varint(kind_tag)           // 0 = CCall, 1 = CIf, 2 = CDeterminate

    // kind_tag = 0 (CCall)
    bytes(target_name_utf8)    // varint(len) + UTF-8 bytes
    hash256(target_identity)   // 32 bytes — function identity
    varint(arg_count)
    NodeRef * arg_count        // each a tagged varint per §Primitive Encoding Rules

    // kind_tag = 1 (CIf)
    NodeRef(cond)
    NodeRef(then)
    NodeRef(else)

    // kind_tag = 2 (CDeterminate)
    // no bytes

NodeRef(result)                // trailing — the bundle's semantic value (§Bundle Result), always present
```

Topological invariant is enforced at decode time: any `NodeRef.node(i)` where
`i >= current_node_index` is a hard error. Any `NodeRef.pool(i)` where
`i >= pool_count` is a hard error. The trailing `NodeRef(result)` is checked
against the fully-decoded table: `node(i)` requires `i < node_count`;
`pool(i)` requires `i < pool_count`.

---

### Axial Binary File Format (.axbi)

The `.axbi` file wraps the canonical payload with a 6-byte header for
format identification. The header is NOT included in the identity hash.

```
offset  size  value
0       4     magic: 0x41 0x58 0x43 0x49  ('A','X','C','I')
4       1     ir_major: 0x00
5       1     ir_minor: 0x05
6       *     canonical payload (serialize_canonical output)
```

A reader MUST reject files where magic ≠ `AXCI`. A reader SHOULD reject
files where `(ir_major, ir_minor) ≠ (0x00, 0x05)` unless it explicitly
supports that version.

The canonical payload starting at offset 6 is byte-identical to the output of
`serialize_canonical`. Identity is `SHA-256(bytes[6..])`.

---

### JSON Format (.axbi.json)

JSON is the human-readable, debug-friendly equivalent of `.axbi`. Semantics
are identical; only the encoding differs.

**Top-level object:**

```json
{
  "axis_core_ir": "0.5",
  "constant_pool": [ ... ],
  "nodes": [ ... ],
  "result": { "space": "node", "index": 0 }
}
```

**Pool entry:**

```json
{
  "def_hash": "a3f9...64 lowercase hex chars...c2b1",
  "payload":  "base64-encoded payload bytes (RFC 4648 §4, with = padding)"
}
```

**NodeRef** (used wherever a `NodeRef` appears in a node):

```json
{ "space": "node", "index": 0 }
{ "space": "pool", "index": 2 }
```

**Node — CCall:**

```json
{
  "kind":            "CCall",
  "target_name":     "int_add",
  "target_identity": "abcd...64 lowercase hex chars...1234",
  "args": [
    { "space": "pool", "index": 0 },
    { "space": "node", "index": 0 }
  ]
}
```

**Node — CIf:**

```json
{
  "kind": "CIf",
  "cond": { "space": "node", "index": 1 },
  "then": { "space": "pool", "index": 0 },
  "else": { "space": "pool", "index": 1 }
}
```

**Node — CDeterminate:**

```json
{ "kind": "CDeterminate" }
```

**Encoding rules for JSON:**

- Hashes are 64-character lowercase hex strings. No `0x` prefix.
- Payload bytes are standard base64 with `=` padding (RFC 4648 §4). Empty payloads encode as `""`.
- Node and pool arrays are ordered; index in the array IS the `NodeRef` index.
- All fields shown above are mandatory. Unknown fields MUST be ignored by readers.
- Topological invariant applies identically: `node.index` must be less than the
  position of the referencing node in the `nodes` array.
- `result` is mandatory and uses the same `NodeRef` shape as everywhere else
  (see §Bundle Result); a reader missing this key MUST reject the document.

**Computing identity from JSON:** parse to in-memory `CoreBundle`, then
`SHA-256(serialize_canonical(bundle))`. Do not hash the JSON bytes directly.

---

### Cap'n Proto (Deprecated)

Cap'n Proto framing (`encode_capnp` / `decode_capnp`) was the original
transport format and remains supported for backward compatibility. It is
**not** used for identity hashing — identity is always computed from the
canonical binary form, never from capnp bytes.

`result` was added here too (`CoreBundle.result @3 :NodeRef`) for schema
completeness, even though this format is deprecated — a capnp-encoded bundle
predating this field decodes with a schema-default (incorrect) `result` and
will silently mis-hash; regenerate any such bundle rather than relying on it.

New consumers MUST NOT adopt capnp as their interchange format. Use `.axbi`
(canonical binary) for performance-sensitive or embedded targets, or `.axbi.json`
for interop and tooling. Capnp support will be removed in a future version
once all existing consumers have migrated.

---

## Relationship to the Registry

Core IR bundles carry `targetIdentity` hashes. The active registry scope
provides the definitions those hashes resolve to. A bundle is therefore
incomplete without a scope — it declares meanings but the registry provides
the contracts and implementations those meanings are bound to.

Resolution failure on any `targetIdentity` is a hard UNKNOWN gate — the bundle
is not executable in that scope. There is no partial execution on unknown
identities.

---

## Relationship to Surface Languages

Surface languages (H1, M1, etc.) lower to Core IR through a spec-driven
pipeline. The pipeline resolves all names to identity hashes and all abstractions
to the flat node table before Core IR is emitted. By the time a bundle is
produced:

- All variable names are resolved to NodeRef indices
- All lambda abstractions are lifted to the registry
- All literals are pool entries
- All fn references are pool entries carrying identity hashes
- Only CCall, CIf, CDeterminate remain

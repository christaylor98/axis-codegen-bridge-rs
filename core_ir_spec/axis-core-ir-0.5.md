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
two flat arrays: `constantPool` and `nodes`. Edges are integer indices
(`NodeRef`), not nested structs.

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

---

## Bundle Identity

The bundle's identity is the content hash of its canonical encoding. There is
no declared identity field — it is computed by the consumer. The `version` field
is a wire-format transport marker only; it is not part of the meaning hash.

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

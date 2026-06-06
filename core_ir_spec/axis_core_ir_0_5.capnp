@0xa4e7f1b8c93d0521;

# ============================================================================
# Axis Core IR 0.5 — Canonical Binary Schema
#
# Status: WORKING DRAFT — not battle-tested against an implementation.
# Subject to revision. The versioning ceremony in 0.4 ("CANONICAL", MUST
# conform) is withheld until the model survives contact with a build.
#
# This schema replaces Core IR 0.4. Read the change block before using.
# ============================================================================


# ----------------------------------------------------------------------------
# CHANGE BLOCK — Core IR 0.4 → Working Draft
# ----------------------------------------------------------------------------
#
# BUNDLE HEADER — all three 0.4 additions are removed:
#
#   provenance   removed → relocated to the Ledger as an admission-event
#                          fact. The producer asserts it as a transit input
#                          to the admission request; it is captured there
#                          and never stored on the artifact.
#
#   effectClass  removed → derived on demand from the bundle's CCall
#                          targets against their registry effect profiles.
#                          The graph is authoritative; any stored value is
#                          a non-authoritative cache. Not declared.
#
#   idempotent   removed → relocated to the Registry as a bridge-leaf fact,
#                          attached to the effect it describes. It is a
#                          property of the bridge's mechanism, not of the
#                          bundle.
#
# ANNOTATIONS — removed from the canonical form entirely.
#   Annotations are external to the canonical bundle. They live in a
#   side channel keyed on node identity, managed by tooling, and stripped
#   at archival. They cannot be consumed by the compiler, the Verifier, or
#   any bridge, because the canonical bundle does not contain them.
#   A toolchain MAY carry them in a working representation; the canonical
#   binary does not.
#
# NODE SET — reduced to exactly two kinds:
#
#   Removed: CIntLit, CBoolLit, CUnitLit
#     Constants are constant pool entries, not nodes. A small value is a
#     (def_hash, bytes) entry in CoreBundle.constantPool, referenced by
#     pool index via NodeRef. No out-of-bundle interning required.
#
#   Removed: CLam, CApp
#     All abstraction is registry-resident. Higher-order surfaces lower to
#     Core IR through closure-conversion and lambda-lifting before the
#     compiler sees the bundle. No inline abstraction exists at this layer.
#
#   Removed: CLet, CVar
#     Binding machinery was tree-tax. In the flat node table, sharing is
#     expressed natively: any two nodes that reference the same dependency
#     carry the same NodeRef index. No explicit naming of intermediate
#     values is required or permitted.
#
#   Remaining: CCall, CIf (exactly two).
#
# BUNDLE STRUCTURE — changed from recursive tree to flat indexed table:
#
#   0.4: CoreBundle.coreTerm is a recursive CoreTerm tree.
#   WD:  CoreBundle has two sections:
#          constantPool : List(ConstantPoolEntry)
#          nodes        : List(Node)            # topological order
#
#   Nodes are emitted in topological order: for every NodeRef.node(i) in
#   the argument list of node at index j, i < j. Edges are integer indices
#   (NodeRef), not nested structs. Cycles are therefore unrepresentable.
#
#   Analysis properties of the flat table:
#     - Sharing is free: two nodes referencing the same value carry the
#       same index. No explicit sharing annotation.
#     - Serialisation is a single forward pass.
#     - Deserialisation builds a random-access array; no pointer chasing.
#     - Blast radius: scan forward for nodes referencing index i. Linear.
#     - Origin trace: read argument indices; recurse to smaller indices.
#     - Cycles: impossible by construction. No cycle detection needed.
#     - Debugging: print top-to-bottom; every dep appears before its use.
#
# CCALL — changed:
#
#   0.4: targetName @0 :Text   (dispatch by name)
#   WD:  targetIdentity @0 :Hash256  (dispatch by registry identity token)
#
#   Names are a projection surface. The canonical form carries the minted
#   identity token. A scope that presents duplicate names to the compiler
#   is invalid (see Registry working draft, Resolution section). The
#   compiler resolves name → identity before lowering; Core IR never
#   carries names.
#
#   0.4: args @1 :List(CoreTerm)  (nested recursive terms)
#   WD:  args @1 :List(NodeRef)   (indices into pool or node table)
#
# CIF — changed:
#
#   0.4: cond/then/else are nested CoreTerms.
#   WD:  cond/then/else are NodeRefs (indices into the flat table).
#
# BUNDLE IDENTITY:
#   The bundle's identity is the content hash of its canonical encoding.
#   There is no declared identity field; it is computed by the consumer.
#   The version field is a wire-format transport marker only — not a
#   semantic field, not part of the meaning hash.
#
# ----------------------------------------------------------------------------


# ============================================================================
# Hash256 — a 256-bit content hash or identity token.
#
# Used for:
#   - Registry function identity tokens (minted, opaque, CCall.targetIdentity)
#   - Type definition hashes (def_hash, ConstantPoolEntry.defHash)
#   - axStorage content hashes (large value references in pool payloads)
#
# All three uses share one width so the fabric runs one hash regime.
# ============================================================================

struct Hash256 {
  w0 @0 :UInt64;
  w1 @1 :UInt64;
  w2 @2 :UInt64;
  w3 @3 :UInt64;
}


# ============================================================================
# NodeRef — a reference to either a node in the node table or a pool entry.
#
# All edges in the graph are expressed as NodeRefs. The two index spaces
# (node table and constant pool) are both 0-based and disjoint; the union
# tag distinguishes them. A NodeRef in a CCall argument means "this value
# flows into this argument position."
# ============================================================================

struct NodeRef {
  union {
    # Index into CoreBundle.nodes. Topological invariant: this index is
    # always less than the index of the node that references it.
    node @0 :UInt32;

    # Index into CoreBundle.constantPool. Pool entries are fully local to
    # this bundle; no external resolution is needed beyond validating the
    # entry's defHash against the active scope.
    pool @1 :UInt32;
  }
}


# ============================================================================
# ConstantPoolEntry — an inline literal value, local to this bundle.
#
# Covers: Int, Bool, Unit, short Text, small records. Not for large values
# (arrays, blobs) — those use axStorage and appear as pool entries whose
# payload is a content hash reference, not the bytes themselves.
# ============================================================================

struct ConstantPoolEntry {
  # The type identity of this value: the content hash of its TypeDef in
  # the registry. Resolved against the active scope at validation time.
  # Not stored in the payload — it is the key used to locate the schema
  # that describes the payload's layout.
  defHash @0 :Hash256;

  # The canonical payload bytes of the value. Encoded per the Registry
  # Format Specification: positional, schema-directed, no embedded names,
  # no type tags. The schema is retrieved by resolving defHash.
  payload @1 :Data;
}


# ============================================================================
# CoreBundle — the canonical binary form of a Core IR program.
#
# Two sections only. No header. No annotations. No nested terms.
# Identity of this bundle = content hash of its canonical encoding.
# ============================================================================

struct CoreBundle {
  # Wire format version marker. Identifies which decoder to use.
  # Not a semantic field; not part of the bundle's meaning hash.
  # Consumers read this field first, then discard it.
  version      @0 :Text;

  # Inline constant values for this bundle.
  # Referenced by NodeRef.pool(i). Local; no external lookup required
  # beyond validating defHash against the active scope.
  constantPool @1 :List(ConstantPoolEntry);

  # The program graph, flat, in topological order.
  # Topological invariant (MUST hold, Verifier checks):
  #   For every NodeRef.node(i) appearing in node at index j: i < j.
  # This makes cycles unrepresentable and all analysis linear.
  nodes        @2 :List(Node);
}


# ============================================================================
# Node — exactly two kinds. The complete node set of Core IR.
# ============================================================================

struct Node {
  union {
    cCall @0 :CCall;
    cIf   @1 :CIf;
  }
}


# ============================================================================
# CCall — a named meaning with an effect.
#
# Every potential effect point in the program is a CCall. The only node
# the Verifier validates against the registry. The only node that can
# cause a side effect.
# ============================================================================

struct CCall {
  # The registry identity of the function being called.
  # A 256-bit minted identity token. Dispatch, resolution, and the UNKNOWN
  # gate all key on this — not on a name.
  #
  # If this identity does not resolve in the active scope: UNKNOWN gate.
  # Hard halt. Never a default. Never a guess.
  #
  # The compiler resolves name → identity before emitting this field.
  # Names are never present in the canonical form.
  targetIdentity @0 :Hash256;

  # Ordered argument values. Each is a NodeRef to either a pool entry
  # (a constant) or a prior node result (by topological index).
  #
  # Validation: the def_hash of each argument's value MUST match the
  # expected def_hash in the function's frozen signature (genesis clause).
  # Mismatch is a hard FAIL — this is the type-check the arity-only
  # registry could not perform.
  args           @1 :List(NodeRef);
}


# ============================================================================
# CIf — a fork in meaning.
#
# Declares that the program's meaning diverges here into two mutually
# exclusive paths. Not a runtime evaluator — a semantic declaration.
#
# For static analysis (blast radius, origin tracing): both branches are
# treated as reachable. Conservative; correct for risk assessment.
# ============================================================================

struct CIf {
  # The condition value. Expected to resolve to a Bool type.
  cond @0 :NodeRef;

  # The result value when the condition is true.
  then @1 :NodeRef;

  # The result value when the condition is false.
  else @2 :NodeRef;
}

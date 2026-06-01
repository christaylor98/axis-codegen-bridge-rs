# Multi-Export Bundle Migration

## Overview

A single Core IR bundle can now contain multiple named exports. Each export compiles
to a separate `#[no_mangle] pub extern "C"` symbol in the output `.a`.

The change is **fully backward compatible**: all existing single-export bundles load
and compile identically to before.

---

## Schema change (Core IR 0.4)

`axis_core_ir_0_4.capnp` gains:

```capnp
struct Export {
  name      @0 :Text;
  term      @1 :CoreTerm;
  effectSig @2 :Text;   # "pure" | "reads" | "writes" | "full_io"
}

struct CoreBundle {
  ...
  exports @8 :List(Export);   # NEW — authoritative if non-empty
}
```

**Authoritative rule:** if `exports` is non-empty, use it. If empty, fall back to
`entrypointName + coreTerm` (the original single-export model).

---

## Producing multi-export bundles

### Programmatically (Rust)

Use `create_core_bundle_multi` from `axis_codegen_bridge::core_ir`:

```rust
use axis_codegen_bridge::core_ir::{CoreTerm, Provenance, create_core_bundle_multi};
use std::rc::Rc;

let add = CoreTerm::Lam("x".into(), Rc::new(CoreTerm::IntLit(42, None)), None);
let mul = CoreTerm::Lam("x".into(), Rc::new(CoreTerm::IntLit(99, None)), None);

let bytes = create_core_bundle_multi(
    &[("add", &add, "pure"), ("mul", &mul, "pure")],
    Provenance::Mechanical,
    true,   // idempotent
);
std::fs::write("mylib.coreir", bytes).unwrap();
```

### Via `ir_write_bundle` runtime function (from Axis code)

Multi-export form — first argument is `List(Tuple([name, term, effectSig]))`:

```
ir_write_bundle(
  List([
    Tuple(["add", add_term, "pure"]),
    Tuple(["mul", mul_term, "pure"]),
  ]),
  "mylib.coreir",
  True
)
```

Single-export form is unchanged:

```
ir_write_bundle(Tuple([term, "out.coreir", "pure", True]))
```

---

## Building multi-export bundles

The `build` subcommand is unchanged. Feed it a multi-export bundle the same way:

```
axis-codegen-bridge build mylib.coreir --out mylib
```

The output `libmylib.a` will contain one symbol per export.

**`--exe` with a multi-export bundle** calls the export named `main` if present,
otherwise the first export.

---

## The `bundle` subcommand (Path A)

For grouping already-compiled single-export `.a` files into one archive, the
`bundle` subcommand remains available:

```
axis-codegen-bridge bundle --out combined.a a.a b.a c.a
```

This merges object files via `ar` and is the alternative to writing a multi-export
bundle from a single Core IR file.

---

## Future work: multi-export from AI2 source

Multi-export bundles are currently produced **programmatically** or via
`ir_write_bundle`. Syntax for declaring multiple top-level exports directly in AI2
source (module-level definitions) is a separate future task and is not included here.

---

## Field index notes

- `exports` is at field index `@8` in the 0.4 schema.
- Indices `@0`–`@7` are existing fields; do not reuse them in future extensions.
- The `Export` struct uses `@0` (name), `@1` (term), `@2` (effectSig).

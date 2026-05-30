# axis-codegen-bridge-rs

Rust foreign function bridge for the axis-lang code generation pipeline.

Provides the runtime layer that axis-lang programs call into for IO, arithmetic,
string operations, list operations, and process interaction. All logic lives in
axis-lang; this bridge is the thin Rust layer below it.

## What this is

- The only Rust in the axis-lang code generation stack
- Foreign function implementations for things axis-lang cannot do itself
- Core IR loader and inspector (Cap'n Proto binary format)
- Rust code emitter (Core IR → Rust source for rustc compilation)

## What this is not

- Not an interpreter or VM
- Not project-specific (axLens, axPlanner etc add their own local bridges)
- Not a place for business logic — that lives in axis-lang

## Build

```sh
cargo build --release
```

Requires: Rust stable, Cap'n Proto compiler (`capnp`)

```sh
# Ubuntu / Debian
apt install capnproto

# macOS
brew install capnp
```

## Use

```sh
# Build a native binary from a Core IR bundle
axis-codegen-bridge build program.coreir --out ./program

# Inspect a Core IR bundle
axis-codegen-bridge inspect program.coreir
```

## Registry

`registry/axis-codegen-bridge.axreg` is the canonical declaration of all foreign
functions this bridge provides. axis-lang programs that call these functions must
reference this registry at compile time.

## Adding foreign functions

1. Implement the function in the appropriate `src/runtime/` module
2. Add the Rust path to the symbol map in `src/emit/rust.rs`
3. Add the declaration to `registry/axis-codegen-bridge.axreg`
4. `cargo build`

Growth is by pull — add functions when axis-lang programs need them, not speculatively.

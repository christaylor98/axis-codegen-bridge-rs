# Registry Audit — axis-codegen-bridge-rs

Audit date: 2026-06-01
Dispatch table: `src/emit/rust.rs:11` (`symbol_map()`)
Registry: `registry/axis.axreg`

---

## Summary

**98 clean, 0 ghost, 4 dark** (all dark entries resolved)

Two bugs were also found and fixed during the audit, neither of which fit cleanly into ghost/dark:

1. **`str_eq`** — in symbol_map, had 4 integration tests, but no Rust function existed.
   Action: implemented `str_ops::str_eq` and added to registry.
2. **`argv_or`** — in registry and symbol_map, but implementation ignored the fallback argument
   when the index was out of range. Tests passed `Tuple(idx, default)` but the function
   only read the index. Action: fixed to unpack the tuple and return the default on miss.

---

## Ghost entries (in registry, no implementation)

**None.** All 97 originally declared registry functions were present in the symbol_map with
concrete Rust implementations.

---

## Dark entries (implemented, not in registry)

Four functions existed in Rust and the symbol_map but had no registry entry.

| Function    | symbol_map line | Rust location              | Action       |
|-------------|-----------------|----------------------------|--------------|
| `list_of_1` | 63              | `src/runtime/list.rs:116`  | Added to registry (List section) |
| `list_of_2` | 64              | `src/runtime/list.rs:120`  | Added to registry (List section) |
| `list_of_3` | 65              | `src/runtime/list.rs:127`  | Added to registry (List section) |
| `str_eq`    | 49              | `src/runtime/str_ops.rs` (new) | Implemented + added to registry (String section) |

**Note — `__eq__` alias**: `src/emit/rust.rs:79` maps `__eq__` to `value_eq`. This is an
emit-layer alias for the `==` operator; `value_eq` is already in the registry. No separate
registry entry is needed for `__eq__`.

---

## Clean entries

After remediation, all 101 functions in registry/axis.axreg are backed by concrete Rust
implementations.

| Section      | Count | Notes |
|--------------|-------|-------|
| Core         | 2     | `tuple_field`, `ctor_field` |
| Math/Bool    | 16    | arithmetic + comparison + boolean ops |
| String       | 13    | includes newly implemented `str_eq` |
| List         | 14    | includes newly registered `list_of_1/2/3` |
| Option       | 5     | |
| IO           | 5     | |
| File         | 3     | |
| Process      | 6     | `argv_or` bug also fixed |
| IR           | 20    | constructors + eval + bundle IO |
| Registry     | 9     | |
| Transitions  | 8     | pass-through stubs, intentional |

**Total: 101 declared, 101 implemented, 0 ghost, 0 dark**

---

## Changes made

| File | Change |
|------|--------|
| `registry/axis.axreg` | Added `list_of_1`, `list_of_2`, `list_of_3` (List section); added `str_eq` (String section) |
| `src/runtime/str_ops.rs` | Implemented `str_eq` — compares two `Str` values for equality, returns `Bool` |
| `src/runtime/process.rs` | Fixed `argv_or` — now unpacks `Tuple(idx, default)` and returns the default on out-of-range index |

`cargo build --release` and `cargo test` both pass (98 integration + 8 link tests, 0 failures).

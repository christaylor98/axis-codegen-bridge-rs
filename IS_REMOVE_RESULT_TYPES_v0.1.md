# IS: Remove Result types — plain return types everywhere
#
# Primary: /home/chris/dev/axis-codegen-bridge-rs/
# Also touches: axis-lang-lab-working (m1-registry.axreg)
#               M1-lang-test (caller updates)
#               axRegistry-working + axAI-axlang-gen-working (propagate)
# Mode: execution-allowed true
# Claude Code tier: T2
# Graph node: hld:m1-no-result-types (to be minted by Claude Desktop)

## Decision

Types are resolved at compile time. A type mismatch is a compile error,
not a runtime Result. Bridge functions return plain types. Runtime failures
panic. Pre-conditions own the rest.

## Part 1 — Remove from m1-registry.axreg (axis-lang-lab-working)

Delete or mark superseded:
  type ResultText  = sum [Ok(Text)  | Err(Text)]
  type ResultBytes = sum [Ok(Bytes) | Err(Text)]
  type ResultUnit  = sum [Ok(Unit)  | Err(Text)]

These types should not appear in any fn signature going forward.

## Part 2 — Fix bridge fn signatures (axis-codegen-bridge.axreg + Rust)

| Function            | Old out      | New out  |
|---------------------|--------------|----------|
| fs_read_text        | Value        | Text     |
| fs_read_bytes       | ResultBytes  | Bytes    |
| fs_write_bytes      | ResultUnit   | Unit     |
| fs_write_text       | Value        | Unit     |
| fs_mkdir_p          | ResultUnit   | Unit     |
| bytes_to_text       | ResultText   | Text     |
| fs_append_text      | Value        | Unit     |

Rust: each fn body stops constructing Ctor Ok/Err wrappers and returns
the plain value directly. Failures panic with a clear message.

## Part 3 — Remove unwrap and ctor_is_ok fns

Remove from axreg and dispatch:
  result_text_unwrap
  result_bytes_unwrap
  result_unit_unwrap
  ctor_is_ok          ← no longer needed

## Part 4 — Add fs_file_exists (new)

`has_object` currently uses ctor_is_ok(fs_read_bytes(...)) to probe
existence. With fs_read_bytes panicking on missing file, we need a clean
existence check.

```
fn fs_file_exists
  identity <sha256("fs_file_exists")>
  kind leaf
  in (Text)
  out Bool
  effect fullIo
  deterministic false
  idempotent true
end
```

Rust: `std::path::Path::new(&path).exists()` → `Value::Bool(true/false)`

## Part 5 — Update M1-lang-test callers

Find every .m1 program using result_text_unwrap, result_bytes_unwrap,
result_unit_unwrap, ctor_is_ok — rewrite to use the plain return value
directly. Examples:

```
# Before
let content = result_text_unwrap(fs_read_text(path))

# After
let content = fs_read_text(path)
```

```
# Before (has_object pattern)
let exists = ctor_is_ok(fs_read_bytes(path))

# After
let exists = fs_file_exists(path)
```

Flag any program where the caller does something unusual with the Result
structure — don't silently fix it.

## Expected outcome / tests

```bash
# 1. Build clean
cargo build 2>&1 | grep "^error" | wc -l
# Expected: 0

# 2. All tests pass
cargo test 2>&1 | tail -3
# Expected: 0 failed

# 3. validate.sh
cd /home/chris/dev/axRegistry-working && bash validate.sh
# Expected: all PASS

# 4. Full harness sweep — no regressions
cd /home/chris/dev/M1-lang-test && python3 harness/sweep.py
# Expected: 121/121 RUN_PASS

# 5. Composition test
# result_text_unwrap must not exist — any .m1 calling it should fail
# to compile. Confirm one such program fails at compile time (expected).

# 6. bytes_to_text composition now works cleanly:
# bytes_to_text(text_to_bytes(Text("hello"))) compiles and returns "hello"
# No unwrap needed.
```

## Registry propagation

- /home/chris/dev/axRegistry-working/axis-bridge.axreg
- /home/chris/dev/axAI-axlang-gen-working/registries/axis-bridge.axreg

## CLAUDE.md

Update: "Bridge functions return plain types. No Result wrappers.
Compile-time type mismatch is the error model. Use fs_file_exists for
existence checks."

## Graph update when complete

Hand SHAs to Claude Desktop:
- hld:m1-no-result-types → SATISFIED
- axVerity turn can re-execute with clean signatures

Commits (one per repo):
- axis-codegen-bridge-rs: "fix: plain return types, remove Result wrappers and unwrap fns, add fs_file_exists"
- axis-lang-lab-working:  "fix: remove ResultText/ResultBytes/ResultUnit from registry"
- axRegistry-working + axAI-axlang-gen-working: "registry: plain return type convention"
- M1-lang-test: "fix: update callers — drop unwrap fns, use plain return types"

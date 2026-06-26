# IS: Strict Result type convention — fix fs_read_text and result_text_unwrap
#
# Run from: /home/chris/dev/axis-codegen-bridge-rs/
# Mode: execution-allowed true
# Claude Code tier: T2
# Graph node: hld:m1-result-type-unwrap-convention (LIVE)

## Objective

The bridge registry has two incompatible unwrap conventions. Fix it now
before the registry grows further. Two changes, cascade through callers:

1. fs_read_text(Text) -> Value   becomes   fs_read_text(Text) -> ResultText
2. result_text_unwrap(Value) -> Text   becomes   result_text_unwrap(ResultText) -> Text

After this, bytes_to_text(Bytes)->ResultText composes cleanly with
result_text_unwrap(ResultText)->Text. All Result-producing functions use
strict types. All unwrap functions take their strict type. Convention is set.

## Rule going forward (record in CLAUDE.md)

Every bridge fn that can fail returns a strict Result type:
  ResultText   = sum [Ok(Text)  | Err(Text)]
  ResultBytes  = sum [Ok(Bytes) | Err(Text)]
  ResultUnit   = sum [Ok(Unit)  | Err(Text)]

Every unwrap fn takes its strict Result type, not Value:
  result_text_unwrap(ResultText)   -> Text
  result_bytes_unwrap(ResultBytes) -> Bytes
  result_unit_unwrap(ResultUnit)   -> Unit   ← add this too for completeness

No fn signature uses Value as a Result carrier. Legacy Value-returning fns
(fs_write_text, fs_append_text, fs_list_dir) are separately tracked for
migration; do NOT touch them in this IS.

## Changes

### registry/axis-codegen-bridge.axreg

Supersede fs_read_text — change out type only:
```
fn fs_read_text
  identity <same sha256("fs_read_text") — identity does not change>
  kind leaf
  in (Text)
  out ResultText        ← was: Value
  effect fullIo
  deterministic false
  idempotent true
end
```

Supersede result_text_unwrap — change in type only:
```
fn result_text_unwrap
  identity <same sha256("result_text_unwrap")>
  kind leaf
  in (ResultText)       ← was: Value
  out Text
  effect pure
  deterministic true
  idempotent true
end
```

Add result_unit_unwrap (new):
```
fn result_unit_unwrap
  identity <sha256("result_unit_unwrap")>
  kind leaf
  in (ResultUnit)
  out Unit
  effect pure
  deterministic true
  idempotent true
end
```

### Rust — src/runtime/bytes_io.rs (or existing io.rs — find where fs_read_text lives)

fs_read_text already returns a Ctor internally — just verify the Rust
Value encoding matches Ok(Text)/Err(Text) for ResultText. If it wraps
differently, align it. Likely a one-line comment change and confirm.

result_text_unwrap: change match arm from untyped Value to ResultText Ctor:
```rust
pub fn result_text_unwrap(val: Value) -> Value {
    match val {
        Value::Ctor(tag, inner) if tag == "Ok"  => *inner,
        Value::Ctor(tag, inner) if tag == "Err" => {
            panic!("result_text_unwrap called on Err: {:?}", inner)
        }
        other => panic!("result_text_unwrap: expected ResultText, got {:?}", other),
    }
}
```
(Implementation body is identical to result_bytes_unwrap — same unwrap
pattern, different type name in the registry.)

result_unit_unwrap (add):
```rust
pub fn result_unit_unwrap(val: Value) -> Value {
    match val {
        Value::Ctor(tag, _) if tag == "Ok"  => Value::Unit,
        Value::Ctor(tag, inner) if tag == "Err" => {
            panic!("result_unit_unwrap called on Err: {:?}", inner)
        }
        other => panic!("result_unit_unwrap: expected ResultUnit, got {:?}", other),
    }
}
```

### Dispatch — src/emit/rust_05.rs

result_text_unwrap and result_unit_unwrap wired if not already present.

## Caller updates — M1-lang-test

Find all .m1 programs that call result_text_unwrap or fs_read_text and
recompile. They should type-check cleanly since:
- fs_read_text now returns ResultText
- result_text_unwrap now expects ResultText
- The composition result_text_unwrap(fs_read_text(path)) still works

If any program passes fs_read_text's result to something other than
result_text_unwrap — flag it, don't silently fix it.

## Expected outcome / tests

```bash
# 1. Build clean
cargo build 2>&1 | grep "^error" | wc -l
# Expected: 0

# 2. Cargo test — ALL tests
cargo test 2>&1 | tail -3
# Expected: 0 failed

# 3. validate.sh
cd /home/chris/dev/axRegistry-working && bash validate.sh
# Expected: all PASS (after propagating to both registries)

# 4. Full harness sweep
cd /home/chris/dev/M1-lang-test && python3 harness/sweep.py
# Expected: 121/121 RUN_PASS — no regressions

# 5. Composition test (the whole point)
# In M1: result_text_unwrap(bytes_to_text(text_to_bytes(Text("hello")))) 
# must compile and run, returning "hello"
# Add this as an example or test

# 6. Identity hash spot-check
echo -n "result_text_unwrap" | sha256sum
echo -n "result_unit_unwrap" | sha256sum
# Cross-check against axreg entries
```

## Registry propagation

After axis-codegen-bridge-rs:
- /home/chris/dev/axRegistry-working/axis-bridge.axreg
- /home/chris/dev/axAI-axlang-gen-working/registries/axis-bridge.axreg

## CLAUDE.md

Update to record the strict Result convention as the binding rule.

## Graph update when complete

Hand SHAs to Claude Desktop:
- hld:m1-result-type-unwrap-convention → SATISFIED
- Approve axVerity thin vertical re-execution

One commit per repo:
- axis-codegen-bridge-rs: "fix: strict Result types — fs_read_text, result_text_unwrap, +result_unit_unwrap"
- axRegistry-working + axAI-axlang-gen-working: "registry: strict Result convention"

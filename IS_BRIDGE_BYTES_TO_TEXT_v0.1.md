# IS: Bridge — bytes_to_text
#
# Run from: /home/chris/dev/axis-codegen-bridge-rs/
# Mode: execution-allowed true
# Claude Code tier: T2
# Graph node: hld:m1-bytes-to-text (LIVE, intent:project-axailanggen)

## Objective

Add bytes_to_text(Bytes) -> ResultText to the bridge. General M1 primitive —
not axVerity-specific. Required by axVerity pull_object to decode stored
bytes as text for io_println, but useful for any M1 program that reads
binary content.

## Identity hash

echo -n "bytes_to_text" | sha256sum

## axreg entry (registry/axis-codegen-bridge.axreg)

```
fn bytes_to_text
  identity <sha256("bytes_to_text")>
  kind leaf
  in (Bytes)
  out ResultText
  effect pure
  deterministic true
  idempotent true
end
```

## Rust (src/runtime/bytes_io.rs)

```rust
pub fn bytes_to_text(val: Value) -> Value {
    match val {
        Value::Bytes(b) => match String::from_utf8(b) {
            Ok(s)  => Value::Ctor("Ok".into(),  Box::new(Value::Str(s))),
            Err(e) => Value::Ctor("Err".into(), Box::new(Value::Str(e.to_string()))),
        },
        other => panic!("bytes_to_text: expected Bytes, got {:?}", other),
    }
}
```

## Dispatch (src/emit/rust_05.rs)

```rust
"bytes_to_text" => "crate::runtime::bytes_io::bytes_to_text",
```

## Registry propagation

- /home/chris/dev/axRegistry-working/axis-bridge.axreg
- /home/chris/dev/axAI-axlang-gen-working/registries/axis-bridge.axreg

## Expected outcome / tests

```bash
# 1. Build clean
cargo build 2>&1 | grep "^error" | wc -l
# Expected: 0

# 2. Cargo test
cargo test 2>&1 | tail -3
# Expected: 0 failed

# 3. Identity hash cross-check
echo -n "bytes_to_text" | sha256sum
# Must match axreg identity field exactly

# 4. validate.sh
cd /home/chris/dev/axRegistry-working && bash validate.sh
# Expected: all PASS

# 5. Functional smoke
# In M1: result_text_unwrap(bytes_to_text(text_to_bytes(Text("hello")))) == "hello"
# Add one test to tests/bytes_io_test.rs covering the round-trip
```

## Graph update when complete

Hand SHAs back to Claude Desktop:
- hld:m1-bytes-to-text → SATISFIED
- turn:axverity:0001 BLOCK for io_println lifted

One commit: "feat: bytes_to_text bridge primitive"

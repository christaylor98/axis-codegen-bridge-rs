# IS: Bridge — result_bytes_unwrap, bytes_hash, fs_mkdir_p
#
# Run from: /home/chris/dev/axis-codegen-bridge-rs/
# Mode: execution-allowed true
# Claude Code tier: T2
# Closes: hld:axverity-result-bytes-unwrap, hld:axverity-bytes-hash, hld:axverity-fs-mkdir

## Objective

Add three bridge functions that unblock turn:axverity:0001. All follow the
same pattern as the recently landed text_to_bytes / fs_write_bytes /
fs_read_bytes in src/runtime/bytes_io.rs.

## Identity hashes (§5b — sha256(utf8_name_bytes), DO NOT recompute)

Compute before writing axreg entries:
  echo -n "result_bytes_unwrap" | sha256sum
  echo -n "bytes_hash"          | sha256sum
  echo -n "fs_mkdir_p"          | sha256sum

## Three functions

### 1. result_bytes_unwrap(ResultBytes) -> Bytes

Unwrap Ok(Bytes) → Bytes. Panic on Err. Symmetric with existing
result_text_unwrap.

axreg entry (registry/axis-codegen-bridge.axreg):
```
fn result_bytes_unwrap
  identity <sha256("result_bytes_unwrap")>
  kind leaf
  in (ResultBytes)
  out Bytes
  effect pure
  deterministic true
  idempotent true
end
```

Rust (src/runtime/bytes_io.rs):
```rust
pub fn result_bytes_unwrap(val: Value) -> Value {
    match val {
        Value::Ctor(tag, inner) if tag == "Ok" => *inner,
        Value::Ctor(tag, inner) if tag == "Err" => {
            panic!("result_bytes_unwrap called on Err: {:?}", inner)
        }
        other => panic!("result_bytes_unwrap: expected ResultBytes, got {:?}", other),
    }
}
```

### 2. bytes_hash(Bytes) -> Text

SHA-256 of a Bytes blob, returns "sha256:{64-hex}". Same crypto as
content_hash but accepts Value::Bytes instead of Value::List.

axreg entry:
```
fn bytes_hash
  identity <sha256("bytes_hash")>
  kind leaf
  in (Bytes)
  out Text
  effect pure
  deterministic true
  idempotent true
end
```

Rust (src/runtime/bytes_io.rs):
```rust
pub fn bytes_hash(val: Value) -> Value {
    match val {
        Value::Bytes(b) => {
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(&b);
            Value::Str(format!("sha256:{:064x}", digest))
        }
        other => panic!("bytes_hash: expected Bytes, got {:?}", other),
    }
}
```

### 3. fs_mkdir_p(Text) -> ResultUnit

Recursive idempotent directory create (std::fs::create_dir_all).
Returns Ok(Unit) on success, Err(Text) with OS message on failure.

axreg entry:
```
fn fs_mkdir_p
  identity <sha256("fs_mkdir_p")>
  kind leaf
  in (Text)
  out ResultUnit
  effect fullIo
  deterministic false
  idempotent true
end
```

Rust (src/runtime/bytes_io.rs):
```rust
pub fn fs_mkdir_p(val: Value) -> Value {
    match val {
        Value::Str(path) => match std::fs::create_dir_all(&path) {
            Ok(()) => Value::Ctor("Ok".into(), Box::new(Value::Unit)),
            Err(e)  => Value::Ctor("Err".into(), Box::new(Value::Str(e.to_string()))),
        },
        other => panic!("fs_mkdir_p: expected Text path, got {:?}", other),
    }
}
```

## Dispatch wiring

Add to src/emit/rust_05.rs dispatch map (same pattern as fs_write_bytes):
```rust
"result_bytes_unwrap" => "crate::runtime::bytes_io::result_bytes_unwrap",
"bytes_hash"          => "crate::runtime::bytes_io::bytes_hash",
"fs_mkdir_p"          => "crate::runtime::bytes_io::fs_mkdir_p",
```

## Registry propagation

After adding to registry/axis-codegen-bridge.axreg, also add to:
- /home/chris/dev/axRegistry-working/axis-bridge.axreg
- /home/chris/dev/axAI-axlang-gen-working/registries/axis-bridge.axreg

## CLAUDE.md

Add result_bytes_unwrap, bytes_hash, fs_mkdir_p to the valid bridge fn
list in CLAUDE.md.

## Expected outcome / tests

```bash
# 1. Build clean
cargo build 2>&1 | grep -E "error|warning" | head -20
# Expected: zero errors

# 2. Cargo test
cargo test 2>&1 | tail -5
# Expected: test result: ok. N passed; 0 failed

# 3. Identity hashes match registry entries
python3 -c "
import hashlib
for name in ['result_bytes_unwrap', 'bytes_hash', 'fs_mkdir_p']:
    h = hashlib.sha256(name.encode()).hexdigest()
    print(f'{name}: {h}')
"
# Cross-check each against the axreg identity field

# 4. validate.sh
cd /home/chris/dev/axRegistry-working && bash validate.sh
# Expected: all axreg PASS

# 5. M1 round-trip (once axVerity IS spec v0.3 executes)
# bytes_hash("hello bytes") must return sha256:{64-hex}
# result_bytes_unwrap(Ok(bytes)) must return the bytes
# fs_mkdir_p("/tmp/axv_test_dir") must return Ok(Unit)
```

## Graph update when complete

Hand back to Claude Desktop. Graph updates:
- hld:axverity-result-bytes-unwrap → LIVE (satisfied)
- hld:axverity-bytes-hash → LIVE (satisfied)
- hld:axverity-fs-mkdir → LIVE (satisfied)
- turn:axverity:0001 BLOCK verdicts lifted → Approve live

One commit: "feat: result_bytes_unwrap, bytes_hash, fs_mkdir_p bridge fns"

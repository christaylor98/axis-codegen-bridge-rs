# IS: Remove remaining Result types — hash256_parse and ir_write_bundle
#
# Primary: /home/chris/dev/axis-codegen-bridge-rs/
# Also: axRegistry-working, axAI-axlang-gen-working
# Mode: execution-allowed true
# Claude Code tier: T2

## Objective

Two bridge fns still carry Result return types:
  hash256_parse(Text) -> ResultText
  ir_write_bundle(Value, Text) -> ResultUnit

Same rule applies: types resolved at compile time, failures panic,
pre-conditions own the rest. Remove the wrappers.

## Changes

### registry/axis-codegen-bridge.axreg

hash256_parse: out ResultText → out Text
ir_write_bundle: out ResultUnit → out Unit

### Rust

hash256_parse: return Value::Str(...) directly on success, panic on
invalid format with a clear message ("hash256_parse: invalid input: {}")

ir_write_bundle: return Value::Unit directly on success, panic on IO
error with the OS message.

Remove the Ok/Err Ctor wrappers from both function bodies.

Also remove ResultText and ResultUnit type declarations from the registry
entirely — no remaining fns use them.

## Expected outcome / tests

```bash
# 1. Build + test
cargo build 2>&1 | grep "^error" | wc -l  # Expected: 0
cargo test 2>&1 | tail -3                  # Expected: 0 failed

# 2. validate.sh (after propagating to both registry repos)
cd /home/chris/dev/axRegistry-working && bash validate.sh
# Expected: all PASS

# 3. Harness sweep
cd /home/chris/dev/M1-lang-test && python3 harness/sweep.py
# Expected: 126/126 RUN_PASS

# 4. No Result types remain in registry
grep -i "ResultText\|ResultUnit\|ResultBytes" \
  /home/chris/dev/axRegistry-working/axis-bridge.axreg
# Expected: zero matches
```

## Registry propagation

- /home/chris/dev/axRegistry-working/axis-bridge.axreg
- /home/chris/dev/axAI-axlang-gen-working/registries/axis-bridge.axreg

Also remove the now-unused type declarations from m1-registry.axreg in
axis-lang-lab-working if ResultText/ResultUnit are still declared there.

## CLAUDE.md

Confirm the plain-return-type rule is now universal — no exceptions.

One commit per repo:
- axis-codegen-bridge-rs: "fix: hash256_parse and ir_write_bundle — plain return types, no Result exceptions"
- axRegistry-working + axAI-axlang-gen-working: "registry: remove last Result types"
- axis-lang-lab-working (if needed): "fix: remove ResultText/ResultUnit declarations"

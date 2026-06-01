# Migration: lib-first build output

The `build` subcommand is now **lib-first**: every successful build produces a
static archive (`.a`) as its primary output. A runnable binary is only produced
when `--exe` is explicitly passed.

## What changed

| Before | After |
|--------|-------|
| `build … --out build/foo` → produces binary `build/foo` | `build … --out build/foo` → produces archive `build/libfoo.a` |
| `build … --out build/foo` → no `.a` | `build … --out build/foo --exe` → produces both `build/libfoo.a` **and** binary `build/foo` |

If `--out` already ends in `.a` the path is used verbatim (no `lib` prefix added).

## Changes needed in `axAI-axlang-gen-working`

Every invocation of `axis-codegen-bridge build` that expects a runnable binary
at the `--out` path must have `--exe` appended. No other change is required.

### `scripts/build.sh`

| Line | Current invocation (abbreviated) | Change needed |
|------|-----------------------------------|---------------|
| 64   | `"$BRIDGE_BINARY" build "$coreir" --out "$bin" --lib-dir … \| return 1` | add `--exe` |
| 100  | `"$BRIDGE_BINARY" build "$BUILD_DIR/generative_form.coreir" --out "$BUILD_DIR/generative_form" --lib-dir …` | add `--exe` |
| 108  | `"$BRIDGE_BINARY" build "$BUILD_DIR/gen_transition.coreir" --out "$BUILD_DIR/gen_transition" --lib-dir …` | add `--exe` |
| 127  | `"$BRIDGE_BINARY" build "$out" --out "$BUILD_DIR/$bname" --lib-dir …` | add `--exe` |
| 136  | `"$BRIDGE_BINARY" build "$BUILD_DIR/gen_lib_fn.coreir" …` | add `--exe` |
| 147  | `"$BRIDGE_BINARY" build "$BUILD_DIR/codegen_pipeline.coreir" --out "$BUILD_DIR/codegen_pipeline" --lib-dir …` | add `--exe` |
| 152  | `"$BRIDGE_BINARY" build "$BUILD_DIR/gen_program.coreir" …` | add `--exe` |
| 159  | `"$BRIDGE_BINARY" build "$BUILD_DIR/budget_gate.coreir" …` | add `--exe` |
| 166  | `"$BRIDGE_BINARY" build "$BUILD_DIR/coreir_view.coreir" …` | add `--exe` |
| 179  | `"$BRIDGE_BINARY" build "$BUILD_DIR/coreir_view_h1.coreir" …` | add `--exe` |
| 201  | `"$BRIDGE_BINARY" build "$BUILD_DIR/pipeline_hello.coreir" --out "$BUILD_DIR/pipeline_hello" --lib-dir …` | add `--exe` |
| 219  | `"$BRIDGE_BINARY" build "$BUILD_DIR/pipeline_arithmetic.coreir" --out "$BUILD_DIR/pipeline_arithmetic" --lib-dir …` | add `--exe` |
| 266  | `"$BRIDGE_BINARY" build "$BUILD_DIR/test_3step.coreir" …` | add `--exe` |
| 308  | `"$BRIDGE_BINARY" build "$coreir" --out "$bin" --lib-dir …` (inside `if`) | add `--exe` |
| 351  | `"$BRIDGE_BINARY" build "$coreir" --out "$bin" --lib-dir …` (inside `if`) | add `--exe` |
| 377  | `"$BRIDGE_BINARY" build "$coreir" --out "$bin" --lib-dir …` (inside `if`) | add `--exe` |
| 402  | `"$BRIDGE_BINARY" build "$coreir" --out "$bin" --lib-dir …` (inside `if`) | add `--exe` |

### `scripts/ratchet.sh`

| Line | Current invocation | Change needed |
|------|-------------------|---------------|
| 642–643 | `"$BRIDGE_BINARY" build "$COREIR_OUT" --out "$BIN_OUT" --lib-dir … --lib-dir …` | add `--exe` |
| 765–766 | `"$BRIDGE_BINARY" build "$COREIR_OUT" --out "$BIN_OUT" --lib-dir … --lib-dir …` (inside `if !`) | add `--exe` |

## Example diff pattern

```diff
-"$BRIDGE_BINARY" build "$COREIR_OUT" --out "$BIN_OUT" \
-    --lib-dir "$LIB_CORE_DIR" --lib-dir "$LIB_TRANS_DIR"
+"$BRIDGE_BINARY" build "$COREIR_OUT" --out "$BIN_OUT" \
+    --lib-dir "$LIB_CORE_DIR" --lib-dir "$LIB_TRANS_DIR" --exe
```

## New capability: `bundle`

Archives can be merged into a single `.a` for distribution:

```sh
axis-codegen-bridge bundle \
  --out build/axis-hot.a \
  build/libfoo.a build/libbar.a build/libbaz.a
```

Requires `ar` on `PATH`.

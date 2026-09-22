# Diagnostics audit — good, bad, ugly (2026-09-23)

What a human sees when each module does something or fails: panic / expect /
unwrap messages, stderr output, returned error strings — and failures that
produce **nothing**. 84 modules, `#[cfg(test)]` code excluded. Branch
`tasks-spawn-join-v1` at `80db5c9`.

## The rubric

| Grade | Means |
|---|---|
| **GOOD** | Every failure says WHERE (fn name), WHAT it expected, WHAT it got (the value / offset / path / handle). Nothing hidden. |
| **BAD** | Failures surface but under-inform: bare `unwrap`/`expect` on fallible ops, or a message that omits the offending value. |
| **UGLY** | Failure hidden or disguised: swallowed error, failure returned as an ordinary-looking value, or a misleading message. |
| **N/A** | No failure paths worth grading. |

A module is graded by its dominant character; its worst spot is named anyway.

## Scoreboard

| | GOOD | BAD | UGLY | N/A |
|---|---|---|---|---|
| Value & language prims (21) | 9 | 6 | 5 | 1 |
| IR & build CLI (12) | 6 | 2 | 1 | 3 |
| Storage (25) | 13 | 4 | 5 | 3 |
| Concurrency & indexes (26) | 12 | 2 | 6 | 6 |
| **Total (84)** | **40** | **14** | **17** | **13** |

**The one-line story:** the panic *messages* are mostly good — fn-named,
value-bearing, `#[track_caller]`. The ugly is not bad messages; it is
**failures that never produce a message at all** — silent fallbacks to
`0`, `""`, `Unit`, `true`, or "end of log".

## UGLY, ranked by consequence

"Verified" = checked by reading the cited lines directly, not just reported.

### Tier 1 — silent data loss or corruption

| Where | What happens | Verified |
|---|---|---|
| `runtime/registry.rs:33` | Unparseable registry JSON → `unwrap_or_default()` → empty store. The next `registry_insert` (:251) saves a one-entry store **over the file, erasing its history**. `registry_verify_chain` says `true` for an unreadable registry. | yes |
| `runtime/rawblk.rs:396-402` | `append_shape_durable` swallows create_dir, open, write **and fsync** errors. `pgbshape.rs:136` then returns the shape id as success. Rows written under that shape can't be decoded at recovery. | yes |
| `runtime/mmapseg.rs:173-175`, `:333` | `msync` return ignored and the synced watermark advances anyway; `mmapseg_flush_file` ignores `fdatasync` errors. After a failed fdatasync the kernel may mark pages clean, so "the next tick retries" can hide lost data. | yes |
| `runtime/hotblk_recover.rs:54`, `walindex.rs:470` | Any block read error (EIO, EACCES) = "clean frontier". Recovery/index stops short and reports a normal, smaller count. | yes (hotblk_recover) |

### Tier 2 — wrong answer that looks right

| Where | What happens | Verified |
|---|---|---|
| `runtime/process.rs:13` | `proc_exit` with a non-Int argument **exits 0 (success)**. | yes |
| `runtime/oneshot.rs:101` | Waiting on an id that was never minted returns "signaled" at once — a garbage id looks like a durability ack. | yes |
| `runtime/tuple.rs:30,45` | `tuple_field` past the end → `Unit`; `value_0/1/2` on any non-compound → `Unit`. CLAUDE.md says an out-of-range index is exactly when to panic. | yes |
| `runtime/arith.rs:143` | `str_to_int("abc")` → `0`. Its siblings `str_to_dec` / `str_to_float` panic with the text. | yes |
| `runtime/list.rs:117` | `list_cons` with a non-list tail discards the tail, returns `[elem]`. | yes |
| `runtime/str_ops.rs:170` | `chr` of an invalid code point → NUL. | yes |
| `runtime/adjacency.rs:341` | `adj_get("in", n)` (lowercase) silently returns **out**-edges. Unreadable packs dropped uncounted (:166-262). | yes |
| `runtime/io.rs:71` | `fs_read_last_line`: documented "missing = empty", but catches **every** error — permission denied and bad UTF-8 also return `""`. | yes |
| `runtime/frontend.rs:80-97`, `enumeration.rs:135` | Bad args / unreadable files return the same value as "nothing found". | yes |

### Tier 3 — hangs or lost diagnostics

| Where | What happens | Verified |
|---|---|---|
| `runtime/unified_wait.rs:258` | fd-reader thread exits on I/O error with no errno; the waiting context parks forever. | yes |
| `runtime/channels.rs:357` | `bchan_*` names are get-or-create: a misspelled name blocks forever. (`channel_send` has a build-time declared-name check; `bchan_*` doesn't.) | agent only |
| `main.rs:216` | CLI arg loop ignores unknown flags — `--regs x` or a trailing `--out` silently changes the build. | yes |

### Low reach (ugly but contained)

- `interner_lockfree_feed.rs:98`, `interner_mutex_feed.rs:117` — invalid handle becomes the string `<invalid-str-N>`. Declared in `mod.rs` but not wired into `Value`.
- `tsmark.rs:417-461` — telemetry write errors swallowed; returns the mark count as if flushed. Can falsify a benchmark, not user data.
- `hotwrite_batch.rs:106` — bench harness; C side ignores `fsync`, returns `-1` as a byte count.
- `scratch.rs:209` — unknown name acts as empty set/map. Documented design; a typo is never reported.

## BAD — surfaces, but under-informs

| Module | Worst spot |
|---|---|
| `arith.rs` | ~50 type-mismatch panics say "expected two Int values" but not what arrived (:12). |
| `ir_eval.rs` | ~70 of 92 panics name the expected shape, never the actual value (:30). |
| `list.rs` | Most ops: "list_len: expected List" with no value (:125); raw index panic at :132. |
| `option.rs` | `option_unwrap: not an option value` — no value (:39). `result.rs` gets this right. |
| `bool_ops.rs` | `ax_assert` → `"assertion failed"`, no fn name (:8). |
| `str_ops.rs` | Char indexing panics as a raw Rust slice panic, no bridge fn name (:28). |
| `tty.rs` | `tty_raw_off` ignores tcsetattr failure — terminal left raw, no message (:106). |
| `main.rs` | Mostly excellent errors; unknown-flag skip (above) and `:793` rename swallow. |
| `pgbshape.rs` | Returns success over the silent `append_shape_durable` failure (:136). |
| `hotmem.rs` | Second writer's arena cell silently orphaned; its writes invisible (:140). |
| `walindex.rs` | Snapshot errors omit the path (:161); read error = frontier (:470). |
| `cursor.rs` | `cursor_append` panics on unknown handle; `cursor_get`/`cursor_len` return `""`/`0` for the same thing (:135). |
| `channels.rs` | `channel_depth` always returns 0 (:155 — a documented stub, still callable). |
| `nameptr.rs` | `""` for an unset slug is indistinguishable from a stored `""` (:102). Safe only while the one caller keeps its fallback. |

## GOOD — the house style done right

`value` `coerce` `result` `fail` `iter` `hash` `bytes_codec` `bytes_io`
`tasks` · `emit/rust_05` `core_ir_05/{mod,loader,serialiser}` `ir_accessors`
`ir_constructors` · `rawmem` `block_flush` `objseg` `pg_store` `hotblk`
`reclog` `logbuf` `slablock` `slabshadow` `sqlite_ro` `prealloc` `chunk`
`seek` · `mpsc_intrusive` `non_blocking_memory` `net` `ack_registry`
`interner_shard` `u32v` `indexer` `fieldidx` `gcidx` `pkindex`
`contradicts` `axbi`

Exemplars worth copying:
- `contradicts.rs:122` — refuses a cold lookup and says what to call first.
- `indexer.rs:116` — `"{path} is {n} bytes, seal recorded byte_len={m} (torn/short flush?)"`: value, expectation, *and* a likely cause.
- `tuple.rs:47-62` — the type-mismatch panics suggest the right fn to use.
- `reclog.rs:271`, `seek.rs:64` — every durability call checked, path + offset + errno.

Notable GOOD-with-a-wart: `emit/rust_05.rs:742` — an unreadable `--reg`
file is a *warning* and the build continues; the real error surfaces later
as "unresolved CCall identity".

## N/A

`transitions` `lib` `emit/mod` `core_ir_05/inspect` `blockfile`
`hotblk_pool` `walshard` `qhm` `slice4` `contentidx` `bindidx` `allocprobe`
`coldprobe` — pure helpers, documented miss-sentinel caches, or probes.

## Patterns

1. **Integer handles are strict; string names are lenient.** `net`, `u32v`,
   `fieldidx`, `pkindex`, `contradicts` panic naming the bad handle.
   `scratch`, `nameptr`, `bchan_*`, `adj_get`'s `dir` are get-or-create or
   empty-on-miss — so a typo is never reported.
2. **"Read error = end of log"** recurs across the recovery path
   (`hotblk_recover`, `walindex`, `rawblk:671`, `indexer:273`). A torn tail
   is a legitimate frontier; EIO is not, and nothing distinguishes them.
3. **`Option` / `Result` exist and are underused.** Several silent sentinels
   (`tcp_connect` → `-1`, `nameptr` → `""`, `proc_run` → `NO_START`) are
   exactly the "failure is an ordinary outcome" case `result.rs` was built for.

## How this was produced

A regex count pass (panic sites, fn-prefixed, value
interpolated, unwrap/expect, swallow patterns) to aim; then four readers
graded the modules against the rubric above, each citing `file:line`. Every
Tier 1–3 claim marked "yes" was re-read at the cited lines before it went in
here. BAD and GOOD grades are the readers' and have **not** all been
individually re-checked — spot-check before acting on one.

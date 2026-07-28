# BRIDGE_SURFACE_AUDIT_V1 — Bridge Surface Verdict Ledger

**Intent:** `BRIDGE_SURFACE_AUDIT_V1` (analysis-only; no source mutation; AI-propose-only)
**Date:** 2026-07-29 · **Auditor:** ClaudeCode (bounded read-only) · **Decision authority:** Chris
**Companion artifact:** `BRIDGE_SURFACE_AUDIT_V1.json` (one record per function, with evidence citations)

## 0. Method and closed set

- **Closed set:** the 181 `contains`-members of `view:axbridge:impl-of-axverity-surface-v1`
  (instance rust-bridge, backing DB `axsemantica-rust-bridge.db`) **plus** the 8 macro-generated
  fns the view's own metadata records as scanner-invisible (`int_add int_sub int_mul int_lt
  int_lte int_gt int_gte text_lt`) = **189 body-audited functions**, plus the declaration delta
  against `axRegistry-working/axis-bridge.axreg` (323 `fn` entries).
  *Discrepancy note:* the view node's statement says "182 members"; the DB holds exactly 181
  `contains` edges (including the hand-picked `channels::wait-v2`). The statement over-counts by one.
- **Body reads:** every one of the 189 fns was read brace-to-brace (8 parallel read-only audit
  agents + lead-auditor spot-verification of `net.rs:270-279`, `channels.rs:164-172`,
  `rawmem.rs:145-155`, `walindex.rs:733-743`, `str_ops.rs:1-30`, `value.rs:120-140`,
  `bytes_io.rs:44-57`). Names/comments were not accepted as behavioural evidence.
- **Caller census:** scripted comment-stripped scan (pattern `\b<sym>\s*\(`, definitions
  excluded) over all `.m1` in axVerity-working + axis-lang-lab-working, all `.rs` under
  `axis-codegen-bridge-rs/src`, and all `.axreg` in the three repos + axis-bridge.axreg.
  axSemantica `calls` edges were NOT used.
- **Liveness:** an M1 call site counts as live iff its file is in `lib/`, `lib_async/`,
  `lib_memchecked/`, `lib_hotwrite_workload/hrw_mint_block.m1`, or a `src/<cmd>.m1` actually
  compiled by `scripts/build.sh` / `scripts/*.sh` (58 built commands). `bench/ spike/ sim/
  tests/ substrate/` and unbuilt `src/` files are non-live.
- **Hot-path:** mechanical BFS over the M1 call graph from `pg_exec_insert*`/`pg_exec_delete*`
  + the five janitor steps (insert side) and `pg_exec_select/join` + `agg_*/cand_*/ev_*/
  block_read*` (select side).

## 1. Verdict tallies (189 audited)

| Verdict | Count |
|---|---|
| PRIMITIVE-LOAD-BEARING | **89** |
| COMPOSITE-REPATRIABLE | **74** (trivial 11 · moderate 17 · blocked 46) |
| EXPERIMENT-RESIDUE | **26** |
| DEAD (audited set) | 0 |
| UNKNOWN | 0 |
| DEAD (declared-only registry sprawl, census-verdict) | **146** |

Every verdict record in the JSON carries impl `file:line`, the caps evidence quote, and the
census citation. No verdict was issued from a name.

## 2. Minimum bridge surface

**89 functions are PRIMITIVE-LOAD-BEARING.** Capability breakdown:

| Irreducible capability | Count | Members |
|---|---|---|
| value-atom | 31 | `bytes_concat`, `bytes_len`, `bytes_slice`, `bytes_to_text`, `chr`, `ctor_field`, `dec_div`, `dec_to_text`, `int_add`, `int_div`, `int_eq`, `int_gt`, `int_gte`, `int_lt`, `int_lte`, `int_mod`, `int_mul`, `int_sub`, `int_to_dec`, `list_get`, `str_concat`, `str_eq`, `str_gt`, `str_gte`, `str_len`, `str_lt`, `str_lte`, `str_slice`, `text_lt`, `text_to_bytes`, `tuple_field` |
| syscall | 21 | `fs_append_text`, `fs_file_exists`, `fs_mkdir_p`, `fs_read_bytes`, `fs_read_range`, `fs_read_text`, `fs_write_bytes`, `idxseg_lookup`, `index_build_batch`, `index_rebuild_dir`, `io_eprint`, `io_println`, `logbuf_flush`, `logbuf_open`, `logbuf_read`, `logbuf_sync`, `tcp_accept`, `tcp_close`, `tcp_listen`, `tcp_read`, `tcp_write` |
| threads/OS | 16 | `ack_register`, `bchan_len`, `bchan_send`, `bchan_take`, `bindidx_get`, `bindidx_put`, `channel_send`, `contentidx_get`, `contentidx_put`, `event_subscribe`, `hotblk_pool_put`, `hotblk_pool_take`, `oneshot_wait`, `oneshot_wait_timeout`, `reclog_submit`, `wait` |
| OS | 5 | `argv_get`, `argv_int`, `now_unix_nanos`, `proc_exit`, `wal_shard_count` |
| atomics | 5 | `cell_cas_raw`, `cell_load_raw`, `cell_new_raw`, `hotmem_read`, `hotmem_reader_start` |
| raw memory | 4 | `mem_free_raw`, `mem_read_raw`, `mem_reserve_raw`, `mem_write_raw` |
| atomics+locks | 3 | `qhm_flush`, `qhm_get`, `qhm_put` |
| control flow | 2 | `loop_count`, `loop_while` |
| syscall+threads | 1 | `reclog_flush_once` |
| syscall+FFI | 1 | `wal_write_seg` |

Two readings of "minimum":

- **Hard OS/concurrency floor — 58 fns** (syscall 21, threads/OS 16, atomics 5, atomics+locks 3,
  raw memory 4, OS 5, syscall+FFI 1, syscall+threads 1, control-flow 2): these need a capability
  M1 cannot express (fs/net/stdio syscalls, condvars, lock-free queues, raw pointers, fallocate,
  process control, iteration itself — "M1 can't loop/recurse", axVerity-working/CLAUDE.md:856).
- **Value-atom floor — +31 fns**: Core IR 0.5 M1 has **no native operators** — zero infix
  arithmetic/comparison expressions exist in `lib/*.m1`; even path concatenation is
  `str_concat(...)` CCalls. Until the language grows native ops (a compiler question, out of
  scope), `int_add`, `str_concat`, `bytes_slice`, `tuple_field` etc. are irreducible atoms:
  removing one leaves the operation inexpressible. This is an interpretive extension of the
  taxonomy's example list, applied under its definition clause ("a capability M1 cannot express
  at all") and flagged here explicitly.

So: **minimum surface = 89 as audited (58 + 31)**; the value-atom tier is compiler-addressable,
the 58 are not. The be-codec quartet (`int16/32_be_encode/decode`, verdict COMPOSITE-blocked)
would collapse to 2 smaller atoms (`bytes_get`, `bytes_of_int`) if those were ever minted.

## 3. Ranked repatriation list (COMPOSITE-REPATRIABLE, 74)

Order: cost ascending, then LOC (surface reduction) descending. Hot-path column per §0.
**Trivial + moderate tiers (28 fns) are the actionable proposal; blocked tier (46) is listed
for the record with the named missing capability.** Proposal only — no repatriation performed.

| fn | cost | LOC | hot path | M1 form |
|---|---|---|---|---|
| `str_after` | trivial | 16 | both | if idx>=0 { str_slice(s,idx+len,slen) } else { Text("") } |
| `str_before` | trivial | 16 | both | if idx>=0 { str_slice(s,0,idx) } else { s } with idx=str_index_of |
| `str_char` | trivial | 11 | none | str_slice(s, i, int_add(i, Int(1))) |
| `str_contains` | trivial | 11 | both | int_gte(str_index_of(h,n), Int(0)) |
| `str_ends_with` | trivial | 11 | select | str_eq(str_slice(s, int_sub(str_len(s),str_len(p)), str_len(s)), p) |
| `str_starts_with` | trivial | 11 | both | str_eq(str_slice(s,0,str_len(p)), p) |
| `int_max` | trivial | 9 | none | if int_gt(a,b) { a } else { b } |
| `bool_and` | trivial | 6 | both | if truthy(a) { b } else { Bool(false) } via native if/match |
| `bool_or` | trivial | 6 | both | native if/match |
| `int_abs` | trivial | 6 | select | if int_lt(n, Int(0)) { int_sub(Int(0), n) } else { n } |
| `bool_not` | trivial | 3 | both | native if/match |
| `pgb_parse_shape` | moderate | 50 | none | str ops INSERT parse + fs_read_text/fs_write_bytes shapes.log append |
| `pg_stream_rows` | moderate | 40 | select | loop over LF tokens + DataRow framing + tcp_write |
| `pgb_record` | moderate | 37 | insert | bytes decode + str formatting over threaded shape table |
| `pg_emit_datarow1` | moderate | 31 | none | int32_be_encode + bytes_concat framing, then tcp_write |
| `str_pad_left` | moderate | 26 | insert | loop_count + str_concat |
| `str_pad_right` | moderate | 26 | none | loop_count + str_concat |
| `cursor_sort` | moderate | 24 | select | loop_while insertion sort over cursor lines + str comparisons |
| `hash256_parse` | moderate | 24 | both | str_len + str_starts_with + loop_count hex-char check |
| `rawblk_frame` | moderate | 22 | insert | str zero-pad (int_to_str + str_pad_left) + text_to_bytes + bytes_concat |
| `pgb_bind_capture` | moderate | 18 | none | int16_be_decode/int32_be_decode walk over Bind bytes; captured params returned as a value  |
| `str_index_of` | moderate | 16 | both | loop_while scan + str_slice + str_eq |
| `str_replace` | moderate | 15 | both | loop_while + str_index_of + str_slice + str_concat |
| `fs_read_last_line` | moderate | 12 | insert | fs_read_text + loop_while reverse line scan |
| `pgb_payload` | moderate | 10 | insert | str_pad_left(shape_id) + text_to_bytes + bytes_concat over explicitly-threaded captured pa |
| `str_to_int` | moderate | 9 | both | loop_count over chars + 10-way digit match |
| `int_to_str` | moderate | 6 | both | loop_while digit extraction (int_mod/int_div) + chr + str_concat |
| `str_trim` | moderate | 6 | both | loop_while + str_char + whitespace compare |
| `hotblk_recover_rebuild` | blocked | 49 | none | fs_read_bytes/fs_read_range segment walk + frame parse + state build |
| `fieldidx_res_get` | blocked | 48 | none | fs_read_bytes/fs_read_range segment walk + frame parse + state build |
| `walidx_res_get` | blocked | 48 | both | fs_read_bytes/fs_read_range segment walk + frame parse + state build |
| `rawblk_recover_rebuild` | blocked | 46 | none | fs_read_bytes/fs_read_range segment walk + frame parse + state build |
| `rawblk_recover_open` | blocked | 30 | none | fs_read_bytes/fs_read_range segment walk + frame parse + state build |
| `contradicts_rebuild` | blocked | 28 | none | fs_read_bytes/fs_read_range segment walk + frame parse + state build |
| `logbuf_append` | blocked | 26 | insert | pure data structure threaded through the CoreIR dataflow as a Value |
| `nameptr_set` | blocked | 25 | insert | pure data structure threaded through the CoreIR dataflow as a Value |
| `fieldidx_snapshot` | blocked | 24 | none | loop-serialize map + fs_write_bytes (already durable tmp+fsync+rename) |
| `pkidx_rebuild` | blocked | 24 | none | fs_read_bytes/fs_read_range segment walk + frame parse + state build |
| `hotblk_dir_guard` | blocked | 23 | insert | pure data structure threaded through the CoreIR dataflow as a Value |
| `hotblk_set` | blocked | 22 | insert | pure data structure threaded through the CoreIR dataflow as a Value |
| `fieldidx_get` | blocked | 21 | none | pure data structure threaded through the CoreIR dataflow as a Value |
| `walidx_snapshot` | blocked | 21 | none | loop-serialize map + fs_write_bytes (already durable tmp+fsync+rename) |
| `walidx_insert` | blocked | 20 | none | pure data structure threaded through the CoreIR dataflow as a Value |
| `fieldidx_res_scope` | blocked | 19 | none | explicit scope teardown once index state is M1-side |
| `walidx_res_scope` | blocked | 19 | none | explicit scope teardown once index state is M1-side |
| `fieldidx_rebuild` | blocked | 18 | none | fs_read_bytes/fs_read_range segment walk + frame parse + state build |
| `walidx_rebuild` | blocked | 18 | none | fs_read_bytes/fs_read_range segment walk + frame parse + state build |
| `pkidx_get` | blocked | 17 | none | pure data structure threaded through the CoreIR dataflow as a Value |
| `contradicts_any` | blocked | 16 | none | pure data structure threaded through the CoreIR dataflow as a Value |
| `cursor_append` | blocked | 16 | select | pure data structure threaded through the CoreIR dataflow as a Value |
| `cursor_line` | blocked | 16 | none | pure data structure threaded through the CoreIR dataflow as a Value |
| `hotblk_recover_open` | blocked | 14 | none | fs_read_bytes/fs_read_range segment walk + frame parse + state build |
| `nameptr_get` | blocked | 14 | insert | pure data structure threaded through the CoreIR dataflow as a Value |
| `cursor_open` | blocked | 13 | select | pure data structure threaded through the CoreIR dataflow as a Value |
| `int16_be_encode` | blocked | 13 | select | int_div/int_mod byte extraction, IF byte<->Bytes atoms existed |
| `int32_be_decode` | blocked | 13 | none | int_div/int_mod byte extraction, IF byte<->Bytes atoms existed |
| `int32_be_encode` | blocked | 13 | both | int_div/int_mod byte extraction, IF byte<->Bytes atoms existed |
| `logbuf_len` | blocked | 13 | none | pure data structure threaded through the CoreIR dataflow as a Value |
| `contradicts_open` | blocked | 12 | none | pure data structure threaded through the CoreIR dataflow as a Value |
| `fieldidx_open` | blocked | 12 | none | pure data structure threaded through the CoreIR dataflow as a Value |
| `chunk_file` | blocked | 11 | none | fs_read_bytes + FastCDC loop + bytes_hash per chunk |
| `cursor_get` | blocked | 11 | select | pure data structure threaded through the CoreIR dataflow as a Value |
| `int16_be_decode` | blocked | 11 | none | int_div/int_mod byte extraction, IF byte<->Bytes atoms existed |
| `pkidx_open` | blocked | 11 | none | pure data structure threaded through the CoreIR dataflow as a Value |
| `walidx_open` | blocked | 11 | none | pure data structure threaded through the CoreIR dataflow as a Value |
| `bytes_hash` | blocked | 10 | both | SHA-256 over Bytes |
| `hotblk_get` | blocked | 10 | insert | pure data structure threaded through the CoreIR dataflow as a Value |
| `cursor_load` | blocked | 9 | select | pure data structure threaded through the CoreIR dataflow as a Value |
| `cursor_len` | blocked | 8 | select | pure data structure threaded through the CoreIR dataflow as a Value |
| `wal_shard_set` | blocked | 8 | none | pure data structure threaded through the CoreIR dataflow as a Value |
| `cursor_close` | blocked | 7 | select | pure data structure threaded through the CoreIR dataflow as a Value |
| `str_to_lower` | blocked | 6 | select | per-char mapping loop |
| `str_to_upper` | blocked | 6 | both | per-char mapping loop |
| `wal_shard_get` | blocked | 3 | insert | pure data structure threaded through the CoreIR dataflow as a Value |

Blocked-tier reasons cluster into exactly four missing capabilities:
1. **Cross-call mutable state** (34 fns: cursor/walidx/fieldidx/pkidx/contradicts/nameptr/
   hotblk-accumulator/wal_shard/logbuf-buffer families) — value threading through CoreIR, shape change.
2. **byte<->Int atoms** (4 fns: be-codecs; also gates the recover/rebuild parsers).
3. **Bitwise integer ops** (2 fns: `bytes_hash` SHA-256, `chunk_file` gear-hash).
4. **Unicode tables** (2 fns: `str_to_upper/lower`).

## 4. Duplicate findings

- `argv_get` is byte-identical to `argv` (process.rs:62; `argv` is declared-only sprawl).
  **Survivor: `argv_get`** (the live name); retire the `argv` declaration.
- `sleep` (ms) duplicates declared-only `proc_sleep` (s). Both currently without live callers;
  survivor if kept: `sleep`.
- `walidx_res_get` / `fieldidx_res_get`: line-for-line parallel 48-LOC twins; `*_res_scope`
  twins are byte-identical modulo the map identifier; `*_snapshot`/`*_open` share the skeleton;
  `write_durable` is **triplicated** (walindex.rs, fieldidx.rs, bytes_io.rs). Not DUPLICATE
  verdicts (key/value shapes differ) but **one parameterised family, not two** — survivor: a
  single generic implementation. pkindex is a degenerate third (no snapshot/frontier/hotblk).
- `rawblk_recover_*` vs `hotblk_recover_*`: **NOT duplicates** — substrate-specific variants
  over different frame formats (RAWF1/`parse_raw_frames` vs blockfile trailer/
  `parse_frames_from_bytes`), zero shared helpers. Settles intent unknown #3.
- `ts_mark` / `ts_markp`: gated twin (SPLITPROBE gate is the only difference).
- `bindidx` vs `contentidx`: same 256-shard mutex skeleton, different value type + eviction
  policy (overwrite-unbounded vs insert-if-absent-FIFO-capped). Parameterisable pair.

## 5. Registry deltas (axis-bridge.axreg, 323 declared)

- **DECLARED∩USED: 177**
- **DECLARED∖USED: 146 — removable sprawl candidates** (T4 confirmed non-empty).
  Each carries a census citation in the JSON. **Alias caution (cb69b96):** before removing any,
  check the other 34 `.axreg` files — several of these names ARE declared elsewhere.
  Full list: `__eq__`, `ack_signal_block`, `all`, `any`, `apply_function`, `argv`, `argv_count`, `argv_or`, `bchan_drain`, `block_flush_write`, `bridge_to_dec`, `bridge_to_float`, `composite_io_passthrough`, `content_hash`, `contradicts_any_warm`, `contradicts_has`, `contradicts_warm`, `count`, `debug_trace`, `drop`, `enumerate`, `extract_subterm_to_function`, `fieldidx_insert`, `fieldidx_replay`, `find_index`, `flat_map`, `flatten`, `foreach`, `frontend_lookup_shape`, `frontend_walk`, `fs_list_dir`, `fs_prealloc`, `fs_write_text`, `groupby_cursor_enabled`, `hotblk_recover_has_hash`, `hotblk_recover_pk_get`, `inline_let_binding`, `int_div_checked`, `introduce_lambda`, `introduce_let_binding`, `io_print`, `io_read_line`, `ir_apply`, `ir_build_fold_from_spec`, `ir_build_program_from_spec`, `ir_bundle_view`, `ir_eval`, `ir_free_vars`, `ir_get_arg`, `ir_get_body`, `ir_get_cond`, `ir_get_else`, `ir_get_fn`, `ir_get_int_val`, `ir_get_kind`, `ir_get_name`, `ir_get_then`, `ir_get_value`, `ir_make_app`, `ir_make_bool_lit`, `ir_make_call`, `ir_make_if`, `ir_make_int_lit`, `ir_make_lam`, `ir_make_let`, `ir_make_unit_lit`, `ir_make_var`, `ir_read_bundle`, `ir_rename`, `ir_subst`, `ir_term_kind`, `ir_to_h1_string`, `ir_to_string`, `ir_write_bundle`, `list_append`, `list_concat`, `list_cons`, `list_get_at`, `list_get_println_if_some`, `list_head`, `list_is_empty`, `list_len`, `list_nil`, `list_of_1`, `list_of_2`, `list_of_3`, `list_reverse`, `list_str_len_lte_if_some`, `list_tail`, `mmapseg_append`, `mmapseg_flush_file`, `mmapseg_frontier`, `mmapseg_msync`, `mmapseg_open`, `mmapseg_read`, `oneshot_new`, `oneshot_signal`, `option_is_none`, `option_is_some`, `option_none`, `option_some`, `option_unwrap`, `pkidx_has`, `proc_args`, `proc_sleep`, `qhm_stats`, `range`, `range_step`, `reference_registry_function`, `registry_all_entries`, `registry_compound_id`, `registry_get_contract`, `registry_get_effect_sig`, `registry_get_provenance`, `registry_has_entry`, `registry_insert`, `registry_lookup`, `registry_verify_chain`, `rename_bound_variable`, `repeat`, `seq`, `slab_append`, `slab_open`, `slab_seal`, `slab_sealed`, `slab_stats`, `slab_tick`, `slice`, `sqlite_ro_tsv`, `str_char_at`, `str_char_code`, `str_join`, `str_repeat`, `str_split`, `take`, `tcp_connect`, `tcp_listen_shared`, `test_lam`, `test_let`, `text_gt`, `value_eq`, `verify_foreign_reference`, `walidx_get`, `walidx_has`, `walidx_replay`, `zip`
- **USED∖DECLARED: 13 — correctness finding.** All 13 are called by axVerity M1 but
  absent from axis-bridge.axreg; each is declared only in a sibling registry:
  `cell_cas_raw cell_load_raw cell_new_raw mem_free_raw mem_read_raw mem_reserve_raw
  mem_write_raw` → `registry/axis-mem-raw.axreg`; `idxseg_lookup index_build_batch
  index_rebuild_dir` → `registry/axis-codegen-bridge.axreg` + `axverity-indexer.axreg`;
  `int_max str_after str_before` → `registry/axis.axreg` only. If axis-bridge.axreg is the
  authoritative surface (assumption, tentative), these 13 declarations should migrate into it.

## 6. Experiment residue (26)

| fn | hot path | what / experiment / status |
|---|---|---|
| `channel_depth` | insert | Stub: get-or-creates channel then returns hardcoded Int(0) (channels.rs:166-171). Experiment: WAY_BACK_CONSOLIDATION depth read removal — CONCLUDED. S |
| `derive_stat` | insert | Always-on derive-pool telemetry (pooldepth-*.txt dumps every 1024 items; rawblk.rs:515-525). Belongs to the memcpy/derive-pool trial (7bb6acf) — ONGOI |
| `derive_stat_flush` | insert | Always-on derive-pool telemetry (pooldepth-*.txt dumps every 1024 items; rawblk.rs:515-525). Belongs to the memcpy/derive-pool trial (7bb6acf) — ONGOI |
| `hotblk_recover_dump_hashes` | none | Same dump/oracle role for the hotblk (blockfile) substrate recovery gate (src/hotblk_recover_dump.m1). |
| `hotblk_recover_dump_pk` | none | Same dump/oracle role for the hotblk (blockfile) substrate recovery gate (src/hotblk_recover_dump.m1). |
| `hotblk_recover_stats` | none | Same dump/oracle role for the hotblk (blockfile) substrate recovery gate (src/hotblk_recover_dump.m1). |
| `hotmem_epoch` | none | Staleness telemetry probes over hotmem_read's thread-local (epoch,missed) record. Diagnostic reads; no functional consumer. |
| `hotmem_missed` | none | Staleness telemetry probes over hotmem_read's thread-local (epoch,missed) record. Diagnostic reads; no functional consumer. |
| `hotwrite_batch_run` | none | Bench scaffold: pure CPU/RAM batch synthesizer with black_box DCE guards; callers only in lib_hotwrite_workload/selftest. Experiment: hotwrite substra |
| `hotwrite_batch_run_c` | none | Bench scaffold crossing FFI to hotwrite_batch.c (SHA-NI dispatch, volatile sink). Selftest-only callers. One of only two FFI-boundary fns in the audit |
| `hotwrite_batch_run_c_durable` | none | Bench scaffold: C-side durable write loop (open/write/fsync/rename + manifest for hotwrite-workload-verify.py). Selftest-only callers. |
| `memcpy_canon_mode` | insert | Env-dial probes for AXVERITY_MEMCPY_HOTPATH_TRIAL_V1 (commit 7bb6acf) — trial ONGOING; dials select experimental write-path arms. |
| `memcpy_hotpath_mode` | insert | Env-dial probes for AXVERITY_MEMCPY_HOTPATH_TRIAL_V1 (commit 7bb6acf) — trial ONGOING; dials select experimental write-path arms. |
| `rawblk_recover_dump_hashes` | none | Diagnostic dumps/counters for the kill-9 recovery verification gate (verify-slice4-kill9.sh; src/rawblk_recover_dump.m1 is a built CLI). Ops-verificat |
| `rawblk_recover_dump_pk` | none | Diagnostic dumps/counters for the kill-9 recovery verification gate (verify-slice4-kill9.sh; src/rawblk_recover_dump.m1 is a built CLI). Ops-verificat |
| `rawblk_recover_stats` | none | Diagnostic dumps/counters for the kill-9 recovery verification gate (verify-slice4-kill9.sh; src/rawblk_recover_dump.m1 is a built CLI). Ops-verificat |
| `slab_shadow_flush_once` | insert | Slab shadow-write measurement experiment: env-gated (AXVERITY_SLAB_SHADOW, default off); flush body exists to emit R/T measurement TSV lines. ONGOING/ |
| `slab_shadow_submit` | insert | Slab shadow-write measurement experiment: env-gated (AXVERITY_SLAB_SHADOW, default off); flush body exists to emit R/T measurement TSV lines. ONGOING/ |
| `slab_to_wire_enabled` | select | Probe over a hardcoded-false flag (net.rs:278). Experiment: AXVERITY_WAY_BACK_CONSOLIDATION_V1 slab-to-wire batching — CONCLUDED (measured no-win, DRO |
| `sleep` | none | OS sleep primitive with zero live callers after built-src filter — only lib_hotwrite_workload/hrw_maybe_pause.m1 (workload pacing). Trivially P-L-B if |
| `slice4_ack_timeout_ms` | insert | Dials for the slice4 block-durability bundle. Default flipped on in commit be69bec (AXVERITY_HOTPATH_UNBLOCK_V1 item 4) — experiment CONCLUDED, dial r |
| `slice4_mode` | insert | Dials for the slice4 block-durability bundle. Default flipped on in commit be69bec (AXVERITY_HOTPATH_UNBLOCK_V1 item 4) — experiment CONCLUDED, dial r |
| `ts_flush` | insert | AXVERITY_HOTPATH_MEASUREMENT_V1 telemetry instruments (thread-local mark buffers, TSV dumps, WRITEPROBE/SPLITPROBE gates). Measurement harness — ongoi |
| `ts_mark` | insert | AXVERITY_HOTPATH_MEASUREMENT_V1 telemetry instruments (thread-local mark buffers, TSV dumps, WRITEPROBE/SPLITPROBE gates). Measurement harness — ongoi |
| `ts_mark_val` | insert | AXVERITY_HOTPATH_MEASUREMENT_V1 telemetry instruments (thread-local mark buffers, TSV dumps, WRITEPROBE/SPLITPROBE gates). Measurement harness — ongoi |
| `ts_markp` | insert | AXVERITY_HOTPATH_MEASUREMENT_V1 telemetry instruments (thread-local mark buffers, TSV dumps, WRITEPROBE/SPLITPROBE gates). Measurement harness — ongoi |

Zero-live-caller audited fns (census, built-src filtered): `argv_int`, `fieldidx_get`, `hotwrite_batch_run`, `hotwrite_batch_run_c`, `hotwrite_batch_run_c_durable`, `list_get`, `logbuf_len`, `logbuf_read`, `sleep`, `tcp_listen`, `wal_shard_set`.

## 7. Named unknowns (open questions, not verdicts)

1. **Hot-path repatriation perf** — whether moving trivial/moderate composites (esp.
   `pg_stream_rows`, `rawblk_frame`, `str_index_of`, be-codecs-after-atoms) into M1 holds
   throughput. Settled only by a measured A/B under a separate intent (T6, deferred).
2. **Value-atom future** — whether Core IR grows native operators, which would reclassify the
   31-atom tier. Compiler-track question.
3. **`slab_shadow_*` disposition** — janitor binary is built and linked but env-gated off;
   is the shadow-measurement experiment concluded?
4. **`wal_shard_set` / `fieldidx_get` / `logbuf_read` / `logbuf_len` / `argv_int`** — zero live
   callers (spike/unbuilt-src only). Are the spike entry points retired? If yes these drift
   toward DEAD at next audit.

## 8. T1–T5 results

| Test | Threshold | Result | Verdict |
|---|---|---|---|
| T1 body-read of 48 pure-value fns; count with syscall/raw-ptr/atomic/FFI | ≤ 5 | **0** of 48 | **PASS** |
| T2 EXPERIMENT-RESIDUE + DEAD (audited set) | ≥ 10 | **26** (+146 declared-DEAD) | **PASS** |
| T3 index families one parameterised shape | divergence confined to key type & path | **PARTIAL**: open/snapshot/res_get/res_scope/write_durable are twins; get/put/rebuild visitors genuinely diverge (posting-list vs position-tuple vs envelope; hotblk negative-seg sentinel only in walidx); bindidx/contentidx are a different storage regime | **PARTIAL PASS** |
| T4 DECLARED∖USED non-empty after caller filter | non-empty | **146** | **PASS** |
| T5 ≥1 fully superseded durability path | zero live INSERT-path M1 callers | **`hotwrite_batch_*` family: 0 live callers** (selftest-only); `reclog_submit` confirmed live in `lib/pg_exec_insert.m1`; logbuf/walshard/slabshadow all have live links (slabshadow gated off) | **PASS** |

T1 caveat: `loop_count`/`loop_while` (iter) carry no syscall/ptr/atomic/FFI and so count 0 for
T1, yet still classify PRIMITIVE via control-flow irreducibility — the prediction's spirit
("overwhelmingly repatriable") holds only for derived ops; the atom tier does not repatriate
(§2). Reported as-is rather than smoothed.

## 9. Reintegration check

- **Identity:** BRIDGE_SURFACE_AUDIT_V1 — this document + JSON are the only outputs; no file
  outside `docs/bridge-audit/` was created or modified; no repatriation implemented.
- **NO_SOURCE_MUTATION:** upheld (artifacts only).
- **EVIDENCE_OR_UNKNOWN:** every audited verdict cites impl file:line + census; the two
  interpretive calls (value-atom primitives; state-blocked composites) are flagged as
  classification notes, and genuinely unsettled questions are in §7, not guessed.
- **NAMES_ARE_NOT_EVIDENCE:** all 189 bodies read; spot-verified sample by lead auditor.
- **CALLS_EDGES_NOT_GROUND_TRUTH:** census is ripgrep-equivalent scripted scan; no axSemantica
  `calls` edges consumed.
- **CLOSED_FUNCTION_SET:** 189 audited = 181 view + 8 macro (view-metadata-named); the 146
  sprawl records are declaration-census only, within the axreg-delta scope; nothing else audited.
- **Priority order respected:** evidence-grounding > completeness (189/189 + 323/323) >
  conservatism (0 UNKNOWN needed; doubtful cases parked as blocked or flagged §7) > speed.
- **Risk register:** load-bearing-misclassification mitigated by (a) conservative
  PRIMITIVE bias for atoms, (b) blocked-cost on all state-carrying composites (not actionable),
  (c) alias caution repeated on every sprawl record. Scope did not expand into the
  substrate bake-off (T6 deferred).
- **Authority:** all rankings are proposals; no change is authorised by this document.

## 10. Bottom line

323 declared / 189 actually called / **89 must stay** (58 hard OS floor + 31 value atoms that
are really a compiler gap) / **28 can come home** (11 trivial, 17 moderate) / **46 could come
home if M1 grew 4 named capabilities** / **26 are experiment residue** / **146 declarations
point at nothing live**. The bridge is ~2.4× its true declared need and ~3.6× its hard floor.

/// Emit Rust source from a Core IR 0.5 bundle.
///
/// The 0.5 model is a flat indexed table (constant pool + node list) where
/// every CCall references its target by a 256-bit identity token (sha256 of
/// the function name for §5b bootstrap functions). The emitter:
///   1. Resolves each CCall identity → bridge runtime path or registry name.
///   2. Decodes pool entries by matching def_hash against known primitive types.
///   3. Emits one `let pool_N` declaration per pool entry.
///   4. Emits one `let node_N` declaration per node in topological order.
///   5. Returns the last node (or first pool entry if no nodes) as the result.

use std::collections::HashMap;

use crate::core_ir_05::{
    bool_type_hash, dec_type_hash, decode_bool_payload, decode_int_payload, decode_text_payload,
    float_type_hash, fn_type_hash, hash256_to_hex, int_type_hash, sha256_bytes, text_type_hash,
    unit_type_hash, ConstantPoolEntry, CoreBundle, Hash256, Node, NodeRef,
};

// ── Symbol map (name → bridge path) ─────────────────────────────────────────

fn symbol_map() -> HashMap<&'static str, &'static str> {
    let mut m = HashMap::new();

    // Arithmetic
    m.insert("int_add",         "axis_codegen_bridge::runtime::arith::int_add");
    m.insert("int_sub",         "axis_codegen_bridge::runtime::arith::int_sub");
    m.insert("int_mul",         "axis_codegen_bridge::runtime::arith::int_mul");
    m.insert("int_div",         "axis_codegen_bridge::runtime::arith::int_div");
    m.insert("int_div_checked", "axis_codegen_bridge::runtime::arith::int_div_checked");
    m.insert("int_mod",         "axis_codegen_bridge::runtime::arith::int_mod");
    m.insert("int_to_str",      "axis_codegen_bridge::runtime::arith::int_to_str");
    m.insert("str_to_int",      "axis_codegen_bridge::runtime::arith::str_to_int");
    m.insert("int_abs",         "axis_codegen_bridge::runtime::arith::int_abs");
    m.insert("int_min",         "axis_codegen_bridge::runtime::arith::int_min");
    m.insert("int_max",         "axis_codegen_bridge::runtime::arith::int_max");
    m.insert("int_clamp",       "axis_codegen_bridge::runtime::arith::int_clamp");
    m.insert("celsius_to_fahrenheit", "axis_codegen_bridge::runtime::arith::celsius_to_fahrenheit");
    m.insert("fahrenheit_to_celsius", "axis_codegen_bridge::runtime::arith::fahrenheit_to_celsius");
    m.insert("is_positive",     "axis_codegen_bridge::runtime::arith::is_positive");

    // Comparison
    m.insert("int_lt",   "axis_codegen_bridge::runtime::arith::int_lt");
    m.insert("int_lte",  "axis_codegen_bridge::runtime::arith::int_lte");
    m.insert("int_gt",   "axis_codegen_bridge::runtime::arith::int_gt");
    m.insert("int_gte",  "axis_codegen_bridge::runtime::arith::int_gte");
    m.insert("int_eq",   "axis_codegen_bridge::runtime::arith::int_eq");
    m.insert("dec_lt",   "axis_codegen_bridge::runtime::arith::dec_lt");
    m.insert("dec_lte",  "axis_codegen_bridge::runtime::arith::dec_lte");
    m.insert("dec_gt",   "axis_codegen_bridge::runtime::arith::dec_gt");
    m.insert("dec_gte",  "axis_codegen_bridge::runtime::arith::dec_gte");
    m.insert("dec_eq",   "axis_codegen_bridge::runtime::arith::dec_eq");
    m.insert("dec_div",  "axis_codegen_bridge::runtime::arith::dec_div");
    m.insert("dec_to_text", "axis_codegen_bridge::runtime::arith::dec_to_text");
    m.insert("float_lt",  "axis_codegen_bridge::runtime::arith::float_lt");
    m.insert("float_lte", "axis_codegen_bridge::runtime::arith::float_lte");
    m.insert("float_gt",  "axis_codegen_bridge::runtime::arith::float_gt");
    m.insert("float_gte", "axis_codegen_bridge::runtime::arith::float_gte");
    m.insert("float_eq", "axis_codegen_bridge::runtime::arith::float_eq");
    m.insert("value_eq", "axis_codegen_bridge::runtime::arith::value_eq");

    // Unit / sequence helpers (§5b bootstrap functions)
    m.insert("unit_id",    "axis_codegen_bridge::runtime::arith::unit_id");
    m.insert("const_unit", "axis_codegen_bridge::runtime::arith::unit_id");
    m.insert("seq_unit",   "axis_codegen_bridge::runtime::arith::seq_unit");
    m.insert("seq",        "axis_codegen_bridge::runtime::arith::seq");

    // Boolean
    m.insert("bool_and",    "axis_codegen_bridge::runtime::bool_ops::bool_and");
    m.insert("bool_or",     "axis_codegen_bridge::runtime::bool_ops::bool_or");
    m.insert("bool_not",    "axis_codegen_bridge::runtime::bool_ops::bool_not");
    m.insert("bool_to_str", "axis_codegen_bridge::runtime::str_ops::bool_to_str");

    // Test assertion (identity = sha256("assert") — BRIDGE_TESTKIT_FINALIZE_V1)
    m.insert("assert",   "axis_codegen_bridge::runtime::bool_ops::ax_assert");

    // String
    m.insert("str_len",         "axis_codegen_bridge::runtime::str_ops::str_len");
    m.insert("str_concat",      "axis_codegen_bridge::runtime::str_ops::str_concat");
    m.insert("str_char",        "axis_codegen_bridge::runtime::str_ops::str_char");
    m.insert("str_char_at",     "axis_codegen_bridge::runtime::str_ops::str_char_at");
    m.insert("str_char_code",   "axis_codegen_bridge::runtime::str_ops::str_char_code");
    m.insert("str_slice",       "axis_codegen_bridge::runtime::str_ops::str_slice");
    m.insert("str_split",       "axis_codegen_bridge::runtime::str_ops::str_split");
    m.insert("str_starts_with", "axis_codegen_bridge::runtime::str_ops::str_starts_with");
    m.insert("str_ends_with",   "axis_codegen_bridge::runtime::str_ops::str_ends_with");
    m.insert("str_trim",        "axis_codegen_bridge::runtime::str_ops::str_trim");
    m.insert("str_contains",    "axis_codegen_bridge::runtime::str_ops::str_contains");
    m.insert("str_index_of",    "axis_codegen_bridge::runtime::str_ops::str_index_of");
    m.insert("str_eq",          "axis_codegen_bridge::runtime::str_ops::str_eq");
    m.insert("text_eq",         "axis_codegen_bridge::runtime::str_ops::text_eq");
    m.insert("text_lt",         "axis_codegen_bridge::runtime::str_ops::text_lt");
    m.insert("text_lte",        "axis_codegen_bridge::runtime::str_ops::text_lte");
    m.insert("text_gt",         "axis_codegen_bridge::runtime::str_ops::text_gt");
    m.insert("text_gte",        "axis_codegen_bridge::runtime::str_ops::text_gte");
    m.insert("str_lt",          "axis_codegen_bridge::runtime::str_ops::str_lt");
    m.insert("str_lte",         "axis_codegen_bridge::runtime::str_ops::str_lte");
    m.insert("str_gt",          "axis_codegen_bridge::runtime::str_ops::str_gt");
    m.insert("str_gte",         "axis_codegen_bridge::runtime::str_ops::str_gte");
    m.insert("str_before",      "axis_codegen_bridge::runtime::str_ops::str_before");
    m.insert("str_after",       "axis_codegen_bridge::runtime::str_ops::str_after");
    m.insert("str_between",     "axis_codegen_bridge::runtime::str_ops::str_between");
    m.insert("chr",             "axis_codegen_bridge::runtime::str_ops::chr");

    // List
    m.insert("list_nil",      "axis_codegen_bridge::runtime::list::list_nil");
    m.insert("list_cons",     "axis_codegen_bridge::runtime::list::list_cons");
    m.insert("list_len",      "axis_codegen_bridge::runtime::list::list_len");
    m.insert("list_get",      "axis_codegen_bridge::runtime::list::list_get");
    m.insert("list_get_at",              "axis_codegen_bridge::runtime::list::list_get_at");
    m.insert("list_get_println_if_some",   "axis_codegen_bridge::runtime::list::list_get_println_if_some");
    m.insert("list_str_len_lte_if_some",   "axis_codegen_bridge::runtime::list::list_str_len_lte_if_some");
    m.insert("list_append",              "axis_codegen_bridge::runtime::list::list_append");
    m.insert("list_concat",   "axis_codegen_bridge::runtime::list::list_concat");
    m.insert("list_reverse",  "axis_codegen_bridge::runtime::list::list_reverse");
    m.insert("list_head",     "axis_codegen_bridge::runtime::list::list_head");
    m.insert("list_tail",     "axis_codegen_bridge::runtime::list::list_tail");
    m.insert("list_is_empty", "axis_codegen_bridge::runtime::list::list_is_empty");
    m.insert("list_of_1",     "axis_codegen_bridge::runtime::list::list_of_1");
    m.insert("list_of_2",     "axis_codegen_bridge::runtime::list::list_of_2");
    m.insert("list_of_3",     "axis_codegen_bridge::runtime::list::list_of_3");

    // Tuple / constructor
    m.insert("tuple_field", "axis_codegen_bridge::runtime::tuple::tuple_field");
    m.insert("ctor_field",  "axis_codegen_bridge::runtime::tuple::ctor_field");

    // M1 compound-value constructors / accessors
    m.insert("value_make", "axis_codegen_bridge::runtime::tuple::value_make");
    m.insert("value_0",    "axis_codegen_bridge::runtime::tuple::value_0");
    m.insert("value_1",    "axis_codegen_bridge::runtime::tuple::value_1");
    m.insert("value_2",    "axis_codegen_bridge::runtime::tuple::value_2");
    m.insert("list_make",  "axis_codegen_bridge::runtime::list::list_make");

    // M1 iteration / list-builder primitives (BRIDGE_FOREIGN_FN_FNREF_M1).
    // `foreach` and `loop_count` use the native multi-arg Rust calling
    // convention — the callee is a bare fn path resolved from a Fn-typed
    // pool entry. `range` is data-only and uses the unary Tuple convention.
    m.insert("range",       "axis_codegen_bridge::runtime::iter::range");
    m.insert("foreach",     "axis_codegen_bridge::runtime::iter::foreach");
    m.insert("loop_count",  "axis_codegen_bridge::runtime::iter::loop_count");
    m.insert("str_join",    "axis_codegen_bridge::runtime::str_ops::str_join");

    // Phase 2 — P1 iteration / list vocabulary.
    m.insert("flat_map",    "axis_codegen_bridge::runtime::iter::flat_map");
    m.insert("any",         "axis_codegen_bridge::runtime::iter::any");
    m.insert("all",         "axis_codegen_bridge::runtime::iter::all");
    m.insert("find_index",  "axis_codegen_bridge::runtime::iter::find_index");
    m.insert("count",       "axis_codegen_bridge::runtime::iter::count");
    m.insert("loop_while",  "axis_codegen_bridge::runtime::iter::loop_while");
    m.insert("range_step",  "axis_codegen_bridge::runtime::iter::range_step");
    m.insert("repeat",      "axis_codegen_bridge::runtime::iter::repeat");
    m.insert("enumerate",   "axis_codegen_bridge::runtime::iter::enumerate");
    m.insert("zip",         "axis_codegen_bridge::runtime::iter::zip");
    m.insert("take",        "axis_codegen_bridge::runtime::iter::take");
    m.insert("drop",        "axis_codegen_bridge::runtime::iter::drop");
    m.insert("slice",       "axis_codegen_bridge::runtime::iter::slice");
    m.insert("flatten",     "axis_codegen_bridge::runtime::iter::flatten");

    // Phase 3 — P1 text emit helpers.
    m.insert("str_replace",  "axis_codegen_bridge::runtime::str_ops::str_replace");
    m.insert("str_repeat",   "axis_codegen_bridge::runtime::str_ops::str_repeat");
    m.insert("str_to_upper", "axis_codegen_bridge::runtime::str_ops::str_to_upper");
    m.insert("str_to_lower", "axis_codegen_bridge::runtime::str_ops::str_to_lower");
    m.insert("str_pad_left", "axis_codegen_bridge::runtime::str_ops::str_pad_left");
    m.insert("str_pad_right","axis_codegen_bridge::runtime::str_ops::str_pad_right");

    // Option
    m.insert("option_none",    "axis_codegen_bridge::runtime::option::option_none_fn");
    m.insert("option_some",    "axis_codegen_bridge::runtime::option::option_some");
    m.insert("option_is_none", "axis_codegen_bridge::runtime::option::option_is_none");
    m.insert("option_is_some", "axis_codegen_bridge::runtime::option::option_is_some");
    m.insert("option_unwrap",  "axis_codegen_bridge::runtime::option::option_unwrap");

    // Equality
    m.insert("__eq__", "axis_codegen_bridge::runtime::arith::value_eq");

    // IO
    m.insert("io_print",      "axis_codegen_bridge::runtime::io::io_print");
    m.insert("io_println",    "axis_codegen_bridge::runtime::io::io_println");
    m.insert("io_eprint",     "axis_codegen_bridge::runtime::io::io_eprint");
    m.insert("io_read_line",  "axis_codegen_bridge::runtime::io::io_read_line");
    m.insert("fs_read_text",  "axis_codegen_bridge::runtime::io::fs_read_text");
    m.insert("fs_write_text", "axis_codegen_bridge::runtime::io::fs_write_text");
    m.insert("fs_append_text","axis_codegen_bridge::runtime::io::fs_append_text");
    m.insert("fs_file_exists","axis_codegen_bridge::runtime::io::fs_file_exists");
    m.insert("fs_list_dir",   "axis_codegen_bridge::runtime::io::fs_list_dir");
    m.insert("debug_trace",   "axis_codegen_bridge::runtime::io::debug_trace");
    m.insert("fs_read_last_line", "axis_codegen_bridge::runtime::io::fs_read_last_line");

    // Process
    m.insert("proc_args",  "axis_codegen_bridge::runtime::process::proc_args");
    m.insert("proc_exit",  "axis_codegen_bridge::runtime::process::proc_exit");
    m.insert("proc_sleep", "axis_codegen_bridge::runtime::process::proc_sleep");
    m.insert("sleep",      "axis_codegen_bridge::runtime::process::sleep");
    m.insert("now_unix_nanos", "axis_codegen_bridge::runtime::process::now_unix_nanos");
    m.insert("argv",       "axis_codegen_bridge::runtime::process::argv");
    m.insert("argv_get",   "axis_codegen_bridge::runtime::process::argv_get");
    m.insert("argv_int",   "axis_codegen_bridge::runtime::process::argv_int");
    m.insert("argv_count", "axis_codegen_bridge::runtime::process::argv_count");
    m.insert("argv_or",    "axis_codegen_bridge::runtime::process::argv_or");

    // Async / IPC primitives (channels.rs — BRIDGE_ASYNC_PRIMITIVES_V1).
    // `wait` carries a single Fn-typed callee slot — see `fn_arg_kinds()`.
    m.insert("event_subscribe", "axis_codegen_bridge::runtime::channels::event_subscribe");
    m.insert("channel_send",    "axis_codegen_bridge::runtime::channels::channel_send");
    m.insert("wait",            "axis_codegen_bridge::runtime::channels::wait");

    // Value coercion family (BRIDGE_VALUE_COERCION_V1 — coerce.rs).
    // Six converters + two tag-dispatching HOFs. Dispatchers carry three FnRef
    // slots — see `fn_arg_kinds()`. Resolution is identity-keyed = sha256(name).
    m.insert("int_to_dec",      "axis_codegen_bridge::runtime::coerce::int_to_dec");
    m.insert("dec_id",          "axis_codegen_bridge::runtime::coerce::dec_id");
    m.insert("float_to_dec",    "axis_codegen_bridge::runtime::coerce::float_to_dec");
    m.insert("int_to_float",    "axis_codegen_bridge::runtime::coerce::int_to_float");
    m.insert("dec_to_float",    "axis_codegen_bridge::runtime::coerce::dec_to_float");
    m.insert("float_id",        "axis_codegen_bridge::runtime::coerce::float_id");
    m.insert("bridge_to_dec",   "axis_codegen_bridge::runtime::coerce::bridge_to_dec");
    m.insert("bridge_to_float", "axis_codegen_bridge::runtime::coerce::bridge_to_float");

    // IR constructors / accessors (kept for backward compat)
    m.insert("ir_make_int_lit",  "axis_codegen_bridge::runtime::ir_constructors::ir_make_int_lit");
    m.insert("ir_make_bool_lit", "axis_codegen_bridge::runtime::ir_constructors::ir_make_bool_lit");
    m.insert("ir_make_unit_lit", "axis_codegen_bridge::runtime::ir_constructors::ir_make_unit_lit");
    m.insert("ir_make_var",      "axis_codegen_bridge::runtime::ir_constructors::ir_make_var");
    m.insert("ir_make_lam",      "axis_codegen_bridge::runtime::ir_constructors::ir_make_lam");
    m.insert("ir_make_let",      "axis_codegen_bridge::runtime::ir_constructors::ir_make_let");
    m.insert("ir_make_if",       "axis_codegen_bridge::runtime::ir_constructors::ir_make_if");
    m.insert("ir_make_app",      "axis_codegen_bridge::runtime::ir_constructors::ir_make_app");
    m.insert("ir_make_call",     "axis_codegen_bridge::runtime::ir_constructors::ir_make_call");
    m.insert("ir_write_bundle",  "axis_codegen_bridge::runtime::ir_constructors::ir_write_bundle");
    m.insert("ir_read_bundle",   "axis_codegen_bridge::runtime::ir_constructors::ir_read_bundle");
    m.insert("ir_bundle_view",   "axis_codegen_bridge::runtime::ir_constructors::ir_bundle_view");
    m.insert("ir_subst",         "axis_codegen_bridge::runtime::ir_constructors::ir_subst");
    m.insert("ir_rename",        "axis_codegen_bridge::runtime::ir_constructors::ir_rename");
    m.insert("ir_free_vars",     "axis_codegen_bridge::runtime::ir_constructors::ir_free_vars");
    m.insert("ir_eval",          "axis_codegen_bridge::runtime::ir_eval::ir_eval");
    m.insert("ir_apply",         "axis_codegen_bridge::runtime::ir_eval::ir_apply");
    m.insert("ir_get_kind",      "axis_codegen_bridge::runtime::ir_accessors::ir_get_kind");
    m.insert("ir_get_name",      "axis_codegen_bridge::runtime::ir_accessors::ir_get_name");
    m.insert("ir_get_int_val",   "axis_codegen_bridge::runtime::ir_accessors::ir_get_int_val");
    m.insert("ir_get_fn",        "axis_codegen_bridge::runtime::ir_accessors::ir_get_fn");
    m.insert("ir_get_arg",       "axis_codegen_bridge::runtime::ir_accessors::ir_get_arg");
    m.insert("ir_get_body",      "axis_codegen_bridge::runtime::ir_accessors::ir_get_body");
    m.insert("ir_get_value",     "axis_codegen_bridge::runtime::ir_accessors::ir_get_value");
    m.insert("ir_get_cond",      "axis_codegen_bridge::runtime::ir_accessors::ir_get_cond");
    m.insert("ir_get_then",      "axis_codegen_bridge::runtime::ir_accessors::ir_get_then");
    m.insert("ir_get_else",      "axis_codegen_bridge::runtime::ir_accessors::ir_get_else");

    // ── Hash (BRIDGE_HASH_PRIMITIVE_M1 — resolves hld:axverity-hash-dependency) ─
    m.insert("content_hash",  "axis_codegen_bridge::runtime::hash::content_hash");
    m.insert("hash256_parse", "axis_codegen_bridge::runtime::hash::hash256_parse");

    // ── Bytes I/O (BRIDGE_BYTES_IO_M1 — resolves hld:axverity-text-to-bytes-dependency) ─
    m.insert("text_to_bytes",        "axis_codegen_bridge::runtime::bytes_io::text_to_bytes");
    m.insert("fs_write_bytes",       "axis_codegen_bridge::runtime::bytes_io::fs_write_bytes");
    m.insert("fs_read_bytes",        "axis_codegen_bridge::runtime::bytes_io::fs_read_bytes");
    // ── Seek / range read (seek.rs — BRIDGE_SEEK_V1, spike:axverity-spike1) ─────
    m.insert("fs_read_range",        "axis_codegen_bridge::runtime::seek::fs_read_range");
    // ── Log buffer (logbuf.rs — BRIDGE_LOGBUF_V1, spike:axverity-spike1) ────────
    // ── mmap-backed durable append segment (mmapseg.rs — AXVERITY_STORAGE_SUBSTRATE_DURABILITY_V1) ──
    m.insert("mmapseg_open",         "axis_codegen_bridge::runtime::mmapseg::mmapseg_open");
    m.insert("mmapseg_append",       "axis_codegen_bridge::runtime::mmapseg::mmapseg_append");
    m.insert("mmapseg_msync",        "axis_codegen_bridge::runtime::mmapseg::mmapseg_msync");
    m.insert("mmapseg_read",         "axis_codegen_bridge::runtime::mmapseg::mmapseg_read");
    m.insert("mmapseg_frontier",     "axis_codegen_bridge::runtime::mmapseg::mmapseg_frontier");
    m.insert("mmapseg_flush_file",   "axis_codegen_bridge::runtime::mmapseg::mmapseg_flush_file");
    m.insert("logbuf_open",          "axis_codegen_bridge::runtime::logbuf::logbuf_open");
    m.insert("logbuf_append",        "axis_codegen_bridge::runtime::logbuf::logbuf_append");
    m.insert("logbuf_sync",          "axis_codegen_bridge::runtime::logbuf::logbuf_sync");
    m.insert("logbuf_flush",         "axis_codegen_bridge::runtime::logbuf::logbuf_flush");
    m.insert("wal_fast_batch_write", "axis_codegen_bridge::runtime::logbuf::wal_fast_batch_write");
    m.insert("logbuf_read",          "axis_codegen_bridge::runtime::logbuf::logbuf_read");
    m.insert("logbuf_len",           "axis_codegen_bridge::runtime::logbuf::logbuf_len");
    m.insert("bytes_hash",           "axis_codegen_bridge::runtime::bytes_io::bytes_hash");
    m.insert("fs_mkdir_p",           "axis_codegen_bridge::runtime::bytes_io::fs_mkdir_p");
    m.insert("bytes_to_text",        "axis_codegen_bridge::runtime::bytes_io::bytes_to_text");

    // ── Background indexer (indexer.rs — AXVERITY_INDEXER_THREADING_V1) ───────
    m.insert("index_build_batch",    "axis_codegen_bridge::runtime::indexer::index_build_batch");
    m.insert("idxseg_lookup",        "axis_codegen_bridge::runtime::indexer::idxseg_lookup");
    m.insert("index_rebuild_dir",    "axis_codegen_bridge::runtime::indexer::index_rebuild_dir");

    // ── Live-path hot-block seal (AXVERITY_FRONTEND_WRITEPATH_INTEGRATION_V1) ──
    //    hotblk_* = per-thread accumulator register (dumb persistence, no logic);
    //    block_flush_write = the async seal-flush wait() handler (I/O glue).
    m.insert("hotblk_get",           "axis_codegen_bridge::runtime::hotblk::hotblk_get");
    m.insert("hotblk_set",           "axis_codegen_bridge::runtime::hotblk::hotblk_set");

    // ── ISOLATION MEASUREMENT ONLY (HOTWRITE_ADMISSION_MINIMAL_CAPTURE_V1) ──
    //    single-call collapse of the per-record capture/stamp/hash/write cycle.
    m.insert("hotwrite_batch_run",   "axis_codegen_bridge::runtime::hotwrite_batch::hotwrite_batch_run");
    m.insert("hotwrite_batch_run_c", "axis_codegen_bridge::runtime::hotwrite_batch::hotwrite_batch_run_c");
    m.insert("hotwrite_batch_run_c_durable", "axis_codegen_bridge::runtime::hotwrite_batch::hotwrite_batch_run_c_durable");
    m.insert("block_flush_write",    "axis_codegen_bridge::runtime::block_flush::block_flush_write");

    // ── SLA-tiered block writer, PHASE A ISOLATED — NOT wired into any live
    //    path (slablock.rs — BRIDGE_SLABLOCK_V1,
    //    AXVERITY_RECLOG_SLA_BLOCK_BUILD_PHASE_A_V1; cutover requires a
    //    separate intent) ────────────────────────────────────────────────────
    m.insert("slab_open",            "axis_codegen_bridge::runtime::slablock::slab_open");
    m.insert("slab_append",          "axis_codegen_bridge::runtime::slablock::slab_append");
    m.insert("slab_tick",            "axis_codegen_bridge::runtime::slablock::slab_tick");
    m.insert("slab_seal",            "axis_codegen_bridge::runtime::slablock::slab_seal");
    m.insert("slab_stats",           "axis_codegen_bridge::runtime::slablock::slab_stats");
    m.insert("slab_sealed",          "axis_codegen_bridge::runtime::slablock::slab_sealed");
    // ── Shadow validation tap (slabshadow.rs — BRIDGE_SLABSHADOW_V1,
    //    AXVERITY_RECLOG_SLA_BLOCK_SHADOW_VALIDATION_V1): measurement only.
    //    submit = post-ack drop-on-full try-send from pg_exec_insert (env-gated
    //    AXVERITY_SLAB_SHADOW=1); flush_once = the shadow janitor round. The
    //    ack path gates on reclog alone, unchanged. ──────────────────────────
    m.insert("slab_shadow_submit",     "axis_codegen_bridge::runtime::slabshadow::slab_shadow_submit");
    m.insert("slab_shadow_flush_once", "axis_codegen_bridge::runtime::slabshadow::slab_shadow_flush_once");

    // ── TEMPORARY timing instrumentation (AXVERITY_INSERT_PATH_TIMING_AUDIT_V1) ─
    m.insert("ts_mark",              "axis_codegen_bridge::runtime::tsmark::ts_mark");
    m.insert("ts_mark_val",          "axis_codegen_bridge::runtime::tsmark::ts_mark_val");
    m.insert("ts_flush",             "axis_codegen_bridge::runtime::tsmark::ts_flush");
    m.insert("channel_depth",        "axis_codegen_bridge::runtime::channels::channel_depth");

    // ── Hot mem arena (hotmem.rs — AXVERITY_HOTMEM_CONSUMER_IMPLEMENTATION_V1,
    //    first slice) ────────────────────────────────────────────────────────
    m.insert("hotmem_write",         "axis_codegen_bridge::runtime::hotmem::hotmem_write");
    m.insert("hotmem_reader_start",  "axis_codegen_bridge::runtime::hotmem::hotmem_reader_start");
    m.insert("hotmem_read",          "axis_codegen_bridge::runtime::hotmem::hotmem_read");
    m.insert("hotmem_epoch",         "axis_codegen_bridge::runtime::hotmem::hotmem_epoch");
    m.insert("hotmem_missed",        "axis_codegen_bridge::runtime::hotmem::hotmem_missed");

    // ── WAL segment pre-allocation (prealloc.rs — BRIDGE_WAL_SEG_ALLOC_V1,
    //    AXVERITY_WAL_ALLOCATION_AND_BLOB_PATH Landing A) ─────────────────────
    m.insert("fs_prealloc",          "axis_codegen_bridge::runtime::prealloc::fs_prealloc");
    m.insert("wal_write_seg",        "axis_codegen_bridge::runtime::prealloc::wal_write_seg");
    // ── WAL internal index: hot thread-local shard + disposable batched
    //    snapshot (walindex.rs — BRIDGE_WALINDEX_V1, same landing) ────────────
    m.insert("walidx_open",          "axis_codegen_bridge::runtime::walindex::walidx_open");
    m.insert("walidx_insert",        "axis_codegen_bridge::runtime::walindex::walidx_insert");
    m.insert("walidx_has",           "axis_codegen_bridge::runtime::walindex::walidx_has");
    m.insert("walidx_get",           "axis_codegen_bridge::runtime::walindex::walidx_get");
    m.insert("walidx_snapshot",      "axis_codegen_bridge::runtime::walindex::walidx_snapshot");
    m.insert("walidx_rebuild",       "axis_codegen_bridge::runtime::walindex::walidx_rebuild");
    // ── pk-index: rebuildable (table,pk)->current-hash projection over the same
    //    shared frame-walk (pkindex.rs — BRIDGE_PKINDEX_V1,
    //    AXVERITY_UNIFIED_DURABLE_STREAMS_V1 phase 2). Replay-only, never fsynced.
    m.insert("pkidx_open",           "axis_codegen_bridge::runtime::pkindex::pkidx_open");
    m.insert("pkidx_has",            "axis_codegen_bridge::runtime::pkindex::pkidx_has");
    m.insert("pkidx_get",            "axis_codegen_bridge::runtime::pkindex::pkidx_get");
    m.insert("pkidx_rebuild",        "axis_codegen_bridge::runtime::pkindex::pkidx_rebuild");
    m.insert("contradicts_open",     "axis_codegen_bridge::runtime::contradicts::contradicts_open");
    m.insert("contradicts_rebuild",  "axis_codegen_bridge::runtime::contradicts::contradicts_rebuild");
    m.insert("contradicts_has",      "axis_codegen_bridge::runtime::contradicts::contradicts_has");
    m.insert("contradicts_any",      "axis_codegen_bridge::runtime::contradicts::contradicts_any");
    m.insert("contradicts_warm",     "axis_codegen_bridge::runtime::contradicts::contradicts_warm");
    m.insert("contradicts_any_warm", "axis_codegen_bridge::runtime::contradicts::contradicts_any_warm");

    // ── Recovery-log group commit + ack backpath
    //    (AXVERITY_HOTPATH_PARALLEL_DISPATCH_V1) ──────────────────────────────
    //    oneshot.rs — net-new single-fire completion primitive.
    m.insert("oneshot_new",          "axis_codegen_bridge::runtime::oneshot::oneshot_new");
    m.insert("oneshot_wait",         "axis_codegen_bridge::runtime::oneshot::oneshot_wait");
    m.insert("oneshot_signal",       "axis_codegen_bridge::runtime::oneshot::oneshot_signal");
    //    channels.rs — bounded, block-on-full channel (distinct from the
    //    existing unbounded Channel; existing one untouched).
    m.insert("bchan_send",           "axis_codegen_bridge::runtime::channels::bchan_send");
    m.insert("bchan_drain",          "axis_codegen_bridge::runtime::channels::bchan_drain");
    //    reclog.rs — the batched recovery-log writer (folds payload + PK bind).
    m.insert("reclog_submit",        "axis_codegen_bridge::runtime::reclog::reclog_submit");
    m.insert("reclog_flush_once",    "axis_codegen_bridge::runtime::reclog::reclog_flush_once");

    // ── WAL shard routing (walshard.rs — BRIDGE_WAL_SHARD_V1) ───────────────────
    m.insert("wal_shard_set",        "axis_codegen_bridge::runtime::walshard::wal_shard_set");
    m.insert("wal_shard_get",        "axis_codegen_bridge::runtime::walshard::wal_shard_get");
    m.insert("wal_shard_count",      "axis_codegen_bridge::runtime::walshard::wal_shard_count");

    // ── SQL-facing field index: hot thread-local shard + disposable batched
    //    snapshot (fieldidx.rs — BRIDGE_FIELDIDX_V1, AXVERITY_INSERT_PATH_FASTPATH)
    m.insert("fieldidx_open",        "axis_codegen_bridge::runtime::fieldidx::fieldidx_open");
    m.insert("fieldidx_insert",      "axis_codegen_bridge::runtime::fieldidx::fieldidx_insert");
    m.insert("fieldidx_get",         "axis_codegen_bridge::runtime::fieldidx::fieldidx_get");
    m.insert("fieldidx_snapshot",    "axis_codegen_bridge::runtime::fieldidx::fieldidx_snapshot");
    m.insert("fieldidx_rebuild",     "axis_codegen_bridge::runtime::fieldidx::fieldidx_rebuild");

    // ── Name-binding volatile head pointer: double-buffered toggle cell
    //    (nameptr.rs — BRIDGE_NAMEPTR_V1, AXVERITY_INSERT_PATH_FASTPATH Landing 2,
    //    intent:axverity-req-immutable-pointer / req-name-gitref) ──────────────
    m.insert("nameptr_set",          "axis_codegen_bridge::runtime::nameptr::nameptr_set");
    m.insert("nameptr_get",          "axis_codegen_bridge::runtime::nameptr::nameptr_get");

    // ── Content-defined chunker (chunk.rs — BRIDGE_CDC_V1, FastCDC,
    //    AXVERITY_LANDING_B_BLOB_CHUNKER) ──────────────────────────────────────
    m.insert("chunk_file",           "axis_codegen_bridge::runtime::chunk::chunk_file");

    // ── Byte codec (bytes_codec.rs — BRIDGE_BYTE_CODEC_V1) ──────────────────────
    m.insert("bytes_concat",         "axis_codegen_bridge::runtime::bytes_codec::bytes_concat");
    m.insert("bytes_len",            "axis_codegen_bridge::runtime::bytes_codec::bytes_len");
    m.insert("bytes_slice",          "axis_codegen_bridge::runtime::bytes_codec::bytes_slice");
    m.insert("int16_be_encode",      "axis_codegen_bridge::runtime::bytes_codec::int16_be_encode");
    m.insert("int16_be_decode",      "axis_codegen_bridge::runtime::bytes_codec::int16_be_decode");
    m.insert("int32_be_encode",      "axis_codegen_bridge::runtime::bytes_codec::int32_be_encode");
    m.insert("int32_be_decode",      "axis_codegen_bridge::runtime::bytes_codec::int32_be_decode");

    // ── TCP sockets (net.rs — BRIDGE_TCP_SOCKET_V1) ─────────────────────────────
    m.insert("tcp_listen",           "axis_codegen_bridge::runtime::net::tcp_listen");
    m.insert("tcp_listen_shared",    "axis_codegen_bridge::runtime::net::tcp_listen_shared");
    m.insert("tcp_connect",          "axis_codegen_bridge::runtime::net::tcp_connect");
    m.insert("tcp_accept",           "axis_codegen_bridge::runtime::net::tcp_accept");
    m.insert("tcp_read",             "axis_codegen_bridge::runtime::net::tcp_read");
    m.insert("tcp_write",            "axis_codegen_bridge::runtime::net::tcp_write");
    m.insert("tcp_close",            "axis_codegen_bridge::runtime::net::tcp_close");

    // ── Raw memory / atomic-cell primitives (rawmem.rs — AXVERITY_MEM_FOREIGN_FNS_V1)
    //    Unchecked, self-describing-handle. See registry/axis-mem-raw.axreg. ───
    m.insert("cell_new_raw",         "axis_codegen_bridge::runtime::rawmem::cell_new_raw");
    m.insert("cell_load_raw",        "axis_codegen_bridge::runtime::rawmem::cell_load_raw");
    m.insert("cell_cas_raw",         "axis_codegen_bridge::runtime::rawmem::cell_cas_raw");
    m.insert("mem_reserve_raw",      "axis_codegen_bridge::runtime::rawmem::mem_reserve_raw");
    m.insert("mem_write_raw",        "axis_codegen_bridge::runtime::rawmem::mem_write_raw");
    m.insert("mem_read_raw",         "axis_codegen_bridge::runtime::rawmem::mem_read_raw");
    m.insert("mem_free_raw",         "axis_codegen_bridge::runtime::rawmem::mem_free_raw");

    m
}

/// Build the identity → bridge path map.
/// Identity = sha256(canonical_name) for all §5b bridge built-ins.
fn bridge_builtin_map() -> HashMap<Hash256, &'static str> {
    let sym = symbol_map();
    let mut map = HashMap::new();
    for (name, path) in sym {
        let identity = sha256_bytes(name.as_bytes());
        map.insert(identity, path);
    }
    map
}

// ── Registry loading ─────────────────────────────────────────────────────────

/// Parse `--reg` files (axis 0.5 registry format) and return identity → name map.
///
/// Accepts both the axis registry format (`fn <name> / identity 0x<hex> / end`) and
/// the older bridge registry format (`fn <name> / arity / end`). If no explicit
/// `identity` line is present, computes sha256(name) per the §5b rule.
pub fn load_registry_identity_map(paths: &[String]) -> HashMap<Hash256, String> {
    let mut map = HashMap::new();
    for path in paths {
        let content = match std::fs::read_to_string(path) {
            Ok(c)  => c,
            Err(e) => { eprintln!("warning: could not read --reg {}: {}", path, e); continue; }
        };
        let mut current_name: Option<String> = None;
        let mut current_identity: Option<Hash256> = None;
        for line in content.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("fn ") {
                let name = rest.split_whitespace().next().unwrap_or("").to_string();
                if !name.is_empty() {
                    current_name = Some(name.clone());
                    current_identity = Some(sha256_bytes(name.as_bytes())); // §5b default
                }
            } else if let Some(rest) = t.strip_prefix("identity ") {
                let hex = rest.trim().trim_start_matches("0x");
                if let Ok(id) = crate::core_ir_05::hex_to_hash256(hex) {
                    current_identity = Some(id);
                }
            } else if t == "end" {
                if let (Some(name), Some(id)) = (current_name.take(), current_identity.take()) {
                    map.insert(id, name);
                }
            }
        }
        // handle files that end without "end" (old format)
        if let (Some(name), Some(id)) = (current_name, current_identity) {
            map.insert(id, name);
        }
    }
    map
}

// ── Bridge-independent async fact scan (BRIDGE_SCAN_INDEPENDENT) ──────────────

/// Facts the bridge extracts from `.axreg` files to dispatch async behaviour.
///
/// Deliberately self-contained: built by a text scan (below) that mirrors
/// [`load_registry_identity_map`]'s `read_to_string` + `lines()` shape, with
/// zero dependency on any external parser or shared struct (BRIDGE_SCAN_INDEPENDENT).
#[derive(Debug, Default, Clone)]
pub struct BridgeAsyncFacts {
    /// Declared channel names — from top-level `channel <name> / type <Ty> / end`.
    pub channels: std::collections::HashSet<String>,
    /// Declared channel element types, keyed by channel name (`type <Ty>` line).
    pub channel_types: HashMap<String, String>,
    /// Fns flagged `background` inside their `bridge_contract … end` block.
    pub background_fns: std::collections::HashSet<String>,
}

/// Scan `--reg` files for channel declarations and `background` fns.
///
/// Grammar (all blocks close with a bare `end`; nesting is tracked by state, not
/// a shared parser):
///
/// ```text
/// channel <name>          fn <name>
///   type <Ty>               …
/// end                       bridge_contract
///                             background
///                           end
///                         end
/// ```
pub fn scan_bridge_async(paths: &[String]) -> BridgeAsyncFacts {
    let mut facts = BridgeAsyncFacts::default();
    for path in paths {
        let content = match std::fs::read_to_string(path) {
            Ok(c)  => c,
            Err(e) => { eprintln!("warning: could not read --reg {}: {}", path, e); continue; }
        };
        scan_bridge_async_str(&content, &mut facts);
    }
    facts
}

/// Text-scan one registry document into `facts`. Split out from
/// [`scan_bridge_async`] so it is unit-testable without touching the filesystem.
fn scan_bridge_async_str(content: &str, facts: &mut BridgeAsyncFacts) {
    let mut current_fn: Option<String> = None;   // fn whose block we're inside
    let mut in_contract = false;                 // inside a `bridge_contract … end`
    let mut channel_name: Option<String> = None; // inside a `channel … end`
    let mut channel_ty: Option<String> = None;

    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("//") {
            continue;
        }
        if let Some(rest) = t.strip_prefix("fn ") {
            current_fn = rest.split_whitespace().next().map(|s| s.to_string());
            in_contract = false;
        } else if let Some(rest) = t.strip_prefix("channel ") {
            channel_name = rest.split_whitespace().next().map(|s| s.to_string());
            channel_ty = None;
        } else if t == "bridge_contract" {
            in_contract = true;
        } else if t == "background" {
            if in_contract {
                if let Some(name) = &current_fn {
                    facts.background_fns.insert(name.clone());
                }
            }
        } else if let Some(rest) = t.strip_prefix("type ") {
            // Only the channel-block form (`type <Ty>`) is meaningful here; the
            // top-level `type X = prim …` declaration carries an `=` and is skipped.
            if channel_name.is_some() && !rest.contains('=') {
                channel_ty = rest.split_whitespace().next().map(|s| s.to_string());
            }
        } else if t == "end" {
            // Close the innermost open block: channel, then bridge_contract, then fn.
            if let Some(name) = channel_name.take() {
                facts.channels.insert(name.clone());
                if let Some(ty) = channel_ty.take() {
                    facts.channel_types.insert(name, ty);
                }
            } else if in_contract {
                in_contract = false;
            } else {
                current_fn = None;
            }
        }
    }
}

// ── Arg-kind metadata (Phase 0: HOF callee-slot signatures) ──────────────────

/// Per-arg kind for a bridge fn.
///
/// `Data`  — an ordinary `Value` argument.
/// `FnRef` — a callee/predicate slot. Must be a Pool ref to a `Fn`-typed entry;
///           emits as a bare Rust fn path (resolved at emit time from the
///           pool entry's 32-byte identity payload).
///
/// Any bridge fn NOT listed in [`fn_arg_kinds`] defaults to all-`Data` — a
/// `Fn`-typed pool ref handed to such a fn fails the type gate. This is the
/// "Fn-as-data is unrepresentable" invariant (intent
/// BRIDGE_FOREIGN_FN_FNREF_M1, FN_REF_IS_CALLEE_ONLY).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArgKind {
    Data,
    FnRef,
}

/// Higher-order primitives — the *only* fns with Fn-typed arg slots.
///
/// A fn here gets emitted as a native multi-arg Rust call (e.g.
/// `foreach(pool_0.clone(), io_println)`) instead of the unary
/// `Value::Tuple`-packed call used for data-only fns.
fn fn_arg_kinds() -> HashMap<&'static str, Vec<ArgKind>> {
    use ArgKind::*;
    let mut m: HashMap<&'static str, Vec<ArgKind>> = HashMap::new();
    m.insert("foreach",    vec![Data, FnRef]);
    m.insert("flat_map",   vec![Data, FnRef]);
    m.insert("any",        vec![Data, FnRef]);
    m.insert("all",        vec![Data, FnRef]);
    m.insert("find_index", vec![Data, FnRef]);
    m.insert("count",      vec![Data, FnRef]);
    m.insert("loop_count", vec![Data, Data, FnRef]);
    m.insert("loop_while", vec![Data, FnRef, FnRef, Data]);
    // Value-coercion dispatchers — runtime tag dispatch over three FnRef arms
    // in positional Int/Dec/Float order (BRIDGE_VALUE_COERCION_V1).
    m.insert("bridge_to_dec",   vec![Data, FnRef, FnRef, FnRef]);
    m.insert("bridge_to_float", vec![Data, FnRef, FnRef, FnRef]);
    // Async: `wait` takes its handler in a single Fn callee slot. The handler is
    // invoked synchronously within wait's own frame (CLOSURE_RULE_HARD).
    m.insert("wait", vec![FnRef]);
    m
}

// ── Pool constant classification ─────────────────────────────────────────────

/// What a pool entry resolves to under Core IR 0.5.
///
/// `Data(expr)`  — emits `let pool_N: Value = <expr>;` and may be cloned into
///                 any data position.
/// `FnRef(path)` — a bridge symbol path. NO `let pool_N` is emitted. The path
///                 may appear ONLY in a CCall callee/predicate slot (an
///                 `ArgKind::FnRef` position). A reference from any data
///                 position is a HARD ERROR.
#[derive(Debug, Clone)]
enum PoolKind {
    Data(String),
    FnRef(String),
    /// A parameter slot. The payload of the pool entry is `varint(slot_index)`
    /// indicating which positional caller argument to substitute at codegen.
    /// Treated as a `Value` at runtime — fully usable in Data positions.
    Param(u32),
}

fn classify_pool_entry(
    entry: &ConstantPoolEntry,
    builtin: &HashMap<Hash256, &'static str>,
    registry: &HashMap<Hash256, String>,
    name_to_path: &HashMap<&'static str, &'static str>,
    xbundle: &HashMap<Hash256, String>,
) -> Result<PoolKind, String> {
    let dh = &entry.def_hash;
    if dh == &[0u8; 32] {
        return Err(
            "all-zero def_hash (UNKNOWN-gate sentinel): pool entry has no resolved \
             type identity. This is an upstream lowering gap — e.g. a lambda or \
             fn-reference that was not assigned a Fn type hash — not a bridge defect."
                .to_string(),
        );
    }
    if dh == &unit_type_hash() {
        return Ok(PoolKind::Data("Value::Unit".to_string()));
    }
    if dh == &bool_type_hash() {
        let v = decode_bool_payload(&entry.payload)?;
        return Ok(PoolKind::Data(format!("Value::Bool({})", v)));
    }
    if dh == &int_type_hash() {
        let v = decode_int_payload(&entry.payload)?;
        return Ok(PoolKind::Data(format!("Value::Int({})", v)));
    }
    if dh == &text_type_hash() {
        let s = decode_text_payload(&entry.payload)?;
        return Ok(PoolKind::Data(format!(
            "Value::Str(axis_codegen_bridge::runtime::value::intern_str({:?}))",
            s
        )));
    }
    if dh == &float_type_hash() {
        let v = crate::core_ir_05::decode_float_payload(&entry.payload)?;
        // Use the bit pattern so NaN/sign-preserving values round-trip exactly.
        return Ok(PoolKind::Data(format!(
            "Value::Float(f64::from_bits({}u64))",
            v.to_bits()
        )));
    }
    if dh == &dec_type_hash() {
        let v = crate::core_ir_05::decode_dec_payload(&entry.payload)?;
        // Emit via the 16-byte canonical deserialize form so the source is
        // representation-stable.
        let bytes = v.serialize();
        let bytes_lit = bytes
            .iter()
            .map(|b| format!("{}u8", b))
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(PoolKind::Data(format!(
            "Value::Dec(axis_codegen_bridge::runtime::value::Decimal::deserialize([{}]))",
            bytes_lit
        )));
    }
    if dh == &crate::core_ir_05::param_type_hash() {
        // Param slot — payload is an unsigned varint encoding the slot index.
        let slot = crate::core_ir_05::decode_unsigned_varint(&entry.payload)
            .map_err(|e| format!("Param pool entry: {}", e))?;
        let slot_u32 = u32::try_from(slot).map_err(|_| {
            format!("Param pool entry: slot index {} doesn't fit in u32", slot)
        })?;
        return Ok(PoolKind::Param(slot_u32));
    }
    if dh == &fn_type_hash() {
        if entry.payload.len() != 32 {
            return Err(format!(
                "Fn-typed pool entry has malformed payload: expected 32-byte identity, got {} bytes",
                entry.payload.len()
            ));
        }
        let mut id: Hash256 = [0u8; 32];
        id.copy_from_slice(&entry.payload);
        if let Some(&path) = builtin.get(&id) {
            return Ok(PoolKind::FnRef(path.to_string()));
        }
        // Composite M1 fn referenced as fn_ref: resolve to a safe wrapper
        // around the xbundle extern. The extern is `unsafe extern "C-unwind" fn`
        // and cannot coerce to the HOF's `fn(Value) -> Value` slot directly —
        // emit_rust_lib_from_bundle generates one `{sym}_xfn` wrapper per
        // xbundle fn-ref symbol; we point at the wrapper here.
        if let Some(sym) = xbundle.get(&id) {
            return Ok(PoolKind::FnRef(format!("{}_xfn", sym)));
        }
        if let Some(name) = registry.get(&id) {
            if let Some(&path) = name_to_path.get(name.as_str()) {
                return Ok(PoolKind::FnRef(path.to_string()));
            }
            return Err(format!(
                "Fn-typed pool entry resolves to registry name '{}' (identity {}) but \
                 that name has no bridge implementation and no xbundle provider",
                name,
                hash256_to_hex(&id)
            ));
        }
        return Err(format!(
            "Fn-typed pool entry references unknown identity {} — \
             not a bridge built-in, not in --reg files, not in --lib providers",
            hash256_to_hex(&id)
        ));
    }
    Err(format!(
        "unknown pool entry type hash: {} (only Unit/Bool/Int/Text/Fn supported)",
        hash256_to_hex(dh)
    ))
}

// ── Node reference expression ────────────────────────────────────────────────

fn ref_expr(r: &NodeRef) -> String {
    match r {
        NodeRef::Node(i) => format!("node_{}", i),
        NodeRef::Pool(i) => format!("pool_{}", i),
    }
}

fn ref_clone(r: &NodeRef) -> String {
    format!("{}.clone()", ref_expr(r))
}

// ── Branch scoping (BRANCH_SCOPING_V1) ───────────────────────────────────────
//
// The flat node list would, if emitted as a straight-line sequence of
// unconditional `let node_N = <expr>;` statements, execute a `CIf`'s then_
// AND else_ subtrees on every call — including any side-effecting CCall
// inside the branch that isn't taken. `CIf` itself only ever *selects*
// between two already-materialized values; it never gates their computation.
//
// To fix this without changing the IR shape (branches are still ordinary
// nodes in the same flat list), the emitter computes, per node, the tightest
// enclosing (CIf, arm) context that ALL of its uses agree on, and only
// hoists a node to the unconditional top-level prelude when at least one use
// requires it there (shared between both arms, referenced by `cond`, or used
// outside any `CIf`). A node whose every use lives inside one arm of one
// `CIf` is emitted as a `let` *inside that arm's Rust block*, so it only runs
// when that arm is actually taken.
//
// This is possible in one descending pass because `NodeRef::Node(i)` inside
// node j always has i < j (forward references are a hard verifier invariant
// — see core_ir_05.rs: "Invariant: for every NodeRef::Node(i) inside node at
// index j, i < j"), so by the time we compute node i's required scope, every
// consumer j (j > i) already has its own scope resolved.

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Branch {
    Then,
    Else,
}

/// A node's required scope: the chain of (CIf node index, arm) it must be
/// nested inside, outermost first. An empty path means "top-level,
/// unconditional".
type ScopePath = Vec<(u32, Branch)>;

enum Use {
    /// This node is the bundle's final result — always required at top level.
    Result,
    /// Used by node `j` in a non-branching position (CCall arg, or CIf cond)
    /// — required wherever `j` itself is required.
    Same(u32),
    /// Used as the `then_`/`else_` value of CIf `j` — required strictly
    /// inside that arm of `j`.
    ThenArm(u32),
    ElseArm(u32),
}

fn longest_common_prefix(paths: &[ScopePath]) -> ScopePath {
    let mut iter = paths.iter();
    let mut prefix = match iter.next() {
        Some(p) => p.clone(),
        None => return Vec::new(),
    };
    for p in iter {
        let common = prefix.iter().zip(p.iter()).take_while(|(a, b)| a == b).count();
        prefix.truncate(common);
        if prefix.is_empty() {
            break;
        }
    }
    prefix
}

/// Compute each node's required `ScopePath`, processed from the last node
/// down to the first so every consumer's scope is already known.
///
/// Errs on the one shape this analysis cannot soundly resolve: a
/// zero-consumer ("orphan") node — a discarded value, e.g. `let _ = eff();
/// tail` — sitting inside a `CIf` whose `then_` is a bare pool ref (no
/// anchor to pin exactly where "then" ends and "else" begins). As of
/// 2026-07-15 M1's `IfExpr` branches ARE `Body` (they can hold a let-chain),
/// but the compiler now threads every discarded branch effect into the arm's
/// result via `seq(eff, result)` (`nf_lowering.rs` `seq_scope_arm_effects`),
/// so a branch orphan is given a consumer edge before it ever reaches this
/// analysis — no orphan-in-branch is emitted from M1. This check therefore
/// stays dormant for M1 today, and exists so that if some producer ever emits
/// a raw un-threaded orphan under a pool-ref branch, the build fails loudly
/// right here instead of silently hoisting it to top level and reintroducing
/// BRANCH_SCOPING_V1's exact bug for that one node.
fn compute_branch_paths(bundle: &CoreBundle) -> Result<Vec<ScopePath>, String> {
    let n = bundle.nodes.len();
    let mut uses: Vec<Vec<Use>> = (0..n).map(|_| Vec::new()).collect();
    // `bundle.result` — not "the last node" — is the authoritative root (see
    // BUG2_RESULT_FIELD_V1). A Pool ref needs no Use entry: pool entries are
    // always hoisted constants, never branch-scoped.
    if let NodeRef::Node(i) = bundle.result {
        uses[i as usize].push(Use::Result);
    }
    for (j, node) in bundle.nodes.iter().enumerate() {
        match node {
            Node::CCall { args, .. } => {
                for a in args {
                    if let NodeRef::Node(i) = a {
                        uses[*i as usize].push(Use::Same(j as u32));
                    }
                }
            }
            Node::CIf { cond, then_, else_ } => {
                if let NodeRef::Node(i) = cond {
                    uses[*i as usize].push(Use::Same(j as u32));
                }
                if let NodeRef::Node(i) = then_ {
                    uses[*i as usize].push(Use::ThenArm(j as u32));
                }
                if let NodeRef::Node(i) = else_ {
                    uses[*i as usize].push(Use::ElseArm(j as u32));
                }
            }
            Node::CDeterminate => {}
        }
    }

    // BRANCH_SCOPING_V1 tripwire: an orphan node positioned after `cond`'s
    // own anchor and before a `CIf` whose `then_` has no anchor cannot be
    // soundly attributed to `then` vs `else` by position alone (see doc
    // comment above). Check this BEFORE computing `path`, since an orphan
    // always defaults to top-level there regardless — the point is to
    // refuse to build rather than silently accept the ambiguous shape.
    for (k, node) in bundle.nodes.iter().enumerate() {
        if let Node::CIf { cond, then_, .. } = node {
            if !matches!(then_, NodeRef::Node(_)) {
                if let NodeRef::Node(cond_anchor) = cond {
                    for i in (*cond_anchor as usize + 1)..k {
                        if uses[i].is_empty() {
                            return Err(format!(
                                "BRANCH_SCOPING_V1: node[{i}] has no data-dependency consumer \
                                 (a discarded value) and sits between CIf node[{k}]'s cond and \
                                 its own position, but node[{k}]'s `then_` is a bare pool ref with \
                                 no anchor to pin the then/else boundary — whether node[{i}] belongs \
                                 to the then-arm or the else-arm cannot be determined from graph \
                                 position alone. Refusing to build rather than silently defaulting \
                                 node[{i}] to unconditional top-level emission (which could re-run \
                                 its effect in the wrong branch, or on every call). This bundle shape \
                                 is not reachable from M1 today; if a new producer emits it, extend \
                                 the branch-scoping analysis (or the IR) to carry explicit sequencing \
                                 before removing this check.",
                                i = i, k = k
                            ));
                        }
                    }
                }
            }
        }
    }

    let mut path: Vec<ScopePath> = (0..n).map(|_| Vec::new()).collect();
    for i in (0..n).rev() {
        let required: Vec<ScopePath> = uses[i]
            .iter()
            .map(|u| match u {
                Use::Result => Vec::new(),
                Use::Same(j) => path[*j as usize].clone(),
                Use::ThenArm(j) => {
                    let mut p = path[*j as usize].clone();
                    p.push((*j, Branch::Then));
                    p
                }
                Use::ElseArm(j) => {
                    let mut p = path[*j as usize].clone();
                    p.push((*j, Branch::Else));
                    p
                }
            })
            .collect();
        // A node with no recorded uses has no consumer edge to anchor a
        // scope to. The tripwire above already refused any case where this
        // matters; anything reaching here is a genuinely safe top-level
        // orphan (e.g. a discarded value preceding an unrelated `CIf`).
        path[i] = longest_common_prefix(&required);
    }
    Ok(path)
}

/// Group node indices by the innermost scope they were assigned — `None` is
/// the unconditional top-level prelude; `Some((k, arm))` is the direct
/// contents of that arm of CIf `k`. Order within each group is ascending by
/// index, matching the bundle's required topological order.
fn group_by_scope(n: usize, path: &[ScopePath]) -> HashMap<Option<(u32, Branch)>, Vec<usize>> {
    let mut groups: HashMap<Option<(u32, Branch)>, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let key = path[i].last().copied();
        groups.entry(key).or_default().push(i);
    }
    groups
}

/// Render every node assigned to scope `key` as Rust `let` statements.
/// `CIf` nodes recurse into their own Then/Else scopes, so a node deferred
/// into a branch is only ever computed when that branch actually runs.
#[allow(clippy::too_many_arguments)]
fn render_scope(
    key: Option<(u32, Branch)>,
    groups: &HashMap<Option<(u32, Branch)>, Vec<usize>>,
    bundle: &CoreBundle,
    pool_kinds: &[PoolKind],
    arg_kind_table: &HashMap<&'static str, Vec<ArgKind>>,
    builtin: &HashMap<Hash256, &'static str>,
    registry: &HashMap<Hash256, String>,
    name_to_path: &HashMap<&'static str, &'static str>,
    xbundle: &HashMap<Hash256, String>,
) -> Result<String, String> {
    let mut out = String::new();
    let Some(indices) = groups.get(&key) else { return Ok(out) };
    for &i in indices {
        match &bundle.nodes[i] {
            Node::CIf { cond, then_, else_ } => {
                // cond / then / else are Data positions. A Fn-typed pool ref
                // here would be "Fn as data" — reject (same gate as before).
                for (label, r) in &[("cond", cond), ("then", then_), ("else", else_)] {
                    if let NodeRef::Pool(pi) = r {
                        let pi_us = *pi as usize;
                        if let Some(PoolKind::FnRef(_)) = pool_kinds.get(pi_us) {
                            return Err(format!(
                                "type gate: CIf {} slot is Data but pool[{}] is Fn-typed — \
                                 Fn refs are callee-only, never condition or branch value \
                                 (FN_REF_IS_CALLEE_ONLY)",
                                label, pi
                            ));
                        }
                    }
                }
                let then_body = render_scope(
                    Some((i as u32, Branch::Then)), groups, bundle, pool_kinds,
                    arg_kind_table, builtin, registry, name_to_path, xbundle,
                )?;
                let else_body = render_scope(
                    Some((i as u32, Branch::Else)), groups, bundle, pool_kinds,
                    arg_kind_table, builtin, registry, name_to_path, xbundle,
                )?;
                out.push_str(&format!(
                    "    let node_{i}: Value = if axis_codegen_bridge::runtime::value::truthy(&{cond}) {{\n\
                     {then_body}        {then_tail}\n    }} else {{\n\
                     {else_body}        {else_tail}\n    }};\n",
                    i = i,
                    cond = ref_expr(cond),
                    then_body = then_body,
                    then_tail = ref_clone(then_),
                    else_body = else_body,
                    else_tail = ref_clone(else_),
                ));
            }
            other => {
                let expr = emit_node(
                    other, pool_kinds, arg_kind_table, builtin, registry, name_to_path, xbundle,
                )
                .map_err(|e| format!("node[{}]: {}", i, e))?;
                out.push_str(&format!("    let node_{}: Value = {};\n", i, expr));
            }
        }
    }
    Ok(out)
}

// ── Node emission ─────────────────────────────────────────────────────────────

fn emit_node(
    node: &Node,
    pool_kinds: &[PoolKind],
    arg_kind_table: &HashMap<&'static str, Vec<ArgKind>>,
    builtin: &HashMap<Hash256, &'static str>,
    registry: &HashMap<Hash256, String>,
    name_to_path: &HashMap<&'static str, &'static str>,
    xbundle: &HashMap<Hash256, String>,
) -> Result<String, String> {
    match node {
        Node::CCall { target_identity, args, target_name } => {
            // Resolve target → (name, callable_path, is_extern). target_name is
            // mandatory per Core IR 0.5 §"Human Display Format" / CCall.
            let (name, path, is_extern): (String, String, bool) =
                if let Some(&p) = builtin.get(target_identity) {
                    (target_name.clone(), p.to_string(), false)
                } else if let Some(n) = registry.get(target_identity) {
                    // Registry has the name. Prefer the Rust impl (leaf path).
                    // If there's no Rust impl, fall through to xbundle — the fn
                    // may be a composite whose body lives in a --lib provider.
                    // Mirrors classify_pool_entry's FnRef path
                    // (FNREF_COMPOSITE_RESOLVER_v0.1).
                    if let Some(&p) = name_to_path.get(n.as_str()) {
                        (n.clone(), p.to_string(), false)
                    } else if let Some(sym) = xbundle.get(target_identity) {
                        (n.clone(), sym.clone(), true)
                    } else {
                        return Err(format!(
                            "CCall identity {} resolves to registry name '{}' but \
                             that name has no bridge implementation and no \
                             xbundle provider",
                            hash256_to_hex(target_identity),
                            n
                        ));
                    }
                } else if let Some(sym) = xbundle.get(target_identity) {
                    (target_name.clone(), sym.clone(), true)
                } else {
                    return Err(format!(
                        "unresolved CCall identity: {} — not in bridge built-ins, --reg files, or --lib providers",
                        hash256_to_hex(target_identity)
                    ));
                };

            // Per-arg kind. Default = all Data; a fn with any Fn-slot MUST
            // appear in [`fn_arg_kinds`].
            let declared = arg_kind_table.get(name.as_str());
            if let Some(kinds) = declared {
                if kinds.len() != args.len() {
                    return Err(format!(
                        "CCall '{}' arg count mismatch: declared {} arg-kinds, got {} args",
                        name, kinds.len(), args.len()
                    ));
                }
            }
            let kinds_owned: Vec<ArgKind> =
                declared.cloned().unwrap_or_else(|| vec![ArgKind::Data; args.len()]);

            // Type gate + per-arg expression.
            let mut arg_exprs: Vec<String> = Vec::with_capacity(args.len());
            let mut any_fn_ref = false;
            for (i, arg) in args.iter().enumerate() {
                match kinds_owned[i] {
                    ArgKind::FnRef => {
                        any_fn_ref = true;
                        match arg {
                            NodeRef::Pool(pi) => {
                                let pi_us = *pi as usize;
                                match pool_kinds.get(pi_us) {
                                    Some(PoolKind::FnRef(path)) => arg_exprs.push(path.clone()),
                                    Some(PoolKind::Data(_)) => return Err(format!(
                                        "type gate: CCall '{}' arg[{}] expects Fn but pool[{}] is Data",
                                        name, i, pi
                                    )),
                                    Some(PoolKind::Param(_)) => return Err(format!(
                                        "type gate: CCall '{}' arg[{}] expects Fn but pool[{}] is a Param slot \
                                         (Fn-by-name resolution requires a statically known identity, \
                                         not a runtime Value)",
                                        name, i, pi
                                    )),
                                    None => return Err(format!(
                                        "CCall '{}' arg[{}]: pool[{}] out of range",
                                        name, i, pi
                                    )),
                                }
                            }
                            NodeRef::Node(j) => return Err(format!(
                                "type gate: CCall '{}' arg[{}] expects Fn but got node[{}] result — \
                                 Fn refs originate from pool entries only",
                                name, i, j
                            )),
                        }
                    }
                    ArgKind::Data => {
                        if let NodeRef::Pool(pi) = arg {
                            let pi_us = *pi as usize;
                            if let Some(PoolKind::FnRef(_)) = pool_kinds.get(pi_us) {
                                return Err(format!(
                                    "type gate: CCall '{}' arg[{}] is a Data slot but pool[{}] is \
                                     Fn-typed — Fn refs are callee-only, never data \
                                     (FN_REF_IS_CALLEE_ONLY)",
                                    name, i, pi
                                ));
                            }
                        }
                        arg_exprs.push(ref_clone(arg));
                    }
                }
            }

            // Calling convention:
            //   any FnRef arg → native multi-arg Rust call (`f(a, b, c)`)
            //   else          → existing Value::Tuple-packed call (data UNARY_INVARIANT)
            let body = if any_fn_ref {
                format!("{}({})", path, arg_exprs.join(", "))
            } else {
                match arg_exprs.len() {
                    0 => format!("{}(Value::Unit)", path),
                    1 => format!("{}({})", path, arg_exprs[0]),
                    _ => format!("{}(Value::Tuple(vec![{}]))", path, arg_exprs.join(", ")),
                }
            };
            Ok(if is_extern { format!("unsafe {{ {} }}", body) } else { body })
        }
        Node::CIf { cond, then_, else_ } => {
            // cond / then / else are Data positions. A Fn-typed pool ref here
            // would be "Fn as data" — reject.
            for (label, r) in &[("cond", cond), ("then", then_), ("else", else_)] {
                if let NodeRef::Pool(pi) = r {
                    let pi_us = *pi as usize;
                    if let Some(PoolKind::FnRef(_)) = pool_kinds.get(pi_us) {
                        return Err(format!(
                            "type gate: CIf {} slot is Data but pool[{}] is Fn-typed — \
                             Fn refs are callee-only, never condition or branch value \
                             (FN_REF_IS_CALLEE_ONLY)",
                            label, pi
                        ));
                    }
                }
            }
            Ok(format!(
                "if axis_codegen_bridge::runtime::value::truthy(&{}) {{ {} }} else {{ {} }}",
                ref_expr(cond),
                ref_clone(then_),
                ref_clone(else_)
            ))
        }
        // A determinacy gate has no operands and yields a Unit discharge token.
        Node::CDeterminate => Ok("Value::Unit".to_string()),
    }
}

/// Return true if `identity` resolves to a bridge built-in (not a §5b user fn).
pub fn is_bridge_builtin(identity: &Hash256) -> bool {
    bridge_builtin_map().contains_key(identity)
}

/// Return the bridge runtime path for a built-in identity, or None if not a built-in.
pub fn builtin_path_for_identity(identity: &Hash256) -> Option<&'static str> {
    bridge_builtin_map().get(identity).copied()
}

/// Parse `--reg` files and return identity → in-clause string (e.g. "(TextList)").
///
/// Used at build time to validate the ABI of foreign-fn entries (ENTRY_ABI_MISMATCH check).
/// Falls back to computing sha256(name) as the identity when no explicit `identity` line is present.
pub fn load_registry_in_map(paths: &[String]) -> HashMap<Hash256, String> {
    let mut map = HashMap::new();
    for path in paths {
        let content = match std::fs::read_to_string(path) {
            Ok(c)  => c,
            Err(e) => { eprintln!("warning: could not read --reg {}: {}", path, e); continue; }
        };
        let mut current_identity: Option<Hash256> = None;
        for line in content.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("fn ") {
                let name = rest.split_whitespace().next().unwrap_or("").to_string();
                if !name.is_empty() {
                    current_identity = Some(sha256_bytes(name.as_bytes()));
                }
            } else if let Some(rest) = t.strip_prefix("identity ") {
                let hex = rest.trim().trim_start_matches("0x");
                if let Ok(id) = crate::core_ir_05::hex_to_hash256(hex) {
                    current_identity = Some(id);
                }
            } else if let Some(rest) = t.strip_prefix("in ") {
                if let Some(id) = current_identity {
                    map.insert(id, rest.trim().to_string());
                }
            } else if t == "end" {
                current_identity = None;
            }
        }
    }
    map
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Sanitise a name into a valid Rust identifier.
pub fn sanitise(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    if s.is_empty() {
        s = "_bundle".to_string();
    }
    s
}

/// Generate a Rust library source file from a 0.5 CoreBundle.
///
/// `fn_name` is the public symbol name for the generated Rust function.
/// `registry_identity_map` maps CCall target identities (from `--reg` files) to function names.
/// `xbundle_providers` maps §5b target identities to their identity-derived extern symbol names
///   (`ax_fn_<64hex>`). Populated from `--lib` / `--lib-dir` bundles by the driver.
///
/// The generated library exposes:
///   `#[no_mangle] pub extern "C" fn <fn_name>(args: Value) -> Value`
///   `#[no_mangle] pub extern "C" fn ax_fn_<hex>(args: Value) -> Value`  ← identity export
///   `#[no_mangle] pub extern "C" fn _ax_exe_<fn_name>(args: Value) -> Value`
pub fn emit_rust_lib_from_bundle(
    bundle: &CoreBundle,
    fn_name: &str,
    registry_identity_map: &HashMap<Hash256, String>,
    xbundle_providers: &HashMap<Hash256, String>,
    declared_channels: &std::collections::HashSet<String>,
) -> Result<String, String> {
    let builtin = bridge_builtin_map();
    let name_to_path = symbol_map();
    let safe_name = sanitise(fn_name);

    let mut out = String::new();
    out.push_str("extern crate axis_codegen_bridge;\n");
    out.push_str("#[allow(unused_imports)]\n");
    out.push_str(
        "use axis_codegen_bridge::runtime::value::{Value, truthy, intern_str, init_runtime};\n\n",
    );

    // Validate: check all CCall targets are resolvable before generating any code
    let mut errors: Vec<String> = Vec::new();
    for node in &bundle.nodes {
        if let Node::CCall { target_identity, target_name, .. } = node {
            if builtin.contains_key(target_identity) {
                // OK: bridge built-in
            } else if let Some(name) = registry_identity_map.get(target_identity) {
                // Prefer Rust impl; else fall through to xbundle (composite
                // body via --lib). Symmetric with classify_pool_entry's
                // FnRef path (FNREF_COMPOSITE_RESOLVER_v0.1).
                if !name_to_path.contains_key(name.as_str())
                    && !xbundle_providers.contains_key(target_identity)
                {
                    errors.push(format!(
                        "registry name '{}' (identity {}…) has no bridge \
                         implementation and no xbundle provider",
                        name,
                        &hash256_to_hex(target_identity)[..16]
                    ));
                }
            } else if xbundle_providers.contains_key(target_identity) {
                // OK: §5b extern — provider supplied via --lib
            } else if !target_name.is_empty()
                && sha256_bytes(target_name.as_bytes()) == *target_identity
            {
                errors.push(format!(
                    "UNRESOLVED_XBUNDLE: '{}' (identity {}…) — no provider in --lib set",
                    target_name,
                    &hash256_to_hex(target_identity)[..16]
                ));
            } else {
                errors.push(format!(
                    "UNKNOWN_GATE: identity {}… — not a bridge built-in, not a §5b identity",
                    &hash256_to_hex(target_identity)[..16]
                ));
            }
        }
    }
    if !errors.is_empty() {
        return Err(format!("unresolved CCall targets:\n  {}", errors.join("\n  ")));
    }

    // CHANNELS_STATIC: a `channel_send` whose name argument is a compile-time
    // literal Text must target a channel declared in the registry. An undeclared
    // name is a HARD ERROR at emit time — the same discipline used for Fn-as-data
    // — so it can never degrade to a silent runtime no-op. (A non-literal name is
    // not statically checkable and is left to the runtime; static topology means
    // channel names are literals in practice.)
    let channel_send_id = sha256_bytes(b"channel_send");
    for node in &bundle.nodes {
        if let Node::CCall { target_identity, args, .. } = node {
            if *target_identity != channel_send_id {
                continue;
            }
            if let Some(NodeRef::Pool(pi)) = args.first() {
                if let Some(entry) = bundle.constant_pool.get(*pi as usize) {
                    if entry.def_hash == text_type_hash() {
                        let cname = decode_text_payload(&entry.payload)
                            .map_err(|e| format!("channel_send: name literal: {}", e))?;
                        if !declared_channels.contains(&cname) {
                            return Err(format!(
                                "CHANNELS_STATIC: channel_send targets undeclared channel {:?} — \
                                 declare it with a `channel {} / type <Ty> / end` block in a \
                                 --reg registry file",
                                cname, cname
                            ));
                        }
                    }
                }
            }
        }
    }

    // Collect the distinct §5b extern symbols this bundle calls (for the extern block)
    let mut extern_syms: Vec<String> = Vec::new();
    let mut seen_extern: std::collections::HashSet<Hash256> = std::collections::HashSet::new();
    for node in &bundle.nodes {
        if let Node::CCall { target_identity, .. } = node {
            if let Some(sym) = xbundle_providers.get(target_identity) {
                if seen_extern.insert(*target_identity) {
                    extern_syms.push(sym.clone());
                }
            }
        }
    }
    // Also scan Fn-typed pool entries: an fn_ref to a composite needs its
    // xbundle symbol declared so the bare path is callable as a fn pointer.
    // We separately collect symbols that need a safe wrapper (fn-ref position)
    // because `unsafe extern "C-unwind" fn` cannot coerce to `fn(Value) -> Value`.
    let fn_th = fn_type_hash();
    let mut xbundle_fnref_syms: Vec<String> = Vec::new();
    let mut seen_xbundle_fnref: std::collections::HashSet<Hash256> = std::collections::HashSet::new();
    for entry in &bundle.constant_pool {
        if entry.def_hash != fn_th || entry.payload.len() != 32 {
            continue;
        }
        let mut id: Hash256 = [0u8; 32];
        id.copy_from_slice(&entry.payload);
        if let Some(sym) = xbundle_providers.get(&id) {
            if seen_extern.insert(id) {
                extern_syms.push(sym.clone());
            }
            if seen_xbundle_fnref.insert(id) {
                xbundle_fnref_syms.push(sym.clone());
            }
        }
    }

    // Emit extern block for §5b cross-bundle symbols.
    // "C-unwind" allows Rust panics to propagate across the ABI boundary so
    // catch_unwind in the multi-entry driver can isolate per-entry failures.
    if !extern_syms.is_empty() {
        out.push_str("#[allow(improper_ctypes)]\nextern \"C-unwind\" {\n");
        for sym in &extern_syms {
            out.push_str(&format!("    fn {}(args: Value) -> Value;\n", sym));
        }
        out.push_str("}\n\n");
    }

    // Safe wrappers for any xbundle symbol used as a HOF fn-ref. The HOFs
    // declare `pred: fn(Value) -> Value` (safe Rust ABI); the extern is unsafe.
    // Each wrapper is a one-liner that calls the extern inside `unsafe {}`.
    if !xbundle_fnref_syms.is_empty() {
        for sym in &xbundle_fnref_syms {
            out.push_str(&format!(
                "fn {sym}_xfn(args: Value) -> Value {{ unsafe {{ {sym}(args) }} }}\n",
                sym = sym
            ));
        }
        out.push_str("\n");
    }

    // Emit main function
    out.push_str(&format!(
        "#[no_mangle]\npub extern \"C-unwind\" fn {}(args: Value) -> Value {{\n",
        safe_name
    ));
    out.push_str("    init_runtime();\n");

    // Classify all pool entries up front. Fn-typed entries are NOT emitted as
    // `let pool_N` — they are resolved to bare Rust fn paths inside CCall.
    let mut pool_kinds: Vec<PoolKind> = Vec::with_capacity(bundle.constant_pool.len());
    for (i, entry) in bundle.constant_pool.iter().enumerate() {
        let kind = classify_pool_entry(
            entry,
            &builtin,
            registry_identity_map,
            &name_to_path,
            xbundle_providers,
        )
        .map_err(|e| format!("pool[{}]: {}", i, e))?;
        pool_kinds.push(kind);
    }

    // Determine the parameter arity from Param-typed pool entries — the
    // max slot index + 1. If no Param entries are present, the function
    // takes no params (or it's a 0-arg / non-composite root bundle).
    let param_count: u32 = pool_kinds
        .iter()
        .filter_map(|k| if let PoolKind::Param(i) = k { Some(*i + 1) } else { None })
        .max()
        .unwrap_or(0);

    // Destructure `args` into `__param_0..__param_{N-1}` per the bridge's
    // caller convention (see emit_node: 1-arg fns receive the arg directly,
    // multi-arg fns receive a Value::Tuple).
    match param_count {
        0 => {}
        1 => {
            out.push_str("    let __param_0: Value = args.clone();\n");
        }
        n => {
            out.push_str(&format!(
                "    let (__params_vec): Vec<Value> = match args.clone() {{\n\
                 \x20       Value::Tuple(es) if es.len() == {n} => es,\n\
                 \x20       other => panic!(\"{safe_name}: expected Value::Tuple of {n} args, got {{:?}}\", other),\n\
                 \x20   }};\n",
                n = n, safe_name = safe_name,
            ));
            for i in 0..n {
                out.push_str(&format!(
                    "    let __param_{i}: Value = __params_vec[{i}].clone();\n", i = i,
                ));
            }
        }
    }

    // Pool entries.
    //   Data → `let pool_N = <constant expr>;`
    //   Param(i) → `let pool_N = __param_i.clone();`
    //   FnRef → no binding (resolved inline at callee position)
    for (i, kind) in pool_kinds.iter().enumerate() {
        match kind {
            PoolKind::Data(expr) => {
                out.push_str(&format!("    let pool_{}: Value = {};\n", i, expr));
            }
            PoolKind::Param(slot) => {
                out.push_str(&format!(
                    "    let pool_{}: Value = __param_{}.clone();\n",
                    i, slot
                ));
            }
            PoolKind::FnRef(_) => {}
        }
    }
    // Suppress unused-variable warnings for args when not destructured.
    if param_count == 0 && (!bundle.constant_pool.is_empty() || !bundle.nodes.is_empty()) {
        out.push_str("    let _ = &args;\n");
    }

    // Nodes — branch-scoped (BRANCH_SCOPING_V1): a node used exclusively
    // within one arm of a `CIf` is emitted inside that arm's Rust block, so
    // it only executes when that arm is actually taken. Nodes required
    // elsewhere (shared between arms, referenced by `cond`, or used outside
    // any `CIf`) stay hoisted at the unconditional top level, exactly as
    // before.
    let arg_kind_table = fn_arg_kinds();
    let branch_paths = compute_branch_paths(bundle)?;
    let scope_groups = group_by_scope(bundle.nodes.len(), &branch_paths);
    out.push_str(&render_scope(
        None,
        &scope_groups,
        bundle,
        &pool_kinds,
        &arg_kind_table,
        &builtin,
        registry_identity_map,
        &name_to_path,
        xbundle_providers,
    )?);

    // Result: the bundle's own authoritative `result` ref (BUG2_RESULT_FIELD_V1)
    // — never guessed from "the last node in `nodes`", which is wrong whenever
    // the source's tail is a bare literal/VarRef following a real call.
    if let NodeRef::Pool(i) = bundle.result {
        if let Some(PoolKind::FnRef(_)) = pool_kinds.get(i as usize) {
            return Err(format!(
                "type gate: bundle result is a bare pool ref but pool[{}] is \
                 Fn-typed — Fn refs are callee-only, never returned as data \
                 (FN_REF_IS_CALLEE_ONLY)",
                i
            ));
        }
    }
    out.push_str(&format!("    {}\n", ref_expr(&bundle.result)));
    out.push_str("}\n\n");

    // Identity-derived export: ax_fn_<hex(sha256(fn_name))>
    // Callers in other bundles link against this symbol (LINK_BY_IDENTITY).
    // Uses "C-unwind" so panics can propagate through the call chain and be
    // caught by the multi-entry driver's catch_unwind (BRIDGE_ENTRY_POINTS_V1).
    let fn_identity = sha256_bytes(fn_name.as_bytes());
    let identity_sym = format!("ax_fn_{}", hash256_to_hex(&fn_identity));
    out.push_str(&format!(
        "#[no_mangle]\npub extern \"C-unwind\" fn {}(args: Value) -> Value {{\n",
        identity_sym
    ));
    out.push_str(&format!("    {}(args)\n", safe_name));
    out.push_str("}\n\n");

    // Exe shim
    out.push_str(&format!(
        "#[no_mangle]\npub extern \"C-unwind\" fn _ax_exe_{}(args: Value) -> Value {{\n",
        safe_name
    ));
    out.push_str(&format!("    {}(args)\n", safe_name));
    out.push_str("}\n");

    Ok(out)
}

#[cfg(test)]
mod async_scan_tests {
    use super::*;

    // A registry document exercising both scanned constructs plus the nesting
    // that trips a naïve `end`-counter: a `bridge_contract … end` inside a
    // `fn … end`, and top-level `channel … end` blocks whose `type` line must
    // not be confused with a top-level `type X = prim …` declaration.
    const DOC: &str = r#"
registry test 0.1

type Value = prim value
type Fn    = prim value

channel a2b
  type Value
end

channel b2a
  type Value
end

fn worker
  identity 0xdeadbeef
  kind     leaf
  in       (Text)
  out      Unit
  effect   fullIo
  deterministic false
  idempotent    false
  bridge_contract
    background
  end
end

fn plain
  identity 0xfeedface
  kind     leaf
  in       (Int)
  out      Int
  effect   pure
  deterministic true
  idempotent    true
end
"#;

    #[test]
    fn scan_extracts_channels_and_background() {
        let mut facts = BridgeAsyncFacts::default();
        scan_bridge_async_str(DOC, &mut facts);

        // Both top-level channels, with their element types.
        assert!(facts.channels.contains("a2b"), "missing channel a2b: {:?}", facts.channels);
        assert!(facts.channels.contains("b2a"), "missing channel b2a: {:?}", facts.channels);
        assert_eq!(facts.channels.len(), 2, "unexpected channels: {:?}", facts.channels);
        assert_eq!(facts.channel_types.get("a2b").map(String::as_str), Some("Value"));

        // Only the fn whose bridge_contract holds `background` is flagged; the
        // fn-closing `end` after the contract's own `end` must not leak state.
        assert!(facts.background_fns.contains("worker"), "worker not flagged: {:?}", facts.background_fns);
        assert!(!facts.background_fns.contains("plain"), "plain wrongly flagged");
        assert_eq!(facts.background_fns.len(), 1);
    }

    #[test]
    fn scan_ignores_toplevel_type_decls() {
        // `type X = prim …` (with `=`) is not a channel element type.
        let mut facts = BridgeAsyncFacts::default();
        scan_bridge_async_str("type Value = prim value\ntype Fn = prim value\n", &mut facts);
        assert!(facts.channels.is_empty());
        assert!(facts.channel_types.is_empty());
        assert!(facts.background_fns.is_empty());
    }

    #[test]
    fn scan_background_outside_contract_is_ignored() {
        // A bare `background` token that is NOT inside a bridge_contract block
        // must not flag the fn (guards against a loose keyword match).
        let doc = "fn f\n  identity 0x00\n  background\nend\n";
        let mut facts = BridgeAsyncFacts::default();
        scan_bridge_async_str(doc, &mut facts);
        assert!(facts.background_fns.is_empty(), "background outside contract flagged: {:?}", facts.background_fns);
    }
}

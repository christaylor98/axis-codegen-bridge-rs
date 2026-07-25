pub mod runtime;
pub mod core_ir_05;
pub mod emit;

// AXVERITY_WRITEPATH_PERF_DECOMPOSITION_V1 — process-wide counting allocator,
// see runtime::allocprobe. Additive measurement scaffolding: counting itself is
// unconditional (four cheap AtomicU64 fetch_adds per alloc/dealloc, no
// behavior change), consumption of the counters is gated by AXVERITY_WRITEPROBE
// (see runtime::tsmark). Overhead measured directly, not assumed — see the
// turn's report.
#[global_allocator]
static AXV_ALLOC_PROBE: runtime::allocprobe::CountingAlloc = runtime::allocprobe::CountingAlloc;

pub mod axis_core_ir_0_5_capnp {
    include!(concat!(env!("OUT_DIR"), "/core_ir_spec/axis_core_ir_0_5_capnp.rs"));
}

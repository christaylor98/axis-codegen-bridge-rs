pub mod runtime;
pub mod core_ir_05;
pub mod emit;

// AXVERITY_HOTPATH_UNBLOCK_V1 — the process-wide counting allocator introduced
// by AXVERITY_WRITEPATH_PERF_DECOMPOSITION_V1 is now OFF BY DEFAULT AT COMPILE
// TIME. It was unconditional, so it was the allocator of every binary linking
// this crate including production axverity-pg_server, where AXVERITY_HOTPATH_
// MEASUREMENT_V1 measured its two lock-prefixed RMWs on one shared 32-byte
// cacheline at 63% + 36.4% of __rust_alloc (malloc: 0.00%), 22.56% of server CPU
// at K=16, and — worst — a SELECT path that delivered 298 ops/s on 1 core and
// 262 on 16 with identical instruction counts and 17.5x the cycles.
//
// The claim that it "could not be made conditional without a branch on every
// allocation" was wrong: a compile-time feature needs no runtime branch at all.
// Enable for a measurement turn with `--features allocprobe`; see
// runtime::allocprobe for why the counters are cacheline-padded if you do.
#[cfg(feature = "allocprobe")]
#[global_allocator]
static AXV_ALLOC_PROBE: runtime::allocprobe::CountingAlloc = runtime::allocprobe::CountingAlloc;

pub mod axis_core_ir_0_5_capnp {
    include!(concat!(env!("OUT_DIR"), "/core_ir_spec/axis_core_ir_0_5_capnp.rs"));
}

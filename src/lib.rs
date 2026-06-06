pub mod runtime;
pub mod core_ir;
pub mod core_ir_05;
pub mod emit;
pub mod executor;

pub mod axis_core_ir_0_4_capnp {
    include!(concat!(env!("OUT_DIR"), "/core_ir_spec/axis_core_ir_0_4_capnp.rs"));
}

pub mod axis_core_ir_0_5_capnp {
    include!(concat!(env!("OUT_DIR"), "/core_ir_spec/axis_core_ir_0_5_capnp.rs"));
}

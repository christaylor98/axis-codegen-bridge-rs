fn main() {
    capnpc::CompilerCommand::new()
        .src_prefix("./")
        .file("./core_ir_spec/axis_core_ir_0_3.capnp")
        .run()
        .expect("Cap'n Proto schema compiler");
}

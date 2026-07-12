fn main() {
    capnpc::CompilerCommand::new()
        .src_prefix("./")
        .file("./core_ir_spec/axis_core_ir_0_5.capnp")
        .run()
        .expect("Cap'n Proto schema compiler (0.5)");

    // ISOLATION MEASUREMENT ONLY (HOTWRITE_ADMISSION_MINIMAL_CAPTURE_V1):
    // standalone C single-call hotwrite variant, -O3 single-TU inlining.
    println!("cargo:rerun-if-changed=src/runtime/hotwrite_batch.c");
    cc::Build::new()
        .file("src/runtime/hotwrite_batch.c")
        .opt_level(3)
        .flag("-msha")
        .flag("-msse4.1")
        .compile("hotwrite_batch_c");
}

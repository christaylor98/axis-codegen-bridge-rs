fn main() {

    // ISOLATION MEASUREMENT ONLY (HOTWRITE_ADMISSION_MINIMAL_CAPTURE_V1):
    // standalone C single-call hotwrite variant, -O3 single-TU inlining.
    println!("cargo:rerun-if-changed=src/runtime/hotwrite_batch.c");
    cc::Build::new()
        .file("src/runtime/hotwrite_batch.c")
        .opt_level(3)
        .flag("-msha")
        .flag("-msse4.1")
        .compile("hotwrite_batch_c");

    build_gpu_kernels();
}

// GPU_P5_V1 (axMachina P6): the GPU kernels are Rust (gpu-kernels/, no_std) built for nvptx64-nvidia-cuda with the
// nightly toolchain; their PTX is embedded by src/runtime/gpu_p5.rs. Without that toolchain the PTX is empty and
// gpu_p5_open answers 0 (no GPU), so callers fall back to the CPU and the bridge still builds on stable alone.
fn build_gpu_kernels() {
    use std::process::Command;
    println!("cargo:rerun-if-changed=gpu-kernels/src/lib.rs");
    println!("cargo:rerun-if-changed=gpu-kernels/Cargo.toml");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let target = out.join("gpu-kernels-target");
    let dst = out.join("axis_gpu_kernels.ptx");
    let ok = Command::new("rustup")
        .args(["run", "nightly", "cargo", "rustc", "--release", "--target", "nvptx64-nvidia-cuda",
               "--manifest-path", "gpu-kernels/Cargo.toml", "--target-dir"])
        .arg(&target)
        .args(["--", "-C", "target-cpu=sm_86"])
        .env_remove("RUSTC").env_remove("RUSTC_WRAPPER").env_remove("RUSTFLAGS").env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("CARGO_TARGET_DIR").env_remove("CARGO_MAKEFLAGS").env_remove("CARGO_BUILD_TARGET")
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let ptx = target.join("nvptx64-nvidia-cuda/release/axis_gpu_kernels.ptx");
    if ok && ptx.exists() {
        std::fs::copy(&ptx, &dst).expect("copy kernels ptx");
    } else {
        println!("cargo:warning=gpu-kernels not built (needs rustup nightly + nvptx64-nvidia-cuda): GPU search disabled");
        std::fs::write(&dst, "").unwrap();
    }
}

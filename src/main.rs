use std::time::Instant;
use axis_codegen_bridge::core_ir;
use axis_codegen_bridge::emit::rust::emit_rust_from_core;

mod axis_core_ir_0_3_capnp {
    include!(concat!(env!("OUT_DIR"), "/core_ir_spec/axis_core_ir_0_3_capnp.rs"));
}

fn usage() -> ! {
    eprintln!("Usage:");
    eprintln!("  axis-codegen-bridge build <input.coreir> --out <output-binary> [--link-lib <name>] [--link-search <path>]");
    eprintln!("  axis-codegen-bridge inspect <input.coreir>");
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 { usage(); }

    match args[1].as_str() {
        "inspect" => cmd_inspect(&args[2]),
        "build"   => cmd_build(&args[2..]),
        _         => usage(),
    }
}

fn cmd_inspect(path: &str) {
    match core_ir::inspect_core_bundle(path) {
        Ok(summary) => { println!("{}", summary); std::process::exit(0); }
        Err(e)      => { eprintln!("error: {}", e); std::process::exit(1); }
    }
}

fn cmd_build(args: &[String]) {
    let t0 = Instant::now();

    if args.is_empty() { usage(); }
    let input = &args[0];

    let mut output   = "a.out".to_string();
    let mut link_libs: Vec<String>    = Vec::new();
    let mut link_search: Vec<String>  = Vec::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--out"         if i + 1 < args.len() => { output = args[i+1].clone(); i += 2; }
            "--link-lib"    if i + 1 < args.len() => { link_libs.push(args[i+1].clone()); i += 2; }
            "--link-search" if i + 1 < args.len() => { link_search.push(args[i+1].clone()); i += 2; }
            _ => { i += 1; }
        }
    }

    let program = match core_ir::load_core_bundle(input) {
        Ok(p)  => p,
        Err(e) => { eprintln!("error: failed to load {}: {}", input, e); std::process::exit(1); }
    };

    let rust_code = emit_rust_from_core(&program.root_term, input, "main");

    let out_path = std::path::Path::new(&output);
    let out_dir  = out_path.parent().unwrap_or(std::path::Path::new("."));

    if let Err(e) = std::fs::create_dir_all(out_dir) {
        eprintln!("error: cannot create output dir: {}", e); std::process::exit(1);
    }

    let generated_rs = out_dir.join("generated.rs");
    if let Err(e) = std::fs::write(&generated_rs, &rust_code) {
        eprintln!("error: cannot write generated.rs: {}", e); std::process::exit(1);
    }

    let exe_dir = std::env::current_exe().expect("current exe").parent().expect("exe dir").to_owned();

    let mut cmd = std::process::Command::new("rustc");
    cmd.arg(&generated_rs)
       .arg("-o").arg(&output)
       .arg("--extern").arg(format!("axis_codegen_bridge={}/libaxis_codegen_bridge.rlib", exe_dir.display()))
       .arg("-L").arg(format!("dependency={}/deps", exe_dir.display()));

    for path in &link_search { cmd.arg("-L").arg(path); }
    for lib  in &link_libs   { cmd.arg("-l").arg(lib);  }

    match cmd.status() {
        Ok(s) if s.success() => {
            eprintln!("built {} in {}ms", output, t0.elapsed().as_millis());
        }
        Ok(s) => {
            eprintln!("error: rustc exited {:?}", s.code());
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("error: failed to invoke rustc: {}", e);
            std::process::exit(1);
        }
    }
}

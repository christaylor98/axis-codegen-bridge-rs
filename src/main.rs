use std::collections::HashSet;
use std::time::Instant;
use axis_codegen_bridge::core_ir;
use axis_codegen_bridge::emit::rust::emit_rust_from_core;

mod axis_core_ir_0_3_capnp {
    include!(concat!(env!("OUT_DIR"), "/core_ir_spec/axis_core_ir_0_3_capnp.rs"));
}

fn usage() -> ! {
    eprintln!("Usage:");
    eprintln!("  axis-codegen-bridge build <input.coreir> --out <output-binary> [options]");
    eprintln!("    --lib <path.coreir>     link a library bundle (repeatable)");
    eprintln!("    --lib-dir <directory>   link all .coreir in directory (repeatable)");
    eprintln!("    --reg <path.axreg>      registry file for CCall validation (repeatable)");
    eprintln!("    --link-lib <name>       pass -l <name> to rustc");
    eprintln!("    --link-search <path>    pass -L <path> to rustc");
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

/// Parse function names from a .axreg registry file.
fn load_registry_names(paths: &[String]) -> HashSet<String> {
    let mut names = HashSet::new();
    for path in paths {
        let content = match std::fs::read_to_string(path) {
            Ok(c)  => c,
            Err(e) => { eprintln!("error: failed to read registry {}: {}", path, e); std::process::exit(1); }
        };
        for line in content.lines() {
            let trimmed = line.trim();
            // Handles both "fn name" block form and "fn name  arity=..." inline form.
            if let Some(rest) = trimmed.strip_prefix("fn ") {
                let name = rest.split_whitespace().next().unwrap_or("").to_string();
                if !name.is_empty() { names.insert(name); }
            }
        }
    }
    names
}

fn cmd_build(args: &[String]) {
    let t0 = Instant::now();

    if args.is_empty() { usage(); }
    let input = &args[0];

    let mut output        = "a.out".to_string();
    let mut link_libs: Vec<String>   = Vec::new();
    let mut link_search: Vec<String> = Vec::new();
    let mut lib_paths: Vec<String>   = Vec::new();
    let mut lib_dirs: Vec<String>    = Vec::new();
    let mut reg_paths: Vec<String>   = Vec::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--out"         if i + 1 < args.len() => { output = args[i+1].clone(); i += 2; }
            "--link-lib"    if i + 1 < args.len() => { link_libs.push(args[i+1].clone()); i += 2; }
            "--link-search" if i + 1 < args.len() => { link_search.push(args[i+1].clone()); i += 2; }
            "--lib"         if i + 1 < args.len() => { lib_paths.push(args[i+1].clone()); i += 2; }
            "--lib-dir"     if i + 1 < args.len() => { lib_dirs.push(args[i+1].clone()); i += 2; }
            "--reg"         if i + 1 < args.len() => { reg_paths.push(args[i+1].clone()); i += 2; }
            _ => { i += 1; }
        }
    }

    // Expand --lib-dir: collect all .coreir files sorted by path.
    for dir in &lib_dirs {
        let entries = match std::fs::read_dir(dir) {
            Ok(e)  => e,
            Err(e) => { eprintln!("error: cannot read --lib-dir {}: {}", dir, e); std::process::exit(1); }
        };
        let mut paths: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map_or(false, |ext| ext == "coreir"))
            .map(|e| e.path().to_string_lossy().to_string())
            .collect();
        paths.sort();
        lib_paths.extend(paths);
    }

    // Load library bundles, deduplicating by function name.
    let mut libs: Vec<(String, core_ir::CoreTerm)> = Vec::new();
    let mut seen_fn_names: HashSet<String> = HashSet::new();
    for lib_path in &lib_paths {
        let prog = match core_ir::load_core_bundle(lib_path) {
            Ok(p)  => p,
            Err(e) => { eprintln!("error: failed to load --lib {}: {}", lib_path, e); std::process::exit(1); }
        };
        // Use entrypointName from the bundle; fall back to filename stem when empty.
        let fn_name = if !prog.entrypoint_name.is_empty() {
            prog.entrypoint_name.clone()
        } else {
            let stem = std::path::Path::new(lib_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if stem.is_empty() {
                eprintln!("error: library {} has empty entrypointName and no filename stem", lib_path);
                std::process::exit(1);
            }
            eprintln!("warning: library {} has empty entrypointName, using stem '{}'", lib_path, stem);
            stem
        };
        if !seen_fn_names.insert(fn_name.clone()) {
            eprintln!("error: duplicate library function name '{}' (from {})", fn_name, lib_path);
            std::process::exit(1);
        }
        libs.push((fn_name, prog.root_term));
    }

    // Parse registry files.
    let registry_names = load_registry_names(&reg_paths);

    // Load main bundle.
    let program = match core_ir::load_core_bundle(input) {
        Ok(p)  => p,
        Err(e) => { eprintln!("error: failed to load {}: {}", input, e); std::process::exit(1); }
    };

    // Generate Rust source.
    let rust_code = match emit_rust_from_core(&program.root_term, input, "main", &libs, &registry_names) {
        Ok(code) => code,
        Err(e)   => { eprintln!("error: {}", e); std::process::exit(1); }
    };

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

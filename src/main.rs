use std::collections::HashSet;
use std::io::Write;
use std::time::Instant;
use axis_codegen_bridge::core_ir;
use axis_codegen_bridge::core_ir_05;
use axis_codegen_bridge::emit::rust::{emit_rust_lib_from_core, sanitise};
use axis_codegen_bridge::emit::rust_05;

fn usage() -> ! {
    eprintln!("Usage:");
    eprintln!("  axis-codegen-bridge build <input.coreir> --out <path> [options]");
    eprintln!("    Produces <dir>/lib<stem>.a (lib-first). Use --out x.a to name verbatim.");
    eprintln!("    --exe                   also compile a runnable binary at <path>");
    eprintln!("    --lib <path.coreir>     link a library bundle (repeatable)");
    eprintln!("    --lib-dir <directory>   link all .coreir in directory (repeatable)");
    eprintln!("    --reg <path.axreg>      registry file for CCall validation (repeatable)");
    eprintln!("    --link-lib <name>       pass -l <name> to rustc");
    eprintln!("    --link-search <path>    pass -L <path> to rustc");
    eprintln!("  axis-codegen-bridge bundle --out <output.a> <input1.a> [<input2.a> ...]");
    eprintln!("    Merges multiple .a archives into a single output.a via ar.");
    eprintln!("  axis-codegen-bridge inspect <input.coreir>");
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 { usage(); }

    match args[1].as_str() {
        "inspect" if args.len() >= 3 => cmd_inspect(&args[2]),
        "build"   if args.len() >= 3 => cmd_build(&args[2..]),
        "build05" if args.len() >= 4 => cmd_build05(&args[2], &args[3]),
        "bundle"                      => cmd_bundle(&args[2..]),
        _ => usage(),
    }
}

/// Lower a 0.4 Core IR bundle (a generator term) to a flat 0.5 CoreBundle.
/// This is the term -> 0.5 path the bridge previously lacked, letting the
/// mechanical generator's output reach the honest 0.5 gate without the compiler.
fn cmd_build05(input: &str, output: &str) {
    let prog = match core_ir::load_core_bundle(input) {
        Ok(p)  => p,
        Err(e) => { eprintln!("error: load 0.4 bundle {}: {}", input, e); std::process::exit(1); }
    };
    let bundle = match core_ir_05::lower_core_term_to_bundle_05(&prog.root_term) {
        Ok(b)  => b,
        Err(e) => { eprintln!("error: lower to 0.5: {}", e); std::process::exit(1); }
    };
    match core_ir_05::write_core_bundle_05_to_file(&bundle, output) {
        Ok(())  => { println!("{} -> {} (0.5, {} nodes)", input, output, bundle.nodes.len()); std::process::exit(0); }
        Err(e)  => { eprintln!("error: write 0.5 {}: {}", output, e); std::process::exit(1); }
    }
}

fn cmd_inspect(path: &str) {
    if let Ok(summary) = core_ir_05::inspect_core_bundle(path) {
        println!("{}", summary);
        std::process::exit(0);
    }
    match core_ir::inspect_core_bundle(path) {
        Ok(summary) => { println!("{}", summary); std::process::exit(0); }
        Err(e)      => { eprintln!("error: {}", e); std::process::exit(1); }
    }
}

fn load_registry_names(paths: &[String]) -> HashSet<String> {
    let mut names = HashSet::new();
    for path in paths {
        let content = match std::fs::read_to_string(path) {
            Ok(c)  => c,
            Err(e) => { eprintln!("error: failed to read registry {}: {}", path, e); std::process::exit(1); }
        };
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("fn ") {
                let name = rest.split_whitespace().next().unwrap_or("").to_string();
                if !name.is_empty() { names.insert(name); }
            }
        }
    }
    names
}

/// Append a function entry to a registry file, idempotent (skips if already present).
fn append_fn_to_registry(reg_path: &str, fn_name: &str, effect_class: &core_ir::EffectClass) {
    let content = std::fs::read_to_string(reg_path).unwrap_or_default();
    for line in content.lines() {
        if let Some(rest) = line.trim().strip_prefix("fn ") {
            if rest.split_whitespace().next().unwrap_or("") == fn_name {
                return;
            }
        }
    }
    let deterministic = matches!(effect_class, core_ir::EffectClass::Pure);
    let profile = match effect_class {
        core_ir::EffectClass::Pure   => "pure",
        core_ir::EffectClass::Reads  => "reads",
        core_ir::EffectClass::Writes => "writes",
        core_ir::EffectClass::FullIo => "full_io",
    };
    let entry = format!(
        "\nfn {}\n  arity 1\n  deterministic {}\n  profile {}\nend\n",
        fn_name, deterministic, profile
    );
    match std::fs::OpenOptions::new().append(true).open(reg_path) {
        Ok(mut f) => { let _ = f.write_all(entry.as_bytes()); }
        Err(e)    => { eprintln!("warning: could not append to registry {}: {}", reg_path, e); }
    }
}

/// Compute the .a output path from the --out argument.
/// If the arg already ends in ".a", use verbatim; otherwise produce <dir>/lib<stem>.a.
fn compute_lib_path(out_arg: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(out_arg);
    if out_arg.ends_with(".rlib") || out_arg.ends_with(".a") {
        p.to_owned()
    } else {
        let dir  = p.parent().unwrap_or(std::path::Path::new("."));
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
        dir.join(format!("lib{}.a", stem))
    }
}

fn cmd_build(args: &[String]) {
    let t0 = Instant::now();
    if args.is_empty() { usage(); }
    let input = &args[0];

    let mut output        = "a.out".to_string();
    let mut exe_flag      = false;
    let mut link_libs:   Vec<String> = Vec::new();
    let mut link_search: Vec<String> = Vec::new();
    let mut lib_paths:   Vec<String> = Vec::new();
    let mut lib_dirs:    Vec<String> = Vec::new();
    let mut reg_paths:   Vec<String> = Vec::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--out"         if i + 1 < args.len() => { output = args[i+1].clone(); i += 2; }
            "--exe"                               => { exe_flag = true; i += 1; }
            "--link-lib"    if i + 1 < args.len() => { link_libs.push(args[i+1].clone()); i += 2; }
            "--link-search" if i + 1 < args.len() => { link_search.push(args[i+1].clone()); i += 2; }
            "--lib"         if i + 1 < args.len() => { lib_paths.push(args[i+1].clone()); i += 2; }
            "--lib-dir"     if i + 1 < args.len() => { lib_dirs.push(args[i+1].clone()); i += 2; }
            "--reg"         if i + 1 < args.len() => { reg_paths.push(args[i+1].clone()); i += 2; }
            _ => { i += 1; }
        }
    }

    // Detect 0.5 bundle: try 0.5 loader first; if it parses, use the 0.5 pipeline.
    if let Ok(bundle) = core_ir_05::load_core_bundle(input) {
        if !lib_paths.is_empty() || !lib_dirs.is_empty() {
            eprintln!("warning: --lib and --lib-dir are not supported for 0.5 bundles \
                       (link pre-compiled rlibs via --link-lib / --link-search instead)");
        }
        let fn_name = std::path::Path::new(input)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("main")
            .to_string();
        let registry_map = rust_05::load_registry_identity_map(&reg_paths);
        let rust_code = match rust_05::emit_rust_lib_from_bundle(&bundle, &fn_name, &registry_map) {
            Ok(code) => code,
            Err(e)   => { eprintln!("error: {}", e); std::process::exit(1); }
        };
        let lib_path = compute_lib_path(&output);
        let out_dir  = lib_path.parent().unwrap_or(std::path::Path::new("."));
        if let Err(e) = std::fs::create_dir_all(out_dir) {
            eprintln!("error: cannot create output dir: {}", e); std::process::exit(1);
        }
        let generated_rs = out_dir.join("generated_lib.rs");
        if let Err(e) = std::fs::write(&generated_rs, &rust_code) {
            eprintln!("error: cannot write generated_lib.rs: {}", e); std::process::exit(1);
        }
        let exe_dir = std::env::current_exe().expect("current exe").parent().expect("exe dir").to_owned();
        let mut cmd = std::process::Command::new("rustc");
        cmd.arg(&generated_rs)
           .arg("--crate-type=rlib")
           .arg("--crate-name=generated")
           .arg("--edition=2021")
           .arg("-o").arg(&lib_path)
           .arg("-C").arg("embed-bitcode=no")
           .arg("-C").arg("strip=debuginfo")
           .arg("--extern").arg(format!("axis_codegen_bridge={}/libaxis_codegen_bridge.rlib", exe_dir.display()))
           .arg("-L").arg(format!("dependency={}/deps", exe_dir.display()));
        for path in &link_search { cmd.arg("-L").arg(path); }
        match cmd.status() {
            Ok(s) if s.success() => {
                eprintln!("built {} in {}ms", lib_path.display(), t0.elapsed().as_millis());
            }
            Ok(s) => { eprintln!("error: rustc exited {:?}", s.code()); std::process::exit(1); }
            Err(e) => { eprintln!("error: failed to invoke rustc: {}", e); std::process::exit(1); }
        }
        if !exe_flag { return; }
        let safe_name = rust_05::sanitise(&fn_name);
        let shim_fn   = format!("_ax_exe_{}", safe_name);
        let shim_code = format!(
            "extern crate axis_codegen_bridge;\n\
             use axis_codegen_bridge::runtime::value::{{Value, init_runtime, intern_str}};\n\n\
             #[allow(improper_ctypes)]\n\
             extern \"C\" {{\n\
                 fn {fn}(args: Value) -> Value;\n\
             }}\n\n\
             fn main() {{\n\
                 init_runtime();\n\
                 let args: Vec<Value> = std::env::args().skip(1)\n\
                     .map(|s| Value::Str(intern_str(&s)))\n\
                     .collect();\n\
                 let result = unsafe {{ {fn}(Value::List(args)) }};\n\
                 if !matches!(result, Value::Unit) {{ println!(\"{{}}\", result); }}\n\
             }}\n",
            fn = shim_fn
        );
        let shim_rs  = out_dir.join("generated_main.rs");
        if let Err(e) = std::fs::write(&shim_rs, &shim_code) {
            eprintln!("error: cannot write generated_main.rs: {}", e); std::process::exit(1);
        }
        let lib_abs = std::fs::canonicalize(&lib_path).unwrap_or_else(|_| lib_path.clone());
        let mut cmd = std::process::Command::new("rustc");
        cmd.arg(&shim_rs)
           .arg("--edition=2021")
           .arg("-o").arg(&output)
           .arg("-C").arg("strip=debuginfo")
           .arg("--extern").arg(format!("axis_codegen_bridge={}/libaxis_codegen_bridge.rlib", exe_dir.display()))
           .arg("-L").arg(format!("dependency={}/deps", exe_dir.display()))
           .arg("-C").arg(format!("link-arg={}", lib_abs.display()));
        for path in &link_search { cmd.arg("-L").arg(path); }
        for lib  in &link_libs   { cmd.arg("-l").arg(lib);  }
        match cmd.status() {
            Ok(s) if s.success() => {
                eprintln!("built {} in {}ms", output, t0.elapsed().as_millis());
            }
            Ok(s) => { eprintln!("error: rustc (exe) exited {:?}", s.code()); std::process::exit(1); }
            Err(e) => { eprintln!("error: failed to invoke rustc: {}", e); std::process::exit(1); }
        }
        return;
    }

    // ── 0.4 path ─────────────────────────────────────────────────────────────

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
        // Use entrypointName if set and meaningful; 'bundle' is a compiler
        // placeholder that means "no real name" — fall back to the file stem.
        let fn_name = if !prog.entrypoint_name.is_empty() && prog.entrypoint_name != "bundle" {
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
            stem
        };
        if !seen_fn_names.insert(fn_name.clone()) {
            eprintln!("error: duplicate library function name '{}' (from {})", fn_name, lib_path);
            std::process::exit(1);
        }
        libs.push((fn_name, prog.root_term));
    }

    let registry_names = load_registry_names(&reg_paths);

    let program = match core_ir::load_core_bundle(input) {
        Ok(p)  => p,
        Err(e) => { eprintln!("error: failed to load {}: {}", input, e); std::process::exit(1); }
    };

    // Exports: use bundle's exports list if non-empty; otherwise single export from metadata.
    let bundle_exports: Vec<(String, core_ir::CoreTerm)> = if !program.exports.is_empty() {
        program.exports.clone()
    } else {
        let fn_name = if !program.entrypoint_name.is_empty() {
            program.entrypoint_name.clone()
        } else {
            std::path::Path::new(input)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("main")
                .to_string()
        };
        vec![(fn_name, program.root_term.clone())]
    };

    let lib_path = compute_lib_path(&output);
    let out_dir  = lib_path.parent().unwrap_or(std::path::Path::new("."));

    if let Err(e) = std::fs::create_dir_all(out_dir) {
        eprintln!("error: cannot create output dir: {}", e); std::process::exit(1);
    }

    // Generate and compile the Rust library (rlib — no std bundling).
    let rust_code = match emit_rust_lib_from_core(&bundle_exports, &libs, &registry_names) {
        Ok(code) => code,
        Err(e)   => { eprintln!("error: {}", e); std::process::exit(1); }
    };

    let generated_rs = out_dir.join("generated_lib.rs");
    if let Err(e) = std::fs::write(&generated_rs, &rust_code) {
        eprintln!("error: cannot write generated_lib.rs: {}", e); std::process::exit(1);
    }

    let exe_dir = std::env::current_exe().expect("current exe").parent().expect("exe dir").to_owned();

    let mut cmd = std::process::Command::new("rustc");
    cmd.arg(&generated_rs)
       .arg("--crate-type=rlib")
       .arg("--crate-name=generated")
       .arg("--edition=2021")
       .arg("-o").arg(&lib_path)
       .arg("-C").arg("embed-bitcode=no")
       .arg("-C").arg("strip=debuginfo")
       .arg("--extern").arg(format!("axis_codegen_bridge={}/libaxis_codegen_bridge.rlib", exe_dir.display()))
       .arg("-L").arg(format!("dependency={}/deps", exe_dir.display()));

    for path in &link_search { cmd.arg("-L").arg(path); }

    match cmd.status() {
        Ok(s) if s.success() => {
            eprintln!("built {} in {}ms", lib_path.display(), t0.elapsed().as_millis());
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

    // Auto-register exports into any --reg paths (idempotent).
    for (fn_name, _) in &bundle_exports {
        for reg_path in &reg_paths {
            append_fn_to_registry(reg_path, fn_name, &program.effect_class);
        }
    }

    if !exe_flag { return; }

    // --exe: compile a thin shim binary that calls the entrypoint from the rlib.
    // Choose: prefer an export named "main", else use the first export.
    let exe_fn_name = bundle_exports.iter()
        .find(|(n, _)| n == "main")
        .or_else(|| bundle_exports.first())
        .map(|(n, _)| n.clone())
        .unwrap_or_else(|| "main".to_string());
    let safe_fn     = sanitise(&exe_fn_name);
    let shim_fn     = format!("_ax_exe_{}", safe_fn);
    let shim_code = format!(
        "extern crate axis_codegen_bridge;\n\
         use axis_codegen_bridge::runtime::value::{{Value, init_runtime, intern_str}};\n\n\
         #[allow(improper_ctypes)]\n\
         extern \"C\" {{\n\
             fn {fn}(args: Value) -> Value;\n\
         }}\n\n\
         fn main() {{\n\
             init_runtime();\n\
             let args: Vec<Value> = std::env::args().skip(1)\n\
                 .map(|s| Value::Str(intern_str(&s)))\n\
                 .collect();\n\
             let result = unsafe {{ {fn}(Value::List(args)) }};\n\
             if !matches!(result, Value::Unit) {{ println!(\"{{}}\", result); }}\n\
         }}\n",
        fn = shim_fn
    );

    let shim_rs = out_dir.join("generated_main.rs");
    if let Err(e) = std::fs::write(&shim_rs, &shim_code) {
        eprintln!("error: cannot write generated_main.rs: {}", e); std::process::exit(1);
    }

    let lib_abs = std::fs::canonicalize(&lib_path).unwrap_or_else(|_| lib_path.clone());

    let mut cmd = std::process::Command::new("rustc");
    cmd.arg(&shim_rs)
       .arg("--edition=2021")
       .arg("-o").arg(&output)
       .arg("-C").arg("strip=debuginfo")
       .arg("--extern").arg(format!("axis_codegen_bridge={}/libaxis_codegen_bridge.rlib", exe_dir.display()))
       .arg("-L").arg(format!("dependency={}/deps", exe_dir.display()))
       .arg("-C").arg(format!("link-arg={}", lib_abs.display()));

    for path in &link_search { cmd.arg("-L").arg(path); }
    for lib  in &link_libs   { cmd.arg("-l").arg(lib);  }

    match cmd.status() {
        Ok(s) if s.success() => {
            eprintln!("built {} in {}ms", output, t0.elapsed().as_millis());
        }
        Ok(s) => {
            eprintln!("error: rustc (exe) exited {:?}", s.code());
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("error: failed to invoke rustc: {}", e);
            std::process::exit(1);
        }
    }
}

fn cmd_bundle(args: &[String]) {
    let mut out_path: Option<String> = None;
    let mut inputs: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--out" if i + 1 < args.len() => { out_path = Some(args[i+1].clone()); i += 2; }
            _                             => { inputs.push(args[i].clone()); i += 1; }
        }
    }

    let out = match out_path {
        Some(p) => p,
        None    => { eprintln!("error: bundle requires --out <output.a>"); std::process::exit(1); }
    };

    if inputs.is_empty() {
        eprintln!("error: bundle requires at least one input .a file");
        std::process::exit(1);
    }

    for input in &inputs {
        if !std::path::Path::new(input).exists() {
            eprintln!("error: input archive does not exist: {}", input);
            std::process::exit(1);
        }
    }

    // Check that ar is available.
    if std::process::Command::new("ar").arg("--version").output().is_err() {
        eprintln!("error: 'ar' not found on PATH");
        std::process::exit(1);
    }

    let tmp_dir = std::env::temp_dir().join(format!("axis-bundle-{}", std::process::id()));
    if let Err(e) = std::fs::create_dir_all(&tmp_dir) {
        eprintln!("error: cannot create temp dir: {}", e); std::process::exit(1);
    }

    let mut all_o: Vec<std::path::PathBuf> = Vec::new();

    for (idx, input_path) in inputs.iter().enumerate() {
        let extract_dir = tmp_dir.join(format!("inp_{}", idx));
        if let Err(e) = std::fs::create_dir_all(&extract_dir) {
            eprintln!("error: {}", e); std::process::exit(1);
        }

        let abs_input = std::fs::canonicalize(input_path).unwrap_or_else(|_| std::path::PathBuf::from(input_path));
        let status = std::process::Command::new("ar")
            .arg("x")
            .arg(&abs_input)
            .current_dir(&extract_dir)
            .status();

        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                eprintln!("error: ar x failed for {} (exit {:?})", input_path, s.code());
                let _ = std::fs::remove_dir_all(&tmp_dir);
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("error: failed to run ar: {}", e);
                let _ = std::fs::remove_dir_all(&tmp_dir);
                std::process::exit(1);
            }
        }

        // Rename each extracted .o to avoid name collisions across archives.
        for entry in std::fs::read_dir(&extract_dir).unwrap().flatten() {
            if entry.path().extension().map_or(false, |e| e == "o") {
                let new_name = format!("{}_{}", idx, entry.file_name().to_string_lossy());
                let new_path = tmp_dir.join(&new_name);
                let _ = std::fs::rename(entry.path(), &new_path);
                all_o.push(new_path);
            }
        }
    }

    let out_path_p = std::path::Path::new(&out);
    if let Some(dir) = out_path_p.parent() {
        if !dir.as_os_str().is_empty() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                eprintln!("error: cannot create output dir: {}", e); std::process::exit(1);
            }
        }
    }

    let abs_out = if out_path_p.is_absolute() {
        out_path_p.to_owned()
    } else {
        std::env::current_dir().unwrap_or_default().join(out_path_p)
    };

    let mut ar_cmd = std::process::Command::new("ar");
    ar_cmd.arg("rcs").arg(&abs_out);
    for o in &all_o { ar_cmd.arg(o); }

    match ar_cmd.status() {
        Ok(s) if s.success() => {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            eprintln!("bundled {} archive(s) into {}", inputs.len(), out);
        }
        Ok(s) => {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            eprintln!("error: ar rcs failed (exit {:?})", s.code());
            std::process::exit(1);
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            eprintln!("error: failed to run ar: {}", e);
            std::process::exit(1);
        }
    }
}

use std::collections::{HashSet, HashMap};
use std::io::Write;
use std::time::Instant;
use sha2::{Sha256, Digest};
use axis_codegen_bridge::core_ir;
use axis_codegen_bridge::core_ir_05::{self, CoreBundle, Node, Hash256, sha256_bytes, hash256_to_hex};
use axis_codegen_bridge::emit::rust::{emit_rust_lib_from_core, sanitise};
use axis_codegen_bridge::emit::rust_05;

fn usage() -> ! {
    eprintln!("Usage:");
    eprintln!("  axis-codegen-bridge build <input.coreir> --out <path> [options]");
    eprintln!("    Produces <dir>/lib<stem>.a (lib-first). Use --out x.a to name verbatim.");
    eprintln!("    --exe                         also compile a runnable binary at <path>");
    eprintln!("    --entries name1,name2,...      named entry points (comma-separated); each");
    eprintln!("                                  runs in its own thread with full argv");
    eprintln!("    --entry <name>                repeatable alias for --entries");
    eprintln!("    --entry-stack-size <bytes>    per-entry stack (default 1 MiB)");
    eprintln!("    --lib <path.coreir>           link a library bundle (repeatable)");
    eprintln!("    --lib-dir <directory>         link all .coreir in directory (repeatable)");
    eprintln!("    --reg <path.axreg>            registry file for CCall validation (repeatable)");
    eprintln!("    --link-lib <name>             pass -l <name> to rustc");
    eprintln!("    --link-search <path>          pass -L <path> to rustc");
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
    let effect = match effect_class {
        core_ir::EffectClass::Pure   => "pure",
        core_ir::EffectClass::Reads  => "reads",
        core_ir::EffectClass::Writes => "writes",
        core_ir::EffectClass::FullIo => "fullIo",
    };
    let deterministic = matches!(effect_class, core_ir::EffectClass::Pure);
    let idempotent    = matches!(effect_class, core_ir::EffectClass::Pure);
    let identity: String = {
        let hash = Sha256::digest(fn_name.as_bytes());
        format!("0x{}", hash.iter().map(|b| format!("{:02x}", b)).collect::<String>())
    };
    let entry = format!(
        "\nfn {}\n  identity {}\n  kind     leaf\n  in       (Value)\n  out      Value\n  effect   {}\n  deterministic {}\n  idempotent    {}\nend\n",
        fn_name, identity, effect, deterministic, idempotent
    );
    match std::fs::OpenOptions::new().append(true).open(reg_path) {
        Ok(mut f) => { let _ = f.write_all(entry.as_bytes()); }
        Err(e)    => { eprintln!("warning: could not append to registry {}: {}", reg_path, e); }
    }
}

/// Locate the bridge rlib next to the binary, falling back to deps/ for debug/test builds.
/// Release builds place the rlib at {exe_dir}/libaxis_codegen_bridge.rlib directly.
/// Debug and `cargo test` builds place it at {exe_dir}/deps/libaxis_codegen_bridge-<hash>.rlib.
fn find_bridge_rlib(exe_dir: &std::path::Path) -> std::path::PathBuf {
    let simple = exe_dir.join("libaxis_codegen_bridge.rlib");
    if simple.exists() { return simple; }
    let deps_dir = exe_dir.join("deps");
    if let Ok(rd) = std::fs::read_dir(&deps_dir) {
        let mut candidates: Vec<std::path::PathBuf> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("libaxis_codegen_bridge") && n.ends_with(".rlib"))
                    .unwrap_or(false)
            })
            .collect();
        // Prefer newest (most recent build)
        candidates.sort_by_key(|p| {
            std::cmp::Reverse(
                p.metadata().and_then(|m| m.modified()).ok()
            )
        });
        if let Some(p) = candidates.into_iter().next() { return p; }
    }
    simple // return even if absent so rustc gives a clear error
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

/// Collect the transitive closure of §5b provider identities needed by `root`
/// and any `extra_roots` (e.g. named entry-point providers), returning them in
/// post-order (deepest dependency first).
///
/// Uses a visited set for cycle safety: if A calls B and B calls A, both end up
/// in the closure. Their mutual extern decls are unresolved at rlib compilation
/// time but resolve at final executable link (CYCLES_ARE_LEGAL).
///
/// `extra_roots` are provider identities (e.g. §5b entry points not called
/// from the root bundle) whose bundles are also traversed, and whose own
/// identity is added to the ordered list so they get compiled.
fn collect_xbundle_closure(
    root: &CoreBundle,
    available: &HashMap<Hash256, (String, CoreBundle)>,
    extra_roots: &[Hash256],
) -> Vec<Hash256> {
    let mut ordered: Vec<Hash256> = Vec::new();
    let mut visited: HashSet<Hash256> = HashSet::new();
    collect_closure_dfs(root, available, &mut visited, &mut ordered);
    for eid in extra_roots {
        if let Some((_, eb)) = available.get(eid) {
            collect_closure_dfs(eb, available, &mut visited, &mut ordered);
        }
        if visited.insert(*eid) {
            ordered.push(*eid);
        }
    }
    ordered
}

fn collect_closure_dfs(
    bundle: &CoreBundle,
    available: &HashMap<Hash256, (String, CoreBundle)>,
    visited: &mut HashSet<Hash256>,
    ordered: &mut Vec<Hash256>,
) {
    for node in &bundle.nodes {
        if let Node::CCall { target_identity, target_name, .. } = node {
            if rust_05::is_bridge_builtin(target_identity) { continue; }
            if target_name.is_empty() || sha256_bytes(target_name.as_bytes()) != *target_identity { continue; }
            if !visited.insert(*target_identity) { continue; } // cycle or already processed
            if let Some((_, dep_bundle)) = available.get(target_identity) {
                collect_closure_dfs(dep_bundle, available, visited, ordered);
                ordered.push(*target_identity);
            }
        }
    }
    // Fn-typed pool entries: an fn_ref to a composite is a live dependency
    // (the HOF will call it at runtime), even though no Node::CCall targets
    // it directly. Walk those too so the provider rlib is compiled and linked.
    let fn_th = core_ir_05::fn_type_hash();
    for entry in &bundle.constant_pool {
        if entry.def_hash != fn_th || entry.payload.len() != 32 {
            continue;
        }
        let mut id: Hash256 = [0u8; 32];
        id.copy_from_slice(&entry.payload);
        if rust_05::is_bridge_builtin(&id) { continue; }
        if !visited.insert(id) { continue; }
        if let Some((_, dep_bundle)) = available.get(&id) {
            collect_closure_dfs(dep_bundle, available, visited, ordered);
            ordered.push(id);
        }
    }
}

fn cmd_build(args: &[String]) {
    let t0 = Instant::now();
    if args.is_empty() { usage(); }
    let input = &args[0];

    let mut output             = "a.out".to_string();
    let mut exe_flag           = false;
    let mut link_libs:   Vec<String> = Vec::new();
    let mut link_search: Vec<String> = Vec::new();
    let mut lib_paths:   Vec<String> = Vec::new();
    let mut lib_dirs:    Vec<String> = Vec::new();
    let mut reg_paths:   Vec<String> = Vec::new();
    let mut entry_names: Vec<String> = Vec::new();
    let mut entry_stack_size: usize  = 1048576; // 1 MiB default

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--out"              if i + 1 < args.len() => { output = args[i+1].clone(); i += 2; }
            "--exe"                                    => { exe_flag = true; i += 1; }
            "--link-lib"         if i + 1 < args.len() => { link_libs.push(args[i+1].clone()); i += 2; }
            "--link-search"      if i + 1 < args.len() => { link_search.push(args[i+1].clone()); i += 2; }
            "--lib"              if i + 1 < args.len() => { lib_paths.push(args[i+1].clone()); i += 2; }
            "--lib-dir"          if i + 1 < args.len() => { lib_dirs.push(args[i+1].clone()); i += 2; }
            "--reg"              if i + 1 < args.len() => { reg_paths.push(args[i+1].clone()); i += 2; }
            "--entries"          if i + 1 < args.len() => {
                for name in args[i+1].split(',') {
                    let n = name.trim().to_string();
                    if !n.is_empty() { entry_names.push(n); }
                }
                i += 2;
            }
            "--entry"            if i + 1 < args.len() => { entry_names.push(args[i+1].clone()); i += 2; }
            "--entry-stack-size" if i + 1 < args.len() => {
                match args[i+1].parse::<usize>() {
                    Ok(n) => entry_stack_size = n,
                    Err(_) => {
                        eprintln!("error: --entry-stack-size must be a positive integer, got {:?}", args[i+1]);
                        std::process::exit(1);
                    }
                }
                i += 2;
            }
            _ => { i += 1; }
        }
    }

    // Detect 0.5 bundle: try 0.5 loader first; if it parses, use the 0.5 pipeline.
    if let Ok(bundle) = core_ir_05::load_core_bundle(input) {
        let fn_name = std::path::Path::new(input)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("main")
            .to_string();
        let lib_path = compute_lib_path(&output);
        let out_dir  = lib_path.parent().unwrap_or(std::path::Path::new(".")).to_owned();
        if let Err(e) = std::fs::create_dir_all(&out_dir) {
            eprintln!("error: cannot create output dir: {}", e); std::process::exit(1);
        }
        let exe_dir = std::env::current_exe().expect("current exe").parent().expect("exe dir").to_owned();
        let registry_map = rust_05::load_registry_identity_map(&reg_paths);

        // ── §5b provider resolution (--lib / --lib-dir) ────────────────────
        // Expand --lib-dir into lib_paths (same logic as the 0.4 path).
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

        // Load each provider bundle; key by sha256(fn_name) per §5b rule.
        let mut provider_map: HashMap<Hash256, (String, CoreBundle)> = HashMap::new();
        for lp in &lib_paths {
            let pbundle = match core_ir_05::load_core_bundle(lp) {
                Ok(b)  => b,
                Err(e) => { eprintln!("error: failed to load --lib {}: {}", lp, e); std::process::exit(1); }
            };
            let pfn = std::path::Path::new(lp)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if pfn.is_empty() {
                eprintln!("error: --lib {} has no usable filename stem", lp);
                std::process::exit(1);
            }
            let pid = sha256_bytes(pfn.as_bytes());
            provider_map.insert(pid, (pfn, pbundle));
        }

        // ── Named-entry resolution (BRIDGE_ENTRY_POINTS_V1) ─────────────────
        // Validate each named entry and collect §5b entry provider IDs for the
        // closure sweep. Built-in entries are ABI-checked (must have `in (TextList)`)
        // to satisfy ENTRY_ABI_MISMATCH. §5b entries must have a provider in the
        // --lib set (UNRESOLVED_ENTRY).
        let in_map = if !entry_names.is_empty() {
            rust_05::load_registry_in_map(&reg_paths)
        } else {
            HashMap::new()
        };
        let mut xbundle_entry_ids: Vec<Hash256> = Vec::new();
        for name in &entry_names {
            let eid = sha256_bytes(name.as_bytes());
            if rust_05::is_bridge_builtin(&eid) {
                match in_map.get(&eid) {
                    Some(in_clause) if in_clause.trim() == "(TextList)" => { /* OK */ }
                    Some(in_clause) => {
                        eprintln!("error: ENTRY_ABI_MISMATCH: '{}' is a foreign fn with `in {}` (expected (TextList))", name, in_clause);
                        std::process::exit(1);
                    }
                    None => {
                        eprintln!("error: ENTRY_ABI_MISMATCH: '{}' is a foreign fn with no `in` entry in registry (expected (TextList)) — add --reg", name);
                        std::process::exit(1);
                    }
                }
            } else if provider_map.contains_key(&eid) {
                xbundle_entry_ids.push(eid);
            } else if eid == sha256_bytes(fn_name.as_bytes()) {
                // Entry is the root bundle's own function. Its ax_fn_<eid> identity
                // export is already compiled into the root rlib — no separate provider.
            } else {
                eprintln!(
                    "error: UNRESOLVED_ENTRY: '{}' (identity {}...) — not a bridge built-in and no provider in --lib set",
                    name, &hash256_to_hex(&eid)[..16]
                );
                std::process::exit(1);
            }
        }

        // Collect the full transitive closure of §5b providers needed by the root
        // plus any §5b entry-point providers (extra_roots), using DFS with a visited
        // set for cycle safety (CYCLES_ARE_LEGAL).
        let all_providers = collect_xbundle_closure(&bundle, &provider_map, &xbundle_entry_ids);

        // Build xbundle_providers map: identity → "ax_fn_<hex>" symbol.
        // This covers the full closure so each bundle (root + providers) can emit
        // extern decls for any §5b target in the closure.
        let xbundle_providers: HashMap<Hash256, String> = provider_map.keys()
            .map(|id| (*id, format!("ax_fn_{}", hash256_to_hex(id))))
            .collect();

        // Compile each provider in the closure to its own rlib.
        //
        // Each rlib gets a UNIQUE Rust crate name (`ax_xb_<safe_pfn>`) so that
        // its `StableCrateId` — and therefore the mangling of every internal
        // monomorphization (`drop_in_place<Value>`, etc.) — is distinct from
        // every other downstream rlib's. Combined with the `--extern`-based
        // link at exe time (below), this is what makes multi-bundle --exe
        // dedup structurally instead of colliding.
        // (BRIDGE_XBUNDLE_LINK_DEDUP: SINGLE_DEFINITION_OF_DROP_GLUE.)
        let mut provider_rlibs: Vec<(String, std::path::PathBuf)> = Vec::new();
        for pid in &all_providers {
            let (pfn, pbundle) = provider_map.get(pid).expect("provider in closure");
            let safe_pfn = rust_05::sanitise(pfn);
            let provider_lib = out_dir.join(format!("lib{}_xb.a", safe_pfn));
            let provider_crate_name = format!("ax_xb_{}", safe_pfn);
            // Always recompile — never cache. Stale _xb.a silently links old
            // code when the source .coreir changes (build-always-recompiles).
            let pcode = match rust_05::emit_rust_lib_from_bundle(pbundle, pfn, &registry_map, &xbundle_providers) {
                Ok(c)  => c,
                Err(e) => { eprintln!("error (provider '{}'): {}", pfn, e); std::process::exit(1); }
            };
            let prs = out_dir.join(format!("generated_{}_xb.rs", safe_pfn));
            if let Err(e) = std::fs::write(&prs, &pcode) {
                eprintln!("error: cannot write provider rs {}: {}", prs.display(), e); std::process::exit(1);
            }
            let mut pcmd = std::process::Command::new("rustc");
            pcmd.arg(&prs)
                .arg("--crate-type=rlib")
                .arg(format!("--crate-name={}", provider_crate_name))
                .arg("--edition=2021")
                .arg("-o").arg(&provider_lib)
                .arg("-C").arg("embed-bitcode=no")
                .arg("-C").arg("strip=debuginfo")
                .arg("--extern").arg(format!("axis_codegen_bridge={}", find_bridge_rlib(&exe_dir).display()))
                .arg("-L").arg(format!("dependency={}/deps", exe_dir.display()));
            match pcmd.status() {
                Ok(s) if s.success() => {}
                Ok(s) => { eprintln!("error: rustc (provider '{}') exited {:?}", pfn, s.code()); std::process::exit(1); }
                Err(e) => { eprintln!("error: failed to invoke rustc for provider '{}': {}", pfn, e); std::process::exit(1); }
            }
            // rustc's --extern path requires `lib<crate_name>.rlib` / `.so`
            // filenames. Mirror the .a archive under the canonical .rlib name
            // so `--extern <provider_crate_name>=<.rlib>` resolves. The .a
            // stays in place for the existing single-bundle test contract.
            let provider_extern = out_dir.join(format!("lib{}.rlib", provider_crate_name));
            if let Err(e) = std::fs::copy(&provider_lib, &provider_extern) {
                eprintln!(
                    "error: failed to mirror provider rlib {} -> {}: {}",
                    provider_lib.display(),
                    provider_extern.display(),
                    e
                );
                std::process::exit(1);
            }
            provider_rlibs.push((provider_crate_name, provider_extern));
        }
        // ── end §5b provider resolution ─────────────────────────────────────

        let rust_code = match rust_05::emit_rust_lib_from_bundle(&bundle, &fn_name, &registry_map, &xbundle_providers) {
            Ok(code) => code,
            Err(e)   => { eprintln!("error: {}", e); std::process::exit(1); }
        };
        let generated_rs = out_dir.join("generated_lib.rs");
        if let Err(e) = std::fs::write(&generated_rs, &rust_code) {
            eprintln!("error: cannot write generated_lib.rs: {}", e); std::process::exit(1);
        }
        // Bundle rlib also gets a UNIQUE crate name so its mangling is
        // distinct from any provider rlib's. The shim links it via
        // `--extern <bundle_crate_name>=<rlib>` below.
        let safe_name = rust_05::sanitise(&fn_name);
        let bundle_crate_name = format!("ax_bundle_{}", safe_name);
        let mut cmd = std::process::Command::new("rustc");
        cmd.arg(&generated_rs)
           .arg("--crate-type=rlib")
           .arg(format!("--crate-name={}", bundle_crate_name))
           .arg("--edition=2021")
           .arg("-o").arg(&lib_path)
           .arg("-C").arg("embed-bitcode=no")
           .arg("-C").arg("strip=debuginfo")
           .arg("--extern").arg(format!("axis_codegen_bridge={}", find_bridge_rlib(&exe_dir).display()))
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
        // Mirror the bundle .a archive under the canonical lib<crate>.rlib
        // filename rustc's --extern path requires.
        // Always copy — never reuse a stale rlib. A cached .rlib compiled
        // against an old bridge version causes E0460 at link time with no
        // clear diagnostic (build-always-recompiles, same as provider path).
        let bundle_extern = out_dir.join(format!("lib{}.rlib", bundle_crate_name));
        if let Err(e) = std::fs::copy(&lib_path, &bundle_extern) {
            eprintln!(
                "error: failed to mirror bundle rlib {} -> {}: {}",
                lib_path.display(),
                bundle_extern.display(),
                e
            );
            std::process::exit(1);
        }

        // Render `extern crate <name>;` lines for every linked downstream
        // rlib (bundle + all providers). This is what couples each rlib into
        // the rustc dep graph so the final exe rustc resolves it via the
        // metadata-aware path (and dedupes shared upstream monomorphizations
        // like drop_in_place<Value>) instead of treating it as an opaque
        // -C link-arg= archive. (BRIDGE_XBUNDLE_LINK_DEDUP.)
        let extern_crate_lines: String = {
            let mut s = String::new();
            s += &format!("#[allow(unused_extern_crates)] extern crate {};\n", bundle_crate_name);
            for (name, _) in &provider_rlibs {
                s += &format!("#[allow(unused_extern_crates)] extern crate {};\n", name);
            }
            s
        };

        let shim_code = if entry_names.is_empty() {
            // Back-compat: single-root.
            let shim_fn = format!("_ax_exe_{}", safe_name);
            format!(
                "extern crate axis_codegen_bridge;\n\
                 {extern_crates}\
                 use axis_codegen_bridge::runtime::value::{{Value, init_runtime, intern_str}};\n\n\
                 #[allow(improper_ctypes)]\n\
                 extern \"C-unwind\" {{\n\
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
                extern_crates = extern_crate_lines,
                fn = shim_fn
            )
        } else {
            // Multi-entry thread driver (BRIDGE_ENTRY_POINTS_V1 §PART5).
            // One OS thread per entry; each receives the full argv as List(Text).
            // catch_unwind isolates panics per entry; exit code 1 if any panicked.
            // AdaptiveCell result sink (BRIDGE_TESTKIT_FINALIZE_V1): each entry writes
            // verdict 1u8 on success; main reads after join for per-entry PASS/FAIL.
            let mut s = String::new();
            s += "extern crate axis_codegen_bridge;\n";
            s += &extern_crate_lines;
            s += "use axis_codegen_bridge::runtime::value::{Value, init_runtime, intern_str};\n";
            s += "use axis_codegen_bridge::runtime::non_blocking_memory::{AdaptiveCell, AdaptiveRegistry};\n";
            s += "use std::sync::{Arc, Mutex};\n\n";

            // Extern block for §5b entry symbols (built-ins are called directly).
            let mut extern_syms: Vec<String> = Vec::new();
            for name in &entry_names {
                let eid = sha256_bytes(name.as_bytes());
                if !rust_05::is_bridge_builtin(&eid) {
                    let sym = format!("ax_fn_{}", hash256_to_hex(&eid));
                    if !extern_syms.contains(&sym) { extern_syms.push(sym); }
                }
            }
            if !extern_syms.is_empty() {
                s += "#[allow(improper_ctypes)]\nextern \"C-unwind\" {\n";
                for sym in &extern_syms {
                    s += &format!("    fn {}(args: Value) -> Value;\n", sym);
                }
                s += "}\n\n";
            }

            let n_entries = entry_names.len();
            s += "fn main() {\n";
            s += "    init_runtime();\n";
            s += "    let _argv: Vec<Value> = std::env::args().skip(1)\n";
            s += "        .map(|s| Value::Str(intern_str(&s)))\n";
            s += "        .collect();\n";
            s += "    let args = Value::List(_argv);\n";
            // Per-entry verdict sinks: harness-internal, opt-in via --entries.
            s += &format!("    let _sink_cells: Vec<Arc<Mutex<AdaptiveCell<u8>>>> = (0..{n}).map(|_| Arc::new(Mutex::new(AdaptiveCell::new()))).collect();\n", n = n_entries);
            s += &format!("    let _sink_regs: Vec<Arc<AdaptiveRegistry>>          = (0..{n}).map(|_| Arc::new(AdaptiveRegistry::new())).collect();\n", n = n_entries);
            s += "    let mut handles = Vec::new();\n\n";

            for (idx, name) in entry_names.iter().enumerate() {
                let eid = sha256_bytes(name.as_bytes());
                let call_expr = if let Some(path) = rust_05::builtin_path_for_identity(&eid) {
                    format!("{}(a)", path)
                } else {
                    format!("unsafe {{ ax_fn_{}(a) }}", hash256_to_hex(&eid))
                };
                s += "    {\n";
                s += "        let a = args.clone();\n";
                s += &format!("        let _sc = _sink_cells[{}].clone();\n", idx);
                s += &format!("        let _sr = _sink_regs[{}].clone();\n", idx);
                s += &format!("        let h = std::thread::Builder::new()\n");
                s += &format!("            .name({:?}.to_string())\n", name);
                s += &format!("            .stack_size({})\n", entry_stack_size);
                s += "            .spawn(move || std::panic::catch_unwind(\n";
                // On success: write PASS verdict (1u8) to sink before returning.
                // On panic: never reaches write; sink stays empty → FAIL.
                s += &format!("                std::panic::AssertUnwindSafe(move || {{ let _v = {}; let _ = unsafe {{ _sc.lock().unwrap().write(1u8, &*_sr) }}; _v }})\n", call_expr);
                s += &format!("            )).expect({:?});\n", format!("spawn {}", name));
                s += &format!("        handles.push(({:?}, {}usize, h));\n", name, idx);
                s += "    }\n\n";
            }

            s += "    let mut bad = false;\n";
            s += "    for (name, _idx, h) in handles {\n";
            s += "        match h.join() {\n";
            // Unit-returning entries: read verdict from sink and print PASS.
            s += "            Ok(Ok(v)) if matches!(v, Value::Unit) => {\n";
            s += "                let _sh = _sink_regs[_idx].acquire();\n";
            s += "                let _sv = { let _c = _sink_cells[_idx].lock().unwrap(); _c.read_pinned(&_sh, 0) };\n";
            s += "                let _ = _sv.as_ref().map(|r| r.is_zero_copy());\n";
            s += "                drop(_sv);\n";
            s += "                println!(\"{}: PASS\", name);\n";
            s += "            }\n";
            s += "            Ok(Ok(v))  => { println!(\"{}: {}\", name, v); }\n";
            s += "            Ok(Err(_)) => { eprintln!(\"{}: PANIC\", name); bad = true; }\n";
            s += "            Err(_)     => { eprintln!(\"{}: thread join failed\", name); bad = true; }\n";
            s += "        }\n";
            s += "    }\n";
            s += "    std::process::exit(if bad { 1 } else { 0 });\n";
            s += "}\n";
            s
        };

        let shim_rs  = out_dir.join("generated_main.rs");
        if let Err(e) = std::fs::write(&shim_rs, &shim_code) {
            eprintln!("error: cannot write generated_main.rs: {}", e); std::process::exit(1);
        }
        let bundle_extern_abs = std::fs::canonicalize(&bundle_extern).unwrap_or_else(|_| bundle_extern.clone());
        let mut cmd = std::process::Command::new("rustc");
        // Bundle + provider rlibs are linked as proper crate dependencies via
        // `--extern <crate>=<rlib>` (BRIDGE_XBUNDLE_LINK_DEDUP). This is what
        // makes rustc's metadata-aware linker logic dedup shared upstream
        // monomorphizations like `drop_in_place<Value>` across all rlibs into
        // a single definition at final link — the structural fix, NOT a
        // duplicate-tolerating linker flag.
        cmd.arg(&shim_rs)
           .arg("--edition=2021")
           .arg("-o").arg(&output)
           .arg("-C").arg("strip=debuginfo")
           .arg("--extern").arg(format!("axis_codegen_bridge={}", find_bridge_rlib(&exe_dir).display()))
           .arg("-L").arg(format!("dependency={}/deps", exe_dir.display()))
           .arg("-L").arg(format!("dependency={}", out_dir.display()))
           .arg("--extern").arg(format!("{}={}", bundle_crate_name, bundle_extern_abs.display()));
        for (name, prlib) in &provider_rlibs {
            let abs = std::fs::canonicalize(prlib).unwrap_or_else(|_| prlib.clone());
            cmd.arg("--extern").arg(format!("{}={}", name, abs.display()));
        }
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
       .arg("--extern").arg(format!("axis_codegen_bridge={}", find_bridge_rlib(&exe_dir).display()))
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
       .arg("--extern").arg(format!("axis_codegen_bridge={}", find_bridge_rlib(&exe_dir).display()))
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

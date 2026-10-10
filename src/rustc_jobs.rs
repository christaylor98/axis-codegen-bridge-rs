//! Parallel, content-addressed rustc for `build` (BRIDGE_BUILD_PARALLEL_CACHE_V1).
//!
//! `build` used to run every rustc one after another and never reuse an
//! output: on the axVerity write path that was 47 sequential rustc runs,
//! 19.4 s wall on a 32-core box using ~1 core. The runs inside one stage do
//! not depend on each other (§5b providers reach each other only through
//! `extern "C-unwind"` symbol decls, resolved at final link), so a stage is
//! handed here as one batch and run on a thread pool.
//!
//! The cache is what replaces "always recompile". That rule existed because a
//! stale archive silently linked old code; here an output is reused only when
//! every input that can change it hashes the same, so a stale hit is ruled out
//! structurally rather than by never hitting:
//!
//!   - `rustc -vV` (compiler identity),
//!   - the working directory and the source path AS PASSED — both are embedded
//!     in panic locations, so the same glue in a different build dir is a
//!     different output,
//!   - the source file's content and every argument, with each
//!     `--extern name=path` replaced by the CONTENT hash of that rlib (the
//!     bridge rlib included — a rebuilt bridge misses the whole cache),
//!   - every file and env var rustc's own dep-info reports (provider crates'
//!     `mod foo;` sub-files, `include_str!`, `env!`), re-verified on lookup.
//!
//! `AX_BUILD_CACHE` = a directory to use, or `off` / `0` to disable. Default:
//! `$XDG_CACHE_HOME/axis-codegen-bridge/rustc`, else `~/.cache/...`. Deleting
//! the directory at any time is safe. Nothing evicts entries yet.
//!
//! Entry layout: `<key>.manifest` (atomically renamed into place) names an
//! immutable blob `<key>-<manifest hash>.rlib` that was renamed into place
//! before it, so a concurrent reader never sees a half-written entry.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use axis_codegen_bridge::core_ir_05::{sha256_bytes, hash256_to_hex};

pub struct Job {
    /// Names the run in the error line: `error: <what> exited ...`.
    pub what: String,
    pub src: PathBuf,
    /// Every rustc argument except the source file, `-o` and `--emit`.
    /// `--extern` must be given as two args: `--extern`, `name=path`.
    pub args: Vec<String>,
    pub out: PathBuf,
}

enum Done {
    Ok { cached: bool, stderr: Vec<u8> },
    Failed { line: String, stderr: Vec<u8> },
}

pub struct Runner {
    cache: Option<PathBuf>,
    rustc_id: String,
    cwd: String,
    hashes: Mutex<HashMap<PathBuf, Option<String>>>,
    tmp_seq: AtomicUsize,
}

fn hash_hex(bytes: &[u8]) -> String { hash256_to_hex(&sha256_bytes(bytes)) }

fn cache_dir() -> Option<PathBuf> {
    match std::env::var("AX_BUILD_CACHE") {
        Ok(v) if v == "off" || v == "0" => return None,
        Ok(v) if !v.is_empty() => return Some(PathBuf::from(v)),
        _ => {}
    }
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("axis-codegen-bridge").join("rustc"))
}

impl Runner {
    pub fn new() -> Runner {
        let rustc_id = std::process::Command::new("rustc").arg("-vV").output().ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
        // No rustc identity, no cache: the compile itself will then fail with
        // the usual "failed to invoke rustc" line.
        let cache = rustc_id.as_ref().and_then(|_| cache_dir())
            .filter(|d| std::fs::create_dir_all(d.join("tmp")).is_ok());
        Runner {
            cache,
            rustc_id: rustc_id.unwrap_or_default(),
            cwd: std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default(),
            hashes: Mutex::new(HashMap::new()),
            tmp_seq: AtomicUsize::new(0),
        }
    }

    /// Content hash of a file, memoised for the life of this `build` (the
    /// bridge rlib is 8.6 MB and keys every job).
    fn file_hash(&self, p: &Path) -> Option<String> {
        if let Some(h) = self.hashes.lock().unwrap().get(p) { return h.clone(); }
        let h = std::fs::read(p).ok().map(|b| hash_hex(&b));
        self.hashes.lock().unwrap().insert(p.to_owned(), h.clone());
        h
    }

    fn key(&self, job: &Job) -> Option<String> {
        let mut k = format!("ax-rustc-cache v1\n{}\ncwd {}\nsrc {}\n{}\n",
            self.rustc_id, self.cwd, job.src.display(), self.file_hash(&job.src)?);
        let mut prev_extern = false;
        for a in &job.args {
            match (prev_extern, a.split_once('=')) {
                (true, Some((name, path))) => {
                    k += &format!("--extern {}={}\n", name, self.file_hash(Path::new(path))?);
                }
                _ => { k += a; k += "\n"; }
            }
            prev_extern = a == "--extern";
        }
        Some(hash_hex(k.as_bytes()))
    }

    /// Reuse `<key>.manifest` if every file and env var it lists is unchanged.
    fn lookup(&self, dir: &Path, key: &str, out: &Path) -> bool {
        let Ok(text) = std::fs::read_to_string(dir.join(format!("{}.manifest", key))) else { return false };
        let mut blob = None;
        for line in text.lines() {
            let f: Vec<&str> = line.splitn(3, '\t').collect();
            let ok = match f.as_slice() {
                ["blob", name] => { blob = Some(dir.join(name)); true }
                ["file", hex, path] => self.file_hash(Path::new(path)).as_deref() == Some(*hex),
                ["env", name, val] => std::env::var(name).ok().as_deref() == val.strip_prefix('='),
                _ => false,
            };
            if !ok { return false; }
        }
        match blob {
            Some(b) => std::fs::copy(&b, out).is_ok(),
            None => false,
        }
    }

    fn run_one(&self, job: &Job) -> Done {
        let cached = self.cache.as_ref().and_then(|d| self.key(job).map(|k| (d.clone(), k)));
        if let Some((dir, key)) = &cached {
            if self.lookup(dir, key, &job.out) {
                return Done::Ok { cached: true, stderr: Vec::new() };
            }
        }
        let seq = self.tmp_seq.fetch_add(1, Ordering::Relaxed);
        let (rustc_out, dep_info) = match &cached {
            Some((dir, key)) => {
                let t = dir.join("tmp").join(format!("{}.{}.{}", key, std::process::id(), seq));
                (t.with_extension("rlib"), Some(t.with_extension("d")))
            }
            None => (job.out.clone(), None),
        };
        let mut cmd = std::process::Command::new("rustc");
        cmd.arg(&job.src).args(&job.args).arg("-o").arg(&rustc_out);
        if let Some(d) = &dep_info {
            cmd.arg(format!("--emit=link,dep-info={}", d.display()));
        }
        let output = match cmd.output() {
            Ok(o) => o,
            Err(e) => return Done::Failed {
                line: format!("error: failed to invoke {}: {}", job.what, e), stderr: Vec::new(),
            },
        };
        if !output.status.success() {
            let _ = std::fs::remove_file(&rustc_out);
            if let Some(d) = &dep_info { let _ = std::fs::remove_file(d); }
            return Done::Failed {
                line: format!("error: {} exited {:?}", job.what, output.status.code()),
                stderr: output.stderr,
            };
        }
        if let (Some((dir, key)), Some(d)) = (&cached, &dep_info) {
            let stored = self.store(dir, key, &rustc_out, d);
            let _ = std::fs::remove_file(d);
            let copied = match &stored {
                Some(blob) => std::fs::copy(blob, &job.out).is_ok(),
                None => false,
            };
            if !copied {
                // Could not file it in the cache: the compile is still good.
                if std::fs::rename(&rustc_out, &job.out).is_err() && std::fs::copy(&rustc_out, &job.out).is_err() {
                    return Done::Failed {
                        line: format!("error: {}: cannot write {}", job.what, job.out.display()),
                        stderr: output.stderr,
                    };
                }
            }
            let _ = std::fs::remove_file(&rustc_out);
        }
        Done::Ok { cached: false, stderr: output.stderr }
    }

    /// Blob first, then the manifest naming it — each by atomic rename.
    fn store(&self, dir: &Path, key: &str, rlib: &Path, dep_info: &Path) -> Option<PathBuf> {
        let text = std::fs::read_to_string(dep_info).ok()?;
        let mut body = String::new();
        for line in text.lines() {
            if let Some(env) = line.strip_prefix("# env-dep:") {
                // `NAME=value`, or bare `NAME` when it was unset.
                match env.split_once('=') {
                    Some((n, v)) => body += &format!("env\t{}\t={}\n", n, v),
                    None => body += &format!("env\t{}\t\n", env),
                }
            } else if !line.starts_with('#') && line.ends_with(':') {
                // rustc lists every source it read as an empty rule `path:`.
                let path = line[..line.len() - 1].replace("\\ ", " ");
                body += &format!("file\t{}\t{}\n", self.file_hash(Path::new(&path))?, path);
            }
        }
        let blob = dir.join(format!("{}-{}.rlib", key, &hash_hex(body.as_bytes())[..16]));
        std::fs::rename(rlib, &blob).ok()?;
        let manifest = format!("blob\t{}\n{}", blob.file_name()?.to_str()?, body);
        let tmp = rlib.with_extension("manifest");
        std::fs::write(&tmp, manifest).ok()?;
        std::fs::rename(&tmp, dir.join(format!("{}.manifest", key))).ok()?;
        Some(blob)
    }

    /// Runs every job, at most `available_parallelism` at a time. Compiler
    /// output is replayed per job in job order (never interleaved). Any
    /// failure exits the process after all jobs have finished.
    pub fn run(&self, jobs: &[Job]) {
        if jobs.is_empty() { return; }
        let next = AtomicUsize::new(0);
        let results: Vec<Mutex<Option<Done>>> = jobs.iter().map(|_| Mutex::new(None)).collect();
        let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(jobs.len());
        std::thread::scope(|s| {
            for _ in 0..workers {
                s.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= jobs.len() { break; }
                    *results[i].lock().unwrap() = Some(self.run_one(&jobs[i]));
                });
            }
        });
        let (mut compiled, mut hits, mut failed) = (0, 0, None);
        for r in results {
            match r.into_inner().unwrap().expect("every job ran") {
                Done::Ok { cached, stderr } => {
                    if cached { hits += 1 } else { compiled += 1 }
                    eprint!("{}", String::from_utf8_lossy(&stderr));
                }
                Done::Failed { line, stderr } => {
                    eprint!("{}", String::from_utf8_lossy(&stderr));
                    failed.get_or_insert(line);
                }
            }
        }
        if let Some(line) = failed {
            eprintln!("{}", line);
            std::process::exit(1);
        }
        eprintln!("rustc: {} compiled, {} cached{}", compiled, hits,
            if self.cache.is_none() { " (cache off)" } else { "" });
    }
}

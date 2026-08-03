//! BRIDGE_ADJACENCY_V1 (AXVERITY_GRAPH_INTERFACE_V1) — the native graph
//! adjacency PROJECTION: a both-direction edge index held in RAM, rebuilt from
//! the object store, and NEVER persisted.
//!
//! ## Why this module exists
//!
//! AXVERITY_NATIVE_LIFT_V1 made a minted EDGE queryable by declaring it into
//! the field index at mint time (`lib/edge_index_add.m1`). That worked, but it
//! put a DERIVED structure on the durable write path: two `FIELDIDX` frames per
//! edge, measured at +4 fsyncs on a 6-fsync mint (10 total — 40% of the mint's
//! sync traffic, and the entire WAL portion of it). The governing hard limit is
//! `INDEXES_ARE_DERIVED_NEVER_DURABLE`: the log is durable, the index is not.
//! This module is the index side of honouring that. Nothing here writes a file.
//!
//! ## Rebuild source — the object store, NOT a log
//!
//! There is no edge log to replay. `fieldidx` recognises exactly two frame
//! shapes (`RECORD`, `FIELDIDX`) and an edge is written by `push_object` into
//! `.axverity/objects/`, never by `wal_push` — so no EDGE frame ever reaches
//! the WAL. The projection is therefore rebuilt by walking the store.
//!
//! BOTH TIERS ARE WALKED, and that is load-bearing. The committed backfill
//! driver (`scripts/axverity-edge-reindex.sh`) walks `.axverity/objects` only.
//! That was survivable while the index was durable — compaction could not
//! unmake an already-persisted posting list. It is NOT survivable for a rebuilt
//! projection: an edge migrated into a pack by `axverity-pack-run.sh` stays
//! perfectly readable through `pack_read` while becoming INVISIBLE to any
//! objects-only rebuild. Exercised before this module was written: after one
//! `pack-run`, `pull` still returned the edge and `edge-reindex` reported
//! `scanned=0 edges-indexed=0`. A RAM projection built on that walk would
//! silently lose adjacency for every compacted edge. So the pack POINTER index
//! (`.axverity/pack/index/<xx>/<62hex>`) is enumerated too — its paths encode
//! object addresses exactly like the loose tier.
//!
//! ## Byte-native by construction
//!
//! Object bodies are handled as BYTES here, start to finish. The M1 pack seam
//! is text-shaped — `pack_writer` does `bytes_to_text(store_read(addr))` and
//! records `str_len`, and `str_len`/`str_slice` are CHAR-counted, so pack
//! pointer offsets are char offsets and the whole path forces UTF-8. That seam
//! is fail-closed rather than lossy (`bytes_to_text` panics on invalid UTF-8),
//! but it means `pull` cannot return a binary object and `pack` cannot compact
//! one — both verified directly against a 512-byte random object. A graph layer
//! has no reason to inherit that: an edge frame is parsed here by splitting raw
//! bytes on `\t`, and only the endpoint/verb FIELDS — which are identifiers by
//! definition — are required to be UTF-8. A malformed body is skipped and
//! counted, never panicked on.
//!
//! One consequence is unavoidable until the pointer format carries byte
//! offsets: a packed object's extent is recorded in chars, so extracting it
//! requires knowing the char→byte mapping of its pack file. This module pays
//! that ONCE PER PACK FILE PER REBUILD (decode, cache, slice by char) instead of
//! once per lookup, and every pack file is valid UTF-8 by construction because
//! `pack_writer` could not have written it otherwise. That keeps the cost
//! bounded and the result exact. A byte-offset pointer alias would reduce it to
//! a pure seek; that is a frozen-seam change and is deliberately NOT made here.
//!
//! ## No shared registry
//!
//! Thread-local only — the same storage model as `logbuf.rs` / `walindex.rs` /
//! `fieldidx.rs`. No `Mutex`, no `RwLock`, no process-global registry.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::value::{get_str, intern_str, Value};

/// The projection. Two direction maps over the SAME edge objects.
///
/// `out[source] -> [edge_addr, ...]`  edges where the node is the SOURCE
/// `inn[target] -> [edge_addr, ...]`  edges where the node is the TARGET
///
/// Keyed by the endpoint STRING exactly as the edge object stores it. That is
/// deliberately whatever `make_edge` was given and nothing more: the corpus
/// carries content hashes on the source side and stable intent-ids on the
/// target side, and re-keying either one is a q1 question this module does not
/// answer. The projection is disposable, so a key change is a rebuild.
#[derive(Default)]
struct Adj {
    out: HashMap<String, Vec<String>>,
    inn: HashMap<String, Vec<String>>,
    edges: i64,
    scanned: i64,
    malformed: i64,
    loose: i64,
    packed: i64,
    built: bool,
    root: String,
}

thread_local! {
    static ADJ: RefCell<Adj> = RefCell::new(Adj::default());
}

fn arg_str(v: &Value, who: &str, i: usize) -> String {
    match v {
        Value::Str(h) => get_str(h),
        other => panic!("{}: arg {} expected Text, got {:?}", who, i, other),
    }
}

/// Reconstruct an object address from its two-level store path.
/// `<dir>/<xx>/<62hex>` -> `sha256:<xx><62hex>`, the same §5b layout the loose
/// object store and the pack pointer index both use.
fn addr_from_path(sub: &str, file: &str) -> Option<String> {
    if sub.len() != 2 || file.len() != 62 {
        return None;
    }
    if !sub.bytes().chain(file.bytes()).all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("sha256:{}{}", sub, file))
}

/// Parse `EDGE \t <source> \t <verb> \t <target> [\t attrs...]` out of RAW
/// BYTES.
///
/// No UTF-8 requirement on the body as a whole — only the three fields, which
/// are identifiers. Returns None for anything that is not a well-formed edge
/// frame so a non-edge object (or a truncated one) is skipped, not fatal.
///
/// AXSEM_W2_READ_COMPLETE_V1 phase-3: the frame may carry ADDITIVE attribute
/// fields after the target (ts= / tsprov= / ord= — mint time, its provenance
/// marker, and the caller-asserted ordinal). splitn(5) ends the target at the
/// fourth TAB, so the attrs are never silently swallowed into the target —
/// which is exactly what the previous splitn(4) would have done, silently
/// corrupting both the adjacency IN-key and every reported endpoint. The
/// 4-field form parses unchanged (alias path, never hard-replaced); this
/// projection only needs the triple, so the attr tail is not interpreted here.
fn parse_edge(body: &[u8]) -> Option<(String, String, String)> {
    const TAG: &[u8] = b"EDGE\t";
    if !body.starts_with(TAG) {
        return None;
    }
    let mut parts = body.splitn(5, |&b| b == b'\t');
    parts.next()?; // "EDGE"
    let src = parts.next()?;
    let verb = parts.next()?;
    let tgt = parts.next()?;
    let src = std::str::from_utf8(src).ok()?;
    let verb = std::str::from_utf8(verb).ok()?;
    let tgt = std::str::from_utf8(tgt).ok()?;
    if src.is_empty() || tgt.is_empty() {
        return None;
    }
    Some((src.to_string(), verb.to_string(), tgt.to_string()))
}

impl Adj {
    fn insert(&mut self, src: String, tgt: String, addr: String) {
        self.out.entry(src).or_default().push(addr.clone());
        self.inn.entry(tgt).or_default().push(addr);
        self.edges += 1;
    }

    /// Walk `.axverity/objects` — the loose tier. Raw bytes, no decode.
    fn scan_loose(&mut self, root: &Path, seen: &mut HashSet<String>) {
        let objdir = root.join(".axverity").join("objects");
        let subs = match fs::read_dir(&objdir) {
            Ok(d) => d,
            Err(_) => return, // no loose tier is not an error
        };
        for sub in subs.flatten() {
            if !sub.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let subname = sub.file_name().to_string_lossy().to_string();
            let files = match fs::read_dir(sub.path()) {
                Ok(f) => f,
                Err(_) => continue,
            };
            for f in files.flatten() {
                let fname = f.file_name().to_string_lossy().to_string();
                // `.ver` files are integrity metadata; `.tmp.*` are in-flight
                // writes from a concurrent push. Neither is an object.
                if fname.ends_with(".ver") || fname.contains(".tmp.") {
                    continue;
                }
                let addr = match addr_from_path(&subname, &fname) {
                    Some(a) => a,
                    None => continue,
                };
                self.scanned += 1;
                if !seen.insert(addr.clone()) {
                    continue;
                }
                let body = match fs::read(f.path()) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                match parse_edge(&body) {
                    Some((s, _v, t)) => {
                        self.loose += 1;
                        self.insert(s, t, addr);
                    }
                    None => {
                        if body.starts_with(b"EDGE") {
                            self.malformed += 1;
                        }
                    }
                }
            }
        }
    }

    /// Walk `.axverity/pack/index` — the compacted tier. THIS is the half the
    /// committed backfill driver is missing; without it a rebuilt projection
    /// silently drops every compacted edge.
    fn scan_packed(&mut self, root: &Path, seen: &mut HashSet<String>) {
        let idxdir = root.join(".axverity").join("pack").join("index");
        let subs = match fs::read_dir(&idxdir) {
            Ok(d) => d,
            Err(_) => return, // no pack tier is not an error
        };
        // One decode per pack file per rebuild, not per lookup. Every pack file
        // is valid UTF-8 by construction (pack_writer goes through
        // bytes_to_text, which panics otherwise), so this never loses bytes.
        let mut packs: HashMap<PathBuf, Option<Vec<char>>> = HashMap::new();

        for sub in subs.flatten() {
            if !sub.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let subname = sub.file_name().to_string_lossy().to_string();
            let files = match fs::read_dir(sub.path()) {
                Ok(f) => f,
                Err(_) => continue,
            };
            for f in files.flatten() {
                let fname = f.file_name().to_string_lossy().to_string();
                let addr = match addr_from_path(&subname, &fname) {
                    Some(a) => a,
                    None => continue,
                };
                self.scanned += 1;
                if !seen.insert(addr.clone()) {
                    continue; // already served by the loose tier
                }
                let meta = match fs::read_to_string(f.path()) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                // `<packid> \t <char_offset> \t <char_len>`
                let mut it = meta.split('\t');
                let (packid, offs, lens) = match (it.next(), it.next(), it.next()) {
                    (Some(a), Some(b), Some(c)) => (a, b, c),
                    _ => continue,
                };
                let (off, len) = match (offs.trim().parse::<usize>(), lens.trim().parse::<usize>())
                {
                    (Ok(o), Ok(l)) => (o, l),
                    _ => continue,
                };
                let packp = root
                    .join(".axverity")
                    .join("pack")
                    .join(format!("{}.pack", packid));
                let entry = packs.entry(packp.clone()).or_insert_with(|| {
                    fs::read(&packp)
                        .ok()
                        .and_then(|b| String::from_utf8(b).ok())
                        .map(|s| s.chars().collect())
                });
                let chars = match entry {
                    Some(c) => c,
                    None => continue,
                };
                let end = off.saturating_add(len).min(chars.len());
                if off >= end {
                    continue;
                }
                let obj: String = chars[off..end].iter().collect();
                match parse_edge(obj.as_bytes()) {
                    Some((s, _v, t)) => {
                        self.packed += 1;
                        self.insert(s, t, addr);
                    }
                    None => {
                        if obj.as_bytes().starts_with(b"EDGE") {
                            self.malformed += 1;
                        }
                    }
                }
            }
        }
    }

    fn build(&mut self, root: &str) {
        let base = Path::new(root);
        self.out.clear();
        self.inn.clear();
        self.edges = 0;
        self.scanned = 0;
        self.malformed = 0;
        self.loose = 0;
        self.packed = 0;
        let mut seen: HashSet<String> = HashSet::new();
        // Loose FIRST: pack-run verifies before deleting the loose copy, so a
        // crash mid-compaction can leave both. The loose file is the tier the
        // write path owns, so it wins the dedup.
        self.scan_loose(base, &mut seen);
        self.scan_packed(base, &mut seen);
        self.built = true;
        self.root = root.to_string();
    }
}

/// `adj_build(root: Text) -> Int` — force a full rebuild of the projection from
/// the object store (both tiers). Returns the edge count. Writes nothing.
#[track_caller]
pub fn adj_build(arg: Value) -> Value {
    let root = match arg {
        Value::Str(h) => get_str(&h),
        other => panic!("adj_build: expected Text root, got {:?}", other),
    };
    let root = if root.is_empty() { ".".to_string() } else { root };
    ADJ.with(|a| {
        let mut a = a.borrow_mut();
        a.build(&root);
        Value::Int(a.edges)
    })
}

/// `adj_get(dir: Text, node: Text) -> Text` — one direction of a node's
/// adjacency as an LF-terminated list of edge-object addresses.
///
/// `dir = "IN"`  edges where `node` is the TARGET (something points at it)
/// `dir = "OUT"` edges where `node` is the SOURCE (it points at something)
///
/// Return shape is deliberately identical to `field_lookup`'s — LF-joined and
/// LF-terminated, empty string when absent — so `lib/neighbors_dir.m1` swaps its
/// index read for this one without touching the fold that consumes it.
///
/// Builds the projection lazily on first use, from `.` (the binaries resolve
/// `.axverity` from CWD, same as every other store path).
#[track_caller]
pub fn adj_get(args: Value) -> Value {
    let es = match args {
        Value::Tuple(es) if es.len() == 2 => es,
        other => panic!("adj_get: expected Tuple(Text, Text), got {:?}", other),
    };
    let dir = arg_str(&es[0], "adj_get", 0);
    let node = arg_str(&es[1], "adj_get", 1);
    ADJ.with(|a| {
        let mut a = a.borrow_mut();
        if !a.built {
            a.build(".");
        }
        let map = if dir == "IN" { &a.inn } else { &a.out };
        let out = match map.get(&node) {
            Some(list) if !list.is_empty() => {
                let mut s = list.join("\n");
                s.push('\n');
                s
            }
            _ => String::new(),
        };
        Value::Str(intern_str(&out))
    })
}

/// `adj_stats(Unit) -> Text` — the projection's own account of its last build,
/// so a caller can assert coverage instead of trusting it. Shape:
/// `edges=<n> scanned=<n> loose=<n> packed=<n> malformed=<n> sources=<n> targets=<n>`
#[track_caller]
pub fn adj_stats(_arg: Value) -> Value {
    ADJ.with(|a| {
        let mut a = a.borrow_mut();
        if !a.built {
            a.build(".");
        }
        let s = format!(
            "edges={} scanned={} loose={} packed={} malformed={} sources={} targets={}",
            a.edges,
            a.scanned,
            a.loose,
            a.packed,
            a.malformed,
            a.out.len(),
            a.inn.len()
        );
        Value::Str(intern_str(&s))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_well_formed_edge_frame() {
        let b = b"EDGE\tsha256:aa\tcalls\tintent:x";
        let (s, v, t) = parse_edge(b).expect("should parse");
        assert_eq!(s, "sha256:aa");
        assert_eq!(v, "calls");
        assert_eq!(t, "intent:x");
    }

    #[test]
    fn an_attr_carrying_edge_frame_keeps_a_clean_target() {
        // AXSEM_W2_READ_COMPLETE_V1 phase-3: additive attr fields end the
        // target at the fourth TAB instead of being swallowed into it.
        let b = b"EDGE\tsha256:aa\tcalls\tintent:x\tts=2026-07-04T02:23:57.123Z\ttsprov=asserted\tord=41";
        let (s, v, t) = parse_edge(b).expect("should parse");
        assert_eq!(s, "sha256:aa");
        assert_eq!(v, "calls");
        assert_eq!(t, "intent:x");
    }

    #[test]
    fn a_verb_may_be_empty_but_endpoints_may_not() {
        assert!(parse_edge(b"EDGE\tsrc\t\ttgt").is_some());
        assert!(parse_edge(b"EDGE\t\tcalls\ttgt").is_none());
        assert!(parse_edge(b"EDGE\tsrc\tcalls\t").is_none());
    }

    #[test]
    fn non_edge_and_truncated_bodies_are_skipped_not_fatal() {
        assert!(parse_edge(b"RECORD\tcolor=red").is_none());
        assert!(parse_edge(b"EDGE\tsrc\tcalls").is_none());
        assert!(parse_edge(b"").is_none());
    }

    #[test]
    fn a_target_holding_a_stable_id_survives_parsing_unchanged() {
        // The corpus is mixed: hash on the source side, intent-id on the
        // target side. The projection must not care.
        let b = b"EDGE\tsha256:deadbeef\tcontains\tcode:m1:lib.foo::foo";
        let (_, _, t) = parse_edge(b).unwrap();
        assert_eq!(t, "code:m1:lib.foo::foo");
    }

    #[test]
    fn address_reconstruction_rejects_non_object_paths() {
        assert_eq!(
            addr_from_path("ab", &"c".repeat(62)),
            Some(format!("sha256:ab{}", "c".repeat(62)))
        );
        assert_eq!(addr_from_path("ab", "short"), None);
        assert_eq!(addr_from_path("zz", &"c".repeat(62)), None);
    }
}

//! Ties `runtime::Value`'s variant list to the `type Value = sum [...]` arms
//! declared in this crate's registries.
//!
//! WHY THIS EXISTS. A registry `.axreg` is an unchecked description of the
//! runtime. A fn's identity is authored literally and never minted from its
//! signature, and a CCall records only that authored identity, so changing a
//! type's arms moves no fn identity, no body hash and no bundle identity —
//! verified by recompiling one source against a 4-arm and a 7-arm `Value` and
//! diffing: byte-identical bundles. That is what makes adding an arm safe, and
//! it is also what let four arms stand against ten runtime variants with
//! nothing signalling the drift. `axis-types.axreg` even carried a comment
//! asserting the four were "the closed, finite typing vocabulary: it cannot
//! grow because the bridge runtime cannot produce a value outside these
//! variants" — while declaring `type Bytes = prim bytes` thirty lines below.
//!
//! The cost of the drift is not cosmetic. `type_admits`
//! (axis-lang-lab `validation/core_ir.rs`) admits an argument of type T into an
//! `in (Value)` slot iff some arm has exactly one payload and that payload is
//! T. A missing arm is a rejected program: with the four-arm `Value`, passing a
//! `Dec`, a `Float` or a `Bytes` into any `Value` slot failed with
//! `TypeMismatch` while `Text`/`Int`/`Bool` passed.
//!
//! WHAT THIS TEST ENFORCES. Every variant of `runtime::Value` is accounted for:
//! either it has a declared single-payload arm, or it is on `UNEXPRESSIBLE`
//! with a reason. Adding a variant to the enum without resolving it here fails
//! the build. `UNEXPRESSIBLE` is deliberately a short, explicit list rather
//! than a wildcard — adding to it is a decision someone has to write down.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Runtime variants that CANNOT be declared as a sum arm in the registry
/// grammar, with the reason. All three are recursive — their payload is
/// `Value`, or a list of it — and the grammar has no recursive types. The
/// honest arm `AsList(ValueList)` (with `ValueList = list Value`) is rejected
/// at load time:
///
///   cannot resolve type(s) [Value, ValueList] — unknown referenced type name
///   or dependency cycle
///
/// which is the correct answer to an ill-founded definition. The consequence
/// is a real, permanent hole: a `ValueList` or a `Tuple` can never implicitly
/// widen into a `Value` slot. It costs nothing today because both already
/// travel as their own declared types (`in (ValueList)`, and `tcp_listen`'s
/// pair is declared `out Value`), so no widening site exists. A new fn that
/// must accept "a list or a scalar" is two fns, not one `Value` fn.
const UNEXPRESSIBLE: &[(&str, &str)] = &[
    ("Tuple", "Value::Tuple(Vec<Value>) — recursive, payload is Value"),
    ("List", "Value::List(Vec<Value>) — recursive; AsList(ValueList) is a load-time cycle"),
    ("Ctor", "Value::Ctor{tag, fields} — recursive, fields are Value"),
];

/// Registries in this crate that declare `type Value` as a sum.
const REGISTRIES: &[&str] = &["registry/axis.axreg", "registry/axis-codegen-bridge.axreg"];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Variant names of `pub enum Value` in src/runtime/value.rs.
///
/// Read from source rather than derived from the type: there is no reflection
/// for this, and a `match` over `Value` would compile-error on a new variant
/// only where it is exhaustive — which is the check we want, but it would not
/// tell us the variant's NAME to compare against an arm. Comment and attribute
/// lines are skipped; the enum body is taken up to the first closing brace at
/// column 0.
fn runtime_variants() -> BTreeSet<String> {
    let src = std::fs::read_to_string(repo_root().join("src/runtime/value.rs"))
        .expect("src/runtime/value.rs must be readable");
    let start = src
        .find("pub enum Value {")
        .expect("src/runtime/value.rs must declare `pub enum Value {`");
    let body = &src[start..];
    let end = body.find("\n}").expect("enum Value must be brace-closed");
    let mut out = BTreeSet::new();
    for line in body[..end].lines().skip(1) {
        let t = line.trim();
        if t.is_empty() || t.starts_with("//") || t.starts_with("#[") {
            continue;
        }
        let name: String = t
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            out.insert(name);
        }
    }
    assert!(
        out.len() >= 4,
        "parsed too few variants from enum Value ({out:?}) — the parser above \
         has drifted from the source layout, fix it rather than the assert"
    );
    out
}

/// Single-payload arm payload type names from `type Value = sum [ ... ]`.
/// Returns e.g. {"Unit", "Bool", "Int", "Text", "Dec", "Float", "Bytes"}.
fn declared_arm_payloads(reg: &Path) -> BTreeSet<String> {
    let text = std::fs::read_to_string(reg)
        .unwrap_or_else(|e| panic!("{} must be readable: {e}", reg.display()));
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("type Value") && l.contains("sum"))
        .unwrap_or_else(|| panic!("{} must declare `type Value = sum [...]`", reg.display()));
    let open = line.find('[').expect("sum needs [");
    let close = line.rfind(']').expect("sum needs ]");
    let mut out = BTreeSet::new();
    for arm in line[open + 1..close].split('|') {
        let arm = arm.trim();
        // `AsText(Text)` -> payload `Text`. A nullary or multi-field arm never
        // participates in widening (type_admits requires exactly one payload),
        // so it is not collected.
        let (Some(o), Some(c)) = (arm.find('('), arm.rfind(')')) else { continue };
        let payload = arm[o + 1..c].trim();
        if !payload.is_empty() && !payload.contains(',') {
            out.insert(payload.to_string());
        }
    }
    out
}

/// Runtime variant name -> the registry type name its payload is declared as.
/// `Str` is the only one whose names differ: the enum calls it `Str`, the
/// registry calls the type `Text`.
fn registry_type_for(variant: &str) -> &'static str {
    match variant {
        "Int" => "Int",
        "Bool" => "Bool",
        "Str" => "Text",
        "Unit" => "Unit",
        "Dec" => "Dec",
        "Float" => "Float",
        "Bytes" => "Bytes",
        other => panic!(
            "runtime::Value::{other} is new: give it a sum arm in {REGISTRIES:?} \
             and a mapping here, or add it to UNEXPRESSIBLE with the reason it \
             cannot be declared"
        ),
    }
}

#[test]
fn every_runtime_value_variant_is_declared_or_explicitly_unexpressible() {
    let variants = runtime_variants();
    let unexpressible: BTreeSet<&str> = UNEXPRESSIBLE.iter().map(|(n, _)| *n).collect();

    for reg in REGISTRIES {
        let path = repo_root().join(reg);
        let declared = declared_arm_payloads(&path);
        let mut missing = Vec::new();
        for v in &variants {
            if unexpressible.contains(v.as_str()) {
                continue;
            }
            let want = registry_type_for(v);
            if !declared.contains(want) {
                missing.push(format!("Value::{v} (needs an arm with payload `{want}`)"));
            }
        }
        assert!(
            missing.is_empty(),
            "{reg}: `type Value` is missing an arm for {} runtime variant(s):\n  {}\n\
             A missing arm is a REJECTED PROGRAM: type_admits only widens a scalar \
             into an `in (Value)` slot through a single-payload arm, so an \
             undeclared variant cannot be passed to any Value-typed fn.\n\
             Declared payloads: {declared:?}",
            missing.len(),
            missing.join("\n  ")
        );
    }
}

#[test]
fn unexpressible_list_names_only_real_variants() {
    // Guards the other direction: an entry that no longer matches a variant is
    // stale and would silently excuse a future variant of the same name.
    let variants = runtime_variants();
    for (name, reason) in UNEXPRESSIBLE {
        assert!(
            variants.contains(*name),
            "UNEXPRESSIBLE lists `{name}` ({reason}) but runtime::Value has no \
             such variant — remove the stale entry"
        );
    }
}

#[test]
fn declared_arms_agree_across_this_crates_registries() {
    // The two registries are separate files with the same obligation. They
    // drifted once already; this keeps a fix to one from missing the other.
    let sets: Vec<(&str, BTreeSet<String>)> = REGISTRIES
        .iter()
        .map(|r| (*r, declared_arm_payloads(&repo_root().join(r))))
        .collect();
    let (first_name, first) = &sets[0];
    for (name, set) in &sets[1..] {
        assert_eq!(
            first, set,
            "`type Value` arms differ between {first_name} and {name}"
        );
    }
}

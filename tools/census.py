"""
census -- every registry builtin, classified by what it does when its result isn't defined (FAULT_AS_UNKNOWN).

    python3 tools/census.py [--csv out.csv] [--examples N]

For each `fn` in the canonical registries: its Rust implementation (symbol_map in src/emit/rust_05.rs), the body
of that function plus one level of same-crate callees, and the markers found in them:

  panics           panic!/unwrap()/expect(/unreachable!/assert!, slice indexing `[..]`, integer `/` or `%`
                   split: TYPE (the message says "expected ..." -- a guard the verifier should make unreachable)
                          vs DOMAIN (empty list, division by zero, an IO error: a real undefined case)
  silent default   unwrap_or / unwrap_or_default / unwrap_or_else, an Err/None arm returning a plain value,
                   `let _ =` / `.ok()` dropping a Result
  silent wrong     unchecked integer + - * on i64, wrapping_*, `as` casts that can truncate or flip sign
  total            none of these
  no impl          declared in the registry, no bridge implementation

These are PATTERNS over source text, not a proof: a marker can sit on a path that can't be reached (false
positive), and a callee two levels down isn't read (false negative). Each row keeps the evidence (the matched
lines) so a classification can be checked by hand.
"""
from __future__ import annotations

import argparse
import collections
import csv
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
AXR = Path("/home/chris/dev/axRegistry-working")
REGS = ["axis-types", "axis", "axis-bridge", "m1-builtins", "gen-working"]


def registry():
    out = {}
    for r in REGS:
        cur = None
        for line in (AXR / f"{r}.axreg").read_text().splitlines():
            s = line.strip()
            if m := re.match(r"^fn\s+(\S+)", s):
                cur = {"name": m.group(1), "reg": r, "kind": "", "effect": "", "deterministic": "", "in": "", "out": ""}
            elif cur and (m := re.match(r"^(kind|effect|deterministic|in|out)\s+(.*)", s)):
                cur[m.group(1)] = m.group(2).strip()
            elif cur and s == "end":
                out.setdefault(cur["name"], cur)
                cur = None
    return out


def symbol_map():
    src = (ROOT / "src/emit/rust_05.rs").read_text()
    block = re.search(r"^fn symbol_map\(\).*?^}", src, re.S | re.M).group(0)
    return dict(re.findall(r'm\.insert\(\s*"([^"]+)",\s*"(axis_codegen_bridge::[^"]+)"\)', block))


_files: dict[str, str] = {}


def _file(mod: str) -> str:
    if mod not in _files:
        p = ROOT / "src/runtime" / f"{mod}.rs"
        _files[mod] = p.read_text() if p.exists() else ""
    return _files[mod]


def fn_body(mod: str, fn: str) -> str | None:
    src = _file(mod)
    m = re.search(rf"\bfn\s+{re.escape(fn)}\s*(<[^>]*>)?\s*\(", src)
    if not m:
        return None
    i = src.index("{", m.end())
    depth, j = 0, i
    while j < len(src):
        c = src[j]
        if c == '"':                                   # skip string literals
            j += 1
            while j < len(src) and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
        elif c == "'" and re.match(r"'(\\.|[^\\'])'", src[j:j + 4]):
            j += re.match(r"'(\\.|[^\\'])'", src[j:j + 4]).end() - 1
        elif c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return src[m.start():j + 1]
        j += 1
    return None


def with_callees(mod: str, body: str) -> str:
    """The body plus one level of same-crate callees (`super::m::f(`, or a bare `f(` defined in this module)."""
    parts = [body]
    for m2, f2 in set(re.findall(r"super::(\w+)::(\w+)\s*\(", body)):
        b = fn_body(m2, f2)
        if b:
            parts.append(b)
    for f2 in set(re.findall(r"(?<![\w:.])([a-z_]\w*)\s*\(", body)):
        if f2 in ("fn", "if", "match", "while", "for", "Some", "Ok", "Err", "vec", "format", "panic"):
            continue
        b = fn_body(mod, f2)
        if b and b != body:
            parts.append(b)
    return "\n".join(parts)


POISON = re.compile(r"\.(lock|read|write)\(\)\s*\.(unwrap\(\)|expect\()|into_inner\(\)")
PAT = {
    "panic": re.compile(r"\bpanic!\s*\(|\.unwrap\(\)|\.expect\(|\bunreachable!|\bassert(_eq|_ne)?!|\btodo!|\bunimplemented!"),
    "index": re.compile(r"\b\w+\s*\[\s*[^\]\[]*?\b(as usize|idx|i|j|k|n|pos|index|off\w*)\b[^\]\[]*\]"),
    "intdiv": re.compile(r"\b[xyab]\s*[/%]\s*[xyab]\b|\.div_euclid\(|\.rem_euclid\("),
    "default": re.compile(r"\.unwrap_or\(|\.unwrap_or_default\(|\.unwrap_or_else\(|Err\(_\)\s*=>\s*(Value::|return Value::)|None\s*=>\s*Value::(Unit|Bool\(false\)|Int\(0\))|\.ok\(\)\s*;"),
    # i64 -> usize flips a negative to a huge index; narrowing casts truncate; unchecked i64 arithmetic wraps.
    # `.len() as i64` and friends are left out: a length can't reach i64::MAX.
    "wrong": re.compile(r"\bValue::Int\(\s*\w+\s*[-+*]\s*\w+\s*\)|\.wrapping_\w+\(|\b(n|i|idx|x|y|v|k|off\w*|pos|start|end|len)\s+as\s+usize\b|\bas\s+(i32|u32|u16|u8)\b"),
}


def classify(body: str):
    ev = collections.defaultdict(list)
    for line in body.splitlines():
        t = line.strip()
        if t.startswith("//"):
            continue
        if POISON.search(t):
            ev["poison"].append(t[:140])
            t = POISON.sub("", t)
        for k, p in PAT.items():
            if p.search(t):
                if k == "default" and "panic!" in t:      # unwrap_or_else(|e| panic!(..)) is a panic, not a default
                    continue
                if k == "wrong" and (re.search(r"\.get\(\s*\*?\w+\s+as\s+usize", t)      # .get() is bounds-safe
                                     or re.search(r"\bh\s*=\s*h\.wrapping_", t)):          # FNV-style hashing: intended
                    continue
                ev[k].append(t[:140])
    cats = set()
    if ev["panic"] or ev["index"] or ev["intdiv"]:
        msgs = " ".join(ev["panic"])
        domain = (ev["index"] or ev["intdiv"] or ev["panic"] and
                  any("expected" not in l.lower() for l in ev["panic"]))
        cats.add("panics:domain" if domain else "panics:type-only")
    if ev["poison"]:
        cats.add("poison-only" if not cats else "poison")
    if ev["default"]:
        cats.add("silent-default")
    if ev["wrong"]:
        cats.add("silent-wrong")
    cats.discard("poison")
    return cats or {"total"}, ev


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--csv", default="")
    ap.add_argument("--examples", type=int, default=3)
    a = ap.parse_args()
    reg, sym = registry(), symbol_map()
    rows = []
    for name, r in sorted(reg.items()):
        if r["kind"] == "composite":
            continue
        path = sym.get(name)
        if not path:
            rows.append({**r, "path": "", "cats": {"no-impl"}, "ev": {}})
            continue
        parts = path.split("::")
        mod, fn = parts[2], parts[-1]
        body = fn_body(mod, fn)
        if body is None:
            rows.append({**r, "path": path, "cats": {"impl-not-found"}, "ev": {}})
            continue
        cats, ev = classify(with_callees(mod, body))
        rows.append({**r, "path": path, "cats": cats, "ev": ev})

    n = len(rows)
    print(f"builtins (leaf, canonical registries): {n}")
    order = ["total", "poison-only", "panics:domain", "panics:type-only", "silent-default", "silent-wrong", "no-impl", "impl-not-found"]
    c = collections.Counter(k for r in rows for k in r["cats"])
    for k in order:
        print(f"  {k:18} {c[k]:4}  ({c[k] / n:.0%})")
    print("  (a builtin can be in several; 'total' = none found)")
    by_eff = collections.defaultdict(collections.Counter)
    for r in rows:
        by_eff[r["effect"] or "?"]["n"] += 1
        for k in r["cats"]:
            by_eff[r["effect"] or "?"][k] += 1
    print("\nby declared effect:")
    for e, cc in sorted(by_eff.items(), key=lambda x: -x[1]["n"]):
        print(f"  {e:14} n={cc['n']:4}  " + "  ".join(f"{k}={cc[k]}" for k in order if cc[k]))
    liars = [r for r in rows if r["effect"] == "pure" and r["cats"] & {"panics:domain", "silent-wrong", "silent-default"}]
    print(f"\ndeclared `effect pure` yet can panic on a domain case or return a silent default/wrong value: {len(liars)}")
    for k in ("panics:domain", "silent-default", "silent-wrong"):
        ex = [r for r in rows if k in r["cats"]][:a.examples]
        print(f"\nexamples -- {k}:")
        for r in ex:
            key = {"panics:domain": ("panic", "index", "intdiv"), "silent-default": ("default",),
                   "silent-wrong": ("wrong",)}[k]
            line = next((l for kk in key for l in r["ev"].get(kk, [])), "")
            print(f"  {r['name']:26} [{r['effect']}]  {line}")
    if a.csv:
        with open(a.csv, "w", newline="") as f:
            w = csv.writer(f)
            w.writerow(["name", "effect", "deterministic", "in", "out", "path", "categories", "evidence"])
            for r in rows:
                evs = " | ".join(f"{k}: {l}" for k, ls in r["ev"].items() for l in ls[:2])
                w.writerow([r["name"], r["effect"], r["deterministic"], r["in"], r["out"], r["path"],
                            ";".join(sorted(r["cats"])), evs])


if __name__ == "__main__":
    main()

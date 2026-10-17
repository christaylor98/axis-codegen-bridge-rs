//! AX146 steps B and C: the speculative fast paths must be INVISIBLE except for speed.
//!
//! Every scenario is built twice from the same Core IR: once as shipped (fast paths on) and once with `--no-fast`
//! (generic code only). Each is run on a battery of inputs and must give byte-identical stdout, stderr and exit code.
//! The battery includes every way the fast path can bail: overflow, division by zero, a wrong type, a sticky Unknown,
//! an index out of range, a non-ASCII character, an effectful arm. Where the generic behaviour is a DEFECT (exit 4) or
//! an Unknown (exit 3) the test pins that too, so "identical" cannot mean "identically wrong".
//! A scenario also asserts the fast path really was emitted (so the comparison is not vacuous).
use axis_codegen_bridge::core_ir_05::{
    bool_type_hash, fn_type_hash, int_type_hash, param_type_hash, sha256_bytes, text_type_hash,
    serialiser::create_core_bundle_05, ConstantPoolEntry, CoreBundle, Node, NodeRef,
    encode_int_payload, encode_text_payload, encode_float_payload, encode_dec_payload, float_type_hash, dec_type_hash,
};
use std::process::Command;
use tempfile::TempDir;

fn bridge() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_axis-codegen-bridge"))
}

/// A tiny Core IR 0.6 builder.
struct P {
    pool: Vec<ConstantPoolEntry>,
    nodes: Vec<Node>,
}

impl P {
    fn new() -> P { P { pool: vec![], nodes: vec![] } }
    fn pool(&mut self, def_hash: [u8; 32], payload: Vec<u8>) -> NodeRef {
        self.pool.push(ConstantPoolEntry { def_hash, payload });
        NodeRef::Pool(self.pool.len() as u32 - 1)
    }
    fn int(&mut self, v: i64) -> NodeRef { self.pool(int_type_hash(), encode_int_payload(v)) }
    fn text(&mut self, s: &str) -> NodeRef { self.pool(text_type_hash(), encode_text_payload(s)) }
    fn float(&mut self, v: f64) -> NodeRef { self.pool(float_type_hash(), encode_float_payload(v)) }
    fn dec(&mut self, v: &str) -> NodeRef { self.pool(dec_type_hash(), encode_dec_payload(v.parse().unwrap())) }
    fn param(&mut self, slot: u8) -> NodeRef { self.pool(param_type_hash(), vec![slot]) }
    fn fnref(&mut self, name: &str) -> NodeRef { self.pool(fn_type_hash(), sha256_bytes(name.as_bytes()).to_vec()) }
    fn call(&mut self, name: &str, args: Vec<NodeRef>) -> NodeRef {
        self.nodes.push(Node::CCall { target_identity: sha256_bytes(name.as_bytes()), target_name: name.to_string(), args });
        NodeRef::Node(self.nodes.len() as u32 - 1)
    }
    /// a block whose value is what `f` returns; the index of its CEnd
    fn arm(&mut self, f: impl FnOnce(&mut P) -> NodeRef) -> u32 {
        self.nodes.push(Node::CBlock);
        let v = f(self);
        self.nodes.push(Node::CEnd { value: v });
        self.nodes.len() as u32 - 1
    }
    fn if_(&mut self, cond: NodeRef, t: impl FnOnce(&mut P) -> NodeRef, e: impl FnOnce(&mut P) -> NodeRef) -> NodeRef {
        let (te, ee) = (self.arm(t), self.arm(e));
        self.nodes.push(Node::CIf { cond, then_: NodeRef::Node(te), else_: NodeRef::Node(ee) });
        NodeRef::Node(self.nodes.len() as u32 - 1)
    }
    fn done(self, result: NodeRef) -> CoreBundle {
        CoreBundle { version: "0.6".into(), constant_pool: self.pool, nodes: self.nodes, result }
    }
}

/// The builtins the scenarios use that the registry declares pure + deterministic (so they are fast-able).
const PURE: &[&str] = &[
    "int_add", "int_sub", "int_mul", "int_div", "int_mod", "int_lt", "int_lte", "int_gt", "int_eq", "int_bit_and",
    "int_bit_xor", "int_shr", "int_bit_or", "bool_not", "bool_and", "value_to_int", "value_to_text", "ctor_field", "value_make",
    "str_trim", "str_to_float", "str_to_dec", "int_to_float", "int_to_dec", "float_id", "dec_id", "str_len", "str_char", "float_add", "float_sub", "float_mul", "float_div", "float_neg", "float_abs", "float_sqrt",
    "float_floor", "float_ceil", "float_round", "float_pow", "float_min", "float_max", "float_is_nan", "float_lt", "float_lte",
    "float_gt", "float_gte", "float_eq", "dec_add", "dec_sub", "dec_mul", "dec_div", "dec_round", "dec_neg", "dec_abs",
    "dec_min", "dec_max", "dec_lt", "dec_lte", "dec_gt", "dec_gte", "dec_eq", "str_char", "str_char_at", "str_eq", "text_eq", "value_0", "str_to_int", "int_to_str", "tuple_field",
];

fn registry(dir: &TempDir) -> std::path::PathBuf {
    let mut reg = "registry fast-path-test 0.5\n\n".to_string();
    for f in PURE {
        let ident: String = sha256_bytes(f.as_bytes()).iter().map(|b| format!("{:02x}", b)).collect();
        reg.push_str(&format!(
            "fn {f}\n  identity 0x{ident}\n  kind leaf\n  in (Value)\n  out Value\n  effect pure\n  deterministic true\n  idempotent true\nend\n\n"
        ));
    }
    let p = dir.path().join("pure.axreg");
    std::fs::write(&p, reg).unwrap();
    p
}

struct Built {
    dir: String,
    exe: std::path::PathBuf,
    glue: String,
    _dir: TempDir,
}

/// Build `root` (entry) with `libs` (name, bundle) as --lib providers.
fn build(root: &CoreBundle, libs: &[(&str, &CoreBundle)], fast: bool) -> Built {
    let dir = TempDir::new().unwrap();
    let reg = registry(&dir);
    let root_path = dir.path().join("root.coreir");
    std::fs::write(&root_path, create_core_bundle_05(root)).unwrap();
    let out = dir.path().join("prog");
    let mut cmd = Command::new(bridge());
    cmd.args(["build", root_path.to_str().unwrap(), "--out", out.to_str().unwrap(), "--exe", "--reg", reg.to_str().unwrap()]);
    for (name, b) in libs {
        let p = dir.path().join(format!("{name}.coreir"));
        std::fs::write(&p, create_core_bundle_05(b)).unwrap();
        cmd.args(["--lib", p.to_str().unwrap()]);
    }
    if !fast {
        cmd.arg("--no-fast");
    }
    let o = cmd.output().expect("bridge failed to run");
    assert!(o.status.success(), "build failed:\n{}", String::from_utf8_lossy(&o.stderr));
    let mut glue = String::new();
    for e in std::fs::read_dir(dir.path()).unwrap().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.starts_with("generated_") && n.ends_with(".rs") && n != "generated_main.rs" {
            glue.push_str(&std::fs::read_to_string(e.path()).unwrap());
        }
    }
    Built { dir: dir.path().to_string_lossy().into_owned(), exe: out, glue, _dir: dir }
}

#[derive(Debug, PartialEq)]
struct Run { code: Option<i32>, stdout: String, stderr: String }

fn run(b: &Built, argv: &[&str]) -> Run {
    let o = Command::new(&b.exe).args(argv).output().expect("run");
    // panic locations are generated-file:line inside a per-build temp dir: compare everything but the dir
    Run { code: o.status.code(), stdout: String::from_utf8_lossy(&o.stdout).into_owned(), stderr: String::from_utf8_lossy(&o.stderr).replace(&b.dir, "<D>") }
}

/// Build both ways, assert the fast build has `expect_fast` hooks and the generic has none, run every `argv` on both and
/// require identical results. Returns the (identical) results.
fn differential(root: &CoreBundle, libs: &[(&str, &CoreBundle)], expect_fast: &[&str], cases: &[&[&str]]) -> Vec<Run> {
    let (f, g) = (build(root, libs, true), build(root, libs, false));
    for name in expect_fast {
        assert!(f.glue.contains(&format!("if let Some(__fast) = {name}_fast(&args)")), "no fast path emitted for {name}:\n{}", f.glue);
        assert!(!g.glue.contains("__fast"), "--no-fast still emitted a fast path");
    }
    let mut out = vec![];
    for argv in cases {
        let (a, b) = (run(&f, argv), run(&g, argv));
        assert_eq!(a, b, "fast and generic differ on argv {:?}", argv);
        out.push(a);
    }
    out
}

fn argv_int(p: &mut P, i: i64) -> NodeRef {
    let (idx, dflt) = (p.int(i), p.text("0"));
    let t = p.call("argv_or", vec![idx, dflt]);
    p.call("str_to_int", vec![t])
}

// ── 1. integer chain: overflow, division by zero, i64::MIN / -1 are DEFECTS exactly as before ─────────────────────

fn arith_lib() -> CoreBundle {
    let mut p = P::new();
    let (a, b) = (p.param(0), p.param(1));
    let t = p.call("int_add", vec![a.clone(), b.clone()]);
    let two = p.int(2);
    let u = p.call("int_mul", vec![t, two]);
    let v = p.call("int_div", vec![u, b.clone()]);
    let five = p.int(5);
    let w = p.call("int_mod", vec![v, five]);
    let r = p.call("int_sub", vec![w, a]);
    p.done(r)
}

fn arith_root() -> CoreBundle {
    let mut p = P::new();
    let a = argv_int(&mut p, 1);
    let b = argv_int(&mut p, 2);
    let f = p.call("arith", vec![a, b]);
    let s = p.call("int_to_str", vec![f]);
    let r = p.call("io_print", vec![s]);
    p.done(r)
}

#[test]
fn integer_chain_matches_generic_including_every_defect() {
    let lib = arith_lib();
    let max = i64::MAX.to_string();
    let min = i64::MIN.to_string();
    let hit_min_div = (-(1i64 << 62) + 1).to_string(); // a + b = -2^62, *2 = i64::MIN, / -1 overflows
    let cases: Vec<Vec<&str>> = vec![
        vec!["7", "2"], vec!["-7", "3"], vec!["0", "5"], vec!["100", "-9"],                  // ordinary (incl. Euclidean mod)
        vec![&max, "1"], vec![&min, "-1"], vec!["5", "0"], vec!["0", "0"], vec![&hit_min_div, "-1"], // defects
        vec!["4611686018427387904", "0"],                                                       // t*2 overflows
    ];
    let cs: Vec<&[&str]> = cases.iter().map(|c| c.as_slice()).collect();
    let r = differential(&arith_root(), &[("arith", &lib)], &["arith"], &cs);
    assert_eq!((r[0].code, r[0].stdout.trim()), (Some(0), "-3"));   // (7+2)*2=18; 18 div 2=9; 9 mod 5=4; 4-7
    for k in [4, 5, 6, 7, 8, 9] {
        assert_eq!(r[k].code, Some(4), "case {k}: a pure builtin's failure is a DEFECT (exit 4): {:?}", r[k]);
        assert!(r[k].stderr.contains("axis: DEFECT in int_"), "case {k}: {:?}", r[k].stderr);
    }
    assert!(r[4].stderr.contains("int_add: overflow"), "{:?}", r[4].stderr);
    assert!(r[6].stderr.contains("int_div: division by zero"), "{:?}", r[6].stderr);
    assert!(r[8].stderr.contains("int_div"), "{:?}", r[8].stderr);
}

// ── 2. loop state through a Ctor: wrong field type, an Unknown state, and a state that overflows ─────────────────

fn step_lib() -> CoreBundle {
    // st = Value(i, acc, n)  ->  Value(i+1, acc + i mod 7, n)
    let mut p = P::new();
    let st = p.param(0);
    let (k0, k1, k2, one, seven) = (p.int(0), p.int(1), p.int(2), p.int(1), p.int(7));
    let f0 = p.call("ctor_field", vec![st.clone(), k0]);
    let i = p.call("value_to_int", vec![f0]);
    let f1 = p.call("ctor_field", vec![st.clone(), k1]);
    let acc = p.call("value_to_int", vec![f1]);
    let f2 = p.call("ctor_field", vec![st, k2]);
    let n = p.call("value_to_int", vec![f2]);
    let i1 = p.call("int_add", vec![i.clone(), one]);
    let m = p.call("int_mod", vec![i, seven]);
    let acc1 = p.call("int_add", vec![acc, m]);
    let r = p.call("value_make", vec![i1, acc1, n]);
    p.done(r)
}

#[test]
fn ctor_state_step_matches_generic_for_bad_types_unknowns_and_overflow() {
    let lib = step_lib();
    // root: mode 0 -> good state from argv; mode 1 -> field 0 is Text; mode 2 -> the state is an Unknown (fail)
    let mut p = P::new();
    let mode = argv_int(&mut p, 1);
    let a = argv_int(&mut p, 2);
    let b = argv_int(&mut p, 3);
    let c = argv_int(&mut p, 4);
    let (m1, m2) = (p.int(1), p.int(2));
    let is1 = p.call("int_eq", vec![mode.clone(), m1]);
    let is2 = p.call("int_eq", vec![mode, m2]);
    let (a2, b2, c2) = (a.clone(), b.clone(), c.clone());
    let st = p.if_(
        is2,
        |p| { let t = p.text("boom"); p.call("fail", vec![t]) },
        |p| {
            let (a3, b3, c3) = (a2.clone(), b2.clone(), c2.clone());
            p.if_(
                is1,
                { let (b4, c4) = (b3.clone(), c3.clone()); move |p: &mut P| { let t = p.text("x"); p.call("value_make", vec![t, b4, c4]) } },
                move |p| p.call("value_make", vec![a3, b3, c3]),
            )
        },
    );
    let _ = (a, b, c);
    let r = p.call("step", vec![st]);
    let root = p.done(r);
    let max = i64::MAX.to_string();
    let cases: Vec<Vec<&str>> = vec![
        vec!["0", "3", "10", "99"],       // ordinary
        vec!["0", "-5", "0", "1"],        // negative index: Euclidean mod
        vec!["1", "3", "10", "99"],       // Text in field 0: DEFECT in value_to_int
        vec!["2", "3", "10", "99"],       // an Unknown state: sticky, exit 3
        vec!["0", "3", &max, "1"],        // acc + (i mod 7) overflows
        vec!["0", "9223372036854775807", "5", "1"], // i+1 overflows? i = MAX: add overflows
    ];
    let cs: Vec<&[&str]> = cases.iter().map(|c| c.as_slice()).collect();
    let r = differential(&root, &[("step", &lib)], &["step"], &cs);
    assert_eq!(r[0].code, Some(0));
    assert_eq!(r[2].code, Some(4), "{:?}", r[2]);
    assert!(r[2].stderr.contains("DEFECT in value_to_int"), "{:?}", r[2].stderr);
    assert_eq!(r[3].code, Some(3), "an Unknown state stays sticky: {:?}", r[3]);
    assert_eq!(r[4].code, Some(4), "{:?}", r[4]);
    assert_eq!(r[5].code, Some(4), "{:?}", r[5]);
}

// ── 3. arms: a cold effectful arm (fail) is a bail, not a rejection; pure arms compute natively ────────────────

fn guarded_div_lib() -> CoreBundle {
    // if b == 0 { fail("zero") } else { if a < b { b - a } else { a / b } }
    let mut p = P::new();
    let (a, b) = (p.param(0), p.param(1));
    let z = p.int(0);
    let c = p.call("int_eq", vec![b.clone(), z]);
    let (a1, b1) = (a.clone(), b.clone());
    let r = p.if_(
        c,
        |p| { let t = p.text("zero"); p.call("fail", vec![t]) },
        move |p| {
            let lt = p.call("int_lt", vec![a1.clone(), b1.clone()]);
            let (a2, b2, a3, b3) = (a1.clone(), b1.clone(), a1.clone(), b1.clone());
            p.if_(lt, move |p| p.call("int_sub", vec![b2, a2]), move |p| p.call("int_div", vec![a3, b3]))
        },
    );
    p.done(r)
}

#[test]
fn effectful_arm_is_a_bail_and_pure_arms_agree() {
    let lib = guarded_div_lib();
    let mut p = P::new();
    let a = argv_int(&mut p, 1);
    let b = argv_int(&mut p, 2);
    let f = p.call("gdiv", vec![a, b]);
    let s = p.call("int_to_str", vec![f]);
    let r = p.call("io_print", vec![s]);
    let root = p.done(r);
    let min = i64::MIN.to_string();
    let cases: Vec<Vec<&str>> = vec![vec!["9", "3"], vec!["3", "9"], vec!["9", "-4"], vec!["5", "0"], vec![&min, "1"]];
    let cs: Vec<&[&str]> = cases.iter().map(|c| c.as_slice()).collect();
    let r = differential(&root, &[("gdiv", &lib)], &["gdiv"], &cs);
    assert_eq!(r[0].stdout.trim(), "3");
    assert_eq!(r[1].stdout.trim(), "6");
    assert_eq!(r[2].stdout.trim(), "-2");                       // Euclidean: 9 = (-4)(-2) + 1
    assert_eq!(r[3].code, Some(3), "fail(...) is the program's own Unknown: {:?}", r[3]);
    assert!(r[3].stderr.contains("axis: fail: zero"), "{:?}", r[3].stderr);
    assert_eq!(r[4].code, Some(4), "1 - MIN overflows, a defect: {:?}", r[4]);
}

// ── 4. an inlined composite (the PyAx shape: a caller whose callee has an effectful cold arm) ───────────────────

#[test]
fn composite_callee_is_inlined_and_agrees() {
    let callee = guarded_div_lib();
    let mut p = P::new();
    let (a, b) = (p.param(0), p.param(1));
    let q = p.call("gdiv", vec![a.clone(), b.clone()]);
    let prod = p.call("int_mul", vec![q, b]);
    let r = p.call("int_sub", vec![a, prod]);
    let caller = p.done(r);
    let mut p = P::new();
    let a = argv_int(&mut p, 1);
    let b = argv_int(&mut p, 2);
    let f = p.call("gmod", vec![a, b]);
    let s = p.call("int_to_str", vec![f]);
    let r = p.call("io_print", vec![s]);
    let root = p.done(r);
    let cases: Vec<Vec<&str>> = vec![vec!["17", "5"], vec!["-17", "5"], vec!["3", "9"], vec!["4", "0"]];
    let cs: Vec<&[&str]> = cases.iter().map(|c| c.as_slice()).collect();
    let r = differential(&root, &[("gdiv", &callee), ("gmod", &caller)], &["gmod", "gdiv"], &cs);
    assert_eq!(r[0].stdout.trim(), "2");        // 17 - (17 div 5)*5
    assert_eq!(r[3].code, Some(3));
    // gmod's fast function contains gdiv's operations (it was inlined, not called)
    let f = build(&root, &[("gdiv", &callee), ("gmod", &caller)], true);
    let gmod_fast = f.glue.split("fn gmod_fast").nth(1).unwrap().split("pub extern").next().unwrap();
    assert!(gmod_fast.contains("checked_div_euclid"), "gdiv was not inlined into gmod_fast:\n{gmod_fast}");
}

// ── 5. Text: ASCII index, out of range, negative, non-ASCII, a wrong type ─────────────────────────────────────

#[test]
fn text_index_fast_path_matches_generic() {
    // t(s, i) = if text_eq(str_char(s, i), "a") { str_len(s) } else { 0 - 1 }
    let mut p = P::new();
    let (s, i) = (p.param(0), p.param(1));
    let ch = p.call("str_char", vec![s.clone(), i]);
    let a = p.text("a");
    let c = p.call("text_eq", vec![ch, a]);
    let s2 = s.clone();
    let r = p.if_(c, move |p| p.call("str_len", vec![s2]), |p| { let (z, o) = (p.int(0), p.int(1)); p.call("int_sub", vec![z, o]) });
    let lib = p.done(r);
    let mut p = P::new();
    let mode = argv_int(&mut p, 1);
    let i = argv_int(&mut p, 2);
    let (s_ascii, s_uni) = (p.text(&"abcdefgh".repeat(10)), p.text(&"é日😀a".repeat(20)));
    let (m1, m0) = (p.int(1), p.int(0));
    let is1 = p.call("int_eq", vec![mode.clone(), m1]);
    let _ = m0;
    let s = p.if_(is1, |_| s_uni.clone(), |_| s_ascii.clone());
    let f = p.call("t", vec![s, i]);
    let st = p.call("int_to_str", vec![f]);
    let r = p.call("io_print", vec![st]);
    let root = p.done(r);
    let cases: Vec<Vec<&str>> = vec![
        vec!["0", "0"], vec!["0", "1"], vec!["0", "79"], vec!["0", "80"], vec!["0", "-1"], // ASCII: hit, miss, last, past end, negative
        vec!["1", "3"], vec!["1", "0"], vec!["1", "79"], vec!["1", "80"],                    // non-ASCII text: a at 3, é at 0, ...
    ];
    let cs: Vec<&[&str]> = cases.iter().map(|c| c.as_slice()).collect();
    let r = differential(&root, &[("t", &lib)], &["t"], &cs);
    assert_eq!(r[0].stdout.trim(), "80");
    assert_eq!(r[1].stdout.trim(), "-1");
    assert_eq!(r[3].code, Some(4), "str_char past the end is a defect: {:?}", r[3]);
    assert_eq!(r[4].code, Some(4), "{:?}", r[4]);
    assert_eq!(r[5].stdout.trim(), "80");        // chars of "é日😀a" repeated: len 80
    assert_eq!(r[8].code, Some(4));
}

// ── 6. loop_while with fast cond/step (step C): same result, same defects, same stuck-on-Unknown ──────────────

fn more_lib() -> CoreBundle {
    let mut p = P::new();
    let st = p.param(0);
    let (k0, k2) = (p.int(0), p.int(2));
    let f0 = p.call("ctor_field", vec![st.clone(), k0]);
    let i = p.call("value_to_int", vec![f0]);
    let f2 = p.call("ctor_field", vec![st, k2]);
    let n = p.call("value_to_int", vec![f2]);
    let r = p.call("int_lt", vec![i, n]);
    p.done(r)
}

#[test]
fn loop_while_with_fast_callbacks_matches_generic() {
    let (more, step) = (more_lib(), step_lib());
    let mut p = P::new();
    let n = argv_int(&mut p, 1);
    let start = argv_int(&mut p, 2);
    let zero = p.int(0);
    let init = p.call("value_make", vec![zero, start, n.clone()]);
    let (cond, stepf) = (p.fnref("more"), p.fnref("step"));
    let one = p.int(1);
    let cap = p.call("int_add", vec![n, one]);
    let r = p.call("loop_while", vec![init, cond, stepf, cap]);
    let k1 = p.int(1);
    let acc = p.call("ctor_field", vec![r, k1]);
    let a = p.call("value_to_int", vec![acc]);
    let s = p.call("int_to_str", vec![a]);
    let out = p.call("io_print", vec![s]);
    let root = p.done(out);
    let max = i64::MAX.to_string();
    let near = (i64::MAX - 20).to_string();
    let cases: Vec<Vec<&str>> = vec![
        vec!["0", "0"], vec!["1", "0"], vec!["100", "0"], vec!["100000", "5"],
        vec!["10", &near],           // acc overflows part-way through the loop: DEFECT after some fast steps
        vec!["10", &max],            // overflows on the first step that adds a non-zero
        vec!["-3", "0"],             // cap <= 0 iterations
    ];
    let cs: Vec<&[&str]> = cases.iter().map(|c| c.as_slice()).collect();
    let r = differential(&root, &[("more", &more), ("step", &step)], &["more", "step"], &cs);
    assert!(r[0].code == Some(0));
    assert_eq!(r[2].stdout.trim(), "295");   // sum of i mod 7 for i in 0..100 (14 full cycles of 21, then 0 and 1)
    // a pure builtin's DEFECT inside a callback is seen by the (effect-gated) loop_while guard: an Unknown, exit 3, as before
    for k in [4, 5] {
        assert_eq!(r[k].code, Some(3), "{:?}", r[k]);
        assert!(r[k].stderr.contains("DEFECT in int_add: int_add: overflow"), "{:?}", r[k].stderr);
    }
    let f = build(&root, &[("more", &more), ("step", &step)], true);
    assert!(f.glue.contains("loop_while_f"), "the call site did not use loop_while_f");
    let g = build(&root, &[("more", &more), ("step", &step)], false);
    assert!(!g.glue.contains("loop_while_f"), "--no-fast must keep the plain loop_while");
}

// ── 7. bit ops, shifts and bool ops (including the shift's own failure and its past-63 saturation) ─────────────

#[test]
fn bit_and_bool_ops_match_generic() {
    // x = a ^ b; y = x & 255; z = y >> b; ok = (z < 100) && !(z == 3); if ok { z } else { 0 - z }
    let mut p = P::new();
    let (a, b) = (p.param(0), p.param(1));
    let x = p.call("int_bit_xor", vec![a, b.clone()]);
    let m = p.int(255);
    let y = p.call("int_bit_and", vec![x, m]);
    let z = p.call("int_shr", vec![y, b]);
    let (h, three) = (p.int(100), p.int(3));
    let lt = p.call("int_lt", vec![z.clone(), h]);
    let eq = p.call("int_eq", vec![z.clone(), three]);
    let ne = p.call("bool_not", vec![eq]);
    let ok = p.call("bool_and", vec![lt, ne]);
    let z2 = z.clone();
    let r = p.if_(ok, move |_| z, move |p| { let zero = p.int(0); p.call("int_sub", vec![zero, z2]) });
    let lib = p.done(r);
    let mut p = P::new();
    let a = argv_int(&mut p, 1);
    let b = argv_int(&mut p, 2);
    let f = p.call("bits", vec![a, b]);
    let s = p.call("int_to_str", vec![f]);
    let r = p.call("io_print", vec![s]);
    let root = p.done(r);
    let cases: Vec<Vec<&str>> = vec![
        vec!["1000", "3"], vec!["12", "2"], vec!["-1", "1"], vec!["77", "70"], vec!["77", "-1"], vec!["255", "0"], vec!["5", "63"],
    ];
    let cs: Vec<&[&str]> = cases.iter().map(|c| c.as_slice()).collect();
    let r = differential(&root, &[("bits", &lib)], &["bits"], &cs);
    assert_eq!(r[0].stdout.trim(), "29");           // (1000^3)&255 = 235; 235>>3 = 29; 29 < 100 and != 3
    assert_eq!(r[4].code, Some(4), "a negative shift count is a defect: {:?}", r[4]);
}

// ── 8. value_make must hand back a sticky argument, not wrap it ───────────────────────────────────────────────

#[test]
fn value_make_of_a_sticky_argument_stays_sticky() {
    let mut p = P::new();
    let x = p.param(0);
    let one = p.int(1);
    let r = p.call("value_make", vec![x, one]);
    let lib = p.done(r);
    let mut p = P::new();
    let mode = argv_int(&mut p, 1);
    let one = p.int(1);
    let is1 = p.call("int_eq", vec![mode, one]);
    let arg = p.if_(is1, |p| { let t = p.text("boom"); p.call("fail", vec![t]) }, |p| p.int(41));
    let f = p.call("wrap", vec![arg]);
    let _ = f.clone();
    let root = p.done(f);
    let cases: Vec<Vec<&str>> = vec![vec!["0"], vec!["1"]];
    let cs: Vec<&[&str]> = cases.iter().map(|c| c.as_slice()).collect();
    let r = differential(&root, &[("wrap", &lib)], &["wrap"], &cs);
    assert_eq!(r[0].code, Some(0));
    assert_eq!(r[1].code, Some(3), "an Unknown argument must come back as the Unknown: {:?}", r[1]);
}

// ── 9. negative constants in receiver position (found by the lab's PyAx fixtures: `-1i64.checked_add` did not parse) ──

#[test]
fn negative_constants_build_and_agree() {
    // f(a) = ((-1 + a) * -3) - (-7 mod a)
    let mut p = P::new();
    let a = p.param(0);
    let (m1, m3, m7) = (p.int(-1), p.int(-3), p.int(-7));
    let t = p.call("int_add", vec![m1, a.clone()]);
    let u = p.call("int_mul", vec![t, m3]);
    let v = p.call("int_mod", vec![m7, a]);
    let r = p.call("int_sub", vec![u, v]);
    let lib = p.done(r);
    let mut p = P::new();
    let a = argv_int(&mut p, 1);
    let f = p.call("neg", vec![a]);
    let s = p.call("int_to_str", vec![f]);
    let r = p.call("io_print", vec![s]);
    let root = p.done(r);
    let cases: Vec<Vec<&str>> = vec![vec!["5"], vec!["-5"], vec!["1"], vec!["0"]];
    let cs: Vec<&[&str]> = cases.iter().map(|c| c.as_slice()).collect();
    let r = differential(&root, &[("neg", &lib)], &["neg"], &cs);
    assert_eq!(r[0].stdout.trim(), "-15");   // (4 * -3) - (-7 mod 5 = 3)
    assert_eq!(r[3].code, Some(4), "-7 mod 0 is a defect: {:?}", r[3]);
}


// ── 10. Float: IEEE all the way -- inf, NaN, signed zero, overflow to inf, all identical to the generic ops ──────

fn argv_float(p: &mut P, i: i64) -> NodeRef {
    let (idx, dflt) = (p.int(i), p.text("0"));
    let t = p.call("argv_or", vec![idx, dflt]);
    p.call("str_to_float", vec![t])
}

#[test]
fn float_ops_match_generic_including_nan_inf_and_signed_zero() {
    // h = sqrt(a*a + b*b); q = h / b; if q < 10.0 { max(q, -h) } else { abs(floor(q)) }
    let mut p = P::new();
    let (a, b) = (p.param(0), p.param(1));
    let aa = p.call("float_mul", vec![a.clone(), a]);
    let bb = p.call("float_mul", vec![b.clone(), b.clone()]);
    let sum = p.call("float_add", vec![aa, bb]);
    let h = p.call("float_sqrt", vec![sum]);
    let q = p.call("float_div", vec![h.clone(), b]);
    let ten = p.float(10.0);
    let c = p.call("float_lt", vec![q.clone(), ten]);
    let (q2, h2, q3) = (q.clone(), h.clone(), q.clone());
    let r = p.if_(
        c,
        move |p| { let nh = p.call("float_neg", vec![h2]); p.call("float_max", vec![q2, nh]) },
        move |p| { let f = p.call("float_floor", vec![q3]); p.call("float_abs", vec![f]) },
    );
    let lib = p.done(r);
    // g = round(pow(a, b)); ceil(g) - min(a, b), and a NaN / equality probe boxed in a value_make
    let mut p = P::new();
    let (a, b) = (p.param(0), p.param(1));
    let pw = p.call("float_pow", vec![a.clone(), b.clone()]);
    let rd = p.call("float_round", vec![pw]);
    let ce = p.call("float_ceil", vec![rd.clone()]);
    let mn = p.call("float_min", vec![a.clone(), b.clone()]);
    let df = p.call("float_sub", vec![ce, mn]);
    let nan = p.call("float_is_nan", vec![df.clone()]);
    let eq = p.call("float_eq", vec![a, b]);
    let r = p.call("value_make", vec![df, nan, eq, rd]);
    let lib2 = p.done(r);
    for (name, l) in [("hyp", &lib), ("pw", &lib2)] {
        let mut p = P::new();
        let a = argv_float(&mut p, 1);
        let b = argv_float(&mut p, 2);
        let r = p.call(name, vec![a, b]);
        let root = p.done(r);
        let cases: Vec<Vec<&str>> = vec![
            vec!["3", "4"], vec!["3", "0"], vec!["0", "0"], vec!["-0", "0"], vec!["1e308", "1e308"], vec!["-2.5", "1e-5"],
            vec!["nan", "2"], vec!["inf", "2"], vec!["2", "inf"], vec!["2", "-3"], vec!["-8", "0.5"], vec!["0.1", "0.2"],
        ];
        let cs: Vec<&[&str]> = cases.iter().map(|c| c.as_slice()).collect();
        let r = differential(&root, &[(name, l)], &[name], &cs);
        assert!(r.iter().all(|x| x.code == Some(0)), "{name}: {:?}", r.iter().map(|x| (x.code, &x.stderr)).collect::<Vec<_>>());
    }
}

#[test]
fn float_wrong_type_is_the_same_defect() {
    // hyp(a, b) fed an Int: DEFECT in float_mul exactly as before
    let mut p = P::new();
    let (a, b) = (p.param(0), p.param(1));
    let r = p.call("float_add", vec![a, b]);
    let lib = p.done(r);
    let mut p = P::new();
    let i = argv_int(&mut p, 1);
    let f = p.float(1.5);
    let r = p.call("fadd", vec![f, i]);
    let root = p.done(r);
    let r = differential(&root, &[("fadd", &lib)], &["fadd"], &[&["2"]]);
    assert_eq!(r[0].code, Some(4), "{:?}", r[0]);
    assert!(r[0].stderr.contains("DEFECT in float_add"), "{:?}", r[0].stderr);
}

// ── 11. Dec: checked ops fall back on overflow; min/max keep the scale rust_decimal's Ord picks ───────────────────

fn argv_dec(p: &mut P, i: i64) -> NodeRef {
    let (idx, dflt) = (p.int(i), p.text("0"));
    let t = p.call("argv_or", vec![idx, dflt]);
    p.call("str_to_dec", vec![t])
}

#[test]
fn dec_ops_match_generic_including_overflow_zero_divisor_and_scale() {
    // s = a + b; m = s * b; d = m / a; r = round(d, 2); max(abs(r), -a) - 0.5
    let mut p = P::new();
    let (a, b) = (p.param(0), p.param(1));
    let s = p.call("dec_add", vec![a.clone(), b.clone()]);
    let m = p.call("dec_mul", vec![s, b]);
    let d = p.call("dec_div", vec![m, a.clone()]);
    let two = p.int(2);
    let r = p.call("dec_round", vec![d, two]);
    let ab = p.call("dec_abs", vec![r]);
    let na = p.call("dec_neg", vec![a]);
    let mx = p.call("dec_max", vec![ab, na]);
    let half = p.dec("0.5");
    let out = p.call("dec_sub", vec![mx, half]);
    let lib = p.done(out);
    // value_make(min(a,b), max(a,b), a<b, a<=b, a>b, a>=b, a==b)
    let mut p = P::new();
    let (a, b) = (p.param(0), p.param(1));
    let mn = p.call("dec_min", vec![a.clone(), b.clone()]);
    let mx = p.call("dec_max", vec![a.clone(), b.clone()]);
    let c1 = p.call("dec_lt", vec![a.clone(), b.clone()]);
    let c2 = p.call("dec_lte", vec![a.clone(), b.clone()]);
    let c3 = p.call("dec_gt", vec![a.clone(), b.clone()]);
    let c4 = p.call("dec_gte", vec![a.clone(), b.clone()]);
    let c5 = p.call("dec_eq", vec![a, b]);
    let r = p.call("value_make", vec![mn, mx, c1, c2, c3, c4, c5]);
    let lib2 = p.done(r);
    let max = "79228162514264337593543950335";
    for (name, l, cases) in [
        ("dchain", &lib, vec![vec!["1.5", "2.25"], vec!["-7.125", "3"], vec!["0", "1"], vec![max, "1"], vec!["3", max], vec!["0.0000000000000000001", "7"], vec!["2.675", "0.005"]]),
        ("dcmp", &lib2, vec![vec!["1.0", "1.00"], vec!["1.00", "1.0"], vec!["2", "-3.5"], vec!["-0.0", "0"], vec!["0.10", "0.1"]]),
    ] {
        let mut p = P::new();
        let a = argv_dec(&mut p, 1);
        let b = argv_dec(&mut p, 2);
        let r = p.call(name, vec![a, b]);
        let root = p.done(r);
        let cs: Vec<&[&str]> = cases.iter().map(|c| c.as_slice()).collect();
        let r = differential(&root, &[(name, l)], &[name], &cs);
        if name == "dchain" {
            assert_eq!(r[2].code, Some(4), "dec_div by zero is a defect: {:?}", r[2]);
            assert!(r[2].stderr.contains("dec_div: division by zero"), "{:?}", r[2].stderr);
            assert_eq!(r[3].code, Some(4), "{:?}", r[3]);
            assert!(r[3].stderr.contains("dec_add: overflow"), "{:?}", r[3].stderr);
            assert_eq!(r[0].code, Some(0), "{:?}", r[0]);
        } else {
            assert!(r.iter().all(|x| x.code == Some(0)), "{:?}", r);
        }
    }
}


// ── 12. Int -> Float / Dec conversions and the identity narrowings PyAx uses ───────────────────────────────────

#[test]
fn int_to_float_and_dec_and_ids_match_generic() {
    // value_make(float_id(int_to_float(a)) * 0.5, dec_id(int_to_dec(a)) + 1.25)
    let mut p = P::new();
    let a = p.param(0);
    let f = p.call("int_to_float", vec![a.clone()]);
    let fi = p.call("float_id", vec![f]);
    let half = p.float(0.5);
    let fm = p.call("float_mul", vec![fi, half]);
    let d = p.call("int_to_dec", vec![a]);
    let di = p.call("dec_id", vec![d]);
    let q = p.dec("1.25");
    let ds = p.call("dec_add", vec![di, q]);
    let r = p.call("value_make", vec![fm, ds]);
    let lib = p.done(r);
    let mut p = P::new();
    let a = argv_int(&mut p, 1);
    let r = p.call("conv", vec![a]);
    let root = p.done(r);
    let max = i64::MAX.to_string();
    let min = i64::MIN.to_string();
    let cases: Vec<Vec<&str>> = vec![vec!["0"], vec!["7"], vec!["-9"], vec![&max], vec![&min], vec!["9007199254740993"]];
    let cs: Vec<&[&str]> = cases.iter().map(|c| c.as_slice()).collect();
    let r = differential(&root, &[("conv", &lib)], &["conv"], &cs);
    assert!(r.iter().all(|x| x.code == Some(0)), "{:?}", r);
}


// ── 13. str_to_float / str_to_dec: a literal is parsed once; a bad literal is the same defect ──────────────────────

#[test]
fn str_to_float_and_dec_literals_and_args_match_generic() {
    // value_make(float(lit) + float(arg), dec(lit2) + dec(arg))
    let mut p = P::new();
    let (a, b) = (p.param(0), p.param(1));
    let l1 = p.text("0.999");
    let f1 = p.call("str_to_float", vec![l1]);
    let f2 = p.call("str_to_float", vec![a]);
    let fs = p.call("float_add", vec![f1, f2]);
    let l2 = p.text("0.25");
    let d1 = p.call("str_to_dec", vec![l2]);
    let d2 = p.call("str_to_dec", vec![b]);
    let ds = p.call("dec_add", vec![d1, d2]);
    let r = p.call("value_make", vec![fs, ds]);
    let lib = p.done(r);
    let mut p = P::new();
    let (ia, ib) = (p.int(1), p.int(2));
    let (d0, d1) = (p.text("0"), p.text("0"));
    let a = p.call("argv_or", vec![ia, d0]);
    let b = p.call("argv_or", vec![ib, d1]);
    let r = p.call("lits", vec![a, b]);
    let root = p.done(r);
    let cases: Vec<Vec<&str>> = vec![vec!["1.5", "2.5"], vec!["inf", "-0.0"], vec!["nan", "1e3"], vec!["abc", "1"], vec!["1", "x.5"]];
    let cs: Vec<&[&str]> = cases.iter().map(|c| c.as_slice()).collect();
    let r = differential(&root, &[("lits", &lib)], &["lits"], &cs);
    assert_eq!(r[0].code, Some(0), "{:?}", r[0]);
    assert_eq!(r[3].code, Some(4), "invalid float text is a defect: {:?}", r[3]);
    assert!(r[3].stderr.contains("DEFECT in str_to_float"), "{:?}", r[3].stderr);
    assert_eq!(r[4].code, Some(4), "{:?}", r[4]);
}


#[test]
fn str_trim_feeding_str_to_dec_matches_generic() {
    // Decimal(trim(lit)) + Decimal(trim(arg))
    let mut p = P::new();
    let a = p.param(0);
    let l = p.text("  1.5 ");
    let tl = p.call("str_trim", vec![l]);
    let d1 = p.call("str_to_dec", vec![tl]);
    let ta = p.call("str_trim", vec![a]);
    let d2 = p.call("str_to_dec", vec![ta]);
    let r = p.call("dec_add", vec![d1, d2]);
    let lib = p.done(r);
    let mut p = P::new();
    let (i, d) = (p.int(1), p.text("0"));
    let a = p.call("argv_or", vec![i, d]);
    let r = p.call("trimd", vec![a]);
    let root = p.done(r);
    let cases: Vec<Vec<&str>> = vec![vec!["2.25"], vec![" \t 7 \n"], vec!["x"], vec![" "]];
    let cs: Vec<&[&str]> = cases.iter().map(|c| c.as_slice()).collect();
    let r = differential(&root, &[("trimd", &lib)], &["trimd"], &cs);
    assert_eq!(r[0].code, Some(0), "{:?}", r[0]);
    assert_eq!(r[2].code, Some(4), "{:?}", r[2]);
}

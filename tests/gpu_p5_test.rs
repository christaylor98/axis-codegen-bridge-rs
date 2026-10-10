//! GPU_P5_V1: one search size through the GPU engine equals the same size done sequentially with the same fbank
//! calls (am_p5a_step's logic), including dedupe by lowest ordinal, all-E skips, the last-size row mask and a 3-arg op.
use axis_codegen_bridge::runtime::fbank::*;
use axis_codegen_bridge::runtime::gpu_p5::*;
use axis_codegen_bridge::runtime::value::{intern_str, ListBuf, Value};

fn i(v: Value) -> i64 { match v { Value::Int(x) => x, o => panic!("{o:?}") } }
fn t(v: &Value) -> String { match v { Value::Str(s) => s.to_string(), o => panic!("{o:?}") } }
fn tl(xs: &[String]) -> Value { Value::List(ListBuf::from(xs.iter().map(|x| Value::Str(intern_str(x))).collect::<Vec<_>>())) }
fn list(v: &Value) -> Vec<String> { match v { Value::List(xs) => xs.iter().map(t).collect(), o => panic!("{o:?}") } }
fn pack(xs: &[String]) -> String {
    let lens: Vec<String> = xs.iter().map(|x| x.chars().count().to_string()).collect();
    format!("{}|{}|{}", xs.len(), lens.join(","), xs.concat())
}
fn intern(v: i64, xs: &[String]) -> Vec<String> { list(&fbank_intern_list(v, tl(xs))) }
fn get(v: i64, ids: &[String]) -> Vec<String> { ids.iter().map(|x| t(&fbank_get(v, x.parse().unwrap()))).collect() }

/// The test's ops (the engine never sees them): 1 = concat (E if either is E, they are equal, or longer than 6),
/// 2 = pick: if a == "t:x" then b else c.
fn op(code: i64, a: &str, b: &str, c: &str) -> String {
    match code {
        1 => if a == "E" || b == "E" || a == b { "E".into() } else { let s = format!("t:{}{}", &a[2..], &b[2..]); if s.len() > 8 { "E".into() } else { s } },
        2 => if a == "E" { "E".into() } else if a == "t:x" { b.into() } else { c.into() },
        _ => panic!(),
    }
}

struct World { v: i64, b: i64, bl: i64, n: usize }

fn world() -> World {
    let n = 3;
    let v = i(fbank_new(100_000, 10_000_000));
    let b = i(fbank_new(100_000, 10_000_000));
    let bl = i(fbank_new(1000, 100_000));
    let cols: Vec<Vec<&str>> = vec![vec!["t:x", "t:y", "t:"], vec!["t:a", "t:a", "t:a"], vec!["t:b", "t:x", "t:b"], vec!["t:x", "t:x", "t:x"], vec!["t:a", "t:a", "t:a"]];
    for (o, c) in cols.iter().enumerate() {
        let ids = intern(v, &c.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        let id = i(fbank_put(b, o as i64, intern_str(&format!("Msg\t{}", pack(&ids)))));
        if id >= 0 { fbank_put(bl, 0, intern_str(&id.to_string())); }
    }
    World { v, b, bl, n }
}

fn job(code: i64, k: i64, count: i64, c2: i64, c3: i64, base: i64, segs: &str) -> String {
    let f: Vec<String> = vec![code.to_string(), k.to_string(), "Msg".into(), "0".into(), count.to_string(), c2.to_string(), c3.to_string(), base.to_string(),
                              segs.into(), if k >= 2 { segs.into() } else { "".into() }, if k >= 3 { segs.into() } else { "".into() }, "op".into()];
    pack(&f)
}

/// The sequential reference: am_p5a_step per combination.
fn reference(w: &World, code: i64, k: i64, m: usize, base: i64, last: bool, mask: &[Option<String>], l: i64, r: i64) {
    let pool: Vec<String> = (0..i(fbank_len(w.bl))).map(|x| t(&fbank_get(w.bl, x))).collect();
    let c = pool.len();
    let eid = intern(w.v, &["E".into()])[0].clone();
    for j in 0..m {
        let (i1, i2, i3) = (j / (c * c), (j / c) % c, j % c);
        let (i1, i2, i3) = if k == 2 { (j / c, j % c, 0) } else { (i1, i2, i3) };
        let vec_of = |id: &str| -> Vec<String> { get(w.v, &{ let e = t(&fbank_get(w.b, id.parse().unwrap())); let p = e.split_once('\t').unwrap().1.to_string(); list(&axis_codegen_bridge::runtime::list::text_list_unpack(intern_str(&p))) }) };
        let (a, b, cc) = (vec_of(&pool[i1]), vec_of(&pool[i2]), if k == 3 { vec_of(&pool[i3]) } else { vec!["".into(); w.n] });
        let outs: Vec<String> = (0..w.n).map(|col| op(code, &a[col], &b[col], &cc[col])).collect();
        let ids = intern(w.v, &outs);
        if ids.iter().all(|x| *x == eid) { continue; }
        if last && !ids.iter().zip(mask).all(|(x, m)| m.as_ref().map_or(true, |m| m == x)) { continue; }
        let p = pack(&ids);
        let ord = base + j as i64;
        let id = if last {
            if i(fbank_find(w.b, intern_str(&format!("Msg\t{p}")))) >= 0 { -1 } else { i(fbank_put(l, ord, intern_str(&p))) }
        } else { i(fbank_put(w.b, ord, intern_str(&format!("Msg\t{p}")))) };
        if id < 0 { continue; }
        let rec = format!("0\t{}\t{}\t{}\t{}", id, pool[i1], pool[i2], if k == 3 { pool[i3].clone() } else { String::new() });
        fbank_put(r, 0, intern_str(&rec));
    }
}

fn run_gpu(w: &World, e: i64, jobs: &[String], n_try: i64, last: bool, l: i64, r: i64, mask: &str) {
    let eid = intern(w.v, &["E".into()])[0].clone();
    let alle = pack(&vec![eid; w.n]);
    let g = i(gpu_p5_size(e, tl(jobs), n_try, last, w.b, w.v, l, r, intern_str(mask), intern_str(&alle)));
    for k in 0..g {
        let code = i(gpu_p5_group_code(e, k));
        let len = i(gpu_p5_group_len(e, k));
        // answered in two ranges, as AI3's tasks do
        for (from, to) in [(0, len / 2), (len / 2, len)] {
            let a = list(&gpu_p5_group_args(e, k, 0, from, to));
            let b = list(&gpu_p5_group_args(e, k, 1, from, to));
            let c = list(&gpu_p5_group_args(e, k, 2, from, to));
            let outs: Vec<String> = (0..a.len()).map(|x| op(code, &a[x], b.get(x).map_or("", |s| s), c.get(x).map_or("", |s| s))).collect();
            assert_eq!(i(gpu_p5_answer(e, k, from, tl(&outs))), 0);
        }
    }
    assert_eq!(i(gpu_p5_step(e)), 0);
}

fn dump(b: i64) -> Vec<String> { (0..i(fbank_len(b))).map(|x| format!("{}#{}", t(&fbank_get(b, x)), i(fbank_ordinal(b, x)))).collect() }
/// A bank of "<role>\t<pack>" or "<pack>" entries with the V ids resolved to values (V numbering is not compared).
fn dumpv(b: i64, v: i64) -> Vec<String> {
    (0..i(fbank_len(b))).map(|x| {
        let e = t(&fbank_get(b, x));
        let (role, p) = e.split_once('\t').map(|(r, p)| (r.to_string(), p.to_string())).unwrap_or((String::new(), e.clone()));
        let ids = list(&axis_codegen_bridge::runtime::list::text_list_unpack(intern_str(&p)));
        format!("{}|{}#{}", role, get(v, &ids).join(","), i(fbank_ordinal(b, x)))
    }).collect()
}

#[test]
fn gpu_size_equals_sequential() {
    let probe = i(gpu_p5_open(3));
    if probe == 0 { eprintln!("no GPU: skipped"); return; }
    gpu_p5_close(probe);
    // size 2 (concat, k=2, 25 combinations), then size 3 (pick, k=3, 125, last size with a row mask)
    let (wa, wb) = (world(), world());
    let segs = format!("{}:{}", wa.bl, 4);
    let segs_b = format!("{}:{}", wb.bl, 4);
    let (ra, rb) = (i(fbank_new(1000, 1_000_000)), i(fbank_new(1000, 1_000_000)));
    reference(&wa, 1, 2, 16, 10, false, &[], 0, ra);
    let e = i(gpu_p5_open(3));
    run_gpu(&wb, e, &[job(1, 2, 16, 4, 1, 10, &segs_b)], 16, false, 0, rb, &pack(&["*".into(), "*".into(), "*".into()]));
    assert_eq!(dumpv(wa.b, wa.v), dumpv(wb.b, wb.v), "B after size 2");
    assert_eq!(dump(ra), dump(rb), "records after size 2");
    // last size: mask = row 0 must be "t:a", others held
    let ma = intern(wa.v, &["t:a".into()])[0].clone();
    let mb = intern(wb.v, &["t:a".into()])[0].clone();
    let (la, lb) = (i(fbank_new(1000, 1_000_000)), i(fbank_new(1000, 1_000_000)));
    let (ra2, rb2) = (i(fbank_new(1000, 1_000_000)), i(fbank_new(1000, 1_000_000)));
    reference(&wa, 2, 3, 50, 40, true, &[Some(ma), None, None], la, ra2);
    let mask_b = pack(&[mb, "*".into(), "*".into()]);
    run_gpu(&wb, e, &[job(2, 3, 64, 4, 4, 40, &segs_b)], 50, true, lb, rb2, &mask_b);
    assert_eq!(dumpv(la, wa.v), dumpv(lb, wb.v), "L at the last size");
    assert_eq!(dump(ra2), dump(rb2), "records at the last size");
    assert!(!dump(rb2).is_empty());
    eprintln!("{}", t(&gpu_p5_stats(e)));
    gpu_p5_close(e);
    let _ = segs;
}

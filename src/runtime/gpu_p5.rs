//! GPU_P5_V2 — the axMachina P5 search engine on the GPU (axMachina P6, 2026-10-10).
//!
//! One size of P5's bottom-up search (the jobs `am_p5` builds, the banks `am_p5a` keeps) run on the GPU. The engine
//! never applies an op: op semantics stay in AI3 (`am_p5f`). The GPU does the grunt work for every combination; the
//! host and AI3 only touch what survives:
//!   1. gather + discover: each combination's argument value ids per column (from the arg pools, B ids, and the B
//!      vectors), and every (op code, a, b, c) key the memo lacks, once (kernels in `gpu-kernels/`, Rust on nvptx)
//!   2. the misses go back to AI3 grouped by op code, in ranges (`gpu_p5_group_*`), so AI3 can answer them on many
//!      tasks with `am_p5f`; the answers are interned into V (`gpu_p5_answer`, callable from any thread)
//!   3. produce + claim: every combination's output vector on the GPU; skipped there when all E, (last size) not
//!      giving the rows, or already in B for its role; the rest put into a per-size table keeping the lowest ordinal
//!      per (role, vector fingerprint) (the last size: per vector, as the reference's per-size set)
//!   4. keep: only the lowest-ordinal holder of each fingerprint comes back to the host, which banks it exactly as
//!      `am_p5a_step` does ("<role>\t<vector>" into B, or the vector into L at the last size) and writes the record
//!      "<job>\t<id>\t<id1>\t<id2>\t<id3>" into R, in ordinal order. So `am_p5a`'s merge reads R unchanged.
//! Fingerprints are 128-bit; the host's fbank_put still compares content, so a collision could only drop a vector,
//! never bank a wrong one. The memo lives as long as the engine (one per search).
//!
//! Calls (Ints are handles, counts or codes; the AI3 side is `am_p5a_gpu`):
//!   gpu_p5_open(ncols) -> engine (> 0), or 0 when no GPU / no kernels (the caller then runs on the CPU)
//!   gpu_p5_size(e, jobs, n, last, B, V, L, R, mask, alle) -> groups of misses pending (>= 0)
//!   gpu_p5_group_code(e, g) -> op code;  gpu_p5_group_len(e, g) -> misses in group g
//!   gpu_p5_group_args(e, g, pos, from, to) -> TextList: argument pos (0..2) of misses from..to (empty when unused)
//!   gpu_p5_answer(e, g, from, outs) -> 0 | -1 (V full: those values become "-2", as the CPU worker writes them)
//!   gpu_p5_step(e) -> 0 = the size is banked and recorded, -1 = a bank is full
//!   gpu_p5_stats(e) -> Text;  gpu_p5_close(e) -> Unit

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use cudarc::driver::{CudaContext, CudaFunction, CudaSlice, CudaStream, DevicePtr, DeviceRepr, LaunchConfig, PushKernelArg};

use super::fbank::{fbank_get, fbank_intern_list, fbank_len, fbank_put};
use super::list::text_list_unpack;
use super::value::{intern_str, ListBuf, Value};

static PTX: &str = include_str!(concat!(env!("OUT_DIR"), "/axis_gpu_kernels.ptx"));

const NONE: u32 = 0xFFFF_FFFE;
/// An answer V could not hold (full): written as "-2" in the vector, as the CPU worker's fbank_intern_list does.
const REFUSED: u32 = 0xFFFF_FFFD;
/// Elements (combinations x columns) per kernel launch.
const CHUNK: u64 = 1 << 24;
/// The tag of the last size's role-less set.
const LTAG: u64 = 0xFFFF_FFFF;

#[repr(C)]
#[derive(Clone, Copy)]
struct JobArg { code: u32, k: u32, c2: u32, c3: u32, p1: u64, p2: u64, p3: u64 }
unsafe impl DeviceRepr for JobArg {}

#[repr(C)]
#[derive(Clone, Copy)]
struct TableArg { st: u64, k0: u64, k1: u64, val: u64, mask: u64 }
unsafe impl DeviceRepr for TableArg {}

#[repr(C)]
#[derive(Clone, Copy)]
struct FiltArg { n: u64, eid: u32, last: u32, role: u64, ltag: u64, mask: u64, ord0: u64 }
unsafe impl DeviceRepr for FiltArg {}

/// A (tag, fingerprint) -> lowest ordinal table on the device.
struct VT { st: CudaSlice<u32>, k0: CudaSlice<u64>, k1: CudaSlice<u64>, ord: CudaSlice<u64>, cap: usize }

impl VT {
    fn new(s: &Arc<CudaStream>, cap: usize) -> VT {
        VT { st: ck(s.alloc_zeros(cap), "alloc vt"), k0: ck(s.alloc_zeros(cap), "alloc vt"), k1: ck(s.alloc_zeros(cap), "alloc vt"),
             ord: ck(s.alloc_zeros(cap), "alloc vt"), cap }
    }
    fn arg(&self, s: &CudaStream) -> TableArg {
        TableArg { st: self.st.device_ptr(s).0, k0: self.k0.device_ptr(s).0, k1: self.k1.device_ptr(s).0, val: self.ord.device_ptr(s).0,
                   mask: (self.cap - 1) as u64 }
    }
}

struct Job {
    x: usize,
    code: u32,
    k: u32,
    role: String,
    start: i64,
    count: i64,
    c2: u32,
    c3: u32,
    base: i64,
    segs: [String; 3],
}

struct Group { code: u32, slots: Vec<u32>, keys: Vec<(u64, u64)>, vals: Vec<u32> }

struct Size {
    jobs: Vec<Job>,
    n_try: i64,
    last: bool,
    b: i64,
    v: i64,
    l: i64,
    r: i64,
    mask: Vec<u32>,
    eid: u32,
    pools: HashMap<String, (Vec<u32>, CudaSlice<u32>)>,
}

struct Engine {
    stream: Arc<CudaStream>,
    ctx: Arc<CudaContext>,
    f_disc: CudaFunction,
    f_prod: CudaFunction,
    f_ins: CudaFunction,
    f_set: CudaFunction,
    f_claim: CudaFunction,
    f_keep: CudaFunction,
    f_badd: CudaFunction,
    n: usize,
    cap: usize,
    tst: CudaSlice<u32>,
    tk0: CudaSlice<u64>,
    tk1: CudaSlice<u64>,
    tval: CudaSlice<u32>,
    answered: Vec<(u64, u64, u32)>,
    bsync: usize,
    bhost: Vec<u32>,
    broles: Vec<u32>,
    roles: HashMap<String, u32>,
    bvec: CudaSlice<u32>,
    brows: usize,
    bseen: VT,
    size: Option<Size>,
    groups: Vec<Group>,
    stats: BTreeMap<&'static str, u64>,
    t_mark: Option<std::time::Instant>,
}

fn engines() -> &'static Mutex<HashMap<i64, Engine>> {
    static E: OnceLock<Mutex<HashMap<i64, Engine>>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_handle() -> i64 {
    static N: AtomicI64 = AtomicI64::new(1);
    N.fetch_add(1, Ordering::Relaxed)
}

fn ctx() -> Option<Arc<CudaContext>> {
    static C: OnceLock<Option<Arc<CudaContext>>> = OnceLock::new();
    C.get_or_init(|| {
        if PTX.is_empty() { return None; }
        let r = std::panic::catch_unwind(|| CudaContext::new(0).ok()).ok().flatten();
        if let Some(c) = &r { unsafe { c.disable_event_tracking(); } }
        r
    }).clone()
}

fn cfg(total: u64) -> LaunchConfig {
    let blocks = ((total + 255) / 256).clamp(1, 65_535) as u32;
    LaunchConfig { grid_dim: (blocks, 1, 1), block_dim: (256, 1, 1), shared_mem_bytes: 0 }
}

fn ck<T, E: std::fmt::Debug>(r: Result<T, E>, what: &str) -> T {
    r.unwrap_or_else(|e| panic!("gpu_p5: {what}: {e:?}"))
}

fn text(v: &Value) -> Arc<str> {
    match v {
        Value::Str(s) => s.clone(),
        other => panic!("gpu_p5: expected Text, got {other:?}"),
    }
}

fn items(pack: &str) -> Vec<Arc<str>> {
    match text_list_unpack(intern_str(pack)) {
        Value::List(xs) => xs.iter().map(text).collect(),
        other => panic!("gpu_p5: expected a list encoding, got {other:?}"),
    }
}

fn num(s: &str) -> i64 {
    s.parse().unwrap_or_else(|_| panic!("gpu_p5: {s:?} is not a number"))
}

fn num_v(v: Value) -> i64 {
    match v { Value::Int(x) => x, other => panic!("gpu_p5: expected Int, got {other:?}") }
}

fn pack_ids(ids: &[u32]) -> String {
    let mut lens = String::new();
    let mut body = String::new();
    for (i, id) in ids.iter().enumerate() {
        let s = if *id == REFUSED { "-2".to_string() } else { id.to_string() };
        if i > 0 { lens.push(','); }
        lens.push_str(&s.len().to_string());
        body.push_str(&s);
    }
    format!("{}|{}|{}", ids.len(), lens, body)
}

fn tr(msg: &str) {
    static ON: OnceLock<bool> = OnceLock::new();
    if *ON.get_or_init(|| std::env::var("AM_GPU_TRACE").as_deref() == Ok("1")) { eprintln!("gpu_p5 {msg}"); }
}

fn add(stats: &mut BTreeMap<&'static str, u64>, k: &'static str, v: u64) {
    *stats.entry(k).or_default() += v;
}

impl Engine {
    fn table_arg(&self) -> TableArg {
        let s = &self.stream;
        TableArg { st: self.tst.device_ptr(s).0, k0: self.tk0.device_ptr(s).0, k1: self.tk1.device_ptr(s).0, val: self.tval.device_ptr(s).0,
                   mask: (self.cap - 1) as u64 }
    }

    /// A fresh memo table of `cap` slots holding every answered key.
    fn rebuild(&mut self, cap: usize) {
        let s = self.stream.clone();
        self.cap = cap;
        self.tst = ck(s.alloc_zeros::<u32>(cap), "alloc table");
        self.tk0 = ck(s.alloc_zeros::<u64>(cap), "alloc table");
        self.tk1 = ck(s.alloc_zeros::<u64>(cap), "alloc table");
        self.tval = ck(s.alloc_zeros::<u32>(cap), "alloc table");
        if self.answered.is_empty() { return; }
        let k0: Vec<u64> = self.answered.iter().map(|e| e.0).collect();
        let k1: Vec<u64> = self.answered.iter().map(|e| e.1).collect();
        let vs: Vec<u32> = self.answered.iter().map(|e| e.2).collect();
        let dk0 = ck(s.clone_htod(&k0), "htod");
        let dk1 = ck(s.clone_htod(&k1), "htod");
        let dvs = ck(s.clone_htod(&vs), "htod");
        let flag = ck(s.alloc_zeros::<u32>(1), "alloc");
        let t = self.table_arg();
        let cnt = k0.len() as u64;
        let mut b = s.launch_builder(&self.f_ins);
        b.arg(&cnt).arg(&dk0).arg(&dk1).arg(&dvs).arg(&t).arg(&flag);
        ck(unsafe { b.launch(cfg(cnt)) }, "launch insert");
        if ck(s.clone_dtoh(&flag), "dtoh")[0] != 0 { panic!("gpu_p5: rebuild could not place every key ({cnt} in {cap})"); }
        add(&mut self.stats, "rebuilds", 1);
    }

    /// B rows from .. to into the B set (role, fingerprint).
    fn bseen_add(&mut self, from: usize, to: usize) {
        if to <= from { return; }
        let s = self.stream.clone();
        let roles = ck(s.clone_htod(&self.broles[from..to]), "htod roles");
        let mut flag = ck(s.alloc_zeros::<u32>(1), "alloc");
        let (n, f, t) = (self.n as u64, from as u64, to as u64);
        let bs = self.bseen.arg(&s);
        loop {
            ck(s.memset_zeros(&mut flag), "zero flag");
            let mut b = s.launch_builder(&self.f_badd);
            b.arg(&self.bvec).arg(&n).arg(&f).arg(&t).arg(&roles).arg(&bs).arg(&flag);
            ck(unsafe { b.launch(cfg(t - f)) }, "launch bseen_add");
            let fl = ck(s.clone_dtoh(&flag), "dtoh")[0];
            if fl & 2 != 0 { panic!("gpu_p5: the B set is full"); }
            if fl & 16 == 0 { break; }
        }
    }

    /// New B entries ("<role>\t<pack of V ids>") into the device copy of the B vectors and the B set.
    fn sync_b(&mut self, b: i64) {
        let len = num_v(fbank_len(b)) as usize;
        if len <= self.bsync { return; }
        for id in self.bsync..len {
            let t = text(&fbank_get(b, id as i64));
            let (role, pack) = t.split_once('\t').unwrap_or_else(|| panic!("gpu_p5: B entry {id} has no role"));
            let ids = items(pack);
            if ids.len() != self.n { panic!("gpu_p5: B entry {id} has {} values, not {}", ids.len(), self.n); }
            for x in ids { self.bhost.push(if &*x == "-2" { REFUSED } else { num(&x) as u32 }); }
            let k = self.roles.len() as u32 + 1;
            let r = *self.roles.entry(role.to_string()).or_insert(k);
            self.broles.push(r);
        }
        let s = self.stream.clone();
        if len > self.brows {
            self.brows = (len * 2).max(1024);
            let mut d = ck(s.alloc_zeros::<u32>(self.brows * self.n), "alloc bvec");
            ck(s.memcpy_htod(&self.bhost, &mut d.slice_mut(0..self.bhost.len())), "htod bvec");
            self.bvec = d;
        } else {
            let from = self.bsync * self.n;
            ck(s.memcpy_htod(&self.bhost[from..], &mut self.bvec.slice_mut(from..self.bhost.len())), "htod bvec");
        }
        if len * 2 > self.bseen.cap {
            self.bseen = VT::new(&s, (len * 4).next_power_of_two());
            self.bseen_add(0, len);
        } else {
            let from = self.bsync;
            self.bseen_add(from, len);
        }
        self.bsync = len;
    }

    fn role_id(&mut self, role: &str) -> u64 {
        let k = self.roles.len() as u32 + 1;
        *self.roles.entry(role.to_string()).or_insert(k) as u64
    }

    fn pool(&mut self, segs: &str) {
        let size = self.size.as_mut().unwrap();
        if size.pools.contains_key(segs) { return; }
        let mut ids = Vec::new();
        for seg in segs.split(',').filter(|x| !x.is_empty()) {
            let (h, c) = seg.split_once(':').unwrap_or_else(|| panic!("gpu_p5: bad pool segment {seg:?}"));
            let (h, c) = (num(h), num(c));
            for i in 0..c { ids.push(num(&text(&fbank_get(h, i))) as u32); }
        }
        let d = ck(self.stream.clone_htod(if ids.is_empty() { &[0u32][..] } else { &ids[..] }), "htod pool");
        size.pools.insert(segs.to_string(), (ids, d));
    }

    fn job_arg(&self, j: &Job) -> JobArg {
        let size = self.size.as_ref().unwrap();
        let p = |i: usize| size.pools.get(&j.segs[i]).map(|x| x.1.device_ptr(&self.stream).0).unwrap_or(0);
        JobArg { code: j.code, k: j.k, c2: j.c2, c3: j.c3, p1: p(0), p2: if j.k >= 2 { p(1) } else { 0 }, p3: if j.k >= 3 { p(2) } else { 0 } }
    }

    /// Combinations job j tries: [0, min(count, n_try - start)).
    fn tries(size: &Size, j: &Job) -> u64 {
        (j.count.min(size.n_try - j.start)).max(0) as u64
    }

    /// The jobs as (index, launch arg, combinations tried).
    fn plan(&self) -> Vec<(usize, JobArg, u64)> {
        let size = self.size.as_ref().unwrap();
        size.jobs.iter().enumerate().map(|(i, j)| (i, self.job_arg(j), Self::tries(size, j))).collect()
    }

    /// Discover the size's misses; on a full table grow it and start again. -> the groups pending.
    fn discover(&mut self) -> i64 {
        let plan = self.plan();
        loop {
            let s = self.stream.clone();
            let cap = self.cap;
            let miss = ck(s.alloc_zeros::<u32>(cap), "alloc miss");
            let mk0 = ck(s.alloc_zeros::<u64>(cap), "alloc miss");
            let mk1 = ck(s.alloc_zeros::<u64>(cap), "alloc miss");
            let nmiss = ck(s.alloc_zeros::<u32>(1), "alloc");
            let t = self.table_arg();
            let n = self.n as u64;
            let misscap = cap as u32;
            // a pass leaves an element unresolved (flag 16) rather than wait on a slot being written; the rerun finds
            // it written. Claims and the miss list carry over between passes.
            let f = loop {
                let flag = ck(s.alloc_zeros::<u32>(1), "alloc");
                for &(_, ja, m) in &plan {
                    let per = (CHUNK / n).max(1);
                    let mut j0 = 0u64;
                    while j0 < m {
                        let mm = per.min(m - j0);
                        let mut b = s.launch_builder(&self.f_disc);
                        b.arg(&ja).arg(&self.bvec).arg(&n).arg(&j0).arg(&mm).arg(&t).arg(&miss).arg(&mk0).arg(&mk1).arg(&nmiss).arg(&misscap).arg(&flag);
                        ck(unsafe { b.launch(cfg(mm * n)) }, "launch discover");
                        j0 += mm;
                    }
                }
                let f = ck(s.clone_dtoh(&flag), "dtoh")[0];
                tr(&format!("discover pass flag={f} cap={cap} jobs={}", plan.len()));
                if f & 16 == 0 || f & 2 != 0 { break f; }
                add(&mut self.stats, "reruns", 1);
            };
            if f & 2 != 0 {
                let c = self.cap * 4;
                self.rebuild(c);
                continue;
            }
            if f & 1 != 0 { panic!("gpu_p5: miss list overflow (cannot happen: it has a slot per table slot)"); }
            add(&mut self.stats, "elements", plan.iter().map(|p| p.2).sum::<u64>() * n);
            let k = ck(s.clone_dtoh(&nmiss), "dtoh")[0] as usize;
            if k == 0 { self.groups.clear(); return 0; }
            let slots = ck(s.clone_dtoh(&miss.slice(0..k)), "dtoh");
            let k0 = ck(s.clone_dtoh(&mk0.slice(0..k)), "dtoh");
            let k1 = ck(s.clone_dtoh(&mk1.slice(0..k)), "dtoh");
            let mut order: Vec<usize> = (0..k).collect();
            order.sort_unstable_by_key(|&i| (k0[i], k1[i]));
            let mut by: BTreeMap<u32, Group> = BTreeMap::new();
            for i in order {
                let code = (k0[i] >> 32) as u32;
                let g = by.entry(code).or_insert_with(|| Group { code, slots: Vec::new(), keys: Vec::new(), vals: Vec::new() });
                g.slots.push(slots[i]);
                g.keys.push((k0[i], k1[i]));
            }
            add(&mut self.stats, "misses", k as u64);
            self.groups = by.into_values().map(|mut g| { g.vals = vec![REFUSED; g.slots.len()]; g }).collect();
            return self.groups.len() as i64;
        }
    }

    /// The answers into the memo.
    fn settle(&mut self) {
        let s = self.stream.clone();
        let groups = std::mem::take(&mut self.groups);
        for g in &groups {
            for (i, &(k0, k1)) in g.keys.iter().enumerate() { self.answered.push((k0, k1, g.vals[i])); }
            let ds = ck(s.clone_htod(&g.slots), "htod");
            let dv = ck(s.clone_htod(&g.vals), "htod");
            let cnt = g.vals.len() as u64;
            let mut b = s.launch_builder(&self.f_set);
            b.arg(&cnt).arg(&ds).arg(&dv).arg(&self.tval);
            ck(unsafe { b.launch(cfg(cnt)) }, "launch setval");
        }
    }

    fn produce_into(&self, ja: &JobArg, j0: u64, mm: u64, out: &CudaSlice<u32>, flag: &CudaSlice<u32>) {
        let s = &self.stream;
        let n = self.n as u64;
        let t = self.table_arg();
        let mut b = s.launch_builder(&self.f_prod);
        b.arg(ja).arg(&self.bvec).arg(&n).arg(&j0).arg(&mm).arg(&t).arg(out).arg(flag);
        ck(unsafe { b.launch(cfg(mm * n)) }, "launch produce");
    }

    /// The size on the GPU: produce, claim (skips and the lowest ordinal per fingerprint), keep; then the survivors
    /// banked and recorded on the host in ordinal order. -> 0 | -1 (a bank is full or a value was refused).
    fn produce(&mut self) -> i64 {
        let s = self.stream.clone();
        let n = self.n as u64;
        let plan = self.plan();
        let total: u64 = plan.iter().map(|p| p.2).sum();
        if total == 0 { return 0; }
        let (last, eid, mask) = { let z = self.size.as_ref().unwrap(); (z.last, z.eid, z.mask.clone()) };
        let roles: Vec<u64> = (0..plan.len()).map(|i| { let r = self.size.as_ref().unwrap().jobs[i].role.clone(); self.role_id(&r) }).collect();
        let dmask = ck(s.clone_htod(&mask), "htod mask");
        let vt = VT::new(&s, ((total * 2) as usize).next_power_of_two().max(1024));
        let cand = ck(s.alloc_zeros::<u8>(total as usize), "alloc cand");
        let mut flag = ck(s.alloc_zeros::<u32>(1), "alloc");
        let per = (CHUNK / n).max(1);
        let maxm = plan.iter().map(|p| p.2.min(per)).max().unwrap_or(1).max(1);
        let out = ck(s.alloc_zeros::<u32>((maxm * n) as usize), "alloc out");
        let bs = self.bseen.arg(&s);
        let vta = vt.arg(&s);
        let candp = cand.device_ptr(&s).0;
        let maskp = dmask.device_ptr(&s).0;
        let bases: Vec<i64> = self.size.as_ref().unwrap().jobs.iter().map(|j| j.base).collect();
        let filt = |ji: usize, j0: u64| FiltArg { n, eid, last: last as u32, role: roles[ji], ltag: LTAG, mask: maskp, ord0: (bases[ji] + j0 as i64) as u64 };
        // pass 1: produce + claim over the whole size (rerun while an element met a slot being written: idempotent)
        let fl = loop {
            ck(s.memset_zeros(&mut flag), "zero flag");
            let mut off = 0u64;
            for &(ji, ja, m) in &plan {
                let mut j0 = 0u64;
                while j0 < m {
                    let mm = per.min(m - j0);
                    self.produce_into(&ja, j0, mm, &out, &flag);
                    let f = filt(ji, j0);
                    let cp = candp + off;
                    let mut b = s.launch_builder(&self.f_claim);
                    b.arg(&f).arg(&out).arg(&mm).arg(&bs).arg(&vta).arg(&cp).arg(&flag);
                    ck(unsafe { b.launch(cfg(mm)) }, "launch claim");
                    off += mm;
                    j0 += mm;
                }
            }
            let fl = ck(s.clone_dtoh(&flag), "dtoh")[0];
            tr(&format!("claim pass flag={fl} total={total} vt={}", vt.cap));
            if fl & 16 == 0 { break fl; }
            add(&mut self.stats, "reruns", 1);
        };
        if fl & 4 != 0 { panic!("gpu_p5: produce met an unanswered key"); }
        if fl & 2 != 0 { panic!("gpu_p5: the per-size table is full (it has two slots per combination)"); }
        let mut full = fl & 8 != 0;
        // pass 2: keep the lowest-ordinal holders, then bank and record them
        let kept = ck(s.alloc_zeros::<u32>(maxm as usize), "alloc kept");
        let kvec = ck(s.alloc_zeros::<u32>((maxm * n) as usize), "alloc kvec");
        let mut off = 0u64;
        let mut nkept = 0u64;
        for &(ji, ja, m) in &plan {
            let mut j0 = 0u64;
            while j0 < m {
                let mm = per.min(m - j0);
                self.produce_into(&ja, j0, mm, &out, &flag);
                let nkz = ck(s.alloc_zeros::<u32>(1), "alloc");
                let f = filt(ji, j0);
                let cp = candp + off;
                let mut b = s.launch_builder(&self.f_keep);
                b.arg(&f).arg(&out).arg(&mm).arg(&vta).arg(&cp).arg(&kept).arg(&kvec).arg(&nkz);
                ck(unsafe { b.launch(cfg(mm)) }, "launch keep");
                let k = ck(s.clone_dtoh(&nkz), "dtoh")[0] as usize;
                if k > 0 {
                    let idx = ck(s.clone_dtoh(&kept.slice(0..k)), "dtoh kept");
                    let vecs = ck(s.clone_dtoh(&kvec.slice(0..k * self.n)), "dtoh kvec");
                    let tr = std::time::Instant::now();
                    full |= self.record(ji, j0, &idx, &vecs);
                    add(&mut self.stats, "us_record", tr.elapsed().as_micros() as u64);
                    nkept += k as u64;
                }
                off += mm;
                j0 += mm;
            }
        }
        add(&mut self.stats, "combinations", total);
        add(&mut self.stats, "survivors", nkept);
        if full { -1 } else { 0 }
    }

    /// The survivors of a chunk (combination indices from j0, unordered, and their vectors): banked and recorded in
    /// ordinal order. -> full.
    fn record(&mut self, ji: usize, j0: u64, idx: &[u32], vecs: &[u32]) -> bool {
        let n = self.n;
        let size = self.size.as_ref().unwrap();
        let j = &size.jobs[ji];
        let mut order: Vec<usize> = (0..idx.len()).collect();
        order.sort_unstable_by_key(|&i| idx[i]);
        let mut full = false;
        let (p1, p2, p3) = (&size.pools[&j.segs[0]].0, size.pools.get(&j.segs[1]).map(|x| &x.0), size.pools.get(&j.segs[2]).map(|x| &x.0));
        let (c2, c3) = (j.c2 as u64, j.c3 as u64);
        let mut kept = 0u64;
        for i in order {
            let v = &vecs[i * n..(i + 1) * n];
            let jj = j0 + idx[i] as u64;
            let ord = j.base + jj as i64;
            let pack: Arc<str> = intern_str(&pack_ids(v));
            let id = if size.last { num_v(fbank_put(size.l, ord, pack)) } else { num_v(fbank_put(size.b, ord, intern_str(&format!("{}\t{}", j.role, pack)))) };
            if id == -2 { full = true; }
            if id < 0 { continue; }
            let (i1, i2, i3) = (jj / (c3 * c2), (jj / c3) % c2, jj % c3);
            let a1 = p1[i1 as usize].to_string();
            let a2 = if j.k >= 2 { p2.unwrap()[i2 as usize].to_string() } else { String::new() };
            let a3 = if j.k >= 3 { p3.unwrap()[i3 as usize].to_string() } else { String::new() };
            let rec = format!("{}\t{}\t{}\t{}\t{}", j.x, id, a1, a2, a3);
            if num_v(fbank_put(size.r, 0, intern_str(&rec))) == -2 { full = true; }
            kept += 1;
        }
        add(&mut self.stats, "kept", kept);
        full
    }
}

fn with<R>(e: i64, f: impl FnOnce(&mut Engine) -> R) -> R {
    let mut m = engines().lock().unwrap();
    let eng = m.get_mut(&e).unwrap_or_else(|| panic!("gpu_p5: {e} is not an engine"));
    ck(eng.ctx.bind_to_thread(), "bind");
    f(eng)
}

/// `gpu_p5_open(ncols: Int) -> Int` — an engine for vectors of `ncols` values, or 0 (no GPU or no kernels).
#[track_caller]
pub fn gpu_p5_open(ncols: i64) -> Value {
    let Some(c) = ctx() else { return Value::Int(0) };
    if ncols < 1 { panic!("gpu_p5_open: ncols must be >= 1 (got {ncols})"); }
    ck(c.bind_to_thread(), "bind");
    // the kernels are loaded once per process (a search opens an engine; a controller runs thousands of searches)
    static M: OnceLock<Arc<cudarc::driver::CudaModule>> = OnceLock::new();
    let m = M.get_or_init(|| ck(c.load_module(cudarc::nvrtc::Ptx::from_src(PTX)), "load kernels")).clone();
    let s = c.default_stream();
    let cap = 1usize << 20;
    let mut e = Engine {
        f_disc: ck(m.load_function("p5_discover"), "p5_discover"),
        f_prod: ck(m.load_function("p5_produce"), "p5_produce"),
        f_ins: ck(m.load_function("p5_insert"), "p5_insert"),
        f_set: ck(m.load_function("p5_setval"), "p5_setval"),
        f_claim: ck(m.load_function("p5_claim"), "p5_claim"),
        f_keep: ck(m.load_function("p5_keep"), "p5_keep"),
        f_badd: ck(m.load_function("p5_bseen_add"), "p5_bseen_add"),
        n: ncols as usize,
        cap,
        tst: ck(s.alloc_zeros::<u32>(1), "alloc"),
        tk0: ck(s.alloc_zeros::<u64>(1), "alloc"),
        tk1: ck(s.alloc_zeros::<u64>(1), "alloc"),
        tval: ck(s.alloc_zeros::<u32>(1), "alloc"),
        answered: Vec::new(),
        bsync: 0,
        bhost: Vec::new(),
        broles: Vec::new(),
        roles: HashMap::new(),
        bvec: ck(s.alloc_zeros::<u32>(1), "alloc"),
        brows: 0,
        bseen: VT::new(&s, 1024),
        size: None,
        groups: Vec::new(),
        stats: BTreeMap::new(),
        t_mark: None,
        ctx: c.clone(),
        stream: s,
    };
    e.rebuild(cap);
    let h = next_handle();
    engines().lock().unwrap().insert(h, e);
    Value::Int(h)
}

/// `gpu_p5_size(e, jobs: TextList, n: Int, last: Bool, B, V, L, R: Int, mask: Text, alle: Text) -> Int` — start one
/// size: `jobs` are am_p5's job packs, `n` the combinations to try (the budget cut). -> miss groups pending.
#[track_caller]
#[allow(clippy::too_many_arguments)]
pub fn gpu_p5_size(e: i64, jobs: Value, n: i64, last: bool, b: i64, v: i64, l: i64, r: i64, mask: Arc<str>, alle: Arc<str>) -> Value {
    with(e, |eng| {
        tr(&format!("size n={n} last={last}"));
        eng.sync_b(b);
        tr("synced");
        let js: Vec<Job> = match &jobs {
            Value::List(xs) => xs.iter().enumerate().map(|(x, p)| {
                let f = items(&text(p));
                let segs = [f[8].to_string(), f[9].to_string(), f[10].to_string()];
                Job { x, code: num(&f[0]) as u32, k: num(&f[1]) as u32, role: f[2].to_string(), start: num(&f[3]), count: num(&f[4]),
                      c2: num(&f[5]) as u32, c3: num(&f[6]) as u32, base: num(&f[7]), segs }
            }).collect(),
            other => panic!("gpu_p5_size: jobs must be a TextList, got {other:?}"),
        };
        let mk: Vec<u32> = items(&mask).iter().map(|x| if &**x == "*" { NONE } else { num(x) as u32 }).collect();
        let eid = num(&items(&alle)[0]) as u32;
        let segs: Vec<String> = js.iter().flat_map(|j| j.segs.iter().take(j.k as usize).cloned()).collect();
        eng.size = Some(Size { jobs: js, n_try: n, last, b, v, l, r, mask: mk, eid, pools: HashMap::new() });
        for sg in segs { eng.pool(&sg); }
        if eng.answered.len() * 3 > eng.cap { let c = eng.cap * 4; eng.rebuild(c); }
        let t0 = std::time::Instant::now();
        let g = eng.discover();
        add(&mut eng.stats, "ms_discover", t0.elapsed().as_millis() as u64);
        eng.t_mark = Some(std::time::Instant::now());
        Value::Int(g)
    })
}

/// `gpu_p5_group_code(e, g: Int) -> Int` — the op code of pending miss group g.
#[track_caller]
pub fn gpu_p5_group_code(e: i64, g: i64) -> Value {
    with(e, |eng| Value::Int(eng.groups[g as usize].code as i64))
}

/// `gpu_p5_group_len(e, g: Int) -> Int` — the misses in group g.
#[track_caller]
pub fn gpu_p5_group_len(e: i64, g: i64) -> Value {
    with(e, |eng| Value::Int(eng.groups[g as usize].slots.len() as i64))
}

/// `gpu_p5_group_args(e, g, pos, from, to: Int) -> TextList` — argument `pos` (0..2) of misses from .. to of group g,
/// as values (texts from V); the empty list for an argument the op does not take. Callable from any thread.
#[track_caller]
pub fn gpu_p5_group_args(e: i64, g: i64, pos: i64, from: i64, to: i64) -> Value {
    let (v, ids) = with(e, |eng| {
        let grp = &eng.groups[g as usize];
        let to = (to.max(0) as usize).min(grp.keys.len());
        let from = (from.max(0) as usize).min(to);
        let ids: Vec<u32> = grp.keys[from..to].iter().map(|&(k0, k1)| match pos { 0 => k0 as u32, 1 => (k1 >> 32) as u32, _ => k1 as u32 }).collect();
        (eng.size.as_ref().unwrap().v, ids)
    });
    if ids.iter().any(|&x| x == NONE) { return Value::List(ListBuf::from(Vec::new())); }
    Value::List(ListBuf::from(ids.iter().map(|&id| if id == REFUSED { Value::Str(intern_str("-2")) } else { fbank_get(v, id as i64) }).collect::<Vec<_>>()))
}

/// `gpu_p5_answer(e, g, from: Int, outs: TextList) -> Int` — the answers of misses from .. from+len(outs) of group g,
/// interned into V. -> 0 | -1 (V is full: those answers become "-2"). Callable from any thread.
#[track_caller]
pub fn gpu_p5_answer(e: i64, g: i64, from: i64, outs: Value) -> Value {
    let v = with(e, |eng| eng.size.as_ref().unwrap().v);
    let ids = fbank_intern_list(v, outs);
    let ids: Vec<i64> = match &ids { Value::List(xs) => xs.iter().map(|x| num(&text(x))).collect(), _ => unreachable!() };
    with(e, |eng| {
        let refused = ids.iter().filter(|&&x| x < 0).count() as u64;
        if refused > 0 { add(&mut eng.stats, "refused", refused); }
        let grp = &mut eng.groups[g as usize];
        let from = from as usize;
        if from + ids.len() > grp.vals.len() { panic!("gpu_p5_answer: answers {from}..{} past the group's {} misses", from + ids.len(), grp.vals.len()); }
        for (i, &x) in ids.iter().enumerate() { grp.vals[from + i] = if x < 0 { REFUSED } else { x as u32 }; }
        add(&mut eng.stats, "answered", ids.len() as u64);
        Value::Int(if refused > 0 { -1 } else { 0 })
    })
}

/// `gpu_p5_step(e) -> Int` — after the answers: the size banked and recorded (0), or -1 (a bank is full).
#[track_caller]
pub fn gpu_p5_step(e: i64) -> Value {
    with(e, |eng| {
        if let Some(t) = eng.t_mark.take() { add(&mut eng.stats, "ms_answer", t.elapsed().as_millis() as u64); }
        let t0 = std::time::Instant::now();
        eng.settle();
        let r = eng.produce();
        add(&mut eng.stats, "ms_produce", t0.elapsed().as_millis() as u64);
        eng.size = None;
        Value::Int(r)
    })
}

/// `gpu_p5_stats(e) -> Text` — "key=value ..." counters.
#[track_caller]
pub fn gpu_p5_stats(e: i64) -> Value {
    with(e, |eng| {
        let s: Vec<String> = eng.stats.iter().map(|(k, v)| format!("{k}={v}")).collect();
        Value::Str(intern_str(&format!("{} memo={} cap={}", s.join(" "), eng.answered.len(), eng.cap)))
    })
}

/// `gpu_p5_close(e) -> Unit` — the engine and its device memory released.
#[track_caller]
pub fn gpu_p5_close(e: i64) -> Value {
    if std::env::var("AM_GPU_STATS").as_deref() == Ok("1") { eprintln!("gpu_p5 {}", text(&gpu_p5_stats(e))); }
    engines().lock().unwrap().remove(&e);
    Value::Unit
}

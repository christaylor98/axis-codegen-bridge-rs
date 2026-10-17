//! AX146 step B: SPECULATIVE NATIVE FAST PATHS.
//!
//! For a function whose straight-line (top-level) nodes are all pure scalar/text operations, the emitter writes a
//! second function beside the generic one:
//!
//! ```text
//! fn <name>_fast(args: &Value) -> Option<Value>
//! ```
//!
//! It reads the arguments by reference, computes in native `i64` / `bool` locals with checked arithmetic and no
//! `Value` box, no `guard_pre`, no `catch_unwind`, and returns `Some(result)`. ANY anomaly returns `None`:
//! a type that is not the one the op wants (which includes a sticky Unknown/Err, whose tag no op accepts), an
//! overflow, a division by zero, an index out of range, an unsupported node reached in an arm. The caller then runs
//! the unchanged generic body from the start.
//!
//! Why that is exact: everything the fast path runs before it returns is a pure, deterministic builtin (the registry
//! says so: `pure_det`), so having run it and thrown the result away is unobservable. When it returns `Some(v)`, every
//! op it ran was on well-typed operands and succeeded, which is exactly when the generic body would have returned the
//! same `v` (with `guard_pre` finding nothing sticky). Every failure -- a DEFECT, an Unknown, an Err -- is therefore
//! produced by the generic body, with its own message, location and exit code, never by this module.
//!
//! Composite callees from the `--lib` set are inlined into the fast path (their nodes are walked the same way).
//! A node the fast path cannot do is a REJECT of the whole function if it sits on the straight line, and a BAIL
//! (`return None`) if it sits in an `if` arm (typically a cold error arm that calls an effectful `fail`).

use crate::core_ir_05::{
    bool_type_hash, decode_bool_payload, decode_int_payload, decode_text_payload, fn_type_hash, int_type_hash,
    param_type_hash, text_type_hash, unit_type_hash, verify_blocks, Arm, Blocks, CoreBundle, Hash256, Node, NodeRef,
};
use std::collections::{HashMap, HashSet};

const RT: &str = "axis_codegen_bridge::runtime";
const MAX_INLINE_DEPTH: usize = 8;
const MAX_INLINE_NODES: usize = 4000;

/// What the fast path needs to resolve a CCall target.
pub struct Resolver<'a> {
    pub builtin: &'a HashMap<Hash256, &'static str>,
    pub registry: &'a HashMap<Hash256, String>,
    pub name_to_path: &'a HashMap<&'static str, &'static str>,
    pub pure_det: &'a HashSet<Hash256>,
    /// composite bodies available for inlining, by the identity a CCall uses
    pub callees: &'a HashMap<Hash256, &'a CoreBundle>,
}

impl Resolver<'_> {
    fn op_path(&self, id: &Hash256) -> Option<&'static str> {
        self.builtin
            .get(id)
            .copied()
            .or_else(|| self.registry.get(id).and_then(|n| self.name_to_path.get(n.as_str()).copied()))
    }
}

/// A node's value in the fast path. Every variant holds a Rust expression that is a plain local or literal.
#[derive(Clone, Debug)]
enum Val {
    I(String),   // i64
    B(String),   // bool
    R(String),   // &Value, borrowed from the arguments
    T(String),   // &Arc<str>
    L(String),   // &'static str (a Text literal)
    P(String),   // &Arc<str> that came from str_char_at: only value_0 may take it
    O(String),   // an owned Value this function built
    U,           // Unit
    Never,       // the code that produced it has already returned None
}

type Res<T> = Result<T, String>;

struct St<'b> {
    b: &'b CoreBundle,
    blocks: Blocks,
    pool: Vec<Option<Val>>,
    vals: Vec<Option<Val>>,
}

struct Gen<'a> {
    r: &'a Resolver<'a>,
    out: String,
    n: usize,
    arm_depth: usize,
    stack: Vec<Hash256>,
    inlined_nodes: usize,
    owned: Vec<HashSet<String>>,
}

fn ty(v: &Val) -> &'static str {
    match v {
        Val::I(_) | Val::Never => "i64",
        Val::B(_) => "bool",
        Val::R(_) => "&Value",
        Val::T(_) | Val::P(_) => "&std::sync::Arc<str>",
        Val::L(_) => "&'static str",
        Val::O(_) => "Value",
        Val::U => "()",
    }
}

fn expr(v: &Val) -> String {
    match v {
        Val::I(e) | Val::B(e) | Val::R(e) | Val::T(e) | Val::L(e) | Val::P(e) | Val::O(e) => e.clone(),
        Val::U => "()".into(),
        Val::Never => "return None".into(),
    }
}

fn with_expr(like: &Val, e: String) -> Val {
    match like {
        Val::I(_) | Val::Never => Val::I(e),
        Val::B(_) => Val::B(e),
        Val::R(_) => Val::R(e),
        Val::T(_) => Val::T(e),
        Val::P(_) => Val::P(e),
        Val::L(_) => Val::L(e),
        Val::O(_) => Val::O(e),
        Val::U => Val::U,
    }
}

impl<'a> Gen<'a> {
    fn fresh(&mut self) -> String {
        self.n += 1;
        format!("t{}", self.n)
    }

    fn line(&mut self, s: String) {
        self.out.push_str("    ");
        self.out.push_str(&s);
        self.out.push('\n');
    }

    // ── coercions: each either yields a native expression or Err (the op is not fast-able here) ────────────

    fn to_i(&mut self, v: &Val) -> Res<String> {
        match v {
            Val::I(e) => Ok(e.clone()),
            Val::R(e) => {
                let t = self.fresh();
                self.line(format!("let {t}: i64 = match {e} {{ Value::Int(x) => *x, _ => return None }};"));
                Ok(t)
            }
            other => Err(format!("not an Int: {:?}", other)),
        }
    }

    fn to_b(&mut self, v: &Val) -> Res<String> {
        match v {
            Val::B(e) => Ok(e.clone()),
            Val::R(e) => {
                let t = self.fresh();
                self.line(format!("let {t}: bool = match {e} {{ Value::Bool(x) => *x, _ => return None }};"));
                Ok(t)
            }
            other => Err(format!("not a Bool: {:?}", other)),
        }
    }

    /// An `&Arc<str>`.
    fn to_t(&mut self, v: &Val) -> Res<String> {
        match v {
            Val::T(e) => Ok(e.clone()),
            Val::R(e) => {
                let t = self.fresh();
                self.line(format!("let {t}: &std::sync::Arc<str> = match {e} {{ Value::Str(x) => x, _ => return None }};"));
                Ok(t)
            }
            other => Err(format!("not a Text: {:?}", other)),
        }
    }

    /// A `&str`.
    fn to_s(&mut self, v: &Val) -> Res<String> {
        match v {
            Val::L(e) => Ok(e.clone()),
            other => {
                let t = self.to_t(other)?;
                Ok(format!("&**{}", t))
            }
        }
    }

    /// The value as an owned `Value`. `move_ok`: this arm/function built it, so it may be moved, not cloned.
    fn boxed(&self, v: &Val, move_ok: bool) -> Res<String> {
        Ok(match v {
            Val::I(e) => format!("Value::Int({e})"),
            Val::B(e) => format!("Value::Bool({e})"),
            Val::R(e) => format!("({e}).clone()"),
            Val::T(e) => format!("Value::Str(std::sync::Arc::clone({e}))"),
            Val::L(e) => format!("Value::Str(std::sync::Arc::from({e}))"),
            Val::O(e) => if move_ok { e.clone() } else { format!("{e}.clone()") },
            Val::U => "Value::Unit".into(),
            Val::Never => "return None".into(),
            Val::P(_) => return Err("an Option(Text) cannot be boxed".into()),
        })
    }

    fn owns(&self, v: &Val) -> bool {
        matches!(v, Val::O(e) if self.owned.last().map_or(false, |s| s.contains(e)))
    }

    fn val(&self, st: &St, r: &NodeRef) -> Res<Val> {
        match r {
            NodeRef::Node(i) => st.vals[*i as usize].clone().ok_or_else(|| format!("node {} has no fast value", i)),
            NodeRef::Pool(i) => st.pool[*i as usize].clone().ok_or_else(|| format!("pool {} is not fast-representable", i)),
        }
    }

    // ── ranges of nodes ──────────────────────────────────────────────────────────────────────────────────

    /// Emit nodes `from..to`. Ok(true): the code already ended in `return None` (nothing after it runs).
    fn gen_range(&mut self, st: &mut St, from: usize, to: usize) -> Res<bool> {
        let mut i = from;
        while i < to {
            match &st.b.nodes[i] {
                Node::CCall { target_identity, args, .. } => {
                    match self.gen_call(st, target_identity, args) {
                        Ok(v) => st.vals[i] = Some(v),
                        Err(e) => {
                            if self.arm_depth > 0 {
                                self.line("return None;".into());
                                return Ok(true);
                            }
                            return Err(e);
                        }
                    }
                    i += 1;
                }
                Node::CBlock => {
                    let e = st.blocks.partner[i].ok_or("unpartnered block")? as usize;
                    match st.blocks.arm[e] {
                        Some((k, Arm::Then)) => {
                            let next = self.gen_if(st, i, e, k as usize)?;
                            match next {
                                (Some(k_next), dead) => {
                                    if dead { return Ok(true); }
                                    i = k_next;
                                }
                                (None, _) => return Ok(true),
                            }
                        }
                        Some((_, Arm::Else)) => return Err("else arm reached on its own".into()),
                        None => {
                            // plain grouping: always runs, every temp is uniquely named, so no braces are needed
                            self.owned.push(HashSet::new());
                            let dead = self.gen_range(st, i + 1, e)?;
                            let inner = self.owned.pop().unwrap_or_default();
                            if dead { return Ok(true); }
                            let Node::CEnd { value } = &st.b.nodes[e] else { return Err("block without CEnd".into()) };
                            let v = self.val(st, value)?;
                            if let Val::O(n) = &v { if inner.contains(n) { self.owned.last_mut().map(|s| s.insert(n.clone())); } }
                            st.vals[e] = Some(v);
                            i = e + 1;
                        }
                    }
                }
                Node::CIf { .. } => return Err("CIf reached outside its arms".into()),
                Node::CEnd { .. } => return Err("stray CEnd".into()),
                Node::CDeterminate => return Err("CDeterminate".into()),
            }
        }
        Ok(false)
    }

    /// An if: `then_open` is the then-arm's CBlock, `then_end` its CEnd, `k` the CIf. Returns the index after the CIf.
    fn gen_if(&mut self, st: &mut St, then_open: usize, then_end: usize, k: usize) -> Res<(Option<usize>, bool)> {
        let Node::CIf { cond, else_, .. } = &st.b.nodes[k] else { return Err("arm without CIf".into()) };
        let cond = cond.clone();
        let else_ = else_.clone();
        let cv = self.val(st, &cond);
        let cexpr = match cv.and_then(|v| self.to_b(&v)) {
            Ok(c) => c,
            Err(e) => {
                if self.arm_depth > 0 { self.line("return None;".into()); return Ok((None, true)); }
                return Err(e);
            }
        };

        self.arm_depth += 1;
        // then arm
        self.owned.push(HashSet::new());
        let saved = std::mem::take(&mut self.out);
        let dead_t = self.gen_range(st, then_open + 1, then_end)?;
        let (tv, t_move) = if dead_t {
            (Val::Never, false)
        } else {
            let Node::CEnd { value } = &st.b.nodes[then_end] else { return Err("arm without CEnd".into()) };
            let v = self.val(st, value)?;
            let mv = self.owns(&v);
            (v, mv)
        };
        let then_code = std::mem::replace(&mut self.out, saved);
        self.owned.pop();

        // else arm (a one-armed if has pool Unit there)
        let (ev, e_move, else_code, after) = match &else_ {
            NodeRef::Node(m) => {
                let m = *m as usize;
                let open = st.blocks.partner[m].ok_or("else CEnd without block")? as usize;
                if open != then_end + 1 || m + 1 != k {
                    return Err("arms are not laid out directly before their CIf".into());
                }
                self.owned.push(HashSet::new());
                let saved = std::mem::take(&mut self.out);
                let dead_e = self.gen_range(st, open + 1, m)?;
                let (ev, mv) = if dead_e {
                    (Val::Never, false)
                } else {
                    let Node::CEnd { value } = &st.b.nodes[m] else { return Err("arm without CEnd".into()) };
                    let v = self.val(st, value)?;
                    let mv = self.owns(&v);
                    (v, mv)
                };
                let code = std::mem::replace(&mut self.out, saved);
                self.owned.pop();
                (ev, mv, code, k + 1)
            }
            NodeRef::Pool(_) => {
                if then_end + 1 != k { return Err("one-armed if is not directly after its arm".into()); }
                (Val::U, false, String::new(), k + 1)
            }
        };
        self.arm_depth -= 1;

        // unify the two arms' values
        let same = |a: &Val, b: &Val| matches!((a, b),
            (Val::I(_), Val::I(_)) | (Val::B(_), Val::B(_)) | (Val::R(_), Val::R(_)) | (Val::T(_), Val::T(_))
            | (Val::L(_), Val::L(_)) | (Val::U, Val::U) | (Val::O(_), Val::O(_)));
        let (ty_s, te, ee, mk): (String, String, String, Box<dyn Fn(String) -> Val>) = match (&tv, &ev) {
            (Val::Never, Val::Never) => ("i64".into(), "return None".into(), "return None".into(), Box::new(Val::I)),
            (Val::Never, o) => {
                let like = o.clone();
                (ty(o).into(), "return None".into(), self.boxed_same(o)?, Box::new(move |e| with_expr(&like, e)))
            }
            (o, Val::Never) => {
                let like = o.clone();
                (ty(o).into(), self.boxed_same(o)?, "return None".into(), Box::new(move |e| with_expr(&like, e)))
            }
            (a, b) if same(a, b) => {
                let like = a.clone();
                (ty(a).into(), expr(a), expr(b), Box::new(move |e| with_expr(&like, e)))
            }
            (a, b) => (
                "Value".into(),
                self.boxed(a, t_move)?,
                self.boxed(b, e_move)?,
                Box::new(Val::O),
            ),
        };
        let t = self.fresh();
        self.line(format!("let {t}: {ty_s} = if {cexpr} {{"));
        self.out.push_str(&then_code);
        self.line(format!("    {te}"));
        self.line("} else {".into());
        self.out.push_str(&else_code);
        self.line(format!("    {ee}"));
        self.line("};".into());
        let v = mk(t.clone());
        if let Val::O(_) = &v { self.owned.last_mut().map(|s| s.insert(t)); }
        st.vals[k] = Some(v);
        Ok((Some(after), false))
    }

    fn boxed_same(&self, v: &Val) -> Res<String> {
        // the arm's own value expression; an owned Value built in the arm is moved out
        Ok(expr(v))
    }

    // ── calls ────────────────────────────────────────────────────────────────────────────────────────────

    fn gen_call(&mut self, st: &St, id: &Hash256, args: &[NodeRef]) -> Res<Val> {
        let mut vals = Vec::with_capacity(args.len());
        for a in args {
            vals.push(self.val(st, a)?);
        }
        if let Some(path) = self.r.op_path(id) {
            if !self.r.pure_det.contains(id) {
                return Err(format!("{} is not pure+deterministic", path));
            }
            let short = path.strip_prefix("axis_codegen_bridge::runtime::").ok_or("op outside the runtime")?;
            return self.gen_op(short, vals);
        }
        if let Some(callee) = self.r.callees.get(id) {
            return self.gen_inline(callee, *id, vals);
        }
        Err("unresolved callee".into())
    }

    fn gen_inline(&mut self, callee: &CoreBundle, id: Hash256, args: Vec<Val>) -> Res<Val> {
        if self.stack.len() >= MAX_INLINE_DEPTH || self.stack.contains(&id) {
            return Err("inline depth / recursion".into());
        }
        self.inlined_nodes += callee.nodes.len();
        if self.inlined_nodes > MAX_INLINE_NODES {
            return Err("inline budget".into());
        }
        let pc = param_count(callee);
        if pc as usize != args.len() {
            return Err("arity differs from the callee's parameter count".into());
        }
        self.stack.push(id);
        let mut st = St::new(callee, &args)?;
        self.owned.push(HashSet::new());
        let dead = self.gen_range(&mut st, 0, callee.nodes.len());
        let inner = self.owned.pop().unwrap_or_default();
        self.stack.pop();
        if dead? {
            return Ok(Val::Never);
        }
        let v = self.val(&st, &callee.result)?;
        if let Val::O(n) = &v { if inner.contains(n) { self.owned.last_mut().map(|s| s.insert(n.clone())); } }
        Ok(v)
    }

    fn gen_op(&mut self, op: &str, a: Vec<Val>) -> Res<Val> {
        let want = |n: usize| if a.len() == n { Ok(()) } else { Err(format!("{}: arity {}", op, a.len())) };
        match op {
            "arith::int_add" | "arith::int_sub" | "arith::int_mul" | "arith::int_div" | "arith::int_mod" => {
                want(2)?;
                let (x, y) = (self.to_i(&a[0])?, self.to_i(&a[1])?);
                let m = match op {
                    "arith::int_add" => "checked_add",
                    "arith::int_sub" => "checked_sub",
                    "arith::int_mul" => "checked_mul",
                    "arith::int_div" => "checked_div_euclid",
                    _ => "checked_rem_euclid",
                };
                let t = self.fresh();
                self.line(format!("let {t}: i64 = match {x}.{m}({y}) {{ Some(r) => r, None => return None }};"));
                Ok(Val::I(t))
            }
            "arith::int_lt" | "arith::int_lte" | "arith::int_gt" | "arith::int_gte" | "arith::int_eq" => {
                want(2)?;
                let (x, y) = (self.to_i(&a[0])?, self.to_i(&a[1])?);
                let o = match op {
                    "arith::int_lt" => "<",
                    "arith::int_lte" => "<=",
                    "arith::int_gt" => ">",
                    "arith::int_gte" => ">=",
                    _ => "==",
                };
                let t = self.fresh();
                self.line(format!("let {t}: bool = {x} {o} {y};"));
                Ok(Val::B(t))
            }
            "arith::int_bit_and" | "arith::int_bit_or" | "arith::int_bit_xor" => {
                want(2)?;
                let (x, y) = (self.to_i(&a[0])?, self.to_i(&a[1])?);
                let o = match op { "arith::int_bit_and" => "&", "arith::int_bit_or" => "|", _ => "^" };
                let t = self.fresh();
                self.line(format!("let {t}: i64 = {x} {o} {y};"));
                Ok(Val::I(t))
            }
            "arith::int_bit_not" => {
                want(1)?;
                let x = self.to_i(&a[0])?;
                let t = self.fresh();
                self.line(format!("let {t}: i64 = !{x};"));
                Ok(Val::I(t))
            }
            "arith::int_shr" => {
                want(2)?;
                let (x, n) = (self.to_i(&a[0])?, self.to_i(&a[1])?);
                let t = self.fresh();
                self.line(format!(
                    "let {t}: i64 = if {n} < 0 {{ return None }} else if {n} > 63 {{ if {x} < 0 {{ -1 }} else {{ 0 }} }} else {{ {x} >> {n} }};"
                ));
                Ok(Val::I(t))
            }
            "bool_ops::bool_and" | "bool_ops::bool_or" => {
                want(2)?;
                let (x, y) = (self.to_b(&a[0])?, self.to_b(&a[1])?);
                let o = if op.ends_with("and") { "&&" } else { "||" };
                let t = self.fresh();
                self.line(format!("let {t}: bool = {x} {o} {y};"));
                Ok(Val::B(t))
            }
            "bool_ops::bool_not" => {
                want(1)?;
                let x = self.to_b(&a[0])?;
                let t = self.fresh();
                self.line(format!("let {t}: bool = !{x};"));
                Ok(Val::B(t))
            }
            "list::value_to_int" => { want(1)?; Ok(Val::I(self.to_i(&a[0])?)) }
            "list::value_to_bool" => { want(1)?; Ok(Val::B(self.to_b(&a[0])?)) }
            "list::value_to_text" => { want(1)?; Ok(Val::T(self.to_t(&a[0])?)) }
            "tuple::ctor_field" => {
                want(2)?;
                let Val::R(r) = &a[0] else { return Err("ctor_field of a non-argument value".into()) };
                let k = self.to_i(&a[1])?;
                let t = self.fresh();
                self.line(format!(
                    "let {t}: &Value = match {r} {{ Value::Ctor {{ tag, fields }} if *tag != {RT}::fault::UNKNOWN_TAG && *tag != {RT}::fault::ERR_TAG => \
                     match usize::try_from({k}).ok().and_then(|k| fields.get(k)) {{ Some(x) => x, None => return None }}, _ => return None }};"
                ));
                Ok(Val::R(t))
            }
            "tuple::tuple_field" => {
                want(2)?;
                let Val::R(r) = &a[0] else { return Err("tuple_field of a non-argument value".into()) };
                let k = self.to_i(&a[1])?;
                let t = self.fresh();
                self.line(format!(
                    "let {t}: &Value = match {r} {{ Value::Tuple(f) => match usize::try_from({k}).ok().and_then(|k| f.get(k)) {{ Some(x) => x, None => return None }}, \
                     Value::List(f) => match usize::try_from({k}).ok().and_then(|k| f.get(k)) {{ Some(x) => x, None => return None }}, _ => return None }};"
                ));
                Ok(Val::R(t))
            }
            "tuple::value_0" | "tuple::value_1" | "tuple::value_2" => {
                want(1)?;
                if let Val::P(e) = &a[0] {
                    if op == "tuple::value_0" { return Ok(Val::T(e.clone())); }
                    return Err("value_N of an Option(Text) other than 0".into());
                }
                let Val::R(r) = &a[0] else { return Err("value_N of a non-argument value".into()) };
                let idx = &op[op.len() - 1..];
                let t = self.fresh();
                self.line(format!(
                    "let {t}: &Value = match {r} {{ Value::Ctor {{ tag, fields }} if *tag != {RT}::fault::UNKNOWN_TAG && *tag != {RT}::fault::ERR_TAG => \
                     match fields.get({idx}) {{ Some(x) => x, None => return None }}, \
                     Value::Tuple(f) => match f.get({idx}) {{ Some(x) => x, None => return None }}, _ => return None }};"
                ));
                Ok(Val::R(t))
            }
            "tuple::value_make" => {
                let mut parts = Vec::new();
                match a.len() {
                    0 => {}
                    1 => match &a[0] {
                        // a single field is the bare value: a Tuple/Unit there would be spliced, so only scalars and Text
                        Val::I(_) | Val::B(_) | Val::T(_) | Val::L(_) => parts.push(self.boxed(&a[0], false)?),
                        _ => return Err("value_make of one non-scalar".into()),
                    },
                    _ => {
                        for v in &a {
                            if let Val::R(e) = v {
                                // guard_pre would hand back a sticky argument instead of building
                                self.line(format!("if {RT}::fault::is_unknown({e}) || {RT}::fault::is_err({e}) {{ return None; }}"));
                            }
                            let mv = self.owns(v);
                            parts.push(self.boxed(v, mv)?);
                        }
                    }
                }
                let t = self.fresh();
                self.line(format!(
                    "let {t}: Value = Value::Ctor {{ tag: {RT}::tuple::value_tag(), fields: vec![{}] }};",
                    parts.join(", ")
                ));
                self.owned.last_mut().map(|s| s.insert(t.clone()));
                Ok(Val::O(t))
            }
            "str_ops::str_len" => {
                want(1)?;
                let s = self.to_t(&a[0])?;
                let t = self.fresh();
                self.line(format!("let {t}: i64 = {RT}::str_ops::fast_len({s});"));
                Ok(Val::I(t))
            }
            "str_ops::str_char" | "str_ops::str_char_at" => {
                want(2)?;
                let s = self.to_t(&a[0])?;
                let k = self.to_i(&a[1])?;
                let t = self.fresh();
                self.line(format!(
                    "let {t}: &std::sync::Arc<str> = match {RT}::str_ops::fast_char({s}, {k}) {{ Some(c) => c, None => return None }};"
                ));
                Ok(if op.ends_with("char") { Val::T(t) } else { Val::P(t) })
            }
            "str_ops::str_char_code" => {
                want(2)?;
                let s = self.to_t(&a[0])?;
                let k = self.to_i(&a[1])?;
                let t = self.fresh();
                self.line(format!("let {t}: i64 = match {RT}::str_ops::fast_char_code({s}, {k}) {{ Some(c) => c, None => return None }};"));
                Ok(Val::I(t))
            }
            "str_ops::str_eq" | "str_ops::text_eq" => {
                want(2)?;
                let (x, y) = (self.to_s(&a[0])?, self.to_s(&a[1])?);
                let t = self.fresh();
                self.line(format!("let {t}: bool = {x} == {y};"));
                Ok(Val::B(t))
            }
            other => Err(format!("{} has no fast form", other)),
        }
    }
}

fn param_count(b: &CoreBundle) -> u32 {
    let ph = param_type_hash();
    b.constant_pool
        .iter()
        .filter(|e| e.def_hash == ph)
        .filter_map(|e| crate::core_ir_05::decode_unsigned_varint(&e.payload).ok())
        .map(|s| s as u32 + 1)
        .max()
        .unwrap_or(0)
}

impl<'b> St<'b> {
    /// `params[slot]` is the value each Param pool entry stands for.
    fn new(b: &'b CoreBundle, params: &[Val]) -> Res<St<'b>> {
        let blocks = verify_blocks(b)?;
        let (ph, ih, bh, th, uh, fh) =
            (param_type_hash(), int_type_hash(), bool_type_hash(), text_type_hash(), unit_type_hash(), fn_type_hash());
        let mut pool = Vec::with_capacity(b.constant_pool.len());
        for e in &b.constant_pool {
            let dh = &e.def_hash;
            pool.push(if *dh == ph {
                let slot = crate::core_ir_05::decode_unsigned_varint(&e.payload)? as usize;
                Some(params.get(slot).cloned().ok_or("param slot out of range")?)
            } else if *dh == ih {
                Some(Val::I(format!("{}i64", decode_int_payload(&e.payload)?)))
            } else if *dh == bh {
                Some(Val::B(format!("{}", decode_bool_payload(&e.payload)?)))
            } else if *dh == th {
                Some(Val::L(format!("{:?}", decode_text_payload(&e.payload)?)))
            } else if *dh == uh {
                Some(Val::U)
            } else if *dh == fh {
                None
            } else {
                None
            });
        }
        Ok(St { b, blocks, pool, vals: vec![None; b.nodes.len()] })
    }
}

/// The text of `fn <safe_name>_fast(args: &Value) -> Option<Value>`, or None when this bundle has no fast form.
pub fn emit_fast(r: &Resolver, bundle: &CoreBundle, safe_name: &str) -> Option<String> {
    let pc = param_count(bundle);
    let mut head = String::new();
    let mut params: Vec<Val> = Vec::new();
    match pc {
        0 => {}
        1 => {
            head.push_str("    let a0: &Value = args;\n");
            params.push(Val::R("a0".into()));
        }
        n => {
            head.push_str(&format!("    let es = match args {{ Value::Tuple(es) if es.len() == {n} => es, _ => return None }};\n"));
            for i in 0..n {
                head.push_str(&format!("    let a{i}: &Value = &es[{i}];\n"));
                params.push(Val::R(format!("a{i}")));
            }
        }
    }
    let mut st = St::new(bundle, &params).ok()?;
    let mut g = Gen { r, out: String::new(), n: 0, arm_depth: 0, stack: Vec::new(), inlined_nodes: 0, owned: vec![HashSet::new()] };
    let dead = g.gen_range(&mut st, 0, bundle.nodes.len()).ok()?;
    if dead {
        return None;
    }
    let res = g.val(&st, &bundle.result).ok()?;
    let mv = g.owns(&res);
    let boxed = g.boxed(&res, mv).ok()?;
    let mut s = String::new();
    s.push_str("#[allow(unused_variables, unused_mut, unreachable_code, dead_code, clippy::all)]\n");
    s.push_str(&format!("fn {safe_name}_fast(args: &Value) -> Option<Value> {{\n"));
    s.push_str(&head);
    s.push_str(&g.out);
    s.push_str(&format!("    Some({boxed})\n}}\n\n"));
    Some(s)
}

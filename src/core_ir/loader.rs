use std::fs;
use std::io::BufReader;
use std::rc::Rc;
use capnp::message::ReaderOptions;
use capnp::serialize;
use super::{CoreTerm, Provenance, EffectClass};

pub struct CoreProgram {
    pub root_term: CoreTerm,
    pub entrypoint_id: usize,
    pub provenance: Provenance,
    pub effect_class: EffectClass,
    pub idempotent: bool,
}

pub fn load_core_bundle(path: &str) -> Result<CoreProgram, String> {
    let file = fs::File::open(path)
        .map_err(|e| format!("failed to open {}: {}", path, e))?;
    let mut reader = BufReader::new(file);
    load_from_reader(&mut reader)
}

pub fn load_core_bundle_from_bytes(bytes: &[u8]) -> Result<CoreProgram, String> {
    load_from_reader(&mut &bytes[..])
}

fn load_from_reader<R: std::io::Read>(r: &mut R) -> Result<CoreProgram, String> {
    let mut opts = ReaderOptions::new();
    opts.traversal_limit_in_words = Some(1 << 30);
    opts.nesting_limit = 1_000_000;

    let msg = serialize::read_message(r, opts)
        .map_err(|e| format!("Cap'n Proto read failed: {}", e))?;

    let bundle = msg
        .get_root::<crate::axis_core_ir_0_4_capnp::core_bundle::Reader>()
        .map_err(|e| format!("get_root failed: {}", e))?;

    let version = bundle.get_version()
        .map_err(|e| format!("get_version failed: {}", e))?
        .to_str()
        .map_err(|e| format!("version utf8: {}", e))?;

    let (provenance, effect_class, idempotent) = match version {
        "0.3" => (Provenance::Mechanical, EffectClass::Pure, true),
        "0.4" => {
            let prov = match bundle.get_provenance() {
                Ok(crate::axis_core_ir_0_4_capnp::Provenance::Mechanical)   => Provenance::Mechanical,
                Ok(crate::axis_core_ir_0_4_capnp::Provenance::LlmCandidate) => Provenance::LlmCandidate,
                Ok(crate::axis_core_ir_0_4_capnp::Provenance::BulkCorpus)   => Provenance::BulkCorpus,
                Err(_) => Provenance::Mechanical,
            };
            let ec = match bundle.get_effect_class() {
                Ok(crate::axis_core_ir_0_4_capnp::EffectClass::Pure)   => EffectClass::Pure,
                Ok(crate::axis_core_ir_0_4_capnp::EffectClass::Reads)  => EffectClass::Reads,
                Ok(crate::axis_core_ir_0_4_capnp::EffectClass::Writes) => EffectClass::Writes,
                Ok(crate::axis_core_ir_0_4_capnp::EffectClass::FullIo) => EffectClass::FullIo,
                Err(_) => EffectClass::Pure,
            };
            let idem = bundle.get_idempotent();
            (prov, ec, idem)
        }
        other => return Err(format!("unsupported Core IR version: {}", other)),
    };

    let entrypoint_id = bundle.get_entrypoint_id() as usize;
    let term_reader   = bundle.get_core_term()
        .map_err(|e| format!("get_core_term failed: {}", e))?;

    let root_term = deserialise(term_reader)?;
    Ok(CoreProgram { root_term, entrypoint_id, provenance, effect_class, idempotent })
}

// ── Iterative deserialiser ───────────────────────────────────────────────────

enum Frame<'a> {
    Leaf(CoreTerm),
    Lam  { param: String, body: crate::axis_core_ir_0_4_capnp::core_term::Reader<'a>, done: bool },
    App  { fn_r: crate::axis_core_ir_0_4_capnp::core_term::Reader<'a>,
           arg_r: crate::axis_core_ir_0_4_capnp::core_term::Reader<'a>,
           fn_done: bool, arg_done: bool },
    Let  { name: String,
           val_r: crate::axis_core_ir_0_4_capnp::core_term::Reader<'a>,
           body_r: crate::axis_core_ir_0_4_capnp::core_term::Reader<'a>,
           val_done: bool, body_done: bool },
    If   { cond_r: crate::axis_core_ir_0_4_capnp::core_term::Reader<'a>,
           then_r: crate::axis_core_ir_0_4_capnp::core_term::Reader<'a>,
           else_r: crate::axis_core_ir_0_4_capnp::core_term::Reader<'a>,
           cond_done: bool, then_done: bool, else_done: bool },
    Call { target: String,
           readers: Vec<crate::axis_core_ir_0_4_capnp::core_term::Reader<'a>>,
           children: Vec<CoreTerm>, next: usize },
}

fn to_frame<'a>(r: crate::axis_core_ir_0_4_capnp::core_term::Reader<'a>) -> Result<Frame<'a>, String> {
    use crate::axis_core_ir_0_4_capnp::core_term::Which;
    match r.which() {
        Ok(Which::CIntLit(l))  => Ok(Frame::Leaf(CoreTerm::IntLit(l.map_err(|e| e.to_string())?.get_value(), None))),
        Ok(Which::CBoolLit(l)) => Ok(Frame::Leaf(CoreTerm::BoolLit(l.map_err(|e| e.to_string())?.get_value(), None))),
        Ok(Which::CUnitLit(_)) => Ok(Frame::Leaf(CoreTerm::UnitLit(None))),
        Ok(Which::CVar(v))     => {
            let v = v.map_err(|e| e.to_string())?;
            Ok(Frame::Leaf(CoreTerm::Var(v.get_name().map_err(|e| e.to_string())?.to_str().map_err(|e| e.to_string())?.to_string(), None)))
        }
        Ok(Which::CLam(l)) => {
            let l = l.map_err(|e| e.to_string())?;
            let param = l.get_param().map_err(|e| e.to_string())?.to_str().map_err(|e| e.to_string())?.to_string();
            let body  = l.get_body().map_err(|e| e.to_string())?;
            Ok(Frame::Lam { param, body, done: false })
        }
        Ok(Which::CApp(a)) => {
            let a = a.map_err(|e| e.to_string())?;
            Ok(Frame::App { fn_r: a.get_fn().map_err(|e| e.to_string())?, arg_r: a.get_arg().map_err(|e| e.to_string())?, fn_done: false, arg_done: false })
        }
        Ok(Which::CLet(l)) => {
            let l = l.map_err(|e| e.to_string())?;
            let name = l.get_name().map_err(|e| e.to_string())?.to_str().map_err(|e| e.to_string())?.to_string();
            Ok(Frame::Let { name, val_r: l.get_value().map_err(|e| e.to_string())?, body_r: l.get_body().map_err(|e| e.to_string())?, val_done: false, body_done: false })
        }
        Ok(Which::CIf(i)) => {
            let i = i.map_err(|e| e.to_string())?;
            Ok(Frame::If { cond_r: i.get_cond().map_err(|e| e.to_string())?, then_r: i.get_then().map_err(|e| e.to_string())?, else_r: i.get_else().map_err(|e| e.to_string())?, cond_done: false, then_done: false, else_done: false })
        }
        Ok(Which::CCall(c)) => {
            let c = c.map_err(|e| e.to_string())?;
            let target = c.get_target_name().map_err(|e| e.to_string())?.to_str().map_err(|e| e.to_string())?.to_string();
            let args_r = c.get_args().map_err(|e| e.to_string())?;
            let readers: Vec<_> = (0..args_r.len()).map(|i| args_r.get(i)).collect();
            Ok(Frame::Call { target, readers, children: Vec::new(), next: 0 })
        }
        Err(e) => Err(format!("unknown CoreTerm variant: {:?}", e)),
    }
}

fn deserialise(root: crate::axis_core_ir_0_4_capnp::core_term::Reader) -> Result<CoreTerm, String> {
    let mut work: Vec<Frame>    = vec![to_frame(root)?];
    let mut results: Vec<CoreTerm> = Vec::new();

    while let Some(frame) = work.pop() {
        match frame {
            Frame::Leaf(t) => results.push(t),

            Frame::Lam { param, body, done } => {
                if !done {
                    work.push(Frame::Lam { param, body, done: true });
                    work.push(to_frame(body)?);
                } else {
                    let b = results.pop().ok_or("Lam: stack underflow")?;
                    results.push(CoreTerm::Lam(param, Rc::new(b), None));
                }
            }

            Frame::App { fn_r, arg_r, fn_done, arg_done } => {
                if !fn_done {
                    work.push(Frame::App { fn_r, arg_r, fn_done: true, arg_done: false });
                    work.push(to_frame(fn_r)?);
                } else if !arg_done {
                    work.push(Frame::App { fn_r, arg_r, fn_done: true, arg_done: true });
                    work.push(to_frame(arg_r)?);
                } else {
                    let arg  = results.pop().ok_or("App: arg underflow")?;
                    let func = results.pop().ok_or("App: fn underflow")?;
                    results.push(CoreTerm::App(Rc::new(func), Rc::new(arg), None));
                }
            }

            Frame::Let { name, val_r, body_r, val_done, body_done } => {
                if !val_done {
                    work.push(Frame::Let { name, val_r, body_r, val_done: true, body_done: false });
                    work.push(to_frame(val_r)?);
                } else if !body_done {
                    work.push(Frame::Let { name, val_r, body_r, val_done: true, body_done: true });
                    work.push(to_frame(body_r)?);
                } else {
                    let body = results.pop().ok_or("Let: body underflow")?;
                    let val  = results.pop().ok_or("Let: val underflow")?;
                    results.push(CoreTerm::Let(name, Rc::new(val), Rc::new(body), None));
                }
            }

            Frame::If { cond_r, then_r, else_r, cond_done, then_done, else_done } => {
                if !cond_done {
                    work.push(Frame::If { cond_r, then_r, else_r, cond_done: true, then_done: false, else_done: false });
                    work.push(to_frame(cond_r)?);
                } else if !then_done {
                    work.push(Frame::If { cond_r, then_r, else_r, cond_done: true, then_done: true, else_done: false });
                    work.push(to_frame(then_r)?);
                } else if !else_done {
                    work.push(Frame::If { cond_r, then_r, else_r, cond_done: true, then_done: true, else_done: true });
                    work.push(to_frame(else_r)?);
                } else {
                    let else_b = results.pop().ok_or("If: else underflow")?;
                    let then_b = results.pop().ok_or("If: then underflow")?;
                    let cond   = results.pop().ok_or("If: cond underflow")?;
                    results.push(CoreTerm::If(Rc::new(cond), Rc::new(then_b), Rc::new(else_b), None));
                }
            }

            Frame::Call { target, readers, mut children, next } => {
                if next < readers.len() {
                    work.push(Frame::Call { target, readers: readers.clone(), children, next: next + 1 });
                    work.push(to_frame(readers[next])?);
                } else {
                    let count = readers.len();
                    for _ in 0..count { children.push(results.pop().ok_or("Call: arg underflow")?); }
                    children.reverse();
                    results.push(CoreTerm::Call(target, children, None));
                }
            }
        }
    }

    results.pop().ok_or_else(|| "deserialise: empty result stack".to_string())
}

use std::fs;
use capnp::message::Builder;
use capnp::serialize;
use super::{CoreTerm, Provenance, EffectClass};

pub fn create_core_bundle(
    term: &CoreTerm,
    entrypoint: &str,
    provenance: Provenance,
    effect_class: EffectClass,
    idempotent: bool,
) -> Vec<u8> {
    let mut msg = Builder::new_default();
    {
        let mut bundle = msg.init_root::<crate::axis_core_ir_0_4_capnp::core_bundle::Builder>();
        bundle.set_version("0.4");
        bundle.set_entrypoint_name(entrypoint);
        bundle.set_entrypoint_id(0);
        bundle.set_provenance(provenance_to_capnp(provenance));
        bundle.set_effect_class(effect_class_to_capnp(effect_class));
        bundle.set_idempotent(idempotent);
        let term_b = bundle.init_core_term();
        serialise_term(term, term_b);
    }
    let mut buf = Vec::new();
    serialize::write_message(&mut buf, &msg).unwrap();
    buf
}

pub fn write_core_bundle_to_file(
    term: &CoreTerm,
    entrypoint: &str,
    provenance: Provenance,
    effect_class: EffectClass,
    idempotent: bool,
    path: &str,
) -> Result<(), String> {
    let bytes = create_core_bundle(term, entrypoint, provenance, effect_class, idempotent);
    fs::write(path, bytes).map_err(|e| format!("write failed: {}", e))
}

/// Write a multi-export bundle. Each entry is (name, term, effect_sig).
/// effect_sig: "pure" | "reads" | "writes" | "full_io"
pub fn create_core_bundle_multi(
    exports: &[(&str, &CoreTerm, &str)],
    provenance: Provenance,
    idempotent: bool,
) -> Vec<u8> {
    let mut msg = Builder::new_default();
    {
        let mut bundle = msg.init_root::<crate::axis_core_ir_0_4_capnp::core_bundle::Builder>();
        bundle.set_version("0.4");
        bundle.set_idempotent(idempotent);
        bundle.set_provenance(provenance_to_capnp(provenance));
        // effect_class on the bundle: use the most permissive declared across exports
        let overall_ec = exports.iter().fold(crate::axis_core_ir_0_4_capnp::EffectClass::Pure, |acc, (_, _, sig)| {
            let ec = match *sig {
                "reads"   => crate::axis_core_ir_0_4_capnp::EffectClass::Reads,
                "writes"  => crate::axis_core_ir_0_4_capnp::EffectClass::Writes,
                "full_io" => crate::axis_core_ir_0_4_capnp::EffectClass::FullIo,
                _         => crate::axis_core_ir_0_4_capnp::EffectClass::Pure,
            };
            if (ec as u16) > (acc as u16) { ec } else { acc }
        });
        bundle.set_effect_class(overall_ec);

        // Set entrypointName to first export for backward compat readers
        if let Some((first_name, first_term, _)) = exports.first() {
            bundle.set_entrypoint_name(first_name);
            let term_b = bundle.reborrow().init_core_term();
            serialise_term(first_term, term_b);
        }

        // Write exports list
        let mut exp_list = bundle.reborrow().init_exports(exports.len() as u32);
        for (i, (name, term, effect_sig)) in exports.iter().enumerate() {
            let mut exp_b = exp_list.reborrow().get(i as u32);
            exp_b.set_name(name);
            exp_b.set_effect_sig(effect_sig);
            let term_b = exp_b.init_term();
            serialise_term(term, term_b);
        }
    }
    let mut buf = Vec::new();
    serialize::write_message(&mut buf, &msg).unwrap();
    buf
}

pub fn write_core_bundle_multi_to_file(
    exports: &[(&str, &CoreTerm, &str)],
    provenance: Provenance,
    idempotent: bool,
    path: &str,
) -> Result<(), String> {
    let bytes = create_core_bundle_multi(exports, provenance, idempotent);
    fs::write(path, bytes).map_err(|e| format!("write failed: {}", e))
}

fn provenance_to_capnp(p: Provenance) -> crate::axis_core_ir_0_4_capnp::Provenance {
    match p {
        Provenance::Mechanical   => crate::axis_core_ir_0_4_capnp::Provenance::Mechanical,
        Provenance::LlmCandidate => crate::axis_core_ir_0_4_capnp::Provenance::LlmCandidate,
        Provenance::BulkCorpus   => crate::axis_core_ir_0_4_capnp::Provenance::BulkCorpus,
    }
}

fn effect_class_to_capnp(ec: EffectClass) -> crate::axis_core_ir_0_4_capnp::EffectClass {
    match ec {
        EffectClass::Pure   => crate::axis_core_ir_0_4_capnp::EffectClass::Pure,
        EffectClass::Reads  => crate::axis_core_ir_0_4_capnp::EffectClass::Reads,
        EffectClass::Writes => crate::axis_core_ir_0_4_capnp::EffectClass::Writes,
        EffectClass::FullIo => crate::axis_core_ir_0_4_capnp::EffectClass::FullIo,
    }
}

fn serialise_term(term: &CoreTerm, b: crate::axis_core_ir_0_4_capnp::core_term::Builder) {
    match term {
        CoreTerm::IntLit(n, _)  => { b.init_c_int_lit().set_value(*n); }
        CoreTerm::BoolLit(v, _) => { b.init_c_bool_lit().set_value(*v); }
        CoreTerm::UnitLit(_)    => { b.init_c_unit_lit(); }
        CoreTerm::Var(n, _)     => { b.init_c_var().set_name(n); }
        CoreTerm::Lam(p, body, _) => {
            let mut l = b.init_c_lam();
            l.set_param(p);
            serialise_term(body, l.init_body());
        }
        CoreTerm::App(f, a, _) => {
            let mut app = b.init_c_app();
            serialise_term(f, app.reborrow().init_fn());
            serialise_term(a, app.init_arg());
        }
        CoreTerm::Let(n, v, body, _) => {
            let mut l = b.init_c_let();
            l.set_name(n);
            serialise_term(v, l.reborrow().init_value());
            serialise_term(body, l.init_body());
        }
        CoreTerm::If(cond, then, els, _) => {
            let mut i = b.init_c_if();
            serialise_term(cond, i.reborrow().init_cond());
            serialise_term(then, i.reborrow().init_then());
            serialise_term(els,  i.init_else());
        }
        CoreTerm::Call(target, args, _) => {
            let mut c = b.init_c_call();
            c.set_target_name(target);
            let mut ab = c.init_args(args.len() as u32);
            for (i, a) in args.iter().enumerate() {
                serialise_term(a, ab.reborrow().get(i as u32));
            }
        }
    }
}

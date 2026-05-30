use std::fs;
use capnp::message::Builder;
use capnp::serialize;
use super::CoreTerm;

pub fn create_core_bundle(term: &CoreTerm, entrypoint: &str) -> Vec<u8> {
    let mut msg = Builder::new_default();
    {
        let mut bundle = msg.init_root::<crate::axis_core_ir_0_3_capnp::core_bundle::Builder>();
        bundle.set_version("0.3");
        bundle.set_entrypoint_name(entrypoint);
        bundle.set_entrypoint_id(0);
        let term_b = bundle.init_core_term();
        serialise_term(term, term_b);
    }
    let mut buf = Vec::new();
    serialize::write_message(&mut buf, &msg).unwrap();
    buf
}

pub fn write_core_bundle_to_file(term: &CoreTerm, entrypoint: &str, path: &str) -> Result<(), String> {
    let bytes = create_core_bundle(term, entrypoint);
    fs::write(path, bytes).map_err(|e| format!("write failed: {}", e))
}

fn serialise_term(term: &CoreTerm, b: crate::axis_core_ir_0_3_capnp::core_term::Builder) {
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

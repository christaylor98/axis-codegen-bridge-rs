use capnp::message::Builder;
use capnp::serialize;
use std::fs;

use super::{ConstantPoolEntry, CoreBundle, Hash256, Node, NodeRef};
use crate::axis_core_ir_0_5_capnp as capnp05;

fn set_hash256(mut b: capnp05::hash256::Builder<'_>, h: &Hash256) {
    b.set_w0(u64::from_be_bytes(h[0..8].try_into().unwrap()));
    b.set_w1(u64::from_be_bytes(h[8..16].try_into().unwrap()));
    b.set_w2(u64::from_be_bytes(h[16..24].try_into().unwrap()));
    b.set_w3(u64::from_be_bytes(h[24..32].try_into().unwrap()));
}

fn set_noderef(ab: capnp05::node_ref::Builder<'_>, nr: &NodeRef) {
    match nr {
        NodeRef::Node(i) => {
            let mut ab = ab;
            ab.set_node(*i);
        }
        NodeRef::Pool(i) => {
            let mut ab = ab;
            ab.set_pool(*i);
        }
    }
}

pub fn create_core_bundle_05(bundle: &CoreBundle) -> Vec<u8> {
    let mut msg = Builder::new_default();
    {
        let mut root = msg.init_root::<capnp05::core_bundle::Builder>();
        root.set_version(bundle.version.as_str());

        {
            let mut pool_b =
                root.reborrow().init_constant_pool(bundle.constant_pool.len() as u32);
            for (i, entry) in bundle.constant_pool.iter().enumerate() {
                let mut eb = pool_b.reborrow().get(i as u32);
                {
                    let hb = eb.reborrow().init_def_hash();
                    set_hash256(hb, &entry.def_hash);
                }
                eb.set_payload(entry.payload.as_slice().into());
            }
        }

        {
            let mut nodes_b = root.reborrow().init_nodes(bundle.nodes.len() as u32);
            for (i, node) in bundle.nodes.iter().enumerate() {
                let node_b = nodes_b.reborrow().get(i as u32);
                match node {
                    Node::CCall { target_identity, args } => {
                        let mut ccall = node_b.init_c_call();
                        {
                            let hb = ccall.reborrow().init_target_identity();
                            set_hash256(hb, target_identity);
                        }
                        let mut args_b = ccall.reborrow().init_args(args.len() as u32);
                        for (j, arg) in args.iter().enumerate() {
                            let ab = args_b.reborrow().get(j as u32);
                            set_noderef(ab, arg);
                        }
                    }
                    Node::CIf { cond, then_, else_ } => {
                        let mut cif = node_b.init_c_if();
                        {
                            let cb = cif.reborrow().init_cond();
                            set_noderef(cb, cond);
                        }
                        {
                            let tb = cif.reborrow().init_then();
                            set_noderef(tb, then_);
                        }
                        {
                            let eb = cif.reborrow().init_else();
                            set_noderef(eb, else_);
                        }
                    }
                }
            }
        }
    }
    let mut buf = Vec::new();
    serialize::write_message(&mut buf, &msg).unwrap();
    buf
}

pub fn write_core_bundle_05_to_file(bundle: &CoreBundle, path: &str) -> Result<(), String> {
    let bytes = create_core_bundle_05(bundle);
    fs::write(path, bytes).map_err(|e| format!("write failed: {}", e))
}

// ── Convenience builders for test fixtures ───────────────────────────────────

pub fn make_unit_bundle() -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![ConstantPoolEntry {
            def_hash: super::unit_type_hash(),
            payload: vec![],
        }],
        nodes: vec![],
    }
}

pub fn make_bool_bundle(v: bool) -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![ConstantPoolEntry {
            def_hash: super::bool_type_hash(),
            payload: super::encode_bool_payload(v),
        }],
        nodes: vec![],
    }
}

pub fn make_int_bundle(v: i64) -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: vec![ConstantPoolEntry {
            def_hash: super::int_type_hash(),
            payload: super::encode_int_payload(v),
        }],
        nodes: vec![],
    }
}

pub fn make_ccall_bundle(
    target_identity: [u8; 32],
    pool: Vec<ConstantPoolEntry>,
    args: Vec<NodeRef>,
) -> CoreBundle {
    CoreBundle {
        version: "0.5".to_string(),
        constant_pool: pool,
        nodes: vec![Node::CCall { target_identity, args }],
    }
}

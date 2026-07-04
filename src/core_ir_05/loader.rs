use capnp::message::ReaderOptions;
use capnp::serialize;
use std::fs;
use std::io::BufReader;

use super::{ConstantPoolEntry, CoreBundle, Hash256, Node, NodeRef};
use crate::axis_core_ir_0_5_capnp as capnp05;

fn read_hash256(r: capnp05::hash256::Reader<'_>) -> Hash256 {
    let mut out = [0u8; 32];
    out[0..8].copy_from_slice(&r.get_w0().to_be_bytes());
    out[8..16].copy_from_slice(&r.get_w1().to_be_bytes());
    out[16..24].copy_from_slice(&r.get_w2().to_be_bytes());
    out[24..32].copy_from_slice(&r.get_w3().to_be_bytes());
    out
}

fn read_node_ref(r: capnp05::node_ref::Reader<'_>) -> Result<NodeRef, String> {
    match r.which().map_err(|e| e.to_string())? {
        capnp05::node_ref::Node(i) => Ok(NodeRef::Node(i)),
        capnp05::node_ref::Pool(i) => Ok(NodeRef::Pool(i)),
    }
}

pub fn load_core_bundle(path: &str) -> Result<CoreBundle, String> {
    let file = fs::File::open(path)
        .map_err(|e| format!("failed to open {}: {}", path, e))?;
    let mut reader = BufReader::new(file);
    load_from_reader(&mut reader)
}

pub fn load_core_bundle_from_bytes(bytes: &[u8]) -> Result<CoreBundle, String> {
    load_from_reader(&mut &bytes[..])
}

fn load_from_reader<R: std::io::Read>(r: &mut R) -> Result<CoreBundle, String> {
    let mut opts = ReaderOptions::new();
    opts.traversal_limit_in_words = Some(1 << 30);
    opts.nesting_limit = 1_000_000;

    let msg = serialize::read_message(r, opts)
        .map_err(|e| format!("Cap'n Proto read failed: {}", e))?;

    let bundle = msg
        .get_root::<capnp05::core_bundle::Reader>()
        .map_err(|e| format!("get_root failed: {}", e))?;

    let version = bundle
        .get_version()
        .map_err(|e| format!("get_version failed: {}", e))?
        .to_str()
        .map_err(|e| format!("version utf8: {}", e))?
        .to_string();

    if version != "0.5" {
        return Err(format!(
            "unsupported Core IR version: {} (this loader requires 0.5)",
            version
        ));
    }

    let pool_r = bundle
        .get_constant_pool()
        .map_err(|e| format!("get_constant_pool failed: {}", e))?;
    let mut constant_pool = Vec::with_capacity(pool_r.len() as usize);
    for i in 0..pool_r.len() {
        let entry = pool_r.get(i);
        let def_hash = read_hash256(
            entry
                .get_def_hash()
                .map_err(|e| format!("pool[{}].get_def_hash: {}", i, e))?,
        );
        let payload = entry
            .get_payload()
            .map_err(|e| format!("pool[{}].get_payload: {}", i, e))?
            .to_vec();
        constant_pool.push(ConstantPoolEntry { def_hash, payload });
    }

    let nodes_r = bundle
        .get_nodes()
        .map_err(|e| format!("get_nodes failed: {}", e))?;
    let mut nodes = Vec::with_capacity(nodes_r.len() as usize);
    for i in 0..nodes_r.len() {
        let node_r = nodes_r.get(i);
        let node = match node_r.which().map_err(|e| format!("node[{}] which: {}", i, e))? {
            capnp05::node::CCall(r) => {
                let cc = r.map_err(|e| format!("node[{}] CCall: {}", i, e))?;
                let target_identity = read_hash256(
                    cc.get_target_identity()
                        .map_err(|e| format!("node[{}] target_identity: {}", i, e))?,
                );
                let args_r = cc
                    .get_args()
                    .map_err(|e| format!("node[{}] get_args: {}", i, e))?;
                let mut args = Vec::with_capacity(args_r.len() as usize);
                for j in 0..args_r.len() {
                    args.push(
                        read_node_ref(args_r.get(j))
                            .map_err(|e| format!("node[{}].args[{}]: {}", i, j, e))?,
                    );
                }
                let target_name = cc
                    .get_target_name()
                    .map_err(|e| format!("node[{}] get_target_name: {}", i, e))?
                    .to_str()
                    .map_err(|e| format!("node[{}] target_name utf8: {}", i, e))?
                    .to_string();
                Node::CCall { target_identity, args, target_name }
            }
            capnp05::node::CDeterminate(r) => {
                // No fields; validate the pointer then record the marker.
                r.map_err(|e| format!("node[{}] CDeterminate: {}", i, e))?;
                Node::CDeterminate
            }
            capnp05::node::CIf(r) => {
                let ci = r.map_err(|e| format!("node[{}] CIf: {}", i, e))?;
                let cond = read_node_ref(
                    ci.get_cond()
                        .map_err(|e| format!("node[{}] cond: {}", i, e))?,
                )
                .map_err(|e| format!("node[{}] cond ref: {}", i, e))?;
                let then_ = read_node_ref(
                    ci.get_then()
                        .map_err(|e| format!("node[{}] then: {}", i, e))?,
                )
                .map_err(|e| format!("node[{}] then ref: {}", i, e))?;
                let else_ = read_node_ref(
                    ci.get_else()
                        .map_err(|e| format!("node[{}] else: {}", i, e))?,
                )
                .map_err(|e| format!("node[{}] else ref: {}", i, e))?;
                Node::CIf { cond, then_, else_ }
            }
        };
        nodes.push(node);
    }

    let result = read_node_ref(
        bundle.get_result().map_err(|e| format!("get_result failed: {}", e))?,
    )
    .map_err(|e| format!("result ref: {}", e))?;

    Ok(CoreBundle { version, constant_pool, nodes, result })
}

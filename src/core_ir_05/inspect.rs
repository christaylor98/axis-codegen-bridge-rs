use super::{hash256_to_hex, CoreBundle, Node, NodeRef, NO_RESULT_TYPE};

pub fn inspect_core_bundle(path: &str) -> Result<String, String> {
    let bundle = super::load_core_bundle(path)?;
    Ok(format_bundle(path, &bundle))
}

fn format_bundle(path: &str, bundle: &CoreBundle) -> String {
    let mut out = String::new();
    out.push_str(&format!("Core IR bundle: {}\n", path));
    out.push_str(&format!("  version:       {}\n", bundle.version));
    out.push_str(&format!("  constant_pool: {} entries\n", bundle.constant_pool.len()));
    for (i, entry) in bundle.constant_pool.iter().enumerate() {
        out.push_str(&format!(
            "    pool[{}]: def_hash={}… payload={} bytes\n",
            i,
            &hash256_to_hex(&entry.def_hash)[..16],
            entry.payload.len()
        ));
    }
    out.push_str(&format!("  nodes:         {}\n", bundle.nodes.len()));
    for (i, node) in bundle.nodes.iter().enumerate() {
        let desc = match node {
            Node::CCall { target_identity, args, result_type } => {
                let arg_list: Vec<String> = args.iter().map(format_ref).collect();
                if result_type != &NO_RESULT_TYPE {
                    format!(
                        "CCall(identity={}…, args=[{}], result_type={}…)",
                        &hash256_to_hex(target_identity)[..16],
                        arg_list.join(", "),
                        &hash256_to_hex(result_type)[..16],
                    )
                } else {
                    format!(
                        "CCall(identity={}…, args=[{}])",
                        &hash256_to_hex(target_identity)[..16],
                        arg_list.join(", ")
                    )
                }
            }
            Node::CIf { cond, then_, else_ } => {
                format!(
                    "CIf(cond={}, then={}, else={})",
                    format_ref(cond),
                    format_ref(then_),
                    format_ref(else_)
                )
            }
        };
        out.push_str(&format!("    node[{}]: {}\n", i, desc));
    }
    out
}

fn format_ref(r: &NodeRef) -> String {
    match r {
        NodeRef::Node(i) => format!("node[{}]", i),
        NodeRef::Pool(i) => format!("pool[{}]", i),
    }
}

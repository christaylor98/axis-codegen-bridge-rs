//! Lower a 0.4 `CoreTerm` AST into a flat 0.5 `CoreBundle` (constant_pool +
//! CCall/CIf nodes). This is the term -> 0.5 lowering the bridge previously
//! lacked: it lets the mechanical generator's Core IR term be written as
//! gate-acceptable 0.5 directly, without routing through the compiler.
//!
//! Supported term shape (the mechanical leaf surface):
//!   - Let-chains              (let-bound names resolve to a NodeRef)
//!   - curried App spines      (head must be Var = a bridge fn name)
//!   - Call(target, args)      (explicit call form)
//!   - Var                     (must be let-bound)
//!   - IntLit / BoolLit / UnitLit -> constant_pool entries
//!   - If                      -> CIf node
//! Lam, free variables, and higher-order App heads are rejected: they require
//! lambda-lifting and are not part of the mechanical leaf surface.

use std::collections::HashMap;

use crate::core_ir::CoreTerm;
use super::{
    bool_type_hash, encode_bool_payload, encode_int_payload, int_type_hash,
    sha256_bytes, unit_type_hash,
    ConstantPoolEntry, CoreBundle, Hash256, Node, NodeRef,
};

struct Lowering {
    pool: Vec<ConstantPoolEntry>,
    nodes: Vec<Node>,
    env: HashMap<String, NodeRef>,
}

impl Lowering {
    fn push_pool(&mut self, def_hash: Hash256, payload: Vec<u8>) -> NodeRef {
        let idx = self.pool.len() as u32;
        self.pool.push(ConstantPoolEntry { def_hash, payload });
        NodeRef::Pool(idx)
    }

    fn push_node(&mut self, node: Node) -> NodeRef {
        let idx = self.nodes.len() as u32;
        self.nodes.push(node);
        NodeRef::Node(idx)
    }

    fn lower(&mut self, t: &CoreTerm) -> Result<NodeRef, String> {
        match t {
            CoreTerm::IntLit(n, _) => Ok(self.push_pool(int_type_hash(), encode_int_payload(*n))),
            CoreTerm::BoolLit(b, _) => Ok(self.push_pool(bool_type_hash(), encode_bool_payload(*b))),
            CoreTerm::UnitLit(_) => Ok(self.push_pool(unit_type_hash(), vec![])),
            CoreTerm::Var(name, _) => self
                .env
                .get(name)
                .cloned()
                .ok_or_else(|| format!("lower05: unbound variable '{}'", name)),
            CoreTerm::Let(name, val, body, _) => {
                let r = self.lower(val)?;
                self.env.insert(name.clone(), r);
                self.lower(body)
            }
            CoreTerm::Call(target, args, _) => {
                let mut arg_refs = Vec::with_capacity(args.len());
                for a in args {
                    arg_refs.push(self.lower(a)?);
                }
                Ok(self.push_node(Node::CCall {
                    target_identity: sha256_bytes(target.as_bytes()),
                    args: arg_refs,
                    target_name: target.clone(),
                }))
            }
            CoreTerm::App(..) => {
                let (fn_name, args) = flatten_app(t)?;
                let mut arg_refs = Vec::with_capacity(args.len());
                for a in &args {
                    arg_refs.push(self.lower(a)?);
                }
                Ok(self.push_node(Node::CCall {
                    target_identity: sha256_bytes(fn_name.as_bytes()),
                    args: arg_refs,
                    target_name: fn_name.clone(),
                }))
            }
            CoreTerm::If(cond, then_, else_, _) => {
                let c = self.lower(cond)?;
                let t2 = self.lower(then_)?;
                let e = self.lower(else_)?;
                Ok(self.push_node(Node::CIf { cond: c, then_: t2, else_: e }))
            }
            CoreTerm::Lam(..) => {
                Err("lower05: Lam not supported (lambda-lifting required)".to_string())
            }
        }
    }
}

/// Flatten a curried App spine: `App(App(Var(f), a), b)` -> `(f, [a, b])`.
fn flatten_app(t: &CoreTerm) -> Result<(String, Vec<&CoreTerm>), String> {
    let mut args: Vec<&CoreTerm> = Vec::new();
    let mut cur = t;
    loop {
        match cur {
            CoreTerm::App(f, a, _) => {
                args.push(a.as_ref());
                cur = f.as_ref();
            }
            CoreTerm::Var(name, _) => {
                args.reverse();
                return Ok((name.clone(), args));
            }
            other => {
                return Err(format!(
                    "lower05: App head must be a Var (bridge fn name), got {:?}",
                    other
                ))
            }
        }
    }
}

/// Lower a root `CoreTerm` to a 0.5 `CoreBundle`. The result of the program is
/// the last node (matching the compiler's lowering convention).
pub fn lower_core_term_to_bundle_05(root: &CoreTerm) -> Result<CoreBundle, String> {
    let mut low = Lowering {
        pool: Vec::new(),
        nodes: Vec::new(),
        env: HashMap::new(),
    };
    low.lower(root)?;
    Ok(CoreBundle {
        version: "0.5".to_string(),
        constant_pool: low.pool,
        nodes: low.nodes,
    })
}

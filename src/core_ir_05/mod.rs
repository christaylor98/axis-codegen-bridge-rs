use sha2::{Digest, Sha256};

pub mod loader;
pub mod inspect;
pub mod serialiser;

pub use loader::{load_core_bundle, load_core_bundle_from_bytes};
pub use inspect::inspect_core_bundle;
pub use serialiser::{create_core_bundle_05, write_core_bundle_05_to_file};

/// A 256-bit hash or identity token: four big-endian u64s packed into 32 bytes.
pub type Hash256 = [u8; 32];

#[derive(Clone, Debug, PartialEq)]
pub enum NodeRef {
    Node(u32),
    Pool(u32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConstantPoolEntry {
    pub def_hash: Hash256,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    CCall {
        target_identity: Hash256,
        args: Vec<NodeRef>,
        /// Human-readable fn name. Mandatory in Core IR 0.5; must match the
        /// registry entry for `target_identity`. Tools and humans key on this;
        /// the bridge, Verifier, and compiler key on `target_identity`.
        target_name: String,
    },
    CIf {
        cond: NodeRef,
        then_: NodeRef,
        else_: NodeRef,
    },
    /// Determinacy discharge gate (schema ordinal @4). A pure marker with no
    /// operands that produces a Unit discharge token; used as a domination
    /// gate for irreversibility checking. No lowering emits this yet.
    CDeterminate,
}

/// ORPHAN_IS_TOP_LEVEL_V1 — scope is carried by data-dependency edges and by
/// nothing else.
///
/// A node with NO consumer edge (not `result`, not an argument of any `CCall`,
/// not the cond/then_/else_ of any `CIf`) is **unconditional and top-level**:
/// it is evaluated once, on every call, in node-index order relative to the
/// other top-level nodes. A node's index position relative to a `CIf` says
/// nothing about branch membership.
///
/// This is a PRODUCER CONTRACT, not merely emitter behaviour. A producer that
/// wants a discarded side effect to be gated by a branch — `if c { let _ =
/// eff(); v }` — MUST give it a consumer edge inside that arm, conventionally
/// by threading it through the arm's result with `seq(eff, result) -> result`.
/// An un-threaded orphan and a top-level orphan are byte-identical in this IR,
/// so a consumer cannot recover the distinction and will correctly treat the
/// effect as unconditional.
///
/// The M1/AI3 front end discharges this in `nf_lowering.rs`
/// `seq_scope_arm_effects`. The emitter side is `compute_branch_paths` in
/// `emit/rust_05.rs`, whose doc comment records why a positional guess at arm
/// membership was tried here and removed.
#[derive(Clone, Debug, PartialEq)]
pub struct CoreBundle {
    pub version: String,
    pub constant_pool: Vec<ConstantPoolEntry>,
    pub nodes: Vec<Node>,
    /// The bundle's semantic value — the ref this bundle's consumer must
    /// emit/return. Unconstrained by kind: may point at any node (CCall/
    /// CIf/CDeterminate) or directly at a bare pool entry. Without this
    /// field, codegen had to guess "the last node in `nodes`," which is
    /// wrong whenever the source's tail is a bare literal/VarRef following
    /// a real call (see BRANCH_SCOPING_V1's sibling bug in emit/rust_05.rs).
    pub result: NodeRef,
}

// ── Hash utilities ───────────────────────────────────────────────────────────

pub fn sha256_bytes(data: &[u8]) -> Hash256 {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().into()
}

pub fn hash256_to_hex(h: &Hash256) -> String {
    h.iter().map(|b| format!("{:02x}", b)).collect()
}

pub fn hex_to_hash256(s: &str) -> Result<Hash256, String> {
    let s = s.trim_start_matches("0x");
    if s.len() != 64 {
        return Err(format!("expected 64 hex chars, got {}", s.len()));
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let hi = hex_nibble(chunk[0]).ok_or_else(|| "invalid hex digit".to_string())?;
        let lo = hex_nibble(chunk[1]).ok_or_else(|| "invalid hex digit".to_string())?;
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

// ── Bundle identity ──────────────────────────────────────────────────────────
//
// Matches axis-lang-lab ir/core_ir_05::{serialize_canonical, bundle_identity}
// byte for byte: sha256 of the canonical encoding, version excluded. This is
// the hash a registry `body 0x…` line names, and what `axis` prints as a
// compiled bundle's `identity`.

fn write_varint(buf: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            buf.push(byte);
            break;
        }
        buf.push(byte | 0x80);
    }
}

fn write_bytes(buf: &mut Vec<u8>, data: &[u8]) {
    write_varint(buf, data.len() as u64);
    buf.extend_from_slice(data);
}

fn write_noderef(buf: &mut Vec<u8>, r: &NodeRef) {
    write_varint(buf, match *r {
        NodeRef::Node(i) => (i as u64) << 1,
        NodeRef::Pool(i) => ((i as u64) << 1) | 1,
    });
}

/// Canonical bytes of a bundle (excludes version).
pub fn serialize_canonical(bundle: &CoreBundle) -> Vec<u8> {
    let mut buf = Vec::new();
    write_varint(&mut buf, bundle.constant_pool.len() as u64);
    for entry in &bundle.constant_pool {
        buf.extend_from_slice(&entry.def_hash);
        write_bytes(&mut buf, &entry.payload);
    }
    write_varint(&mut buf, bundle.nodes.len() as u64);
    for node in &bundle.nodes {
        match node {
            Node::CCall { target_identity, args, target_name } => {
                write_varint(&mut buf, 0);
                write_bytes(&mut buf, target_name.as_bytes());
                buf.extend_from_slice(target_identity);
                write_varint(&mut buf, args.len() as u64);
                for a in args {
                    write_noderef(&mut buf, a);
                }
            }
            Node::CIf { cond, then_, else_ } => {
                write_varint(&mut buf, 1);
                write_noderef(&mut buf, cond);
                write_noderef(&mut buf, then_);
                write_noderef(&mut buf, else_);
            }
            Node::CDeterminate => write_varint(&mut buf, 2),
        }
    }
    write_noderef(&mut buf, &bundle.result);
    buf
}

/// The bundle's content identity: what a registry `body` line names.
pub fn bundle_identity(bundle: &CoreBundle) -> Hash256 {
    sha256_bytes(&serialize_canonical(bundle))
}

fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

// ── Type identity hashes ─────────────────────────────────────────────────────
//
// Matches axis-lang-lab registry/core05/codec::type_identity:
//   encode_type(Primitive(code)) = [TAG_TYPE_DEF=0x01, shape_kind=0x00, prim_code]
//   type_identity = sha256(encode_type(...))
//
// PrimCode: Unit=0, Bool=1, Int=2, Float=3, Bytes=4, Text=5, Value=6, Dec=7, Fn=8

pub fn primitive_type_hash(prim_code: u8) -> Hash256 {
    sha256_bytes(&[0x01, 0x00, prim_code])
}

pub fn unit_type_hash()  -> Hash256 { primitive_type_hash(0) }
pub fn bool_type_hash()  -> Hash256 { primitive_type_hash(1) }
pub fn int_type_hash()   -> Hash256 { primitive_type_hash(2) }
pub fn float_type_hash() -> Hash256 { primitive_type_hash(3) }
pub fn bytes_type_hash() -> Hash256 { primitive_type_hash(4) }
pub fn text_type_hash()  -> Hash256 { primitive_type_hash(5) }
pub fn value_type_hash() -> Hash256 { primitive_type_hash(6) }
pub fn dec_type_hash()   -> Hash256 { primitive_type_hash(7) }
pub fn fn_type_hash()    -> Hash256 { primitive_type_hash(8) }
/// `Param` type — a pool entry of this type is a parameter slot whose payload
/// is `varint(slot_index)`. The bridge substitutes such pool entries with the
/// caller's corresponding arg at codegen (see `emit_rust_lib_from_bundle`).
pub fn param_type_hash() -> Hash256 { primitive_type_hash(9) }

/// List type hash: sha256([0x01, 0x03, element_type_hash...]).
/// shape_kind 0x03 = list.
pub fn list_type_hash(element: &Hash256) -> Hash256 {
    let mut buf = vec![0x01u8, 0x03];
    buf.extend_from_slice(element);
    sha256_bytes(&buf)
}

/// TextList = List(Text).
pub fn text_list_type_hash() -> Hash256 {
    list_type_hash(&text_type_hash())
}

/// ValueList = List(Value) — homogeneous list of `Value` (data only).
pub fn value_list_type_hash() -> Hash256 {
    list_type_hash(&value_type_hash())
}

// ── Payload codecs ────────────────────────────────────────────────────────────
//
// Matching axis-lang-lab fabric/codec value payload encoders:
//   bool: single 0x00/0x01 byte
//   int:  zig-zag then minimal unsigned LEB128
//   text: length-prefixed (varint) UTF-8
//   unit: empty

pub fn encode_bool_payload(v: bool) -> Vec<u8> {
    vec![if v { 0x01 } else { 0x00 }]
}

pub fn encode_int_payload(v: i64) -> Vec<u8> {
    let zigzag = ((v << 1) ^ (v >> 63)) as u64;
    let mut buf = Vec::new();
    let mut n = zigzag;
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            buf.push(byte);
            break;
        } else {
            buf.push(byte | 0x80);
        }
    }
    buf
}

pub fn encode_text_payload(v: &str) -> Vec<u8> {
    let bytes = v.as_bytes();
    let mut buf = Vec::new();
    let mut len = bytes.len() as u64;
    loop {
        let byte = (len & 0x7f) as u8;
        len >>= 7;
        if len == 0 {
            buf.push(byte);
            break;
        } else {
            buf.push(byte | 0x80);
        }
    }
    buf.extend_from_slice(bytes);
    buf
}

pub fn decode_bool_payload(payload: &[u8]) -> Result<bool, String> {
    match payload {
        // Strict per the bridge codec: Bool is exactly one byte, 0x00 | 0x01.
        // An empty payload is INVALID — the producer must encode a valid zero
        // value (false = [0x00]). The bridge stays the strict validator so a
        // malformed/empty sentinel is caught here, not silently coerced.
        [0x00] => Ok(false),
        [0x01] => Ok(true),
        other => Err(format!("invalid bool payload: {:?}", other)),
    }
}

/// Decode a plain unsigned LEB128 varint (no zigzag). Matches the compiler's
/// `fabric::codec::write_varint`. Used for `Param` pool entry payloads where
/// the value is a slot index (always non-negative).
pub fn decode_unsigned_varint(payload: &[u8]) -> Result<u64, String> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    for &byte in payload {
        let payload_bits = (byte & 0x7f) as u64;
        result |= payload_bits << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
        if shift >= 64 {
            return Err("unsigned varint overflow".to_string());
        }
    }
    Err("truncated unsigned varint".to_string())
}

pub fn decode_int_payload(payload: &[u8]) -> Result<i64, String> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    let mut consumed = false;
    for &byte in payload {
        let payload_bits = (byte & 0x7f) as u64;
        result |= payload_bits << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            consumed = true;
            break;
        }
        if shift >= 64 {
            return Err("int varint overflow".to_string());
        }
    }
    if !consumed {
        return Err("truncated int payload".to_string());
    }
    Ok(((result >> 1) as i64) ^ -((result & 1) as i64))
}

pub fn decode_text_payload(payload: &[u8]) -> Result<String, String> {
    let mut pos = 0usize;
    let mut len: u64 = 0;
    let mut shift: u32 = 0;
    loop {
        if pos >= payload.len() {
            return Err("truncated text length".to_string());
        }
        let byte = payload[pos];
        pos += 1;
        len |= ((byte & 0x7f) as u64) << shift;
        shift += 7;
        if byte & 0x80 == 0 { break; }
        if shift >= 64 { return Err("text length varint overflow".to_string()); }
    }
    let len = len as usize;
    if pos + len > payload.len() {
        return Err("truncated text payload".to_string());
    }
    String::from_utf8(payload[pos..pos + len].to_vec())
        .map_err(|e| format!("text UTF-8 error: {}", e))
}

// ── Float / Dec payload codecs (BRIDGE_VALUE_COERCION_V1) ────────────────────
//
//   float: 8 bytes, IEEE 754 binary64 little-endian.
//   dec:   16 bytes, rust_decimal::Decimal::serialize() canonical form.

pub fn encode_float_payload(v: f64) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

pub fn decode_float_payload(payload: &[u8]) -> Result<f64, String> {
    if payload.len() != 8 {
        return Err(format!("invalid float payload: expected 8 bytes, got {}", payload.len()));
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(payload);
    Ok(f64::from_le_bytes(buf))
}

pub fn encode_dec_payload(v: rust_decimal::Decimal) -> Vec<u8> {
    v.serialize().to_vec()
}

pub fn decode_dec_payload(payload: &[u8]) -> Result<rust_decimal::Decimal, String> {
    if payload.len() != 16 {
        return Err(format!("invalid dec payload: expected 16 bytes, got {}", payload.len()));
    }
    let mut buf = [0u8; 16];
    buf.copy_from_slice(payload);
    Ok(rust_decimal::Decimal::deserialize(buf))
}

// ── Bytes payload codec (BRIDGE_BYTES_IO_M1) ─────────────────────────────────
//
//   bytes: length-prefixed (varint) opaque blob — same envelope shape as text,
//          but no UTF-8 validation.

pub fn encode_bytes_payload(v: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(v.len() + 5);
    let mut len = v.len() as u64;
    loop {
        let byte = (len & 0x7f) as u8;
        len >>= 7;
        if len == 0 {
            buf.push(byte);
            break;
        } else {
            buf.push(byte | 0x80);
        }
    }
    buf.extend_from_slice(v);
    buf
}

pub fn decode_bytes_payload(payload: &[u8]) -> Result<Vec<u8>, String> {
    let mut pos = 0usize;
    let mut len: u64 = 0;
    let mut shift: u32 = 0;
    loop {
        if pos >= payload.len() {
            return Err("truncated bytes length".to_string());
        }
        let byte = payload[pos];
        pos += 1;
        len |= ((byte & 0x7f) as u64) << shift;
        shift += 7;
        if byte & 0x80 == 0 { break; }
        if shift >= 64 { return Err("bytes length varint overflow".to_string()); }
    }
    let len = len as usize;
    if pos + len > payload.len() {
        return Err("truncated bytes payload".to_string());
    }
    Ok(payload[pos..pos + len].to_vec())
}

// ── Native wire format (.axbi): the canonical binary, no third-party code ──
//
// The decoder half of serialize_canonical, and the 6-byte file header ('A','X','C','I', ir_major 0, ir_minor 5) of the spec
// (core_ir_spec/axis-core-ir-0.5.md, "Axial Binary File Format"). Strict like the spec says: a non-minimal varint, a forward node
// reference, a pool reference past the pool, a bad kind tag, bad UTF-8 or trailing bytes is a hard error. Bundle identity is
// SHA-256 of the canonical bytes (bytes[6..] of a file), so a bundle read from .axbi and written back has the identity it came with.

pub const AXBI_MAGIC: [u8; 4] = *b"AXCI";

struct CanonReader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> CanonReader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.b.len() - self.pos < n {
            return Err(format!("axbi: truncated at byte {} (need {} more)", self.pos, n));
        }
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    /// Unsigned LEB128 in minimal form: a redundant continuation byte is a hard error.
    fn varint(&mut self) -> Result<u64, String> {
        let start = self.pos;
        let mut v: u64 = 0;
        let mut shift = 0u32;
        loop {
            let byte = self.take(1)?[0];
            if shift >= 64 || (shift == 63 && (byte & 0x7f) > 1) {
                return Err(format!("axbi: varint overflows u64 at byte {}", start));
            }
            v |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                if byte == 0 && self.pos - start > 1 {
                    return Err(format!("axbi: non-minimal varint at byte {}", start));
                }
                return Ok(v);
            }
            shift += 7;
        }
    }

    fn len(&mut self) -> Result<usize, String> {
        let n = self.varint()?;
        usize::try_from(n).map_err(|_| "axbi: length does not fit usize".to_string())
    }

    fn hash(&mut self) -> Result<Hash256, String> {
        let mut h = [0u8; 32];
        h.copy_from_slice(self.take(32)?);
        Ok(h)
    }

    fn noderef(&mut self, nodes_so_far: u64, pool_count: u64) -> Result<NodeRef, String> {
        let v = self.varint()?;
        let idx = v >> 1;
        if v & 1 == 1 {
            if idx >= pool_count {
                return Err(format!("axbi: pool({}) out of range (pool has {})", idx, pool_count));
            }
            Ok(NodeRef::Pool(idx as u32))
        } else {
            if idx >= nodes_so_far {
                return Err(format!("axbi: node({}) breaks the topological order (only {} before it)", idx, nodes_so_far));
            }
            Ok(NodeRef::Node(idx as u32))
        }
    }
}

/// The inverse of `serialize_canonical`. The version is not in the canonical bytes; the result is a 0.5 bundle.
pub fn deserialize_canonical(bytes: &[u8]) -> Result<CoreBundle, String> {
    let mut r = CanonReader { b: bytes, pos: 0 };
    let pool_count = r.varint()?;
    let mut constant_pool = Vec::new();
    for _ in 0..pool_count {
        let def_hash = r.hash()?;
        let n = r.len()?;
        constant_pool.push(ConstantPoolEntry { def_hash, payload: r.take(n)?.to_vec() });
    }
    let node_count = r.varint()?;
    let mut nodes = Vec::new();
    for i in 0..node_count {
        match r.varint()? {
            0 => {
                let n = r.len()?;
                let target_name = String::from_utf8(r.take(n)?.to_vec())
                    .map_err(|e| format!("axbi: target name is not UTF-8: {}", e))?;
                let target_identity = r.hash()?;
                let argc = r.varint()?;
                let mut args = Vec::new();
                for _ in 0..argc {
                    args.push(r.noderef(i, pool_count)?);
                }
                nodes.push(Node::CCall { target_identity, args, target_name });
            }
            1 => {
                let cond = r.noderef(i, pool_count)?;
                let then_ = r.noderef(i, pool_count)?;
                let else_ = r.noderef(i, pool_count)?;
                nodes.push(Node::CIf { cond, then_, else_ });
            }
            2 => nodes.push(Node::CDeterminate),
            k => return Err(format!("axbi: unknown node kind tag {}", k)),
        }
    }
    let result = r.noderef(node_count, pool_count)?;
    if r.pos != bytes.len() {
        return Err(format!("axbi: {} trailing byte(s) after the result", bytes.len() - r.pos));
    }
    Ok(CoreBundle { version: "0.5".to_string(), constant_pool, nodes, result })
}

/// A whole .axbi file: the 6-byte header (not part of the identity) and the canonical payload.
pub fn serialize_axbi(bundle: &CoreBundle) -> Vec<u8> {
    let mut out = AXBI_MAGIC.to_vec();
    out.push(0);
    out.push(5);
    out.extend_from_slice(&serialize_canonical(bundle));
    out
}

pub fn is_axbi(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[0..4] == AXBI_MAGIC
}

pub fn parse_axbi(bytes: &[u8]) -> Result<CoreBundle, String> {
    if !is_axbi(bytes) {
        return Err("axbi: bad magic (not AXCI)".to_string());
    }
    if bytes.len() < 6 || bytes[4] != 0 || bytes[5] != 5 {
        return Err(format!("axbi: unsupported IR version (this reader requires 0.5)"));
    }
    deserialize_canonical(&bytes[6..])
}

#[cfg(test)]
mod axbi_tests {
    use super::*;

    fn sample() -> CoreBundle {
        CoreBundle {
            version: "0.5".to_string(),
            constant_pool: vec![
                ConstantPoolEntry { def_hash: param_type_hash(), payload: vec![0] },
                ConstantPoolEntry { def_hash: text_type_hash(), payload: encode_text_payload("none") },
                ConstantPoolEntry { def_hash: int_type_hash(), payload: encode_int_payload(-300) },
            ],
            nodes: vec![
                Node::CCall { target_identity: [7u8; 32], args: vec![NodeRef::Pool(0), NodeRef::Pool(2)], target_name: "int_eq".to_string() },
                Node::CIf { cond: NodeRef::Node(0), then_: NodeRef::Pool(1), else_: NodeRef::Pool(1) },
                Node::CDeterminate,
            ],
            result: NodeRef::Node(1),
        }
    }

    #[test]
    fn a_bundle_survives_the_native_roundtrip_with_its_identity() {
        let b = sample();
        let bytes = serialize_axbi(&b);
        assert!(is_axbi(&bytes));
        let back = parse_axbi(&bytes).expect("decodes");
        assert_eq!(back, b);
        assert_eq!(bundle_identity(&back), bundle_identity(&b));
    }

    #[test]
    fn the_decoder_is_strict() {
        let good = serialize_axbi(&sample());
        // a bad magic
        let mut bad = good.clone();
        bad[0] = b'B';
        assert!(parse_axbi(&bad).is_err());
        // a wrong version
        let mut bad = good.clone();
        bad[5] = 4;
        assert!(parse_axbi(&bad).is_err());
        // trailing bytes
        let mut bad = good.clone();
        bad.push(0);
        assert!(parse_axbi(&bad).is_err());
        // a truncated payload
        assert!(parse_axbi(&good[..good.len() - 1]).is_err());
        // a non-minimal varint for the pool count (0x80 0x00 is a padded zero)
        assert!(deserialize_canonical(&[0x80, 0x00]).is_err());
        // a forward node reference: one CIf whose cond is node(0) while it is node 0 itself
        let mut buf = Vec::new();
        write_varint(&mut buf, 0); // empty pool
        write_varint(&mut buf, 1); // one node
        write_varint(&mut buf, 1); // CIf
        write_noderef(&mut buf, &NodeRef::Node(0));
        write_noderef(&mut buf, &NodeRef::Node(0));
        write_noderef(&mut buf, &NodeRef::Node(0));
        write_noderef(&mut buf, &NodeRef::Node(0));
        assert!(deserialize_canonical(&buf).is_err());
        // an unknown node kind
        let mut buf = Vec::new();
        write_varint(&mut buf, 0);
        write_varint(&mut buf, 1);
        write_varint(&mut buf, 9);
        assert!(deserialize_canonical(&buf).is_err());
    }
}

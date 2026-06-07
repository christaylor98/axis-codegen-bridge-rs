use sha2::{Digest, Sha256};

pub mod loader;
pub mod inspect;
pub mod serialiser;
pub mod lower;

pub use loader::{load_core_bundle, load_core_bundle_from_bytes};
pub use inspect::inspect_core_bundle;
pub use serialiser::{create_core_bundle_05, write_core_bundle_05_to_file};
pub use lower::lower_core_term_to_bundle_05;

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
        /// Output type def_hash. All-zero = not annotated (verifier falls back to
        /// registry lookup). Set to the function's declared return type hash.
        result_type: Hash256,
    },
    CIf {
        cond: NodeRef,
        then_: NodeRef,
        else_: NodeRef,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct CoreBundle {
    pub version: String,
    pub constant_pool: Vec<ConstantPoolEntry>,
    pub nodes: Vec<Node>,
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
// PrimCode: Unit=0, Bool=1, Int=2, Text=5

pub fn primitive_type_hash(prim_code: u8) -> Hash256 {
    sha256_bytes(&[0x01, 0x00, prim_code])
}

pub fn unit_type_hash() -> Hash256 { primitive_type_hash(0) }
pub fn bool_type_hash() -> Hash256 { primitive_type_hash(1) }
pub fn int_type_hash()  -> Hash256 { primitive_type_hash(2) }
pub fn text_type_hash() -> Hash256 { primitive_type_hash(5) }

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

/// All-zero hash sentinel meaning "result type not annotated".
pub const NO_RESULT_TYPE: Hash256 = [0u8; 32];

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
        [0x00] => Ok(false),
        [0x01] => Ok(true),
        other => Err(format!("invalid bool payload: {:?}", other)),
    }
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

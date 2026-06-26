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

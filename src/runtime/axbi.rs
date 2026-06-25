//! AXBI bridge — single parse function for Core IR 0.5 binary format.
//!
//! ONE public bridge function: `axbi_parse`.
//!
//! Takes a `ValueList` of raw bytes (each element `Value::Int` 0–255),
//! validates the .axbi header, and returns a fully-structured `Value` tree
//! that M1 can navigate with standard list/tuple bridge functions.
//!
//! Bridge spec: core_ir_spec/axbi-m1-bridge-spec.md
//! M1 examples: examples/surface-m1/format/ (placeholder — see note in files)

use super::value::{Value, intern_str, intern_tag};

// ── Magic / version ───────────────────────────────────────────────────────────

const MAGIC: &[u8; 4] = b"AXCI";
const IR_MAJOR: u8 = 0x00;
const IR_MINOR: u8 = 0x05;

// ── UNKNOWN gate ──────────────────────────────────────────────────────────────

#[cold]
#[track_caller]
fn hard_fail(msg: &str) -> ! {
    panic!("axbi_parse UNKNOWN gate: {}", msg)
}

// ── Varint (unsigned LEB128, minimal form) ────────────────────────────────────

fn read_varint(data: &[u8], pos: usize) -> (u64, usize) {
    let mut result = 0u64;
    let mut shift  = 0u32;
    let mut p = pos;
    loop {
        if p >= data.len() { hard_fail("truncated varint"); }
        let byte = data[p]; p += 1;
        let payload = (byte & 0x7F) as u64;
        if shift >= 64 || (shift == 63 && payload > 1) { hard_fail("varint overflow"); }
        result |= payload << shift;
        if byte & 0x80 == 0 {
            if byte == 0 && shift != 0 { hard_fail("non-minimal varint"); }
            return (result, p);
        }
        shift += 7;
    }
}

// ── Byte helpers ──────────────────────────────────────────────────────────────

fn input_to_bytes(v: Value) -> Vec<u8> {
    match v {
        Value::List(es) => es.into_iter().map(|e| match e {
            Value::Int(n) if (0..=255).contains(&n) => n as u8,
            Value::Int(n) => hard_fail(&format!("byte value {n} out of 0..=255")),
            _ => hard_fail("byte list element is not Int"),
        }).collect(),
        _ => hard_fail("axbi_parse: input must be a List of Int bytes"),
    }
}

fn bytes_to_list(bytes: &[u8]) -> Value {
    Value::List(bytes.iter().map(|&b| Value::Int(b as i64)).collect())
}

fn hex_str(bytes: &[u8]) -> Value {
    let s: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
    Value::Str(intern_str(&s))
}

// ── NodeRef encoding ──────────────────────────────────────────────────────────
//
// A NodeRef is returned as Value::Tuple([Value::Str("node"|"pool"), Value::Int(idx)]).
// M1 can decompose it with tuple_get(ref, 0) for space and tuple_get(ref, 1) for index.

fn read_edge(data: &[u8], pos: usize, node_idx: u32, pool_len: u32) -> (Value, usize) {
    let (tagged, p) = read_varint(data, pos);
    let idx = (tagged >> 1) as u32;
    if tagged & 1 == 1 {
        if idx >= pool_len { hard_fail(&format!("pool edge {idx} out of range ({pool_len})")); }
        (Value::Tuple(vec![Value::Str(intern_str("pool")), Value::Int(idx as i64)]), p)
    } else {
        if idx >= node_idx { hard_fail(&format!("forward edge at node {node_idx}: references {idx}")); }
        (Value::Tuple(vec![Value::Str(intern_str("node")), Value::Int(idx as i64)]), p)
    }
}

// ── Canonical binary → Value tree ────────────────────────────────────────────
//
// Returned structure:
//
//   Value::Tuple([
//     pool:  Value::List([
//              Value::Tuple([def_hash_hex: Str, payload: List<Int>]),
//              ...
//            ]),
//     nodes: Value::List([
//              Value::Ctor { tag:"CCall", fields:[name:Str, identity_hex:Str, args:List<Tuple>] },
//              Value::Ctor { tag:"CIf",   fields:[cond:Tuple, then:Tuple, else:Tuple] },
//              Value::Ctor { tag:"CDeterminate", fields:[] },
//              ...
//            ]),
//   ])

fn parse_canonical(data: &[u8]) -> Value {
    let mut pos = 0;

    // pool
    let (pool_count, p) = read_varint(data, pos); pos = p;
    let mut pool: Vec<Value> = Vec::with_capacity(pool_count as usize);
    for _ in 0..pool_count {
        if pos + 32 > data.len() { hard_fail("truncated def_hash"); }
        let def_hash = &data[pos..pos + 32]; pos += 32;
        let (payload_len, p) = read_varint(data, pos); pos = p;
        let end = pos + payload_len as usize;
        if end > data.len() { hard_fail("truncated payload"); }
        let payload = &data[pos..end]; pos = end;
        pool.push(Value::Tuple(vec![hex_str(def_hash), bytes_to_list(payload)]));
    }

    // nodes
    let (node_count, p) = read_varint(data, pos); pos = p;
    let pool_len = pool_count as u32;
    let mut nodes: Vec<Value> = Vec::with_capacity(node_count as usize);

    for i in 0..node_count as u32 {
        let (kind, p) = read_varint(data, pos); pos = p;
        match kind {
            0 => {
                // CCall
                let (name_len, p) = read_varint(data, pos); pos = p;
                let end = pos + name_len as usize;
                if end > data.len() { hard_fail("truncated target_name"); }
                let name = std::str::from_utf8(&data[pos..end])
                    .unwrap_or_else(|_| hard_fail("target_name not valid UTF-8"));
                let name_val = Value::Str(intern_str(name));
                pos = end;
                if pos + 32 > data.len() { hard_fail("truncated target_identity"); }
                let identity = hex_str(&data[pos..pos + 32]); pos += 32;
                let (arg_count, p) = read_varint(data, pos); pos = p;
                let mut args: Vec<Value> = Vec::with_capacity(arg_count as usize);
                for _ in 0..arg_count {
                    let (edge, p) = read_edge(data, pos, i, pool_len); pos = p;
                    args.push(edge);
                }
                nodes.push(Value::Ctor {
                    tag:    intern_tag("CCall"),
                    fields: vec![name_val, identity, Value::List(args)],
                });
            }
            1 => {
                // CIf
                let (cond,  p) = read_edge(data, pos, i, pool_len); pos = p;
                let (then_, p) = read_edge(data, pos, i, pool_len); pos = p;
                let (else_, p) = read_edge(data, pos, i, pool_len); pos = p;
                nodes.push(Value::Ctor {
                    tag:    intern_tag("CIf"),
                    fields: vec![cond, then_, else_],
                });
            }
            2 => {
                nodes.push(Value::Ctor { tag: intern_tag("CDeterminate"), fields: vec![] });
            }
            k => hard_fail(&format!("unknown node kind tag {k}")),
        }
    }

    Value::Tuple(vec![Value::List(pool), Value::List(nodes)])
}

// ── Public bridge function ────────────────────────────────────────────────────

/// `axbi_parse(data: ValueList) → Value`
///
/// Parse a `.axbi` byte stream into a structured Value tree.
///
/// Input:  `Value::List` where each element is `Value::Int(0..=255)`.
/// Output: `Value::Tuple([pool_list, node_list])` — see §Canonical binary → Value
///         tree above for the full shape.
///
/// Hard-fails (UNKNOWN gate — panic) on: bad magic, unsupported version,
/// truncated data, non-minimal varint, forward edge, out-of-range pool index.
#[track_caller]
pub fn axbi_parse(v: Value) -> Value {
    let bytes = input_to_bytes(v);
    if bytes.len() < 6 { hard_fail("input too short to be .axbi (need ≥ 6 bytes)"); }
    if &bytes[0..4] != MAGIC.as_slice() {
        hard_fail(&format!("bad magic {:?}: expected b\"AXCI\"", &bytes[0..4]));
    }
    if bytes[4] != IR_MAJOR || bytes[5] != IR_MINOR {
        hard_fail(&format!("unsupported IR version {}.{} (expected 0.5)", bytes[4], bytes[5]));
    }
    parse_canonical(&bytes[6..])
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn write_varint_test(buf: &mut Vec<u8>, mut n: u64) {
        loop {
            let b = (n & 0x7F) as u8;
            n >>= 7;
            if n == 0 { buf.push(b); return; }
            buf.push(b | 0x80);
        }
    }

    fn make_axbi() -> Vec<u8> {
        // pool: 1 entry — def_hash=0x00*32, payload=[0x01]
        // nodes: 1 CCall "test_fn" (identity=0x02*32, args=[pool[0]])
        let mut canonical: Vec<u8> = Vec::new();
        write_varint_test(&mut canonical, 1);       // pool_count
        canonical.extend_from_slice(&[0u8; 32]);    // def_hash
        write_varint_test(&mut canonical, 1);        // payload_len
        canonical.push(0x01);                        // payload

        write_varint_test(&mut canonical, 1);        // node_count
        write_varint_test(&mut canonical, 0);        // kind = CCall
        let name = b"test_fn";
        write_varint_test(&mut canonical, name.len() as u64);
        canonical.extend_from_slice(name);
        canonical.extend_from_slice(&[0x02u8; 32]); // target_identity
        write_varint_test(&mut canonical, 1);        // arg_count
        write_varint_test(&mut canonical, 1);        // pool(0) → (0<<1)|1

        let mut out = Vec::new();
        out.extend_from_slice(b"AXCI");
        out.push(0x00); out.push(0x05);
        out.extend_from_slice(&canonical);
        out
    }

    fn as_list(bytes: &[u8]) -> Value {
        Value::List(bytes.iter().map(|&b| Value::Int(b as i64)).collect())
    }

    #[test]
    fn parse_structure() {
        let bundle = axbi_parse(as_list(&make_axbi()));

        let (pool, nodes) = match bundle {
            Value::Tuple(ref es) => match (&es[0], &es[1]) {
                (Value::List(p), Value::List(n)) => (p, n),
                _ => panic!("expected Tuple([List, List])"),
            },
            _ => panic!("expected Tuple"),
        };

        assert_eq!(pool.len(), 1);
        assert_eq!(nodes.len(), 1);

        // pool entry: Tuple([def_hash_hex, payload_list])
        match &pool[0] {
            Value::Tuple(es) => {
                assert_eq!(es[0], Value::Str(intern_str(&"00".repeat(32))));
                assert_eq!(es[1], Value::List(vec![Value::Int(0x01)]));
            }
            _ => panic!("pool entry not Tuple"),
        }

        // node: Ctor { tag: CCall, fields: [name, identity_hex, args] }
        match &nodes[0] {
            Value::Ctor { tag, fields } => {
                use crate::runtime::value::get_tag_name;
                assert_eq!(get_tag_name(*tag), "CCall");
                assert_eq!(fields[0], Value::Str(intern_str("test_fn")));
                assert_eq!(fields[1], Value::Str(intern_str(&"02".repeat(32))));
                match &fields[2] {
                    Value::List(args) => {
                        assert_eq!(args.len(), 1);
                        match &args[0] {
                            Value::Tuple(ref_es) => {
                                assert_eq!(ref_es[0], Value::Str(intern_str("pool")));
                                assert_eq!(ref_es[1], Value::Int(0));
                            }
                            _ => panic!("arg not Tuple"),
                        }
                    }
                    _ => panic!("args not List"),
                }
            }
            _ => panic!("node not Ctor"),
        }
    }

    #[test]
    #[should_panic(expected = "UNKNOWN gate")]
    fn bad_magic_panics() {
        let mut bad = make_axbi();
        bad[0] = 0xFF;
        axbi_parse(as_list(&bad));
    }

    #[test]
    #[should_panic(expected = "UNKNOWN gate")]
    fn too_short_panics() {
        axbi_parse(as_list(&[0x41, 0x58]));
    }
}

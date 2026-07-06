//! BRIDGE_BYTE_CODEC_V1 (AXLANG_TURN_0002_BYTE_CODEC_PRIMITIVES) — the minimal,
//! load-bearing set of byte-buffer + big-endian-integer primitives that let M1
//! assemble and parse arbitrary binary wire-format messages (length-prefixed
//! frames, big-endian integers) as raw `Bytes`, without ever routing binary
//! framing through UTF-8-validated `Text`. Unblocks turn:axverity:0017 (the
//! Postgres wire protocol), whose length prefixes and typmods routinely produce
//! byte sequences that are not valid standalone UTF-8.
//!
//! Five conceptual primitives (seven fns — the two int codecs are encode/decode
//! pairs):
//!
//!   * `bytes_concat(a: Bytes, b: Bytes) -> Bytes`
//!         Append `b` to `a`. Binary, not n-ary — assemble a many-field message
//!         by folding `bytes_concat` in M1 (composition over speculative arity),
//!         the same discipline that keeps `byte_at` out of the bridge.
//!
//!   * `bytes_len(Bytes) -> Int`
//!         Length of the blob in bytes.
//!
//!   * `bytes_slice(b: Bytes, start: Int, end: Int) -> Bytes`
//!         Half-open `[start, end)` sub-blob. Bounds are STRICT: panics on a
//!         negative bound, on `start > end`, or on `end > len`. Wire framing
//!         must fail loudly on a bad offset — a silent clamp would hide the
//!         off-by-one framing bugs this module exists to make impossible.
//!         (This is deliberately stricter than `str_slice`, which clamps `end`;
//!         `byte_at` composes as `bytes_slice(b, i, i+1)`.)
//!
//!   * `int16_be_encode(Int) -> Bytes` / `int16_be_decode(Bytes) -> Int`
//!   * `int32_be_encode(Int) -> Bytes` / `int32_be_decode(Bytes) -> Int`
//!         Big-endian fixed-width integer codecs (Postgres wire is big-endian
//!         throughout). `encode` accepts either the signed or the unsigned range
//!         of the width and emits the two's-complement big-endian bytes, so one
//!         fn serves both a signed field (typmod = -1 → `FF FF FF FF`) and an
//!         unsigned one (a length prefix). `decode` requires EXACTLY the field
//!         width (2 or 4 bytes — a wrong length is a framing bug, so it panics)
//!         and interprets the bytes as SIGNED two's complement, so `typmod = -1`
//!         round-trips to `-1`. NOTE the resulting asymmetry: a value above the
//!         signed max encodes fine but decodes back negative (e.g. `int16` of
//!         `65535` → `FF FF` → `-1`). This is intrinsic to representing both a
//!         signed and an unsigned 16/32-bit field in one `Int`; the *bytes on
//!         the wire* are always correct, which is what the protocol cares about.
//!
//! All fns are panic-only leaf fns — no `Result` wrapper — matching net.rs and
//! the plain-return-type convention. These are pure (no I/O, no handle state):
//! `effect pure`, deterministic, idempotent. Identities are `sha256(name_utf8)`,
//! same convention as the rest of the bridge leaves.

use super::value::Value;

// ── bytes_concat ─────────────────────────────────────────────────────────────

#[track_caller]
pub fn bytes_concat(args: Value) -> Value {
    let (a, b) = match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("bytes_concat: expected Tuple(Bytes, Bytes), got {:?}", other),
    };
    let mut a = match a {
        Value::Bytes(b) => b,
        other => panic!("bytes_concat: arg 0 expected Bytes, got {:?}", other),
    };
    let b = match b {
        Value::Bytes(b) => b,
        other => panic!("bytes_concat: arg 1 expected Bytes, got {:?}", other),
    };
    a.extend_from_slice(&b);
    Value::Bytes(a)
}

// ── bytes_len ────────────────────────────────────────────────────────────────

#[track_caller]
pub fn bytes_len(v: Value) -> Value {
    match v {
        Value::Bytes(b) => Value::Int(b.len() as i64),
        other => panic!("bytes_len: expected Bytes, got {:?}", other),
    }
}

// ── bytes_slice ──────────────────────────────────────────────────────────────

#[track_caller]
pub fn bytes_slice(args: Value) -> Value {
    let (bytes, start, end) = match args {
        Value::Tuple(es) if es.len() == 3 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("bytes_slice: expected Tuple(Bytes, Int, Int), got {:?}", other),
    };
    let bytes = match bytes {
        Value::Bytes(b) => b,
        other => panic!("bytes_slice: arg 0 expected Bytes, got {:?}", other),
    };
    let start = match start {
        Value::Int(n) => n,
        other => panic!("bytes_slice: arg 1 expected Int start, got {:?}", other),
    };
    let end = match end {
        Value::Int(n) => n,
        other => panic!("bytes_slice: arg 2 expected Int end, got {:?}", other),
    };
    if start < 0 || end < 0 {
        panic!("bytes_slice: negative bound(s) start={} end={}", start, end);
    }
    let (start, end) = (start as usize, end as usize);
    if start > end {
        panic!("bytes_slice: start {} > end {}", start, end);
    }
    if end > bytes.len() {
        panic!("bytes_slice: end {} out of range for Bytes of len {}", end, bytes.len());
    }
    Value::Bytes(bytes[start..end].to_vec())
}

// ── int16_be_encode / int16_be_decode ────────────────────────────────────────

#[track_caller]
pub fn int16_be_encode(v: Value) -> Value {
    let n = match v {
        Value::Int(n) => n,
        other => panic!("int16_be_encode: expected Int, got {:?}", other),
    };
    // Accept either the signed (i16) or unsigned (u16) range of a 16-bit field.
    if !(-32768..=65535).contains(&n) {
        panic!("int16_be_encode: {} out of range for a 16-bit field (-32768..=65535)", n);
    }
    // `n as u16` takes the low 16 bits, i.e. two's-complement for negatives:
    // -1 -> 0xFFFF, -32768 -> 0x8000. Identical bytes to the unsigned value.
    Value::Bytes((n as u16).to_be_bytes().to_vec())
}

#[track_caller]
pub fn int16_be_decode(v: Value) -> Value {
    let b = match v {
        Value::Bytes(b) => b,
        other => panic!("int16_be_decode: expected Bytes, got {:?}", other),
    };
    if b.len() != 2 {
        panic!("int16_be_decode: expected exactly 2 bytes, got {}", b.len());
    }
    // Signed interpretation so a two's-complement field (e.g. -1) round-trips.
    Value::Int(i16::from_be_bytes([b[0], b[1]]) as i64)
}

// ── int32_be_encode / int32_be_decode ────────────────────────────────────────

#[track_caller]
pub fn int32_be_encode(v: Value) -> Value {
    let n = match v {
        Value::Int(n) => n,
        other => panic!("int32_be_encode: expected Int, got {:?}", other),
    };
    // Accept either the signed (i32) or unsigned (u32) range of a 32-bit field.
    if !(-2_147_483_648..=4_294_967_295).contains(&n) {
        panic!("int32_be_encode: {} out of range for a 32-bit field (-2147483648..=4294967295)", n);
    }
    // `n as u32` takes the low 32 bits (two's-complement for negatives):
    // -1 -> 0xFFFFFFFF. Identical bytes to the unsigned value.
    Value::Bytes((n as u32).to_be_bytes().to_vec())
}

#[track_caller]
pub fn int32_be_decode(v: Value) -> Value {
    let b = match v {
        Value::Bytes(b) => b,
        other => panic!("int32_be_decode: expected Bytes, got {:?}", other),
    };
    if b.len() != 4 {
        panic!("int32_be_decode: expected exactly 4 bytes, got {}", b.len());
    }
    // Signed interpretation so a two's-complement field (e.g. typmod = -1)
    // round-trips; Postgres message lengths are always < 2^31 so they are
    // unaffected.
    Value::Int(i32::from_be_bytes([b[0], b[1], b[2], b[3]]) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(v: Value) -> Vec<u8> {
        match v {
            Value::Bytes(b) => b,
            other => panic!("expected Bytes, got {:?}", other),
        }
    }
    fn int(v: Value) -> i64 {
        match v {
            Value::Int(n) => n,
            other => panic!("expected Int, got {:?}", other),
        }
    }

    // ── Big-endian codecs vs. KNOWN REAL Postgres wire values ────────────────
    // (Not just self-consistent round-trips — the exact bytes are asserted
    // against the documented wire format, per the turn's HIGH-rated risk.)

    #[test]
    fn int32_be_matches_real_postgres_wire_values() {
        // AuthenticationOk body length prefix is int32 = 8 -> 00 00 00 08.
        assert_eq!(bytes(int32_be_encode(Value::Int(8))), vec![0x00, 0x00, 0x00, 0x08]);
        assert_eq!(int(int32_be_decode(Value::Bytes(vec![0x00, 0x00, 0x00, 0x08]))), 8);

        // A no-modifier column reports typmod = -1 in RowDescription:
        // FF FF FF FF, and MUST decode back to -1 (not 4294967295).
        assert_eq!(bytes(int32_be_encode(Value::Int(-1))), vec![0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(int(int32_be_decode(Value::Bytes(vec![0xFF, 0xFF, 0xFF, 0xFF]))), -1);

        // int4 type OID = 23 -> 00 00 00 17.
        assert_eq!(bytes(int32_be_encode(Value::Int(23))), vec![0x00, 0x00, 0x00, 0x17]);
        assert_eq!(int(int32_be_decode(Value::Bytes(vec![0x00, 0x00, 0x00, 0x17]))), 23);

        // Startup-message protocol version 196608 (3.0) -> 00 03 00 00.
        assert_eq!(bytes(int32_be_encode(Value::Int(196608))), vec![0x00, 0x03, 0x00, 0x00]);
        assert_eq!(int(int32_be_decode(Value::Bytes(vec![0x00, 0x03, 0x00, 0x00]))), 196608);
    }

    #[test]
    fn int16_be_matches_real_postgres_wire_values() {
        // Format code 0 (text) / 1 (binary) are int16 fields.
        assert_eq!(bytes(int16_be_encode(Value::Int(0))), vec![0x00, 0x00]);
        assert_eq!(bytes(int16_be_encode(Value::Int(1))), vec![0x00, 0x01]);
        // A column/parameter count of 3 -> 00 03.
        assert_eq!(bytes(int16_be_encode(Value::Int(3))), vec![0x00, 0x03]);
        assert_eq!(int(int16_be_decode(Value::Bytes(vec![0x00, 0x03]))), 3);
        // int2 type size for int4 columns is 4 -> 00 04.
        assert_eq!(bytes(int16_be_encode(Value::Int(4))), vec![0x00, 0x04]);
        // Two's-complement: -1 -> FF FF, decodes back to -1.
        assert_eq!(bytes(int16_be_encode(Value::Int(-1))), vec![0xFF, 0xFF]);
        assert_eq!(int(int16_be_decode(Value::Bytes(vec![0xFF, 0xFF]))), -1);
    }

    #[test]
    fn encode_accepts_unsigned_range_low_bits() {
        // 40000 is above i16::MAX but a valid u16; bytes are 0x9C40.
        assert_eq!(bytes(int16_be_encode(Value::Int(40000))), vec![0x9C, 0x40]);
        // 3000000000 is above i32::MAX but a valid u32; bytes are 0xB2D05E00.
        assert_eq!(bytes(int32_be_encode(Value::Int(3_000_000_000))), vec![0xB2, 0xD0, 0x5E, 0x00]);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn int16_encode_rejects_over_u16_max() {
        int16_be_encode(Value::Int(65536));
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn int16_encode_rejects_under_i16_min() {
        int16_be_encode(Value::Int(-32769));
    }

    #[test]
    #[should_panic(expected = "expected exactly 4 bytes")]
    fn int32_decode_rejects_wrong_length() {
        int32_be_decode(Value::Bytes(vec![0x00, 0x00, 0x08]));
    }

    // ── bytes_concat / bytes_len / bytes_slice: assemble + disassemble a
    //    mock single-field RowDescription and confirm every field ────────────

    #[test]
    fn assemble_and_disassemble_mock_row_description_field() {
        use crate::runtime::bytes_io::text_to_bytes;
        use crate::runtime::value::intern_str;

        // One RowDescription field for column "id" int4:
        //   name "id\0" | tableOID=0 | attnum=0 | typeOID=23 | typeSize=4 |
        //   typmod=-1 | format=0
        let name = bytes(text_to_bytes(Value::Str(intern_str("id"))));
        let mut msg = Value::Bytes(name);
        let nul = Value::Bytes(vec![0x00]);
        msg = bytes_concat(Value::Tuple(vec![msg, nul]));                       // "id\0"
        msg = bytes_concat(Value::Tuple(vec![msg, int32_be_encode(Value::Int(0))]));   // tableOID
        msg = bytes_concat(Value::Tuple(vec![msg, int16_be_encode(Value::Int(0))]));   // attnum
        msg = bytes_concat(Value::Tuple(vec![msg, int32_be_encode(Value::Int(23))]));  // typeOID
        msg = bytes_concat(Value::Tuple(vec![msg, int16_be_encode(Value::Int(4))]));   // typeSize
        msg = bytes_concat(Value::Tuple(vec![msg, int32_be_encode(Value::Int(-1))]));  // typmod
        msg = bytes_concat(Value::Tuple(vec![msg, int16_be_encode(Value::Int(0))]));   // format

        // name(3) + 4 + 2 + 4 + 2 + 4 + 2 = 21 bytes.
        assert_eq!(int(bytes_len(msg.clone())), 21);

        // Byte-for-byte expected assembly.
        assert_eq!(
            bytes(msg.clone()),
            vec![
                0x69, 0x64, 0x00,             // "id\0"
                0x00, 0x00, 0x00, 0x00,       // tableOID = 0
                0x00, 0x00,                   // attnum = 0
                0x00, 0x00, 0x00, 0x17,       // typeOID = 23
                0x00, 0x04,                   // typeSize = 4
                0xFF, 0xFF, 0xFF, 0xFF,       // typmod = -1
                0x00, 0x00,                   // format = 0
            ]
        );

        // Slice fields back out (hand-computed offsets) and decode them.
        let type_oid = bytes_slice(Value::Tuple(vec![msg.clone(), Value::Int(9), Value::Int(13)]));
        assert_eq!(int(int32_be_decode(type_oid)), 23);
        let typmod = bytes_slice(Value::Tuple(vec![msg.clone(), Value::Int(15), Value::Int(19)]));
        assert_eq!(int(int32_be_decode(typmod)), -1);
        let format = bytes_slice(Value::Tuple(vec![msg.clone(), Value::Int(19), Value::Int(21)]));
        assert_eq!(int(int16_be_decode(format)), 0);

        // byte_at composes as bytes_slice(b, i, i+1): first byte is 'i' = 0x69.
        let first = bytes_slice(Value::Tuple(vec![msg, Value::Int(0), Value::Int(1)]));
        assert_eq!(bytes(first), vec![0x69]);
    }

    #[test]
    fn bytes_slice_empty_and_full_ranges() {
        let b = Value::Bytes(vec![1, 2, 3, 4]);
        assert_eq!(bytes(bytes_slice(Value::Tuple(vec![b.clone(), Value::Int(2), Value::Int(2)]))), Vec::<u8>::new());
        assert_eq!(bytes(bytes_slice(Value::Tuple(vec![b, Value::Int(0), Value::Int(4)]))), vec![1, 2, 3, 4]);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn bytes_slice_end_past_len_panics() {
        bytes_slice(Value::Tuple(vec![Value::Bytes(vec![1, 2]), Value::Int(0), Value::Int(3)]));
    }

    #[test]
    #[should_panic(expected = "start 3 > end 1")]
    fn bytes_slice_start_after_end_panics() {
        bytes_slice(Value::Tuple(vec![Value::Bytes(vec![1, 2, 3, 4]), Value::Int(3), Value::Int(1)]));
    }
}

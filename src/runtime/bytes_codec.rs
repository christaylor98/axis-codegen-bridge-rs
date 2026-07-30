//! BRIDGE_BYTE_CODEC_V1 (AXLANG_TURN_0002_BYTE_CODEC_PRIMITIVES) — the minimal,
//! load-bearing set of byte-buffer + big-endian-integer primitives that let M1
//! assemble and parse arbitrary binary wire-format messages (length-prefixed
//! frames, big-endian integers) as raw `Bytes`, without ever routing binary
//! framing through UTF-8-validated `Text`. Unblocks turn:axverity:0017 (the
//! Postgres wire protocol), whose length prefixes and typmods routinely produce
//! byte sequences that are not valid standalone UTF-8.
//!
//! Six conceptual primitives (seven fns). BYTE_INT_CODEC_COLLAPSE_V1 added the
//! bytes_get/bytes_push atoms and retired the two width-named decoders:
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
//!   * `bytes_get(b: Bytes, i: Int) -> Int` (BYTE_INT_CODEC_COLLAPSE_V1)
//!         Byte at index `i` as an `Int` in `0..=255`. STRICT bounds — panics on
//!         a negative or past-the-end index.
//!
//!   * `bytes_push(b: Bytes, n: Int) -> Bytes` (BYTE_INT_CODEC_COLLAPSE_V1)
//!         `b` with one byte appended; `n` must be `0..=255` or it panics (no
//!         silent `mod 256`).
//!
//!         These two are the `Bytes <-> Int` atoms. Before them, the ONLY way to
//!         cross that boundary was one of the four width-named codecs below —
//!         a memory-layout fact (16 vs 32 bit) leaking into the bridge identity
//!         surface. With them, BE-width codecs are M1 compositions
//!         (`be16_decode`, `be16_encode`, `be32_decode`, `be32_encode` in
//!         axVerity's lib/), and a future int8/int64 needs no new bridge fn and
//!         no new registry entry. Both are width-agnostic by construction:
//!         neither name nor body mentions a width.
//!
//!   * `int16_be_encode(Int) -> Bytes`
//!   * `int32_be_encode(Int) -> Bytes`
//!         Big-endian fixed-width integer ENCODERS (Postgres wire is big-endian
//!         throughout). Each accepts either the signed or the unsigned range of
//!         the width and emits the two's-complement big-endian bytes, so one fn
//!         serves both a signed field (typmod = -1 → `FF FF FF FF`) and an
//!         unsigned one (a length prefix). NOTE the asymmetry this creates on the
//!         way back: a value above the signed max encodes fine but decodes back
//!         negative (e.g. `int16` of `65535` → `FF FF` → `-1`). That is intrinsic
//!         to representing both a signed and an unsigned 16/32-bit field in one
//!         `Int`; the *bytes on the wire* are always correct, which is what the
//!         protocol cares about.
//!
//!         These two are SUPERSEDED but not yet retired. BYTE_INT_CODEC_COLLAPSE_V1
//!         replaced them with `be16_encode` / `be32_encode` in axVerity's lib/,
//!         composed over `bytes_push`; they remain here only because they are on
//!         the hot path (`int16_be_encode` hot=select, `int32_be_encode` hot=both)
//!         and their cutover is gated on that intent's Phase 6 measurement
//!         decision. Do NOT add a new width-named codec alongside them.
//!
//!         Their decode counterparts `int16_be_decode` / `int32_be_decode` were
//!         RETIRED by that intent's Phase 7 at verified zero live callers,
//!         replaced by `be16_decode` / `be32_decode` over `bytes_get`.
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

// ── bytes_get ────────────────────────────────────────────────────────────────

/// `bytes_get(b: Bytes, i: Int) -> Int`
///
/// The byte at index `i` as an `Int` in `0..=255`. Panics on a negative index or
/// one at/past the end — STRICT, like `bytes_slice`, never a silent clamp.
///
/// BYTE_INT_CODEC_COLLAPSE_V1: this is the `Bytes -> Int` atom the surface
/// lacked. Width-agnostic by construction — it knows nothing about 16/32-bit
/// fields, so BE-width decoding becomes M1 composition (see `be16_decode` /
/// `be32_decode`) instead of a width-named bridge fn.
#[track_caller]
pub fn bytes_get(args: Value) -> Value {
    let (bytes, idx) = match args {
        Value::Tuple(ref es) if es.len() == 2 => match (&es[0], &es[1]) {
            (Value::Bytes(b), Value::Int(i)) => (b, *i),
            _ => panic!("bytes_get: expected (Bytes, Int), got {:?}", args),
        },
        other => panic!("bytes_get: expected Tuple(Bytes, Int), got {:?}", other),
    };
    if idx < 0 || idx as usize >= bytes.len() {
        panic!("bytes_get: index {} out of range for Bytes of len {}", idx, bytes.len());
    }
    Value::Int(bytes[idx as usize] as i64)
}

// ── bytes_push ───────────────────────────────────────────────────────────────

/// `bytes_push(b: Bytes, n: Int) -> Bytes`
///
/// `b` with one byte appended. `n` MUST be in `0..=255` — out of range panics
/// rather than truncating mod 256.
///
/// The strict check is load-bearing, not defensive. It is what lets an M1
/// BE-encode composition drop its own upper-bound check: `be16_encode(65536)`
/// computes a top byte of `256`, which panics here. A silent `mod 256` would
/// turn an out-of-range field into plausible-looking wire bytes — exactly the
/// class of framing bug this module exists to make impossible (cf. the STRICT
/// bounds on `bytes_slice`).
///
/// BYTE_INT_CODEC_COLLAPSE_V1: the `Int -> Bytes` atom. Binary, not variadic —
/// fold it in M1 over `text_to_bytes(Text(""))`, the same "composition over
/// speculative arity" discipline that keeps `bytes_concat` binary.
#[track_caller]
pub fn bytes_push(args: Value) -> Value {
    let (mut bytes, byte) = match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            match (it.next().unwrap(), it.next().unwrap()) {
                (Value::Bytes(b), Value::Int(n)) => (b, n),
                (x, y) => panic!("bytes_push: expected (Bytes, Int), got ({:?}, {:?})", x, y),
            }
        }
        other => panic!("bytes_push: expected Tuple(Bytes, Int), got {:?}", other),
    };
    if !(0..=255).contains(&byte) { panic!("bytes_push: {} is not a byte value (0..=255)", byte) }
    bytes.push(byte as u8);
    Value::Bytes(bytes)
}

// ── int16_be_encode ──────────────────────────────────────────────────────────

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

// int16_be_decode was RETIRED by BYTE_INT_CODEC_COLLAPSE_V1 Phase 7 (zero live
// callers). Its replacement is axVerity's lib/be16_decode.m1, composed over
// bytes_get + integer arithmetic. Do not reintroduce a width-named decoder here.

// ── int32_be_encode ──────────────────────────────────────────────────────────

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

// int32_be_decode was RETIRED by BYTE_INT_CODEC_COLLAPSE_V1 Phase 7 (zero live
// callers). Its replacement is axVerity's lib/be32_decode.m1. Do not reintroduce
// a width-named decoder here.

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

    fn get(b: &[u8], i: i64) -> i64 {
        int(bytes_get(Value::Tuple(vec![Value::Bytes(b.to_vec()), Value::Int(i)])))
    }
    fn push(b: Vec<u8>, n: i64) -> Vec<u8> {
        bytes(bytes_push(Value::Tuple(vec![Value::Bytes(b), Value::Int(n)])))
    }

    // ── Rust mirrors of the M1 BE codec compositions ──────────────────────────
    //
    // These four mirror axVerity's lib/be{16,32}_{en,de}code.m1 line for line,
    // over the same two atoms (bytes_get / bytes_push) plus integer arithmetic
    // ONLY — no bitwise op anywhere, which is the whole point of
    // BYTE_INT_CODEC_COLLAPSE_V1.
    //
    // They serve two purposes. Before Phase 7 they were the differential oracle
    // against the width-named bridge fns. Now that int16_be_decode and
    // int32_be_decode are RETIRED, they are the only in-Rust statement of the
    // decode semantics, so the real-Postgres-wire assertions below can keep
    // asserting decode behaviour without the deleted fns. The M1 originals are
    // separately checked end-to-end over the emitted CoreIR by
    // axVerity's scripts/be-codec-sweep.sh.

    fn be16_enc(n: i64) -> Vec<u8> {
        assert!(n >= -32768, "be16_encode: {} below the 16-bit field minimum", n);
        let u = if n < 0 { n + 65536 } else { n };
        push(push(vec![], u / 256), u % 256)
    }
    fn be16_dec(b: &[u8]) -> i64 {
        let u = get(b, 0) * 256 + get(b, 1);
        if u >= 32768 { u - 65536 } else { u }
    }
    fn be32_enc(n: i64) -> Vec<u8> {
        assert!(n >= -2_147_483_648, "be32_encode: {} below the 32-bit field minimum", n);
        let u = if n < 0 { n + 4_294_967_296 } else { n };
        push(push(push(push(vec![], u / 16_777_216), u / 65_536 % 256), u / 256 % 256), u % 256)
    }
    fn be32_dec(b: &[u8]) -> i64 {
        let u = ((get(b, 0) * 256 + get(b, 1)) * 256 + get(b, 2)) * 256 + get(b, 3);
        if u >= 2_147_483_648 { u - 4_294_967_296 } else { u }
    }

    // ── Big-endian codecs vs. KNOWN REAL Postgres wire values ────────────────
    // (Not just self-consistent round-trips — the exact bytes are asserted
    // against the documented wire format, per the turn's HIGH-rated risk.)

    #[test]
    fn int32_be_matches_real_postgres_wire_values() {
        // AuthenticationOk body length prefix is int32 = 8 -> 00 00 00 08.
        assert_eq!(bytes(int32_be_encode(Value::Int(8))), vec![0x00, 0x00, 0x00, 0x08]);
        assert_eq!(be32_dec(&[0x00, 0x00, 0x00, 0x08]), 8);

        // A no-modifier column reports typmod = -1 in RowDescription:
        // FF FF FF FF, and MUST decode back to -1 (not 4294967295).
        assert_eq!(bytes(int32_be_encode(Value::Int(-1))), vec![0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(be32_dec(&[0xFF, 0xFF, 0xFF, 0xFF]), -1);

        // int4 type OID = 23 -> 00 00 00 17.
        assert_eq!(bytes(int32_be_encode(Value::Int(23))), vec![0x00, 0x00, 0x00, 0x17]);
        assert_eq!(be32_dec(&[0x00, 0x00, 0x00, 0x17]), 23);

        // Startup-message protocol version 196608 (3.0) -> 00 03 00 00.
        assert_eq!(bytes(int32_be_encode(Value::Int(196608))), vec![0x00, 0x03, 0x00, 0x00]);
        assert_eq!(be32_dec(&[0x00, 0x03, 0x00, 0x00]), 196608);
    }

    #[test]
    fn int16_be_matches_real_postgres_wire_values() {
        // Format code 0 (text) / 1 (binary) are int16 fields.
        assert_eq!(bytes(int16_be_encode(Value::Int(0))), vec![0x00, 0x00]);
        assert_eq!(bytes(int16_be_encode(Value::Int(1))), vec![0x00, 0x01]);
        // A column/parameter count of 3 -> 00 03.
        assert_eq!(bytes(int16_be_encode(Value::Int(3))), vec![0x00, 0x03]);
        assert_eq!(be16_dec(&[0x00, 0x03]), 3);
        // int2 type size for int4 columns is 4 -> 00 04.
        assert_eq!(bytes(int16_be_encode(Value::Int(4))), vec![0x00, 0x04]);
        // Two's-complement: -1 -> FF FF, decodes back to -1.
        assert_eq!(bytes(int16_be_encode(Value::Int(-1))), vec![0xFF, 0xFF]);
        assert_eq!(be16_dec(&[0xFF, 0xFF]), -1);
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

    /// The retired `int32_be_decode` panicked unless the input was EXACTLY 4
    /// bytes. The composition cannot reproduce that check — M1 has no
    /// error-raising primitive — so this test pins down what IS still enforced:
    /// a SHORT buffer (the truncated-frame case, the one that actually matters)
    /// still panics, and precisely, from `bytes_get`'s own bounds check.
    #[test]
    #[should_panic(expected = "index 3 out of range for Bytes of len 3")]
    fn be32_decode_still_panics_on_a_truncated_frame() {
        be32_dec(&[0x00, 0x00, 0x08]);
    }

    /// The other half of that trade, made explicit rather than left implicit:
    /// trailing bytes beyond the field are now TOLERATED where the retired fn
    /// rejected them. Recorded as a test so the behaviour change is discoverable
    /// here and not only in the intent report. No current caller can hit it —
    /// every decode call site passes `bytes_slice(msg, start, start + width)`.
    #[test]
    fn be32_decode_tolerates_trailing_bytes_unlike_the_retired_fn() {
        assert_eq!(be32_dec(&[0x00, 0x00, 0x00, 0x17, 0xDE, 0xAD]), 23);
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
        assert_eq!(be32_dec(&bytes(type_oid)), 23);
        let typmod = bytes_slice(Value::Tuple(vec![msg.clone(), Value::Int(15), Value::Int(19)]));
        assert_eq!(be32_dec(&bytes(typmod)), -1);
        let format = bytes_slice(Value::Tuple(vec![msg.clone(), Value::Int(19), Value::Int(21)]));
        assert_eq!(be16_dec(&bytes(format)), 0);

        // byte_at composes as bytes_slice(b, i, i+1): first byte is 'i' = 0x69.
        let first = bytes_slice(Value::Tuple(vec![msg, Value::Int(0), Value::Int(1)]));
        assert_eq!(bytes(first), vec![0x69]);
    }

    // ── bytes_get / bytes_push (BYTE_INT_CODEC_COLLAPSE_V1) ──────────────────
    // (the `get` / `push` wrappers live at the top of this module, alongside the
    //  be16/be32 mirrors that are built from them)

    #[test]
    fn bytes_get_reads_every_byte_as_unsigned() {
        // 0xFF must read as 255, NOT -1 — the BE decode composition relies on
        // each byte being an unsigned magnitude, with sign applied only once at
        // the end from the assembled value.
        assert_eq!(get(&[0x00, 0x7F, 0x80, 0xFF], 0), 0);
        assert_eq!(get(&[0x00, 0x7F, 0x80, 0xFF], 1), 127);
        assert_eq!(get(&[0x00, 0x7F, 0x80, 0xFF], 2), 128);
        assert_eq!(get(&[0x00, 0x7F, 0x80, 0xFF], 3), 255);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn bytes_get_past_end_panics() {
        get(&[1, 2], 2);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn bytes_get_negative_index_panics() {
        get(&[1, 2], -1);
    }

    #[test]
    fn bytes_push_appends_and_accepts_the_full_byte_range() {
        assert_eq!(push(vec![], 0), vec![0x00]);
        assert_eq!(push(vec![0xAA], 255), vec![0xAA, 0xFF]);
        assert_eq!(push(push(push(push(vec![], 0xB2), 0xD0), 0x5E), 0x00),
                   vec![0xB2, 0xD0, 0x5E, 0x00]);
    }

    /// The strict range check is what lets the M1 encode composition omit its own
    /// UPPER bound check: an over-range field produces a top byte of 256+, which
    /// must panic here rather than silently truncating to plausible wire bytes.
    #[test]
    #[should_panic(expected = "not a byte value")]
    fn bytes_push_rejects_256_rather_than_truncating() {
        push(vec![], 256);
    }

    #[test]
    #[should_panic(expected = "not a byte value")]
    fn bytes_push_rejects_negative() {
        push(vec![], -1);
    }

    /// The two atoms round-trip through each other for every byte value, which is
    /// the property the BE codec compositions are built on.
    #[test]
    fn bytes_get_push_round_trip_all_256_values() {
        for n in 0..=255i64 {
            let b = push(vec![], n);
            assert_eq!(b.len(), 1);
            assert_eq!(get(&b, 0), n, "round-trip failed for byte {}", n);
        }
    }

    /// Executable proof that the four width-named codecs are reproducible from
    /// the two atoms plus integer arithmetic ONLY — no bitwise op. These mirror
    /// the M1 compositions (be16/be32 encode/decode) line for line, so if this
    /// test passes, T12's "bitwise-free" prediction holds for the M1 side too.
    #[test]
    fn atoms_plus_arith_reproduce_all_four_width_codecs_without_bitwise() {
        // ENCODE: differential against the surviving width-named bridge fns.
        // Every value here is asserted against a real Postgres wire vector
        // elsewhere in this module, plus both width boundaries.
        for n in [0i64, 1, 3, 4, 23, 40000, 65535, 32767, -1, -32768] {
            assert_eq!(be16_enc(n), bytes(int16_be_encode(Value::Int(n))),
                       "be16 encode composition diverged at {}", n);
        }
        for n in [0i64, 8, 23, 196608, 3_000_000_000, 4_294_967_295, 2_147_483_647,
                  -1, -2_147_483_648] {
            assert_eq!(be32_enc(n), bytes(int32_be_encode(Value::Int(n))),
                       "be32 encode composition diverged at {}", n);
        }

        // DECODE: int16_be_decode / int32_be_decode are RETIRED, so the oracle is
        // std's own from_be_bytes rather than the deleted fn. That is a STRONGER
        // check than the differential it replaces: from_be_bytes is an independent
        // implementation, whereas the retired fn was itself a thin wrapper over it.
        // Exhaustive over all 65536 two-byte inputs, so the signed boundary at
        // 0x8000 cannot hide.
        for hi in 0..=255u8 {
            for lo in 0..=255u8 {
                assert_eq!(be16_dec(&[hi, lo]), i16::from_be_bytes([hi, lo]) as i64,
                           "be16 decode diverged at {:02X}{:02X}", hi, lo);
            }
        }
        // Exhaustive is 4.3e9 for 32-bit, so: every real Postgres wire vector,
        // both signed boundaries, and every 0x00/0x7F/0x80/0xFF byte pattern.
        for b in [[0x00, 0x00, 0x00, 0x08], [0xFF, 0xFF, 0xFF, 0xFF],
                  [0x00, 0x03, 0x00, 0x00], [0xB2, 0xD0, 0x5E, 0x00],
                  [0x7F, 0xFF, 0xFF, 0xFF], [0x80, 0x00, 0x00, 0x00]] {
            assert_eq!(be32_dec(&b), i32::from_be_bytes(b) as i64,
                       "be32 decode diverged at {:02X?}", b);
        }
        for p in [0x00u8, 0x7F, 0x80, 0xFF] {
            for q in [0x00u8, 0x7F, 0x80, 0xFF] {
                for r in [0x00u8, 0x7F, 0x80, 0xFF] {
                    for t in [0x00u8, 0x7F, 0x80, 0xFF] {
                        let b = [p, q, r, t];
                        assert_eq!(be32_dec(&b), i32::from_be_bytes(b) as i64,
                                   "be32 decode diverged at {:02X?}", b);
                    }
                }
            }
        }
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

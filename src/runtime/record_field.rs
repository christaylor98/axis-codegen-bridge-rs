//! AXVERITY_ZEROCOPY_READPATH_BUILD_V1 — SITE 4 of
//! design:axverity-readpath-gap-is-representation-not-guarantee: pg_field_value's
//! linear tab-delimited re-parse (lib/pg_field_value.m1:19-24).
//!
//! Two view-based variants of the turn-0015 RECORD field extractor, selected at
//! runtime by `AXVERITY_QHM_FIELD` (off | onepass | cached). `off` keeps the pure
//! M1 body (`str_after` then `str_before` — TWO allocations, the first O(rest of
//! record)); both variants are additive and flag-gated with the M1 body as the
//! always-available fallback.
//!
//! RECORD line: `"RECORD\t<c1>=<v1>\t<c2>=<v2>\t…"`. Format constraints
//! (turn-0015): field names contain no TAB and no `=`; values contain no TAB but
//! MAY contain `=` (name/value split on the FIRST `=`). A field is located by the
//! needle `"\t<field>="` in a TAB-prefixed record, so it matches only at a token
//! boundary and the trailing `=` rules out prefix matches ("color" vs "colorful").
//! Absent field => `""` (matches `str_after`'s `split_once(..).unwrap_or("")`).
//!
//! "Don't move the bytes until you know where they land" (Chris, 2026-07-22): the
//! true endpoint of an ORDER BY key extraction is ONE field, not the record, so
//! these extract a VIEW of just that field and materialise only it — no O(record)
//! intermediate `str_after` tail is ever allocated.

use std::cell::RefCell;
use std::sync::OnceLock;

use super::value::{intern_str, Value};

// ── mode flag ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FieldMode {
    Off,
    OnePass,
    Cached,
}

fn field_mode() -> FieldMode {
    static M: OnceLock<FieldMode> = OnceLock::new();
    *M.get_or_init(|| {
        match std::env::var("AXVERITY_QHM_FIELD")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "onepass" | "view" => FieldMode::OnePass,
            "cached" => FieldMode::Cached,
            _ => FieldMode::Off,
        }
    })
}

/// `record_field_mode(_: Unit) -> Text` — "off" | "onepass" | "cached". Lets
/// `pg_field_value` keep its exact M1 body under `off` and route to the bridge
/// extractor otherwise.
#[track_caller]
pub fn record_field_mode(_arg: Value) -> Value {
    let s = match field_mode() {
        FieldMode::Off => "off",
        FieldMode::OnePass => "onepass",
        FieldMode::Cached => "cached",
    };
    Value::Str(intern_str(s))
}

// ── core extraction (a VIEW into `rec`; byte-identical to pg_field_value.m1) ───

/// Return the value of `field` in `rec` as a borrowed slice of `rec`. One scan;
/// zero record-sized allocation (only the tiny needle is built). Absent => "".
fn extract<'a>(rec: &'a str, field: &str) -> &'a str {
    // A token boundary is start-of-record OR immediately after a TAB. So `field`
    // matches iff `rec` starts with "<field>=" (first token) or contains
    // "\t<field>=". We build only the small needle, never "\t"++rec.
    let mut head = String::with_capacity(field.len() + 1);
    head.push_str(field);
    head.push('='); // "<field>="

    let start = if rec.as_bytes().starts_with(head.as_bytes()) {
        head.len()
    } else {
        let mut tneedle = String::with_capacity(head.len() + 1);
        tneedle.push('\t');
        tneedle.push_str(&head); // "\t<field>="
        match rec.find(&tneedle) {
            Some(p) => p + tneedle.len(),
            None => return "",
        }
    };
    let tail = &rec[start..];
    match tail.find('\t') {
        Some(e) => &tail[..e], // value ends at the next token TAB
        None => tail,          // last field: to end of record
    }
}

/// `record_field(rec: Text, field: Text) -> Text` — the `onepass` variant. Reads
/// both args by BORROW (no `get_str` String clone of the record — site 2 applied
/// here too), scans once, materialises only the field value.
#[track_caller]
pub fn record_field(args: Value) -> Value {
    let (r, f) = two_args(args, "record_field");
    let rec = as_str(&r, "record_field", "rec");
    let field = as_str(&f, "record_field", "field");
    Value::Str(intern_str(extract(rec, field)))
}

// ── cached variant: thread-local parse-once memo ──────────────────────────────
//
// Parses ALL fields of a record ONCE into a per-thread single-slot memo keyed by
// a content fingerprint (fnv1a + len). A second field access on the SAME record
// is then O(#fields) with no re-scan. The memo is keyed by CONTENT (not pointer
// identity) so it can never alias a freed/reused Arc — sound, at the cost of an
// O(len) fingerprint per call. So this WINS only when several fields are read
// from the same record consecutively, and is expected to be ~even-or-worse than
// `onepass` for the single-field ORDER BY case — a "measure, don't assume" result
// the intent explicitly wants surfaced rather than pre-judged.

struct Memo {
    key: u64,
    len: usize,
    fields: Vec<(Box<str>, Box<str>)>,
}

thread_local! {
    static MEMO: RefCell<Option<Memo>> = const { RefCell::new(None) };
}

#[inline]
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in s.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn parse_all(rec: &str) -> Vec<(Box<str>, Box<str>)> {
    // Split on TAB; the first token is "RECORD" (no '='), skipped by the
    // find('=') guard. Each subsequent token is "<name>=<value>" (value may
    // contain '=', so split on the FIRST '=').
    let mut out = Vec::new();
    for tok in rec.split('\t') {
        if let Some(eq) = tok.find('=') {
            out.push((Box::from(&tok[..eq]), Box::from(&tok[eq + 1..])));
        }
    }
    out
}

/// `record_field_cached(rec: Text, field: Text) -> Text` — the `cached` variant.
#[track_caller]
pub fn record_field_cached(args: Value) -> Value {
    let (r, f) = two_args(args, "record_field_cached");
    let rec = as_str(&r, "record_field_cached", "rec");
    let field = as_str(&f, "record_field_cached", "field");
    let key = fnv1a(rec);
    let len = rec.len();
    MEMO.with(|m| {
        let mut slot = m.borrow_mut();
        let fresh = match slot.as_ref() {
            Some(e) if e.key == key && e.len == len => false,
            _ => true,
        };
        if fresh {
            *slot = Some(Memo {
                key,
                len,
                fields: parse_all(rec),
            });
        }
        let e = slot.as_ref().expect("memo just populated");
        let val = e
            .fields
            .iter()
            .find(|(n, _)| n.as_ref() == field)
            .map(|(_, v)| v.as_ref())
            .unwrap_or("");
        Value::Str(intern_str(val))
    })
}

// ── arg helpers (borrow, no get_str clone) ────────────────────────────────────

#[track_caller]
fn two_args(args: Value, who: &str) -> (Value, Value) {
    match args {
        Value::Tuple(es) if es.len() == 2 => {
            let mut it = es.into_iter();
            (it.next().unwrap(), it.next().unwrap())
        }
        other => panic!("{}: expected Tuple(Text, Text), got {:?}", who, other),
    }
}

#[track_caller]
fn as_str<'a>(v: &'a Value, who: &str, arg: &str) -> &'a str {
    match v {
        Value::Str(s) => s.as_ref(),
        other => panic!("{}: arg {} expected Text, got {:?}", who, arg, other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rf_onepass(rec: &str, field: &str) -> String {
        match record_field(Value::Tuple(vec![
            Value::Str(intern_str(rec)),
            Value::Str(intern_str(field)),
        ])) {
            Value::Str(s) => s.to_string(),
            _ => unreachable!(),
        }
    }

    fn rf_cached(rec: &str, field: &str) -> String {
        match record_field_cached(Value::Tuple(vec![
            Value::Str(intern_str(rec)),
            Value::Str(intern_str(field)),
        ])) {
            Value::Str(s) => s.to_string(),
            _ => unreachable!(),
        }
    }

    // The reference oracle: exactly what lib/pg_field_value.m1 computes.
    // padded = "\t"+rec; str_before(str_after(padded, "\t"+field+"="), "\t").
    fn oracle(rec: &str, field: &str) -> String {
        let padded = format!("\t{}", rec);
        let needle = format!("\t{}=", field);
        let after = match padded.split_once(&needle) {
            Some((_, a)) => a,
            None => "",
        };
        match after.split_once('\t') {
            Some((b, _)) => b.to_string(),
            None => after.to_string(),
        }
    }

    #[test]
    fn onepass_matches_oracle() {
        let cases: &[(&str, &str)] = &[
            ("RECORD\tcolor=red\tsize=large\tprice=030", "color"),
            ("RECORD\tcolor=red\tsize=large\tprice=030", "size"),
            ("RECORD\tcolor=red\tsize=large\tprice=030", "price"), // last field
            ("RECORD\tcolor=red\tsize=large\tprice=030", "missing"), // absent
            ("RECORD\tcolorful=x\tcolor=red", "color"),   // prefix must not match
            ("RECORD\tcolorful=x\tcolor=red", "colorful"),
            ("RECORD\texpr=a=b\tk=v", "expr"),            // value contains '='
            ("RECORD\tk=", "k"),                          // empty value
            ("RECORD\tonly=v", "only"),                   // single field
            ("RECORD", "color"),                          // no fields
            ("RECORD\tRECORD=trap\tk=v", "RECORD"),       // 'RECORD' as a real field name
        ];
        for (rec, field) in cases {
            assert_eq!(
                rf_onepass(rec, field),
                oracle(rec, field),
                "onepass mismatch rec={:?} field={:?}",
                rec,
                field
            );
        }
    }

    #[test]
    fn cached_matches_oracle_and_reuses() {
        let rec = "RECORD\tcolor=red\tsize=large\tprice=030\texpr=a=b";
        // multiple fields from the SAME record (the case cached is for)
        for field in ["color", "size", "price", "expr", "missing"] {
            assert_eq!(rf_cached(rec, field), oracle(rec, field), "field={}", field);
        }
        // switching records re-parses correctly
        let rec2 = "RECORD\tcolor=blue";
        assert_eq!(rf_cached(rec2, "color"), "blue");
        assert_eq!(rf_cached(rec, "color"), "red");
    }
}

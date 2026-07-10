use std::sync::{OnceLock, Mutex, Arc};
use std::collections::HashMap;
pub use rust_decimal::Decimal;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    // M1_VALUE_STR_ARC_IMPLEMENTATION_V1: Str now carries the string inline as
    // Arc<str> (cheap clone via atomic refcount, no shared interner). Replaces
    // the former Str(u32) handle into the global Mutex-guarded STRING_TABLE —
    // the Spike-4 P>=8 contention source on the str path. Arc<str> is Send+Sync
    // and derives Clone/Debug/PartialEq (PartialEq compares str contents, which
    // preserves the interner's equal-string => equal-value semantics).
    Str(Arc<str>),
    Unit,
    Tuple(Vec<Value>),
    List(Vec<Value>),
    Ctor { tag: u32, fields: Vec<Value> },
    // BRIDGE_VALUE_COERCION_V1: numeric tags for the to_dec / to_float family.
    // Dec is rust_decimal::Decimal (128-bit fixed decimal, ~28 significant digits).
    // Float is IEEE 754 f64. Float-to-Dec via `Decimal::from_f64_retain` preserves
    // significant digits within Decimal's range; values outside the range panic.
    Dec(Decimal),
    Float(f64),
    // BRIDGE_BYTES_IO_M1: opaque byte blob (PrimCode::Bytes=4). Carrier for
    // fs_read_bytes / fs_write_bytes / text_to_bytes. NOT a List<Int> — kept
    // as Vec<u8> so the bridge can pass blobs without per-element overhead.
    Bytes(Vec<u8>),
}

// M1_VALUE_STR_ARC_IMPLEMENTATION_V1 hard invariant (VALUE_MUST_STAY_SEND_SYNC):
// Value crosses thread boundaries via main.rs's `--entries` thread::spawn
// closures and channels.rs's Mutex<VecDeque<Value>>. Arc<str> is Send + Sync,
// so Value remains Send + Sync. This assertion makes that a compile-time gate:
// if any future field breaks it, the crate fails to build here.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Value>();
};

impl Value {
    #[track_caller]
    pub fn as_int(&self) -> i64 {
        match self {
            Value::Int(n) => *n,
            _ => panic!("expected Int, got {:?}", self),
        }
    }

    #[track_caller]
    pub fn as_bool(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            _ => panic!("expected Bool, got {:?}", self),
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Int(n)    => write!(f, "{}", n),
            Value::Bool(b)   => write!(f, "{}", b),
            Value::Str(h)    => write!(f, "{}", get_str(h)),
            Value::Unit      => write!(f, "()"),
            Value::Dec(d)    => write!(f, "{}", d),
            Value::Float(x)  => write!(f, "{}", x),
            Value::Tuple(es) => {
                write!(f, "(")?;
                for (i, e) in es.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", e)?;
                }
                write!(f, ")")
            }
            Value::List(es) => {
                write!(f, "[")?;
                for (i, e) in es.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", e)?;
                }
                write!(f, "]")
            }
            Value::Ctor { tag, fields } => {
                write!(f, "{}(", get_tag_name(*tag))?;
                for (i, fld) in fields.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", fld)?;
                }
                write!(f, ")")
            }
            Value::Bytes(bs) => write!(f, "<{} bytes>", bs.len()),
        }
    }
}

#[track_caller]
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b)    => *b,
        Value::Int(n)     => *n != 0,
        Value::Str(h)     => !get_str(h).is_empty(),
        Value::Unit       => false,
        Value::Tuple(es)  => !es.is_empty(),
        Value::List(es)   => !es.is_empty(),
        Value::Ctor { .. } => true,
        Value::Dec(d)     => !d.is_zero(),
        Value::Float(x)   => *x != 0.0,
        Value::Bytes(bs)  => !bs.is_empty(),
    }
}

// ── String values (M1_VALUE_STR_ARC_IMPLEMENTATION_V1) ───────────────────────
//
// Str values are carried inline as Arc<str>. There is no longer a global
// STRING_TABLE / STRING_MAP interner — that Mutex-guarded shared structure was
// the str-path contention source and is removed entirely (no shared, mutable,
// lock-guarded structure remains here). `intern_str` and `get_str` are retained
// only as thin constructor/accessor shims so the ~200 existing call sites keep
// compiling; the names are historical (there is no interning anymore).
//
// `intern_str` allocates a fresh Arc<str> from the given string. Callers that
// build a Value::Str do `Value::Str(intern_str(s))`, unchanged from before.

#[track_caller]
pub fn intern_str(s: &str) -> Arc<str> {
    Arc::from(s)
}

// `get_str` returns an owned String from anything str-like — an owned
// `Arc<str>` (from a by-value `Value::Str(h)` match), a `&Arc<str>` (from a
// by-reference match), a `&str`, or a `String`. The clone-on-read behavior of
// the old handle-based implementation is preserved: callers still receive a
// fresh owned String, so no aliasing/isolation assumptions change.
#[track_caller]
pub fn get_str<S: AsRef<str>>(s: S) -> String {
    s.as_ref().to_string()
}

// ── Tag table ────────────────────────────────────────────────────────────────

static TAG_TABLE: OnceLock<Mutex<Vec<String>>>         = OnceLock::new();
static TAG_MAP:   OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();

#[track_caller]
pub fn intern_tag(name: &str) -> u32 {
    let map = TAG_MAP.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = map.lock().unwrap();
    if let Some(&t) = map.get(name) { return t; }
    let tbl = TAG_TABLE.get_or_init(|| Mutex::new(Vec::new()));
    let mut tbl = tbl.lock().unwrap();
    let t = tbl.len() as u32;
    tbl.push(name.to_string());
    map.insert(name.to_string(), t);
    t
}

#[track_caller]
pub fn get_tag_name(tag: u32) -> String {
    let tbl = TAG_TABLE.get_or_init(|| Mutex::new(Vec::new()));
    let tbl = tbl.lock().unwrap();
    tbl.get(tag as usize).cloned().unwrap_or_else(|| "Unknown".to_string())
}

// ── Process args (captured once at startup) ──────────────────────────────────

static PROCESS_ARGS: OnceLock<Vec<String>> = OnceLock::new();

#[track_caller]
pub fn init_runtime() {
    PROCESS_ARGS.get_or_init(|| std::env::args().collect());
    // STRING_TABLE / STRING_MAP removed: Value::Str is inline Arc<str> now.
}

#[track_caller]
pub fn get_process_args() -> &'static Vec<String> {
    PROCESS_ARGS.get_or_init(|| std::env::args().collect())
}

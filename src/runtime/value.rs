use std::sync::{OnceLock, Mutex};
use std::collections::HashMap;
pub use rust_decimal::Decimal;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Str(u32),
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
}

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
            Value::Str(h)    => write!(f, "{}", get_str(*h)),
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
        }
    }
}

#[track_caller]
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b)    => *b,
        Value::Int(n)     => *n != 0,
        Value::Str(h)     => !get_str(*h).is_empty(),
        Value::Unit       => false,
        Value::Tuple(es)  => !es.is_empty(),
        Value::List(es)   => !es.is_empty(),
        Value::Ctor { .. } => true,
        Value::Dec(d)     => !d.is_zero(),
        Value::Float(x)   => *x != 0.0,
    }
}

// ── String table ────────────────────────────────────────────────────────────

static STRING_TABLE: OnceLock<Mutex<Vec<String>>>         = OnceLock::new();
static STRING_MAP:   OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();

#[track_caller]
pub fn intern_str(s: &str) -> u32 {
    let map = STRING_MAP.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = map.lock().unwrap();
    if let Some(&h) = map.get(s) { return h; }
    let tbl = STRING_TABLE.get_or_init(|| Mutex::new(vec!["".to_string()]));
    let mut tbl = tbl.lock().unwrap();
    let h = tbl.len() as u32;
    tbl.push(s.to_string());
    map.insert(s.to_string(), h);
    h
}

#[track_caller]
pub fn get_str(handle: u32) -> String {
    let tbl = STRING_TABLE.get_or_init(|| Mutex::new(vec!["".to_string()]));
    let tbl = tbl.lock().unwrap();
    tbl.get(handle as usize).cloned().unwrap_or_else(|| format!("<invalid-str-{}>", handle))
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
    STRING_TABLE.get_or_init(|| Mutex::new(vec!["".to_string()]));
    STRING_MAP.get_or_init(|| Mutex::new(HashMap::new()));
}

#[track_caller]
pub fn get_process_args() -> &'static Vec<String> {
    PROCESS_ARGS.get_or_init(|| std::env::args().collect())
}

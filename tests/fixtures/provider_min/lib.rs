//! Minimal fixture "provider crate" for the M1-provider `--provider-crate` /
//! `--dispatch` tests (tests/cli_build_05_test.rs). Stands in for a real
//! external crate like `axis_stdlib`: depends on `axis_codegen_bridge` for
//! `Value`, is compiled as its own rlib, and is linked into generated glue
//! by name via `--provider-crate provider_min=<this file>`.

use axis_codegen_bridge::runtime::value::Value;

pub fn prov_double(v: Value) -> Value {
    match v {
        Value::Int(n) => Value::Int(n * 2),
        other => panic!("prov_double: expected Int, got {:?}", other),
    }
}

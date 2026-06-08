extern crate axis_codegen_bridge;
use axis_codegen_bridge::runtime::value::{Value, init_runtime};

#[allow(dead_code)]
fn double(x: Value) -> Value {
    axis_codegen_bridge::runtime::arith::int_mul(Value::Tuple(vec![x, Value::Int(2)]))
}

#[allow(dead_code)]
fn greet(x: Value) -> Value {
    axis_codegen_bridge::runtime::io::io_println(axis_codegen_bridge::runtime::arith::int_to_str(x))
}

fn main() {
    init_runtime();
    let result = {
        let d = double(Value::Int(21));
        greet(d)
    };
    if !matches!(result, Value::Unit) { println!("{}", result); }
}

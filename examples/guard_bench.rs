// FAULT_AS_UNKNOWN prototype: the guard's cost on a hot path (no panic), vs the direct builtin call.
use axis_codegen_bridge::runtime::{arith, fault, str_ops, value::{Value, init_runtime}};
use std::hint::black_box;
use std::time::Instant;
use std::sync::Arc;

fn main() {
    init_runtime();
    let n: i64 = 20_000_000;
    for round in 0..3 {
        let t = Instant::now();
        let mut acc = Value::Int(0);
        for i in 0..n { acc = arith::int_add(black_box(acc.as_int()), black_box(i & 7)); }
        let direct = t.elapsed();
        let t = Instant::now();
        let mut acc2 = Value::Int(0);
        for i in 0..n { let a = acc2.clone(); let b = Value::Int(i & 7); acc2 = fault::guard("int_add", false, &[&a, &b], || arith::int_add(black_box(a.as_int()), black_box(b.as_int()))); }
        let guarded = t.elapsed();
        let s: Value = Value::Str(Arc::from("abc"));
        let t = Instant::now();
        for _ in 0..n / 10 { black_box(str_ops::str_concat(black_box(s.as_text()), black_box(s.as_text()))); }
        let sd = t.elapsed();
        let t = Instant::now();
        for _ in 0..n / 10 { black_box(fault::guard("str_concat", false, &[&s, &s], || str_ops::str_concat(black_box(s.as_text()), black_box(s.as_text())))); }
        let sg = t.elapsed();
        println!("round {round}: int_add direct {:.2}ns/call guarded {:.2}ns/call | str_concat direct {:.1}ns guarded {:.1}ns",
                 direct.as_nanos() as f64 / n as f64, guarded.as_nanos() as f64 / n as f64,
                 sd.as_nanos() as f64 / (n / 10) as f64, sg.as_nanos() as f64 / (n / 10) as f64);
        black_box((acc, acc2));
    }
}

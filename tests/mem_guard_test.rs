//! sys_mem_available / proc_rss: real numbers on Linux, and RSS grows when memory is touched.
use axis_codegen_bridge::runtime::process::{proc_rss, sys_mem_available};
use axis_codegen_bridge::runtime::value::Value;

fn int(v: Value) -> i64 { match v { Value::Int(n) => n, o => panic!("not Int: {o:?}") } }

#[test]
fn available_and_rss_are_real() {
    let avail = int(sys_mem_available(Value::Unit));
    let rss0 = int(proc_rss(Value::Unit));
    assert!(avail > 64 << 20, "MemAvailable {avail}");
    assert!(rss0 > 1 << 20, "VmRSS {rss0}");
    let mut v = vec![0u8; 256 << 20];
    for i in (0..v.len()).step_by(4096) { v[i] = 1; }
    let v = std::hint::black_box(v);
    let rss1 = int(proc_rss(Value::Unit));
    assert!(rss1 - rss0 > 200 << 20, "touching 256 MiB moved RSS only {} bytes", rss1 - rss0);
    drop(v);
}

//! slab_probe — AXVERITY_EXTENT_WRITE_PATH step 5, Q1 fact-finding.
//! Two handles on two directories, as S2's two streams would need. Prints
//! exactly what lands on disk.
use axis_codegen_bridge::runtime::slablock;
use axis_codegen_bridge::runtime::value::Value;
use std::sync::Arc;
fn h(v: Value) -> i64 { match v { Value::Int(n) => n, o => panic!("{:?}", o) } }
fn main() {
    let root = std::env::args().nth(1).expect("dir");
    let a = format!("{}/structure", root);
    let b = format!("{}/payload", root);
    let ha = h(slablock::slab_open(Arc::from(a.as_str()), 1, 1000));
    let hb = h(slablock::slab_open(Arc::from(b.as_str()), 1, 1000));
    // Enough to force several rotations in each, interleaved as the two
    // streams really do fill.
    for i in 0..6 {
        slablock::slab_append(ha, vec![b'S'; 600]);
        if i % 2 == 0 { slablock::slab_append(hb, vec![b'P'; 600]); }
    }
    slablock::slab_seal(ha);
    slablock::slab_seal(hb);
    for d in [&a, &b] {
        let mut v: Vec<String> = std::fs::read_dir(d).unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        println!("{:<12} {}", d.rsplit('/').next().unwrap(), v.join("  "));
    }
}

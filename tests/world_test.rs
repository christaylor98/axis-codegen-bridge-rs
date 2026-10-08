//! W1 world layer (axm-req-r99): every w_ fn in real, record and replay; replay touches nothing outside the process,
//! answers reads and runs from the trace, sees its own writes, and logs writes in order. One test: the world is one
//! per process, so the modes run in sequence.
use axis_codegen_bridge::runtime::value::Value;
use axis_codegen_bridge::runtime::world::*;
use std::sync::Arc;

fn t(s: &str) -> Arc<str> { Arc::from(s) }
fn txt(v: Value) -> String { match v { Value::Str(s) => s.to_string(), o => panic!("not Text: {o:?}") } }
fn boo(v: Value) -> bool { match v { Value::Bool(b) => b, o => panic!("not Bool: {o:?}") } }
fn run3(v: Value) -> (i64, String, String) {
    match v {
        Value::Ctor { fields, .. } => match &fields[..] {
            [Value::Int(c), Value::Str(o), Value::Str(e)] => (*c, o.to_string(), e.to_string()),
            f => panic!("bad record {f:?}"),
        },
        o => panic!("not a record: {o:?}"),
    }
}

/// The same little program, whatever the world: read, list, run, write, read back, append, state.
fn program(dir: &str) -> Vec<String> {
    let mut out = Vec::new();
    out.push(txt(w_read(t(&format!("{dir}/in.txt")))));
    out.push(txt(w_read(t(&format!("{dir}/nope.txt")))));
    out.push(boo(w_exists(t(&format!("{dir}/in.txt")))).to_string());
    out.push(txt(w_list(t(dir))));
    out.push(txt(w_list(t(&format!("{dir}/in.txt")))));
    let (c, o, _) = run3(w_run(t(&format!("cat {dir}/in.txt; exit 3"))));
    out.push(format!("{c}|{o}"));
    out.push(txt(w_write(t(&format!("{dir}/out.txt")), t("written"))));
    out.push(txt(w_read(t(&format!("{dir}/out.txt")))));
    out.push(txt(w_append(t(&format!("{dir}/out.txt")), t("+more"))));
    out.push(txt(w_read(t(&format!("{dir}/out.txt")))));
    out.push(txt(w_mkdir(t(&format!("{dir}/made/deep")))));
    w_print(t("printed "));
    w_set(t("m"), t("k"), t("v"));
    out.push(txt(w_get(t("m"), t("k"))));
    out
}

#[test]
fn real_record_replay() {
    let base = std::env::temp_dir().join(format!("world_test_{}", std::process::id()));
    let dir = base.to_string_lossy().to_string();
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("sub")).unwrap();
    std::fs::write(base.join("in.txt"), "hello\tworld\nline2").unwrap();

    // real
    world_set_mode("real");
    let real = program(&dir);
    assert_eq!(real[0], "Ok:hello\tworld\nline2");
    assert_eq!(real[1], "Err:missing");
    assert_eq!(real[2], "true");
    assert_eq!(real[3], "Ok:in.txt\nsub/");
    assert_eq!(real[4], "Err:not_a_dir");
    assert_eq!(real[5], "3|hello\tworld\nline2");
    assert_eq!(real[6], "Ok:");
    assert_eq!(real[7], "Ok:written");
    assert_eq!(real[9], "Ok:written+more");
    assert_eq!(real[10], "Ok:");
    assert!(base.join("made/deep").is_dir());
    assert_eq!(real[11], "v");

    // record: same answers, a trace of every outer-world call
    for p in ["out.txt", "made"] { let _ = std::fs::remove_dir_all(base.join(p)); let _ = std::fs::remove_file(base.join(p)); }
    let trace = format!("{dir}.trace.tsv");
    let _ = std::fs::remove_file(&trace);
    world_set_mode(&format!("record:{trace}"));
    let rec = program(&dir);
    assert_eq!(rec, real, "record answers what real does");
    let lines = std::fs::read_to_string(&trace).unwrap();
    assert_eq!(lines.lines().count(), 12, "12 outer-world calls recorded (state is not):\n{lines}");
    assert!(!lines.contains("w_get") && !lines.contains("w_set"));

    // replay: the outer world is gone and must not be touched
    std::fs::remove_file(base.join("in.txt")).unwrap();
    std::fs::remove_file(base.join("out.txt")).unwrap();
    std::fs::remove_dir_all(base.join("made")).unwrap();
    world_set_mode(&format!("replay:{trace}"));
    let rep = program(&dir);
    assert_eq!(rep, rec, "replay answers what record saw");
    assert!(!base.join("out.txt").exists() && !base.join("made").exists(), "replay wrote to the outer world");
    let writes = std::fs::read_to_string(format!("{trace}.out")).unwrap();
    let names: Vec<&str> = writes.lines().map(|l| l.split('\t').next().unwrap()).collect();
    assert_eq!(names, ["w_write", "w_append", "w_mkdir", "w_print"], "writes logged in order");

    // replay of a call the trace does not hold
    assert_eq!(txt(w_read(t(&format!("{dir}/never.txt")))), "Err:replay_miss");
    assert_eq!(run3(w_run(t("echo unrecorded"))).0, -1);
    // a write in replay is read back from the overlay
    w_write(t(&format!("{dir}/fresh.txt")), t("new"));
    assert_eq!(txt(w_read(t(&format!("{dir}/fresh.txt")))), "Ok:new");
    assert!(!base.join("fresh.txt").exists());

    world_set_mode("real");
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_file(&trace);
    let _ = std::fs::remove_file(format!("{trace}.out"));
}

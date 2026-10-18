//! L12: rand_bytes(n) -> n bytes from the OS CSPRNG.
use axis_codegen_bridge::runtime::process::rand_bytes;
use axis_codegen_bridge::runtime::Value;

fn bytes(v: Value) -> Vec<u8> {
    match v {
        Value::Bytes(b) => b.to_vec(),
        other => panic!("expected Bytes, got {:?}", other),
    }
}

#[test]
fn gives_the_length_asked_for() {
    assert_eq!(bytes(rand_bytes(0)).len(), 0);
    assert_eq!(bytes(rand_bytes(32)).len(), 32);
    assert_eq!(bytes(rand_bytes(100_000)).len(), 100_000);
}

#[test]
fn two_draws_differ() {
    assert_ne!(bytes(rand_bytes(32)), bytes(rand_bytes(32)));
}

#[test]
#[should_panic(expected = "negative length")]
fn a_negative_length_panics() {
    rand_bytes(-1);
}

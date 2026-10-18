//! runtime/http.rs: a request in, typed parts out, one reply.
use std::io::{Read, Write};
use std::sync::Arc;

use axis_codegen_bridge::runtime::http::*;
use axis_codegen_bridge::runtime::Value;

fn int(v: Value) -> i64 {
    match v { Value::Int(i) => i, o => panic!("expected Int, got {:?}", o) }
}
fn text(v: Value) -> String {
    match v { Value::Str(s) => s.to_string(), o => panic!("expected Str, got {:?}", o) }
}

#[test]
fn a_post_is_read_and_answered() {
    let s = int(http_listen(Arc::from("127.0.0.1"), 0));
    let port = int(http_port(s));
    assert!(port > 0);
    let client = std::thread::spawn(move || {
        let mut c = std::net::TcpStream::connect(("127.0.0.1", port as u16)).unwrap();
        c.write_all("POST /act?p=x HTTP/1.1\r\nHost: a\r\nCookie: axt=k\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhé=1".as_bytes()).unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        out
    });
    let r = int(http_next(s));
    assert_eq!(text(http_method(r)), "POST");
    assert_eq!(text(http_url(r)), "/act?p=x");
    assert_eq!(text(http_header(r, Arc::from("cookie"))), "axt=k");
    assert_eq!(text(http_header(r, Arc::from("x-none"))), "");
    assert_eq!(text(http_body(r)), "hé=1");
    http_reply(r, 303, Arc::from("Location: /\nSet-Cookie: a=b"), Arc::from("ok"));
    let out = client.join().unwrap();
    assert!(out.starts_with("HTTP/1.1 303"), "{}", out);
    assert!(out.contains("Location: /") && out.contains("Set-Cookie: a=b") && out.ends_with("ok"), "{}", out);
}

#[test]
#[should_panic(expected = "no request")]
fn a_request_is_answered_once() {
    http_reply(987654, 200, Arc::from(""), Arc::from(""));
}

//! HTTP server over tiny_http (approved by Chris 2026-10-18 for the axChat cockpit: "you add it in to the bridge
//! now"). The bridge parses and frames; the program decides. A request arrives as an id; the program reads its parts
//! with typed accessors and answers it once with `http_reply`.
//!
//!   http_listen(host: Text, port: Int) -> Int        a server handle; binds host:port (127.0.0.1 keeps it local; port 0 = any)
//!   http_port(server: Int) -> Int                    the port actually bound
//!   http_next(server: Int) -> Int                    blocks until the next request is parsed; its id
//!   http_method(req) / http_url(req) / http_body(req) -> Text
//!   http_header(req: Int, name: Text) -> Text        case-insensitive; "" when absent
//!   http_reply(req: Int, status: Int, headers: Text, body: Text) -> Unit
//!                                                    headers: "Name: value" lines; the request is then gone
//!
//! A handle or request id that does not exist panics: it is a program bug, not a runtime condition.
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::value::Value;

struct Req {
    method: Arc<str>,
    url: Arc<str>,
    headers: Vec<(String, String)>,
    body: Arc<str>,
    inner: tiny_http::Request,
}

fn servers() -> &'static Mutex<HashMap<i64, Arc<tiny_http::Server>>> {
    static S: OnceLock<Mutex<HashMap<i64, Arc<tiny_http::Server>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn requests() -> &'static Mutex<HashMap<i64, Req>> {
    static R: OnceLock<Mutex<HashMap<i64, Req>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT: AtomicI64 = AtomicI64::new(1);

fn server(h: i64) -> Arc<tiny_http::Server> {
    lock(servers()).get(&h).cloned().unwrap_or_else(|| panic!("http: no server {}", h))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())     // a panic elsewhere must not take the server down with it
}

fn with_req<T>(id: i64, f: impl FnOnce(&Req) -> T) -> T {
    let out = lock(requests()).get(&id).map(f);     // the lock is released before any panic below
    out.unwrap_or_else(|| panic!("http: no request {} (already answered?)", id))
}

#[track_caller]
pub fn http_listen(host: Arc<str>, port: i64) -> Value {
    if !(0..=65535).contains(&port) {
        panic!("http_listen: port {} out of range", port);
    }
    let s = tiny_http::Server::http((&*host, port as u16))
        .unwrap_or_else(|e| panic!("http_listen({}:{}): {}", host, port, e));
    let h = NEXT.fetch_add(1, Ordering::Relaxed);
    lock(servers()).insert(h, Arc::new(s));
    Value::Int(h)
}

#[track_caller]
pub fn http_port(h: i64) -> Value {
    Value::Int(server(h).server_addr().to_ip().map(|a| a.port() as i64).unwrap_or(0))
}

#[track_caller]
pub fn http_next(h: i64) -> Value {
    let s = server(h);
    let mut r = s.recv().unwrap_or_else(|e| panic!("http_next({}): {}", h, e));
    let mut body = Vec::new();
    // a body that cannot be read whole is answered 400 here and the next request is taken
    if let Err(e) = r.as_reader().read_to_end(&mut body) {
        let _ = r.respond(tiny_http::Response::from_string(format!("bad request body: {}", e)).with_status_code(400));
        return http_next(h);
    }
    let req = Req {
        method: Arc::from(r.method().as_str()),
        url: Arc::from(r.url()),
        headers: r.headers().iter().map(|x| (x.field.as_str().as_str().to_string(), x.value.as_str().to_string())).collect(),
        body: Arc::from(String::from_utf8_lossy(&body).as_ref()),
        inner: r,
    };
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    lock(requests()).insert(id, req);
    Value::Int(id)
}

#[track_caller]
pub fn http_method(id: i64) -> Value {
    Value::Str(with_req(id, |r| r.method.clone()))
}

#[track_caller]
pub fn http_url(id: i64) -> Value {
    Value::Str(with_req(id, |r| r.url.clone()))
}

#[track_caller]
pub fn http_body(id: i64) -> Value {
    Value::Str(with_req(id, |r| r.body.clone()))
}

#[track_caller]
pub fn http_header(id: i64, name: Arc<str>) -> Value {
    let v = with_req(id, |r| {
        r.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(&name)).map(|(_, v)| v.clone()).unwrap_or_default()
    });
    Value::Str(Arc::from(v.as_str()))
}

#[track_caller]
pub fn http_reply(id: i64, status: i64, headers: Arc<str>, body: Arc<str>) -> Value {
    let req = lock(requests()).remove(&id);
    let req = req.unwrap_or_else(|| panic!("http_reply: no request {}", id));
    let mut resp = tiny_http::Response::from_data(body.as_bytes().to_vec()).with_status_code(status as u16);
    for line in headers.lines() {
        if let Some((k, v)) = line.split_once(':') {
            match tiny_http::Header::from_bytes(k.trim().as_bytes(), v.trim().as_bytes()) {
                Ok(h) => resp.add_header(h),
                Err(_) => panic!("http_reply: bad header line {:?}", line),
            }
        }
    }
    // the client may have gone; that is its loss, not the server's
    let _ = req.inner.respond(resp);
    Value::Unit
}

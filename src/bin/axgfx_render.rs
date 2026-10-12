//! The stream renderer (F24 slice 3): a separate process that receives frames from a sim over a socket (runtime::gfx_stream) and draws them.
//!   axgfx_render --listen <host:port> [--ready <file>]      port 0 picks a free one; the address is written to the ready file once it listens. One sim session, then it exits.
//! The receive thread keeps ONLY THE NEWEST frame: a frame still waiting when a newer one arrives is dropped (a renderer that falls behind drops stale frames, D03), so the
//! frame drawn last is always the last one sent. Frames are judged by the same rules as in the sim (parse_frame, stale sequence numbers refused, an unknown op refuses the whole
//! frame); refusals and the window's events (key, close, resize, info, presented) go back to the sim as text lines.
//! Environment (TEST-ONLY hooks; none changes behaviour when unset): AXGFX_NULL=1 (no window), AXGFX_DUMP=<file> (every frame received, raw), AXGFX_DRAWLOG=<file> ("draw<TAB>seq<TAB>commands"
//! per frame drawn), AXGFX_RENDER_DELAY_MS=<n> (the draw of each frame takes n ms longer: a slow renderer).
use axis_codegen_bridge::runtime::gfx::{self, parse_frame, Frame, Null, Sink};
use axis_codegen_bridge::runtime::gfx_stream::{read_record, Record};
use std::collections::VecDeque;
use std::io::Write;
use std::net::TcpListener;
use std::sync::{Arc, Condvar, Mutex};

struct Slot {
    q: VecDeque<Frame>,
    done: bool,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let addr = arg("--listen").unwrap_or_else(|| "127.0.0.1:0".to_string());
    let listener = TcpListener::bind(&addr).expect("bind");
    if let Some(p) = arg("--ready") { std::fs::write(p, listener.local_addr().unwrap().to_string()).expect("ready file"); }
    let (stream, _) = listener.accept().expect("accept");
    let _ = stream.set_nodelay(true);
    let mut reader = stream.try_clone().expect("clone");
    let mut writer = stream.try_clone().expect("clone");
    let slot = Arc::new((Mutex::new(Slot { q: VecDeque::new(), done: false }), Condvar::new()));
    let sink: Arc<Mutex<Option<Box<dyn Sink>>>> = Arc::new(Mutex::new(None));
    let null = std::env::var("AXGFX_NULL").map_or(false, |v| v == "1");

    // events of the window (and refusals) go back to the sim
    let ev_stream = stream.try_clone().expect("clone");
    let finished = Arc::new(Mutex::new(false));
    let fin2 = finished.clone();
    let events = std::thread::spawn(move || {
        let mut w = ev_stream;
        loop {
            let done = *fin2.lock().unwrap();
            for e in gfx::drain_events() { if writeln!(w, "{}", e).is_err() { return; } }
            if done { return; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    });

    // the receive thread
    let (slot_r, sink_r) = (slot.clone(), sink.clone());
    let recv = std::thread::spawn(move || {
        let mut last: Option<u32> = None;
        gfx::dump_reset();
        loop {
            match read_record(&mut reader) {
                Ok(Record::Open { w, h, title }) => {
                    let k: Option<Box<dyn Sink>> = if null { Some(Box::new(Null(w, h, 0))) } else { gfx::open_window(w, h, &title) };
                    if k.is_none() { gfx::push_event("close".to_string()); }
                    *sink_r.lock().unwrap() = k;
                }
                Ok(Record::Frame(raw)) => {
                    gfx::dump_frame(&raw);
                    match parse_frame(&raw) {
                        Err(why) => gfx::push_event(format!("refused\t{}", why)),
                        Ok(f) => {
                            if last.map_or(false, |l| f.seq <= l) { gfx::push_event(format!("refused\tstale: sequence {} is not greater than {}", f.seq, last.unwrap())); continue; }
                            last = Some(f.seq);
                            let (m, cv) = &*slot_r;
                            let mut s = m.lock().unwrap();
                            s.q.clear();
                            s.q.push_back(f);
                            cv.notify_one();
                        }
                    }
                }
                Err(_) => break,
            }
        }
        let (m, cv) = &*slot_r;
        m.lock().unwrap().done = true;
        cv.notify_one();
    });

    // the draw loop: the newest frame only
    let delay = std::env::var("AXGFX_RENDER_DELAY_MS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let log = std::env::var("AXGFX_DRAWLOG").ok().filter(|p| !p.is_empty());
    let (m, cv) = &*slot;
    loop {
        let f = {
            let mut s = m.lock().unwrap();
            while s.q.is_empty() && !s.done { s = cv.wait(s).unwrap(); }
            match s.q.pop_front() { Some(f) => f, None => break }
        };
        if delay > 0 { std::thread::sleep(std::time::Duration::from_millis(delay)); }
        if f.cmds.is_empty() { continue; }
        if let Some(p) = log.as_ref().filter(|_| !null) { if let Ok(mut l) = std::fs::OpenOptions::new().create(true).append(true).open(p) { let _ = writeln!(l, "draw\t{}\t{}", f.seq, f.cmds.len()); } }
        if let Some(k) = sink.lock().unwrap().as_mut() {
            let mut raw = Vec::new();
            let _ = &mut raw;
            if let Some(e) = k.accept(&f, &raw) { gfx::push_event(e); }
        }
    }
    let _ = recv.join();
    *finished.lock().unwrap() = true;
    let _ = events.join();
    let _ = writer.flush();
    let last_sink = sink.lock().unwrap().take();
    if let Some(mut k) = last_sink { k.close(); }
}

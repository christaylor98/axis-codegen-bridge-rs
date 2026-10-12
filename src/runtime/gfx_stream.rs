//! F24 slice 3 — the frame stream over a socket. The sim side (`StreamSink`, selected by AXGFX_STREAM=host:port) sends the accepted frames to a renderer in
//! another process (bin axgfx_render); the renderer sends its events back as text lines (key, close, resize, info, presented, refused), which come out of gfx_poll.
//! The frame itself is unchanged (frame format version 1); only the stream carries a record around it:
//!   sim -> renderer:  u8 kind; kind 1 = open: u32 width, u32 height, u16 title length, title bytes; kind 2 = frame: u32 length, the frame bytes (all little-endian)
//!   renderer -> sim:  lines of text, "<event>\n" ("key<TAB>Escape", "close", "resize<TAB>w<TAB>h", "refused<TAB>reason", ...)
//! A dead renderer (connection closed or a failed write) is reported once as a `close` event, so the program ends the way it does when the window is closed.

use super::gfx::{push_event, Frame, Sink};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, Mutex};

pub enum Record {
    Open { w: u32, h: u32, title: String },
    Frame(Vec<u8>),
}

pub fn write_open(w: &mut impl Write, width: u32, height: u32, title: &str) -> std::io::Result<()> {
    let t = title.as_bytes();
    let t = &t[..t.len().min(65535)];
    let mut b = vec![1u8];
    b.extend_from_slice(&width.to_le_bytes());
    b.extend_from_slice(&height.to_le_bytes());
    b.extend_from_slice(&(t.len() as u16).to_le_bytes());
    b.extend_from_slice(t);
    w.write_all(&b)
}

pub fn write_frame(w: &mut impl Write, raw: &[u8]) -> std::io::Result<()> {
    let mut b = Vec::with_capacity(5 + raw.len());
    b.push(2u8);
    b.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    b.extend_from_slice(raw);
    w.write_all(&b)
}

/// One record, or Err at the end of the stream or on a malformed record.
pub fn read_record(r: &mut impl Read) -> std::io::Result<Record> {
    let mut k = [0u8; 1];
    r.read_exact(&mut k)?;
    match k[0] {
        1 => {
            let mut h = [0u8; 10];
            r.read_exact(&mut h)?;
            let (w, hh, n) = (u32::from_le_bytes([h[0], h[1], h[2], h[3]]), u32::from_le_bytes([h[4], h[5], h[6], h[7]]), u16::from_le_bytes([h[8], h[9]]) as usize);
            let mut t = vec![0u8; n];
            r.read_exact(&mut t)?;
            Ok(Record::Open { w, h: hh, title: String::from_utf8_lossy(&t).into_owned() })
        }
        2 => {
            let mut l = [0u8; 4];
            r.read_exact(&mut l)?;
            let n = u32::from_le_bytes(l) as usize;
            if n > 64 * 1024 * 1024 { return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "frame record too long")); }
            let mut f = vec![0u8; n];
            r.read_exact(&mut f)?;
            Ok(Record::Frame(f))
        }
        k => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("unknown record kind {}", k))),
    }
}

struct StreamSink {
    stream: TcpStream,
    size: Arc<Mutex<(u32, u32)>>,
    dead: bool,
    closing: Arc<Mutex<bool>>,
}

impl Sink for StreamSink {
    fn accept(&mut self, _frame: &Frame, raw: &[u8]) -> Option<String> {
        if self.dead { return None; }
        if write_frame(&mut self.stream, raw).is_err() { self.dead = true; return Some("close".to_string()); }
        None
    }
    fn size(&self) -> (u32, u32) { *self.size.lock().unwrap() }
    fn close(&mut self) {
        *self.closing.lock().unwrap() = true;
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

/// Connect to the renderer at `addr`, say what to open, and start the thread that turns the renderer's lines into events.
pub fn connect(addr: &str, w: u32, h: u32, title: &str) -> Result<Box<dyn Sink>, String> {
    let mut stream = TcpStream::connect(addr).map_err(|e| format!("stream renderer {}: {}", addr, e))?;
    let _ = stream.set_nodelay(true);
    write_open(&mut stream, w, h, title).map_err(|e| format!("stream renderer {}: {}", addr, e))?;
    let size = Arc::new(Mutex::new((w, h)));
    let closing = Arc::new(Mutex::new(false));
    let rd = stream.try_clone().map_err(|e| e.to_string())?;
    let (size2, closing2) = (size.clone(), closing.clone());
    std::thread::spawn(move || {
        for line in BufReader::new(rd).lines() {
            let line = match line { Ok(l) => l, Err(_) => break };
            if let Some(rest) = line.strip_prefix("resize\t") {
                let mut it = rest.split('\t');
                if let (Some(a), Some(b)) = (it.next().and_then(|v| v.parse().ok()), it.next().and_then(|v| v.parse().ok())) { *size2.lock().unwrap() = (a, b); }
            }
            push_event(line);
        }
        if !*closing2.lock().unwrap() { push_event("close".to_string()); }
    });
    Ok(Box::new(StreamSink { stream, size, dead: false, closing }))
}

//! F24 — the frame command stream (axMachina, approved by Chris 2026-10-12). A program draws by building a frame: a byte string of
//! drawing commands handed to the bridge. The frame is plain data (version 1: magic AXFR, version u16, sequence u32, command count
//! u32, then the commands: op u8 and arguments, f32 pixels with the origin at the top left, RGBA u32 as 0xRRGGBBAA; clear(rgba) is
//! op 1, rect(x, y, w, h, rgba) op 2). This module owns the frame RULES; the renderer only draws commands.
//!
//! Frame rules (axDisplay): a frame replaces the whole surface; a frame whose sequence number is not greater than the last accepted
//! one is refused as stale (D03); an empty frame (zero commands) draws nothing and is not an error; a frame with an op code the
//! renderer does not know (or a bad header, or truncated bytes) is refused WHOLE, never drawn in part.
//!
//! Foreign functions (declared in axis-bridge-gfx.axreg, bound in axis-bridge-gfx.dispatch.toml):
//!   gfx_open(width, height, title) -> Bool     false when no display is available (headless, or no renderer compiled in)
//!   gfx_submit(frame) -> Bool                  false when the frame is refused; the reason is a gfx_poll event
//!   gfx_poll() -> Text                         events since the last call, tab/line text: key<TAB>name, close, resize<TAB>w<TAB>h, refused<TAB>reason, info<TAB>...
//!   gfx_size() -> Text                         "width<TAB>height" of the drawing surface
//!   gfx_close() -> Unit
//!   clock_ms() -> Int                          a monotonic clock in milliseconds
//! Environment: AXGFX_DUMP=<file> appends every submitted frame to the file (the frame-dump option; the file is truncated at gfx_open);
//! AXGFX_NULL=1 makes gfx_open succeed without a window (a null renderer: the rules run, nothing is drawn; for tests and recording);
//! AXGFX_DRAWLOG=<file> makes the null renderer log every frame it is asked to draw.
//! AXGFX_NULL_ESCAPE_AFTER=<n> makes the null renderer report an Escape key press after its n-th frame.

use super::value::{BytesBuf, Value};
use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

const MAGIC: &[u8; 4] = b"AXFR";
const VERSION: u16 = 1;
const HEADER: usize = 14;
const OP_CLEAR: u8 = 1;
const OP_RECT: u8 = 2;

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    Clear(u32),
    Rect { x: f32, y: f32, w: f32, h: f32, rgba: u32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub seq: u32,
    pub cmds: Vec<Cmd>,
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Parse one whole frame; any fault refuses the whole frame (the reason is the Err text).
pub fn parse_frame(b: &[u8]) -> Result<Frame, String> {
    if b.len() < HEADER { return Err(format!("truncated header ({} bytes)", b.len())); }
    if &b[0..4] != MAGIC { return Err("bad magic".to_string()); }
    let version = u16::from_le_bytes([b[4], b[5]]);
    if version != VERSION { return Err(format!("unknown version {}", version)); }
    let seq = u32_at(b, 6);
    let count = u32_at(b, 10) as usize;
    let mut pos = HEADER;
    let mut cmds = Vec::new();
    for i in 0..count {
        if pos >= b.len() { return Err(format!("truncated: command {} of {} missing", i, count)); }
        match b[pos] {
            OP_CLEAR => {
                if pos + 5 > b.len() { return Err(format!("truncated clear at byte {}", pos)); }
                cmds.push(Cmd::Clear(u32_at(b, pos + 1)));
                pos += 5;
            }
            OP_RECT => {
                if pos + 21 > b.len() { return Err(format!("truncated rect at byte {}", pos)); }
                cmds.push(Cmd::Rect { x: f32_at(b, pos + 1), y: f32_at(b, pos + 5), w: f32_at(b, pos + 9), h: f32_at(b, pos + 13), rgba: u32_at(b, pos + 17) });
                pos += 21;
            }
            op => return Err(format!("unknown op {} at byte {}", op, pos)),
        }
    }
    if pos != b.len() { return Err(format!("{} trailing bytes after the last command", b.len() - pos)); }
    Ok(Frame { seq, cmds })
}

/// What draws the frames. The null renderer draws nothing; the wgpu window (feature gfx-wgpu) draws the commands.
pub trait Sink: Send {
    fn draw(&mut self, frame: &Frame) -> Option<String>;   // an event to report (the null renderer's Escape test), pushed by the caller: this runs under the state lock
    fn size(&self) -> (u32, u32);
    fn close(&mut self);
}

struct Null(u32, u32, u32);

impl Sink for Null {
    // AXGFX_DRAWLOG=<file>: one line per frame the null renderer was asked to draw ("draw<TAB>seq<TAB>commands"), so a test sees what was drawn
    fn draw(&mut self, frame: &Frame) -> Option<String> {
        // AXGFX_NULL_ESCAPE_AFTER=<n>: after the n-th frame the null renderer reports an Escape key press (the test of "Escape ends the run cleanly")
        self.2 += 1;
        let esc = std::env::var("AXGFX_NULL_ESCAPE_AFTER").ok().and_then(|v| v.parse::<u32>().ok()) == Some(self.2);
        if let Some(p) = std::env::var("AXGFX_DRAWLOG").ok().filter(|p| !p.is_empty()) {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) { let _ = writeln!(f, "draw\t{}\t{}", frame.seq, frame.cmds.len()); }
        }
        if esc { Some("key\tEscape".to_string()) } else { None }
    }
    fn size(&self) -> (u32, u32) { (self.0, self.1) }
    fn close(&mut self) {}
}

struct State {
    last_seq: Option<u32>,
    events: VecDeque<String>,
    sink: Option<Box<dyn Sink>>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State { last_seq: None, events: VecDeque::new(), sink: None }))
}

/// The renderer thread reports key, close, resize and info events here.
pub fn push_event(line: String) {
    state().lock().unwrap().events.push_back(line);
}

fn dump_path() -> Option<String> {
    std::env::var("AXGFX_DUMP").ok().filter(|p| !p.is_empty())
}

#[cfg(feature = "gfx-wgpu")]
fn open_window(w: u32, h: u32, title: &str) -> Option<Box<dyn Sink>> {
    match super::gfx_wgpu::open(w, h, title) {
        Ok(s) => Some(s),
        Err(why) => { push_event(format!("refused\topen: {}", why)); None }
    }
}

#[cfg(not(feature = "gfx-wgpu"))]
fn open_window(_w: u32, _h: u32, _title: &str) -> Option<Box<dyn Sink>> {
    push_event("refused\topen: no renderer compiled in (build the bridge with --features gfx-wgpu)".to_string());
    None
}

/// `gfx_open(width: Int, height: Int, title: Text) -> Bool`
pub fn gfx_open(width: i64, height: i64, title: std::sync::Arc<str>) -> Value {
    let (w, h) = (width.clamp(1, 16384) as u32, height.clamp(1, 16384) as u32);
    if let Some(p) = dump_path() { let _ = std::fs::write(p, b""); }
    let sink: Option<Box<dyn Sink>> = if std::env::var("AXGFX_NULL").map_or(false, |v| v == "1") { Some(Box::new(Null(w, h, 0))) } else { open_window(w, h, &title) };
    let mut s = state().lock().unwrap();
    s.last_seq = None;
    let ok = sink.is_some();
    s.sink = sink;
    Value::Bool(ok)
}

/// `gfx_submit(frame: Bytes) -> Bool`
pub fn gfx_submit(frame: BytesBuf) -> Value {
    if let Some(p) = dump_path() {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) { let _ = f.write_all(&frame); }
    }
    let mut s = state().lock().unwrap();
    if s.sink.is_none() { s.events.push_back("refused\tnot open".to_string()); return Value::Bool(false); }
    match parse_frame(&frame) {
        Err(why) => { s.events.push_back(format!("refused\t{}", why)); Value::Bool(false) }
        Ok(f) => {
            if s.last_seq.map_or(false, |last| f.seq <= last) {
                let last = s.last_seq.unwrap();
                s.events.push_back(format!("refused\tstale: sequence {} is not greater than {}", f.seq, last));
                return Value::Bool(false);
            }
            s.last_seq = Some(f.seq);
            if !f.cmds.is_empty() {
                let ev = s.sink.as_mut().and_then(|k| k.draw(&f));
                if let Some(e) = ev { s.events.push_back(e); }
            }
            Value::Bool(true)
        }
    }
}

/// `gfx_poll() -> Text` — the events since the last call, one per line, empty when none.
pub fn gfx_poll(_: Value) -> Value {
    let mut s = state().lock().unwrap();
    let mut out = String::new();
    while let Some(e) = s.events.pop_front() { out.push_str(&e); out.push('\n'); }
    Value::Str(out.into())
}

/// `gfx_size() -> Text` — "width<TAB>height", "0<TAB>0" when no surface is open.
pub fn gfx_size(_: Value) -> Value {
    let s = state().lock().unwrap();
    let (w, h) = s.sink.as_ref().map_or((0, 0), |k| k.size());
    Value::Str(format!("{}\t{}", w, h).into())
}

/// `gfx_close() -> Unit`
pub fn gfx_close(_: Value) -> Value {
    let mut s = state().lock().unwrap();
    if let Some(mut k) = s.sink.take() { k.close(); }
    s.last_seq = None;
    Value::Unit
}

/// `clock_ms() -> Int` — monotonic milliseconds since the first call in this process.
pub fn clock_ms(_: Value) -> Value {
    static T0: OnceLock<std::time::Instant> = OnceLock::new();
    let t0 = T0.get_or_init(std::time::Instant::now);
    Value::Int(t0.elapsed().as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(seq: u32, count: u32) -> Vec<u8> {
        let mut v = b"AXFR".to_vec();
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&seq.to_le_bytes());
        v.extend_from_slice(&count.to_le_bytes());
        v
    }

    #[test]
    fn empty_frame_parses() {
        assert_eq!(parse_frame(&head(5, 0)), Ok(Frame { seq: 5, cmds: vec![] }));
    }

    #[test]
    fn clear_and_rect_parse() {
        let mut f = head(1, 2);
        f.push(OP_CLEAR);
        f.extend_from_slice(&0x102030ffu32.to_le_bytes());
        f.push(OP_RECT);
        for x in [0.5f32, -1.0, 100.0, 12.25] { f.extend_from_slice(&x.to_le_bytes()); }
        f.extend_from_slice(&0xff0000ffu32.to_le_bytes());
        let fr = parse_frame(&f).unwrap();
        assert_eq!(fr.cmds, vec![Cmd::Clear(0x102030ff), Cmd::Rect { x: 0.5, y: -1.0, w: 100.0, h: 12.25, rgba: 0xff0000ff }]);
    }

    #[test]
    fn unknown_op_refuses_the_whole_frame() {
        let mut f = head(1, 2);
        f.push(OP_CLEAR);
        f.extend_from_slice(&1u32.to_le_bytes());
        f.push(99);
        assert!(parse_frame(&f).unwrap_err().contains("unknown op 99"));
    }

    #[test]
    fn truncated_and_trailing_and_bad_header_are_refused() {
        let mut f = head(1, 1);
        f.push(OP_RECT);
        assert!(parse_frame(&f).unwrap_err().contains("truncated rect"));
        let mut g = head(1, 0);
        g.push(0);
        assert!(parse_frame(&g).unwrap_err().contains("trailing"));
        assert!(parse_frame(b"AXF").is_err());
        let mut h = head(1, 0);
        h[0] = b'B';
        assert!(parse_frame(&h).unwrap_err().contains("magic"));
        let mut v = head(1, 0);
        v[4] = 2;
        assert!(parse_frame(&v).unwrap_err().contains("version"));
    }
}

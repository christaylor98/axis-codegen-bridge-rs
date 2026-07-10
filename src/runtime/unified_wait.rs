//! Unified wait — prototype (AXVERITY_UNIFIED_WAIT_V1 candidate).
//!
//! One sleep point per context, sources normalized at the edge. Each waiting
//! context (one per OS thread, matching the `--entries` driver's
//! thread-per-entry model) owns exactly one **inbox**
//! (`mpsc_intrusive::Queue<Value>` — many producers, one consumer) whose
//! parked-consumer wake word is the ONLY thing the context ever sleeps on.
//! Heterogeneous sources never teach the consumer new ways to sleep; each
//! source type gets an **adapter** that translates its native notification
//! scheme into `inbox.push(descriptor)`:
//!
//! * **channels** — producers push directly (they already run in normal
//!   thread context).
//! * **OS signals** — the self-pipe trick. HARD RULE: a signal handler may
//!   only call async-signal-safe functions, and `Queue::push` ALLOCATES
//!   (`Box::new` — malloc is not async-signal-safe), so a handler must NEVER
//!   push. The handler does exactly one `write(2)` of the signal number to a
//!   pipe; a dedicated adapter thread reads the pipe in normal context and
//!   does the push there.
//! * **hardware / fd sources** — userspace never sees an interrupt directly;
//!   the kernel delivers hardware as an fd (GPIO chardev, UIO, serial,
//!   socket). Prototype: one reader thread per subscribed fd translating
//!   reads into descriptors. Production consolidation is ONE epoll poller
//!   thread over all subscribed fds (and, further out, io_uring's
//!   FUTEX_WAIT ops would let the consumer sleep on the wake word and fd
//!   readiness in a single wait) — the descriptor normalization here is
//!   forward-compatible with both, since only the adapter's sleep site
//!   changes.
//! * **deadlines** — not a source thread at all: `uwait_deadline` swaps
//!   `park` for `park_timeout` (`Queue::pop_blocking_until`) and delivers a
//!   `Tick` descriptor on timeout, so a deadline is just another event the
//!   handler dispatches on.
//!
//! Descriptors are tagged Ctors over a small fixed vocabulary — exactly what
//! `intern_tag`/TAG_TABLE is scoped for:
//!
//!   ChannelMsg { name: Str, payload }   OsSignal { signum: Int }
//!   HwEdge { line: Str, seq: Int }      Tick { }
//!
//! `uwait` preserves `channels.rs`'s wait contract: CLOSURE_RULE_HARD (bare
//! `fn(Value) -> Value` handler, invoked once in `uwait`'s own frame, never
//! stored) and WAIT_ALWAYS_LIST (handler always receives a `Value::List`,
//! blocking until it is non-empty — for `uwait_deadline`, the `Tick` IS the
//! ≥1th element, so the list is still never empty).
//!
//! Status: Rust-level prototype. Not yet registry-exposed (no axreg entries,
//! no emit-time wiring) — that lands after the shape settles. The existing
//! `channels.rs` static-topology channels are untouched.

use super::mpsc_intrusive::{PopResult, Queue};
use super::value::{intern_str, intern_tag, Value};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

// ── Per-context inbox ────────────────────────────────────────────────────────

pub struct WaitCtx {
    inbox: Queue<Value>,
}

thread_local! {
    static CTX: RefCell<Option<Arc<WaitCtx>>> = const { RefCell::new(None) };
}

/// This thread's wait context, created on first touch. One context per OS
/// thread — the single-consumer contract on the inbox is enforced by
/// construction, because only this thread ever drains it.
fn my_ctx() -> Arc<WaitCtx> {
    CTX.with(|c| {
        c.borrow_mut()
            .get_or_insert_with(|| Arc::new(WaitCtx { inbox: Queue::new() }))
            .clone()
    })
}

// ── Descriptor constructors (small fixed tag vocabulary) ────────────────────

fn channel_msg(name: &str, payload: Value) -> Value {
    Value::Ctor {
        tag: intern_tag("ChannelMsg"),
        fields: vec![Value::Str(intern_str(name)), payload],
    }
}

fn os_signal(signum: i32) -> Value {
    Value::Ctor { tag: intern_tag("OsSignal"), fields: vec![Value::Int(signum as i64)] }
}

fn hw_edge(line: &str, seq: i64) -> Value {
    Value::Ctor {
        tag: intern_tag("HwEdge"),
        fields: vec![Value::Str(intern_str(line)), Value::Int(seq)],
    }
}

fn tick() -> Value {
    Value::Ctor { tag: intern_tag("Tick"), fields: vec![] }
}

// ── Channel source ───────────────────────────────────────────────────────────

/// Route for one named channel: who is waiting on it, plus messages that
/// arrived before anyone subscribed (drained into the inbox at subscribe
/// time, preserving the race-free send/subscribe ordering the static
/// channels already guarantee). The registry mutex is control-plane only —
/// the data-plane push happens after the guard is dropped.
struct ChannelRoute {
    subscriber: Option<Arc<WaitCtx>>,
    pending: VecDeque<Value>,
}

fn channels() -> &'static Mutex<HashMap<String, ChannelRoute>> {
    static REG: OnceLock<Mutex<HashMap<String, ChannelRoute>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Subscribe the calling thread's context to channel `name`. Messages sent
/// before subscription are delivered first, in send order.
pub fn uwait_subscribe_channel(name: &str) {
    let ctx = my_ctx();
    let mut reg = channels().lock().unwrap();
    let route = reg
        .entry(name.to_string())
        .or_insert_with(|| ChannelRoute { subscriber: None, pending: VecDeque::new() });
    for msg in route.pending.drain(..) {
        ctx.inbox.push(msg);
    }
    route.subscriber = Some(ctx);
}

/// Producer side: fire-and-forget, never blocks on the subscriber. The push
/// itself is lock-free; the registry lock is held only to resolve the route.
pub fn uwait_emit(name: &str, payload: Value) {
    let mut desc = Some(channel_msg(name, payload));
    let target = {
        let mut reg = channels().lock().unwrap();
        let route = reg
            .entry(name.to_string())
            .or_insert_with(|| ChannelRoute { subscriber: None, pending: VecDeque::new() });
        match &route.subscriber {
            Some(ctx) => Some(Arc::clone(ctx)),
            None => {
                route.pending.push_back(desc.take().unwrap());
                None
            }
        }
    };
    if let Some(ctx) = target {
        ctx.inbox.push(desc.take().unwrap());
    }
}

// ── OS-signal source (self-pipe adapter) ─────────────────────────────────────

/// Write end of the self-pipe, readable by the async-signal handler.
/// -1 until the adapter is initialized.
static SIG_WR: AtomicI32 = AtomicI32::new(-1);

/// The signal handler. HARD RULE: async-signal-safe calls ONLY. One raw
/// `write(2)` of the signal number — no allocation, no locks, no push, no
/// unpark. Everything else happens in the adapter thread.
extern "C" fn on_signal(signum: libc::c_int) {
    let fd = SIG_WR.load(Ordering::Relaxed);
    if fd >= 0 {
        let byte = signum as u8;
        unsafe {
            libc::write(fd, &byte as *const u8 as *const libc::c_void, 1);
        }
    }
}

fn signal_routes() -> &'static Mutex<HashMap<u8, Arc<WaitCtx>>> {
    static REG: OnceLock<Mutex<HashMap<u8, Arc<WaitCtx>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Spawn the process-wide signal adapter thread on first use: reads signal
/// numbers off the self-pipe in normal thread context and pushes `OsSignal`
/// descriptors to whichever context subscribed to that signal.
fn init_signal_adapter() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let mut fds = [0i32; 2];
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(rc, 0, "unified_wait: self-pipe creation failed");
        let (rd, wr) = (fds[0], fds[1]);
        SIG_WR.store(wr, Ordering::SeqCst);
        std::thread::Builder::new()
            .name("uwait-signal-adapter".to_string())
            .spawn(move || {
                let mut byte = 0u8;
                loop {
                    let n = unsafe {
                        libc::read(rd, &mut byte as *mut u8 as *mut libc::c_void, 1)
                    };
                    if n <= 0 {
                        // EINTR or pipe gone; EINTR just retries.
                        if n < 0 && std::io::Error::last_os_error().kind()
                            == std::io::ErrorKind::Interrupted
                        {
                            continue;
                        }
                        break;
                    }
                    let target = signal_routes().lock().unwrap().get(&byte).cloned();
                    if let Some(ctx) = target {
                        ctx.inbox.push(os_signal(byte as i32));
                    }
                }
            })
            .expect("unified_wait: failed to spawn signal adapter");
    });
}

/// Route OS signal `signum` to the calling thread's context and install the
/// (process-wide) self-pipe handler for it. One subscriber per signal in the
/// prototype — a later subscriber replaces the route.
pub fn uwait_subscribe_signal(signum: i32) {
    init_signal_adapter();
    signal_routes().lock().unwrap().insert(signum as u8, my_ctx());
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_signal as usize;
        sa.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut sa.sa_mask);
        let rc = libc::sigaction(signum, &sa, std::ptr::null_mut());
        assert_eq!(rc, 0, "unified_wait: sigaction({}) failed", signum);
    }
}

// ── Hardware / fd source (per-fd reader adapter) ─────────────────────────────

/// Route readable events on `fd` to the calling thread's context as
/// `HwEdge(line, seq)` descriptors — `seq` counts reads, standing in for a
/// device timestamp. Prototype shape: one reader thread per fd; the
/// production consolidation (one epoll poller thread for all fds) changes
/// only this adapter, not the descriptor or the consumer. The reader owns
/// `fd` and exits on EOF/error.
pub fn uwait_subscribe_fd_line(fd: i32, line: &str) {
    let ctx = my_ctx();
    let line = line.to_string();
    std::thread::Builder::new()
        .name(format!("uwait-fd-{}", line))
        .spawn(move || {
            let mut buf = [0u8; 256];
            let mut seq = 0i64;
            loop {
                let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, 256) };
                if n <= 0 {
                    if n < 0 && std::io::Error::last_os_error().kind()
                        == std::io::ErrorKind::Interrupted
                    {
                        continue;
                    }
                    unsafe { libc::close(fd) };
                    break;
                }
                seq += 1;
                ctx.inbox.push(hw_edge(&line, seq));
            }
        })
        .expect("unified_wait: failed to spawn fd reader");
}

// ── The wait itself ──────────────────────────────────────────────────────────

/// Drain everything currently in the inbox after the first item. An
/// `Inconsistent` result here is a producer mid-push — transient, retry;
/// only `Empty` ends the batch.
fn drain_rest(ctx: &WaitCtx, out: &mut Vec<Value>) {
    loop {
        match ctx.inbox.pop() {
            PopResult::Value(v) => out.push(v),
            PopResult::Inconsistent => std::thread::yield_now(),
            PopResult::Empty => break,
        }
    }
}

/// Block until ≥1 descriptor is available (parked at zero cost while idle),
/// drain the batch, call `handler(List)` once, return its result.
/// CLOSURE_RULE_HARD and WAIT_ALWAYS_LIST as in `channels.rs::wait`.
pub fn uwait(handler: fn(Value) -> Value) -> Value {
    let ctx = my_ctx();
    let first = ctx.inbox.pop_blocking();
    let mut drained = vec![first];
    drain_rest(&ctx, &mut drained);
    handler(Value::List(drained))
}

/// Deadline-bounded wait: block until ≥1 descriptor OR `deadline`. On
/// timeout the handler receives `List([Tick])` — the deadline is delivered
/// as an event, so the list is never empty and handlers dispatch on tags
/// uniformly. This is the semantic-loop composition: a lane that acts on
/// events when they come and ticks on schedule when they don't.
pub fn uwait_deadline(handler: fn(Value) -> Value, deadline: Instant) -> Value {
    let ctx = my_ctx();
    match ctx.inbox.pop_blocking_until(deadline) {
        Some(first) => {
            let mut drained = vec![first];
            drain_rest(&ctx, &mut drained);
            handler(Value::List(drained))
        }
        None => handler(Value::List(vec![tick()])),
    }
}

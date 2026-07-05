//! Static-topology channels + event subscription for H1/M1 concurrent process
//! loops (BRIDGE_ASYNC_PRIMITIVES_V1).
//!
//! Three foreign fns compose to express IPC between process loops that run on
//! separate execution contexts (threads, via the `--entries` driver):
//!
//!   * `channel_send(name, data)` — enqueue `data` on the named channel.
//!   * `event_subscribe(name)`    — this context will `wait` on the named channel.
//!   * `wait(handler)`            — block until a subscribed channel has data,
//!                                  then call `handler(List)` synchronously.
//!
//! ## Invariants (hard limits from the intent)
//!
//! * **CLOSURE_RULE_HARD** — `wait`'s handler is a bare `fn(Value) -> Value`
//!   pointer, moved into the call, invoked once inside `wait`'s own frame, and
//!   dropped when `wait` returns. Nothing (no struct field, thread-local, or
//!   global) stores it, so it cannot escape the frame or be re-invoked from a
//!   timer / interrupt / async context. The illegal state is unrepresentable:
//!   there is no storage path to abuse.
//!
//! * **WAIT_ALWAYS_LIST** — the handler's sole argument is always a
//!   `Value::List`, never a bare scalar, regardless of how many messages were
//!   drained (0 is not delivered — `wait` blocks until ≥1).
//!
//! * **CHANNELS_STATIC** — the set of legal channel names is fixed by the
//!   registry (`channel <name> … end`) and enforced at emit time: a
//!   `channel_send` to an undeclared name is a hard compile error (see
//!   `emit/rust_05.rs`), never a silent no-op. At runtime a channel's buffer is
//!   created on first touch by either endpoint, so send/subscribe ordering
//!   across contexts is race-free — the *names* are still compile-time literals,
//!   so this is not dynamic channel creation.

use super::value::{get_str, Value};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

/// A single channel's buffer. One `VecDeque` guarded by a mutex; senders push
/// the back, `wait` drains the front. Shared across contexts via `Arc`.
struct Channel {
    queue: Mutex<VecDeque<Value>>,
}

/// Process-global channel registry, keyed by declared channel name.
fn registry() -> &'static Mutex<HashMap<String, Arc<Channel>>> {
    static REG: OnceLock<Mutex<HashMap<String, Arc<Channel>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Return the buffer for `name`, creating an empty one on first touch.
///
/// Creation is keyed on a compile-time-literal name — the emit-time gate rejects
/// sends to names absent from the static `channel` declarations — so this is not
/// dynamic channel creation; the topology is fixed at build time. Get-or-create
/// (rather than reject-if-absent) makes send/subscribe ordering across contexts
/// race-free, exactly as the old `signals.rs` `OnceLock` channels were.
fn channel_for(name: &str) -> Arc<Channel> {
    let mut reg = registry().lock().unwrap();
    reg.entry(name.to_string())
        .or_insert_with(|| Arc::new(Channel { queue: Mutex::new(VecDeque::new()) }))
        .clone()
}

thread_local! {
    /// Channels the current execution context waits on. Per-context: the bridge
    /// owns subscriptions; H1 holds nothing between calls.
    static SUBSCRIPTIONS: RefCell<Vec<String>> = RefCell::new(Vec::new());
}

fn name_of(v: &Value) -> String {
    match v {
        Value::Str(h) => get_str(*h),
        other => panic!("channel name must be Text, got {:?}", other),
    }
}

/// `channel_send(name: Text, data: Value) -> Unit`.
///
/// Calling convention: unary `Value::Tuple([name, data])` (the data-fn convention
/// the emitter uses for 2-arg bridge fns).
#[track_caller]
pub fn channel_send(args: Value) -> Value {
    let (name, data) = match args {
        Value::Tuple(mut es) if es.len() == 2 => {
            let data = es.pop().unwrap();
            let name = es.pop().unwrap();
            (name_of(&name), data)
        }
        other => panic!("channel_send: expected Tuple(Text, Value), got {:?}", other),
    };
    let chan = channel_for(&name);
    chan.queue.lock().unwrap().push_back(data);
    Value::Unit
}

/// `event_subscribe(name: Text) -> Unit`. Registers the current context as a
/// waiter on `name` and declares the channel buffer eagerly. Idempotent per
/// context — subscribing twice to the same name is a no-op.
#[track_caller]
pub fn event_subscribe(name: Value) -> Value {
    let n = name_of(&name);
    let _ = channel_for(&n); // declare the buffer eagerly (race-free with senders)
    SUBSCRIPTIONS.with(|s| {
        let mut s = s.borrow_mut();
        if !s.iter().any(|c| c == &n) {
            s.push(n);
        }
    });
    Value::Unit
}

/// `wait(handler: Fn) -> Value`. Blocks until at least one subscribed channel
/// has a message, drains every currently-pending message across all subscribed
/// channels into a single `Value::List`, then calls `handler(list)`
/// synchronously and returns its result.
///
/// `handler` is a bare `fn(Value) -> Value` pointer (see CLOSURE_RULE_HARD in the
/// module docs): it is a local, used exactly once, and never stored.
#[track_caller]
pub fn wait(handler: fn(Value) -> Value) -> Value {
    // Snapshot the buffers this context subscribes to. `channel_for` is
    // get-or-create, so a subscribed-but-never-sent channel simply stays empty.
    let chans: Vec<Arc<Channel>> =
        SUBSCRIPTIONS.with(|s| s.borrow().iter().map(|n| channel_for(n)).collect());
    if chans.is_empty() {
        panic!("wait: current context has no subscriptions (call event_subscribe first)");
    }

    let mut drained: Vec<Value> = Vec::new();
    loop {
        for chan in &chans {
            let mut q = chan.queue.lock().unwrap();
            while let Some(v) = q.pop_front() {
                drained.push(v);
            }
        }
        if !drained.is_empty() {
            break;
        }
        std::thread::yield_now();
    }

    // Synchronous, in-frame invocation. WAIT_ALWAYS_LIST: the argument is a List.
    handler(Value::List(drained))
}

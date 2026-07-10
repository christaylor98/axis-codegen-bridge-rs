//! Unified-wait prototype demo: one context waiting on heterogeneous
//! sources — a channel, an OS signal (SIGUSR1 via the self-pipe adapter),
//! and an fd/"hardware" source (a pipe standing in for a GPIO line) — all
//! landing as tagged Ctor descriptors in ONE drained list from ONE sleep
//! point. Plus the deadline composition (`uwait_deadline` → `Tick`) and the
//! pre-subscription pending-delivery guarantee.

use axis_codegen_bridge::runtime::unified_wait::{
    uwait, uwait_deadline, uwait_emit, uwait_subscribe_channel, uwait_subscribe_fd_line,
    uwait_subscribe_signal,
};
use axis_codegen_bridge::runtime::value::{get_str, get_tag_name, intern_str, Value};
use std::collections::HashSet;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Identity handler — `uwait` returns the drained list itself.
fn take_list(v: Value) -> Value {
    v
}

fn tag_of(v: &Value) -> String {
    match v {
        Value::Ctor { tag, .. } => get_tag_name(*tag),
        other => panic!("expected Ctor descriptor, got {:?}", other),
    }
}

/// All three source kinds delivered to one waiting context. The consumer
/// thread subscribes (bindings are thread-local, so subscription must happen
/// on the waiting thread), then loops deadline-bounded waits until it has
/// seen a ChannelMsg, an OsSignal, and a HwEdge.
#[test]
fn heterogeneous_sources_one_wait_point() {
    let mut pipe_fds = [0i32; 2];
    assert_eq!(unsafe { libc::pipe(pipe_fds.as_mut_ptr()) }, 0);
    let (gpio_rd, gpio_wr) = (pipe_fds[0], pipe_fds[1]);

    let (ready_tx, ready_rx) = mpsc::channel::<()>();

    let consumer = thread::spawn(move || {
        uwait_subscribe_channel("cmds");
        uwait_subscribe_signal(libc::SIGUSR1);
        uwait_subscribe_fd_line(gpio_rd, "gpio0");
        ready_tx.send(()).unwrap();

        let mut kinds_seen: HashSet<String> = HashSet::new();
        let mut events: Vec<Value> = Vec::new();
        let overall_deadline = Instant::now() + Duration::from_secs(10);
        while kinds_seen.len() < 3 {
            assert!(Instant::now() < overall_deadline, "sources never all arrived: {:?}", kinds_seen);
            let batch = uwait_deadline(take_list, Instant::now() + Duration::from_secs(1));
            let Value::List(es) = batch else { panic!("wait must deliver a List") };
            assert!(!es.is_empty(), "WAIT_ALWAYS_LIST: list is never empty");
            for e in es {
                let t = tag_of(&e);
                if t != "Tick" {
                    kinds_seen.insert(t);
                    events.push(e);
                }
            }
        }
        events
    });

    ready_rx.recv().unwrap();

    // Channel source.
    uwait_emit("cmds", Value::Str(intern_str("advance")));
    // OS-signal source: raise on this thread; the handler self-pipes it to
    // the adapter, which routes it to the consumer's context.
    unsafe { libc::raise(libc::SIGUSR1) };
    // "Hardware" source: one edge on the gpio pipe.
    assert_eq!(unsafe { libc::write(gpio_wr, b"\x01".as_ptr() as *const libc::c_void, 1) }, 1);

    let events = consumer.join().unwrap();
    let tags: HashSet<String> = events.iter().map(tag_of).collect();
    assert!(tags.contains("ChannelMsg"), "missing ChannelMsg in {:?}", tags);
    assert!(tags.contains("OsSignal"), "missing OsSignal in {:?}", tags);
    assert!(tags.contains("HwEdge"), "missing HwEdge in {:?}", tags);

    for e in &events {
        if let Value::Ctor { tag, fields } = e {
            match get_tag_name(*tag).as_str() {
                "ChannelMsg" => {
                    assert_eq!(fields.len(), 2);
                    match (&fields[0], &fields[1]) {
                        (Value::Str(n), Value::Str(p)) => {
                            assert_eq!(get_str(n), "cmds");
                            assert_eq!(get_str(p), "advance");
                        }
                        other => panic!("bad ChannelMsg fields: {:?}", other),
                    }
                }
                "OsSignal" => {
                    assert_eq!(fields, &vec![Value::Int(libc::SIGUSR1 as i64)]);
                }
                "HwEdge" => match (&fields[0], &fields[1]) {
                    (Value::Str(line), Value::Int(seq)) => {
                        assert_eq!(get_str(line), "gpio0");
                        assert_eq!(*seq, 1);
                    }
                    other => panic!("bad HwEdge fields: {:?}", other),
                },
                t => panic!("unexpected tag {}", t),
            }
        }
    }

    unsafe { libc::close(gpio_wr) };
}

/// Messages sent before anyone subscribes are delivered first, in order —
/// the race-free send/subscribe guarantee carried over from channels.rs.
#[test]
fn pending_messages_delivered_after_subscribe_in_order() {
    uwait_emit("early", Value::Int(1));
    uwait_emit("early", Value::Int(2));
    uwait_emit("early", Value::Int(3));

    let consumer = thread::spawn(|| {
        uwait_subscribe_channel("early");
        uwait(take_list)
    });

    let Value::List(es) = consumer.join().unwrap() else { panic!("expected List") };
    let payloads: Vec<i64> = es
        .iter()
        .map(|e| match e {
            Value::Ctor { fields, .. } => match &fields[1] {
                Value::Int(n) => *n,
                other => panic!("bad payload {:?}", other),
            },
            other => panic!("expected Ctor, got {:?}", other),
        })
        .collect();
    assert_eq!(payloads, vec![1, 2, 3], "pending drained in send order");
}

/// A burst sent to an already-waiting context arrives as one batch, in
/// per-producer order (WAIT_ALWAYS_LIST batch semantics).
#[test]
fn burst_drains_as_single_batch() {
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let consumer = thread::spawn(move || {
        uwait_subscribe_channel("bulk");
        ready_tx.send(()).unwrap();
        // Collect until all 5 arrive (they may split across a couple of
        // batches depending on scheduling, but never reorder).
        let mut seen: Vec<i64> = Vec::new();
        while seen.len() < 5 {
            let Value::List(es) = uwait(take_list) else { panic!("expected List") };
            for e in es {
                if let Value::Ctor { fields, .. } = e {
                    if let Value::Int(n) = fields[1] {
                        seen.push(n);
                    }
                }
            }
        }
        seen
    });

    ready_rx.recv().unwrap();
    for n in 1..=5 {
        uwait_emit("bulk", Value::Int(n));
    }
    assert_eq!(consumer.join().unwrap(), vec![1, 2, 3, 4, 5]);
}

/// Deadline composition: with no sources firing, `uwait_deadline` delivers
/// `[Tick]` at ~the deadline; with an event in flight, the event wins and
/// arrives well before it.
#[test]
fn deadline_delivers_tick_or_event() {
    let consumer = thread::spawn(|| {
        uwait_subscribe_channel("quiet");

        // Nothing arrives: Tick at the deadline.
        let start = Instant::now();
        let Value::List(es) = uwait_deadline(take_list, start + Duration::from_millis(200))
        else { panic!("expected List") };
        let waited = start.elapsed();
        assert_eq!(es.len(), 1);
        assert_eq!(tag_of(&es[0]), "Tick");
        assert!(
            waited >= Duration::from_millis(190) && waited < Duration::from_secs(2),
            "tick at ~200ms, got {:?}",
            waited
        );

        // An event arrives: it wins, long before the deadline.
        let start = Instant::now();
        let Value::List(es) = uwait_deadline(take_list, start + Duration::from_secs(5))
        else { panic!("expected List") };
        let waited = start.elapsed();
        assert_eq!(tag_of(&es[0]), "ChannelMsg");
        assert!(waited < Duration::from_secs(2), "event should beat deadline, took {:?}", waited);
        waited
    });

    thread::sleep(Duration::from_millis(400));
    uwait_emit("quiet", Value::Int(7));
    let event_latency = consumer.join().unwrap();
    eprintln!("deadline test: event delivered {:?} after wait began", event_latency);
}

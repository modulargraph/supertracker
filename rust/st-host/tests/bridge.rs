//! The bridge's guarantees: one reply per request, on every path; order; concurrency; events.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Barrier, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use st_host::testing::TestHost;
use st_host::{Bridge, CallError, NoPayload, Next, Outbox, Reply, Result};

#[derive(Deserialize)]
#[serde(default)]
struct Add {
    a: i64,
    b: i64,
}

impl Default for Add {
    fn default() -> Self {
        Add { a: 0, b: 100 }
    }
}

fn add(payload: Add) -> Result<i64> {
    match payload.a.checked_add(payload.b) {
        Some(sum) => Ok(sum),
        None => st_host::fail!("The sum does not fit."),
    }
}

#[test]
fn a_typed_command_answers_with_its_result_or_its_error() {
    let host = TestHost::start(|bridge| bridge.commands().register("math.add", add));
    assert_eq!(host.request("math.add", json!({"a": 2, "b": 3})), Ok(json!(5)));
    assert_eq!(host.request("math.add", json!({"a": i64::MAX, "b": 1})), Err("The sum does not fit.".to_string()));
}

#[test]
fn missing_fields_default_and_unknown_fields_are_ignored() {
    let host = TestHost::start(|bridge| bridge.commands().register("math.add", add));
    assert_eq!(host.request("math.add", json!({"a": 1})), Ok(json!(101)));
    assert_eq!(host.request("math.add", json!({"a": 1, "b": 1, "later": {"addition": true}})), Ok(json!(2)));
    // No payload at all is an empty object.
    host.connection.receive(r#"{"id": 900, "command": "math.add"}"#);
    assert_eq!(host.reply(900), Ok(json!(100)));
}

#[test]
fn a_payload_that_does_not_fit_is_an_error_naming_the_command() {
    let host = TestHost::start(|bridge| bridge.commands().register("math.add", add));
    let error = host.request("math.add", json!({"a": "two"})).unwrap_err();
    assert!(error.starts_with("`math.add` was sent with an unusable payload: invalid type: string \"two\""), "{error}");
}

#[test]
fn unknown_and_unimplemented_commands_answer_with_an_error_naming_them() {
    let host = TestHost::start(|_| {});
    assert_eq!(
        host.request("plugin.load", json!({})),
        Err("`plugin.load` is not implemented in the Rust host yet (it belongs to st-audio).".to_string())
    );
    assert_eq!(host.request("made.up", json!({})), Err("`made.up` is not a command this host knows.".to_string()));
}

#[test]
fn a_request_without_a_command_is_answered_and_garbage_is_ignored() {
    let host = TestHost::start(|_| {});
    host.connection.receive("not json at all");
    host.connection.receive(r#"{"command": "no.id"}"#);
    host.connection.receive(r#"{"id": 41}"#);
    assert_eq!(host.reply(41), Err("The request does not name a command.".to_string()));
    assert_eq!(host.bridge.pending(), 0);
}

#[test]
fn every_request_gets_exactly_one_reply_under_concurrent_load() {
    let host = TestHost::start(|bridge| {
        bridge.commands().register("quick.echo", |value: Value| -> Result<Value> { Ok(value) });
        bridge.commands().register_blocking("slow.echo", |value: Value| -> Result<Value> {
            std::thread::sleep(Duration::from_millis(2));
            Ok(value)
        });
        bridge.commands().register("quick.fail", |_: NoPayload| -> Result<()> { st_host::fail!("no") });
    });
    let host = Arc::new(host);
    let total = 600u64;
    let senders: Vec<_> = (0..4u64)
        .map(|thread| {
            let host = host.clone();
            std::thread::spawn(move || {
                for index in 0..total / 4 {
                    let id = 10_000 + thread * 1_000 + index;
                    let command = ["quick.echo", "slow.echo", "quick.fail", "nobody.home"][(index % 4) as usize];
                    host.connection.receive(&json!({"id": id, "command": command, "payload": {"n": id}}).to_string());
                }
            })
        })
        .collect();
    for sender in senders {
        sender.join().unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut replies: Vec<Value> = Vec::new();
    while replies.len() < total as usize && Instant::now() < deadline {
        replies.extend(host.messages().into_iter().filter(|message| message["id"].as_u64().is_some_and(|id| id >= 10_000)));
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(50));
    replies.extend(host.messages().into_iter().filter(|message| message["id"].as_u64().is_some_and(|id| id >= 10_000)));
    let mut ids: Vec<u64> = replies.iter().map(|reply| reply["id"].as_u64().unwrap()).collect();
    ids.sort_unstable();
    let before = ids.len();
    ids.dedup();
    assert_eq!(before, total as usize, "one reply per request");
    assert_eq!(ids.len(), total as usize, "no request answered twice");
    for reply in &replies {
        let id = reply["id"].as_u64().unwrap();
        match (id % 1_000) % 4 {
            0 | 1 => assert_eq!(reply["result"], json!({"n": id})),
            2 => assert_eq!(reply["error"], "no"),
            _ => assert!(reply["error"].as_str().unwrap().contains("nobody.home")),
        }
    }
    assert_eq!(host.bridge.pending(), 0);
}

#[test]
fn quick_commands_run_in_arrival_order_while_a_blocking_command_waits() {
    let (release, wait) = mpsc::channel::<()>();
    let wait = Mutex::new(wait);
    let order = Arc::new(Mutex::new(Vec::new()));
    let log = order.clone();
    let host = TestHost::start(move |bridge| {
        bridge.commands().register_blocking("dialog.wait", move |_: NoPayload| -> Result<&'static str> {
            wait.lock().unwrap().recv_timeout(Duration::from_secs(10)).map_err(|_| st_host::Error::new("never released"))?;
            Ok("chosen")
        });
        bridge.commands().register("note", move |number: u32| -> Result<u32> {
            log.lock().unwrap().push(number);
            Ok(number)
        });
    });
    let dialog = host.send("dialog.wait", json!({}));
    let notes: Vec<u64> = (0..200).map(|number| host.send("note", json!(number))).collect();
    // All quick commands are answered while the "dialog" is still open.
    for (number, id) in notes.iter().enumerate() {
        assert_eq!(host.reply(*id), Ok(json!(number)));
    }
    assert_eq!(*order.lock().unwrap(), (0..200).collect::<Vec<u32>>());
    assert_eq!(host.bridge.pending(), 1);
    release.send(()).unwrap();
    assert_eq!(host.reply(dialog), Ok(json!("chosen")));
}

#[test]
fn blocking_commands_run_concurrently_with_each_other() {
    let barrier = Arc::new(Barrier::new(3));
    let meeting = barrier.clone();
    let host = TestHost::start(move |bridge| {
        bridge.commands().register_blocking("meet", move |_: NoPayload| -> Result<bool> {
            // Returns only when three requests are inside the handler at the same time.
            meeting.wait();
            Ok(true)
        });
    });
    let ids: Vec<u64> = (0..3).map(|_| host.send("meet", json!({}))).collect();
    for id in ids {
        assert_eq!(host.reply(id), Ok(json!(true)));
    }
}

#[test]
fn a_deferred_reply_answers_from_another_thread_and_an_abandoned_one_answers_with_an_error() {
    let parked: Arc<Mutex<Vec<Reply<String>>>> = Arc::default();
    let park = parked.clone();
    let host = TestHost::start(move |bridge| {
        bridge.commands().register_deferred("menu.show", move |_: NoPayload, reply: Reply<String>| park.lock().unwrap().push(reply));
    });
    let first = host.send("menu.show", json!({}));
    let second = host.send("menu.show", json!({}));
    let third = host.send("menu.show", json!({}));
    let deadline = Instant::now() + Duration::from_secs(5);
    while parked.lock().unwrap().len() < 3 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut replies: Vec<Reply<String>> = parked.lock().unwrap().drain(..).collect();
    assert_eq!(host.bridge.pending(), 3);
    let chosen = replies.remove(0);
    assert_eq!(chosen.command(), "menu.show");
    std::thread::spawn(move || chosen.ok("copy".to_string())).join().unwrap();
    replies.remove(0).error("The menu was dismissed.");
    drop(replies);
    assert_eq!(host.reply(first), Ok(json!("copy")));
    assert_eq!(host.reply(second), Err("The menu was dismissed.".to_string()));
    assert_eq!(host.reply(third), Err("`menu.show` ended without an answer. This is a bug in SuperTracker.".to_string()));
    assert_eq!(host.bridge.pending(), 0);
}

#[test]
fn a_handler_that_panics_still_answers_and_the_bridge_keeps_working() {
    // Release builds abort on panic; this covers builds where panics unwind (tests).
    let host = TestHost::start(|bridge| {
        bridge.commands().register("quick.panic", |_: NoPayload| -> Result<()> { panic!("handler bug") });
        bridge.commands().register_blocking("slow.panic", |_: NoPayload| -> Result<()> { panic!("handler bug") });
        bridge.commands().register("quick.fine", |_: NoPayload| -> Result<u8> { Ok(1) });
    });
    for command in ["quick.panic", "slow.panic"] {
        let error = host.request(command, json!({})).unwrap_err();
        assert_eq!(error, format!("`{command}` failed unexpectedly. This is a bug in SuperTracker."));
    }
    assert_eq!(host.request("quick.fine", json!({})), Ok(json!(1)));
}

#[test]
fn shutdown_answers_everything_that_was_still_waiting() {
    let (started_tx, started) = mpsc::channel::<()>();
    let started_tx = Mutex::new(started_tx);
    let release = Arc::new(AtomicBool::new(false));
    let released = release.clone();
    let parked: Arc<Mutex<Vec<Reply<()>>>> = Arc::default();
    let park = parked.clone();
    let ran = Arc::new(AtomicUsize::new(0));
    let counter = ran.clone();
    let host = TestHost::start(move |bridge| {
        bridge.commands().register_blocking("export.run", move |_: NoPayload| -> Result<&'static str> {
            started_tx.lock().unwrap().send(()).unwrap();
            while !released.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok("finished")
        });
        bridge.commands().register_deferred("menu.show", move |_: NoPayload, reply: Reply<()>| park.lock().unwrap().push(reply));
        bridge.commands().register("quick.count", move |_: NoPayload| -> Result<usize> { Ok(counter.fetch_add(1, Ordering::SeqCst)) });
    });
    let finishing = host.send("export.run", json!({}));
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let menu = host.send("menu.show", json!({}));
    assert_eq!(host.request("quick.count", json!({})), Ok(json!(0)));

    // The export finishes within the grace period: its real answer is delivered.
    let finisher = std::thread::spawn({
        let release = release.clone();
        move || {
            std::thread::sleep(Duration::from_millis(30));
            release.store(true, Ordering::SeqCst);
        }
    });
    host.bridge.shutdown(Duration::from_secs(5));
    finisher.join().unwrap();
    assert_eq!(host.reply(finishing), Ok(json!("finished")));
    // The menu nobody will answer any more, and a request that arrives after shutdown.
    assert_eq!(host.reply(menu), Err("SuperTracker shut down before `menu.show` finished.".to_string()));
    assert_eq!(host.request("quick.count", json!({})), Err("SuperTracker is shutting down.".to_string()));
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    assert_eq!(host.bridge.pending(), 0);
    // The late deferred answer is dropped, not sent as a second reply.
    parked.lock().unwrap().drain(..).for_each(|reply| reply.ok(()));
    assert!(host.messages().is_empty());
}

#[test]
fn shutdown_does_not_wait_forever_for_a_handler_that_never_returns() {
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let (started_tx, started) = mpsc::channel::<()>();
    let started_tx = Mutex::new(started_tx);
    let host = TestHost::start(move |bridge| {
        bridge.commands().register_blocking("hang", move |_: NoPayload| -> Result<()> {
            started_tx.lock().unwrap().send(()).unwrap();
            while !stopped.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(())
        });
    });
    let hung = host.send("hang", json!({}));
    let queued: Vec<u64> = (0..40).map(|_| host.send("hang", json!({}))).collect();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let began = Instant::now();
    host.bridge.shutdown(Duration::from_millis(50));
    assert!(began.elapsed() < Duration::from_secs(2));
    assert_eq!(host.reply(hung), Err("SuperTracker shut down before `hang` finished.".to_string()));
    for id in queued {
        let error = host.reply(id).unwrap_err();
        assert!(error == "SuperTracker is shutting down." || error == "SuperTracker shut down before `hang` finished.", "{error}");
    }
    assert_eq!(host.bridge.pending(), 0);
    stop.store(true, Ordering::SeqCst);
}

#[test]
fn events_reach_ready_interfaces_only_and_snapshots_coalesce() {
    let mut builder = Bridge::builder();
    let events = builder.events();
    builder.commands().register("ready", |_: NoPayload| -> Result<()> { Ok(()) });
    let bridge = builder.start();
    events.emit("too.early", json!({}));

    let loading = Outbox::new();
    let loading_page = bridge.connect(loading.clone());
    let ready = Outbox::new();
    let ready_page = bridge.connect(ready.clone());
    ready_page.receive(r#"{"id": 1, "command": "ready"}"#);
    assert_eq!(ready.wait(Duration::from_secs(5)), Next::Message(r#"{"id":1,"ok":true,"result":null}"#.to_string()));
    assert!(ready_page.is_ready() && !loading_page.is_ready() && bridge.has_ready_interface());

    // From a service thread, as the audio engine would.
    let sink = events.clone();
    std::thread::spawn(move || {
        sink.emit("plugins.changed", json!({"plugins": []}));
        for position in 0..500 {
            sink.emit_latest("status", json!({"position": position}));
        }
        sink.emit("export.finished", json!({"path": "/tmp/mix.wav"}));
    })
    .join()
    .unwrap();
    let texts = ready.drain();
    let received: Vec<Value> = texts.iter().map(|text| serde_json::from_str(text).unwrap()).collect();
    assert_eq!(
        received,
        [
            json!({"type": "plugins.changed", "payload": {"plugins": []}}),
            json!({"type": "status", "payload": {"position": 499}}),
            json!({"type": "export.finished", "payload": {"path": "/tmp/mix.wav"}}),
        ]
    );
    assert!(loading.is_empty(), "an interface that has not sent ready gets no events");

    // A detached interface gets nothing more, and nobody is left to listen.
    ready_page.close();
    events.emit("after.close", json!({}));
    assert!(ready.is_empty());
    assert!(!bridge.has_ready_interface());
}

#[test]
fn replies_go_to_the_interface_that_asked() {
    let mut builder = Bridge::builder();
    builder.commands().register("echo", |value: Value| -> Result<Value> { Ok(value) });
    let bridge = builder.start();
    let (first, second) = (Outbox::new(), Outbox::new());
    let (first_page, second_page) = (bridge.connect(first.clone()), bridge.connect(second.clone()));
    first_page.receive(r#"{"id": 1, "command": "echo", "payload": "first"}"#);
    second_page.receive(r#"{"id": 1, "command": "echo", "payload": "second"}"#);
    assert_eq!(first.wait(Duration::from_secs(5)), Next::Message(r#"{"id":1,"ok":true,"result":"first"}"#.to_string()));
    assert_eq!(second.wait(Duration::from_secs(5)), Next::Message(r#"{"id":1,"ok":true,"result":"second"}"#.to_string()));
}

#[test]
fn commands_can_call_each_other_through_the_table() {
    let mut builder = Bridge::builder();
    let calls = builder.calls();
    assert_eq!(calls.call("library.roots", json!({})), Err(CallError::Failed("The host has not finished starting.".to_string())));
    builder.commands().register("library.roots", |_: NoPayload| -> Result<Vec<&'static str>> { Ok(vec!["/samples"]) });
    builder.commands().register("library.broken", |_: NoPayload| -> Result<()> { st_host::fail!("The index is damaged.") });
    let inner = calls.clone();
    builder.commands().register_blocking("ready", move |_: NoPayload| -> Result<Value> {
        Ok(json!({
            "roots": inner.call("library.roots", json!({}))?,
            "plugins": inner.call("plugins.list", json!({})).unwrap_or(json!([])),
        }))
    });
    let bridge = builder.start();
    assert!(calls.implements("library.roots") && !calls.implements("plugins.list"));
    assert_eq!(calls.implemented(), ["library.broken", "library.roots", "ready"]);
    assert_eq!(calls.call("library.broken", json!({})), Err(CallError::Failed("The index is damaged.".to_string())));
    assert!(matches!(calls.call("plugins.list", json!({})), Err(CallError::NotImplemented(message)) if message.contains("plugins.list")));

    let page = Outbox::new();
    let connection = bridge.connect(page.clone());
    connection.receive(r#"{"id": 1, "command": "ready"}"#);
    assert_eq!(page.wait(Duration::from_secs(5)), Next::Message(r#"{"id":1,"ok":true,"result":{"plugins":[],"roots":["/samples"]}}"#.to_string()));
    assert_eq!(bridge.pending(), 0);
}

#[test]
#[should_panic(expected = "the command `x.y` is registered twice")]
fn registering_a_name_twice_is_caught_at_startup() {
    let mut builder = Bridge::builder();
    builder.commands().register("x.y", |_: NoPayload| -> Result<()> { Ok(()) });
    builder.commands().register_blocking("x.y", |_: NoPayload| -> Result<()> { Ok(()) });
}

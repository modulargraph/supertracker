//! The control socket from both ends: the client against a minimal stand-in for the application
//! (ported from tests/AgentMCPTests.swift, `socketRoundTrip`), and the real server against a
//! scripted page (the guarantees of tests/AgentConnectionTests.swift and of AgentControlServer).
#![cfg(unix)]

mod common;

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use common::{wait_for, FakePage, TempDir};
use serde_json::{json, Map, Value};
use st_agent::framing::{self, LineBuffer};
use st_agent::mcp::{catalogue, McpServer};
use st_agent::{ControlClient, ControlError, ControlServer, Page};

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

/// A minimal stand-in for the application's listener: answers each request line on a thread.
struct FakeApp {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    streams: Arc<Mutex<Vec<UnixStream>>>,
}

impl FakeApp {
    fn start(path: &Path) -> Self {
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path).expect("bind the fake application");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let streams = Arc::new(Mutex::new(Vec::new()));
        let (flag, kept) = (stop.clone(), streams.clone());
        thread::spawn(move || {
            for stream in listener.incoming() {
                if flag.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(stream) = stream else { return };
                kept.lock().unwrap().push(stream.try_clone().unwrap());
                thread::spawn(move || FakeApp::serve(stream));
            }
        });
        FakeApp { path: path.to_path_buf(), stop, streams }
    }

    fn serve(mut stream: UnixStream) {
        let mut buffer = LineBuffer::default();
        let mut chunk = [0u8; 4096];
        loop {
            let count = match stream.read(&mut chunk) {
                Ok(0) | Err(_) => return,
                Ok(count) => count,
            };
            for line in buffer.append(&chunk[..count]) {
                let Ok(message) = framing::decode(&line) else { continue };
                let id = message.get("id").cloned().unwrap_or(Value::Null);
                let method = message.get("method").and_then(Value::as_str).unwrap_or("");
                let response = if method == "boom" {
                    framing::error_response(Some(&id), -32000, "Unknown method \"boom\".")
                } else {
                    json!({"jsonrpc": "2.0", "id": id, "result": {"method": method, "params": message.get("params").cloned().unwrap_or(json!({}))},
                        "notices": if method == "noisy" { json!(["Saved"]) } else { json!([]) }})
                };
                // Sent in two writes to exercise reassembly.
                let data = framing::encode(&response);
                let half = data.len() / 2;
                let _ = stream.write_all(&data[..half]);
                thread::sleep(Duration::from_millis(2));
                let _ = stream.write_all(&data[half..]);
            }
        }
    }

    fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
        for stream in self.streams.lock().unwrap().drain(..) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        let _ = UnixStream::connect(&self.path); // wakes the accept loop
        let _ = std::fs::remove_file(&self.path);
    }
}

#[test]
fn client_round_trip_reconnect_and_offline_message() {
    let folder = TempDir::new("client");
    let path = folder.socket();
    let mut absent = ControlClient::new(&path);
    match absent.call("summary", Map::new()) {
        Err(error @ ControlError::NotRunning(_)) => assert!(error.to_string().contains("Open SuperTracker first"), "the not-running message tells the user what to do"),
        other => panic!("calls fail when the app is not running: {other:?}"),
    }

    let app = FakeApp::start(&path);
    let mut client = ControlClient::new(&path);
    let first = client.call("summary", object(json!({"a": 1}))).unwrap();
    assert_eq!(first.result["method"], "summary", "the request reaches the app and the result returns");
    assert_eq!(first.result["params"]["a"], 1, "params travel intact");
    assert_eq!(client.call("noisy", Map::new()).unwrap().notices, ["Saved"], "app notices are returned");
    match client.call("boom", Map::new()) {
        Err(error) => assert_eq!(error.to_string(), "Unknown method \"boom\".", "remote error text is preserved"),
        Ok(_) => panic!("remote errors are errors"),
    }
    assert!(client.is_connected(), "one persistent connection");

    // MCP over the real socket client.
    {
        let mut server = McpServer::new(|method, params| client.call(method, params).map(|outcome| (outcome.result, outcome.notices)));
        let response = server.handle(&object(json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": "get_song", "arguments": {}}}))).unwrap();
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"method\":\"summary\""), "get_song proxies to summary over the socket: {text}");
    }

    // The application restarts: the client reconnects once.
    app.stop();
    let restarted = FakeApp::start(&path);
    thread::sleep(Duration::from_millis(20));
    let again = client.call("uiState", Map::new()).unwrap();
    assert_eq!(again.result["method"], "uiState", "the client reconnects after the app restarts");
    restarted.stop();
    thread::sleep(Duration::from_millis(20));

    let mut offline = McpServer::new(|method, params| ControlClient::new(&path).call(method, params).map(|outcome| (outcome.result, outcome.notices)));
    let result = offline.call_tool(catalogue().tool("get_song").unwrap(), Map::new());
    assert_eq!(result["isError"], true);
    assert!(result["content"][0]["text"].as_str().unwrap().contains("Open SuperTracker first"), "tools explain how to start the app");
}

#[test]
fn a_client_gives_up_when_the_application_does_not_answer() {
    let folder = TempDir::new("timeout");
    let path = folder.socket();
    let listener = UnixListener::bind(&path).unwrap();
    let held = thread::spawn(move || listener.accept().map(|(stream, _)| stream));
    let mut client = ControlClient::new(&path);
    let started = Instant::now();
    let outcome = client.call_within("summary", Map::new(), Duration::from_millis(150));
    assert_eq!(outcome, Err(ControlError::Remote("SuperTracker did not answer in time.".into())));
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(!client.is_connected(), "a connection with an unanswered request is not reused");
    drop(held.join());
}

fn started(page: &Arc<FakePage>, folder: &TempDir) -> ControlServer {
    let server = ControlServer::new(folder.socket(), page.clone() as Arc<dyn Page>);
    server.start().expect("the control socket opens");
    server
}

fn request(stream: &mut UnixStream, message: Value) -> Value {
    stream.write_all(&framing::encode(&message)).unwrap();
    read_message(stream).expect("an answer")
}

fn read_message(stream: &mut UnixStream) -> Option<Value> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(1) if byte[0] == b'\n' => return serde_json::from_slice(&line).ok(),
            Ok(1) => line.push(byte[0]),
            _ => return None,
        }
    }
}

#[test]
fn the_server_runs_requests_through_the_page() {
    let folder = TempDir::new("server");
    let page = FakePage::new();
    let server = started(&page, &folder);
    assert_eq!(std::fs::metadata(folder.socket()).unwrap().permissions().mode() & 0o777, 0o600, "the socket is user-only");
    assert!(server.state().listening);

    let mut client = ControlClient::new(folder.socket());
    let echo = client.call("readPattern", object(json!({"pattern": 2}))).unwrap();
    assert_eq!(echo.result["method"], "readPattern");
    assert_eq!(echo.result["params"]["pattern"], 2);
    assert_eq!(echo.result["params"]["_host"], json!({"projectPath": null}), "the page learns the project's file");
    wait_for("the client to be counted", || server.state().clients == 1);

    assert_eq!(client.call("noisy", Map::new()).unwrap().notices, ["Saved"]);
    assert_eq!(
        client.call("boom", Map::new()),
        Err(ControlError::Remote("Unknown method \"boom\".\nApp message: Careful".into())),
        "page errors carry the application's messages"
    );
    assert_eq!(client.call("garbage", Map::new()), Err(ControlError::Remote("The interface returned an unreadable answer.".into())));
    assert_eq!(client.call("offline", Map::new()), Err(ControlError::Remote("The interface could not run offline: the interface went away.".into())));

    // Host-only methods never reach the page.
    let before = page.methods().len();
    let ping = client.call("app.ping", Map::new()).unwrap().result;
    assert_eq!(ping["app"], "SuperTracker");
    assert_eq!(ping["pid"], std::process::id());
    assert_eq!(ping["socket"], folder.socket().to_string_lossy().as_ref());
    assert!(ping["project"].is_null());
    let shot = client.call("app.screenshot", object(json!({"width": 99999}))).unwrap().result;
    assert_eq!(shot["width"], 2560, "the width is limited to 320–2560");
    assert_eq!(page.methods().len(), before);

    // Saving a session that has no file needs a path.
    assert_eq!(
        client.call("save", Map::new()),
        Err(ControlError::Remote("This session has never been saved. Pass path (an absolute path ending in .supertracker).".into()))
    );
    assert!(client.call("save", object(json!({"path": "/tmp/x.supertracker"}))).is_ok());
    *page.project.lock().unwrap() = Some("/music/song.supertracker".into());
    let saved = client.call("save", Map::new()).unwrap();
    assert_eq!(saved.result["params"]["_host"]["projectPath"], "/music/song.supertracker");
    assert_eq!(client.call("app.ping", Map::new()).unwrap().result["project"], "/music/song.supertracker");

    *page.connected.lock().unwrap() = false;
    assert_eq!(client.call("summary", Map::new()), Err(ControlError::Remote("The SuperTracker window is not open.".into())));
    *page.connected.lock().unwrap() = true;

    // Malformed requests are answered, never executed.
    let mut raw = UnixStream::connect(folder.socket()).unwrap();
    let before = page.methods().len();
    assert_eq!(request(&mut raw, json!({"jsonrpc": "2.0", "id": 1}))["error"]["message"], "Missing method.");
    assert_eq!(request(&mut raw, json!({"jsonrpc": "2.0", "id": 2, "method": "undo", "params": []}))["error"]["message"], "params must be a JSON object when provided.");
    raw.write_all(b"[1,2]\n").unwrap();
    let invalid = read_message(&mut raw).unwrap();
    assert!(invalid["id"].is_null() && invalid["error"]["code"] == -32000 && invalid["error"]["message"] == "Expected one JSON object per line.");
    assert_eq!(page.methods().len(), before);
    // Requests on one connection are answered in the order they were sent.
    raw.write_all(b"{\"id\":10,\"method\":\"a\"}\n{\"id\":11,\"method\":\"b\"}\r\n\n{\"id\":12,\"method\":\"c\"}\n").unwrap();
    let ids: Vec<Value> = (0..3).map(|_| read_message(&mut raw).unwrap()["id"].clone()).collect();
    assert_eq!(ids, [json!(10), json!(11), json!(12)]);
    assert_eq!(server.respond(b"{\"id\":5,\"method\":\"echo\"}").last(), Some(&b'\n'), "one request line in, one response line out");

    drop(raw);
    client.disconnect();
    wait_for("the clients to be gone", || server.state().clients == 0);
    server.stop();
    assert!(!folder.socket().exists(), "stopping removes the socket");
}

#[test]
fn a_second_application_does_not_take_over_a_live_socket_but_replaces_a_stale_one() {
    let folder = TempDir::new("second");
    let page = FakePage::new();
    let first = started(&page, &folder);
    let second = ControlServer::new(folder.socket(), page.clone() as Arc<dyn Page>);
    assert_eq!(second.start(), Err(ControlError::Transport("Another SuperTracker window is already accepting AI control.".into())));
    assert_eq!(second.state().error.as_deref(), Some("Another SuperTracker window is already accepting AI control."));
    assert!(!second.state().listening);
    assert!(ControlClient::new(folder.socket()).call("echo", Map::new()).is_ok(), "the first application keeps its socket");
    first.stop();

    // A socket file nobody listens on (a crashed application) is replaced.
    drop(UnixListener::bind(folder.socket()).unwrap());
    assert!(folder.socket().exists());
    assert_eq!(second.start(), Ok(()));
    assert_eq!(second.state().error, None);
    assert!(ControlClient::new(folder.socket()).call("echo", Map::new()).is_ok());
    second.stop();
}

#[test]
fn turning_ai_control_off_closes_every_connection_at_once() {
    let folder = TempDir::new("off");
    let page = FakePage::new();
    let server = started(&page, &folder);
    let mut idle = UnixStream::connect(folder.socket()).unwrap();
    let mut busy = UnixStream::connect(folder.socket()).unwrap();
    busy.write_all(b"{\"id\":1,\"method\":\"slow\"}\n").unwrap();
    page.wait_slow();
    wait_for("both clients", || server.state().clients == 2);

    let started = Instant::now();
    server.set_enabled(false);
    server.stop();
    assert!(started.elapsed() < Duration::from_millis(500), "stopping waits for nothing, not even a running request");
    assert_eq!(server.state().clients, 0);
    assert!(!server.state().listening);
    for stream in [&mut idle, &mut busy] {
        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut byte = [0u8; 1];
        assert_eq!(stream.read(&mut byte).unwrap(), 0, "the client sees the connection end");
    }
    assert!(matches!(ControlClient::new(folder.socket()).call("echo", Map::new()), Err(ControlError::NotRunning(_))));
    // The request that was running finishes in the page; its answer goes nowhere and nothing hangs.
    page.release();
    assert!(server.wait_idle(Duration::from_secs(5)));
    // While off, requests are refused even if one could still be delivered.
    let refused: Value = serde_json::from_slice(&server.respond(b"{\"id\":7,\"method\":\"undo\"}")).unwrap();
    assert_eq!(refused["error"]["message"], "AI control is turned off in SuperTracker Settings.");
    assert_eq!(refused["id"], 7);
    assert_eq!(server.start(), Ok(()), "starting while off is a quiet no-op");
    assert!(!server.state().listening);

    server.set_enabled(true);
    server.start().unwrap();
    assert!(ControlClient::new(folder.socket()).call("echo", Map::new()).is_ok(), "and it opens again when turned on");
    server.stop();
}

/// A client that never reads its answers costs only its own thread: the application's side
/// (`stop`, other clients) is never held up, and nothing is written after the connection is closed.
#[test]
fn a_stalled_client_blocks_nothing() {
    let folder = TempDir::new("stall");
    let page = FakePage::new();
    let server = started(&page, &folder);
    for _ in 0..3 {
        let mut stalled = UnixStream::connect(folder.socket()).unwrap();
        // Megabytes of answers the client never reads: the server's writer fills the socket and
        // waits, and then stops reading, so the requests are sent from a thread of their own.
        let mut sender = stalled.try_clone().unwrap();
        thread::spawn(move || {
            let padding = "x".repeat(256 * 1024);
            for id in 0..16 {
                if sender.write_all(&framing::encode(&json!({"id": id, "method": "echo", "params": {"padding": padding}}))).is_err() {
                    return;
                }
            }
        });
        wait_for("the stalled client", || server.state().clients >= 1);
        thread::sleep(Duration::from_millis(30));

        // Another client is served meanwhile.
        let mut other = ControlClient::new(folder.socket());
        let started = Instant::now();
        assert!(other.call_within("echo", Map::new(), Duration::from_secs(5)).is_ok());
        assert!(started.elapsed() < Duration::from_secs(2), "a stalled client does not delay another");
        other.disconnect();

        let started = Instant::now();
        server.stop();
        assert!(started.elapsed() < Duration::from_millis(500), "stop does not wait for a blocked writer");
        // What the stalled client can still read ends with the end of the stream: no write after close.
        stalled.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut rest = Vec::new();
        // The end of the stream, or a reset because our own unread requests were discarded with it.
        match stalled.read_to_end(&mut rest) {
            Ok(_) => {}
            Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset, "the stream ends"),
        }
        server.start().unwrap();
    }
    server.stop();
}

#[test]
fn an_oversized_request_closes_the_connection() {
    let folder = TempDir::new("big");
    let page = FakePage::new();
    let server = started(&page, &folder);
    let mut stream = UnixStream::connect(folder.socket()).unwrap();
    let chunk = vec![b'x'; 1 << 20];
    let mut sent = 0usize;
    // No line feed ever comes: the server gives up at the line limit instead of buffering on.
    let closed = loop {
        match stream.write_all(&chunk) {
            Ok(()) => sent += chunk.len(),
            Err(_) => break true,
        }
        if sent > framing::MAX_LINE_BYTES + (8 << 20) {
            break false;
        }
    };
    if !closed {
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut byte = [0u8; 1];
        assert!(matches!(stream.read(&mut byte), Ok(0) | Err(_)), "the connection is closed");
    }
    wait_for("the client to be dropped", || server.state().clients == 0);
    assert!(page.methods().is_empty());
    server.stop();
}

#[test]
fn quitting_fences_requests_and_cancelling_does_not_revive_them() {
    let folder = TempDir::new("quit");
    let page = FakePage::new();
    let server = started(&page, &folder);
    let mut stream = UnixStream::connect(folder.socket()).unwrap();
    // One request runs; two more are already waiting behind it on the same connection.
    stream.write_all(b"{\"id\":1,\"method\":\"slow\"}\n{\"id\":2,\"method\":\"undo\"}\n{\"id\":3,\"method\":\"undo\"}\n").unwrap();
    page.wait_slow();

    server.suspend_admission();
    assert!(server.state().suspended);
    assert!(!server.wait_idle(Duration::from_millis(100)), "the running request has not settled");
    let late: Value = serde_json::from_slice(&server.respond(b"{\"id\":9,\"method\":\"undo\"}")).unwrap();
    assert_eq!(late["error"]["message"], "The AI command was canceled because SuperTracker started quitting. Send it again after canceling Quit.");

    page.release();
    assert!(server.wait_idle(Duration::from_secs(5)), "the request that was running settles");
    assert_eq!(read_message(&mut stream).unwrap()["result"]["method"], "slow", "and is answered");
    server.resume_admission(); // Quit was cancelled
    for id in [2, 3] {
        let cancelled = read_message(&mut stream).unwrap();
        assert_eq!(cancelled["id"], id);
        assert_eq!(cancelled["error"]["message"], "The AI command was canceled because SuperTracker started quitting. Send it again after canceling Quit.");
    }
    assert_eq!(page.methods(), ["slow"], "requests queued before or during the fence never ran");
    assert_eq!(request(&mut stream, json!({"id": 4, "method": "undo"}))["result"]["method"], "undo", "new requests run again");
    server.stop();
}

//! The AI control area through the bridge: a control-socket client calls a method, the page
//! (played by the test) gets the `agent.call` event and answers with `agent.reply`; the setting,
//! the status events and the quit fence.
#![cfg(unix)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use st_agent::ControlClient;
use st_host::areas::agent::AgentOptions;
use st_host::areas::services::ServiceOptions;
use st_host::testing::{TempDir, TestHost};
use st_host::{install, Host, HostOptions, NoDialogs, QuitRequest};

struct Fixture {
    host: TestHost,
    services: Host,
    socket: std::path::PathBuf,
}

fn start(support: &std::path::Path) -> Fixture {
    let agent = AgentOptions::for_support(support);
    let socket = agent.socket_path.clone();
    let mut services = None;
    let host = TestHost::start(|bridge| {
        let mut options = HostOptions::new(support.to_path_buf(), Arc::new(NoDialogs));
        options.autosave_every = None;
        options.audio = None;
        options.services = ServiceOptions { agent: Some(agent), ..ServiceOptions::none() };
        services = Some(install(bridge, options).expect("install the host"));
    });
    host.request("ready", json!({})).expect("the handshake");
    Fixture { host, services: services.unwrap(), socket }
}

fn params(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap()
}

/// Calls `method` from a client thread while the test, as the page, answers the `agent.call`
/// event with `reply` (what `STAgent.dispatch` resolves to: JSON text).
fn call_through_the_page(fixture: &Fixture, client: ControlClient, method: &'static str, arguments: Value, reply: Value) -> (ControlClient, Result<Value, String>, Value) {
    let calling = std::thread::spawn(move || {
        let mut client = client;
        let outcome = client.call_within(method, params(arguments), Duration::from_secs(10)).map(|outcome| json!({ "result": outcome.result, "notices": outcome.notices })).map_err(|error| error.to_string());
        (client, outcome)
    });
    let call = fixture.host.wait_for_event("agent.call", Duration::from_secs(10)).expect("the page is asked");
    assert_eq!(fixture.host.request("agent.reply", json!({ "id": call["id"], "reply": reply.to_string() })), Ok(Value::Null));
    let (client, outcome) = calling.join().unwrap();
    (client, outcome, call)
}

fn status_event(host: &TestHost, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match host.wait_for_event("agent.status", Duration::from_millis(100)) {
            Some(status) if wanted(&status) => return status,
            _ => {}
        }
    }
    panic!("no agent.status event where {what}");
}

#[test]
fn a_client_reads_and_edits_through_the_page_and_the_setting_and_the_quit_fence_cut_it_off() {
    let folder = TempDir::new("agent");
    let support = folder.path().join("support");
    let fixture = start(&support);
    let host = &fixture.host;
    assert_eq!(fixture.socket, support.join("control.sock"), "the socket lives in the support folder");

    let ready = host.request("ready", json!({})).unwrap();
    for command in ["agent.status", "agent.configure", "agent.reply"] {
        assert!(ready["commands"].as_array().unwrap().contains(&json!(command)), "{command} is implemented");
    }
    // Nothing listens until the executable says its transport is up.
    let before = host.request("agent.status", json!({})).unwrap();
    assert_eq!((before["available"].clone(), before["enabled"].clone(), before["listening"].clone(), before["clients"].clone()), (json!(true), json!(true), json!(false), json!(0)), "{before}");
    assert!(!fixture.socket.exists());
    fixture.services.services.start_agent();
    status_event(host, "the socket is open", |status| status["listening"] == true);
    assert_eq!(host.request("agent.status", json!({})).unwrap()["socket"], json!(fixture.socket));

    // The host answers app.ping itself.
    let mut client = ControlClient::new(&fixture.socket);
    let ping = client.call("app.ping", Map::new()).unwrap();
    assert_eq!((ping.result["app"].clone(), ping.result["project"].clone()), (json!("SuperTracker"), Value::Null));
    status_event(host, "the client is counted", |status| status["clients"] == 1);

    // A read and an edit go to the page as agent.call events and come back through agent.reply.
    let (client, read, call) = call_through_the_page(&fixture, client, "summary", json!({}), json!({ "ok": true, "result": { "name": "Demo", "bpm": 128 } }));
    assert_eq!((call["method"].clone(), call["params"]["_host"].clone()), (json!("summary"), json!({ "projectPath": null })));
    assert_eq!(read, Ok(json!({ "result": { "name": "Demo", "bpm": 128 }, "notices": [] })));
    let steps = json!({ "steps": [{ "track": 0, "row": 0, "note": "C-4" }] });
    let (client, edit, call) = call_through_the_page(&fixture, client, "writeSteps", steps.clone(), json!({ "ok": true, "result": { "changed": true }, "notices": ["Wrote 1 step"] }));
    assert_eq!((call["method"].clone(), call["params"]["steps"].clone()), (json!("writeSteps"), steps["steps"].clone()));
    assert_eq!(edit, Ok(json!({ "result": { "changed": true }, "notices": ["Wrote 1 step"] })));
    // The page's own errors reach the client as they are.
    let (mut client, failed, _) = call_through_the_page(&fixture, client, "writeSteps", json!({}), json!({ "ok": false, "error": "steps must be a list." }));
    assert!(failed.unwrap_err().contains("steps must be a list."));
    // An answer nobody waits for is refused.
    assert!(host.request("agent.reply", json!({ "id": 9999, "reply": "{}" })).unwrap_err().contains("9999"));

    // The quit handshake fences AI control: requests are refused while the question is open...
    assert_eq!(fixture.services.lifecycle.request_quit(), QuitRequest::Asked);
    assert!(host.wait_for_event("app.quitRequested", Duration::from_secs(5)).is_some());
    status_event(host, "admission is suspended", |status| status["suspendedForTermination"] == true);
    let refused = client.call("summary", Map::new()).unwrap_err().to_string();
    assert!(refused.to_lowercase().contains("quit"), "{refused}");
    assert!(host.wait_for_event("agent.call", Duration::from_millis(200)).is_none(), "nothing reaches the page meanwhile");
    // ... and accepted again when the answer is "stay".
    host.request("app.quitAnswer", json!({ "state": "stay" })).unwrap();
    status_event(host, "admission is open again", |status| status["suspendedForTermination"] == false);
    let (client, again, _) = call_through_the_page(&fixture, client, "summary", json!({}), json!({ "ok": true, "result": { "name": "Demo" } }));
    assert!(again.is_ok(), "{again:?}");

    // Turning AI control off in Settings disconnects the client at once and remembers the choice.
    let off = host.request("agent.configure", json!({ "enabled": false })).unwrap();
    assert_eq!((off["enabled"].clone(), off["listening"].clone(), off["clients"].clone()), (json!(false), json!(false), json!(0)), "{off}");
    assert_eq!(fixture.services.settings.get_raw("agent"), Some(json!({ "enabled": false })));
    let mut client = client;
    assert!(client.call_within("summary", Map::new(), Duration::from_secs(2)).is_err(), "the client is cut off");
    assert!(!fixture.socket.exists(), "the socket is gone");

    // On again: a new connection works. Quitting closes everything.
    let on = host.request("agent.configure", json!({ "enabled": true })).unwrap();
    assert_eq!((on["enabled"].clone(), on["listening"].clone()), (json!(true), json!(true)), "{on}");
    let mut client = ControlClient::new(&fixture.socket);
    assert!(client.call("app.ping", Map::new()).is_ok());
    fixture.services.services.shutdown();
    assert!(client.call_within("app.ping", Map::new(), Duration::from_secs(2)).is_err());
    assert!(!fixture.socket.exists());
}

#[test]
fn a_host_started_with_ai_control_off_opens_no_socket() {
    let folder = TempDir::new("agent-off");
    let support = folder.path().join("support");
    std::fs::create_dir_all(&support).unwrap();
    std::fs::write(support.join("settings.json"), json!({ "agent": { "enabled": false } }).to_string()).unwrap();
    let fixture = start(&support);
    fixture.services.services.start_agent();
    let status = fixture.host.request("agent.status", json!({})).unwrap();
    assert_eq!((status["enabled"].clone(), status["listening"].clone()), (json!(false), json!(false)), "{status}");
    assert!(!fixture.socket.exists());
    assert!(ControlClient::new(&fixture.socket).call_within("app.ping", Map::new(), Duration::from_secs(1)).is_err());
}

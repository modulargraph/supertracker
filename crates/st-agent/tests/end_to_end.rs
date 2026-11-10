//! End to end: an MCP client talks to the `supertracker-mcp` process over stdio, the process to the
//! application's control socket, and the application to a page that answers over the bridge
//! (`agent.call` events and `agent.reply` commands), as `st-host` wires it.
#![cfg(unix)]

mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread;
use std::time::Duration;

use common::{wait_for, TempDir};
use serde::de::IgnoredAny;
use serde_json::{json, Value};
use st_agent::{AgentOptions, AgentService, AgentSettings, BridgePage, Configure, PageReply};

/// The host as far as AI control needs it: settings storage, the event stream, and a page that
/// answers `agent.call` like `ui/agent-api.js` does (with JSON text).
struct Host {
    agent: Arc<AgentService>,
    page: Arc<BridgePage>,
    events: Arc<Mutex<Vec<(String, Value)>>>,
    saved: Arc<Mutex<Vec<AgentSettings>>>,
    calls: Arc<Mutex<Vec<(String, Value)>>>,
}

fn host(folder: &TempDir, settings: AgentSettings) -> Host {
    let events = Arc::new(Mutex::new(Vec::new()));
    let saved = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let slot: Arc<OnceLock<Weak<BridgePage>>> = Arc::new(OnceLock::new());
    let page = BridgePage::new(
        Box::new({
            let (slot, calls) = (slot.clone(), calls.clone());
            move |name, payload| {
                assert_eq!(name, "agent.call");
                let Some(page) = slot.get().and_then(Weak::upgrade) else { return };
                let calls = calls.clone();
                // The page answers later, from its own thread, through the command `agent.reply`.
                thread::spawn(move || {
                    let (id, method, params) = (payload["id"].as_u64().unwrap(), payload["method"].as_str().unwrap().to_string(), payload["params"].clone());
                    calls.lock().unwrap().push((method.clone(), params.clone()));
                    let reply = match method.as_str() {
                        "summary" => json!({"ok": true, "result": {"name": "Demo", "bpm": 128.0, "path": params["_host"]["projectPath"]}}),
                        "readPattern" => json!({"ok": true, "result": {"header": "row | 00 Bass", "lines": ["000 | C-4"], "pattern": {"id": 0}}}),
                        "writeSteps" => json!({"ok": true, "result": {"changed": true, "undoLabel": "AI: bass"}, "notices": ["Loaded"]}),
                        "silent" => return, // a page that never answers
                        _ => json!({"ok": false, "error": format!("Unknown method \"{method}\".")}),
                    };
                    page.reply(PageReply { id, reply: Value::String(reply.to_string()) }).unwrap();
                });
            }
        }),
        || true,
        || Some("/music/demo.supertracker".to_string()),
    );
    slot.set(Arc::downgrade(&page)).unwrap();
    let agent = AgentService::new(AgentOptions {
        socket_path: folder.socket(),
        page: page.clone(),
        settings,
        save_settings: Box::new({
            let saved = saved.clone();
            move |settings| {
                saved.lock().unwrap().push(settings.clone());
                Ok(())
            }
        }),
        events: Box::new({
            let events = events.clone();
            move |name, payload| events.lock().unwrap().push((name.to_string(), payload))
        }),
    });
    Host { agent, page, events, saved, calls }
}

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Mcp {
    fn spawn(folder: &TempDir) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_supertracker-mcp"))
            .env("SUPERTRACKER_CONTROL_SOCKET", folder.socket())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start the MCP server process");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Mcp { child, stdin, stdout, next: 1 }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        writeln!(self.stdin, "{}", json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("the MCP server answers");
        let response: Value = serde_json::from_str(&line).unwrap_or_else(|_| panic!("a JSON line, got {line:?}"));
        assert_eq!(response["id"], id);
        response
    }

    fn tool(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))["result"].clone()
    }

    fn finish(mut self) {
        drop(self.stdin);
        let status = self.child.wait().unwrap();
        assert!(status.success(), "the MCP server exits cleanly when its input ends");
    }
}

fn text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap_or_else(|| panic!("text content in {result}"))
}

#[test]
fn mcp_process_to_control_socket_to_page() {
    let folder = TempDir::new("e2e");
    let host = host(&folder, AgentSettings::default());
    host.agent.start();
    let status = host.agent.status(IgnoredAny).unwrap();
    assert!(status.available && status.enabled && status.listening && status.clients == 0 && status.error.is_none());
    assert_eq!(status.socket, folder.socket().to_string_lossy());

    let mut mcp = Mcp::spawn(&folder);
    let initialize = mcp.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "e2e", "version": "1"}}));
    assert_eq!(initialize["result"]["serverInfo"]["name"], "supertracker");
    writeln!(mcp.stdin, "{}", json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).unwrap();
    assert_eq!(mcp.request("tools/list", json!({}))["result"]["tools"].as_array().unwrap().len(), 51);
    assert_eq!(host.agent.current_status().clients, 0, "listing tools does not need the application");

    let song = mcp.tool("get_song", json!({}));
    assert_eq!(text(&song), r#"{"bpm":128,"name":"Demo","path":"/music/demo.supertracker"}"#);
    wait_for("the client to be counted", || host.agent.current_status().clients == 1);
    assert!(host.events.lock().unwrap().iter().any(|(name, payload)| name == "agent.status" && payload["clients"] == 1), "the interface is told about the client");

    let grid = mcp.tool("read_pattern", json!({"format": "grid"}));
    assert_eq!(text(&grid), "row | 00 Bass\n000 | C-4\n{\"pattern\":{\"id\":0}}");
    let written = mcp.tool("write_steps", json!({"steps": [{"track": 0, "row": 0, "note": "C-4"}]}));
    assert_eq!(text(&written), "{\"changed\":true,\"undoLabel\":\"AI: bass\"}\nApp messages: Loaded");
    {
        let calls = host.calls.lock().unwrap();
        let (method, params) = calls.last().unwrap();
        assert_eq!(method, "writeSteps");
        assert_eq!(params["steps"][0]["note"], "C-4");
        assert_eq!(params["_host"]["projectPath"], "/music/demo.supertracker");
    }
    let failed = mcp.tool("call_api", json!({"method": "nonsense"}));
    assert_eq!(failed["isError"], true);
    assert_eq!(text(&failed), "Unknown method \"nonsense\".");
    let ping = mcp.tool("call_api", json!({"method": "app.ping"}));
    assert!(text(&ping).contains("\"app\":\"SuperTracker\"") && text(&ping).contains("\"project\":\"/music/demo.supertracker\""), "{}", text(&ping));
    let shot = mcp.tool("screenshot", json!({}));
    assert_eq!(shot["isError"], true);
    assert_eq!(text(&shot), "Screenshots are not available in this version of SuperTracker.");

    // A page that does not answer: the call fails after the bridge's limit, and nothing is left waiting.
    host.page.set_timeout(Duration::from_millis(100));
    let silent = mcp.tool("call_api", json!({"method": "silent"}));
    assert_eq!(text(&silent), "The interface could not run silent: the interface did not answer in time.");
    assert_eq!(host.page.pending(), 0);
    assert!(host.page.reply(PageReply { id: 9999, reply: Value::Null }).is_err(), "a late answer is refused");
    host.page.set_timeout(Duration::from_secs(30));

    // The setting turns the socket off: the proxy's connection is closed at once and tools say what to do.
    let off = host.agent.configure(Configure { enabled: Some(false) }).unwrap();
    assert!(!off.enabled && !off.listening && off.clients == 0);
    assert_eq!(host.saved.lock().unwrap().last(), Some(&AgentSettings { enabled: false }));
    assert!(!folder.socket().exists());
    let refused = mcp.tool("get_song", json!({}));
    assert_eq!(refused["isError"], true);
    assert!(text(&refused).contains("Open SuperTracker first"), "{}", text(&refused));

    // Turned on again (or the application restarted): the proxy reconnects by itself.
    let on = host.agent.configure(Configure { enabled: Some(true) }).unwrap();
    assert!(on.enabled && on.listening);
    assert_eq!(text(&mcp.tool("get_song", json!({}))), r#"{"bpm":128,"name":"Demo","path":"/music/demo.supertracker"}"#);
    host.agent.shutdown();
    host.agent.start();
    assert_eq!(text(&mcp.tool("get_song", json!({}))), r#"{"bpm":128,"name":"Demo","path":"/music/demo.supertracker"}"#, "reconnects once after a restart");
    assert!(host.agent.configure(Configure::default()).unwrap().enabled, "an empty request only reads");

    mcp.finish();
    wait_for("the proxy's connection to end", || host.agent.current_status().clients == 0);
    host.agent.shutdown();
    assert!(!host.agent.current_status().listening);
}

#[test]
fn the_stored_setting_gates_the_socket_and_the_command_line_works() {
    let folder = TempDir::new("gate");
    let host = host(&folder, AgentSettings { enabled: false });
    host.agent.start();
    let status = host.agent.current_status();
    assert!(status.available && !status.enabled && !status.listening, "off in settings: nothing listens");
    assert!(!folder.socket().exists());
    let ctl = |arguments: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_supertracker-mcp")).arg("--ctl").args(arguments).env("SUPERTRACKER_CONTROL_SOCKET", folder.socket()).output().unwrap()
    };
    let refused = ctl(&["summary"]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("AI control is off"));

    host.agent.configure(Configure { enabled: Some(true) }).unwrap();
    let summary = ctl(&["summary"]);
    assert_eq!(summary.status.code(), Some(0), "{}", String::from_utf8_lossy(&summary.stderr));
    assert_eq!(String::from_utf8_lossy(&summary.stdout), "{\n  \"bpm\": 128,\n  \"name\": \"Demo\",\n  \"path\": \"/music/demo.supertracker\"\n}\n");
    let noisy = ctl(&["writeSteps", r#"{"steps":[{"track":0,"row":0,"note":"C-4"}]}"#]);
    let shown: Value = serde_json::from_slice(&noisy.stdout).unwrap();
    assert_eq!(shown, json!({"result": {"changed": true, "undoLabel": "AI: bass"}, "notices": ["Loaded"]}));
    assert_eq!(ctl(&["nonsense"]).status.code(), Some(1));
    assert_eq!(ctl(&[]).status.code(), Some(64));
    assert_eq!(ctl(&["summary", "[1]"]).status.code(), Some(64));
    host.agent.shutdown();

    // A setting that cannot be stored is reported, and the switch still takes effect.
    let failing = AgentService::new(AgentOptions {
        socket_path: folder.path().join("other.sock"),
        page: host.page.clone(),
        settings: AgentSettings::default(),
        save_settings: Box::new(|_| Err("the disk is full.".to_string())),
        events: Box::new(|_, _| {}),
    });
    let status = failing.configure(Configure { enabled: Some(false) }).unwrap();
    assert!(!status.enabled);
    assert_eq!(status.error.as_deref(), Some("Could not save the setting: the disk is full."));
    assert_eq!(serde_json::to_value(&status).unwrap()["suspendedForTermination"], false, "status fields are camelCase for the page");
    assert_eq!(serde_json::from_value::<AgentSettings>(json!({})).unwrap(), AgentSettings { enabled: true }, "AI control is on by default");
}

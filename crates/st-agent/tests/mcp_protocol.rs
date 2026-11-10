//! The MCP server without the application: initialize, tools/list, tools/call, notifications and
//! errors (ported from tests/AgentMCPTests.swift, `mcpProtocol`), and the tool catalogue.

use std::cell::RefCell;

use serde_json::{json, Map, Value};
use st_agent::mcp::{self, catalogue, McpServer, Tool};
use st_agent::ControlError;

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

type Calls = RefCell<Vec<(String, Map<String, Value>)>>;

fn server(calls: &Calls) -> McpServer<'_> {
    McpServer::new(move |method, params| {
        calls.borrow_mut().push((method.to_string(), params));
        match method {
            "app.screenshot" => Ok((json!({"mimeType": "image/png", "data": "iVBORw0KGgo=", "width": 640, "height": 400}), vec![])),
            "readPattern" => Ok((json!({"header": "row | 00 Bass", "lines": ["000 | C-4", "001 | ---"], "pattern": {"id": 0}}), vec![])),
            "writeSteps" => Ok((json!({"changed": true, "undoLabel": "AI: bass"}), vec!["Loaded".to_string()])),
            "fail" => Err(ControlError::Remote("steps[0].row must be 0–63".into())),
            _ => Ok((json!({"ok": method}), vec![])),
        }
    })
}

fn text_of(result: &Value) -> &str {
    result["content"][0]["text"].as_str().expect("text content")
}

#[test]
fn initialize_ping_and_unknown_methods() {
    let calls = Calls::default();
    let mut server = server(&calls);
    let initialize = server
        .handle(&object(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}})))
        .unwrap();
    let info = &initialize["result"];
    assert_eq!(info["protocolVersion"], "2025-06-18", "the requested supported version is echoed");
    assert!(info["capabilities"]["tools"].is_object(), "tools capability advertised");
    assert_eq!(info["capabilities"]["tools"]["listChanged"], false);
    assert_eq!(info["serverInfo"]["name"], "supertracker");
    assert_eq!(info["serverInfo"]["title"], "SuperTracker");
    assert_eq!(info["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(info["instructions"].as_str().unwrap().starts_with("SuperTracker is a tracker DAW"));
    assert!(server.initialized());
    let old = server.handle(&object(json!({"jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {"protocolVersion": "1999-01-01"}}))).unwrap();
    assert_eq!(old["result"]["protocolVersion"], mcp::SUPPORTED_VERSIONS[0], "unknown versions get the latest supported one");
    assert!(server.handle(&object(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))).is_none(), "notifications get no response");
    assert_eq!(server.handle(&object(json!({"jsonrpc": "2.0", "id": 3, "method": "ping"}))).unwrap()["result"], json!({}));
    assert!(server.handle(&object(json!({"jsonrpc": "2.0", "method": "ping"}))).is_none());
    let missing = server.handle(&object(json!({"jsonrpc": "2.0", "id": 4, "method": "nope"}))).unwrap();
    assert_eq!(missing["error"]["code"], -32601);
    assert_eq!(missing["error"]["message"], "Method not found: nope");
    assert!(server.handle(&object(json!({"jsonrpc": "2.0", "method": "nope"}))).is_none(), "unknown notifications stay silent");
    for (method, key) in [("resources/list", "resources"), ("resources/templates/list", "resourceTemplates"), ("prompts/list", "prompts")] {
        assert_eq!(server.handle(&object(json!({"jsonrpc": "2.0", "id": 5, "method": method}))).unwrap()["result"][key], json!([]));
    }
    assert!(server.handle(&object(json!({"jsonrpc": "2.0", "id": 6, "result": {}}))).is_none(), "a response to us is ignored");
    assert_eq!(server.handle(&object(json!({"jsonrpc": "2.0", "id": 6}))).unwrap()["error"]["code"], -32600);
    assert!(calls.borrow().is_empty());
}

#[test]
fn tools_are_listed_with_object_schemas() {
    let calls = Calls::default();
    let mut server = server(&calls);
    let list = server.handle(&object(json!({"jsonrpc": "2.0", "id": 5, "method": "tools/list"}))).unwrap();
    let tools = list["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|tool| tool["name"].as_str().unwrap()).collect();
    let unique: std::collections::BTreeSet<&str> = names.iter().copied().collect();
    assert!(unique.len() == names.len() && names.len() >= 40, "{} uniquely named tools", names.len());
    for required in [
        "get_song", "read_pattern", "write_steps", "transform_steps", "edit_song_sequence", "edit_track", "set_song_settings",
        "edit_instrument", "load_sample", "search_library", "load_plugin", "set_plugin_parameters", "load_nks", "set_automation",
        "transport", "undo", "save_project", "export_audio", "run_command", "get_ui_state", "screenshot",
    ] {
        assert!(names.contains(&required), "tool {required} is listed");
    }
    for tool in tools {
        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object", "{} has an object schema", tool["name"]);
        assert_eq!(schema["additionalProperties"], false, "{} rejects unknown arguments", tool["name"]);
        for key in schema["required"].as_array().map(Vec::as_slice).unwrap_or_default() {
            assert!(schema["properties"].get(key.as_str().unwrap()).is_some(), "{} requires a declared property {key}", tool["name"]);
        }
        assert!(tool["description"].as_str().unwrap().len() > 10, "{} is described", tool["name"]);
    }
}

/// The catalogue is the public contract: its names, order, methods and the wording that clients
/// have been shown must not drift.
#[test]
fn the_catalogue_is_the_ported_table() {
    let catalogue = catalogue();
    let names: Vec<&str> = catalogue.tools.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "get_song", "get_ui_state", "screenshot", "read_pattern", "write_steps", "clear_steps", "insert_rows", "delete_rows",
            "transform_steps", "create_pattern", "duplicate_pattern", "rename_pattern", "delete_pattern", "set_pattern_length",
            "edit_song_sequence", "add_track", "edit_track", "set_song_settings", "get_instrument", "create_instrument",
            "edit_instrument", "search_library", "load_sample", "list_plugins", "load_plugin", "get_plugin_parameters",
            "set_plugin_parameters", "search_nks", "load_nks", "list_automation_targets", "set_automation", "delete_automation",
            "get_mixer", "edit_channel", "create_bus", "remove_bus", "set_send", "set_send_effects", "edit_insert",
            "enable_standard_routing", "set_performance", "transport", "undo", "redo", "get_history", "save_project", "export_audio",
            "list_commands", "run_command", "show_in_app", "call_api",
        ]
    );
    let method = |name: &str| catalogue.tool(name).unwrap().method.as_str();
    assert_eq!(method("get_song"), "summary");
    assert_eq!(method("get_ui_state"), "uiState");
    assert_eq!(method("screenshot"), "app.screenshot");
    assert_eq!(method("transform_steps"), "transform");
    assert_eq!(method("edit_song_sequence"), "editSong");
    assert_eq!(method("get_plugin_parameters"), "pluginParameters");
    assert_eq!(method("list_automation_targets"), "automationTargets");
    assert_eq!(method("get_mixer"), "mixer");
    assert_eq!(method("save_project"), "save");
    assert_eq!(method("show_in_app"), "show");
    assert_eq!(method("call_api"), "*");

    let load_nks = catalogue.tool("load_nks").unwrap();
    assert_eq!(load_nks.method, "loadNKS");
    assert_eq!(
        load_nks.description,
        "Load an NKS preset (path from search_nks, or the best match for query) on a track as its instrument, or as an effect in a slot."
    );
    assert_eq!(
        load_nks.properties["slot"],
        json!({"type": "integer", "description": "Effect slot 0–15; defaults to the first free slot (a full rack needs an explicit slot to replace).", "minimum": 0, "maximum": 15})
    );
    assert_eq!(load_nks.properties["instrument"]["description"], "\"new\" to add a new instrument instead of replacing the track's.");
    assert_eq!(load_nks.properties["instrument"]["type"], json!(["integer", "string"]));
    assert!(load_nks.required.is_empty());
    assert!(load_nks.definition()["inputSchema"].get("required").is_none(), "an empty required list is left out");

    let write = catalogue.tool("write_steps").unwrap();
    assert_eq!(write.required, ["steps"]);
    assert_eq!(write.properties["steps"]["items"]["required"], json!(["track", "row"]));
    assert_eq!(write.properties["steps"]["items"]["properties"]["column"]["maximum"], 7);
    assert_eq!(write.properties["steps"]["items"]["properties"]["gate"]["type"], json!(["number", "null"]));
    assert!(write.properties["steps"]["items"]["properties"]["fx1"]["description"].as_str().unwrap().starts_with("Step effect, e.g. \"probability 50%\""));
    assert_eq!(catalogue.tool("set_send_effects").unwrap().properties["delay"]["additionalProperties"], false);
    assert_eq!(catalogue.tool("transport").unwrap().properties["action"]["enum"], json!(["play", "playPattern", "playSong", "stop", "status"]));
    assert_eq!(catalogue.tool("undo").unwrap().properties["steps"]["maximum"], 90);
    assert!(catalogue.instructions.contains("Do not start playback unless the user asks."));
    // Every tool that docs/MCP.md documents exists, and the other way round.
    let Ok(documented) = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/MCP.md")) else {
        return; // the documentation moved; the catalogue checks above stand on their own
    };
    let table = &documented[documented.find("## Tools").unwrap()..documented.find("### Mixer and routing").unwrap()];
    for name in &names {
        assert!(table.contains(&format!("`{name}`")), "{name} is documented in docs/MCP.md");
    }
}

#[test]
fn tool_calls_reach_the_backend_and_render_as_text() {
    let calls = Calls::default();
    let mut server = server(&calls);
    let call = server
        .handle(&object(json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call",
            "params": {"name": "write_steps", "arguments": {"steps": [{"track": 0, "row": 0, "note": "C-4"}]}}})))
        .unwrap();
    {
        let calls = calls.borrow();
        let (method, params) = calls.last().unwrap();
        assert_eq!(method, "writeSteps");
        assert_eq!(params["steps"].as_array().unwrap().len(), 1, "tools map to API methods with their arguments");
    }
    let text = text_of(&call["result"]);
    assert!(text.contains("\"undoLabel\":\"AI: bass\"") && text.contains("App messages: Loaded"), "{text}");
    assert!(call["result"].get("isError").is_none(), "success is not an error");

    let grid = server.call_tool(catalogue().tool("read_pattern").unwrap(), object(json!({"format": "grid"})));
    assert!(text_of(&grid).starts_with("row | 00 Bass\n000 | C-4\n001 | ---\n"), "tracker grids are plain text lines");

    let shot = server.call_tool(catalogue().tool("screenshot").unwrap(), object(json!({"width": 640})));
    assert_eq!(shot["content"][0], json!({"type": "image", "mimeType": "image/png", "data": "iVBORw0KGgo="}));
    assert_eq!(shot["content"][1]["text"], "SuperTracker window, 640×400 px.");
    assert_eq!(calls.borrow().last().unwrap().0, "app.screenshot");
    assert_eq!(calls.borrow().last().unwrap().1["width"], 640, "screenshot width is forwarded");

    let generic = server.call_tool(catalogue().tool("call_api").unwrap(), object(json!({"method": "help", "params": {"x": 1}})));
    assert_eq!(calls.borrow().last().unwrap().0, "help");
    assert_eq!(calls.borrow().last().unwrap().1["x"], 1);
    assert!(generic.get("isError").is_none(), "call_api forwards any method");
    let before = calls.borrow().len();
    let blocked = server.call_tool(catalogue().tool("call_api").unwrap(), object(json!({"method": "app.screenshot"})));
    assert_eq!(blocked["isError"], true, "call_api does not reach native-only methods");
    assert_eq!(text_of(&blocked), "call_api needs method: an STAgent method name (try \"help\").");
    let nameless = server.call_tool(catalogue().tool("call_api").unwrap(), object(json!({})));
    assert_eq!(nameless["isError"], true);
    assert_eq!(calls.borrow().len(), before);
    server.call_tool(catalogue().tool("call_api").unwrap(), object(json!({"method": "app.ping"})));
    assert_eq!(calls.borrow().last().unwrap().0, "app.ping", "app.ping is reachable through call_api");

    let failing = Tool { name: "x".into(), method: "fail".into(), description: String::new(), properties: Map::new(), required: vec![] };
    let failure = server.call_tool(&failing, Map::new());
    assert_eq!(failure["isError"], true);
    assert!(text_of(&failure).contains("row must be 0–63"), "API errors become isError tool results");
    let unknown = server.handle(&object(json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "explode"}}))).unwrap();
    assert_eq!(unknown["error"]["code"], -32602, "unknown tools are invalid params");
    assert_eq!(unknown["error"]["message"], "Unknown tool: explode");
}

#[test]
fn stdio_lines_and_batches() {
    let calls = Calls::default();
    let mut server = server(&calls);
    let code = |response: &Value| response["error"]["code"].as_i64();
    let empty = server.responses_to_line("[]");
    assert!(empty.len() == 1 && code(&empty[0]) == Some(-32600) && empty[0]["id"].is_null(), "an empty batch gets one Invalid Request error");
    let mixed = server.responses_to_line(r#"[1, {"jsonrpc":"2.0","id":8,"method":"ping"}, {"jsonrpc":"2.0","method":"notifications/initialized"}]"#);
    assert!(mixed.len() == 2 && code(&mixed[0]) == Some(-32600) && mixed[1]["id"] == 8, "batch entries answer individually; notifications stay silent");
    assert_eq!(code(&server.responses_to_line("{nope")[0]), Some(-32700), "parse errors");
    assert!(server.responses_to_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_empty(), "a lone notification gets no response");
    assert_eq!(code(&server.responses_to_line("7")[0]), Some(-32600));
}

#[test]
fn malformed_arguments_never_invoke_a_tool() {
    let calls = Calls::default();
    let mut server = server(&calls);
    for malformed in [json!([]), json!("wrong"), Value::Null, json!(7), json!(true)] {
        let count = calls.borrow().len();
        let bad_arguments = server
            .handle(&object(json!({"jsonrpc": "2.0", "id": 99, "method": "tools/call", "params": {"name": "undo", "arguments": malformed}})))
            .unwrap();
        assert_eq!(bad_arguments["error"]["code"], -32602, "malformed arguments must be rejected before Undo");
        assert_eq!(calls.borrow().len(), count, "malformed arguments must never invoke the backend");
        let bad_params = server.call_tool(catalogue().tool("call_api").unwrap(), object(json!({"method": "undo", "params": malformed})));
        assert!(bad_params["isError"] == true && calls.borrow().len() == count, "malformed call_api params cannot become an empty Undo");
        let bad_envelope = server.handle(&object(json!({"jsonrpc": "2.0", "id": 100, "method": "tools/call", "params": malformed}))).unwrap();
        assert!(bad_envelope["error"]["code"] == -32602 && calls.borrow().len() == count, "malformed JSON-RPC params cannot invoke a tool");
    }
    let count = calls.borrow().len();
    let absent = server.handle(&object(json!({"jsonrpc": "2.0", "id": 101, "method": "tools/call", "params": {"name": "undo"}}))).unwrap();
    assert!(absent.get("error").is_none() && calls.borrow().len() == count + 1 && calls.borrow().last().unwrap().0 == "undo", "omitted arguments still support zero-argument tools");
}

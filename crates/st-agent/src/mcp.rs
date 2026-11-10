//! The Model Context Protocol server (`supertracker --mcp`): a stdio JSON-RPC server that offers
//! the tool catalogue and proxies each tool call to the running application over the control socket.
//! It opens no window and starts no audio. [`run_control`] is the scripting command line
//! (`supertracker --ctl <method> '<json>'`).

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::client::ControlClient;
use crate::framing::{self, LineBuffer, MAX_LINE_BYTES};
use crate::ControlError;

/// MCP protocol revisions this server speaks, newest first.
pub const SUPPORTED_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// One tool of the catalogue.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Tool {
    /// The tool's name as MCP clients see it.
    pub name: String,
    /// The control method it calls: a method of the page's agent API, `app.screenshot`, or `*` for
    /// the tool that takes the method as an argument (`call_api`).
    pub method: String,
    /// What the tool does, for the model.
    pub description: String,
    /// JSON Schema of each argument.
    pub properties: Map<String, Value>,
    /// Arguments that must be given.
    pub required: Vec<String>,
}

impl Tool {
    /// The tool as `tools/list` returns it.
    pub fn definition(&self) -> Value {
        let mut schema = json!({"type": "object", "properties": self.properties, "additionalProperties": false});
        if !self.required.is_empty() {
            schema["required"] = json!(self.required);
        }
        json!({"name": self.name, "description": self.description, "inputSchema": schema})
    }
}

/// The tool catalogue: the public contract of AI control. It lives in `tools.json` next to this
/// file (ported from the Swift host's `MCPCatalogue`, one entry per tool, in the order clients list
/// them).
#[derive(Debug)]
pub struct Catalogue {
    /// The text clients show the model as the server's instructions.
    pub instructions: String,
    /// Every tool, in listing order.
    pub tools: Vec<Tool>,
    by_name: HashMap<String, usize>,
}

impl Catalogue {
    /// The tool called `name`.
    pub fn tool(&self, name: &str) -> Option<&Tool> {
        self.by_name.get(name).map(|&index| &self.tools[index])
    }
}

/// The built-in catalogue.
pub fn catalogue() -> &'static Catalogue {
    static CATALOGUE: OnceLock<Catalogue> = OnceLock::new();
    CATALOGUE.get_or_init(|| {
        #[derive(Deserialize)]
        struct File {
            instructions: String,
            tools: Vec<Tool>,
        }
        // The file is part of the source; `cargo test` reads it through this same path.
        let file: File = serde_json::from_str(include_str!("tools.json")).expect("tools.json is a valid tool catalogue");
        let by_name = file.tools.iter().enumerate().map(|(index, tool)| (tool.name.clone(), index)).collect();
        Catalogue { instructions: file.instructions, tools: file.tools, by_name }
    })
}

/// Runs a control method for the MCP server: `(method, params) -> (result, notices)`.
pub type Backend<'a> = Box<dyn FnMut(&str, Map<String, Value>) -> Result<(Value, Vec<String>), ControlError> + 'a>;

/// The MCP server proper: JSON-RPC messages in, responses out. [`run_stdio`] feeds it lines.
pub struct McpServer<'a> {
    backend: Backend<'a>,
    initialized: bool,
}

impl<'a> McpServer<'a> {
    /// A server whose tool calls go to `backend`.
    pub fn new(backend: impl FnMut(&str, Map<String, Value>) -> Result<(Value, Vec<String>), ControlError> + 'a) -> Self {
        McpServer { backend: Box::new(backend), initialized: false }
    }

    /// True once a client sent `initialize`.
    pub fn initialized(&self) -> bool {
        self.initialized
    }

    /// Handles one JSON-RPC message. Returns the response, or `None` for notifications.
    pub fn handle(&mut self, message: &Map<String, Value>) -> Option<Value> {
        let id = message.get("id");
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            if id.is_some() && (message.contains_key("result") || message.contains_key("error")) {
                return None; // a response to something we never asked: ignore
            }
            return Some(framing::error_response(id, -32600, "Invalid request"));
        };
        let is_notification = id.is_none_or(Value::is_null);
        let params = match framing::named_object(message.get("params"), "params") {
            Ok(params) => params,
            Err(error) => return (!is_notification).then(|| framing::error_response(id, -32602, &error.to_string())),
        };
        match method {
            "initialize" => {
                let requested = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(SUPPORTED_VERSIONS[0]);
                self.initialized = true;
                Some(success(
                    id,
                    json!({
                        "protocolVersion": if SUPPORTED_VERSIONS.contains(&requested) { requested } else { SUPPORTED_VERSIONS[0] },
                        "capabilities": {"tools": {"listChanged": false}},
                        "serverInfo": {"name": "supertracker", "title": "SuperTracker", "version": env!("CARGO_PKG_VERSION")},
                        "instructions": catalogue().instructions,
                    }),
                ))
            }
            "notifications/initialized" | "notifications/cancelled" | "notifications/roots/list_changed" => None,
            "ping" => (!is_notification).then(|| success(id, json!({}))),
            "tools/list" => Some(success(id, json!({"tools": catalogue().tools.iter().map(Tool::definition).collect::<Vec<_>>()}))),
            "resources/list" => Some(success(id, json!({"resources": []}))),
            "resources/templates/list" => Some(success(id, json!({"resourceTemplates": []}))),
            "prompts/list" => Some(success(id, json!({"prompts": []}))),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str);
                let Some(tool) = name.and_then(|name| catalogue().tool(name)) else {
                    return Some(framing::error_response(id, -32602, &format!("Unknown tool: {}", name.unwrap_or("(none)"))));
                };
                match framing::named_object(params.get("arguments"), "arguments") {
                    Ok(arguments) => Some(success(id, self.call_tool(tool, arguments))),
                    Err(error) => Some(framing::error_response(id, -32602, &error.to_string())),
                }
            }
            _ => (!is_notification).then(|| framing::error_response(id, -32601, &format!("Method not found: {method}"))),
        }
    }

    /// Runs a tool and returns its MCP result (`content`, and `isError` on failure).
    pub fn call_tool(&mut self, tool: &Tool, arguments: Map<String, Value>) -> Value {
        let (method, params) = if tool.method == "*" {
            let name = arguments.get("method").and_then(Value::as_str).unwrap_or("");
            if name.is_empty() || (name.starts_with("app.") && name != "app.ping") {
                return error_result("call_api needs method: an STAgent method name (try \"help\").");
            }
            match framing::named_object(arguments.get("params"), "params") {
                Ok(params) => (name.to_string(), params),
                Err(error) => return error_result(&error.to_string()),
            }
        } else {
            (tool.method.clone(), arguments)
        };
        match (self.backend)(&method, params) {
            Ok((result, notices)) => {
                if tool.method == "app.screenshot" {
                    if let Some(data) = result.get("data").and_then(Value::as_str) {
                        let side = |key: &str| result.get(key).map(compact_json).unwrap_or_else(|| "?".into());
                        let mime = result.get("mimeType").and_then(Value::as_str).unwrap_or("image/png");
                        return json!({"content": [
                            {"type": "image", "data": data, "mimeType": mime},
                            {"type": "text", "text": format!("SuperTracker window, {}×{} px.", side("width"), side("height"))},
                        ]});
                    }
                }
                json!({"content": [{"type": "text", "text": render(&result, &notices)}]})
            }
            Err(error) => error_result(&error.to_string()),
        }
    }

    /// The responses to one line of input: a message, or a batch. An empty batch and a batch entry
    /// that is not an object are Invalid Requests (JSON-RPC 2.0, section 6).
    pub fn responses_to_line(&mut self, line: &str) -> Vec<Value> {
        let Ok(parsed) = serde_json::from_str::<Value>(line) else {
            return vec![framing::error_response(None, -32700, "Parse error")];
        };
        let invalid = || framing::error_response(None, -32600, "Invalid request");
        match parsed {
            Value::Object(message) => self.handle(&message).into_iter().collect(),
            Value::Array(batch) if !batch.is_empty() => batch
                .iter()
                .filter_map(|entry| match entry {
                    Value::Object(message) => self.handle(message),
                    _ => Some(invalid()),
                })
                .collect(),
            _ => vec![invalid()],
        }
    }
}

fn success(id: Option<&Value>, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id.cloned().unwrap_or(Value::Null), "result": result})
}

fn error_result(message: &str) -> Value {
    json!({"content": [{"type": "text", "text": message}], "isError": true})
}

/// A tool result as text for the model: compact JSON, with tracker grids (`header` and `lines`)
/// shown as plain text lines before the rest, and the application's messages after it.
pub fn render(result: &Value, notices: &[String]) -> String {
    let lines: Option<Vec<&str>> =
        result.get("lines").and_then(Value::as_array).and_then(|lines| lines.iter().map(Value::as_str).collect::<Option<Vec<_>>>());
    let mut text = match (result, lines) {
        (Value::Object(object), Some(lines)) => {
            let mut rest = object.clone();
            rest.remove("lines");
            let header = rest.remove("header");
            let mut text = String::new();
            for line in header.as_ref().and_then(Value::as_str).into_iter().chain(lines) {
                text.push_str(line);
                text.push('\n');
            }
            text + &compact_json(&Value::Object(rest))
        }
        (Value::String(text), _) => text.clone(),
        _ => compact_json(result),
    };
    if !notices.is_empty() {
        text.push_str("\nApp messages: ");
        text.push_str(&notices.join(" | "));
    }
    text
}

/// JSON text with sorted keys and the shortest numbers (`0.8`, not `0.80000000000000004`; `128`,
/// not `128.0`), for tool results the model reads.
pub fn compact_json(value: &Value) -> String {
    let mut out = String::new();
    write_json(value, &mut out, false, 0);
    out
}

/// [`compact_json`] over several lines, indented by two spaces, for `--ctl` output.
pub fn pretty_json(value: &Value) -> String {
    let mut out = String::new();
    write_json(value, &mut out, true, 0);
    out
}

fn write_json(value: &Value, out: &mut String, pretty: bool, indent: usize) {
    let newline = if pretty { format!("\n{}", "  ".repeat(indent + 1)) } else { String::new() };
    let close = if pretty { format!("\n{}", "  ".repeat(indent)) } else { String::new() };
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => match (number.as_i64(), number.as_u64(), number.as_f64()) {
            (Some(integer), _, _) => out.push_str(&integer.to_string()),
            (_, Some(integer), _) => out.push_str(&integer.to_string()),
            (_, _, Some(float)) if !float.is_finite() => out.push_str("null"),
            (_, _, Some(float)) if float == float.round() && float.abs() < 1e15 => out.push_str(&(float as i64).to_string()),
            _ => out.push_str(&number.to_string()),
        },
        Value::String(text) => escape(text, out),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&newline);
                write_json(item, out, pretty, indent + 1);
            }
            out.push_str(&close);
            out.push(']');
        }
        Value::Object(object) => {
            if object.is_empty() {
                out.push_str("{}");
                return;
            }
            // Sorted here rather than relying on the map's own order, which a cargo feature of
            // serde_json can change for the whole build.
            let mut keys: Vec<&String> = object.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&newline);
                escape(key, out);
                out.push_str(if pretty { ": " } else { ":" });
                write_json(&object[key], out, pretty, indent + 1);
            }
            out.push_str(&close);
            out.push('}');
        }
    }
}

fn escape(text: &str, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", control as u32)),
            other => out.push(other),
        }
    }
    out.push('"');
}

/// Serves MCP over `input` and `output` until the input ends: one JSON-RPC message (or batch) per
/// line, answered in order. A line longer than [`MAX_LINE_BYTES`] is answered with an error and
/// skipped; it is never held in memory.
pub fn serve(input: impl BufRead, output: impl Write, backend: Backend<'_>) -> io::Result<()> {
    serve_with_limit(input, output, backend, MAX_LINE_BYTES)
}

/// [`serve`] with another line limit (tests).
pub fn serve_with_limit(mut input: impl BufRead, mut output: impl Write, backend: Backend<'_>, limit: usize) -> io::Result<()> {
    let mut server = McpServer { backend, initialized: false };
    let mut buffer = LineBuffer::new(limit);
    loop {
        let (lines, consumed) = {
            let available = match input.fill_buf() {
                Ok(available) => available,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if available.is_empty() {
                return Ok(());
            }
            // Fed in bounded slices, so the buffer never holds more than the limit plus one slice.
            let slice = &available[..available.len().min(64 * 1024)];
            (buffer.append(slice), slice.len())
        };
        input.consume(consumed);
        if buffer.take_overflow() {
            write_message(&mut output, &framing::error_response(None, -32600, "The request is too large."))?;
        }
        for line in lines {
            let text = String::from_utf8_lossy(&line);
            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }
            for response in server.responses_to_line(trimmed) {
                write_message(&mut output, &response)?;
            }
        }
    }
}

fn write_message(output: &mut impl Write, message: &Value) -> io::Result<()> {
    output.write_all(&framing::encode(message))?;
    output.flush()
}

/// `supertracker --mcp`: serves MCP on standard input and output, proxying to the application's
/// control socket at `socket_path`. Returns the process's exit code. Logs go to standard error only.
pub fn run_stdio(socket_path: &Path) -> i32 {
    let mut client = ControlClient::new(socket_path);
    eprintln!("SuperTracker MCP server ready (control socket {}).", socket_path.display());
    let stdin = io::stdin();
    let stdout = io::stdout();
    let backend: Backend<'_> = Box::new(|method, params| client.call(method, params).map(|outcome| (outcome.result, outcome.notices)));
    match serve(stdin.lock(), stdout.lock(), backend) {
        Ok(()) => 0,
        // The client closed our output: nothing is left to serve.
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => 0,
        Err(error) => {
            eprintln!("SuperTracker MCP server stopped: {error}.");
            1
        }
    }
}

/// `supertracker --ctl <method> ['<json params>']`: calls one control method and prints its result
/// as JSON to `output`; errors go to `errors`. Returns the exit code: 0, 1 when the call failed, 64
/// for wrong usage.
pub fn run_control(arguments: &[String], socket_path: &Path, mut output: impl Write, mut errors: impl Write) -> i32 {
    let Some(method) = arguments.first() else {
        let _ = errors.write_all(
            b"Usage: supertracker --ctl <method> ['<json params>']\nExample: supertracker --ctl summary\n         supertracker --ctl writeSteps '{\"steps\":[{\"track\":0,\"row\":0,\"note\":\"C-4\"}]}'\n         supertracker --ctl help\n",
        );
        return 64;
    };
    let params = match arguments.get(1).map(|text| serde_json::from_str::<Value>(text)) {
        None => Map::new(),
        Some(Ok(Value::Object(params))) => params,
        Some(_) => {
            let _ = errors.write_all(b"The params must be a JSON object, e.g. '{\"pattern\":0}'.\n");
            return 64;
        }
    };
    match ControlClient::new(socket_path).call(method, params) {
        Ok(outcome) => {
            let shown = if outcome.notices.is_empty() { outcome.result } else { json!({"result": outcome.result, "notices": outcome.notices}) };
            let _ = writeln!(output, "{}", pretty_json(&shown));
            0
        }
        Err(error) => {
            let _ = writeln!(errors, "{error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_text_uses_shortest_numbers_sorted_keys_and_valid_escapes() {
        let parsed: Value = serde_json::from_str(r#"{"g":0.8,"b":true,"i":3,"f":128.0,"s":"a\"b\n/","n":null,"a":[1.5,-0.001]}"#).unwrap();
        assert_eq!(compact_json(&parsed), r#"{"a":[1.5,-0.001],"b":true,"f":128,"g":0.8,"i":3,"n":null,"s":"a\"b\n/"}"#);
        let pretty = pretty_json(&parsed);
        assert_eq!(serde_json::from_str::<Value>(&pretty).unwrap(), serde_json::from_str::<Value>(&compact_json(&parsed)).unwrap());
        assert!(pretty.starts_with("{\n  \"a\": [\n    1.5,\n    -0.001\n  ],\n  \"b\": true"), "{pretty}");
        assert_eq!(compact_json(&json!({"x": "\u{1}", "empty": {}, "none": []})), r#"{"empty":{},"none":[],"x":"\u0001"}"#);
        assert_eq!(compact_json(&json!(1e20)), "1e+20");
        assert_eq!(compact_json(&json!(u64::MAX)), u64::MAX.to_string());
    }

    #[test]
    fn grids_render_as_text_lines() {
        let grid = json!({"header": "row | 00 Bass", "lines": ["000 | C-4", "001 | ---"], "pattern": {"id": 0}});
        assert_eq!(render(&grid, &[]), "row | 00 Bass\n000 | C-4\n001 | ---\n{\"pattern\":{\"id\":0}}");
        assert_eq!(render(&json!("plain"), &["Saved".into(), "Done".into()]), "plain\nApp messages: Saved | Done");
        assert_eq!(render(&json!({"lines": [1, 2]}), &[]), r#"{"lines":[1,2]}"#, "lines that are not text stay JSON");
    }

    #[test]
    fn oversized_input_lines_are_reported_and_skipped() {
        let mut input = Vec::new();
        input.extend_from_slice(format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\",\"pad\":\"{}\"}}\n", "x".repeat(500)).as_bytes());
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n");
        let mut output = Vec::new();
        serve_with_limit(io::Cursor::new(input), &mut output, Box::new(|_, _| Ok((Value::Null, Vec::new()))), 128).unwrap();
        let lines: Vec<Value> = String::from_utf8(output).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["error"]["code"], -32600);
        assert_eq!(lines[0]["error"]["message"], "The request is too large.");
        assert_eq!(lines[1]["id"], 2);
    }
}

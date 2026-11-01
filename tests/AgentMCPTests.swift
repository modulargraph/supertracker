import Foundation
import Darwin

/// Standalone checks for the AI-control framing, the control-socket client and the MCP server
/// (initialize, tools/list, tools/call, notifications, errors) without the app.
@main enum AgentMCPTests {
    static var checks = 0
    static func expect(_ condition: @autoclosure () -> Bool, _ message: String, line: Int = #line) {
        checks += 1
        if !condition() { fputs("FAIL line \(line): \(message)\n", stderr); exit(1) }
    }

    static func main() throws {
        framing()
        socketPaths()
        try mcpProtocol()
        try socketRoundTrip()
        print("PASS agent MCP: \(checks) checks")
    }

    static func framing() {
        var buffer = LineBuffer(limit: 64)
        expect(buffer.append(Data("{\"a\":1}\n{\"b\"".utf8)) == [Data("{\"a\":1}".utf8)], "complete line split, partial kept")
        expect(buffer.append(Data(":2}\r\n\n".utf8)) == [Data("{\"b\":2}".utf8)], "partial completed, CR and blank lines dropped")
        _ = buffer.append(Data(String(repeating: "x", count: 100).utf8))
        expect(buffer.overflowed, "an unterminated line beyond the limit is rejected")
        let encoded = try! AgentControl.encode(["text": "line\nbreak", "path": "/a/b"])
        expect(encoded.last == 0x0A && encoded.dropLast().firstIndex(of: 0x0A) == nil, "encoded messages are exactly one line")
        expect(String(decoding: encoded, as: UTF8.self).contains("/a/b"), "slashes stay readable")
        expect((try? AgentControl.decode(Data("[1]".utf8))) == nil, "non-object lines are rejected")
        let parsed = try! JSONSerialization.jsonObject(with: Data(#"{"g":0.8,"b":true,"i":3,"f":128.0,"s":"a\"b\n/","n":null,"a":[1.5,-0.001]}"#.utf8))
        expect(CompactJSON.string(parsed) == #"{"a":[1.5,-0.001],"b":true,"f":128,"g":0.8,"i":3,"n":null,"s":"a\"b\n/"}"#, "tool text uses shortest numbers, sorted keys and valid escapes: \(CompactJSON.string(parsed))")
        expect((try? JSONSerialization.jsonObject(with: Data(CompactJSON.string(parsed, pretty: true).utf8))) != nil, "pretty output is valid JSON")
    }

    static func socketPaths() {
        expect(AgentControl.socketPath(environment: ["SUPERTRACKER_CONTROL_SOCKET": "/tmp/x.sock"]) == "/tmp/x.sock", "explicit socket path wins")
        expect(AgentControl.socketPath(environment: ["SUPERTRACKER_SUPPORT_DIR": "/Users/me/ST"]) == "/Users/me/ST/control.sock", "support folder override")
        expect(AgentControl.socketPath(environment: [:]).hasSuffix("Library/Application Support/SuperTracker/control.sock"), "default lives in Application Support")
        let long = "/private/tmp/" + String(repeating: "deep/", count: 30)
        let fallback = AgentControl.socketPath(environment: ["SUPERTRACKER_SUPPORT_DIR": long])
        expect(fallback.utf8.count < AgentControl.maxSocketPathBytes && fallback.hasSuffix(".sock"), "long support paths fall back to a short per-user path")
        expect(fallback == AgentControl.socketPath(environment: ["SUPERTRACKER_SUPPORT_DIR": long]), "the fallback is stable for app and client")
        expect(fallback != AgentControl.socketPath(environment: ["SUPERTRACKER_SUPPORT_DIR": long + "other/"]), "and distinct per support folder")
    }

    static func mcpProtocol() throws {
        var calls: [(String, [String: Any])] = []
        let server = MCPServer { method, params in
            calls.append((method, params))
            switch method {
            case "app.screenshot": return (["mimeType": "image/png", "data": "iVBORw0KGgo=", "width": 640, "height": 400], [])
            case "readPattern": return (["header": "row | 00 Bass", "lines": ["000 | C-4", "001 | ---"], "pattern": ["id": 0]], [])
            case "writeSteps": return (["changed": true, "undoLabel": "AI: bass"], ["Loaded"])
            case "fail": throw AgentControlError.remote("steps[0].row must be 0–63")
            default: return (["ok": method], [])
            }
        }
        let initialize = server.handle(["jsonrpc": "2.0", "id": 1, "method": "initialize", "params": ["protocolVersion": "2025-06-18", "capabilities": [:], "clientInfo": ["name": "test", "version": "1"]]])!
        let info = initialize["result"] as! [String: Any]
        expect(info["protocolVersion"] as? String == "2025-06-18", "the requested supported version is echoed")
        expect(((info["capabilities"] as? [String: Any])?["tools"]) != nil, "tools capability advertised")
        expect((info["serverInfo"] as? [String: Any])?["name"] as? String == "supertracker", "server name")
        let old = server.handle(["jsonrpc": "2.0", "id": 2, "method": "initialize", "params": ["protocolVersion": "1999-01-01"]])!
        expect((old["result"] as? [String: Any])?["protocolVersion"] as? String == MCPServer.supportedVersions[0], "unknown versions get the latest supported one")
        expect(server.handle(["jsonrpc": "2.0", "method": "notifications/initialized"]) == nil, "notifications get no response")
        expect((server.handle(["jsonrpc": "2.0", "id": 3, "method": "ping"])?["result"] as? [String: Any])?.isEmpty == true, "ping")
        let missing = server.handle(["jsonrpc": "2.0", "id": 4, "method": "nope"])!
        expect((missing["error"] as? [String: Any])?["code"] as? Int == -32601, "unknown method error")

        let list = server.handle(["jsonrpc": "2.0", "id": 5, "method": "tools/list"])!
        let tools = (list["result"] as! [String: Any])["tools"] as! [[String: Any]]
        let names = tools.compactMap { $0["name"] as? String }
        expect(Set(names).count == names.count && names.count >= 40, "\(names.count) uniquely named tools")
        for required in ["get_song", "read_pattern", "write_steps", "transform_steps", "edit_song_sequence", "edit_track", "set_song_settings", "edit_instrument", "load_sample", "search_library", "load_plugin", "set_plugin_parameters", "load_nks", "set_automation", "transport", "undo", "save_project", "export_audio", "run_command", "get_ui_state", "screenshot"] {
            expect(names.contains(required), "tool \(required) is listed")
        }
        for tool in tools {
            let schema = tool["inputSchema"] as? [String: Any]
            expect(schema?["type"] as? String == "object", "\(tool["name"]!) has an object schema")
            let properties = schema?["properties"] as? [String: Any] ?? [:]
            for key in schema?["required"] as? [String] ?? [] { expect(properties[key] != nil, "\(tool["name"]!) requires a declared property \(key)") }
            expect((tool["description"] as? String)?.count ?? 0 > 10, "\(tool["name"]!) is described")
            expect(JSONSerialization.isValidJSONObject(tool), "\(tool["name"]!) serialises")
        }

        let call = server.handle(["jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": ["name": "write_steps", "arguments": ["steps": [["track": 0, "row": 0, "note": "C-4"]]]]])!
        let callResult = call["result"] as! [String: Any]
        expect(calls.last?.0 == "writeSteps" && (calls.last?.1["steps"] as? [Any])?.count == 1, "tools map to API methods with their arguments")
        let text = ((callResult["content"] as! [[String: Any]])[0]["text"] as! String)
        expect(text.contains("\"undoLabel\":\"AI: bass\"") && text.contains("App messages: Loaded"), "results and app messages are returned as text")
        expect(callResult["isError"] == nil, "success is not an error")

        let grid = server.callTool(MCPCatalogue.byName["read_pattern"]!, arguments: ["format": "grid"])
        let gridText = ((grid["content"] as! [[String: Any]])[0]["text"] as! String)
        expect(gridText.hasPrefix("row | 00 Bass\n000 | C-4\n001 | ---\n"), "tracker grids are plain text lines")

        let shot = server.callTool(MCPCatalogue.byName["screenshot"]!, arguments: ["width": 640])
        let content = shot["content"] as! [[String: Any]]
        expect(content[0]["type"] as? String == "image" && content[0]["mimeType"] as? String == "image/png" && content[0]["data"] as? String == "iVBORw0KGgo=", "screenshots are MCP image content")
        expect(calls.last?.0 == "app.screenshot" && (calls.last?.1["width"] as? Int) == 640, "screenshot width is forwarded")

        let generic = server.callTool(MCPCatalogue.byName["call_api"]!, arguments: ["method": "help", "params": ["x": 1]])
        expect(calls.last?.0 == "help" && calls.last?.1["x"] as? Int == 1 && generic["isError"] == nil, "call_api forwards any method")
        let blocked = server.callTool(MCPCatalogue.byName["call_api"]!, arguments: ["method": "app.screenshot"])
        expect(blocked["isError"] as? Bool == true, "call_api does not reach native-only methods")

        let failure = server.callTool(MCPTool(name: "x", method: "fail", description: "", properties: [:], required: []), arguments: [:])
        expect(failure["isError"] as? Bool == true && (((failure["content"] as! [[String: Any]])[0]["text"] as! String).contains("row must be 0–63")), "API errors become isError tool results")
        let unknown = server.handle(["jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": ["name": "explode"]])!
        expect((unknown["error"] as? [String: Any])?["code"] as? Int == -32602, "unknown tools are invalid params")
        // stdio lines: an empty batch and non-object batch entries are Invalid Requests (JSON-RPC 2.0 §6).
        let code = { (response: [String: Any]) in (response["error"] as? [String: Any])?["code"] as? Int }
        let empty = server.responses(toLine: "[]")
        expect(empty.count == 1 && code(empty[0]) == -32600 && empty[0]["id"] is NSNull, "an empty batch gets one Invalid Request error")
        let mixed = server.responses(toLine: #"[1, {"jsonrpc":"2.0","id":8,"method":"ping"}, {"jsonrpc":"2.0","method":"notifications/initialized"}]"#)
        expect(mixed.count == 2 && code(mixed[0]) == -32600 && mixed[1]["id"] as? Int == 8, "batch entries answer individually; notifications stay silent")
        expect(code(server.responses(toLine: "{nope")[0]) == -32700, "parse errors")
        expect(server.responses(toLine: #"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).isEmpty, "a lone notification gets no response")
        for malformed: Any in [[Any](), "wrong", NSNull(), 7, true] {
            let count = calls.count
            let badArguments = server.handle(["jsonrpc":"2.0", "id":99, "method":"tools/call", "params":["name":"undo", "arguments":malformed]])!
            expect((badArguments["error"] as? [String:Any])?["code"] as? Int == -32602, "malformed arguments must be rejected before Undo")
            expect(calls.count == count, "malformed arguments must never invoke the backend")
            let badParams = server.callTool(MCPCatalogue.byName["call_api"]!, arguments:["method":"undo", "params":malformed])
            expect(badParams["isError"] as? Bool == true && calls.count == count, "malformed call_api params cannot become an empty Undo")
            let badEnvelope = server.handle(["jsonrpc":"2.0", "id":100, "method":"tools/call", "params":malformed])!
            expect((badEnvelope["error"] as? [String:Any])?["code"] as? Int == -32602 && calls.count == count, "malformed JSON-RPC params cannot invoke a tool")
        }
        let count = calls.count
        let absent = server.handle(["jsonrpc":"2.0", "id":101, "method":"tools/call", "params":["name":"undo"]])!
        expect(absent["error"] == nil && calls.count == count + 1 && calls.last?.0 == "undo", "omitted arguments still support zero-argument tools")

    }

    /// A minimal stand-in for the app's listener: answers each request line on a background thread.
    final class FakeApp {
        let path: String
        private var listener: Int32 = -1
        private(set) var requests: [[String: Any]] = []
        private let lock = NSLock()
        init(path: String) { self.path = path }

        func start() throws {
            unlink(path)
            let fd = socket(AF_UNIX, SOCK_STREAM, 0)
            var address = try AgentControl.makeAddress(path)
            let bound = withUnsafePointer(to: &address) { $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) } }
            guard bound == 0, chmod(path, 0o600) == 0, listen(fd, 4) == 0 else { throw AgentControlError.transport("fake bind failed errno \(errno)") }
            listener = fd
            Thread.detachNewThread { [self] in
                while true {
                    let client = accept(fd, nil, nil)
                    if client < 0 { return }
                    Thread.detachNewThread { [self] in serve(client) }
                }
            }
        }
        func stop() { if listener >= 0 { shutdown(listener, SHUT_RDWR); close(listener); listener = -1; unlink(path) } }

        private func serve(_ fd: Int32) {
            var buffer = LineBuffer(), chunk = [UInt8](repeating: 0, count: 4096)
            while true {
                let count = read(fd, &chunk, chunk.count)
                if count <= 0 { close(fd); return }
                for line in buffer.append(Data(chunk[0..<count])) {
                    guard let message = try? AgentControl.decode(line) else { continue }
                    lock.lock(); requests.append(message); lock.unlock()
                    let id = message["id"] ?? NSNull(), method = message["method"] as? String ?? ""
                    var response: [String: Any]
                    if method == "boom" { response = AgentControl.errorResponse(id: id, code: -32000, message: "Unknown method \"boom\".") }
                    else { response = ["jsonrpc": "2.0", "id": id, "result": ["method": method, "params": message["params"] ?? [:]], "notices": method == "noisy" ? ["Saved"] : []] }
                    // Send in two writes to exercise reassembly.
                    let data = try! AgentControl.encode(response), half = data.count / 2
                    data.withUnsafeBytes { raw in _ = write(fd, raw.baseAddress!, half); usleep(2000); _ = write(fd, raw.baseAddress!.advanced(by: half), data.count - half) }
                }
            }
        }
    }

    static func socketRoundTrip() throws {
        signal(SIGPIPE, SIG_IGN)
        let path = (AgentControl.userTemporaryDirectory() as NSString).appendingPathComponent("st-mcp-test-\(getpid()).sock")
        defer { unlink(path) }
        let absent = AgentControlClient(path: path)
        do { _ = try absent.call("summary"); expect(false, "calls fail when the app is not running") }
        catch let error as AgentControlError { if case .notRunning = error { expect(error.localizedDescription.contains("Open SuperTracker first"), "the not-running message tells the user what to do") } else { expect(false, "not-running error kind") } }

        let app = FakeApp(path: path); try app.start()
        var mode = stat(); stat(path, &mode)
        expect(mode.st_mode & 0o777 == 0o600, "the socket is user-only")
        let client = AgentControlClient(path: path)
        let first = try client.call("summary", params: ["a": 1])
        expect((first.result as? [String: Any])?["method"] as? String == "summary", "request reaches the app and the result returns")
        expect(((first.result as? [String: Any])?["params"] as? [String: Any])?["a"] as? Int == 1, "params travel intact")
        let noisy = try client.call("noisy")
        expect(noisy.notices == ["Saved"], "app notices are returned")
        do { _ = try client.call("boom"); expect(false, "remote errors throw") }
        catch { expect(error.localizedDescription == "Unknown method \"boom\".", "remote error text is preserved") }
        expect(client.isConnected, "one persistent connection")

        // MCP over the real socket client.
        let server = MCPServer { method, params in try client.call(method, params: params) }
        let response = server.handle(["jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": ["name": "get_song", "arguments": [:]]])!
        let text = (((response["result"] as! [String: Any])["content"] as! [[String: Any]])[0]["text"] as! String)
        expect(text.contains("\"method\":\"summary\""), "get_song proxies to summary over the socket")

        // App restart: the client reconnects once.
        app.stop()
        let restarted = FakeApp(path: path); try restarted.start()
        usleep(20000)
        let again = try client.call("uiState")
        expect((again.result as? [String: Any])?["method"] as? String == "uiState", "the client reconnects after the app restarts")
        restarted.stop()

        let offline = MCPServer { method, params in try AgentControlClient(path: path).call(method, params: params) }
        let result = offline.callTool(MCPCatalogue.byName["get_song"]!, arguments: [:])
        expect(result["isError"] as? Bool == true && (((result["content"] as! [[String: Any]])[0]["text"] as! String).contains("Open SuperTracker first")), "tools explain how to start the app")
    }
}

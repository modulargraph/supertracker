import AppKit
import Foundation
import WebKit

// Hidden actual renderer + actual AgentControlServer. No AudioHost, audio frameworks,
// control listener, hardware, file panels or vendors. Native replies are synthetic.
private enum AdmissionTestError: LocalizedError {
    case failed(String)
    var errorDescription: String? { if case let .failed(message) = self { return message }; return nil }
}
@MainActor private final class AdmissionBridge: NSObject, WKScriptMessageHandler {
    weak var web: WKWebView?
    var commands: [String] = [], unexpected: [String] = [], held: [[String: Any]] = [], syntheticRequests: [[String: Any]] = []
    var holdParameters = false
    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let body = message.body as? [String: Any], let id = body["id"] as? Int, let command = body["command"] as? String else { return }
        commands.append(command)
        let payload = body["payload"] as? [String: Any] ?? [:]
        syntheticRequests.append(["command": command, "payload": payload])
        let result: Any
        switch command {
        case "ready": result = ["version": "Isolated Agent admission fixture", "plugins": [], "roots": [], "audio": [:], "midi": [:]] as [String: Any]
        case "engine.project", "project.autosave", "engine.panic", "sample.stopPreview": result = [:] as [String: Any]
        case "timing.capabilities", "routing.capabilities": result = ["supported": false, "enabled": false]
        case "agent.status": result = ["available": true, "enabled": true, "clients": 1]
        case "midi.configure":
            guard Set(payload.keys) == ["track", "instrument"], let track = payload["track"] as? Int, (0..<16).contains(track), let instrument = payload["instrument"] as? Int, (0..<64).contains(instrument) else {
                unexpected.append("midi.configure invalid fixture payload"); reply(["id": id, "ok": false, "error": "Only synthetic renderer routing metadata is permitted."]); return
            }
            result = [:] as [String: Any]
        case "midi.performance":
            let validMap = payload["map"] is NSNull || ((payload["map"] as? [[Int]]).map { $0.count == 128 && $0.allSatisfy { !$0.isEmpty && $0.count <= 8 && $0.allSatisfy { (0..<128).contains($0) } } } ?? false)
            guard Set(payload.keys) == ["map"], validMap else { unexpected.append("midi.performance invalid fixture payload"); reply(["id": id, "ok": false, "error": "Only a synthetic keyboard performance map is permitted."]); return }
            result = [:] as [String: Any]
        case "plugin.parameters": result = [["address": "7", "name": "Original synthetic parameter", "value": 0.2, "normalized": 0.2, "min": 0, "max": 1, "writable": true]] as [[String: Any]]
        default:
            unexpected.append(command); reply(["id": id, "ok": false, "error": "No native service exists for \(command) in this fixture."]); return
        }
        let response: [String: Any] = ["id": id, "ok": true, "result": result]
        if command == "plugin.parameters" && holdParameters { held.append(response) } else { reply(response) }
    }
    func release() { holdParameters = false; let replies = held; held = []; for response in replies { reply(response) } }
    func reply(_ response: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: response), let json = String(data: data, encoding: .utf8) else { return }
        web?.evaluateJavaScript("window.nativeReceive(\(json));", completionHandler: nil)
    }
}
@MainActor private final class AdmissionRun {
    let web: WKWebView, bridge: AdmissionBridge, server: AgentControlServer, output: URL, webRoot: URL, settings: URL
    var checks: [[String: Any]] = []
    init(webRoot: URL, output: URL) throws {
        self.output = output; self.webRoot = webRoot
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        settings = output.appendingPathComponent("private-agent-settings.json")
        try Data("{\"enabled\":true,\"fixture\":\"preserve\"}".utf8).write(to: settings, options: .atomic)
        let config = WKWebViewConfiguration(); config.websiteDataStore = .nonPersistent()
        bridge = AdmissionBridge(); config.userContentController.add(bridge, name: "supertracker")
        web = WKWebView(frame: CGRect(x: 0, y: 0, width: 1080, height: 700), configuration: config); bridge.web = web
        server = AgentControlServer(webView: web, path: output.appendingPathComponent("never-opened.sock").path, settingsURL: settings, projectPath: { nil })
    }
    func js(_ source: String) async throws -> Any { try await web.callAsyncJavaScript(source, arguments: [:], in: nil, contentWorld: .page) ?? NSNull() }
    func wait(_ condition: @MainActor () async throws -> Bool, _ message: String) async throws {
        let end = Date().addingTimeInterval(8)
        while Date() < end { if try await condition() { return }; try await Task.sleep(nanoseconds: 20_000_000) }
        throw AdmissionTestError.failed(message)
    }
    func check(_ passed: Bool, _ name: String) throws {
        checks.append(["name": name, "passed": passed]); print("\(passed ? "PASS" : "FAIL") \(name)"); fflush(stdout)
        if !passed { throw AdmissionTestError.failed(name) }
    }
    func line(_ method: String, _ params: [String: Any] = [:]) throws -> Data { try AgentControl.encode(["id": 19, "method": method, "params": params]) }
    func successful(_ data: Data) -> Bool { (try? JSONSerialization.jsonObject(with: data) as? [String: Any])?["result"] != nil }
    func call(_ method: String, _ params: [String: Any] = [:]) async throws -> Bool { successful(await server.respond(to: try line(method, params))) }
    func report(_ error: String? = nil) throws {
        var body: [String: Any] = ["passed": error == nil && !checks.isEmpty, "checks": checks, "commands": bridge.commands, "unexpected": bridge.unexpected, "syntheticRequests": bridge.syntheticRequests, "audioHostCreated": false, "socketListenerStarted": server.listening, "physicalKeyboardTested": false, "nativeSaveDialogTested": false, "scope": "Actual hidden WKWebView renderer and AgentControlServer admission lifecycle; synthetic metadata/native replies. No app termination, audio, hardware, panels or vendors.", "webRoot": webRoot.path]
        if let error { body["error"] = error }
        try JSONSerialization.data(withJSONObject: body, options: [.prettyPrinted, .sortedKeys]).write(to: output.appendingPathComponent("report.json"), options: .atomic)
    }
    func run() async throws {
        try report("Not completed")
        let originalSettings = try Data(contentsOf: settings)
        defer { web.stopLoading(); bridge.web = nil; web.configuration.userContentController.removeScriptMessageHandler(forName: "supertracker") }
        do {
            web.loadFileURL(webRoot.appendingPathComponent("index.html"), allowingReadAccessTo: webRoot)
            try await wait({ (try await self.js("return !!window.STAgent&&!!ST.state.engineReady&&ST.state.pending.size===0;")) as? Bool == true }, "Renderer did not boot")
            _ = try await js("ST.cancelPendingEngineSync();ST.state.project.audioPlugins=[{track:0,kind:'effect',slot:0,instanceID:'fixture-fx',component:'fixture:never-instantiated',state:'opaque',name:'Fixture'}];window.__beforeQuit=JSON.stringify(ST.state.project);return true;")
            server.suspendAdmissionForTermination(); try await server.settleForTermination()
            try check(!(try await call("writeSteps", ["steps": [["track": 0, "row": 0, "note": 60]]])), "Native admission rejects an edit after quit checkpoint")
            try check((try await js("return JSON.stringify(ST.state.project)===__beforeQuit&&ST.state.undo.length===0;")) as? Bool == true, "Checkpoint project and Undo remain unchanged while decision is pending")
            await server.resumeAfterTermination()
            try check(try await call("writeSteps", ["steps": [["track": 0, "row": 0, "note": 61]]]), "Cancel reopens new native Agent commands")

            bridge.holdParameters = true
            let active = Task { @MainActor in try await self.call("pluginParameters", ["track": 0, "kind": "effect"]) }
            try await wait({ self.bridge.held.count == 1 }, "Active Agent request did not reach synthetic native boundary")
            let queuedBefore = server.responseHandler()
            _ = try await js("window.__queuedQuit=STAgent.call('writeSteps',{steps:[{track:0,row:1,note:62}]});return true;")
            server.suspendAdmissionForTermination()
            var settled = false
            let settling = Task { @MainActor in try await self.server.settleForTermination(); settled = true }
            try await Task.sleep(nanoseconds: 80_000_000)
            try check(!settled, "Quit waits for the already-running native Agent operation")
            bridge.release(); try check(try await active.value, "Already-running metadata operation drains")
            try await settling.value
            try check((try await js("return !(await __queuedQuit).ok;")) as? Bool == true, "Renderer-queued edit is canceled before quit capture")
            let queuedDuring = server.responseHandler()
            await server.resumeAfterTermination()
            let edit = try line("writeSteps", ["steps": [["track": 0, "row": 2, "note": 63]]])
            let beforeResult = await queuedBefore(edit), duringResult = await queuedDuring(edit)
            try check(!successful(beforeResult) && !successful(duringResult), "Native requests queued before or during Quit cannot revive after Cancel")
            try check(try await call("writeSteps", ["steps": [["track": 0, "row": 2, "note": 64]]]), "A freshly admitted command works after Cancel")

            bridge.holdParameters = true
            let slow = Task { @MainActor in try await self.call("pluginParameters", ["track": 0, "kind": "effect"]) }
            try await wait({ self.bridge.held.count == 1 }, "Timeout fixture did not reach native boundary")
            let stale = server.responseHandler()
            server.suspendAdmissionForTermination()
            var timedOut = false
            do { try await server.settleForTermination() } catch { timedOut = true }
            try check(timedOut && server.status["suspendedForTermination"] as? Bool == true, "Unsettled command times out fail-closed")
            try check(!(try await call("writeSteps", ["steps": [["track": 0, "row": 3, "note": 65]]])), "Timeout cannot admit another command")
            await server.resumeAfterTermination(); bridge.release(); _ = try await slow.value
            try check(!successful(await stale(edit)), "A timed-out generation remains canceled after its active request finishes")
            try check(try await call("writeSteps", ["steps": [["track": 0, "row": 3, "note": 66]]]), "New generation succeeds after timeout then Cancel")
            try check(try Data(contentsOf: settings) == originalSettings && server.enabled, "Temporary admission never changes the saved AI preference")
            try check(!server.listening && bridge.unexpected.isEmpty && NSApp.windows.allSatisfy { !$0.isVisible }, "No visible windows or listener; all renderer metadata requests stayed in the validated synthetic handler")
            try report()
        } catch { try? report(error.localizedDescription); throw error }
    }
}
@main private enum NativeAgentTerminationTests {
    @MainActor static func main() {
        func arg(_ key: String) -> String? { guard let i = CommandLine.arguments.firstIndex(of: key), CommandLine.arguments.count > i + 1 else { return nil }; return CommandLine.arguments[i + 1] }
        guard let webRoot = arg("--web-root"), let output = arg("--output") else { fputs("Usage: agent-termination --web-root /absolute/Resources/Web --output /absolute/private-output\n", stderr); exit(2) }
        let app = NSApplication.shared; app.setActivationPolicy(.accessory)
        Task { @MainActor in do { let run = try AdmissionRun(webRoot: URL(fileURLWithPath: webRoot), output: URL(fileURLWithPath: output)); try await run.run(); exit(0) } catch { fputs("Agent admission fixture: \(error.localizedDescription)\n", stderr); exit(1) } }
        Task { try? await Task.sleep(nanoseconds: 30_000_000_000); fputs("Agent admission fixture exceeded 30 seconds.\n", stderr); exit(3) }
        app.run()
    }
}

import AppKit
import WebKit
import Darwin

/// In-app listener for AI control (MCP). Accepts local connections from the same user on the
/// control socket and runs each JSON-RPC request on the main thread through window.STAgent,
/// so edits go through ST.mutate exactly like the user's own (refresh, engine sync, ⌘Z).
/// Native-only requests: app.ping, app.screenshot.
@MainActor
final class AgentControlServer {
    weak var webView: WKWebView?
    let path: String
    /// The Settings toggle lives in the support folder (so SUPERTRACKER_SUPPORT_DIR runs stay isolated).
    let settingsURL: URL
    private let projectPath: () -> String?
    private var listener: Int32 = -1
    private var acceptSource: DispatchSourceRead?
    private var connections: [ObjectIdentifier: AgentConnection] = [:]
    private var terminationSuspended = false
    private var admissionGeneration: UInt64 = 0
    private(set) var lastError: String?
    private let ioQueue = DispatchQueue(label: "SuperTracker.AgentControl.io")

    init(webView: WKWebView?, path: String = AgentControl.socketPath(), settingsURL: URL, projectPath: @escaping () -> String?) {
        self.webView = webView; self.path = path; self.settingsURL = settingsURL; self.projectPath = projectPath
    }

    /// On by default; the socket is local and user-only.
    var enabled: Bool {
        guard let data = try? Data(contentsOf: settingsURL), let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return true }
        return object["enabled"] as? Bool ?? true
    }
    var listening: Bool { listener >= 0 }

    var status: [String: Any] {
        var result: [String: Any] = ["available": true, "enabled": enabled, "listening": listening, "clients": connections.count, "socket": path, "suspendedForTermination": terminationSuspended]
        if let lastError { result["error"] = lastError }
        return result
    }

    func setEnabled(_ on: Bool) {
        do {
            try FileManager.default.createDirectory(at: settingsURL.deletingLastPathComponent(), withIntermediateDirectories: true)
            try JSONSerialization.data(withJSONObject: ["enabled": on]).write(to: settingsURL, options: .atomic)
        } catch { lastError = "Could not save the setting: \(error.localizedDescription)" }
        if on { start() } else { stop() }
        publishStatus()
    }

    func start() {
        guard enabled, listener < 0 else { return }
        lastError = nil
        do { try openListener() }
        catch { lastError = error.localizedDescription; NSLog("AI control: %@", error.localizedDescription) }
        publishStatus()
    }

    func stop() {
        for connection in connections.values { connection.close() }
        connections.removeAll()
        acceptSource?.cancel(); acceptSource = nil
        if listener >= 0 { Darwin.close(listener); listener = -1; unlink(path) }
        publishStatus()
    }

    private func openListener() throws {
        let directory = (path as NSString).deletingLastPathComponent
        try FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
        if FileManager.default.fileExists(atPath: path) {
            // A live socket belongs to another SuperTracker; a stale one is replaced.
            let probe = AgentControlClient(path: path)
            if (try? probe.connect()) != nil { probe.disconnect(); throw AgentControlError.transport("Another SuperTracker window is already accepting AI control.") }
            unlink(path)
        }
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw AgentControlError.transport("Could not create the control socket (errno \(errno)).") }
        _ = fcntl(fd, F_SETFD, FD_CLOEXEC)
        var address = try AgentControl.makeAddress(path)
        let bound = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        guard bound == 0 else { let code = errno; Darwin.close(fd); throw AgentControlError.transport("Could not open the control socket: \(String(cString: strerror(code))).") }
        // User-only access before any connection can be accepted.
        guard chmod(path, 0o600) == 0, listen(fd, 8) == 0 else { let code = errno; Darwin.close(fd); unlink(path); throw AgentControlError.transport("Could not secure the control socket: \(String(cString: strerror(code))).") }
        _ = fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK)
        listener = fd
        let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: .main)
        source.setEventHandler { [weak self] in MainActor.assumeIsolated { self?.acceptPending() } }
        source.resume()
        acceptSource = source
    }

    private func acceptPending() {
        while listener >= 0 {
            let fd = accept(listener, nil, nil)
            if fd < 0 { break }
            var uid: uid_t = 0, gid: gid_t = 0
            guard getpeereid(fd, &uid, &gid) == 0, uid == getuid() else { Darwin.close(fd); continue }
            var on: Int32 = 1
            setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
            _ = fcntl(fd, F_SETFD, FD_CLOEXEC)
            let connection = AgentConnection(fd: fd, queue: ioQueue)
            connection.onLine = { [weak self, weak connection] line in
                Task { @MainActor in
                    guard let self, let connection else { return }
                    connection.enqueue(line, handler: self.responseHandler())
                }
            }
            connection.onClose = { [weak self, weak connection] in
                Task { @MainActor in
                    guard let self, let connection else { return }
                    self.connections[ObjectIdentifier(connection)] = nil
                    self.publishStatus()
                }
            }
            connections[ObjectIdentifier(connection)] = connection
            connection.start()
            publishStatus()
        }
    }

    func publishStatus() {
        guard let webView, JSONSerialization.isValidJSONObject(status),
              let data = try? JSONSerialization.data(withJSONObject: status), let json = String(data: data, encoding: .utf8) else { return }
        webView.evaluateJavaScript("window.STAgent && window.STAgent.setConnection(\(json));", completionHandler: nil)
    }

    /// A temporary admission fence, separate from the user's saved AI preference.
    /// App installs this synchronously before beginning any asynchronous quit work.
    func suspendAdmissionForTermination() {
        terminationSuspended = true
        admissionGeneration &+= 1
        publishStatus()
    }

    func settleForTermination() async throws {
        guard terminationSuspended, let webView else { throw AgentControlError.remote("The AI control window could not be secured before quitting.") }
        // The page waits up to 3 s for a running command; a page that does not answer at all is hung.
        let value = try await awaitInterfaceReply(within: 8) {
            try await webView.callAsyncJavaScript("if (!window.STAgent?.suspendForTermination) throw new Error('The AI control interface is unavailable.'); return await window.STAgent.suspendForTermination(3000);",
                arguments: [:], in: nil, contentWorld: .page)
        }
        guard value as? Bool == true else { throw AgentControlError.remote("An AI command is still running. Keep working and finish or cancel it before quitting.") }
    }

    func resumeAfterTermination() async {
        // Reopen renderer admission before admitting new socket requests. A page
        // that disappeared will fail ordinary command delivery without mutation.
        if let webView {
            // Bounded: Quit stays blocked until this returns (terminationPending), so a hung page must not hold it.
            _ = try? await awaitInterfaceReply(within: 3) { try await webView.callAsyncJavaScript("window.STAgent?.resumeAfterTermination?.(); return true;", arguments: [:], in: nil, contentWorld: .page) }
        }
        terminationSuspended = false
        publishStatus()
    }

    /// Pin admission when a socket line is queued, not when its turn finally runs.
    /// Canceling Quit must not revive commands queued before or during the fence.
    func responseHandler() -> @MainActor (Data) async -> Data {
        let generation = admissionGeneration, admitted = !terminationSuspended
        return { [weak self] line in
            guard let self else { return Data("{}\n".utf8) }
            return await self.respond(to: line, generation: generation, admitted: admitted)
        }
    }

    /// One request line in, one response line out.
    func respond(to line: Data) async -> Data { await responseHandler()(line) }

    private func respond(to line: Data, generation: UInt64, admitted: Bool) async -> Data {
        var id: Any? = nil
        do {
            let message = try AgentControl.decode(line)
            id = message["id"]
            guard admitted, generation == admissionGeneration else { throw AgentControlError.remote("The AI command was canceled because SuperTracker started quitting. Send it again after canceling Quit.") }
            guard let method = message["method"] as? String else { throw AgentControlError.protocolError("Missing method.") }
            let params = try AgentControl.namedObject(message["params"], field: "params")
            let (result, notices) = try await perform(method, params: params)
            var response: [String: Any] = ["jsonrpc": "2.0", "id": id ?? NSNull(), "result": result]
            if !notices.isEmpty { response["notices"] = notices }
            return try AgentControl.encode(response)
        } catch {
            return (try? AgentControl.encode(AgentControl.errorResponse(id: id, code: -32000, message: error.localizedDescription))) ?? Data("{}\n".utf8)
        }
    }

    private func perform(_ method: String, params: [String: Any]) async throws -> (Any, [String]) {
        guard enabled else { throw AgentControlError.remote("AI control is turned off in SuperTracker Settings.") }
        guard !terminationSuspended else { throw AgentControlError.remote("SuperTracker is preparing to quit. Cancel quitting before sending more AI commands.") }
        switch method {
        case "app.ping":
            return (["app": "SuperTracker", "version": "0.1.0", "pid": Int(getpid()), "socket": path, "project": projectPath() as Any? ?? NSNull()], [])
        case "app.screenshot":
            return (try await screenshot(width: (params["width"] as? NSNumber)?.intValue), [])
        default:
            guard let webView else { throw AgentControlError.remote("The SuperTracker window is not open.") }
            if method == "save", params["path"] == nil, projectPath() == nil {
                throw AgentControlError.remote("This session has never been saved. Pass path (an absolute path ending in .supertracker).")
            }
            var arguments = params
            arguments["_host"] = ["projectPath": projectPath() as Any? ?? NSNull()]
            guard JSONSerialization.isValidJSONObject(arguments) else { throw AgentControlError.protocolError("params must be a JSON object.") }
            let json = String(decoding: try JSONSerialization.data(withJSONObject: arguments), as: UTF8.self)
            let value: Any?
            do {
                value = try await webView.callAsyncJavaScript("if (!window.STAgent) throw new Error('The SuperTracker interface is still loading.'); return await window.STAgent.dispatch(method, params);",
                                                              arguments: ["method": method, "params": json], in: nil, contentWorld: .page)
            } catch {
                throw AgentControlError.remote("The interface could not run \(method): \(error.localizedDescription)")
            }
            guard let text = value as? String, let data = text.data(using: .utf8),
                  let reply = try? JSONSerialization.jsonObject(with: data, options: [.fragmentsAllowed]) as? [String: Any] else {
                throw AgentControlError.protocolError("The interface returned an unreadable answer.")
            }
            let notices = reply["notices"] as? [String] ?? []
            if reply["ok"] as? Bool == true { return (reply["result"] ?? NSNull(), notices) }
            throw AgentControlError.remote(([reply["error"] as? String ?? "The request failed."] + notices.map { "App message: \($0)" }).joined(separator: "\n"))
        }
    }

    func screenshot(width requested: Int?) async throws -> [String: Any] {
        guard let webView else { throw AgentControlError.remote("The SuperTracker window is not open.") }
        let image = try await webView.takeSnapshot(configuration: WKSnapshotConfiguration())
        let points = image.size
        guard points.width > 0, points.height > 0 else { throw AgentControlError.remote("The window has no visible content to capture.") }
        let width = max(320, min(2560, requested ?? Int(points.width.rounded())))
        let height = max(1, Int((Double(width) * points.height / points.width).rounded()))
        guard let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: width, pixelsHigh: height, bitsPerSample: 8, samplesPerPixel: 4,
                                            hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0) else {
            throw AgentControlError.remote("Could not allocate the screenshot.")
        }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
        NSGraphicsContext.current?.imageInterpolation = .high
        (webView.window?.backgroundColor ?? .black).setFill()
        NSRect(x: 0, y: 0, width: width, height: height).fill()
        image.draw(in: NSRect(x: 0, y: 0, width: width, height: height), from: .zero, operation: .sourceOver, fraction: 1)
        NSGraphicsContext.restoreGraphicsState()
        guard let png = bitmap.representation(using: .png, properties: [:]) else { throw AgentControlError.remote("Could not encode the screenshot.") }
        return ["mimeType": "image/png", "data": png.base64EncodedString(), "width": width, "height": height]
    }
}

/// One client connection: reads on a background queue, answers requests strictly in order.
final class AgentConnection: @unchecked Sendable {
    let fd: Int32
    private let queue: DispatchQueue
    private var source: DispatchSourceRead?
    private var buffer = LineBuffer()
    // Set on `queue`, read by writes on `writeQueue`; the fd is closed on writeQueue so no write can outlive it.
    private let closedLock = NSLock()
    private var closedFlag = false
    private var closed: Bool { closedLock.lock(); defer { closedLock.unlock() }; return closedFlag }
    private let writeQueue = DispatchQueue(label: "SuperTracker.AgentControl.write")
    var onLine: ((Data) -> Void)?
    var onClose: (() -> Void)?
    // Main-actor request queue.
    private var pending: [(line: Data, handler: @MainActor (Data) async -> Data)] = []
    private var draining = false

    init(fd: Int32, queue: DispatchQueue) { self.fd = fd; self.queue = queue }

    func start() {
        let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: queue)
        source.setEventHandler { [weak self] in self?.readAvailable() }
        source.setCancelHandler { [fd, writeQueue] in writeQueue.async { Darwin.close(fd) } }
        self.source = source
        source.resume()
    }

    private func readAvailable() {
        var chunk = [UInt8](repeating: 0, count: 64 * 1024)
        let count = Darwin.read(fd, &chunk, chunk.count)
        if count <= 0 { if count < 0 && (errno == EAGAIN || errno == EINTR) { return }; close(); return }
        let lines = buffer.append(Data(chunk[0..<count]))
        if buffer.overflowed { close(); return }
        for line in lines { onLine?(line) }
    }

    @MainActor func enqueue(_ line: Data, handler: @escaping @MainActor (Data) async -> Data) {
        pending.append((line, handler))
        guard !draining else { return }
        draining = true
        Task { @MainActor in
            while !self.pending.isEmpty {
                let next = self.pending.removeFirst()
                let response = await next.handler(next.line)
                self.send(response)
            }
            self.draining = false
        }
    }

    func send(_ data: Data) {
        writeQueue.async { [weak self] in
            guard let self, !self.closed else { return }
            data.withUnsafeBytes { raw in
                var offset = 0
                while offset < raw.count, !self.closed {
                    let written = Darwin.write(self.fd, raw.baseAddress!.advanced(by: offset), raw.count - offset)
                    if written < 0 { if errno == EINTR || errno == EAGAIN { usleep(1000); continue }; break }
                    offset += written
                }
            }
        }
    }

    func close() {
        queue.async { [weak self] in
            guard let self else { return }
            self.closedLock.lock(); let already = self.closedFlag; self.closedFlag = true; self.closedLock.unlock()
            guard !already else { return }
            self.source?.cancel(); self.source = nil
            self.onClose?()
        }
    }
}

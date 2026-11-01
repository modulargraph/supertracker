import Foundation
import Darwin

/// Local AI-control channel shared by the app (listener) and `SuperTracker --mcp` / `--ctl` (clients).
/// Transport: a Unix domain socket readable and writable only by the current user (0600, peer uid
/// checked). Framing: one JSON-RPC 2.0 message per line (UTF-8 JSON never contains a raw newline).
enum AgentControl {
    static let maxLineBytes = 64 * 1024 * 1024
    static let defaultTimeout: TimeInterval = 600

    /// SUPERTRACKER_CONTROL_SOCKET, else <support>/control.sock where support is SUPERTRACKER_SUPPORT_DIR
    /// or ~/Library/Application Support/SuperTracker. Paths too long for sockaddr_un fall back to a
    /// per-user temporary folder, keyed by the support folder so both sides agree.
    static func socketPath(environment: [String: String] = ProcessInfo.processInfo.environment) -> String {
        if let explicit = environment["SUPERTRACKER_CONTROL_SOCKET"], explicit.hasPrefix("/") { return explicit }
        let support: String
        if let override = environment["SUPERTRACKER_SUPPORT_DIR"], override.hasPrefix("/") { support = override }
        else { support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("SuperTracker", isDirectory: true).path }
        let preferred = (support as NSString).appendingPathComponent("control.sock")
        if preferred.utf8.count < maxSocketPathBytes { return preferred }
        return (userTemporaryDirectory() as NSString).appendingPathComponent("SuperTracker-\(fnv1a(support)).sock")
    }

    static var maxSocketPathBytes: Int { MemoryLayout.size(ofValue: sockaddr_un().sun_path) }

    /// The per-user Darwin temporary folder (stable for this user, unlike $TMPDIR in sandboxes).
    static func userTemporaryDirectory() -> String {
        var buffer = [CChar](repeating: 0, count: Int(PATH_MAX))
        if confstr(_CS_DARWIN_USER_TEMP_DIR, &buffer, buffer.count) > 0 { return String(cString: buffer) }
        return NSTemporaryDirectory()
    }

    static func fnv1a(_ text: String) -> String {
        var hash: UInt64 = 0xcbf29ce484222325
        for byte in text.utf8 { hash ^= UInt64(byte); hash = hash &* 0x100000001b3 }
        return String(hash, radix: 16)
    }

    static func makeAddress(_ path: String) throws -> sockaddr_un {
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        let bytes = Array(path.utf8)
        guard bytes.count < maxSocketPathBytes else { throw AgentControlError.transport("The control socket path is too long: \(path)") }
        withUnsafeMutableBytes(of: &address.sun_path) { raw in
            raw.copyBytes(from: bytes)
            raw[bytes.count] = 0
        }
        return address
    }

    static func encode(_ object: Any) throws -> Data {
        guard JSONSerialization.isValidJSONObject(object) else { throw AgentControlError.protocolError("The message is not valid JSON.") }
        var data = try JSONSerialization.data(withJSONObject: object, options: [.withoutEscapingSlashes])
        data.append(0x0A)
        return data
    }

    static func decode(_ line: Data) throws -> [String: Any] {
        guard let object = try? JSONSerialization.jsonObject(with: line, options: [.fragmentsAllowed]), let message = object as? [String: Any] else {
            throw AgentControlError.protocolError("Expected one JSON object per line.")
        }
        return message
    }

    /// Omitted named arguments are empty; malformed supplied arguments must
    /// never be converted to a valid zero-argument action such as Undo.
    static func namedObject(_ value: Any?, field: String) throws -> [String: Any] {
        guard let value else { return [:] }
        guard let object = value as? [String: Any] else {
            throw AgentControlError.protocolError("\(field) must be a JSON object when provided.")
        }
        return object
    }

    static func errorResponse(id: Any?, code: Int, message: String) -> [String: Any] {
        ["jsonrpc": "2.0", "id": id ?? NSNull(), "error": ["code": code, "message": message]]
    }
}

enum AgentControlError: Error, LocalizedError, Equatable {
    case notRunning(String)
    case transport(String)
    case protocolError(String)
    case remote(String)

    var errorDescription: String? {
        switch self {
        case .notRunning(let detail): return "SuperTracker is not running or AI control is off. Open SuperTracker first (Settings › Allow AI control (MCP) must be on). [\(detail)]"
        case .transport(let message), .protocolError(let message), .remote(let message): return message
        }
    }
}

/// Splits a byte stream into newline-terminated lines (the newline is removed; CR before it too).
struct LineBuffer {
    private var pending = Data()
    private(set) var overflowed = false
    let limit: Int

    init(limit: Int = AgentControl.maxLineBytes) { self.limit = limit }

    mutating func append(_ data: Data) -> [Data] {
        pending.append(data)
        var lines: [Data] = []
        while let index = pending.firstIndex(of: 0x0A) {
            var line = pending.subdata(in: pending.startIndex..<index)
            if line.last == 0x0D { line.removeLast() }
            pending.removeSubrange(pending.startIndex...index)
            if !line.isEmpty { lines.append(line) }
        }
        if pending.count > limit { overflowed = true; pending.removeAll() }
        return lines
    }
}

/// Blocking client used by `--mcp` and `--ctl`. Keeps one connection open (the app shows a
/// status-bar indicator while it is connected) and reconnects once if the app restarted.
final class AgentControlClient {
    let path: String
    private var socket: Int32 = -1
    private var buffer = LineBuffer()
    private var queued: [Data] = []
    private var nextID = 1

    init(path: String = AgentControl.socketPath()) { self.path = path }
    deinit { disconnect() }

    var isConnected: Bool { socket >= 0 }

    func connect() throws {
        if socket >= 0 { return }
        let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw AgentControlError.transport("Could not create a socket (errno \(errno)).") }
        var on: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
        _ = fcntl(fd, F_SETFD, FD_CLOEXEC)
        var address = try AgentControl.makeAddress(path)
        let result = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        guard result == 0 else {
            let code = errno; close(fd)
            throw AgentControlError.notRunning("\(String(cString: strerror(code))) at \(path)")
        }
        socket = fd
        buffer = LineBuffer()
        queued = []
    }

    func disconnect() {
        if socket >= 0 { close(socket); socket = -1 }
    }

    /// Calls a control method and returns (result, notices). Throws AgentControlError.
    func call(_ method: String, params: [String: Any] = [:], timeout: TimeInterval = AgentControl.defaultTimeout) throws -> (result: Any, notices: [String]) {
        do { return try attempt(method, params: params, timeout: timeout) }
        catch AgentControlError.transport(_) where isConnected == false {
            // The app may have restarted since the last call: reconnect once.
            return try attempt(method, params: params, timeout: timeout)
        }
    }

    private func attempt(_ method: String, params: [String: Any], timeout: TimeInterval) throws -> (result: Any, notices: [String]) {
        try connect()
        let id = nextID; nextID += 1
        let request: [String: Any] = ["jsonrpc": "2.0", "id": id, "method": method, "params": params]
        let data = try AgentControl.encode(request)
        try send(data)
        let deadline = Date().addingTimeInterval(timeout)
        while true {
            let response = try receiveLine(deadline: deadline)
            let message = try AgentControl.decode(response)
            guard (message["id"] as? Int) == id else { continue }
            let notices = message["notices"] as? [String] ?? []
            if let error = message["error"] as? [String: Any] {
                let text = error["message"] as? String ?? "The request failed."
                throw AgentControlError.remote(notices.isEmpty ? text : text + "\n" + notices.joined(separator: "\n"))
            }
            return (message["result"] ?? NSNull(), notices)
        }
    }

    private func send(_ data: Data) throws {
        try data.withUnsafeBytes { raw in
            var offset = 0
            while offset < raw.count {
                let written = Darwin.write(socket, raw.baseAddress!.advanced(by: offset), raw.count - offset)
                if written < 0 {
                    if errno == EINTR { continue }
                    disconnect(); throw AgentControlError.transport("SuperTracker closed the control connection.")
                }
                offset += written
            }
        }
    }

    private func receiveLine(deadline: Date) throws -> Data {
        var chunk = [UInt8](repeating: 0, count: 256 * 1024)
        while true {
            if !queued.isEmpty { return queued.removeFirst() }
            let remaining = deadline.timeIntervalSinceNow
            guard remaining > 0 else { disconnect(); throw AgentControlError.remote("SuperTracker did not answer in time.") }
            var poller = pollfd(fd: socket, events: Int16(POLLIN), revents: 0)
            let ready = poll(&poller, 1, Int32(min(remaining, 3600) * 1000))
            if ready < 0 { if errno == EINTR { continue }; disconnect(); throw AgentControlError.transport("The control connection failed.") }
            if ready == 0 { continue }
            let count = Darwin.read(socket, &chunk, chunk.count)
            if count < 0 { if errno == EINTR || errno == EAGAIN { continue }; disconnect(); throw AgentControlError.transport("The control connection failed.") }
            if count == 0 { disconnect(); throw AgentControlError.transport("SuperTracker closed the control connection (it may have quit or AI control was turned off).") }
            let lines = buffer.append(Data(chunk[0..<count]))
            if buffer.overflowed { disconnect(); throw AgentControlError.protocolError("The response was too large.") }
            queued.append(contentsOf: lines)
        }
    }
}

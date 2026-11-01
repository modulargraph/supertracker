import Foundation

/// Native→interface delivery limits, kept free of WebKit so scripts/test-steps.sh can test them with swiftc.
///
/// Snapshot events replace each other: "status" (transport, meters, every 50 ms) and the sample editor's
/// playheads (30 per second). While the interface has not finished handling the previous one, a newer one is
/// dropped instead of queued. A busy or hung page otherwise piled up tens of thousands of WebKit script
/// calls (the window then hung) and, when it came back, worked through all of them, stale ones first.
struct SnapshotEventGate {
    static let coalesced: Set<String> = ["status", "sampler.playheads"]
    /// A delivery that never reports back (lost callback, page reload) stops holding newer ones back after this.
    var staleAfter: TimeInterval = 5
    private var inFlight: [String: (token: UInt64, since: TimeInterval)] = [:]
    private var nextToken: UInt64 = 0

    /// nil drops the event; otherwise a token for `finish` (0 for events that are never coalesced).
    mutating func begin(_ type: String, now: TimeInterval) -> UInt64? {
        guard Self.coalesced.contains(type) else { return 0 }
        if let previous = inFlight[type], now - previous.since < staleAfter { return nil }
        nextToken &+= 1
        inFlight[type] = (nextToken, now)
        return nextToken
    }

    /// The interface finished handling (or failed) the delivery with `token`.
    mutating func finish(_ type: String, token: UInt64) {
        guard token != 0, inFlight[type]?.token == token else { return }
        inFlight.removeValue(forKey: type)
    }
}

enum InterfaceReplyError: LocalizedError {
    case timedOut
    var errorDescription: String? {
        "The interface is not responding, so the latest edits could not be collected."
    }
}

/// Awaits a reply from the web page for at most `seconds`. WebKit answers a script call only once the page has
/// run it, so a hung page left Quit waiting forever with no way out but Force Quit. The operation keeps running
/// after a timeout (WebKit cannot cancel it); its late result is ignored.
@MainActor func awaitInterfaceReply<T>(within seconds: Double, _ operation: @escaping @MainActor () async throws -> T) async throws -> T {
    try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<T, Error>) in
        var finished = false
        func finish(_ result: Result<T, Error>) {
            guard !finished else { return }
            finished = true
            continuation.resume(with: result)
        }
        Task { @MainActor in
            do { finish(.success(try await operation())) } catch { finish(.failure(error)) }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { MainActor.assumeIsolated { finish(.failure(InterfaceReplyError.timedOut)) } }
    }
}

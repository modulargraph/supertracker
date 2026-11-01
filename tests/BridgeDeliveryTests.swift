import Foundation

/// Bridge delivery limits: snapshot events are not queued behind a busy page (#36, ~50,000 queued
/// interface updates), and a page that never answers cannot hold Quit forever.
@main enum BridgeDeliveryTests {
    static func check(_ condition: Bool, _ message: String) {
        if !condition { FileHandle.standardError.write(Data(("BRIDGE DELIVERY: " + message + "\n").utf8)); exit(1) }
    }

    @MainActor static func main() async {
        var gate = SnapshotEventGate()
        // A hung page answers nothing: 20 status ticks a second for an hour put one call in flight, not 72,000.
        guard let first = gate.begin("status", now: 0) else { return check(false, "first status was dropped") }
        var queued = 1
        for tick in 1..<(20 * 4) where gate.begin("status", now: Double(tick) / 20) != nil { queued += 1 }
        check(queued == 1, "status queued behind an unanswered one (\(queued) in flight)")
        // Ordinary events are never coalesced or dropped.
        for _ in 0..<100 { check(gate.begin("midi.note", now: 1) == 0, "an ordinary event was gated") }
        gate.finish("midi.note", token: 0)
        // Once the page has handled the call, the next status goes out.
        gate.finish("status", token: first)
        guard let second = gate.begin("status", now: 4.1) else { return check(false, "status stayed blocked after delivery") }
        check(gate.begin("sampler.playheads", now: 4.1) != nil, "playheads are gated by status")
        // A late answer for an older call never releases the newer one.
        gate.finish("status", token: first)
        check(gate.begin("status", now: 4.2) == nil, "a stale completion released the call in flight")
        // A delivery that never reports back stops gating after the stale limit.
        guard let third = gate.begin("status", now: 4.1 + gate.staleAfter) else { return check(false, "a lost completion blocked status forever") }
        check(third != second, "a reissued status reused the old token")

        // Interface replies: a page that answers in time returns its value; one that never answers times out.
        do {
            let value = try await awaitInterfaceReply(within: 2) { () async throws -> Int in
                try await Task.sleep(nanoseconds: 10_000_000); return 7
            }
            check(value == 7, "a prompt reply was lost")
        } catch { check(false, "a prompt reply failed: \(error)") }
        let started = Date()
        do {
            _ = try await awaitInterfaceReply(within: 0.2) { () async throws -> Int in
                try await Task.sleep(nanoseconds: 60_000_000_000) // never answers in time
                return 0
            }
            check(false, "a hung page reply succeeded")
        } catch {
            check(error is InterfaceReplyError, "a hung page reply failed with \(error)")
            check(Date().timeIntervalSince(started) < 2, "a hung page reply waited \(Date().timeIntervalSince(started)) s")
        }
        do {
            _ = try await awaitInterfaceReply(within: 2) { () async throws -> Int in throw CancellationError() }
            check(false, "a failing reply succeeded")
        } catch { check(error is CancellationError, "a failing reply lost its error: \(error)") }
        print("Bridge delivery tests passed")
    }
}

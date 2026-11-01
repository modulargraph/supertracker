import Foundation

/// Detects a realtime graph whose device stopped delivering render cycles.
/// Pure state so it can be tested without a device: feed it the render-quantum
/// count on each poll. Only a graph that was rendering and then stopped counts
/// (a device that never starts is left to the normal error path), it reports
/// once per cooldown, and at most `maximumReports` times per session.
struct AudioStallWatch {
    static let stallSeconds = 1.5
    static let cooldownSeconds = 5.0
    static let maximumReports = 3
    private var lastQuanta: Int64 = -1
    private var lastProgress = 0.0
    private var quietUntil = 0.0
    private var rendered = false
    private(set) var reports = 0

    mutating func stalled(now: Double, quanta: Int64, eligible: Bool) -> Bool {
        guard eligible else { lastQuanta = -1; rendered = false; return false }
        if quanta != lastQuanta {
            if lastQuanta >= 0 { rendered = true }
            lastQuanta = quanta; lastProgress = now; return false
        }
        guard rendered, reports < Self.maximumReports, now - lastProgress >= Self.stallSeconds, now >= quietUntil else { return false }
        quietUntil = now + Self.cooldownSeconds; lastQuanta = -1; rendered = false; reports += 1
        return true
    }
}

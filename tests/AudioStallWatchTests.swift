import Foundation

// Stall watchdog (AudioStallWatch): a graph that was rendering and whose render count then stops for 1.5 s is
// reported once per cooldown, at most three times per session; progress, a device that never started, or a graph
// that is not eligible (stopped, exporting, reconfiguring) never is.
@main
struct AudioStallWatchTests {
    static func main() {
        func require(_ ok: Bool, _ message: String) { if !ok { print("FAIL stall watch: \(message)"); exit(1) } }
        var watch = AudioStallWatch()
        var quanta: Int64 = 0
        for tick in 0..<100 { quanta += 1; require(!watch.stalled(now: Double(tick) * 0.05, quanta: quanta, eligible: true), "progress is never a stall") }
        let frozenAt = 5.0
        require(!watch.stalled(now: frozenAt, quanta: quanta, eligible: true), "the last progress is not a stall")
        require(!watch.stalled(now: frozenAt + 1.0, quanta: quanta, eligible: true), "1 s without cycles is tolerated")
        require(watch.stalled(now: frozenAt + 1.6, quanta: quanta, eligible: true), "1.6 s without cycles after rendering is a stall")
        require(!watch.stalled(now: frozenAt + 1.7, quanta: quanta, eligible: true), "a stall is reported once")
        require(!watch.stalled(now: frozenAt + 9.0, quanta: quanta, eligible: true), "after a restart that never renders, no new report")

        var dead = AudioStallWatch()
        for tick in 0..<200 { require(!dead.stalled(now: Double(tick) * 0.05, quanta: 0, eligible: true), "a device that never rendered is not restarted") }

        var idle = AudioStallWatch()
        for tick in 0..<200 { require(!idle.stalled(now: Double(tick) * 0.05, quanta: 7, eligible: false), "an ineligible graph is never a stall") }

        var flaky = AudioStallWatch()
        var now = 0.0, count: Int64 = 0, reported = 0
        for _ in 0..<10 {
            for _ in 0..<5 { count += 1; now += 0.05; _ = flaky.stalled(now: now, quanta: count, eligible: true) }
            for _ in 0..<200 { now += 0.05; if flaky.stalled(now: now, quanta: count, eligible: true) { reported += 1 } }
        }
        require(reported == AudioStallWatch.maximumReports, "restarts stop after \(AudioStallWatch.maximumReports) per session (got \(reported))")
        print("PASS a stalled audio device is restarted only after it rendered, once per cooldown, at most three times")
    }
}

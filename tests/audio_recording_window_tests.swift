// Pure, deterministic fixtures; no engine, Audio Units, devices, files, or GUI.
// From the repository root:
// swiftc -swift-version 5 -module-cache-path /private/tmp/supertracker-recording-window-module-cache Sources/SuperTracker/AudioRecordingWindow.swift tests/audio_recording_window_tests.swift -o /private/tmp/supertracker-recording-window-tests
// /private/tmp/supertracker-recording-window-tests
import Foundation

@main struct AudioRecordingWindowTests {
    typealias Window = AudioRecordingWindow
    typealias Clock = Window.Clock
    typealias Error = Window.WindowError
    struct TestFailure: Swift.Error { let message: String }
    static var checks = 0
    static let clock = Clock(id: "source-epoch-1", sampleRate: 48_000)

    static func require(_ condition: @autoclosure () -> Bool, _ message: String) throws {
        guard condition() else { throw TestFailure(message: message) }
        checks += 1
    }

    static func rejects(_ expected: Error, _ operation: () throws -> Void) throws {
        do { try operation() }
        catch let actual as Error {
            try require(actual == expected, "Expected \(expected), received \(actual)")
            return
        }
        throw TestFailure(message: "Expected rejection: \(expected)")
    }

    /// Original integer-indexed PCM makes every missing, repeated, or wrongly
    /// trimmed sample observable without reading or writing an audio file.
    static func packet(_ window: inout Window, start: Int64, count: Int,
                       into selected: inout [Int64]) throws -> Window.Selection {
        let previousWatermark = window.receivedThrough
        let result = try window.receive(startFrame: start, frameCount: count, clock: clock)
        try require(result.offset >= 0 && result.count >= 0 && result.offset + result.count <= count,
                    "Selection exceeds packet bounds")
        try require(result.sourceStartFrame == start + Int64(result.offset), "Incorrect source coordinate")
        if let previousWatermark {
            try require(window.receivedThrough! >= previousWatermark, "Received watermark moved backwards")
        }
        if result.count > 0 {
            let pcm = (0..<count).map { start + Int64($0) }
            selected.append(contentsOf: pcm[result.offset..<(result.offset + result.count)])
        }
        try require(Int64(selected.count) == window.selectedFrames, "Selection count differs from generated PCM")
        return result
    }

    static func asyncBatchFixture() throws {
        var window = try Window(clock: clock, startFrame: 1_001)
        var samples: [Int64] = []
        try require(!window.isComplete && window.receivedThrough == nil && window.selectedThrough == 1_001,
                    "New open window claims audio or completion")
        let first = try packet(&window, start: 0, count: 4_800, into: &samples)
        try require(first.offset == 1_001 && first.count == 3_799, "Pre-start batch was not trimmed exactly")
        let retry = try packet(&window, start: 0, count: 4_800, into: &samples)
        try require(retry.count == 0, "Duplicate batch selected twice")
        _ = try packet(&window, start: 4_800, count: 4_800, into: &samples)
        try window.acceptEnd(9_899, clock: clock)
        try require(!window.isComplete && window.selectedThrough == 9_600, "Stop completed before pending tail arrived")
        try window.acceptEnd(9_899, clock: clock)
        let tail = try packet(&window, start: 9_600, count: 4_800, into: &samples)
        try require(tail.offset == 0 && tail.count == 299, "Pending tail was not trimmed at accepted end")
        try require(window.isComplete && window.receivedThrough == 14_400 && window.selectedThrough == 9_899,
                    "Selection boundary and full packet watermark were conflated")
        try require(samples == Array(Int64(1_001)..<Int64(9_899)), "Asynchronous 4800-frame batches lost or duplicated audio")
        let after = try packet(&window, start: 14_400, count: 4_800, into: &samples)
        let old = try packet(&window, start: 0, count: 4_800, into: &samples)
        try require(after.count == 0 && old.count == 0 && window.isComplete && window.receivedThrough == 19_200,
                    "Completed capture accepted extra PCM or regressed its watermark")
    }

    static func boundariesAndOverlap() throws {
        var window = try Window(clock: clock, startFrame: 2_000, endFrame: 2_400)
        var samples: [Int64] = []
        let single = try packet(&window, start: 0, count: 4_800, into: &samples)
        try require(single.offset == 2_000 && single.count == 400 && window.isComplete,
                    "A single batch could not satisfy both boundaries")
        try require(samples == Array(Int64(2_000)..<Int64(2_400)), "Single-batch selection was not half-open")

        window = try Window(clock: clock, startFrame: -29, endFrame: 30)
        samples = []
        _ = try packet(&window, start: -100, count: 10, into: &samples)
        _ = try packet(&window, start: -50, count: 21, into: &samples)
        try require(samples.isEmpty && window.receivedThrough == -29 && !window.isComplete,
                    "Irrelevant pre-start gaps selected audio or completed a nonempty interval")
        _ = try packet(&window, start: -40, count: 45, into: &samples)
        let overlap = try packet(&window, start: -10, count: 30, into: &samples)
        try require(overlap.offset == 15 && overlap.count == 15, "Partial overlap was selected twice")
        _ = try packet(&window, start: -90, count: 50, into: &samples)
        _ = try packet(&window, start: 20, count: 10, into: &samples)
        try require(window.isComplete && window.receivedThrough == 30 && samples == Array(Int64(-29)..<Int64(30)),
                    "Signed source coordinates or exact exclusive packet end failed")

        window = try Window(clock: clock, startFrame: 100)
        samples = []
        _ = try packet(&window, start: 90, count: 30, into: &samples)
        try require(!window.isComplete, "Unknown end completed")
        try window.acceptEnd(120, clock: clock)
        try require(window.isComplete, "Already-observed exact end unnecessarily required another packet")
    }

    static func emptyIntervals() throws {
        var window = try Window(clock: clock, startFrame: 100, endFrame: 100)
        var samples: [Int64] = []
        try require(!window.isComplete, "Empty interval completed without a packet watermark")
        _ = try packet(&window, start: 0, count: 99, into: &samples)
        try require(!window.isComplete && window.selectedFrames == 0, "Empty interval completed before its boundary")
        _ = try packet(&window, start: 99, count: 1, into: &samples)
        try require(window.isComplete && samples.isEmpty, "Empty interval did not complete at an observed boundary")

        window = try Window(clock: clock, startFrame: -20, endFrame: -20)
        _ = try packet(&window, start: 0, count: 1, into: &samples)
        try require(window.isComplete, "Empty interval incorrectly required nonexistent PCM across a gap")

        window = try Window(clock: clock, startFrame: 20)
        _ = try packet(&window, start: 0, count: 20, into: &samples)
        try window.acceptEnd(20, clock: clock)
        try require(window.isComplete, "Late acceptance of an observed empty boundary failed")
    }

    static func failures() throws {
        for invalid in [Clock(id: "", sampleRate: 48_000), Clock(id: " \n\t", sampleRate: 48_000),
                        Clock(id: "clock", sampleRate: .nan), Clock(id: "clock", sampleRate: .infinity),
                        Clock(id: "clock", sampleRate: -.infinity), Clock(id: "clock", sampleRate: 0),
                        Clock(id: "clock", sampleRate: -1)] {
            try rejects(.invalidClock) { _ = try Window(clock: invalid, startFrame: 0) }
        }
        try rejects(.invalidEnd(startFrame: 10, endFrame: 9)) { _ = try Window(clock: clock, startFrame: 10, endFrame: 9) }
        try rejects(.intervalOverflow) { _ = try Window(clock: clock, startFrame: .min, endFrame: .max) }

        for count in [0, -1, Int.min] {
            var window = try Window(clock: clock, startFrame: 0)
            try rejects(.invalidFrameCount(count)) { _ = try window.receive(startFrame: 0, frameCount: count, clock: clock) }
            try require(window.receivedThrough == nil && window.selectedThrough == 0 && !window.isComplete,
                        "Invalid packet changed capture progress")
        }
        var window = try Window(clock: clock, startFrame: 0)
        try rejects(.packetOverflow(startFrame: .max, frameCount: 1)) {
            _ = try window.receive(startFrame: .max, frameCount: 1, clock: clock)
        }
        try require(window.receivedThrough == nil && window.selectedFrames == 0, "Overflow changed progress")

        for changed in [Clock(id: "source-epoch-2", sampleRate: 48_000), Clock(id: clock.id, sampleRate: 44_100)] {
            window = try Window(clock: clock, startFrame: 0, endFrame: 10)
            try rejects(.clockChanged(expected: clock, received: changed)) {
                _ = try window.receive(startFrame: 0, frameCount: 10, clock: changed)
            }
            try require(window.receivedThrough == nil && window.selectedFrames == 0 && !window.isComplete,
                        "Clock change advanced capture")
            try rejects(.clockChanged(expected: clock, received: changed)) { try window.acceptEnd(10, clock: clock) }

            window = try Window(clock: clock, startFrame: 0)
            try rejects(.clockChanged(expected: clock, received: changed)) { try window.acceptEnd(10, clock: changed) }
            try require(window.endFrame == nil, "Clock-changing end became accepted")
        }
        window = try Window(clock: clock, startFrame: 0)
        try rejects(.invalidClock) {
            _ = try window.receive(startFrame: 0, frameCount: 10, clock: Clock(id: clock.id, sampleRate: .nan))
        }

        window = try Window(clock: clock, startFrame: 100, endFrame: 200)
        try rejects(.gap(expectedFrame: 100, receivedFrame: 101)) {
            _ = try window.receive(startFrame: 101, frameCount: 100, clock: clock)
        }
        try require(window.receivedThrough == nil && window.failure == .gap(expectedFrame: 100, receivedFrame: 101),
                    "First missing sample did not latch an explicit gap")
        try rejects(.gap(expectedFrame: 100, receivedFrame: 101)) {
            _ = try window.receive(startFrame: 100, frameCount: 100, clock: clock)
        }
        try require(!window.isComplete, "A failed capture later claimed completion")

        window = try Window(clock: clock, startFrame: 0, endFrame: 30)
        _ = try window.receive(startFrame: 0, frameCount: 10, clock: clock)
        try rejects(.gap(expectedFrame: 10, receivedFrame: 11)) {
            _ = try window.receive(startFrame: 11, frameCount: 20, clock: clock)
        }
        try require(window.receivedThrough == 10 && window.selectedThrough == 10 && window.selectedFrames == 10,
                    "Gap advanced a watermark or selected samples")

        window = try Window(clock: clock, startFrame: 0)
        _ = try window.receive(startFrame: 0, frameCount: 4_800, clock: clock)
        try rejects(.lateEnd(selectedThrough: 4_800, proposed: 2_000)) { try window.acceptEnd(2_000, clock: clock) }
        try require(window.endFrame == nil && window.selectedFrames == 4_800 && !window.isComplete,
                    "Late end silently truncated already-selected PCM")

        window = try Window(clock: clock, startFrame: 0, endFrame: 100)
        try window.acceptEnd(100, clock: clock)
        try rejects(.endChanged(accepted: 100, proposed: 101)) { try window.acceptEnd(101, clock: clock) }
        try require(window.endFrame == 100 && window.receivedThrough == nil, "Changed end replaced accepted boundary")

        // No huge PCM allocation: exercise count overflow with metadata only.
        window = try Window(clock: clock, startFrame: .min)
        let enormous = try window.receive(startFrame: .min, frameCount: Int.max, clock: clock)
        try require(enormous.count == Int.max && window.selectedThrough == -1 && window.selectedFrames == Int64.max,
                    "Valid maximum-sized interval metadata failed")
        try rejects(.intervalOverflow) { _ = try window.receive(startFrame: -1, frameCount: 1, clock: clock) }
        try require(window.selectedThrough == -1 && window.receivedThrough == -1 && window.selectedFrames == .max,
                    "Selection count overflow changed progress")
    }

    struct Generator {
        var state: UInt64 = 0x7f193bd96425ca01
        mutating func next(_ bound: Int) -> Int {
            state = state &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407
            return Int((state >> 32) % UInt64(bound))
        }
    }

    static func generatedSchedules() throws {
        var random = Generator()
        for fixture in 0..<256 {
            let start = Int64(random.next(2_001) - 1_000)
            let end = start + Int64(1 + random.next(16_000))
            let knownEnd = fixture % 2 == 0
            var window = try Window(clock: clock, startFrame: start, endFrame: knownEnd ? end : nil)
            var samples: [Int64] = []
            var cursor = start - Int64(random.next(1_024))
            var lastStart = cursor
            var lastCount = 1
            while !window.isComplete {
                let count = 1 + random.next(4_800)
                // Advance the accepted end before handing out any PCM beyond it.
                if !knownEnd && window.endFrame == nil && cursor + Int64(count) >= end {
                    try window.acceptEnd(end, clock: clock)
                    try require(!window.isComplete, "Pending final packet completed without arrival")
                }
                lastStart = cursor
                lastCount = count
                _ = try packet(&window, start: cursor, count: count, into: &samples)
                let through = cursor + Int64(count)
                if random.next(3) == 0 {
                    let duplicate = try packet(&window, start: cursor, count: count, into: &samples)
                    try require(duplicate.count == 0, "Generated duplicate added samples")
                }
                // Subsequent packets may repeat a prefix but must make progress.
                cursor = through - Int64(random.next(min(count, 128)))
            }
            try require(samples == Array(start..<end), "Generated schedule \(fixture) did not select exact interval")
            try require(window.selectedFrames == end - start && window.selectedThrough == end && window.receivedThrough! >= end,
                        "Generated schedule completed with incorrect accounting")
            let duplicate = try packet(&window, start: lastStart, count: lastCount, into: &samples)
            try require(duplicate.count == 0, "Retry after completion added PCM")
        }
    }

    static func main() throws {
        try asyncBatchFixture()
        try boundariesAndOverlap()
        try emptyIntervals()
        try failures()
        try generatedSchedules()
        let report: [String: Any] = [
            "passed": true, "assertions": checks, "generatedSchedules": 256,
            "scope": "Pure Swift frame-window selection with original deterministic integer PCM; no audio engines, Audio Units, vendors, hardware, files, or GUI.",
            "completionMeans": "Contiguous selection plus actual packet watermark; writer flush remains the caller's responsibility."
        ]
        let data = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
        print(String(decoding: data, as: UTF8.self))
    }
}

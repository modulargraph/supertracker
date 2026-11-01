// Pure scalar/concurrency fixtures; no audio engine, devices, vendors, or GUI.
// From the repository root:
// swiftc -swift-version 5 -module-cache-path /private/tmp/supertracker-recording-window-module-cache Sources/SuperTracker/AudioRecordingBoundary.swift Sources/SuperTracker/AudioRecordingWindow.swift tests/audio_recording_boundary_tests.swift -o /private/tmp/supertracker-recording-boundary-tests
// /private/tmp/supertracker-recording-boundary-tests
import Foundation
import Dispatch

@main struct AudioRecordingBoundaryTests {
    typealias Boundary = AudioRecordingBoundary
    struct Failure: Error { let message: String }
    static var checks = 0
    static let rate = 48_000.0
    static let epoch: UInt64 = 71

    static func require(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
        guard try condition() else { throw Failure(message: message) }
        checks += 1
    }
    static func snapshot(_ boundary: Boundary) throws -> Boundary.Snapshot {
        guard let snapshot = boundary.trySnapshot() else { throw Failure(message: "Stable publication could not be read") }
        return snapshot
    }
    static func observe(_ boundary: Boundary, _ start: Int64, _ count: Int = 512) -> Boundary.Observation {
        boundary.observeQuantum(frame: start, frames: count, sampleRate: rate, epoch: epoch)
    }

    static func lifecycle() throws {
        let boundary = try Boundary(sampleRate: rate, epoch: epoch)
        try require(boundary.sampleRate == rate && boundary.epoch == epoch, "Fixed source identity changed")
        let initial = try snapshot(boundary)
        try require(initial.revision == 0 && initial.startFrame == nil && initial.endFrame == nil &&
                    initial.lastObservedStart == nil && initial.lastObservedThrough == nil && initial.errorCode == .none,
                    "New boundary guessed an unobserved clock")
        try require(initial.errorDescription == nil && boundary.trySnapshot(maxAttempts: 0) == nil && boundary.trySnapshot(maxAttempts: -1) == nil,
                    "Bounded snapshot or empty-error contract failed")
        try require(observe(boundary, -29) == .started, "Valid signed first quantum did not start")
        let first = try snapshot(boundary)
        try require(first.revision == 2 && first.startFrame == -29 && first.endFrame == nil &&
                    first.lastObservedStart == -29 && first.lastObservedThrough == 483, "First quantum published incorrect coordinates")
        try require(observe(boundary, -29) == .duplicate, "Exact latest repeat was not ignored")
        try require(try snapshot(boundary) == first, "Duplicate changed a publication")
        try require(observe(boundary, 483, 4_800) == .advanced, "Contiguous variable quantum was rejected")
        boundary.requestStop()
        boundary.requestStop()
        try require(observe(boundary, 483, 4_800) == .duplicate, "Stop mailbox changed duplicate classification")
        try require(try snapshot(boundary).endFrame == nil, "Cached repeat consumed a stop request")
        try require(observe(boundary, 5_283, 37) == .ended, "Forward source boundary did not accept stop")
        let ended = try snapshot(boundary)
        try require(ended.revision == 6 && ended.startFrame == -29 && ended.endFrame == 5_283 &&
                    ended.lastObservedStart == 5_283 && ended.lastObservedThrough == 5_320 && ended.errorCode == .none,
                    "End was inferred from prior batch size or wall time")
        boundary.requestStop()
        try require(observe(boundary, 5_320) == .closed, "Ended boundary accepted another quantum")
        try require(boundary.observeQuantum(sampleTime: .nan, frames: 0, sampleRate: .nan,
                    epoch: 0, sampleTimeValid: false, failedRender: true) == .closed,
                    "Post-end failure changed the accepted interval")
        try require(try snapshot(boundary) == ended, "Closed state changed")

        let empty = try Boundary(sampleRate: rate, epoch: epoch)
        empty.requestStop()
        try require(try snapshot(empty).startFrame == nil, "Stop request fabricated a starting clock")
        try require(observe(empty, 1_001, 4_800) == .ended, "Pre-start stop did not produce a valid empty interval")
        let zero = try snapshot(empty)
        try require(zero.startFrame == 1_001 && zero.endFrame == 1_001 && zero.errorCode == .none,
                    "Pre-start stop selected unrequested PCM")
    }

    static func invalidInputs() throws {
        for invalid in [Double.nan, .infinity, -.infinity, 0, -1] {
            do { _ = try Boundary(sampleRate: invalid, epoch: epoch); throw Failure(message: "Invalid fixed sample rate accepted") }
            catch let error as Boundary.ErrorCode { try require(error == .invalidSampleRate, "Wrong rate configuration error") }
        }
        let cases: [(Double, Bool)] = [(.nan, true), (.infinity, true), (-.infinity, true),
                                      (0.5, true), (-0.5, true), (9_223_372_036_854_775_808, true),
                                      (-9_223_372_036_854_777_856, true), (0, false)]
        for (time, valid) in cases {
            let boundary = try Boundary(sampleRate: rate, epoch: epoch)
            try require(boundary.observeQuantum(sampleTime: time, frames: 512, sampleRate: rate,
                        epoch: epoch, sampleTimeValid: valid) == .failed, "Invalid timestamp accepted")
            let failure = try snapshot(boundary)
            try require(failure.errorCode == .invalidTimestamp && failure.startFrame == nil && failure.endFrame == nil &&
                        failure.errorSampleTime.bitPattern == time.bitPattern && failure.errorDescription != nil,
                        "Invalid timestamp details or state were lost")
            boundary.requestStop()
            try require(observe(boundary, 0) == .failed && (try snapshot(boundary)).revision == failure.revision,
                        "A timestamp failure did not remain sticky")
        }
        let minimum = try Boundary(sampleRate: rate, epoch: epoch)
        try require(minimum.observeQuantum(sampleTime: Double(Int64.min), frames: 1, sampleRate: rate,
                    epoch: epoch, sampleTimeValid: true) == .started, "Exact signed minimum timestamp rejected")
        try require(try snapshot(minimum).startFrame == Int64.min, "Signed minimum conversion changed its coordinate")
        let maximum = try Boundary(sampleRate: rate, epoch: epoch)
        try require(observe(maximum, .max - 1, 1) == .started, "Valid last exclusive coordinate rejected")
        try require(observe(maximum, .max, 1) == .failed, "Frame end overflow accepted")
        try require(try snapshot(maximum).errorCode == .frameOverflow, "Wrong frame overflow error")

        let validations: [(Boundary.ErrorCode, Int, Double, UInt64, Bool)] = [
            (.invalidFrameCount, 0, rate, epoch, false), (.invalidFrameCount, -1, rate, epoch, false),
            (.invalidSampleRate, 1, .nan, epoch, false), (.invalidSampleRate, 1, 0, epoch, false),
            (.sampleRateChanged, 1, 44_100, epoch, false), (.epochChanged, 1, rate, epoch + 1, false),
            (.renderFailed, 1, rate, epoch, true)
        ]
        for (code, count, incomingRate, incomingEpoch, failed) in validations {
            let boundary = try Boundary(sampleRate: rate, epoch: epoch)
            try require(boundary.observeQuantum(frame: 100, frames: count, sampleRate: incomingRate,
                        epoch: incomingEpoch, failedRender: failed) == .failed, "Invalid source quantum accepted")
            let failure = try snapshot(boundary)
            try require(failure.errorCode == code && failure.errorFrame == 100 && failure.errorFrameCount == Int64(count) &&
                        failure.errorSampleRate.bitPattern == incomingRate.bitPattern && failure.errorEpoch == incomingEpoch &&
                        failure.startFrame == nil && failure.lastObservedThrough == nil, "First failure details changed")
            try require(observe(boundary, 100) == .failed && (try snapshot(boundary)).revision == failure.revision,
                        "Invalid source error was not sticky")
        }
    }

    static func discontinuities() throws {
        let invalid: [(Int64, Int, Boundary.ErrorCode)] = [
            (513, 512, .gap), (1, 512, .overlap), (0, 513, .overlap),
            (0, 511, .overlap), (-1, 512, .regression)
        ]
        for (frame, frames, code) in invalid {
            let boundary = try Boundary(sampleRate: rate, epoch: epoch)
            _ = observe(boundary, 0)
            boundary.requestStop()
            try require(observe(boundary, frame, frames) == .failed, "Discontinuity consumed stop instead of failing")
            let failure = try snapshot(boundary)
            try require(failure.errorCode == code && failure.endFrame == nil && failure.startFrame == 0 &&
                        failure.lastObservedStart == 0 && failure.lastObservedThrough == 512,
                        "Invalid quantum changed accepted source coordinates")
            try require(observe(boundary, 512) == .failed && (try snapshot(boundary)).revision == failure.revision,
                        "Discontinuity error recovered silently")
        }
        let priorDuplicate = try Boundary(sampleRate: rate, epoch: epoch)
        _ = observe(priorDuplicate, 0)
        _ = observe(priorDuplicate, 512)
        try require(observe(priorDuplicate, 0) == .failed && (try snapshot(priorDuplicate)).errorCode == .regression,
                    "Old cached quantum was mistaken for the latest exact duplicate")
        let failedRepeat = try Boundary(sampleRate: rate, epoch: epoch)
        _ = observe(failedRepeat, 0)
        try require(failedRepeat.observeQuantum(frame: 0, frames: 512, sampleRate: rate, epoch: epoch, failedRender: true) == .failed &&
                    (try snapshot(failedRepeat)).errorCode == .renderFailed, "Failed repeated render was silently ignored")
        let changedRepeat = try Boundary(sampleRate: rate, epoch: epoch)
        _ = observe(changedRepeat, 0)
        try require(changedRepeat.observeQuantum(frame: 0, frames: 512, sampleRate: rate, epoch: epoch + 1) == .failed &&
                    (try snapshot(changedRepeat)).errorCode == .epochChanged, "Clock change was hidden by repeat detection")
    }

    static func windowIntegration() throws {
        let boundary = try Boundary(sampleRate: rate, epoch: epoch)
        _ = observe(boundary, 1_024)
        for frame in stride(from: Int64(1_536), through: Int64(9_728), by: 512) { _ = observe(boundary, frame) }
        boundary.requestStop()
        _ = observe(boundary, 10_240)
        let accepted = try snapshot(boundary)
        let clock = AudioRecordingWindow.Clock(id: "epoch-71", sampleRate: rate)
        var window = try AudioRecordingWindow(clock: clock, startFrame: accepted.startFrame!, endFrame: accepted.endFrame!)
        var selected: [Int64] = []
        // Tap batches arrive asynchronously and are larger than render quanta.
        for start: Int64 in [0, 4_800, 4_800, 9_600] {
            let selection = try window.receive(startFrame: start, frameCount: 4_800, clock: clock)
            if selection.count > 0 {
                selected.append(contentsOf: (0..<selection.count).map { selection.sourceStartFrame + Int64($0) })
            }
            if start < 9_600 { try require(!window.isComplete, "Accepted source end was confused with received PCM watermark") }
        }
        try require(window.isComplete && selected == Array(Int64(1_024)..<Int64(10_240)),
                    "Render-boundary fence plus batch trimming lost or repeated PCM")
    }

    static func concurrentSnapshots() throws {
        let boundary = try Boundary(sampleRate: rate, epoch: epoch)
        let group = DispatchGroup()
        // Readers each own one result cell. Main reads only after group.wait.
        let results = UnsafeMutablePointer<Int>.allocate(capacity: 6)
        results.initialize(repeating: 0, count: 6)
        defer { results.deinitialize(count: 6); results.deallocate() }
        group.enter()
        DispatchQueue.global().async {
            for frame in 0..<20_000 { _ = observe(boundary, Int64(frame), 1) }
            boundary.requestStop()
            _ = observe(boundary, 20_000, 1)
            group.leave()
        }
        for reader in 0..<3 {
            group.enter()
            DispatchQueue.global().async {
                var observed = 0, invalid = 0
                var priorRevision: UInt64 = 0
                for _ in 0..<30_000 {
                    guard let value = boundary.trySnapshot(maxAttempts: 2) else { continue }
                    observed += 1
                    if value.revision & 1 != 0 || value.revision < priorRevision || value.errorCode != .none { invalid += 1 }
                    priorRevision = value.revision
                    if let start = value.startFrame, let last = value.lastObservedStart, let through = value.lastObservedThrough {
                        if start != 0 || through != last + 1 || value.revision != UInt64(last + 1) * 2 { invalid += 1 }
                        if let end = value.endFrame, end != 20_000 || last != 20_000 { invalid += 1 }
                    } else if value.revision != 0 || value.endFrame != nil { invalid += 1 }
                }
                results[reader * 2] = observed
                results[reader * 2 + 1] = invalid
                group.leave()
            }
        }
        group.wait()
        for reader in 0..<3 {
            try require(results[reader * 2] > 0 && results[reader * 2 + 1] == 0,
                        "Concurrent reader received a torn or regressing publication")
        }
        let final = try snapshot(boundary)
        try require(final.startFrame == 0 && final.endFrame == 20_000 && final.lastObservedThrough == 20_001 && final.errorCode == .none,
                    "Concurrent snapshot consumers changed writer state")
    }

    static func concurrentStopMailbox() throws {
        let boundary = try Boundary(sampleRate: rate, epoch: epoch)
        let began = DispatchSemaphore(value: 0), requested = DispatchSemaphore(value: 0)
        let finished = DispatchSemaphore(value: 0)
        let results = UnsafeMutablePointer<Int>.allocate(capacity: 2)
        results.initialize(repeating: 0, count: 2)
        defer { results.deinitialize(count: 2); results.deallocate() }
        DispatchQueue.global().async {
            _ = observe(boundary, 0)
            began.signal()
            requested.wait() // Test scheduling only; the helper never waits.
            results[0] = observe(boundary, 0) == .duplicate ? 1 : 0
            results[1] = observe(boundary, 512) == .ended ? 1 : 0
            finished.signal()
        }
        began.wait()
        let before = try snapshot(boundary)
        try require(before.startFrame == 0 && before.endFrame == nil, "Concurrent source did not publish start")
        boundary.requestStop()
        boundary.requestStop()
        try require(try snapshot(boundary) == before, "Control stop modified render-owned state directly")
        requested.signal()
        finished.wait()
        let after = try snapshot(boundary)
        try require(results[0] == 1 && results[1] == 1 && after.endFrame == 512 && after.errorCode == .none,
                    "Concurrent stop was lost or accepted on a cached duplicate")
    }

    static func main() throws {
        try lifecycle()
        try invalidInputs()
        try discontinuities()
        try windowIntegration()
        try concurrentSnapshots()
        try concurrentStopMailbox()
        let report: [String: Any] = ["passed": true, "assertions": checks,
            "concurrentSnapshotAttempts": 90_000, "sourceQuantumPublications": 20_001,
            "scope": "Pure scalar clock fences and original deterministic PCM; no audio engines, hardware, vendors, or GUI."]
        let data = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
        print(String(decoding: data, as: UTF8.self))
    }
}

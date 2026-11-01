// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation

/// Exercises the production keyboard-input bridge and its timer-owned ACK ledger.
/// Original native synthesis and private manual graphs only; no devices or AUs.
enum NativeInputAcknowledgementTests {
    private static let rate = 48_000.0

    @MainActor static func run(output: URL) async throws -> [String] {
        let folder = output.appendingPathComponent("input-acknowledgements", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        var checks: [String] = [], observations: [[String: Any]] = []
        var completed = false
        defer {
            let report: [String: Any] = ["passed": completed, "checks": checks, "observations": observations,
                "sampleRate": rate, "physicalPlayback": false, "physicalMIDI": false,
                "microphoneAccess": false, "vendorPlugins": false]
            if let data = try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]) {
                try? data.write(to: folder.appendingPathComponent("metrics.json"), options: .atomic)
            }
        }

        do {
            let fixture = try await Fixture.make(session: "input-rows")
            defer { fixture.close() }
            try await fixture.arm(true)
            _ = try await fixture.host.handle("transport.play", payload: ["mode": "song", "fromStart": true])
            _ = try fixture.render(6000)
            let prior = fixture.host.status()
            let onsetRequest = try await begin(fixture.host, owner: "row-key", on: true)
            _ = try fixture.render(1)
            let onset = try await resolve(onsetRequest)
            try applied(onset, owner: "row-key", on: true, playing: true, recording: true)
            try location(onset, pattern: 7, row: 1, sequence: 0, frame: 6000, beat: 0.25)
            // 120 BPM: frame 42000 is beat 1.75, row 3 of the second pattern, regardless of UI polling.
            _ = try fixture.render(35_999)
            let releaseRequest = try await begin(fixture.host, owner: "row-key", on: false)
            _ = try fixture.render(1)
            let release = try await resolve(releaseRequest)
            try applied(release, owner: "row-key", on: false, playing: true, recording: true)
            try location(release, pattern: 19, row: 3, sequence: 1, frame: 42_000, beat: 1.75)
            let firstClock = try clock(onset), lastClock = try clock(release)
            try require(firstClock["runID"] as? String == lastClock["runID"] as? String &&
                firstClock["timingRevision"] as? String == lastClock["timingRevision"] as? String,
                "Pattern boundaries must preserve the same input recording run.")
            let gate = (number(lastClock, "distanceBeats") - number(firstClock, "distanceBeats")) / 0.25
            try require(abs(gate - 6) < 1e-11, "The gate must span six onset-row spacings across the pattern change.")
            try require(integer(prior, "row") == 0 && integer(onset, "row") == 1,
                "The onset must use the newly applied row rather than the previous status row.")
            observations.append(["name": "row-boundary-and-gate", "statusRowBefore": integer(prior, "row"),
                "onset": onset, "release": release, "gate": gate])
            checks.append("Applied input crosses the exact row boundary, preserves saved pattern IDs, and measures a six-row gate across a pattern change")
        }

        do {
            let tracked = try await Fixture.make(session: "input-shared-tracked")
            let ordinary = try await Fixture.make(session: "input-shared-ordinary")
            defer { tracked.close(); ordinary.close() }
            for fixture in [tracked, ordinary] {
                try await fixture.arm(true)
                _ = try await fixture.host.handle("transport.play", payload: ["mode": "song", "fromStart": true])
                try await fixture.note(on: true)
            }
            var maximumDifference = try compare(tracked.render(257), ordinary.render(257), label: "Shared-note initial voice")
            let sharedOn = try await begin(tracked.host, owner: "second-owner", on: true, applyNote: false)
            maximumDifference = max(maximumDifference, try compare(tracked.render(511), ordinary.render(511), label: "Capture-only shared onset"))
            let onset = try await resolve(sharedOn)
            try applied(onset, owner: "second-owner", on: true, playing: true, recording: true, applyNote: false)
            let sharedOff = try await begin(tracked.host, owner: "second-owner", on: false, applyNote: false)
            let sustained = try tracked.render(509)
            maximumDifference = max(maximumDifference, try compare(sustained, ordinary.render(509), label: "Capture-only shared release"))
            let release = try await resolve(sharedOff)
            try applied(release, owner: "second-owner", on: false, playing: true, recording: true, applyNote: false)
            try require(peak(sustained) > 0.01, "Capture-only release must leave the shared sounding voice active.")
            let releaseActual = try await begin(tracked.host, owner: "first-owner", on: false)
            try await ordinary.note(on: false)
            maximumDifference = max(maximumDifference, try compare(tracked.render(2048), ordinary.render(2048), label: "Final sounding release"))
            let actual = try await resolve(releaseActual)
            try applied(actual, owner: "first-owner", on: false, playing: true, recording: true)
            observations.append(["name": "capture-only-shared-edge", "onset": onset, "release": release,
                "finalRelease": actual, "maximumPCMDifference": maximumDifference])
            checks.append("Capture-only shared onset/release acknowledges both edges without retriggering or releasing the sounding native voice")
        }

        do {
            let fixture = try await Fixture.make(session: "input-stopped")
            defer { fixture.close() }
            try await fixture.warmStopped()
            try await fixture.arm(true)
            let request = try await begin(fixture.host, owner: "stopped-key", on: true)
            let pcm = try fixture.render(64)
            let ack = try await resolve(request)
            try applied(ack, owner: "stopped-key", on: true, playing: false, recording: true)
            try require(ack["recordingClock"] == nil && peak(pcm) > 0.00001,
                "A stopped applied edge must sound normally but must not invent a live recording clock.")
            observations.append(["name": "stopped-applied-edge", "acknowledgement": ack, "peak": peak(pcm)])
            checks.append("An applied note while transport is stopped remains audible and provides no live gate endpoint")
        }

        do {
            let tracked = try await Fixture.make(session: "input-timeout-tracked")
            let ordinary = try await Fixture.make(session: "input-timeout-ordinary")
            defer { tracked.close(); ordinary.close() }
            try await tracked.warmStopped(); try await ordinary.warmStopped()
            let started = ContinuousClock.now
            let request = try await begin(tracked.host, owner: "timeout-key", on: true)
            // No audio quantum follows this accepted edge until its ownership
            // expires. The delayed command must remain a single performance edge.
            let ack = try await resolve(request, timeout: 3)
            let elapsed = seconds(started.duration(to: ContinuousClock.now))
            try unavailable(ack, owner: "timeout-key")
            try require(ack["requestID"] as? String != nil && elapsed >= 0.8 && elapsed < 3,
                "An accepted unrendered input edge must expire within the bounded acknowledgement wait.")
            try await ordinary.note(on: true)
            var maximumDifference = try compare(tracked.render(1024), ordinary.render(1024), label: "Late accepted edge after timeout")
            // A fresh ACK proves that the timer drained the old expired entry as
            // well as the new one, without retrying the old audible operation.
            let probe = try await begin(tracked.host, owner: "after-timeout", on: false, applyNote: false)
            maximumDifference = max(maximumDifference, try compare(tracked.render(1024), ordinary.render(1024), label: "Expired ACK drain"))
            let next = try await resolve(probe)
            try applied(next, owner: "after-timeout", on: false, playing: false, recording: false, applyNote: false)
            try increasingIDs(ack, next)
            maximumDifference = max(maximumDifference, try compare(tracked.render(1024), ordinary.render(1024), label: "No delayed performance retry"))
            observations.append(["name": "no-progress-timeout", "elapsedSeconds": elapsed,
                "expired": ack, "next": next, "maximumPCMDifference": maximumDifference])
            checks.append("A no-progress ACK timeout returns unavailable, drains its late acknowledgement, and never duplicates the accepted performance note")
        }

        do {
            let fixture = try await Fixture.make(session: "input-fenced")
            defer { fixture.close() }
            try await fixture.arm(true)
            _ = try await fixture.host.handle("transport.play", payload: ["mode": "song", "fromStart": true])
            _ = try fixture.render(64)
            let disarmed = try await begin(fixture.host, owner: "disarmed-owner", on: true)
            _ = try fixture.render(1)
            _ = try await fixture.host.handle("transport.recordArm", payload: ["armed": false])
            let disarmAck = try await resolve(disarmed)
            try unavailable(disarmAck, owner: "disarmed-owner")
            try await fixture.arm(true)
            let replaced = try await begin(fixture.host, owner: "old-session-owner", on: true)
            _ = try fixture.render(1)
            var replacement = fixture.project
            replacement["id"] = "input-replacement"
            _ = try await fixture.host.handle("engine.project", payload: ["project": replacement, "replace": true, "awaitPlugins": true])
            let replacementAck = try await resolve(replaced)
            try unavailable(replacementAck, owner: "old-session-owner")
            try require(fixture.host.status()["noteRecordArmed"] as? Bool == false,
                "Replacing the session must clear record-arm intent.")
            try increasingIDs(disarmAck, replacementAck)
            observations.append(["name": "ownership-fences", "disarmed": disarmAck, "replacement": replacementAck])
            checks.append("Disarming and replacing the session reject already-rendered acknowledgements before they can attach to a stale recording owner")
        }

        do {
            let fixture = try await Fixture.make(session: "input-scalar-edit")
            defer { fixture.close() }
            try await fixture.arm(true)
            _ = try await fixture.host.handle("transport.play", payload: ["mode": "song", "fromStart": true])
            _ = try fixture.render(64)
            let pending = try await begin(fixture.host, owner: "scalar-owner", on: true)
            _ = try fixture.render(1)
            var edited = fixture.project
            edited["masterVolume"] = 0.4
            edited["name"] = "Same session with an ordinary scalar edit"
            _ = try await fixture.host.handle("engine.project", payload: ["project": edited, "replace": false, "awaitPlugins": true])
            let onset = try await resolve(pending)
            try applied(onset, owner: "scalar-owner", on: true, playing: true, recording: true)
            _ = try fixture.render(128)
            let releaseRequest = try await begin(fixture.host, owner: "scalar-owner", on: false)
            _ = try fixture.render(1)
            let release = try await resolve(releaseRequest)
            try applied(release, owner: "scalar-owner", on: false, playing: true, recording: true)
            let before = try clock(onset), after = try clock(release)
            try require(before["runID"] as? String == after["runID"] as? String &&
                before["timingRevision"] as? String == after["timingRevision"] as? String,
                "An ordinary same-session scalar edit must not change the recording clock run.")
            let onsetContext = onset["recordingContext"] as? [String: Any] ?? [:]
            let releaseContext = release["recordingContext"] as? [String: Any] ?? [:]
            try require(NSDictionary(dictionary: onsetContext).isEqual(to: releaseContext) && !onsetContext.isEmpty,
                "The request-local recording context must exclude ordinary mix/name edits.")
            try require(number(after, "distanceBeats") > number(before, "distanceBeats"),
                "The retained recording run must still advance after the scalar publication.")
            observations.append(["name": "same-session-scalar-edit", "onset": onset, "release": release])
            checks.append("An ordinary same-session scalar edit preserves pending ownership, captured musical context, and the live recording run")
        }
        completed = true
        return checks
    }

    @MainActor private final class Pending {
        var entered = false
        var finished = false
        var task: Task<[String: Any], Error>?
    }

    @MainActor private static func begin(_ host: AudioHost, owner: String, on: Bool, applyNote: Bool = true) async throws -> Pending {
        let pending = Pending()
        pending.task = Task { @MainActor in
            pending.entered = true
            defer { pending.finished = true }
            let response = try await host.handle(on ? "note.on" : "note.off", payload: ["captureInput": true,
                "inputOwner": owner, "applyNote": applyNote, "track": 0, "instrument": 0,
                "note": 60, "velocity": on ? 100 : 0]) as? [String: Any] ?? [:]
            guard response["ok"] as? Bool == true, let ack = response["inputAcknowledgement"] as? [String: Any] else {
                throw failure("The native input response omitted its acknowledgement envelope.")
            }
            return ack
        }
        // Both functions are MainActor-isolated. The note command's synchronous
        // prefix enqueues the edge before suspending on its ACK continuation.
        try await waitUntil(timeout: 1, message: "The input task did not enter its command handler.") { pending.entered }
        try require(!pending.finished, "A valid captured edge must wait for its applied audio frame.")
        return pending
    }

    @MainActor private static func resolve(_ pending: Pending, timeout: Double = 2) async throws -> [String: Any] {
        try await waitUntil(timeout: timeout, message: "The host did not complete its bounded acknowledgement wait.") { pending.finished }
        return try await pending.task!.value
    }

    @MainActor private static func waitUntil(timeout: Double, message: String, predicate: () -> Bool) async throws {
        let deadline = ContinuousClock.now.advanced(by: .seconds(timeout))
        while !predicate() {
            try require(ContinuousClock.now < deadline, message)
            try await Task.sleep(for: .milliseconds(1))
        }
    }

    @MainActor private final class Fixture {
        let graph: AVAudioEngine
        let host: AudioHost
        let buffer: AVAudioPCMBuffer
        let project: [String: Any]

        private init(session: String) throws {
            graph = AVAudioEngine()
            let format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 2)!
            try graph.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
            host = try AudioHost(audioGraph: graph, midiInputIDs: [], midiInputSourceProvider: { [] })
            buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4096)!
            let data: [String: Any] = ["version": 1, "id": session, "name": "Original input acknowledgement fixture",
                "bpm": 120, "linesPerBeat": 4, "swing": 0, "masterVolume": 0.5,
                "activePattern": 7, "sequence": [7, 19], "loopSong": false, "metronome": false,
                "tracks": [["routingID": "stable-input-track", "name": "Native sine", "instrument": 0,
                    "volume": 0.5, "pan": 0, "delaySend": 0, "reverbSend": 0]],
                "instruments": [["id": "stable-input-instrument", "type": "synth", "waveform": "sine",
                    "volume": 0.5, "attack": 0, "decay": 0, "sustain": 1, "release": 0.01]],
                "patterns": [7, 19].map { ["id": $0, "length": 4, "steps": [[Any]](), "automation": []] as [String: Any] },
                "audioPlugins": [], "delay": ["mix": 0], "reverb": ["mix": 0]]
            project = data
        }

        static func make(session: String) async throws -> Fixture {
            let fixture = try Fixture(session: session)
            do {
                _ = try await fixture.host.handle("engine.project", payload: ["project": fixture.project, "replace": true, "awaitPlugins": true])
                return fixture
            } catch { fixture.close(); throw error }
        }

        func close() { host.shutdown() }
        func arm(_ value: Bool) async throws {
            _ = try await host.handle("transport.recordArm", payload: ["armed": value])
        }
        func note(on: Bool, velocity: Int = 100) async throws {
            _ = try await host.handle(on ? "note.on" : "note.off", payload: ["track": 0, "instrument": 0,
                "note": 60, "velocity": on ? velocity : 0])
        }
        func warmStopped() async throws {
            // Start through the real host, consume its initial graph/run reset,
            // and silence this throwaway voice before any captured input edge.
            try await note(on: true, velocity: 1)
            _ = try await host.handle("transport.seek", payload: ["pattern": 7, "row": 0, "sequence": 0])
            _ = try await host.handle("engine.panic", payload: [:])
            _ = try render(1)
            try require(host.status()["playing"] as? Bool == false, "The stopped fixture accidentally started transport.")
        }
        func render(_ frames: Int) throws -> [[Float]] {
            var remaining = frames
            var result = [[Float](), [Float]()]
            while remaining > 0 {
                let count = AVAudioFrameCount(min(remaining, 4096))
                let status = try host.renderOfflineQuantum(frames: count, to: buffer)
                try require(status == .success && buffer.frameLength == count,
                    "Private offline input fixture failed to render \(count) frames: \(status.rawValue).")
                guard let data = buffer.floatChannelData else { throw failure("The private input fixture has no Float32 PCM.") }
                for channel in 0..<2 { result[channel].append(contentsOf: UnsafeBufferPointer(start: data[channel], count: Int(count))) }
                remaining -= Int(count)
            }
            return result
        }
    }

    private static func applied(_ ack: [String: Any], owner: String, on: Bool, playing: Bool,
                                recording: Bool, applyNote: Bool = true) throws {
        try require(integer(ack, "version") == 1 && ack["status"] as? String == "applied" &&
            ack["applied"] as? Bool == true && ack["inputOwner"] as? String == owner &&
            ack["on"] as? Bool == on && ack["applyNote"] as? Bool == applyNote &&
            ack["playing"] as? Bool == playing && ack["recording"] as? Bool == recording &&
            ack["mapped"] as? Bool == false && integer(ack, "track") == 0 && integer(ack, "instrument") == 0 &&
            integer(ack, "note") == 60 && integer(ack, "velocity") == (on ? 100 : 0),
            "Applied input acknowledgement changed its captured edge or status: \(ack).")
        _ = try canonicalUInt(ack["requestID"], label: "requestID")
        try require(!(ack["recordingContext"] as? [String: Any] ?? [:]).isEmpty,
            "An input acknowledgement must retain its request-local recording context.")
        if playing { _ = try clock(ack) }
    }

    private static func unavailable(_ ack: [String: Any], owner: String) throws {
        try require(ack["inputOwner"] as? String == owner && ack["status"] as? String == "unavailable" &&
            ack["applied"] as? Bool == false && ack["playing"] as? Bool == false &&
            ack["recordingClock"] == nil && !(ack["reason"] as? String ?? "").isEmpty,
            "A stale or timed-out request must explicitly reject its live recording endpoint: \(ack).")
        _ = try canonicalUInt(ack["requestID"], label: "rejected requestID")
    }

    private static func clock(_ ack: [String: Any]) throws -> [String: Any] {
        guard let value = ack["recordingClock"] as? [String: Any] else { throw failure("The applied playing edge omitted its recording clock.") }
        try require(number(value, "distanceBeats").isFinite && number(value, "distanceBeats") >= 0,
            "The recording clock has an invalid musical distance.")
        _ = try canonicalUInt(value["runID"], label: "runID")
        _ = try canonicalUInt(value["timingRevision"], label: "timingRevision", allowZero: true)
        return value
    }

    private static func location(_ ack: [String: Any], pattern: Int, row: Int, sequence: Int, frame: Int64, beat: Double) throws {
        try require(integer(ack, "pattern") == pattern && integer(ack, "row") == row && integer(ack, "sequence") == sequence &&
            ack["renderFrame"] as? String == String(frame) && ack["graphFrame"] as? String == String(frame),
            "The applied edge has an incorrect saved pattern, row, occurrence or sample frame: \(ack).")
        try require(abs(number(try clock(ack), "distanceBeats") - beat) < 1e-11,
            "The applied input clock disagrees with the hand-calculated tempo map.")
    }

    private static func increasingIDs(_ first: [String: Any], _ last: [String: Any]) throws {
        let a = try canonicalUInt(first["requestID"], label: "first requestID")
        let b = try canonicalUInt(last["requestID"], label: "last requestID")
        try require(b > a, "Input request IDs must increase even after an expired or stale request.")
    }
    private static func canonicalUInt(_ raw: Any?, label: String, allowZero: Bool = false) throws -> UInt64 {
        guard let text = raw as? String, let value = UInt64(text), String(value) == text, allowZero || value > 0 else {
            throw failure("The \(label) must be a canonical unsigned decimal string.")
        }
        return value
    }
    private static func compare(_ a: [[Float]], _ b: [[Float]], label: String) throws -> Float {
        try require(a.count == 2 && b.count == 2 && a[0].count == b[0].count && a[1].count == b[1].count,
            "\(label) returned different PCM dimensions.")
        var difference: Float = 0
        for channel in 0..<2 {
            for frame in a[channel].indices {
                try require(a[channel][frame].isFinite && b[channel][frame].isFinite, "\(label) produced nonfinite PCM.")
                difference = max(difference, abs(a[channel][frame] - b[channel][frame]))
            }
        }
        try require(difference == 0, "\(label) duplicated, retriggered or released native performance audio; max difference \(difference).")
        return difference
    }
    private static func peak(_ pcm: [[Float]]) -> Float { pcm.flatMap { $0 }.reduce(0) { max($0, abs($1)) } }
    private static func number(_ object: [String: Any], _ key: String) -> Double { (object[key] as? NSNumber)?.doubleValue ?? -.infinity }
    private static func integer(_ object: [String: Any], _ key: String) -> Int { (object[key] as? NSNumber)?.intValue ?? -1 }
    private static func seconds(_ duration: Duration) -> Double {
        let value = duration.components
        return Double(value.seconds) + Double(value.attoseconds) / 1_000_000_000_000_000_000
    }
    private static func require(_ condition: Bool, _ message: String) throws { if !condition { throw failure(message) } }
    private static func failure(_ message: String) -> AudioSampleError { .invalid("Input acknowledgement regression: " + message) }
}

#endif

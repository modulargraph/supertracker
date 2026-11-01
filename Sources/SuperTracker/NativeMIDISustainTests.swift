// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation

/// Native Host packet ingress -> public Host configuration -> native synth PCM.
/// Synthetic source IDs are passed directly to the production packet handler; no MIDI
/// sources/endpoints are created and no packets are broadcast. CoreMIDI parsing is not
/// covered here. The graph enters offline mode before Host creation, with all inputs off.
enum NativeMIDISustainTests {
    private static let rate = 48_000.0

    @MainActor static func run(output: URL) async throws -> [String] {
        let folder = output.appendingPathComponent("midi-sustain", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        var checks: [String] = [], observations: [[String: Any]] = []
        var completed = false
        defer {
            let report: [String: Any] = ["passed": completed, "checks": checks, "observations": observations,
                "sampleRate": rate, "physicalPlayback": false, "physicalMIDI": false,
                "microphoneAccess": false, "vendorPlugins": false, "coreMIDIParsing": false,
                "midiBroadcasts": false, "inputPath": "direct production Host packet ingress"]
            if let data = try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]) {
                try? data.write(to: folder.appendingPathComponent("metrics.json"), options: .atomic)
            }
        }
        let fixture = try await Fixture.make()
        do {
            func checkPCM(_ name: String, left: Bool, right: Bool, using selected: Fixture? = nil) throws {
                let subject = selected ?? fixture
                let levels = try subject.levels()
                observations.append(["name": name, "leftRMS": levels[0], "rightRMS": levels[1],
                    "leftExpectedAudible": left, "rightExpectedAudible": right,
                    "graphFrame": String(subject.graph.manualRenderingSampleTime),
                    "ingressPackets": subject.ingressPackets])
                for channel in 0..<2 {
                    let audible = channel == 0 ? left : right
                    try require(levels[channel].isFinite && (audible ? levels[channel] > 0.0001 : levels[channel] < 0.000001),
                        "\(name): channel \(channel) RMS \(levels[channel]), expected \(audible ? "held tone" : "silence").")
                }
            }

            // Same MIDI channel, independent source IDs. A owns the left voice;
            // B's released right voice must not inherit A's pedal state.
            try await fixture.reset()
            try await fixture.send(.a, 0xB0, 64, 127)
            try await fixture.send(.a, 0x90, 60, 100)
            try await fixture.send(.a, 0x80, 60, 0)
            try checkPCM("source-A-pedal-holds-A", left: true, right: false)
            try await fixture.target(track: 1)
            try await fixture.send(.b, 0x90, 67, 100)
            try checkPCM("source-B-note-on", left: true, right: true)
            try await fixture.send(.b, 0x80, 67, 0)
            try checkPCM("source-A-pedal-does-not-hold-B", left: true, right: false)
            try await fixture.send(.a, 0xB0, 64, 0)
            try checkPCM("source-A-own-pedal-release", left: false, right: false)
            checks.append("A pedal cannot hold a released note from another synthetic source on the same channel")

            // Same source, different MIDI channels.
            try await fixture.reset()
            try await fixture.send(.a, 0xB0, 64, 127)
            try await fixture.send(.a, 0x90, 60, 100)
            try await fixture.send(.a, 0x80, 60, 0)
            try await fixture.target(track: 1)
            try await fixture.send(.a, 0x91, 67, 100)
            try checkPCM("same-source-channel-2-note-on", left: true, right: true)
            try await fixture.send(.a, 0x81, 67, 0)
            try checkPCM("channel-1-pedal-does-not-hold-channel-2", left: true, right: false)
            try await fixture.send(.a, 0xB0, 64, 0)
            try checkPCM("channel-1-own-release", left: false, right: false)
            checks.append("A pedal cannot hold a released note from another channel of the same synthetic source")

            // Pedal-up, as well as pedal-down, is scoped to its owning input.
            try await fixture.reset()
            try await fixture.send(.a, 0xB0, 64, 127)
            try await fixture.send(.a, 0x90, 60, 100)
            try await fixture.send(.a, 0x80, 60, 0)
            try checkPCM("unrelated-release-baseline", left: true, right: false)
            try await fixture.send(.b, 0xB0, 64, 0)
            try checkPCM("source-B-up-retains-A", left: true, right: false)
            try await fixture.send(.a, 0xB1, 64, 0)
            try checkPCM("channel-2-up-retains-channel-1", left: true, right: false)
            try await fixture.send(.a, 0xB0, 64, 0)
            try checkPCM("only-owner-up-releases-A", left: false, right: false)
            checks.append("Unrelated source/channel pedal-up leaves A sustained until its own pedal-up")

            // Changing only selected track/instrument must preserve original held
            // note ownership. The old release must neither stick nor release B.
            try await fixture.reset()
            try await fixture.send(.a, 0x90, 60, 100)
            try checkPCM("target-change-held-A", left: true, right: false)
            try await fixture.target(track: 1)
            try await fixture.send(.b, 0x90, 67, 100)
            try checkPCM("target-change-both-held", left: true, right: true)
            try await fixture.send(.a, 0x80, 60, 0)
            try checkPCM("old-keyoff-releases-original-track", left: false, right: true)
            try await fixture.send(.b, 0x80, 67, 0)
            try checkPCM("new-target-keyoff", left: false, right: false)
            checks.append("Track-only target changes retain old key-off ownership without releasing the new target")

            // Instrument 1 accepts only source B / channel 2 for new notes.
            // An already owned source-A/channel-1 release still needs to pass.
            try await fixture.reset()
            try await fixture.send(.a, 0xB0, 64, 127)
            try await fixture.send(.a, 0x90, 60, 100)
            try checkPCM("filtered-target-held-A", left: true, right: false)
            try await fixture.target(track: 1, instrument: 1)
            try await fixture.send(.a, 0x80, 60, 0)
            try checkPCM("old-keyoff-is-deferred-after-target-filter-change", left: true, right: false)
            try await fixture.send(.a, 0xB0, 64, 0)
            try checkPCM("old-pedal-release-bypasses-new-note-filter", left: false, right: false)
            try await fixture.send(.a, 0x90, 72, 100)
            try checkPCM("new-note-from-filtered-source-stays-rejected", left: false, right: false)
            try await fixture.send(.b, 0x91, 67, 100)
            try checkPCM("new-target-accepted-source-channel", left: false, right: true)
            try await fixture.send(.b, 0x81, 67, 0)
            try checkPCM("filtered-target-final-release", left: false, right: false)
            checks.append("Owned pedal release survives selected-instrument source/channel filtering while new notes obey it")

            await fixture.close()
            let kitFixture = try await Fixture.make(kitMode: true)
            do {
                // Pads are member tracks of two kit groups: pad 1 of the first kit (left), pad 16 of the last (right).
                for (track, left) in [(0, true), (1, false)] {
                    try await kitFixture.reset()
                    try await kitFixture.target(track: track, instrument: track)
                    try await kitFixture.send(.a, 0xB0, 64, 127)
                    try await kitFixture.send(.a, 0x90, 60, 100)
                    try await kitFixture.send(.a, 0x80, 60, 0)
                    try checkPCM("Kit pedal holds pad track \(track)", left: left, right: !left, using: kitFixture)
                    // The pedal release still belongs to the original pad after selection changes.
                    try await kitFixture.target(track: 1 - track, instrument: 1 - track)
                    try await kitFixture.send(.a, 0xB0, 64, 0)
                    try checkPCM("Kit pedal releases pad track \(track)", left: false, right: false, using: kitFixture)
                    checks.append("Kit sustain holds and releases pad track \(track) after selecting a pad of another kit")
                }
                await kitFixture.close()
            } catch { await kitFixture.close(); throw error }
            completed = true
            return checks
        } catch {
            observations.append(["error": error.localizedDescription, "ingressPackets": fixture.ingressPackets])
            await fixture.close()
            throw error
        }
    }

    private static func require(_ value: Bool, _ message: String) throws {
        if !value { throw AudioSampleError.invalid("MIDI sustain regression: " + message) }
    }

    private enum Source {
        case a, b
        var id: Int32 { self == .a ? 1_010_101 : 2_020_202 }
    }

    @MainActor private final class Fixture {
        private(set) var ingressPackets = 0
        let graph: AVAudioEngine
        let host: AudioHost
        let buffer: AVAudioPCMBuffer
        private var closed = false

        private init() throws {
            graph = AVAudioEngine()
            let format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 2)!
            try graph.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
            host = try AudioHost(audioGraph: graph, midiInputIDs: [], midiInputSourceProvider: { [] })
            buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4096)!
        }

        static func make(kitMode: Bool = false) async throws -> Fixture {
            let fixture = try Fixture()
            do {
                let synth: [String: Any] = ["type": "synth", "waveform": "sine", "volume": 0.4,
                    "attack": 0, "decay": 0, "sustain": 1, "release": 0.001,
                    "cutoff": 18_000, "resonance": 0, "drive": 0]
                var filtered = synth
                filtered["midiInput"] = Source.b.id; filtered["midiInputChannel"] = 2
                var project: [String: Any] = ["version": 1, "id": "private-midi-sustain", "bpm": 120,
                    "linesPerBeat": 4, "masterVolume": 0.5, "activePattern": 0, "sequence": [0],
                    "loopSong": false, "metronome": false,
                    "tracks": [["name": "A left", "instrument": 0, "volume": 1, "pan": -1, "delaySend": 0, "reverbSend": 0],
                               ["name": "B right", "instrument": 0, "volume": 1, "pan": 1, "delaySend": 0, "reverbSend": 0]],
                    "instruments": [synth, filtered],
                    "patterns": [["id": 0, "length": 4, "steps": [[Any]](), "automation": []]],
                    "audioPlugins": [], "audioClips": [], "delay": ["mix": 0], "reverb": ["mix": 0]]
                if kitMode {
                    project["instruments"] = [synth, synth]
                    project["groups"] = [["id": "k0", "name": "Kit 1", "busID": "g0"], ["id": "k3", "name": "Kit 4", "busID": "g3"]]
                    project["tracks"] = [["routingID": "k0-pad0", "groupID": "k0", "pad": 0, "instrument": 0, "volume": 1, "pan": -1],
                        ["routingID": "k3-pad15", "groupID": "k3", "pad": 15, "instrument": 1, "volume": 1, "pan": 1]]
                    let buses: [[String: Any]] = [["id": "g0", "name": "Kit 1", "type": "group", "groupID": "k0", "volume": 1],
                                                  ["id": "g3", "name": "Kit 4", "type": "group", "groupID": "k3", "volume": 1]]
                    project["routing"] = ["version": 1, "mixModel": "standard", "buses": buses,
                        "outputs": ["track:k0-pad0": "bus:g0", "track:k3-pad15": "bus:g3"], "sends": [], "sidechains": []]
                }
                _ = try await fixture.host.handle("engine.project", payload: ["project": project, "replace": true, "awaitPlugins": true])
                let devices = try await fixture.host.handle("midi.configure", payload: ["inputIDs": [Int32](),
                    "inputChannel": 0, "track": 0, "instrument": 0]) as? [String: Any] ?? [:]
                try require(devices["allInputs"] as? Bool == false, "Fixture unexpectedly subscribed to all MIDI inputs.")
                try require((devices["inputIDs"] as? [Any])?.isEmpty == true, "Fixture selected an external MIDI input.")
                try require(fixture.graph.isInManualRenderingMode && fixture.graph.manualRenderingMode == .offline,
                    "The fixture left offline manual rendering.")
                return fixture
            } catch { await fixture.close(); throw error }
        }

        func target(track: Int, instrument: Int = 0) async throws {
            _ = try await host.handle("midi.configure", payload: ["track": track, "instrument": instrument])
        }
        func reset() async throws {
            _ = try await host.handle("engine.panic", payload: [:])
            try await target(track: 0)
            let silent = try levels()
            try require(silent.allSatisfy { $0 < 0.000001 }, "Panic did not clear the preceding fixture notes.")
        }
        func send(_ source: Source, _ status: UInt8, _ first: UInt8, _ second: UInt8) async throws {
            try require(!closed && graph.isInManualRenderingMode, "Host ingress fixture must remain open and offline.")
            // Synchronous MainActor call uses the exact production handler after
            // parser delivery, including original-route lookup and input filters.
            host.receiveMIDI(status, first, second, source: source.id)
            ingressPackets += 1
        }
        func levels() throws -> [Double] {
            // Discard >100x the one-ms release, then measure a full later quantum.
            for _ in 0..<2 {
                let result = try host.renderOfflineQuantum(frames: 4096, to: buffer)
                try require(result == .success && buffer.frameLength == 4096, "Offline synth rendering failed.")
            }
            guard let data = buffer.floatChannelData else { throw AudioSampleError.invalid("MIDI fixture has no Float32 PCM.") }
            return (0..<2).map { channel in
                let values = UnsafeBufferPointer(start: data[channel], count: 4096)
                return sqrt(values.reduce(0.0) { $0 + Double($1) * Double($1) } / 4096)
            }
        }
        func close() async {
            guard !closed else { return }; closed = true
            await host.shutdownAndWait()
        }
    }
}

#endif

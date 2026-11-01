// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation
import AudioToolbox
import Darwin

/// Exercises the application's real rack and export commands with deterministic,
/// original Audio Units. The components are registered only when tests are invoked.
enum NativeLatencyTests {
    private static let shortSubtype: OSType = 0x6C743634 // lt64
    private static let longSubtype: OSType = 0x6C323537 // l257
    private static let instrumentSubtype: OSType = 0x6C694D64 // liMd
    private static let manufacturer: OSType = 0x53745473 // StTs
    private static let registration: Void = {
        for (subtype, name) in [(shortSubtype, "64"), (longSubtype, "257")] {
            AUAudioUnit.registerSubclass(NativeLatencyTestUnit.self,
                as: description(subtype), name: "SuperTracker Test: \(name)-frame latency", version: 1)
        }
        AUAudioUnit.registerSubclass(NativeLatencyInstrumentTestUnit.self,
            as: description(instrumentSubtype, type: kAudioUnitType_MusicDevice),
            name: "SuperTracker Test: 64-frame MIDI instrument", version: 1)
    }()

    /// Registers the fixtures and returns the 257-frame latency effect's component (freeze tests).
    static func longLatencyEffect() -> String { _ = registration; return identifier(longSubtype) }

    private static func description(_ subtype: OSType, type: OSType = kAudioUnitType_Effect) -> AudioComponentDescription {
        AudioComponentDescription(componentType: type, componentSubType: subtype,
            componentManufacturer: manufacturer, componentFlags: 0, componentFlagsMask: 0)
    }

    private static func identifier(_ subtype: OSType, type: OSType = kAudioUnitType_Effect) -> String {
        "\(type):\(subtype):\(manufacturer)"
    }

    @MainActor static func run(output: URL) async throws -> [String] {
        _ = registration
        let folder = output.appendingPathComponent("latency", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        var measurements: [String: Any] = [:]
        var renderedMeasurements: [[String: Any]] = []
        var failures: [String] = []
        var passed: [String] = []
        defer {
            measurements["renders"] = renderedMeasurements
            measurements["failures"] = failures
            if let data = try? JSONSerialization.data(withJSONObject: measurements, options: [.prettyPrinted, .sortedKeys]) {
                try? data.write(to: folder.appendingPathComponent("metrics.json"), options: .atomic)
            }
        }
        func check(_ condition: Bool, _ message: String) {
            if !condition { failures.append(message) }
        }
        func firstSignal(_ samples: [Float]) -> Int {
            samples.firstIndex { abs($0) > 0.000001 } ?? -1
        }
        func maximumDifference(_ a: DecodedSample, _ b: DecodedSample) -> Float {
            var result: Float = 0
            for (left, right) in [(a.left, b.left), (a.right, b.right)] {
                for (x, y) in zip(left, right) { result = max(result, abs(x - y)) }
            }
            return result
        }

        // Establish that this platform does not silently align the mixer itself. The
        // same AU below then tests explicit rack compensation, rather than a no-op AU.
        let baseline = try uncompensatedBaseline()
        measurements["uncompensated"] = baseline
        check(baseline["leftFrame"] == 1024 && baseline["rightFrame"] == 1088,
              "Uncompensated engine must expose the fixture's physical 64-frame delay: \(baseline).")
        passed.append("An uncompensated AVAudioEngine exposes the test Audio Unit's physical 64-frame latency")

        let host = try AudioHost(midiInputIDs: [])
        defer { host.shutdown() }
        let rate = 48_000.0
        let sourceURL = folder.appendingPathComponent("alignment-pulses.wav")
        var pulses = [Float](repeating: 0, count: 48_000)
        for (frame, level): (Int, Float) in [(1024, 0.25), (4095, -0.2), (8192, 0.15), (47_936, 0.25)] {
            pulses[frame] = level
        }
        try AudioSamples.write(DecodedSample(path: sourceURL.path, sampleRate: rate,
            left: pulses, right: pulses, originalChannels: 2), to: sourceURL)
        var steps = [Any](repeating: NSNull(), count: 8)
        // Sustain the one-shot through the final impulse, close to the export boundary.
        steps[0] = ["note": 60, "instrument": 0, "velocity": 100, "gate": 16]
        var project: [String: Any] = ["version": 1, "name": "Latency alignment verification", "bpm": 120,
            "linesPerBeat": 4, "masterVolume": 1, "activePattern": 0, "loopSong": false,
            "tracks": [["name": "Reference left", "instrument": 0, "volume": 0.7, "pan": -1, "delaySend": 0, "reverbSend": 0],
                       ["name": "Processed right", "instrument": 0, "volume": 0.7, "pan": 1, "delaySend": 0, "reverbSend": 0]],
            "instruments": [["type": "sample", "samplePath": sourceURL.path, "rootNote": 60, "tune": 0,
                             "volume": 0.7, "mode": "oneshot", "attack": 0, "decay": 0, "sustain": 1,
                             "release": 0.001, "cutoff": 20_000, "resonance": 0, "drive": 0, "bitDepth": 16]],
            "patterns": [["id": 0, "length": 8, "steps": [steps, steps], "automation": []]], "sequence": [0],
            "audioPlugins": [], "delay": ["mix": 0], "reverb": ["mix": 0]]

        func latency(_ expected: Int, in value: [String: Any], label: String) {
            guard let status = value["latencyCompensation"] as? [String: Any] else {
                failures.append("\(label) does not expose latencyCompensation status.")
                return
            }
            check((status["frames"] as? Int) == expected,
                  "\(label) should report \(expected) compensation frames, got \(status).")
            check((status["limited"] as? Bool) == false, "\(label) unexpectedly exceeds the compensation limit.")
            if let milliseconds = status["milliseconds"] as? Double {
                check(abs(milliseconds - Double(expected) * 1000 / rate) < 0.000001,
                      "\(label) reports inconsistent milliseconds and frames.")
            } else { failures.append("\(label) does not expose compensation milliseconds.") }
        }

        func render(_ name: String, expectedLatency: Int, frames: Int = 48_000,
                    reference: DecodedSample? = nil, mode: String = "song", tail: Double = 0) async throws -> DecodedSample {
            let file = folder.appendingPathComponent(name + ".wav")
            let result = try await host.handle("audio.export", payload: ["path": file.path, "mode": mode, "tail": tail]) as? [String: Any] ?? [:]
            let audio = try AudioSamples.decode(file.path)
            latency(expectedLatency, in: result, label: name + " export")
            latency(expectedLatency, in: host.status(), label: name + " status")
            check(audio.left.count == frames && audio.right.count == frames,
                  "\(name) must retain \(frames) musical/tail frames after trimming compensation; got \(audio.left.count).")
            check(audio.left.allSatisfy(\.isFinite) && audio.right.allSatisfy(\.isFinite), "\(name) contains non-finite PCM.")
            let leftFrame = firstSignal(audio.left), rightFrame = firstSignal(audio.right)
            check(leftFrame >= 0 && leftFrame == rightFrame,
                  "\(name) must align left/right onset exactly; got \(leftFrame)/\(rightFrame).")
            var values: [String: Any] = ["name": name, "frames": audio.left.count,
                "leftFrame": leftFrame, "rightFrame": rightFrame, "compensationFrames": expectedLatency]
            if let reference {
                let difference = maximumDifference(audio, reference)
                values["maximumDifferenceFromDry"] = difference
                check(audio.left.count == reference.left.count && audio.right.count == reference.right.count,
                      "\(name) differs in length from its dry reference.")
                check(difference <= 0.000002,
                      "\(name) must match dry PCM including the final impulse after trimming; maximum difference \(difference).")
            }
            renderedMeasurements.append(values)
            return audio
        }

        func load(_ subtype: OSType, slot: Int) async throws {
            _ = try await host.handle("plugin.load", payload: ["track": 1, "kind": "effect", "slot": slot,
                "component": identifier(subtype)])
        }
        func remove(_ slot: Int) async throws {
            _ = try await host.handle("plugin.remove", payload: ["track": 1, "kind": "effect", "slot": slot])
        }

        _ = try await host.handle("engine.project", payload: ["project": project, "replace": true])
        // Export awaits asynchronous project, sample and plugin restoration before a
        // directly loaded fixture can be mistaken for a stale project plugin.
        let dry = try await render("sample-dry", expectedLatency: 0)
        check(firstSignal(dry.left) >= 1024 && firstSignal(dry.left) < 1032,
              "Dry sampler impulse should begin near its source frame 1024.")
        check(dry.left.suffix(64).contains { abs($0) > 0.0001 },
              "The dry fixture must exercise the final 64 frames of the export.")
        try await load(shortSubtype, slot: 0)
        _ = try await render("sample-delay-64", expectedLatency: 64, reference: dry)
        try await load(longSubtype, slot: 1)
        let chained = try await render("sample-chain-321", expectedLatency: 321, reference: dry)
        passed.append("Native samples align through 64-frame and serial 321-frame AU chains without shifting or truncating exports")

        _ = try await host.handle("plugin.parameter", payload: ["track": 1, "kind": "effect", "slot": 0, "bypass": true])
        _ = try await render("sample-chain-bypassed", expectedLatency: 321, reference: dry)
        _ = try await host.handle("plugin.parameter", payload: ["track": 1, "kind": "effect", "slot": 0, "bypass": false])
        _ = try await render("sample-chain-repeat", expectedLatency: 321, reference: chained)
        _ = try await host.handle("transport.seek", payload: ["pattern": 0, "row": 4, "sequence": 0])
        _ = try await render("sample-chain-after-seek", expectedLatency: 321, reference: dry)
        // This checks export reset after a seek, not real-time monitor behavior.
        passed.append("Latency-preserving AU bypass and repeated exports after a seek retain exact PCM alignment")

        let tailDryURL = folder.appendingPathComponent("sample-chain-tail.wav")
        let tailResult = try await host.handle("audio.export", payload: ["path": tailDryURL.path, "mode": "pattern", "tail": 0.25]) as? [String: Any] ?? [:]
        let tailAudio = try AudioSamples.decode(tailDryURL.path)
        latency(321, in: tailResult, label: "pattern tail")
        check(tailAudio.left.count == 60_000 && tailAudio.right.count == 60_000,
              "Pattern export plus a quarter-second tail must remain exactly 60,000 frames with compensation.")
        check(firstSignal(tailAudio.left) == firstSignal(dry.left) && firstSignal(tailAudio.right) == firstSignal(dry.right),
              "Pattern export with a tail must retain the original musical onset.")
        check(tailAudio.left.suffix(10_000).allSatisfy { abs($0) < 0.000001 } &&
              tailAudio.right.suffix(10_000).allSatisfy { abs($0) < 0.000001 },
              "Latency flush/tail rendering must not retrigger the first pattern row.")
        passed.append("Compensated pattern export preserves its requested tail without retriggering notes")

        try await remove(1)
        _ = try await render("sample-after-remove-257", expectedLatency: 64, reference: dry)
        try await remove(0)
        _ = try await render("sample-after-remove-all", expectedLatency: 0, reference: dry)
        passed.append("Removing latency-producing AU effects recalculates compensation and restores the original timing")

        let emptyRows = [Any](repeating: NSNull(), count: 8)
        project["patterns"] = [["id": 0, "length": 8, "steps": [emptyRows, emptyRows], "automation": []]]
        project["audioPlugins"] = []
        project["audioClips"] = (0..<2).map { track -> [String: Any] in
            ["id": "latency-clip-\(track)", "track": track, "path": sourceURL.path,
             "startBeat": 0.5, "durationBeats": 2.0, "sourceOffset": 0,
             "gain": 0.5, "fadeIn": 0, "fadeOut": 0, "muted": false]
        }
        _ = try await host.handle("engine.project", payload: ["project": project, "replace": true])
        let clipDry = try await render("clip-dry", expectedLatency: 0, frames: 60_000)
        check(firstSignal(clipDry.left) == 13_024 && clipDry.left.suffix(64).contains { abs($0) > 0.0001 },
              "Timeline fixture must start at beat 0.5 plus frame 1024 and exercise its last 64 frames.")
        try await load(shortSubtype, slot: 0)
        try await load(longSubtype, slot: 1)
        _ = try await render("clip-chain-321", expectedLatency: 321, frames: 60_000, reference: clipDry)
        _ = try await render("clip-chain-repeat", expectedLatency: 321, frames: 60_000, reference: clipDry)
        passed.append("Timeline audio clips retain exact start/end timing through compensated AU chains")

        let stemResult = try await host.handle("audio.export", payload: ["path": folder.appendingPathComponent("stems").path,
            "mode": "stems", "tail": 0]) as? [String: Any] ?? [:]
        let stemPaths = stemResult["paths"] as? [String] ?? []
        check(stemPaths.count == 2, "Compensated clip stem export must produce both tracks.")
        latency(321, in: stemResult, label: "stems")
        for (index, path) in stemPaths.prefix(2).enumerated() {
            let audio = try AudioSamples.decode(path)
            let active = index == 0 ? audio.left : audio.right
            let silent = index == 0 ? audio.right : audio.left
            let expected = index == 0 ? clipDry.left : clipDry.right
            check(active.count == 60_000 && firstSignal(active) == 13_024,
                  "Compensated stem \(index) must retain its 60,000-frame extent and musical onset.")
            check(zip(active, expected).allSatisfy { abs($0 - $1) < 0.000002 } &&
                  silent.allSatisfy { abs($0) < 0.000001 },
                  "Compensated stem \(index) must isolate its track with exact full-length PCM.")
        }
        passed.append("Compensated stem exports isolate tracks while retaining common musical alignment")

        // The right branch now originates inside an AU receiving sequenced MIDI.
        // Its 64-frame pipeline delays generated PCM, including pending audio after
        // note-off. This covers instrument latency independently of sample inserts.
        let sampleInstrument = (project["instruments"] as! [[String: Any]])[0]
        project["instruments"] = [sampleInstrument, ["type": "au", "volume": 0.7, "pan": 0]]
        var instrumentTracks = project["tracks"] as! [[String: Any]]
        instrumentTracks[1]["instrument"] = 1
        project["tracks"] = instrumentTracks
        var midiSteps = steps
        midiSteps[0] = ["note": 60, "instrument": 1, "velocity": 100, "gate": 16]
        project["patterns"] = [["id": 0, "length": 8, "steps": [steps, midiSteps], "automation": []]]
        project["audioClips"] = []
        project["audioPlugins"] = [["track": 1, "kind": "instrument", "slot": 0,
            "component": identifier(instrumentSubtype, type: kAudioUnitType_MusicDevice)]]
        _ = try await host.handle("engine.project", payload: ["project": project, "replace": true])
        let instrumentDry = try await render("midi-instrument-64", expectedLatency: 64)
        check(firstSignal(instrumentDry.left) == firstSignal(dry.left) &&
              firstSignal(instrumentDry.right) == firstSignal(dry.right),
              "A sequenced AU instrument and native sample must share their original musical onset.")
        check(instrumentDry.right.suffix(64).contains { abs($0) > 0.0001 },
              "AU instrument compensation must flush its pipeline through the last impulse at the export end.")
        try await load(longSubtype, slot: 0)
        _ = try await render("midi-instrument-chain-321", expectedLatency: 321, reference: instrumentDry)
        _ = try await render("midi-instrument-chain-repeat", expectedLatency: 321, reference: instrumentDry)
        passed.append("Sequenced AU instruments align with native samples and retain musical timing through additional insert latency")
        _ = try await host.handle("plugin.parameter", payload: ["track": 1, "kind": "effect", "slot": 0, "bypass": true])
        _ = try await render("midi-instrument-insert-bypassed", expectedLatency: 321, reference: instrumentDry)
        try await remove(0)
        _ = try await render("midi-instrument-after-remove", expectedLatency: 64, reference: instrumentDry)
        passed.append("AU instrument insert bypass and removal preserve complete rendered PCM and compensation status")

        let nativeReturn = try await nativeReturnAlignment()
        measurements["nativeReturn"] = nativeReturn
        check(nativeReturn["nativeTrackFrame"] == 1088 && nativeReturn["nativeReturnFrame"] == 1088 &&
              nativeReturn["compensationFrames"] == 64,
              "Native return bus must align with a 64-frame track insert; measured \(nativeReturn).")
        passed.append("The shared native send/metronome return route aligns with latency-producing track inserts")
        if !failures.isEmpty {
            throw AudioSampleError.invalid("Latency integration: " + failures.joined(separator: "\n"))
        }
        return passed
    }

    /// The C++ engine's metronome and native sends share source bus 16. Timestamped
    /// synthetic pulses isolate that actual rack route without making claims about
    /// the metronome's sound or enabling audible/live playback during tests.
    @MainActor private static func nativeReturnAlignment() async throws -> [String: Int] {
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        let engine = AVAudioEngine()
        let rack = try AudioPluginRack(engine: engine, trackCount: 16, sampleRate: 48_000)
        defer { engine.stop(); withExtendedLifetime(rack) {} }
        let sources = (0...16).map { bus in
            AVAudioSourceNode(format: format) { silence, timestamp, frames, list in
                silence.pointee = false
                for (channel, buffer) in UnsafeMutableAudioBufferListPointer(list).enumerated() {
                    guard let data = buffer.mData?.assumingMemoryBound(to: Float.self) else { return kAudio_ParamError }
                    let isSignal = (bus == 0 && channel == 0) || (bus == 16 && channel == 1)
                    for frame in 0..<Int(frames) {
                        data[frame] = isSignal && Int(timestamp.pointee.mSampleTime) + frame == 1024 ? 0.25 : 0
                    }
                }
                return noErr
            }
        }
        rack.installNativeSources(sources)
        _ = try await rack.load(["track": 0, "kind": "effect", "slot": 0, "component": identifier(shortSubtype)])
        try engine.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
        engine.reset(); engine.prepare()
        rack.refreshLatencyCompensation(); rack.resetLatencyCompensation()
        try engine.start()
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4096)!
        var found = [-1, -1], offset = 0, attempts = 0
        while offset < 8192 && attempts < 20 {
            attempts += 1
            guard try engine.renderOffline(4096, to: buffer) == .success else { continue }
            for channel in 0..<2 where found[channel] < 0 {
                for frame in 0..<Int(buffer.frameLength) where abs(buffer.floatChannelData![channel][frame]) > 0.01 {
                    found[channel] = offset + frame
                    break
                }
            }
            offset += Int(buffer.frameLength)
        }
        return ["nativeTrackFrame": found[0], "nativeReturnFrame": found[1],
                "compensationFrames": rack.compensationLatencyFrames, "referenceInputFrame": 1024]
    }

    @MainActor private static func uncompensatedBaseline() throws -> [String: Int] {
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        let engine = AVAudioEngine()
        let effect = AVAudioUnitEffect(audioComponentDescription: description(shortSubtype))
        func source(channel: Int) -> AVAudioSourceNode {
            AVAudioSourceNode(format: format) { silence, timestamp, frames, list in
                silence.pointee = false
                for (index, buffer) in UnsafeMutableAudioBufferListPointer(list).enumerated() {
                    guard let data = buffer.mData?.assumingMemoryBound(to: Float.self) else { return kAudio_ParamError }
                    for frame in 0..<Int(frames) {
                        data[frame] = index == channel && Int(timestamp.pointee.mSampleTime) + frame == 1024 ? 0.25 : 0
                    }
                }
                return noErr
            }
        }
        let left = source(channel: 0), right = source(channel: 1)
        for node in [left, right, effect] as [AVAudioNode] { engine.attach(node) }
        engine.connect(left, to: engine.mainMixerNode, fromBus: 0, toBus: 0, format: format)
        engine.connect(right, to: effect, format: format)
        engine.connect(effect, to: engine.mainMixerNode, fromBus: 0, toBus: 1, format: format)
        try engine.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
        try engine.start()
        defer { engine.stop() }
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4096)!
        var found = [-1, -1], offset = 0, attempts = 0
        while offset < 8192 && attempts < 20 {
            attempts += 1
            guard try engine.renderOffline(4096, to: buffer) == .success else { continue }
            for channel in 0..<2 where found[channel] < 0 {
                for frame in 0..<Int(buffer.frameLength) where abs(buffer.floatChannelData![channel][frame]) > 0.01 {
                    found[channel] = offset + frame
                    break
                }
            }
            offset += Int(buffer.frameLength)
        }
        return ["leftFrame": found[0], "rightFrame": found[1], "differenceFrames": found[1] - found[0],
                "reportedLatencyFrames": Int((effect.auAudioUnit.latency * 48_000).rounded())]
    }
}

/// A single-voice, sample-exact test instrument. MIDI controls an original pulse
/// generator, which feeds a real 64-frame delay. Note-off silences the generator while
/// retaining already buffered audio, just as an instrument's algorithmic pipeline must.
private final class NativeLatencyInstrumentTestUnit: AUAudioUnit {
    private var output: AUAudioUnitBus!
    private var inputs: AUAudioUnitBusArray!
    private var outputs: AUAudioUnitBusArray!
    private let ring = UnsafeMutablePointer<Float>.allocate(capacity: 64)
    private var scratchLeft = UnsafeMutablePointer<Float>.allocate(capacity: 16_384)
    private var scratchRight = UnsafeMutablePointer<Float>.allocate(capacity: 16_384)
    private var scratchCapacity = 16_384
    private var cursor = 0
    private var age = -1
    private var velocity: Float = 0

    override init(componentDescription: AudioComponentDescription,
                  options: AudioComponentInstantiationOptions = []) throws {
        ring.initialize(repeating: 0, count: 64)
        scratchLeft.initialize(repeating: 0, count: 16_384)
        scratchRight.initialize(repeating: 0, count: 16_384)
        try super.init(componentDescription: componentDescription, options: options)
        output = try AUAudioUnitBus(format: AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!)
        output.supportedChannelCounts = [2]
        inputs = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: [])
        outputs = AUAudioUnitBusArray(audioUnit: self, busType: .output, busses: [output])
        maximumFramesToRender = 16_384
    }

    deinit {
        ring.deinitialize(count: 64); ring.deallocate()
        scratchLeft.deinitialize(count: scratchCapacity); scratchLeft.deallocate()
        scratchRight.deinitialize(count: scratchCapacity); scratchRight.deallocate()
    }

    override var inputBusses: AUAudioUnitBusArray { inputs }
    override var outputBusses: AUAudioUnitBusArray { outputs }
    override var channelCapabilities: [NSNumber]? { [0, 2] }
    override var latency: TimeInterval { 64 / output.format.sampleRate }
    override var maximumFramesToRender: AUAudioFrameCount {
        get { super.maximumFramesToRender }
        set { super.maximumFramesToRender = max(16_384, newValue) }
    }

    override func shouldChange(to format: AVAudioFormat, for bus: AUAudioUnitBus) -> Bool {
        !renderResourcesAllocated && format.channelCount == 2 &&
            format.commonFormat == .pcmFormatFloat32 && !format.isInterleaved
    }

    override func allocateRenderResources() throws {
        let requested = max(16_384, Int(maximumFramesToRender))
        if requested > scratchCapacity {
            scratchLeft.deinitialize(count: scratchCapacity); scratchLeft.deallocate()
            scratchRight.deinitialize(count: scratchCapacity); scratchRight.deallocate()
            scratchCapacity = requested
            scratchLeft = .allocate(capacity: requested); scratchRight = .allocate(capacity: requested)
            scratchLeft.initialize(repeating: 0, count: requested)
            scratchRight.initialize(repeating: 0, count: requested)
        }
        reset()
        try super.allocateRenderResources()
    }

    override func reset() {
        memset(ring, 0, 64 * MemoryLayout<Float>.size)
        cursor = 0; age = -1; velocity = 0
        super.reset()
    }

    override var internalRenderBlock: AUInternalRenderBlock {
        { [unowned self] flags, timestamp, frames, bus, outputData, events, _ in
            guard bus == 0 else { return kAudioUnitErr_InvalidElement }
            let count = Int(frames)
            guard count <= scratchCapacity else { return kAudioUnitErr_TooManyFramesToProcess }
            let buffers = UnsafeMutableAudioBufferListPointer(outputData)
            guard buffers.count == 2 else { return kAudioUnitErr_FormatNotSupported }
            let bytes = UInt32(count * MemoryLayout<Float>.size)
            if buffers[0].mData == nil { buffers[0].mData = UnsafeMutableRawPointer(scratchLeft); buffers[0].mDataByteSize = bytes }
            if buffers[1].mData == nil { buffers[1].mData = UnsafeMutableRawPointer(scratchRight); buffers[1].mDataByteSize = bytes }
            guard buffers[0].mDataByteSize >= bytes, buffers[1].mDataByteSize >= bytes,
                  let left = buffers[0].mData?.assumingMemoryBound(to: Float.self),
                  let right = buffers[1].mData?.assumingMemoryBound(to: Float.self) else { return kAudio_ParamError }
            flags.pointee.remove(.unitRenderAction_OutputIsSilence)
            var next = events
            let sampleTime = AUEventSampleTime(timestamp.pointee.mSampleTime)
            for frame in 0..<count {
                while let event = next, event.pointee.head.eventSampleTime <= sampleTime + AUEventSampleTime(frame) {
                    if event.pointee.head.eventType == .MIDI {
                        let message = event.pointee.MIDI
                        if message.length >= 3 {
                            let kind = message.data.0 & 0xF0
                            if kind == 0x90 && message.data.2 > 0 {
                                age = 0; velocity = Float(message.data.2) / 127
                            } else if kind == 0x80 || (kind == 0x90 && message.data.2 == 0) ||
                                      (kind == 0xB0 && (message.data.1 == 120 || message.data.1 == 123)) {
                                age = -1
                            }
                        }
                    }
                    next = event.pointee.head.next.map { UnsafePointer($0) }
                }
                let generated: Float
                switch age {
                case 1024, 47_936: generated = 0.25 * velocity
                case 4095: generated = -0.2 * velocity
                case 8192: generated = 0.15 * velocity
                default: generated = 0
                }
                let delayed = ring[cursor]
                ring[cursor] = generated
                left[frame] = delayed; right[frame] = delayed
                cursor = (cursor + 1) & 63
                if age >= 0 { age += 1 }
            }
            buffers[0].mDataByteSize = bytes; buffers[1].mDataByteSize = bytes
            return noErr
        }
    }
}

/// Original fixed-latency fixture: a transparent delay is its algorithmic pipeline,
/// so bypass retains that pipeline and its declared latency. All PCM is preallocated.
private final class NativeLatencyTestUnit: AUAudioUnit {
    private var input: AUAudioUnitBus!
    private var output: AUAudioUnitBus!
    private var inputs: AUAudioUnitBusArray!
    private var outputs: AUAudioUnitBusArray!
    private let delay: Int
    private let left: UnsafeMutablePointer<Float>
    private let right: UnsafeMutablePointer<Float>
    private var scratchLeft: UnsafeMutablePointer<Float>
    private var scratchRight: UnsafeMutablePointer<Float>
    private var scratchCapacity = 16_384
    private var cursor = 0

    override init(componentDescription: AudioComponentDescription,
                  options: AudioComponentInstantiationOptions = []) throws {
        delay = componentDescription.componentSubType == 0x6C743634 ? 64 : 257
        left = .allocate(capacity: delay); right = .allocate(capacity: delay)
        left.initialize(repeating: 0, count: delay); right.initialize(repeating: 0, count: delay)
        scratchLeft = .allocate(capacity: 16_384); scratchRight = .allocate(capacity: 16_384)
        scratchLeft.initialize(repeating: 0, count: 16_384); scratchRight.initialize(repeating: 0, count: 16_384)
        try super.init(componentDescription: componentDescription, options: options)
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        input = try AUAudioUnitBus(format: format); output = try AUAudioUnitBus(format: format)
        input.supportedChannelCounts = [2]; output.supportedChannelCounts = [2]
        inputs = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: [input])
        outputs = AUAudioUnitBusArray(audioUnit: self, busType: .output, busses: [output])
        maximumFramesToRender = 16_384
    }

    deinit {
        left.deinitialize(count: delay); left.deallocate()
        right.deinitialize(count: delay); right.deallocate()
        scratchLeft.deinitialize(count: scratchCapacity); scratchLeft.deallocate()
        scratchRight.deinitialize(count: scratchCapacity); scratchRight.deallocate()
    }

    override var inputBusses: AUAudioUnitBusArray { inputs }
    override var outputBusses: AUAudioUnitBusArray { outputs }
    override var channelCapabilities: [NSNumber]? { [2, 2] }
    override var canProcessInPlace: Bool { true }
    override var latency: TimeInterval { Double(delay) / output.format.sampleRate }
    override var maximumFramesToRender: AUAudioFrameCount {
        get { super.maximumFramesToRender }
        set { super.maximumFramesToRender = max(16_384, newValue) }
    }

    override func shouldChange(to format: AVAudioFormat, for bus: AUAudioUnitBus) -> Bool {
        !renderResourcesAllocated && format.channelCount == 2 &&
            format.commonFormat == .pcmFormatFloat32 && !format.isInterleaved
    }

    override func allocateRenderResources() throws {
        guard input.format == output.format else {
            throw NSError(domain: NSOSStatusErrorDomain, code: Int(kAudioUnitErr_FormatNotSupported))
        }
        let requested = max(16_384, Int(maximumFramesToRender))
        if requested > scratchCapacity {
            scratchLeft.deinitialize(count: scratchCapacity); scratchLeft.deallocate()
            scratchRight.deinitialize(count: scratchCapacity); scratchRight.deallocate()
            scratchCapacity = requested
            scratchLeft = .allocate(capacity: requested); scratchRight = .allocate(capacity: requested)
            scratchLeft.initialize(repeating: 0, count: requested)
            scratchRight.initialize(repeating: 0, count: requested)
        }
        reset()
        try super.allocateRenderResources()
    }

    override func reset() {
        memset(left, 0, delay * MemoryLayout<Float>.size)
        memset(right, 0, delay * MemoryLayout<Float>.size)
        cursor = 0
        super.reset()
    }

    override var internalRenderBlock: AUInternalRenderBlock {
        { [unowned self] flags, timestamp, frames, bus, output, _, pull in
            guard bus == 0, let pull else { return kAudioUnitErr_NoConnection }
            let count = Int(frames)
            guard count <= scratchCapacity else { return kAudioUnitErr_TooManyFramesToProcess }
            let buffers = UnsafeMutableAudioBufferListPointer(output)
            guard buffers.count == 2 else { return kAudioUnitErr_FormatNotSupported }
            let bytes = UInt32(count * MemoryLayout<Float>.size)
            if buffers[0].mData == nil { buffers[0].mData = UnsafeMutableRawPointer(scratchLeft); buffers[0].mDataByteSize = bytes }
            if buffers[1].mData == nil { buffers[1].mData = UnsafeMutableRawPointer(scratchRight); buffers[1].mDataByteSize = bytes }
            guard buffers[0].mDataByteSize >= bytes, buffers[1].mDataByteSize >= bytes else { return kAudio_ParamError }
            let result = pull(flags, timestamp, frames, 0, output)
            guard result == noErr else { return result }
            guard let l = buffers[0].mData?.assumingMemoryBound(to: Float.self),
                  let r = buffers[1].mData?.assumingMemoryBound(to: Float.self) else { return kAudio_ParamError }
            let silence = flags.pointee.contains(.unitRenderAction_OutputIsSilence)
            flags.pointee.remove(.unitRenderAction_OutputIsSilence)
            for frame in 0..<count {
                let previousL = left[cursor], previousR = right[cursor]
                left[cursor] = silence ? 0 : l[frame]; right[cursor] = silence ? 0 : r[frame]
                l[frame] = previousL; r[frame] = previousR
                cursor += 1
                if cursor == delay { cursor = 0 }
            }
            buffers[0].mDataByteSize = bytes; buffers[1].mDataByteSize = bytes
            return noErr
        }
    }
}

#endif

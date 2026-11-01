// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation
import AudioToolbox
import Darwin

/// Exercises sample timing through the public project/export commands. The original
/// test instrument has a real, declared 64-frame pipeline, so these assertions also
/// detect automation applied on the wrong side of latency compensation.
enum NativeAUMixTimingTests {
    private static let subtype: OSType = 0x6D677463 // mgtc
    private static let zeroEffectSubtype: OSType = 0x6D676530 // mge0
    private static let delayedEffectSubtype: OSType = 0x6D676536 // mge6
    private static let manufacturer: OSType = 0x53745473 // StTs
    private static let registration: Void = {
        let description = AudioComponentDescription(componentType: kAudioUnitType_MusicDevice,
            componentSubType: subtype, componentManufacturer: manufacturer,
            componentFlags: 0, componentFlagsMask: 0)
        AUAudioUnit.registerSubclass(NativeAUMixTimingInstrument.self, as: description,
            name: "SuperTracker Test: constant MIDI timing instrument", version: 1)
        for (subtype, frames) in [(zeroEffectSubtype, 0), (delayedEffectSubtype, 64)] {
            let effect = AudioComponentDescription(componentType: kAudioUnitType_Effect,
                componentSubType: subtype, componentManufacturer: manufacturer,
                componentFlags: 0, componentFlagsMask: 0)
            AUAudioUnit.registerSubclass(NativeAUMixTimingEffect.self, as: effect,
                name: "SuperTracker Test: \(frames)-frame timing gain", version: 1)
        }
    }()

    @MainActor static func run(output: URL) async throws -> [String] {
        _ = registration
        let folder = output.appendingPathComponent("mix-timing", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let host = try AudioHost(midiInputIDs: [])
        defer { host.shutdown() }
        let rate = 48_000.0, epsilon: Float = 0.000001
        let onset = 6_000, noteEnd = 18_000, musicalFrames = 24_000
        var measurements: [String: Any] = ["sampleRate": rate, "renderQuantum": 4_096,
            "automationFrame": onset, "noteOffFrame": noteEnd, "fixtureLatencyFrames": 64]
        var renders: [[String: Any]] = [], failures: [String] = [], passed: [String] = []
        defer {
            measurements["renders"] = renders
            measurements["failures"] = failures
            if let data = try? JSONSerialization.data(withJSONObject: measurements, options: [.prettyPrinted, .sortedKeys]) {
                try? data.write(to: folder.appendingPathComponent("metrics.json"), options: .atomic)
            }
        }
        func check(_ condition: Bool, _ message: String) {
            if !condition { failures.append(message) }
        }
        func firstSignal(_ values: [Float], from: Int = 0) -> Int {
            guard from < values.count else { return -1 }
            return values.indices.dropFirst(max(0, from)).first { abs(values[$0]) > epsilon } ?? -1
        }
        func peak(_ values: [Float], _ range: Range<Int>) -> Float {
            let lower = max(0, min(values.count, range.lowerBound))
            let upper = max(lower, min(values.count, range.upperBound))
            return values[lower..<upper].reduce(0) { max($0, abs($1)) }
        }
        func maximumDifference(_ a: DecodedSample, _ b: DecodedSample, _ range: Range<Int>) -> Float {
            let lower = max(0, min(min(a.left.count, b.left.count), range.lowerBound))
            let upper = max(lower, min(min(a.left.count, b.left.count), range.upperBound))
            var result: Float = 0
            for frame in lower..<upper {
                result = max(result, max(abs(a.left[frame] - b.left[frame]), abs(a.right[frame] - b.right[frame])))
            }
            return result
        }
        func difference(_ a: DecodedSample, _ b: DecodedSample, name: String) throws -> DecodedSample {
            check(a.left.count == b.left.count, "\(name) reference and automated export lengths differ.")
            let count = min(a.left.count, b.left.count)
            let file = folder.appendingPathComponent(name + ".wav")
            let result = DecodedSample(path: file.path, sampleRate: rate,
                left: (0..<count).map { a.left[$0] - b.left[$0] },
                right: (0..<count).map { a.right[$0] - b.right[$0] }, originalChannels: 2)
            try AudioSamples.write(result, to: file)
            return result
        }
        func rms(_ sample: DecodedSample, from: Int) -> Double {
            let first = max(0, min(sample.left.count, from))
            guard first < sample.left.count else { return 0 }
            var energy = 0.0
            for frame in first..<sample.left.count {
                energy += Double(sample.left[frame]) * Double(sample.left[frame])
                energy += Double(sample.right[frame]) * Double(sample.right[frame])
            }
            return sqrt(energy / Double((sample.left.count - first) * 2))
        }
        func lane(_ target: String, before: Double = 0, after: Double = 1) -> [String: Any] {
            ["track": 0, "target": target, "interpolation": "step",
             "points": [["row": 0, "value": before], ["row": 1, "value": after]]]
        }
        var rows = [Any](repeating: NSNull(), count: 4)
        rows[0] = ["note": 60, "instrument": 0, "velocity": 127, "gate": 3]
        let identifier = "\(kAudioUnitType_MusicDevice):\(subtype):\(manufacturer)"
        let baseProject: [String: Any] = ["version": 1, "name": "AU mix timing verification",
            "bpm": 120, "linesPerBeat": 4, "masterVolume": 1, "activePattern": 0, "loopSong": false,
            "tracks": [["name": "Constant AU", "instrument": 0, "volume": 1,
                        "pan": 0, "delaySend": 0, "reverbSend": 0]],
            "instruments": [["type": "au", "volume": 1, "pan": 0]],
            "patterns": [["id": 0, "length": 4, "steps": [rows], "automation": []]],
            "sequence": [0], "audioPlugins": [["track": 0, "kind": "instrument", "slot": 0, "component": identifier]],
            "delay": ["time": 0.05, "feedback": 0, "mix": 0],
            "reverb": ["size": 0.8, "damping": 0, "mix": 0]]

        func render(_ name: String, automation: [[String: Any]] = [], pan: Double = 0,
                    delayMix: Double = 0, reverbMix: Double = 0, tail: Double = 0,
                    effects: [[String: Any]] = [], expectedLatency: Int = 64) async throws -> DecodedSample {
            var project = baseProject
            var tracks = project["tracks"] as! [[String: Any]]
            tracks[0]["pan"] = pan
            project["tracks"] = tracks
            project["patterns"] = [["id": 0, "length": 4, "steps": [rows], "automation": automation]]
            project["delay"] = ["time": 0.05, "feedback": 0, "mix": delayMix]
            project["reverb"] = ["size": 0.8, "damping": 0, "mix": reverbMix]
            project["audioPlugins"] = (baseProject["audioPlugins"] as! [[String: Any]]) + effects
            _ = try await host.handle("engine.project", payload: ["project": project, "replace": true])
            let file = folder.appendingPathComponent(name + ".wav")
            let result = try await host.handle("audio.export", payload: ["path": file.path, "mode": "song", "tail": tail]) as? [String: Any] ?? [:]
            let sample = try AudioSamples.decode(file.path)
            let expectedFrames = musicalFrames + Int(tail * rate)
            check(sample.left.count == expectedFrames && sample.right.count == expectedFrames,
                  "\(name) must retain \(expectedFrames) frames after compensation trimming; got \(sample.left.count).")
            check(sample.sampleRate == rate, "\(name) must retain the 48 kHz engine sample rate.")
            check(sample.left.allSatisfy(\.isFinite) && sample.right.allSatisfy(\.isFinite),
                  "\(name) contains non-finite PCM.")
            let latency = result["latencyCompensation"] as? [String: Any] ?? [:]
            check((latency["frames"] as? Int) == expectedLatency && (latency["limited"] as? Bool) == false,
                  "\(name) must compensate the fixture's actual \(expectedLatency)-frame pipeline; got \(latency).")
            let mixAutomation = host.status()["mixAutomation"] as? [String: Any] ?? [:]
            check((mixAutomation["droppedEvents"] as? Int) == 0,
                  "\(name) dropped mix automation events: \(mixAutomation).")
            renders.append(["name": name, "frames": sample.left.count, "compensation": latency,
                "mixAutomation": mixAutomation,
                "firstLeftFrame": firstSignal(sample.left), "firstRightFrame": firstSignal(sample.right),
                "leftBeforeAutomationPeak": peak(sample.left, 0..<onset),
                "rightBeforeAutomationPeak": peak(sample.right, 0..<onset)])
            return sample
        }

        let centered = try await render("reference-center")
        let left = try await render("reference-left", pan: -1)
        let right = try await render("reference-right", pan: 1)
        check(firstSignal(centered.left) == 0 && firstSignal(centered.right) == 0,
              "The unautomated instrument must start at musical frame 0 after PDC trimming.")
        check(peak(centered.left, 0..<noteEnd) > 0.01 && peak(centered.right, 0..<noteEnd) > 0.01,
              "The constant reference must contain audible stereo PCM.")
        check(peak(centered.left, noteEnd..<musicalFrames) <= epsilon &&
              peak(centered.right, noteEnd..<musicalFrames) <= epsilon,
              "The fixture must become silent at gate frame 18000, including its compensated pipeline.")
        check(peak(left.right, 0..<musicalFrames) <= epsilon && peak(right.left, 0..<musicalFrames) <= epsilon,
              "Fully panned reference renders must completely silence their opposite channels.")
        if centered.left.count >= noteEnd && centered.right.count >= noteEnd {
            let levelL = centered.left[0], levelR = centered.right[0]
            check(centered.left.prefix(noteEnd).allSatisfy { abs($0 - levelL) <= epsilon } &&
                  centered.right.prefix(noteEnd).allSatisfy { abs($0 - levelR) <= epsilon },
                  "The fixture's held note must remain constant up to its gate boundary.")
        }
        passed.append("A deterministic MIDI AU starts at frame 0 and ends at frame 18000 after 64-frame latency compensation")

        for target in ["volume", "instrumentVolume"] {
            let sample = try await render(target + "-row1", automation: [lane(target)])
            let observedLeft = firstSignal(sample.left), observedRight = firstSignal(sample.right)
            check(observedLeft == onset && observedRight == onset,
                  "\(target) 0→1 must start both channels exactly at frame 6000, got \(observedLeft)/\(observedRight).")
            check(peak(sample.left, 0..<onset) <= epsilon && peak(sample.right, 0..<onset) <= epsilon,
                  "\(target) automation leaked audio before frame 6000.")
            let error = maximumDifference(sample, centered, onset..<musicalFrames)
            check(error <= epsilon, "\(target) must immediately match the constant reference from frame 6000; max error \(error).")
            measurements[target] = ["firstLeftFrame": observedLeft, "firstRightFrame": observedRight,
                "maximumReferenceErrorAfterAutomation": error]
            passed.append("AU \(target) automation changes at frame 6000 inside a 4096-frame render quantum")
        }

        for target in ["pan", "instrumentPan"] {
            let sample = try await render(target + "-row1", automation: [lane(target, before: -1, after: 1)])
            let errorBefore = maximumDifference(sample, left, 0..<onset)
            let errorAfter = maximumDifference(sample, right, onset..<musicalFrames)
            let observedRight = firstSignal(sample.right)
            check(errorBefore <= epsilon && errorAfter <= epsilon,
                  "\(target) must match full-left before frame 6000 and full-right from frame 6000; errors \(errorBefore)/\(errorAfter).")
            check(observedRight == onset && peak(sample.right, 0..<onset) <= epsilon &&
                  peak(sample.left, onset..<musicalFrames) <= epsilon,
                  "\(target) channel switch must occur exactly at frame 6000; right onset \(observedRight).")
            measurements[target] = ["rightOnsetFrame": observedRight,
                "maximumReferenceErrorBeforeAutomation": errorBefore,
                "maximumReferenceErrorAfterAutomation": errorAfter]
            passed.append("AU \(target) automation switches channels exactly at frame 6000 with latency compensation")
        }

        let delayDry = try await render("delay-reference", delayMix: 1, tail: 1)
        let delayAutomated = try await render("delay-row1", automation: [lane("delay")], delayMix: 1, tail: 1)
        let wetDelay = try difference(delayAutomated, delayDry, name: "delay-wet-difference")
        let delayLeft = firstSignal(wetDelay.left), delayRight = firstSignal(wetDelay.right)
        let expectedEcho = onset + Int(0.05 * rate)
        check(peak(wetDelay.left, 0..<onset) <= epsilon && peak(wetDelay.right, 0..<onset) <= epsilon,
              "AU delay send automation must not change PCM before frame 6000.")
        // Apple's low-pass delay can take a few samples to cross the amplitude
        // threshold; the creative delay itself must remain 50 ms after PDC trimming.
        check(abs(delayLeft - expectedEcho) <= 8 && abs(delayRight - expectedEcho) <= 8,
              "The automated AU send's 50 ms echo must begin near frame 8400, got \(delayLeft)/\(delayRight).")
        let delayTail = rms(wetDelay, from: noteEnd)
        check(delayTail > 0.0001, "AU delay send automation must produce an audible tail after note-off.")
        measurements["delay"] = ["expectedFirstWetFrame": expectedEcho, "firstLeftWetFrame": delayLeft,
            "firstRightWetFrame": delayRight, "tailRMS": delayTail]
        passed.append("AU delay send automation stays dry before frame 6000 and produces its 50 ms echo near frame 8400")

        let reverbDry = try await render("reverb-reference", reverbMix: 1, tail: 1)
        let reverbAutomated = try await render("reverb-row1", automation: [lane("reverb")], reverbMix: 1, tail: 1)
        let wetReverb = try difference(reverbAutomated, reverbDry, name: "reverb-wet-difference")
        let reverbLeft = firstSignal(wetReverb.left), reverbRight = firstSignal(wetReverb.right)
        let reverbTail = rms(wetReverb, from: noteEnd)
        check(peak(wetReverb.left, 0..<onset) <= epsilon && peak(wetReverb.right, 0..<onset) <= epsilon,
              "AU reverb send automation must not change PCM before frame 6000.")
        check(reverbLeft >= onset && reverbRight >= onset && reverbTail > 0.00001,
              "AU reverb send automation must produce a later audible stereo tail; onsets \(reverbLeft)/\(reverbRight), RMS \(reverbTail).")
        measurements["reverb"] = ["firstLeftWetFrame": reverbLeft, "firstRightWetFrame": reverbRight, "tailRMS": reverbTail]
        passed.append("AU reverb send automation stays dry before frame 6000 and produces an audible stereo tail")

        func effect(_ subtype: OSType, slot: Int) -> [String: Any] {
            ["track": 0, "kind": "effect", "slot": slot,
             "component": "\(kAudioUnitType_Effect):\(subtype):\(manufacturer)"]
        }
        // First isolate the instrument-to-insert offset with a zero-latency gain.
        // Then use two distinct instances of the 64-frame effect: slot 1 must receive
        // the parameter at 6000 + 64 + 64, before its own 64-frame audio pipeline.
        for (name, chain, target, compensation, upstream) in [
            ("zero-insert", [effect(zeroEffectSubtype, slot: 0)], "plugin:effect:0:0", 64, 64),
            ("serial-inserts", [effect(delayedEffectSubtype, slot: 0), effect(delayedEffectSubtype, slot: 1)], "plugin:effect:1:0", 192, 128)
        ] {
            let reference = try await render(name + "-reference", effects: chain, expectedLatency: compensation)
            let referenceError = maximumDifference(reference, centered, 0..<musicalFrames)
            check(referenceError <= epsilon,
                  "\(name) unity-gain pipeline must reproduce compensated instrument PCM; max error \(referenceError).")
            let sample = try await render(name + "-parameter-row1", automation: [lane(target)],
                                          effects: chain, expectedLatency: compensation)
            let firstLeft = firstSignal(sample.left), firstRight = firstSignal(sample.right)
            let afterError = maximumDifference(sample, reference, onset..<musicalFrames)
            check(firstLeft == onset && firstRight == onset,
                  "\(name) parameter automation must begin exactly at frame 6000 after \(compensation)-frame PDC, got \(firstLeft)/\(firstRight).")
            check(peak(sample.left, 0..<onset) <= epsilon && peak(sample.right, 0..<onset) <= epsilon && afterError <= epsilon,
                  "\(name) gain must stay silent before frame 6000 and match its constant reference immediately afterward; max error \(afterError).")
            measurements[name] = ["firstLeftFrame": firstLeft, "firstRightFrame": firstRight,
                "compensationFrames": compensation, "upstreamLatencyFrames": upstream,
                "maximumUnityReferenceError": referenceError, "maximumReferenceErrorAfterAutomation": afterError]
            passed.append("\(name) AU gain automation follows \(upstream) upstream latency frames and starts at musical frame 6000 after compensation")
        }

        let parameter = try await render("instrument-parameter-row1", automation: [lane("plugin:instrument:0:0")])
        let parameterLeft = firstSignal(parameter.left), parameterRight = firstSignal(parameter.right)
        let parameterError = maximumDifference(parameter, centered, onset..<musicalFrames)
        check(parameterLeft == onset && parameterRight == onset,
              "AU instrument parameter gain 0→1 must start exactly at frame 6000 after PDC, got \(parameterLeft)/\(parameterRight).")
        check(peak(parameter.left, 0..<onset) <= epsilon && peak(parameter.right, 0..<onset) <= epsilon && parameterError <= epsilon,
              "The scheduled instrument parameter must retain silence before frame 6000 and immediately match reference PCM after it; max error \(parameterError).")
        measurements["instrumentParameter"] = ["firstLeftFrame": parameterLeft, "firstRightFrame": parameterRight,
            "maximumReferenceErrorAfterAutomation": parameterError]
        passed.append("Scheduled AU instrument parameter automation reaches its generator at frame 6000 without an extra latency offset")

        if !failures.isEmpty { throw AudioSampleError.invalid("AU mix timing integration: " + failures.joined(separator: "\n")) }
        return passed
    }
}

/// Minimal original AU instrument, used only by the native verification command.
/// Scheduled MIDI and gain events are consumed at their exact sample positions.
/// The output buffers and 64-frame stereo pipeline allocate only outside rendering.
private final class NativeAUMixTimingInstrument: AUAudioUnit {
    private var output: AUAudioUnitBus!
    private var inputs: AUAudioUnitBusArray!
    private var outputs: AUAudioUnitBusArray!
    private var parameters: AUParameterTree!
    private let ringLeft = UnsafeMutablePointer<Float>.allocate(capacity: 64)
    private let ringRight = UnsafeMutablePointer<Float>.allocate(capacity: 64)
    private var scratchLeft = UnsafeMutablePointer<Float>.allocate(capacity: 16_384)
    private var scratchRight = UnsafeMutablePointer<Float>.allocate(capacity: 16_384)
    private var scratchCapacity = 16_384
    private var cursor = 0
    private var heldNote = -1
    private var defaultGain: AUValue = 1
    private var gain: AUValue = 1

    override init(componentDescription: AudioComponentDescription,
                  options: AudioComponentInstantiationOptions = []) throws {
        ringLeft.initialize(repeating: 0, count: 64)
        ringRight.initialize(repeating: 0, count: 64)
        scratchLeft.initialize(repeating: 0, count: 16_384)
        scratchRight.initialize(repeating: 0, count: 16_384)
        try super.init(componentDescription: componentDescription, options: options)
        output = try AUAudioUnitBus(format: AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!)
        output.supportedChannelCounts = [2]
        inputs = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: [])
        outputs = AUAudioUnitBusArray(audioUnit: self, busType: .output, busses: [output])
        let level = AUParameterTree.createParameter(withIdentifier: "gain", name: "Gain", address: 0,
            min: 0, max: 1, unit: .linearGain, unitName: nil,
            flags: [.flag_IsReadable, .flag_IsWritable], valueStrings: nil, dependentParameters: nil)
        parameters = AUParameterTree.createTree(withChildren: [level])
        parameters.implementorValueObserver = { [weak self] _, value in
            self?.defaultGain = value
            self?.gain = value
        }
        parameters.implementorValueProvider = { [weak self] _ in self?.gain ?? 1 }
        level.value = 1
        maximumFramesToRender = 16_384
    }

    deinit {
        ringLeft.deinitialize(count: 64); ringLeft.deallocate()
        ringRight.deinitialize(count: 64); ringRight.deallocate()
        scratchLeft.deinitialize(count: scratchCapacity); scratchLeft.deallocate()
        scratchRight.deinitialize(count: scratchCapacity); scratchRight.deallocate()
    }

    override var inputBusses: AUAudioUnitBusArray { inputs }
    override var outputBusses: AUAudioUnitBusArray { outputs }
    override var parameterTree: AUParameterTree? {
        get { parameters }
        set { parameters = newValue }
    }
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
        memset(ringLeft, 0, 64 * MemoryLayout<Float>.size)
        memset(ringRight, 0, 64 * MemoryLayout<Float>.size)
        cursor = 0; heldNote = -1; gain = defaultGain
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
                    switch event.pointee.head.eventType {
                    case .MIDI:
                        let midi = event.pointee.MIDI
                        if midi.length >= 3 {
                            let kind = midi.data.0 & 0xF0, note = Int(midi.data.1)
                            if kind == 0x90 && midi.data.2 > 0 { heldNote = note }
                            else if (kind == 0x80 || (kind == 0x90 && midi.data.2 == 0)) && heldNote == note { heldNote = -1 }
                            else if kind == 0xB0 && (midi.data.1 == 120 || midi.data.1 == 123) { heldNote = -1 }
                        }
                    case .parameter, .parameterRamp:
                        if event.pointee.parameter.parameterAddress == 0 {
                            gain = max(0, min(1, event.pointee.parameter.value))
                        }
                    default: break
                    }
                    next = event.pointee.head.next.map { UnsafePointer($0) }
                }
                let generated: Float = heldNote >= 0 ? 0.25 * gain : 0
                left[frame] = ringLeft[cursor]; right[frame] = ringRight[cursor]
                ringLeft[cursor] = generated; ringRight[cursor] = generated
                cursor = (cursor + 1) & 63
            }
            buffers[0].mDataByteSize = bytes; buffers[1].mDataByteSize = bytes
            return noErr
        }
    }
}

/// An original gain processor with either no latency or a physical 64-frame delay.
/// Parameter changes affect incoming audio before the pipeline, so a serial-chain
/// test distinguishes upstream latency from this effect's own processing latency.
private final class NativeAUMixTimingEffect: AUAudioUnit {
    private var input: AUAudioUnitBus!
    private var output: AUAudioUnitBus!
    private var inputs: AUAudioUnitBusArray!
    private var outputs: AUAudioUnitBusArray!
    private var parameters: AUParameterTree!
    private let delayFrames: Int
    private let ringLeft = UnsafeMutablePointer<Float>.allocate(capacity: 64)
    private let ringRight = UnsafeMutablePointer<Float>.allocate(capacity: 64)
    private var scratchLeft = UnsafeMutablePointer<Float>.allocate(capacity: 16_384)
    private var scratchRight = UnsafeMutablePointer<Float>.allocate(capacity: 16_384)
    private var scratchCapacity = 16_384
    private var cursor = 0
    private var defaultGain: AUValue = 1
    private var gain: AUValue = 1

    override init(componentDescription: AudioComponentDescription,
                  options: AudioComponentInstantiationOptions = []) throws {
        delayFrames = componentDescription.componentSubType == 0x6D676536 ? 64 : 0
        ringLeft.initialize(repeating: 0, count: 64)
        ringRight.initialize(repeating: 0, count: 64)
        scratchLeft.initialize(repeating: 0, count: 16_384)
        scratchRight.initialize(repeating: 0, count: 16_384)
        try super.init(componentDescription: componentDescription, options: options)
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        input = try AUAudioUnitBus(format: format); output = try AUAudioUnitBus(format: format)
        input.supportedChannelCounts = [2]; output.supportedChannelCounts = [2]
        inputs = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: [input])
        outputs = AUAudioUnitBusArray(audioUnit: self, busType: .output, busses: [output])
        let level = AUParameterTree.createParameter(withIdentifier: "gain", name: "Gain", address: 0,
            min: 0, max: 1, unit: .linearGain, unitName: nil,
            flags: [.flag_IsReadable, .flag_IsWritable], valueStrings: nil, dependentParameters: nil)
        parameters = AUParameterTree.createTree(withChildren: [level])
        parameters.implementorValueObserver = { [weak self] _, value in
            self?.defaultGain = value
            self?.gain = value
        }
        parameters.implementorValueProvider = { [weak self] _ in self?.gain ?? 1 }
        level.value = 1
        maximumFramesToRender = 16_384
    }

    deinit {
        ringLeft.deinitialize(count: 64); ringLeft.deallocate()
        ringRight.deinitialize(count: 64); ringRight.deallocate()
        scratchLeft.deinitialize(count: scratchCapacity); scratchLeft.deallocate()
        scratchRight.deinitialize(count: scratchCapacity); scratchRight.deallocate()
    }

    override var inputBusses: AUAudioUnitBusArray { inputs }
    override var outputBusses: AUAudioUnitBusArray { outputs }
    override var parameterTree: AUParameterTree? {
        get { parameters }
        set { parameters = newValue }
    }
    override var channelCapabilities: [NSNumber]? { [2, 2] }
    override var canProcessInPlace: Bool { true }
    override var latency: TimeInterval { Double(delayFrames) / output.format.sampleRate }
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
        memset(ringLeft, 0, 64 * MemoryLayout<Float>.size)
        memset(ringRight, 0, 64 * MemoryLayout<Float>.size)
        cursor = 0; gain = defaultGain
        super.reset()
    }

    override var internalRenderBlock: AUInternalRenderBlock {
        { [unowned self] flags, timestamp, frames, bus, outputData, events, pull in
            guard bus == 0, let pull else { return kAudioUnitErr_NoConnection }
            let count = Int(frames)
            guard count <= scratchCapacity else { return kAudioUnitErr_TooManyFramesToProcess }
            let buffers = UnsafeMutableAudioBufferListPointer(outputData)
            guard buffers.count == 2 else { return kAudioUnitErr_FormatNotSupported }
            let bytes = UInt32(count * MemoryLayout<Float>.size)
            if buffers[0].mData == nil { buffers[0].mData = UnsafeMutableRawPointer(scratchLeft); buffers[0].mDataByteSize = bytes }
            if buffers[1].mData == nil { buffers[1].mData = UnsafeMutableRawPointer(scratchRight); buffers[1].mDataByteSize = bytes }
            guard buffers[0].mDataByteSize >= bytes, buffers[1].mDataByteSize >= bytes else { return kAudio_ParamError }
            let result = pull(flags, timestamp, frames, 0, outputData)
            guard result == noErr else { return result }
            guard let left = buffers[0].mData?.assumingMemoryBound(to: Float.self),
                  let right = buffers[1].mData?.assumingMemoryBound(to: Float.self) else { return kAudio_ParamError }
            let silent = flags.pointee.contains(.unitRenderAction_OutputIsSilence)
            flags.pointee.remove(.unitRenderAction_OutputIsSilence)
            var next = events
            let sampleTime = AUEventSampleTime(timestamp.pointee.mSampleTime)
            for frame in 0..<count {
                while let event = next, event.pointee.head.eventSampleTime <= sampleTime + AUEventSampleTime(frame) {
                    if (event.pointee.head.eventType == .parameter || event.pointee.head.eventType == .parameterRamp),
                       event.pointee.parameter.parameterAddress == 0 {
                        gain = max(0, min(1, event.pointee.parameter.value))
                    }
                    next = event.pointee.head.next.map { UnsafePointer($0) }
                }
                let level: Float = shouldBypassEffect ? 1 : gain
                let inputLeft = silent ? 0 : left[frame] * level
                let inputRight = silent ? 0 : right[frame] * level
                if delayFrames == 0 {
                    left[frame] = inputLeft; right[frame] = inputRight
                } else {
                    left[frame] = ringLeft[cursor]; right[frame] = ringRight[cursor]
                    ringLeft[cursor] = inputLeft; ringRight[cursor] = inputRight
                    cursor = (cursor + 1) & 63
                }
            }
            buffers[0].mDataByteSize = bytes; buffers[1].mDataByteSize = bytes
            return noErr
        }
    }
}

#endif

// Standalone original-fixture graph tests. Manual offline rendering only; no GUI,
// vendor plug-ins, speaker output, project files, or application build required.
// swiftc -swift-version 5 -target arm64-apple-macos14.0 \
//   -module-cache-path /private/tmp/supertracker-graph-fixture-module-cache \
//   Sources/SuperTracker/AudioRouting.swift Sources/SuperTracker/AudioMixAutomation.swift \
//   Sources/SuperTracker/AudioDelayCompensation.swift Sources/SuperTracker/AudioRoutingCapture.swift \
//   Sources/SuperTracker/AudioRoutingGraph.swift tests/AudioRoutingGraphTests.swift \
//   -o /private/tmp/supertracker-routing-graph-tests
// /private/tmp/supertracker-routing-graph-tests [optional test name]
//
// Actual VST3 adapter integration additionally requires the repository's original
// test-output/SidechainFixture.vst3 and native objects built for macOS 14 at the
// paths below. The module map exposes Sources/VST3Host/include/VST3Host.h.
// swiftc -D VST3_ROUTING_ADAPTER_TEST -swift-version 5 -target arm64-apple-macos14.0 \
//   -module-cache-path /private/tmp/supertracker-graph-fixture-module-cache \
//   -I test-output/vst3-sidechain-native-build/modules \
//   Sources/SuperTracker/AudioRouting.swift Sources/SuperTracker/AudioMixAutomation.swift \
//   Sources/SuperTracker/AudioDelayCompensation.swift Sources/SuperTracker/AudioRoutingCapture.swift \
//   Sources/SuperTracker/AudioRoutingGraph.swift Sources/SuperTracker/VST3AudioUnit.swift \
//   Sources/SuperTracker/VST3Catalogue.swift Sources/SuperTracker/VST3Compatibility.swift \
//   tests/AudioRoutingGraphTests.swift test-output/vst3-sidechain-native-build/VST3Host.o \
//   test-output/vst3-sidechain-native-build/VST3SDK.o -framework AppKit -framework CoreFoundation \
//   -framework AVFoundation -framework AudioToolbox -framework CoreAudioKit -lc++ \
//   -o /private/tmp/supertracker-routing-vst3-graph-tests
// /private/tmp/supertracker-routing-vst3-graph-tests vst3-indexed
import Foundation
import AVFoundation
import AudioToolbox

private let routingTestRate = 48_000.0
private let routingTestFormat = AVAudioFormat(standardFormatWithSampleRate: routingTestRate, channels: 2)!

private struct TestFailure: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

private func require(_ condition: @autoclosure () -> Bool, _ message: String) throws {
    guard condition() else { throw TestFailure(message: message) }
}

/// Read only after synchronous offline rendering returns. Overlapping timestamps
/// detect duplicate advancement even when a fixture's deterministic PCM hides it.
private final class RenderProbe {
    var calls = 0
    var frames = 0
    var overlappingFrames = 0
    var lastEnd: Int64?
    func record(_ timestamp: Double, _ count: Int) {
        let start = Int64(timestamp)
        if let lastEnd, start < lastEnd { overlappingFrames += min(count, Int(lastEnd - start)) }
        lastEnd = start + Int64(count); calls += 1; frames += count
    }
}

private final class OriginalSource {
    let probe = RenderProbe()
    let node: AVAudioSourceNode
    init(channels: AVAudioChannelCount = 2, sample: @escaping (Int64, Int) -> Float) throws {
        let probe = self.probe
        let format = AVAudioFormat(standardFormatWithSampleRate: routingTestRate, channels: channels)!
        node = AVAudioSourceNode(format: format) { silent, timestamp, count, list in
            probe.record(timestamp.pointee.mSampleTime, Int(count))
            let buffers = UnsafeMutableAudioBufferListPointer(list)
            var allSilent = true
            for (channel, buffer) in buffers.enumerated() {
                guard let output = buffer.mData?.assumingMemoryBound(to: Float.self) else { return kAudio_ParamError }
                for frame in 0..<Int(count) {
                    let value = sample(Int64(timestamp.pointee.mSampleTime) + Int64(frame), channel)
                    output[frame] = value; allSilent = allSilent && value == 0
                }
            }
            silent.pointee = ObjCBool(allSilent)
            return noErr
        }
        // AVAudioSourceNode's initializer specifies its render-block format but
        // leaves the exposed output bus at 44.1 kHz until explicitly configured.
        try node.auAudioUnit.outputBusses[0].setFormat(format)
    }
    convenience init(left: Float, right: Float) throws {
        try self.init { _, channel in channel == 0 ? left : right }
    }
}

/// Original AU: affine processing makes sparse insert order audible, an actual
/// second input controls a precise 1/8 gain, and an optional ring reports its real
/// algorithmic latency. Both main0/key1 and key0/main1 layouts are supported.
private final class OriginalRoutingEffect: AUAudioUnit {
    private var ins: AUAudioUnitBusArray!
    private var outs: AUAudioUnitBusArray!
    private var mainScratch: AVAudioPCMBuffer!
    private var keyScratch: AVAudioPCMBuffer!
    private var delayLeft = [Float](repeating: 0, count: 4096)
    private var delayRight = [Float](repeating: 0, count: 4096)
    private var delayCursor = 0
    let probe = RenderProbe()
    private(set) var mainPulls = 0
    private(set) var keyPulls = 0
    var mainBus = 0
    var gain: Float = 1
    var bias: Float = 0
    var delayFrames = 0
    var keyBus: Int { 1 - mainBus }

    override init(componentDescription: AudioComponentDescription,
                  options: AudioComponentInstantiationOptions = []) throws {
        try super.init(componentDescription: componentDescription, options: options)
        let a = try AUAudioUnitBus(format: routingTestFormat)
        let b = try AUAudioUnitBus(format: routingTestFormat)
        let output = try AUAudioUnitBus(format: routingTestFormat)
        a.supportedChannelCounts = [1, 2]; b.supportedChannelCounts = [1, 2]
        output.supportedChannelCounts = [2]
        ins = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: [a, b])
        outs = AUAudioUnitBusArray(audioUnit: self, busType: .output, busses: [output])
        a.name = "Main"; b.name = "Key"; b.isEnabled = false
        maximumFramesToRender = 16_384
    }
    override var inputBusses: AUAudioUnitBusArray { ins }
    override var outputBusses: AUAudioUnitBusArray { outs }
    override var latency: TimeInterval { Double(delayFrames) / routingTestRate }
    override var canProcessInPlace: Bool { true }
    override var maximumFramesToRender: AUAudioFrameCount {
        get { max(16_384, super.maximumFramesToRender) }
        set { super.maximumFramesToRender = max(16_384, newValue) }
    }
    override func shouldChange(to format: AVAudioFormat, for bus: AUAudioUnitBus) -> Bool {
        !renderResourcesAllocated && format.sampleRate == routingTestRate &&
            (1...2).contains(format.channelCount) && format.commonFormat == .pcmFormatFloat32 && !format.isInterleaved
    }
    func configure(mainBus: Int, keyChannels: AVAudioChannelCount, gain: Float, bias: Float, delay: Int) throws {
        self.mainBus = mainBus; self.gain = gain; self.bias = bias; delayFrames = delay
        ins[mainBus].name = "Main"; ins[keyBus].name = "Key"
        try ins[mainBus].setFormat(routingTestFormat)
        try ins[keyBus].setFormat(AVAudioFormat(standardFormatWithSampleRate: routingTestRate, channels: keyChannels)!)
        ins[mainBus].isEnabled = true; ins[keyBus].isEnabled = false
    }
    override func allocateRenderResources() throws {
        mainScratch = AVAudioPCMBuffer(pcmFormat: routingTestFormat, frameCapacity: maximumFramesToRender)!
        keyScratch = AVAudioPCMBuffer(pcmFormat: ins[keyBus].format, frameCapacity: maximumFramesToRender)!
        delayCursor = 0
        for index in delayLeft.indices { delayLeft[index] = 0; delayRight[index] = 0 }
        try super.allocateRenderResources()
    }
    override var internalRenderBlock: AUInternalRenderBlock {
        { [unowned self] flags, timestamp, count, bus, output, _, pull in
            guard bus == 0, let pull, count <= maximumFramesToRender else { return kAudio_ParamError }
            probe.record(timestamp.pointee.mSampleTime, Int(count))
            let buffers = UnsafeMutableAudioBufferListPointer(output)
            guard buffers.count == 2 else { return kAudioUnitErr_FormatNotSupported }
            for channel in 0..<2 where buffers[channel].mData == nil {
                buffers[channel].mData = UnsafeMutableRawPointer(mainScratch.floatChannelData![channel])
                buffers[channel].mDataByteSize = count * 4
            }
            mainPulls += 1
            let mainStatus = pull(flags, timestamp, count, mainBus, output)
            guard mainStatus == noErr else { return mainStatus }
            let mainSilent = flags.pointee.contains(.unitRenderAction_OutputIsSilence)
            var keySilent = true
            if ins[keyBus].isEnabled {
                keyScratch.frameLength = count
                var keyFlags = AudioUnitRenderActionFlags()
                keyPulls += 1
                let keyStatus = pull(&keyFlags, timestamp, count, keyBus, keyScratch.mutableAudioBufferList)
                guard keyStatus == noErr else { return keyStatus }
                keySilent = keyFlags.contains(.unitRenderAction_OutputIsSilence)
            }
            guard let left = buffers[0].mData?.assumingMemoryBound(to: Float.self),
                  let right = buffers[1].mData?.assumingMemoryBound(to: Float.self) else { return kAudio_ParamError }
            var allSilent = true
            for frame in 0..<Int(count) {
                let key: Float = keySilent ? 0 : abs(keyScratch.floatChannelData![0][frame])
                let reduction: Float = key > 0.25 ? 0.125 : 1
                var l = ((mainSilent ? 0 : left[frame]) * gain + bias) * reduction
                var r = ((mainSilent ? 0 : right[frame]) * gain + bias) * reduction
                if delayFrames > 0 {
                    let oldL = delayLeft[delayCursor], oldR = delayRight[delayCursor]
                    delayLeft[delayCursor] = l; delayRight[delayCursor] = r
                    l = oldL; r = oldR; delayCursor = (delayCursor + 1) % delayFrames
                }
                left[frame] = l; right[frame] = r; allSilent = allSilent && l == 0 && r == 0
            }
            if allSilent { flags.pointee.insert(.unitRenderAction_OutputIsSilence) }
            else { flags.pointee.remove(.unitRenderAction_OutputIsSilence) }
            return noErr
        }
    }
}

private struct EffectFixture {
    let node: AVAudioUnitEffect
    let unit: OriginalRoutingEffect
    private static let component = AudioComponentDescription(componentType: kAudioUnitType_Effect,
        componentSubType: 0x72746774, componentManufacturer: 0x53745473, componentFlags: 0, componentFlagsMask: 0)
    private static let register: Void = {
        AUAudioUnit.registerSubclass(OriginalRoutingEffect.self, as: component,
            name: "SuperTracker Tests: Original Routing Effect", version: 1)
    }()
    init(mainBus: Int = 0, keyChannels: AVAudioChannelCount = 2,
         gain: Float = 1, bias: Float = 0, delay: Int = 0) throws {
        _ = Self.register
        node = AVAudioUnitEffect(audioComponentDescription: Self.component)
        guard let unit = node.auAudioUnit as? OriginalRoutingEffect else {
            throw TestFailure(message: "Original routing AU registration failed")
        }
        self.unit = unit
        try unit.configure(mainBus: mainBus, keyChannels: keyChannels, gain: gain, bias: bias, delay: delay)
    }
    func effect(_ id: String, owner: String = "track:a", slot: Int = 0) -> AudioRoutingGraph.Effect {
        AudioRoutingGraph.Effect(owner: owner, slot: slot, instanceID: id, node: node, mainInputBus: unit.mainBus,
            inputBuses: [.init(index: unit.keyBus, name: "Key", channels: Int(unit.inputBusses[unit.keyBus].format.channelCount), isAuxiliary: true)])
    }
}

private struct StereoSamples {
    var left: [Float] = []
    var right: [Float] = []
    mutating func append(_ buffer: AVAudioPCMBuffer) {
        for frame in 0..<Int(buffer.frameLength) {
            left.append(buffer.floatChannelData![0][frame]); right.append(buffer.floatChannelData![1][frame])
        }
    }
    func matches(_ label: String, tolerance: Float = 0.00002, expected: (Int, Int) -> Float) throws {
        for (channel, values) in [left, right].enumerated() {
            for (frame, value) in values.enumerated() {
                let wanted = expected(frame, channel)
                try require(value.isFinite && abs(value - wanted) <= tolerance,
                    "\(label): channel \(channel), frame \(frame), expected \(wanted), got \(value)")
            }
        }
    }
}

private final class GraphFixture {
    let engine = AVAudioEngine()
    let routing: AudioRoutingGraph
    let buffer = AVAudioPCMBuffer(pcmFormat: routingTestFormat, frameCapacity: 512)!
    init() throws {
        // Must precede touching mainMixerNode/outputNode, so no audio device starts.
        try engine.enableManualRenderingMode(.offline, format: routingTestFormat, maximumFrameCount: 512)
        routing = try AudioRoutingGraph(engine: engine, sampleRate: routingTestRate, maximumDelayFrames: 4096)
    }
    deinit { engine.stop() }
    func render(_ frames: Int, chunks: [Int] = [17, 127, 64, 251, 53],
                afterBlock: (() throws -> Void)? = nil) throws -> StereoSamples {
        if !engine.isRunning { try engine.start() }
        var result = StereoSamples(), index = 0, retries = 0
        while result.left.count < frames {
            let count = min(chunks[index % chunks.count], frames - result.left.count)
            let status = try engine.renderOffline(AVAudioFrameCount(count), to: buffer)
            if status == .success {
                try require(buffer.frameLength == count, "Offline render returned an incomplete successful block")
                result.append(buffer); index += 1; retries = 0
                try afterBlock?()
            } else if status == .cannotDoInCurrentContext && retries < 20 { retries += 1 }
            else { throw TestFailure(message: "Offline render failed: \(status.rawValue)") }
        }
        return result
    }
    func stop() { engine.stop() }
}

private func definition(tracks: [[String: Any]] = [["routingID": "a"], ["routingID": "b"]],
                        buses: [[String: Any]] = [], outputs: [String: Any] = [:],
                        sends: [[String: Any]] = [], sidechains: [[String: Any]] = [],
                        master: Double = 1) throws -> AudioRouting.Definition {
    let explicitTracks = tracks.map { track -> [String: Any] in
        var result = track
        if result["volume"] == nil { result["volume"] = 1.0 }
        return result
    }
    return try AudioRouting.parse(project: ["tracks": explicitTracks, "masterVolume": master,
        "routing": ["version": 1, "mixModel": "standard", "buses": buses,
                    "outputs": outputs, "sends": sends, "sidechains": sidechains]])
}
private func send(_ id: String, source: String = "track:a", destination: String = "bus:r",
                  tap: String = "postFader", gain: Double = 1) -> [String: Any] {
    ["id": id, "source": source, "destination": destination, "tap": tap, "gain": gain, "enabled": true]
}
private func key(_ id: String = "key", source: String = "track:b", instance: String = "keyed",
                 bus: Int = 1, channels: Int = 2, enabled: Bool = true) -> [String: Any] {
    ["id": id, "source": source, "tap": "postFader", "gain": 1, "enabled": enabled,
     "destination": ["instanceID": instance, "inputBus": bus, "layoutSignature": ["name": "Key", "channels": channels]]]
}
private func returnBus(_ id: String = "r") -> [String: Any] { ["id": id, "name": id, "type": "return"] }

/// Physical node identities, bus numbers, formats and attachments; equality after
/// rollback detects lost external fanouts and orphaned candidate helpers.
private func physicalGraph(_ engine: AVAudioEngine) -> Set<String> {
    var result = Set<String>()
    for node in engine.attachedNodes {
        let id = String(describing: ObjectIdentifier(node)); result.insert("node:" + id)
        for bus in 0..<node.numberOfOutputs {
            let format = node.outputFormat(forBus: bus)
            for destination in engine.outputConnectionPoints(for: node, outputBus: bus) {
                if let destinationNode = destination.node {
                    result.insert("\(id):\(bus)>\(ObjectIdentifier(destinationNode)):\(destination.bus)@\(format.sampleRate)/\(format.channelCount)")
                }
            }
        }
    }
    return result
}

@main
private struct AudioRoutingGraphTests {
    static func stripsAndSparseEffects() throws {
        let fixture = try GraphFixture(), a = try OriginalSource(left: 0.2, right: 0.4), b = try OriginalSource(left: 0.6, right: 0.1)
        let first = try EffectFixture(gain: 2, bias: 0.1), last = try EffectFixture(gain: 0.5, bias: 0.02)
        let model = try definition(tracks: [["routingID": "a", "volume": 0.5], ["routingID": "b", "volume": 0.25]],
            buses: [["id": "g", "name": "Group", "type": "group", "volume": 0.5]],
            outputs: ["track:a": "bus:g"], master: 0.8)
        let before = physicalGraph(fixture.engine)
        let prepared = try fixture.routing.prepare(definition: model, sources: ["track:a": a.node, "track:b": b.node],
            effects: [last.effect("last", slot: 6), first.effect("first", slot: 1)])
        try require(physicalGraph(fixture.engine) == before, "prepare changed the live graph")
        try fixture.routing.install(prepared)
        let samples = try fixture.render(512)
        try samples.matches("standard track/group/master gains and sparse insert order") { _, channel in
            let inputA: Float = channel == 0 ? 0.2 : 0.4, inputB: Float = channel == 0 ? 0.6 : 0.1
            return (((inputA * 2 + 0.1) * 0.5 + 0.02) * 0.5 * 0.5 + inputB * 0.25) * 0.8
        }
        try require(first.unit.probe.overlappingFrames == 0 && last.unit.probe.overlappingFrames == 0, "Sparse inserts advanced twice")
    }

    static func fanoutAdvancesOnce() throws {
        let fixture = try GraphFixture(), a = try OriginalSource(left: 0.8, right: 0.4), b = try OriginalSource(left: 0.16, right: 0.08)
        let split = try EffectFixture(gain: 0.5), keyed = try EffectFixture()
        let model = try definition(buses: [returnBus()], sends: [send("parallel", gain: 0.75)],
            sidechains: [key(source: "track:a", instance: "keyed-b")])
        let prepared = try fixture.routing.prepare(definition: model, sources: ["track:a": a.node, "track:b": b.node],
            effects: [split.effect("split"), keyed.effect("keyed-b", owner: "track:b")])
        try fixture.routing.install(prepared)
        let destinations = fixture.engine.outputConnectionPoints(for: prepared.strips["track:a"]!.postFader, outputBus: 0)
        try require(destinations.count == 3, "Post-fader output/send/key branches were not installed together")
        try fixture.render(1024).matches("fanout output + send + keyed destination") { _, channel in
            let value: Float = channel == 0 ? 0.4 : 0.2, main: Float = channel == 0 ? 0.16 : 0.08
            return value * 1.75 + main * 0.125
        }
        for (label, probe) in [("source", a.probe), ("insert", split.unit.probe), ("keyed insert", keyed.unit.probe)] {
            try require(probe.overlappingFrames == 0 && probe.frames == 1024, "\(label) advanced more than once through fanout")
        }
        try require(keyed.unit.keyPulls > 0, "Auxiliary input was never pulled")
    }

    static func sidechainLayouts() throws {
        for mainBus in [0, 1] {
            for channels: AVAudioChannelCount in [1, 2] {
                let fixture = try GraphFixture(), a = try OriginalSource(left: 0.4, right: 0.2)
                let b = try OriginalSource { time, _ in (64..<192).contains(time) ? 0.8 : 0 }
                let keyed = try EffectFixture(mainBus: mainBus, keyChannels: channels)
                let model = try definition(outputs: ["track:b": NSNull()],
                    sidechains: [key(bus: 1 - mainBus, channels: Int(channels))])
                let prepared = try fixture.routing.prepare(definition: model, sources: ["track:a": a.node, "track:b": b.node],
                    effects: [keyed.effect("keyed", slot: 5)])
                try require(prepared.graph.unavailableSidechains.isEmpty, "Supported auxiliary bus did not resolve")
                try fixture.routing.install(prepared)
                try fixture.render(320).matches("main\(mainBus)/key\(1 - mainBus), \(channels) channel key") { frame, channel in
                    let input: Float = channel == 0 ? 0.4 : 0.2
                    return input * ((64..<192).contains(frame) ? 0.125 : 1)
                }
                try require(keyed.unit.mainPulls > 0 && keyed.unit.keyPulls > 0, "Main and key must each pull their actual bus")
                try require(b.probe.overlappingFrames == 0, "Disconnected audible key source advanced twice")
            }
        }
    }

    static func alignedMainAndKeyImpulse() throws {
        let fixture = try GraphFixture()
        let a = try OriginalSource { frame, channel in frame == 64 ? (channel == 0 ? 0.5 : 0.25) : 0 }
        let b = try OriginalSource { frame, _ in frame == 101 ? 1 : 0 }
        let keyed = try EffectFixture(mainBus: 1, delay: 17)
        let model = try definition(outputs: ["track:b": NSNull()], sidechains: [key(bus: 0)])
        let prepared = try fixture.routing.prepare(definition: model, sources: ["track:a": a.node, "track:b": b.node],
            effects: [keyed.effect("keyed", slot: 3)], sourceLatencies: ["track:b": 37])
        try require(prepared.pluginInputArrival["keyed"] == 37 && prepared.latencyFrames == 54, "Main/key PDC included wrong latency")
        try fixture.routing.install(prepared)
        try fixture.render(256).matches("37-frame main/key alignment plus 17-frame effect delay") { frame, channel in
            frame == 118 ? (channel == 0 ? 0.5 : 0.25) * 0.125 : 0
        }
    }

    static func summedKeys() throws {
        let fixture = try GraphFixture(), a = try OriginalSource(left: 0.2, right: 0.1)
        let b = try OriginalSource { frame, _ in frame >= 13 ? 0.2 : 0 }
        let keyed = try EffectFixture()
        var ownKey = key("own-key", source: "track:a")
        ownKey["tap"] = "preInserts"
        let model = try definition(outputs: ["track:b": NSNull()], sidechains: [ownKey, key("external-key")])
        let prepared = try fixture.routing.prepare(definition: model, sources: ["track:a": a.node, "track:b": b.node],
            effects: [keyed.effect("keyed")], sourceLatencies: ["track:b": 13])
        try require(prepared.graph.resolvedSidechains.count == 2 && prepared.pluginInputArrival["keyed"] == 13,
            "Two keys did not preserve independent routing and latency")
        try fixture.routing.install(prepared)
        try fixture.render(256).matches("two individually subthreshold keys sum after independent PDC") { frame, channel in
            frame < 13 ? 0 : (channel == 0 ? 0.2 : 0.1) * 0.125
        }
        try require(a.probe.frames == 256 && a.probe.overlappingFrames == 0, "Self key and main duplicated source advancement")
    }

    static func fixedSendsAndAutomation() throws {
        let fixture = try GraphFixture(), a = try OriginalSource(left: 0.5, right: 0.25), b = try OriginalSource(left: 0, right: 0)
        let wet = try EffectFixture()
        let model = try definition(tracks: [["routingID": "a", "volume": 0.5], ["routingID": "b"]],
            outputs: ["track:a": NSNull(), "track:b": NSNull()])
        let prepared = try fixture.routing.prepare(definition: model, sources: ["track:a": a.node, "track:b": b.node], effects: [],
            systemReturns: [.init(id: "delay", processor: wet.node, gains: ["track:a": 0.4], returnGain: 0.5)])
        try fixture.routing.install(prepared)
        // AU allocation starts a new automation epoch. The host schedules each
        // quantum after engine initialization and before the matching render.
        try fixture.engine.start()
        prepared.scheduleMix(owner: "track:a", target: 0, value: 1, sampleTime: 64)
        prepared.scheduleBuiltinSend(owner: "track:a", target: 4, value: 0.8, sampleTime: 128)
        try fixture.render(256).matches("fixed wet send follows track fader and independent send automation") { frame, channel in
            let input: Float = channel == 0 ? 0.5 : 0.25
            return input * (frame < 64 ? 0.5 : 1) * (frame < 128 ? 0.4 : 0.8) * 0.5
        }
    }

    static func soloKeepsKeyDependency() throws {
        let fixture = try GraphFixture(), a = try OriginalSource(left: 0.4, right: 0.2), b = try OriginalSource(left: 0.8, right: 0.8)
        let keyed = try EffectFixture(), wet = try EffectFixture()
        let model = try definition(tracks: [["routingID": "a", "solo": true], ["routingID": "b"]],
            buses: [returnBus()], sends: [send("key-audible-send", source: "track:b")], sidechains: [key()])
        let prepared = try fixture.routing.prepare(definition: model, sources: ["track:a": a.node, "track:b": b.node],
            effects: [keyed.effect("keyed")],
            systemReturns: [.init(id: "reverb", processor: wet.node, gains: ["track:b": 1])])
        let dependencies = try prepared.graph.dependencies(of: [AudioRouting.nodeID(owner: "track:a", stage: .postFader)])
        try require(dependencies.contains(AudioRouting.nodeID(owner: "track:b", stage: .source)), "Soloed track lost its detector dependency")
        try fixture.routing.install(prepared)
        try fixture.render(256).matches("solo silences key output and sends while retaining detector") { _, channel in
            (channel == 0 ? 0.4 : 0.2) * 0.125
        }
        try require(b.probe.frames == 256 && b.probe.overlappingFrames == 0, "Solo did not retain one key-source advancement")
        let physical = physicalGraph(fixture.engine)
        let cleared = try definition(buses:[returnBus()],sends:[send("key-audible-send",source:"track:b")],sidechains:[key()])
        prepared.updateSolo(cleared)
        try fixture.render(256).matches("clearing live solo restores dry, user send and builtin return") { _,channel in
            (channel == 0 ? 0.4 : 0.2)*0.125 + 2.4
        }
        prepared.updateSolo(model)
        try fixture.render(256).matches("reapplying live solo keeps detector but gates its audible branches") { _,channel in
            (channel == 0 ? 0.4 : 0.2)*0.125
        }
        try require(physicalGraph(fixture.engine) == physical && b.probe.frames == 768 && b.probe.overlappingFrames == 0,
                    "Live solo changed graph topology or duplicated detector advancement")
    }

    static func soloKeepsBusKeyAncestors() throws {
        let fixture = try GraphFixture(), a = try OriginalSource(left: 0.4, right: 0.2), b = try OriginalSource(left: 0.8, right: 0.8)
        let keyed = try EffectFixture()
        let model = try definition(tracks: [["routingID": "a", "solo": true], ["routingID": "b"]],
            buses: [["id": "k", "name": "Key bus", "type": "group"]], outputs: ["track:b": "bus:k"],
            sidechains: [key(source: "bus:k")])
        let prepared = try fixture.routing.prepare(definition: model, sources: ["track:a": a.node, "track:b": b.node], effects: [keyed.effect("keyed")])
        try fixture.routing.install(prepared)
        try fixture.render(256).matches("solo retains key-bus ancestors and excludes the bus audible output") { _, channel in
            (channel == 0 ? 0.4 : 0.2) * 0.125
        }
        try require(b.probe.frames == 256 && b.probe.overlappingFrames == 0, "Solo pruned or duplicated the key-bus input source")
    }

    /// A soloed track keeps its pre-insert/pre-fader sends audible, like its post-fader sends.
    static func soloKeepsEarlySends() throws {
        let fixture = try GraphFixture(), a = try OriginalSource(left: 0.4, right: 0.2), b = try OriginalSource(left: 0.8, right: 0.8)
        let model = try definition(tracks: [["routingID": "a", "solo": true, "volume": 0.5], ["routingID": "b"]],
            buses: [returnBus()], sends: [send("early", tap: "preFader"), send("other", source: "track:b")])
        let prepared = try fixture.routing.prepare(definition: model, sources: ["track:a": a.node, "track:b": b.node], effects: [])
        try fixture.routing.install(prepared)
        try fixture.render(256).matches("soloed track's pre-fader send reaches its return") { _, channel in
            (channel == 0 ? 0.4 : 0.2) * 1.5
        }
    }

    static func installRollback() throws {
        let fixture = try GraphFixture(), a = try OriginalSource(left: 0.2, right: 0.1), b = try OriginalSource(left: 0.4, right: 0.2)
        let model = try definition(buses: [returnBus()], sends: [send("parallel", gain: 0.5)])
        let original = try fixture.routing.prepare(definition: model, sources: ["track:a": a.node, "track:b": b.node], effects: [])
        try fixture.routing.install(original); try fixture.routing.commit()
        let before = physicalGraph(fixture.engine)
        let candidateEffect = try EffectFixture(gain: 0.125)
        let candidate = try fixture.routing.prepare(definition: model, sources: ["track:a": a.node, "track:b": b.node],
            effects: [candidateEffect.effect("candidate")])
        do {
            try fixture.routing.install(candidate) { throw TestFailure(message: "injected post-connect validation failure") }
            throw TestFailure(message: "install accepted failed validation")
        } catch let error as TestFailure {
            try require(error.message == "injected post-connect validation failure", "install failed for an unexpected reason: \(error.message)")
        }
        try require(fixture.routing.current === original, "Failed install published the candidate")
        try require(physicalGraph(fixture.engine) == before, "Rollback did not restore every physical fanout and attachment")
        try require(candidateEffect.node.engine == nil, "Rollback leaked an attached new external effect")
        try fixture.render(256).matches("resumed original graph after validation failure") { _, channel in channel == 0 ? 0.7 : 0.35 }
    }

    static func capturesDisconnectedOwner() throws {
        let fixture = try GraphFixture(), a = try OriginalSource(left: 0.2, right: 0.1)
        let b = try OriginalSource { frame, channel in frame >= 29 ? (channel == 0 ? 0.6 : 0.3) : 0 }
        let model = try definition(outputs: ["track:b": NSNull()])
        let prepared = try fixture.routing.prepare(definition: model, sources: ["track:a": a.node, "track:b": b.node], effects: [],
            sourceLatencies: ["track:b": 29], captureOwners: ["track:b", "master"], captureCapacityFrames: 512)
        let selected = prepared.captures["track:b"]!, master = prepared.captures["master"]!
        try require(prepared.latencyFrames == 29, "Silent capture anchor was not included in latency alignment")
        try selected.configureCapture(enabled: true, arrivalFrames: 29, frameLimit: 128)
        try master.configureCapture(enabled: true, arrivalFrames: 29, frameLimit: 128)
        try fixture.routing.install(prepared)
        let samples = try fixture.render(256)
        try samples.matches("disconnected captured owner is inaudible at master") { frame, channel in
            frame >= 29 ? (channel == 0 ? 0.2 : 0.1) : 0
        }
        let drain = AVAudioPCMBuffer(pcmFormat: routingTestFormat, frameCapacity: 512)!
        let selectedChunk = try selected.drain(into: drain)
        try require(selectedChunk.startFrame == 0 && selectedChunk.frameCount == 128, "Selected capture did not trim pre-roll and limit")
        var selectedSamples = StereoSamples(); selectedSamples.append(drain)
        try selectedSamples.matches("disconnected stem retains PCM") { _, channel in channel == 0 ? 0.6 : 0.3 }
        let masterChunk = try master.drain(into: drain)
        try require(masterChunk.startFrame == 0 && masterChunk.frameCount == 128, "Master capture interval changed")
        var masterSamples = StereoSamples(); masterSamples.append(drain)
        try masterSamples.matches("master capture excludes silent anchor") { _, channel in channel == 0 ? 0.2 : 0.1 }
        try require(b.probe.overlappingFrames == 0 && selected.error == nil && master.error == nil, "Capture duplicated source pulls or lost frames")
    }

    #if VST3_ROUTING_ADAPTER_TEST
    /// This optional integration uses only the repository's original VST3 bundle.
    /// The C++ owner builds its module and objects separately from the application.
    static func vst3IndexedAdapter() throws {
        let bundle = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("test-output/SidechainFixture.vst3").path
        try require(FileManager.default.fileExists(atPath: bundle), "Build the original SidechainFixture.vst3 before adapter tests")
        let layouts: [(String, String, Int, Int, Int)] = [
            ("A0B10000000000000000000000000001", "Stereo Key Fixture", 0, 2, 2),
            ("A0B10000000000000000000000000002", "Mono Key Fixture", 0, 2, 1),
            ("A0B10000000000000000000000000003", "Swapped Main Fixture", 1, 0, 2)
        ]
        for (classID, name, main, auxiliary, channels) in layouts {
            let fixture = try GraphFixture()
            let a = try OriginalSource { frame, channel in
                (384..<448).contains(frame) ? 0 : (channel == 0 ? 0.5 : 0.25)
            }
            let b = try OriginalSource { frame, channel in
                (128..<256).contains(frame) ? 0 : (channel == 0 || channels == 1 ? 0.125 : 0.0625)
            }
            let descriptor = VST3PluginDescriptor(classID: classID, bundlePath: bundle, name: name,
                manufacturer: "SuperTracker Tests", version: "1", category: "Fx", isInstrument: false)
            let node = try VST3AudioUnit.makeNode(descriptor: descriptor, sampleRate: routingTestRate)
            guard let adapter = node.auAudioUnit as? VST3AudioUnit else { throw TestFailure(message: "Original VST3 adapter missing") }
            try require(adapter.mainInputBus == main, "VST3 main input ordinal changed")
            let effect = AudioRoutingGraph.Effect(owner: "track:a", slot: 4, instanceID: "indexed", node: node,
                mainInputBus: adapter.mainInputBus, inputBuses: adapter.inputBusMetadata(),
                configureAuxiliary: { try adapter.configureAuxiliaryInput(bus: $0, enabled: $1) })
            func model(enabled: Bool) throws -> AudioRouting.Definition {
                try definition(outputs: ["track:b": NSNull()], sidechains: [[
                    "id": "indexed-key", "source": "track:b", "tap": "postFader", "gain": 1, "enabled": enabled,
                    "destination": ["instanceID": "indexed", "inputBus": auxiliary,
                        "layoutSignature": ["name": "Detector Key", "channels": channels]]
                ]])
            }
            let prepared = try fixture.routing.prepare(definition: model(enabled: true),
                sources: ["track:a": a.node, "track:b": b.node], effects: [effect])
            try require(prepared.graph.unavailableSidechains.isEmpty, "Original VST3 layout failed to resolve")
            try fixture.routing.install(prepared)
            try require(adapter.inputBusses[main].isEnabled && adapter.inputBusses[auxiliary].isEnabled, "Actual VST3 inputs were not enabled")
            if main == 0 { try require(!adapter.inputBusses[1].isEnabled, "Unused VST3 auxiliary bus was activated") }
            // Aligned silent blocks exercise independent input action flags:
            // a silent key must not silence main, nor may silent main erase key.
            try fixture.render(512, chunks: [64]).matches("VST3 \(name) indexed main/key subtraction") { frame, channel in
                let mainValue: Float = (384..<448).contains(frame) ? 0 : (channel == 0 ? 0.5 : 0.25)
                // AVAudioMixerNode's default stereo-to-mono matrix uses equal-
                // power coefficients, so identical L/R keys sum to sqrt(2)*key.
                let keyValue: Float = (128..<256).contains(frame) ? 0 :
                    (channels == 1 ? 0.125 * Float(2.0.squareRoot()) : (channel == 0 ? 0.125 : 0.0625))
                return mainValue - keyValue
            }
            try require(a.probe.frames == 512 && b.probe.frames == 512 && a.probe.overlappingFrames == 0 && b.probe.overlappingFrames == 0,
                "VST3 indexed input pull duplicated or skipped a source")
            adapter.shouldBypassEffect = true
            try fixture.render(128).matches("VST3 \(name) bypass uses the actual main bus") { _, channel in channel == 0 ? 0.5 : 0.25 }
            adapter.shouldBypassEffect = false
            fixture.stop()
            let disabled = try fixture.routing.prepare(definition: model(enabled: false),
                sources: ["track:a": a.node, "track:b": b.node], effects: [effect])
            try fixture.routing.install(disabled)
            try require(!adapter.inputBusses[auxiliary].isEnabled && adapter.inputBusses[main].isEnabled,
                "Disabling a VST3 sidechain changed the main input or left the key active")
            try fixture.render(128).matches("VST3 \(name) key disabled") { _, channel in channel == 0 ? 0.5 : 0.25 }
            print("PASS VST3 \(name): main\(main), key\(auxiliary), \(channels) channel key, silence, bypass, disable")
        }
    }
    #endif

    static func run() throws {
        var tests: [(String, () throws -> Void)] = [
            ("strips-sparse", stripsAndSparseEffects), ("fanout", fanoutAdvancesOnce),
            ("sidechain-layouts", sidechainLayouts), ("pdc", alignedMainAndKeyImpulse), ("summed-keys", summedKeys),
            ("fixed-sends", fixedSendsAndAutomation), ("solo-key", soloKeepsKeyDependency),
            ("solo-bus-key", soloKeepsBusKeyAncestors), ("solo-early-sends", soloKeepsEarlySends),
            ("rollback", installRollback), ("capture-disconnected", capturesDisconnectedOwner)
        ]
        #if VST3_ROUTING_ADAPTER_TEST
        tests.append(("vst3-indexed", vst3IndexedAdapter))
        #endif
        let requested = CommandLine.arguments.dropFirst()
        let selected = requested.isEmpty ? tests : tests.filter { requested.contains($0.0) }
        try require(!selected.isEmpty, "Unknown test name")
        for (name, test) in selected {
            try autoreleasepool { try test() }
            print("PASS \(name)")
        }
        print("PASS \(selected.count) offline audio routing graph tests")
    }
    static func main() {
        setbuf(stdout, nil)
        do { try run() }
        catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
    }
}

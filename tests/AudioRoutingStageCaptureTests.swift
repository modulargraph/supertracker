// Standalone stage-capture acceptance. Original source/effect helpers copied from
// AudioRoutingGraphTests.swift; no installed vendor, device, MIDI, GUI or host state.
// This tests graph capture plumbing, not playable freeze or transport integration.
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

@main private struct AudioRoutingStageCaptureTests {
    typealias Key = AudioRoutingGraph.StageKey
    static let before = Key(owner: "track:a", stage: .preInserts)
    static let after = Key(owner: "track:a", stage: .preFader)
    static let post = Key(owner: "track:a", stage: .postFader)
    static let master = Key(owner: "master", stage: .postFader)
    static func signal(_ frame: Int64, _ channel: Int) -> Float {
        guard frame >= 0 else { return 0 }
        return ((frame / 37) % 2 == 0 ? 0.5 : 0.25) * (channel == 0 ? 1 : 0.5)
    }
    struct Setup {
        let fixture: GraphFixture
        let a: OriginalSource, b: OriginalSource
        let insert: EffectFixture, masterInsert: EffectFixture
        let prepared: AudioRoutingGraph.Prepared
    }
    static func setup(capture: Bool, muted: Bool = false, disconnected: Bool = false,
                      capacity: Int = 4096) throws -> Setup {
        let fixture = try GraphFixture(), a = try OriginalSource { frame, channel in signal(frame - 29, channel) }
        let b = try OriginalSource { frame, _ in frame >= 11 ? 0.5 : 0 }
        let insert = try EffectFixture(gain: 2, delay: 17), masterInsert = try EffectFixture(gain: 2, delay: 7)
        let model = try definition(tracks: [["routingID": "a", "volume": 0.5, "pan": muted ? -0.4 : 0.0, "mute": muted], ["routingID": "b"]],
            buses: [returnBus()], outputs: ["track:b": NSNull(), "track:a": disconnected ? NSNull() : "master" as Any],
            sends: disconnected ? [] : [send("early", tap: "preInserts", gain: 0.25)], sidechains: [key()])
        let prepared = try fixture.routing.prepare(definition: model, sources: ["track:a": a.node, "track:b": b.node],
            effects: [insert.effect("keyed"), masterInsert.effect("master-insert", owner: "master")],
            sourceLatencies: ["track:a": 29, "track:b": 11], captureOwners: capture && !disconnected ? ["track:a", "master"] : [],
            captureStages: capture ? (disconnected ? [before, after] : [before, after, post, master]) : [],
            captureCapacityFrames: capacity)
        try fixture.routing.install(prepared)
        return Setup(fixture: fixture, a: a, b: b, insert: insert, masterInsert: masterInsert, prepared: prepared)
    }
    static func render(_ setup: Setup, frames: Int, limit: Int = 1024,
                       drain: Bool = true) throws -> (StereoSamples, [Key: StereoSamples]) {
        let fixture = setup.fixture, captures = setup.prepared.stageCaptures
        try setup.prepared.configureCaptures(frameLimit: Int64(limit))
        let chunkBuffer = AVAudioPCMBuffer(pcmFormat: routingTestFormat, frameCapacity: 257)!
        var collected = Dictionary(uniqueKeysWithValues: captures.keys.map { ($0, StereoSamples()) })
        var output = StereoSamples(), retries = 0, blockIndex = 0
        let chunks = [17, 127, 64, 251, 53]
        try fixture.engine.start()
        defer { fixture.stop() }
        while output.left.count < frames {
            let count = min(chunks[blockIndex % chunks.count], frames - output.left.count)
            let start = Int64(fixture.engine.manualRenderingSampleTime)
            for capture in captures.values { try capture.beginRenderQuantum(sampleTime: start, frameCount: count) }
            let status: AVAudioEngineManualRenderingStatus
            do { status = try fixture.engine.renderOffline(AVAudioFrameCount(count), to: fixture.buffer) }
            catch { for capture in captures.values { capture.endRenderQuantum() }; throw error }
            for capture in captures.values { capture.endRenderQuantum() }
            if drain {
                for (key, capture) in captures {
                    if let error = capture.error { throw error }
                    while capture.availableFrames > 0 {
                        let chunk = try capture.drain(into: chunkBuffer)
                        try require(chunk.startFrame == Int64(collected[key]!.left.count), "\(key.id) lost its contiguous capture position")
                        collected[key]!.append(chunkBuffer)
                    }
                }
            }
            if status == .success {
                try require(fixture.buffer.frameLength == count, "Successful offline block had the wrong frame count")
                output.append(fixture.buffer); blockIndex += 1; retries = 0
            } else if (status == .cannotDoInCurrentContext || status == .insufficientDataFromInputNode) && retries < 20 { retries += 1 }
            else { throw TestFailure(message: "Offline stage capture failed: \(status.rawValue)") }
        }
        return (output, collected)
    }
    static func equal(_ a: StereoSamples, _ b: StereoSamples, _ message: String) throws {
        try require(a.left.count == b.left.count && a.right.count == b.right.count, message + " length")
        try a.matches(message) { frame, channel in channel == 0 ? b.left[frame] : b.right[frame] }
    }
    static func unchangedOutputAndExactStages() throws {
        let plain = try setup(capture: false), baseline = try render(plain, frames: 1100).0
        let captured = try setup(capture: true), result = try render(captured, frames: 1100)
        try equal(baseline, result.0, "Adding stage captures changed the full graph's master PCM")
        try require(captured.prepared.stageCaptures.count == 4 && captured.prepared.captures.count == 2,
                    "Owner aliases and StageKeys created duplicate capture processors")
        try require(captured.prepared.captures["track:a"] === captured.prepared.stageCaptures[post] && captured.prepared.captures["master"] === captured.prepared.stageCaptures[master], "Post-fader aliases must reference the exact same capture")
        let arrivals = captured.prepared.stageCaptureArrivalFrames
        try require(arrivals[before] == 29 && arrivals[after] == 46 && arrivals[post] == 46 && arrivals[master] == 53, "Stage PDC arrival was taken from the wrong boundary")
        for key in [before, after, post, master] { try require(result.1[key]!.left.count == 1024, "Stage capture failed its exact frame limit") }
        try result.1[before]!.matches("pre-insert capture after input summing") { frame, channel in signal(Int64(frame), channel) }
        try result.1[after]!.matches("pre-fader capture includes delayed keyed insert once") { frame, channel in signal(Int64(frame), channel) * 0.25 }
        try result.1[post]!.matches("post-fader alias stays after strip gain") { frame, channel in signal(Int64(frame), channel) * 0.125 }
        try result.1[master]!.matches("early send and keyed track align through master insert") { frame, channel in signal(Int64(frame), channel) * 0.75 }
        try require(captured.insert.unit.keyPulls > 0 && captured.a.probe.overlappingFrames == 0 && captured.b.probe.overlappingFrames == 0 && captured.insert.unit.probe.overlappingFrames == 0,
                    "Stage capture changed detector routing or independently pulled a source/insert")
    }
    static func faderIndependence() throws {
        let plain = try setup(capture: false, muted: true), baseline = try render(plain, frames: 1100).0
        let captured = try setup(capture: true, muted: true), result = try render(captured, frames: 1100)
        try equal(baseline, result.0, "Stage capture changed muted/panned graph output")
        try result.1[before]!.matches("pre-insert capture remains before mute/pan") { frame, channel in signal(Int64(frame), channel) }
        try result.1[after]!.matches("pre-fader capture remains before mute/pan") { frame, channel in signal(Int64(frame), channel) * 0.25 }
        try result.1[post]!.matches("muted post-fader remains silent") { _, _ in 0 }
        try result.1[master]!.matches("muting track preserves its independent early send") { frame, channel in signal(Int64(frame), channel) * 0.5 }
    }
    static func disconnectedStages() throws {
        let fixture = try setup(capture: true, disconnected: true), result = try render(fixture, frames: 1100)
        try result.0.matches("Disconnected captured stages never become audible") { _, _ in 0 }
        try result.1[before]!.matches("disconnected pre-insert stage") { frame, channel in signal(Int64(frame), channel) }
        try result.1[after]!.matches("disconnected pre-fader stage") { frame, channel in signal(Int64(frame), channel) * 0.25 }
        try require(fixture.a.probe.overlappingFrames == 0 && fixture.b.probe.overlappingFrames == 0 && fixture.insert.unit.probe.overlappingFrames == 0,
                    "Two silent anchors duplicated a shared dependency's render")
    }
    static func boundedOverflow() throws {
        let fixture = try setup(capture: true, capacity: 8)
        _ = try render(fixture, frames: 256, limit: 1024, drain: false)
        var overflows = 0
        for capture in fixture.prepared.stageCaptures.values {
            if case .overflow(let capacity, _, _)? = capture.error { try require(capacity == 8, "Overflow reported wrong bounded capacity"); overflows += 1 }
        }
        try require(overflows > 0, "Undrained small stage rings did not expose overflow")
    }
    static func rejectionAndRollback() throws {
        let fixture = try setup(capture: false), graph = fixture.fixture.routing, engine = fixture.fixture.engine
        try graph.commit()
        let previous = graph.current!, original = physicalGraph(engine)
        let sources = ["track:a": fixture.a.node, "track:b": fixture.b.node]
        let effects = [fixture.insert.effect("keyed"), fixture.masterInsert.effect("master-insert", owner: "master")]
        for stage in [Key(owner: "track:missing", stage: .preFader)] {
            do { _ = try graph.prepare(definition: previous.definition, sources: sources, effects: effects, captureStages: [stage]); throw TestFailure(message: "Unavailable capture owner was accepted") }
            catch is AudioRoutingGraph.GraphError {}
            try require(physicalGraph(engine) == original && graph.current === previous, "Invalid preparation mutated the attached graph")
        }
        let candidate = try graph.prepare(definition: previous.definition, sources: sources, effects: effects,
            sourceLatencies: ["track:a": 29, "track:b": 11], captureStages: [before, after])
        do { try graph.install(candidate, validate: { throw TestFailure(message: "injected validation failure") }); throw TestFailure(message: "Validation unexpectedly succeeded") }
        catch let error as TestFailure { try require(error.message == "injected validation failure", "Wrong validation result") }
        try require(graph.current === previous && physicalGraph(engine) == original, "Stage-capture rollback lost connections or attached helper nodes")
        let result = try render(fixture, frames: 1100).0
        try result.matches("original graph after stage-capture rollback") { frame, channel in signal(Int64(frame) - 53, channel) * 0.75 }
    }
    static func legacyAliases() throws {
        let fixture = try GraphFixture(), a = try OriginalSource(left: 0.5, right: 0.25), b = try OriginalSource(left: 0, right: 0)
        let legacy = try AudioRouting.parse(project: ["tracks": [["routingID": "a"], ["routingID": "b"]], "routing": ["version": 1, "mixModel": "legacy", "buses": [], "outputs": [:], "sends": [], "sidechains": []]])
        for stage in [before, after] {
            do { _ = try fixture.routing.prepare(definition: legacy, sources: ["track:a": a.node, "track:b": b.node], effects: [], captureStages: [stage]); throw TestFailure(message: "Legacy pre-fader capture was accepted") }
            catch is AudioRoutingGraph.GraphError {}
        }
        let allowed = try fixture.routing.prepare(definition: legacy, sources: ["track:a": a.node, "track:b": b.node], effects: [], captureOwners: ["track:a"])
        try require(allowed.captures["track:a"] === allowed.stageCaptures[post] && allowed.captureArrivalFrames["track:a"] == 0,
                    "Legacy post-fader capture compatibility changed")
    }
    static func main() {
        do { try run() }
        catch { fputs("FAIL stage capture: \(error.localizedDescription)\n", stderr); exit(1) }
    }
    static func run() throws {
        let groups: [(String, () throws -> Void)] = [
            ("unchanged master, exact boundaries, sparse physical latency and sidechain/send integrity", unchangedOutputAndExactStages),
            ("pre-fader/pre-insert independence from strip mute and pan", faderIndependence),
            ("disconnected stages use silent anchors with no duplicate source advancement", disconnectedStages),
            ("bounded stage rings expose overflow", boundedOverflow),
            ("invalid preparation and failed install preserve the prior physical graph", rejectionAndRollback),
            ("legacy owner capture aliases retain behavior and pre-fader requests reject", legacyAliases)]
        for (name, test) in groups { try test(); print("PASS stage capture: " + name) }
        print("\(groups.count) original offline stage capture groups passed")
    }
}

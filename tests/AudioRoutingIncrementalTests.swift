// Incremental routing: owners added or removed while the graph renders, compared
// against a full rebuild of the same definition. Manual offline rendering only.
import Foundation
import AVFoundation
import AudioToolbox

private let rate = 48_000.0
private let format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 2)!
private struct Failure: LocalizedError { let message: String; var errorDescription: String? { message } }
private func require(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
    guard try condition() else { throw Failure(message: message) }
}

/// A constant (DC) stereo source per track, so outputs are time-invariant sums.
private func source(_ value: Float) throws -> AVAudioSourceNode {
    let node = AVAudioSourceNode(format: format) { silent, _, count, list in
        for buffer in UnsafeMutableAudioBufferListPointer(list) {
            guard let output = buffer.mData?.assumingMemoryBound(to: Float.self) else { return kAudio_ParamError }
            for frame in 0..<Int(count) { output[frame] = value }
        }
        silent.pointee = false
        return noErr
    }
    try node.auAudioUnit.outputBusses[0].setFormat(format)
    return node
}

private final class Fixture {
    let engine = AVAudioEngine()
    let routing: AudioRoutingGraph
    let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 512)!
    var sources: [String: AVAudioNode] = [:]
    init() throws {
        try engine.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 512)
        routing = try AudioRoutingGraph(engine: engine, sampleRate: rate, maximumDelayFrames: 4096)
    }
    deinit { engine.stop() }
    func sources(for definition: AudioRouting.Definition, values: [String: Float]) throws -> [String: AVAudioNode] {
        for owner in definition.owners where owner.trackIndex != nil && sources[owner.id] == nil {
            sources[owner.id] = try source(values[owner.id] ?? 0)
        }
        return sources.filter { key, _ in definition.owners.contains { $0.id == key && $0.trackIndex != nil } }
    }
    /// Steady-state output level (left, right) after the mixers' parameter ramps.
    func level() throws -> (Float, Float) {
        if !engine.isRunning { try engine.start() }
        var left: Float = 0, right: Float = 0
        for _ in 0..<40 {
            guard try engine.renderOffline(512, to: buffer) == .success, let data = buffer.floatChannelData else { throw Failure(message: "Offline render failed") }
            left = data[0][511]; right = data[1][511]
        }
        return (left, right)
    }
}

private func definition(_ tracks: [(id: String, output: String)], buses: [String] = ["g"]) throws -> AudioRouting.Definition {
    var outputs: [String: Any] = [:]
    for track in tracks { outputs["track:" + track.id] = track.output }
    return try AudioRouting.parse(project: [
        "tracks": tracks.map { ["routingID": $0.id, "volume": 1.0] },
        "masterVolume": 1.0,
        "routing": ["version": 1, "mixModel": "standard", "buses": buses.map { ["id": $0, "name": $0, "type": "group", "volume": 1.0] },
                    "outputs": outputs, "sends": [], "sidechains": []]])
}

private func reference(_ definition: AudioRouting.Definition, values: [String: Float]) throws -> (Float, Float) {
    let fixture = try Fixture()
    let prepared = try fixture.routing.prepare(definition: definition, sources: fixture.sources(for: definition, values: values), effects: [])
    try fixture.routing.install(prepared); try fixture.routing.commit()
    return try fixture.level()
}

private func close(_ a: (Float, Float), _ b: (Float, Float)) -> Bool { abs(a.0 - b.0) < 1e-5 && abs(a.1 - b.1) < 1e-5 }

@main enum AudioRoutingIncrementalTests {
    static func main() throws {
        var groups = 0
        let values: [String: Float] = ["track:a": 0.1, "track:b": 0.2, "track:c": 0.05, "track:d": 0.025]
        let fixture = try Fixture()
        let start = try definition([("a", "master"), ("b", "bus:g")])
        try fixture.routing.install(try fixture.routing.prepare(definition: start, sources: fixture.sources(for: start, values: values), effects: []))
        try fixture.routing.commit()
        let before = try fixture.level()
        try require(close(before, try reference(start, values: values)), "The installed graph matches its reference")
        let strip = fixture.routing.current?.strips["track:a"]

        // Two tracks join while rendering: one to master, one to the existing group.
        let grown = try definition([("a", "master"), ("b", "bus:g"), ("c", "master"), ("d", "bus:g")])
        guard let change = try fixture.routing.prepareIncremental(definition: grown, sources: fixture.sources(for: grown, values: values), effects: []) else {
            throw Failure(message: "Adding tracks with their own routes must be incremental")
        }
        try require(change.addedOwners.sorted() == ["track:c", "track:d"] && change.removedOwners.isEmpty, "The change names the added tracks")
        try require(fixture.engine.isRunning, "The fixture renders while the change is prepared")
        try fixture.routing.installIncremental(change)
        try require(fixture.engine.isRunning, "Installing the change keeps the engine running")
        try require(fixture.routing.current?.strips["track:a"] === strip, "Existing strips are reused, not rebuilt")
        try require(close(try fixture.level(), try reference(grown, values: values)), "The grown graph sounds like a full rebuild")
        groups += 1; print("PASS tracks added while rendering join master and an existing group")

        // A track leaves; the rest keeps playing unchanged.
        let shrunk = try definition([("b", "bus:g"), ("c", "master"), ("d", "bus:g")])
        guard let removal = try fixture.routing.prepareIncremental(definition: shrunk, sources: fixture.sources(for: shrunk, values: values), effects: []) else {
            throw Failure(message: "Removing a track must be incremental")
        }
        try fixture.routing.installIncremental(removal)
        try require(fixture.engine.isRunning && removal.removedOwners == ["track:a"], "Removal keeps rendering")
        try require(close(try fixture.level(), try reference(shrunk, values: values)), "The shrunk graph sounds like a full rebuild")
        groups += 1; print("PASS a removed track disconnects without stopping")

        // A new bus with a new track routed to it.
        var withE = values; withE["track:e"] = 0.3
        let bussed = try definition([("b", "bus:g"), ("c", "master"), ("d", "bus:g"), ("e", "bus:h")], buses: ["g", "h"])
        guard let busChange = try fixture.routing.prepareIncremental(definition: bussed, sources: fixture.sources(for: bussed, values: withE), effects: []) else {
            throw Failure(message: "A new bus with its new track must be incremental")
        }
        try fixture.routing.installIncremental(busChange)
        try require(close(try fixture.level(), try reference(bussed, values: withE)), "The new bus sounds like a full rebuild")
        groups += 1; print("PASS a new group bus and its track join while rendering")

        // Changing an existing track's route is a full, stopped rebuild.
        let moved = try definition([("b", "bus:g"), ("c", "bus:h"), ("d", "bus:g"), ("e", "bus:h")], buses: ["g", "h"])
        try require(try fixture.routing.prepareIncremental(definition: moved, sources: fixture.sources(for: moved, values: withE), effects: []) == nil,
                    "Moving an existing track to another bus needs a full rebuild")
        // Removing a bus that still has tracks routed to it changes those tracks' routes too.
        let unbussed = try definition([("b", "bus:g"), ("c", "master"), ("d", "bus:g"), ("e", "master")], buses: ["g"])
        try require(try fixture.routing.prepareIncremental(definition: unbussed, sources: fixture.sources(for: unbussed, values: withE), effects: []) == nil,
                    "Rerouting a track off a removed bus needs a full rebuild")
        groups += 1; print("PASS changed routes of existing owners fall back to a full rebuild")

        // A full rebuild after incremental changes still works and detaches retired helpers.
        fixture.engine.stop()
        let attachedBefore = fixture.engine.attachedNodes.count
        try fixture.routing.install(try fixture.routing.prepare(definition: unbussed, sources: fixture.sources(for: unbussed, values: withE), effects: []))
        try fixture.routing.commit()
        try require(fixture.engine.attachedNodes.count < attachedBefore, "Retired and previous helpers are detached by the next quiescent commit")
        try require(close(try fixture.level(), try reference(unbussed, values: withE)), "The rebuilt graph sounds right")
        groups += 1; print("PASS a later full rebuild replaces the incrementally grown graph")

        // Channels follow track order, as in the app: removing the first track moves every
        // later track to the previous channel's source. Strips stay, sources are rewired live.
        let channels = try (0..<4).map { try source(Float($0 + 1) * 0.01) }
        func channelSources(_ definition: AudioRouting.Definition) -> [String: AVAudioNode] {
            Dictionary(uniqueKeysWithValues: definition.owners.compactMap { owner in owner.trackIndex.map { (owner.id, channels[$0] as AVAudioNode) } })
        }
        let shifted = try Fixture()
        let four = try definition([("p", "master"), ("q", "bus:g"), ("r", "master"), ("s", "bus:g")])
        try shifted.routing.install(try shifted.routing.prepare(definition: four, sources: channelSources(four), effects: []))
        try shifted.routing.commit(); _ = try shifted.level()
        let three = try definition([("q", "bus:g"), ("r", "master"), ("s", "bus:g")])
        let qStrip = shifted.routing.current?.strips["track:q"]
        guard let shift = try shifted.routing.prepareIncremental(definition: three, sources: channelSources(three), effects: []) else {
            throw Failure(message: "Removing the first track must be incremental")
        }
        try require(shift.rewiredOwners.sorted() == ["track:q", "track:r", "track:s"] && shift.removedOwners == ["track:p"], "Later tracks are rewired to their new channels")
        try shifted.routing.installIncremental(shift)
        try require(shifted.engine.isRunning && shifted.routing.current?.strips["track:q"]?.input === qStrip?.input, "Rewired tracks keep their strips")
        // Reference: channels 0..2 feed q, r, s; channel 3 is silent.
        let referenceFixture = try Fixture()
        let referenceChannels = try (0..<3).map { try source(Float($0 + 1) * 0.01) }
        let referenceSources = Dictionary(uniqueKeysWithValues: three.owners.compactMap { owner in owner.trackIndex.map { (owner.id, referenceChannels[$0] as AVAudioNode) } })
        try referenceFixture.routing.install(try referenceFixture.routing.prepare(definition: three, sources: referenceSources, effects: []))
        try referenceFixture.routing.commit()
        try require(close(try shifted.level(), try referenceFixture.level()), "Shifted channels sound like a full rebuild")
        groups += 1; print("PASS removing the first track rewires later tracks to their channels without stopping")

        // The same with input stages collapsed onto the sources (as the app assembles routing):
        // rewired tracks reconnect their fader to the new channel source while rendering.
        let direct = try Fixture()
        direct.routing.collapsesTrackInputs = true
        let directChannels = try (0..<4).map { try source(Float($0 + 1) * 0.01) }
        func directSources(_ definition: AudioRouting.Definition) -> [String: AVAudioNode] {
            Dictionary(uniqueKeysWithValues: definition.owners.compactMap { owner in owner.trackIndex.map { (owner.id, directChannels[$0] as AVAudioNode) } })
        }
        try direct.routing.install(try direct.routing.prepare(definition: four, sources: directSources(four), effects: []))
        try direct.routing.commit(); _ = try direct.level()
        try require(direct.routing.current?.strips["track:q"]?.input === directChannels[1], "A track's input stage is its source")
        let mixers = direct.engine.attachedNodes.filter { $0 is AVAudioMixerNode }.count
        try require(mixers < shifted.engine.attachedNodes.filter { $0 is AVAudioMixerNode }.count, "Collapsed inputs use fewer mixers")
        guard let directShift = try direct.routing.prepareIncremental(definition: three, sources: directSources(three), effects: []) else {
            throw Failure(message: "Removing the first track must stay incremental with direct sources: \(direct.routing.lastIncrementalRefusal ?? "")")
        }
        try require(directShift.rewiredOwners.sorted() == ["track:q", "track:r", "track:s"], "Later direct tracks are rewired")
        try direct.routing.installIncremental(directShift)
        try require(direct.engine.isRunning && direct.routing.current?.strips["track:q"]?.input === directChannels[0], "Rewired direct tracks take their new channel")
        try require(close(try direct.level(), try referenceFixture.level()), "Shifted direct channels sound like a full rebuild")
        // Adding tracks to a collapsed graph stays incremental too.
        let directGrown = try definition([("q", "bus:g"), ("r", "master"), ("s", "bus:g"), ("t", "master")])
        guard let directAdd = try direct.routing.prepareIncremental(definition: directGrown, sources: directSources(directGrown), effects: []) else {
            throw Failure(message: "Adding a direct track must be incremental")
        }
        try direct.routing.installIncremental(directAdd)
        let directReference = try Fixture()
        directReference.routing.collapsesTrackInputs = true
        let directReferenceChannels = try (0..<4).map { try source(Float($0 + 1) * 0.01) }
        try directReference.routing.install(try directReference.routing.prepare(definition: directGrown, sources: Dictionary(uniqueKeysWithValues: directGrown.owners.compactMap { owner in owner.trackIndex.map { (owner.id, directReferenceChannels[$0] as AVAudioNode) } }), effects: []))
        try directReference.routing.commit()
        try require(close(try direct.level(), try directReference.level()), "A direct track added live sounds like a full rebuild")
        groups += 1; print("PASS direct-source strips rewire and grow live")

        // Kits add many tracks at once: 64 tracks to a group, timed against a full rebuild.
        let large = try Fixture()
        let base = (0..<16).map { (id: "t\($0)", output: "bus:g") }
        let baseDefinition = try definition(base)
        var many = values
        for index in 0..<80 { many["track:t\(index)"] = 0.001 }
        try large.routing.install(try large.routing.prepare(definition: baseDefinition, sources: large.sources(for: baseDefinition, values: many), effects: []))
        try large.routing.commit(); _ = try large.level()
        let grownDefinition = try definition((0..<80).map { (id: "t\($0)", output: "bus:g") })
        let sources = try large.sources(for: grownDefinition, values: many)
        let incrementalStart = Date()
        guard let kit = try large.routing.prepareIncremental(definition: grownDefinition, sources: sources, effects: []) else {
            throw Failure(message: "Sixty-four added tracks must be incremental")
        }
        try large.routing.installIncremental(kit)
        let incremental = Date().timeIntervalSince(incrementalStart)
        try require(large.engine.isRunning, "The engine kept rendering through the kit-sized change")
        let level = try large.level()
        large.engine.stop()
        let fullStart = Date()
        try large.routing.install(try large.routing.prepare(definition: grownDefinition, sources: sources, effects: [])); try large.routing.commit()
        let full = Date().timeIntervalSince(fullStart)
        try require(close(level, try large.level()), "Incremental and full graphs of 80 tracks sound the same")
        print(String(format: "PASS 64 tracks added live in %.3f s (full rebuild of 80 tracks: %.3f s)", incremental, full))
        groups += 1

        // Loading a kit: a new group bus and its sixteen new member tracks join while rendering.
        let kitFixture = try Fixture()
        let playing = (0..<4).map { (id: "p\($0)", output: "master") }
        var kitValues: [String: Float] = [:]
        for index in 0..<4 { kitValues["track:p\(index)"] = 0.01 }
        for pad in 0..<16 { kitValues["track:pad\(pad)"] = Float(pad + 1) * 0.001 }
        let before4 = try definition(playing, buses: [])
        try kitFixture.routing.install(try kitFixture.routing.prepare(definition: before4, sources: kitFixture.sources(for: before4, values: kitValues), effects: []))
        try kitFixture.routing.commit(); _ = try kitFixture.level()
        let withKit = try definition(playing + (0..<16).map { (id: "pad\($0)", output: "bus:kit") }, buses: ["kit"])
        guard let kitChange = try kitFixture.routing.prepareIncremental(definition: withKit, sources: kitFixture.sources(for: withKit, values: kitValues), effects: []) else {
            throw Failure(message: "A kit (new group bus and its tracks) must be incremental")
        }
        try kitFixture.routing.installIncremental(kitChange)
        try require(kitFixture.engine.isRunning, "The engine kept rendering while the kit joined")
        try require(close(try kitFixture.level(), try reference(withKit, values: kitValues)), "The kit's bus and tracks sound like a full rebuild")
        groups += 1; print("PASS a kit's new group bus and sixteen tracks join while rendering")

        // As in the app: every track also sends to the fixed delay and reverb returns, so a
        // new member strip joins the live graph through its sends before its bus joins master.
        let returnsFixture = try Fixture()
        func processor() throws -> AVAudioNode {
            let node = AVAudioUnitEQ(numberOfBands: 1)
            try node.auAudioUnit.inputBusses[0].setFormat(format); try node.auAudioUnit.outputBusses[0].setFormat(format)
            return node
        }
        let processors = [try processor(), try processor()]
        func returns(_ definition: AudioRouting.Definition, _ nodes: [AVAudioNode]? = nil) -> [AudioRoutingGraph.SystemReturn] {
            let ids = definition.owners.filter { $0.trackIndex != nil }.map(\.id)
            return zip(["delay", "reverb"], nodes ?? processors).map { id, node in
                AudioRoutingGraph.SystemReturn(id: id, processor: node, gains: Dictionary(uniqueKeysWithValues: ids.map { ($0, Float(0.5)) }), returnGain: 1)
            }
        }
        let metronome = [AudioRoutingGraph.Source(id: "metronome", node: try source(0))]
        try returnsFixture.routing.install(try returnsFixture.routing.prepare(definition: before4, sources: returnsFixture.sources(for: before4, values: kitValues), effects: [], systemReturns: returns(before4), extraMasterSources: metronome))
        try returnsFixture.routing.commit(); _ = try returnsFixture.level()
        // As in the app, each new track's source is its channel's combined mixer, already attached
        // and fed by the channel's render source before the track's strip exists.
        var kitSources = try returnsFixture.sources(for: before4, values: kitValues)
        for pad in 0..<16 {
            let render = try source(kitValues["track:pad\(pad)"]!), combined = try processor()
            returnsFixture.engine.attach(render); returnsFixture.engine.attach(combined)
            returnsFixture.engine.connect(render, to: combined, format: format)
            kitSources["track:pad\(pad)"] = combined
        }
        guard let returnsChange = try returnsFixture.routing.prepareIncremental(definition: withKit, sources: kitSources, effects: [], systemReturns: returns(withKit), extraMasterSources: metronome) else {
            throw Failure(message: "A kit with system returns must be incremental")
        }
        try returnsFixture.routing.installIncremental(returnsChange)
        let liveLevel = try returnsFixture.level()
        let referenceReturns = try Fixture()
        let referenceProcessors = [try processor(), try processor()]
        let ids = withKit.owners.filter { $0.trackIndex != nil }.map(\.id)
        let referenceList = zip(["delay", "reverb"], referenceProcessors).map { id, node in
            AudioRoutingGraph.SystemReturn(id: id, processor: node, gains: Dictionary(uniqueKeysWithValues: ids.map { ($0, Float(0.5)) }), returnGain: 1)
        }
        try referenceReturns.routing.install(try referenceReturns.routing.prepare(definition: withKit, sources: referenceReturns.sources(for: withKit, values: kitValues), effects: [], systemReturns: referenceList, extraMasterSources: [AudioRoutingGraph.Source(id: "metronome", node: try source(0))]))
        try referenceReturns.routing.commit()
        try require(returnsFixture.engine.isRunning && close(liveLevel, try referenceReturns.level()), "A kit joining with delay/reverb sends sounds like a full rebuild: \(liveLevel)")
        groups += 1; print("PASS a kit's bus and tracks join a graph with delay and reverb returns while rendering")
        // Sixteen new tracks join an existing group bus while rendering, with returns.
        let existingBus = try Fixture()
        let existingProcessors = [try processor(), try processor()]
        let busFirst = try definition(playing + [(id: "pad0", output: "bus:kit")], buses: ["kit"])
        try existingBus.routing.install(try existingBus.routing.prepare(definition: busFirst, sources: existingBus.sources(for: busFirst, values: kitValues), effects: [], systemReturns: returns(busFirst, existingProcessors)))
        try existingBus.routing.commit(); _ = try existingBus.level()
        guard let joinBus = try existingBus.routing.prepareIncremental(definition: withKit, sources: existingBus.sources(for: withKit, values: kitValues), effects: [], systemReturns: returns(withKit, existingProcessors)) else {
            throw Failure(message: "Tracks joining an existing group must be incremental")
        }
        try existingBus.routing.installIncremental(joinBus)
        let joinedLevel = try existingBus.level()
        let joinedReference = try Fixture()
        let joinedProcessors = [try processor(), try processor()]
        let joinedReturns = zip(["delay", "reverb"], joinedProcessors).map { id, node in
            AudioRoutingGraph.SystemReturn(id: id, processor: node, gains: Dictionary(uniqueKeysWithValues: ids.map { ($0, Float(0.5)) }), returnGain: 1)
        }
        try joinedReference.routing.install(try joinedReference.routing.prepare(definition: withKit, sources: joinedReference.sources(for: withKit, values: kitValues), effects: [], systemReturns: joinedReturns))
        try joinedReference.routing.commit()
        try require(close(joinedLevel, try joinedReference.level()), "Fifteen tracks joining an existing group with returns sound like a full rebuild: \(joinedLevel)")
        groups += 1; print("PASS fifteen tracks join an existing group bus with delay and reverb sends while rendering")
        // Delay/reverb send routes exist only for tracks whose send can be audible
        // (SystemReturn.senders). A kept track gaining a send joins the running returns live;
        // a return's first send needs a full rebuild (AVAudioEngine does not render an input
        // added later to a chain that joined empty).
        let sendFixture = try Fixture()
        let sendProcessors = [try processor(), try processor()]
        func partial(_ definition: AudioRouting.Definition, senders: Set<String>, _ nodes: [AVAudioNode]) -> [AudioRoutingGraph.SystemReturn] {
            let ids = definition.owners.filter { $0.trackIndex != nil }.map(\.id)
            return zip(["delay", "reverb"], nodes).map { id, node in
                AudioRoutingGraph.SystemReturn(id: id, processor: node, gains: Dictionary(uniqueKeysWithValues: ids.map { ($0, Float(0.5)) }), returnGain: 1, senders: senders)
            }
        }
        let sendStart: Set<String> = ["track:p0"]
        try sendFixture.routing.install(try sendFixture.routing.prepare(definition: withKit, sources: sendFixture.sources(for: withKit, values: kitValues), effects: [], systemReturns: partial(withKit, senders: sendStart, sendProcessors)))
        try sendFixture.routing.commit(); _ = try sendFixture.level()
        let sendStrip = sendFixture.routing.current?.strips["track:pad3"]
        let sendGrown: Set<String> = ["track:p0", "track:pad3", "track:p2"]
        guard let sendChange = try sendFixture.routing.prepareIncremental(definition: withKit, sources: sendFixture.sources(for: withKit, values: kitValues), effects: [],
                systemReturns: partial(withKit, senders: sendGrown, sendProcessors)) else {
            throw Failure(message: "Kept tracks gaining delay/reverb sends must be incremental: \(sendFixture.routing.lastIncrementalRefusal ?? "")")
        }
        try sendFixture.routing.installIncremental(sendChange)
        try require(sendFixture.engine.isRunning && sendFixture.routing.current?.strips["track:pad3"] === sendStrip, "Sends join without rebuilding strips")
        let sendLevel = try sendFixture.level()
        let sendReference = try Fixture()
        try sendReference.routing.install(try sendReference.routing.prepare(definition: withKit, sources: sendReference.sources(for: withKit, values: kitValues), effects: [],
            systemReturns: partial(withKit, senders: sendGrown, [try processor(), try processor()])))
        try sendReference.routing.commit()
        try require(close(sendLevel, try sendReference.level()), "Sends added live sound like a full rebuild: \(sendLevel)")
        // Fewer send routes than tracks: zero sends are not routed at all.
        try require((sendFixture.routing.current?.builtinSendEndpoints.count ?? 0) == sendGrown.count, "Only sending tracks have send routes")
        // A return without any send gains its first one only through a full rebuild.
        let emptyFixture = try Fixture()
        let emptyProcessors = [try processor(), try processor()]
        try emptyFixture.routing.install(try emptyFixture.routing.prepare(definition: withKit, sources: emptyFixture.sources(for: withKit, values: kitValues), effects: [],
            systemReturns: partial(withKit, senders: [], emptyProcessors)))
        try emptyFixture.routing.commit(); _ = try emptyFixture.level()
        try require(try emptyFixture.routing.prepareIncremental(definition: withKit, sources: emptyFixture.sources(for: withKit, values: kitValues), effects: [],
            systemReturns: partial(withKit, senders: ["track:pad1"], emptyProcessors)) == nil &&
            emptyFixture.routing.lastIncrementalRefusal == "a delay/reverb return gains its first send", "A return's first send needs a full rebuild")
        groups += 1; print("PASS delay/reverb sends of kept tracks join the running returns; a return's first send rebuilds")
        print("Incremental routing: \(groups) groups passed")
    }
}

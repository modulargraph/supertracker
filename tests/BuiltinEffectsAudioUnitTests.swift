import Foundation
import AVFoundation
import AudioToolbox
import TrackerEngine

enum TrackerError: LocalizedError { case message(String); var errorDescription: String? { if case .message(let s) = self { return s }; return nil } }
@main struct BuiltinEffectsAudioUnitTests {
    static var checks = 0
    static func check(_ condition: @autoclosure () -> Bool, _ message: String) throws {
        checks += 1
        if !condition() { throw TrackerError.message(message) }
    }
    @MainActor static func main() throws {
        let catalogue = BuiltinEffectsAudioUnit.catalogue()
        try check(catalogue.count == 34, "All 34 effect models must be registered")
        try check(Set(catalogue.compactMap { $0["id"] as? String }).count == catalogue.count, "Effect IDs must be unique")
        for sampleRate in [48_000.0, 96_000.0] {
            for item in catalogue {
                let id = item["id"] as! String
                let node = try BuiltinEffectsAudioUnit.makeNode(id: id, sampleRate: sampleRate)
                let unit = node.auAudioUnit as! BuiltinEffectsAudioUnit
                let format = AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: 2)!
                try unit.outputBusses[0].setFormat(format)
                for index in 0..<unit.inputBusses.count { try unit.inputBusses[index].setFormat(format) }
                try unit.allocateRenderResources()
                defer { unit.deallocateRenderResources() }
                let parameters = unit.parameterTree!.allParameters
                try check(!parameters.isEmpty, id + " parameters")
                // The rack's plugin.parameters (mixer inserts, plug-in pages, MK3 knobs) reads
                // exactly this tree, for track, bus, kit Sound/Group and Master slots alike.
                let info = st_builtin_effect_find(BuiltinEffectsAudioUnit.dspID(id))!.pointee
                try check(parameters.count == Int(info.parameterCount), id + " exposes every DSP parameter in its tree")
                try check(parameters.map(\.address) == (0..<UInt64(info.parameterCount)).map { $0 }, id + " parameter addresses are contiguous from 0")
                try check(Set(parameters.map(\.identifier)).count == parameters.count && parameters.allSatisfy { !$0.identifier.isEmpty && !$0.displayName.isEmpty }, id + " parameter identifiers and names")
                unit.synchronizeParameterValues()
                for parameter in parameters {
                    try check(parameter.flags.contains(.flag_IsReadable) && parameter.flags.contains(.flag_IsWritable), id + " " + parameter.identifier + " is readable and writable")
                    try check(parameter.minValue == 0 && parameter.maxValue == 1 && parameter.value.isFinite && (0...1).contains(parameter.value), id + " " + parameter.identifier + " normalized range")
                    try check(!parameter.string(fromValue: nil).isEmpty, id + " " + parameter.identifier + " display string")
                    try check(unit.parameterStepCount(address: parameter.address) != nil, id + " " + parameter.identifier + " step metadata")
                    try check(unit.parameterTree?.parameter(withAddress: parameter.address) === parameter, id + " " + parameter.identifier + " resolves by address")
                }
                let original = try unit.documentState()
                if let parameter = parameters.first {
                    parameter.value = 0.37
                    let saved = try unit.documentState()
                    parameter.value = 0.82
                    try unit.restoreDocumentState(saved)
                    let restored = try unit.documentState()
                    try check(NSDictionary(dictionary: saved).isEqual(to: restored), id + " parameter state roundtrip")
                    var invalid = saved; invalid["id"] = "invalid"
                    var rejected=false
                    do { try unit.restoreDocumentState(invalid) } catch { rejected=true }
                    let afterInvalid=try unit.documentState()
                    try check(rejected && NSDictionary(dictionary: restored).isEqual(to: afterInvalid), id + " rejected state is atomic")
                }
                try unit.restoreDocumentState(original)
                let input = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 512)!, output = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 512)!
                input.frameLength = 512; output.frameLength = 512
                let pull: AURenderPullInputBlock = { _, _, frames, bus, list in
                    if bus != 0 { return kAudioUnitErr_NoConnection }
                    let buffers = UnsafeMutableAudioBufferListPointer(list)
                    for channel in 0..<2 {
                        if let destination = buffers[channel].mData { memcpy(destination, input.floatChannelData![channel], Int(frames) * 4) }
                        else { buffers[channel].mData = UnsafeMutableRawPointer(input.floatChannelData![channel]) }
                        buffers[channel].mDataByteSize = frames * 4
                    }
                    return noErr
                }
                let render = unit.internalRenderBlock
                for block in 0..<8 {
                    for frame in 0..<512 {
                        input.floatChannelData![0][frame] = block == 0 && frame == 0 ? 0.5 : 0
                        input.floatChannelData![1][frame] = Float(sin(Double(block * 512 + frame) * 0.03) * 0.1)
                    }
                    var time = AudioTimeStamp(); time.mSampleTime = Double(block * 512); time.mFlags = .sampleTimeValid
                    var flags = AudioUnitRenderActionFlags()
                    let status = render(&flags, &time, 512, 0, output.mutableAudioBufferList, nil, pull)
                    try check(status == noErr, id + " offline render status")
                    for channel in 0..<2 { for frame in 0..<512 { try check(output.floatChannelData![channel][frame].isFinite, id + " nonfinite output") } }
                }
                // Bypassed (an imported Maschine module that was switched off): once the
                // crossfade is done the processor is skipped and the output is the
                // latency-aligned dry signal; engaging it again resumes processing.
                if unit.latency == 0 {
                    unit.shouldBypassEffect = true
                    for block in 0..<3 {
                        var time = AudioTimeStamp(); time.mSampleTime = Double((8 + block) * 512); time.mFlags = .sampleTimeValid
                        var flags = AudioUnitRenderActionFlags()
                        try check(render(&flags, &time, 512, 0, output.mutableAudioBufferList, nil, pull) == noErr, id + " bypassed render")
                    }
                    for channel in 0..<2 { for frame in 0..<512 {
                        try check(output.floatChannelData![channel][frame] == input.floatChannelData![channel][frame], id + " dormant bypass is exactly dry")
                    } }
                    unit.shouldBypassEffect = false
                    var time = AudioTimeStamp(); time.mSampleTime = Double(11 * 512); time.mFlags = .sampleTimeValid
                    var flags = AudioUnitRenderActionFlags()
                    try check(render(&flags, &time, 512, 0, output.mutableAudioBufferList, nil, pull) == noErr, id + " re-engaged render")
                    for channel in 0..<2 { for frame in 0..<512 { try check(output.floatChannelData![channel][frame].isFinite, id + " re-engaged output") } }
                }
                // Automation events are applied at the requested sample and a
                // ramp is continued across callbacks, independently of UI reads.
                if let parameter = parameters.first, unit.parameterStepCount(address: parameter.address) == 0 {
                    parameter.value = 0
                    var event = AURenderEvent()
                    event.parameter = AUParameterEvent(next: nil, eventSampleTime: 4_096, eventType: .parameterRamp,
                        reserved: (0,0,0), rampDurationSampleFrames: 1_024, parameterAddress: parameter.address, value: 1)
                    var time = AudioTimeStamp(); time.mSampleTime = 4_096; time.mFlags = .sampleTimeValid
                    var flags = AudioUnitRenderActionFlags()
                    try check(render(&flags, &time, 512, 0, output.mutableAudioBufferList, &event, pull) == noErr, id + " ramp start")
                    unit.synchronizeParameterValues()
                    let midpoint = parameter.value
                    try check(abs(midpoint - 0.5) < 0.002, id + " ramp midpoint across first block: \(midpoint), state=\(try! unit.documentState())")
                    time.mSampleTime += 512
                    try check(render(&flags, &time, 512, 0, output.mutableAudioBufferList, nil, pull) == noErr, id + " ramp continuation")
                    unit.synchronizeParameterValues()
                    try check(abs(parameter.value - 1) < 0.002, id + " ramp endpoint: \(parameter.value)")
                    parameter.value = 0
                    time.mSampleTime = 6_000; event.parameter.eventSampleTime = 6_000
                    try check(render(&flags, &time, 512, 0, output.mutableAudioBufferList, &event, pull) == noErr, id + " interrupted ramp start")
                    var restored = try unit.documentState()
                    var restoredParameters = restored["parameters"] as! [String: Double]
                    restoredParameters[parameter.identifier] = 0.23
                    restored["parameters"] = restoredParameters
                    try unit.restoreDocumentState(restored)
                    time.mSampleTime += 512
                    try check(render(&flags, &time, 512, 0, output.mutableAudioBufferList, nil, pull) == noErr, id + " restored ramp render")
                    unit.synchronizeParameterValues()
                    try check(abs(parameter.value - 0.23) < 0.002, id + " stale ramp overwrote restored preset")
                }
            }
        }
        try chainChecks(catalogue.compactMap { $0["id"] as? String })
        print("PASS BuiltinEffectsAudioUnit: \(checks) checks; 34 models, 48/96 kHz, state, PCM and scheduled ramps; insert chains")
    }

    // MARK: insert chains

    /// One unit rendered as its own node, fed by `pull`.
    static func makeUnit(_ id: String, sampleRate: Double) throws -> BuiltinEffectsAudioUnit {
        let node = try BuiltinEffectsAudioUnit.makeNode(id: id, sampleRate: sampleRate)
        let unit = node.auAudioUnit as! BuiltinEffectsAudioUnit
        let format = AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: 2)!
        try unit.outputBusses[0].setFormat(format)
        for index in 0..<unit.inputBusses.count { try unit.inputBusses[index].setFormat(format) }
        try unit.allocateRenderResources()
        retained.append(node)
        return unit
    }
    static var retained: [AnyObject] = []

    /// Per-node reference: each unit renders as its own node, pulling the previous one's output.
    final class NodeChain {
        var units: [BuiltinEffectsAudioUnit]
        let scratch: [AVAudioPCMBuffer]
        let format: AVAudioFormat
        init(_ units: [BuiltinEffectsAudioUnit], format: AVAudioFormat) {
            self.units = units; self.format = format
            scratch = (0..<64).map { _ in let b = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4096)!; b.frameLength = 4096; return b }
        }
        func render(input: AVAudioPCMBuffer, frames: AUAudioFrameCount, time: Double) throws -> [[Float]] {
            var source: AVAudioPCMBuffer = input
            for (index, unit) in units.enumerated() {
                let upstream = source, destination = scratch[index]
                let pull: AURenderPullInputBlock = { _, _, count, bus, list in
                    if bus != 0 { return kAudioUnitErr_NoConnection }
                    let buffers = UnsafeMutableAudioBufferListPointer(list)
                    for channel in 0..<2 {
                        if let target = buffers[channel].mData { memcpy(target, upstream.floatChannelData![channel], Int(count) * 4) }
                        else { buffers[channel].mData = UnsafeMutableRawPointer(upstream.floatChannelData![channel]) }
                        buffers[channel].mDataByteSize = count * 4
                    }
                    return noErr
                }
                var stamp = AudioTimeStamp(); stamp.mSampleTime = time; stamp.mFlags = .sampleTimeValid
                var flags = AudioUnitRenderActionFlags()
                guard unit.internalRenderBlock(&flags, &stamp, frames, 0, destination.mutableAudioBufferList, nil, pull) == noErr else {
                    throw TrackerError.message("per-node render failed")
                }
                source = destination
            }
            return (0..<2).map { channel in Array(UnsafeBufferPointer(start: source.floatChannelData![channel], count: Int(frames))) }
        }
    }

    static func renderChain(_ chain: BuiltinEffectsChain, input: AVAudioPCMBuffer, output: AVAudioPCMBuffer, frames: AUAudioFrameCount, time: Double) throws -> [[Float]] {
        let pull: AURenderPullInputBlock = { _, _, count, bus, list in
            if bus != 0 { return kAudioUnitErr_NoConnection }
            let buffers = UnsafeMutableAudioBufferListPointer(list)
            for channel in 0..<2 {
                if let target = buffers[channel].mData { memcpy(target, input.floatChannelData![channel], Int(count) * 4) }
                else { buffers[channel].mData = UnsafeMutableRawPointer(input.floatChannelData![channel]) }
                buffers[channel].mDataByteSize = count * 4
            }
            return noErr
        }
        var stamp = AudioTimeStamp(); stamp.mSampleTime = time; stamp.mFlags = .sampleTimeValid
        var flags = AudioUnitRenderActionFlags()
        guard chain.node.auAudioUnit.renderBlock(&flags, &stamp, frames, 0, output.mutableAudioBufferList, pull) == noErr else {
            throw TrackerError.message("chain render failed")
        }
        return (0..<2).map { channel in Array(UnsafeBufferPointer(start: output.floatChannelData![channel], count: Int(frames))) }
    }

    static func maxDifference(_ a: [[Float]], _ b: [[Float]]) -> Float {
        var worst: Float = 0
        for channel in 0..<2 { for frame in 0..<a[channel].count { worst = max(worst, abs(a[channel][frame] - b[channel][frame])) } }
        return worst
    }

    /// Chained processing equals per-node processing (output, automation, bypass, in-place
    /// reorder/add/remove, latency, state) for a chain of real built-in effects.
    @MainActor static func chainChecks(_ ids: [String]) throws {
        let sampleRate = 48_000.0
        let format = AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: 2)!
        // Every model, in groups of five, plus one long chain of twelve.
        var groups: [[String]] = stride(from: 0, to: ids.count, by: 5).map { Array(ids[$0..<min(ids.count, $0 + 5)]) }
        groups.append(Array(ids.shuffled().prefix(12)))
        var seed: UInt64 = 0x9E3779B97F4A7C15
        func random() -> Float { seed = seed &* 6364136223846793005 &+ 1442695040888963407; return Float(seed >> 40) / Float(1 << 24) }
        for (groupIndex, group) in groups.enumerated() {
            let reference = try group.map { try makeUnit($0, sampleRate: sampleRate) }
            let members = try group.map { try makeUnit($0, sampleRate: sampleRate) }
            // Identical, randomized (non-default) parameter settings for both copies.
            for (a, b) in zip(reference, members) {
                for parameter in a.parameterTree!.allParameters {
                    let value = AUValue(random())
                    parameter.value = value
                    b.parameterTree!.parameter(withAddress: parameter.address)!.value = value
                }
            }
            let chain = try BuiltinEffectsChain(sampleRate: sampleRate)
            try chain.node.auAudioUnit.allocateRenderResources()
            try chain.setMembers(members.map(\.chainMember))
            try check(chain.members.count == members.count, "chain holds every member")
            try check(chain.latencyFrames == members.reduce(0) { $0 + Int(($1.latency * sampleRate).rounded()) }, "chain latency is the members' sum")
            try check(abs(chain.node.auAudioUnit.latency * sampleRate - Double(chain.latencyFrames)) < 0.5, "chain unit reports its members' latency")
            let nodes = NodeChain(reference, format: format)
            let input = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4096)!, output = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4096)!
            input.frameLength = 4096; output.frameLength = 4096
            var time = 0.0, worst: Float = 0, energy: Float = 0
            var order = Array(0..<group.count)
            for block in 0..<160 {
                let frames: AUAudioFrameCount = [128, 256, 512, 64, 1000][block % 5]
                for frame in 0..<Int(frames) {
                    let t = time + Double(frame)
                    let burst: Float = (block / 12) % 3 == 2 ? 0 : 1   // silences exercise gating and tails
                    input.floatChannelData![0][frame] = burst * Float(sin(t * 0.021) * 0.4) + (random() - 0.5) * 0.05 * burst
                    input.floatChannelData![1][frame] = burst * Float(sin(t * 0.017 + 1) * 0.3)
                }
                // Scheduled automation through scheduleParameterBlock (absolute and immediate times, ramps).
                if block % 7 == 3 {
                    let index = Int(random() * Float(group.count)) % group.count
                    if let parameter = reference[index].parameterTree!.allParameters.first(where: { reference[index].parameterStepCount(address: $0.address) == 0 }) {
                        let value = AUValue(random()), at = AUEventSampleTime(time) + AUEventSampleTime(random() * Float(frames))
                        let ramp = AUAudioFrameCount(block % 2 == 0 ? 300 : 0)
                        reference[index].scheduleParameterBlock(at, ramp, parameter.address, value)
                        members[index].scheduleParameterBlock(at, ramp, parameter.address, value)
                        reference[index].scheduleParameterBlock(AUEventSampleTimeImmediate + 5, 0, parameter.address, value * 0.5)
                        members[index].scheduleParameterBlock(AUEventSampleTimeImmediate + 5, 0, parameter.address, value * 0.5)
                    }
                }
                // Bypass toggles (click-free crossfade, dormant skip, re-engage).
                if block % 11 == 5 {
                    let index = Int(random() * Float(group.count)) % group.count
                    let bypass = !reference[index].shouldBypassEffect
                    reference[index].shouldBypassEffect = bypass; members[index].shouldBypassEffect = bypass
                }
                // UI parameter change while playing.
                if block % 13 == 6 {
                    let index = Int(random() * Float(group.count)) % group.count
                    if let parameter = reference[index].parameterTree!.allParameters.last {
                        let value = AUValue(random())
                        parameter.value = value; members[index].parameterTree!.parameter(withAddress: parameter.address)!.value = value
                    }
                }
                // In-place reorder, remove and re-add (state continuity of every member).
                if block == 40 || block == 90 {
                    order.reverse()
                    nodes.units = order.map { reference[$0] }
                    try chain.setMembers(order.map { members[$0].chainMember })
                }
                if block == 60, order.count > 1 {
                    let removed = order.removeLast()
                    nodes.units = order.map { reference[$0] }
                    try chain.setMembers(order.map { members[$0].chainMember })
                    try check(chain.members.count == order.count, "member removed in place")
                    order.append(removed)
                }
                if block == 75 {
                    nodes.units = order.map { reference[$0] }
                    try chain.setMembers(order.map { members[$0].chainMember })
                }
                // Save/restore while chained: the member's state is still its own.
                if block == 120, let first = members.first {
                    let saved = try first.documentState()
                    try first.restoreDocumentState(saved)
                    try reference[0].restoreDocumentState(try reference[0].documentState())
                    let after = try first.documentState()
                    try check(NSDictionary(dictionary: saved).isEqual(to: after), "member state round-trips while chained")
                }
                let expected = try nodes.render(input: input, frames: frames, time: time)
                let actual = try renderChain(chain, input: input, output: output, frames: frames, time: time)
                worst = max(worst, maxDifference(expected, actual))
                for channel in 0..<2 { for value in actual[channel] { try check(value.isFinite, "chain output finite"); energy += value * value } }
                time += Double(frames)
            }
            try check(worst <= 1e-6, "group \(groupIndex) \(group): chained output equals per-node output (max difference \(worst))")
            try check(energy > 0, "group \(groupIndex) chain is audible")
            chain.collectRetired()
        }
        // scheduleParameterBlock lands exactly where AUAudioUnit's own event schedule would
        // deliver the same event to the render (absolute and immediate-relative times, ramps,
        // events beyond the current block), so the per-node and chained paths behave as before.
        for id in ids {
            let listed = try makeUnit(id, sampleRate: sampleRate), scheduled = try makeUnit(id, sampleRate: sampleRate)
            guard let parameter = listed.parameterTree!.allParameters.first(where: { listed.parameterStepCount(address: $0.address) == 0 }) else { continue }
            let input = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 512)!
            input.frameLength = 512
            let outputs = [AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 512)!, AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 512)!]
            for buffer in outputs { buffer.frameLength = 512 }
            let pull: AURenderPullInputBlock = { _, _, count, bus, list in
                if bus != 0 { return kAudioUnitErr_NoConnection }
                let buffers = UnsafeMutableAudioBufferListPointer(list)
                for channel in 0..<2 {
                    if let target = buffers[channel].mData { memcpy(target, input.floatChannelData![channel], Int(count) * 4) }
                    else { buffers[channel].mData = UnsafeMutableRawPointer(input.floatChannelData![channel]) }
                    buffers[channel].mDataByteSize = count * 4
                }
                return noErr
            }
            // (block, offset in block, ramp, value); the last two arrive one block early via the schedule.
            let plan: [(Int, Int, UInt32, Float)] = [(0, 0, 0, 0.1), (1, 100, 0, 0.8), (2, 37, 700, 0.2), (4, 0, 0, 0.6), (5, 300, 64, 0.95)]
            for block in 0..<8 {
                let start = AUEventSampleTime(block * 512)
                for frame in 0..<512 {
                    input.floatChannelData![0][frame] = Float(sin(Double(block * 512 + frame) * 0.013) * 0.5)
                    input.floatChannelData![1][frame] = Float(sin(Double(block * 512 + frame) * 0.007) * 0.4)
                }
                var events: [AURenderEvent] = []
                for item in plan where item.0 == block {
                    var event = AURenderEvent()
                    event.parameter = AUParameterEvent(next: nil, eventSampleTime: start + AUEventSampleTime(item.1), eventType: item.2 > 0 ? .parameterRamp : .parameter,
                        reserved: (0, 0, 0), rampDurationSampleFrames: item.2, parameterAddress: parameter.address, value: item.3)
                    events.append(event)
                }
                for item in plan where item.0 == block + 1 && block >= 3 {
                    scheduled.scheduleParameterBlock(start + 512 + AUEventSampleTime(item.1), item.2, parameter.address, item.3)
                }
                for item in plan where item.0 == block && block < 4 {
                    // Immediate-relative for the first, absolute for the rest.
                    let time = item.1 == 0 && block == 0 ? AUEventSampleTimeImmediate : start + AUEventSampleTime(item.1)
                    scheduled.scheduleParameterBlock(time, item.2, parameter.address, item.3)
                }
                try events.withUnsafeMutableBufferPointer { list in
                    for index in 0..<list.count { list[index].head.next = index + 1 < list.count ? list.baseAddress! + index + 1 : nil }
                    var stamp = AudioTimeStamp(); stamp.mSampleTime = Double(start); stamp.mFlags = .sampleTimeValid
                    var flags = AudioUnitRenderActionFlags()
                    try check(listed.internalRenderBlock(&flags, &stamp, 512, 0, outputs[0].mutableAudioBufferList, list.isEmpty ? nil : list.baseAddress, pull) == noErr, id + " listed render")
                    flags = AudioUnitRenderActionFlags()
                    try check(scheduled.internalRenderBlock(&flags, &stamp, 512, 0, outputs[1].mutableAudioBufferList, nil, pull) == noErr, id + " scheduled render")
                }
                for channel in 0..<2 { for frame in 0..<512 {
                    try check(outputs[0].floatChannelData![channel][frame] == outputs[1].floatChannelData![channel][frame], id + " scheduled automation matches delivered events (block \(block))")
                } }
            }
        }
        // Rejects a member twice and foreign objects.
        let chain = try BuiltinEffectsChain(sampleRate: sampleRate)
        let unit = try makeUnit(ids[0], sampleRate: sampleRate)
        var rejected = false
        do { try chain.setMembers([unit.chainMember, unit.chainMember]) } catch { rejected = true }
        try check(rejected, "a member is accepted once per chain")
        rejected = false
        do { try chain.setMembers([NSObject()]) } catch { rejected = true }
        try check(rejected, "only built-in effects can be chain members")
        // An empty chain passes audio through unchanged.
        try chain.node.auAudioUnit.allocateRenderResources()
        try chain.setMembers([])
        let input = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 256)!, output = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 256)!
        input.frameLength = 256; output.frameLength = 256
        for frame in 0..<256 { input.floatChannelData![0][frame] = Float(frame) / 256; input.floatChannelData![1][frame] = -Float(frame) / 256 }
        let through = try renderChain(chain, input: input, output: output, frames: 256, time: 0)
        try check(through[0] == Array(UnsafeBufferPointer(start: input.floatChannelData![0], count: 256)), "empty chain is a pass-through")
    }
}

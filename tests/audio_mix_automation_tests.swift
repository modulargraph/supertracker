// Standalone, offline tests: this executable never opens an audio device.
// swiftc -swift-version 5 -module-cache-path /tmp/supertracker-swift-cache \
//   Sources/SuperTracker/AudioMixAutomation.swift tests/audio_mix_automation_tests.swift \
//   -o /tmp/supertracker-audio-mix-tests
// /tmp/supertracker-audio-mix-tests
import Foundation
import AVFoundation
import AudioToolbox

private let testRate = 48_000.0
private let tolerance: Float = 0.000001

private func require(_ condition: @autoclosure () -> Bool, _ message: String) throws {
    guard condition() else {
        throw NSError(domain: "AudioMixAutomationTests", code: 1,
                      userInfo: [NSLocalizedDescriptionKey: message])
    }
}

/// Supplies distinct stereo PCM to the AU's actual render implementation.
/// Separate output-list storage permits testing the nil-mData ownership contract.
private final class MixRenderFixture {
    let mix: AudioMixAutomation
    let capacity: Int
    private let listStorage: UnsafeMutableRawPointer
    private let list: UnsafeMutablePointer<AudioBufferList>
    private let leftStorage: UnsafeMutablePointer<Float>
    private let rightStorage: UnsafeMutablePointer<Float>
    private let pullLeft: UnsafeMutablePointer<Float>
    private let pullRight: UnsafeMutablePointer<Float>
    private(set) var samplePosition = 0

    init(capacity: Int = 8192, requestedMaximum: Int = 4096) throws {
        self.capacity = capacity
        mix = try AudioMixAutomation(sampleRate: testRate)
        let bytes = MemoryLayout<AudioBufferList>.size + MemoryLayout<AudioBuffer>.stride
        listStorage = .allocate(byteCount: bytes, alignment: MemoryLayout<AudioBufferList>.alignment)
        listStorage.initializeMemory(as: UInt8.self, repeating: 0, count: bytes)
        list = listStorage.assumingMemoryBound(to: AudioBufferList.self)
        list.pointee.mNumberBuffers = 2
        leftStorage = .allocate(capacity: capacity)
        rightStorage = .allocate(capacity: capacity)
        pullLeft = .allocate(capacity: capacity)
        pullRight = .allocate(capacity: capacity)
        for pointer in [leftStorage, rightStorage, pullLeft, pullRight] {
            pointer.initialize(repeating: 0, count: capacity)
        }
        let unit = mix.node.auAudioUnit
        let format = AVAudioFormat(standardFormatWithSampleRate: testRate, channels: 2)!
        try require(unit.inputBusses.count == 1 && unit.outputBusses.count == 1,
                    "Mix AU provides one stereo input bus and one stereo output bus")
        try unit.inputBusses[0].setFormat(format)
        try unit.outputBusses[0].setFormat(format)
        unit.maximumFramesToRender = AUAudioFrameCount(requestedMaximum)
        try unit.allocateRenderResources()
        defaults()
        mix.reset()
    }

    deinit {
        mix.node.auAudioUnit.deallocateRenderResources()
        for pointer in [leftStorage, rightStorage, pullLeft, pullRight] {
            pointer.deinitialize(count: capacity)
            pointer.deallocate()
        }
        listStorage.deallocate()
    }

    func defaults(trackVolume: Float = 1, instrumentVolume: Float = 1, masterVolume: Float = 1,
                  trackPan: Float = 0, instrumentPan: Float = 0, muted: Bool = false) {
        mix.setDefaults(trackVolume: trackVolume, instrumentVolume: instrumentVolume,
                        masterVolume: masterVolume, trackPan: trackPan,
                        instrumentPan: instrumentPan, muted: muted)
    }

    func constant(count: Int, chunks: [Int] = [4096], nilOutput: Bool = false,
                  publicBlock: Bool = false) throws -> ([Float], [Float]) {
        try render([Float](repeating: 0.5, count: count), [Float](repeating: -0.25, count: count),
                   chunks: chunks, nilOutput: nilOutput, publicBlock: publicBlock)
    }

    func render(_ left: [Float], _ right: [Float], chunks: [Int] = [4096],
                nilOutput: Bool = false, publicBlock: Bool = false) throws -> ([Float], [Float]) {
        try require(left.count == right.count, "Input channel lengths agree")
        try require(!chunks.isEmpty && chunks.allSatisfy { $0 > 0 && $0 <= capacity },
                    "Render block sizes fit fixture capacity")
        var resultLeft: [Float] = [], resultRight: [Float] = []
        resultLeft.reserveCapacity(left.count)
        resultRight.reserveCapacity(right.count)
        let internalRender = mix.node.auAudioUnit.internalRenderBlock
        let publicRender = mix.node.auAudioUnit.renderBlock
        var offset = 0, blockIndex = 0
        while offset < left.count {
            let frames = min(chunks[blockIndex % chunks.count], left.count - offset)
            let output = UnsafeMutableAudioBufferListPointer(list)
            for channel in 0..<2 {
                let storage = channel == 0 ? leftStorage : rightStorage
                for frame in 0..<frames { storage[frame] = .nan }
                output[channel].mNumberChannels = 1
                output[channel].mDataByteSize = UInt32(frames * MemoryLayout<Float>.size)
                output[channel].mData = nilOutput ? nil : UnsafeMutableRawPointer(storage)
            }
            var timestamp = AudioTimeStamp()
            timestamp.mSampleTime = Double(samplePosition)
            timestamp.mFlags = .sampleTimeValid
            var flags = AudioUnitRenderActionFlags()
            var pulled = false
            let inputOffset = offset
            let pull: AURenderPullInputBlock = { _, _, requested, bus, inputList in
                guard bus == 0 && requested == AUAudioFrameCount(frames) else { return kAudio_ParamError }
                let input = UnsafeMutableAudioBufferListPointer(inputList)
                guard input.count == 2 else { return kAudio_ParamError }
                for channel in 0..<2 {
                    if input[channel].mData == nil {
                        input[channel].mData = UnsafeMutableRawPointer(channel == 0 ? self.pullLeft : self.pullRight)
                    }
                    guard let data = input[channel].mData?.assumingMemoryBound(to: Float.self) else {
                        return kAudio_ParamError
                    }
                    input[channel].mNumberChannels = 1
                    input[channel].mDataByteSize = UInt32(frames * MemoryLayout<Float>.size)
                    for frame in 0..<frames {
                        data[frame] = channel == 0 ? left[inputOffset + frame] : right[inputOffset + frame]
                    }
                }
                pulled = true
                return noErr
            }
            let status = publicBlock ?
                publicRender(&flags, &timestamp, AUAudioFrameCount(frames), 0, list, pull) :
                internalRender(&flags, &timestamp, AUAudioFrameCount(frames), 0, list, nil, pull)
            try require(status == noErr,
                        "Render succeeds at absolute frame \(samplePosition), count \(frames), status \(status)")
            try require(pulled, "Mix effect pulls upstream PCM in every render block")
            for channel in 0..<2 {
                try require(output[channel].mData != nil, "AU supplies storage when output mData is nil")
                try require(output[channel].mDataByteSize >= UInt32(frames * MemoryLayout<Float>.size),
                            "AU advertises sufficient initialized output PCM")
            }
            let renderedLeft = output[0].mData!.assumingMemoryBound(to: Float.self)
            let renderedRight = output[1].mData!.assumingMemoryBound(to: Float.self)
            for frame in 0..<frames {
                resultLeft.append(renderedLeft[frame])
                resultRight.append(renderedRight[frame])
            }
            offset += frames
            samplePosition += frames
            blockIndex += 1
        }
        try require(resultLeft.allSatisfy(\.isFinite) && resultRight.allSatisfy(\.isFinite),
                    "All rendered samples are initialized and finite")
        return (resultLeft, resultRight)
    }
}

@main
private struct AudioMixAutomationTests {
    static func assertConstant(_ output: ([Float], [Float]), label: String,
                               factors: (Int) -> (Float, Float)) throws {
        for index in output.0.indices {
            let (left, right) = factors(index)
            try require(abs(output.0[index] - 0.5 * left) <= tolerance,
                        "\(label): left frame \(index), expected \(0.5 * left), got \(output.0[index])")
            try require(abs(output.1[index] + 0.25 * right) <= tolerance,
                        "\(label): right frame \(index), expected \(-0.25 * right), got \(output.1[index])")
        }
    }

    static func absoluteSampleTiming() throws {
        for publicBlock in [false, true] {
            let fixture = try MixRenderFixture()
            fixture.mix.schedule(target: 0, value: 0.25, sampleTime: 6000)
            let output = try fixture.constant(count: 12_288, publicBlock: publicBlock)
            try assertConstant(output, label: "Absolute 6000-frame event") { frame in
                let gain: Float = frame < 6000 ? 1 : 0.25
                return (gain, gain)
            }
        }
        print("PASS absolute frame 6000 gain change occurs exactly 1904 frames into second 4096-frame block through direct and public AU rendering")
    }

    static func legacyImmediateOffsets() throws {
        let fixture = try MixRenderFixture()
        _ = try fixture.constant(count: 4096, publicBlock: true)
        fixture.mix.schedule(target: 0, value: 0.25, sampleTime: AUEventSampleTimeImmediate + 123)
        try assertConstant(fixture.constant(count: 4096, publicBlock: true),
                           label: "Legacy immediate offset at absolute frame 4219") { frame in
            let gain: Float = frame < 123 ? 1 : 0.25
            return (gain, gain)
        }
        // Resolve the offset once, when the event first reaches the renderer.
        // Keeping it in the future queue must not rebase it onto subsequent blocks.
        fixture.mix.schedule(target: 0, value: 0.75, sampleTime: AUEventSampleTimeImmediate + 6000)
        try assertConstant(fixture.constant(count: 8192, publicBlock: true),
                           label: "Legacy immediate offset retained across blocks") { frame in
            let gain: Float = frame < 6000 ? 0.25 : 0.75
            return (gain, gain)
        }
        fixture.mix.schedule(target: 0, value: 0.5, sampleTime: AUEventSampleTimeImmediate)
        try assertConstant(fixture.constant(count: 128, publicBlock: true),
                           label: "Immediate sentinel with zero offset") { _ in (0.5, 0.5) }
        fixture.mix.schedule(target: 0, value: 0.8, sampleTime: -123)
        try assertConstant(fixture.constant(count: 128, publicBlock: true),
                           label: "Ordinary negative absolute time remains a past event") { _ in (0.8, 0.8) }
        print("PASS legacy immediate+123 changes precisely at absolute frame 4219, larger offsets retain their first-block origin, and ordinary negative timestamps remain past events")
    }

    static func multiplyingVolumes() throws {
        let fixture = try MixRenderFixture()
        fixture.defaults(trackVolume: 0.8, instrumentVolume: 0.5, masterVolume: 0.75)
        fixture.mix.schedule(target: 0, value: 0.4, sampleTime: 6000)
        fixture.mix.schedule(target: 23, value: 0.25, sampleTime: 6500)
        fixture.mix.schedule(target: 0, value: 0.2, sampleTime: 7000)
        fixture.mix.schedule(target: 23, value: 0.8, sampleTime: 7000)
        let output = try fixture.constant(count: 9000, chunks: [1, 7, 64, 127, 4096, 13])
        try assertConstant(output, label: "Independent multiplicative volume targets") { frame in
            let track: Float = frame < 6000 ? 0.8 : frame < 7000 ? 0.4 : 0.2
            let instrument: Float = frame < 6500 ? 0.5 : frame < 7000 ? 0.25 : 0.8
            let gain = track * instrument * 0.75
            return (gain, gain)
        }
        print("PASS track/instrument/master levels multiply independently, including simultaneous automation events and irregular render quanta")
    }

    static func additiveStereoBalance() throws {
        let fixture = try MixRenderFixture()
        fixture.mix.schedule(target: 1, value: -1, sampleTime: 100)
        fixture.mix.schedule(target: 1, value: 1, sampleTime: 200)
        fixture.mix.schedule(target: 1, value: 0.25, sampleTime: 300)
        fixture.mix.schedule(target: 24, value: 0.25, sampleTime: 300)
        fixture.mix.schedule(target: 24, value: -0.75, sampleTime: 400)
        fixture.mix.schedule(target: 1, value: -0.75, sampleTime: 500)
        fixture.mix.schedule(target: 1, value: 0.75, sampleTime: 600)
        fixture.mix.schedule(target: 24, value: 0.75, sampleTime: 600)
        fixture.mix.schedule(target: 1, value: 0, sampleTime: 700)
        fixture.mix.schedule(target: 24, value: 0, sampleTime: 700)
        let output = try fixture.constant(count: 1024, chunks: [127, 1, 64, 7])
        try assertConstant(output, label: "Additive stereo balance") { frame in
            switch frame {
            case 100..<200: return (1, 0)
            case 200..<300: return (0, 1)
            case 300..<400: return (0.5, 1)
            case 400..<500: return (1, 0.5)
            case 500..<600: return (1, 0)
            case 600..<700: return (0, 1)
            default: return (1, 1)
            }
        }
        print("PASS center preserves stereo unity, track/instrument balance adds and clamps, and hard pan mutes only the opposite channel")
    }

    static func muteAndLiveDefaults() throws {
        let muted = try MixRenderFixture()
        muted.defaults(muted: true)
        muted.mix.schedule(target: 0, value: 0.5, sampleTime: 500)
        muted.mix.schedule(target: 1, value: 1, sampleTime: 700)
        try assertConstant(muted.constant(count: 1024), label: "Mute overrides automation") { _ in (0, 0) }

        let fixture = try MixRenderFixture()
        fixture.defaults(trackVolume: 0.8, instrumentVolume: 0.5, masterVolume: 0.5)
        fixture.mix.schedule(target: 0, value: 0.25, sampleTime: 32)
        let initial = try fixture.constant(count: 128)
        try assertConstant(initial, label: "Default levels before automation") { frame in
            let gain: Float = (frame < 32 ? 0.8 : 0.25) * 0.5 * 0.5
            return (gain, gain)
        }
        // Updating only master or pan must leave the automated track level intact.
        fixture.defaults(trackVolume: 0.8, instrumentVolume: 0.5, masterVolume: 0.8)
        try assertConstant(fixture.constant(count: 128), label: "Master default preserves automated track level") { _ in (0.1, 0.1) }
        fixture.defaults(trackVolume: 0.8, instrumentVolume: 0.5, masterVolume: 0.8, trackPan: -0.5)
        try assertConstant(fixture.constant(count: 128), label: "Pan default preserves automated track level") { _ in (0.1, 0.05) }
        // An actual edit of this same target becomes the new live level immediately.
        fixture.defaults(trackVolume: 0.6, instrumentVolume: 0.5, masterVolume: 0.8, trackPan: -0.5)
        try assertConstant(fixture.constant(count: 128), label: "Edited target receives changed live default") { _ in (0.24, 0.12) }
        print("PASS mute suppresses all PCM and unrelated live-default edits preserve automated target values")
    }

    static func sendGainTargets() throws {
        for target in [4, 5] {
            let fixture = try MixRenderFixture()
            fixture.mix.setGain(0.2)
            fixture.mix.schedule(target: target, value: 0.6, sampleTime: 6000)
            fixture.mix.schedule(target: target, value: 0, sampleTime: 7000)
            let output = try fixture.constant(count: 8192, publicBlock: true)
            try assertConstant(output, label: "Send target \(target)") { frame in
                let gain: Float = frame < 6000 ? 0.2 : frame < 7000 ? 0.6 : 0
                return (gain, gain)
            }
            fixture.mix.setGain(0.4)
            try assertConstant(fixture.constant(count: 64), label: "Live send default \(target)") { _ in (0.4, 0.4) }
        }
        print("PASS delay/reverb send targets 4 and 5 change gain at exact sample positions and accept live default edits")
    }

    static func resetRestoresDefaults() throws {
        let fixture = try MixRenderFixture()
        fixture.defaults(trackVolume: 0.8, instrumentVolume: 0.5, masterVolume: 0.5,
                         trackPan: 0.25, instrumentPan: -0.25)
        fixture.mix.schedule(target: 0, value: 0.3, sampleTime: 6000)
        fixture.mix.schedule(target: 23, value: 0.6, sampleTime: 6500)
        fixture.mix.schedule(target: 1, value: -1, sampleTime: 7000)
        _ = try fixture.constant(count: 8192)
        fixture.mix.schedule(target: 0, value: 0.9, sampleTime: 10_000)
        fixture.mix.schedule(target: 24, value: 1, sampleTime: 10_500)
        fixture.mix.reset()
        try assertConstant(fixture.constant(count: 4096), label: "Reset cancels pending events and restores defaults") { _ in (0.2, 0.2) }
        // Fresh events after reset must survive; resetting cannot permanently mute
        // the queue or reject new playback's automation.
        let resumeAt = fixture.samplePosition + 100
        fixture.mix.schedule(target: 23, value: 0.25, sampleTime: Int64(resumeAt))
        try assertConstant(fixture.constant(count: 256), label: "Fresh event after reset") { frame in
            let gain: Float = frame < 100 ? 0.2 : 0.1
            return (gain, gain)
        }

        let send = try MixRenderFixture()
        send.mix.setGain(0.3)
        send.mix.schedule(target: 4, value: 0.9, sampleTime: 0)
        _ = try send.constant(count: 128)
        send.mix.schedule(target: 4, value: 0.1, sampleTime: 200)
        send.mix.reset()
        try assertConstant(send.constant(count: 512), label: "Reset restores send default and clears future send event") { _ in (0.3, 0.3) }
        print("PASS reset restores latest track/instrument/pan/send defaults, cancels future events, and accepts a fresh playback event")
    }

    static func oversizedPublicRenders() throws {
        let count = 24_576
        let left = (0..<count).map { Float(($0 % 509) - 254) / 2048 }
        let right = (0..<count).map { Float(($0 % 127) - 63) / 512 }
        for nilOutput in [false, true] {
            let fixture = try MixRenderFixture(capacity: 8192, requestedMaximum: 4096)
            try require(fixture.mix.node.auAudioUnit.maximumFramesToRender >= 8192,
                        "Nominal 4096-frame maximum retains room for larger upstream pulls")
            fixture.mix.schedule(target: 0, value: 0.25, sampleTime: 4096)
            fixture.mix.schedule(target: 23, value: 0.5, sampleTime: 8192)
            fixture.mix.schedule(target: 1, value: -0.5, sampleTime: 12_288)
            let output = try fixture.render(left, right, chunks: [4097, 8192, 4096, 1],
                                            nilOutput: nilOutput, publicBlock: true)
            for frame in 0..<count {
                let gain: Float = frame < 4096 ? 1 : frame < 8192 ? 0.25 : 0.125
                let rightBalance: Float = frame < 12_288 ? 1 : 0.5
                try require(abs(output.0[frame] - left[frame] * gain) <= tolerance &&
                            abs(output.1[frame] - right[frame] * gain * rightBalance) <= tolerance,
                            "Large render preserves sample \(frame), nil storage \(nilOutput)")
            }
        }
        print("PASS 4097/8192-frame public AU renders with a nominal 4096-frame maximum, exact automation, independent stereo, and nil output buffers")
    }

    static func publicationOrderAndQueueRecovery() throws {
        let ordered = try MixRenderFixture()
        ordered.mix.schedule(target: 0, value: 0.1, sampleTime: 7000)
        ordered.mix.schedule(target: 0, value: 0.2, sampleTime: 6000)
        ordered.mix.schedule(target: 0, value: 0.3, sampleTime: 6500)
        ordered.mix.schedule(target: 0, value: 0.4, sampleTime: 6500)
        try assertConstant(ordered.constant(count: 8192), label: "Out-of-order publication and stable equal timestamps") { frame in
            let gain: Float = frame < 6000 ? 1 : frame < 6500 ? 0.2 : frame < 7000 ? 0.4 : 0.1
            return (gain, gain)
        }
        ordered.mix.schedule(target: 0, value: 0.75, sampleTime: 100)
        try assertConstant(ordered.constant(count: 128), label: "Late published event takes effect at next render") { _ in (0.75, 0.75) }

        let saturated = try MixRenderFixture()
        // Well beyond the bounded queue's capacity. The exact implementation
        // capacity is not important: overflow must be observable and safe.
        for _ in 0..<20_000 {
            saturated.mix.schedule(target: 0, value: 0.5, sampleTime: 0)
        }
        try require(saturated.mix.droppedEvents > 0 && saturated.mix.droppedEvents < 20_000,
                    "Bounded producer queue reports overflow while accepting events")
        try assertConstant(saturated.constant(count: 128), label: "Producer overflow keeps accepted events valid") { _ in (0.5, 0.5) }
        let droppedAfterProducerOverflow = saturated.mix.droppedEvents
        saturated.mix.schedule(target: 0, value: 0.25, sampleTime: 192)
        try assertConstant(saturated.constant(count: 128), label: "Producer queue resumes after draining") { frame in
            let gain: Float = frame < 64 ? 0.5 : 0.25
            return (gain, gain)
        }
        try require(saturated.mix.droppedEvents == droppedAfterProducerOverflow,
                    "A drained producer queue accepts a fresh event")

        // Fill the future-event store, drain the producer ring into it, and then
        // publish more: both storage bounds must remain safe independently.
        for _ in 0..<20_000 {
            saturated.mix.schedule(target: 0, value: 0.8, sampleTime: 500_000)
        }
        _ = try saturated.constant(count: 1)
        let droppedBeforePendingOverflow = saturated.mix.droppedEvents
        for _ in 0..<32 {
            saturated.mix.schedule(target: 0, value: 0.9, sampleTime: 500_001)
        }
        try assertConstant(saturated.constant(count: 128), label: "Future queue saturation preserves present playback") { _ in (0.25, 0.25) }
        try require(saturated.mix.droppedEvents > droppedBeforePendingOverflow,
                    "Bounded future-event storage reports overflow")
        saturated.mix.reset()
        try assertConstant(saturated.constant(count: 128), label: "Reset recovers from pending overflow") { _ in (1, 1) }
        saturated.mix.schedule(target: 0, value: 0.6, sampleTime: Int64(saturated.samplePosition + 64))
        try assertConstant(saturated.constant(count: 128), label: "Fresh automation works after overflow and reset") { frame in
            let gain: Float = frame < 64 ? 1 : 0.6
            return (gain, gain)
        }
        print("PASS out-of-order and same-time event ordering, late events, bounded producer/future overflow reporting, and recovery after draining/reset")
    }

    static func maximumLatencyAutomationCapacity() throws {
        let fixture = try MixRenderFixture()
        let offset = 96_000, spacing = 32, segments = 3512
        let scheduledEvents = segments * 4
        try require(scheduledEvents == 14_048 && fixture.mix.eventCapacity >= scheduledEvents,
                    "Default 48 kHz capacity covers four controls through two seconds of latency plus render headroom")
        func controls(_ segment: Int) -> (volume: Float, instrument: Float, pan: Float, instrumentPan: Float) {
            (Float(segment % 5 + 1) / 5,
             Float(segment % 7 + 1) / 8,
             Float(segment % 9 - 4) / 4,
             Float(segment % 5 - 2) / 4)
        }
        // Publish the full lookahead burst before the first render. This exercises
        // both the producer ring and future-event heap at realistic maximum PDC.
        for segment in 0..<segments {
            let values = controls(segment)
            let time = Int64(offset + spacing * segment)
            fixture.mix.schedule(target: 0, value: values.volume, sampleTime: time)
            fixture.mix.schedule(target: 23, value: values.instrument, sampleTime: time)
            fixture.mix.schedule(target: 1, value: values.pan, sampleTime: time)
            fixture.mix.schedule(target: 24, value: values.instrumentPan, sampleTime: time)
        }
        try require(fixture.mix.droppedEvents == 0,
                    "Producer ring accepts all 14048 latency-offset events before rendering")
        let count = offset + segments * spacing + 128
        let output = try fixture.constant(count: count, publicBlock: true)
        try assertConstant(output, label: "Four controls after two-second latency offset") { frame in
            guard frame >= offset else { return (1, 1) }
            let values = controls(min(segments - 1, (frame - offset) / spacing))
            let gain = values.volume * values.instrument
            let pan = max(-1, min(1, values.pan + values.instrumentPan))
            return (gain * (pan > 0 ? 1 - pan : 1), gain * (pan < 0 ? 1 + pan : 1))
        }
        try require(fixture.mix.droppedEvents == 0,
                    "Future-event heap retains every control event through maximum latency")
        let highRate = try AudioMixAutomation(sampleRate: 96_000)
        try require(highRate.eventCapacity == 32_768,
                    "Default capacity grows to 32768 events at 96 kHz")
        let send = try AudioMixAutomation(sampleRate: testRate, eventCapacity: 4096)
        try require(send.eventCapacity == 4096,
                    "Single-target send supports an explicitly bounded 4096-event queue")
        print("PASS all 14048 four-target events survive a 96000-frame latency offset with exact 32-frame controls; 96 kHz scales capacity and send queues remain configurable")
    }

    /// The block-segmented vector render must equal the original per-sample formula
    /// bit for bit (sample*gain, then *pan factor), including unity and zero segments.
    static func segmentedRenderMatchesPerSampleReference() throws {
        let count = 6000
        let left = (0..<count).map { Float(sin(Double($0) * 0.031)) * 0.9 }
        let right = (0..<count).map { Float(cos(Double($0) * 0.017)) * -0.7 }
        let fixture = try MixRenderFixture()
        fixture.defaults(trackVolume: 0.8, instrumentVolume: 0.7, masterVolume: 0.9, trackPan: 0.3, instrumentPan: -0.1)
        // (time, target, value): volume, instrument volume, pan and instrument pan changes,
        // including unity, zero and hard-panned segments and events on chunk edges.
        let events: [(Int, Int, Float)] = [(0, 0, 0.8), (37, 1, -0.6), (128, 0, 1), (128, 1, 0), (129, 24, 0), (500, 23, 1),
            (512, 0, 0), (700, 0, 0.33), (701, 1, 1), (1500, 24, -1), (2047, 0, 1.7), (3000, 1, 0), (3000, 24, 0.2), (4095, 23, 0.4), (5000, 0, 1)]
        for (time, target, value) in events { fixture.mix.schedule(target: target, value: value, sampleTime: Int64(time)) }
        let output = try fixture.render(left, right, chunks: [128, 37, 1, 512, 4096])
        var values: [Float] = [0.8, 0.7, 0.9, 0.3, -0.1, 0, 1, 1]
        let index = [0: 0, 1: 3, 23: 1, 24: 4]
        var next = 0
        for frame in 0..<count {
            while next < events.count && events[next].0 <= frame {
                let (_, target, value) = events[next]; let slot = index[target]!
                values[slot] = slot == 3 || slot == 4 ? max(-1, min(1, value)) : max(0, min(2, value)); next += 1
            }
            let gain = values[5] > 0 ? 0 : values[0] * values[1] * values[2] * values[6] * values[7]
            let pan = max(-1, min(1, values[3] + values[4]))
            let l = left[frame] * gain * (pan > 0 ? 1 - pan : 1), r = right[frame] * gain * (pan < 0 ? 1 + pan : 1)
            try require(output.0[frame] == l || (output.0[frame] == 0 && l == 0), "Segmented left equals reference at \(frame): \(output.0[frame]) != \(l)")
            try require(output.1[frame] == r || (output.1[frame] == 0 && r == 0), "Segmented right equals reference at \(frame): \(output.1[frame]) != \(r)")
        }
        print("PASS block-segmented vector render equals the per-sample reference bit for bit across unity, zero and panned segments")
    }

    /// The solo gate multiplies into the gain, survives send automation and reset,
    /// and in-render metering reports the output peak only while enabled.
    static func soloGainAndMetering() throws {
        let fixture = try MixRenderFixture()
        fixture.mix.setGain(0.5)
        fixture.mix.setSoloGain(0)
        fixture.mix.schedule(target: 4, value: 0.9, sampleTime: 64)
        try assertConstant(fixture.constant(count: 128), label: "Solo gate silences a send whatever its scheduled gain") { _ in (0, 0) }
        fixture.mix.setSoloGain(1)
        try assertConstant(fixture.constant(count: 128), label: "Opened solo gate keeps the automated send gain") { _ in (0.9, 0.9) }
        fixture.mix.reset()
        try assertConstant(fixture.constant(count: 64), label: "Reset restores the send default, the gate stays open") { _ in (0.5, 0.5) }
        try require(fixture.mix.takePeaks() == (0, 0), "Metering is off by default")
        fixture.mix.meteringEnabled = true
        _ = try fixture.render([0.25, -0.75, 0.5, 0.2, 0.1], [0.1, 0.2, -0.3, 0, 0])
        let peaks = fixture.mix.takePeaks()
        try require(peaks.left == 0.375 && abs(peaks.right - 0.15) < 1e-7, "In-render meter reports finite output peaks: \(peaks)")
        try require(fixture.mix.takePeaks() == (0, 0), "Taking peaks clears them")
        fixture.mix.meteringEnabled = false
        _ = try fixture.constant(count: 64)
        try require(fixture.mix.takePeaks() == (0, 0), "Disabled metering measures nothing")
        print("PASS solo gate multiplies into automated send gain; in-render metering reports finite peaks only while enabled")
    }

    @MainActor static func main() throws {
        setbuf(stdout, nil)
        try absoluteSampleTiming()
        try legacyImmediateOffsets()
        try multiplyingVolumes()
        try additiveStereoBalance()
        try muteAndLiveDefaults()
        try sendGainTargets()
        try resetRestoresDefaults()
        try oversizedPublicRenders()
        try publicationOrderAndQueueRecovery()
        try maximumLatencyAutomationCapacity()
        try segmentedRenderMatchesPerSampleReference()
        try soloGainAndMetering()
        print("PASS all AudioMixAutomation standalone offline tests")
    }
}

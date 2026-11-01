// Standalone delay-compensation tests. Renders offline; never plays speakers.
// swiftc -swift-version 5 -module-cache-path /tmp/supertracker-swift-cache \
//   Sources/SuperTracker/AudioDelayCompensation.swift tests/audio_delay_unit_tests.swift \
//   -o /tmp/supertracker-audio-delay-tests
// /tmp/supertracker-audio-delay-tests
import Foundation
import AVFoundation
import AudioToolbox

private let testRate = 48_000.0
private let tolerance: Float = 0.000001

private func require(_ condition: @autoclosure () -> Bool, _ message: String) throws {
    guard condition() else {
        throw NSError(domain: "AudioDelayTests", code: 1,
                      userInfo: [NSLocalizedDescriptionKey: message])
    }
}

/// Calls the AU directly with irregular block sizes and an ordinary upstream pull.
/// The output list is independent of AVAudioPCMBuffer ownership, so nil mData is safe.
private final class DelayRenderFixture {
    let delay: AudioDelayCompensator
    private let capacity: Int
    private let listStorage: UnsafeMutableRawPointer
    private let list: UnsafeMutablePointer<AudioBufferList>
    private let leftStorage: UnsafeMutablePointer<Float>
    private let rightStorage: UnsafeMutablePointer<Float>
    private let pullLeft: UnsafeMutablePointer<Float>
    private let pullRight: UnsafeMutablePointer<Float>
    private var samplePosition = 0

    init(frames: Int, maximum: Int = 8192, renderCapacity: Int = 512,
         requestedMaximum: Int? = nil) throws {
        capacity = renderCapacity
        delay = try AudioDelayCompensator(sampleRate: testRate, maximumFrames: maximum)
        let bytes = MemoryLayout<AudioBufferList>.size + MemoryLayout<AudioBuffer>.stride
        listStorage = .allocate(byteCount: bytes, alignment: MemoryLayout<AudioBufferList>.alignment)
        listStorage.initializeMemory(as: UInt8.self, repeating: 0, count: bytes)
        list = listStorage.assumingMemoryBound(to: AudioBufferList.self)
        list.pointee.mNumberBuffers = 2
        leftStorage = .allocate(capacity: capacity)
        rightStorage = .allocate(capacity: capacity)
        pullLeft = .allocate(capacity: capacity)
        pullRight = .allocate(capacity: capacity)
        leftStorage.initialize(repeating: 0, count: capacity)
        rightStorage.initialize(repeating: 0, count: capacity)
        pullLeft.initialize(repeating: 0, count: capacity)
        pullRight.initialize(repeating: 0, count: capacity)
        let unit = delay.node.auAudioUnit
        let format = AVAudioFormat(standardFormatWithSampleRate: testRate, channels: 2)!
        try require(unit.inputBusses.count == 1 && unit.outputBusses.count == 1,
                    "Delay has one input and one output bus")
        try unit.inputBusses[0].setFormat(format)
        try unit.outputBusses[0].setFormat(format)
        unit.maximumFramesToRender = AUAudioFrameCount(requestedMaximum ?? capacity)
        delay.frames = frames
        try unit.allocateRenderResources()
        delay.reset()
    }

    deinit {
        delay.node.auAudioUnit.deallocateRenderResources()
        for pointer in [leftStorage, rightStorage, pullLeft, pullRight] {
            pointer.deinitialize(count: capacity)
            pointer.deallocate()
        }
        listStorage.deallocate()
    }

    func render(_ left: [Float], _ right: [Float], nilOutput: Bool = false,
                usePublicRenderBlock: Bool = false,
                chunks: [Int] = [1, 7, 64, 127, 13, 256, 512]) throws -> ([Float], [Float]) {
        try require(left.count == right.count, "Test input channels have equal lengths")
        try require(!chunks.isEmpty && chunks.allSatisfy { $0 > 0 && $0 <= capacity },
                    "Test block sizes fit allocated render resources")
        var resultLeft: [Float] = [], resultRight: [Float] = []
        resultLeft.reserveCapacity(left.count)
        resultRight.reserveCapacity(right.count)
        var offset = 0, blockIndex = 0
        let render = delay.node.auAudioUnit.internalRenderBlock
        let publicRender = delay.node.auAudioUnit.renderBlock
        while offset < left.count {
            let frameCount = min(chunks[blockIndex % chunks.count], left.count - offset)
            let buffers = UnsafeMutableAudioBufferListPointer(list)
            for channel in 0..<2 {
                let storage = channel == 0 ? leftStorage : rightStorage
                for frame in 0..<frameCount { storage[frame] = .nan }
                buffers[channel].mNumberChannels = 1
                buffers[channel].mDataByteSize = UInt32(frameCount * MemoryLayout<Float>.size)
                buffers[channel].mData = nilOutput ? nil : UnsafeMutableRawPointer(storage)
            }
            var timestamp = AudioTimeStamp()
            timestamp.mSampleTime = Double(samplePosition)
            timestamp.mFlags = .sampleTimeValid
            var flags = AudioUnitRenderActionFlags()
            var pulled = false
            let inputOffset = offset
            let pull: AURenderPullInputBlock = {
                _, _, requestedFrames, bus, inputList in
                guard bus == 0 && requestedFrames == AUAudioFrameCount(frameCount) else {
                    return kAudio_ParamError
                }
                let input = UnsafeMutableAudioBufferListPointer(inputList)
                guard input.count == 2 else { return kAudio_ParamError }
                for channel in 0..<2 {
                    if input[channel].mData == nil {
                        input[channel].mData = UnsafeMutableRawPointer(channel == 0 ? self.pullLeft : self.pullRight)
                    }
                    guard let samples = input[channel].mData?.assumingMemoryBound(to: Float.self) else {
                        return kAudio_ParamError
                    }
                    input[channel].mNumberChannels = 1
                    input[channel].mDataByteSize = UInt32(frameCount * MemoryLayout<Float>.size)
                    for frame in 0..<frameCount {
                        samples[frame] = channel == 0 ? left[inputOffset + frame] : right[inputOffset + frame]
                    }
                }
                pulled = true
                return noErr
            }
            let status = usePublicRenderBlock ?
                publicRender(&flags, &timestamp, AUAudioFrameCount(frameCount), 0, list, pull) :
                render(&flags, &timestamp, AUAudioFrameCount(frameCount), 0, list, nil, pull)
            try require(status == noErr, "Direct render succeeds at sample \(samplePosition), status \(status)")
            try require(pulled, "Every effect block pulls its upstream input")
            for channel in 0..<2 {
                try require(buffers[channel].mData != nil, "AU supplies output storage when mData is nil")
                try require(buffers[channel].mDataByteSize >= UInt32(frameCount * MemoryLayout<Float>.size),
                            "Output buffer advertises sufficient valid PCM")
            }
            let renderedLeft = buffers[0].mData!.assumingMemoryBound(to: Float.self)
            let renderedRight = buffers[1].mData!.assumingMemoryBound(to: Float.self)
            for frame in 0..<frameCount {
                resultLeft.append(renderedLeft[frame])
                resultRight.append(renderedRight[frame])
            }
            offset += frameCount
            samplePosition += frameCount
            blockIndex += 1
        }
        try require(resultLeft.allSatisfy(\.isFinite) && resultRight.allSatisfy(\.isFinite),
                    "Every output sample is initialized and finite")
        return (resultLeft, resultRight)
    }
}

@main
private struct AudioDelayTests {
    static func assertDelayed(_ input: ([Float], [Float]), _ output: ([Float], [Float]),
                              frames: Int, label: String) throws {
        for (channel, pair) in [(input.0, output.0), (input.1, output.1)].enumerated() {
            try require(pair.0.count == pair.1.count, "\(label): equal input/output lengths")
            for index in pair.1.indices {
                let expected: Float = index >= frames ? pair.0[index - frames] : 0
                try require(abs(pair.1[index] - expected) <= tolerance,
                            "\(label): channel \(channel), frame \(index), expected \(expected), got \(pair.1[index])")
            }
        }
    }

    static func pulses(count: Int) -> ([Float], [Float]) {
        var left = [Float](repeating: 0, count: count)
        var right = left
        for (index, value): (Int, Float) in [(0, 0.5), (127, -0.25), (511, 0.75), (1025, -0.5)] where index < count {
            left[index] = value
        }
        for (index, value): (Int, Float) in [(3, -0.75), (128, 0.375), (512, -0.125), (1024, 0.5)] where index < count {
            right[index] = value
        }
        return (left, right)
    }

    static func exactTiming() throws {
        for frames in [0, 1, 64, 4097, 8192] {
            let fixture = try DelayRenderFixture(frames: frames)
            try require(fixture.delay.node.auAudioUnit.latency == 0,
                        "Compensation delay reports zero AU latency to avoid a second engine correction")
            let input = pulses(count: frames + 1600)
            let output = try fixture.render(input.0, input.1)
            try assertDelayed(input, output, frames: frames, label: "Delay \(frames)")
            print("PASS exact \(frames)-frame delay across irregular blocks; independent stereo")
        }
    }

    static func boundsAndReset() throws {
        let fixture = try DelayRenderFixture(frames: 0, maximum: 4096)
        try require(fixture.delay.maximumFrames == 4096, "Explicit delay capacity is retained")
        fixture.delay.frames = Int.min
        try require(fixture.delay.frames == 0, "Negative delay clamps to zero without integer overflow")
        fixture.delay.frames = Int.max
        try require(fixture.delay.frames == fixture.delay.maximumFrames,
                    "Oversized delay clamps to allocated capacity without integer overflow")
        fixture.delay.reset()
        let input = pulses(count: 5700)
        try assertDelayed(input, fixture.render(input.0, input.1), frames: 4096, label: "Maximum clamp")

        fixture.delay.frames = 2048
        fixture.delay.reset()
        var left = [Float](repeating: 0, count: 127), right = left
        left[126] = 0.75
        right[63] = -0.5
        _ = try fixture.render(left, right)
        fixture.delay.reset()
        try require(fixture.delay.frames == 2048, "Reset preserves the requested delay")
        let silence = [Float](repeating: 0, count: 3000)
        let cleared = try fixture.render(silence, silence)
        try require(cleared.0.allSatisfy { abs($0) <= tolerance } && cleared.1.allSatisfy { abs($0) <= tolerance },
                    "Reset removes all old samples from both delay channels")
        let fresh = pulses(count: 3800)
        try assertDelayed(fresh, fixture.render(fresh.0, fresh.1), frames: 2048, label: "Timing after reset")
        print("PASS maximum/minimum clamping and reset clears history while preserving timing")
    }

    static func nilOutputBuffers() throws {
        for frames in [0, 1, 64, 1025] {
            let fixture = try DelayRenderFixture(frames: frames, maximum: 2048)
            let input = pulses(count: frames + 1600)
            try assertDelayed(input, fixture.render(input.0, input.1, nilOutput: true),
                              frames: frames, label: "Nil output storage, delay \(frames)")
        }
        print("PASS nil output mData receives valid AU-owned PCM storage")
    }

    static func largeRenderQuanta() throws {
        let sampleCount = 32_768
        var left = (0..<sampleCount).map { Float(($0 % 509) - 254) / 2048 }
        var right = (0..<sampleCount).map { Float(($0 % 127) - 63) / 512 }
        // Mark both sides of the larger block boundaries as well as every sample
        // with a sequence, so a missing/duplicated frame cannot pass a pulse-only test.
        for index in [4095, 4096, 4097, 8191, 8192, 12_288, 12_289] {
            left[index] += 0.5
            right[index] -= 0.25
        }
        for frames in [0, 1537, 8192] {
            for nilOutput in [false, true] {
                let fixture = try DelayRenderFixture(frames: frames, renderCapacity: 8192,
                                                    requestedMaximum: 4096)
                try require(fixture.delay.node.auAudioUnit.maximumFramesToRender >= 8192,
                            "A nominal 4096-frame configuration retains upstream render headroom")
                let rendered = try fixture.render(left, right, nilOutput: nilOutput,
                                                  usePublicRenderBlock: true,
                                                  chunks: [4097, 8192, 4096, 1])
                try assertDelayed((left, right), rendered, frames: frames,
                                  label: "Large quantum, delay \(frames), nil storage \(nilOutput)")
            }
        }
        print("PASS direct 4097/8192-frame pulls after nominal 4096-frame configuration; exact continuous stereo and boundary pulses")
    }

    static func smoothChanges() throws {
        let fixture = try DelayRenderFixture(frames: 0, maximum: 4096)
        let constantLeft = [Float](repeating: 0.375, count: 5000)
        let constantRight = [Float](repeating: -0.25, count: 5000)
        _ = try fixture.render(constantLeft, constantRight)
        for frames in [2048, 64, 0] {
            fixture.delay.frames = frames
            let steadyLeft = [Float](repeating: 0.375, count: 256)
            let steadyRight = [Float](repeating: -0.25, count: 256)
            let changing = try fixture.render(steadyLeft, steadyRight, chunks: [17, 127, 64, 48])
            try require(changing.0.allSatisfy { abs($0 - 0.375) <= tolerance } &&
                        changing.1.allSatisfy { abs($0 + 0.25) <= tolerance },
                        "Changing to \(frames) frames preserves established DC without gain modulation")
            // A pulse after the settling window establishes the new delay itself,
            // rather than accepting a permanently frozen old delay that preserves DC.
            var left = [Float](repeating: 0.375, count: frames + 513)
            var right = [Float](repeating: -0.25, count: frames + 513)
            left[0] += 0.25
            right[0] -= 0.125
            let output = try fixture.render(left, right)
            for index in output.0.indices {
                let expectedLeft: Float = index == frames ? 0.625 : 0.375
                let expectedRight: Float = index == frames ? -0.375 : -0.25
                try require(abs(output.0[index] - expectedLeft) <= tolerance &&
                            abs(output.1[index] - expectedRight) <= tolerance,
                            "Delay change settles within 256 frames: requested \(frames), sample \(index)")
            }
            // Flush the pulse before exercising another change of the read position.
            _ = try fixture.render(constantLeft, constantRight)
        }

        // A newly selected, not-yet-filled read position contains silence. Crossing
        // into or out of that history must ramp, rather than dropping DC in one sample.
        let cold = try DelayRenderFixture(frames: 0, maximum: 4096)
        let dc = [Float](repeating: 0.5, count: 512)
        _ = try cold.render(dc, dc)
        for (frames, previous, settled): (Int, Float, Float) in [(2048, 0.5, 0), (0, 0, 0.5)] {
            cold.delay.frames = frames
            let changed = try cold.render(dc, dc, chunks: [1, 7, 64, 127, 13, 256])
            for channel in [changed.0, changed.1] {
                var last = previous
                for value in channel {
                    try require(value >= -tolerance && value <= 0.5 + tolerance,
                                "Delay transition remains bounded")
                    try require(abs(value - last) <= 0.025,
                                "Delay transition does not abruptly jump: \(last) -> \(value)")
                    last = value
                }
                try require(channel.dropFirst(256).allSatisfy { abs($0 - settled) <= tolerance },
                            "Delay transition reaches the new history within 256 frames")
            }
        }
        print("PASS live delay changes preserve DC, settle within 256 frames, and ramp unfilled history")
    }

    static func offlineGraph() throws {
        let engine = AVAudioEngine()
        let format = AVAudioFormat(standardFormatWithSampleRate: testRate, channels: 2)!
        let compensationFrames = 1537
        let pulseFrame = 1024
        let delay = try AudioDelayCompensator(sampleRate: testRate, maximumFrames: 4096)
        delay.frames = compensationFrames
        delay.reset()
        func source(channel: Int, pulse: Int) -> AVAudioSourceNode {
            AVAudioSourceNode(format: format) { _, timestamp, count, list in
                for (index, buffer) in UnsafeMutableAudioBufferListPointer(list).enumerated() {
                    guard let data = buffer.mData?.assumingMemoryBound(to: Float.self) else {
                        return kAudio_ParamError
                    }
                    for frame in 0..<Int(count) {
                        data[frame] = index == channel && Int(timestamp.pointee.mSampleTime) + frame == pulse ? 0.5 : 0
                    }
                }
                return noErr
            }
        }
        // The second source models a plugin path whose pulse already arrives late.
        // Delay the first path by that amount and verify both reach the mixer together.
        let early = source(channel: 0, pulse: pulseFrame)
        let latent = source(channel: 1, pulse: pulseFrame + compensationFrames)
        for node in [early, latent, delay.node] as [AVAudioNode] { engine.attach(node) }
        engine.connect(early, to: delay.node, format: format)
        engine.connect(delay.node, to: engine.mainMixerNode, fromBus: 0, toBus: 0, format: format)
        engine.connect(latent, to: engine.mainMixerNode, fromBus: 0, toBus: 1, format: format)
        try engine.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 512)
        try engine.start()
        defer { engine.stop(); engine.disableManualRenderingMode() }
        let output = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 512)!
        var pulses: [[Int]] = [[], []]
        var renderedFrames = 0, attempts = 0
        while renderedFrames < 6144 && attempts < 100 {
            attempts += 1
            let status = try engine.renderOffline(512, to: output)
            switch status {
            case .success:
                try require(output.frameLength > 0, "Offline graph makes forward progress")
                for channel in 0..<2 {
                    for frame in 0..<Int(output.frameLength) {
                        let value = output.floatChannelData![channel][frame]
                        try require(value.isFinite, "Offline graph emits finite audio")
                        if abs(value) > 0.1 { pulses[channel].append(renderedFrames + frame) }
                    }
                }
                renderedFrames += Int(output.frameLength)
            case .cannotDoInCurrentContext:
                continue
            case .insufficientDataFromInputNode, .error:
                throw NSError(domain: "AudioDelayTests.OfflineGraph", code: Int(status.rawValue),
                              userInfo: [NSLocalizedDescriptionKey: "Offline graph could not render its connected sources"])
            @unknown default:
                throw NSError(domain: "AudioDelayTests.OfflineGraph", code: -1)
            }
        }
        try require(renderedFrames >= 6144, "Offline graph completes within a bounded number of attempts")
        let expected = [pulseFrame + compensationFrames]
        try require(pulses[0] == expected && pulses[1] == expected,
                    "Compensated paths align exactly: left \(pulses[0]), right \(pulses[1]), expected \(expected)")
        try require(delay.node.auAudioUnit.latency == 0, "Connected compensation AU still reports zero latency")
        print("PASS real offline AVAudioEngine parallel paths align at frame \(expected[0])")
    }

    static func appleDelayGraph() throws {
        struct Pull {
            var timestamp: Int = 0
            var frames: Int = 0
        }
        let compensationFrames = 1537
        let renderFrames = 4096
        let renderBlocks = 8
        let pullCapacity = 128

        func render(wetDryMix: Float, compensate: Bool) throws -> (audio: [[Float]], pulls: [Pull]) {
            let engine = AVAudioEngine()
            let format = AVAudioFormat(standardFormatWithSampleRate: testRate, channels: 2)!
            let pulls = UnsafeMutablePointer<Pull>.allocate(capacity: pullCapacity)
            pulls.initialize(repeating: Pull(), count: pullCapacity)
            defer { pulls.deinitialize(count: pullCapacity); pulls.deallocate() }
            var pullCount = 0
            let sourceOffset = compensate ? 0 : compensationFrames
            let source = AVAudioSourceNode(format: format) { _, timestamp, count, list in
                guard pullCount < pullCapacity else { return kAudio_ParamError }
                let sampleTime = Int(timestamp.pointee.mSampleTime)
                pulls[pullCount] = Pull(timestamp: sampleTime, frames: Int(count))
                pullCount += 1
                for (channel, buffer) in UnsafeMutableAudioBufferListPointer(list).enumerated() {
                    guard let data = buffer.mData?.assumingMemoryBound(to: Float.self) else {
                        return kAudio_ParamError
                    }
                    for frame in 0..<Int(count) {
                        let position = sampleTime + frame - sourceOffset
                        guard position >= 0 else { data[frame] = 0; continue }
                        let sequence = channel == 0 ? Float((position % 509) - 254) / 2048 :
                                                      Float((position % 127) - 63) / 512
                        let isPulse = (4095...4097).contains(position) || (8191...8192).contains(position)
                        let pulse: Float = isPulse ? 0.5 : 0
                        data[frame] = sequence + (channel == 0 ? pulse : -pulse)
                    }
                }
                return noErr
            }
            let nativeDelay = AVAudioUnitEffect(audioComponentDescription: AudioComponentDescription(
                componentType: kAudioUnitType_Effect, componentSubType: kAudioUnitSubType_Delay,
                componentManufacturer: kAudioUnitManufacturer_Apple,
                componentFlags: 0, componentFlagsMask: 0))
            for (address, value): (AudioUnitParameterID, Float) in [
                (kDelayParam_DelayTime, 0.2), (kDelayParam_Feedback, 0),
                (kDelayParam_LopassCutoff, 20_000), (kDelayParam_WetDryMix, wetDryMix)] {
                guard let parameter = nativeDelay.auAudioUnit.parameterTree?.parameter(withAddress: AUParameterAddress(address)) else {
                    throw NSError(domain: "AudioDelayTests.AppleDelay", code: -1,
                                  userInfo: [NSLocalizedDescriptionKey: "Apple AUDelay parameter is unavailable"])
                }
                parameter.value = value
            }
            try nativeDelay.auAudioUnit.inputBusses[0].setFormat(format)
            try nativeDelay.auAudioUnit.outputBusses[0].setFormat(format)
            let compensation = try AudioDelayCompensator(sampleRate: testRate, maximumFrames: 8192)
            compensation.frames = compensationFrames
            compensation.reset()
            let inputMixer = AVAudioMixerNode()
            let converterMixer = AVAudioMixerNode()
            let outputMixer = AVAudioMixerNode()
            var siblingPulls: [Pull] = []
            let nativeReturn = AVAudioSourceNode(format: format) { _, timestamp, count, list in
                siblingPulls.append(Pull(timestamp: Int(timestamp.pointee.mSampleTime), frames: Int(count)))
                for buffer in UnsafeMutableAudioBufferListPointer(list) {
                    guard let data = buffer.mData?.assumingMemoryBound(to: Float.self) else {
                        return kAudio_ParamError
                    }
                    for frame in 0..<Int(count) { data[frame] = 0 }
                }
                return noErr
            }
            engine.attach(source)
            engine.attach(nativeDelay)
            engine.attach(inputMixer)
            engine.attach(converterMixer)
            engine.attach(outputMixer)
            engine.attach(nativeReturn)
            if compensate {
                engine.attach(compensation.node)
                engine.connect(source, to: compensation.node, format: format)
                engine.connect(compensation.node, to: inputMixer, format: format)
            } else {
                // Independent reference: bake the same delay into the source itself.
                engine.connect(source, to: inputMixer, format: format)
            }
            engine.connect(inputMixer, to: nativeDelay, format: nativeDelay.inputFormat(forBus: 0))
            engine.connect(nativeDelay, to: converterMixer, format: nativeDelay.outputFormat(forBus: 0))
            engine.connect(converterMixer, to: outputMixer, format: format)
            engine.connect(outputMixer, to: engine.mainMixerNode, fromBus: 0, toBus: 0, format: format)
            engine.connect(nativeReturn, to: engine.mainMixerNode, fromBus: 0, toBus: 1, format: format)
            try engine.enableManualRenderingMode(.offline, format: format,
                                                 maximumFrameCount: AVAudioFrameCount(renderFrames))
            engine.reset()
            try engine.start()
            defer { engine.stop(); engine.disableManualRenderingMode() }
            let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(renderFrames))!
            var audio: [[Float]] = [[], []]
            var completed = 0, attempts = 0
            while completed < renderBlocks && attempts < 100 {
                attempts += 1
                let status = try engine.renderOffline(AVAudioFrameCount(renderFrames), to: buffer)
                switch status {
                case .success:
                    try require(buffer.frameLength == AVAudioFrameCount(renderFrames),
                                "Apple delay renders a complete 4096-frame output block")
                    for channel in 0..<2 {
                        audio[channel].append(contentsOf: UnsafeBufferPointer(start: buffer.floatChannelData![channel],
                                                                            count: renderFrames))
                    }
                    completed += 1
                case .cannotDoInCurrentContext:
                    continue
                case .insufficientDataFromInputNode, .error:
                    throw NSError(domain: "AudioDelayTests.AppleDelay", code: Int(status.rawValue),
                                  userInfo: [NSLocalizedDescriptionKey: "Apple delay graph failed to render"])
                @unknown default:
                    throw NSError(domain: "AudioDelayTests.AppleDelay", code: -1)
                }
            }
            try require(completed == renderBlocks, "Apple delay graph completes its bounded render")
            try require(siblingPulls.count == renderBlocks && siblingPulls.allSatisfy { $0.frames == renderFrames },
                        "Parallel native return renders each nominal 4096-frame block")
            return (audio, Array(UnsafeBufferPointer(start: pulls, count: pullCount)))
        }

        for wetDryMix: Float in [100, 50] {
            let actual = try render(wetDryMix: wetDryMix, compensate: true)
            let reference = try render(wetDryMix: wetDryMix, compensate: false)
            let summary = actual.pulls.map { "\($0.frames)@\($0.timestamp)" }.joined(separator: ", ")
            // Apple's upstream quantum depends on graph conversion and OS version.
            // The public-renderBlock test above deterministically forces 4097/8192;
            // this graph captures whatever the real system effect requests and
            // checks its PCM against an independent source-delayed reference.
            try require(!actual.pulls.isEmpty && actual.pulls.allSatisfy { $0.frames > 0 },
                        "Real Apple AUDelay pulls its upstream source; observed [\(summary)]")
            for index in actual.pulls.indices.dropFirst() {
                let previous = actual.pulls[index - 1]
                try require(actual.pulls[index].timestamp == previous.timestamp + previous.frames,
                            "Apple upstream pulls preserve continuous sample times across oversized blocks")
            }
            for channel in 0..<2 {
                try require(actual.audio[channel].contains { abs($0) > 0.01 },
                            "Apple delay emits audible signal at wet/dry \(wetDryMix)")
                try require(actual.audio[channel].count == reference.audio[channel].count,
                            "Apple delay and independent delayed-source reference lengths agree")
                for index in actual.audio[channel].indices {
                    let value = actual.audio[channel][index]
                    try require(value.isFinite && abs(value - reference.audio[channel][index]) <= tolerance,
                                "Apple delay matches source-delayed reference: wet/dry \(wetDryMix), channel \(channel), frame \(index)")
                }
            }
            print("PASS Apple AUDelay wet/dry \(Int(wetDryMix)) offline4096, observed upstream [\(summary)]; exact 1537-frame compensation against reference")
        }
    }

    /// Zero delay renders as a block pass-through that still records history: a later
    /// change crossfades from the real past samples, matching a per-sample reference.
    static func zeroDelayPassThroughKeepsHistory() throws {
        let fixture = try DelayRenderFixture(frames: 0, maximum: 1024)
        let count = 3000
        let left = (0..<count).map { Float($0 % 97) / 97 - 0.5 }, right = (0..<count).map { 0.25 - Float($0 % 31) / 62 }
        let first = try fixture.render(Array(left[..<1500]), Array(right[..<1500]))
        try require(first.0 == Array(left[..<1500]) && first.1 == Array(right[..<1500]), "Zero delay passes PCM through exactly")
        fixture.delay.frames = 300
        let second = try fixture.render(Array(left[1500...]), Array(right[1500...]))
        for (channel, input, output) in [(0, left, second.0), (1, right, second.1)] {
            for index in output.indices {
                let frame = 1500 + index, old = input[frame]
                let new: Float = frame >= 300 ? input[frame - 300] : 0
                let expected = index < 256 ? old + (new - old) * (Float(index + 1) / 256) : new
                try require(output[index] == expected, "Crossfade from zero-delay history, channel \(channel), frame \(frame): \(output[index]) != \(expected)")
            }
        }
        print("PASS zero-delay pass-through records history for exact later crossfades")
    }

    @MainActor static func main() throws {
        setbuf(stdout, nil)
        try exactTiming()
        try boundsAndReset()
        try nilOutputBuffers()
        try largeRenderQuanta()
        try smoothChanges()
        try zeroDelayPassThroughKeepsHistory()
        try offlineGraph()
        try appleDelayGraph()
        print("PASS all AudioDelayCompensator unit and offline graph tests")
    }
}

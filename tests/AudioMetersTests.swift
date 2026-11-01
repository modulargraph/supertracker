import Foundation
import AVFoundation

// Standalone: swiftc Sources/SuperTracker/AudioMeters.swift tests/AudioMetersTests.swift -o /tmp/audio-meters-tests
@main
struct AudioMetersTests {
    static func near(_ actual: Float, _ expected: Float, _ message: String) {
        precondition(abs(actual - expected) < 0.00001, "\(message): \(actual) != \(expected)")
    }

    static func buffer(_ channels: AVAudioChannelCount, interleaved: Bool = false) -> AVAudioPCMBuffer {
        let format: AVAudioFormat
        if channels == 4 {
            format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 48_000,
                                   interleaved: interleaved,
                                   channelLayout: AVAudioChannelLayout(layoutTag: kAudioChannelLayoutTag_Quadraphonic)!)
        } else {
            format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 48_000,
                                   channels: channels, interleaved: interleaved)!
        }
        let result = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 64)!
        result.frameLength = 64
        for audio in UnsafeMutableAudioBufferListPointer(result.mutableAudioBufferList) {
            memset(audio.mData!, 0, Int(audio.mDataByteSize))
        }
        return result
    }

    static func set(_ buffer: AVAudioPCMBuffer, channel: Int, frame: Int = 5, value: Float) {
        if buffer.format.isInterleaved {
            buffer.floatChannelData![0][frame * Int(buffer.format.channelCount) + channel] = value
        } else {
            buffer.floatChannelData![channel][frame] = value
        }
    }

    static func main() {
        let meters = AudioMeters()
        let stereo = buffer(2)
        set(stereo, channel: 0, value: -0.7)
        set(stereo, channel: 1, value: 1.2)
        meters.measure(stereo, bus: 16)
        let silence = buffer(2)
        for _ in 0..<100 { meters.measure(silence, bus: 16) }
        let peak = meters.snapshot()
        near(peak.left, 0.7, "Transient retained between snapshots")
        near(peak.right, 1.2, "Clipping peak retained")
        precondition(peak.tracks.count == 16, "Input does not add a track meter")
        let decay = meters.snapshot()
        near(decay.left, 0.7 * 0.88, "Master decay")

        meters.reset()
        let mono = buffer(1)
        set(mono, channel: 0, value: -0.3)
        meters.measureInput(mono, channels: [0])
        let monoPeak = meters.snapshot()
        near(monoPeak.inputLeft, 0.3, "Mono input left")
        near(monoPeak.inputRight, 0.3, "Mono input mirrored right")
        near(monoPeak.left, 0, "Input does not affect master")
        precondition(monoPeak.tracks.allSatisfy { $0 == 0 }, "Input does not affect tracks")
        let inputDecay = meters.snapshot()
        near(inputDecay.inputLeft, 0.3 * 0.88, "Input decay")
        near(inputDecay.inputRight, 0.3 * 0.88, "Mirrored input decay")

        for interleaved in [false, true] {
            meters.reset()
            let quad = buffer(4, interleaved: interleaved)
            set(quad, channel: 0, value: 1.5)
            set(quad, channel: 1, value: -0.4)
            set(quad, channel: 2, value: -0.9)
            set(quad, channel: 3, value: 0.8)
            set(quad, channel: 1, frame: 0, value: .infinity)
            set(quad, channel: 3, frame: 0, value: .nan)
            meters.measureInput(quad, channels: [3, 1])
            let selected = meters.snapshot()
            near(selected.inputLeft, 0.8, "Selected channel 3, interleaved \(interleaved)")
            near(selected.inputRight, 0.4, "Selected channel 1; nonfinite samples ignored")
            meters.resetInput()
            meters.measureInput(quad, channels: [2])
            let selectedMono = meters.snapshot()
            near(selectedMono.inputLeft, 0.9, "One channel of multichannel input")
            near(selectedMono.inputRight, 0.9, "Selected mono channel mirrored")

            meters.resetInput()
            meters.measure(quad, bus: 16)
            let folded = meters.snapshot()
            near(folded.left, 1.5, "Existing multichannel folding left")
            near(folded.right, 0.8, "Existing multichannel folding right")
        }

        meters.reset()
        meters.measure(stereo, bus: 15)
        meters.measure(stereo, bus: 16)
        meters.measureInput(stereo, channels: [0, 1])
        meters.resetInput()
        let inputReset = meters.snapshot()
        near(inputReset.inputLeft, 0, "Reset clears pending input left")
        near(inputReset.inputRight, 0, "Reset clears pending input right")
        near(inputReset.tracks[15], 1.2, "Input reset retains pending track")
        near(inputReset.left, 0.7, "Input reset retains pending master")
        meters.measureInput(stereo, channels: [0, 1])
        _ = meters.snapshot()
        meters.resetInput()
        let displayReset = meters.snapshot()
        near(displayReset.inputLeft, 0, "Reset clears displayed input")
        precondition(displayReset.left > 0 && displayReset.tracks[15] > 0,
                     "Input reset retains displayed master/track")

        meters.reset()
        meters.measureInput(stereo, channels: [])
        meters.measureInput(stereo, channels: [-1, 99])
        meters.measure(stereo, bus: -1)
        meters.measure(stereo, bus: 17)
        let invalid = meters.snapshot()
        near(invalid.inputLeft, 0, "Invalid input selection is silent")
        near(invalid.inputRight, 0, "Invalid bus cannot write input slot")
        near(invalid.left, 0, "Invalid bus cannot write master")
        print("PASS AudioMeters: track/master compatibility, input selection, mono mirroring, planar/interleaved multichannel, transient hold, decay, nonfinite samples, clipping, isolated reset and invalid selections")
    }
}

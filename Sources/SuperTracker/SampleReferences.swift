import Foundation
import TrackerEngine

/// Installs the app side of sample decoding: built-in sample references
/// (BuiltinSamples, rendered drums included) and wavetable frame-size detection.
/// AudioSamples and BuiltinSamples are pure Swift and also compiled on their own
/// by tests, so they reach the engine and WavetableFormat only through these hooks.
enum SampleReferences {
    static let drumSampleRate = 48_000.0
    /// The longest drum decay is 12 s; one more second of headroom.
    static let maximumDrumFrames = Int(drumSampleRate * 13)

    static func install() {
        guard BuiltinSamples.drumRenderer == nil else { return }
        BuiltinSamples.drumRenderer = { model, parameters in renderDrum(model: model, parameters: parameters) }
        AudioSamples.referenceResolver = { try BuiltinSamples.resolve($0) }
        AudioSamples.cycleDetector = { file, frames in
            WavetableFormat.detectFrameSize(declared: WavetableFormat.declaredFrameSize(atPath: file), frames: frames)
        }
    }

    static func renderDrum(model: Int, parameters: [Float]) -> [Float] {
        var output = [Float](repeating: 0, count: maximumDrumFrames)
        let frames = parameters.withUnsafeBufferPointer { values in
            output.withUnsafeMutableBufferPointer { out in
                st_drum_render(Int32(model), values.baseAddress, UInt32(values.count), drumSampleRate, out.baseAddress, UInt32(out.count))
            }
        }
        return Array(output.prefix(Int(frames)))
    }
}

// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation
import AudioToolbox

/// Original fixture used only by the hidden duplicate-track acceptance test.
enum NativeUIDuplicateFixture {
    static let address: AUParameterAddress = 42
    private static let subtype: OSType = 0x64706566 // dpef
    private static let manufacturer: OSType = 0x53745473 // StTs
    static let description = AudioComponentDescription(componentType: kAudioUnitType_Effect,
        componentSubType: subtype, componentManufacturer: manufacturer, componentFlags: 0, componentFlagsMask: 0)
    static let identifier = "\(kAudioUnitType_Effect):\(subtype):\(manufacturer)"
    private static let registration: Void = {
        AUAudioUnit.registerSubclass(NativeUIDuplicateEffect.self, as: description,
            name: "SuperTracker Test: Duplicate opaque-state effect", version: 1)
    }()
    @MainActor static func register() async throws {
        _ = registration
        // ComponentManager can refresh on a later main-loop turn after the
        // synchronous in-process registration. Never scan installed vendors.
        let deadline = ContinuousClock.now.advanced(by: .seconds(3))
        while AVAudioUnitComponentManager.shared().components(matching: description).isEmpty {
            guard ContinuousClock.now < deadline else { throw TrackerError.message("Duplicate fixture effect registration was not discovered.") }
            try await Task.sleep(for: .milliseconds(10))
        }
    }
}

private final class NativeUIDuplicateEffect: AUAudioUnit {
    private var inputs: AUAudioUnitBusArray!
    private var outputs: AUAudioUnitBusArray!
    private var parameters: AUParameterTree!
    private var gain: AUValue = 0.5
    private var contents: [String: Any] = ["originalFixture": "Duplicate typed effect",
        "gain": Double(0.5), "opaque": Data([0, 1, 255, 0, 42]),
        "typed": ["enabled": true, "index": 7, "name": "Do not rewrite vendor data"] as [String: Any]]

    override init(componentDescription: AudioComponentDescription, options: AudioComponentInstantiationOptions = []) throws {
        try super.init(componentDescription: componentDescription, options: options)
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        let input = try AUAudioUnitBus(format: format), output = try AUAudioUnitBus(format: format)
        input.supportedChannelCounts = [2]; output.supportedChannelCounts = [2]
        inputs = AUAudioUnitBusArray(audioUnit: self, busType: .input, busses: [input])
        outputs = AUAudioUnitBusArray(audioUnit: self, busType: .output, busses: [output])
        let level = AUParameterTree.createParameter(withIdentifier: "duplicateEffectGain", name: "Original effect gain",
            address: NativeUIDuplicateFixture.address, min: 0, max: 1, unit: .linearGain, unitName: nil,
            flags: [.flag_IsReadable, .flag_IsWritable], valueStrings: nil, dependentParameters: nil)
        parameters = AUParameterTree.createTree(withChildren: [level])
        parameters.implementorValueObserver = { [weak self] _, value in self?.gain = value; self?.contents["gain"] = Double(value) }
        parameters.implementorValueProvider = { [weak self] _ in self?.gain ?? 0.5 }
        level.value = 0.5; maximumFramesToRender = 16_384
    }
    override var inputBusses: AUAudioUnitBusArray { inputs }
    override var outputBusses: AUAudioUnitBusArray { outputs }
    override var channelCapabilities: [NSNumber]? { [2, 2] }
    override var parameterTree: AUParameterTree? { get { parameters } set { parameters = newValue } }
    override var fullState: [String: Any]? {
        get { contents }
        set {
            guard let value = newValue, let level = value["gain"] as? NSNumber else { return }
            contents = value; parameters.parameter(withAddress: NativeUIDuplicateFixture.address)?.value = AUValue(level.doubleValue)
        }
    }
    override var fullStateForDocument: [String: Any]? { get { fullState } set { fullState = newValue } }
    override func shouldChange(to format: AVAudioFormat, for bus: AUAudioUnitBus) -> Bool {
        !renderResourcesAllocated && format.channelCount == 2 && format.commonFormat == .pcmFormatFloat32 && !format.isInterleaved
    }
    override var internalRenderBlock: AUInternalRenderBlock {
        { [unowned self] flags, timestamp, frames, bus, output, _, pull in
            guard bus == 0 else { return kAudioUnitErr_InvalidElement }
            guard let pull else { return kAudioUnitErr_NoConnection }
            let result = pull(flags, timestamp, frames, 0, output)
            guard result == noErr else { return result }
            for buffer in UnsafeMutableAudioBufferListPointer(output) {
                guard let samples = buffer.mData?.assumingMemoryBound(to: Float.self),
                      buffer.mDataByteSize >= frames * UInt32(MemoryLayout<Float>.size) else { return kAudio_ParamError }
                for frame in 0..<Int(frames) { samples[frame] *= gain }
            }
            return noErr
        }
    }
}

#endif

import AVFoundation
import AudioToolbox
#if canImport(AURenderShim)
// The app and the unit tests link the Objective-C render shim: render blocks pass the
// host's pull block through without allocating (a Swift internalRenderBlock bridges it
// into a new heap closure on every render call of every node).
import AURenderShim
#else
// Standalone diagnostic builds without the shim module: the same API in Swift. It
// allocates per render call, which only matters for real-time use.
typealias STAURenderFunction = @convention(c) (UnsafeMutableRawPointer?, UnsafeMutablePointer<AudioUnitRenderActionFlags>,
    UnsafePointer<AudioTimeStamp>, AUAudioFrameCount, Int, UnsafeMutablePointer<AudioBufferList>,
    UnsafePointer<AURenderEvent>?, UnsafeRawPointer?) -> AUAudioUnitStatus

private final class AURenderPullBox {
    let pull: AURenderPullInputBlock
    init(_ pull: @escaping AURenderPullInputBlock) { self.pull = pull }
}

class STAURenderUnit: AUAudioUnit {
    private var function: STAURenderFunction?
    private var context: UnsafeMutableRawPointer?
    func setRenderFunction(_ function: STAURenderFunction, context: UnsafeMutableRawPointer?) {
        self.function = function; self.context = context
    }
    /// The Swift fallback always renders through AUAudioUnit's public render block.
    func setRendersDirectly(_ direct: Bool) {}
    override var internalRenderBlock: AUInternalRenderBlock {
        let function = self.function, context = self.context
        return { flags, timestamp, frames, bus, output, events, pull in
            guard let function else { return kAudioUnitErr_Uninitialized }
            guard let pull else { return function(context, flags, timestamp, frames, bus, output, events, nil) }
            let box = AURenderPullBox(pull)
            return withExtendedLifetime(box) {
                function(context, flags, timestamp, frames, bus, output, events, UnsafeRawPointer(Unmanaged.passUnretained(box).toOpaque()))
            }
        }
    }
}

func st_au_pull(_ pull: UnsafeRawPointer?, _ flags: UnsafeMutablePointer<AudioUnitRenderActionFlags>,
                _ timestamp: UnsafePointer<AudioTimeStamp>, _ frames: AUAudioFrameCount, _ bus: Int,
                _ data: UnsafeMutablePointer<AudioBufferList>) -> AUAudioUnitStatus {
    guard let pull else { return kAudioUnitErr_NoConnection }
    return Unmanaged<AURenderPullBox>.fromOpaque(pull).takeUnretainedValue().pull(flags, timestamp, frames, bus, data)
}
#endif

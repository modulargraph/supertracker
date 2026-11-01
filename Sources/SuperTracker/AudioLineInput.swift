import Accelerate
import AVFoundation
import AudioToolbox
#if canImport(AURenderShim)
import AURenderShim
#endif

/// One immutable mono/stereo selection from the shared, unmapped hardware stream.
/// Selection happens before the track inserts; a mono input is duplicated to stereo.
final class AudioLineInput {
    let node: AVAudioUnitEffect
    private static let component = AudioComponentDescription(componentType:kAudioUnitType_Effect,
        componentSubType:0x73746C69,componentManufacturer:0x53705472,componentFlags:0,componentFlagsMask:0)
    private static let registration:Void = {
        AUAudioUnit.registerSubclass(LineInputUnit.self,as:component,name:"SuperTracker: Line Input",version:1)
    }()
    init(format:AVAudioFormat,channels:[Int],gain:Float = 1) throws {
        guard (1...2).contains(channels.count),channels.allSatisfy({$0 >= 0 && $0 < Int(format.channelCount)}),
              gain.isFinite,(0...2).contains(gain),format.channelCount <= 128 else {
            throw AudioSampleError.invalid("Choose one or two available line-input channels and a gain from 0 to 2.")
        }
        _ = Self.registration
        node = AVAudioUnitEffect(audioComponentDescription:Self.component)
        guard let unit = node.auAudioUnit as? LineInputUnit else { throw AudioSampleError.invalid("Cannot create a line-input channel selector.") }
        unit.channels=channels;unit.gain=gain
        try unit.inputBusses[0].setFormat(format)
        try unit.outputBusses[0].setFormat(AVAudioFormat(standardFormatWithSampleRate:format.sampleRate,channels:2)!)
    }
}

/// Render-thread view of the allocated selector: plain values and raw pointers only,
/// so the render block performs no retain/release (no class, array or buffer object).
private struct LineInputRender {
    var list:UnsafeMutablePointer<AudioBufferList>?=nil
    var storage:UnsafeMutablePointer<Float>?=nil
    var stereo:UnsafeMutablePointer<Float>?=nil
    var inputChannels=0,first=0,second=1
    var capacity:UInt32=0
    var gain:Float=1
}

private final class LineInputUnit:STAURenderUnit {
    var channels=[0,1]
    var gain:Float=1
    private var inputs:AUAudioUnitBusArray!
    private var outputs:AUAudioUnitBusArray!
    private let render=UnsafeMutablePointer<LineInputRender>.allocate(capacity:1)
    override init(componentDescription:AudioComponentDescription,options:AudioComponentInstantiationOptions=[]) throws {
        render.initialize(to:LineInputRender())
        try super.init(componentDescription:componentDescription,options:options)
        let format=AVAudioFormat(standardFormatWithSampleRate:48_000,channels:2)!
        inputs=AUAudioUnitBusArray(audioUnit:self,busType:.input,busses:[try AUAudioUnitBus(format:format)])
        outputs=AUAudioUnitBusArray(audioUnit:self,busType:.output,busses:[try AUAudioUnitBus(format:format)])
        maximumFramesToRender=16_384
        setRenderFunction(Self.renderInput,context:UnsafeMutableRawPointer(render))
    }
    deinit { releaseRenderStorage(); render.deinitialize(count:1); render.deallocate() }
    override var inputBusses:AUAudioUnitBusArray { inputs }
    override var outputBusses:AUAudioUnitBusArray { outputs }
    override var channelCapabilities:[NSNumber]? { [-1,2] }
    override var canProcessInPlace:Bool { false }
    override func shouldChange(to format:AVAudioFormat,for bus:AUAudioUnitBus)->Bool {
        !renderResourcesAllocated && format.commonFormat == .pcmFormatFloat32 && !format.isInterleaved &&
        format.channelCount > 0 && format.channelCount <= 128 && (bus === inputs[0] || format.channelCount == 2)
    }
    override func allocateRenderResources() throws {
        let count=Int(inputs[0].format.channelCount),capacity=Int(maximumFramesToRender)
        guard inputs[0].format.sampleRate == outputs[0].format.sampleRate,(1...2).contains(channels.count),
              channels.allSatisfy({$0 >= 0 && $0 < count}),capacity > 0 else {
            throw AudioSampleError.invalid("The line-input format changed. Select available channels again.")
        }
        releaseRenderStorage()
        let storage=UnsafeMutablePointer<Float>.allocate(capacity:count*capacity),stereo=UnsafeMutablePointer<Float>.allocate(capacity:2*capacity)
        storage.initialize(repeating:0,count:count*capacity); stereo.initialize(repeating:0,count:2*capacity)
        let list=AudioBufferList.allocate(maximumBuffers:count)
        render.pointee=LineInputRender(list:list.unsafeMutablePointer,storage:storage,stereo:stereo,inputChannels:count,
            first:channels[0],second:channels[min(1,channels.count-1)],capacity:UInt32(capacity),gain:gain)
        try super.allocateRenderResources()
    }
    override func deallocateRenderResources() { super.deallocateRenderResources(); releaseRenderStorage() }
    private func releaseRenderStorage() {
        let value=render.pointee
        if let list=value.list { free(list) }
        if let storage=value.storage { storage.deinitialize(count:value.inputChannels*Int(value.capacity)); storage.deallocate() }
        if let stereo=value.stereo { stereo.deinitialize(count:2*Int(value.capacity)); stereo.deallocate() }
        render.pointee=LineInputRender()
    }
    /// The render callback (see STAURenderUnit); `context` is this unit's LineInputRender.
    private static let renderInput:STAURenderFunction = { context,flags,time,frames,bus,output,_,pull in
            guard let context else { return kAudioUnitErr_Uninitialized }
            let state=context.assumingMemoryBound(to:LineInputRender.self).pointee
            guard bus == 0,let pull,let list=state.list,let storage=state.storage,let stereo=state.stereo,
                  frames <= state.capacity else { return kAudioUnitErr_TooManyFramesToProcess }
            let source=UnsafeMutableAudioBufferListPointer(list)
            for channel in 0..<state.inputChannels {
                source[channel]=AudioBuffer(mNumberChannels:1,mDataByteSize:frames*4,
                    mData:UnsafeMutableRawPointer(storage.advanced(by:channel*Int(state.capacity))))
            }
            let status=st_au_pull(pull,flags,time,frames,0,list)
            guard status == noErr else { return status }
            let destination=UnsafeMutableAudioBufferListPointer(output)
            guard destination.count == 2,source.count == state.inputChannels else { return kAudio_ParamError }
            for channel in 0..<2 {
                guard let input=source[channel == 0 ? state.first : state.second].mData?.assumingMemoryBound(to:Float.self) else { return kAudio_ParamError }
                if destination[channel].mData == nil {
                    // AU clients may request owned output pointers; keep stereo storage
                    // separate so mono duplication never applies gain twice.
                    destination[channel].mData=UnsafeMutableRawPointer(stereo.advanced(by:channel*Int(state.capacity)))
                    destination[channel].mDataByteSize=frames*4
                }
                guard destination[channel].mDataByteSize >= frames*4,let target=destination[channel].mData?.assumingMemoryBound(to:Float.self) else { return kAudio_ParamError }
                var gain=state.gain
                vDSP_vsmul(input,1,&gain,target,1,vDSP_Length(frames))
                destination[channel].mDataByteSize=frames*4
            }
            return noErr
    }
}

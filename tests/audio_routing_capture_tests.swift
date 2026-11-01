// Standalone original-pulse capture tests. No vendors, GUI, or audio-device I/O.
// From the repository root:
// swiftc -swift-version 5 -target arm64-apple-macosx14.0 \
//   -module-cache-path /private/tmp/supertracker-routing-module-cache \
//   Sources/SuperTracker/AudioRoutingCapture.swift tests/audio_routing_capture_tests.swift \
//   -o /private/tmp/supertracker-audio-routing-capture-tests
// /private/tmp/supertracker-audio-routing-capture-tests
// Apple's in-process AU registration may require running outside the filesystem sandbox.
import AVFoundation
import AudioToolbox
import Foundation

@main struct CaptureTests {
    static var checks = 0
    static func need(_ condition: @autoclosure () -> Bool, _ message: String) throws {
        if !condition() { throw NSError(domain:"CaptureTests",code:1,userInfo:[NSLocalizedDescriptionKey:message]) }; checks += 1
    }
    static func sample(_ frame:Int64)->Float { frame % 97 == 0 ? 0.5 : 0 }
    static func main() throws {
        let format = AVAudioFormat(standardFormatWithSampleRate:48000,channels:2)!
        let capture = try AudioRoutingCapture(sampleRate:48000,capacityFrames:131072)
        try capture.node.auAudioUnit.allocateRenderResources()
        defer { capture.node.auAudioUnit.deallocateRenderResources() }
        let storage = UnsafeMutableRawPointer.allocate(byteCount:MemoryLayout<AudioBufferList>.size+MemoryLayout<AudioBuffer>.stride,alignment:MemoryLayout<AudioBufferList>.alignment)
        defer { storage.deallocate() }
        storage.initializeMemory(as:UInt8.self,repeating:0,count:MemoryLayout<AudioBufferList>.size+MemoryLayout<AudioBuffer>.stride)
        let list=storage.assumingMemoryBound(to:AudioBufferList.self); list.pointee.mNumberBuffers=2
        let buffers=UnsafeMutableAudioBufferListPointer(list)
        let outputL=UnsafeMutablePointer<Float>.allocate(capacity:65536),outputR=UnsafeMutablePointer<Float>.allocate(capacity:65536)
        outputL.initialize(repeating:0,count:65536);outputR.initialize(repeating:0,count:65536)
        defer { outputL.deinitialize(count:65536);outputL.deallocate();outputR.deinitialize(count:65536);outputR.deallocate() }
        func render(_ start:Double,_ count:Int,nilOutput:Bool=false,silent:Bool=false,status:OSStatus=noErr,mutateTimestamp:Bool=false,validTimeFlags:Bool=true)throws {
            for channel in 0..<2 { buffers[channel]=AudioBuffer(mNumberChannels:1,mDataByteSize:UInt32(count*4),mData:nilOutput ? nil : UnsafeMutableRawPointer(channel==0 ? outputL:outputR)) }
            var timestamp=AudioTimeStamp();timestamp.mSampleTime=start;timestamp.mFlags = validTimeFlags ? .sampleTimeValid : []
            var flags=AudioUnitRenderActionFlags()
            let result=capture.node.auAudioUnit.internalRenderBlock(&flags,&timestamp,UInt32(count),0,list,nil,{ actions,time,frames,bus,data in
                if status != noErr { return status }
                if silent { actions.pointee.insert(.unitRenderAction_OutputIsSilence) }
                let data=UnsafeMutableAudioBufferListPointer(data)
                for channel in 0..<2 { let target=data[channel].mData!.assumingMemoryBound(to:Float.self);for index in 0..<Int(frames) { target[index]=Self.sample(Int64(start.isFinite ? start : 0)+Int64(index))*(channel==0 ? 1 : -1) } }
                if mutateTimestamp { let mutable=UnsafeMutablePointer(mutating:time);mutable.pointee.mSampleTime=0;mutable.pointee.mFlags=[] }
                return noErr
            })
            try need(result==status,"Pass-through status changed")
            if result==noErr { for channel in 0..<2 { let values=buffers[channel].mData!.assumingMemoryBound(to:Float.self);for index in 0..<count { try need(values[index]==sample(Int64(start.isFinite ? start:0)+Int64(index))*(channel==0 ? 1 : -1),"Pass-through PCM changed") } } }
        }
        let drain=AVAudioPCMBuffer(pcmFormat:format,frameCapacity:127)!
        func collect()->([Float],[Float]) { var left:[Float]=[],right:[Float]=[];while capture.availableFrames>0 { let chunk=try! capture.drain(into:drain);precondition(chunk.frameCount<=127);left += Array(UnsafeBufferPointer(start:drain.floatChannelData![0],count:Int(chunk.frameCount)));right += Array(UnsafeBufferPointer(start:drain.floatChannelData![1],count:Int(chunk.frameCount))) };return(left,right) }
        try capture.configureCapture(enabled:true,arrivalFrames:64,frameLimit:10000)
        try render(0,32,nilOutput:true);try need(capture.availableFrames==0,"Preroll captured")
        try render(32,4097,nilOutput:true)
        try render(4000,129)
        try render(4128,4097)
        try render(8192,2048)
        try need(capture.capturedFrames==10000 && capture.error==nil,"Overlap/limit accounting failed")
        let audio=collect();try need(audio.0.count==10000 && capture.drainedFrames==10000,"Drain length failed")
        for index in 0..<10000 { try need(audio.0[index]==sample(Int64(index+64)) && audio.1[index] == -audio.0[index],"Captured samples lost alignment") }
        try render(4000,64);try need(capture.availableFrames==0,"Old pull duplicated already drained frames")
        let empty=try capture.drain(into:drain);try need(empty.frameCount==0 && drain.frameLength==0 && empty.startFrame==10000,"Empty drain invalid")
        try capture.configureCapture(enabled:true,arrivalFrames:0,startFrame:500,frameLimit:50)
        try render(0,512);try render(512,64);let offset=collect();try need(offset.0.count==50 && offset.0[0]==sample(500),"Start-frame clipping failed")
        try capture.configureCapture(enabled:true,arrivalFrames:0,frameLimit:200000)
        try render(0,65536,nilOutput:true);_ = collect();try render(65536,65536);_ = collect();try render(131072,65536);let wrapped=collect()
        try need(wrapped.0.count==65536 && wrapped.0[0]==sample(131072) && capture.error==nil,"Ring wrap failed")
        try capture.configureCapture(enabled:true,arrivalFrames:0)
        try render(0,65536);try render(65536,65536);try render(131072,1)
        try need(capture.error == .overflow(capacity:131072,pending:131072,incoming:1),"Overflow not recorded")
        do { _ = try capture.drain(into:drain);throw NSError(domain:"ExpectedError",code:1) } catch is AudioRoutingCapture.CaptureError { checks += 1 }
        try capture.configureCapture(enabled:true,arrivalFrames:0)
        try render(0,16);try render(17,16);try need(capture.error == .gap(expected:16,received:17),"Gap not recorded")
        try capture.configureCapture(enabled:true,arrivalFrames:0)
        try render(.nan,1);try need(capture.error == .invalidTimestamp,"Invalid timestamp not recorded")
        try capture.configureCapture(enabled:true,arrivalFrames:0)
        try render(0,16,mutateTimestamp:true);try render(16,16,mutateTimestamp:true)
        let preserved=collect();try need(capture.error==nil && preserved.0.count==32,"Upstream timestamp storage reuse corrupted capture request time")
        for index in 0..<32 { try need(preserved.0[index]==sample(Int64(index)),"Incoming capture time was not preserved before upstream pull") }
        try capture.configureCapture(enabled:true,arrivalFrames:0)
        try render(0.5,1,mutateTimestamp:true)
        try need(capture.error == .invalidTimestamp,"Fractional incoming timestamp was accepted after upstream rewrote it")
        try need((capture.diagnostics["sampleTime"] as? Double)==0.5 && (capture.diagnostics["sampleTimeAfterPull"] as? Double)==0,
                 "Invalid timestamp diagnostics lost before/after evidence")
        try capture.configureCapture(enabled:true,arrivalFrames:0)
        try render(0,1,validTimeFlags:false)
        try need(capture.error == .invalidTimestamp && (capture.diagnostics["timeFlags"] as? UInt32)==0,"Missing incoming validity flags were accepted")
        try capture.configureCapture(enabled:true,arrivalFrames:0)
        try capture.beginRenderQuantum(sampleTime:0,frameCount:16)
        try render(0,17,validTimeFlags:false) // One lookahead frame beyond the master window.
        capture.endRenderQuantum()
        try need(collect().0.count==17 && capture.unflaggedFallbackCount==1,"Independent offline window did not authorize matching lookahead")
        try capture.beginRenderQuantum(sampleTime:16,frameCount:16)
        try render(16,16,validTimeFlags:false);try render(16,16,validTimeFlags:false)
        capture.endRenderQuantum()
        try need(collect().0.count==15 && capture.capturedFrames==32,"Advancing window or same-window deduplication failed")
        try capture.beginRenderQuantum(sampleTime:16,frameCount:16) // Exact master retry after drain.
        try render(16,16,validTimeFlags:false);capture.endRenderQuantum()
        try need(capture.availableFrames==0 && capture.capturedFrames==32 && capture.unflaggedFallbackCount==4,"Retry duplicated already-drained unflagged frames")
        try capture.beginRenderQuantum(sampleTime:32,frameCount:16)
        try render(0,16,validTimeFlags:false);capture.endRenderQuantum()
        try need(capture.error == .invalidTimestamp && (capture.diagnostics["failureWindowSampleTime"] as? Int64)==32 &&
                 (capture.diagnostics["failureWindowActive"] as? Bool)==true,"Always-zero callback clock incorrectly followed progressing master windows")
        for (time,count) in [(0.0,8),(1.0,16),(0.5,16)] {
            try capture.configureCapture(enabled:true,arrivalFrames:0)
            try capture.beginRenderQuantum(sampleTime:0,frameCount:16)
            try render(time,count,validTimeFlags:false);capture.endRenderQuantum()
            try need(capture.error == .invalidTimestamp && capture.unflaggedFallbackCount==0,"Split, offset, or fractional unflagged request was guessed")
        }
        try capture.configureCapture(enabled:true,arrivalFrames:0)
        try capture.beginRenderQuantum(sampleTime:0,frameCount:16);capture.endRenderQuantum()
        for (time,count) in [(17,16),(0,8)] {
            do { try capture.beginRenderQuantum(sampleTime:Int64(time),frameCount:count);throw NSError(domain:"ExpectedWindowError",code:1) }
            catch is AudioRoutingCapture.CaptureError { checks += 1 }
        }
        try capture.configureCapture(enabled:true,arrivalFrames:0)
        try capture.beginRenderQuantum(sampleTime:0,frameCount:16);capture.endRenderQuantum()
        try render(0,16,validTimeFlags:false)
        try need(capture.error == .invalidTimestamp,"Ended master window remained eligible")
        try capture.configureCapture(enabled:true,arrivalFrames:0)
        try render(0,16,silent:true);let silence=collect();try need(silence.0.allSatisfy{$0==0} && silence.1.allSatisfy{$0==0},"Silence flag captured stale PCM")
        try capture.configureCapture(enabled:false,arrivalFrames:0)
        try render(1000,32);try need(capture.availableFrames==0 && capture.error==nil,"Disabled capture recorded data/error")
        try capture.configureCapture(enabled:true,arrivalFrames:0)
        try render(0,1,status:kAudioUnitErr_CannotDoInCurrentContext);try need(capture.error == .upstreamRender(kAudioUnitErr_CannotDoInCurrentContext),"Upstream failure lost")
        print("PASS: \(checks) capture PCM/counter/error assertions; max block 65536, no hardware/vendors")
    }
}

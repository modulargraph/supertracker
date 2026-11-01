import Foundation
import AVFoundation

enum NativeFreezeCheckpointDebugTests {
    @MainActor static func run(output:URL) async throws -> [String:Any] {
        try FileManager.default.createDirectory(at:output,withIntermediateDirectories:true)
        // Discovery is never requested; even catalogue reads stay inside this fixture.
        var vst2Configuration=VST2Catalogue.Configuration()
        vst2Configuration.roots=[];vst2Configuration.cacheURL=output.appendingPathComponent("empty-vst2-cache.json")
        var vst3Configuration=VST3Catalogue.Configuration()
        vst3Configuration.roots=[];vst3Configuration.cacheURL=output.appendingPathComponent("empty-vst3-cache.json")
        let vst2Catalogue=VST2Catalogue(configuration:vst2Configuration),vst3Catalogue=VST3Catalogue(configuration:vst3Configuration)
        let format=AVAudioFormat(standardFormatWithSampleRate:48000,channels:2)!
        let head=output.appendingPathComponent("head.wav"),loop=output.appendingPathComponent("loop.wav")
        func write(_ url:URL,base:Float) throws {
            let file=try AVAudioFile(forWriting:url,settings:AudioExportFormat.float32.settings,commonFormat:.pcmFormatFloat32,interleaved:false)
            let buffer=AVAudioPCMBuffer(pcmFormat:format,frameCapacity:24000)!;buffer.frameLength=24000
            for c in 0..<2 { for frame in 0..<24000 { buffer.floatChannelData![c][frame]=base+Float(frame)/240000 } }
            try file.write(from:buffer)
            if #available(macOS 15.0,*) { file.close() }
        }
        try write(head,base:0.1);try write(loop,base:0.4)
        let record=AudioFreezeTrack.Record(id:"original-print",path:head.path,sha256:"",sampleRate:48000,frames:24000,
            musicalFrames:24000,tailFrames:0,musicalEndBeat:1,tailSeconds:0,dormantPluginIDs:[],fingerprint:"",mixMode:"flat",createdAt:"",
            patternPrints:[AudioFreezeTrack.PatternPrint(pattern:0,path:head.path,sha256:"",loopPath:loop.path,loopSha256:"",frames:24000,passFrames:24000,loopFrames:24000,passes:2)])
        let project:[String:Any]=["id":"checkpoint-fixture","version":1,"name":"Original frozen checkpoint","bpm":120,"linesPerBeat":4,
            "masterVolume":1.0,"activePattern":0,"loopSong":false,
            "tracks":[["name":"Printed","instrument":0,"volume":1.0,"pan":0,"delaySend":0,"reverbSend":0,"frozen":record.dictionary]],
            "instruments":[["type":"synth","volume":0]],
            "patterns":[["id":0,"length":4,"steps":[[NSNull(),NSNull(),NSNull(),NSNull()]]]],"sequence":[0],"audioPlugins":[],
            "delay":["mix":0],"reverb":["mix":0]]
        func capture(prefix:Int,interrupted:Bool,name:String) async throws -> [Float] {
            let graph=AVAudioEngine();try graph.enableManualRenderingMode(.offline,format:format,maximumFrameCount:4096)
            let host=try AudioHost(audioGraph:graph,vst3Catalogue:vst3Catalogue,midiInputIDs:[],midiInputSourceProvider:{[]},vst2Catalogue:vst2Catalogue)
            defer { host.shutdown() }
            _ = try await host.handle("engine.project",payload:["project":project,"replace":true,"awaitPlugins":true])
            _ = try await host.handle("transport.play",payload:["mode":"pattern","fromStart":true])
            let buffer=AVAudioPCMBuffer(pcmFormat:format,frameCapacity:4096)!
            var left=prefix
            while left>0 {
                let count=AVAudioFrameCount(min(4096,left))
                guard try host.renderOfflineQuantum(frames:count,to:buffer) == .success else { throw AudioSampleError.invalid("Checkpoint warmup did not render.") }
                left-=Int(buffer.frameLength)
            }
            if interrupted { _ = try await host.handle("audio.export",payload:["path":output.appendingPathComponent(name+"-export.wav").path,"mode":"song","tail":0]) }
            guard try host.renderOfflineQuantum(frames:1024,to:buffer) == .success,buffer.frameLength==1024 else { throw AudioSampleError.invalid("Checkpoint continuation did not render.") }
            let result=Array(UnsafeBufferPointer(start:buffer.floatChannelData![0],count:1024))
            await host.shutdownAndWait();return result
        }
        var checks:[[String:Any]]=[]
        for (name,prefix) in [("mid-row head",6001),("mid-row steady loop",25001)] {
            let plain=try await capture(prefix:prefix,interrupted:false,name:name)
            let restored=try await capture(prefix:prefix,interrupted:true,name:name)
            let error=zip(plain,restored).map { abs(Double($0)-Double($1)) }.max() ?? 0
            checks.append(["name":name,"passed":error<1e-6 && (plain.max() ?? 0)>0.01,"maximumPCMError":error,
                           "referenceFirst":plain.first ?? 0,"restoredFirst":restored.first ?? 0,"prefixFrames":prefix,"comparedFrames":1024])
            try JSONSerialization.data(withJSONObject:checks,options:[.prettyPrinted,.sortedKeys]).write(to:output.appendingPathComponent("progress.json"),options:.atomic)
        }
        return ["passed":checks.allSatisfy{$0["passed"] as? Bool == true},"checks":checks,"scope":"actual Host export/restoration; generated frozen PCM only, manual offline graph, no MIDI inputs"]
    }
}

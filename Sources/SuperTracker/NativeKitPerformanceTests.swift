// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation
import Darwin

/// Offline throughput only: generated media, empty vendor catalogues and no MIDI
/// inputs. Root executes a pinned release build; these timings do not establish
/// device deadlines. Setup, output inspection and reporting are outside timing.
enum NativeKitPerformanceTests {
    private struct Scenario {
        let name:String, kits:Int, regular:Int, sounds:Int
        var sparse=false, drums=false
        var audible:Bool { regular > 0 || sounds > 0 }
    }
    @MainActor static func run(output:URL) async throws -> [String] {
        let folder=output.appendingPathComponent("kit-performance",isDirectory:true)
        try FileManager.default.createDirectory(at:folder,withIntermediateDirectories:true)
        let format=AVAudioFormat(standardFormatWithSampleRate:48_000,channels:2)!
        let sampleURL=folder.appendingPathComponent("generated-loop.wav")
        do {
            let pcm=AVAudioPCMBuffer(pcmFormat:format,frameCapacity:4800)!
            pcm.frameLength=4800
            for i in 0..<4800 {
                let value=Float(sin(Double(i)*2*Double.pi/240))*0.01
                pcm.floatChannelData![0][i]=value;pcm.floatChannelData![1][i]=value
            }
            let file=try AVAudioFile(forWriting:sampleURL,settings:format.settings,commonFormat:.pcmFormatFloat32,interleaved:false)
            try file.write(from:pcm)
        }
        let graph=AVAudioEngine()
        try graph.enableManualRenderingMode(.offline,format:format,maximumFrameCount:512)
        let host=try AudioHost(audioGraph:graph,
            vst3Catalogue:VST3Catalogue(configuration:.init(roots:[],cacheURL:folder.appendingPathComponent("vst3.json"))),
            midiInputIDs:[],vst2Catalogue:VST2Catalogue(configuration:.init(roots:[],cacheURL:folder.appendingPathComponent("vst2.json"))))
        defer { host.shutdown() }
        var timebase=mach_timebase_info_data_t();mach_timebase_info(&timebase)
        let secondsPerTick=Double(timebase.numer)/Double(timebase.denom)/1e9
        func cpuSeconds()->Double {
            var time=timespec();clock_gettime(CLOCK_PROCESS_CPUTIME_ID,&time)
            return Double(time.tv_sec)+Double(time.tv_nsec)/1e9
        }
        func summary(_ values:[Double])->[String:Double] {
            let sorted=values.sorted()
            func percentile(_ p:Double)->Double { sorted[min(sorted.count-1,max(0,Int(ceil(p*Double(sorted.count)))-1))] }
            return ["medianSeconds":percentile(0.5),"p95Seconds":percentile(0.95),"p99Seconds":percentile(0.99),"maxSeconds":sorted.last ?? 0,"totalSeconds":values.reduce(0,+)]
        }
        let scenarios:[Scenario]=[
            .init(name:"plain-silence",kits:0,regular:0,sounds:0),
            .init(name:"one-silent-kit",kits:1,regular:0,sounds:0),
            .init(name:"four-silent-kits",kits:4,regular:0,sounds:0),
            .init(name:"sixteen-regular-voices",kits:0,regular:16,sounds:0),
            .init(name:"sixteen-regular-plus-silent-kit",kits:1,regular:16,sounds:0),
            .init(name:"one-sample-sound",kits:1,regular:0,sounds:1),
            .init(name:"sixty-four-sample-sounds",kits:4,regular:0,sounds:64),
            .init(name:"sparse-final-sound",kits:4,regular:0,sounds:1,sparse:true),
            .init(name:"drums-and-builtin-eq",kits:1,regular:0,sounds:6,drums:true)]
        var records:[[String:Any]]=[],checks:[String]=[],failure:String?
        defer {
            var report:[String:Any]=["version":1,"hardware":false,"vendors":false,"mode":"manual-offline","sampleRate":48000,
                "routing":"standard in every case","warmupFrames":8192,"measuredFramesPerTrial":192000,"trials":3,
                "blockSizes":[128,256,512],"processCPUIncludesOtherProcessThreads":true,"timedScope":"renderOfflineQuantum only; clock sampling overhead is not subtracted",
                "build":"Caller must pin release compiler flags, source and binary hashes","checks":checks,"records":records,"passed":failure == nil]
            if let failure { report["error"]=failure }
            if let data=try? JSONSerialization.data(withJSONObject:report,options:[.prettyPrinted,.sortedKeys]) { try? data.write(to:folder.appendingPathComponent("report.json")) }
        }
        do {
            for scenario in scenarios {
                let setupStart=mach_absolute_time()
                let sample:[String:Any]=["type":"sample","samplePath":sampleURL.path,"rootNote":60,"volume":1,"mode":"forward","loopStart":0,"loopEnd":1,"attack":0,"decay":0,"sustain":1,"release":0,"voiceMode":"mono"]
                // Kits are track groups: sixteen pad tracks per kit, each with its own sampler, behind a group bus.
                var instruments:[[String:Any]]=[sample]
                var tracks:[[String:Any]]=(0..<16).map { ["routingID":"t\($0)","instrument":0,"volume":1,"pan":0] }
                var rows=[[Any]](repeating:[Any](repeating:NSNull(),count:64),count:16)
                for lane in 0..<scenario.regular {
                    for row in 0..<64 { rows[lane][row]=["note":60,"velocity":100,"gate":1] }
                }
                var groups:[[String:Any]]=[],outputs:[String:Any]=[:]
                let active=Set(scenario.sparse ? [63] : Array(0..<scenario.sounds))
                for k in 0..<scenario.kits {
                    groups.append(["id":"k\(k)","name":"Kit \(k + 1)","busID":"g\(k)"])
                    for pad in 0..<16 {
                        var instrument=sample
                        if scenario.drums && pad < 6 {
                            instrument=["type":"synth","drumModel":["kick","snare","hihat","tom","perc","cymbal"][pad],"volume":0.08,"attack":0,"decay":0,"sustain":1,"release":0]
                        }
                        instruments.append(instrument)
                        let id="k\(k)-pad\(pad)"
                        tracks.append(["routingID":id,"groupID":"k\(k)","pad":pad,"instrument":instruments.count-1,"volume":1,"pan":0,"voiceMode":"mono"])
                        outputs["track:"+id]="bus:g\(k)"
                        rows.append((0..<64).map { _ -> Any in active.contains(k*16+pad) ? ["note":60,"velocity":100,"gate":1] as [String:Any] : NSNull() })
                    }
                }
                let buses:[[String:Any]]=(0..<scenario.kits).map { ["id":"g\($0)","name":"Kit \($0 + 1)","type":"group","groupID":"k\($0)","volume":1] }
                let project:[String:Any]=["id":"kit-performance-\(scenario.name)","version":1,"bpm":120,"linesPerBeat":4,"masterVolume":1,
                    "tracks":tracks,"instruments":instruments,"groups":groups,"patterns":[["id":0,"length":64,"steps":rows,"automation":[]]],
                    "sequence":[0],"activePattern":0,"loopSong":true,"audioPlugins":[],"delay":["mix":0],"reverb":["mix":0],
                    "routing":["version":1,"mixModel":"standard","buses":buses,"outputs":outputs,"sends":[],"sidechains":[]]]
                _ = try await host.handle("engine.project",payload:["project":project,"replace":true,"awaitPlugins":true])
                if scenario.drums { _ = try await host.handle("plugin.load",payload:["owner":"bus:g0","kind":"effect","slot":0,"component":"stfx:eq"]) }
                let setupSeconds=Double(mach_absolute_time()-setupStart)*secondsPerTick
                for block in [128,256,512] {
                    let buffer=AVAudioPCMBuffer(pcmFormat:format,frameCapacity:AUAudioFrameCount(block))!
                    for trial in 0..<3 {
                        _ = try await host.handle("transport.stop",payload:[:])
                        _ = try await host.handle("engine.panic",payload:[:])
                        _ = try await host.handle("transport.play",payload:["mode":"pattern","fromStart":true])
                        var warmed=0
                        while warmed < 8192 {
                            let frames=min(block,8192-warmed)
                            guard try host.renderOfflineQuantum(frames:AUAudioFrameCount(frames),to:buffer) == .success else { throw AudioSampleError.invalid("Kit performance warmup render failed: \(scenario.name)") }
                            warmed += frames
                        }
                        let count=(192000+block-1)/block
                        var walls=[Double](repeating:0,count:count),cpus=[Double](repeating:0,count:count)
                        var rendered=0,index=0,squared=0.0,peak=0.0
                        while rendered < 192000 {
                            let frames=min(block,192000-rendered)
                            let cpuStart=cpuSeconds(),wallStart=mach_absolute_time()
                            let result=try host.renderOfflineQuantum(frames:AUAudioFrameCount(frames),to:buffer)
                            let wallEnd=mach_absolute_time(),cpuEnd=cpuSeconds()
                            walls[index]=Double(wallEnd-wallStart)*secondsPerTick;cpus[index]=max(0,cpuEnd-cpuStart)
                            guard result == .success,buffer.frameLength == frames,let pcm=buffer.floatChannelData else { throw AudioSampleError.invalid("Kit performance render failed: \(scenario.name), block \(block), trial \(trial)") }
                            for channel in 0..<2 { for frame in 0..<frames {
                                let value=Double(pcm[channel][frame])
                                guard value.isFinite else { throw AudioSampleError.invalid("Kit performance produced non-finite PCM") }
                                squared += value*value;peak=max(peak,abs(value))
                            } }
                            rendered += frames;index += 1
                        }
                        let rms=sqrt(squared/Double(192000*2))
                        guard scenario.audible ? rms > 0.0000001 : peak == 0 else { throw AudioSampleError.invalid("Kit performance unexpected \(scenario.audible ? "silence" : "audio") in \(scenario.name): rms=\(rms), peak=\(peak)") }
                        records.append(["case":scenario.name,"blockFrames":block,"trial":trial,"setupSeconds":setupSeconds,"renderCalls":index,
                            "wall":summary(walls),"processCPU":summary(cpus),"wallPercentOfAudioDuration":walls.reduce(0,+)/4*100,
                            "processCPUPercentOfAudioDuration":cpus.reduce(0,+)/4*100,"finite":true,"renderErrors":0,"rms":rms,"peak":peak])
                    }
                }
                checks.append("\(scenario.name): 3 block sizes ×3 trials; finite \(scenario.audible ? "audible" : "silent") PCM")
            }
        } catch { failure=error.localizedDescription;throw error }
        return checks
    }
}

#endif

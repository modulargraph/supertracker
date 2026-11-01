// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation
import Darwin

/// Offline render cost of representative projects through the full production graph
/// (native sources, strips, routing, meters), in percent of real time. Generated
/// media only, no vendor plug-ins, no MIDI or audio devices.
///   --render-performance <output>            every scenario, three block sizes
///   ST_RENDER_PERF_SCENARIO=<name,...>       only those scenarios
///   ST_RENDER_PERF_SECONDS=<seconds>         measured audio per trial (default 4)
///   ST_RENDER_PERF_BUDGET=<percent>          fail when a scenario's best trial (render-thread CPU) exceeds it
///   ST_RENDER_PERF_PROJECT=<file>            also measure a saved project ("project-file"), vendor plug-ins included.
///     The file is only read. Line inputs are dropped (no device offline), and so are plug-ins whose
///     bundle path, name or "format:<AU|VST2|VST3|Builtin>" matches ST_RENDER_PERF_SKIP_PLUGINS
///     (comma-separated substrings, for example hardware editors that talk to real synths and
///     render silence; default none).
/// The budget is a coarse regression guard, not a device deadline: keep it generous.
enum NativeRenderPerformanceTests {
    struct Scenario {
        let name:String
        let project:[String:Any]
        var playing=true
        var mode="pattern"
    }

    static func wav(_ url:URL,_ format:AVAudioFormat,frames:Int,_ value:(Int)->(Float,Float)) throws {
        let pcm=AVAudioPCMBuffer(pcmFormat:format,frameCapacity:AVAudioFrameCount(frames))!
        pcm.frameLength=AVAudioFrameCount(frames)
        for i in 0..<frames { let (l,r)=value(i); pcm.floatChannelData![0][i]=l; pcm.floatChannelData![1][i]=r }
        let file=try AVAudioFile(forWriting:url,settings:format.settings,commonFormat:.pcmFormatFloat32,interleaved:false)
        try file.write(from:pcm)
    }

    /// The user's typical session: the hardware-editor template's eight tracks (their
    /// editors and line inputs are outside this offline graph) and one 16-pad kit group of
    /// sampler tracks with drum-length one-shots, plus a few wavetable and sampler synths.
    static func templateAndKit(folder:URL,playing:Bool) -> Scenario {
        var instruments:[[String:Any]]=[]
        var tracks:[[String:Any]]=(0..<8).map { ["routingID":"hw\($0)","name":"Track \($0+1)","instrument":-1,"volume":0.8,"pan":0] }
        var rows=[[Any]](repeating:[Any](repeating:NSNull(),count:64),count:8)
        var outputs:[String:Any]=[:]
        for pad in 0..<16 {
            instruments.append(["type":"sample","samplePath":folder.appendingPathComponent("pad\(pad).wav").path,"rootNote":60,"volume":0.5,
                "mode":"oneshot","attack":0,"decay":0,"sustain":1,"release":0.05])
            let id="kit-pad\(pad)"
            tracks.append(["routingID":id,"groupID":"k0","pad":pad,"instrument":instruments.count-1,"volume":0.8,"pan":0,"voiceMode":"mono"])
            outputs["track:"+id]="bus:g0"
            // Kick on quarters, snare on backbeats, hats on eighths, the rest sparse.
            rows.append((0..<64).map { row -> Any in
                let hit:Bool
                switch pad { case 0: hit=row%4==0; case 1: hit=row%8==4; case 2: hit=row%2==0; case 3: hit=row%16==14; default: hit=row%16==(pad*5)%16 }
                return hit ? ["note":60,"velocity":100,"gate":1] as [String:Any] : NSNull()
            })
        }
        // Two wavetable pads (sustained chords) and a looping sampler bass.
        for synth in 0..<3 {
            let wavetable=synth<2
            instruments.append(wavetable
                ? ["type":"sample","samplePath":folder.appendingPathComponent("table.wav").path,"rootNote":60,"volume":0.3,"mode":"wavetable",
                   "wavetableFrameSize":2048,"wavetablePosition":0.3,"cutoff":4000,"resonance":0.2,"attack":0.01,"decay":0.3,"sustain":0.7,"release":0.3,
                   "lfoRate":0.5,"lfoDepth":0.2,"lfoTarget":"position"]
                : ["type":"sample","samplePath":folder.appendingPathComponent("loop.wav").path,"rootNote":60,"volume":0.4,"mode":"forward",
                   "loopStart":0.1,"loopEnd":0.9,"attack":0,"decay":0.2,"sustain":0.8,"release":0.1])
            tracks.append(["routingID":"synth\(synth)","instrument":instruments.count-1,"volume":0.8,"pan":0])
            rows.append((0..<64).map { row -> Any in
                row%16==0 ? ["note":wavetable ? 48+synth*7 : 36,"velocity":90,"gate":12,"chord":wavetable ? [0,4,7] : [0]] as [String:Any] : NSNull()
            })
        }
        let project:[String:Any]=["id":"render-performance-template-kit","version":1,"bpm":120,"linesPerBeat":4,"masterVolume":0.8,
            "tracks":tracks,"instruments":instruments,"groups":[["id":"k0","name":"Kit","busID":"g0"]],
            "patterns":[["id":0,"length":64,"steps":rows,"automation":[]]],"sequence":[0],"activePattern":0,"loopSong":true,"audioPlugins":[],
            "routing":["version":1,"mixModel":"standard","buses":[["id":"g0","name":"Kit","type":"group","groupID":"k0","volume":1]],
                       "outputs":outputs,"sends":[],"sidechains":[]]]
        return Scenario(name:playing ? "template-kit-playing" : "template-kit-stopped",project:project,playing:playing)
    }

    /// Sixty-four sampler tracks, each playing a looping sample every other row.
    static func manyTracks(folder:URL) -> Scenario {
        var instruments:[[String:Any]]=[],tracks:[[String:Any]]=[],rows:[[Any]]=[]
        for track in 0..<64 {
            instruments.append(["type":"sample","samplePath":folder.appendingPathComponent(track%2==0 ? "loop.wav" : "pad\(track%16).wav").path,
                "rootNote":60,"volume":0.1,"mode":track%2==0 ? "forward" : "oneshot","attack":0,"decay":0.1,"sustain":0.8,"release":0.05])
            tracks.append(["routingID":"t\(track)","instrument":track,"volume":0.8,"pan":Double(track%5-2)*0.2])
            rows.append((0..<64).map { row -> Any in row%2==track%2 ? ["note":48+(row+track)%24,"velocity":90,"gate":1.5] as [String:Any] : NSNull() })
        }
        let project:[String:Any]=["id":"render-performance-64","version":1,"bpm":120,"linesPerBeat":4,"masterVolume":0.8,
            "tracks":tracks,"instruments":instruments,"patterns":[["id":0,"length":64,"steps":rows,"automation":[]]],"sequence":[0],
            "activePattern":0,"loopSong":true,"audioPlugins":[],
            "routing":["version":1,"mixModel":"standard","buses":[],"outputs":[:],"sends":[],"sidechains":[]]]
        return Scenario(name:"sixty-four-tracks",project:project)
    }

    /// A saved project as the user hears it, minus hardware it cannot reach offline.
    static func projectFile(_ url:URL,skipping:[String]) throws -> Scenario {
        guard var project=try JSONSerialization.jsonObject(with:Data(contentsOf:url)) as? [String:Any] else {
            throw AudioSampleError.invalid("Render performance: \(url.path) is not a project")
        }
        let plugins=(project["audioPlugins"] as? [[String:Any]] ?? []).filter { plugin in
            // Matched against the bundle path, name and format ("format:AU" skips every Audio Unit).
            let key=(plugin["bundlePath"] as? String ?? "")+"|"+(plugin["name"] as? String ?? "")+"|format:"+(plugin["format"] as? String ?? "")
            return !skipping.contains { !$0.isEmpty && key.contains($0) }
        }
        project["audioPlugins"]=plugins
        project["tracks"]=(project["tracks"] as? [[String:Any]] ?? []).map { var track=$0; track.removeValue(forKey:"audioInput"); return track }
        project["loopSong"]=true
        // Media paths are relative to the project file.
        let folder=url.deletingLastPathComponent()
        func resolve(_ value:Any) -> Any {
            if let list=value as? [Any] { return list.map(resolve) }
            guard var object=value as? [String:Any] else { return value }
            for (key,item) in object {
                if key == "samplePath",let path=item as? String,!path.isEmpty,!path.hasPrefix("/") { object[key]=folder.appendingPathComponent(path).path }
                else { object[key]=resolve(item) }
            }
            return object
        }
        project["instruments"]=resolve(project["instruments"] ?? [])
        project["audioClips"]=resolve(project["audioClips"] ?? [])
        return Scenario(name:"project-file",project:project,mode:"song")
    }

    /// Nodes the output actually pulls (attached nodes include dormant strips and the
    /// per-insert units that render inside chain nodes), by Audio Unit name.
    static func renderedNodes(_ graph:AVAudioEngine) -> (count:Int,summary:String) {
        var seen=Set<ObjectIdentifier>(),stack:[AVAudioNode]=[graph.outputNode],kinds:[String:Int]=[:]
        while let node=stack.popLast() {
            guard seen.insert(ObjectIdentifier(node)).inserted else { continue }
            kinds[(node as? AVAudioUnit)?.auAudioUnit.audioUnitName ?? String(describing:type(of:node)),default:0]+=1
            for bus in 0..<node.numberOfInputs { if let upstream=graph.inputConnectionPoint(for:node,inputBus:bus)?.node { stack.append(upstream) } }
        }
        return (seen.count,kinds.sorted { $0.value > $1.value }.map { "\($0.key)=\($0.value)" }.joined(separator:", "))
    }

    @MainActor static func run(output:URL) async throws -> [String] {
        let folder=output.appendingPathComponent("render-performance",isDirectory:true)
        try FileManager.default.createDirectory(at:folder,withIntermediateDirectories:true)
        let format=AVAudioFormat(standardFormatWithSampleRate:48_000,channels:2)!
        // Drum-length one-shots (60 ms .. 1.6 s), a one-second loop and a 16-frame wavetable.
        for pad in 0..<16 {
            let frames=Int(48_000*(0.06+0.1*Double(pad)))
            var seed=UInt32(pad*7919+1)
            try wav(folder.appendingPathComponent("pad\(pad).wav"),format,frames:frames) { i in
                seed=seed &* 1664525 &+ 1013904223
                let noise=Float(Int32(bitPattern:seed))/Float(Int32.max)
                let decay=Float(exp(-Double(i)/Double(frames)*5))
                let tone=Float(sin(Double(i)*2*Double.pi*Double(60+pad*40)/48_000))
                return ((tone*0.7+noise*0.3)*decay*0.5,(tone*0.6+noise*0.4)*decay*0.5)
            }
        }
        try wav(folder.appendingPathComponent("loop.wav"),format,frames:48_000) { i in
            let v=Float(sin(Double(i)*2*Double.pi*110/48_000)*0.3+sin(Double(i)*2*Double.pi*220.5/48_000)*0.1); return (v,v*0.9)
        }
        try wav(folder.appendingPathComponent("table.wav"),format,frames:2048*16) { i in
            let frame=Double(i/2048),phase=Double(i%2048)/2048
            var v=0.0; for h in 1...8 { v+=sin(phase*2*Double.pi*Double(h))/Double(h)*(h==1 ? 1 : frame/15) }
            return (Float(v*0.4),Float(v*0.4))
        }
        let graph=AVAudioEngine()
        try graph.enableManualRenderingMode(.offline,format:format,maximumFrameCount:512)
        // A saved project needs the installed plug-ins (scanned into this run's private caches).
        let installed=ProcessInfo.processInfo.environment["ST_RENDER_PERF_PROJECT"] != nil
        var vst3=VST3Catalogue.Configuration(),vst2=VST2Catalogue.Configuration()
        vst3.cacheURL=folder.appendingPathComponent("vst3.json"); vst2.cacheURL=folder.appendingPathComponent("vst2.json")
        if !installed { vst3.roots=[]; vst2.roots=[] }
        let host=try AudioHost(audioGraph:graph,vst3Catalogue:VST3Catalogue(configuration:vst3),
            midiInputIDs:[],vst2Catalogue:VST2Catalogue(configuration:vst2))
        defer { host.shutdown() }
        var timebase=mach_timebase_info_data_t();mach_timebase_info(&timebase)
        let secondsPerTick=Double(timebase.numer)/Double(timebase.denom)/1e9
        func threadCPU()->Double { var t=timespec();clock_gettime(CLOCK_THREAD_CPUTIME_ID,&t);return Double(t.tv_sec)+Double(t.tv_nsec)/1e9 }
        let environment=ProcessInfo.processInfo.environment
        let only=environment["ST_RENDER_PERF_SCENARIO"].map { Set($0.split(separator:",").map(String.init)) }
        let measuredSeconds=max(0.5,Double(environment["ST_RENDER_PERF_SECONDS"] ?? "") ?? 4)
        let budget=Double(environment["ST_RENDER_PERF_BUDGET"] ?? "")
        let blocks=(environment["ST_RENDER_PERF_BLOCKS"] ?? "128,256,512").split(separator:",").compactMap { Int($0) }
        let scenarios=[templateAndKit(folder:folder,playing:true),templateAndKit(folder:folder,playing:false),manyTracks(folder:folder)]
            .filter { only?.contains($0.name) ?? true }
            + (try environment["ST_RENDER_PERF_PROJECT"].map { [try projectFile(URL(fileURLWithPath:$0),
                skipping:(environment["ST_RENDER_PERF_SKIP_PLUGINS"] ?? "").split(separator:",").map(String.init))] } ?? [])
        var records:[[String:Any]]=[],checks:[String]=[]
        for scenario in scenarios {
            _ = try await host.handle("engine.project",payload:["project":scenario.project,"replace":true,"awaitPlugins":true])
            try host.start()
            var kinds:[String:Int]=[:]
            for node in graph.attachedNodes {
                let name=(node as? AVAudioUnit)?.auAudioUnit.audioUnitName ?? String(describing:type(of:node))
                kinds[name,default:0]+=1
            }
            print("render-performance \(scenario.name) nodes: \(graph.attachedNodes.count) "+kinds.sorted { $0.value > $1.value }.map { "\($0.key)=\($0.value)" }.joined(separator:", "))
            let rendered=renderedNodes(graph)
            print("render-performance \(scenario.name) rendered nodes: \(rendered.count) \(rendered.summary)")
            for block in blocks {
                let buffer=AVAudioPCMBuffer(pcmFormat:format,frameCapacity:AUAudioFrameCount(block))!
                var trials:[Double]=[],cpus:[Double]=[]
                for _ in 0..<3 {
                    _ = try await host.handle("transport.stop",payload:[:])
                    _ = try await host.handle("engine.panic",payload:[:])
                    if scenario.playing { _ = try await host.handle("transport.play",payload:["mode":scenario.mode,"fromStart":true]) }
                    for _ in 0..<(8192/block) { _ = try host.renderOfflineQuantum(frames:AUAudioFrameCount(block),to:buffer) }
                    let total=Int(measuredSeconds*48_000)/block*block
                    var rendered=0,squared=0.0,cpu=0.0,wall=0.0
                    while rendered < total {
                        let c0=threadCPU(),w0=mach_absolute_time()
                        guard try host.renderOfflineQuantum(frames:AUAudioFrameCount(block),to:buffer) == .success,let pcm=buffer.floatChannelData else {
                            throw AudioSampleError.invalid("Render performance: render failed in \(scenario.name)")
                        }
                        wall+=Double(mach_absolute_time()-w0)*secondsPerTick;cpu+=threadCPU()-c0
                        for channel in 0..<2 { for frame in 0..<block { let v=Double(pcm[channel][frame]);guard v.isFinite else { throw AudioSampleError.invalid("Render performance: non-finite PCM") };squared+=v*v } }
                        rendered+=block
                    }
                    let seconds=Double(total)/48_000
                    guard scenario.playing ? squared > 0 : true else { throw AudioSampleError.invalid("Render performance: \(scenario.name) is silent") }
                    trials.append(wall/seconds*100);cpus.append(cpu/seconds*100)
                    records.append(["case":scenario.name,"blockFrames":block,"wallPercent":wall/seconds*100,"threadCPUPercent":cpu/seconds*100])
                }
                let median=trials.sorted()[1]
                print(String(format:"render-performance %@ block %d: %.2f%% of real time (median of 3 wall; min %.2f; render-thread CPU min %.2f%%)",scenario.name,block,median,trials.min()!,cpus.min()!))
                // The budget compares the best trial's render-thread CPU, which other load on the machine barely moves.
                if let budget, cpus.min()! > budget { throw AudioSampleError.invalid(String(format:"Render performance: %@ at %d frames took %.1f%% of real time (render-thread CPU), budget %.1f%%",scenario.name,block,cpus.min()!,budget)) }
            }
            checks.append("\(scenario.name): \(blocks.count) block sizes ×3 trials")
        }
        if let data=try? JSONSerialization.data(withJSONObject:["records":records],options:[.prettyPrinted,.sortedKeys]) { try? data.write(to:folder.appendingPathComponent("report.json")) }
        return checks
    }
}
#endif

// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation
import Darwin

/// No fixed track count: 16-, 64- and 200-track projects load, play (legacy and
/// standard routing), edit incrementally, grow while playing, save/reload and export
/// MIDI. Offline manual rendering with generated synths only (no vendors, no MIDI
/// devices, no audio device changes). Timings are throughput, not device deadlines.
enum NativeManyTracksTests {
    @MainActor static func run(output:URL) async throws -> [String] {
        let folder=output.appendingPathComponent("many-tracks",isDirectory:true)
        try FileManager.default.createDirectory(at:folder,withIntermediateDirectories:true)
        let format=AVAudioFormat(standardFormatWithSampleRate:48_000,channels:2)!
        var checks:[String]=[],records:[[String:Any]]=[],failure:String?
        defer {
            var report:[String:Any]=["version":1,"mode":"manual-offline","sampleRate":48000,"blockFrames":512,"checks":checks,"records":records,"passed":failure == nil]
            if let failure { report["error"]=failure }
            if let data=try? JSONSerialization.data(withJSONObject:report,options:[.prettyPrinted,.sortedKeys]) { try? data.write(to:folder.appendingPathComponent("report.json")) }
        }
        var timebase=mach_timebase_info_data_t();mach_timebase_info(&timebase)
        let secondsPerTick=Double(timebase.numer)/Double(timebase.denom)/1e9
        func seconds(_ start:UInt64)->Double { Double(mach_absolute_time()-start)*secondsPerTick }
        func require(_ condition:Bool,_ message:@autoclosure ()->String) throws { if !condition { throw AudioSampleError.invalid(message()) } }

        func project(tracks count:Int,routing:Bool,patterns:Int=8,soloLast:Bool=false,muteLast:Bool=false,changed:Int = -1)->[String:Any] {
            var tracks:[[String:Any]]=(0..<count).map { ["id":$0,"name":"T\($0 + 1)","routingID":"t\($0)","instrument":0,"volume":0.8,"pan":0] }
            if soloLast { tracks[count-1]["solo"]=true }
            if muteLast { tracks[count-1]["mute"]=true }
            let list:[[String:Any]]=(0..<patterns).map { p in
                let steps:[[Any]]=(0..<count).map { t in (0..<64).map { r -> Any in
                    r % 8 == (t % 8) ? ["note":48 + (t + r + p) % 24,"instrument":0,"velocity":100,"gate":4.0] as [String:Any] : NSNull() } }
                var pattern:[String:Any]=["id":p,"name":"P\(p)","length":64,"steps":steps,"automation":[]]
                if p == changed, var first=pattern["steps"] as? [[Any]] { first[0][1]=["note":72,"instrument":0,"velocity":90,"gate":1.0]; pattern["steps"]=first }
                return pattern
            }
            var value:[String:Any]=["id":"many-tracks","version":1,"name":"Many tracks","bpm":120.0,"linesPerBeat":4,"masterVolume":0.7,
                "tracks":tracks,"instruments":[["id":0,"name":"Sine","type":"synth","waveform":"sine","volume":0.02,"attack":0,"decay":0,"sustain":1,"release":0.01]],
                "patterns":list,"sequence":Array(0..<patterns),"activePattern":0,"loopSong":true,"audioPlugins":[],"delay":["mix":0],"reverb":["mix":0]]
            if routing {
                var outputs:[String:Any]=[:]
                for t in 0..<count { outputs["track:t\(t)"]=t % 2 == 0 ? "bus:g0" : "master" }
                value["routing"]=["version":1,"mixModel":"standard","buses":[["id":"g0","name":"Even tracks","type":"group","volume":1]],"outputs":outputs,"sends":[],"sidechains":[]]
            }
            return value
        }
        func render(_ host:AudioHost,_ buffer:AVAudioPCMBuffer,frames total:Int)throws->(rms:Double,wall:Double) {
            var squared=0.0,rendered=0,wall=0.0
            while rendered < total {
                let start=mach_absolute_time()
                let status=try host.renderOfflineQuantum(frames:512,to:buffer)
                wall+=seconds(start)
                guard status == .success,let pcm=buffer.floatChannelData else { throw AudioSampleError.invalid("Offline render failed") }
                for channel in 0..<2 { for frame in 0..<Int(buffer.frameLength) { let v=Double(pcm[channel][frame]);guard v.isFinite else { throw AudioSampleError.invalid("Non-finite PCM") };squared+=v*v } }
                rendered+=512
            }
            return (sqrt(squared/Double(rendered*2)),wall)
        }

        do {
            // SUPERTRACKER_MANY_TRACKS="200:legacy,64:standard" narrows the matrix for profiling.
            let only=ProcessInfo.processInfo.environment["SUPERTRACKER_MANY_TRACKS"]?.split(separator:",").map(String.init)
            for count in [16,64,200] {
                for routing in [false,true] where only?.contains("\(count):\(routing ? "standard" : "legacy")") ?? true {
                    let graph=AVAudioEngine()
                    try graph.enableManualRenderingMode(.offline,format:format,maximumFrameCount:512)
                    let host=try AudioHost(audioGraph:graph,
                        vst3Catalogue:VST3Catalogue(configuration:.init(roots:[],cacheURL:folder.appendingPathComponent("vst3.json"))),
                        midiInputIDs:[],vst2Catalogue:VST2Catalogue(configuration:.init(roots:[],cacheURL:folder.appendingPathComponent("vst2.json"))))
                    defer { host.shutdown() }
                    let buffer=AVAudioPCMBuffer(pcmFormat:format,frameCapacity:512)!
                    let name="\(count)-tracks-\(routing ? "standard" : "legacy")"
                    // Load (a full project replacement).
                    var start=mach_absolute_time()
                    _ = try await host.handle("engine.project",payload:["project":project(tracks:count,routing:routing),"replace":true,"awaitPlugins":true])
                    let load=seconds(start)
                    _ = try await host.handle("transport.play",payload:["mode":"song","fromStart":true])
                    _ = try render(host,buffer,frames:8192)
                    let audio=try render(host,buffer,frames:96_000)
                    try require(audio.rms > 1e-6,"\(name): playback is silent")
                    // An edit that changes one step of one pattern re-sends only that pattern.
                    start=mach_absolute_time()
                    _ = try await host.handle("engine.project",payload:["project":project(tracks:count,routing:routing,changed:3)])
                    let edit=seconds(start)
                    try require(AudioProject.lastReusedPatterns == 7,"\(name): an edit of one pattern reused \(AudioProject.lastReusedPatterns) of 7 unchanged patterns")
                    start=mach_absolute_time()
                    _ = try await host.handle("engine.project",payload:["project":project(tracks:count,routing:routing,changed:3)])
                    let unchanged=seconds(start)
                    try require(AudioProject.lastReusedPatterns == 8,"\(name): an unchanged project reused \(AudioProject.lastReusedPatterns) of 8 patterns")
                    // The highest-numbered track reaches the output: solo it, then also mute it.
                    _ = try await host.handle("engine.project",payload:["project":project(tracks:count,routing:routing,soloLast:true)])
                    _ = try render(host,buffer,frames:4096)
                    let solo=try render(host,buffer,frames:48_000)
                    try require(solo.rms > 1e-6,"\(name): the soloed last track is silent")
                    _ = try await host.handle("engine.project",payload:["project":project(tracks:count,routing:routing,soloLast:true,muteLast:true)])
                    _ = try render(host,buffer,frames:8192)
                    let muted=try render(host,buffer,frames:24_000)
                    try require(muted.rms < 1e-9,"\(name): a muted soloed track still sounds (rms \(muted.rms))")
                    // Grow by eight tracks while playing: the graph keeps running and the new last track plays.
                    _ = try await host.handle("engine.project",payload:["project":project(tracks:count,routing:routing)])
                    let nodesBefore=graph.attachedNodes.count
                    let rebuildsBefore=(host.status()["routing"] as? [String:Any])?["fullRebuilds"] as? Int ?? 0
                    start=mach_absolute_time()
                    _ = try await host.handle("engine.project",payload:["project":project(tracks:count+8,routing:routing,soloLast:true)])
                    let grow=seconds(start)
                    if routing {
                        // Standard routing grows live: the new strips join the running graph, nothing is rebuilt.
                        let routingStatus=host.status()["routing"] as? [String:Any] ?? [:]
                        try require(routingStatus["fullRebuilds"] as? Int == rebuildsBefore && (routingStatus["incrementalChanges"] as? Int ?? 0) > 0,
                                    "\(name): adding tracks rebuilt the routing graph instead of extending it: \(routingStatus)")
                    }
                    try require(graph.isRunning,"\(name): adding tracks stopped the graph")
                    _ = try render(host,buffer,frames:8192)
                    let grown=try render(host,buffer,frames:48_000)
                    try require(grown.rms > 1e-6,"\(name): a track added while playing is silent")
                    let nodes=graph.attachedNodes.count
                    records.append(["case":name,"tracks":count,"routing":routing ? "standard" : "legacy","loadSeconds":load,"editOnePatternSeconds":edit,
                        "unchangedApplySeconds":unchanged,"growEightTracksSeconds":grow,"renderWallPercentOfRealtime":audio.wall/2*100,
                        "attachedNodes":nodes,"nodesAddedForEightTracks":nodes-nodesBefore])
                    checks.append("\(name): load, play, incremental edit, solo/mute of the last track, grow while playing")
                }
            }
            // Save/reload and MIDI export of a 200-track project.
            let store=try ProjectStore(directory:folder.appendingPathComponent("store",isDirectory:true))
            let big=project(tracks:200,routing:true)
            try store.update(big)
            let file=folder.appendingPathComponent("two-hundred.supertracker")
            _ = try store.save(to:file,collectSamples:false)
            let reloaded=try store.read(file)
            try require((reloaded["tracks"] as? [[String:Any]])?.count == 200 && (reloaded["patterns"] as? [[String:Any]])?.count == 8,"200-track project round-trips")
            var tooMany=big;tooMany["tracks"]=(0..<(ProjectLayout.trackSanityLimit+1)).map { ["id":$0,"routingID":"x\($0)","instrument":0] }
            var rejected=false
            do { try store.validate(tooMany) } catch { rejected=true }
            try require(rejected,"the sanity limit rejects \(ProjectLayout.trackSanityLimit+1) tracks")
            let midi=folder.appendingPathComponent("two-hundred.mid")
            _ = try MIDIFile.export(reloaded,to:midi)
            try require((try MIDIFile.importProject(midi)["tracks"] as? [[String:Any]])?.count == 200,"200-track MIDI export reimports with 200 tracks")
            checks.append("200 tracks: save, reload, sanity limit, MIDI export/reimport")
        } catch { failure=error.localizedDescription;throw error }
        return checks
    }
}
#endif

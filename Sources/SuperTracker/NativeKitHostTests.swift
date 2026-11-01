// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation
import Darwin

/// Kits are track groups. Generated PCM and built-in processors only, offline manual
/// rendering: no hardware device, MIDI source or installed vendor is selected.
/// Loading a sixteen-pad kit while playing extends the running routing graph (no full
/// rebuild, no gap in the audio of the tracks already playing); member, group-bus and
/// master stems add up; choke groups cut within their group; group previews, member and
/// bus inserts, plain-project transitions and a migrated kit-lane project all work.
enum NativeKitHostTests {
    @MainActor static func run(output:URL) async throws -> [String] {
        let folder=output.appendingPathComponent("kit-groups",isDirectory:true)
        try FileManager.default.createDirectory(at:folder,withIntermediateDirectories:true)
        let format=AVAudioFormat(standardFormatWithSampleRate:48_000,channels:2)!
        let graph=AVAudioEngine()
        try graph.enableManualRenderingMode(.offline,format:format,maximumFrameCount:4096)
        let vst3=VST3Catalogue(configuration:.init(roots:[],cacheURL:folder.appendingPathComponent("vst3.json")))
        let vst2=VST2Catalogue(configuration:.init(roots:[],cacheURL:folder.appendingPathComponent("vst2.json")))
        let host=try AudioHost(audioGraph:graph,vst3Catalogue:vst3,midiInputIDs:[],vst2Catalogue:vst2)
        defer { host.shutdown() }
        var checks:[String]=[],metrics:[String:Any]=["hardware":false,"vendors":false,"mode":"manual-offline"]
        defer {
            metrics["checks"]=checks
            if let data=try? JSONSerialization.data(withJSONObject:metrics,options:[.prettyPrinted,.sortedKeys]) { try? data.write(to:folder.appendingPathComponent("metrics.json")) }
        }
        func step(_ name:String) { metrics["phase"]=name; print("kit groups: \(name)"); fflush(stdout) }
        func require(_ okay:Bool,_ message:@autoclosure ()->String) throws { if !okay { throw AudioSampleError.invalid("Kit groups: "+message()) } }
        var timebase=mach_timebase_info_data_t();mach_timebase_info(&timebase)
        let secondsPerTick=Double(timebase.numer)/Double(timebase.denom)/1e9
        func seconds(_ start:UInt64)->Double { Double(mach_absolute_time()-start)*secondsPerTick }
        func sample(_ name:String,_ value:Float,frames:Int=24_000) throws -> String {
            let url=folder.appendingPathComponent(name+".wav")
            let pcm=AVAudioPCMBuffer(pcmFormat:format,frameCapacity:AVAudioFrameCount(frames))!;pcm.frameLength=AVAudioFrameCount(frames)
            for c in 0..<2 { pcm.floatChannelData![c].update(repeating:value,count:frames) }
            let file=try AVAudioFile(forWriting:url,settings:format.settings,commonFormat:.pcmFormatFloat32,interleaved:false)
            try file.write(from:pcm);return url.path
        }
        func difference(_ x:[Float],_ y:[Float])->Float { zip(x,y).map { abs($0-$1) }.max() ?? .infinity }
        func rms<C:Collection>(_ values:C)->Double where C.Element == Float {
            values.isEmpty ? 0 : sqrt(values.reduce(0.0) { $0+Double($1)*Double($1) }/Double(values.count))
        }
        func routing()->[String:Any] { host.status()["routing"] as? [String:Any] ?? [:] }
        func snapshot() async throws -> [String:Any] { try await host.handle("engine.snapshot",payload:[:]) as! [String:Any] }
        func stems(_ name:String,_ owners:[String]) async throws -> [String:[Float]] {
            let result=try await host.handle("audio.export",payload:["mode":"stems","path":folder.appendingPathComponent(name).path,"owners":owners,"tail":0,"encoding":"float32"]) as! [String:Any]
            var pcm:[String:[Float]]=[:]
            for record in result["stems"] as? [[String:Any]] ?? [] {
                if let owner=record["owner"] as? String,let path=record["path"] as? String { pcm[owner]=try AudioSamples.decode(path).left }
            }
            try require(Set(pcm.keys) == Set(owners),"stems were omitted or renamed: \(pcm.keys.sorted())")
            try require(pcm.values.allSatisfy { $0.allSatisfy(\.isFinite) },"a stem contains non-finite PCM")
            return pcm
        }

        // Four ordinary tracks hold a sustained sine (continuous audio to watch for gaps).
        let sine:[String:Any]=["id":0,"name":"Sine","type":"synth","waveform":"sine","volume":0.05,"attack":0,"decay":0,"sustain":1,"release":0.01]
        func baseProject(rows:Int=8)->[String:Any] {
            let tracks:[[String:Any]]=(0..<4).map { ["id":$0,"name":"T\($0 + 1)","routingID":"t\($0)","instrument":0,"volume":0.8,"pan":0] }
            let steps:[[Any]]=(0..<4).map { t in (0..<rows).map { r -> Any in r == 0 ? ["note":48+t*7,"instrument":0,"velocity":100,"gate":Double(rows)] as [String:Any] : NSNull() } }
            return ["id":"kit-groups-host","version":1,"name":"Kit groups","bpm":120.0,"linesPerBeat":4,"masterVolume":0.7,
                "tracks":tracks,"instruments":[sine],"groups":[],
                "patterns":[["id":0,"name":"P0","length":rows,"steps":steps,"automation":[]]],"sequence":[0],"activePattern":0,"loopSong":true,
                "audioPlugins":[],"audioClips":[],"delay":["mix":0],"reverb":["mix":0],
                "routing":["version":1,"mixModel":"standard","buses":[],"outputs":[:],"sends":[],"sidechains":[]]]
        }
        // A sixteen-pad kit file (the .stkit layout) as kit.read hands it to the UI: a group project.
        let padSamples=try (0..<16).map { try sample("pad\($0 + 1)",Float(0.01*Double($0 + 1))) }
        var kitSounds:[[String:Any]]=[]
        for pad in 0..<16 {
            var sound:[String:Any]=["id":"s\(pad)","name":"Pad \(pad + 1)","channel":["volume":1,"pan":0],
                "instrument":["type":"sample","samplePath":padSamples[pad],"rootNote":60,"volume":1,"mode":"oneshot","attack":0,"decay":0,"sustain":1,"release":0]]
            if pad < 2 { sound["chokeGroup"]=1 }   // Pads 1 and 2 choke each other (open/closed hat).
            kitSounds.append(sound)
        }
        let kitFile:[String:Any]=["version":1,"name":"Generated kit","tracks":[],"instruments":[],"patterns":[],"audioPlugins":[],
            "kits":[["id":"srckit","name":"Generated kit","busID":"srcbus","sounds":kitSounds]],
            "routing":["version":1,"mixModel":"standard","buses":[["id":"srcbus","name":"Generated kit","type":"group","volume":1]],"outputs":[:],"sends":[],"sidechains":[]]]
        let source=try KitGroups.groupProject(fromKitProject:kitFile)
        let sourceTracks=source["tracks"] as? [[String:Any]] ?? []
        try require(sourceTracks.count == 16 && (source["groups"] as? [[String:Any]])?.count == 1,"the kit file did not convert to one group of sixteen tracks")
        try require(sourceTracks.prefix(2).allSatisfy { $0["chokeGroup"] as? Int == 1 } && sourceTracks.dropFirst(2).allSatisfy { $0["chokeGroup"] == nil },"choke groups did not become member track properties")

        /// The UI's insertGroup (kit-groups.js) in Swift: members appended with fresh IDs, their
        /// instruments, the group record, the group bus and member outputs into it.
        func withKit(_ base:[String:Any],groupID:String="kit",busID:String="kitbus")->[String:Any] {
            var p=base
            var tracks=p["tracks"] as! [[String:Any]],instruments=p["instruments"] as! [[String:Any]]
            var patterns=p["patterns"] as! [[String:Any]]
            var routing=p["routing"] as! [String:Any]
            var outputs=routing["outputs"] as! [String:Any],buses=routing["buses"] as! [[String:Any]]
            let sourceInstruments=source["instruments"] as? [[String:Any]] ?? []
            for (i,member) in sourceTracks.enumerated() {
                var instrument=sourceInstruments[member["instrument"] as! Int];instrument["id"]=instruments.count
                instruments.append(instrument)
                var track=member;track["id"]=tracks.count;track["instrument"]=instruments.count-1
                track["routingID"]="\(groupID)-pad\(i)";track["groupID"]=groupID
                tracks.append(track)
                outputs["track:\(groupID)-pad\(i)"]="bus:"+busID
                for index in patterns.indices {
                    var steps=patterns[index]["steps"] as! [[Any]]
                    steps.append([Any](repeating:NSNull(),count:patterns[index]["length"] as! Int));patterns[index]["steps"]=steps
                }
            }
            buses.append(["id":busID,"name":"Generated kit","type":"group","groupID":groupID,"volume":1,"pan":0,"mute":false,"solo":false])
            outputs["bus:"+busID]="master"
            routing["outputs"]=outputs;routing["buses"]=buses
            p["tracks"]=tracks;p["instruments"]=instruments;p["patterns"]=patterns;p["routing"]=routing
            p["groups"]=(p["groups"] as? [[String:Any]] ?? [])+[["id":groupID,"name":"Generated kit","color":"#e2a44f","collapsed":false,"mixerCollapsed":false,"busID":busID]]
            return p
        }
        func withHits(_ base:[String:Any],_ hits:[(track:Int,row:Int)])->[String:Any] {
            var p=base,patterns=p["patterns"] as! [[String:Any]],steps=patterns[0]["steps"] as! [[Any]]
            for hit in hits { steps[hit.track][hit.row]=["note":60,"velocity":127,"gate":8.0] }
            patterns[0]["steps"]=steps;p["patterns"]=patterns;return p
        }

        // 1. Load the kit while playing: the graph extends live and the playing tracks never drop out.
        let buffer=AVAudioPCMBuffer(pcmFormat:format,frameCapacity:512)!
        func quanta(_ count:Int) throws -> [Double] {
            try (0..<count).map { _ in
                guard try host.renderOfflineQuantum(frames:512,to:buffer) == .success,let pcm=buffer.floatChannelData else { throw AudioSampleError.invalid("Kit groups: offline render failed") }
                return rms(UnsafeBufferPointer(start:pcm[0],count:512))
            }
        }
        /// Applies a project while a concurrent task keeps rendering, the way the audio device
        /// does: graph edits wait for the render thread. Returns the rms of each rendered quantum.
        step("load base")
        _ = try await host.handle("engine.project",payload:["project":baseProject(),"replace":true,"awaitPlugins":true])
        step("play")
        _ = try await host.handle("transport.play",payload:["mode":"pattern","fromStart":true])
        step("render before")
        let before=try quanta(48)
        try require(before.dropFirst(4).allSatisfy { $0 > 1e-4 },"the four playing tracks are not continuously audible before the kit loads")
        let rebuildsBefore=routing()["fullRebuilds"] as? Int ?? 0,incrementalBefore=routing()["incrementalChanges"] as? Int ?? 0
        let nodesBefore=graph.attachedNodes.count
        let droppedBefore=[host.status()["droppedCommands"],host.status()["droppedEvents"]].map { String(describing:$0 ?? "") }
        let loaded=withKit(baseProject())
        step("load kit")
        var start=mach_absolute_time()
        _ = try await host.handle("engine.project",payload:["project":loaded,"awaitPlugins":true])
        let loadSeconds=seconds(start)
        step("render during")
        let during=try quanta(94)   // One second after the load, quantum by quantum.
        let status=routing()
        try require(graph.isRunning && host.status()["playing"] as? Bool == true,"loading the kit stopped the graph or the transport")
        try require(status["fullRebuilds"] as? Int == rebuildsBefore && (status["incrementalChanges"] as? Int ?? 0) > incrementalBefore,
                    "loading a kit rebuilt the routing graph instead of extending it: \(status["lastFullRebuildReason"] ?? "no reason")")
        let droppedAfter=[host.status()["droppedCommands"],host.status()["droppedEvents"]].map { String(describing:$0 ?? "") }
        try require(droppedAfter == droppedBefore,"the engine dropped commands or events while the kit loaded: \(droppedBefore) → \(droppedAfter)")
        let quietest=during.min() ?? 0
        try require(quietest > before.dropFirst(4).min()!*0.5,"the playing tracks dropped out while the kit loaded (quietest quantum rms \(quietest))")
        let state=try await snapshot()
        try require((state["tracks"] as? [[String:Any]])?.count == 20 && (state["groups"] as? [[String:Any]])?.count == 1,"the host project lacks the kit group and its sixteen tracks")
        metrics["load"]=["seconds":loadSeconds,"nodesBefore":nodesBefore,"nodesAfter":graph.attachedNodes.count,"quietestQuantumRMS":quietest,
            "playingQuantumRMSBefore":before.dropFirst(4).min()!,
            "fullRebuilds":status["fullRebuilds"] ?? 0,"incrementalChanges":status["incrementalChanges"] ?? 0]
        checks.append("Loading a 16-pad kit while playing extends the running routing graph incrementally (\(String(format:"%.0f",loadSeconds*1000)) ms) with no gap in the tracks already playing")

        // 2. Hits on every pad: member, bus and master stems add up; the second hat chokes the first.
        // Rows are 6000 frames (120 BPM, 4 lines per beat). Pad 1 at row 0, pad 2 at row 2, pads 3–16 at row 4.
        var hits:[(track:Int,row:Int)]=[(4,0),(5,2)]
        for pad in 2..<16 { hits.append((4+pad,4)) }
        let played=withHits(loaded,hits)
        step("hits")
        _ = try await host.handle("engine.project",payload:["project":played,"awaitPlugins":true])
        // The kit sounds live, through the graph that was extended while playing
        // (after the pads' samples finish decoding in the background).
        try await Task.sleep(nanoseconds:500_000_000)
        _ = try await host.handle("transport.play",payload:["mode":"pattern","fromStart":true])
        var livePeaks:[String:Float]=[:]
        for _ in 0..<12 {
            _ = try quanta(8)
            for (owner,value) in host.status()["busPeaks"] as? [String:[Float]] ?? [:] { livePeaks[owner]=max(livePeaks[owner] ?? 0,value.max() ?? 0) }
        }
        let silentLive=(["bus:kitbus"]+(0..<16).map { "track:kit-pad\($0)" }).filter { (livePeaks[$0] ?? 0) < 0.001 }
        metrics["livePeaks"]=livePeaks
        try require(silentLive.isEmpty,"the kit is silent in the live graph: \(silentLive)")
        _ = try await host.handle("transport.stop",payload:[:])
        let members=(0..<16).map { "track:kit-pad\($0)" }
        step("stems")
        let pcm=try await stems("kit-stems",members+["bus:kitbus","master"])
        for (pad,owner) in members.enumerated() { try require(rms(pcm[owner]!) > 0.001,"pad \(pad + 1) is silent") }
        let summed=(0..<pcm["bus:kitbus"]!.count).map { frame in members.reduce(Float(0)) { $0+pcm[$1]![frame] } }
        let busError=difference(pcm["bus:kitbus"]!,summed)
        try require(busError < 0.0001,"the group bus is not the sum of its members (difference \(busError))")
        let hat=pcm[members[0]]!
        let open=rms(hat[1000..<11_000]),choked=rms(hat[13_000..<23_000])
        try require(open > 0.005 && choked < open*0.01,"pad 2 did not choke pad 1 (rms before \(open), after \(choked))")
        let unchoked=pcm[members[2]]!
        try require(rms(unchoked[30_000..<44_000]) > 0.005,"a pad in no choke group was cut")
        metrics["stems"]=["busError":busError,"hatBeforeChoke":open,"hatAfterChoke":choked]
        checks.append("All sixteen pads and the group bus sound in the live graph; member stems sum exactly to the group bus; pad 2 chokes pad 1 while other pads ring on")

        // 3. The group preview captures only the bus and leaves project and transport untouched.
        step("preview")
        let beforePreview=try await snapshot(),transportBefore=host.status()
        let previewPath=folder.appendingPathComponent("kit-preview.wav").path
        let preview=try await host.handle("audio.kitPreview",payload:["groupID":"kit","path":previewPath,"pattern":0]) as! [String:Any]
        let previewPCM=try AudioSamples.decode(previewPath)
        let afterPreview=try await snapshot(),transportAfter=host.status()
        try require(preview["previewOwner"] as? String == "bus:kitbus" && previewPCM.left.allSatisfy(\.isFinite) && rms(previewPCM.left) > 0.001,"the kit preview did not capture the group bus")
        try require(try JSONSerialization.data(withJSONObject:beforePreview,options:.sortedKeys) == JSONSerialization.data(withJSONObject:afterPreview,options:.sortedKeys),"the kit preview changed the project")
        for key in ["playing","songMode","pattern","row","sequence"] {
            try require(String(describing:transportBefore[key] ?? "") == String(describing:transportAfter[key] ?? ""),"the kit preview changed transport \(key)")
        }
        metrics["previewFrames"]=previewPCM.left.count
        checks.append("Group preview renders only the kit's bus and keeps the project and transport unchanged")

        // 4. Member and bus inserts keep their identity when the group moves in the track order.
        guard let effect=BuiltinEffectsAudioUnit.catalogue().first,let component=effect["id"] as? String else { throw AudioSampleError.invalid("No built-in effect catalogue") }
        step("inserts")
        var inserts:[[String:Any]]=[]
        for owner in ["track:kit-pad0","track:kit-pad15","bus:kitbus"] {
            _ = try await host.handle("plugin.load",payload:["owner":owner,"kind":"effect","slot":0,"component":component])
            inserts.append(try await host.handle("plugin.state",payload:["owner":owner,"kind":"effect","slot":0]) as! [String:Any])
        }
        var moved=try await snapshot()
        let order=Array(4..<20)+Array(0..<4)
        let oldTracks=moved["tracks"] as! [[String:Any]]
        moved["tracks"]=order.enumerated().map { new,old -> [String:Any] in var t=oldTracks[old];t["id"]=new;return t }
        var movedPatterns=moved["patterns"] as! [[String:Any]]
        for index in movedPatterns.indices { let steps=movedPatterns[index]["steps"] as! [[Any]];movedPatterns[index]["steps"]=order.map { steps[$0] } }
        moved["patterns"]=movedPatterns
        moved["audioPlugins"]=(moved["audioPlugins"] as! [[String:Any]]).map { plugin in
            var copy=plugin;if let track=plugin["track"] as? Int { copy["track"]=order.firstIndex(of:track)! };return copy
        }
        _ = try await host.handle("engine.project",payload:["project":moved,"awaitPlugins":true])
        let afterMove=(try await snapshot())["audioPlugins"] as? [[String:Any]] ?? []
        for expected in inserts {
            let actual=afterMove.first { $0["owner"] as? String == expected["owner"] as? String && $0["kind"] as? String == "effect" }
            try require(actual?["instanceID"] as? String == expected["instanceID"] as? String && actual?["component"] as? String == component,
                        "moving the group replaced the insert on \(expected["owner"] ?? "?")")
        }
        checks.append("Member and group-bus inserts keep their instances when the group moves to the front of the session")

        // 4b. Deleting a member with an insert: the next member shifts into its channel and keeps its own insert.
        step("delete member")
        _ = try await host.handle("plugin.load",payload:["owner":"track:kit-pad1","kind":"effect","slot":0,"component":component])
        let survivor=try await host.handle("plugin.state",payload:["owner":"track:kit-pad1","kind":"effect","slot":0]) as! [String:Any]
        var deleted=try await snapshot()
        let deletedTracks=deleted["tracks"] as! [[String:Any]]
        let gone=deletedTracks.firstIndex { $0["routingID"] as? String == "kit-pad0" }!
        deleted["tracks"]=deletedTracks.enumerated().filter { $0.offset != gone }.enumerated().map { new,item -> [String:Any] in var t=item.element;t["id"]=new;return t }
        var deletedPatterns=deleted["patterns"] as! [[String:Any]]
        for index in deletedPatterns.indices { var steps=deletedPatterns[index]["steps"] as! [[Any]];steps.remove(at:gone);deletedPatterns[index]["steps"]=steps }
        deleted["patterns"]=deletedPatterns
        if var routing=deleted["routing"] as? [String:Any], var outputs=routing["outputs"] as? [String:Any] {
            outputs.removeValue(forKey:"track:kit-pad0");routing["outputs"]=outputs;deleted["routing"]=routing
        }
        deleted["audioPlugins"]=(deleted["audioPlugins"] as! [[String:Any]]).compactMap { plugin -> [String:Any]? in
            guard plugin["owner"] as? String != "track:kit-pad0" else { return nil }
            var copy=plugin;if let track=plugin["track"] as? Int, track > gone { copy["track"]=track-1 };return copy
        }
        _ = try await host.handle("engine.project",payload:["project":deleted,"awaitPlugins":true])
        let afterDelete=(try await snapshot())["audioPlugins"] as? [[String:Any]] ?? []
        try require(!afterDelete.contains { $0["owner"] as? String == "track:kit-pad0" },"the deleted member's insert survived")
        try require(afterDelete.first { $0["owner"] as? String == "track:kit-pad1" && $0["kind"] as? String == "effect" }?["instanceID"] as? String == survivor["instanceID"] as? String,
                    "the member that shifted into the deleted channel lost its own insert")
        inserts.removeAll { $0["owner"] as? String == "track:kit-pad0" }
        checks.append("Deleting a member with an insert removes only that insert; the member shifting into its channel keeps its own")

        // 5. Plain project and back: no stale processors, the kit's inserts restore from their descriptors.
        step("plain")
        let kitRestore=try await snapshot()
        let plain:[String:Any]=["id":"plain-after-kit","version":1,"bpm":120,"linesPerBeat":4,"masterVolume":1,
            "tracks":[["instrument":0,"volume":1,"pan":0]],
            "instruments":[["type":"sample","samplePath":padSamples[15],"rootNote":60,"volume":1,"mode":"oneshot","attack":0,"decay":0,"sustain":1,"release":0]],
            "patterns":[["id":0,"length":4,"steps":[[["note":60,"velocity":127,"gate":1]]],"automation":[]]],
            "sequence":[0],"activePattern":0,"loopSong":false,"audioPlugins":[],"delay":["mix":0],"reverb":["mix":0]]
        _ = try await host.handle("engine.project",payload:["project":plain,"replace":true,"awaitPlugins":true])
        let plainState=try await snapshot()
        try require((plainState["audioPlugins"] as? [[String:Any]] ?? []).isEmpty && routing()["enabled"] as? Bool == false,"the plain project kept the kit's processors or routing")
        let plainPath=folder.appendingPathComponent("plain-after-kit.wav").path
        _ = try await host.handle("audio.export",payload:["mode":"pattern","path":plainPath,"tail":0,"encoding":"float32"])
        try require(rms(try AudioSamples.decode(plainPath).left) > 0.01,"the plain project is silent after the kit")
        _ = try await host.handle("engine.project",payload:["project":kitRestore,"replace":true,"awaitPlugins":true])
        let restored=(try await snapshot())["audioPlugins"] as? [[String:Any]] ?? []
        try require(Set(restored.compactMap { $0["instanceID"] as? String }) == Set((kitRestore["audioPlugins"] as! [[String:Any]]).compactMap { $0["instanceID"] as? String }),
                    "returning from the plain project lost member or bus insert identities")
        for plugin in kitRestore["audioPlugins"] as! [[String:Any]] {
            let rows=try await host.handle("plugin.parameters",payload:["owner":plugin["owner"]!,"kind":plugin["kind"]!,"slot":plugin["slot"]!,"instanceID":plugin["instanceID"]!]) as? [[String:Any]] ?? []
            try require(!rows.isEmpty,"the restored insert on \(plugin["owner"] ?? "?") exposed no parameters")
        }
        checks.append("A plain project and back: no stale kit processors; member and bus inserts restore with their parameters")

        // 6. An old project with a kit lane (stacked hits on two Sounds) migrates to a group and plays.
        let lane:[String:Any]=["routingID":"lane0","name":"Kit lane","kind":"kit","kitID":"old","soundID":"s0","instrument":0,"volume":1]
        var oldRows=[Any](repeating:NSNull(),count:8)
        oldRows[0]=["note":60,"soundID":"s0","velocity":127,"gate":8,"kitEvents":[["note":60,"soundID":"s1","velocity":127,"gate":8]]]
        let oldSounds:[[String:Any]]=(0..<16).map { pad in ["id":"s\(pad)","name":"Old \(pad + 1)","channel":["volume":1,"pan":0],
            "instrument":["type":"sample","samplePath":pad < 2 ? padSamples[pad+4] : "","rootNote":60,"volume":1,"mode":"oneshot","attack":0,"decay":0,"sustain":1,"release":0]] }
        let legacy:[String:Any]=["id":"old-kit-lane","version":1,"bpm":120,"linesPerBeat":4,"masterVolume":1,
            "tracks":[lane],"instruments":[["type":"sample","samplePath":"","volume":0]],
            "kits":[["id":"old","name":"Old kit","busID":"oldbus","sounds":oldSounds]],
            "patterns":[["id":0,"length":8,"steps":[oldRows],"automation":[]]],"sequence":[0],"activePattern":0,"loopSong":false,
            "audioPlugins":[],"delay":["mix":0],"reverb":["mix":0],
            "routing":["version":1,"mixModel":"standard","buses":[["id":"oldbus","name":"Old kit","type":"group","volume":1]],"outputs":[:],"sends":[],"sidechains":[]]]
        step("migration")
        let migrated=ProjectMigration.migratedWithNotes(legacy)
        let migratedTracks=migrated.project["tracks"] as? [[String:Any]] ?? []
        try require(migrated.project["kits"] == nil && (migrated.project["groups"] as? [[String:Any]])?.count == 1 && migratedTracks.count == 2,
                    "the kit lane did not migrate to a group of its two loaded pads: \(migrated.notes)")
        start=mach_absolute_time()
        _ = try await host.handle("engine.project",payload:["project":migrated.project,"replace":true,"awaitPlugins":true])
        metrics["migratedLoadSeconds"]=seconds(start)
        let migratedOwners=migratedTracks.map { "track:"+($0["routingID"] as? String ?? "") }
        let old=try await stems("migrated-stems",migratedOwners+["bus:oldbus"])
        try require(migratedOwners.allSatisfy { rms(old[$0]!) > 0.001 },"a stacked hit of the migrated kit lane is silent")
        let oldSum=(0..<old["bus:oldbus"]!.count).map { frame in migratedOwners.reduce(Float(0)) { $0+old[$1]![frame] } }
        try require(difference(old["bus:oldbus"]!,oldSum) < 0.0001,"the migrated group bus is not the sum of its members")
        metrics["migrationNotes"]=migrated.notes
        checks.append("A migrated kit-lane project plays both stacked hits on their member tracks through the group bus")
        return checks
    }
}

#endif

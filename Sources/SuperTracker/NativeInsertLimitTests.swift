// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation

/// Sixteen inserts per channel, end to end: generated PCM and built-in EQs only, offline
/// manual rendering, no hardware device, MIDI source or installed vendor plug-in.
/// A flat EQ is transparent; turning one deep insert's output gain to -24 dB proves that
/// the insert is in that channel's signal path and nowhere else.
enum NativeInsertLimitTests {
    @MainActor static func run(output:URL) async throws -> [String] {
        let folder=output.appendingPathComponent("insert-limit",isDirectory:true)
        try FileManager.default.createDirectory(at:folder,withIntermediateDirectories:true)
        let format=AVAudioFormat(standardFormatWithSampleRate:48_000,channels:2)!
        let graph=AVAudioEngine()
        try graph.enableManualRenderingMode(.offline,format:format,maximumFrameCount:4096)
        let vst3=VST3Catalogue(configuration:.init(roots:[],cacheURL:folder.appendingPathComponent("vst3.json")))
        let vst2=VST2Catalogue(configuration:.init(roots:[],cacheURL:folder.appendingPathComponent("vst2.json")))
        var host:AudioHost?=try AudioHost(audioGraph:graph,vst3Catalogue:vst3,midiInputIDs:[],vst2Catalogue:vst2)
        defer { host?.shutdown() }
        var checks:[String]=[],metrics:[String:Any]=["hardware":false,"vendors":false,"insertSlots":MixerChannel.insertSlotCount]
        defer {
            metrics["checks"]=checks
            if let data=try? JSONSerialization.data(withJSONObject:metrics,options:[.prettyPrinted,.sortedKeys]) { try? data.write(to:folder.appendingPathComponent("metrics.json")) }
        }
        func require(_ okay:Bool,_ message:String) throws { if !okay { throw AudioSampleError.invalid("Insert limit: "+message) } }
        func handle(_ command:String,_ payload:[String:Any]) async throws -> Any { try await host!.handle(command,payload:payload) }
        // A one-second 220 Hz sine at -12 dBFS.
        let samplePath=folder.appendingPathComponent("sine.wav")
        do {
            let pcm=AVAudioPCMBuffer(pcmFormat:format,frameCapacity:48_000)!;pcm.frameLength=48_000
            for c in 0..<2 { for i in 0..<48_000 { pcm.floatChannelData![c][i]=Float(0.25*sin(2*Double.pi*220*Double(i)/48_000)) } }
            let file=try AVAudioFile(forWriting:samplePath,settings:format.settings,commonFormat:.pcmFormatFloat32,interleaved:false)
            try file.write(from:pcm)
        }
        let instrument:[String:Any]=["type":"sample","samplePath":samplePath.path,"rootNote":60,"volume":1,"mode":"oneshot","attack":0,"decay":0,"sustain":1,"release":0]
        let note:[String:Any]=["note":60,"velocity":127,"gate":1]
        // The note sounds for the first 0.25 s; measure 50...250 ms, after the parameter smoothing settles.
        func rms(_ x:[Float]) -> Float { guard x.count >= 12_000 else { return 0 }; let held=x[2_400..<12_000]; return sqrt(held.map { $0*$0 }.reduce(0,+)/Float(held.count)) }
        func difference(_ x:[Float],_ y:[Float]) -> Float { x.count == y.count ? zip(x,y).map { abs($0-$1) }.max() ?? 0 : .infinity }
        func valid(_ x:[Float]) -> Bool { x.count == 24_000 && x.allSatisfy(\.isFinite) && x.contains { abs($0) > 0.01 } }
        func gainAddress(_ payload:[String:Any]) async throws -> String {
            let rows=try await handle("plugin.parameters",payload) as? [[String:Any]] ?? []
            guard let address=rows.first(where:{ $0["name"] as? String == "Output gain" })?["address"] as? String, UInt64(address) != nil else {
                throw AudioSampleError.invalid("Insert limit: the EQ exposes no numeric Output gain address")
            }
            return address
        }
        let quiet=Float(pow(10,-24.0/20))

        // 1. Legacy tracks: track 0 holds sixteen inserts, track 1 five. Pattern automation of track 0's
        // insert 14 (slot 13) must reach that insert only. With the former nine-endpoint stride, slot 13
        // of track 0 addressed the endpoint of track 1's slot 4.
        let legacy:[String:Any]=["id":"insert-limit-legacy","version":1,"bpm":120.0,"linesPerBeat":4,"masterVolume":1,
            "tracks":[["instrument":0,"volume":1,"pan":0,"delaySend":0,"reverbSend":0],["instrument":1,"volume":1,"pan":0,"delaySend":0,"reverbSend":0]],
            "instruments":[instrument,instrument],
            "patterns":[["id":0,"length":4,"steps":[[note],[note]],"automation":[]]],
            "sequence":[0],"activePattern":0,"loopSong":false,"audioPlugins":[],"delay":["mix":0],"reverb":["mix":0]]
        _ = try await handle("engine.project",["project":legacy,"awaitPlugins":true])
        for (track,count) in [(0,MixerChannel.insertSlotCount),(1,5)] {
            for slot in 0..<count { _ = try await handle("plugin.load",["track":track,"kind":"effect","slot":slot,"component":"stfx:eq"]) }
        }
        var rejectedSlot=false
        do { _ = try await handle("plugin.load",["track":0,"kind":"effect","slot":MixerChannel.insertSlotCount,"component":"stfx:eq"]) } catch { rejectedSlot=true }
        try require(rejectedSlot,"slot \(MixerChannel.insertSlotCount) was accepted")
        let deepAddress=try await gainAddress(["track":0,"kind":"effect","slot":13])
        func renderTrack(_ track:Int,_ name:String) async throws -> [Float] {
            let path=folder.appendingPathComponent(name+".wav").path
            _ = try await handle("audio.export",["mode":"track","track":track,"path":path,"tail":0,"encoding":"float32"])
            return try AudioSamples.decode(path).left
        }
        let before0=try await renderTrack(0,"legacy-track0"),before1=try await renderTrack(1,"legacy-track1")
        try require(valid(before0) && valid(before1),"sixteen-insert legacy tracks did not render audibly")
        var automated=try await handle("engine.snapshot",[:]) as! [String:Any]
        let legacyPlugins=automated["audioPlugins"] as? [[String:Any]] ?? []
        try require(legacyPlugins.filter { $0["kind"] as? String == "effect" }.count == MixerChannel.insertSlotCount+5,"the snapshot lost deep track inserts")
        var patterns=automated["patterns"] as! [[String:Any]]
        patterns[0]["automation"]=[["track":0,"target":"plugin:effect:13:\(deepAddress)","interpolation":"step","points":[["row":0,"value":0.0]]]]
        automated["patterns"]=patterns
        _ = try await handle("engine.project",["project":automated,"awaitPlugins":true])
        let after0=try await renderTrack(0,"legacy-track0-automated"),after1=try await renderTrack(1,"legacy-track1-automated")
        let legacyRatio=rms(after0)/max(1e-9,rms(before0)),neighbour=difference(after1,before1)
        metrics["legacyAutomatedRatio"]=legacyRatio;metrics["legacyNeighbourDifference"]=neighbour
        try require(abs(legacyRatio-quiet) < quiet*0.25,"automation of track 0 insert 14 did not reach it (level ratio \(legacyRatio))")
        try require(neighbour < 0.00001,"automation of track 0 insert 14 changed track 1 (difference \(neighbour))")
        checks.append("Track insert 14 automation reaches only its own insert; a track's deep slots do not collide with the next track's endpoints")

        // 2. Standard routing: a kit Group with thirteen inserts and a full sixteen-insert Master (the last
        // endpoint of the rack) render, react to their deepest inserts, save, reopen and render the same.
        // A kit group: its first pad plays the sample, through its group bus.
        let kitProject:[String:Any]=["id":"insert-limit-kit","version":1,"bpm":120.0,"linesPerBeat":4,"masterVolume":1,
            "tracks":[["routingID":"pad0","name":"Pad 1","groupID":"k0","pad":0,"instrument":0,"volume":1]],
            "instruments":[instrument],"groups":[["id":"k0","name":"Deep Kit","busID":"g0"]],
            "patterns":[["id":0,"length":4,"steps":[[["note":60,"velocity":127,"gate":1]]],"automation":[]]],
            "sequence":[0],"activePattern":0,"loopSong":false,"audioPlugins":[],"delay":["mix":0],"reverb":["mix":0],
            "routing":["version":1,"mixModel":"standard","buses":[["id":"g0","name":"Deep Kit","type":"group","groupID":"k0","volume":1]],
                       "outputs":["track:pad0":"bus:g0"],"sends":[],"sidechains":[]]]
        _ = try await handle("engine.project",["project":kitProject,"replace":true,"awaitPlugins":true])
        var instanceIDs:[String:String]=[:]
        for (owner,count) in [("bus:g0",13),("master",MixerChannel.insertSlotCount)] {
            for slot in 0..<count {
                let loaded=try await handle("plugin.load",["owner":owner,"kind":"effect","slot":slot,"component":"stfx:eq"]) as! [String:Any]
                instanceIDs["\(owner)/\(slot)"]=loaded["instanceID"] as? String
            }
        }
        func renderStems(_ name:String) async throws -> [String:[Float]] {
            let result=try await handle("audio.export",["mode":"stems","path":folder.appendingPathComponent(name).path,"owners":["bus:g0","master"],"tail":0,"encoding":"float32"]) as! [String:Any]
            var pcm:[String:[Float]]=[:]
            for record in result["stems"] as? [[String:Any]] ?? [] { if let owner=record["owner"] as? String,let path=record["path"] as? String { pcm[owner]=try AudioSamples.decode(path).left } }
            try require(Set(pcm.keys) == ["bus:g0","master"],"stems were omitted: \(pcm.keys.sorted())")
            return pcm
        }
        let flat=try await renderStems("kit-flat")
        try require(valid(flat["bus:g0"]!) && valid(flat["master"]!),"thirteen Group inserts and sixteen Master inserts did not render audibly")
        for (owner,slot) in [("bus:g0",12),("master",MixerChannel.insertSlotCount-1)] {
            let target:[String:Any]=["owner":owner,"kind":"effect","slot":slot,"instanceID":instanceIDs["\(owner)/\(slot)"]!]
            let address=try await gainAddress(target)
            var payload=target;payload["address"]=address;payload["value"]=0.0;payload["normalized"]=true
            _ = try await handle("plugin.parameter",payload)
        }
        let deep=try await renderStems("kit-deep")
        let groupRatio=rms(deep["bus:g0"]!)/max(1e-9,rms(flat["bus:g0"]!)),masterRatio=rms(deep["master"]!)/max(1e-9,rms(flat["master"]!))
        metrics["groupRatio"]=groupRatio;metrics["masterRatio"]=masterRatio
        try require(abs(groupRatio-quiet) < quiet*0.25,"Group insert 13 is not in the Group's signal path (ratio \(groupRatio))")
        try require(abs(masterRatio-quiet*quiet) < quiet*quiet*0.25,"Master insert 16 is not in the Master's signal path (ratio \(masterRatio))")
        checks.append("Kit Group insert 13 and Master insert 16 process their channel's audio")

        let snapshot=try await handle("engine.snapshot",[:]) as! [String:Any]
        let store=try ProjectStore(directory:folder.appendingPathComponent("store"))
        let projectURL=folder.appendingPathComponent("Deep inserts.supertracker")
        _ = try store.save(to:projectURL,snapshot:snapshot,collectSamples:false)
        let reopened=try store.read(projectURL)
        func layout(_ project:[String:Any],_ owner:String) -> [Int] { (project["audioPlugins"] as? [[String:Any]] ?? []).filter { $0["owner"] as? String == owner && $0["kind"] as? String == "effect" }.compactMap { $0["slot"] as? Int }.sorted() }
        try require(layout(reopened,"bus:g0") == Array(0..<13) && layout(reopened,"master") == Array(0..<MixerChannel.insertSlotCount),"Save/Open changed the deep insert layout")
        await host!.shutdownAndWait();host=nil
        let freshGraph=AVAudioEngine()
        try freshGraph.enableManualRenderingMode(.offline,format:format,maximumFrameCount:4096)
        host=try AudioHost(audioGraph:freshGraph,vst3Catalogue:vst3,midiInputIDs:[],vst2Catalogue:vst2)
        _ = try await handle("engine.project",["project":reopened,"replace":true,"awaitPlugins":true])
        await host!.pluginRestorationBarrier()?.value
        let restored=try await handle("engine.snapshot",[:]) as! [String:Any]
        try require(layout(restored,"bus:g0") == Array(0..<13) && layout(restored,"master") == Array(0..<MixerChannel.insertSlotCount),"reopening did not restore every deep insert")
        let reopenedPCM=try await renderStems("kit-reopened")
        let reopenError=max(difference(reopenedPCM["bus:g0"]!,deep["bus:g0"]!),difference(reopenedPCM["master"]!,deep["master"]!))
        metrics["reopenMaximumError"]=reopenError
        try require(reopenError < 0.0001,"the reopened project renders differently (difference \(reopenError))")
        checks.append("A project with thirteen Group and sixteen Master inserts saves, reopens in a fresh host and renders identically")
        return checks
    }
}

#endif

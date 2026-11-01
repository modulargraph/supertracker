// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation

/// Real Rack rollback + native cache/ACK regression. Only the original synth,
/// first-party built-in effects and a private manual graph are constructed.
enum NativeRenderRestartTests {
    @MainActor static func run(output:URL) async throws -> [String] {
        let folder=output.appendingPathComponent("render-restart",isDirectory:true)
        try FileManager.default.createDirectory(at:folder,withIntermediateDirectories:true)
        let key="SUPERTRACKER_RENDER_CLOCK_DIAGNOSTICS",previous=ProcessInfo.processInfo.environment[key]
        setenv(key,"1",1)
        defer { if let previous { setenv(key,previous,1) } else { unsetenv(key) } }
        var checks:[String]=[],metrics:[String:Any]=[:]
        defer {
            metrics["checks"]=checks
            if let data=try? JSONSerialization.data(withJSONObject:metrics,options:[.prettyPrinted,.sortedKeys]) {
                try? data.write(to:folder.appendingPathComponent("metrics.json"),options:.atomic)
            }
        }
        func require(_ condition:Bool,_ message:String) throws { if !condition { throw AudioSampleError.invalid("Render restart: "+message) } }
        let graph=AVAudioEngine(),format=AVAudioFormat(standardFormatWithSampleRate:48_000,channels:2)!
        try graph.enableManualRenderingMode(.offline,format:format,maximumFrameCount:4096)
        let injected=NSError(domain:"SuperTracker.RenderRestart",code:941)
        var failResume=false
        let host=try AudioHost(beforeEffectOrderResume:{ if failResume { throw injected } },audioGraph:graph,
            vst3Catalogue:VST3Catalogue(configuration:.init(roots:[],cacheURL:folder.appendingPathComponent("vst3.json"))),midiInputIDs:[],
            vst2Catalogue:VST2Catalogue(configuration:.init(roots:[],cacheURL:folder.appendingPathComponent("vst2.json"))))
        defer { host.shutdown() }
        let rows=[Any](repeating:NSNull(),count:256)
        let project:[String:Any]=["id":"render-restart","version":1,"name":"Render restart","bpm":120,"linesPerBeat":4,
            "masterVolume":0,"tracks":[["name":"Original synth","routingID":"lane","instrument":0,"volume":1]],
            "instruments":[["type":"synth","waveform":"sine","volume":0.5,"attack":0,"decay":0,"sustain":1,"release":0]],
            "patterns":[["id":0,"length":256,"steps":[rows],"automation":[]]],"sequence":[0],"activePattern":0,
            "audioPlugins":[],"loopSong":false,"delay":["mix":0],"reverb":["mix":0]]
        _ = try await host.handle("engine.project",payload:["project":project,"replace":true,"awaitPlugins":true])
        guard let component=BuiltinEffectsAudioUnit.catalogue().first?["id"] as? String else { throw AudioSampleError.invalid("Missing original built-in effect") }
        for slot in 0..<2 {
            _ = try await host.handle("plugin.load",payload:["track":0,"kind":"effect","slot":slot,"component":component])
            _ = try await host.handle("plugin.parameter",payload:["track":0,"kind":"effect","slot":slot,"bypass":true])
        }
        func descriptors() async throws -> [[String:Any]] {
            let value=try await host.handle("engine.snapshot",payload:[:]) as! [String:Any]
            return value["audioPlugins"] as? [[String:Any]] ?? []
        }
        let beforePlugins=try await descriptors()
        let buffer=AVAudioPCMBuffer(pcmFormat:format,frameCapacity:4096)!
        func render(_ count:UInt32) throws {
            try require(try host.renderOfflineQuantum(frames:count,to:buffer) == .success,"Manual render failed")
            for channel in 0..<2 { for n in 0..<Int(count) {
                try require(buffer.floatChannelData![channel][n] == 0,"Muted output produced nonzero/nonfinite PCM")
            } }
        }
        func clock() async throws -> [String:Any] {
            let value=try await host.handle("audio.renderDiagnostics",payload:[:]) as! [String:Any]
            return value["renderClock"] as? [String:Any] ?? [:]
        }
        func epoch(_ value:[String:Any])->UInt64 { UInt64(value["retainedClockEpoch"] as? String ?? "") ?? 0 }
        // Arm before the baseline so its legitimate epoch transition cannot
        // masquerade as the graph-cache reset being tested.
        _ = try await host.handle("transport.recordArm",payload:["armed":true])
        _ = try await host.handle("transport.play",payload:["mode":"pattern","fromStart":true])
        for _ in 0..<24 { try render(4096) }
        let beforeClock=try await clock(),beforeSeconds=(host.status()["seconds"] as! NSNumber).doubleValue
        try require(epoch(beforeClock)>0 && graph.isRunning,"Fixture did not warm a running retained cache")
        failResume=true
        var caught:NSError?
        do { _ = try await host.handle("plugin.move",payload:["track":0,"fromSlot":0,"toSlot":1]) }
        catch { caught=error as NSError }
        failResume=false
        try require(caught?.domain == injected.domain && caught?.code == injected.code && graph.isRunning,
            "Rollback lost the original error or running intent")
        let afterPlugins=try await descriptors()
        for prior in beforePlugins {
            guard let after=afterPlugins.first(where: { $0["instanceID"] as? String == prior["instanceID"] as? String }) else { throw AudioSampleError.invalid("Rollback replaced an original effect") }
            try require(after["slot"] as? Int == prior["slot"] as? Int && after["component"] as? String == prior["component"] as? String && after["bypass"] as? Bool == prior["bypass"] as? Bool,"Rollback changed an effect binding")
        }
        checks.append("Failed effect move restores exact instances/slots and preserves running intent and original error")
        var entered=false,finished=false
        let request=Task { @MainActor in
            entered=true;defer { finished=true }
            return try await host.handle("note.on",payload:["track":0,"instrument":0,"note":60,"velocity":100,"captureInput":true,"inputOwner":"after-rollback"]) as! [String:Any]
        }
        while !entered { await Task.yield() }
        let firstFrame=graph.manualRenderingSampleTime
        try render(64)
        let deadline=ContinuousClock.now.advanced(by:.seconds(2))
        while !finished && ContinuousClock.now < deadline { try await Task.sleep(for:.milliseconds(5)) }
        try require(finished,"Post-rollback input did not complete")
        let response=try await request.value,ack=response["inputAcknowledgement"] as? [String:Any] ?? [:]
        let afterClock=try await clock(),afterSeconds=(host.status()["seconds"] as! NSNumber).doubleValue
        metrics["beforeClock"]=beforeClock;metrics["afterClock"]=afterClock;metrics["ack"]=ack
        metrics["firstGraphFrame"]=firstFrame;metrics["advancedFrames"]=(afterSeconds-beforeSeconds)*48_000
        try require(epoch(afterClock)>epoch(beforeClock),"Explicit Rack stop/resume reused the prior graph cache epoch")
        try require(ack["applied"] as? Bool == true && ack["recording"] as? Bool == true && ack["graphFrame"] as? String == String(firstFrame),"First post-rollback quantum replayed cache instead of applying the queued input")
        try require(abs((afterSeconds-beforeSeconds)*48_000-64)<0.00001,"Restart lost or duplicated transport advancement")
        checks.append("First post-stop quantum uses a fresh cache epoch and applies the queued input at its actual graph timestamp")
        checks.append("Muted restarted graph advances exactly 64 frames once and outputs finite exact-zero PCM")
        // Model a Core Audio device interruption: the graph stops independently
        // of rack mutation hooks, and its hardware sample clock starts afresh.
        let externalStopClock = try await clock()
        graph.stop()
        _ = try await host.handle("note.on",payload:["track":0,"instrument":0,"note":64,"velocity":100])
        try require(graph.isRunning,"A note did not restart an externally stopped graph")
        let externalStopSeconds = (host.status()["seconds"] as! NSNumber).doubleValue
        try render(64)
        let recoveredClock = try await clock()
        let recoveredSeconds = (host.status()["seconds"] as! NSNumber).doubleValue
        try require(epoch(recoveredClock)>epoch(externalStopClock),"External device stop reused the previous render cache epoch")
        try require(abs((recoveredSeconds-externalStopSeconds)*48_000-64)<0.00001,"External restart lost or duplicated transport advancement")
        checks.append("An externally stopped audio graph restarts with a fresh cache epoch and advances exactly once")
        metrics["externalRestartClock"] = recoveredClock
        // Pattern-editor Solo publishes an ordinary project update. Its routing
        // topology changes while transport is already playing.
        var routed=project
        routed["audioPlugins"]=beforePlugins
        var soloTracks=routed["tracks"] as! [[String:Any]]
        soloTracks.append(["name":"Other synth","routingID":"other","instrument":0,"volume":1])
        routed["tracks"]=soloTracks
        routed["routing"]=["version":1,"mixModel":"standard","buses":[],
            "outputs":["track:lane":"master","track:other":"master"],"sends":[],"sidechains":[]] as [String:Any]
        _ = try await host.handle("engine.project",payload:["project":routed,"awaitPlugins":true])
        for mixModel in ["standard","legacy"] {
        var route=routed["routing"] as! [String:Any];route["mixModel"]=mixModel;routed["routing"]=route
        _ = try await host.handle("engine.project",payload:["project":routed,"awaitPlugins":true])
        let soloNodes=Set(graph.attachedNodes.map(ObjectIdentifier.init))
        for (solo,mute) in [(true,false),(false,false),(false,true),(false,false),(true,false),(false,false)] {
            var tracks=routed["tracks"] as! [[String:Any]]
            tracks[0]["solo"]=solo;tracks[0]["mute"]=mute;routed["tracks"]=tracks
            let beforeSolo=(host.status()["seconds"] as! NSNumber).doubleValue
            _ = try await host.handle("engine.project",payload:["project":routed,"awaitPlugins":true])
            try require(graph.isRunning && host.status()["playing"] as? Bool == true,"Solo stopped the active transport")
            try require(Set(graph.attachedNodes.map(ObjectIdentifier.init)) == soloNodes,"Solo/Mute rebuilt live graph nodes instead of updating gains")
            try render(64)
            let afterSolo=(host.status()["seconds"] as! NSNumber).doubleValue
            try require(abs((afterSolo-beforeSolo)*48_000-64)<0.00001,"Solo routing edit replayed a stale clock or stopped transport advancement")
        }
        }
        checks.append("Repeated live Solo, clear-Solo and Mute toggles preserve playback and advance exactly once after each graph rebuild")
        checks.append(try await realtimePause(folder:folder))
        return checks
    }

    /// Real-time regression for the "transport stuck" report: AVAudioEngine.pause()
    /// restarts the main mixer's render clock at zero while the engine cache stays at
    /// the old graph frame. Rack pauses (insert reorder here; VST3 state capture on
    /// every autosave with the hardware editors) must invalidate that cache, or Play,
    /// Stop and acknowledged note entry stall for as long as audio had been running.
    /// A manual-rendering graph keeps its clock, so this needs the real output device
    /// (muted master).
    @MainActor private static func realtimePause(folder:URL) async throws -> String {
        func require(_ condition:Bool,_ message:String) throws { if !condition { throw AudioSampleError.invalid("Realtime pause: "+message) } }
        let host=try AudioHost(beforeEffectOrderResume:nil,midiInputIDs:[],midiInputSourceProvider:{ [] },midiOutputDestinationProvider:{ [] })
        defer { host.shutdown() }
        let rows=[Any](repeating:NSNull(),count:256)
        let project:[String:Any]=["id":"render-pause","version":1,"name":"Render pause","bpm":120,"linesPerBeat":4,
            "masterVolume":0,"tracks":[["name":"Original synth","routingID":"lane","instrument":0,"volume":0]],
            "instruments":[["type":"synth","waveform":"sine","volume":0,"attack":0,"decay":0,"sustain":1,"release":0]],
            "patterns":[["id":0,"length":256,"steps":[rows],"automation":[]]],"sequence":[0],"activePattern":0,
            "audioPlugins":[],"loopSong":true,"delay":["mix":0],"reverb":["mix":0]]
        _ = try await host.handle("engine.project",payload:["project":project,"replace":true,"awaitPlugins":true])
        guard let component=BuiltinEffectsAudioUnit.catalogue().first?["id"] as? String else { throw AudioSampleError.invalid("Missing original built-in effect") }
        for slot in 0..<2 {
            _ = try await host.handle("plugin.load",payload:["track":0,"kind":"effect","slot":slot,"component":component])
            _ = try await host.handle("plugin.parameter",payload:["track":0,"kind":"effect","slot":slot,"bypass":true])
        }
        func seconds() -> Double { (host.status()["seconds"] as? NSNumber)?.doubleValue ?? 0 }
        func masterFrame() async throws -> Double {
            let value=try await host.handle("audio.renderDiagnostics",payload:[:]) as? [String:Any] ?? [:]
            return ((value["renderClock"] as? [String:Any])?["masterSampleTime"] as? NSNumber)?.doubleValue ?? -1
        }
        _ = try await host.handle("transport.play",payload:["mode":"pattern","fromStart":true])
        // Run longer than the engine's retained history (65536 frames) so a stale
        // cache cannot be caught up within the observation window below.
        let warm=ContinuousClock.now.advanced(by:.seconds(6))
        while seconds() < 1.8 && ContinuousClock.now < warm { try await Task.sleep(for:.milliseconds(20)) }
        try require(seconds() >= 1.8 && host.status()["playing"] as? Bool == true,"Real-time playback did not start on the output device")
        let masterBefore=try await masterFrame()
        _ = try await host.handle("plugin.move",payload:["track":0,"fromSlot":0,"toSlot":1])
        let resumed=seconds()
        try await Task.sleep(for:.milliseconds(600))
        let advanced=seconds()-resumed,masterAfter=try await masterFrame()
        let metrics:[String:Any]=["masterBefore":masterBefore,"masterAfter":masterAfter,"advancedSeconds":advanced]
        if let data=try? JSONSerialization.data(withJSONObject:metrics,options:[.prettyPrinted,.sortedKeys]) {
            try? data.write(to:folder.appendingPathComponent("realtime-pause.json"),options:.atomic)
        }
        try require(host.status()["playing"] as? Bool == true,"The insert reorder stopped playback")
        try require(advanced > 0.3,"Playback froze after a rack pause (advanced \(advanced)s in 0.6s; master clock \(masterBefore) -> \(masterAfter))")
        _ = try await host.handle("transport.stop",payload:[:])
        return "A real-time rack pause (insert reorder) resumes playback immediately although the mixer clock restarts"
    }
}

#endif

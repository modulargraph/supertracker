// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation
import AudioToolbox
import CoreMIDI

/// Public-command integration with original test AUs and an offline-only engine.
/// No installed third-party plug-in or physical audio output is opened.
enum NativeRoutingHostTests {
    private static let manufacturer:OSType = 0x53745274 // StRt
    private static let detector:OSType = 0x72686B79 // rhky
    private static let saturator:OSType = 0x72687374 // rhst
    private static let delay:OSType = 0x72683634 // rh64
    private static let mute:OSType = 0x72686D74 // rhmt
    private static let registration:Void = {
        for (subtype,name) in [(detector,"Routing detector"),(saturator,"Routing saturation"),(delay,"Routing 64-frame delay"),(mute,"Routing mute")] {
            AUAudioUnit.registerSubclass(NativeRoutingHostUnit.self,as:description(subtype),name:"SuperTracker Test: "+name,version:1)
        }
    }()
    private static func description(_ subtype:OSType)->AudioComponentDescription {
        AudioComponentDescription(componentType:kAudioUnitType_Effect,componentSubType:subtype,
            componentManufacturer:manufacturer,componentFlags:0,componentFlagsMask:0)
    }
    private static func identifier(_ subtype:OSType)->String { "\(kAudioUnitType_Effect):\(subtype):\(manufacturer)" }

    @MainActor static func run(output:URL) async throws -> [String] {
        _ = registration
        let folder = output.appendingPathComponent("routing-host",isDirectory:true)
        try FileManager.default.createDirectory(at:folder,withIntermediateDirectories:true)
        let format = AVAudioFormat(standardFormatWithSampleRate:48_000,channels:2)!
        let graph = AVAudioEngine()
        try graph.enableManualRenderingMode(.offline,format:format,maximumFrameCount:4096)
        let midiSources = RoutingMIDISourceList()
        let host = try AudioHost(audioGraph:graph,midiInputIDs:[],midiInputSourceProvider:{midiSources.snapshot()})
        defer { host.shutdown() }
        var midiClient:MIDIClientRef = 0,midiSource:MIDIEndpointRef = 0
        guard MIDIClientCreateWithBlock("SuperTracker routing test" as CFString,&midiClient,nil) == noErr else {
            throw AudioSampleError.invalid("Cannot create the routing test's virtual MIDI client.")
        }
        defer {
            host.shutdown()
            if midiSource != 0 { MIDIEndpointDispose(midiSource) }
            MIDIClientDispose(midiClient)
        }
        guard MIDISourceCreate(midiClient,"Routing validation test source" as CFString,&midiSource) == noErr else {
            throw AudioSampleError.invalid("Cannot create the routing test's virtual MIDI source.")
        }
        guard MIDIObjectSetIntegerProperty(midiSource,kMIDIPropertyPrivate,1) == noErr else {
            throw AudioSampleError.invalid("Cannot isolate the routing test's MIDI source from other applications.")
        }
        midiSources.set([midiSource])
        var midiSourceID:Int32 = 0
        MIDIObjectGetIntegerProperty(midiSource,kMIDIPropertyUniqueID,&midiSourceID)
        _ = try await host.handle("midi.configure",payload:["inputIDs":[midiSourceID]])
        graph.stop() // MIDI setup starts the engine; this fixture explicitly begins idle.
        var incomingMIDI = 0
        host.onEvent = { name,_ in if name == "midi.note" || name == "midi.message" { incomingMIDI += 1 } }
        var passed:[String] = [], measurements:[String:Any] = [:]
        defer {
            measurements["checks"] = passed; measurements["physicalPlayback"] = false
            if let data = try? JSONSerialization.data(withJSONObject:measurements,options:[.prettyPrinted,.sortedKeys]) {
                try? data.write(to:folder.appendingPathComponent("metrics.json"),options:.atomic)
            }
        }
        func require(_ condition:Bool,_ message:String) throws {
            if !condition { throw AudioSampleError.invalid("Routing host regression: "+message) }
        }
        func writeJSON(_ value:Any,_ name:String) throws {
            try JSONSerialization.data(withJSONObject:value,options:[.sortedKeys,.prettyPrinted])
                .write(to:folder.appendingPathComponent(name+".json"),options:.atomic)
        }
        func jsonSafePlist(_ value:Any)->Any {
            if let data = value as? Data { return ["$data":data.base64EncodedString()] }
            if let date = value as? Date { return ["$date":date.timeIntervalSince1970] }
            if let object = value as? [String:Any] { return object.mapValues(jsonSafePlist) }
            if let array = value as? [Any] { return array.map(jsonSafePlist) }
            return value
        }
        func semanticSnapshot(_ value:[String:Any])->[String:Any] {
            var result = value
            if let plugins = value["audioPlugins"] as? [[String:Any]] {
                result["audioPlugins"] = plugins.map { plugin -> [String:Any] in
                    var decoded = plugin
                    if let encoded = plugin["state"] as? String,let data = Data(base64Encoded:encoded),
                       let state = try? PropertyListSerialization.propertyList(from:data,options:[],format:nil) {
                        decoded["state"] = jsonSafePlist(state)
                    }
                    return decoded
                }
            }
            return result
        }
        func differingPaths(_ left:Any,_ right:Any,path:String = "$")->[String] {
            if let a = left as? [String:Any],let b = right as? [String:Any] {
                return Set(a.keys).union(b.keys).sorted().flatMap { key -> [String] in
                    guard let x = a[key],let y = b[key] else { return [path+"."+key+" (missing)"] }
                    return differingPaths(x,y,path:path+"."+key)
                }
            }
            if let a = left as? [Any],let b = right as? [Any] {
                var differences = a.count == b.count ? [] : [path+".count"]
                for index in 0..<min(a.count,b.count) { differences += differingPaths(a[index],b[index],path:path+"[\(index)]") }
                return differences
            }
            return NSDictionary(dictionary:["value":left]).isEqual(to:["value":right]) ? [] : [path]
        }
        func snapshotEvidence(_ name:String,expected:[String:Any],actual:[String:Any]) throws {
            try writeJSON(expected,name+"-expected"); try writeJSON(actual,name+"-actual")
            let semanticExpected = semanticSnapshot(expected),semanticActual = semanticSnapshot(actual)
            try writeJSON(semanticExpected,name+"-expected-semantic"); try writeJSON(semanticActual,name+"-actual-semantic")
            measurements[name+"RawDifferences"] = differingPaths(expected,actual)
            measurements[name+"SemanticDifferences"] = differingPaths(semanticExpected,semanticActual)
        }
        var snapshotIndex = 0
        func snapshot() async throws -> [String:Any] {
            let result = try await host.handle("engine.snapshot",payload:[:]) as? [String:Any] ?? [:]
            snapshotIndex += 1; try writeJSON(result,String(format:"snapshot-%02d",snapshotIndex))
            return result
        }
        func apply(_ value:[String:Any]) async throws -> [String:Any] {
            let result = try await host.handle("routing.apply",payload:["project":value]) as? [String:Any] ?? [:]
            await host.pluginRestorationBarrier()?.value
            return result
        }
        func render(_ name:String,owners:[String]? = nil,mode:String = "song",tail:Double = 0) async throws -> (DecodedSample?,[String:DecodedSample],[String:Any]) {
            let destination = folder.appendingPathComponent(name+(mode == "stems" ? "" : ".wav"),isDirectory:mode == "stems")
            var payload:[String:Any] = ["path":destination.path,"mode":mode,"tail":tail]
            if let owners { payload["owners"] = owners }
            let result = try await host.handle("audio.export",payload:payload) as? [String:Any] ?? [:]
            var stems:[String:DecodedSample] = [:]
            for item in result["stems"] as? [[String:Any]] ?? [] {
                if let owner = item["owner"] as? String,let path = item["path"] as? String { stems[owner] = try AudioSamples.decode(path) }
            }
            return (mode == "stems" ? nil : try AudioSamples.decode(destination.path),stems,result)
        }
        func difference(_ a:DecodedSample,_ b:DecodedSample)->Double {
            guard a.left.count == b.left.count && a.right.count == b.right.count else { return .infinity }
            return max(zip(a.left,b.left).reduce(0) { max($0,abs(Double($1.0-$1.1))) },
                       zip(a.right,b.right).reduce(0) { max($0,abs(Double($1.0-$1.1))) })
        }
        func onset(_ sample:DecodedSample)->[Int] { [sample.left,sample.right].map { $0.firstIndex(where:{abs($0) > 0.000001}) ?? -1 } }
        func energy(_ sample:DecodedSample)->Double { sqrt(sample.left.reduce(0) { $0+Double($1)*Double($1) }/Double(max(1,sample.left.count))) }
        func publishMIDI(_ bytes:[UInt8]) throws {
            var list = MIDIPacketList()
            let status = withUnsafeMutablePointer(to:&list) { pointer -> OSStatus in
                let first = MIDIPacketListInit(pointer)
                _ = bytes.withUnsafeBufferPointer { MIDIPacketListAdd(pointer,MemoryLayout<MIDIPacketList>.size,first,0,bytes.count,$0.baseAddress!) }
                return MIDIReceived(midiSource,pointer)
            }
            try require(status == noErr,"Virtual MIDI input publication failed.")
        }
        let toneURL = folder.appendingPathComponent("tone-source.wav"),keyURL = folder.appendingPathComponent("key-source.wav")
        let tone = (0..<24_000).map { frame in Float(sin(Double(frame)*2*Double.pi*220/48_000)*0.25) }
        let key = [Float](repeating:0.4,count:24_000)
        try AudioSamples.write(DecodedSample(path:toneURL.path,sampleRate:48_000,left:tone,right:tone,originalChannels:2),to:toneURL)
        try AudioSamples.write(DecodedSample(path:keyURL.path,sampleRate:48_000,left:key,right:key,originalChannels:2),to:keyURL)
        let rows:[Any] = [["note":60,"instrument":0,"velocity":127,"gate":8],NSNull(),NSNull(),NSNull()]
        let keyRows:[Any] = [["note":60,"instrument":1,"velocity":127,"gate":8],NSNull(),NSNull(),NSNull()]
        var project:[String:Any] = ["version":1,"name":"Routing host fixture","bpm":120,"linesPerBeat":4,"masterVolume":0.7,
            "activePattern":0,"loopSong":false,"metronome":false,
            "tracks":[["routingID":"tone","name":"Tone","instrument":0,"volume":0.8,"pan":0],
                      ["routingID":"key","name":"Detector","instrument":1,"volume":0.7,"pan":0]],
            "instruments":[toneURL,keyURL].map { ["type":"sample","samplePath":$0.path,"rootNote":60,"volume":1,
                "mode":"oneshot","attack":0,"decay":0,"sustain":1,"release":0.001,"cutoff":20_000,"resonance":0,"drive":0] as [String:Any] },
            "patterns":[["id":0,"length":4,"steps":[rows,keyRows],"automation":[]]],"sequence":[0],"audioPlugins":[],
            "delay":["mix":0],"reverb":["mix":0],
            "routing":["version":1,"mixModel":"standard",
                "buses":[["id":"group","name":"Group","type":"group","volume":0.6,"pan":0],
                         ["id":"return","name":"Return","type":"return","volume":0.4,"pan":0]],
                "outputs":["track:tone":"bus:group","track:key":NSNull(),"bus:group":"master","bus:return":"master"],
                "sends":[["id":"room","source":"track:tone","destination":"bus:return","tap":"postFader","gain":0.2,"enabled":true]],"sidechains":[]]]
        let caps = try await host.handle("routing.capabilities",payload:[:]) as? [String:Any] ?? [:]
        try require(caps["supported"] as? Bool == true && caps["fullGraphStems"] as? Bool != true,"Legacy state advertised an unavailable full-graph export path.")
        var ownerExportRejected = false
        do { _ = try await host.handle("audio.export",payload:["path":folder.appendingPathComponent("invalid-legacy-owners").path,"mode":"stems","owners":["master"]]) }
        catch { ownerExportRejected = true }
        try require(ownerExportRejected && !graph.isRunning,"Legacy export silently ignored an explicit master/owner selection.")
        let applied = try await apply(project)
        let enabledCaps = try await host.handle("routing.capabilities",payload:[:]) as? [String:Any] ?? [:]
        try require(enabledCaps["fullGraphStems"] as? Bool == true,"Applied routing did not advertise its available full-graph captures.")
        try require((applied["project"] as? [String:Any])?["name"] as? String == project["name"] as? String,"Routing apply did not return the authoritative complete project.")
        let originalSnapshot = try await snapshot()
        var invalid = project, cycle = project["routing"] as! [String:Any]
        cycle["outputs"] = ["track:tone":"bus:group","bus:group":"bus:return","bus:return":"bus:group"]
        invalid["routing"] = cycle
        var rejected = false
        do { _ = try await host.handle("routing.apply",payload:["project":invalid]) } catch { rejected = true }
        try require(rejected,"Cyclic routing was accepted.")
        let afterRejected = try await snapshot()
        try require(NSDictionary(dictionary:originalSnapshot).isEqual(to:afterRejected),"A rejected route changed the saved project.")
        passed.append("Routing capability negotiation and full-project apply reject cycles without altering the authoritative project")

        let selected = ["track:tone","track:key","bus:group","bus:return","master"]
        let (_,linear,result) = try await render("linear-stems",owners:selected,mode:"stems")
        try require(result["renderPasses"] as? Int == 1 && linear.count == selected.count,"Stem export did not capture every owner in one pass.")
        try require(linear.values.allSatisfy {$0.left.count == 24_000 && $0.right.count == 24_000},"Stem files are not sample-aligned to the half-second arrangement.")
        let group = linear["bus:group"]!,wet = linear["bus:return"]!,master = linear["master"]!,track = linear["track:tone"]!
        var linearError = 0.0,groupError = 0.0
        for frame in master.left.indices {
            linearError = max(linearError,abs(Double(master.left[frame])-0.7*Double(group.left[frame]+wet.left[frame])))
            groupError = max(groupError,abs(Double(group.left[frame])-0.6*Double(track.left[frame])))
        }
        measurements["linearSumError"] = linearError; measurements["groupGainError"] = groupError
        try require(linearError < 0.000003 && groupError < 0.000003,"Group/return/master faders are not applied once at their defined stages.")
        try require(energy(linear["track:key"]!) > 0.01,"A disconnected track could not be captured without adding it to the audible mix.")
        let (linearMaster,_,_) = try await render("linear-master")
        try require(difference(master,linearMaster!) < 0.000003,"Adding selected captures altered the complete master render.")
        passed.append("One graph pass captures tracks, disconnected tracks, groups, returns and master into aligned WAV files with correct gain stages")

        // Reuse a dense destination ID for a different audible owner while the
        // transport is paused between two manual quanta, without seeking/restarting.
        // A separate static graph provides the exact next-quantum reference PCM.
        var automated = project, automatedPatterns = project["patterns"] as! [[String:Any]]
        automatedPatterns[0]["automation"] = [["owner":"bus:group","target":"volume","interpolation":"step",
            "points":[["row":0,"value":0.2]]]]
        automated["patterns"] = automatedPatterns; _ = try await apply(automated)
        _ = try await host.handle("transport.play",payload:["mode":"song","fromStart":true])
        let liveBuffer = AVAudioPCMBuffer(pcmFormat:format,frameCapacity:512)!
        try require(try graph.renderOffline(512,to:liveBuffer) == .success,"Could not warm up the routing automation fixture.")
        let beforeMapping = host.status()
        var reassigned = project,reassignedRouting = project["routing"] as! [String:Any]
        var reassignedBuses = reassignedRouting["buses"] as! [[String:Any]]
        reassignedBuses[0]["id"] = "new-group"; reassignedBuses[0]["name"] = "Replacement group"
        reassignedRouting["buses"] = reassignedBuses
        reassignedRouting["outputs"] = ["track:tone":"bus:new-group","track:key":NSNull(),"bus:new-group":"master","bus:return":"master"]
        reassigned["routing"] = reassignedRouting
        automatedPatterns[0]["automation"] = [["owner":"bus:return","target":"volume","interpolation":"step",
            "points":[["row":0,"value":0.9]]]]
        reassigned["patterns"] = automatedPatterns
        _ = try await apply(reassigned)
        let afterMapping = host.status()
        try require(beforeMapping["position"] as? Double == afterMapping["position"] as? Double &&
                    beforeMapping["seconds"] as? Double == afterMapping["seconds"] as? Double,
                    "Replacing automation destinations advanced the transport.")
        try require(try graph.renderOffline(512,to:liveBuffer) == .success,"Could not render the first reassigned automation quantum.")
        let actualNext = Array(UnsafeBufferPointer(start:liveBuffer.floatChannelData![0],count:512))
        let referenceGraph = AVAudioEngine()
        try referenceGraph.enableManualRenderingMode(.offline,format:format,maximumFrameCount:512)
        let referenceHost = try AudioHost(audioGraph:referenceGraph,midiInputIDs:[])
        do {
            defer { referenceHost.shutdown() }
            var fixed = reassigned,fixedPatterns = automatedPatterns
            fixedPatterns[0]["automation"] = []; fixed["patterns"] = fixedPatterns
            reassignedBuses[1]["volume"] = 0.9; reassignedRouting["buses"] = reassignedBuses; fixed["routing"] = reassignedRouting
            _ = try await referenceHost.handle("engine.project",payload:["project":fixed])
            _ = try await referenceHost.handle("transport.play",payload:["mode":"song","fromStart":true])
            let referenceBuffer = AVAudioPCMBuffer(pcmFormat:format,frameCapacity:512)!
            try require(try referenceGraph.renderOffline(512,to:referenceBuffer) == .success,"Could not warm up the fixed automation reference.")
            try require(try referenceGraph.renderOffline(512,to:referenceBuffer) == .success,"Could not render the fixed automation reference.")
            let expectedNext = Array(UnsafeBufferPointer(start:referenceBuffer.floatChannelData![0],count:512))
            let remapError = zip(actualNext,expectedNext).reduce(0.0) { max($0,abs(Double($1.0-$1.1))) }
            measurements["reassignedAutomationError"] = remapError
            try require(remapError < 0.000003,"A removed owner's old automation reached a replacement owner, or the new owner lane did not take effect.")
        }
        _ = try await host.handle("transport.stop",payload:[:]); _ = try await host.handle("engine.panic",payload:[:])
        try require(try graph.renderOffline(128,to:liveBuffer) == .success,"Could not consume the automation fixture's stop command.")
        graph.stop(); _ = try await apply(project)
        passed.append("Removing and reassigning owner lanes across a topology change preserves the cursor and cannot send stale dense-ID automation to a new owner")

        let detectorDescriptor = try await host.handle("plugin.load",payload:["owner":"bus:group","kind":"effect","slot":0,"component":identifier(detector)]) as? [String:Any] ?? [:]
        _ = try await host.handle("plugin.load",payload:["owner":"master","kind":"effect","slot":0,"component":identifier(saturator)])
        _ = try await host.handle("plugin.load",payload:["owner":"track:tone","kind":"effect","slot":0,"component":identifier(delay)])
        project = try await snapshot()
        var routed = project["routing"] as! [String:Any]
        let instance = detectorDescriptor["instanceID"] as? String ?? ""
        try require(!instance.isEmpty,"Group insert did not expose its stable instance ID.")
        routed["sidechains"] = [["id":"duck","source":"track:key","tap":"postFader","gain":1,"enabled":true,
            "destination":["instanceID":instance,"inputBus":1,"layoutSignature":["name":"Detector","channels":2]]]]
        project["routing"] = routed; _ = try await apply(project)
        let (_,keyed,keyedResult) = try await render("keyed-stems",owners:selected,mode:"stems")
        let (keyedMaster,_,_) = try await render("keyed-master")
        measurements["keyedMasterDifference"] = difference(keyed["master"]!,keyedMaster!)
        measurements["delayedTrackDifference"] = difference(keyed["track:tone"]!,linear["track:tone"]!)
        measurements["keyedOnsets"] = keyed.mapValues(onset)
        measurements["linearOnsets"] = linear.mapValues(onset)
        measurements["keyedMasterOnset"] = onset(keyedMaster!)
        measurements["keyedCaptureDiagnostics"] = keyedResult["captureDiagnostics"]
        measurements["keyedCaptureArrivals"] = (keyedResult["stems"] as? [[String:Any]] ?? []).map {
            ["owner":$0["owner"] ?? "", "arrivalFrames":$0["arrivalFrames"] ?? -1]
        }
        try require(difference(keyed["master"]!,keyedMaster!) < 0.000003,"Full-graph stems changed nonlinear master processing or detector input.")
        try require(keyed.values.allSatisfy {$0.left.count == 24_000},"Latency compensation changed selected stem length.")
        try require(difference(keyed["track:tone"]!,linear["track:tone"]!) < 0.000003,"A delayed insert's stem did not trim its own arrival latency.")
        let latency = keyedResult["latencyCompensation"] as? [String:Any] ?? [:]
        try require((latency["frames"] as? Int ?? 0) >= 64,"The routed graph did not report the original fixture's physical insert delay.")
        let keyedEnergy = energy(keyed["bus:group"]!)
        routed["sidechains"] = []; project["routing"] = routed; _ = try await apply(project)
        let (_,unkeyed,_) = try await render("unkeyed-stems",owners:["bus:group"],mode:"stems")
        measurements["keyedGroupEnergy"] = keyedEnergy; measurements["unkeyedGroupEnergy"] = energy(unkeyed["bus:group"]!)
        try require(keyedEnergy < energy(unkeyed["bus:group"]!)*0.75,"A disconnected detector source did not drive the group's auxiliary input.")
        passed.append("Group and master inserts, disconnected sidechain keys and insert latency survive simultaneous stems without changing nonlinear master audio")

        routed["sidechains"] = [["id":"duck","source":"track:key","tap":"postFader","gain":1,"enabled":true,
            "destination":["instanceID":instance,"inputBus":1,"layoutSignature":["name":"Detector","channels":2]]]]
        project["routing"] = routed; _ = try await apply(project)
        _ = try await host.handle("plugin.remove",payload:["owner":"bus:group","kind":"effect","slot":0])
        let removed = try await snapshot(),removedRouting = removed["routing"] as? [String:Any] ?? [:]
        try require((removedRouting["sidechains"] as? [[String:Any]])?.isEmpty == true,"Removing a group insert left sidechains targeting its deleted instance.")
        let (_,defaults,_) = try await render("default-stems",mode:"stems")
        try require(Set(defaults.keys) == ["track:tone","track:key"],"Default routed stems did not select all and only track owners.")
        let unchanged = try await snapshot()
        try snapshotEvidence("export-cleanup",expected:removed,actual:unchanged)
        try require(NSDictionary(dictionary:semanticSnapshot(removed)).isEqual(to:semanticSnapshot(unchanged)),
                    "Export leaked its temporary project, capture selection or semantic plug-in state into the saved session.")
        try require(graph.isInManualRenderingMode && !graph.isRunning,"Export failed to restore the original idle offline graph.")
        passed.append("Removing an insert removes only its sidechain targets; default stems and cleanup preserve the complete project and idle transport")

        // Saved unsupported intent is retained only for explicit session replace.
        // Ordinary routing.apply still rejects without replacing this snapshot.
        var unsupported = removed,unsupportedRouting = removedRouting
        unsupportedRouting["outputs"] = ["track:tone":"bus:group","bus:group":"bus:return","bus:return":"bus:group"]
        unsupported["routing"] = unsupportedRouting; unsupported["name"] = "Unavailable saved routing"
        var opaque = unsupported["audioPlugins"] as? [[String:Any]] ?? []
        opaque.append(["owner":"bus:group","kind":"effect","slot":7,"component":"missing.saved.component",
            "instanceID":"preserve-unavailable-instance","state":"opaque-state-that-must-not-be-replaced"])
        unsupported["audioPlugins"] = opaque
        try publishMIDI([0xB0,1,64])
        for _ in 0..<100 where incomingMIDI == 0 { try await Task.sleep(nanoseconds:10_000_000) }
        try require(incomingMIDI > 0,"The valid routing project did not receive the test's virtual MIDI control message.")
        _ = try await host.handle("engine.project",payload:["project":unsupported,"replace":true])
        let unavailable = try await snapshot()
        try require(NSDictionary(dictionary:unsupported).isEqual(to:unavailable),"Opening invalid saved routing replaced its desired project or opaque plug-in state.")
        try require(!graph.isRunning && host.status()["playing"] as? Bool == false && host.status()["routingFailure"] is String,
                    "Invalid saved routing did not stop previous audio and expose its failure.")
        let unavailableCaps = try await host.handle("routing.capabilities",payload:[:]) as? [String:Any] ?? [:]
        let unavailableStatus = try await host.handle("routing.status",payload:[:]) as? [String:Any] ?? [:]
        try require(unavailableCaps["fullGraphStems"] as? Bool == false &&
                    (unavailableCaps["exportTargets"] as? [[String:Any]] ?? []).allSatisfy {$0["available"] as? Bool == false} &&
                    unavailableStatus["projectUnavailable"] as? Bool == true && !(unavailableStatus["unavailable"] is Bool),
                    "Unavailable routing advertised the previous graph's export targets or broke sidechain status metadata.")
        let persisted = folder.appendingPathComponent("unavailable-roundtrip.supertracker")
        try JSONSerialization.data(withJSONObject:unavailable,options:[.sortedKeys]).write(to:persisted,options:.atomic)
        let reread = try JSONSerialization.jsonObject(with:Data(contentsOf:persisted)) as! [String:Any]
        try require(NSDictionary(dictionary:unsupported).isEqual(to:reread),"Saving the unavailable project lost its routing or plug-in state.")
        var playbackRejected = false
        do { _ = try await host.handle("transport.play",payload:[:]) } catch { playbackRejected = true }
        try require(playbackRejected && !graph.isRunning,"Unavailable routing played the previous project.")
        incomingMIDI = 0
        try publishMIDI([0x90,60,100,0xB0,1,127,0xC0,9,0xD0,64,0xE0,0,100,0xFA])
        try await Task.sleep(nanoseconds:150_000_000)
        try require(incomingMIDI == 0 && !graph.isRunning && host.status()["playing"] as? Bool == false,
                    "Incoming MIDI notes, controllers or transport reached the previous project while desired routing was unavailable.")
        do { _ = try await host.handle("routing.apply",payload:["project":unsupported]); throw AudioSampleError.invalid("Invalid repair was accepted.") }
        catch let error as AudioRouting.RoutingError { _ = error }
        try require(NSDictionary(dictionary:unsupported).isEqual(to:try await snapshot()),"A rejected repair changed the retained unavailable snapshot.")
        _ = try await apply(removed)
        let repaired = try await snapshot()
        measurements["repairStatus"] = host.status()
        try snapshotEvidence("repair",expected:removed,actual:repaired)
        try require(host.status()["routingFailure"] is NSNull && NSDictionary(dictionary:semanticSnapshot(removed)).isEqual(to:semanticSnapshot(repaired)),
                    "A valid routing repair did not restore the desired session and clear its failure.")
        let (repairedAudio,_,_) = try await render("repaired-master")
        try require(energy(repairedAudio!) > 0.001,"The repaired routing project could not render audio.")
        passed.append("Unsupported saved routing remains exactly saveable with opaque plug-in state, blocks stale audio and virtual MIDI input, rejects invalid repairs and recovers through a valid routing apply")

        // Hold an original test AU before instantiation so cancellation stays
        // suspended while public snapshot, MIDI and transport commands run.
        let restoreGate = RoutingInstantiationGate()
        let gatedGraph = AVAudioEngine()
        try gatedGraph.enableManualRenderingMode(.offline,format:format,maximumFrameCount:4096)
        let gatedHost = try AudioHost(beforePluginInstantiation:{ await restoreGate.pause() },audioGraph:gatedGraph,
            midiInputIDs:[],midiInputSourceProvider:{midiSources.snapshot()})
        defer { restoreGate.release(); gatedHost.shutdown() }
        _ = try await gatedHost.handle("midi.configure",payload:["inputIDs":[midiSourceID]])
        gatedGraph.stop()
        var gatedInput = 0
        gatedHost.onEvent = { name,_ in if name == "midi.note" || name == "midi.message" { gatedInput += 1 } }
        var pendingProject = removed
        pendingProject["audioPlugins"] = [["owner":"track:tone","track":0,"kind":"effect","slot":0,"component":identifier(mute)]]
        _ = try await gatedHost.handle("engine.project",payload:["project":pendingProject,"replace":true])
        for _ in 0..<1_000 where !restoreGate.waiting { await Task.yield() }
        try require(restoreGate.waiting,"The pending-restore fixture did not reach its instantiation gate.")
        var pendingExportEntered = false
        let pendingExport = Task { @MainActor in
            pendingExportEntered = true
            do {
                _ = try await gatedHost.handle("audio.export",payload:["path":folder.appendingPathComponent("rejected-pending-export.wav").path,"tail":0])
                return false
            } catch { return true }
        }
        for _ in 0..<1_000 where !pendingExportEntered { await Task.yield() }
        try require(pendingExportEntered,"The pending export fixture did not start before the invalid replacement.")
        var pendingInvalid = pendingProject
        pendingInvalid["routing"] = unsupportedRouting; pendingInvalid["name"] = "Unavailable during pending restore"
        var invalidOpenCompleted = false
        let invalidOpen = Task { @MainActor in
            _ = try await gatedHost.handle("engine.project",payload:["project":pendingInvalid,"replace":true])
            invalidOpenCompleted = true
        }
        for _ in 0..<1_000 where !(gatedHost.status()["routingFailure"] is String) { await Task.yield() }
        let suspendedSnapshot = try await gatedHost.handle("engine.snapshot",payload:[:]) as? [String:Any] ?? [:]
        try require(!invalidOpenCompleted && NSDictionary(dictionary:pendingInvalid).isEqual(to:suspendedSnapshot),
                    "Invalid project intent was not published before awaiting a cancelled plug-in restoration.")
        var suspendedPlayRejected = false
        do { _ = try await gatedHost.handle("transport.play",payload:[:]) } catch { suspendedPlayRejected = true }
        try publishMIDI([0xB0,1,100,0x90,60,100,0xFA])
        try await Task.sleep(nanoseconds:150_000_000)
        try require(suspendedPlayRejected && gatedInput == 0 && !gatedGraph.isRunning,
                    "MIDI or playback reached the previous session during pending restore cancellation.")
        restoreGate.release()
        try await invalidOpen.value
        try require(await pendingExport.value && !gatedGraph.isRunning,
                    "An export begun before invalid replacement rendered the previous project after its asset wait.")
        _ = try await gatedHost.handle("routing.apply",payload:["project":pendingProject])
        await gatedHost.pluginRestorationBarrier()?.value
        let repairedPlugin = try await gatedHost.handle("plugin.state",payload:["owner":"track:tone","kind":"effect","slot":0]) as? [String:Any] ?? [:]
        try require(restoreGate.entries == 2 && repairedPlugin["component"] as? String == identifier(mute) && repairedPlugin["stateError"] == nil && repairedPlugin["instanceID"] is String && gatedHost.status()["routingFailure"] is NSNull,
                    "Repairing identical plug-in descriptors skipped the previously cancelled restoration.")
        passed.append("Unavailable project intent blocks snapshot/MIDI/play/export races while AU cancellation is suspended; identical-descriptor repair retries instantiation")

        var mixRows:[Any] = [["note":60,"instrument":0,"velocity":127,"gate":8],
            ["fx1":["type":"volume","value":0.2],"fx2":["type":"pan","value":-1]],NSNull(),
            ["fx1":["type":"volume","value":0.5],"fx2":["type":"pan","value":0]]]
        func mixLane(_ target:String,_ values:[Double],owner:Bool)->[String:Any] {
            var lane:[String:Any] = ["target":target,"interpolation":"step",
                "points":values.enumerated().map {["row":$0.offset,"value":$0.element] as [String:Any]}]
            if owner { lane["owner"] = "track:tone" } else { lane["track"] = 0 }
            return lane
        }
        var mixProject:[String:Any] = ["version":1,"name":"Routing mix-rowlock fixture","bpm":120,"linesPerBeat":4,"masterVolume":1,
            "activePattern":0,"loopSong":false,"tracks":[["routingID":"tone","name":"Tone","instrument":0,"volume":1,"pan":0]],
            "instruments":[["type":"sample","samplePath":toneURL.path,"rootNote":60,"volume":1,"mode":"oneshot",
                "attack":0,"decay":0,"sustain":1,"release":0.001,"cutoff":20_000,"resonance":0,"drive":0]],
            "patterns":[["id":0,"length":4,"steps":[mixRows],"automation":[
                mixLane("volume",[0.7,0.9,0.6,0.8],owner:true),mixLane("pan",[0,1,0.8,-1],owner:true)]]],
            "sequence":[0],"audioPlugins":[["owner":"track:tone","track":0,"kind":"effect","slot":0,"component":identifier(delay)]],
            "delay":["mix":0],"reverb":["mix":0],"routing":["version":1,"mixModel":"standard","buses":[],"sends":[],"sidechains":[]]]
        _ = try await apply(mixProject)
        let (_,ownerMix,_) = try await render("owner-mix-rowlocks",owners:["track:tone"],mode:"stems")
        mixProject["patterns"] = [["id":0,"length":4,"steps":[mixRows],"automation":[
            mixLane("volume",[0.7,0.9,0.6,0.8],owner:false),mixLane("pan",[0,1,0.8,-1],owner:false)]]]
        _ = try await apply(mixProject)
        let (_,nativeMix,_) = try await render("indexed-mix-rowlocks",owners:["track:tone"],mode:"stems")
        mixRows[1] = NSNull(); mixRows[3] = NSNull()
        mixProject["patterns"] = [["id":0,"length":4,"steps":[mixRows],"automation":[
            mixLane("volume",[0.7,0.2,0.6,0.5],owner:false),mixLane("pan",[0,-1,0.8,0],owner:false)]]]
        _ = try await apply(mixProject)
        let (_,explicitMix,_) = try await render("explicit-locked-mix",owners:["track:tone"],mode:"stems")
        func stereoDifference(_ a:DecodedSample,_ b:DecodedSample)->Double {
            guard a.left.count == b.left.count && a.right.count == b.right.count else { return .infinity }
            return max(zip(a.left,b.left).reduce(0) { max($0,abs(Double($1.0-$1.1))) },
                       zip(a.right,b.right).reduce(0) { max($0,abs(Double($1.0-$1.1))) })
        }
        let ownerDifference = stereoDifference(ownerMix["track:tone"]!,nativeMix["track:tone"]!)
        let rowlockDifference = stereoDifference(ownerMix["track:tone"]!,explicitMix["track:tone"]!)
        measurements["ownerTrackLaneDifference"] = ownerDifference; measurements["ownerRowLockDifference"] = rowlockDifference
        try require(ownerDifference < 0.000003 && rowlockDifference < 0.000003,
                    "Owner volume/pan lanes bypassed native row locks or their 64-frame insert compensation.")
        passed.append("Standard owner track volume/pan lanes match native indexed lanes and explicit step-lock values through a 64-frame insert")

        // Disconnect dry output so every audible sample must have passed through
        // a fixed send. A mute insert proves these sends take their input after FX.
        let dlsID = "\(kAudioUnitType_MusicDevice):\(kAudioUnitSubType_DLSSynth):\(kAudioUnitManufacturer_Apple)"
        var aliases = mixProject
        aliases["routing"] = ["version":1,"mixModel":"standard","buses":[],"outputs":["track:tone":NSNull()],"sends":[],"sidechains":[]]
        var aliasMetrics:[[String:Any]] = []
        for sourceKind in ["native","apple-au"] {
            if sourceKind == "apple-au" { aliases["instruments"] = [["type":"au","volume":1,"pan":0]] }
            let instrumentPlugins:[[String:Any]] = sourceKind == "apple-au" ? [["owner":"track:tone","track":0,"kind":"instrument","slot":0,"component":dlsID]] : []
            for effect in ["delay","reverb"] {
                func aliasProject(mix:Double,suppressed:Bool = false,mutedInsert:Bool = false)->[String:Any] {
                    var value = aliases
                    value["tracks"] = [["routingID":"tone","name":"Wet-only source","instrument":0,"volume":1,"pan":0,
                        "delaySend":effect == "delay" ? 1 : 0,"reverbSend":effect == "reverb" ? 1 : 0]]
                    value["audioPlugins"] = instrumentPlugins+(mutedInsert ? [["owner":"track:tone","track":0,"kind":"effect","slot":0,"component":identifier(mute)]] : [])
                    value["delay"] = ["time":0.2,"feedback":0.6,"mix":effect == "delay" ? mix : 0]
                    value["reverb"] = ["size":0.8,"damping":0.25,"mix":effect == "reverb" ? mix : 0]
                    var rows:[Any] = [["note":60,"instrument":0,"velocity":100,"gate":0.5],NSNull(),NSNull(),NSNull()]
                    if suppressed {
                        for index in rows.indices {
                            var row = rows[index] as? [String:Any] ?? [:]
                            row["fx1"] = ["type":effect,"value":0]; rows[index] = row
                        }
                    }
                    value["patterns"] = [["id":0,"length":4,"steps":[rows],"automation":[]]]
                    return value
                }
                _ = try await apply(aliasProject(mix:0))
                let (zeroAlias,_,_) = try await render("\(sourceKind)-\(effect)-alias-zero",tail:0.25)
                _ = try await apply(aliasProject(mix:1))
                let (wetAlias,_,_) = try await render("\(sourceKind)-\(effect)-alias-wet",tail:0.25)
                _ = try await apply(aliasProject(mix:1,suppressed:true))
                let (suppressedAlias,_,_) = try await render("\(sourceKind)-\(effect)-alias-step-zero",tail:0.25)
                _ = try await apply(aliasProject(mix:1,mutedInsert:true))
                let (mutedAlias,_,_) = try await render("\(sourceKind)-\(effect)-alias-insert-zero",tail:0.25)
                let wetEnergy = energy(wetAlias!),zeroEnergy = energy(zeroAlias!),suppressedEnergy = energy(suppressedAlias!),mutedEnergy = energy(mutedAlias!)
                let tailValues = wetAlias!.left.dropFirst(24_000)
                let tailEnergy = sqrt(tailValues.reduce(0) { $0+Double($1)*Double($1) }/Double(max(1,tailValues.count)))
                aliasMetrics.append(["source":sourceKind,"effect":effect,"wetRMS":wetEnergy,"tailRMS":tailEnergy,
                    "zeroAliasRMS":zeroEnergy,"stepSuppressedRMS":suppressedEnergy,"mutedInsertRMS":mutedEnergy])
                measurements["fixedAliases"] = aliasMetrics
                try require(wetEnergy > 0.00001 && tailEnergy > 0.000001 && zeroEnergy < 0.000001 && suppressedEnergy < 0.000001 && mutedEnergy < 0.000001,
                            "Standard \(sourceKind) \(effect) alias did not receive post-insert audio, honor its return mix and step FX, or produce a wet tail: \(aliasMetrics.last!).")
            }
        }
        passed.append("Fixed Delay/Reverb aliases receive post-insert native and Apple DLS audio, honor return gain and D/R step suppression, and retain audible tails")
        // Finish all host callbacks before removing this fixture's source. The
        // next native suite may create a MIDI client immediately after return.
        try publishMIDI([0x80,60,0,0xB0,64,0,0xB0,123,0,0xFC])
        _ = try await gatedHost.handle("engine.panic",payload:[:])
        _ = try await host.handle("engine.panic",payload:[:])
        await gatedHost.shutdownAndWait()
        await host.shutdownAndWait()
        midiSources.set([])
        return passed
    }
}

private final class RoutingMIDISourceList:@unchecked Sendable {
    private let lock = NSLock()
    private var sources:[MIDIEndpointRef] = []
    func set(_ value:[MIDIEndpointRef]) { lock.lock(); sources = value; lock.unlock() }
    func snapshot()->[MIDIEndpointRef] { lock.lock(); defer {lock.unlock()}; return sources }
}

@MainActor private final class RoutingInstantiationGate {
    private var continuation:CheckedContinuation<Void,Never>?
    private var enabled = true
    private(set) var entries = 0
    var waiting:Bool { continuation != nil }
    func pause() async {
        entries += 1
        guard enabled else { return }
        await withCheckedContinuation { continuation = $0 }
    }
    func release() {
        enabled = false
        let pending = continuation; continuation = nil; pending?.resume()
    }
}

private final class NativeRoutingHostUnit:AUAudioUnit {
    private var ins:AUAudioUnitBusArray!,outs:AUAudioUnitBusArray!
    private var mainScratch:AVAudioPCMBuffer!,keyScratch:AVAudioPCMBuffer!
    private let subtype:OSType
    private let delayed:Bool
    private var left = [Float](repeating:0,count:64),right = [Float](repeating:0,count:64)
    private var cursor = 0
    override init(componentDescription:AudioComponentDescription,options:AudioComponentInstantiationOptions = []) throws {
        subtype = componentDescription.componentSubType; delayed = subtype == 0x72683634
        try super.init(componentDescription:componentDescription,options:options)
        let format = AVAudioFormat(standardFormatWithSampleRate:48_000,channels:2)!
        let main = try AUAudioUnitBus(format:format); main.name = "Main"
        var buses = [main]
        if subtype == 0x72686B79 { let key = try AUAudioUnitBus(format:format); key.name = "Detector"; buses.append(key) }
        ins = AUAudioUnitBusArray(audioUnit:self,busType:.input,busses:buses)
        outs = AUAudioUnitBusArray(audioUnit:self,busType:.output,busses:[try AUAudioUnitBus(format:format)])
        maximumFramesToRender = 65_536
    }
    override var inputBusses:AUAudioUnitBusArray { ins }
    override var outputBusses:AUAudioUnitBusArray { outs }
    override var channelCapabilities:[NSNumber]? { [2,2] }
    override var latency:TimeInterval { delayed ? 64/48_000 : 0 }
    override var canProcessInPlace:Bool { true }
    override func allocateRenderResources() throws {
        try super.allocateRenderResources()
        mainScratch = AVAudioPCMBuffer(pcmFormat:outs[0].format,frameCapacity:maximumFramesToRender)!
        keyScratch = AVAudioPCMBuffer(pcmFormat:outs[0].format,frameCapacity:maximumFramesToRender)!
        reset()
    }
    override func reset() { super.reset(); for index in 0..<64 { left[index] = 0; right[index] = 0 }; cursor = 0 }
    override var internalRenderBlock:AUInternalRenderBlock {
        { [unowned self] flags,time,frames,bus,output,_,pull in
            guard bus == 0,let pull,frames <= self.maximumFramesToRender else { return kAudioUnitErr_TooManyFramesToProcess }
            let buffers = UnsafeMutableAudioBufferListPointer(output)
            guard buffers.count == 2 else { return kAudioUnitErr_FormatNotSupported }
            for channel in 0..<2 where buffers[channel].mData == nil {
                buffers[channel].mData = UnsafeMutableRawPointer(self.mainScratch.floatChannelData![channel])
                buffers[channel].mDataByteSize = frames*4
            }
            let status = pull(flags,time,frames,0,output)
            guard status == noErr else { return status }
            let a = buffers[0].mData!.assumingMemoryBound(to:Float.self),b = buffers[1].mData!.assumingMemoryBound(to:Float.self)
            if self.subtype == 0x72686B79,self.ins[1].isEnabled {
                self.keyScratch.frameLength = frames
                var keyFlags = AudioUnitRenderActionFlags()
                let keyStatus = pull(&keyFlags,time,frames,1,self.keyScratch.mutableAudioBufferList)
                if keyStatus == noErr && !keyFlags.contains(.unitRenderAction_OutputIsSilence) {
                    let key = self.keyScratch.floatChannelData!
                    for frame in 0..<Int(frames) { let gain:Float = 1/(1+20*max(abs(key[0][frame]),abs(key[1][frame]))); a[frame] *= gain; b[frame] *= gain }
                } else if keyStatus != noErr && keyStatus != kAudioUnitErr_NoConnection { return keyStatus }
            } else if self.subtype == 0x72686D74 {
                for frame in 0..<Int(frames) { a[frame] = 0; b[frame] = 0 }
                flags.pointee.insert(.unitRenderAction_OutputIsSilence)
            } else if self.subtype == 0x72687374 {
                for frame in 0..<Int(frames) { a[frame] = tanhf(a[frame]*3); b[frame] = tanhf(b[frame]*3) }
            } else if self.delayed {
                for frame in 0..<Int(frames) {
                    let index = self.cursor,oldA = self.left[index],oldB = self.right[index]
                    self.left[index] = a[frame]; self.right[index] = b[frame]; a[frame] = oldA; b[frame] = oldB
                    self.cursor = (index+1)%64
                }
                flags.pointee.remove(.unitRenderAction_OutputIsSilence)
            }
            return noErr
        }
    }
}

#endif

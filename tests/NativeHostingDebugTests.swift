import Foundation
import AVFoundation
import VST2Host

/// Private original fixtures only. The runner supplies a pinned generated bundle.
enum NativeHostingDebugTests {
    @MainActor static func run(output:URL,bundle:URL) async throws -> [String:Any] {
        try FileManager.default.createDirectory(at:output,withIntermediateDirectories:true)
        var checks:[[String:Any]]=[]
        func stage(_ name:String,_ body:() async throws -> Void) async {
            do { try await body();checks.append(["name":name,"passed":true]) }
            catch { checks.append(["name":name,"passed":false,"error":error.localizedDescription]) }
            if let data=try? JSONSerialization.data(withJSONObject:checks,options:[.prettyPrinted,.sortedKeys]) {
                try? data.write(to:output.appendingPathComponent("progress.json"),options:.atomic)
            }
        }
        func require(_ condition:Bool,_ message:String) throws {
            if !condition { throw AudioSampleError.invalid(message) }
        }
        let catalogue=try privateCatalogue(bundle:bundle,folder:output)
        var vst3Configuration=VST3Catalogue.Configuration()
        vst3Configuration.roots=[];vst3Configuration.cacheURL=output.appendingPathComponent("empty-vst3-cache.json")
        let vst3Catalogue=VST3Catalogue(configuration:vst3Configuration)
        func newHost(gate:(() async throws -> Void)? = nil) throws -> AudioHost {
            let graph=AVAudioEngine()
            try graph.enableManualRenderingMode(.offline,format:AVAudioFormat(standardFormatWithSampleRate:48000,channels:2)!,maximumFrameCount:4096)
            return try AudioHost(beforePluginInstantiation:gate,audioGraph:graph,vst3Catalogue:vst3Catalogue,midiInputIDs:[],midiInputSourceProvider:{[]},vst2Catalogue:catalogue)
        }
        let target:[String:Any]=["track":0,"kind":"instrument","slot":0]
        let load:[String:Any]=["track":0,"kind":"instrument","slot":0,"component":"vst2:44624731"]
        await stage("Rejected VST2 saved state preserves the previously loaded instance") {
            let host=try newHost();defer { host.shutdown() }
            let original=try await host.handle("plugin.load",payload:load) as! [String:Any]
            let saved=original["state"] as! String
            var envelope=try PropertyListSerialization.propertyList(from:Data(base64Encoded:saved)!,format:nil) as! [String:Any]
            envelope["data"]=Data([0,1,2,3])
            var request=load;request["state"]=try PropertyListSerialization.data(fromPropertyList:envelope,format:.binary,options:0).base64EncodedString()
            var rejected=false
            do { _ = try await host.handle("plugin.load",payload:request) } catch { rejected=true }
            let after=try await host.handle("plugin.state",payload:target) as! [String:Any]
            try require(rejected,"Invalid VST2 saved data was acknowledged as restored.")
            try require(after["instanceID"] as? String == original["instanceID"] as? String,"Invalid candidate replaced the valid live instance.")
            await host.shutdownAndWait()
        }
        await stage("Unavailable required VST2 chunk rejects capture rather than replacing it with parameters") {
            let host=try newHost();defer { host.shutdown() }
            let original = try await host.handle("plugin.load",payload:load) as! [String:Any]
            let originalState = original["state"] as! String
            var edit=target;edit["address"]="1";edit["value"]=1.0
            let parameter = try await host.handle("plugin.parameter",payload:edit) as! [String:Any]
            let failed = try await host.handle("plugin.state",payload:target) as! [String:Any]
            let snapshot=try await host.handle("engine.snapshot",payload:[:]) as! [String:Any]
            let item=(snapshot["audioPlugins"] as? [[String:Any]])?.first
            var saveRejected = false
            do { _ = try PluginSnapshotCapture.states(from:snapshot) } catch { saveRejected = true }
            try JSONSerialization.data(withJSONObject:["parameter":parameter,"targetStateError":failed["stateError"] ?? NSNull(),
                "targetRetainedState":failed["state"] as? String == originalState,"snapshotStateError":item?["stateError"] ?? NSNull(),
                "snapshotRetainedState":item?["state"] as? String == originalState,"saveRejected":saveRejected],
                options:[.prettyPrinted,.sortedKeys]).write(to:output.appendingPathComponent("required-chunk-capture.json"))
            try require((parameter["value"] as? NSNumber)?.doubleValue == 1,"The required-chunk failure control was not applied.")
            // Public capture intentionally preserves the last good descriptor with
            // explicit error metadata. Persistence must reject that descriptor.
            try require((failed["stateError"] as? String)?.contains("required chunk") == true,"Required chunk failure returned successful targeted state.")
            try require(failed["state"] as? String == originalState,"Targeted failure replaced the last good opaque state.")
            try require(item?["stateError"] is String && saveRejected,"Snapshot did not preserve capture failure for Save to reject.")
            try require(item?["state"] as? String == originalState,"Snapshot failure replaced the last good opaque state with a fallback envelope.")
            edit["value"]=0.0;_ = try await host.handle("plugin.parameter",payload:edit)
            let recovered=try await host.handle("plugin.state",payload:target) as! [String:Any]
            try require(recovered["state"] is String && recovered["stateError"] == nil,"Fresh capture did not recover after the fixture supplied its chunk again.")
            await host.shutdownAndWait()
        }
        await stage("VST2 parameter changes respect their exact sample offset") {
            var error=[CChar](repeating:0,count:2048)
            guard let handle=st_vst2_create(bundle.path,0,48000,64,&error,error.count) else { throw AudioSampleError.invalid(String(cString:error)) }
            defer { st_vst2_destroy(handle) }
            try require(st_vst2_prepare(handle,48000,64,&error,error.count) != 0,String(cString:error))
            var left=[Float](repeating:0,count:64),right=left
            var change=STVST2ParameterChange(sample_offset:32,index:0,value:0.75)
            let result=left.withUnsafeMutableBufferPointer { l in right.withUnsafeMutableBufferPointer { r in
                st_vst2_process(handle,nil,nil,l.baseAddress,r.baseAddress,64,nil,0,&change,1,nil)
            } }
            let expected=(0..<64).map { $0 < 32 ? Float(0.25):Float(0.75) }
            try JSONSerialization.data(withJSONObject:["left":left,"expected":expected],options:[.prettyPrinted,.sortedKeys]).write(to:output.appendingPathComponent("parameter-offset.json"))
            try require(result != 0 && left == expected && right == expected,"VST2 rendered a frame32 change before its deadline.")
        }
        await stage("VST2 failed input pull returns the failure without advancing its child") {
            let effect=bundle.deletingLastPathComponent().appendingPathComponent("DebugVST2Effect.vst")
            try require(Bundle(url:effect)?.bundleIdentifier == "studio.supertracker.tests.debug-vst2-effect","Only the original effect fixture is allowed.")
            let descriptor=VST2PluginDescriptor(uniqueID:0x44624732,shellID:0,bundlePath:effect.path,name:"Original Debug VST2",manufacturer:"SuperTracker Tests",version:"1",isInstrument:false,hasEditor:false,inputs:2,outputs:2)
            let node=try VST2AudioUnit.makeNode(descriptor:descriptor,sampleRate:48000)
            let unit=node.auAudioUnit as! VST2AudioUnit
            defer { unit.deallocateRenderResources();unit.closePlugin() }
            try unit.allocateRenderResources()
            let output=AVAudioPCMBuffer(pcmFormat:unit.outputBusses[0].format,frameCapacity:64)!
            output.frameLength=64
            var flags=AudioUnitRenderActionFlags(),stamp=AudioTimeStamp();stamp.mFlags = .sampleTimeValid
            let result=unit.internalRenderBlock(&flags,&stamp,64,0,output.mutableAudioBufferList,nil,{_,_,_,_,_ in kAudioUnitErr_CannotDoInCurrentContext})
            unit.synchronizeParameters()
            let advanced=unit.parameterTree?.parameter(withAddress:2)?.value ?? -1
            try require(result == kAudioUnitErr_CannotDoInCurrentContext,"VST2 hid an upstream render failure as successful audio.")
            try require(advanced == 0,"VST2 child advanced after its input failed.")
        }
        await stage("Superseded async project restore cannot overwrite the newer Freeze canonical project") {
            let gate=Gate();let host=try newHost(gate:{ await gate.pause() });defer { gate.release();host.shutdown() }
            func project(_ name:String,plugins:[[String:Any]])->[String:Any] {
                ["id":name,"version":1,"name":name,"bpm":120,"linesPerBeat":4,"activePattern":0,"masterVolume":0,
                 "tracks":[["name":"Original","instrument":0]],"instruments":[["type":"au"]],
                 "patterns":[["id":0,"length":4,"steps":[[NSNull(),NSNull(),NSNull(),NSNull()]]]],"sequence":[0],"audioPlugins":plugins]
            }
            _ = try await host.handle("engine.project",payload:["project":project("before",plugins:[])])
            let first=Task { @MainActor in try await host.handle("engine.project",payload:["project":project("older",plugins:[load]),"awaitPlugins":true]) }
            defer { first.cancel() }
            let deadline=ProcessInfo.processInfo.systemUptime+2
            while !gate.entered && ProcessInfo.processInfo.systemUptime<deadline { try await Task.sleep(nanoseconds:1_000_000) }
            try require(gate.entered,"Original instantiation gate was never reached.")
            _ = try await host.handle("engine.project",payload:["project":project("newer",plugins:[]),"replace":true])
            gate.release()
            var rejected=false;do { _ = try await first.value } catch { rejected=true }
            try require(rejected,"Older publication unexpectedly acknowledged the newer project.")
            try require(host.freezeTrackRuntime.canonical?["name"] as? String == "newer","Older failed publication rolled Freeze canonical state back over the newer project.")
            await host.shutdownAndWait()
        }
        await stage("Session replacement is rejected throughout the Freeze ownership interval") {
            let host=try newHost();defer { host.shutdown() }
            host.freezeTrackRuntime.busy=true
            var rejected=false;do { try host.requireSessionReplacementAllowed() } catch { rejected=true }
            host.freezeTrackRuntime.busy=false
            try require(rejected,"New/Open could replace the session while Freeze owned its restoration.")
            try host.requireSessionReplacementAllowed()
            await host.shutdownAndWait()
        }
        return ["passed":checks.allSatisfy{$0["passed"] as? Bool == true},"checks":checks,
                "scope":"original VST2 bundle, private catalogue, manual offline Host; no installed scans, hardware, or MIDI inputs"]
    }

    @MainActor private final class Gate {
        var entered=false;var continuation:CheckedContinuation<Void,Never>?
        func pause() async { entered=true;await withCheckedContinuation { continuation=$0 } }
        func release() { let saved=continuation;continuation=nil;saved?.resume() }
    }
    private static func privateCatalogue(bundle:URL,folder:URL) throws -> VST2Catalogue {
        guard Bundle(url:bundle)?.bundleIdentifier == "studio.supertracker.tests.debug-vst2" else { throw AudioSampleError.invalid("Only the original debug VST2 fixture is allowed.") }
        let executable=bundle.appendingPathComponent("Contents/MacOS/DebugVST2")
        let attrs=try FileManager.default.attributesOfItem(atPath:executable.path)
        let descriptor=VST2PluginDescriptor(uniqueID:0x44624731,shellID:0,bundlePath:bundle.path,name:"Original Debug VST2",manufacturer:"SuperTracker Tests",version:"1",isInstrument:true,hasEditor:false,inputs:0,outputs:2)
        let raw=try JSONSerialization.jsonObject(with:JSONEncoder().encode(descriptor))
        let cache:[String:Any]=["schema":1,"entries":[["path":bundle.path,"fingerprint":["size":attrs[.size] ?? 0,"modified":(attrs[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0,"version":"1:1"],"records":[raw],"intelOnly":false]]]
        let url=folder.appendingPathComponent("private-vst2-cache.json")
        try JSONSerialization.data(withJSONObject:cache,options:[.sortedKeys]).write(to:url)
        var configuration=VST2Catalogue.Configuration();configuration.roots=[];configuration.cacheURL=url
        return VST2Catalogue(configuration:configuration)
    }
}

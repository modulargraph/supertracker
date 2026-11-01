import Foundation
import AVFoundation

/// Synthetic packets enter the real Host after CoreMIDI parsing. No endpoint is
/// created, selected, or broadcast; every render uses a private offline graph.
enum NativeMIDIPerformanceDebugTests {
    @MainActor static func run(output:URL) async throws -> [String:Any] {
        try FileManager.default.createDirectory(at:output,withIntermediateDirectories:true)
        // Discovery is never requested; even catalogue reads stay inside this fixture.
        var vst2Configuration=VST2Catalogue.Configuration()
        vst2Configuration.roots=[];vst2Configuration.cacheURL=output.appendingPathComponent("empty-vst2-cache.json")
        var vst3Configuration=VST3Catalogue.Configuration()
        vst3Configuration.roots=[];vst3Configuration.cacheURL=output.appendingPathComponent("empty-vst3-cache.json")
        let vst2Catalogue=VST2Catalogue(configuration:vst2Configuration),vst3Catalogue=VST3Catalogue(configuration:vst3Configuration)
        var checks:[[String:Any]]=[]
        func require(_ ok:Bool,_ reason:String) throws { if !ok { throw AudioSampleError.invalid(reason) } }
        func stage(_ name:String,_ body:() async throws -> [String:Any]) async {
            do { var result=try await body();result["name"]=name;result["passed"]=true;checks.append(result) }
            catch { checks.append(["name":name,"passed":false,"error":error.localizedDescription]) }
            try? JSONSerialization.data(withJSONObject:checks,options:[.prettyPrinted,.sortedKeys]).write(to:output.appendingPathComponent("progress.json"),options:.atomic)
        }
        let format=AVAudioFormat(standardFormatWithSampleRate:48000,channels:2)!
        let printURL=output.appendingPathComponent("silent-print.wav")
        do {
            let file=try AVAudioFile(forWriting:printURL,settings:AudioExportFormat.float32.settings,commonFormat:.pcmFormatFloat32,interleaved:false)
            let buffer=AVAudioPCMBuffer(pcmFormat:format,frameCapacity:24000)!;buffer.frameLength=24000
            for channel in 0..<2 { buffer.floatChannelData![channel].initialize(repeating:0,count:24000) }
            try file.write(from:buffer)
            if #available(macOS 15.0,*) { file.close() }
        }
        let record=AudioFreezeTrack.Record(id:"frozen-original",path:printURL.path,sha256:"",sampleRate:48000,frames:24000,musicalFrames:24000,tailFrames:0,
            musicalEndBeat:1,tailSeconds:0,dormantPluginIDs:[],fingerprint:"",mixMode:"flat",createdAt:"")
        func project(frozen:Bool)->[String:Any] {
            var first:[String:Any]=["name":"First","instrument":0,"volume":1,"pan":0,"delaySend":0,"reverbSend":0]
            if frozen { first["frozen"]=record.dictionary }
            return ["id":"private-midi-performance","version":1,"bpm":120,"linesPerBeat":4,"masterVolume":0.5,"activePattern":0,"sequence":[0],"loopSong":false,"metronome":false,
                "tracks":[first,["name":"Live","instrument":0,"volume":1,"pan":0,"delaySend":0,"reverbSend":0]],
                "instruments":[["type":"synth","waveform":"sine","volume":0.4,"attack":0,"decay":0,"sustain":1,"release":0.001,"cutoff":18000,"resonance":0,"drive":0]],
                "patterns":[["id":0,"length":4,"steps":[[Any]](),"automation":[]]],"audioPlugins":[],"audioClips":[],"delay":["mix":0],"reverb":["mix":0]]
        }
        func fixture(frozen:Bool=false) async throws -> AudioHost {
            let graph=AVAudioEngine();try graph.enableManualRenderingMode(.offline,format:format,maximumFrameCount:4096)
            let host=try AudioHost(audioGraph:graph,vst3Catalogue:vst3Catalogue,midiInputIDs:[],midiInputSourceProvider:{[]},vst2Catalogue:vst2Catalogue)
            do {
                _ = try await host.handle("engine.project",payload:["project":project(frozen:frozen),"replace":true,"awaitPlugins":true])
                let devices=try await host.handle("midi.configure",payload:["track":0,"instrument":0]) as! [String:Any]
                try require(devices["allInputs"] as? Bool == false && (devices["inputIDs"] as? [Any])?.isEmpty == true,"Fixture selected a MIDI input.")
                return host
            } catch { await host.shutdownAndWait();throw error }
        }
        func levels(_ host:AudioHost) throws -> Double {
            let buffer=AVAudioPCMBuffer(pcmFormat:format,frameCapacity:4096)!
            for _ in 0..<2 { try require(try host.renderOfflineQuantum(frames:4096,to:buffer) == .success && buffer.frameLength == 4096,"Offline render failed.") }
            var sum=0.0
            for channel in 0..<2 { for i in 0..<4096 { let x=Double(buffer.floatChannelData![channel][i]);sum += x*x } }
            return sqrt(sum/8192)
        }
        func map(_ notes:[Int])->[[Int]] { var value=(0..<128).map{[$0]};value[60]=notes;return value }
        func note(_ host:AudioHost,_ on:Bool,_ pitch:UInt8=60) { host.receiveMIDI(on ? 0x90:0x80,pitch,on ? 100:0,source:1010101) }
        await stage("Disabling performance mode releases every pitch owned by the held chord") {
            let host=try await fixture();defer{host.shutdown()}
            var releases:[Int]=[];host.onEvent={ name,data in if name == "midi.note",data["on"] as? Bool == false,let n=data["note"] as? Int {releases.append(n)} }
            _ = try await host.handle("midi.performance",payload:["map":map([60,64,67])]);note(host,true)
            let held=try levels(host);_ = try await host.handle("midi.performance",payload:[:]);note(host,false)
            let released=try levels(host)
            let metrics:[String:Any]=["heldRMS":held,"releasedRMS":released,"releasedPitches":releases]
            try JSONSerialization.data(withJSONObject:metrics,options:[.prettyPrinted,.sortedKeys]).write(to:output.appendingPathComponent("disable-held.json"))
            try require(held>0.0001 && released<0.000001 && releases == [60,64,67],"Disabling chord mode lost held-note release ownership.")
            await host.shutdownAndWait();return metrics
        }
        await stage("Enabling a transposed map while a raw key is held releases its original raw pitch") {
            let host=try await fixture();defer{host.shutdown()}
            note(host,true);let held=try levels(host)
            _ = try await host.handle("midi.performance",payload:["map":map([72,76,79])]);note(host,false)
            let released=try levels(host)
            try require(held>0.0001 && released<0.000001,"A newly enabled map redirected an existing raw release.")
            await host.shutdownAndWait();return ["heldRMS":held,"releasedRMS":released]
        }
        await stage("Retrigger after disabling a map releases the previous expansion before the new raw onset") {
            let host=try await fixture();defer{host.shutdown()}
            _ = try await host.handle("midi.performance",payload:["map":map([60,64,67])]);note(host,true);_ = try levels(host)
            _ = try await host.handle("midi.performance",payload:[:]);note(host,true);let retriggered=try levels(host);note(host,false)
            let released=try levels(host)
            try require(retriggered>0.0001 && released<0.000001,"Retrigger left the previous chord expansion sounding.")
            await host.shutdownAndWait();return ["retriggeredRMS":retriggered,"releasedRMS":released]
        }
        await stage("Ordinary raw-note retriggers do not acquire an extra synthetic note-off") {
            let host=try await fixture();defer{host.shutdown()}
            var edges:[Bool]=[];host.onEvent={ name,data in if name == "midi.note",let on=data["on"] as? Bool {edges.append(on)} }
            note(host,true);_ = try levels(host);note(host,true);let held=try levels(host);note(host,false);let released=try levels(host)
            try require(edges == [true,true,false] && held>0.0001 && released<0.000001,"Raw-note retrigger semantics changed.")
            await host.shutdownAndWait();return ["edges":edges,"heldRMS":held,"releasedRMS":released]
        }
        await stage("Frozen native track rejects direct MIDI audition while the unfrozen track remains playable") {
            let host=try await fixture(frozen:true);defer{host.shutdown()}
            var onsets=0,errors=0;host.onEvent={name,data in if name == "midi.note",data["on"] as? Bool == true {onsets += 1};if name == "audio.error" {errors += 1} }
            var keyboardRejected=false;do{_ = try await host.handle("note.on",payload:["track":0,"instrument":0,"note":60])}catch{keyboardRejected=true}
            note(host,true);let frozen=try levels(host);note(host,false)
            let frozenOnsets=onsets
            _ = try await host.handle("midi.configure",payload:["track":1,"instrument":0]);note(host,true);let live=try levels(host);note(host,false);let released=try levels(host)
            let metrics:[String:Any]=["frozenRMS":frozen,"liveRMS":live,"releasedRMS":released,"frozenOnsets":frozenOnsets,"errorEvents":errors,"keyboardRejected":keyboardRejected]
            try JSONSerialization.data(withJSONObject:metrics,options:[.prettyPrinted,.sortedKeys]).write(to:output.appendingPathComponent("frozen-audition.json"))
            try require(keyboardRejected && frozen<0.000001 && frozenOnsets==0 && errors>0 && live>0.0001 && released<0.000001,"Direct MIDI bypassed frozen-track audition admission or blocked the live track.")
            await host.shutdownAndWait();return metrics
        }
        await stage("An owned release still reaches the live original track after selecting a frozen track") {
            let host=try await fixture(frozen:true);defer{host.shutdown()}
            _ = try await host.handle("midi.configure",payload:["track":1,"instrument":0]);note(host,true);let held=try levels(host)
            _ = try await host.handle("midi.configure",payload:["track":0,"instrument":0]);note(host,false);let released=try levels(host)
            try require(held>0.0001 && released<0.000001,"Frozen-track admission blocked a previously owned key release.")
            await host.shutdownAndWait();return ["heldRMS":held,"releasedRMS":released]
        }
        return ["passed":checks.allSatisfy{$0["passed"] as? Bool == true},"checks":checks,"scope":"synthetic Host MIDI ingress and generated/native PCM only; offline manual graph, no MIDI endpoints, devices, or vendor plugins"]
    }
}

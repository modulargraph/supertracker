#if SELFTESTS
import Foundation
import AVFoundation
import AudioToolbox
import CoreAudio

enum NativeLineInputTests {
    @MainActor static func run(output:URL) async throws -> [String] {
        func require(_ condition:Bool,_ text:String) throws { if !condition { throw AudioSampleError.invalid("Line inputs: "+text) } }
        let graph=AVAudioEngine(),stereo=AVAudioFormat(standardFormatWithSampleRate:48_000,channels:2)!
        let layout=AVAudioChannelLayout(layoutTag:kAudioChannelLayoutTag_DiscreteInOrder | 6)!
        let multichannel=AVAudioFormat(standardFormatWithSampleRate:48_000,channelLayout:layout)
        try graph.enableManualRenderingMode(.offline,format:stereo,maximumFrameCount:64)
        let source=AVAudioSourceNode(format:multichannel) { silence,_,frames,data in
            silence.pointee=false
            let buffers=UnsafeMutableAudioBufferListPointer(data)
            for channel in 0..<buffers.count {
                let values=buffers[channel].mData!.assumingMemoryBound(to:Float.self)
                for frame in 0..<Int(frames) { values[frame]=Float(channel+1)*0.1 }
            }
            return noErr
        }
        let high=try AudioLineInput(format:multichannel,channels:[4,5])
        let mono=try AudioLineInput(format:multichannel,channels:[2],gain:0.5)
        graph.attach(source);graph.attach(high.node);graph.attach(mono.node)
        graph.connect(source,to:[AVAudioConnectionPoint(node:high.node,bus:0),AVAudioConnectionPoint(node:mono.node,bus:0)],fromBus:0,format:multichannel)
        let rack=try AudioPluginRack(engine:graph,trackCount:2,sampleRate:48_000)
        defer { graph.stop();_ = rack.closeAllPluginsAfterStop() }
        try rack.setLineInputSources([0:high.node,1:mono.node])
        rack.setTrackMix(track:0,gain:1,pan:0,muted:false,instrumentVolume:1,masterVolume:1,trackVolume:1)
        rack.setTrackMix(track:1,gain:1,pan:0,muted:false,instrumentVolume:1,masterVolume:1,trackVolume:1)
        let buffer=AVAudioPCMBuffer(pcmFormat:stereo,frameCapacity:64)!
        try graph.start()
        func render()->(Float,Float) {
            for _ in 0..<3 { _ = try? graph.renderOffline(64,to:buffer) }
            return (buffer.floatChannelData![0][63],buffer.floatChannelData![1][63])
        }
        let both=render()
        try require(abs(both.0-0.65)<0.0001 && abs(both.1-0.75)<0.0001,"Independent stereo channels 5/6 and mono channel 3 did not reach their track chains: \(both)")
        rack.setTrackMix(track:0,gain:0,pan:0,muted:true,instrumentVolume:1,masterVolume:1,trackVolume:1)
        let muted=render()
        try require(abs(muted.0-0.15)<0.0001 && abs(muted.1-0.15)<0.0001,"Muting one track affected another input or bypassed the track mixer: \(muted)")
        rack.setTrackMix(track:1,gain:0.5,pan:0,muted:false,instrumentVolume:1,masterVolume:1,trackVolume:0.5)
        let faded=render()
        try require(abs(faded.0-0.075)<0.0001 && abs(faded.1-0.075)<0.0001,"Live input did not follow the track fader")
        graph.stop()
        // A hardware return is late by one buffer plus input latency: the mix waits for it, the return itself is not delayed.
        try rack.setLineInputSources([1:mono.node])
        rack.hardwareReturnFrames=480
        rack.refreshLatencyCompensation()
        try require(rack.compensationLatencyFrames == 480 && rack.trackCompensators[0].frames == 480 && rack.trackCompensators[1].frames == 0,
                    "Hardware return latency was not compensated: total \(rack.compensationLatencyFrames), tracks \(rack.trackCompensators[0].frames)/\(rack.trackCompensators[1].frames)")
        rack.hardwareReturnFrames=0
        rack.refreshLatencyCompensation()
        try require(rack.compensationLatencyFrames == 0 && rack.trackCompensators[0].frames == 0,"Clearing the hardware return left a delay")
        try rack.setLineInputSources([0:high.node,1:mono.node])
        var routed:[String:Any]=["masterVolume":1,"tracks":[
            ["name":"Stereo return","routingID":"stereo","volume":0.5,"pan":0,"mute":false],
            ["name":"Mono return","routingID":"mono","volume":0.25,"pan":0,"mute":false]],
            "audioPlugins":[],"routing":["version":1,"mixModel":"standard","buses":[],
                "outputs":["track:stereo":"master","track:mono":"master"],"sends":[],"sidechains":[]]]
        try rack.configureRouting(routed)
        rack.setTrackMix(track:0,gain:1,pan:0,muted:false,instrumentVolume:1,masterVolume:1,trackVolume:1)
        rack.setTrackMix(track:1,gain:1,pan:0,muted:false,instrumentVolume:1,masterVolume:1,trackVolume:1)
        try graph.start()
        let routedBoth=render()
        try require(abs(routedBoth.0-0.2875)<0.0001 && abs(routedBoth.1-0.3375)<0.0001,"Routed line inputs bypassed the track mix: \(routedBoth)")
        let soloNodes=Set(graph.attachedNodes.map(ObjectIdentifier.init))
        for (soloTrack,expected) in [(0,(Float(0.25),Float(0.3))),(1,(Float(0.0375),Float(0.0375))),(-1,(Float(0.2875),Float(0.3375)))] {
            var tracks=routed["tracks"] as! [[String:Any]]
            for index in tracks.indices { tracks[index]["solo"]=index == soloTrack }
            routed["tracks"]=tracks
            try rack.configureRouting(routed)
            try require(graph.isRunning && Set(graph.attachedNodes.map(ObjectIdentifier.init)) == soloNodes,"Solo rebuilt or stopped the routed graph")
            let value=render()
            try require(abs(value.0-expected.0)<0.0001 && abs(value.1-expected.1)<0.0001,"Solo/clear did not gate and restore the other live lane: \(value)")
        }
        graph.stop()
        try rack.setLineInputSources([1:mono.node])
        try graph.start()
        let removed=render()
        try require(abs(removed.0-0.0375)<0.0001 && abs(removed.1-0.0375)<0.0001,"Removing one live source left stale audio or broke the other lane: \(removed)")
        graph.stop()
        var routedTracks=routed["tracks"] as! [[String:Any]]
        routedTracks[1]["volume"]=1;routed["tracks"]=routedTracks
        try rack.configureRouting(routed,force:true)
        try graph.start()
        let rewired=render()
        try require(abs(rewired.0-0.15)<0.0001 && abs(rewired.1-0.15)<0.0001,"Routing rebuild lost the surviving line input: \(rewired)")
        graph.stop()
        // A hardware-editor track: a hosted instrument plug-in (here Apple's DLS synth, silent without notes)
        // on the same track as its synth's live audio return. The return must still reach the track.
        let dlsID="\(kAudioUnitType_MusicDevice):\(kAudioUnitSubType_DLSSynth):\(kAudioUnitManufacturer_Apple)"
        _ = try await rack.load(["owner":"track:mono","track":1,"kind":"instrument","slot":0,"component":dlsID])
        routed["audioPlugins"]=[["owner":"track:mono","track":1,"kind":"instrument","slot":0,"component":dlsID]]
        try rack.configureRouting(routed,force:true)
        if !graph.isRunning { try graph.start() }
        let hosted=render()
        try require(abs(hosted.0-0.15)<0.0001 && abs(hosted.1-0.15)<0.0001,"A track with a hosted instrument plug-in lost its line input: \(hosted)")
        graph.stop()
        let inputBuffer=AVAudioPCMBuffer(pcmFormat:multichannel,frameCapacity:8)!
        inputBuffer.frameLength=8
        for channel in 0..<6 { for frame in 0..<8 { inputBuffer.floatChannelData![channel][frame]=Float(channel+1) } }
        let plan=try AudioRecordingChannelPlan(sourceFormat:multichannel,sourceChannels:[4,5])
        let recording=try plan.copy(inputBuffer)
        try require(recording.format.channelCount == 2 && recording.floatChannelData![0][0] == 5 && recording.floatChannelData![1][0] == 6,"Recording remapped high hardware channels twice")
        var rejected=false
        do { _ = try AudioLineInput(format:multichannel,channels:[6]) } catch { rejected=true }
        try require(rejected,"Unavailable hardware channel was accepted")
        try FileManager.default.createDirectory(at:output,withIntermediateDirectories:true)
        let metrics:[String:Any]=["passed":true,"physicalPlayback":false,"flatStereoAndMono":[both.0,both.1],
            "muted":[muted.0,muted.1],"faded":[faded.0,faded.1],"routed":[routedBoth.0,routedBoth.1],
            "removed":[removed.0,removed.1],"rewired":[rewired.0,rewired.1],"recordingChannels":[4,5]]
        try JSONSerialization.data(withJSONObject:metrics,options:[.prettyPrinted,.sortedKeys]).write(to:output.appendingPathComponent("line-input-report.json"))
        let trackCount=try await trackCountKeepsGraph(output:output)
        let reported=try await pausedRouteReportedOnce(output:output)
        let aggregate=try await privateAggregateTopology()
        try [trackCount,reported,aggregate].joined(separator:"\n").write(to:output.appendingPathComponent("line-input-checks.txt"),atomically:true,encoding:.utf8)
        return [trackCount,reported,aggregate,"Routing rebuild and source removal preserve independent line inputs", "Two simultaneous multichannel line selections reach independent track chains", "Line inputs follow track mute and fader", "High hardware recording channels remain exact and invalid channels reject"]
    }

    /// A route saved against an interface that is not the running input stays saved,
    /// is reported once (not on every edit) and never fails the project edit.
    @MainActor private static func pausedRouteReportedOnce(output:URL) async throws -> String {
        func require(_ condition:Bool,_ text:String) throws { if !condition { throw AudioSampleError.invalid("Line inputs: "+text) } }
        let format=AVAudioFormat(standardFormatWithSampleRate:48_000,channels:2)!,graph=AVAudioEngine()
        try graph.enableManualRenderingMode(.offline,format:format,maximumFrameCount:4096)
        let host=try AudioHost(audioGraph:graph,midiInputIDs:[],midiInputSourceProvider:{ [] })
        defer { host.shutdown() }
        var errors:[String]=[]
        host.onEvent={ name,payload in if name == "audio.error" { errors.append(payload["error"] as? String ?? "\(payload)") } }
        // A UID no machine has, so the result never depends on what is plugged in.
        let absent="AppleUSBAudioEngine:SuperTracker:SelfTestInterface:0000:1,2"
        func project(tracks count:Int)->[String:Any] {
            let tracks=(0..<count).map { index -> [String:Any] in
                var track:[String:Any]=["name":"Return \(index+1)","instrument":0,"volume":0.5,"pan":0,"delaySend":0,"reverbSend":0]
                if index < 2 { track["audioInput"]=["enabled":true,"channels":index == 0 ? [4,5] : [3],"deviceUID":absent,"gain":1] }
                return track
            }
            return ["version":1,"name":"Paused line inputs","bpm":120,"linesPerBeat":4,"masterVolume":0.5,"activePattern":0,
                "loopSong":false,"metronome":false,"tracks":tracks,
                "instruments":[["type":"synth","waveform":"sine","volume":0.5,"attack":0,"decay":0,"sustain":1,"release":0.001]],
                "patterns":[["id":0,"length":16,"steps":[[Any]](),"automation":[]]],"sequence":[0],"audioPlugins":[],
                "delay":["mix":0],"reverb":["mix":0]]
        }
        _ = try await host.handle("engine.project",payload:["project":project(tracks:3),"replace":true,"awaitPlugins":true])
        for edit in 0..<6 { _ = try await host.handle("engine.project",payload:["project":project(tracks:3+edit%2)]) }
        try require(errors.count == 1,"A paused/inactive line input was reported \(errors.count) times over 7 edits: \(errors)")
        let authorized=AVCaptureDevice.authorizationStatus(for:.audio) == .authorized
        let devices=try await host.handle("audio.devices",payload:[:]) as? [String:Any] ?? [:]
        let statuses=devices["lineInputs"] as? [[String:Any]] ?? []
        try require(statuses.count == 2 && statuses.allSatisfy { $0["active"] as? Bool == false },"Saved routes were dropped or claimed live: \(statuses)")
        if authorized {
            try require(errors[0].contains("SelfTestInterface not connected — returns paused"),"Pause notice is unclear: \(errors[0])")
            try require(statuses.allSatisfy { $0["state"] as? String == "paused" && ($0["status"] as? String ?? "").contains("returns paused") },"Routes not marked paused: \(statuses)")
        }
        return "Absent-interface line inputs stay saved and are reported once, not on every edit"+(authorized ? " (paused status)" : " (microphone access not granted)")
    }

    /// Reads a PRIVATE aggregate of the built-in devices (invisible to other apps,
    /// destroyed here). Never changes the system default devices.
    @MainActor private static func privateAggregateTopology() async throws -> String {
        func require(_ condition:Bool,_ text:String) throws { if !condition { throw AudioSampleError.invalid("Line inputs: "+text) } }
        let devices=LineInputTopology.objectList(AudioObjectID(kAudioObjectSystemObject),kAudioHardwarePropertyDevices)
        let input=devices.first { AudioDeviceSelection.uid($0) == "BuiltInMicrophoneDevice" }
        guard let input,devices.contains(where: { AudioDeviceSelection.uid($0) == "BuiltInSpeakerDevice" }) else { return "Private aggregate topology skipped (no built-in microphone and speakers)" }
        let defaults=(AudioDeviceSelection.defaultDevice(input:true),AudioDeviceSelection.defaultDevice(input:false))
        let inputs=AudioDeviceSelection.channels(input,input:true)
        // Output first, so the microphone's channels sit after the speaker's (zero) inputs.
        var description=AudioDeviceSelection.pairDescription(inputUID:"BuiltInMicrophoneDevice",inputChannels:inputs,outputUID:"BuiltInSpeakerDevice",outputChannels:2)
        description[kAudioAggregateDeviceSubDeviceListKey]=Array((description[kAudioAggregateDeviceSubDeviceListKey] as! [[String:Any]]).reversed())
        description[kAudioAggregateDeviceNameKey]="SuperTracker Line Input Self-Test"
        var aggregate:AudioDeviceID=0
        guard AudioHardwareCreateAggregateDevice(description as CFDictionary,&aggregate) == noErr,aggregate != 0 else {
            return "Private aggregate topology skipped (Core Audio refused a private aggregate)"
        }
        defer { AudioHardwareDestroyAggregateDevice(aggregate) }
        var topology=LineInputTopology.current(device:aggregate,inputChannels:AudioDeviceSelection.channels(aggregate,input:true))
        for _ in 0..<40 where topology.members.count < 2 {
            try await Task.sleep(nanoseconds:50_000_000)
            topology=LineInputTopology.current(device:aggregate,inputChannels:AudioDeviceSelection.channels(aggregate,input:true))
        }
        try require(topology.members.map(\.uid) == ["BuiltInSpeakerDevice","BuiltInMicrophoneDevice"],"Aggregate members not read in order: \(topology.members)")
        try require(topology.members.map(\.inputChannels) == [0,inputs] && topology.inputChannels == inputs,"Member input channels wrong: \(topology.members) / \(topology.inputChannels)")
        try require(topology.aggregates[topology.uid] == topology.members,"Configured layout disagrees with the live members: \(topology.aggregates)")
        try require(topology.resolve(deviceUID:"BuiltInMicrophoneDevice",channels:[0]) == .live([0]),"Built-in microphone did not map into the aggregate")
        if case .live=topology.resolve(deviceUID:"AppleUSBAudioEngine:SuperTracker:SelfTestInterface:0000:1,2",channels:[0]) { try require(false,"An absent interface resolved live") }
        try require(AudioDeviceSelection.defaultDevice(input:true) == defaults.0 && AudioDeviceSelection.defaultDevice(input:false) == defaults.1,"System default devices changed")
        return "Private aggregate members, order and channel counts read from Core Audio map line inputs (system defaults untouched)"
    }

    /// Regression: adding/removing tracks with no line inputs must not stop the
    /// audio graph (instruments reset, tails cut), and must not fail during a
    /// master recording ("Finish recording before changing live input routes.").
    @MainActor private static func trackCountKeepsGraph(output:URL) async throws -> String {
        func require(_ condition:Bool,_ text:String) throws { if !condition { throw AudioSampleError.invalid("Line inputs: "+text) } }
        let folder=output.appendingPathComponent("track-count",isDirectory:true)
        try FileManager.default.createDirectory(at:folder,withIntermediateDirectories:true)
        let format=AVAudioFormat(standardFormatWithSampleRate:48_000,channels:2)!,graph=AVAudioEngine()
        try graph.enableManualRenderingMode(.offline,format:format,maximumFrameCount:4096)
        let host=try AudioHost(audioGraph:graph,midiInputIDs:[],midiInputSourceProvider:{ [] })
        defer { host.shutdown() }
        var errors:[String]=[]
        host.onEvent={ name,payload in if name == "audio.error" { errors.append(payload["error"] as? String ?? "\(payload)") } }
        func project(tracks count:Int)->[String:Any] {
            let tracks=(0..<count).map { index -> [String:Any] in
                var track:[String:Any]=["name":"Synth \(index+1)","instrument":0,"volume":0.5,"pan":0,"delaySend":0,"reverbSend":0]
                // Saved but disabled routes are not live inputs either.
                if index == 1 { track["audioInput"]=["enabled":false,"channels":[0,1],"deviceUID":"absent-interface","gain":1] }
                return track
            }
            return ["version":1,"name":"Line input track count","bpm":120,"linesPerBeat":4,"masterVolume":0.5,"activePattern":0,
                "loopSong":false,"metronome":false,"tracks":tracks,
                "instruments":[["type":"synth","waveform":"sine","volume":0.5,"attack":0,"decay":0,"sustain":1,"release":0.001]],
                "patterns":[["id":0,"length":16,"steps":[[Any]](),"automation":[]]],"sequence":[0],"audioPlugins":[],
                "delay":["mix":0],"reverb":["mix":0]]
        }
        let buffer=AVAudioPCMBuffer(pcmFormat:format,frameCapacity:4096)!
        func render(_ frames:AVAudioFrameCount=512) throws {
            try require(try host.renderOfflineQuantum(frames:frames,to:buffer) == .success,"Offline render failed")
        }
        _ = try await host.handle("engine.project",payload:["project":project(tracks:1),"replace":true,"awaitPlugins":true])
        _ = try await host.handle("transport.play",payload:["mode":"pattern","fromStart":true])
        try render()
        let baseline=host.lineInputGraphRebuilds
        try require(graph.isRunning,"Fixture graph did not start")
        for count in [2,3,1,4] {
            _ = try await host.handle("engine.project",payload:["project":project(tracks:count)])
            try render()
            try require(graph.isRunning && host.lineInputGraphRebuilds == baseline,"Changing to \(count) tracks without line inputs restarted the audio graph")
        }
        let url=folder.appendingPathComponent("master-take.wav")
        _ = try await host.handle("audio.record",payload:["source":"master","path":url.path])
        try render()
        for count in [5,2] {
            _ = try await host.handle("engine.project",payload:["project":project(tracks:count)])
            try render()
        }
        try require(errors.isEmpty,"Track edits during a master recording reported audio errors: \(errors)")
        try require(host.status()["recording"] as? Bool == true && graph.isRunning && host.lineInputGraphRebuilds == baseline,
            "Track edits interrupted the master recording or restarted the graph")
        var stopped=false
        let stop=Task { @MainActor () throws -> Any in defer { stopped=true };return try await host.handle("audio.stopRecord",payload:[:]) }
        var flushed=0
        while !stopped && flushed < 64 { try render(1024);flushed += 1;await Task.yield() }
        _ = try await stop.value
        return "Adding/removing tracks without line inputs keeps the graph running, including during a master recording"
    }
}
#endif

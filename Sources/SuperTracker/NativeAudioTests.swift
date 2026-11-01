// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation

enum NativeAudioTests {
    @MainActor static func run(output:URL) async throws -> [String] {
        let folder = output.appendingPathComponent("sampler",isDirectory:true)
        try FileManager.default.createDirectory(at:folder,withIntermediateDirectories:true)
        func writeProgress(_ checks:[String],complete:Bool) {
            let progress:[String:Any] = ["complete":complete,"checks":checks,"generatedAt":ISO8601DateFormatter().string(from:Date())]
            if let data=try? JSONSerialization.data(withJSONObject:progress,options:[.prettyPrinted,.sortedKeys]) {
                try? data.write(to:output.appendingPathComponent("audio-regression-progress.json"),options:.atomic)
            }
        }
        var passed:[String] = [] { didSet { writeProgress(passed,complete:false) } }
        var complete=false
        defer { writeProgress(passed,complete:complete) }
        passed = try await NativeAudioRecordingTests.run(output: output)
        passed += try await NativeRecordingFenceTests.run(output: output)
        passed += try await NativeInputAcknowledgementTests.run(output: output)
        func check(_ result:Bool,_ message:String) throws {
            if !result { throw AudioSampleError.invalid("Audio integration test: "+message) }
        }
        func rms(_ sample:DecodedSample)->Double {
            sqrt(sample.left.reduce(0.0) { $0+Double($1)*Double($1) } / Double(max(1,sample.left.count)))
        }
        let rate = 48000.0, sourceURL = folder.appendingPathComponent("source.wav")
        let wave = (0..<9600).map { Float(sin(Double($0)*2*Double.pi*220/rate)*0.4) }
        try AudioSamples.write(DecodedSample(path:sourceURL.path,sampleRate:rate,left:wave,right:wave,originalChannels:2),to:sourceURL)
        let sourceBytes = try Data(contentsOf:sourceURL)
        let original = try AudioSamples.decode(sourceURL.path)
        let host = try AudioHost(midiInputIDs: [])
        defer { host.shutdown() }
        let loaded = try await host.handle("sample.load",payload:["path":sourceURL.path,"instrument":0]) as? [String:Any]
        try check((loaded?["peaks"] as? [Float])?.count == 512,"Sample load did not return a bounded waveform.")
        let detected = try await host.handle("sample.analyzeSlices",payload:["instrument":0,"start":0.1,"end":0.9]) as? [String:Any] ?? [:]
        let points = detected["slicePoints"] as? [Double] ?? []
        try check(!points.isEmpty && points.first == 0 && points.allSatisfy { $0 >= 0 && $0 < 1 },"Transient analysis must return bounded markers relative to the selected sample range.")
        try check(try Data(contentsOf:sourceURL) == sourceBytes,"Transient analysis overwrote source audio.")
        passed.append("Native transient analysis returns selection-relative markers without modifying source audio")
        let reverse = try await host.handle("sample.edit",payload:["instrument":0,"path":sourceURL.path,"operation":"reverse","outputDirectory":folder.path]) as! [String:Any]
        let reversed = try AudioSamples.decode(reverse["path"] as! String)
        try check(reversed.left.count == original.left.count && zip(reversed.left,original.left.reversed()).allSatisfy { abs($0-$1)<0.000002 },"Reverse must reverse audio exactly.")
        let stretch = try await host.handle("sample.edit",payload:["instrument":0,"path":sourceURL.path,"operation":"stretch","ratio":1.5,"outputDirectory":folder.path]) as! [String:Any]
        let stretched = try AudioSamples.decode(stretch["path"] as! String)
        try check(abs(stretched.duration-0.3)<0.001,"Stretch must change duration by the requested ratio.")
        try check(try Data(contentsOf:sourceURL) == sourceBytes,"Sample edits overwrote the original audio.")
        passed.append("Sample decoding, waveform, reverse and pitch-preserving stretch are non-destructive")

        func pattern(_ id:Int,_ rows:Int)->[String:Any] {
            var steps = Array(repeating:[Any](repeating:NSNull(),count:rows),count:2)
            for row in stride(from:0,to:rows,by:4) {
                steps[0][row] = ["note":60,"instrument":0,"velocity":100,"gate":0.8]
                steps[1][row] = ["note":67,"instrument":0,"velocity":80,"gate":0.8]
            }
            return ["id":id,"length":rows,"steps":steps,"automation":[]]
        }
        var project:[String:Any] = ["version":1,"name":"Sampler integration","bpm":120,"linesPerBeat":4,"masterVolume":0.6,"activePattern":1,"loopSong":false,
            "tracks":[["name":"Low","instrument":0,"volume":0.6,"pan":0],["name":"High","instrument":0,"volume":0.6,"pan":0]],
            "instruments":[["type":"sample","samplePath":sourceURL.path,"rootNote":60,"volume":0.7,"mode":"oneshot","attack":0,"decay":0,"sustain":1,"release":0.001]],
            "patterns":[pattern(0,16),pattern(1,8)],"sequence":[0,1],"audioPlugins":[],"delay":["mix":0],"reverb":["mix":0]]
        _ = try await host.handle("engine.project",payload:["project":project])
        let patternURL = folder.appendingPathComponent("pattern.wav")
        _ = try await host.handle("audio.export",payload:["path":patternURL.path,"mode":"pattern","tail":0])
        let renderedPattern = try AudioSamples.decode(patternURL.path)
        try check(abs(renderedPattern.duration-1)<0.001 && rms(renderedPattern)>0.005,"Pattern export must render only the one-second active pattern.")
        // Use a genuinely one-channel file: AudioSamples.write intentionally writes stereo.
        let monoURL=folder.appendingPathComponent("mono-microphone.wav")
        let monoFormat=AVAudioFormat(standardFormatWithSampleRate:rate,channels:1)!
        let monoBuffer=AVAudioPCMBuffer(pcmFormat:monoFormat,frameCapacity:AVAudioFrameCount(wave.count))!
        monoBuffer.frameLength=AVAudioFrameCount(wave.count)
        for index in wave.indices { monoBuffer.floatChannelData![0][index]=wave[index] }
        do {
            let file=try AVAudioFile(forWriting:monoURL,settings:monoFormat.settings)
            try file.write(from:monoBuffer)
        }
        let monoDecoded=try AudioSamples.decode(monoURL.path)
        try check(monoDecoded.originalChannels == 1 && monoDecoded.left == monoDecoded.right,"Mono microphone decoding must duplicate the selected channel to both speakers.")
        var monoProject=project
        var monoInstruments=project["instruments"] as! [[String:Any]]
        monoInstruments[0]["samplePath"]=monoURL.path;monoProject["instruments"]=monoInstruments
        _ = try await host.handle("engine.project",payload:["project":monoProject])
        let monoRenderURL=folder.appendingPathComponent("mono-centered-pattern.wav")
        _ = try await host.handle("audio.export",payload:["path":monoRenderURL.path,"mode":"pattern","tail":0])
        let monoRendered=try AudioSamples.decode(monoRenderURL.path)
        try check(rms(monoRendered)>0.005 && zip(monoRendered.left,monoRendered.right).allSatisfy { abs($0-$1)<0.000001 },"A centered mono microphone sample must produce audible, equal left and right PCM.")
        passed.append("One-channel microphone sample decodes and renders equally to both stereo outputs")
        _ = try await host.handle("engine.project",payload:["project":project])
        let tailURL = folder.appendingPathComponent("pattern-tail.wav")
        _ = try await host.handle("audio.export",payload:["path":tailURL.path,"mode":"pattern","tail":0.5])
        let withTail = try AudioSamples.decode(tailURL.path)
        try check(abs(withTail.duration-1.5)<0.001 && withTail.left.suffix(19000).allSatisfy { abs($0)<0.000001 },"Pattern export tails must decay without replaying pattern notes.")
        passed.append("Sampler pattern-only export has correct duration and audible PCM")
        let stems = folder.appendingPathComponent("stems",isDirectory:true)
        let stemResult = try await host.handle("audio.export",payload:["path":stems.path,"mode":"stems","tail":0]) as! [String:Any]
        let paths = stemResult["paths"] as? [String] ?? []
        try check(paths.count == 2,"Stem export must produce one file per track.")
        let first = try AudioSamples.decode(paths[0]), second = try AudioSamples.decode(paths[1])
        try check(abs(first.duration-3)<0.001 && abs(second.duration-3)<0.001,"Stems must cover the complete three-second song.")
        try check(rms(first)>0.005 && rms(second)>0.005 && first.left != second.left,"Stems must isolate the distinct notes on each track.")
        passed.append("Stereo stem export isolates each track for the complete arrangement")
        let bounce = try await host.handle("audio.bounceTrack",payload:["path":folder.appendingPathComponent("bounced-track.wav").path,"track":0,"tail":0]) as! [String:Any]
        let bounced = try AudioSamples.decode(bounce["path"] as! String)
        try check(abs(bounced.duration-3)<0.001 && rms(bounced)>0.005,"Track bounce must contain the isolated complete arrangement.")
        passed.append("Track bounce creates reusable audio for the complete arrangement")

        var missing = project["instruments"] as! [[String:Any]]
        let unavailableURL = folder.appendingPathComponent("missing-audio-file.wav")
        try? FileManager.default.removeItem(at: unavailableURL)
        missing[0]["samplePath"] = unavailableURL.path
        project["instruments"] = missing
        _ = try await host.handle("engine.project",payload:["project":project])
        let missingURL = folder.appendingPathComponent("missing-sample.wav")
        _ = try await host.handle("audio.export",payload:["path":missingURL.path,"mode":"pattern","tail":0])
        try check(rms(try AudioSamples.decode(missingURL.path))<0.000001,"A missing sample must not play PCM from the previous project.")
        passed.append("Missing sample references produce silence instead of stale audio")
        try sourceBytes.write(to: unavailableURL)
        _ = try await host.handle("engine.project",payload:["project":project])
        let recoveredURL = folder.appendingPathComponent("recovered-sample.wav")
        _ = try await host.handle("audio.export",payload:["path":recoveredURL.path,"mode":"pattern","tail":0])
        try check(rms(try AudioSamples.decode(recoveredURL.path))>0.005,"Reapplying an unchanged missing reference must retry after the audio file becomes available.")
        passed.append("Missing sample references recover after the same file path becomes available")
        // Keep the independent timeline-asset checks isolated from sampler output.
        missing[0]["samplePath"] = folder.appendingPathComponent("still-unavailable.wav").path
        try? FileManager.default.removeItem(atPath: missing[0]["samplePath"] as! String)
        project["instruments"] = missing

        project["audioClips"] = [["id":"independent-clip","track":0,"path":sourceURL.path,"startBeat":8.0,"durationBeats":0.4,"sourceOffset":0,"gain":1,"fadeIn":0,"fadeOut":0,"muted":false]]
        _ = try await host.handle("engine.project",payload:["project":project])
        let clipURL = folder.appendingPathComponent("timeline-clip.wav")
        _ = try await host.handle("audio.export",payload:["path":clipURL.path,"mode":"song","tail":0])
        let withClip = try AudioSamples.decode(clipURL.path)
        try check(abs(withClip.duration-4.2)<0.002,"Audio clips beyond the pattern sequence must extend song export to their end.")
        try check(withClip.left.prefix(191900).allSatisfy { abs($0)<0.000001 } && withClip.left.suffix(9000).contains { abs($0)>0.01 },"Timeline clip must start at beat8 without consuming a sample instrument slot.")
        let noClipPatternURL = folder.appendingPathComponent("timeline-pattern.wav")
        _ = try await host.handle("audio.export",payload:["path":noClipPatternURL.path,"mode":"pattern","tail":0])
        try check(rms(try AudioSamples.decode(noClipPatternURL.path))<0.000001,"Pattern-only export must exclude song timeline clips.")
        passed.append("Audio clips have independent PCM assets, accurate timeline placement and extend song exports")
        let missingPlugin:[String:Any] = ["track":0,"kind":"instrument","slot":0,"component":"missing.test.component","name":"Unavailable test instrument","state":"cHJlc2VydmUtb3BhcXVlLXN0YXRl"]
        project["audioPlugins"] = [missingPlugin]
        _ = try await host.handle("engine.project",payload:["project":project,"replace":true])
        _ = try await host.handle("audio.export",payload:["path":folder.appendingPathComponent("missing-plugin.wav").path,"mode":"pattern","tail":0])
        let retained = try await host.handle("engine.snapshot",payload:[:]) as! [String:Any]
        let retainedPlugins = retained["audioPlugins"] as? [[String:Any]] ?? []
        try check(retainedPlugins.count == 1 && retainedPlugins[0]["state"] as? String == missingPlugin["state"] as? String,"Unavailable Audio Unit descriptors and opaque state must survive project snapshots.")
        _ = try await host.handle("plugin.remove",payload:["track":0,"kind":"instrument","slot":0])
        let removed = try await host.handle("engine.snapshot",payload:[:]) as! [String:Any]
        try check((removed["audioPlugins"] as? [[String:Any]])?.isEmpty == true,"Explicitly removing an unavailable Audio Unit must remove its saved descriptor.")
        passed.append("Unavailable Audio Unit state survives snapshots until explicitly removed")
        // Failed media requests must be retryable after a removable drive returns,
        // for zones and timeline assets as well as the primary sampler path.
        for media in ["zone", "clip"] {
            let mediaURL = folder.appendingPathComponent("retry-"+media+".wav")
            try? FileManager.default.removeItem(at:mediaURL)
            var retryProject = project
            retryProject["audioPlugins"] = []
            retryProject["audioClips"] = []
            retryProject["activePattern"] = 1
            retryProject["sequence"] = [1]
            if media == "zone" {
                retryProject["instruments"] = [["type":"sample","rootNote":60,"volume":0.7,"mode":"oneshot","attack":0,"decay":0,"sustain":1,"release":0.001,
                    "zones":[["path":mediaURL.path,"rootNote":60,"keyLow":0,"keyHigh":127,"velocityLow":0,"velocityHigh":127,"volume":1]]]]
            } else {
                retryProject["instruments"] = [["type":"sample","samplePath":"","volume":0.7]]
                retryProject["patterns"] = [["id":1,"length":8,"steps":Array(repeating:[Any](repeating:NSNull(),count:8),count:2),"automation":[]]]
                retryProject["audioClips"] = [["track":0,"path":mediaURL.path,"startBeat":0,"durationBeats":0.4,"gain":1]]
            }
            _ = try await host.handle("engine.project",payload:["project":retryProject,"replace":true])
            let renderedURL = folder.appendingPathComponent("retry-"+media+"-render.wav")
            _ = try await host.handle("audio.export",payload:["path":renderedURL.path,"mode":"song","tail":0])
            try check(rms(try AudioSamples.decode(renderedURL.path))<0.000001,"An unavailable "+media+" must not play an earlier asset.")
            try sourceBytes.write(to:mediaURL)
            _ = try await host.handle("engine.project",payload:["project":retryProject])
            _ = try await host.handle("audio.export",payload:["path":renderedURL.path,"mode":"song","tail":0])
            try check(rms(try AudioSamples.decode(renderedURL.path))>0.005,"An unchanged "+media+" reference failed to retry when its file became available.")
        }
        passed.append("Missing multisample zones and timeline clips retry the same path after media returns")
        for name in try await NativeExportTransportTests.run(output:output) { passed.append(name) }
        for name in try await NativePluginMoveTests.run(output:output) { passed.append(name) }
        for name in try await NativePluginStateTests.run(output:output) { passed.append(name) }
        for name in try await NativeEndpointLifetimeTests.run(output:output) { passed.append(name) }
        for name in try await NativeAUEffectTests.run(output:output) { passed.append(name) }
        for name in try await NativeModulationTests.run(output:output) { passed.append(name) }
        for name in try await NativeLatencyTests.run(output:output) { passed.append(name) }
        for name in try await NativeRoutingHostTests.run(output:output) { passed.append(name) }
        for name in try await NativeAUStateReuseTests.run(output:output) { passed.append(name) }
        for name in try await NativePluginStepFXTests.run(output:output) { passed.append(name) }
        complete=true
        return passed
    }
}

#endif

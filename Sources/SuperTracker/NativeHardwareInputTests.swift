#if SELFTESTS
import Foundation
import AVFoundation

/// Explicit opt-in only: records two twelve-second microphone files, with monitoring muted.
/// Unlike synthetic graph tests, this traverses the actual configure/record bridge commands.
enum NativeHardwareInputTests {
    @MainActor static func run(output:URL) async throws -> [String] {
        try FileManager.default.createDirectory(at:output,withIntermediateDirectories:true)
        let graph=AVAudioEngine()
        // Reproduce the implicit mixed-rate graph even when hardware is48k.
        graph.connect(graph.mainMixerNode,to:graph.outputNode,
            format:AVAudioFormat(standardFormatWithSampleRate:44_100,channels:2)!)
        let host=try AudioHost(audioGraph:graph,midiInputIDs:[])
        defer { host.shutdown() }
        func require(_ condition:Bool,_ text:String) throws { if !condition { throw AudioSampleError.invalid("Hardware input: "+text) } }
        let before=try await host.handle("audio.devices",payload:[:]) as! [String:Any]
        guard let input=(before["defaultInput"] as? NSNumber)?.uint32Value,
              let device=(before["inputs"] as? [[String:Any]])?.first(where:{($0["id"] as? NSNumber)?.uint32Value == input}) else {
            throw AudioSampleError.invalid("Connect a default microphone before running this opt-in test.")
        }
        let project:[String:Any] = ["version":1,"name":"Hardware capture","bpm":120,"linesPerBeat":4,
            "masterVolume":0,"tracks":[["name":"Capture","instrument":0,"volume":1]],
            "instruments":[["type":"synth","waveform":"sine"]],
            "patterns":[["id":0,"length":16,"steps":[[Any](repeating:NSNull(),count:16)],"automation":[]]],
            "sequence":[0],"activePattern":0,"audioPlugins":[]]
        _ = try await host.handle("engine.project",payload:["project":project,"replace":true,"awaitPlugins":true])
        let path=output.appendingPathComponent("microphone-capture.wav").path
        let outputMapBefore=graph.outputNode.auAudioUnit.channelMap
        let configured=try await host.handle("audio.configure",payload:["inputDevice":input,"inputChannels":[0],"monitor":false,"inputMeter":true]) as! [String:Any]
        try require(graph.mainMixerNode.outputFormat(forBus:0).sampleRate == 48_000,
            "Project start did not align the master render clock with native48k sources")
        try require((configured["outputDevice"] as? NSNumber) == (before["outputDevice"] as? NSNumber),"Selecting microphone changed the output device")
        // AVAudioEngine may create its implicit duplex aggregate on the first
        // inputNode access. Once initialized, reselecting that mic must be a no-op.
        try require(graph.outputNode.auAudioUnit.channelMap == outputMapBefore,"Input selection changed the output channel map")
        let reselected=try await host.handle("audio.configure",payload:["inputDevice":input,"inputChannels":[0],"monitor":false,"inputMeter":true]) as! [String:Any]
        try require((reselected["engineDevice"] as? NSNumber) == (configured["engineDevice"] as? NSNumber),"Reselecting the current microphone replaced the shared duplex device")
        try require((reselected["outputDevice"] as? NSNumber) == (before["outputDevice"] as? NSNumber),"Reselecting microphone changed the output device")
        try require(configured["monitoring"] as? Bool == false && (configured["monitorVolume"] as? NSNumber)?.floatValue == 0,"Microphone monitoring was not muted: \(configured)")
        _ = try await host.handle("audio.record",payload:["source":"input","path":path,"monitor":false])
        for _ in 0..<120 {
            _ = try await host.handle("audio.devices",payload:[:])
            try await Task.sleep(for:.milliseconds(100))
        }
        let stopped=try await host.handle("audio.stopRecord",payload:[:]) as! [String:Any]
        let recorded=try AVAudioFile(forReading:URL(fileURLWithPath:path))
        try require(recorded.length > 0 && recorded.processingFormat.channelCount == 1,"Microphone capture did not produce mono frames")
        _ = try await host.handle("sample.load",payload:["path":path,"instrument":0])
        let secondPath=output.appendingPathComponent("microphone-capture-repeat.wav").path
        _ = try await host.handle("audio.record",payload:["source":"input","path":secondPath,"monitor":false])
        for _ in 0..<120 {
            _ = try await host.handle("engine.snapshot",payload:[:])
            try await Task.sleep(for:.milliseconds(100))
        }
        let second=try await host.handle("audio.stopRecord",payload:[:]) as! [String:Any]
        _ = try await host.handle("sample.load",payload:["path":secondPath,"instrument":0])
        try require(recorded.length >= Int64(recorded.processingFormat.sampleRate*10),"Long capture lost its accepted PCM")
        try require((second["frames"] as? NSNumber)?.int64Value ?? 0 >= Int64(recorded.processingFormat.sampleRate*10),"Repeated long capture lost PCM")
        // Exercise the exact restore request that previously failed with -10851.
        let restored=try await host.handle("audio.configure",payload:["inputDevice":input,"inputChannels":[0],"monitor":false,"inputMeter":false]) as! [String:Any]
        try require((restored["outputDevice"] as? NSNumber) == (before["outputDevice"] as? NSNumber) && restored["monitoring"] as? Bool == false,"Restoring input changed output or enabled monitoring")
        try require(graph.outputNode.auAudioUnit.channelMap == outputMapBefore,"Recording or input restoration changed the output channel map")
        let report:[String:Any]=["passed":true,"outputChannelMapUnchanged":true,"mixedRateGraphRepaired":true,"inputName":device["name"] ?? "Microphone","before":before,"configured":configured,
            "reselected":reselected,"restored":restored,"recording":stopped,"repeatedRecording":second,"recordedFrames":recorded.length,"recordedSampleRate":recorded.processingFormat.sampleRate,
            "recordedChannels":recorded.processingFormat.channelCount,"physicalMicrophoneCapture":true,"microphoneMonitoring":false]
        try JSONSerialization.data(withJSONObject:report,options:[.prettyPrinted,.sortedKeys]).write(to:output.appendingPathComponent("hardware-input-report.json"))
        await host.shutdownAndWait()
        return ["Current microphone selection preserves the output and shared duplex unit", "Real microphone records mono PCM and restores settings with monitoring off"]
    }
}
#endif

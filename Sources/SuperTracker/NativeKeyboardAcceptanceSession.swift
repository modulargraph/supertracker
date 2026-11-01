import AppKit

/// An opt-in visible QA session. This prepares private fixtures and dependencies;
/// the production AppDelegate still creates the window, menus and WKWebView.
@MainActor
final class NativeKeyboardAcceptanceSession {
    static let bundleID = "studio.supertracker.keyboardqa"
    static let bundleOptInKey = "SuperTrackerKeyboardAcceptance"
    static let windowTitle = "SuperTracker Keyboard Acceptance"
    let output: URL
    private(set) var directory: URL?

    init(output: URL) { self.output = output.standardizedFileURL }

    struct Services {
        let store: ProjectStore
        let audio: AudioHost
        let library: LibraryService
        let policy: KeyboardAcceptancePolicy
    }

    func prepare() async throws -> Services {
        let files = FileManager.default
        try files.createDirectory(at:output,withIntermediateDirectories:true)
        let root = output.appendingPathComponent("session-\(UUID().uuidString)",isDirectory:true)
        try files.createDirectory(at:root,withIntermediateDirectories:true)
        directory = root
        let samples = root.appendingPathComponent("Generated samples",isDirectory:true)
        try files.createDirectory(at:samples,withIntermediateDirectories:true)
        let names = ["One Shots/Kick_120bpm_Cmin.wav","One Shots/Snare_100bpm_Dmaj.wav","Loops/Keys_120bpm_Cmin_loop.wav","Textures/Soft_air.wav"]
        var paths: [String] = []
        for (index,name) in names.enumerated() {
            let path = samples.appendingPathComponent(name)
            try files.createDirectory(at:path.deletingLastPathComponent(),withIntermediateDirectories:true)
            let duration = index == 2 ? 2.0 : 0.4
            let pcm = (0..<Int(duration * 48000)).map { frame -> Float in
                let t = Double(frame) / 48000
                let envelope = index == 2 ? 0.15 : 0.3 * exp(-t * 15)
                return Float(sin(t * 2 * .pi * [82.4,220,261.63,440][index]) * envelope)
            }
            try AudioSamples.write(DecodedSample(path:path.path,sampleRate:48000,left:pcm,right:pcm,originalChannels:2),to:path)
            paths.append(path.path)
        }
        var project = NativeSelfTest.testProject()
        project["name"] = "Keyboard acceptance · private fixture"
        project["masterVolume"] = 0; project["audioPlugins"] = []
        project["tracks"] = (0..<8).map { index in
            ["id":index,"name":"Track \(index + 1)","instrument":index % 3,"volume":0.7,"pan":0,"delaySend":0.1,"reverbSend":0.1,"mute":false,"solo":false,"color":"#7dafaa"] as [String: Any]
        }
        project["instruments"] = [
            ["id":0,"name":"Original kick","type":"sample","samplePath":paths[0],"mode":"oneshot","rootNote":60,"volume":0.7,"start":0,"end":1,"attack":0.002,"release":0.1],
            ["id":1,"name":"Original keys loop","type":"sample","samplePath":paths[2],"mode":"slice","sliceCount":8,"rootNote":60,"volume":0.7,"start":0,"end":1,"attack":0.002,"release":0.1],
            ["id":2,"name":"Original synth","type":"synth","waveform":"triangle","rootNote":60,"volume":0.5,"attack":0.005,"decay":0.2,"sustain":0.6,"release":0.2]
        ] as [[String: Any]]
        var patterns = project["patterns"] as! [[String: Any]]
        for index in patterns.indices {
            var lanes = patterns[index]["steps"] as! [[Any]]
            while lanes.count < 8 { lanes.append(Array(repeating:NSNull(),count:16)) }
            patterns[index]["steps"] = lanes
            patterns[index]["name"] = index == 0 ? "Opening" : "Variation"
        }
        project["patterns"] = patterns
        project["audioClips"] = [
            ["id":"keyboard-loop-1","name":"First loop","track":0,"path":paths[2],"startBeat":0,"durationBeats":4,"sourceOffset":0,"gain":0.5],
            ["id":"keyboard-loop-2","name":"Second loop","track":1,"path":paths[2],"startBeat":4,"durationBeats":4,"sourceOffset":0,"gain":0.5]
        ] as [[String: Any]]
        let store = try ProjectStore(directory:root.appendingPathComponent("Project storage",isDirectory:true))
        try store.update(project)
        _ = try store.save(to:root.appendingPathComponent("Keyboard fixture.supertracker"),collectSamples:false)
        let recovery = try PluginRecoveryStore(directory:root.appendingPathComponent("Plugin recovery",isDirectory:true))
        try recovery.startSession()
        let audio = try AudioHost(beforePluginInstantiation: { throw TrackerError.message("Plug-ins are disabled in the isolated keyboard acceptance session.") },recoveryStore:recovery)
        var ready = false
        defer { if !ready { audio.shutdown() } }
        do { _ = try await audio.handle("midi.configure",payload:["enabled":false,"output":"none"]) }
        catch { await audio.shutdownAndWait(); throw error }
        let library = LibraryService(cacheDirectory:root.appendingPathComponent("Library cache",isDirectory:true),defaultRoots:[samples],loadCacheAsynchronously:false)
        _ = try await library.handle("library.scan",payload:["roots":[samples.path]])
        let manifest: [String: Any] = ["sessionDirectory":root.path,"project":store.url!.path,"sampleRoot":samples.path,"windowTitle":Self.windowTitle,"bundleID":Bundle.main.bundleIdentifier ?? "command-line","masterMuted":true,"midiEnabled":false,"pluginLoadsAllowed":false,"externalFilesAllowed":false,"generatedAt":ISO8601DateFormatter().string(from:Date())]
        let data = try JSONSerialization.data(withJSONObject:manifest,options:[.prettyPrinted,.sortedKeys])
        try data.write(to:root.appendingPathComponent("session.json"),options:.atomic)
        try data.write(to:output.appendingPathComponent("latest-session.json"),options:.atomic)
        ready = true
        return Services(store:store,audio:audio,library:library,policy:KeyboardAcceptancePolicy(directory:root))
    }
}

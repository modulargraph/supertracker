import Foundation

@main enum RecordingLocationTests {
    static func main() {
        guard CommandLine.arguments.count == 2 else {
            fputs("Usage: recording-location-tests <private-output-directory>\n", stderr)
            exit(64)
        }
        do { try run() }
        catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
    }
    private static func run() throws {
        let root = URL(fileURLWithPath:CommandLine.arguments[1],isDirectory:true)
        try FileManager.default.createDirectory(at:root,withIntermediateDirectories:true)
        let privateRoot = root.appendingPathComponent("private-session",isDirectory:true)
        let fallback = root.appendingPathComponent("fallback-support",isDirectory:true)
        func require(_ value:Bool,_ message:String) throws {
            if !value { throw NSError(domain:"RecordingLocationTests",code:1,userInfo:[NSLocalizedDescriptionKey:message]) }
        }
        let captured = try AudioRecordingLocation.destination(["name":"  ../../Take:* A  "],
            environment:["SUPERTRACKER_SUPPORT_DIR":privateRoot.path],applicationSupport:fallback)
        try require(captured.deletingLastPathComponent() .standardizedFileURL.path == privateRoot.appendingPathComponent("Recordings",isDirectory:true).standardizedFileURL.path,"Private override was ignored.")
        try require(captured.lastPathComponent.hasPrefix("Take A-") && captured.pathExtension == "wav","Recording filename sanitization changed.")
        try require(!FileManager.default.fileExists(atPath:fallback.path),"Resolving a private recording touched the default support root.")
        let explicit = root.appendingPathComponent("chosen.wav")
        try require(try AudioRecordingLocation.destination(["path":explicit.path],environment:["SUPERTRACKER_SUPPORT_DIR":privateRoot.path],applicationSupport:fallback) .standardizedFileURL.path == explicit.standardizedFileURL.path,"Explicit destination was redirected.")
        let defaulted = try AudioRecordingLocation.destination(["name":"///"],environment:[:],applicationSupport:fallback)
        try require(defaulted.deletingLastPathComponent() .standardizedFileURL.path == fallback.appendingPathComponent("SuperTracker/Recordings",isDirectory:true).standardizedFileURL.path,"Default support location changed.")
        try require(defaulted.lastPathComponent.hasPrefix("Recording-"),"Empty sanitized name lost its default.")
        let relative = try AudioRecordingLocation.destination([:],environment:["SUPERTRACKER_SUPPORT_DIR":"relative"],applicationSupport:fallback)
        try require(relative.deletingLastPathComponent() .standardizedFileURL.path == defaulted.deletingLastPathComponent().standardizedFileURL.path,"Relative support override was accepted.")
        let long = try AudioRecordingLocation.destination(["name":String(repeating:"a",count:100)],environment:["SUPERTRACKER_SUPPORT_DIR":privateRoot.path])
        try require(long.deletingPathExtension().lastPathComponent.count == 89,"Filename bound changed.")
        let report:[String:Any] = ["passed":true,"groups":5,"scope":"Foundation path resolution and private directories only; no recording or audio graph"]
        try JSONSerialization.data(withJSONObject:report,options:[.prettyPrinted,.sortedKeys]).write(to:root.appendingPathComponent("report.json"))
        print("PASS 5 recording destination groups")
    }
}

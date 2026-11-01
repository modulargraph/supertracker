import Foundation

/// Resolve automatic recordings under the same private support root as sessions.
/// Explicit user destinations retain their existing meaning.
enum AudioRecordingLocation {
    static func destination(_ payload:[String:Any],
                            environment:[String:String] = ProcessInfo.processInfo.environment,
                            applicationSupport:URL? = nil) throws -> URL {
        if let path = payload["path"] as? String, !path.isEmpty { return URL(fileURLWithPath:path) }
        let support:URL
        if let value = environment["SUPERTRACKER_SUPPORT_DIR"], value.hasPrefix("/") {
            support = URL(fileURLWithPath:value,isDirectory:true)
        } else {
            support = try (applicationSupport ?? FileManager.default.url(for:.applicationSupportDirectory,
                in:.userDomainMask,appropriateFor:nil,create:true)).appendingPathComponent("SuperTracker",isDirectory:true)
        }
        let base = support.appendingPathComponent("Recordings",isDirectory:true)
        try FileManager.default.createDirectory(at:base,withIntermediateDirectories:true)
        let allowed = CharacterSet.alphanumerics.union(.whitespaces).union(CharacterSet(charactersIn:"-_"))
        let proposed = payload["name"] as? String ?? "Recording"
        let safe = String(String.UnicodeScalarView(proposed.unicodeScalars.filter { allowed.contains($0) }.prefix(80)))
            .trimmingCharacters(in:.whitespacesAndNewlines)
        return base.appendingPathComponent("\(safe.isEmpty ? "Recording" : safe)-\(UUID().uuidString.prefix(8)).wav")
    }
}

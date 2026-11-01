import Foundation

enum ExportDestination {
    /// Reserve a new folder atomically; a previous export or unrelated file is
    /// never reused merely because it has the same project name.
    static func reserveStemsFolder(parent: URL, projectName: String, files: FileManager = .default) throws -> URL {
        guard parent.isFileURL, try parent.resourceValues(forKeys: [.isDirectoryKey]).isDirectory == true else {
            throw NSError(domain: "SuperTracker.Export", code: 1, userInfo: [NSLocalizedDescriptionKey: "Choose an existing folder for the stem export."])
        }
        let forbidden = CharacterSet.controlCharacters.union(CharacterSet(charactersIn: "/:"))
        let cleaned = projectName.unicodeScalars.map { forbidden.contains($0) ? "-" : String($0) }.joined()
            .trimmingCharacters(in: .whitespacesAndNewlines.union(CharacterSet(charactersIn: ".")))
        var name = ""
        for character in cleaned {
            let next = name + String(character)
            if next.utf8.count > 160 { break }
            name = next
        }
        if name.isEmpty { name = "Untitled" }
        let base = name + " stems"
        for attempt in 0..<10_000 {
            let candidate = parent.appendingPathComponent(attempt == 0 ? base : "\(base) \(attempt + 1)", isDirectory: true)
            do {
                try files.createDirectory(at: candidate, withIntermediateDirectories: false)
                return candidate
            } catch {
                // Also handles another export reserving the same name between
                // the attempt and the filesystem operation.
                if files.fileExists(atPath: candidate.path) { continue }
                throw error
            }
        }
        throw NSError(domain: "SuperTracker.Export", code: 2, userInfo: [NSLocalizedDescriptionKey: "Could not reserve a new stem folder. Choose another destination."])
    }
}

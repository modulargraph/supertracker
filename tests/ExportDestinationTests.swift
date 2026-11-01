import Foundation

@main enum ExportDestinationTests {
    static func main() throws {
        let files = FileManager.default
        let parent = files.temporaryDirectory.appendingPathComponent("SuperTracker-export-\(UUID().uuidString)", isDirectory: true)
        try files.createDirectory(at: parent, withIntermediateDirectories: false)
        defer { try? files.removeItem(at: parent) }
        let first = try ExportDestination.reserveStemsFolder(parent: parent, projectName: "Session")
        precondition(first.lastPathComponent == "Session stems")
        let preserved = first.appendingPathComponent("Drums.wav")
        try Data([1, 2, 3]).write(to: preserved)
        let second = try ExportDestination.reserveStemsFolder(parent: parent, projectName: "Session")
        precondition(second.lastPathComponent == "Session stems 2")
        let preservedBytes = try Data(contentsOf: preserved)
        precondition(preservedBytes == Data([1, 2, 3]))
        let collision = parent.appendingPathComponent("Another stems")
        try Data([4, 5]).write(to: collision)
        let third = try ExportDestination.reserveStemsFolder(parent: parent, projectName: "Another")
        precondition(third.lastPathComponent == "Another stems 2")
        let collisionBytes = try Data(contentsOf: collision)
        precondition(collisionBytes == Data([4, 5]))
        for name in ["../Outside/name:\n", String(repeating: "🎹", count: 300), "... "] {
            let result = try ExportDestination.reserveStemsFolder(parent: parent, projectName: name)
            precondition(result.deletingLastPathComponent().standardizedFileURL == parent.standardizedFileURL)
            precondition(result.lastPathComponent.utf8.count < 200)
            precondition(!result.lastPathComponent.contains("/"))
        }
        do {
            _ = try ExportDestination.reserveStemsFolder(parent: collision, projectName: "Invalid")
            preconditionFailure("A file cannot be used as the parent directory")
        } catch {}
        print("PASS stem destinations: fresh directory, file/directory collision preservation, bounded Unicode names and invalid parent")
    }
}

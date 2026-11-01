import Foundation

/// Manual: benchmarks search over a COPY of a real library cache directory.
/// swiftc -O Sources/SuperTracker/KitLibrary.swift Sources/SuperTracker/LibraryService.swift Sources/SuperTracker/NKSMetadata.swift tests/LibraryIndexBenchmark.swift -o /tmp/library-bench && /tmp/library-bench <copied cache dir>
@main struct LibraryIndexBenchmark {
    static func main() throws {
        guard CommandLine.arguments.count > 1 else { print("usage: library-bench <copy of SuperTracker/Library>"); exit(64) }
        let directory = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        let queries = ["kick", "808 kick", "dark pad", "vocal chop", "snare 909", "bass fm", "loop 120", "a", "jungle", "bpm:120", "key:Am"]
        let filters: [[String: Any]] = [["categories": ["Kick"], "collections": ["Maschine expansions"]], ["collections": ["Loopcloud"]], ["bpm": ["min": 118, "max": 122]], ["keys": ["A minor"], "sampleTypes": ["loop"]]]
        let result = LibraryService.benchmarkCachedIndex(directory: directory, queries: queries, filters: filters)
        print(String(data: try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys]), encoding: .utf8)!)
    }
}

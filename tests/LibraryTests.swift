import Foundation

/// Fixture-only functional regressions; never index or modify the user's libraries/cache.
@main
struct LibraryTests {
    static func main() throws {
        for check in try LibraryService.selfTest() { print("PASS library: \(check)") }
        for check in try NKSMetadata.selfTest() { print("PASS \(check)") }
        for check in try NKSArtwork.selfTest() { print("PASS \(check)") }
    }
}

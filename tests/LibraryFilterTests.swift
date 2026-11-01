import Foundation

@main
struct LibraryFilterTests {
    static func main() async throws {
        for check in try LibraryService.filterSelfTest() { print("PASS library filters: \(check)") }
        for check in try await LibraryService.browsingSelfTest() { print("PASS library browsing: \(check)") }
    }
}

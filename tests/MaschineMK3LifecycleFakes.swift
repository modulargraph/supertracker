import Foundation
import Dispatch

enum TrackerError: Error, LocalizedError { case message(String);var errorDescription:String? { switch self { case .message(let text):return text } } }
struct MK3AgentRecord { var name = "fake" }
enum MK3AgentController {
    static let lock = NSLock()
    static var entered = false
    static var gate = DispatchSemaphore(value: 0)
    static var stopCalls = 0
    static var restoreCalls = 0
    static var restoreFailures = 0
    static var recorded: [MK3AgentRecord] = []
    static var opens = 0
    static func stop(log: (String) -> Void) throws -> [MK3AgentRecord] {
        lock.lock(); entered = true; stopCalls += 1; lock.unlock()
        guard gate.wait(timeout: .now() + 5) == .success else { throw TrackerError.message("test gate timeout") }
        lock.lock(); recorded = [MK3AgentRecord()]; lock.unlock()
        return [MK3AgentRecord()]
    }
    static func hasEntered() -> Bool { lock.lock(); defer { lock.unlock() }; return entered }
    static func restore(_ records: [MK3AgentRecord], log: (String) -> Void) -> Bool {
        lock.lock(); defer { lock.unlock() }; restoreCalls += 1
        if restoreFailures > 0 { restoreFailures -= 1; return false }
        recorded = []; return true
    }
    static func readState() -> [MK3AgentRecord]? { lock.lock(); defer { lock.unlock() }; return recorded }
    static func recoverIfNeeded(log: (String) -> Void) {}
}
final class MK3HIDSession: @unchecked Sendable {
    let queue = DispatchQueue(label: "fixture.hid")
    var onEvents: (([MK3InputEvent], Bool) -> Void)?
    var onConnection: ((Bool, String?) -> Void)?
    var onRawReport: ((UInt32, [UInt8]) -> Void)?
    var reportCounts: [UInt32: Int] = [:]
    var firstReports: [UInt32: [UInt8]] = [:]
    var initialReportResult: Int32?
    var ledReportsSent = 0
    var ledReportFailures: [String] = []
    static var active:MK3HIDSession?
    func start() -> String? { Self.active=self;onConnection?(true,nil); return nil }
    static func reconnect() { active?.onConnection?(true,nil) }
    func stop() { onConnection?(false,nil) }
    func send(_ reports: [[UInt8]], force: Bool = false) {}
    func sendNow(_ reports: [[UInt8]]) {}
}
final class MK3ScreenSession: @unchecked Sendable {
    let queue = DispatchQueue(label: "fixture.screen")
    let dumpDirectory: URL? = nil
    var onFailure: ((String) -> Void)?
    var framesSent = 0, bytesSent = 0, dumpMismatches = 0
    static var present = true
    static var openFailures = 0
    static var initializeFailures = 0
    static var initializeCalls = 0
    static var blockClose = false
    static var closeEntered = false
    static let closeGate = DispatchSemaphore(value: 0)
    static func hasEnteredClose() -> Bool { MK3AgentController.lock.lock(); defer { MK3AgentController.lock.unlock() }; return closeEntered }
    func isDevicePresent() -> Bool { Self.present }
    func open() throws {
        MK3AgentController.opens += 1
        if Self.openFailures > 0 { Self.openFailures -= 1;throw TrackerError.message("fixture USB open failure") }
    }
    func initializeDisplays() throws {
        Self.initializeCalls += 1
        if Self.initializeFailures > 0 { Self.initializeFailures -= 1;throw TrackerError.message("fixture incomplete USB screen transfer") }
    }
    func close(blank: Bool) {
        if Self.blockClose {
            Self.blockClose = false
            MK3AgentController.lock.lock(); Self.closeEntered = true; MK3AgentController.lock.unlock()
            _ = Self.closeGate.wait(timeout: .now() + 5)
        }
    }
    func submit(screen: Int, ops: [MK3DrawOp], dumpTag: String? = nil) {}
}

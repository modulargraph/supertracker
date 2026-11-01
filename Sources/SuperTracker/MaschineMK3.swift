import Foundation
import IOKit
import IOKit.hid
import IOUSBHost

// Maschine MK3 device layer.
//
// Shared mode (default, automatic at launch): IOHIDManager on interface 4, no seize. Buttons, pads, encoders
// and LEDs work alongside Native Instruments' agents (if those agents leave HID input unseized).
// Exclusive mode (only on `mk3.connect {exclusive:true}`): stops NIHostIntegrationAgent and NIHardwareAgent
// for this GUI user, opens interface 5 for the screens, and restores the agents on disconnect, quit, failure
// or — through a state file — on the next launch after a crash.

// MARK: - NI agent control

struct MK3AgentRecord: Codable, Equatable {
    var name: String
    var label: String?
    var appPath: String
    var plistPath: String?
}

enum MK3AgentController {
    /// (process name, launchd label fragment, app bundle). NIHardwareAgent's bundle id is NIHardwareService.
    static let agents: [(name: String, labelFragment: String, appPath: String)] = [
        ("NIHostIntegrationAgent", "com.native-instruments.nihostintegrationagent",
         "/Library/Application Support/Native Instruments/Hardware/NIHostIntegrationAgent.app"),
        ("NIHardwareAgent", "com.native-instruments.nihardwareservice",
         "/Library/Application Support/Native Instruments/Hardware/NIHardwareAgent.app"),
    ]

    static var stateURL: URL {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("Library/Application Support")
        return support.appendingPathComponent("SuperTracker/MaschineMK3/stopped-ni-agents.json")
    }

    static var domain: String { "gui/\(getuid())" }

    @discardableResult
    static func run(_ executable: String, _ arguments: [String]) -> (status: Int32, output: String) {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = arguments
        let pipe = Pipe()
        process.standardOutput = pipe; process.standardError = pipe
        do { try process.run() } catch { return (-1, error.localizedDescription) }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return (process.terminationStatus, String(decoding: data, as: UTF8.self))
    }

    /// launchd jobs of this GUI session whose label mentions Native Instruments.
    static func launchdJobs() -> [(pid: pid_t?, label: String)] {
        run("/bin/launchctl", ["list"]).output.split(separator: "\n").compactMap { line in
            let columns = line.split(separator: "\t", omittingEmptySubsequences: false)
            guard columns.count >= 3, columns[2].lowercased().contains("native") else { return nil }
            return (pid_t(columns[0]), String(columns[2]))
        }
    }

    static func pids(_ name: String) -> [pid_t] {
        run("/usr/bin/pgrep", ["-x", "-U", "\(getuid())", name]).output.split(separator: "\n").compactMap { pid_t($0.trimmingCharacters(in: .whitespaces)) }
    }

    static func isRunning(_ name: String) -> Bool { !pids(name).isEmpty }

    private static func plistPath(label: String) -> String? {
        for line in run("/bin/launchctl", ["print", "\(domain)/\(label)"]).output.split(separator: "\n") {
            let text = line.trimmingCharacters(in: .whitespaces)
            if text.hasPrefix("path = ") {
                let path = String(text.dropFirst(7))
                return path.hasSuffix(".plist") && FileManager.default.fileExists(atPath: path) ? path : nil
            }
        }
        return nil
    }

    static func readState() -> [MK3AgentRecord]? {
        guard let data = try? Data(contentsOf: stateURL) else { return nil }
        return try? JSONDecoder().decode([MK3AgentRecord].self, from: data)
    }

    static func writeState(_ records: [MK3AgentRecord]) throws {
        try FileManager.default.createDirectory(at: stateURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        try JSONEncoder().encode(records).write(to: stateURL, options: .atomic)
    }

    static func clearState() { try? FileManager.default.removeItem(at: stateURL) }

    /// Stops the running NI agents and returns what must be restored. The state file is written before
    /// anything is stopped so a crash in between still restores them on the next launch.
    static func stop(log: (String) -> Void) throws -> [MK3AgentRecord] {
        let jobs = launchdJobs()
        var records = readState() ?? []
        for agent in agents where isRunning(agent.name) || jobs.contains(where: { $0.label.lowercased().contains(agent.labelFragment) && $0.pid != nil }) {
            let label = jobs.first { $0.label.lowercased().contains(agent.labelFragment) }?.label
            let record = MK3AgentRecord(name: agent.name, label: label, appPath: agent.appPath,
                                        plistPath: label.flatMap { $0.hasPrefix("application.") ? nil : plistPath(label: $0) })
            if !records.contains(where: { $0.name == record.name }) { records.append(record) }
        }
        guard !records.isEmpty else { log("NI agents are not running"); return [] }
        try writeState(records)
        for attempt in 0..<3 {
            for record in records where isRunning(record.name) {
                if let label = record.label {
                    if record.plistPath != nil {
                        // A plist job may have KeepAlive: boot it out so launchd does not respawn it.
                        let result = run("/bin/launchctl", ["bootout", "\(domain)/\(label)"])
                        log("launchctl bootout \(label) → \(result.status)")
                    } else {
                        let result = run("/bin/launchctl", ["kill", "SIGTERM", "\(domain)/\(label)"])
                        log("launchctl kill SIGTERM \(label) → \(result.status)")
                    }
                }
                for pid in pids(record.name) { kill(pid, SIGTERM) }
            }
            let deadline = Date().addingTimeInterval(3)
            while Date() < deadline, records.contains(where: { isRunning($0.name) }) { usleep(100_000) }
            for record in records { for pid in pids(record.name) { log("SIGKILL \(record.name) \(pid)"); kill(pid, SIGKILL) } }
            usleep(attempt == 0 ? 800_000 : 400_000)
            // Some NI components relaunch each other; repeat until none came back.
            if !records.contains(where: { isRunning($0.name) }) { break }
            log("NI agents came back; stopping again")
        }
        let survivors = records.filter { isRunning($0.name) }.map(\.name)
        if !survivors.isEmpty { throw TrackerError.message("Could not stop \(survivors.joined(separator: ", ")).") }
        log("stopped \(records.map(\.name).joined(separator: ", "))")
        return records
    }

    /// Restarts stopped agents (NIHardwareAgent first). Clears the state file when all are running again.
    @discardableResult
    static func restore(_ records: [MK3AgentRecord], log: (String) -> Void) -> Bool {
        let ordered = records.filter { $0.name == "NIHardwareAgent" } + records.filter { $0.name != "NIHardwareAgent" }
        for record in ordered where !isRunning(record.name) {
            if let plist = record.plistPath, let label = record.label {
                let boot = run("/bin/launchctl", ["bootstrap", domain, plist])
                let kick = run("/bin/launchctl", ["kickstart", "\(domain)/\(label)"])
                log("launchctl bootstrap/kickstart \(label) → \(boot.status)/\(kick.status)")
            } else if FileManager.default.fileExists(atPath: record.appPath) {
                let result = run("/usr/bin/open", ["-g", "-j", record.appPath])
                log("open \(record.name) → \(result.status)")
            }
            let deadline = Date().addingTimeInterval(4)
            while Date() < deadline, !isRunning(record.name) { usleep(100_000) }
        }
        let missing = records.filter { !isRunning($0.name) }.map(\.name)
        if missing.isEmpty { clearState(); log("NI agents restored"); return true }
        log("could not restart \(missing.joined(separator: ", "))")
        return false
    }

    /// Crash recovery: restores agents recorded by an earlier run that did not shut down cleanly.
    static func recoverIfNeeded(log: (String) -> Void) {
        guard let records = readState(), !records.isEmpty else { return }
        log("restoring NI agents left stopped by an earlier run")
        restore(records, log: log)
    }
}

// MARK: - HID (interface 4)

final class MK3HIDSession: @unchecked Sendable {
    let queue = DispatchQueue(label: "supertracker.mk3.hid", qos: .userInteractive)
    private var manager: IOHIDManager?
    private var device: IOHIDDevice?
    private var parser = MK3InputParser()
    private var lastSent: [UInt8: [UInt8]] = [:]
    private var pendingReports: [UInt8: [UInt8]] = [:]
    private var flushScheduled = false
    private var lastFlush = DispatchTime(uptimeNanoseconds: 0)
    private(set) var reportCounts: [UInt32: Int] = [:]
    private(set) var firstReports: [UInt32: [UInt8]] = [:]
    private(set) var initialReportResult: IOReturn?
    private(set) var ledReportsSent = 0
    private(set) var ledReportFailures: [String] = []
    var minimumOutputInterval = 1.0 / 60

    /// Called on `queue`.
    var onEvents: (([MK3InputEvent], Bool) -> Void)?
    var onConnection: ((Bool, String?) -> Void)?
    var onRawReport: ((UInt32, [UInt8]) -> Void)?

    private var context: UnsafeMutableRawPointer { Unmanaged.passUnretained(self).toOpaque() }

    /// Starts matching 17cc:1600. Must be called on `queue`.
    func start() -> String? {
        guard manager == nil else { return nil }
        let manager = IOHIDManagerCreate(kCFAllocatorDefault, IOOptionBits(kIOHIDOptionsTypeNone))
        IOHIDManagerSetDeviceMatching(manager, [kIOHIDVendorIDKey: MK3Protocol.vendorID, kIOHIDProductIDKey: MK3Protocol.productID] as CFDictionary)
        IOHIDManagerRegisterDeviceMatchingCallback(manager, { context, _, _, device in
            guard let context else { return }
            Unmanaged<MK3HIDSession>.fromOpaque(context).takeUnretainedValue().attached(device)
        }, context)
        IOHIDManagerRegisterDeviceRemovalCallback(manager, { context, _, _, device in
            guard let context else { return }
            Unmanaged<MK3HIDSession>.fromOpaque(context).takeUnretainedValue().detached(device)
        }, context)
        // Dispatch-queue HID objects only accept callbacks before activation: register input on the manager.
        IOHIDManagerRegisterInputReportCallback(manager, { context, _, sender, _, reportID, report, length in
            guard let context else { return }
            let bytes = Array(UnsafeBufferPointer(start: report, count: max(0, length)))
            let session = Unmanaged<MK3HIDSession>.fromOpaque(context).takeUnretainedValue()
            if let sender, let device = session.device, sender != Unmanaged.passUnretained(device).toOpaque() { return }
            session.received(reportID: reportID, bytes: bytes)
        }, context)
        IOHIDManagerSetDispatchQueue(manager, queue)
        let result = IOHIDManagerOpen(manager, IOOptionBits(kIOHIDOptionsTypeNone))
        IOHIDManagerActivate(manager)
        self.manager = manager
        return result == kIOReturnSuccess ? nil : String(format: "IOHIDManagerOpen failed (0x%08x)", result)
    }

    /// Must be called on `queue`.
    func stop() {
        if let device { finishDevice(device, notify: true) }
        if let manager {
            IOHIDManagerCancel(manager)
            IOHIDManagerClose(manager, IOOptionBits(kIOHIDOptionsTypeNone))
        }
        manager = nil
    }

    var isConnected: Bool { device != nil }

    private func attached(_ device: IOHIDDevice) {
        guard self.device == nil else { return }
        self.device = device
        parser = MK3InputParser()
        lastSent = [:]
        onConnection?(true, nil)
        // Ask for the current control state (GET_REPORT input 0x01) so knob baselines and held buttons are
        // known before the first interrupt report. Devices may refuse this; that is harmless.
        var buffer = [UInt8](repeating: 0, count: 64)
        var length = CFIndex(buffer.count)
        buffer[0] = MK3Protocol.buttonReportID
        let result = IOHIDDeviceGetReport(device, kIOHIDReportTypeInput, CFIndex(MK3Protocol.buttonReportID), &buffer, &length)
        initialReportResult = result
        if result == kIOReturnSuccess, length > 0 { received(reportID: UInt32(MK3Protocol.buttonReportID), bytes: Array(buffer.prefix(length))) }
    }

    private func detached(_ device: IOHIDDevice) {
        guard let current = self.device, CFEqual(current, device) else { return }
        finishDevice(device, notify: true)
    }

    private func finishDevice(_ device: IOHIDDevice, notify: Bool) {
        self.device = nil
        let released = parser.releaseAll()
        if !released.isEmpty { onEvents?(released, true) }
        if notify { onConnection?(false, nil) }
    }

    private func received(reportID: UInt32, bytes: [UInt8]) {
        let report = MK3InputParser.normalize(bytes, reportID: reportID)
        let id = UInt32(report.first ?? 0)
        reportCounts[id, default: 0] += 1
        if firstReports[id] == nil { firstReports[id] = report }
        onRawReport?(id, report)
        let events = parser.parse(report, now: ProcessInfo.processInfo.systemUptime)
        if !events.isEmpty { onEvents?(events, false) }
    }

    /// Queues output reports (byte 0 = report ID); sends only changed reports, at most ~60 per second.
    func send(_ reports: [[UInt8]], force: Bool = false) {
        queue.async { [self] in
            if force { lastSent = [:] }
            for report in reports { if let id = report.first { pendingReports[id] = report } }
            guard !flushScheduled else { return }
            flushScheduled = true
            let earliest = lastFlush + minimumOutputInterval
            queue.asyncAfter(deadline: max(earliest, .now())) { [self] in flush() }
        }
    }

    private func flush() {
        flushScheduled = false
        lastFlush = .now()
        guard let device else { return }
        for (id, report) in pendingReports.sorted(by: { $0.key < $1.key }) where lastSent[id] != report {
            let result = report.withUnsafeBufferPointer { buffer in
                IOHIDDeviceSetReport(device, kIOHIDReportTypeOutput, CFIndex(id), buffer.baseAddress!, buffer.count)
            }
            if result == kIOReturnSuccess { lastSent[id] = report; ledReportsSent += 1 }
            else {
                NSLog("Maschine MK3 LED report 0x%02x failed: 0x%08x", id, result)
                ledReportFailures = Array((ledReportFailures + [String(format: "0x%02x: 0x%08x", id, result)]).suffix(8))
            }
        }
        pendingReports = [:]
    }

    /// Sends immediately and synchronously (shutdown). Must be called on `queue`.
    func sendNow(_ reports: [[UInt8]]) {
        for report in reports { if let id = report.first { pendingReports[id] = report } }
        lastSent = [:]
        flush()
    }
}

// MARK: - Screens (interface 5, bulk OUT 0x04)

/// A successful USB request can still report fewer bytes than the frame packet.
/// Such a transfer must not advance the screen delta baseline or confirm takeover.
enum MK3ScreenTransferValidation {
    static func requireComplete(requested: Int, transferred: Int) throws {
        guard requested > 0, transferred == requested else {
            throw TrackerError.message("Maschine MK3 screen transfer was incomplete (\(transferred) of \(requested) bytes). Check the USB connection and retry Take over screens.")
        }
    }
}

final class MK3ScreenSession: @unchecked Sendable {
    let queue = DispatchQueue(label: "supertracker.mk3.screen", qos: .userInitiated)
    private var interface: IOUSBHostInterface?
    private var pipe: IOUSBHostPipe?
    private var renderer: MK3ScreenRenderer?
    private var pending: [Int: [MK3DrawOp]] = [:]
    private var sent: [Int: [UInt16]] = [:]
    /// Debug dump (SUPERTRACKER_MK3_SCREEN_DUMP=<dir>): every rendered frame is written as PNG, also without
    /// screens (render-only), so layouts can be checked without the device.
    let dumpDirectory: URL? = ProcessInfo.processInfo.environment["SUPERTRACKER_MK3_SCREEN_DUMP"].map { URL(fileURLWithPath: $0, isDirectory: true) }
    private var pendingTag: [Int: String] = [:]
    private var decoded: [Int: [UInt16]] = [:]
    private var dumpSent: [Int: [UInt16]] = [:]
    private var dumpCount = 0
    private(set) var dumpMismatches = 0
    private var scheduled = false
    private var lastFrame = DispatchTime(uptimeNanoseconds: 0)
    private var failures = 0
    var frameInterval = 1.0 / 30
    private(set) var framesSent = 0
    private(set) var bytesSent = 0
    /// Called on `queue` when the screens stop working.
    var onFailure: ((String) -> Void)?

    var isOpen: Bool { pipe != nil }

    private static var matching: NSDictionary {
        ["IOProviderClass": "IOUSBHostInterface",
         "IOPropertyMatch": ["idVendor": MK3Protocol.vendorID, "idProduct": MK3Protocol.productID, "bInterfaceNumber": 5]]
    }

    /// Read-only preflight; discovering a service does not open or seize it.
    func isDevicePresent() -> Bool {
        let service = IOServiceGetMatchingService(kIOMainPortDefault, Self.matching as CFDictionary)
        guard service != IO_OBJECT_NULL else { return false }
        IOObjectRelease(service)
        return true
    }

    /// Opens interface 5. Must be called on `queue`. Retries while the previous owner lets go.
    func open(timeout: TimeInterval = 4) throws {
        guard pipe == nil else { return }
        let deadline = Date().addingTimeInterval(timeout)
        var lastError: Error?
        var seize = false
        while true {
            // IOUSBHostInterface.createMatchingDictionary's top-level keys do not match through
            // IOServiceGetMatchingService on this macOS; IOPropertyMatch does (verified against the MK3).
            let service = IOServiceGetMatchingService(kIOMainPortDefault, Self.matching as CFDictionary)
            if service != IO_OBJECT_NULL {
                defer { IOObjectRelease(service) }
                do {
                    let interface = try IOUSBHostInterface(__ioService: service, options: seize ? [.deviceSeize] : [], queue: nil, interestHandler: nil)
                    do {
                        let pipe = try interface.copyPipe(withAddress: 0x04)
                        self.interface = interface; self.pipe = pipe
                        sent = [:]; failures = 0
                        return
                    } catch { interface.destroy(); throw error }
                } catch { lastError = error }
            } else {
                lastError = TrackerError.message("Maschine MK3 screen interface not found.")
            }
            if Date() >= deadline {
                if !seize { seize = true; continue }
                throw TrackerError.message("Could not open the Maschine MK3 screens: \(lastError?.localizedDescription ?? "unknown error")")
            }
            usleep(250_000)
        }
    }

    /// Establish both display baselines synchronously before advertising screens.
    /// Failure leaves ownership with the driver, which closes and restores agents.
    func initializeDisplays() throws {
        let black = [UInt16](repeating: 0, count: MK3Screen.pixelCount)
        for screen in 0..<2 {
            guard let packet = MK3ScreenEncoder.packet(screen: screen, previous: nil, next: black) else {
                throw TrackerError.message("Could not initialize the Maschine MK3 screen packet.")
            }
            if let error = write(packet) { throw error }
            sent[screen] = black
            framesSent += 1
        }
    }

    /// Must be called on `queue`.
    func close(blank: Bool) {
        if blank, pipe != nil {
            let black = [UInt16](repeating: 0, count: MK3Screen.pixelCount)
            for screen in 0..<2 { if let packet = MK3ScreenEncoder.packet(screen: screen, previous: sent[screen], next: black) { _ = write(packet) } }
        }
        pending = [:]; sent = [:]
        pipe = nil
        interface?.destroy()
        interface = nil
    }

    /// Replaces the pending display list for a screen; frames are coalesced to ≤ 30 per second.
    /// `dumpTag` names the dumped PNGs of this frame (debug dumps only).
    func submit(screen: Int, ops: [MK3DrawOp], dumpTag: String? = nil) {
        queue.async { [self] in
            guard pipe != nil || dumpDirectory != nil else { return }
            pending[screen] = ops
            if let dumpTag { pendingTag[screen] = dumpTag }
            guard !scheduled else { return }
            scheduled = true
            queue.asyncAfter(deadline: max(lastFrame + frameInterval, .now())) { [self] in renderPending() }
        }
    }

    private func write(_ packet: [UInt8]) -> Error? {
        guard let pipe else { return TrackerError.message("Screens are closed.") }
        let data = NSMutableData(bytes: packet, length: packet.count)
        var transferred = 0
        do {
            try pipe.__sendIORequest(with: data, bytesTransferred: &transferred, completionTimeout: 2)
            try MK3ScreenTransferValidation.requireComplete(requested: packet.count, transferred: transferred)
            bytesSent += transferred
            return nil
        } catch { return error }
    }

    private func renderPending() {
        scheduled = false
        lastFrame = .now()
        guard pipe != nil || dumpDirectory != nil else { pending = [:]; return }
        let work = pending, tags = pendingTag
        pending = [:]; pendingTag = [:]
        if renderer == nil { renderer = MK3ScreenRenderer() }
        guard let renderer else { return }
        for (screen, ops) in work.sorted(by: { $0.key < $1.key }) {
            renderer.render(ops)
            let frame = renderer.framebuffer()
            if dumpDirectory != nil { dump(screen: screen, renderer: renderer, frame: frame, tag: tags[screen]) }
            guard pipe != nil else { continue }
            guard let packet = MK3ScreenEncoder.packet(screen: screen, previous: sent[screen], next: frame) else { continue }
            if let error = write(packet) {
                sent[screen] = nil
                failures += 1
                NSLog("Maschine MK3 screen write failed: %@", error.localizedDescription)
                if failures >= 3 { onFailure?(error.localizedDescription); return }
                // The interface sends a display list only when it changes: retry this frame (in full, since the
                // baseline is gone) unless a newer one is already waiting, or the screen stays stale.
                if pending[screen] == nil { pending[screen] = ops }
                if !scheduled {
                    scheduled = true
                    queue.asyncAfter(deadline: .now() + frameInterval) { [self] in renderPending() }
                }
            } else {
                sent[screen] = frame; failures = 0; framesSent += 1
            }
        }
    }

    /// Writes `<dir>/left.png` (rendered bitmap) and `left-device.png` (16-bit framebuffer decoded back from the
    /// bulk packet), plus `<tag>-left*.png` when tagged and numbered copies with SUPERTRACKER_MK3_SCREEN_DUMP_ALL.
    private func dump(screen: Int, renderer: MK3ScreenRenderer, frame: [UInt16], tag: String?) {
        guard let folder = dumpDirectory else { return }
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        // Decode through the same packet path the device gets (diffed against the previous dumped frame).
        var device = decoded[screen] ?? [UInt16](repeating: 0, count: MK3Screen.pixelCount)
        if let packet = MK3ScreenEncoder.packet(screen: screen, previous: dumpSent[screen], next: frame),
           !MK3ScreenEncoder.apply(packet: packet, to: &device) {
            NSLog("Maschine MK3 dump: packet for screen %d does not decode", screen)
        }
        decoded[screen] = device; dumpSent[screen] = frame
        if device != frame {
            dumpMismatches += 1
            NSLog("Maschine MK3 dump: decoded framebuffer differs from rendered frame on screen %d", screen)
        }
        let side = screen == 0 ? "left" : "right"
        let bitmap = renderer.image(), deviceImage = MK3ScreenDump.image(framebuffer: device)
        var names = [side]
        if let tag, !tag.isEmpty {
            let safe = String(tag.map { $0.isLetter || $0.isNumber || $0 == "-" || $0 == "_" ? $0 : "_" })
            names.append("\(safe)-\(side)")
        }
        if ProcessInfo.processInfo.environment["SUPERTRACKER_MK3_SCREEN_DUMP_ALL"] != nil {
            dumpCount += 1
            names.append(String(format: "frame-%05d-%@", dumpCount, side))
        }
        for name in names {
            MK3ScreenDump.writePNG(bitmap, to: folder.appendingPathComponent(name + ".png"))
            MK3ScreenDump.writePNG(deviceImage, to: folder.appendingPathComponent(name + "-device.png"))
        }
    }
}

// MARK: - Driver

@MainActor
final class MaschineMK3Driver {
    static let shared = MaschineMK3Driver()

    /// Contract `mk3.input` payloads.
    var onInput: (([String: Any]) -> Void)?
    /// Contract `mk3.status` payloads.
    var onStatus: (([String: Any]) -> Void)?
    var onLog: ((String) -> Void)?
    var onRawReport: ((UInt32, [UInt8]) -> Void)? {
        didSet { let handler = onRawReport; hid.queue.async { [hid] in hid.onRawReport = handler } }
    }

    private(set) var running = false
    private(set) var connected = false
    private(set) var screens = false
    private(set) var exclusive = false
    private(set) var inputSeen = false
    private(set) var lastError: String?
    private var leds = MK3LEDState()
    private var stoppedAgents: [MK3AgentRecord] = []
    private var connectionTask: Task<Void, Never>?
    private var connectionRevision: UInt64 = 0
    private var shuttingDown = false
    private var signalSources: [DispatchSourceSignal] = []
    private var recovered = false
    private let hid = MK3HIDSession()
    private let screen = MK3ScreenSession()
    private let control = DispatchQueue(label: "supertracker.mk3.control")

    private init() {
        hid.onEvents = { [weak self] events, cancelled in
            let payloads = events.map { event -> [String: Any] in
                var payload = event.payload
                if cancelled { payload["cancelled"] = true }
                return payload
            }
            DispatchQueue.main.async { MainActor.assumeIsolated { self?.deliver(payloads) } }
        }
        hid.onConnection = { [weak self] connected, error in
            DispatchQueue.main.async { MainActor.assumeIsolated { self?.connectionChanged(connected, error: error) } }
        }
        screen.onFailure = { [weak self] message in
            DispatchQueue.main.async { MainActor.assumeIsolated { self?.screenFailed(message) } }
        }
    }

    var status: [String: Any] {
        var value: [String: Any] = ["connected": connected, "screens": screens, "exclusive": exclusive, "inputSeen": inputSeen, "running": running]
        if let lastError { value["error"] = lastError }
        return value
    }

    private func log(_ message: String) {
        if let onLog { onLog(message) } else { NSLog("Maschine MK3: %@", message) }
    }

    private func publishStatus() { onStatus?(status) }

    private func deliver(_ payloads: [[String: Any]]) {
        guard !shuttingDown else { return }
        if !inputSeen { inputSeen = true; publishStatus() }
        for payload in payloads { onInput?(payload) }
    }

    private func connectionChanged(_ isConnected: Bool, error: String?) {
        guard !shuttingDown else { return }
        connected = isConnected
        if isConnected {
            // HID reconnect is independent of the screen interface: it cannot
            // clear a failed takeover or failed agent-restoration diagnosis.
            hid.send([leds.buttonReport(), leds.padReport()], force: true)
        } else {
            inputSeen = false
            if exclusive { Task { await serializeConnection { await self.leaveExclusive(reason: "The Maschine MK3 was disconnected.") } } }
        }
        publishStatus()
    }

    private func screenFailed(_ message: String) {
        guard exclusive else { return }
        Task { await serializeConnection { await self.leaveExclusive(reason: "Maschine MK3 screens stopped: \(message)") } }
    }

    private func onControl<T>(_ work: @escaping () -> T) async -> T {
        await withCheckedContinuation { continuation in control.async { continuation.resume(returning: work()) } }
    }

    private func onScreen<T>(_ work: @escaping () -> T) async -> T {
        await withCheckedContinuation { continuation in screen.queue.async { continuation.resume(returning: work()) } }
    }

    /// Restores agents a crashed earlier run left stopped. Runs once per process.
    func recoverAgentsIfNeeded() async {
        guard !recovered else { return }
        recovered = true
        let logger: (String) -> Void = { [weak self] message in DispatchQueue.main.async { MainActor.assumeIsolated { self?.log(message) } } }
        await onControl { MK3AgentController.recoverIfNeeded(log: logger) }
    }

    // Main-actor methods re-enter at every await. Queue whole ownership changes so Release cannot
    // finish in the middle of an older takeover and then have that takeover reopen the screens.
    private func serializeConnection(_ operation: @escaping @MainActor () async -> Void) async {
        guard !shuttingDown else { return }
        let previous = connectionTask
        connectionRevision &+= 1
        let revision = connectionRevision
        let task = Task { @MainActor in
            await previous?.value
            guard !self.shuttingDown, !Task.isCancelled else { return }
            await operation()
        }
        connectionTask = task
        await task.value
        if connectionRevision == revision { connectionTask = nil }
    }

    /// `mk3.connect`. Shared mode never touches NI's agents; exclusive mode takes over the screens.
    @discardableResult
    func connect(exclusive wantExclusive: Bool) async -> [String: Any] {
        await serializeConnection { await self.performConnect(exclusive: wantExclusive) }
        return status
    }

    private func performConnect(exclusive wantExclusive: Bool) async {
        await recoverAgentsIfNeeded()
        guard !shuttingDown else { return }
        if !running {
            running = true
            let hid = self.hid
            let error = await withCheckedContinuation { continuation in hid.queue.async { continuation.resume(returning: hid.start()) } }
            if let error { lastError = error; log(error) }
        }
        guard !shuttingDown else { return }
        if wantExclusive && !exclusive { await enterExclusive() }
        else if !wantExclusive && (exclusive || !stoppedAgents.isEmpty) { await leaveExclusive(reason: nil) }
        publishStatus()
    }

    /// `mk3.disconnect`: releases the device entirely and restores NI's agents if they were stopped.
    @discardableResult
    func disconnect() async -> [String: Any] {
        await serializeConnection { await self.performDisconnect() }
        return status
    }

    private func performDisconnect() async {
        if exclusive || !stoppedAgents.isEmpty { await leaveExclusive(reason: nil) }
        if running {
            let hid = self.hid, off = MK3LEDState()
            await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
                hid.queue.async { hid.sendNow([off.buttonReport(), off.padReport()]); hid.stop(); continuation.resume() }
            }
            running = false
        }
        connected = false; inputSeen = false
        publishStatus()
    }

    private func enterExclusive() async {
        lastError = nil
        let screen = self.screen
        let present = await onScreen { screen.isDevicePresent() }
        guard !shuttingDown else { return }
        guard present else {
            lastError = "Maschine MK3 screens were not found. Connect and power on the MK3, then retry Take over screens. Native Instruments agents were not paused."
            log(lastError!)
            return
        }
        let logger: (String) -> Void = { [weak self] message in DispatchQueue.main.async { MainActor.assumeIsolated { self?.log(message) } } }
        let stopped: Result<[MK3AgentRecord], Error> = await onControl { Result { try MK3AgentController.stop(log: logger) } }
        guard !shuttingDown else { return }
        switch stopped {
        case .failure(let error):
            lastError = error.localizedDescription
            let records = MK3AgentController.readState() ?? []
            let restored = await onControl { MK3AgentController.restore(records, log: logger) }
            if !restored, !shuttingDown { stoppedAgents = records }
            return
        case .success(let records):
            stoppedAgents = records
        }
        exclusive = true
        installSignalHandlers()
        let opened: Error? = await onScreen {
            do {
                try screen.open()
                try screen.initializeDisplays()
                return nil
            } catch { return error }
        }
        guard !shuttingDown else { return }
        if let opened {
            log(opened.localizedDescription)
            await leaveExclusive(reason: opened.localizedDescription)
            return
        }
        screens = true
        // Agents may have rewritten LEDs while shutting down.
        hid.send([leds.buttonReport(), leds.padReport()], force: true)
        log("exclusive mode: screens open")
    }

    private func leaveExclusive(reason: String?) async {
        guard exclusive || !stoppedAgents.isEmpty else { return }
        let screen = self.screen
        await onScreen { screen.close(blank: true) }
        guard !shuttingDown else { return }
        screens = false
        exclusive = false
        let records = stoppedAgents.isEmpty ? (MK3AgentController.readState() ?? []) : stoppedAgents
        stoppedAgents = []
        let logger: (String) -> Void = { [weak self] message in DispatchQueue.main.async { MainActor.assumeIsolated { self?.log(message) } } }
        let restored = await onControl { MK3AgentController.restore(records, log: logger) }
        if !restored, !shuttingDown { stoppedAgents = records }
        removeSignalHandlers()
        if let reason { lastError = reason }
        publishStatus()
    }

    /// App termination: synchronous release of the device and restoration of NI's agents.
    func shutdownSynchronously() {
        guard !shuttingDown else { return }
        shuttingDown = true
        let hadOperation = connectionTask != nil
        connectionTask?.cancel()
        let screen = self.screen, hid = self.hid
        if screens || exclusive { screen.queue.sync { screen.close(blank: true) } }
        if running {
            let off = MK3LEDState()
            hid.queue.sync { hid.sendNow([off.buttonReport(), off.padReport()]); hid.stop() }
        }
        let ownedRecords = stoppedAgents
        if hadOperation || exclusive || !ownedRecords.isEmpty {
            // A takeover may still be writing its ownership record on the control queue. Wait for it,
            // then read that durable record, even if the main-actor continuation has not run yet.
            control.sync {
                let records = ownedRecords.isEmpty ? (MK3AgentController.readState() ?? []) : ownedRecords
                if !records.isEmpty { _ = MK3AgentController.restore(records, log: { NSLog("Maschine MK3: %@", $0) }) }
            }
        }
        stoppedAgents = []; exclusive = false; screens = false; running = false; connected = false
        removeSignalHandlers()
    }

    /// While NI's agents are stopped, SIGINT/SIGTERM restore them before the process exits.
    private func installSignalHandlers() {
        guard signalSources.isEmpty else { return }
        for number in [SIGINT, SIGTERM, SIGHUP] {
            signal(number, SIG_IGN)
            let source = DispatchSource.makeSignalSource(signal: number, queue: .main)
            source.setEventHandler { [weak self] in
                MainActor.assumeIsolated { self?.shutdownSynchronously() }
                exit(128 + number)
            }
            source.resume()
            signalSources.append(source)
        }
    }

    private func removeSignalHandlers() {
        for source in signalSources { source.cancel() }
        if !signalSources.isEmpty { for number in [SIGINT, SIGTERM, SIGHUP] { signal(number, SIG_DFL) } }
        signalSources = []
    }

    /// `mk3.leds` (partial update).
    func setLEDs(_ payload: [String: Any]) {
        leds.apply(payload)
        if running { hid.send([leds.buttonReport(), leds.padReport()]) }
    }

    /// `mk3.screen`: display lists per screen; ignored unless the screens are taken over.
    @discardableResult
    func setScreens(_ payload: [String: Any]) -> Bool {
        // With a debug dump directory the frames are rendered (and dumped) even without the screens.
        guard screens || screen.dumpDirectory != nil else { return false }
        let tag = payload["dumpTag"] as? String
        if let left = payload["left"] as? [Any] { screen.submit(screen: 0, ops: MK3DrawOp.parse(left), dumpTag: tag) }
        if let right = payload["right"] as? [Any] { screen.submit(screen: 1, ops: MK3DrawOp.parse(right), dumpTag: tag) }
        return screens
    }

    var diagnostics: [String: Any] {
        let hid = self.hid, screen = self.screen
        let counts = hid.queue.sync { hid.reportCounts }
        let first = hid.queue.sync { hid.firstReports }
        let frames = screen.queue.sync { (screen.framesSent, screen.bytesSent, screen.dumpMismatches) }
        let getReport = hid.queue.sync { hid.initialReportResult }
        let ledReports = hid.queue.sync { (hid.ledReportsSent, hid.ledReportFailures) }
        return ["reports": Dictionary(uniqueKeysWithValues: counts.map { (String(format: "0x%02x", $0.key), $0.value) }),
                "firstReports": Dictionary(uniqueKeysWithValues: first.map { (String(format: "0x%02x", $0.key), $0.value.map { String(format: "%02x", $0) }.joined(separator: " ")) }),
                "ledReportsSent": ledReports.0, "ledReportFailures": ledReports.1, "framesSent": frames.0, "screenBytes": frames.1, "dumpMismatches": frames.2,
                "getInputReport": getReport.map { String(format: "0x%08x", $0) } ?? "not attempted"]
    }

    /// `mk3.*` bridge commands.
    func handle(_ command: String, payload: [String: Any]) async throws -> Any {
        switch command {
        case "mk3.connect": return await connect(exclusive: payload["exclusive"] as? Bool ?? false)
        case "mk3.disconnect": return await disconnect()
        case "mk3.status":
            if payload["diagnostics"] as? Bool == true { var value = status; value["diagnostics"] = diagnostics; return value }
            return status
        case "mk3.leds": setLEDs(payload); return ["ok": true]
        case "mk3.screen": return ["screens": setScreens(payload)]
        default: throw TrackerError.message("Unknown Maschine MK3 command \(command).")
        }
    }
}

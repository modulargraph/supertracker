// Virtual CoreMIDI only: creates its own source/destination, never selects hardware.
// swiftc -swift-version 5 -module-cache-path .build/clang-module-cache \
//   Sources/SuperTracker/MIDIHost.swift tests/MIDILoopbackTests.swift \
//   -o .build/test-bin/midi-loopback
// .build/test-bin/midi-loopback test-output/midi-loopback-report.json
// Needs CoreMIDI service access. Exit 77 means skipped, not passed.
import Foundation
import CoreMIDI
import Darwin

private struct TestFailure: Error, CustomStringConvertible {
    let description: String
}

private final class CapturedMessages: @unchecked Sendable {
    private let lock = NSLock()
    private var input: [[Int]] = []
    private var output: [UInt8] = []
    let received = DispatchSemaphore(value: 0)
    let sent = DispatchSemaphore(value: 0)

    func appendInput(_ message: [Int]) {
        lock.lock(); input.append(message); lock.unlock(); received.signal()
    }
    func appendOutput(_ bytes: [UInt8]) {
        lock.lock(); output.append(contentsOf: bytes); lock.unlock(); sent.signal()
    }
    func inputSnapshot() -> [[Int]] { lock.lock(); defer { lock.unlock() }; return input }
    func outputSnapshot() -> [UInt8] { lock.lock(); defer { lock.unlock() }; return output }
}

@main struct MIDILoopbackTests {
    static func check(_ value: @autoclosure () -> Bool, _ message: String) throws {
        if !value() { throw TestFailure(description: message) }
    }
    static func checkStatus(_ value: OSStatus, _ operation: String) throws {
        try check(value == noErr, "\(operation): CoreMIDI status \(value)")
    }
    static func publish(_ bytes: [UInt8], source: MIDIEndpointRef) throws {
        try check(!bytes.isEmpty && bytes.count <= 256, "Test packet exceeds its stack buffer")
        var list = MIDIPacketList()
        let result = withUnsafeMutablePointer(to: &list) { pointer -> OSStatus in
            let first = MIDIPacketListInit(pointer)
            _ = bytes.withUnsafeBufferPointer {
                MIDIPacketListAdd(pointer, MemoryLayout<MIDIPacketList>.size, first, 0, bytes.count, $0.baseAddress!)
            }
            return MIDIReceived(source, pointer)
        }
        try checkStatus(result, "Publishing test-source bytes")
    }
    static func wait(_ signal: DispatchSemaphore, count: Int) throws {
        for _ in 0..<count { try check(signal.wait(timeout: .now() + 2) == .success, "Virtual MIDI receive timeout") }
    }
    static func writeReport(_ report: [String: Any]) throws {
        guard CommandLine.arguments.count > 1 else { return }
        let url = URL(fileURLWithPath: CommandLine.arguments[1])
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        let data = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
        try data.write(to: url, options: .atomic)
    }

    static func main() {
        var report: [String: Any] = ["generatedAt": ISO8601DateFormatter().string(from: Date()),
            "virtualOnly": true, "hardwareTested": false]
        var client: MIDIClientRef = 0
        let status = MIDIClientCreateWithBlock("SuperTracker virtual MIDI regression" as CFString, &client, nil)
        guard status == noErr else {
            report["status"] = "skipped"; report["passed"] = false; report["coreMIDIStatus"] = status
            report["reason"] = "CoreMIDI service unavailable in this execution environment"
            try? writeReport(report)
            print("SKIP MIDI loopback: CoreMIDI service unavailable (\(status)); no loopback evidence recorded.")
            exit(77)
        }
        defer { MIDIClientDispose(client) }
        let capture = CapturedMessages()
        do {
            let suffix = UUID().uuidString
            var source: MIDIEndpointRef = 0, destination: MIDIEndpointRef = 0
            try checkStatus(MIDIDestinationCreateWithBlock(client, "SuperTracker Test Destination \(suffix)" as CFString, &destination) { list, _ in
                var packet = UnsafeRawPointer(list).advanced(by: MemoryLayout<MIDIPacketList>.offset(of: \.packet)!).assumingMemoryBound(to: MIDIPacket.self)
                var bytes: [UInt8] = []
                for _ in 0..<list.pointee.numPackets {
                    let data = UnsafeRawPointer(packet).advanced(by: MemoryLayout<MIDIPacket>.offset(of: \.data)!).assumingMemoryBound(to: UInt8.self)
                    bytes.append(contentsOf: UnsafeBufferPointer(start: data, count: Int(packet.pointee.length)))
                    packet = UnsafePointer(MIDIPacketNext(packet))
                }
                capture.appendOutput(bytes)
            }, "Creating virtual destination")
            try checkStatus(MIDIObjectSetIntegerProperty(destination,kMIDIPropertyPrivate,1),"Making virtual destination private")
            try checkStatus(MIDISourceCreate(client, "SuperTracker Test Source \(suffix)" as CFString, &source), "Creating virtual source")
            try checkStatus(MIDIObjectSetIntegerProperty(source,kMIDIPropertyPrivate,1),"Making virtual source private")
            var sourceID: Int32 = 0, destinationID: Int32 = 0
            try checkStatus(MIDIObjectGetIntegerProperty(source, kMIDIPropertyUniqueID, &sourceID), "Reading source ID")
            try checkStatus(MIDIObjectGetIntegerProperty(destination, kMIDIPropertyUniqueID, &destinationID), "Reading destination ID")

            let privateSource = source,privateDestination = destination
            let host = try MIDIHost(inputIDs:[sourceID],inputSourceProvider:{[privateSource]},outputDestinationProvider:{[privateDestination]})
            defer { host.shutdown() }
            let devices = try host.configure(payload: ["inputIDs": [sourceID], "outputID": destinationID])
            try check((devices["sources"] as? [[String: Any]])?.contains { ($0["id"] as? Int) == Int(sourceID) } == true, "Virtual source was not discovered")
            host.onPacket = { status, first, second, source in capture.appendInput([Int(status), Int(first), Int(second), Int(source)]) }

            // CoreMIDI packets contain complete messages; retain running status and realtime bytes.
            report["stage"] = "virtual input"
            let input: [UInt8] = [0x90,60,100,61,0,0xF8,0x90,62,90,0xB0,64,127,0xE0,0,64,0xFC]
            let expected = [[0x90,60,100], [0x90,61,0], [0xF8,0,0], [0x90,62,90], [0xB0,64,127], [0xE0,0,64], [0xFC,0,0]]
            try publish(input, source: source); try wait(capture.received, count: expected.count)
            let received = capture.inputSnapshot()
            try check(received.map { Array($0.prefix(3)) } == expected, "Virtual input parser changed message bytes/order")
            try check(received.allSatisfy { $0[3] == Int(sourceID) }, "Virtual input lost its source identity")

            report["stage"] = "channel filter"
            _ = try host.configure(payload: ["inputChannel": 3])
            try publish([0x90,65,80,0x92,66,90,0xF8], source: source)
            try wait(capture.received, count: 2)
            let filtered = Array(capture.inputSnapshot().dropFirst(expected.count)).map { Array($0.prefix(3)) }
            try check(filtered == [[0x92,66,90],[0xF8,0,0]], "Input channel filter or realtime handling failed")

            report["stage"] = "virtual output"
            guard let route = host.prepareDestination(destinationID) else { throw TestFailure(description: "Virtual output route unavailable") }
            try check(route.uniqueID == destinationID && route.endpoint == destination, "Output route did not select the test destination")
            let messages: [[UInt8]] = [[0x92,60,110], [0x82,60,0], [0xB2,64,127], [0xE2,0,64],
                                      [0xB2,123,0], [0xF8], [0xFA], [0xC9,64]]
            for message in messages {
                try check(route.send(status: message[0], data1: message.count > 1 ? message[1] : 0,
                                     data2: message.count > 2 ? message[2] : 0), "Sending virtual output failed")
            }
            try check(host.send(status: 0xF2, data1: 2, data2: 3), "Control-thread virtual send failed")
            let expectedBytes: [UInt8] = Array(messages.joined()) + [0xF2,2,3]
            let deadline = Date().addingTimeInterval(2)
            while capture.outputSnapshot().count < expectedBytes.count && Date() < deadline {
                _ = capture.sent.wait(timeout: .now() + 0.1)
            }
            try check(capture.outputSnapshot() == expectedBytes, "Output status lengths, channels or all-notes-off bytes changed")
            do { _ = try host.configure(payload: ["channel": 17]); throw TestFailure(description: "Invalid channel accepted") }
            catch MIDIHost.HostError.invalid { }
            _ = try host.configure(payload:["inputChannel":0])
            let releaseMessages:[UInt8] = [0x80,60,0,0x80,62,0,0x82,66,0,0xB0,64,0,0xB0,123,0,0xB2,123,0]
            try publish(releaseMessages,source:source); try wait(capture.received,count:6)
            _ = try host.configure(payload: ["enabled": false])
            try check(host.prepareDestination(destinationID) != nil, "Disabling MIDI input also disabled hardware output")
            try check(host.send(status:0x92,data1:64,data2:100) && host.send(status:0x82,data1:64,data2:0), "Input-disabled external note/release failed")
            let disabledDeadline=Date().addingTimeInterval(2)
            while capture.outputSnapshot().count < expectedBytes.count+6 && Date() < disabledDeadline { _ = capture.sent.wait(timeout:.now()+0.1) }
            try check(Array(capture.outputSnapshot().suffix(6)) == [0x92,64,100,0x82,64,0], "Input-disabled output bytes were lost")

            report["status"] = "passed"; report["passed"] = true
            report["privateEndpoints"] = true
            report["inputMessages"] = expected.count + filtered.count + 6; report["releaseMessages"] = 6
            report["outputMessages"] = messages.count + 3
            report.removeValue(forKey: "stage")
            report["checks"] = ["virtual source/destination selection by unique ID", "running status and realtime input",
                "note/sustain/pitch input bytes and source identity", "channel filtering", "output channel/status lengths",
                "note-off and CC123 all-notes-off output bytes", "clock/transport/program output bytes", "invalid channel and output independence from input disable"]
            try writeReport(report)
            print("PASS CoreMIDI virtual loopback: IDs, parser, notes/CC/pitch, channels, all-notes-off bytes, clock/transport and disable. No hardware tested.")
        } catch {
            report["status"] = "failed"; report["passed"] = false; report["error"] = String(describing: error)
            report["receivedMessages"] = capture.inputSnapshot().map { Array($0.prefix(3)) }
            report["outputBytes"] = capture.outputSnapshot().map(Int.init)
            try? writeReport(report)
            print("FAIL MIDI loopback: \(error)")
            exit(1)
        }
    }
}

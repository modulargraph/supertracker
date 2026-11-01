import Foundation
import CoreMIDI

/// CoreMIDI MIDI 1.0 input/output. Callbacks arrive on CoreMIDI's receive thread.
/// Device IDs are persistent CoreMIDI unique IDs, not an array index.
final class MIDIHost {
    var onMessage: ((UInt8, UInt8, UInt8) -> Void)?
    var onPacket: ((UInt8, UInt8, UInt8, Int32) -> Void)?
    var onEvent: (([String: Any]) -> Void)?

    private var client: MIDIClientRef = 0
    private var inputPort: MIDIPortRef = 0
    private var outputPort: MIDIPortRef = 0
    private let control = DispatchQueue(label: "SuperTracker.MIDI.control")
    private let controlKey = DispatchSpecificKey<UInt8>()
    private let lock = NSLock()
    private var enabled = true
    private var stopped = false
    private var connectedSources: [MIDIEndpointRef] = []
    private var connectedSourceIDs: [MIDIEndpointRef: Int32] = [:]
    private var requestedInputs: [String]? // nil connects all sources, [] connects none
    private var requestedOutput: String?
    private var outputEndpoint: MIDIEndpointRef = 0
    private var destinationEndpoints: [MIDIEndpointRef] = []
    private var outputChannel = 1
    private var inputChannel = 0 // 0 receives all channels
    private var parsers: [Int32: MIDIByteParser] = [:]
    // Delivered keys (channel << 7 | note) and sustain pedals per source. An
    // unplugged source can never send their releases, so hotplug synthesizes them.
    private var heldKeys: [Int32: Set<Int>] = [:]
    private var heldPedals: [Int32: Set<Int>] = [:]
    private var refreshScheduled = false
    typealias InputConnector = (MIDIPortRef,MIDIEndpointRef,UnsafeMutableRawPointer?)->OSStatus
    private let connectInput:InputConnector
    private let inputSourceProvider:(()->[MIDIEndpointRef])?
    private let outputDestinationProvider:(()->[MIDIEndpointRef])?

    enum HostError: LocalizedError {
        case system(String, OSStatus)
        case invalid(String)
        var errorDescription: String? {
            switch self {
            case .system(let operation, let status): return "\(operation) failed (CoreMIDI \(status))."
            case .invalid(let message): return message
            }
        }
    }

    /// Immutable route copied into the audio render snapshot. Sending performs no
    /// allocation, locking, endpoint lookup, queue dispatch, or Swift callback.
    /// The owning MIDIHost must outlive the route. Stop rendering before shutting
    /// the host down, and refresh routes after configuration/device-change events.
    struct OutputRoute: Sendable {
        let port: MIDIPortRef
        let endpoint: MIDIEndpointRef
        let uniqueID: Int32

        init(port:MIDIPortRef,endpoint:MIDIEndpointRef,uniqueID:Int32) {
            self.port = port; self.endpoint = endpoint; self.uniqueID = uniqueID
        }

        /// `timestamp` is a Core Audio host-time tick value; zero sends immediately.
        /// Status includes its destination channel, e.g. 0x90 | (channel - 1).
        @inline(__always)
        @discardableResult
        func send(status: UInt8, data1: UInt8 = 0, data2: UInt8 = 0, timestamp: MIDITimeStamp = 0) -> Bool {
            sendStatus(status: status, data1: data1, data2: data2, timestamp: timestamp) == noErr
        }

        @inline(__always)
        fileprivate func sendStatus(status: UInt8, data1: UInt8, data2: UInt8, timestamp: MIDITimeStamp = 0) -> OSStatus {
            guard status >= 0x80, status != 0xF0, status != 0xF7 else { return -50 }
            // A single short packet fits entirely on the stack. Direct tuple
            // assignment avoids constructing a Swift Array or copying a buffer.
            var list = MIDIPacketList()
            list.numPackets = 1
            list.packet.timeStamp = timestamp
            list.packet.length = UInt16(MIDIByteParser.dataLength(status) + 1)
            list.packet.data.0 = status
            list.packet.data.1 = data1 & 0x7F
            list.packet.data.2 = data2 & 0x7F
            return MIDISend(port, endpoint, &list)
        }
    }

    /// An initial ID selection avoids subscribing to unrelated sources before a
    /// saved configuration is applied. The connector seam permits deterministic
    /// hot-unplug regression tests without opening physical MIDI devices.
    init(inputIDs:[Int32]? = nil,inputSourceProvider:(()->[MIDIEndpointRef])? = nil,
         outputDestinationProvider:(()->[MIDIEndpointRef])? = nil,inputConnector:InputConnector? = nil) throws {
        requestedInputs = inputIDs?.map(String.init)
        self.inputSourceProvider = inputSourceProvider
        self.outputDestinationProvider = outputDestinationProvider
        connectInput = inputConnector ?? { MIDIPortConnectSource($0,$1,$2) }
        control.setSpecific(key: controlKey, value: 1)
        try Self.check(MIDIClientCreateWithBlock("SuperTracker" as CFString, &client) { [weak self] notification in
            switch notification.pointee.messageID {
            case .msgSetupChanged, .msgObjectAdded, .msgObjectRemoved, .msgPropertyChanged:
                self?.scheduleRefresh()
            default: break
            }
        }, "Creating MIDI client")
        do {
            try Self.check(MIDIInputPortCreateWithBlock(client, "SuperTracker Input" as CFString, &inputPort) { [weak self] packets, sourceContext in
                let endpoint = MIDIEndpointRef(UInt(bitPattern: sourceContext))
                self?.receive(packets, source: endpoint)
            }, "Creating MIDI input")
            try Self.check(MIDIOutputPortCreate(client, "SuperTracker Output" as CFString, &outputPort), "Creating MIDI output")
            _ = try withControl { try reconnect() }
        } catch {
            shutdown()
            throw error
        }
    }

    deinit { shutdown() }

    func devices() -> [String: Any] {
        withControl { deviceSnapshot() }
    }

    /// Input may be "all", "none", an endpoint unique ID, or an endpoint name.
    /// `inputIDs` selects several inputs. Missing fields retain their current value.
    /// `channel` stores a convenient default output channel; send() preserves the supplied status.
    @discardableResult
    func configure(payload: [String: Any]) throws -> [String: Any] {
        var releases: [(Int32, [[UInt8]])] = []
        let result: [String: Any] = try withControl {
            guard !stopped else { throw HostError.invalid("The MIDI host is shut down.") }
            var inputs = requestedInputs
            var output = requestedOutput
            var newEnabled = enabled
            var outChannel = outputChannel
            var inChannel = inputChannel
            if let value = payload["enabled"] as? Bool { newEnabled = value }
            if let raw = payload["inputIDs"] as? [Any] {
                inputs = raw.map { String(describing: $0) }
            } else if let raw = payload["inputs"] as? [Any] {
                inputs = raw.map { String(describing: $0) }
            } else if let raw = payload["inputID"] ?? payload["input"] {
                let value = String(describing: raw)
                inputs = value.lowercased() == "all" ? nil : (Self.isNone(value) ? [] : [value])
            }
            if let raw = payload["outputID"] ?? payload["output"] {
                let value = String(describing: raw)
                output = Self.isNone(value) ? nil : value
            }
            if let value = PayloadValue.integer(payload["channel"] ?? payload["outputChannel"]) {
                guard (1...16).contains(value) else { throw HostError.invalid("MIDI output channel must be between 1 and 16.") }
                outChannel = value
            }
            if let value = PayloadValue.integer(payload["inputChannel"]) {
                guard (0...16).contains(value) else { throw HostError.invalid("MIDI input channel must be 0 (all), or between 1 and 16.") }
                inChannel = value
            }
            // Resolve every explicit selection before replacing a working configuration.
            let sources = currentSources()
            if inputs != requestedInputs, let inputs {
                for identifier in inputs where Self.resolve(identifier, in: sources) == nil {
                    throw HostError.invalid("MIDI input '\(identifier)' is not connected.")
                }
            }
            if output != requestedOutput, let output, Self.resolve(output, in: currentDestinations()) == nil {
                throw HostError.invalid("MIDI output '\(output)' is not connected.")
            }
            // The client releases every input note when the effective input route changes; forget them then so
            // a later unplug cannot repeat them. An unchanged route keeps them for hotplug note-off synthesis.
            let inputRouteChanged = inputs.map(Set.init) != requestedInputs.map(Set.init) || newEnabled != enabled || inChannel != inputChannel
            requestedInputs = inputs; requestedOutput = output; outputChannel = outChannel
            lock.lock()
            enabled = newEnabled; inputChannel = inChannel
            parsers.removeAll()
            if inputRouteChanged { heldKeys.removeAll(); heldPedals.removeAll() }
            lock.unlock()
            // A source that vanished before its hotplug refresh still owes releases.
            releases = try reconnect().map { ($0, takeReleases($0)) }
            return deviceSnapshot()
        }
        for (source, messages) in releases where !messages.isEmpty { deliver(messages, source: source) }
        onEvent?(["type": "midiConfiguration", "devices": result])
        return result
    }

    /// Resolve an instrument destination on the control thread, before publishing
    /// its render snapshot. A nil destination selects the globally configured output.
    /// Input enablement is independent of output; returns nil when disconnected or shut down.
    func prepareDestination(_ destination: Int32? = nil) -> OutputRoute? {
        withControl { makeOutputRoute(destination) }
    }

    private func makeOutputRoute(_ destination: Int32?) -> OutputRoute? {
        guard !stopped else { return nil }
        let endpoint: MIDIEndpointRef
        if let destination {
            guard let resolved = Self.resolve(String(destination), in: destinationEndpoints) else { return nil }
            endpoint = resolved
        } else {
            endpoint = outputEndpoint
        }
        guard endpoint != 0 else { return nil }
        return OutputRoute(port: outputPort, endpoint: endpoint, uniqueID: Self.uniqueID(endpoint))
    }

    /// Control-thread convenience API. Render callbacks must use a prepared OutputRoute.
    /// Sends to a specific instrument destination, or to the globally configured destination.
    /// Supply the channel in the status byte: 0x90 | (channel - 1). A timestamp of zero
    /// requests immediate dispatch. System realtime/transport messages use one byte.
    @discardableResult
    func send(status: UInt8, data1: UInt8 = 0, data2: UInt8 = 0, destination: Int32? = nil) -> Bool {
        guard status >= 0x80, status != 0xF0, status != 0xF7 else { return false }
        var error: OSStatus = noErr
        let success = withControl { () -> Bool in
            guard let route = makeOutputRoute(destination) else { return false }
            error = route.sendStatus(status: status, data1: data1, data2: data2)
            return error == noErr
        }
        if error != noErr { onEvent?(["type": "midiError", "message": "MIDI output failed (\(error))."]) }
        return success
    }

    func shutdown() {
        withControl {
            guard !stopped else { return }
            lock.lock(); stopped = true; enabled = false; parsers.removeAll(); heldKeys.removeAll(); heldPedals.removeAll(); lock.unlock()
            for endpoint in connectedSources { MIDIPortDisconnectSource(inputPort, endpoint) }
            connectedSources.removeAll(); connectedSourceIDs.removeAll()
            if inputPort != 0 { MIDIPortDispose(inputPort); inputPort = 0 }
            if outputPort != 0 { MIDIPortDispose(outputPort); outputPort = 0 }
            if client != 0 { MIDIClientDispose(client); client = 0 }
        }
    }

    private func receive(_ list: UnsafePointer<MIDIPacketList>, source: MIDIEndpointRef) {
        let sourceID = Self.uniqueID(source)
        let offset = MemoryLayout<MIDIPacketList>.offset(of: \.packet)!
        var packet = UnsafeRawPointer(list).advanced(by: offset).assumingMemoryBound(to: MIDIPacket.self)
        var messages: [[UInt8]] = []
        lock.lock()
        guard enabled, !stopped else { lock.unlock(); return }
        var parser = parsers[sourceID] ?? MIDIByteParser()
        let channel = inputChannel
        for _ in 0..<list.pointee.numPackets {
            let bytes = UnsafeRawPointer(packet).advanced(by: MemoryLayout<MIDIPacket>.offset(of: \.data)!).assumingMemoryBound(to: UInt8.self)
            for index in 0..<Int(packet.pointee.length) {
                if let message = parser.consume(bytes[index]) {
                    if message[0] >= 0xF0 || channel == 0 || Int(message[0] & 0x0F) + 1 == channel {
                        messages.append(message)
                        track(message, source: sourceID)
                    }
                }
            }
            packet = UnsafePointer(MIDIPacketNext(packet))
        }
        parsers[sourceID] = parser
        lock.unlock()
        deliver(messages, source: sourceID)
    }

    // Per-message delivery goes through onMessage/onPacket only. A configuration
    // event per incoming byte stream (clock, aftertouch) made the client refresh
    // every output route and notify the UI dozens of times per second.
    private func deliver(_ messages: [[UInt8]], source: Int32) {
        for message in messages {
            let status = message[0], first = message.count > 1 ? message[1] : 0, second = message.count > 2 ? message[2] : 0
            onMessage?(status, first, second)
            onPacket?(status, first, second, source)
        }
    }

    /// Caller holds `lock`.
    private func track(_ message: [UInt8], source: Int32) {
        guard message.count == 3, message[0] < 0xF0 else { return }
        let kind = message[0] & 0xF0, channel = Int(message[0] & 0x0F), data1 = Int(message[1])
        if kind == 0x90 && message[2] > 0 { heldKeys[source, default: []].insert(channel << 7 | data1) }
        else if kind == 0x80 || kind == 0x90 { heldKeys[source]?.remove(channel << 7 | data1) }
        else if kind == 0xB0 && data1 == 64 {
            if message[2] >= 64 { heldPedals[source, default: []].insert(channel) } else { heldPedals[source]?.remove(channel) }
        } else if kind == 0xB0 && (data1 == 120 || data1 == 123) { heldKeys[source]?.subtract((0..<128).map { channel << 7 | $0 }) }
    }

    /// Releases a removed source still owes: key note-offs, then pedal-up, in a stable order.
    private func takeReleases(_ source: Int32) -> [[UInt8]] {
        lock.lock(); defer { lock.unlock() }
        parsers.removeValue(forKey: source)
        let keys = heldKeys.removeValue(forKey: source) ?? [], pedals = heldPedals.removeValue(forKey: source) ?? []
        return keys.sorted().map { [0x80 | UInt8($0 >> 7), UInt8($0 & 0x7F), 0] } + pedals.sorted().map { [0xB0 | UInt8($0), 64, 0] }
    }

    private func scheduleRefresh() {
        control.async { [weak self] in
            guard let self, !self.stopped, !self.refreshScheduled else { return }
            self.refreshScheduled = true
            self.control.asyncAfter(deadline: .now() + 0.1) { [weak self] in
                guard let self, !self.stopped else { return }
                self.refreshScheduled = false
                do {
                    // A source that disappeared while a key or pedal was down never
                    // sends its release; deliver one so nothing keeps sounding.
                    for source in try self.reconnect(retryDisappearedSources:false) {
                        let releases = self.takeReleases(source)
                        if !releases.isEmpty { self.deliver(releases, source: source) }
                    }
                    let result = self.deviceSnapshot()
                    // Avoid calling UI/client code while holding the configuration queue.
                    DispatchQueue.main.async { [weak self] in self?.onEvent?(["type": "midiDevicesChanged", "devices": result]) }
                } catch {
                    let message = error.localizedDescription
                    DispatchQueue.main.async { [weak self] in self?.onEvent?(["type": "midiError", "message": message]) }
                }
            }
        }
    }

    /// Returns the unique IDs of disconnected sources (recorded at connection:
    /// a disposed endpoint no longer reports its identity).
    @discardableResult
    private func reconnect(retryDisappearedSources:Bool = true) throws -> [Int32] {
        let allSources = currentSources()
        let wanted = enabled ? (requestedInputs.map { identifiers in identifiers.compactMap { Self.resolve($0, in: allSources) } } ?? allSources) : []
        for source in connectedSources where !wanted.contains(source) { MIDIPortDisconnectSource(inputPort, source) }
        let removed = connectedSources.filter { !wanted.contains($0) }.compactMap { connectedSourceIDs.removeValue(forKey: $0) }
        connectedSources.removeAll { !wanted.contains($0) }
        for source in wanted where !connectedSources.contains(source) {
            var identity:Int32 = 0
            let identityStatus = MIDIObjectGetIntegerProperty(source,kMIDIPropertyUniqueID,&identity)
            let result = connectInput(inputPort,source,UnsafeMutableRawPointer(bitPattern:UInt(source)))
            let endpointFailure = result == -50 || result == kMIDIUnknownEndpoint || result == kMIDIObjectNotFound
            if endpointFailure, Self.sourceDisappeared(source,identity:identityStatus == noErr ? identity : nil,currentSources:currentSources()) {
                // Enumeration and connection are separate CoreMIDI operations.
                // A source may vanish between them; do not discard its saved ID
                // or abort an otherwise usable audio host on that confirmed race.
                if retryDisappearedSources { scheduleRefresh() }
                continue
            }
            try Self.check(result,"Connecting MIDI input")
            connectedSources.append(source); connectedSourceIDs[source] = Self.uniqueID(source)
        }
        destinationEndpoints = currentDestinations()
        outputEndpoint = requestedOutput.flatMap { Self.resolve($0, in: destinationEndpoints) } ?? 0
        return removed
    }

    private func withControl<T>(_ operation: () throws -> T) rethrows -> T {
        if DispatchQueue.getSpecific(key: controlKey) != nil { return try operation() }
        return try control.sync(execute: operation)
    }

    private func deviceSnapshot() -> [String: Any] {
        let inputs = currentSources().map { endpoint -> [String: Any] in
            var value = Self.describe(endpoint)
            value["connected"] = connectedSources.contains(endpoint)
            return value
        }
        let outputs = currentDestinations().map { endpoint -> [String: Any] in
            var value = Self.describe(endpoint)
            value["selected"] = endpoint == outputEndpoint
            return value
        }
        return ["sources": inputs, "destinations": outputs, "inputs": inputs, "outputs": outputs,
                "enabled": enabled, "inputIDs": connectedSources.map { Int(Self.uniqueID($0)) },
                "allInputs": requestedInputs == nil, "outputID": outputEndpoint == 0 ? NSNull() : NSNumber(value: Self.uniqueID(outputEndpoint)),
                "requestedInputIDs": requestedInputs as Any? ?? NSNull(),
                "requestedOutputID": requestedOutput as Any? ?? NSNull(),
                "channel": outputChannel, "inputChannel": inputChannel]
    }

    private static func sources() -> [MIDIEndpointRef] { (0..<MIDIGetNumberOfSources()).map { MIDIGetSource($0) }.filter { $0 != 0 } }
    private func currentSources()->[MIDIEndpointRef] { inputSourceProvider?() ?? Self.sources() }
    private func currentDestinations()->[MIDIEndpointRef] { outputDestinationProvider?() ?? Self.destinations() }
    private static func destinations() -> [MIDIEndpointRef] { (0..<MIDIGetNumberOfDestinations()).map { MIDIGetDestination($0) }.filter { $0 != 0 } }

    /// A connect error alone is never evidence of an unplug. Query the current
    /// endpoint identity and topology, preserving failures for valid sources.
    private static func sourceDisappeared(_ source:MIDIEndpointRef,identity:Int32?,currentSources:[MIDIEndpointRef]) -> Bool {
        let stillListed = currentSources.contains(source)
        var currentID:Int32 = 0
        let identityStatus = MIDIObjectGetIntegerProperty(source,kMIDIPropertyUniqueID,&currentID)
        if identityStatus == kMIDIObjectNotFound || identityStatus == kMIDIUnknownEndpoint { return true }
        var offline:Int32 = 0
        if MIDIObjectGetIntegerProperty(source,kMIDIPropertyOffline,&offline) == noErr && offline != 0 { return true }
        if let lookupID = identity ?? (identityStatus == noErr ? currentID : nil) {
            var found:MIDIObjectRef = 0
            var kind:MIDIObjectType = .other
            let foundStatus = MIDIObjectFindByUniqueID(lookupID,&found,&kind)
            if foundStatus == kMIDIObjectNotFound { return true }
            if foundStatus == noErr, found != source || kind != .source { return true }
            // A successful identity lookup remains authoritative if enumeration
            // transiently returns no sources (which can also signal an error).
            if !stillListed, foundStatus == noErr { return false }
        }
        return false
    }

    private static func uniqueID(_ endpoint: MIDIEndpointRef) -> Int32 {
        var result: Int32 = 0
        if MIDIObjectGetIntegerProperty(endpoint, kMIDIPropertyUniqueID, &result) != noErr { return Int32(bitPattern: endpoint) }
        return result
    }

    private static func property(_ endpoint: MIDIEndpointRef, _ key: CFString) -> String {
        var string: Unmanaged<CFString>?
        guard MIDIObjectGetStringProperty(endpoint, key, &string) == noErr, let string else { return "" }
        return string.takeRetainedValue() as String
    }

    private static func describe(_ endpoint: MIDIEndpointRef) -> [String: Any] {
        let displayName = property(endpoint, kMIDIPropertyDisplayName)
        let name = property(endpoint, kMIDIPropertyName)
        var offline: Int32 = 0
        MIDIObjectGetIntegerProperty(endpoint, kMIDIPropertyOffline, &offline)
        return ["id": Int(uniqueID(endpoint)), "endpoint": Int(endpoint), "name": displayName.isEmpty ? name : displayName,
                "manufacturer": property(endpoint, kMIDIPropertyManufacturer), "offline": offline != 0]
    }

    private static func resolve(_ identifier: String, in endpoints: [MIDIEndpointRef]) -> MIDIEndpointRef? {
        if let number = Int64(identifier) {
            if let endpoint = endpoints.first(where: { Int64(uniqueID($0)) == number }) { return endpoint }
            return nil // Saved routes are unique IDs, never transient endpoint handles.
        }
        return endpoints.first {
            property($0, kMIDIPropertyDisplayName).caseInsensitiveCompare(identifier) == .orderedSame ||
            property($0, kMIDIPropertyName).caseInsensitiveCompare(identifier) == .orderedSame
        }
    }

    private static func isNone(_ value: String) -> Bool { ["", "none", "off", "null", "<null>"].contains(value.lowercased()) }
    private static func check(_ status: OSStatus, _ operation: String) throws {
        if status != noErr { throw HostError.system(operation, status) }
    }
}

/// Running status persists across packets; realtime bytes may occur within any message.
/// System-exclusive payloads are skipped without accidentally treating their bytes as notes.
private struct MIDIByteParser {
    var runningStatus: UInt8?
    var status: UInt8?
    var data: [UInt8] = []
    var inSysEx = false

    static func dataLength(_ status: UInt8) -> Int {
        if status < 0xF0 { return status & 0xE0 == 0xC0 ? 1 : 2 }
        switch status { case 0xF1, 0xF3: return 1; case 0xF2: return 2; default: return 0 }
    }

    mutating func consume(_ byte: UInt8) -> [UInt8]? {
        if byte >= 0xF8 { return [byte] }
        if byte >= 0x80 {
            data.removeAll(keepingCapacity: true)
            status = nil
            if byte == 0xF0 { inSysEx = true; runningStatus = nil; return nil }
            if byte == 0xF7 { inSysEx = false; runningStatus = nil; return nil }
            inSysEx = false
            runningStatus = byte < 0xF0 ? byte : nil
            if Self.dataLength(byte) == 0 { return [byte] }
            status = byte
            return nil
        }
        guard !inSysEx, let current = status ?? runningStatus else { return nil }
        status = current
        data.append(byte)
        guard data.count == Self.dataLength(current) else { return nil }
        let result = [current] + data
        data.removeAll(keepingCapacity: true)
        status = nil
        return result
    }
}

// Original virtual endpoints only. No hardware ports are opened or messages
// published until private-endpoint discovery has been verified in this process.
import Foundation
import CoreMIDI
import Darwin

private struct MIDIHotplugFailure:Error,CustomStringConvertible { let description:String }

@main struct MIDIHotplugTests {
    @MainActor static func main() async {
        var report:[String:Any] = ["virtualOnly":true,"hardwareTested":false,"checks":[]]
        var client:MIDIClientRef = 0
        let status = MIDIClientCreateWithBlock("SuperTracker private hotplug regression" as CFString,&client,nil)
        guard status == noErr else {
            report["status"] = "skipped"; report["coreMIDIStatus"] = status
            save(report); print("SKIP CoreMIDI service unavailable: \(status)"); exit(77)
        }
        defer { MIDIClientDispose(client) }
        var endpoints:[MIDIEndpointRef] = []
        let sourceList = MIDIHotplugSourceList()
        var hosts:[MIDIHost] = []
        defer {
            for host in hosts { host.shutdown() }
            for endpoint in endpoints { MIDIEndpointDispose(endpoint) }
        }
        func require(_ condition:Bool,_ message:String) throws {
            if !condition { throw MIDIHotplugFailure(description:message) }
        }
        func createSource(_ name:String,id:Int32? = nil) throws -> (MIDIEndpointRef,Int32) {
            var source:MIDIEndpointRef = 0
            try require(MIDISourceCreate(client,name as CFString,&source) == noErr,"Cannot create virtual source.")
            endpoints.append(source)
            try require(MIDIObjectSetIntegerProperty(source,kMIDIPropertyPrivate,1) == noErr,"Cannot make virtual source private.")
            if let id { try require(MIDIObjectSetIntegerProperty(source,kMIDIPropertyUniqueID,id) == noErr,"Cannot restore virtual source identity.") }
            var identity:Int32 = 0
            try require(MIDIObjectGetIntegerProperty(source,kMIDIPropertyUniqueID,&identity) == noErr,"Cannot read virtual source identity.")
            sourceList.add(source)
            return (source,identity)
        }
        func selected(_ host:MIDIHost)->Set<Int> { Set(host.devices()["inputIDs"] as? [Int] ?? []) }
        func waitFor(_ condition:()->Bool) async throws {
            for _ in 0..<200 {
                if condition() { return }
                try await Task.sleep(nanoseconds:10_000_000)
            }
            throw MIDIHotplugFailure(description:"Timed out waiting for virtual endpoint notification.")
        }
        do {
            let (stable,stableID) = try createSource("Private stable source")
            let enumerated = (0..<MIDIGetNumberOfSources()).map { MIDIGetSource($0) }
            report["privateVisibleWithinProcess"] = enumerated.contains(stable)
            var probeClient:MIDIClientRef = 0,probePort:MIDIPortRef = 0
            try require(MIDIClientCreateWithBlock("Private connection probe" as CFString,&probeClient,nil) == noErr,"Cannot create private probe client.")
            defer { MIDIClientDispose(probeClient) }
            try require(MIDIInputPortCreateWithBlock(probeClient,"Private probe input" as CFString,&probePort,{_,_ in}) == noErr,"Cannot create private probe port.")
            defer { MIDIPortDispose(probePort) }
            let privateConnectStatus = MIDIPortConnectSource(probePort,stable,nil)
            report["privateExplicitConnectStatus"] = privateConnectStatus
            var found:MIDIObjectRef = 0,foundKind:MIDIObjectType = .other
            report["privateUniqueIDLookupStatus"] = MIDIObjectFindByUniqueID(stableID,&found,&foundKind)
            report["privateUniqueIDLookupObject"] = found
            try require(privateConnectStatus == noErr && found == stable,"Private source cannot connect through an explicit same-process endpoint reference.")
            let privateHost = try MIDIHost(inputIDs:[stableID],inputSourceProvider:{ sourceList.snapshot() }); hosts.append(privateHost)
            try require(selected(privateHost) == [Int(stableID)],"Separate same-process MIDIHost cannot connect the private source.")
            privateHost.shutdown()
            var checks = ["Private source is hidden from public enumeration but connects by explicit same-process endpoint reference"]

            let (removed,removedID) = try createSource("Private disappearing source")
            var removalTriggered = false,failedConnectStatus:OSStatus = noErr,connectionAttempts = 0
            let host = try MIDIHost(inputIDs:[removedID,stableID],inputSourceProvider:{ sourceList.snapshot() },inputConnector:{ port,source,context in
                connectionAttempts += 1
                if source == removed && !removalTriggered {
                    removalTriggered = true
                    MIDIEndpointDispose(source)
                    sourceList.remove(source)
                    let result = MIDIPortConnectSource(port,source,context)
                    failedConnectStatus = result
                    return result
                }
                return MIDIPortConnectSource(port,source,context)
            })
            hosts.append(host); endpoints.removeAll {$0 == removed}
            try require(removalTriggered && failedConnectStatus != noErr,"The fixture did not reproduce a real disappearing-source connection error.")
            try require(selected(host) == [Int(stableID)],"A disappeared source was retained as connected or prevented the surviving source from connecting.")
            try await Task.sleep(nanoseconds:250_000_000)
            _ = host.devices() // Synchronize after the coalesced refresh.
            try require(connectionAttempts == 2,"An absent endpoint caused repeated connection retries.")
            report["disappearedConnectStatus"] = failedConnectStatus
            checks.append("Actual disposal between enumeration and connection is skipped with one bounded refresh; surviving source remains connected")

            _ = try host.configure(payload:["channel":7])
            try require((host.devices()["requestedInputIDs"] as? [String])?.contains(String(removedID)) == true,"Unrelated setting discarded the disconnected input selection.")
            let (returned,_) = try createSource("Private returned source",id:removedID)
            try await waitFor { selected(host).contains(Int(removedID)) }
            try require(selected(host) == [Int(stableID),Int(removedID)],"Saved selected input identity was lost during disappearance.")
            let capture = MIDIHotplugCapture()
            host.onPacket = { status,a,b,source in capture.append([Int(status),Int(a),Int(b),Int(source)]) }
            try publish([0x90,60,100,0x80,60,0,0xB0,123,0],source:returned)
            try await waitFor { capture.snapshot().count == 3 }
            try require(capture.snapshot() == [[0x90,60,100,Int(removedID)],[0x80,60,0,Int(removedID)],[0xB0,123,0,Int(removedID)]],"Reconnected private source changed note/release bytes or identity.")
            checks.append("A source returning with the same saved ID reconnects by notification and delivers note/release messages")

            // Unplugging while a key and the sustain pedal are down: the source can never
            // send their releases, so the host must deliver them before forgetting it.
            try publish([0x91,62,90,0xB1,64,127,0x92,70,80,0x82,70,0],source:returned)
            try await waitFor { capture.snapshot().count == 7 }
            // Re-sending the unchanged route (Settings resubmits every field) must not forget held keys.
            _ = try host.configure(payload:["inputIDs":[removedID,stableID],"inputChannel":0,"enabled":true])
            sourceList.remove(returned); MIDIEndpointDispose(returned); endpoints.removeAll {$0 == returned}
            try await waitFor { capture.snapshot().count == 9 }
            try await Task.sleep(nanoseconds:250_000_000)
            try require(Array(capture.snapshot().suffix(from:7)) == [[0x81,62,0,Int(removedID)],[0xB1,64,0,Int(removedID)]] && capture.snapshot().count == 9,
                        "An unplugged source left a held key or sustain pedal without a release: \(capture.snapshot())")
            checks.append("Unplugging a source with a held key and pedal delivers exactly one note-off and pedal-up under its saved ID")

            var validFailureSurfaced = false
            do {
                let unexpected = try MIDIHost(inputIDs:[stableID],inputSourceProvider:{ sourceList.snapshot() },inputConnector:{ _,_,_ in -50 })
                unexpected.shutdown()
            } catch MIDIHost.HostError.system(_,let error) { validFailureSurfaced = error == -50 }
            try require(validFailureSurfaced,"A -50 connection failure for a still-valid source was swallowed.")
            let (portFailure,portFailureID) = try createSource("Private port failure source")
            var portFailureSurfaced = false
            do {
                let unexpected = try MIDIHost(inputIDs:[portFailureID],inputSourceProvider:{ sourceList.snapshot() },inputConnector:{ _,source,_ in
                    MIDIEndpointDispose(source); sourceList.remove(source); return kMIDIInvalidPort
                })
                unexpected.shutdown()
            } catch MIDIHost.HostError.system(_,let error) { portFailureSurfaced = error == kMIDIInvalidPort }
            endpoints.removeAll {$0 == portFailure}
            try require(portFailureSurfaced,"An invalid-port error was swallowed because a source also disappeared.")
            checks.append("Still-valid source -50 and invalid-port failures remain visible, even during simultaneous source removal")

            for index in 0..<32 {
                let (source,id) = try createSource("Private teardown \(index)")
                let temporary = try MIDIHost(inputIDs:[id],inputSourceProvider:{ sourceList.snapshot() }); temporary.shutdown()
                try require(MIDIEndpointDispose(source) == noErr,"Cannot dispose orderly test source.")
                endpoints.removeAll {$0 == source}
                sourceList.remove(source)
                let next = try MIDIHost(inputIDs:[stableID],inputSourceProvider:{ sourceList.snapshot() }); next.shutdown()
            }
            checks.append("32 orderly shutdown/remove/immediate-host-create cycles complete without stale endpoint failures")
            let destinations=MIDIHotplugSourceList()
            func createDestination(_ id:Int32? = nil) throws -> (MIDIEndpointRef,Int32) {
                var endpoint:MIDIEndpointRef=0
                try require(MIDIDestinationCreateWithBlock(client,"Private output reconnect" as CFString,&endpoint,{_,_ in}) == noErr,"Cannot create private output.")
                endpoints.append(endpoint)
                try require(MIDIObjectSetIntegerProperty(endpoint,kMIDIPropertyPrivate,1) == noErr,"Cannot make output private.")
                if let id { try require(MIDIObjectSetIntegerProperty(endpoint,kMIDIPropertyUniqueID,id) == noErr,"Cannot restore output identity.") }
                var uniqueID:Int32=0
                try require(MIDIObjectGetIntegerProperty(endpoint,kMIDIPropertyUniqueID,&uniqueID) == noErr,"Cannot read output ID.")
                destinations.add(endpoint);return (endpoint,uniqueID)
            }
            let (output,outputID)=try createDestination()
            let outputHost=try MIDIHost(inputIDs:[stableID],inputSourceProvider:{sourceList.snapshot()},outputDestinationProvider:{destinations.snapshot()})
            hosts.append(outputHost)
            _ = try outputHost.configure(payload:["outputID":outputID])
            try require(outputHost.prepareDestination()?.endpoint == output,"Default output not resolved by saved unique ID.")
            try require(outputHost.prepareDestination(Int32(bitPattern:output)) == nil || Int32(bitPattern:output)==outputID,"Transient endpoint handle was mistaken for a persistent ID.")
            destinations.remove(output);MIDIEndpointDispose(output);endpoints.removeAll {$0 == output}
            try await waitFor { outputHost.prepareDestination() == nil }
            _ = try outputHost.configure(payload:["channel":12,"inputChannel":4])
            try require(outputHost.devices()["requestedOutputID"] as? String == String(outputID),"Disconnected output preference was erased.")
            let (restoredOutput,_)=try createDestination(outputID)
            try await waitFor { outputHost.prepareDestination()?.endpoint == restoredOutput }
            _ = try outputHost.configure(payload:["enabled":false])
            try require(outputHost.prepareDestination(outputID)?.endpoint == restoredOutput,"Input enablement incorrectly gates independent hardware output.")
            checks.append("Output unique IDs survive unplug and unrelated settings, reconnect to the replacement endpoint, and remain enabled with MIDI input disabled")

            report["checks"] = checks; report["status"] = "passed"; report["passed"] = true
            report["teardownCycles"] = 32; report["privateMessageCount"] = capture.snapshot().count
            save(report); print("PASS private CoreMIDI hotplug: \(checks.count) checks, 32 teardown cycles; no hardware or public MIDI messages.")
        } catch {
            report["status"] = "failed"; report["passed"] = false; report["error"] = String(describing:error)
            save(report); print("FAIL private CoreMIDI hotplug: \(error)"); exit(1)
        }
    }

    static func publish(_ bytes:[UInt8],source:MIDIEndpointRef) throws {
        var list = MIDIPacketList()
        let status = withUnsafeMutablePointer(to:&list) { pointer in
            let packet = MIDIPacketListInit(pointer)
            _ = bytes.withUnsafeBufferPointer { MIDIPacketListAdd(pointer,MemoryLayout<MIDIPacketList>.size,packet,0,bytes.count,$0.baseAddress!) }
            return MIDIReceived(source,pointer)
        }
        if status != noErr { throw MIDIHotplugFailure(description:"Publishing private MIDI failed: \(status)") }
    }
    static func save(_ report:[String:Any]) {
        guard CommandLine.arguments.count > 1 else { return }
        let url = URL(fileURLWithPath:CommandLine.arguments[1])
        try? FileManager.default.createDirectory(at:url.deletingLastPathComponent(),withIntermediateDirectories:true)
        if let data = try? JSONSerialization.data(withJSONObject:report,options:[.sortedKeys,.prettyPrinted]) { try? data.write(to:url,options:.atomic) }
    }
}

private final class MIDIHotplugCapture:@unchecked Sendable {
    private let lock = NSLock()
    private var values:[[Int]] = []
    func append(_ value:[Int]) { lock.lock(); values.append(value); lock.unlock() }
    func snapshot()->[[Int]] { lock.lock(); defer {lock.unlock()}; return values }
}

private final class MIDIHotplugSourceList:@unchecked Sendable {
    private let lock = NSLock()
    private var values:[MIDIEndpointRef] = []
    func add(_ value:MIDIEndpointRef) { lock.lock(); values.append(value); lock.unlock() }
    func remove(_ value:MIDIEndpointRef) { lock.lock(); values.removeAll {$0 == value}; lock.unlock() }
    func snapshot()->[MIDIEndpointRef] { lock.lock(); defer {lock.unlock()}; return values }
}

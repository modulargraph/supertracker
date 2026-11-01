// In-app self-test: compiled only with -D SELFTESTS (debug builds and scripts/build.sh selftest). See docs/TESTING.md.
#if SELFTESTS
import Foundation
import AVFoundation
import AudioToolbox

/// Original registered AU fixtures only. No device playback, MIDI or vendor scan.
enum NativeAUStateReuseTests {
    private static let description = AudioComponentDescription(componentType:kAudioUnitType_Effect,
        componentSubType:0x74707273,componentManufacturer:0x53745473,componentFlags:0,componentFlagsMask:0)
    private static let registration:Void = {
        AUAudioUnit.registerSubclass(TypedStateFixtureUnit.self,as:description,name:"SuperTracker Test: Typed state",version:1)
    }()
    @MainActor static func run(output:URL) async throws -> [String] {
        _ = registration
        // Core Audio registers the in-process component synchronously, while an
        // already-created AVAudioUnitComponentManager refreshes its inventory on
        // a later main-loop turn. This fixture has no asynchronous host setup to
        // provide that turn; wait for actual discovery before testing rack.load.
        var registeredDescription = description
        guard AudioComponentFindNext(nil,&registeredDescription) != nil else {
            throw AudioSampleError.invalid("AU state reuse: original component registration failed")
        }
        let discoveryStart = ContinuousClock.now
        let discoveryDeadline = discoveryStart.advanced(by:.seconds(3))
        var discoveryPolls = 0
        while AVAudioUnitComponentManager.shared().components(matching:description).isEmpty {
            guard ContinuousClock.now < discoveryDeadline else {
                throw AudioSampleError.invalid("AU state reuse: original component registration did not become discoverable")
            }
            discoveryPolls += 1
            try await Task.sleep(for:.milliseconds(10))
        }
        let folder = output.appendingPathComponent("au-state-reuse",isDirectory:true)
        try FileManager.default.createDirectory(at:folder,withIntermediateDirectories:true)
        let graph = AVAudioEngine(),format = AVAudioFormat(standardFormatWithSampleRate:48_000,channels:2)!
        try graph.enableManualRenderingMode(.offline,format:format,maximumFrameCount:512)
        let rack = try AudioPluginRack(engine:graph,trackCount:1,sampleRate:48_000)
        let source = AVAudioSourceNode(format:format) { silence,_,frames,output in
            for buffer in UnsafeMutableAudioBufferListPointer(output) {
                if let pointer = buffer.mData { memset(pointer,0,Int(frames)*MemoryLayout<Float>.size) }
            }
            silence.pointee = true; return noErr
        }
        rack.installNativeSources([source])
        defer { graph.stop(); _ = rack.closeAllPluginsAfterStop() }
        var checks:[String] = [],metrics:[String:Any] = ["discoveryPolls":discoveryPolls]
        func require(_ condition:Bool,_ message:String) throws {
            if !condition { throw AudioSampleError.invalid("AU state reuse: "+message) }
        }
        func target(_ slot:Int)->[String:Any] { ["track":0,"kind":"effect","slot":slot] }
        func encode(_ value:[String:Any],format:PropertyListSerialization.PropertyListFormat = .binary) throws -> String {
            try PropertyListSerialization.data(fromPropertyList:value,format:format,options:0).base64EncodedString()
        }
        func decode(_ descriptor:[String:Any]) throws -> [String:Any] {
            let data = Data(base64Encoded:descriptor["state"] as! String)!
            return try PropertyListSerialization.propertyList(from:data,format:nil) as! [String:Any]
        }
        func state(_ slot:Int = 0) throws -> [String:Any] { try rack.state(target(slot)) as! [String:Any] }
        func load(_ slot:Int = 0,saved:String? = nil) async throws -> TypedStateFixtureUnit {
            var request = target(slot)
            request["component"] = "\(description.componentType):\(description.componentSubType):\(description.componentManufacturer)"
            request["state"] = saved
            _ = try await rack.load(request)
            guard let unit = TypedStateFixtureUnit.lastCreated else { throw AudioSampleError.invalid("Original AU was not created") }
            return unit
        }
        func stats(_ entries:Int,_ label:String) throws {
            let value = rack.auStateReuseStatistics()
            try require(value.entries == entries && value.retainedBytes <= 32*1024*1024,"\(label) cache ownership/budget differs")
        }

        var unit = try await load()
        let initial = try state()["state"] as! String,reads = unit.reads
        for _ in 0..<24 { try require(try state()["state"] as? String == initial,"equal state changed encoded bytes") }
        try require(unit.reads == reads+24,"cache bypassed a fresh AU getter")
        let xml = try encode(unit.contents,format:.xml)
        var restore = target(0); restore["state"] = xml
        let seeded = try rack.state(restore) as! [String:Any]
        try require(seeded["state"] as? String == xml,"successful restore did not preserve saved encoding")
        try require(unit.latestRawEncoding != xml,"fixture must freshly serialize different binary bytes")
        metrics["freshReads"] = unit.reads; metrics["rawEncodingVariants"] = unit.rawEncodings.count
        checks.append("Repeated native AU captures read fresh state yet preserve stable bytes for equal property lists and restored encoding differences")

        var distinct = Set<String>()
        for value in [NSNumber(value:true),NSNumber(value:1),NSNumber(value:1.0)] {
            unit.contents["typed"] = value; distinct.insert(try state()["state"] as! String)
        }
        try require(distinct.count == 3,"Bool/integer/real AU edits were collapsed")
        let beforeBlob = try state()["state"] as! String
        unit.contents["blob"] = Data([0,1,3,0])
        try require(try state()["state"] as? String != beforeBlob,"opaque Data edit was hidden")
        _ = try rack.parameter(["track":0,"kind":"effect","slot":0,"address":"42","normalized":false,"value":0.71])
        try require(abs((try decode(state())["gain"] as! NSNumber).doubleValue-0.71)<0.00001,"parameter edit was hidden")
        checks.append("Native AU capture preserves Boolean/integer/real changes, exact opaque data and live parameter edits")

        unit.invalidCapture = true
        var captureFailed = false
        do { _ = try state() } catch { captureFailed = true }
        let failedSnapshot = rack.snapshot()[0]
        try require(captureFailed && failedSnapshot["stateError"] is String && failedSnapshot["state"] == nil,"cached state concealed serialization failure")
        unit.invalidCapture = false
        _ = try state(); try stats(1,"capture recovery")
        checks.append("A fresh AU serialization failure propagates and remains a snapshot stateError despite an existing cached state")

        let originalGain = (unit.contents["gain"] as! NSNumber).doubleValue
        unit.ignoreRestore = true
        var ignored = unit.contents; ignored["gain"] = 0.13
        restore["state"] = try encode(ignored,format:.xml)
        let actual = try rack.state(restore) as! [String:Any]
        try require((try decode(actual)["gain"] as! NSNumber).doubleValue == originalGain && actual["state"] as? String != restore["state"] as? String,
                    "ignored AU restore returned seeded requested state instead of actual state")
        unit.ignoreRestore = false
        let oldID = rack.loadedEffects(track:0)[0]["instanceID"] as! String
        let restoresBeforeCandidate = TypedStateFixtureUnit.restorations
        TypedStateFixtureUnit.rejectNextFormat = true
        var candidateRejected = false
        do { _ = try await load(saved:try encode(unit.contents)) } catch { candidateRejected = true }
        try require(candidateRejected && rack.loadedEffects(track:0)[0]["instanceID"] as? String == oldID,"failed seeded candidate replaced the old AU")
        try require(TypedStateFixtureUnit.restorations == restoresBeforeCandidate+1,"candidate did not reach restore/seed before format rejection")
        try stats(1,"failed seeded candidate")
        restore["state"] = "not base64"
        var restoreRejected = false
        do { _ = try rack.state(restore) } catch { restoreRejected = true }
        try require(restoreRejected && rack.loadedEffects(track:0).isEmpty,"invalid restoration was accepted")
        try stats(0,"failed direct restore retirement")
        checks.append("Ignored restores return freshly observed state; malformed restores and abandoned seeded candidates cannot retain stale cache entries")

        unit = try await load()
        _ = try await load(1)
        func permutation(_ descriptors:[[String:Any]],changing changed:Any? = nil) throws -> [[String:Any]] {
            try descriptors.enumerated().map { index,descriptor in
                var item = descriptor; item["slot"] = 1-(descriptor["slot"] as! Int)
                if index == 0,let changed { var data = try decode(item); data["typed"] = changed; item["state"] = try encode(data) }
                return item
            }
        }
        for changed in [NSNumber(value:1),NSNumber(value:1.0)] {
            let before = rack.snapshot(),ids = rack.loadedEffects(track:0).sorted { ($0["slot"] as! Int)<($1["slot"] as! Int) }.map { $0["instanceID"] as! String }
            let accepted = try rack.restoreEffectOrder(permutation(before,changing:changed))
            let after = rack.loadedEffects(track:0).sorted { ($0["slot"] as! Int)<($1["slot"] as! Int) }.map { $0["instanceID"] as! String }
            try require(!accepted && ids == after,"effect permutation mistook a typed state edit for an unchanged instance")
        }
        let original = rack.snapshot(),beforeMoveReads = unit.reads,creations = TypedStateFixtureUnit.creations
        try require(try rack.restoreEffectOrder(permutation(original)),"unchanged typed effect permutation was rejected")
        try require(TypedStateFixtureUnit.creations == creations && unit.reads == beforeMoveReads,"pure permutation recreated/read a unit unexpectedly")
        try stats(2,"effect reorder")
        var integerState = try decode(state()); integerState["typed"] = NSNumber(value:1)
        var integerRestore = target(0); integerRestore["state"] = try encode(integerState)
        _ = try rack.state(integerRestore)
        try require(!(try rack.restoreEffectOrder(permutation(rack.snapshot(),changing:NSNumber(value:1.0)))),"integer/real permutation edit was accepted")
        checks.append("Effect permutation recognition rejects Boolean/integer/real edits while preserving actual unchanged AU instances and their cache identities")

        let previous = try state(), previousID = previous["instanceID"] as! String
        unit = try await load()
        let replacement = try state(), replacementID = replacement["instanceID"] as! String
        try require(replacementID != previousID,"same-component replacement retained old identity")
        let badIdentities:[Any] = [previousID,"",NSNumber(value:42)]
        for badIdentity in badIdentities {
            var stale = target(0); stale["instanceID"] = badIdentity
            stale["address"] = "42"; stale["normalized"] = false; stale["value"] = 0.02
            var rejected = false
            do { _ = try rack.parameter(stale) } catch { rejected = true }
            try require(rejected,"stale/malformed instance parameter request was accepted")
            stale["state"] = previous["state"]
            rejected = false
            do { _ = try rack.state(stale) } catch { rejected = true }
            try require(rejected && (try state()["instanceID"] as? String) == replacementID,
                        "stale restore modified or removed the valid replacement")
            try require((try decode(state())["gain"] as! NSNumber).doubleValue == 0.5,"stale parameter changed replacement")
        }
        try stats(2,"same-component replacement")
        checks.append("Stale or malformed instance IDs reject parameter and state writes before touching a same-slot replacement")

        _ = try rack.remove(target(0)); try stats(1,"retired unit")
        _ = try rack.remove(target(1)); try stats(0,"all retired units")
        _ = try await load(); try stats(1,"reloaded unit")
        graph.stop(); try require(rack.closeAllPluginsAfterStop(),"rack did not close after stopped graph")
        try stats(0,"shutdown")
        metrics["finalEntries"] = rack.auStateReuseStatistics().entries
        checks.append("AU state cache entries are evicted on retirement, failed restore, replacement and complete shutdown")
        metrics["checks"] = checks; metrics["passed"] = true; metrics["plugins"] = "original registered AU only"
        metrics["generatedAt"] = ISO8601DateFormatter().string(from:Date())
        try JSONSerialization.data(withJSONObject:metrics,options:[.prettyPrinted,.sortedKeys]).write(to:folder.appendingPathComponent("metrics.json"),options:.atomic)
        return checks
    }
}

private final class TypedStateFixtureUnit:AUAudioUnit {
    static weak var lastCreated:TypedStateFixtureUnit?
    static var creations = 0
    static var restorations = 0
    static var rejectNextFormat = false
    private let rejectsFormat:Bool
    private var inputs:AUAudioUnitBusArray!,outputs:AUAudioUnitBusArray!
    var reads = 0,rawEncodings = Set<String>(),latestRawEncoding = ""
    var invalidCapture = false,ignoreRestore = false
    var contents:[String:Any] = ["typed":NSNumber(value:true),"gain":NSNumber(value:0.5),"blob":Data([0,1,2,0]),
        "nested":["name":"Original","date":Date(timeIntervalSinceReferenceDate:1.125),"array":[1,2,3]]]
    override init(componentDescription:AudioComponentDescription,options:AudioComponentInstantiationOptions = []) throws {
        rejectsFormat = Self.rejectNextFormat; Self.rejectNextFormat = false
        try super.init(componentDescription:componentDescription,options:options)
        let format = AVAudioFormat(standardFormatWithSampleRate:rejectsFormat ? 44_100 : 48_000,channels:2)!
        inputs = AUAudioUnitBusArray(audioUnit:self,busType:.input,busses:[try AUAudioUnitBus(format:format)])
        outputs = AUAudioUnitBusArray(audioUnit:self,busType:.output,busses:[try AUAudioUnitBus(format:format)])
        let parameter = AUParameterTree.createParameter(withIdentifier:"gain",name:"Gain",address:42,min:0,max:1,unit:.generic,unitName:nil,
            flags:[.flag_IsReadable,.flag_IsWritable],valueStrings:nil,dependentParameters:nil)
        parameterTree = AUParameterTree.createTree(withChildren:[parameter])
        parameterTree?.implementorValueObserver = { [weak self] _,value in self?.contents["gain"] = NSNumber(value:value) }
        parameterTree?.implementorValueProvider = { [weak self] _ in (self?.contents["gain"] as? NSNumber)?.floatValue ?? 0 }
        Self.lastCreated = self; Self.creations += 1
    }
    override var fullStateForDocument:[String:Any]? {
        get {
            reads += 1
            if invalidCapture { return ["invalid":NSNull()] }
            var result:[String:Any] = [:]
            let keys = contents.keys.sorted()
            for key in (reads % 2 == 0 ? keys : Array(keys.reversed())) { result[key] = contents[key] }
            if let bytes = try? PropertyListSerialization.data(fromPropertyList:result,format:.binary,options:0) {
                latestRawEncoding = bytes.base64EncodedString(); rawEncodings.insert(latestRawEncoding)
            }
            return result
        }
        set { Self.restorations += 1; if !ignoreRestore,let newValue { contents = newValue } }
    }
    override var inputBusses:AUAudioUnitBusArray { inputs }
    override var outputBusses:AUAudioUnitBusArray { outputs }
    override var channelCapabilities:[NSNumber]? { [2,2] }
    override func shouldChange(to format:AVAudioFormat,for bus:AUAudioUnitBus)->Bool { !rejectsFormat }
    override var internalRenderBlock:AUInternalRenderBlock {
        { flags,time,frames,_,output,_,pull in
            guard let pull else { return kAudioUnitErr_NoConnection }
            return pull(flags,time,frames,0,output)
        }
    }
}

#endif

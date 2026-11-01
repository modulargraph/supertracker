import Foundation

/// Independently generated archive bytes only. No installed plug-in files or execution.
@main
struct MaschineGroupPluginStateTests {
    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }
    struct Fixture {
        var bytes: Data
        var ids: [Range<Int>]
        var bools: [Range<Int>]
        var modernMarkers: [Int]
        var identityLength: Range<Int>
        var identityBytes: Range<Int>
        var nameLength: Range<Int>
        var nameBytes: Range<Int>
        var chunkLength: Range<Int>
        var chunkBytes: Range<Int>
        var preset: Range<Int>
    }
    static var checks = 0
    static let objectID = 253
    static let payload = Data([0, 255, 17, 0, 128, 64, 9, 2, 1, 254, 0, 7, 19])
    static let identity = "Native Instruments:VST:INSTR:1315523937"
    static let flags = [true, false, true, false]

    static func main() throws {
        try validLayouts()
        try identities()
        try invalidFrames()
        try documentWrappers()
        print("PASS Maschine plug-in state: \(checks) generated checks; modern/legacy layouts, exact identity and opaque payload, strict bounds, VST2 bank header and binary-plist descriptor, unsupported state rejection")
    }
    static func check(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
        checks += 1
        guard try condition() else { throw Failure(message: message) }
    }
    static func rejects(_ bytes: Data, _ message: String, modern: Bool = true, base: Int = objectID,
                        moduleVersion: Int = 1, maximum: Int = 32 * 1024 * 1024) throws {
        checks += 1
        do { _ = try MaschineGroupPluginState.decode(body: bytes, objectID: base, modern: modern, moduleVersion: moduleVersion, maxStateBytes: maximum) }
        catch { return }
        throw Failure(message: message)
    }

    /// Test writer deliberately does not use the production portable-integer encoder.
    static func portable(_ value: UInt32) -> Data {
        if value == 0 { return Data([0]) }
        var number = value, digits: [UInt8] = []
        while number > 0 { digits.append(UInt8(number & 0xff)); number >>= 8 }
        return Data([UInt8(digits.count)] + digits)
    }
    static func word(_ value: UInt32) -> Data { Data((0..<4).map { UInt8(truncatingIfNeeded: value >> ($0 * 8)) }) }
    static func fixture(modern: Bool = true, intro: Bool = false, identity: String = identity, name: String = "Prism – Éclat",
                        chunkVersion: UInt32 = 1, chunkType: UInt32 = 3, payload: Data = payload,
                        preset: UInt32 = 257) -> Fixture {
        var bytes = Data(), ids: [Range<Int>] = [], bools: [Range<Int>] = [], markers: [Int] = []
        func append(_ value: Data) -> Range<Int> {
            let start = bytes.count; bytes.append(value); return start..<bytes.count
        }
        var next = UInt32(objectID + 1)
        for flag in flags {
            for _ in 0..<(modern ? 4 : 3) { ids.append(append(portable(next))); next += 1 }
            if modern { markers.append(bytes.count); bytes.append(0) }
            for value: UInt32 in [flag ? 1 : 0, flag ? 0 : 1, flag ? 1 : 0, 0, 1] { bools.append(append(portable(value))) }
        }
        let identityData = Data(identity.utf8), nameData = Data(name.utf8)
        let identityLength = append(portable(UInt32(identityData.count))), identityBytes = append(identityData)
        let nameLength = append(portable(UInt32(nameData.count))), nameBytes = append(nameData)
        if intro { bytes.append(contentsOf: [0, 0]) }
        let chunk = word(chunkVersion) + word(chunkType) + word(UInt32(payload.count)) + payload
        let chunkLength = append(portable(UInt32(chunk.count))), chunkBytes = append(chunk)
        let presetRange = append(portable(preset))
        return Fixture(bytes: bytes, ids: ids, bools: bools, modernMarkers: markers, identityLength: identityLength,
                       identityBytes: identityBytes, nameLength: nameLength, nameBytes: nameBytes,
                       chunkLength: chunkLength, chunkBytes: chunkBytes, preset: presetRange)
    }
    static func replacing(_ source: Data, _ range: Range<Int>, with replacement: Data) -> Data {
        var bytes = source; bytes.replaceSubrange(range, with: replacement); return bytes
    }

    static func validLayouts() throws {
        for modern in [false, true] {
            for intro in [false, true] {
                for type in UInt32(0)...7 {
                    let value = fixture(modern: modern, intro: intro, chunkType: type)
                    let before = value.bytes
                    let decoded = try MaschineGroupPluginState.decode(body: value.bytes, objectID: objectID, modern: modern)
                    try check(decoded.uniquePluginID == identity && decoded.pluginName == "Prism – Éclat", "Identity and Unicode display name must survive exactly")
                    try check(decoded.vst2UniqueID == UInt32(1_315_523_937) && decoded.isInstrument == true, "Decimal VST instrument identity must decode exactly")
                    try check(decoded.flags == flags, "Each BoolParameter flag must use its current value, not a default or metadata flag")
                    try check(decoded.chunkVersion == 1 && decoded.chunkType == type && decoded.payload == payload && decoded.presetIndex == 257,
                              "Chunk metadata, opaque payload and preset index must remain exact")
                    try check(value.bytes == before, "Decoding must not mutate source bytes")
                    try check(JSONSerialization.isValidJSONObject(decoded.dictionary) && decoded.dictionary["payload"] == nil && decoded.dictionary["state"] == nil,
                              "Browser metadata must stay JSON-safe and exclude opaque state bytes")
                    try check(decoded.dictionary["requiresHostStateValidation"] as? Bool == true,
                              "File parsing must not claim host state compatibility")
                    let tail = Data([255, 0, 9, 0, 0, 1, 255]) + Data("uninterpreted PluginParameters and window".utf8)
                    let withTail = try MaschineGroupPluginState.decode(body: value.bytes + tail, objectID: objectID, modern: modern)
                    try check(withTail.payload == payload && withTail.flags == flags && withTail.presetIndex == 257,
                              "Trailing parameter/window data must not be searched or interpreted as state")
                }
            }
        }
        for index in [UInt32(0), UInt32(255), UInt32(256), UInt32(65_535), UInt32.max] {
            let decoded = try MaschineGroupPluginState.decode(body: fixture(preset: index).bytes, objectID: objectID, modern: true)
            try check(decoded.presetIndex == index, "Portable preset indices must preserve the complete UInt32 range")
        }
    }

    static func identities() throws {
        let fxID = "Fixture Vendor:VST:FX:4294967295"
        let effect = try MaschineGroupPluginState.decode(body: fixture(identity: fxID, name: "Instrument-looking name").bytes, objectID: objectID, modern: true)
        try check(effect.uniquePluginID == fxID && effect.vst2UniqueID == UInt32.max && effect.isInstrument == false,
                  "FX identity must come from exact tokens, including unsigned IDs above Int32.max")
        let namedFX = try MaschineGroupPluginState.decode(body: fixture(name: "FX Effect Processor").bytes, objectID: objectID, modern: true)
        try check(namedFX.isInstrument == true, "The display name must never override the INSTR identity token")
        for unknown in ["Fixture Vendor:AU:INSTR:1315523937", "Fixture Vendor:VST3:INSTR:1315523937", "Fixture Vendor:Custom:FX:1315523937"] {
            let decoded = try MaschineGroupPluginState.decode(body: fixture(identity: unknown).bytes, objectID: objectID, modern: true)
            try check(decoded.uniquePluginID == unknown && decoded.vst2UniqueID == nil && decoded.payload == payload,
                      "Unknown technology must preserve identity/state without inventing a VST2 ID")
            try check(decoded.isInstrument == !unknown.contains(":FX:"), "An exact role token may classify an otherwise unknown technology")
        }
        let unknownRole = try MaschineGroupPluginState.decode(body: fixture(identity: "Fixture FX Vendor:VST:UNKNOWN:1315523937", name: "FX").bytes, objectID: objectID, modern: true)
        try check(unknownRole.isInstrument == nil, "FX inside a vendor or display name must not classify an unknown role")
        for suffix in ["4294967296", "-1", "+1", "1e3", "0x4E692431", " 123", "123 ", "١٢٣", "123:trailing", "", "0"] {
            let malformedIdentity = "Fixture:VST:INSTR:" + suffix
            let decoded = try MaschineGroupPluginState.decode(body: fixture(identity: malformedIdentity).bytes, objectID: objectID, modern: true)
            try check(decoded.uniquePluginID == malformedIdentity && decoded.vst2UniqueID == nil && decoded.payload == payload,
                      "An invalid decimal ID must remain opaque rather than being truncated, coerced or guessed")
        }
        for unknown in ["Fixture:VST:1315523937", "Fixture:VST:fx:1315523937", "Fixture:VST:INSTRUMENT:1315523937"] {
            let decoded = try MaschineGroupPluginState.decode(body: fixture(identity: unknown, name: "FX processor").bytes, objectID: objectID, modern: true)
            try check(decoded.isInstrument == nil && decoded.vst2UniqueID == nil, "Only exact role tokens establish a host identity")
        }
    }

    static func invalidFrames() throws {
        for modern in [false, true] {
            for intro in [false, true] {
                let value = fixture(modern: modern, intro: intro)
                for count in 0..<value.bytes.count {
                    try rejects(Data(value.bytes.prefix(count)), "Every truncated required field must fail (modern=\(modern), intro=\(intro), length=\(count))", modern: modern)
                }
                try rejects(value.bytes, "Legacy/modern layouts must not be interchanged", modern: !modern)
            }
        }
        let value = fixture()
        for index in value.ids.indices {
            let bad = replacing(value.bytes, value.ids[index], with: portable(UInt32(objectID + index + 3)))
            try rejects(bad, "Every parameter identity must match its expected consecutive object ID")
        }
        for index in value.bools.indices {
            try rejects(replacing(value.bytes, value.bools[index], with: portable(2)), "Every persisted BoolParameter member must be restricted to 0 or 1")
        }
        for marker in value.modernMarkers {
            try rejects(replacing(value.bytes, marker..<(marker + 1), with: Data([1])), "Modern class marker must be zero")
        }
        for range in [value.ids[0], value.bools[0], value.identityLength, value.nameLength, value.chunkLength, value.preset] {
            for bad in [Data([1, 0]), Data([2, 1, 0]), Data([5, 1, 2, 3, 4, 5]), Data([255])] {
                try rejects(replacing(value.bytes, range, with: bad), "Noncanonical or oversized portable integer must fail")
            }
        }
        for range in [value.identityLength, value.nameLength] {
            try rejects(replacing(value.bytes, range, with: portable(UInt32.max)), "Oversized declared string length must fail without allocation")
        }
        for range in [value.identityBytes, value.nameBytes] {
            try rejects(replacing(value.bytes, range.lowerBound..<(range.lowerBound + 1), with: Data([255])), "Invalid UTF-8 strings must fail")
            try rejects(replacing(value.bytes, range.lowerBound..<(range.lowerBound + 1), with: Data([0])), "Embedded NUL in strings must fail")
        }
        try rejects(fixture(identity: String(repeating: "x", count: 10_000)).bytes, "Identity strings must have a bounded decoded length")
        try rejects(fixture(name: String(repeating: "x", count: 10_000)).bytes, "Display names must have a bounded decoded length")
        try rejects(fixture(identity: "").bytes, "Empty unique identity must fail")
        try rejects(fixture(name: "").bytes, "Empty display name must fail")
        for intro in [Data([0]), Data([0, 0, 0]), Data([0, 1])] {
            try rejects(replacing(value.bytes, value.chunkLength.lowerBound..<value.chunkLength.lowerBound, with: intro),
                        "An incomplete or unsupported PluginChunk class introduction must fail")
        }
        try rejects(replacing(value.bytes, value.chunkLength, with: portable(UInt32.max)), "Declared state length must be bounded before allocation")
        try rejects(replacing(value.bytes, value.chunkLength, with: portable(11)), "Chunk length must include its complete fixed header")
        try rejects(replacing(value.bytes, value.chunkLength, with: portable(UInt32(value.chunkBytes.count + 1))), "Chunk byte length must agree with its embedded payload size")
        let sizeRange = (value.chunkBytes.lowerBound + 8)..<(value.chunkBytes.lowerBound + 12)
        try rejects(replacing(value.bytes, sizeRange, with: word(UInt32.max)), "Embedded payload size must remain inside the chunk")
        try rejects(replacing(value.bytes, sizeRange, with: word(UInt32(payload.count - 1))), "Unused bytes inside a chunk must not be silently accepted")
        for version in [UInt32(0), UInt32(2), UInt32.max] { try rejects(fixture(chunkVersion: version).bytes, "Unknown chunk version must fail") }
        for type in [UInt32(8), UInt32.max] { try rejects(fixture(chunkType: type).bytes, "Unknown chunk type must fail") }
        for version in [-1, 2, Int.max] { try rejects(value.bytes, "Unknown module version must fail", moduleVersion: version) }
        // Version 0 (Maschine 1.x) keeps the member order but always uses three-ID legacy
        // Bool parameters, even inside a Maschine 2 archive.
        try rejects(value.bytes, "A version 0 PluginHost must not accept four-ID parameters", moduleVersion: 0)
        let legacy = try MaschineGroupPluginState.decode(body: fixture(modern: false).bytes, objectID: objectID, modern: true, moduleVersion: 0)
        try check(legacy.flags == flags && legacy.payload == payload && legacy.presetIndex == 257, "Version 0 PluginHost state decodes exactly")
        try rejects(value.bytes, "Negative module object ID must fail", base: -1)
        try rejects(value.bytes, "Zero module object ID must fail", base: 0)
        try rejects(value.bytes, "Object ID arithmetic must reject overflow", base: Int.max)
        try rejects(value.bytes, "Consecutive object IDs must remain inside UInt32", base: Int(UInt32.max) - 15)
        try rejects(value.bytes, "Negative byte budget must fail", maximum: -1)
        try rejects(value.bytes, "Zero byte budget must fail", maximum: 0)
        try rejects(value.bytes, "Configured byte budget must not exceed the hard parser limit", maximum: 32 * 1024 * 1024 + 1)
        try rejects(fixture(payload: Data(repeating: 7, count: 65)).bytes, "Opaque state must honor its configured byte budget", maximum: 64)
        try rejects(fixture(payload: Data()).bytes, "A chunk with no opaque payload must fail")
        try rejects(value.bytes, "One byte over the configured payload budget must fail", maximum: payload.count - 1)
        let bounded = try MaschineGroupPluginState.decode(body: value.bytes, objectID: objectID, modern: true, maxStateBytes: 64)
        try check(bounded.payload == payload, "A valid state within the configured budget must remain readable")
        let exact = try MaschineGroupPluginState.decode(body: value.bytes, objectID: objectID, modern: true, maxStateBytes: payload.count)
        try check(exact.payload == payload, "The exact configured payload limit must remain readable")
    }

    static func documentWrappers() throws {
        func decoded(_ identity: String = identity, type: UInt32 = 1, preset: UInt32 = 54) throws -> MaschineGroupPluginState {
            try MaschineGroupPluginState.decode(body: fixture(identity: identity, chunkType: type, preset: preset).bytes,
                                                objectID: objectID, modern: true)
        }
        func wordAt(_ data: Data, _ offset: Int) -> UInt32 {
            (0..<4).reduce(UInt32(0)) { $0 | UInt32(data[offset + $1]) << ($1 * 8) }
        }
        func rejectsOperation(_ message: String, _ operation: () throws -> Void) throws {
            checks += 1
            do { try operation() } catch { return }
            throw Failure(message: message)
        }
        for uniqueID in [UInt32(1_315_523_937), 0x80000001, UInt32.max] {
            for preset in [UInt32(0), 54, UInt32(Int32.max)] {
                let state = try decoded("Fixture:VST:INSTR:\(uniqueID)", preset: preset)
                let document = try state.documentState()
                try check(document["format"] as? String == "SuperTracker.VST2" && document["version"] as? Int == 1,
                          "Document format must match the existing VST2 adapter")
                try check(document["id"] as? String == String(format: "vst2:%08X", uniqueID) && document["bundlePath"] == nil,
                          "Component identity must be exact; no installation path may be guessed from its name")
                guard let packet = document["data"] as? Data else { throw Failure(message: "VST2 document requires a binary packet") }
                try check(packet.count == 24 + payload.count && packet.prefix(4) == Data("STV2".utf8), "StateHeader must have its native 24-byte layout")
                try check(wordAt(packet,4) == 1 && wordAt(packet,8) == uniqueID && wordAt(packet,12) == 1,
                          "Header must retain version, complete signed-ID bits and bank kind")
                try check(wordAt(packet,16) == preset && wordAt(packet,20) == payload.count && packet.dropFirst(24) == payload,
                          "Header must preserve the selected program and exact opaque bank bytes")
                try check(state.payload == payload && state.presetIndex == preset, "Document creation must not modify original state")
            }
        }
        let state = try decoded(), path = "/private/tmp/Private fixture.vst"
        let descriptor = try state.descriptor(owner: "sound:kit-1:pad-14", instanceID: "fixture-instance", bundlePath: path, enabled: false)
        try check(JSONSerialization.isValidJSONObject(descriptor), "Saved descriptor must be JSON-safe")
        try check(descriptor["component"] as? String == "vst2:4E694D61" && descriptor["format"] as? String == "VST2"
                  && descriptor["kind"] as? String == "instrument" && descriptor["slot"] as? Int == 0,
                  "Instrument descriptor must use the existing exact VST2 loading route")
        try check(descriptor["owner"] as? String == "sound:kit-1:pad-14" && descriptor["instanceID"] as? String == "fixture-instance"
                  && descriptor["bypass"] as? Bool == true && descriptor["bundlePath"] as? String == path,
                  "Caller ownership, stable instance identity and bypass must be preserved")
        let metadata = descriptor["maschinePluginState"] as! [String: Any]
        try check(metadata["uniquePluginID"] as? String == identity && metadata["presetIndex"] as? UInt32 == 54
                  && !(metadata["warnings"] as? [String] ?? []).isEmpty && metadata["requiresHostStateValidation"] as? Bool == true,
                  "Descriptor must retain original state metadata and honest content/recall warnings")
        let encoded = Data(base64Encoded: descriptor["state"] as! String)!
        var format = PropertyListSerialization.PropertyListFormat.xml
        let restored = try PropertyListSerialization.propertyList(from: encoded, options: [], format: &format) as! [String: Any]
        try check(format == .binary && restored["format"] as? String == "SuperTracker.VST2"
                  && restored["id"] as? String == "vst2:4E694D61" && restored["bundlePath"] as? String == path,
                  "Descriptor state must deserialize through the host's binary-plist path")
        try check(restored["data"] as? Data == state.documentState(bundlePath: path)["data"] as? Data,
                  "Property-list encoding must preserve every packet byte")
        let effect = try decoded("Fixture:VST:FX:4294967295")
        let effectDescriptor = try effect.descriptor(owner: "bus:kit", slot: 7, instanceID: "effect-instance")
        try check(effectDescriptor["kind"] as? String == "effect" && effectDescriptor["slot"] as? Int == 7
                  && effectDescriptor["component"] as? String == "vst2:FFFFFFFF", "FX slots and high-bit IDs must retain their exact role")
        for type in UInt32(0)...7 where type != 1 {
            try rejectsOperation("Only verified NI bank chunks may produce VST2 document state") { _ = try decoded(type: type).documentState() }
        }
        for identity in ["Fixture:AU:INSTR:1315523937", "Fixture:VST:UNKNOWN:1315523937", "Fixture:VST:INSTR:4294967296"] {
            try rejectsOperation("An unresolved identity must never become a VST2 load descriptor") { _ = try decoded(identity).descriptor(owner: "sound:k:s") }
        }
        try rejectsOperation("A UInt32 index outside the native signed program range must fail") { _ = try decoded(preset: UInt32.max).documentState() }
        for path in ["relative.vst", "/tmp/../else.vst", "/tmp/NUL\u{0}.vst"] {
            try rejectsOperation("Installation hints must be canonical absolute paths") { _ = try state.documentState(bundlePath: path) }
        }
        try rejectsOperation("Instrument slots other than zero are invalid") { _ = try state.descriptor(owner: "sound:k:s", slot: 1) }
        for slot in [-1,16,Int.max] { try rejectsOperation("Effect slots must remain inside host bounds") { _ = try effect.descriptor(owner: "bus:k", slot: slot) } }
        for slot in [8,15] { _ = try effect.descriptor(owner: "bus:k", slot: slot) }
        for identity in ["", "bad/id", String(repeating: "x", count: 129)] {
            try rejectsOperation("Invalid instance IDs must fail before descriptor construction") { _ = try state.descriptor(owner: "sound:k:s", instanceID: identity) }
        }
        try rejectsOperation("Missing ownership must fail") { _ = try state.descriptor(owner: "") }
    }
}

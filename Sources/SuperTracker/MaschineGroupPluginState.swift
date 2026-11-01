import Foundation

/// File-only extraction of the versioned PluginHost member sequence.
/// State stays opaque: this reader never loads a plug-in, rewrites vendor paths,
/// substitutes an AU, or claims that a VST3 accepts its predecessor's VST2 state.
struct MaschineGroupPluginState {
    let uniquePluginID: String
    let pluginName: String
    let vst2UniqueID: UInt32?
    let isInstrument: Bool?
    let chunkVersion: UInt32
    let chunkType: UInt32
    let payload: Data
    let presetIndex: UInt32
    let flags: [Bool]

    var dictionary: [String: Any] {
        var result: [String: Any] = [
            "uniquePluginID": uniquePluginID, "pluginName": pluginName,
            "chunkVersion": chunkVersion, "chunkType": chunkType,
            "stateBytes": payload.count, "presetIndex": presetIndex,
            "stateEncoding": "NI.HOSTING.PluginChunk", "requiresHostStateValidation": true,
            "hostFlags": flags
        ]
        if let vst2UniqueID { result["vst2UniqueID"] = vst2UniqueID }
        if let isInstrument { result["kind"] = isInstrument ? "instrument" : "effect" }
        return result
    }

    var restorationWarnings: [String] {
        ["The matching VST2 plug-in must restore this bank and its selected program. Vendor content paths are unchanged; missing ensembles or samples may require repair inside the plug-in."]
    }

    /// The existing VST2 adapter's document format. Only proven NI bank chunks
    /// are wrapped; their payload is copied byte-for-byte, not converted to AU/VST3.
    func documentState(bundlePath: String? = nil) throws -> [String: Any] {
        guard let uniqueID = vst2UniqueID, uniqueID != 0, isInstrument != nil,
              chunkVersion == 1, chunkType == 1, !payload.isEmpty,
              payload.count <= 32 * 1024 * 1024, presetIndex <= UInt32(Int32.max) else {
            throw Self.invalid("only an identified VST2 bank chunk with a supported program index can become document state.")
        }
        if let bundlePath {
            guard bundlePath.hasPrefix("/"), !bundlePath.utf8.contains(0), bundlePath.utf8.count <= 16_384,
                  URL(fileURLWithPath: bundlePath).standardizedFileURL.path == bundlePath else {
                throw Self.invalid("a VST2 installation hint must be a canonical absolute bundle path.")
            }
        }
        // StateHeader in VST2Host.mm: 4-byte magic plus five 32-bit fields.
        // macOS arm64/x86_64 are little-endian. Encoding fields explicitly avoids
        // Swift struct padding and preserves IDs whose signed int32 bit is set.
        var packet = Data("STV2".utf8)
        for word: UInt32 in [1, uniqueID, 1, presetIndex, UInt32(payload.count)] {
            for offset in 0..<4 { packet.append(UInt8(truncatingIfNeeded: word >> (offset * 8))) }
        }
        packet.append(payload)
        var result: [String: Any] = ["format": "SuperTracker.VST2", "version": 1,
                                    "id": String(format: "vst2:%08X", uniqueID), "data": packet]
        if let bundlePath { result["bundlePath"] = bundlePath }
        return result
    }

    /// Mirrors `MixerChannel.insertSlotCount` (this file is also built standalone).
    static let insertSlotCount = 16

    /// A normal saved plug-in descriptor, ready for the existing loader's exact
    /// component-ID resolution. Installation lookup and vendor execution stay there.
    func descriptor(owner: String, slot: Int = 0, instanceID: String = UUID().uuidString.lowercased(),
                    bundlePath: String? = nil, enabled: Bool = true) throws -> [String: Any] {
        let identityCharacters = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_.")
        guard !owner.isEmpty, owner.utf8.count <= 256, !owner.utf8.contains(0),
              !instanceID.isEmpty, instanceID.utf8.count <= 128,
              instanceID.unicodeScalars.allSatisfy({ identityCharacters.contains($0) }),
              (0..<Self.insertSlotCount).contains(slot), isInstrument != true || slot == 0 else {
            throw Self.invalid("the external plug-in owner, instance identity, or slot is invalid.")
        }
        let document = try documentState(bundlePath: bundlePath)
        let encoded = try PropertyListSerialization.data(fromPropertyList: document, format: .binary, options: 0)
        let component = document["id"] as! String
        var metadata = dictionary; metadata["warnings"] = restorationWarnings
        var result: [String: Any] = ["instanceID": instanceID, "owner": owner,
                                    "kind": isInstrument == true ? "instrument" : "effect", "slot": slot,
                                    "component": component, "id": component, "format": "VST2",
                                    "name": pluginName, "state": encoded.base64EncodedString(),
                                    "bypass": !enabled, "maschinePluginState": metadata]
        if let bundlePath { result["bundlePath"] = bundlePath }
        return result
    }

    static func read(archive: MaschineGroupArchive, module: MaschineGroupArchive.Module,
                     maxStateBytes: Int = 32 * 1024 * 1024) throws -> Self {
        guard module.type == "PluginHost", module.bodyOffset >= 0,
              module.end >= module.bodyOffset, module.end <= archive.bytes.count else {
            throw invalid("the selected module is not a bounded PluginHost record.")
        }
        return try decode(body: Data(archive.bytes[module.bodyOffset..<module.end]), objectID: module.objectID,
                          modern: archive.modern, moduleVersion: module.version, maxStateBytes: maxStateBytes)
    }

    /// Exposed separately for deterministic generated fixtures. Version 1's order
    /// is Module, four Bool parameters, unique identity, display name, PluginChunk,
    /// preset index, then parameter mapping/window members (not interpreted here).
    static func decode(body: Data, objectID: Int, modern: Bool, moduleVersion: Int = 1,
                       maxStateBytes: Int = 32 * 1024 * 1024) throws -> Self {
        // Version 0 (Maschine 1.x) has the same member order; its four Bool parameters
        // use the three-ID legacy parameter form even where version 1 uses four IDs.
        guard moduleVersion == 0 || moduleVersion == 1 else { throw invalid("PluginHost version \(moduleVersion) has no verified state layout.") }
        let modern = modern && moduleVersion == 1
        guard maxStateBytes > 0, maxStateBytes <= 32 * 1024 * 1024,
              body.count <= 32 * 1024 * 1024, objectID > 0, objectID <= Int(UInt32.max) - 16 else {
            throw invalid("PluginHost object or state bounds are invalid.")
        }
        var reader = Reader(bytes: Array(body))
        var expected = objectID, flags: [Bool] = []
        for _ in 0..<4 {
            for _ in 0..<(modern ? 4 : 3) {
                expected += 1
                guard try reader.integer() == expected else { throw invalid("PluginHost parameter identities do not match their member order.") }
            }
            if modern, try reader.byte() != 0 { throw invalid("PluginHost parameter base tracking is unsupported.") }
            var values: [Int] = []
            for _ in 0..<5 {
                let value = try reader.integer()
                guard value <= 1 else { throw invalid("PluginHost Boolean parameter is invalid.") }
                values.append(value)
            }
            flags.append(values[0] != 0)
        }
        let uniqueID = try reader.string(maximum: 4096)
        let name = try reader.string(maximum: 4096)
        guard !uniqueID.isEmpty, !name.isEmpty else { throw invalid("PluginHost identity or display name is empty.") }
        // The first non-pointer PluginChunk introduces its non-tracked class,
        // version zero. Subsequent PluginHosts reuse that archive registration.
        if reader.remaining >= 2, reader.bytes[reader.position] == 0, reader.bytes[reader.position + 1] == 0 {
            reader.position += 2
        }
        let length = try reader.integer()
        guard length >= 12, length - 12 <= maxStateBytes, length <= reader.remaining else {
            throw invalid("PluginHost chunk is truncated or exceeds the state limit.")
        }
        let chunkEnd = reader.position + length
        let version = try reader.word(), type = try reader.word(), size = try reader.word()
        guard version == 1 else { throw invalid("NI plug-in chunk version \(version) is unsupported.") }
        guard type <= 7, size > 0, Int(size) == length - 12 else {
            throw invalid("NI plug-in chunk type or payload size is invalid.")
        }
        let payload = Data(reader.bytes[reader.position..<chunkEnd])
        reader.position = chunkEnd
        let preset = try reader.integer()
        let fields = uniqueID.split(separator: ":", maxSplits: 3, omittingEmptySubsequences: false).map(String.init)
        var vst2ID: UInt32?, instrument: Bool?
        if fields.count == 4, !fields[0].isEmpty {
            if fields[2] == "INSTR" { instrument = true }
            else if fields[2] == "FX" { instrument = false }
            if fields[1] == "VST", instrument != nil,
               !fields[3].isEmpty, fields[3].utf8.allSatisfy({ (48...57).contains($0) }),
               let value = UInt32(fields[3]), value != 0 {
                vst2ID = value
            }
        }
        return Self(uniquePluginID: uniqueID, pluginName: name, vst2UniqueID: vst2ID,
                    isInstrument: instrument, chunkVersion: version, chunkType: type,
                    payload: payload, presetIndex: UInt32(preset), flags: flags)
    }

    private struct Reader {
        let bytes: [UInt8]
        var position = 0
        var remaining: Int { bytes.count - position }
        mutating func byte() throws -> UInt8 {
            guard remaining > 0 else { throw invalid("truncated PluginHost record.") }
            defer { position += 1 }
            return bytes[position]
        }
        mutating func integer() throws -> Int {
            let width = Int(try byte())
            guard width <= 4, width <= remaining,
                  width == 0 || bytes[position + width - 1] != 0 else {
                throw invalid("non-canonical PluginHost archive integer.")
            }
            var result: UInt32 = 0
            for offset in 0..<width { result |= UInt32(bytes[position + offset]) << (offset * 8) }
            position += width
            return Int(result)
        }
        mutating func word() throws -> UInt32 {
            guard remaining >= 4 else { throw invalid("truncated plug-in chunk header.") }
            var result: UInt32 = 0
            for offset in 0..<4 { result |= UInt32(bytes[position + offset]) << (offset * 8) }
            position += 4
            return result
        }
        mutating func string(maximum: Int) throws -> String {
            let length = try integer()
            guard length <= maximum, length <= remaining else { throw invalid("PluginHost string exceeds its bounds.") }
            let data = bytes[position..<(position + length)]
            guard !data.contains(0), let result = String(bytes: data, encoding: .utf8) else {
                throw invalid("PluginHost string is not valid UTF-8.")
            }
            position += length
            return result
        }
    }
    private static func invalid(_ message: String) -> MaschineGroupArchive.Invalid {
        MaschineGroupArchive.Invalid(message: message)
    }
}

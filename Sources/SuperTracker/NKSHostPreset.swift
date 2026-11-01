import Foundation
import CoreFoundation

/// Selected-preset loader. Library scanning continues to use NKSMetadata and never reads PCHK.
/// NKS RIFF structure is observed in installed presets; Steinberg defines the separate VST3
/// identity/parameter/state interfaces. A matching identity alone does not prove state compatibility.
struct NKSHostPreset {
    struct PluginIdentity: Equatable {
        /// Canonical 32 uppercase hexadecimal digits, using macOS FUID word order.
        let vst3ClassID: String?
        let vst2Magic: UInt32?
        let name: String?
        let vendor: String?
        var dictionary: [String: Any] {
            var value: [String: Any] = ["format": vst3ClassID == nil ? "vst2" : "vst3"]
            if let vst3ClassID { value["vst3ClassID"] = vst3ClassID }
            if let vst2Magic { value["vst2Magic"] = vst2Magic }
            if let name { value["name"] = name }
            if let vendor { value["vendor"] = vendor }
            return value
        }
    }

    struct ControllerSlot {
        let slot: Int
        let name: String
        let section: String?
        /// NICA id. With legacy VST2-only identity this is not a VST3 ParamID.
        let parameterID: UInt32?
        var dictionary: [String: Any] {
            var value: [String: Any] = ["slot": slot, "name": name]
            if let section { value["section"] = section }
            if let parameterID { value["parameterID"] = parameterID }
            return value
        }
    }

    struct ControllerPage {
        let index: Int
        let name: String
        let slots: [ControllerSlot]
        var dictionary: [String: Any] { ["index": index, "name": name, "parameters": slots.map(\.dictionary)] }
    }

    let url: URL
    let identity: PluginIdentity
    let name: String
    /// Exact PCHK payload after its recognized little-endian version word. No guessed conversion,
    /// decompression, AU conversion, or speculative component/controller split is performed.
    let state: Data
    let stateVersion: UInt32
    let controllerPages: [ControllerPage]
    let warnings: [String]
    let metadata: [String: Any]

    /// JSON-safe summary deliberately excludes plug-in state. Hosts pass `state` as a binary stream.
    var dictionary: [String: Any] {
        ["path": url.path, "name": name, "identity": identity.dictionary,
         "stateVersion": stateVersion, "stateBytes": state.count,
         "stateEncoding": "nativePluginState", "controllerPages": controllerPages.map(\.dictionary),
         "warnings": warnings, "requiresHostStateValidation": true]
    }

    init(url: URL, maxStateBytes: Int = 256 * 1024 * 1024) throws {
        guard url.isFileURL else { throw Self.failure("Choose a local NKS preset.") }
        guard maxStateBytes > 0, maxStateBytes <= 512 * 1024 * 1024 else { throw Self.failure("Invalid NKS state memory limit.") }
        let before = try Self.signature(url)
        guard before.size >= 12, before.size <= 512 * 1024 * 1024 else { throw Self.failure("The NKS preset is truncated or exceeds the 512 MiB file limit.") }
        let metadata = try NKSMetadata(url: url).dictionary
        guard let plid = metadata["pluginID"] as? [String: Any] else { throw Self.failure("This preset has no readable plug-in identity (PLID).") }
        let versions = metadata["chunkVersions"] as? [String: UInt32] ?? [:]
        guard versions["PLID"] == 1 else { throw Self.failure("This preset uses an unrecognized PLID version. It can be browsed but cannot be loaded directly yet.") }
        var warnings = metadata["warnings"] as? [String] ?? []
        let identity = try Self.identity(plid)
        if identity.vst3ClassID == nil {
            warnings.append("This preset identifies a VST2 plug-in. It loads into the native VST2, or into the vendor's VST2-compatible VST3 release.")
        }
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        let header = try Self.read(handle, at: 0, count: 12)
        guard header.prefix(4) == Data("RIFF".utf8), ["NIKS", "NKSF", "NKSX"].contains(String(decoding: header[8..<12], as: UTF8.self)) else {
            throw Self.failure("This file is not a supported NKS RIFF container.")
        }
        let end = UInt64(Self.word(header, 4)) + 8
        guard end >= 12, end <= before.size else { throw Self.failure("The NKS container length is invalid.") }
        var position: UInt64 = 12
        var chunks = 0
        var state: Data?
        var version: UInt32 = 0
        var identities = 0
        while position < end {
            chunks += 1
            guard chunks <= 4096, end - position >= 8 else { throw Self.failure("The NKS chunk table is invalid or exceeds its limit.") }
            let chunk = try Self.read(handle, at: position, count: 8)
            let tag = String(decoding: chunk.prefix(4), as: UTF8.self)
            let length = UInt64(Self.word(chunk, 4))
            let start = position + 8
            guard length <= end - start, length + (length & 1) <= end - start else { throw Self.failure("The NKS \(tag) chunk exceeds the container bounds.") }
            if tag == "PLID" { identities += 1 }
            if tag == "PCHK" {
                guard state == nil else { throw Self.failure("The preset contains multiple plug-in states. Direct loading would be ambiguous.") }
                guard length > 4 else { throw Self.failure("The NKS preset has no saved plug-in state.") }
                guard length - 4 <= UInt64(maxStateBytes) else { throw Self.failure("The preset's plug-in state exceeds the configured memory limit.") }
                let prefix = try Self.read(handle, at: start, count: 4)
                version = Self.word(prefix, 0)
                guard version == 1 else { throw Self.failure("PCHK state version \(version) is not supported. The state was not sent to a plug-in.") }
                state = try Self.read(handle, at: start + 4, count: Int(length - 4))
            }
            position = start + length + (length & 1)
        }
        guard identities == 1 else { throw Self.failure("The preset contains multiple plug-in identities. Direct loading would be ambiguous.") }
        guard let state else { throw Self.failure("The NKS preset has no PCHK plug-in state.") }
        guard before == (try Self.signature(url)) else { throw Self.failure("The NKS preset changed while it was being read. Try loading it again.") }
        let pages: [ControllerPage]
        if versions["NICA"] == 1 {
            pages = Self.pages(metadata["controllerMappings"] as? [String: Any] ?? [:], warnings: &warnings)
        } else {
            pages = []
            if metadata["controllerMappings"] != nil { warnings.append("Controller mappings use an unrecognized NICA version and were not bound.") }
        }
        self.url = url.standardizedFileURL
        self.identity = identity
        self.name = metadata["name"] as? String ?? url.deletingPathExtension().lastPathComponent
        self.state = state
        self.stateVersion = version
        self.controllerPages = pages
        self.warnings = warnings
        self.metadata = metadata
    }

    /// Bind only to IDs actually exposed by the matching VST3 component after state restoration.
    /// Order is never interpreted as a parameter ID; missing and legacy mappings stay visibly unbound.
    func validatedMappings(vst3ClassID: String, parameterIDs: Set<UInt32>) -> [[String: Any]] {
        let identityMatches = matches(vst3ClassID: vst3ClassID)
        return controllerPages.map { page in
            var value = page.dictionary
            value["parameters"] = page.slots.map { slot -> [String: Any] in
                var result = slot.dictionary
                let mapped = identityMatches && slot.parameterID.map { parameterIDs.contains($0) } == true
                result["mapped"] = mapped
                result["mappingStatus"] = slot.parameterID == nil ? "unassigned" : !identityMatches ? "identityMismatch" : mapped ? "mapped" : "parameterUnavailable"
                return result
            }
            return value
        }
    }

    /// Exact VST3 identity, or the VST2-compatible class convention that vendors use
    /// when a VST3 release replaces a VST2 one ("VST" + magic + name, or the magic
    /// in the third word). Such classes accept their VST2 NKS state and parameter IDs.
    func matches(vst3ClassID: String) -> Bool {
        let wanted = vst3ClassID.uppercased()
        if let exact = identity.vst3ClassID { return exact == wanted }
        guard let magic = identity.vst2Magic else { return false }
        return NKSHostPreset.isVST2Compatible(classID: wanted, vst2Magic: magic)
    }

    /// VST2 NICA IDs are parameter indices of the identified plug-in.
    func validatedMappings(vst2UniqueID: UInt32, parameterIDs: Set<UInt32>) -> [[String: Any]] {
        let identityMatches = identity.vst2Magic == vst2UniqueID
        return controllerPages.map { page in
            var value = page.dictionary
            value["parameters"] = page.slots.map { slot -> [String: Any] in
                var result = slot.dictionary
                let mapped = identityMatches && slot.parameterID.map { parameterIDs.contains($0) } == true
                result["mapped"] = mapped
                result["mappingStatus"] = slot.parameterID == nil ? "unassigned" : !identityMatches ? "identityMismatch" : mapped ? "mapped" : "parameterUnavailable"
                return result
            }
            return value
        }
    }

    /// Steinberg's VST2 migration convention stores "VST" followed by the four-character
    /// VST2 ID in the class UID (NI, Waves, iZotope). Arturia stores it in the third word.
    static func isVST2Compatible(classID: String, vst2Magic: UInt32) -> Bool {
        let characters = Array(classID.uppercased())
        guard characters.count == 32 else { return false }
        let magic = String(format: "%08X", vst2Magic)
        let steinberg = String(characters[0..<6]) == "565354" && String(characters[6..<14]) == magic
        let arturia = String(characters[0..<8]) == "41727475" && String(characters[16..<24]) == magic
        return steinberg || arturia
    }

    private static func identity(_ fields: [String: Any]) throws -> PluginIdentity {
        var classID: String?
        if let value = fields["VST3.uid"] {
            guard let words = value as? [Any], words.count == 4 else { throw failure("The NKS VST3 class identity must contain four 32-bit words.") }
            let numbers = try words.map { word -> UInt32 in
                guard let number = uint32(word, allowSignedBits: true) else { throw failure("The NKS VST3 class identity contains an invalid word.") }
                return number
            }
            guard numbers.contains(where: { $0 != 0 }) else { throw failure("The NKS VST3 class identity is empty.") }
            classID = numbers.map { String(format: "%08X", $0) }.joined()
        }
        let magic = uint32(fields["VST.magic"], allowSignedBits: true).flatMap { $0 == 0 ? nil : $0 }
        guard classID != nil || magic != nil else { throw failure("This preset does not identify a supported VST2 or VST3 plug-in. It can still be browsed.") }
        return PluginIdentity(vst3ClassID: classID, vst2Magic: magic,
                              name: PayloadValue.text(fields["pluginName"]) ?? PayloadValue.text(fields["VST3.plugin_name"]),
                              vendor: PayloadValue.text(fields["pluginVendor"]))
    }

    private static func pages(_ fields: [String: Any], warnings: inout [String]) -> [ControllerPage] {
        guard let pages = fields["ni8"] as? [Any] else { return [] }
        if pages.count > 128 { warnings.append("Only the first 128 NKS controller pages are exposed.") }
        var badIDs = 0
        let result = pages.prefix(128).enumerated().compactMap { pageIndex, raw -> ControllerPage? in
            guard let slots = raw as? [Any] else { return nil }
            if slots.count > 8 { warnings.append("NKS page \(pageIndex + 1) contains more than eight slots; extra slots are not exposed.") }
            let controls = slots.prefix(8).enumerated().map { slot, raw -> ControllerSlot in
                let fields = raw as? [String: Any] ?? [:]
                var id = uint32(fields["id"])
                if id == UInt32.max { id = nil }
                if fields["id"] != nil && id == nil { badIDs += 1 }
                return ControllerSlot(slot: slot, name: PayloadValue.text(fields["name"]) ?? "Control \(slot + 1)", section: PayloadValue.text(fields["section"]), parameterID: id)
            }
            return ControllerPage(index: pageIndex, name: controls.compactMap(\.section).first ?? "Page \(pageIndex + 1)", slots: controls)
        }
        if badIDs > 0 { warnings.append("\(badIDs) invalid or unassigned controller IDs were left unbound.") }
        return result
    }

    private struct FileSignature: Equatable { let size: UInt64; let modified: Date; let inode: UInt64 }
    private static func signature(_ url: URL) throws -> FileSignature {
        let values = try FileManager.default.attributesOfItem(atPath: url.path)
        guard values[.type] as? FileAttributeType == .typeRegular,
              let size = values[.size] as? NSNumber, let modified = values[.modificationDate] as? Date else {
            throw failure("The NKS preset must be a readable regular file.")
        }
        return FileSignature(size: size.uint64Value, modified: modified, inode: (values[.systemFileNumber] as? NSNumber)?.uint64Value ?? 0)
    }
    private static func uint32(_ value: Any?, allowSignedBits: Bool = false) -> UInt32? {
        guard let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID() else { return nil }
        let raw = number.doubleValue
        guard raw.isFinite, raw.rounded(.towardZero) == raw else { return nil }
        if raw >= 0, raw <= Double(UInt32.max) { return UInt32(raw) }
        if allowSignedBits, raw >= Double(Int32.min), raw < 0 { return UInt32(bitPattern: Int32(raw)) }
        return nil
    }
    private static func read(_ handle: FileHandle, at offset: UInt64, count: Int) throws -> Data {
        try handle.seek(toOffset: offset)
        guard let data = try handle.read(upToCount: count), data.count == count else { throw failure("The NKS preset was truncated while reading.") }
        return data
    }
    private static func word(_ data: Data, _ index: Int) -> UInt32 {
        UInt32(data[index]) | UInt32(data[index + 1]) << 8 | UInt32(data[index + 2]) << 16 | UInt32(data[index + 3]) << 24
    }
    private static func failure(_ message: String) -> NSError { NSError(domain: "SuperTracker.NKSHostPreset", code: 1, userInfo: [NSLocalizedDescriptionKey: message]) }
}

import Foundation
import ImageIO
import SQLite3

/// Read-only NKS preset discovery. Plugin state is deliberately never read or applied.
/// The container layout is verified against local vendor presets; MessagePack follows
/// https://github.com/msgpack/msgpack/blob/master/spec.md.
struct NKSMetadata {
    let dictionary: [String: Any]

    var name: String { dictionary["name"] as? String ?? "Untitled preset" }
    var categories: [String] { dictionary["categories"] as? [String] ?? [] }
    var tags: [String] { dictionary["tags"] as? [String] ?? [] }
    var warnings: [String] { dictionary["warnings"] as? [String] ?? [] }
    var hasPluginState: Bool { dictionary["hasPluginState"] as? Bool ?? false }

    private static let metadataLimit = 8 * 1024 * 1024
    private static let limitations = [
        "Presets that identify an installed VST3 plug-in, or a VST2 plug-in available natively or through the vendor's compatible VST3, load directly with their mapped NKS control pages.",
        "NKS plug-in state is not an Audio Unit preset and is never applied to an Audio Unit.",
        "Komplete Kontrol hardware integration and Kontakt (.nki) instruments are not part of this host."
    ]

    init(url: URL) throws {
        guard url.isFileURL else { throw ReadError.invalid("An NKS preset must be a local file.") }
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        let fileSize = try handle.seekToEnd()
        guard fileSize >= 12 else { throw ReadError.invalid("The NKS RIFF header is truncated.") }
        let header = try Self.read(handle, at: 0, count: 12)
        guard String(bytes: header[0..<4], encoding: .ascii) == "RIFF" else {
            throw ReadError.invalid("This is not a RIFF NKS preset.")
        }
        let form = String(bytes: header[8..<12], encoding: .ascii) ?? ""
        guard ["NIKS", "NKSF", "NKSX"].contains(form) else {
            throw ReadError.invalid("Unsupported NKS RIFF form: \(form).")
        }
        let riffEnd = UInt64(Self.little32(header, at: 4)) + 8
        guard riffEnd >= 12, riffEnd <= fileSize else {
            throw ReadError.invalid("The NKS container length exceeds the file bounds.")
        }

        var parsed: [String: [String: Any]] = [:]
        var warnings: [String] = []
        var chunkNames: [String] = []
        var chunkVersions: [String: UInt32] = [:]
        var hasPluginState = false
        var metadataBytes = 0
        var offset: UInt64 = 12
        var chunkCount = 0
        if riffEnd < fileSize { warnings.append("Bytes after the declared RIFF container were ignored.") }

        while offset < riffEnd {
            chunkCount += 1
            guard chunkCount <= 4096 else { throw ReadError.invalid("Too many NKS chunks.") }
            guard riffEnd - offset >= 8 else { throw ReadError.invalid("An NKS chunk header is truncated.") }
            let chunkHeader = try Self.read(handle, at: offset, count: 8)
            let chunk = String(bytes: chunkHeader[0..<4], encoding: .ascii) ?? "????"
            let length = UInt64(Self.little32(chunkHeader, at: 4))
            let start = offset + 8
            guard length <= riffEnd - start else {
                throw ReadError.invalid("The \(chunk) chunk exceeds the NKS container bounds.")
            }
            let end = start + length
            let paddedEnd = end + (length & 1)
            guard paddedEnd <= riffEnd else { throw ReadError.invalid("The \(chunk) chunk is missing its RIFF padding.") }
            chunkNames.append(chunk)

            if chunk == "PCHK" {
                // A vendor state can be hundreds of megabytes. Seeking skips it entirely.
                hasPluginState = hasPluginState || length > 0
            } else if ["NISI", "NICA", "PLID"].contains(chunk) {
                if parsed[chunk] != nil {
                    warnings.append("Duplicate \(chunk) metadata was ignored.")
                } else if length > UInt64(Self.metadataLimit - metadataBytes) {
                    warnings.append("The \(chunk) chunk exceeds the 8 MiB metadata budget and was skipped.")
                } else {
                    metadataBytes += Int(length)
                    let bytes = try Self.read(handle, at: start, count: Int(length))
                    do {
                        var content = bytes
                        // NKS metadata normally starts with a little-endian version word.
                        if bytes.count >= 5, Self.little32(bytes, at: 0) <= 1024 {
                            let version = Self.little32(bytes, at: 0)
                            chunkVersions[chunk] = version
                            content = Array(bytes.dropFirst(4))
                            if version != 1 { warnings.append("\(chunk) uses metadata version \(version); fields are interpreted conservatively.") }
                        }
                        var decoder = MessagePack(bytes: content)
                        let value = try decoder.decode()
                        guard decoder.isAtEnd, let fields = value as? [String: Any] else {
                            throw ReadError.invalid("Metadata must contain one complete MessagePack map.")
                        }
                        parsed[chunk] = fields
                    } catch {
                        warnings.append("Could not read \(chunk): \(error.localizedDescription)")
                    }
                }
            }
            offset = paddedEnd
        }

        let info = parsed["NISI"] ?? [:]
        if parsed["NISI"] == nil { warnings.append("No readable NISI summary was found; the file name is used for browsing.") }
        var result: [String: Any] = [
            "name": Self.nonempty(info["name"]) ?? url.deletingPathExtension().lastPathComponent,
            "path": url.path,
            "format": "NKS",
            "categories": Self.categoryPaths(info["types"]),
            "tags": Self.unique(Self.strings(info["modes"]) + Self.strings(info["tags"]) + Self.strings(info["characters"]) + Self.strings(info["character"])),
            "banks": Self.unique(Self.strings(info["bankchain"])),
            "typePaths": Self.typePaths(info["types"]),
            "characters": Self.unique(Self.strings(info["characters"]) + Self.strings(info["character"]) + Self.strings(info["modes"])),
            "hasPluginState": hasPluginState,
            "warnings": warnings,
            "limitations": Self.limitations,
            "directAUPresetLoadingSupported": false,
            "chunks": chunkNames,
            "chunkVersions": chunkVersions,
            "rawMetadata": info
        ]
        for key in ["author", "vendor", "comment", "deviceType", "UUID"] {
            if let value = Self.nonempty(info[key]) { result[key] = value }
        }
        // NKS bank chains are positional: product, bank, sub-bank. Empty positions stay empty.
        let chain = (info["bankchain"] as? [Any] ?? []).map { Self.nonempty($0) ?? "" }
        for (index, key) in ["product", "bank", "subBank"].enumerated() where index < chain.count && !chain[index].isEmpty { result[key] = chain[index] }
        if let pluginID = parsed["PLID"] { result["pluginID"] = pluginID }
        if let mappings = parsed["NICA"] {
            result["controllerMappings"] = mappings
            result["controllerPages"] = Self.controllerPages(mappings)
        }
        if let preview = Self.preview(for: url) { result["previewPath"] = preview.path }
        dictionary = result
    }

    private static func read(_ handle: FileHandle, at offset: UInt64, count: Int) throws -> [UInt8] {
        guard count >= 0, count <= metadataLimit else { throw ReadError.invalid("NKS read exceeds the metadata limit.") }
        try handle.seek(toOffset: offset)
        guard let data = try handle.read(upToCount: count), data.count == count else {
            throw ReadError.invalid("The NKS file changed or was truncated while reading.")
        }
        return Array(data)
    }

    private static func little32(_ bytes: [UInt8], at i: Int) -> UInt32 {
        UInt32(bytes[i]) | UInt32(bytes[i + 1]) << 8 | UInt32(bytes[i + 2]) << 16 | UInt32(bytes[i + 3]) << 24
    }

    private static func nonempty(_ value: Any?) -> String? {
        guard let value = value as? String else { return nil }
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    private static func strings(_ value: Any?) -> [String] {
        if let string = nonempty(value) { return [string] }
        if let array = value as? [Any] { return array.flatMap { strings($0) } }
        return []
    }

    private static func unique(_ values: [String]) -> [String] {
        var seen = Set<String>()
        return values.filter { seen.insert($0.lowercased()).inserted }
    }

    private static func categoryPaths(_ value: Any?) -> [String] {
        guard let array = value as? [Any] else { return strings(value) }
        return unique(array.compactMap { item in
            let parts = strings(item)
            return parts.isEmpty ? nil : parts.joined(separator: " / ")
        })
    }

    /// Type paths are `[[Type], [Type, Sub-type]]`; each entry keeps at most its first two levels.
    static func typePaths(_ value: Any?) -> [[String]] {
        guard let array = value as? [Any] else { return strings(value).map { [$0] } }
        var seen = Set<String>()
        return array.compactMap { item -> [String]? in
            let parts = Array(strings(item).prefix(2))
            guard !parts.isEmpty, seen.insert(parts.joined(separator: "\u{1f}").lowercased()).inserted else { return nil }
            return parts
        }
    }

    private static func controllerPages(_ mappings: [String: Any]) -> [[String: Any]] {
        guard let pages = mappings["ni8"] as? [[Any]] else { return [] }
        return pages.enumerated().map { pageIndex, slots in
            var section: String?
            let parameters = slots.enumerated().compactMap { slotIndex, item -> [String: Any]? in
                guard let item = item as? [String: Any] else { return nil }
                if section == nil { section = nonempty(item["section"]) }
                var parameter = item
                parameter["slot"] = slotIndex
                parameter["name"] = nonempty(item["name"]) ?? "Parameter \(slotIndex + 1)"
                parameter["requiresPluginParameterMapping"] = true
                return parameter
            }
            return ["index": pageIndex, "name": section ?? "Page \(pageIndex + 1)", "parameters": parameters]
        }
    }

    private static func preview(for url: URL) -> URL? {
        let directory = url.deletingLastPathComponent()
        let previews = directory.appendingPathComponent(".previews", isDirectory: true)
        let stem = url.deletingPathExtension().lastPathComponent
        let candidates = [
            previews.appendingPathComponent(url.lastPathComponent + ".ogg"),
            previews.appendingPathComponent(stem + ".ogg"),
            previews.appendingPathComponent(url.lastPathComponent + ".wav"),
            previews.appendingPathComponent(stem + ".wav"),
            directory.appendingPathComponent(stem + ".ogg")
        ]
        return candidates.first { candidate in
            (try? candidate.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile) == true
        }
    }

    private enum ReadError: LocalizedError {
        case invalid(String)
        var errorDescription: String? { if case let .invalid(message) = self { return message }; return nil }
    }

    /// Bounded MessagePack subset with all standard scalar/container encodings. Binary and
    /// extension values are retained as JSON-safe descriptors, never executed or interpreted.
    private struct MessagePack {
        let bytes: [UInt8]
        var offset = 0
        var valuesRemaining = 100_000
        var isAtEnd: Bool { offset == bytes.count }

        mutating func decode(depth: Int = 0) throws -> Any {
            guard depth <= 32, valuesRemaining > 0 else { throw ReadError.invalid("MessagePack nesting or object budget exceeded.") }
            valuesRemaining -= 1
            let marker = try byte()
            switch marker {
            case 0x00...0x7f: return Int(marker)
            case 0x80...0x8f: return try map(Int(marker & 0x0f), depth: depth)
            case 0x90...0x9f: return try array(Int(marker & 0x0f), depth: depth)
            case 0xa0...0xbf: return try string(Int(marker & 0x1f))
            case 0xc0: return NSNull()
            case 0xc2: return false
            case 0xc3: return true
            case 0xc4: return try binary(Int(unsigned(1)))
            case 0xc5: return try binary(Int(unsigned(2)))
            case 0xc6: return try binary(Int(unsigned(4)))
            case 0xc7: return try ext(Int(unsigned(1)))
            case 0xc8: return try ext(Int(unsigned(2)))
            case 0xc9: return try ext(Int(unsigned(4)))
            case 0xca:
                let value = Double(Float(bitPattern: UInt32(try unsigned(4))))
                return value.isFinite ? value as Any : String(describing: value)
            case 0xcb:
                let value = Double(bitPattern: try unsigned(8))
                return value.isFinite ? value as Any : String(describing: value)
            case 0xcc: return Int(try unsigned(1))
            case 0xcd: return Int(try unsigned(2))
            case 0xce: return Int(try unsigned(4))
            case 0xcf:
                let value = try unsigned(8)
                return value <= 9_007_199_254_740_991 ? NSNumber(value: value) as Any : String(value)
            case 0xd0: return Int(Int8(bitPattern: try byte()))
            case 0xd1: return Int(Int16(bitPattern: UInt16(try unsigned(2))))
            case 0xd2: return Int(Int32(bitPattern: UInt32(try unsigned(4))))
            case 0xd3:
                let value = Int64(bitPattern: try unsigned(8))
                return (-9_007_199_254_740_991...9_007_199_254_740_991).contains(value) ? NSNumber(value: value) as Any : String(value)
            case 0xd4: return try ext(1)
            case 0xd5: return try ext(2)
            case 0xd6: return try ext(4)
            case 0xd7: return try ext(8)
            case 0xd8: return try ext(16)
            case 0xd9: return try string(Int(unsigned(1)))
            case 0xda: return try string(Int(unsigned(2)))
            case 0xdb: return try string(Int(unsigned(4)))
            case 0xdc: return try array(Int(unsigned(2)), depth: depth)
            case 0xdd: return try array(Int(unsigned(4)), depth: depth)
            case 0xde: return try map(Int(unsigned(2)), depth: depth)
            case 0xdf: return try map(Int(unsigned(4)), depth: depth)
            case 0xe0...0xff: return Int(Int8(bitPattern: marker))
            default: throw ReadError.invalid("Reserved MessagePack marker 0x\(String(marker, radix: 16)).")
            }
        }

        private mutating func byte() throws -> UInt8 {
            guard offset < bytes.count else { throw ReadError.invalid("MessagePack data is truncated.") }
            let value = bytes[offset]
            offset += 1
            return value
        }

        private mutating func unsigned(_ count: Int) throws -> UInt64 {
            var result: UInt64 = 0
            for _ in 0..<count { result = (result << 8) | UInt64(try byte()) }
            return result
        }

        private mutating func payload(_ length: Int) throws -> ArraySlice<UInt8> {
            guard length >= 0, length <= 1_048_576, length <= bytes.count - offset else {
                throw ReadError.invalid("MessagePack payload is truncated or exceeds the 1 MiB field limit.")
            }
            let result = bytes[offset..<(offset + length)]
            offset += length
            return result
        }

        private mutating func string(_ length: Int) throws -> String {
            guard let value = String(bytes: try payload(length), encoding: .utf8) else {
                throw ReadError.invalid("MessagePack string is not valid UTF-8.")
            }
            return value
        }

        private mutating func binary(_ length: Int) throws -> [String: Any] {
            ["encoding": "base64", "data": Data(try payload(length)).base64EncodedString()]
        }

        private mutating func ext(_ length: Int) throws -> [String: Any] {
            let type = Int(Int8(bitPattern: try byte()))
            return ["extensionType": type, "encoding": "base64", "data": Data(try payload(length)).base64EncodedString()]
        }

        private mutating func array(_ count: Int, depth: Int) throws -> [Any] {
            guard count <= 65_536, count <= bytes.count - offset, count <= valuesRemaining else {
                throw ReadError.invalid("MessagePack array exceeds its data or object budget.")
            }
            var result: [Any] = []
            result.reserveCapacity(count)
            for _ in 0..<count { result.append(try decode(depth: depth + 1)) }
            return result
        }

        private mutating func map(_ count: Int, depth: Int) throws -> [String: Any] {
            guard count <= 32_768, count <= (bytes.count - offset) / 2, count <= valuesRemaining / 2 else {
                throw ReadError.invalid("MessagePack map exceeds its data or object budget.")
            }
            var result: [String: Any] = [:]
            result.reserveCapacity(count)
            for _ in 0..<count {
                let keyValue = try decode(depth: depth + 1)
                let key: String
                if let text = keyValue as? String { key = text }
                else if let number = keyValue as? NSNumber { key = number.stringValue }
                else { throw ReadError.invalid("Unsupported MessagePack map key.") }
                guard result[key] == nil else { throw ReadError.invalid("Duplicate MessagePack map key.") }
                result[key] = try decode(depth: depth + 1)
            }
            return result
        }
    }

    /// Synthetic fixtures exercise RIFF bounds, padding, metadata extraction and malicious
    /// MessagePack lengths/depth without depending on the user's preset library.
    static func selfTest() throws -> [String] {
        let temporary = FileManager.default.temporaryDirectory.appendingPathComponent("SuperTracker-NKS-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: temporary, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: temporary) }
        func check(_ passed: Bool, _ message: String) throws {
            if !passed { throw ReadError.invalid("NKS self-test failed: \(message)") }
        }
        func word(_ value: UInt32) -> [UInt8] {
            [UInt8(truncatingIfNeeded: value), UInt8(truncatingIfNeeded: value >> 8), UInt8(truncatingIfNeeded: value >> 16), UInt8(truncatingIfNeeded: value >> 24)]
        }
        func str(_ value: String) -> [UInt8] {
            let bytes = Array(value.utf8)
            return bytes.count < 32 ? [0xa0 | UInt8(bytes.count)] + bytes : [0xd9, UInt8(bytes.count)] + bytes
        }
        func chunk(_ name: String, _ content: [UInt8]) -> [UInt8] {
            Array(name.utf8) + word(UInt32(content.count)) + content + (content.count % 2 == 1 ? [0] : [])
        }
        func riff(_ chunks: [UInt8]) -> [UInt8] { Array("RIFF".utf8) + word(UInt32(chunks.count + 4)) + Array("NIKS".utf8) + chunks }
        func write(_ bytes: [UInt8], name: String) throws -> URL {
            let url = temporary.appendingPathComponent(name)
            try Data(bytes).write(to: url)
            return url
        }
        var summary: [UInt8] = [0x86]
        summary += str("name") + str("Night Keys")
        summary += str("author") + str("Test Author")
        summary += str("vendor") + str("Test Vendor")
        summary += str("types") + [0x91, 0x92] + str("Keys") + str("Electric Piano")
        summary += str("modes") + [0x92] + str("Warm") + str("Soft")
        summary += str("bankchain") + [0x91] + str("Factory")
        let control = [UInt8(0x81)] + str("ni8") + [0x91, 0x91, 0x83] + str("id") + [0x2a] + str("name") + str("Tone") + str("section") + str("Main")
        let plugin = [UInt8(0x81)] + str("VST.magic") + [0xce, 0x4b, 0x61, 0x74, 0x31]
        let fixture = riff(chunk("NISI", word(1) + summary) + chunk("NICA", word(1) + control) + chunk("PLID", word(1) + plugin) + chunk("PCHK", [1, 2, 3]))
        let url = try write(fixture, name: "Fixture.nksf")
        let metadata = try NKSMetadata(url: url)
        try check(metadata.name == "Night Keys", "name extraction")
        try check(metadata.categories == ["Keys / Electric Piano"], "hierarchical categories")
        try check(metadata.tags == ["Warm", "Soft"], "tags")
        try check(metadata.hasPluginState, "state presence without decoding")
        try check((metadata.dictionary["controllerPages"] as? [[String: Any]])?.count == 1, "controller pages")
        try check(JSONSerialization.isValidJSONObject(metadata.dictionary), "JSON serialization")
        let raw = try write(riff(chunk("NISI", summary)), name: "Raw.nksfx")
        try check(try NKSMetadata(url: raw).name == "Night Keys", "unprefixed map")

        var badLength = fixture
        badLength.replaceSubrange(16..<20, with: word(UInt32.max))
        let malformed: [[UInt8]] = [[], Array("not a preset".utf8), Array(fixture.dropLast()), badLength]
        for (index, bytes) in malformed.enumerated() {
            let invalid = try write(bytes, name: "Malformed-\(index).nksf")
            var failed = false
            do { _ = try NKSMetadata(url: invalid) } catch { failed = true }
            try check(failed, "malformed RIFF \(index)")
        }
        let invalidValues: [[UInt8]] = [[0xc1], [0xdb, 0xff, 0xff, 0xff, 0xff], [0xdd, 0xff, 0xff, 0xff, 0xff], Array(repeating: 0x91, count: 40) + [0], [0x81, 0xa1, 0x61], [0xa1, 0xff]]
        for (index, bytes) in invalidValues.enumerated() {
            var decoder = MessagePack(bytes: bytes)
            var failed = false
            do { _ = try decoder.decode() } catch { failed = true }
            try check(failed, "malformed MessagePack \(index)")
        }
        // Catalogue: product/bank/type/sub-type/character browsing, previews, effects and cache.
        func array(_ values: [[UInt8]]) -> [UInt8] { [0x90 | UInt8(values.count)] + values.flatMap { $0 } }
        func preset(_ name: String, product: String, bank: String, device: String, types: [[String]], characters: [String]) -> [UInt8] {
            var map: [UInt8] = [0x86]
            map += str("name") + str(name) + str("vendor") + str("Fixture Audio") + str("deviceType") + str(device)
            map += str("bankchain") + array([str(product), str(bank), str("")])
            map += str("types") + array(types.map { array($0.map(str)) }) + str("characters") + array(characters.map(str))
            return riff(chunk("NISI", word(1) + map) + chunk("PLID", word(1) + plugin) + chunk("PCHK", [1]))
        }
        let library = temporary.appendingPathComponent("Library/Synth One/Presets", isDirectory: true)
        try FileManager.default.createDirectory(at: library.appendingPathComponent(".previews"), withIntermediateDirectories: true)
        try Data(preset("Alpha Pad", product: "Synth One", bank: "Bank A", device: "INST", types: [["Synth Pad"], ["Synth Pad", "Warm"]], characters: ["Warm", "Analog"])).write(to: library.appendingPathComponent("Alpha.nksf"))
        try Data(preset("Beta Bass", product: "Synth One", bank: "Bank B", device: "INST", types: [["Bass"]], characters: ["Analog"])).write(to: library.appendingPathComponent("Beta.nksf"))
        try Data(preset("Tape Echo", product: "Echo FX", bank: "", device: "FX", types: [["Delay", "Tape"]], characters: [])).write(to: library.appendingPathComponent("Echo.nksfx"))
        try Data([0]).write(to: library.appendingPathComponent(".previews/Alpha.nksf.ogg"))
        let cache = temporary.appendingPathComponent("Cache", isDirectory: true)
        let catalog = NKSCatalog(cacheDirectory: cache, discoverSystemContent: false, roots: [temporary.appendingPathComponent("Library").path])
        catalog.refreshNow()
        func browse(_ payload: [String: Any], favorites: Set<String> = [], in value: NKSCatalog = catalog) -> [String: Any] { value.browse(payload, favorites: favorites) }
        func names(_ result: [String: Any]) -> [String] { (result["items"] as? [[String: Any]] ?? []).compactMap { $0["name"] as? String } }
        func facet(_ result: [String: Any], _ key: String) -> [String] { (result[key] as? [[String: Any]] ?? []).compactMap { ($0["label"] ?? $0["name"]) as? String } }
        let instruments = browse(["kind": "instrument"])
        try check(names(instruments) == ["Alpha Pad", "Beta Bass"], "catalogue instruments")
        try check(facet(instruments, "products") == ["Synth One"] && (instruments["counts"] as? [String: Int])?["effect"] == 1, "catalogue products and kind counts")
        try check((instruments["items"] as? [[String: Any]])?.first?["previewPath"] as? String == library.appendingPathComponent(".previews/Alpha.nksf.ogg").path, "catalogue preview")
        try check(names(browse(["kind": "effect"])) == ["Tape Echo"] && facet(browse(["kind": "effect"]), "subtypes") == ["Tape"], "catalogue effects and sub-types")
        let product = browse(["kind": "instrument", "product": "Synth One"])
        try check(facet(product, "banks") == ["Bank A", "Bank B"], "catalogue banks")
        try check(names(browse(["kind": "instrument", "product": "Synth One", "bank": "Bank B"])) == ["Beta Bass"], "catalogue bank filter")
        let typed = browse(["kind": "instrument", "types": ["Bass"]])
        try check(names(typed) == ["Beta Bass"] && facet(typed, "types") == ["Bass", "Synth Pad"], "catalogue type filter keeps sibling types")
        try check(names(browse(["kind": "instrument", "characters": ["Warm", "Analog"]])) == ["Alpha Pad"], "catalogue characters match all")
        try check(names(browse(["kind": "instrument", "query": "beta"])) == ["Beta Bass"], "catalogue text search")
        try check(names(browse(["kind": "instrument", "favorites": true], favorites: [library.appendingPathComponent("Beta.nksf").path])) == ["Beta Bass"], "catalogue favorites")
        try FileManager.default.removeItem(at: library.appendingPathComponent("Beta.nksf"))
        catalog.refreshNow()
        try check(names(browse(["kind": "instrument"])) == ["Alpha Pad"], "catalogue removes deleted presets")
        let reopened = NKSCatalog(cacheDirectory: cache, discoverSystemContent: false, roots: [temporary.appendingPathComponent("Offline").path])
        try check(names(reopened.browse(["kind": "instrument"], favorites: [])) == ["Alpha Pad"], "catalogue cache reload")
        try FileManager.default.removeItem(at: library.appendingPathComponent(".previews/Alpha.nksf.ogg"))
        catalog.refreshNow()
        try check((browse(["kind": "instrument"])["items"] as? [[String: Any]])?.first?["previewPath"] == nil, "catalogue clears removed preview")
        let contributed = NKSCatalog(cacheDirectory: temporary.appendingPathComponent("ContributedCache"), discoverSystemContent: false)
        let root = temporary.appendingPathComponent("Library"), disconnected = temporary.appendingPathComponent("Disconnected")
        contributed.extraRoots = { [root.path] }
        contributed.extraPresets = { [library.appendingPathComponent("Alpha.nksf").path, library.appendingPathComponent("Echo.nksfx").path] }
        contributed.refreshNow()
        try check(names(browse(["kind": "instrument"], in: contributed)) == ["Alpha Pad"], "catalogue contributed library preset")
        try FileManager.default.moveItem(at: root, to: disconnected)
        contributed.refreshNow()
        let disconnectedResult = browse(["kind": "instrument"], in: contributed)
        try check(names(disconnectedResult) == ["Alpha Pad"] && (disconnectedResult["items"] as? [[String: Any]])?.first?["available"] as? Bool == false, "catalogue preserves offline contributed presets and reports availability")
        try check(contributed.status["error"] != nil, "catalogue reports unavailable root")
        try FileManager.default.moveItem(at: disconnected, to: root)
        contributed.refreshNow()
        try check((browse(["kind": "instrument"], in: contributed)["items"] as? [[String: Any]])?.first?["available"] as? Bool != false && contributed.status["error"] == nil, "catalogue reconnect restores availability and clears diagnostic")
        try FileManager.default.removeItem(at: library.appendingPathComponent("Alpha.nksf"))
        contributed.refreshNow()
        try check(names(browse(["kind": "instrument"], in: contributed)).isEmpty, "catalogue removes deleted contributed preset from online root")
        try check(NKSCatalog.posixPath(fromHFS: "Sound Drive:Arturia:DX7 V:", bootVolume: "Macintosh HD") == "/Volumes/Sound Drive/Arturia/DX7 V" &&
                  NKSCatalog.posixPath(fromHFS: "Macintosh HD:Library:Massive X:", bootVolume: "Macintosh HD") == "/Library/Massive X" &&
                  NKSCatalog.posixPath(fromHFS: "/:Applications:Waves:", bootVolume: "Macintosh HD") == "/Applications/Waves", "content registration paths")
        return ["NKS: RIFF bounds and odd-byte padding", "NKS: summary, tags, banks, plugin IDs and controller pages", "NKS: version-prefixed and unprefixed MessagePack", "NKS: JSON-safe output and opaque plugin-state detection", "NKS: malformed lengths, UTF-8 and nesting rejected", "NKS catalogue: products, banks, types, characters, previews, effects, search, favorites and cache", "NKS catalogue: deleted previews, contributed offline roots, diagnostics, reconnect and online deletion"]
    }
}

// MARK: - Preset catalogue

extension NKSMetadata {
    /// Browsing summary for the preset catalogue: NISI and PLID only. NICA and PCHK
    /// payloads are skipped by seeking, so thousands of presets index quickly.
    static func catalogSummary(url: URL) throws -> [String: Any] {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        let fileSize = try handle.seekToEnd()
        guard fileSize >= 12 else { throw ReadError.invalid("The NKS RIFF header is truncated.") }
        let header = try read(handle, at: 0, count: 12)
        guard String(bytes: header[0..<4], encoding: .ascii) == "RIFF",
              ["NIKS", "NKSF", "NKSX"].contains(String(bytes: header[8..<12], encoding: .ascii) ?? "") else {
            throw ReadError.invalid("This is not an NKS preset.")
        }
        let riffEnd = min(fileSize, UInt64(little32(header, at: 4)) + 8)
        var offset: UInt64 = 12, chunks = 0, hasControls = false
        var info: [String: Any]?, plugin: [String: Any]?
        while offset + 8 <= riffEnd, chunks < 4096, info == nil || plugin == nil || !hasControls {
            chunks += 1
            let chunkHeader = try read(handle, at: offset, count: 8)
            let chunk = String(bytes: chunkHeader[0..<4], encoding: .ascii) ?? "????"
            let length = UInt64(little32(chunkHeader, at: 4)), start = offset + 8
            guard length <= riffEnd - start else { break }
            if chunk == "NICA" { hasControls = length > 0 }
            else if chunk == "NISI" || chunk == "PLID", length <= UInt64(metadataLimit) {
                let bytes = try read(handle, at: start, count: Int(length))
                var content = bytes
                if bytes.count >= 5, little32(bytes, at: 0) <= 1024 { content = Array(bytes.dropFirst(4)) }
                var decoder = MessagePack(bytes: content)
                if let fields = (try? decoder.decode()) as? [String: Any] {
                    if chunk == "NISI" { info = fields } else { plugin = fields }
                }
            }
            offset = start + length + (length & 1)
        }
        guard let info else { throw ReadError.invalid("No readable NISI summary.") }
        let chain = (info["bankchain"] as? [Any] ?? []).map { nonempty($0) ?? "" }
        let keys = (plugin ?? [:]).keys.map { $0.lowercased() }
        var result: [String: Any] = [
            "name": nonempty(info["name"]) ?? url.deletingPathExtension().lastPathComponent,
            "product": chain.first ?? "", "bank": chain.count > 1 ? chain[1] : "", "subBank": chain.count > 2 ? chain[2] : "",
            "typePaths": typePaths(info["types"]),
            "characters": unique(strings(info["characters"]) + strings(info["character"]) + strings(info["modes"])),
            "hasControls": hasControls,
            "format": keys.contains { $0.hasPrefix("vst3") } ? "VST3" : keys.contains { $0.hasPrefix("vst") } ? "VST2" : keys.contains { $0.hasPrefix("au") } ? "AU" : ""
        ]
        for key in ["vendor", "author", "comment", "deviceType"] { result[key] = nonempty(info[key]) ?? "" }
        return result
    }
}

/// Maschine / Komplete Kontrol style NKS preset catalogue: product, bank, type, sub-type and
/// character browsing over every installed preset. Discovery follows the public content
/// registrations (`ContentDir` in /Library/Preferences/com.native-instruments.*.plist, with
/// product names from Service Center XML) plus NKS presets already found in library folders.
/// Only NISI/PLID metadata is read; plug-in state is never touched while indexing.
final class NKSCatalog: @unchecked Sendable {
    struct Entry: Codable {
        var path: String
        var size: Int64
        var modified: Double
        var name: String
        var vendor: String
        var product: String
        var bank: String
        var subBank: String
        var effect: Bool
        var types: [String]
        var subtypes: [String]
        var characters: [String]
        var author: String
        var comment: String
        var preview: String?
        var format: String
        var hasControls: Bool
        // Derived search fields are rebuilt after decoding.
        var search = ""
        var foldedName = ""
        private enum CodingKeys: String, CodingKey {
            case path, size, modified, name, vendor, product, bank, subBank, effect, types, subtypes, characters, author, comment, preview, format, hasControls
        }
        mutating func prepare() {
            foldedName = NKSCatalog.fold(name)
            search = NKSCatalog.fold(([name, product, vendor, bank, subBank, author, comment] + types + subtypes.map { $0.replacingOccurrences(of: NKSCatalog.subtypeSeparator, with: " ") } + characters).joined(separator: " "))
        }
    }

    struct Registration {
        let name: String
        let company: String
        let contentDir: String
        let effect: Bool?
    }

    private struct CacheFile: Codable {
        var version = 1
        var entries: [Entry]
    }

    private let lock = NSLock()
    private let loadLock = NSLock()
    private let work = DispatchQueue(label: "SuperTracker.nks.catalog", qos: .utility)
    private let browseQueue = DispatchQueue(label: "SuperTracker.nks.browse", qos: .userInitiated)
    private let cacheURL: URL
    private let discoverSystemContent: Bool
    private let fixedRoots: [String]
    private var entries: [String: Entry] = [:]
    private var revision: UInt64 = 0
    private var loaded = false
    private var activated = false
    private var indexing = false
    private var refreshRequested = false
    private var indexDone = 0
    private var indexTotal = 0
    private var lastError: String?
    private var lastRootCount = 0
    /// Additional preset files (for example NKS presets indexed by library folders).
    var extraPresets: (() -> [String])?
    /// Registered library roots, used only to retain cached presets while a folder is unavailable.
    var extraRoots: (() -> [String])?
    var onEvent: ((String, [String: Any]) -> Void)?
    /// Product and expansion pictures from Native Access's shared resources.
    let artwork: NKSArtwork

    init(cacheDirectory: URL, discoverSystemContent: Bool = true, roots: [String] = [], artworkRoot: URL? = nil) {
        cacheURL = cacheDirectory.appendingPathComponent("nks-catalog-v1.json")
        self.discoverSystemContent = discoverSystemContent
        // Fixture catalogues never read the machine's installed artwork unless given a root.
        artwork = NKSArtwork(root: artworkRoot ?? (discoverSystemContent ? NKSArtwork.defaultRoot : cacheDirectory.appendingPathComponent("no-artwork")),
                             cacheDirectory: cacheDirectory.appendingPathComponent("nks-artwork-v1", isDirectory: true))
        fixedRoots = roots.map { URL(fileURLWithPath: ($0 as NSString).expandingTildeInPath).standardizedFileURL.path }
    }

    var isActive: Bool { locked { activated } }

    func handleBrowse(_ payload: [String: Any], favorites: Set<String>) async -> [String: Any] {
        await withCheckedContinuation { continuation in
            browseQueue.async { continuation.resume(returning: self.browse(payload, favorites: favorites)) }
        }
    }

    /// Starts an incremental background refresh. A request during indexing runs once more afterwards.
    func refresh() {
        let start = locked { () -> Bool in
            activated = true
            if indexing { refreshRequested = true; return false }
            indexing = true; indexDone = 0; indexTotal = 0
            return true
        }
        guard start else { return }
        emitStatus()
        work.async { [self] in runRefresh() }
    }

    /// Synchronous refresh for fixtures.
    func refreshNow() {
        locked { activated = true; indexing = true; indexDone = 0; indexTotal = 0 }
        work.sync { runRefresh(chain: false) }
    }

    var status: [String: Any] {
        locked {
            var value: [String: Any] = ["indexing": indexing, "done": indexDone, "total": indexTotal, "count": entries.count, "revision": revision, "roots": lastRootCount]
            if let lastError { value["error"] = lastError }
            return value
        }
    }

    // MARK: Indexing

    private func loadCacheIfNeeded() {
        loadLock.lock(); defer { loadLock.unlock() }
        if locked({ loaded }) { return }
        var decoded: [String: Entry] = [:]
        if let data = try? Data(contentsOf: cacheURL), data.count <= 256 * 1024 * 1024,
           let file = try? JSONDecoder().decode(CacheFile.self, from: data), file.version == 1 {
            for var entry in file.entries { entry.prepare(); decoded[entry.path] = entry }
        }
        locked { if !loaded { entries = decoded; loaded = true; revision &+= 1 } }
    }

    private func persist() {
        let snapshot = locked { Array(entries.values) }
        do {
            try FileManager.default.createDirectory(at: cacheURL.deletingLastPathComponent(), withIntermediateDirectories: true)
            try JSONEncoder().encode(CacheFile(entries: snapshot)).write(to: cacheURL, options: .atomic)
        } catch { locked { lastError = "Could not save the NKS catalogue cache: \(error.localizedDescription)" } }
    }

    private func runRefresh(chain: Bool = true) {
        loadCacheIfNeeded()
        let registrations = discoverSystemContent ? Self.discoverRegistrations() : []
        var roots = fixedRoots
        if discoverSystemContent {
            roots += registrations.map(\.contentDir)
            roots.append(FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Documents/Native Instruments/User Content").path)
        }
        // Scan nested roots once.
        var scanRoots: [String] = []
        for root in Set(roots).sorted(by: { $0.count < $1.count }) where !scanRoots.contains(where: { Self.isInside(root, $0) }) { scanRoots.append(root) }
        var found: [String: (size: Int64, modified: Double)] = [:]
        var previews: [String: Set<String>] = [:]
        var unavailable: [String] = []
        var scanErrors: [String] = []
        func recordUnavailable(_ root: String, _ message: String) {
            if !unavailable.contains(root) { unavailable.append(root) }
            if scanErrors.count < 16 { scanErrors.append(message) }
        }
        // Library files arrive through extraPresets, including cached paths on disconnected drives.
        // Do not recursively scan those roots again, or discard their cache while absent.
        for root in extraRoots?() ?? [] {
            var directory: ObjCBool = false
            if !FileManager.default.fileExists(atPath: root, isDirectory: &directory) || !directory.boolValue || !FileManager.default.isReadableFile(atPath: root) {
                recordUnavailable(root, "Library folder unavailable: \(root)")
            }
        }
        let keys: [URLResourceKey] = [.isDirectoryKey, .isRegularFileKey, .fileSizeKey, .contentModificationDateKey, .isSymbolicLinkKey]
        for root in scanRoots {
            var directory: ObjCBool = false
            guard FileManager.default.fileExists(atPath: root, isDirectory: &directory), directory.boolValue,
                  let enumerator = FileManager.default.enumerator(at: URL(fileURLWithPath: root, isDirectory: true), includingPropertiesForKeys: keys, options: [.skipsHiddenFiles], errorHandler: { url, error in
                      recordUnavailable(root, "Could not read \(url.path): \(error.localizedDescription)"); return true
                  }) else {
                recordUnavailable(root, "NKS folder unavailable: \(root)"); continue
            }
            var visited = 0
            while let url = enumerator.nextObject() as? URL {
                visited += 1
                if visited > 600_000 { recordUnavailable(root, "NKS folder scan limit reached: \(root). Previously indexed presets have been retained."); break }
                if visited % 512 == 0 { Thread.sleep(forTimeInterval: 0.002) }
                autoreleasepool {
                    guard let values = try? url.resourceValues(forKeys: Set(keys)) else { recordUnavailable(root, "Could not read NKS folder entry: \(url.path)"); return }
                    if values.isSymbolicLink == true { enumerator.skipDescendants(); return }
                    let ext = url.pathExtension.lowercased()
                    if values.isDirectory == true {
                        if ["app", "component", "vst", "vst3", "bundle", "framework", "aaxplugin"].contains(ext) || url.lastPathComponent.lowercased() == "samples" { enumerator.skipDescendants() }
                        return
                    }
                    guard values.isRegularFile == true, ext == "nksf" || ext == "nksfx", (values.fileSize ?? 0) > 0 else { return }
                    found[url.standardizedFileURL.path] = (Int64(values.fileSize ?? 0), values.contentModificationDate?.timeIntervalSince1970 ?? 0)
                }
            }
        }
        for path in extraPresets?() ?? [] where found[path] == nil && ["nksf", "nksfx"].contains((path as NSString).pathExtension.lowercased()) {
            guard let attributes = try? FileManager.default.attributesOfItem(atPath: path) else { continue }
            found[path] = ((attributes[.size] as? NSNumber)?.int64Value ?? 0, (attributes[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0)
        }
        func previewPath(_ path: String) -> String? {
            let directory = (path as NSString).deletingLastPathComponent
            let names: Set<String>
            if let cached = previews[directory] { names = cached }
            else {
                names = Set((try? FileManager.default.contentsOfDirectory(atPath: directory + "/.previews")) ?? [])
                previews[directory] = names
            }
            let file = (path as NSString).lastPathComponent, stem = (file as NSString).deletingPathExtension
            return [file + ".ogg", stem + ".ogg", file + ".wav", stem + ".wav"].first(where: names.contains).map { directory + "/.previews/" + $0 }
        }
        let snapshot = locked { entries }
        var pending: [String] = []
        var previewUpdates: [String: String?] = [:]
        for (path, signature) in found {
            if let entry = snapshot[path], entry.size == signature.size, entry.modified == signature.modified {
                let preview = previewPath(path)
                if preview != entry.preview { previewUpdates.updateValue(preview, forKey: path) }
            } else { pending.append(path) }
        }
        pending.sort()
        locked {
            for (path, preview) in previewUpdates { entries[path]?.preview = preview }
            if !previewUpdates.isEmpty { revision &+= 1 }
            indexTotal = pending.count; lastRootCount = scanRoots.count
            lastError = scanErrors.isEmpty ? nil : scanErrors.joined(separator: "\n")
        }
        emitStatus()
        var batch: [Entry] = []
        var lastCommit = Date()
        func commit() {
            guard !batch.isEmpty else { return }
            let count = batch.count
            locked { for entry in batch { entries[entry.path] = entry }; indexDone += count; revision &+= 1 }
            batch.removeAll(keepingCapacity: true); lastCommit = Date()
            emitStatus()
        }
        for (index, path) in pending.enumerated() {
            autoreleasepool {
                let signature = found[path]!
                if let entry = makeEntry(path: path, size: signature.size, modified: signature.modified, registrations: registrations, preview: previewPath(path)) { batch.append(entry) }
                else { locked { indexDone += 1 } }
            }
            if index % 128 == 127 { Thread.sleep(forTimeInterval: 0.001) }
            if batch.count >= 400 || Date().timeIntervalSince(lastCommit) > 0.5 { commit() }
        }
        commit()
        locked {
            // Keep presets on disconnected volumes so reconnecting restores them without a full reindex.
            let stale = entries.keys.filter { path in found[path] == nil && !unavailable.contains(where: { root in Self.isInside(path, root) }) }
            for path in stale { entries.removeValue(forKey: path) }
            if !stale.isEmpty { revision &+= 1 }
        }
        persist()
        let again = locked { () -> Bool in
            indexing = false
            let again = refreshRequested && chain
            refreshRequested = false
            return again
        }
        emitStatus()
        if again { refresh() }
    }

    private func makeEntry(path: String, size: Int64, modified: Double, registrations: [Registration], preview: String?) -> Entry? {
        guard let summary = try? NKSMetadata.catalogSummary(url: URL(fileURLWithPath: path)) else { return nil }
        let registration = registrations.filter { Self.isInside(path, $0.contentDir) }.max { $0.contentDir.count < $1.contentDir.count }
        let string = { (key: String) in summary[key] as? String ?? "" }
        let typePaths = summary["typePaths"] as? [[String]] ?? []
        let device = string("deviceType").uppercased()
        let ext = (path as NSString).pathExtension.lowercased()
        let effect = device == "FX" ? true : device == "INST" ? false : registration?.effect ?? (ext == "nksfx")
        let folder = URL(fileURLWithPath: path).deletingLastPathComponent().lastPathComponent
        // Vendors spell the same NKS type differently ("Piano/Keys", "Piano / Keys").
        let tidy = { (value: String) in value.replacingOccurrences(of: "\\s*/\\s*", with: " / ", options: .regularExpression) }
        var types: [String] = [], subtypes: [String] = []
        for parts in typePaths.map({ $0.map(tidy) }) {
            if let first = parts.first, !types.contains(first) { types.append(first) }
            if parts.count > 1 { let key = parts[0] + NKSCatalog.subtypeSeparator + parts[1]; if !subtypes.contains(key) { subtypes.append(key) } }
        }
        var entry = Entry(path: path, size: size, modified: modified, name: string("name"),
                          vendor: string("vendor").isEmpty ? registration?.company ?? "" : string("vendor"),
                          product: string("product").isEmpty ? registration?.name ?? folder : string("product"),
                          bank: string("bank"), subBank: string("subBank"), effect: effect, types: types, subtypes: subtypes,
                          characters: summary["characters"] as? [String] ?? [], author: string("author"), comment: string("comment"),
                          preview: preview, format: string("format"), hasControls: summary["hasControls"] as? Bool ?? false)
        entry.prepare()
        return entry
    }

    /// Reads public product registrations. Each preference names a content directory as an
    /// HFS path; the matching Service Center XML names the product, vendor and whether it is an effect.
    static func discoverRegistrations(preferences: [URL]? = nil, serviceCenter: URL = URL(fileURLWithPath: "/Library/Application Support/Native Instruments/Service Center")) -> [Registration] {
        let directories = preferences ?? [URL(fileURLWithPath: "/Library/Preferences"), FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Preferences")]
        let bootVolume = (try? URL(fileURLWithPath: "/").resourceValues(forKeys: [.volumeNameKey]).volumeName) ?? "Macintosh HD"
        var result: [String: Registration] = [:]
        for directory in directories {
            guard let names = try? FileManager.default.contentsOfDirectory(atPath: directory.path) else { continue }
            for name in names where name.hasPrefix("com.native-instruments.") && name.hasSuffix(".plist") {
                let key = String(name.dropFirst("com.native-instruments.".count).dropLast(".plist".count))
                guard let data = FileManager.default.contents(atPath: directory.appendingPathComponent(name).path), data.count < 1_048_576,
                      let plist = try? PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any],
                      let content = plist["ContentDir"] as? String, let path = posixPath(fromHFS: content, bootVolume: bootVolume) else { continue }
                var product = key, company = "", effect: Bool?
                if let xml = try? String(contentsOf: serviceCenter.appendingPathComponent(key + ".xml"), encoding: .utf8) {
                    product = xmlValue("Name", in: xml) ?? key
                    company = xmlValue("Company", in: xml) ?? ""
                    if let range = xml.range(of: "<PluginID fx=\"(true|false)\"", options: .regularExpression) { effect = xml[range].contains("true") }
                }
                if company.isEmpty, let dash = key.firstIndex(of: "-"), product == key { company = String(key[..<dash]) }
                let normalized = URL(fileURLWithPath: path).standardizedFileURL.path
                result[normalized] = Registration(name: product, company: company, contentDir: normalized, effect: effect)
            }
        }
        return result.values.sorted { $0.contentDir < $1.contentDir }
    }

    static func posixPath(fromHFS value: String, bootVolume: String) -> String? {
        if value.hasPrefix("/") && !value.hasPrefix("/:") { return value.contains(":") ? nil : value }
        var parts = value.split(separator: ":", omittingEmptySubsequences: false).map(String.init)
        while parts.last == "" { parts.removeLast() }
        guard let volume = parts.first, !volume.isEmpty, parts.count > 1 else { return nil }
        let rest = parts.dropFirst().joined(separator: "/")
        return (volume == "/" || volume == bootVolume ? "" : "/Volumes/" + volume) + "/" + rest
    }

    private static func xmlValue(_ tag: String, in xml: String) -> String? {
        guard let start = xml.range(of: "<\(tag)>"), let end = xml.range(of: "</\(tag)>", range: start.upperBound..<xml.endIndex) else { return nil }
        let value = String(xml[start.upperBound..<end.lowerBound]).trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: "&lt;", with: "<").replacingOccurrences(of: "&gt;", with: ">")
            .replacingOccurrences(of: "&quot;", with: "\"").replacingOccurrences(of: "&apos;", with: "'").replacingOccurrences(of: "&amp;", with: "&")
        return value.isEmpty ? nil : value
    }

    // MARK: Browsing

    func browse(_ payload: [String: Any], favorites: Set<String>) -> [String: Any] {
        loadCacheIfNeeded()
        if !locked({ activated }) { refresh() }
        let snapshot = locked { (Array(entries.values), revision) }
        let all = snapshot.0
        let effect = payload["kind"] as? String == "effect"
        let words = Self.fold(String((payload["query"] as? String ?? "").prefix(256))).split(whereSeparator: \.isWhitespace).map(String.init)
        let product = payload["product"] as? String
        let bank = payload["bank"] as? String
        let subBank = bank == nil ? nil : payload["subBank"] as? String
        let vendor = payload["vendor"] as? String
        func set(_ key: String) -> Set<String> { Set((payload[key] as? [String] ?? []).prefix(128)) }
        let types = set("types"), subtypes = set("subtypes"), characters = set("characters")
        let onlyFavorites = payload["favorites"] as? Bool ?? false
        let offset = max(0, (payload["offset"] as? NSNumber)?.intValue ?? 0)
        let limit = min(1000, max(1, (payload["limit"] as? NSNumber)?.intValue ?? 200))

        var kindCounts = [0, 0]
        var productCounts: [String: (count: Int, vendors: [String: Int])] = [:]
        var vendorCounts: [String: Int] = [:], subBankCounts: [String: Int] = [:]
        var bankCounts: [String: Int] = [:], typeCounts: [String: Int] = [:], subtypeCounts: [String: Int] = [:], characterCounts: [String: Int] = [:]
        var matches: [(entry: Entry, score: Int)] = []
        for entry in all {
            guard onlyFavorites ? favorites.contains(entry.path) : true, words.allSatisfy({ entry.search.contains($0) }) else { continue }
            // Vendor narrows the product list; a chosen product keeps its own vendor.
            let p = (product == nil || entry.product == product!) && (vendor == nil || entry.vendor == vendor!)
            let b = (bank == nil || entry.bank == bank!) && (subBank == nil || entry.subBank == subBank!)
            let t = types.isEmpty || entry.types.contains(where: types.contains)
            let s = subtypes.isEmpty || entry.subtypes.contains(where: subtypes.contains)
            let c = characters.isSubset(of: entry.characters)
            kindCounts[entry.effect ? 1 : 0] += 1 // Tab totals follow only search and favorites; product and tags are per kind.
            guard entry.effect == effect else { continue }
            if t && s && c { vendorCounts[entry.vendor, default: 0] += 1 }
            if t && s && c && (vendor == nil || entry.vendor == vendor!) { productCounts[entry.product, default: (0, [:])].count += 1; productCounts[entry.product]!.vendors[entry.vendor, default: 0] += 1 }
            if product != nil && p && t && s && c { bankCounts[entry.bank, default: 0] += 1 }
            if bank != nil && p && entry.bank == bank! && t && s && c && !entry.subBank.isEmpty { subBankCounts[entry.subBank, default: 0] += 1 }
            if p && b && c { for value in entry.types { typeCounts[value, default: 0] += 1 } }
            if p && b && t && c { for value in entry.subtypes where types.isEmpty || types.contains(where: { value.hasPrefix($0 + Self.subtypeSeparator) }) { subtypeCounts[value, default: 0] += 1 } }
            if p && b && t && s { for value in entry.characters where Self.visibleCharacter(value) { characterCounts[value, default: 0] += 1 } }
            guard p && b && t && s && c else { continue }
            let score = words.reduce(0) { $0 + (entry.foldedName.hasPrefix($1) ? 3 : entry.foldedName.contains($1) ? 2 : 0) }
            matches.append((entry, score))
        }
        matches.sort { left, right in
            if left.score != right.score { return left.score > right.score }
            if left.entry.foldedName != right.entry.foldedName { return left.entry.foldedName < right.entry.foldedName }
            return left.entry.path < right.entry.path
        }
        let page = matches.dropFirst(offset).prefix(limit).map { item -> [String: Any] in
            let entry = item.entry
            var value: [String: Any] = ["path": entry.path, "id": entry.path, "kind": "nks", "name": entry.name, "product": entry.product, "vendor": entry.vendor,
                "bank": entry.bank, "subBank": entry.subBank, "effect": entry.effect, "types": entry.types, "subtypes": entry.subtypes,
                "characters": entry.characters.filter(Self.visibleCharacter), "author": entry.author, "comment": entry.comment, "format": entry.format,
                "hasControls": entry.hasControls, "favorite": favorites.contains(entry.path), "extension": (entry.path as NSString).pathExtension.lowercased()]
            if let preview = entry.preview { value["previewPath"] = preview }
            let art = artwork.match(product: entry.product, vendor: entry.vendor, bank: entry.bank)
            if let key = art.bank ?? art.product { value["art"] = key }
            if let key = art.product { value["productArt"] = key }
            if let color = art.color { value["color"] = color }
            // Check only the bounded visible page, never stat the entire catalogue per keystroke.
            if !FileManager.default.isReadableFile(atPath: entry.path) {
                value["available"] = false
                let parts = entry.path.split(separator: "/")
                if parts.count > 1, parts[0] == "Volumes" { value["volume"] = String(parts[1]) }
            }
            return value
        }
        func facet(_ counts: [String: Int], byCount: Bool = false) -> [[String: Any]] {
            counts.sorted { byCount && $0.value != $1.value ? $0.value > $1.value : $0.key.localizedCaseInsensitiveCompare($1.key) == .orderedAscending }
                .prefix(1000).map { ["name": $0.key, "count": $0.value] }
        }
        let products = productCounts.sorted { $0.key.localizedCaseInsensitiveCompare($1.key) == .orderedAscending }.map { key, value -> [String: Any] in
            let vendor = value.vendors.max { $0.value < $1.value || ($0.value == $1.value && $0.key > $1.key) }?.key ?? ""
            var result: [String: Any] = ["name": key, "count": value.count, "vendor": vendor]
            let art = artwork.match(product: key, vendor: vendor)
            if let key = art.product { result["art"] = key }
            if let color = art.color { result["color"] = color }
            return result
        }
        let productVendor = product.flatMap { name in products.first { $0["name"] as? String == name }?["vendor"] as? String } ?? ""
        let banks = facet(bankCounts).map { value -> [String: Any] in
            guard let product, let name = value["name"] as? String, !name.isEmpty else { return value }
            let art = artwork.match(product: product, vendor: productVendor, bank: name)
            var result = value
            if let key = art.bank { result["art"] = key }
            if let color = art.color { result["color"] = color }
            return result
        }
        // Sub-types are keyed "Type<US>Sub-type"; the label omits the parent type.
        let subtypeFacet = facet(subtypeCounts).map { value -> [String: Any] in
            let parts = (value["name"] as? String ?? "").components(separatedBy: Self.subtypeSeparator)
            return value.merging(["type": parts.first ?? "", "label": parts.last ?? ""]) { first, _ in first }
        }
        var result: [String: Any] = ["items": Array(page), "total": matches.count, "offset": offset, "limit": limit,
            "products": products, "banks": banks, "subBanks": facet(subBankCounts), "vendors": facet(vendorCounts.filter { !$0.key.isEmpty }, byCount: true), "types": facet(typeCounts), "subtypes": subtypeFacet,
            "characters": facet(characterCounts, byCount: true), "counts": ["instrument": kindCounts[0], "effect": kindCounts[1]]]
        var snapshotStatus = status
        // A refresh can publish while this page is sorted. Its paging revision must describe its entries.
        snapshotStatus["revision"] = snapshot.1
        result["status"] = snapshotStatus
        return result
    }

    // MARK: Helpers

    static let subtypeSeparator = "\u{1f}"
    /// Some vendors ship internal selection markers ("_ SEL LEAD (ROUND 4)") as NKS modes.
    static func visibleCharacter(_ value: String) -> Bool { !value.hasPrefix("_") && !value.trimmingCharacters(in: .whitespaces).isEmpty }

    private func emitStatus() { onEvent?("library.nks.progress", status) }
    private func locked<T>(_ body: () throws -> T) rethrows -> T { lock.lock(); defer { lock.unlock() }; return try body() }
    static func fold(_ value: String) -> String { value.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: Locale(identifier: "en_US_POSIX")) }
    static func isInside(_ path: String, _ root: String) -> Bool { path == root || path.hasPrefix(root == "/" ? "/" : root + "/") }
}

// MARK: - Product and expansion artwork

/// Komplete Kontrol / Maschine artwork for NKS products and expansions, read from the shared
/// "NI Resources" folder that Native Access maintains:
///   image/<product>/                 NI products and expansions (MST_artwork, VB_artwork, logos, NKS2 banners)
///   image/<vendor>/<product>/        partner products (Arturia, Waves, iZotope, …)
///   image/<product>/<sub-library>/   sub-libraries (Maschine 2 Factory Selection)
///   _DatabaseResources/color/color.db  background colours keyed "product" or "product|bank"
///   dist_database/<vendor>/<product>/color.json  partner background colours
/// Each folder's .meta names the product exactly as presets spell it in their NISI bank chain.
/// Images leave this class only as bounded JPEG/PNG thumbnails (data URLs), keyed by a folder
/// that the index itself found, so the web view can never ask for an arbitrary file.
final class NKSArtwork: @unchecked Sendable {
    struct Match: Equatable { var product: String?; var bank: String?; var color: String? }

    static let defaultRoot = URL(fileURLWithPath: "/Users/Shared/NI Resources", isDirectory: true)
    static let imageNames = ["mst_artwork.png", "vb_artwork.png", "mst_logo.png", "vb_logo.png", "oso_logo.png",
                             "nks2_hardware_banner.webp", "nks2_hardware_banner.png", "nks2_software_tile.webp", "mst_plugin.png"]
    private static let variants: [String: (files: [String], size: Int, png: Bool)] = [
        "tile": (["mst_artwork.png", "vb_artwork.png", "nks2_software_tile.webp", "nks2_hardware_banner.webp", "nks2_hardware_banner.png"], 268, false),
        "banner": (["nks2_hardware_banner.webp", "nks2_hardware_banner.png", "nks2_software_tile.webp", "mst_artwork.png", "vb_artwork.png"], 960, false),
        "logo": (["vb_logo.png", "oso_logo.png", "mst_logo.png"], 360, true)
    ]

    private struct Index {
        var single: [String: String] = [:]   // folded name -> relative folder
        var pairs: [String: String] = [:]    // folded "parent<US>child" -> relative folder
        var vendors: [String: String] = [:]  // relative folder -> folded vendor from its .meta
        var folders: [String: [String: String]] = [:] // relative folder -> lowercased file name -> real name
        var colors: [String: String] = [:]   // folded location -> "#rrggbb"
    }

    let root: URL
    private let cacheDirectory: URL?
    private let lock = NSLock()
    private var index: Index?
    private var matches: [String: Match] = [:]
    private var images: [String: String] = [:]
    private var imageOrder: [String] = []

    init(root: URL = NKSArtwork.defaultRoot, cacheDirectory: URL? = nil) {
        self.root = root.standardizedFileURL
        self.cacheDirectory = cacheDirectory
    }

    static func fold(_ value: String) -> String {
        NKSCatalog.fold(value).replacingOccurrences(of: "\u{2019}", with: "'")
            .split(whereSeparator: \.isWhitespace).joined(separator: " ")
    }
    private static let separator = "\u{1f}"

    /// Artwork folders for a preset's bank chain. Bank art is used only for a real sub-library:
    /// a folder nested under the product, or an NI expansion folder for an NI product.
    func match(product: String, vendor: String, bank: String = "") -> Match {
        let cacheKey = [product, vendor, bank].joined(separator: Self.separator)
        if let cached = locked({ matches[cacheKey] }) { return cached }
        let index = loadIndex()
        let p = Self.fold(product), v = Self.fold(vendor), b = Self.fold(bank)
        var result = Match()
        if !p.isEmpty {
            result.product = index.pairs[v + Self.separator + p] ?? index.single[p]
            if !b.isEmpty {
                if let nested = index.pairs[p + Self.separator + b] { result.bank = nested }
                else if let folder = index.single[b], folder != result.product, v == "native instruments",
                        (index.vendors[folder] ?? "native instruments") == "native instruments" { result.bank = folder }
            }
            result.color = (b.isEmpty ? nil : index.colors[p + "|" + b]) ?? index.colors[p] ?? index.colors[v + Self.separator + p]
        }
        locked { if matches.count > 20_000 { matches.removeAll() }; matches[cacheKey] = result }
        return result
    }

    /// Thumbnails for known artwork folders: ["image/massive x": "data:image/jpeg;base64,…"].
    /// Unknown keys and folders without the requested picture are omitted.
    func thumbnails(keys: [String], variant: String) -> [String: String] {
        guard let spec = Self.variants[variant] else { return [:] }
        let index = loadIndex()
        var result: [String: String] = [:]
        for key in Set(keys.prefix(400)) {
            guard let files = index.folders[key] else { continue }
            let cacheKey = variant + "#" + key
            if let cached = locked({ images[cacheKey] }) { if !cached.isEmpty { result[key] = cached }; continue }
            var url: String? = nil
            for name in spec.files {
                guard let real = files[name] else { continue }
                if let value = thumbnail(root.appendingPathComponent(key).appendingPathComponent(real), size: spec.size, png: spec.png) { url = value; break }
            }
            locked {
                images[cacheKey] = url ?? ""; imageOrder.append(cacheKey)
                if imageOrder.count > 1200 { images.removeValue(forKey: imageOrder.removeFirst()) }
            }
            if let url { result[key] = url }
        }
        return result
    }

    private func thumbnail(_ url: URL, size: Int, png: Bool) -> String? {
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: url.path),
              let bytes = (attributes[.size] as? NSNumber)?.intValue, bytes > 0, bytes < 16 * 1024 * 1024 else { return nil }
        let modified = (attributes[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0
        let mime = png ? "image/png" : "image/jpeg"
        var hash: UInt64 = 0xcbf29ce484222325
        for byte in "\(url.path)|\(bytes)|\(modified)|\(size)|\(png)".utf8 { hash = (hash ^ UInt64(byte)) &* 0x100000001b3 }
        let cached = cacheDirectory?.appendingPathComponent(String(hash, radix: 16) + (png ? ".png" : ".jpg"))
        if let cached, let data = try? Data(contentsOf: cached), !data.isEmpty { return "data:\(mime);base64," + data.base64EncodedString() }
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
              let image = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                kCGImageSourceCreateThumbnailFromImageAlways: true, kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceThumbnailMaxPixelSize: size, kCGImageSourceShouldCacheImmediately: true] as CFDictionary) else { return nil }
        let data = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(data, (png ? "public.png" : "public.jpeg") as CFString, 1, nil) else { return nil }
        CGImageDestinationAddImage(destination, image, png ? nil : [kCGImageDestinationLossyCompressionQuality: 0.84] as CFDictionary)
        guard CGImageDestinationFinalize(destination), data.length > 0 else { return nil }
        if let cached, let directory = cacheDirectory {
            try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            try? (data as Data).write(to: cached, options: .atomic)
        }
        return "data:\(mime);base64," + (data as Data).base64EncodedString()
    }

    // MARK: Index

    private func loadIndex() -> Index {
        if let index = locked({ index }) { return index }
        let built = buildIndex()
        locked { if index == nil { index = built } }
        return locked { index! }
    }

    private func buildIndex() -> Index {
        var index = Index()
        let manager = FileManager.default
        let images = root.appendingPathComponent("image", isDirectory: true)
        func children(_ url: URL) -> [String] { ((try? manager.contentsOfDirectory(atPath: url.path)) ?? []).filter { !$0.hasPrefix(".") }.sorted() }
        func isDirectory(_ url: URL) -> Bool { var flag: ObjCBool = false; return manager.fileExists(atPath: url.path, isDirectory: &flag) && flag.boolValue }
        func meta(_ url: URL, _ names: [String]) -> (name: String?, vendor: String?) {
            guard let file = names.first(where: { $0.lowercased().hasSuffix(".meta") }),
                  let data = manager.contents(atPath: url.appendingPathComponent(file).path), data.count < 65_536,
                  let xml = String(data: data, encoding: .utf8) else { return (nil, nil) }
            func tag(_ name: String) -> String? {
                guard let start = xml.range(of: "<\(name)>"), let end = xml.range(of: "</\(name)>", range: start.upperBound..<xml.endIndex) else { return nil }
                let value = String(xml[start.upperBound..<end.lowerBound]).trimmingCharacters(in: .whitespacesAndNewlines)
                    .replacingOccurrences(of: "&amp;", with: "&").replacingOccurrences(of: "&apos;", with: "'")
                return value.isEmpty ? nil : value
            }
            return (tag("name"), tag("vendor"))
        }
        // Registers a folder that holds pictures. Returns false for grouping folders (vendors).
        func register(_ relative: String, url: URL, names: [String]) -> Bool {
            var files: [String: String] = [:]
            for name in names where Self.imageNames.contains(name.lowercased()) { files[name.lowercased()] = name }
            guard !files.isEmpty else { return false }
            index.folders[relative] = files
            return true
        }
        for top in children(images) {
            let url = images.appendingPathComponent(top, isDirectory: true)
            guard isDirectory(url) else { continue }
            let names = children(url), info = meta(url, names), relative = "image/" + top
            let hasImages = register(relative, url: url, names: names)
            if hasImages {
                for key in [top, info.name].compactMap({ $0 }).map(Self.fold) where index.single[key] == nil { index.single[key] = relative }
                if let vendor = info.vendor { index.vendors[relative] = Self.fold(vendor) }
            }
            // Nested folders: partner products under a vendor folder, or sub-libraries under a product.
            for child in names {
                let childURL = url.appendingPathComponent(child, isDirectory: true)
                guard isDirectory(childURL) else { continue }
                let childNames = children(childURL), childInfo = meta(childURL, childNames), childRelative = relative + "/" + child
                guard register(childRelative, url: childURL, names: childNames) else { continue }
                let parents = Set([top, info.name, childInfo.vendor].compactMap { $0 }.map(Self.fold))
                let selves = Set([child, childInfo.name].compactMap { $0 }.map(Self.fold))
                for parent in parents { for name in selves { index.pairs[parent + Self.separator + name] = childRelative } }
                if let vendor = childInfo.vendor { index.vendors[childRelative] = Self.fold(vendor) }
                if !hasImages { for name in selves where index.single[name] == nil { index.single[name] = childRelative } }
            }
        }
        // Background colours: NI products and expansions from color.db, partners from dist_database.
        var database: OpaquePointer?
        let colorDB = root.appendingPathComponent("_DatabaseResources/color/color.db").path
        if manager.fileExists(atPath: colorDB), sqlite3_open_v2(colorDB, &database, SQLITE_OPEN_READONLY, nil) == SQLITE_OK {
            var statement: OpaquePointer?
            if sqlite3_prepare_v2(database, "SELECT Location, Name, Value FROM color WHERE Name IN ('MST_bgcolor','VB_bgcolor') LIMIT 20000", -1, &statement, nil) == SQLITE_OK {
                while sqlite3_step(statement) == SQLITE_ROW {
                    guard let location = sqlite3_column_text(statement, 0), let name = sqlite3_column_text(statement, 1), let value = sqlite3_column_text(statement, 2) else { continue }
                    let key = String(cString: location).split(separator: "|").map { Self.fold(String($0)) }.joined(separator: "|")
                    if let color = Self.hexColor(String(cString: value)), index.colors[key] == nil || String(cString: name) == "MST_bgcolor" { index.colors[key] = color }
                }
            }
            sqlite3_finalize(statement)
        }
        sqlite3_close(database)
        let partners = root.appendingPathComponent("dist_database", isDirectory: true)
        for vendor in children(partners) {
            for product in children(partners.appendingPathComponent(vendor)) {
                guard let data = manager.contents(atPath: partners.appendingPathComponent(vendor).appendingPathComponent(product).appendingPathComponent("color.json").path), data.count < 4096,
                      let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                      let color = (json["MST_bgcolor"] ?? json["VB_bgcolor"]) as? String, let hex = Self.hexColor(color) else { continue }
                index.colors[Self.fold(vendor) + Self.separator + Self.fold(product)] = hex
            }
        }
        return index
    }

    /// Fixture NI Resources tree: product, NI expansion, partner vendor folder, colours, thumbnails,
    /// cache, unknown keys, plus catalogue vendor / sub-bank filters and artwork fields.
    static func selfTest() throws -> [String] {
        let manager = FileManager.default
        let temporary = manager.temporaryDirectory.appendingPathComponent("SuperTracker-NKSArt-\(UUID().uuidString)", isDirectory: true)
        defer { try? manager.removeItem(at: temporary) }
        func check(_ passed: Bool, _ message: String) throws {
            if !passed { throw NSError(domain: "SuperTracker.NKSArtwork", code: 1, userInfo: [NSLocalizedDescriptionKey: "NKS artwork self-test failed: \(message)"]) }
        }
        let resources = temporary.appendingPathComponent("NI Resources", isDirectory: true)
        func png(_ url: URL, width: Int, height: Int) throws {
            try manager.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            guard let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(),
                                          bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { throw CocoaError(.fileWriteUnknown) }
            context.setFillColor(red: 0.9, green: 0.4, blue: 0.1, alpha: 1); context.fill(CGRect(x: 0, y: 0, width: width, height: height))
            guard let image = context.makeImage(), let destination = CGImageDestinationCreateWithURL(url as CFURL, "public.png" as CFString, 1, nil) else { throw CocoaError(.fileWriteUnknown) }
            CGImageDestinationAddImage(destination, image, nil)
            guard CGImageDestinationFinalize(destination) else { throw CocoaError(.fileWriteUnknown) }
        }
        func meta(_ url: URL, name: String, vendor: String? = nil) throws {
            let xml = "<?xml version=\"1.0\"?>\n<resource>\n\(vendor.map { "<vendor>\($0)</vendor>\n" } ?? "")<name>\(name)</name>\n<type>image</type>\n</resource>\n"
            try Data(xml.utf8).write(to: url)
        }
        let image = resources.appendingPathComponent("image")
        try png(image.appendingPathComponent("synth one/MST_artwork.png"), width: 134, height: 66)
        try png(image.appendingPathComponent("synth one/VB_logo.png"), width: 227, height: 47)
        try meta(image.appendingPathComponent("synth one/synth one.meta"), name: "Synth One")
        try png(image.appendingPathComponent("expansion x/MST_artwork.png"), width: 134, height: 66)
        try png(image.appendingPathComponent("expansion x/NKS2_hardware_banner.png"), width: 1280, height: 212)
        try meta(image.appendingPathComponent("expansion x/expansion x.meta"), name: "Expansion X")
        try png(image.appendingPathComponent("acme/acme gizmo/VB_artwork.png"), width: 96, height: 47)
        try meta(image.appendingPathComponent("acme/acme gizmo/acme gizmo.meta"), name: "Acme Gizmo", vendor: "Acme")
        let partner = resources.appendingPathComponent("dist_database/acme/acme gizmo")
        try manager.createDirectory(at: partner, withIntermediateDirectories: true)
        try Data(#"{ "VB_bgcolor" : "112233" }"#.utf8).write(to: partner.appendingPathComponent("color.json"))
        let colorDirectory = resources.appendingPathComponent("_DatabaseResources/color")
        try manager.createDirectory(at: colorDirectory, withIntermediateDirectories: true)
        var database: OpaquePointer?
        try check(sqlite3_open(colorDirectory.appendingPathComponent("color.db").path, &database) == SQLITE_OK, "fixture colour database")
        sqlite3_exec(database, "CREATE TABLE color (Location text NOT NULL COLLATE NOCASE, Name text NOT NULL COLLATE NOCASE, Value text NOT NULL, seqidx integer DEFAULT 0); INSERT INTO color VALUES ('Synth One','MST_bgcolor','aa5500',0),('Synth One|Expansion X','VB_bgcolor','0055aa',0);", nil, nil, nil)
        sqlite3_close(database)

        let cache = temporary.appendingPathComponent("cache", isDirectory: true)
        let art = NKSArtwork(root: resources, cacheDirectory: cache)
        try check(art.match(product: "Synth One", vendor: "Native Instruments") == Match(product: "image/synth one", bank: nil, color: "#aa5500"), "NI product folder and colour")
        try check(art.match(product: "SYNTH ONE", vendor: "Native Instruments", bank: "Expansion X") == Match(product: "image/synth one", bank: "image/expansion x", color: "#0055aa"), "NI expansion bank art and bank colour, case-insensitive")
        try check(art.match(product: "Acme Gizmo", vendor: "Acme", bank: "Expansion X") == Match(product: "image/acme/acme gizmo", bank: nil, color: "#112233"), "partner vendor folder; NI expansion art is not borrowed by another vendor")
        try check(art.match(product: "Unknown", vendor: "Nobody") == Match(), "unknown product")
        let tiles = art.thumbnails(keys: ["image/synth one", "image/expansion x", "../../etc", "/etc/passwd", "image"], variant: "tile")
        try check(Set(tiles.keys) == ["image/synth one", "image/expansion x"], "only indexed folders are served")
        try check(tiles.values.allSatisfy { $0.hasPrefix("data:image/jpeg;base64,") }, "tile thumbnails are JPEG data URLs")
        try check(art.thumbnails(keys: ["image/synth one"], variant: "logo")["image/synth one"]?.hasPrefix("data:image/png;base64,") == true, "logos keep transparency")
        try check(art.thumbnails(keys: ["image/acme/acme gizmo"], variant: "logo").isEmpty, "folders without a logo return nothing")
        try check(art.thumbnails(keys: ["image/synth one"], variant: "../x").isEmpty, "unknown variants rejected")
        let banner = art.thumbnails(keys: ["image/expansion x"], variant: "banner")["image/expansion x"] ?? ""
        if let data = Data(base64Encoded: String(banner.dropFirst("data:image/jpeg;base64,".count))),
           let source = CGImageSourceCreateWithData(data as CFData, nil),
           let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any] {
            try check((properties[kCGImagePropertyPixelWidth] as? Int ?? 0) <= 960, "banners are downscaled")
        } else { try check(false, "banner decodes") }
        let cached = (try? manager.contentsOfDirectory(atPath: cache.path)) ?? []
        try check(cached.count >= 4, "thumbnails persisted to disk")
        try check(NKSArtwork(root: resources, cacheDirectory: cache).thumbnails(keys: ["image/expansion x"], variant: "banner")["image/expansion x"] == banner, "disk cache reused")
        try check(NKSArtwork(root: temporary.appendingPathComponent("missing"), cacheDirectory: nil).match(product: "Synth One", vendor: "Native Instruments") == Match(), "missing NI Resources degrades to no artwork")

        // Catalogue: vendor and sub-bank drill-down, artwork fields and hidden internal modes.
        func word(_ value: UInt32) -> [UInt8] { (0..<4).map { UInt8(truncatingIfNeeded: value >> ($0 * 8)) } }
        func str(_ value: String) -> [UInt8] { let bytes = Array(value.utf8); return [0xa0 | UInt8(bytes.count)] + bytes }
        func array(_ values: [[UInt8]]) -> [UInt8] { [0x90 | UInt8(values.count)] + values.flatMap { $0 } }
        func chunk(_ name: String, _ content: [UInt8]) -> [UInt8] { Array(name.utf8) + word(UInt32(content.count)) + content + (content.count % 2 == 1 ? [0] : []) }
        func preset(_ name: String, vendor: String, chain: [String], characters: [String] = []) -> Data {
            var map: [UInt8] = [0x85]
            map += str("name") + str(name) + str("vendor") + str(vendor) + str("deviceType") + str("INST")
            map += str("bankchain") + array(chain.map(str)) + str("characters") + array(characters.map(str))
            let body = chunk("NISI", word(1) + map) + chunk("PLID", word(1) + [0x81] + str("VST.magic") + [0xce, 0x4b, 0x61, 0x74, 0x31])
            return Data(Array("RIFF".utf8) + word(UInt32(body.count + 4)) + Array("NIKS".utf8) + body)
        }
        let presets = temporary.appendingPathComponent("Presets", isDirectory: true)
        try manager.createDirectory(at: presets, withIntermediateDirectories: true)
        try preset("One", vendor: "Native Instruments", chain: ["Synth One", "Expansion X", "Leads"], characters: ["Bright", "_ SEL LEAD (ROUND 4)"]).write(to: presets.appendingPathComponent("One.nksf"))
        try preset("Two", vendor: "Native Instruments", chain: ["Synth One", "Expansion X", "Pads"]).write(to: presets.appendingPathComponent("Two.nksf"))
        try preset("Three", vendor: "Native Instruments", chain: ["Synth One", "Factory", ""]).write(to: presets.appendingPathComponent("Three.nksf"))
        try preset("Four", vendor: "Acme", chain: ["Acme Gizmo"]).write(to: presets.appendingPathComponent("Four.nksf"))
        let catalog = NKSCatalog(cacheDirectory: temporary.appendingPathComponent("catalog"), discoverSystemContent: false, roots: [presets.path], artworkRoot: resources)
        catalog.refreshNow()
        func names(_ result: [String: Any]) -> [String] { (result["items"] as? [[String: Any]] ?? []).compactMap { $0["name"] as? String } }
        let all = catalog.browse(["kind": "instrument"], favorites: [])
        try check(names(all) == ["Four", "One", "Three", "Two"], "catalogue fixture")
        try check((all["vendors"] as? [[String: Any]])?.compactMap { $0["name"] as? String } == ["Native Instruments", "Acme"], "vendor facet by count")
        let products = all["products"] as? [[String: Any]] ?? []
        try check(products.first { $0["name"] as? String == "Synth One" }?["art"] as? String == "image/synth one", "product facet artwork")
        try check(products.first { $0["name"] as? String == "Acme Gizmo" }?["color"] as? String == "#112233", "product facet colour")
        let one = (all["items"] as? [[String: Any]] ?? []).first { $0["name"] as? String == "One" } ?? [:]
        try check(one["art"] as? String == "image/expansion x" && one["productArt"] as? String == "image/synth one", "preset rows carry expansion and product art")
        try check(one["characters"] as? [String] == ["Bright"], "internal selection modes are hidden from presets")
        try check(!((all["characters"] as? [[String: Any]] ?? []).contains { ($0["name"] as? String ?? "").hasPrefix("_") }), "internal selection modes are hidden from filters")
        let acme = catalog.browse(["kind": "instrument", "vendor": "Acme"], favorites: [])
        try check(names(acme) == ["Four"] && (acme["products"] as? [[String: Any]])?.count == 1, "vendor narrows products and presets")
        let bank = catalog.browse(["kind": "instrument", "product": "Synth One", "bank": "Expansion X"], favorites: [])
        try check(names(bank) == ["One", "Two"], "bank drill-down")
        try check((bank["subBanks"] as? [[String: Any]])?.compactMap { $0["name"] as? String } == ["Leads", "Pads"], "sub-bank facet")
        try check((bank["banks"] as? [[String: Any]])?.first { $0["name"] as? String == "Expansion X" }?["art"] as? String == "image/expansion x", "bank facet artwork")
        try check((bank["banks"] as? [[String: Any]])?.first { $0["name"] as? String == "Factory" }?["art"] == nil, "banks without artwork stay plain")
        try check(names(catalog.browse(["kind": "instrument", "product": "Synth One", "bank": "Expansion X", "subBank": "Pads"], favorites: [])) == ["Two"], "sub-bank filter")
        try check(names(catalog.browse(["kind": "instrument", "product": "Synth One", "subBank": "Pads"], favorites: [])).count == 3, "a sub-bank without its bank is ignored")
        return ["NKS artwork: NI products, expansions, partner vendors, colours and bounded thumbnails",
                "NKS artwork: unknown keys, disk cache and missing resources",
                "NKS catalogue: vendor and sub-bank drill-down, artwork fields, hidden internal modes"]
    }

    static func hexColor(_ value: String) -> String? {
        let hex = value.trimmingCharacters(in: CharacterSet(charactersIn: "# \n")).lowercased()
        guard hex.count == 6, hex.allSatisfy(\.isHexDigit) else { return nil }
        return "#" + hex
    }

    private func locked<T>(_ body: () throws -> T) rethrows -> T { lock.lock(); defer { lock.unlock() }; return try body() }
}

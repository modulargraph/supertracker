import Foundation

@main
struct NKSHostPresetTests {
    static func main() throws {
        let temp = FileManager.default.temporaryDirectory.appendingPathComponent("SuperTracker-NKS-host-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: temp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: temp) }
        var serial = 0
        func write(_ bytes: Data) throws -> URL {
            serial += 1
            let url = temp.appendingPathComponent("Fixture\(serial).nksf")
            try bytes.write(to: url)
            return url
        }
        func check(_ condition: @autoclosure () -> Bool, _ message: String) throws {
            if !condition() { throw NSError(domain: "NKSHostPresetTests", code: 1, userInfo: [NSLocalizedDescriptionKey: message]) }
        }
        func rejects(_ bytes: Data, _ message: String, maxStateBytes: Int = 1024) throws {
            let url = try write(bytes)
            var rejected = false
            do { _ = try NKSHostPreset(url: url, maxStateBytes: maxStateBytes) }
            catch { rejected = true }
            try check(rejected, message)
        }
        let identity: [String: Any] = ["VST3.uid": [0x5653544E, 0x6924486D, 0x61737369, 0x76652078], "VST.magic": 0,
                                       "pluginName": "Fixture Synth", "pluginVendor": "Fixture Vendor"]
        let mapping: [String: Any] = ["ni8": [[
            ["id": 0, "name": "Cutoff", "section": "Tone"],
            ["id": 9001, "name": "Resonance"],
            ["name": "Empty"],
            ["id": -1, "name": "Invalid"],
            ["id": 99, "name": "Unavailable"]
        ]]]
        let nativeState = Data([2, 0, 0, 0, 3, 0, 0, 0, 1, 2, 3, 4, 5])
        func fixture(plid: [String: Any] = identity, state: Data = nativeState, version: UInt32 = 1,
                     nica: [String: Any] = mapping, nicaVersion: UInt32 = 1, extra: Data = Data()) -> Data {
            riff(chunk("NISI", word(1) + pack(["name": "Preset Fixture"])) +
                 chunk("PLID", word(1) + pack(plid)) + chunk("NICA", word(nicaVersion) + pack(nica)) +
                 chunk("PCHK", word(version) + state) + extra)
        }
        let file = try write(fixture())
        let preset = try NKSHostPreset(url: file)
        try check(preset.identity.vst3ClassID == "5653544E6924486D6173736976652078", "macOS FUID word order")
        try check(preset.identity.vst2Magic == nil && preset.identity.name == "Fixture Synth", "structured identity")
        try check(preset.state == nativeState && preset.stateVersion == 1, "only NKS PCHK version removed; vendor bytes preserved")
        try check(preset.controllerPages.count == 1 && preset.controllerPages[0].name == "Tone", "controller page grouping")
        try check(preset.controllerPages[0].slots[0].parameterID == 0, "zero is a valid mapped parameter ID")
        try check(preset.controllerPages[0].slots[3].parameterID == nil, "invalid negative ID remains unbound")
        let bindings = preset.validatedMappings(vst3ClassID: "5653544e6924486d6173736976652078", parameterIDs: [0, 9001])
        let controls = bindings[0]["parameters"] as! [[String: Any]]
        try check(controls[0]["mapped"] as? Bool == true && controls[1]["mapped"] as? Bool == true, "ID-based runtime binding")
        try check(controls[2]["mappingStatus"] as? String == "unassigned" && controls[4]["mappingStatus"] as? String == "parameterUnavailable", "unassigned and missing parameter distinction")
        let wrong = preset.validatedMappings(vst3ClassID: String(repeating: "A", count: 32), parameterIDs: [0, 9001])
        try check((wrong[0]["parameters"] as! [[String: Any]])[0]["mapped"] as? Bool == false, "wrong plug-in cannot claim mappings")
        try check(JSONSerialization.isValidJSONObject(preset.dictionary) && preset.dictionary["state"] == nil, "summary JSON excludes binary state")
        let legacy = try NKSHostPreset(url: write(fixture(plid: ["VST.magic": 0x4E692448])))
        try check(legacy.identity.vst3ClassID == nil && legacy.identity.vst2Magic == 0x4E692448 && !legacy.warnings.isEmpty, "legacy identity remains VST2")
        let legacyBindings = legacy.validatedMappings(vst3ClassID: "5653544E6924486D6173736976652078", parameterIDs: [0, 9001])
        try check((legacyBindings[0]["parameters"] as! [[String: Any]])[0]["mapped"] as? Bool == true, "VST2 IDs bind to the vendor's VST2-compatible VST3 class")
        let unrelated = legacy.validatedMappings(vst3ClassID: "41727475415649534478373150726F63", parameterIDs: [0, 9001])
        try check((unrelated[0]["parameters"] as! [[String: Any]])[0]["mapped"] as? Bool == false, "VST2 IDs never bind to an unrelated VST3 class")
        let signed = try NKSHostPreset(url: write(fixture(plid: ["VST3.uid": [-1, Int32.min, 0, 1]])))
        try check(signed.identity.vst3ClassID == "FFFFFFFF800000000000000000000001", "signed PLID words retain exact bits")
        let unknownMapping = try NKSHostPreset(url: write(fixture(nicaVersion: 2)))
        try check(unknownMapping.controllerPages.isEmpty && !unknownMapping.warnings.isEmpty, "future NICA versions not mapped")
        try rejects(fixture(version: 2), "future PCHK version rejected")
        try rejects(fixture(state: Data()), "empty state rejected")
        try rejects(fixture(), "state memory limit enforced", maxStateBytes: 4)
        try rejects(fixture(), "invalid memory limit rejected", maxStateBytes: 0)
        try rejects(fixture(extra: chunk("PCHK", word(1) + nativeState)), "ambiguous multiple state chunks rejected")
        try rejects(fixture(extra: chunk("PLID", word(1) + pack(identity))), "ambiguous multiple identities rejected")
        try rejects(fixture(plid: ["pluginName": "Unknown"]), "name matching cannot replace stable identity")
        for bad: Any in [[1, 2, 3], [0, 0, 0, 0], [true, 1, 2, 3], [0.5, 1, 2, 3], [4_294_967_296 as UInt64, 1, 2, 3]] {
            try rejects(fixture(plid: ["VST3.uid": bad]), "malformed VST3 UID rejected")
        }
        var truncated = fixture()
        truncated.removeLast()
        try rejects(truncated, "truncated RIFF/padding rejected")
        try rejects(riff(chunk("PLID", word(1) + pack(identity))), "absent state rejected")
        try rejects(riff(chunk("PLID", word(2) + pack(identity)) + chunk("PCHK", word(1) + nativeState)), "future PLID version rejected")
        let unchanged = try Data(contentsOf: file)
        try check(unchanged == fixture(), "source preset is read-only")
        print("PASS NKS host preset: bounded RIFF/state extraction, exact VST3 identity, legacy separation, validated parameter mappings, malformed inputs, source preservation")
        if CommandLine.arguments.contains("--installed-smoke") {
            let actual = URL(fileURLWithPath: "/Library/Application Support/Native Instruments/Massive X/Presets/Contender.nksf")
            let installed = try NKSHostPreset(url: actual)
            try check(installed.identity.vst3ClassID == "5653544E6924486D6173736976652078", "installed Massive X identity")
            try check(installed.state.count == 31_597 && installed.controllerPages.first?.slots.first?.parameterID == 0, "installed Massive X state and mapping")
            print("PASS installed Massive X Contender: explicit VST3 identity, 31597-byte opaque state, NICA controls; sound restoration requires host test")
        }
    }

    static func word(_ value: UInt32) -> Data { Data((0..<4).map { UInt8(truncatingIfNeeded: value >> ($0 * 8)) }) }
    static func chunk(_ name: String, _ data: Data) -> Data { Data(name.utf8) + word(UInt32(data.count)) + data + (data.count % 2 == 0 ? Data() : Data([0])) }
    static func riff(_ chunks: Data) -> Data { Data("RIFF".utf8) + word(UInt32(chunks.count + 4)) + Data("NIKS".utf8) + chunks }
    /// Independent tiny fixture writer: production decoding uses the bounded NKSMetadata reader.
    static func pack(_ value: Any) -> Data {
        if let string = value as? String {
            let bytes = Data(string.utf8)
            return (bytes.count < 32 ? Data([0xA0 | UInt8(bytes.count)]) : Data([0xD9, UInt8(bytes.count)])) + bytes
        }
        if let map = value as? [String: Any] {
            var data = Data([0x80 | UInt8(map.count)])
            for key in map.keys.sorted() { data += pack(key); data += pack(map[key]!) }
            return data
        }
        if let array = value as? [Any] {
            var data = Data([0x90 | UInt8(array.count)])
            for item in array { data += pack(item) }
            return data
        }
        if let number = value as? NSNumber {
            if String(cString: number.objCType) == "c" { return Data([number.boolValue ? 0xC3 : 0xC2]) }
            if number.doubleValue.rounded(.towardZero) != number.doubleValue {
                let bits = number.doubleValue.bitPattern
                return Data([0xCB] + (0..<8).map { UInt8(truncatingIfNeeded: bits >> ((7 - $0) * 8)) })
            }
            let bits = UInt64(bitPattern: number.int64Value)
            return Data([number.int64Value < 0 ? 0xD3 : 0xCF] + (0..<8).map { UInt8(truncatingIfNeeded: bits >> ((7 - $0) * 8)) })
        }
        return Data([0xC0])
    }
}

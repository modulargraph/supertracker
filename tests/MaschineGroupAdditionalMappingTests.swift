import Foundation
import TrackerEngine

@main
struct AdditionalMappingTests {
    static var checks = 0
    static func require(_ condition: @autoclosure () -> Bool, _ message: String) {
        checks += 1
        if !condition() { fatalError(message) }
    }
    static func physical(_ mapping: [String: Any], _ key: String) -> Double? {
        let id = String((mapping["id"] as! String).dropFirst(5))
        let parameters = mapping["parameters"] as! [[String: Any]]
        for address in 0..<st_builtin_effect_find(id)!.pointee.parameterCount {
            let metadata = st_builtin_effect_parameter_info(id, address)!.pointee
            if String(cString: metadata.id) == key,
               let value = parameters.first(where: { ($0["address"] as? Int) == Int(address) })?["value"] as? Double {
                return st_builtin_effect_from_normalized(id, address, value)
            }
        }
        return nil
    }
    static func close(_ value: Double?, _ expected: Double, _ message: String) {
        require(value != nil && abs(value! - expected) < max(1e-7, abs(expected) * 1e-6), message)
    }
    static func warning(_ mapping: [String: Any], _ text: String) -> Bool {
        (mapping["warnings"] as! [String]).contains { $0.contains(text) }
    }
    static func map(_ type: String, _ raw: [Double], version: Int = 0) -> [String: Any] {
        guard let result = MaschineGroupAdditionalMapping.map(type: type, version: version, parameters: raw) else { fatalError("missing \(type)") }
        return result
    }
    static func main() {
        let gate = map("NI::MASCHINE::DATA::Gate", [0.316227766, 0.1, 0.2, 0.3, 0.4])
        close(physical(gate, "threshold"), -20, "Gate logarithmic threshold")
        close(physical(gate, "hold"), 200, "Gate hold uses serializer third value")
        close(physical(gate, "mix"), 40, "Gate fifth value is mix")
        require(warning(gate, "millisecond curves are approximated"), "Gate unit approximation disclosed")
        let invalidGate = map("Gate", [0, 0, 0, 0, 1])
        require(physical(invalidGate, "threshold") == nil, "zero gate threshold rejected before logarithm")
        let limiter = map("Limiter", [-8, 1, 0.5, 0.75], version: 1)
        close(physical(limiter, "threshold"), -8, "Limiter threshold is raw dB, not normalized")
        close(physical(limiter, "mode"), 1, "Limiter mode follows threshold")
        close(physical(limiter, "release"), sqrt(500), "Limiter exponential release")
        close(physical(limiter, "ceiling"), -10.225, "Limiter linear ceiling")
        for endpoint in [0.0, 1.0] {
            let result = map("Limiter", [-40 * endpoint, endpoint, endpoint, endpoint], version: 1)
            close(physical(result, "release"), endpoint == 0 ? 1 : 500, "Limiter release endpoints")
            close(physical(result, "ceiling"), endpoint == 0 ? -40 : -0.3, "Limiter ceiling endpoints")
        }
        let transient = map("TransientDesigner", [-0.5, -0.25, 0.75, 1])
        close(physical(transient, "gain"), -12, "Transient serialized input gain first, negative curve")
        close(physical(transient, "attack"), -25, "Transient signed attack")
        close(physical(transient, "sustain"), 75, "Transient sustain third")
        close(physical(map("TransientDesigner", [0.5, 0, 0, 0]), "gain"), 3, "Transient asymmetric positive gain")
        let cabinet = map("CabinetEmulation", [3, 5, 0.2, 0.3])
        close(physical(cabinet, "cabinet"), 3, "Cabinet raw enum not normalized")
        close(physical(cabinet, "microphone"), 5, "Microphone sixth enum")
        close(physical(cabinet, "distance"), 20, "Cabinet distance")
        close(physical(cabinet, "mix"), 30, "Cabinet wet dry")
        let filter = map("Filter", [2, 0.5, 0.3, -0.4, 2, 0.6, 0.7, 0.8, 0.2, 0.1, 0.9])
        close(physical(filter, "mode"), 2, "HP filter enum")
        close(physical(filter, "cutoff"), sqrt(46 * 12540), "Filter verified exponential endpoints")
        close(physical(filter, "amount"), -2, "Filter signed modulation")
        close(physical(filter, "source"), 2, "Filter envelope source")
        close(physical(filter, "phase"), 288, "Filter phase maps its own field")
        require(physical(filter, "shape") == nil, "Continuous LFO shape not mistaken for enum")
        let phaser = map("Phaser", [0.1, -0.6, 1, 0.3, 0.4, 0.5, 0.7, 1, 0.9])
        close(physical(phaser, "amount"), -60, "Phaser negative sweep survives")
        close(physical(phaser, "source"), 1, "Phaser source enum is third field")
        close(physical(phaser, "feedback"), 70, "Phaser feedback seventh field")
        close(physical(phaser, "eightpole"), 1, "Phaser boolean eighth field")
        close(physical(phaser, "mix"), 90, "Phaser mix ninth field")
        require(warning(phaser, "four-beat"), "Unknown tempo subdivision retained with warning")
        let flanger = map("Flanger", [0.2, -0.3, 0.4, 0.5, 0.6, 0.7, 1, 0.8])
        close(physical(flanger, "amount"), -30, "Flanger signed amount")
        close(physical(flanger, "source"), 0, "Flanger v0 contains no source field")
        close(physical(flanger, "stereo"), 108, "Flanger v0 indexing after omitted enum")
        close(physical(flanger, "feedback"), 70, "Flanger feedback sixth")
        close(physical(flanger, "invert"), 1, "Flanger invert seventh")
        close(physical(flanger, "mix"), 80, "Flanger mix eighth")
        let shift = map("FreqShifter", [0.6, 0.1, 0.7, 0.3, 1, 0.5])
        require(physical(shift, "coarse") == nil && physical(shift, "fine") == nil, "Unknown Hz mapping not invented")
        close(physical(shift, "feedback"), 70, "Frequency shifter feedback")
        close(physical(shift, "stereo"), 30, "Frequency shifter stereo")
        require(warning(shift, "remain at zero"), "Untranslated pitch explicit")
        let maximizer = map("Maximizer", [0.2, 0.4, 1])
        close(physical(maximizer, "amount"), 6, "Maximizer approximate drive")
        close(physical(maximizer, "curve"), 40, "Maximizer curve percentage")
        close(physical(maximizer, "turbo"), 1, "Maximizer turbo")
        let chorus = map("Chorus", [0.5, 0.6, 0.7])
        close(physical(chorus, "amount"), 60, "Chorus amount")
        close(physical(chorus, "mix"), 70, "Chorus mix")
        let fm = map("FM", [0.2, 0.3, 0.4, 0.5])
        close(physical(fm, "contour"), 40, "FM contour")
        close(physical(fm, "amount"), 50, "FM amount")
        let invalid = map("CabinetEmulation", [2.5, 1000, -1, 0.5])
        require(physical(invalid, "cabinet") == nil && physical(invalid, "microphone") == nil, "Noninteger and invalid enums not rounded")
        require(physical(invalid, "distance") == nil, "Invalid range keeps default")
        close(physical(invalid, "mix"), 50, "Other valid fields survive")
        let clipped = map("FreqShifter", [0, 0, 1, 0, 0, 1])
        close(physical(clipped, "feedback"), 95, "Feedback bounded for stability")
        require(warning(clipped, "limited"), "Clamped fields disclosed")
        let fixtures: [(String, Int, [Double])] = [
            ("Gate", 0, [0.5, 0, 0, 0, 0]), ("Limiter", 1, [-8, 0, 0, 1]),
            ("Maximizer", 0, [0, 0, 0]), ("TransientDesigner", 0, [0, 0, 0, 0]),
            ("CabinetEmulation", 0, [0, 0, 0, 0]), ("Chorus", 0, [0, 0, 0]),
            ("FM", 0, [0, 0, 0, 0]), ("Filter", 0, Array(repeating: 0, count: 11)),
            ("Flanger", 0, Array(repeating: 0, count: 8)), ("Phaser", 0, Array(repeating: 0, count: 9)),
            ("FreqShifter", 0, Array(repeating: 0, count: 6))]
        for (type, version, raw) in fixtures {
            let result = map(type, raw, version: version)
            let parameters = result["parameters"] as! [[String: Any]]
            let addresses = parameters.map { $0["address"] as! Int }
            require(addresses == addresses.sorted() && Set(addresses).count == addresses.count, "Stable unique parameter addresses \(type)")
            require(parameters.allSatisfy { let v = $0["value"] as! Double; return v.isFinite && (0...1).contains(v) }, "All output is normalized \(type)")
            require(warning(result, "independent"), "Approximation disclosed \(type)")
            require(MaschineGroupAdditionalMapping.map(type: type, version: version + 10, parameters: raw) == nil, "Unknown version rejected \(type)")
            require(MaschineGroupAdditionalMapping.map(type: type, version: version, parameters: Array(raw.dropLast())) == nil, "Short payload rejected \(type)")
            require(MaschineGroupAdditionalMapping.map(type: type, version: version, parameters: raw + [0]) == nil, "Long payload rejected \(type)")
            for bad in [Double.nan, Double.infinity, -Double.infinity] {
                var badRaw = raw; badRaw[0] = bad
                require(MaschineGroupAdditionalMapping.map(type: type, version: version, parameters: badRaw) == nil, "Nonfinite payload rejected \(type)")
            }
        }
        require(MaschineGroupAdditionalMapping.map(type: "Unknown", version: 0, parameters: []) == nil, "Unknown type rejected")
        print("Maschine additional mapping tests passed (\(checks) checks)")
    }
}

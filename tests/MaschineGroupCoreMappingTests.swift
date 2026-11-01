import Foundation
import TrackerEngine

@main
struct CoreMappingTests {
    static var checks = 0
    static func require(_ condition: @autoclosure () -> Bool, _ message: String) {
        checks += 1
        if !condition() { fatalError(message) }
    }
    static func physical(_ mapping: [String: Any], _ key: String) -> Double? {
        let id = String((mapping["id"] as! String).dropFirst(5))
        guard let info = st_builtin_effect_find(id)?.pointee else { return nil }
        let parameters = mapping["parameters"] as! [[String: Any]]
        for address in 0..<info.parameterCount {
            guard let metadata = st_builtin_effect_parameter_info(id, address)?.pointee,
                  String(cString: metadata.id) == key,
                  let parameter = parameters.first(where: { ($0["address"] as? Int) == Int(address) }),
                  let value = parameter["value"] as? Double else { continue }
            return st_builtin_effect_from_normalized(id, address, value)
        }
        return nil
    }
    static func close(_ value: Double?, _ expected: Double, _ message: String) {
        require(value != nil && abs(value! - expected) < max(1e-8, abs(expected) * 1e-6), message)
    }
    static func main() {
        // Distinct serialized values detect constructor/UI-order confusion.
        let raw = [0.0, -0.8, 0.25, -0.4, 0.75, 0.2, 1.0, 0.6, 0.0, 1.0, -0.25]
        let eq = MaschineGroupCoreMapping.map(type: "NI::MASCHINE::DATA::EQ", version: 0, parameters: raw)!
        require(eq["id"] as? String == "stfx:eq", "EQ native effect identity")
        require((eq["parameters"] as! [[String: Any]]).count == 11, "all EQ fields map")
        close(physical(eq, "lowfreq"), 20, "low frequency endpoint")
        close(physical(eq, "lowmidfreq"), 178.8854382, "low-mid exponential quarter point")
        close(physical(eq, "highmidfreq"), 3577.708764, "high-mid field stays in serializer position")
        close(physical(eq, "highfreq"), 20000, "high frequency endpoint")
        close(physical(eq, "lowgain"), -16, "signed low gain")
        close(physical(eq, "lowmidgain"), -8, "signed low-mid gain")
        close(physical(eq, "highmidgain"), 4, "positive high-mid gain")
        close(physical(eq, "highgain"), 12, "positive high gain")
        close(physical(eq, "lowmidwidth"), 0.1, "width lower endpoint in octaves")
        close(physical(eq, "highmidwidth"), 4, "width upper endpoint differs from target metadata range")
        close(physical(eq, "gain"), -5, "output gain after widths")
        let neutral = MaschineGroupCoreMapping.map(type: "EQ", version: 0, parameters: [0.5, 0, 0.5, 0, 0.5, 0, 0.5, 0, 0.5, 0.5, 0])!
        close(physical(neutral, "lowgain"), 0, "signed zero is neutral gain, not minimum")
        close(physical(neutral, "lowmidwidth"), 2.05, "source width midpoint")
        close(physical(neutral, "lowfreq"), 400, "log frequency midpoint")
        let compressor = MaschineGroupCoreMapping.map(type: "Compressor", version: 1, parameters: [1, 0.5, 0.8, 0.3, 0.5, 0.5, 0.75])!
        close(physical(compressor, "mode"), 1, "Feedback mode enum")
        close(physical(compressor, "threshold"), -30, "inverted normalized threshold")
        close(physical(compressor, "attack"), 1, "cubic attack is 1 ms at midpoint")
        close(physical(compressor, "gain"), -9, "gain maps negative dB domain")
        require(physical(compressor, "release")! > 99 && physical(compressor, "release")! < 101, "release midpoint is approximately 100 ms")
        require(physical(compressor, "ratio") == nil && physical(compressor, "knee") == nil, "unverified ratio/knee are never guessed")
        let warnings = compressor["warnings"] as! [String]
        require(warnings.contains(where: { $0.contains("Amount and Knee") }), "unmapped controls reported")
        let old = MaschineGroupCoreMapping.map(type: "Compressor", version: 0, parameters: [1, 0.8, 0.3, 0, 0, 0])!
        close(physical(old, "mode"), 0, "old archive omits mode and defaults to Classic")
        close(physical(old, "threshold"), -60, "version zero indexing")
        close(physical(old, "attack"), 0.1, "zero attack limited to host minimum")
        close(physical(old, "gain"), -24, "source -36 dB limited to host minimum")
        require((old["warnings"] as! [String]).filter { $0.contains("limited") }.count == 2, "each clipped physical parameter is disclosed")
        for (type, version, values) in [("EQ", 1, raw), ("Compressor", 2, raw), ("Unknown", 0, raw), ("EQ", 0, []), ("Compressor", 1, [0, 0, 0, 0, 0, 0])] {
            require(MaschineGroupCoreMapping.map(type: type, version: version, parameters: values) == nil, "unknown version/shape fails closed")
        }
        var invalid = raw; invalid[1] = .nan
        require(MaschineGroupCoreMapping.map(type: "EQ", version: 0, parameters: invalid) == nil, "NaN fails closed")
        invalid = raw; invalid[0] = .infinity
        require(MaschineGroupCoreMapping.map(type: "EQ", version: 0, parameters: invalid) == nil, "infinity fails closed")
        invalid = raw; invalid[0] = -0.1
        let partial = MaschineGroupCoreMapping.map(type: "EQ", version: 0, parameters: invalid)!
        require(physical(partial, "lowfreq") == nil, "out-of-domain source retains destination default")
        close(physical(partial, "highfreq"), 20000, "bad source control does not discard valid controls")
        require((partial["warnings"] as! [String]).contains(where: { $0.contains("verified range") }), "out-of-domain field disclosed")
        print("Maschine core mapping tests passed (\(checks) checks)")
    }
}

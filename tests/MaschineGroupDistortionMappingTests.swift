import Foundation
import TrackerEngine

@main
struct DistortionMappingTests {
    static var checks = 0
    static func require(_ value: @autoclosure () -> Bool, _ message: String) {
        checks += 1
        if !value() { fatalError(message) }
    }
    static func map(_ type: String, _ raw: [Double], version: Int = 0) -> [String: Any] {
        guard let result = MaschineGroupDistortionMapping.map(type: type, version: version, parameters: raw) else { fatalError("Missing \(type)") }
        return result
    }
    static func physical(_ result: [String: Any], _ key: String) -> Double? {
        let id = String((result["id"] as! String).dropFirst(5))
        for parameter in result["parameters"] as! [[String: Any]] {
            let address = UInt32(parameter["address"] as! Int)
            if String(cString: st_builtin_effect_parameter_info(id, address)!.pointee.id) == key {
                return st_builtin_effect_from_normalized(id, address, parameter["value"] as! Double)
            }
        }
        return nil
    }
    static func close(_ value: Double?, _ expected: Double, _ message: String) {
        require(value != nil && abs(value! - expected) < max(1e-6, abs(expected) * 1e-6), message)
    }
    static func warning(_ result: [String: Any], _ text: String) -> Bool {
        (result["warnings"] as! [String]).contains { $0.contains(text) }
    }
    static func main() {
        let lofi = map("NI::MASCHINE::DATA::LoFi", [0, 0.3, 0.4, 0.6, 0.8])
        close(physical(lofi, "sample_rate"), 44100, "LoFi maximum source rate")
        close(physical(lofi, "smooth"), 30, "Smooth is second serialized field")
        close(physical(lofi, "bits"), 7.6, "Bits is third field with descending curve")
        close(physical(lofi, "stereo"), 60, "Source stereo percentage")
        close(physical(lofi, "mix"), 80, "Source mix")
        let degraded = map("LoFi", [1, 1, 1, 0, 1])
        close(physical(degraded, "sample_rate"), 44100 * pow(Double(Float(bitPattern: 0x3f879c63)), -105.5), "Source minimum rate")
        close(physical(degraded, "bits"), 1, "Bits endpoint")
        let saturator: [Double] = [1, 0.1, 0.2, 0.3, 0.5, -0.6, 0.5, 0.7, 3, 0.8, 0.4, -7, 1, 1]
        let tape = map("Saturator", saturator)
        close(physical(tape, "input"), 15, "Tape input exact dB curve")
        close(physical(tape, "drive"), 35, "Tape negative drive preserved as negative tilt")
        close(physical(tape, "contour"), 100 * log(16) / log(20), "Tape cutoff16kHz translated to target knob")
        require(physical(tape, "output") == nil, "Inactive Tube gain does not overwrite Tape output")
        var tubeRaw = saturator; tubeRaw[0] = 2
        let tube = map("Saturator", tubeRaw)
        close(physical(tube, "mode"), 2, "Raw enum2 survives")
        close(physical(tube, "drive"), 70, "Tube drive uses its own field")
        close(physical(tube, "charge"), 75, "Charge domain0...4")
        close(physical(tube, "output"), -7, "Tube gain is signed raw dB")
        close(physical(tube, "overload"), 12, "Overload boolean maps disclosed boost")
        close(physical(tube, "eq_bypass"), 1, "EQ bypass separate boolean")
        require(physical(tube, "bass") == nil && physical(tube, "treble") == nil, "Unverified EQ gain conversion not invented")
        require(warning(tube, "remain neutral"), "Unsupported EQ disclosed")
        var classicRaw = saturator; classicRaw[0] = 0
        let classic = map("Saturator", classicRaw)
        close(physical(classic, "drive"), 20, "Classic drive differs from Tape and Tube")
        close(physical(classic, "contour"), 30, "Classic contour")
        require(warning(classic, "approximated"), "Classic input approximation disclosed")
        let distortionRaw: [Double] = [0, 0.2, 0.3, 0.4, 1, 0.6, 0.7, 0.1, 0.8, -0.5, 0.9, -0.4, 1, 0.5, 0.25]
        let fuzz = map("MullhollandDrive", distortionRaw, version: 1)
        close(physical(fuzz, "drive"), 4.2, "Fuzz drive field8")
        close(physical(fuzz, "mix"), 25, "Fuzz mix uses final field")
        close(physical(fuzz, "feedback"), 0, "Negative feedback not changed into positive")
        close(physical(fuzz, "tone_mod"), 0, "Negative tone modulation not changed into positive")
        close(physical(fuzz, "gate_release"), 1000 * (Double(Float(bitPattern: 0x3c23d70a)) + (0.25 + (0.0625 - 0.25) * Double(Float(bitPattern: 0x3f4f41f2))) * Double(Float(bitPattern: 0x409fae14))), "Verified release quartic")
        require(warning(fuzz, "signed source"), "Signed unsupported fields disclosed")
        var analogRaw = distortionRaw; analogRaw[0] = 1
        let analog = map("MullhollandDrive", analogRaw, version: 1)
        close(physical(analog, "drive"), 0, "Inactive Fuzz drive cannot multiply Analog saturation")
        close(physical(analog, "saturation"), 20, "Analog saturation")
        close(physical(analog, "mix"), 70, "Analog mix uses seventh field")
        close(physical(analog, "tone"), 150 * pow(16000.0 / 150, 0.4), "Analog tone uses fourth field")
        var limitedRaw = distortionRaw; limitedRaw[13] = 1; limitedRaw[9] = 1
        let limited = map("MullhollandDrive", limitedRaw, version: 1)
        close(physical(limited, "gate_release"), 2000, "Release target maximum enforced")
        close(physical(limited, "feedback"), 95, "Feedback target stability bound")
        require(warning(limited, "limited"), "Clamping disclosed")
        for badMode in [-1.0, 0.5, 3] {
            var invalid = saturator; invalid[0] = badMode
            let result = map("Saturator", invalid)
            require((result["parameters"] as! [[String: Any]]).isEmpty, "Invalid mode cannot choose unrelated active controls")
            require(warning(result, "Unknown mode"), "Invalid mode disclosed")
        }
        for (type, version, raw) in [("LoFi", 0, [0.0, 0, 0, 0, 0]), ("Saturator", 0, saturator), ("MullhollandDrive", 1, distortionRaw)] {
            let result = map(type, raw, version: version)
            let parameters = result["parameters"] as! [[String: Any]]
            let addresses = parameters.map { $0["address"] as! Int }
            require(addresses == addresses.sorted() && Set(addresses).count == addresses.count, "Unique stable addresses")
            require(parameters.allSatisfy { (0...1).contains($0["value"] as! Double) }, "Output parameters normalized")
            require(MaschineGroupDistortionMapping.map(type: type, version: version + 1, parameters: raw) == nil, "Unsupported version rejected")
            require(MaschineGroupDistortionMapping.map(type: type, version: version, parameters: Array(raw.dropLast())) == nil, "Short schema rejected")
            require(MaschineGroupDistortionMapping.map(type: type, version: version, parameters: raw + [0]) == nil, "Long schema rejected")
            for bad in [Double.nan, .infinity, -.infinity] {
                var invalid = raw; invalid[1] = bad
                require(MaschineGroupDistortionMapping.map(type: type, version: version, parameters: invalid) == nil, "Nonfinite payload rejected")
            }
        }
        require(MaschineGroupDistortionMapping.map(type: "Unknown", version: 0, parameters: []) == nil, "Unknown type rejected")
        print("Maschine distortion mapping tests passed (\(checks) checks)")
    }
}

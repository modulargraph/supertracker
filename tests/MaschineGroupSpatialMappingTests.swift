import Foundation
import TrackerEngine

@main
struct MaschineGroupSpatialMappingTests {
    static var checks = 0
    static func check(_ result: @autoclosure () -> Bool, _ message: String) {
        checks += 1
        if !result() { fatalError(message) }
    }
    static func physical(_ result: [String: Any], _ key: String) -> Double? {
        guard let id = result["id"] as? String, let rows = result["parameters"] as? [[String: Any]] else { return nil }
        let dspID = String(id.dropFirst(5))
        for row in rows {
            guard let address = row["address"] as? Int, let value = row["value"] as? Double,
                  let p = st_builtin_effect_parameter_info(dspID, UInt32(address))?.pointee else { continue }
            if String(cString: p.id) == key { return st_builtin_effect_from_normalized(dspID, UInt32(address), value) }
        }
        return nil
    }
    static func main() {
        let ice = MaschineGroupSpatialMapping.map(type: "NI::MASCHINE::DATA::Iceverb", version: 0, parameters: [0.2, 0.7, 0.9, 0.4])!
        check(ice["id"] as? String == "stfx:ice", "registry identifier")
        check(physical(ice, "color") == 20 && physical(ice, "ice") == 70, "Ice controls use independent stable parameter IDs")
        check(physical(ice, "mix") == 40, "Ice mix")
        check(!(ice["warnings"] as! [String]).isEmpty, "independent DSP approximation is explicit")
        let legacy = MaschineGroupSpatialMapping.map(type: "Reverb", version: 0, parameters: [2, 0.4, 0.5, 0.5, -0.75, 0.8, 0.25, 1])!
        check(physical(legacy, "pan") == -75, "signed archive pan is not clamped to zero")
        check(physical(legacy, "freeze") == 1 && physical(legacy, "mix") == 25, "archive Mix/Freeze order differs from rack order")
        check(abs(physical(legacy, "low")!) < 1e-9 && abs(physical(legacy, "high")!) < 1e-9, "EQ midpoint remains neutral")
        let centered = MaschineGroupSpatialMapping.map(type: "Reverb", version: 0, parameters: [0, 0.5, 0.5, 0.5, 0, 0, 0.5, 0])!
        check(abs(physical(centered, "stereo")! - 100) < 1e-9, "bipolar NI Stereo zero preserves normal width")
        let narrowed = MaschineGroupSpatialMapping.map(type: "Reverb", version: 0, parameters: [0, 0.5, 0.5, 0.5, 0, -0.5, 0.5, 0])!
        check(abs(physical(narrowed, "stereo")! - 50) < 1e-9, "negative NI Stereo values narrow the reverb")
        for unit in 0...2 {
            let delay = MaschineGroupSpatialMapping.map(type: "BeatDelay", version: 0, parameters: [3, 0, 0.4, 0.2, 0.8, 0.3, -1, 0.5, Double(unit)])!
            check(physical(delay, "time") == 5, "NI delay enum3 is three units, target enum5")
            check(physical(delay, "unit") == Double([2, 5, 1][unit]), "verified NI straight/triplet/32nd unit order")
            check(physical(delay, "stereo") == -100, "negative stereo preserved")
        }
        let half = MaschineGroupSpatialMapping.map(type: "BeatDelay", version: 0, parameters: [0, 2, 0.2, 0.2, 0.8, 0, 0, 0.5, 0])!
        check(physical(half, "time") == 0, "half-unit enum")
        check((half["warnings"] as! [String]).contains(where: { $0.contains("Offset") }), "unmatched secondary offset is visible")
        let grain = MaschineGroupSpatialMapping.map(type: "GrainDelay", version: 0, parameters: [-0.5, 0.25, 0, 1, 0.2, 0.5, 0, 0.3, 1])!
        check(physical(grain, "pitch") == -12 && physical(grain, "reverse") == 1, "signed grain pitch and bool")
        for (choice, steps) in [1, 2, 3, 4, 6, 8, 12, 16, 32].enumerated() {
            let stretch = MaschineGroupSpatialMapping.map(type: "GrainStretch", version: 0, parameters: [1, 0.5, Double(choice), 0.5, 0, 0.5, 1])!
            check(physical(stretch, "loop") == Double(steps), "Grain Stretch verified enum choices preserve loop length")
        }
        let reso = MaschineGroupSpatialMapping.map(type: "Resochord", version: 0, parameters: [1, 1, 2, 0.5, -12, 0.7, 0.5, 0.5, 0.3])!
        check(physical(reso, "mode") == 0 && physical(reso, "style") == 3, "Resochord mode order and minor seventh character preserved")
        check(physical(reso, "chord") == nil && physical(reso, "tune") == -12, "NI voicing enum never corrupts pitch root; raw Tune is semitones")
        let string = MaschineGroupSpatialMapping.map(type: "Resochord", version: 0, parameters: [0, 0, 1, 0.5, 0, 0.7, 0.5, 0.5, 0.3])!
        check(physical(string, "mode") == 1, "Resochord String mode maps from zero")
        let plate = MaschineGroupSpatialMapping.map(type: "PlateReverb", version: 0, parameters: [0.4, 0.5, 0.3, 0.5, 0.7])!
        check(plate["id"] as? String == "stfx:plate", "old Plate archive mode implied")
        check(physical(plate, "predelay") == 100 && physical(plate, "mix") == 30, "old Plate serialization order")
        var modern = Array(repeating: 0.5, count: 23); modern[2] = 0.42; modern[5] = 1; modern[15] = 0.3; modern[16] = 0.81
        let hall = MaschineGroupSpatialMapping.map(type: "PlateReverb", version: 1, parameters: modern)!
        check(hall["id"] as? String == "stfx:reverb" && physical(hall, "mode") == 1, "modern Hall routed to correct engine")
        check(physical(hall, "size") == 81 && physical(hall, "mix") == 42, "active Hall parameter bank selected")
        modern[21] = 0; modern[22] = 0
        let flatHall = MaschineGroupSpatialMapping.map(type: "PlateReverb", version: 1, parameters: modern)!
        check(abs(physical(flatHall, "highcut")! - 20000) < 1e-8 && abs(physical(flatHall, "lowshelf")!) < 1e-9, "NI zero-valued EQ controls preserve the neutral default, not maximum filtering")
        modern[5] = 2
        let modernPlate = MaschineGroupSpatialMapping.map(type: "PlateReverb", version: 1, parameters: modern)!
        check(modernPlate["id"] as? String == "stfx:plate", "modern Plate bank selected")
        check(MaschineGroupSpatialMapping.map(type: "Iceverb", version: 1, parameters: [0, 0, 0, 0]) == nil, "unknown versions rejected")
        check(MaschineGroupSpatialMapping.map(type: "PlateReverb", version: 1, parameters: [0, 0, 0, 0, 0]) == nil, "truncated mode banks rejected")
        check(MaschineGroupSpatialMapping.map(type: "Iceverb", version: 0, parameters: [.nan, 0, 0, 0]) == nil, "nonfinite archive rejected")
        check(MaschineGroupSpatialMapping.map(type: "Unknown", version: 0, parameters: []) == nil, "unknown module rejected")
        let invalid = MaschineGroupSpatialMapping.map(type: "Iceverb", version: 0, parameters: [-1, 0.5, 0.5, 0.5])!
        check(physical(invalid, "color") == nil, "invalid raw control does not masquerade as valid normalized value")
        check((invalid["warnings"] as! [String]).contains(where: { $0.contains("outside") }), "invalid control warning")
        print("PASS: \(checks) Maschine spatial mapping checks")
    }
}

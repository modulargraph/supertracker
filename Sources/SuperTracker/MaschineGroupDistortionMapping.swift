import Foundation
import TrackerEngine

/// Source serializer order and units were checked by static analysis; see
/// docs/research/maschine-distortion-parameter-mapping.md. These are independent
/// processors, with explicit warnings for approximate and unsupported controls.
/// The importer keeps every original field alongside these editable parameters.
enum MaschineGroupDistortionMapping {
    static func map(type: String, version: Int, parameters raw: [Double]) -> [String: Any]? {
        let type = type.components(separatedBy: "::").last ?? type
        let schemas: [String: (String, Int, Int)] = [
            "LoFi": ("lofi", 0, 5), "Saturator": ("saturator", 0, 14),
            "MullhollandDrive": ("distortion", 1, 15)]
        guard let schema = schemas[type], version == schema.1, raw.count == schema.2,
              raw.allSatisfy(\.isFinite), let info = st_builtin_effect_find(schema.0)?.pointee else { return nil }
        let id = schema.0
        var values: [UInt32: Double] = [:]
        var warnings = ["\(type) uses an independent SuperTracker effect; its sound is an approximation of the original."]
        func warn(_ text: String) { if !warnings.contains(text) { warnings.append(text) } }
        func value(_ index: Int, range: ClosedRange<Double> = 0...1) -> Double? {
            guard range.contains(raw[index]) else {
                warn("Original parameter \(index + 1) is outside its known range; the SuperTracker default was retained.")
                return nil
            }
            return raw[index]
        }
        func parameter(_ key: String, _ physical: Double) {
            guard physical.isFinite else { return }
            for address in 0..<info.parameterCount {
                guard let metadata = st_builtin_effect_parameter_info(id, address)?.pointee,
                      String(cString: metadata.id) == key else { continue }
                if physical < metadata.minimum || physical > metadata.maximum {
                    warn("\(key) exceeds the SuperTracker range and was limited to the nearest supported value.")
                }
                values[address] = st_builtin_effect_to_normalized(id, address, physical)
                return
            }
            warn("\(key) has no matching SuperTracker control and was retained only in the original import data.")
        }
        func percent(_ index: Int, _ key: String) {
            if let v = value(index) { parameter(key, 100 * v) }
        }
        func choice(_ index: Int, _ key: String, maximum: Int) -> Int? {
            let v = raw[index]
            guard v >= 0, v <= Double(maximum), v.rounded() == v else {
                warn("Unknown \(key) choice; the SuperTracker default was retained."); return nil
            }
            parameter(key, v)
            return Int(v)
        }
        func tone(_ index: Int) {
            if let v = value(index) { parameter("tone", 150 * pow(16000 / 150, v)) }
        }
        switch type {
        case "LoFi":
            // Persisted order is SR, Smooth, Bits, Stereo, Mix, unlike its UI.
            if let v = value(0) {
                parameter("sample_rate", 44100 * pow(Double(Float(bitPattern: 0x3f879c63)), -105.5 * v))
            }
            percent(1, "smooth")
            if let v = value(2) { parameter("bits", 12 - 11 * v) }
            percent(3, "stereo"); percent(4, "mix")
            warn("LoFi Smooth and Stereo use corresponding percentage controls; the original filtering and stereo algorithms are approximated.")
        case "Saturator":
            guard let mode = choice(0, "mode", maximum: 2) else {
                return ["id": "stfx:" + id, "parameters": [[String: Any]](), "warnings": warnings]
            }
            // Each source mode persists its own settings. Map only the active
            // mode into our shared knobs; inactive values stay in source data.
            switch mode {
            case 0:
                if let v = value(1) { parameter("input", 24 * v) }
                percent(2, "drive"); percent(3, "contour")
                warn("Classic Input is a source percentage control; its conversion to input gain in dB is approximated.")
            case 1:
                if let v = value(4) { parameter("input", 30 * v) }
                // Our tape Drive gives -6...6 dB of low-frequency tilt.
                if let v = value(5, range: -1...1) { parameter("drive", 50 + 25 * v) }
                if let v = value(6) { parameter("contour", 100 * log((12000 + 8000 * v) / 1000) / log(20)) }
                warn("Tape Drive and Contour preserve the source's displayed dB and frequency using approximate low-frequency tilt and low-pass controls.")
            default:
                percent(7, "drive")
                if let v = value(8, range: 0...4) { parameter("charge", 25 * v) }
                _ = value(9); _ = value(10)
                if let v = value(11, range: -12...12) { parameter("output", v) }
                if raw[12] == 0 || raw[12] == 1 { parameter("overload", raw[12] * 12) }
                else { warn("Unknown overload choice; the SuperTracker default was retained.") }
                _ = choice(13, "eq_bypass", maximum: 1)
                warn("Tube Bass and Treble have no verified dB conversion and remain neutral; their original values are retained in import data. The Overload switch uses an approximate 12 dB boost.")
            }
            warn("Inactive Saturator mode settings are retained in original import data; changing modes uses the shared SuperTracker controls.")
        case "MullhollandDrive":
            guard let mode = choice(0, "mode", maximum: 1) else {
                return ["id": "stfx:" + id, "parameters": [[String: Any]](), "warnings": warnings]
            }
            // v1 prepends Mode and six Analog fields to the eight Fuzz fields.
            percent(1, "saturation"); percent(2, "bass")
            _ = choice(4, "focus", maximum: 1); percent(5, "definition")
            percent(8, "color")
            if let v = value(9, range: -1...1) {
                if v >= 0 { parameter("feedback", v * 100) }
                else {
                    parameter("feedback", 0)
                    warn("Negative Feedback is unsupported by this processor and was disabled; its signed source value is retained in import data.")
                }
            }
            if let v = value(11, range: -1...1) {
                if v >= 0 { parameter("tone_mod", v * 100) }
                else {
                    parameter("tone_mod", 0)
                    warn("Negative Tone Mod is unsupported by this processor and was disabled; its signed source value is retained in import data.")
                }
            }
            _ = choice(12, "gate", maximum: 1)
            if let v = value(13) {
                let squared = v * v
                let shaped = squared + (squared * squared - squared) * Double(Float(bitPattern: 0x3f4f41f2))
                parameter("gate_release", 1000 * (Double(Float(bitPattern: 0x3c23d70a)) + shaped * Double(Float(bitPattern: 0x409fae14))))
            }
            if mode == 0 {
                if let v = value(7) { parameter("drive", 42 * v) }
                tone(10); percent(14, "mix")
            } else {
                parameter("drive", 0)
                tone(3); percent(6, "mix")
            }
            parameter("output", 0)
            warn("Distortion Drive and Tone use approximate gain and frequency curves. Inactive mode values remain in original import data.")
        default: return nil
        }
        let parameters: [[String: Any]] = values.keys.sorted().map { ["address": Int($0), "value": values[$0]!] }
        return ["id": "stfx:" + id, "parameters": parameters, "warnings": warnings]
    }
}

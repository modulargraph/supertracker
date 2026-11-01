import Foundation
import TrackerEngine

/// Saved-parameter interoperability for our independent drum voices. Archive
/// arrays are serializer order, never UI/constructor order. Only verified units
/// are translated; NI engine algorithms and percent Decay curves are not copied.
/// Evidence: docs/research/maschine-drum-parameter-evidence.json.
enum MaschineGroupDrumMapping {
    static func map(type: String, version: Int, parameters raw: [Double]) -> [String: Any]? {
        let type = type.components(separatedBy: "::").last ?? type
        let shape: (version: Int, count: Int, model: String, engines: ClosedRange<Int>)
        switch type {
        case "Kick": shape = (2, 31, "kick", 0...9)
        case "Snare": shape = (1, 22, "snare", 0...9)
        case "Hihat": shape = (2, 18, "hihat", 0...2)
        case "Tom": shape = (1, 23, "tom", 0...3)
        case "Perc": shape = (1, 29, "perc", 0...2)
        case "Cymbal": shape = (0, 13, "cymbal", 0...1)
        default: return nil
        }
        guard version == shape.version, raw.count == shape.count,
              raw.allSatisfy(\.isFinite), raw[0].rounded(.towardZero) == raw[0],
              raw[0] >= 0, raw[0] <= Double(Int32.max) else { return nil }
        let model = st_drum_model_for_id(shape.model)
        guard model > 0 else { return nil }
        let engine = Int(raw[0])
        var values = (0..<8).map { st_drum_parameter_default(model, UInt32($0)) }
        var mapped: [Int] = []
        var warnings = ["\(type) engine \(engine) uses an independent SuperTracker drum model; timbre, base pitch and velocity response are approximations."]
        func value(_ index: Int) -> Double? {
            guard (0...1).contains(raw[index]) else {
                warnings.append("Original \(type) parameter \(index + 1) is outside its verified range; the corresponding default was retained.")
                return nil
            }
            return raw[index]
        }
        func set(_ target: Int, _ physical: Double) {
            guard let info = st_drum_parameter_info(model, UInt32(target))?.pointee else { return }
            if physical < info.minimum || physical > info.maximum {
                warnings.append("\(String(cString: info.name)) exceeds the SuperTracker range and was limited to its supported range.")
            }
            values[target] = st_drum_parameter_to_normalized(model, UInt32(target), physical)
            mapped.append(target)
        }
        if shape.engines.contains(engine) {
            // Verified numeric Tune display slopes. Preserve the semitone offset
            // from the source midpoint; do not claim our model has NI's base tone.
            let span: Double?
            switch type {
            case "Kick": span = [0:24.0, 1:24, 3:24, 4:24, 5:24, 7:12][engine]
            case "Snare": span = [0:24.0, 4:24, 5:18, 6:24, 7:24, 9:24][engine]
            case "Tom": span = [0:24.0, 1:24, 2:14, 3:14][engine]
            case "Perc": span = [0:48.0, 1:24][engine]
            default: span = nil
            }
            if let span, let tune = value(1) {
                set(0, span * (tune - 0.5))
                warnings.append("Tune preserves the verified relative semitone offset around the original control midpoint, not the original engine's absolute pitch.")
            } else {
                warnings.append("This engine's Tune has no verified semitone conversion; the SuperTracker Tune default was retained.")
            }
            // Shared Impact is inactive in Sub Kick, Volt/other electronic
            // Snares and Shaker. Never import a saved inactive control value.
            let impactIndex: Int?
            switch type {
            case "Kick": impactIndex = engine > 0 ? 4 : nil
            case "Snare": impactIndex = (2...7).contains(engine) ? 4 : nil
            case "Tom": impactIndex = 4
            case "Perc": impactIndex = engine < 2 ? 26 : nil
            case "Cymbal": impactIndex = 3
            default: impactIndex = nil // Hihat's two Strike controls are distinct.
            }
            if let index = impactIndex, let impact = value(index) { set(3, impact * 100) }
        } else {
            warnings.append("Unknown original engine selection; all SuperTracker drum defaults were retained.")
        }
        warnings.append("Original Decay is an engine-specific percentage with no verified millisecond conversion. Decay and other unmapped controls retain SuperTracker defaults; original parameter data must remain in the import metadata.")
        return ["drumModel": shape.model, "drumParameters": values,
                "mappedParameters": mapped.sorted(), "originalEngine": engine, "warnings": warnings]
    }
}

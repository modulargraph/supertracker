import Foundation
import TrackerEngine

/// Maps decoded numeric archive fields, in serializer order, to our own effect
/// metadata. This performs no archive scanning, plug-in loading, or audio I/O.
/// EQ v0: LF frequency/gain, M1 frequency/gain, M2 frequency/gain,
/// HF frequency/gain, M1 width, M2 width, output gain.
/// Compressor v0: threshold, amount, knee, attack, release, gain.
/// Compressor v1: mode followed by the same six values. Its separately serialized
/// SideChainFilter object is not part of this array.
/// Evidence: docs/research/maschine-core-parameter-evidence.json.
enum MaschineGroupCoreMapping {
    static func map(type: String, version: Int, parameters raw: [Double]) -> [String: Any]? {
        let type = type.components(separatedBy: "::").last ?? type
        let id: String
        switch type {
        case "EQ":
            guard version == 0, raw.count == 11 else { return nil }
            id = "eq"
        case "Compressor":
            guard (version == 0 && raw.count == 6) || (version == 1 && raw.count == 7) else { return nil }
            id = "compressor"
        default: return nil
        }
        guard raw.allSatisfy(\.isFinite), let info = st_builtin_effect_find(id)?.pointee else { return nil }
        var values: [UInt32: Double] = [:]
        var warnings = ["\(type) uses an independent SuperTracker effect; its sound is an approximation of the original."]
        func warn(_ message: String) { if !warnings.contains(message) { warnings.append(message) } }
        func value(_ index: Int, range: ClosedRange<Double> = 0...1) -> Double? {
            guard range.contains(raw[index]) else {
                warn("Original parameter \(index + 1) is outside its verified range; the SuperTracker default was retained.")
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
        if type == "EQ" {
            for (index, key, low) in [(0, "lowfreq", 20.0), (2, "lowmidfreq", 40.0),
                                      (4, "highmidfreq", 40.0), (6, "highfreq", 50.0)] {
                if let v = value(index) { parameter(key, low * pow(400, v)) }
            }
            for (index, key) in [(1, "lowgain"), (3, "lowmidgain"), (5, "highmidgain"),
                                 (7, "highgain"), (10, "gain")] {
                if let v = value(index, range: -1...1) { parameter(key, 20 * v) }
            }
            // Source's converter format is "%.1f oc": these are octaves,
            // not Q. Both our EQ and the source expose physical bandwidth.
            for (index, key) in [(8, "lowmidwidth"), (9, "highmidwidth")] {
                if let v = value(index) { parameter(key, 0.1 + 3.9 * v) }
            }
        } else {
            let start = version == 1 ? 1 : 0
            if version == 0 { parameter("mode", 0) }
            else if raw[0] == 0 || raw[0] == 1 { parameter("mode", raw[0]) }
            else { warn("Unknown compressor mode; the SuperTracker default was retained.") }
            if let v = value(start) { parameter("threshold", -60 * v) }
            if let v = value(start + 3) { parameter("attack", 8 * v * v * v) }
            if let v = value(start + 4) {
                // The NI converter stores this base as IEEE float 0x3f8f9e4d.
                let base = Double(Float(bitPattern: 0x3f8f9e4d))
                parameter("release", min(251, max(40, pow(base, 32 + 16 * v))))
            }
            if let v = value(start + 5) { parameter("gain", -36 + 36 * v) }
            _ = value(start + 1); _ = value(start + 2)
            warn("Original Amount and Knee use different controls with no verified ratio/dB conversion; SuperTracker Ratio and Knee defaults were retained.")
            warn("Original compressor side-chain source and filter settings are stored separately and were not mapped by this helper.")
        }
        let parameters: [[String: Any]] = values.keys.sorted().map { ["address": Int($0), "value": values[$0]!] }
        return ["id": "stfx:" + id, "parameters": parameters, "warnings": warnings]
    }
}

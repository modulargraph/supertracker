import Foundation
import TrackerEngine

/// Raw Maschine serializer fields mapped into independent first-party processors.
/// Verified unit conversions and deliberately approximate perceptual curves are
/// distinguished in docs/research/maschine-additional-parameter-mapping.md.
/// The importer retains the raw fields alongside these editable controls.
enum MaschineGroupAdditionalMapping {
    static func map(type: String, version: Int, parameters raw: [Double]) -> [String: Any]? {
        let type = type.components(separatedBy: "::").last ?? type
        let schemas: [String: (String, Int, Int)] = [
            "Gate": ("gate", 0, 5), "Limiter": ("limiter", 1, 4),
            "Maximizer": ("maximizer", 0, 3), "TransientDesigner": ("transient", 0, 4),
            "CabinetEmulation": ("cabinet", 0, 4), "Chorus": ("chorus", 0, 3),
            "FM": ("fm", 0, 4), "Filter": ("filter", 0, 11),
            "Flanger": ("flanger", 0, 8), "FreqShifter": ("freqshifter", 0, 6),
            "Phaser": ("phaser", 0, 9)]
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
        func percent(_ index: Int, _ key: String, signed: Bool = false) {
            if let v = value(index, range: signed ? -1...1 : 0...1) { parameter(key, 100 * v) }
        }
        func choice(_ index: Int, _ key: String, maximum: Int) {
            let v = raw[index]
            guard v >= 0, v <= Double(maximum), v.rounded() == v else {
                warn("Unknown \(key) choice; the SuperTracker default was retained."); return
            }
            parameter(key, v)
        }
        // These are explicit SuperTracker approximations of controls whose
        // original UI displays percent, not claims of recovered NI DSP laws.
        func curve(_ index: Int, _ key: String, _ low: Double, _ high: Double, log: Bool = false) {
            if let v = value(index) { parameter(key, log ? low * pow(high / low, v) : low + (high - low) * v) }
        }
        func modulation(_ speed: Int, _ shape: Int, _ stereo: Int, source: Int?) {
            if let source { choice(source, "source", maximum: 2) }
            else { parameter("source", 0) } // Flanger v0 predates the persisted Source enum.
            curve(speed, "speed", 0.03, 8, log: true)
            curve(shape, "envshape", 0.25, 4, log: true)
            curve(stereo, "stereo", 0, 180)
            warn("Modulation speed, shape and stereo curves are approximated. The original continuously variable LFO shape is not reproduced.")
            if let source, raw[source] == 1 {
                warn("The original synced speed choice has no verified division mapping; the four-beat LFO period was retained.")
            }
        }
        switch type {
        case "Gate":
            if let v = value(0, range: Double(Float(0.1))...1) { parameter("threshold", 40 * log10(v)) }
            curve(1, "attack", 0.05, 100, log: true)
            curve(2, "hold", 0, 1000)
            curve(3, "release", 1, 2000, log: true)
            percent(4, "mix")
            warn("Gate Attack, Hold and Release display percentages in the original; their millisecond curves are approximated.")
        case "Limiter":
            // Serializer order differs from its UI: Threshold, Mode, Release, Ceiling.
            if let v = value(0, range: -40...0) { parameter("threshold", v) }
            choice(1, "mode", maximum: 1)
            if let v = value(2) {
                parameter("release", min(500, max(1, pow(Double(Float(bitPattern: 0x402df854)),
                                                       Double(Float(bitPattern: 0x40c6de12)) * v))))
            }
            if let v = value(3) { parameter("ceiling", -40 + 39.7 * v) }
            warn("Original limiter side-chain routing and filter settings are stored separately and were not mapped by this helper.")
        case "Maximizer":
            curve(0, "amount", 0, 30); percent(1, "curve"); choice(2, "turbo", maximum: 1)
            warn("Maximizer Amount is translated from its original percentage into an approximate drive in dB.")
        case "TransientDesigner":
            // Constructor order is Attack/Sustain/Gain; persisted order starts with Gain.
            if let v = value(0, range: -1...1) { parameter("gain", v * (v < 0 ? 24 : 6)) }
            percent(1, "attack", signed: true); percent(2, "sustain", signed: true); choice(3, "limit", maximum: 1)
        case "CabinetEmulation":
            choice(0, "cabinet", maximum: 3); choice(1, "microphone", maximum: 5)
            percent(2, "distance"); percent(3, "mix")
            warn("Cabinet and microphone names correspond to similar filter characters; the original cabinet responses are not included.")
        case "Chorus":
            curve(0, "rate", 0.03, 8, log: true); percent(1, "amount"); percent(2, "mix")
            warn("Chorus Rate is translated from its original percentage using an approximate frequency curve.")
        case "FM":
            curve(0, "rate", 0.1, 10000, log: true); curve(1, "split", 20, 16000, log: true)
            percent(2, "contour"); percent(3, "amount")
            warn("FM Rate and Split use approximate frequency curves; the original controls display percentages.")
        case "Filter":
            choice(0, "mode", maximum: 3)
            // Installed converter endpoints are 46 and 12540 Hz.
            if let v = value(1) { parameter("cutoff", 46 * pow(12540 / 46, v)) }
            percent(2, "resonance")
            if let v = value(3, range: -1...1) { parameter("amount", 5 * v) }
            choice(4, "source", maximum: 2)
            curve(5, "speed", 0.03, 16, log: true)
            // A continuously morphing source shape is not an enum. Retain our
            // sine default rather than rounding 0...1 into unrelated shapes.
            _ = value(6)
            curve(7, "phase", 0, 360); curve(8, "decay", 5, 2000, log: true)
            curve(9, "smooth", 0.1, 100, log: true); curve(10, "envshape", 0.25, 4, log: true)
            warn("Filter modulation depth, timing, phase and envelope curves are approximated. The original morphing LFO shape uses the SuperTracker sine default.")
            if raw[4] == 1 { warn("The original synced speed choice has no verified division mapping; the four-beat LFO period was retained.") }
        case "Flanger":
            curve(0, "frequency", 20, 5000, log: true); percent(1, "amount", signed: true)
            modulation(2, 3, 4, source: nil)
            percent(5, "feedback"); choice(6, "invert", maximum: 1); percent(7, "mix")
            warn("Flanger frequency uses an approximate curve. Version 0 has no saved Source field and uses free LFO mode.")
        case "Phaser":
            curve(0, "frequency", 20, 10000, log: true); percent(1, "amount", signed: true)
            modulation(3, 4, 5, source: 2)
            percent(6, "feedback"); choice(7, "eightpole", maximum: 1); percent(8, "mix")
            warn("Phaser frequency uses an approximate curve.")
        case "FreqShifter":
            // Neither original percentage control establishes a signed Hz law.
            // Preserve the source rather than invent a pitch shift on import.
            _ = value(0); _ = value(1)
            percent(2, "feedback"); percent(3, "stereo"); choice(4, "invert", maximum: 1); percent(5, "mix")
            warn("Frequency Shifter Coarse and Fine have no verified Hz conversion; both target shifts remain at zero. The original values are retained in import data.")
        default: return nil
        }
        let parameters: [[String: Any]] = values.keys.sorted().map { ["address": Int($0), "value": values[$0]!] }
        return ["id": "stfx:" + id, "parameters": parameters, "warnings": warnings]
    }
}

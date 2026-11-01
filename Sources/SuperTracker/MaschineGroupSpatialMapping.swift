import Foundation
import TrackerEngine

/// Interoperability mapping of serialized Maschine spatial FX controls. The
/// input contains raw numeric archive values: some are signed, integer or enum
/// values, so treating the whole array as normalized 0...1 would corrupt it.
/// Parameter order was checked against the installed Maschine 3.7.1 serializers.
/// Our DSP is independent; perceptual curves without a verified unit conversion
/// are approximations and are explicitly reported with the imported effect.
enum MaschineGroupSpatialMapping {
    static func map(type: String, version: Int, parameters raw: [Double]) -> [String: Any]? {
        let type = type.components(separatedBy: "::").last ?? type
        let counts = ["Iceverb": 4, "Ice": 4, "Reflex": 4, "Metaverb": 5,
                      "Reverb": 8, "BeatDelay": 9, "GrainDelay": 9,
                      "GrainStretch": 7, "Resochord": 9]
        if type == "PlateReverb" {
            guard (version == 0 && raw.count == 5) || (version == 1 && raw.count == 23) else { return nil }
        } else {
            guard version == 0, let count = counts[type], raw.count == count else { return nil }
        }
        guard raw.allSatisfy(\.isFinite) else { return nil }
        let ids = ["Iceverb": "ice", "Ice": "ice", "Reflex": "reflex", "Metaverb": "metaverb",
                   "Reverb": "reverblegacy", "BeatDelay": "beatdelay", "GrainDelay": "graindelay",
                   "GrainStretch": "grainstretch", "Resochord": "resochord"]
        let plateMode: Int? = type == "PlateReverb" ? (version == 0 ? 2 : exactInteger(raw[5], range: 0...2)) : nil
        if type == "PlateReverb" && plateMode == nil { return nil }
        let id = ids[type] ?? (plateMode == 2 ? "plate" : "reverb")
        guard let info = st_builtin_effect_find(id)?.pointee else { return nil }
        var values: [UInt32: Double] = [:]
        var warnings = ["\(type) uses an independent SuperTracker effect; its sound is an approximation of the original."]
        func warn(_ text: String) { if !warnings.contains(text) { warnings.append(text) } }
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
        func value(_ index: Int, range: ClosedRange<Double> = 0...1) -> Double? {
            let v = raw[index]
            guard range.contains(v) else {
                warn("Original parameter \(index + 1) is outside its known range; the SuperTracker default was retained.")
                return nil
            }
            return v
        }
        func percent(_ index: Int, _ key: String) { if let v = value(index) { parameter(key, v * 100) } }
        func signedPercent(_ index: Int, _ key: String) { if let v = value(index, range: -1...1) { parameter(key, v * 100) } }
        func toggle(_ index: Int, _ key: String) {
            guard raw[index] == 0 || raw[index] == 1 else { warn("Invalid \(key) switch value; the default was retained."); return }
            parameter(key, raw[index])
        }
        func curve(_ index: Int, _ key: String, low: Double, high: Double, logarithmic: Bool = false) {
            if let v = value(index) { parameter(key, logarithmic ? low * pow(high / low, v) : low + (high - low) * v) }
        }
        func eq(_ index: Int, _ key: String) {
            if let v = value(index) { parameter(key, v < 0.5 ? (v - 0.5) * 48 : (v - 0.5) * 24) }
        }
        switch type {
        case "Iceverb", "Ice":
            percent(0, "color"); percent(1, "ice"); percent(2, "size"); percent(3, "mix")
        case "Reflex":
            percent(0, "color"); percent(1, "smooth"); percent(2, "size"); percent(3, "mix")
        case "Metaverb":
            percent(0, "size"); eq(1, "low"); eq(2, "high"); signedPercent(3, "pan"); percent(4, "mix")
            warn("Low and High EQ curves are approximated around their neutral midpoint.")
        case "Reverb":
            if let room = exactInteger(raw[0], range: 0...3) { parameter("room", Double(room)) }
            else { warn("Unrecognized legacy room type; General was retained.") }
            percent(1, "size"); eq(2, "low"); eq(3, "high"); signedPercent(4, "pan")
            // NI Stereo is bipolar with zero as its neutral default. Our width is unipolar.
            if let stereo = value(5, range: -1...1) { parameter("stereo", 100 + stereo * (stereo < 0 ? 100 : 50)) }
            percent(6, "mix"); toggle(7, "freeze")
            warn("Legacy room, EQ and stereo response are approximated.")
        case "BeatDelay":
            if let time = exactInteger(raw[0], range: 0...16) { parameter("time", time == 0 ? 0 : Double(time * 2 - 1)) }
            else { warn("Unknown delay Time choice; the default was retained.") }
            if raw[1] != 0 { warn("The original delay Offset uses a different secondary-tap routing and was not applied.") }
            percent(2, "feedback"); percent(3, "crossover"); percent(4, "color"); percent(5, "split")
            signedPercent(6, "stereo"); percent(7, "mix")
            if let unit = exactInteger(raw[8], range: 0...2) { parameter("unit", Double([2, 5, 1][unit])) }
            else { warn("Unknown delay Unit choice; the default was retained.") }
        case "GrainDelay":
            if let v = value(0, range: -1...1) { parameter("pitch", v * 24) }
            curve(1, "size", low: 5, high: 500, logarithmic: true); percent(2, "jitter"); toggle(3, "reverse")
            curve(4, "space", low: 0, high: 2000); percent(5, "density"); percent(6, "mod"); percent(7, "mix"); percent(8, "stereo")
            warn("Grain pitch, size and spacing curves are approximated; grain timing is not NI-identical.")
        case "GrainStretch":
            // Persist the capture switch, but no captured audio is embedded in this module state.
            toggle(0, "on"); curve(1, "stretch", low: 0, high: 100)
            // Verified from the installed module's enum labels. Values are sixteenth-note steps.
            if let loop = exactInteger(raw[2], range: 0...8) {
                parameter("loop", Double([1, 2, 3, 4, 6, 8, 12, 16, 32][loop]))
            } else { warn("Unknown Grain Stretch loop length; the default was retained.") }
            curve(3, "pitch", low: -24, high: 24); toggle(4, "link")
            curve(5, "size", low: 5, high: 300, logarithmic: true); percent(6, "mix")
            warn("Grain Stretch pitch, size and stretch curves are approximated. Fresh audio must be captured.")
        case "Resochord":
            // NI's Mode labels are String, Chord; this rack presents Chord first.
            if let mode = exactInteger(raw[1], range: 0...1) { parameter("mode", Double(1 - mode)) }
            else { warn("Unknown Resochord mode; the default was retained.") }
            if let style = exactInteger(raw[0], range: 0...1) {
                // Common major/minor triad and seventh variants have a close musical counterpart.
                if raw[2] == 1 { parameter("style", Double(style)) }
                else if raw[2] == 2 || raw[2] == 3 { parameter("style", Double(style == 0 ? 2 : 3)) }
                else { parameter("style", Double(style)) }
            } else { warn("Resochord alt/meta/frank voicings are not matched; the default Major style was retained.") }
            percent(3, "spread")
            if let tune = value(4, range: -24...24), tune.rounded() == tune { parameter("tune", tune) }
            percent(5, "brightness"); percent(6, "feedback"); curve(7, "decay", low: 0.03, high: 20, logarithmic: true); percent(8, "mix")
            warn("Resochord voicing and Decay are approximated; NI chord choices are not pitch-root indices.")
        case "PlateReverb":
            if plateMode == 2 {
                curve(0, "predelay", low: 0, high: 250); curve(1, "decay", low: 0.2, high: 20.2, logarithmic: true)
                percent(2, "mix"); curve(3, "lowshelf", low: 0, high: -24); percent(4, "highdamp")
            } else {
                let start = plateMode == 0 ? 7 : 15
                parameter("mode", Double(plateMode!)); curve(start, "time", low: 0.5, high: 20.2, logarithmic: true)
                percent(start + 1, "size"); percent(start + 2, "damping"); percent(start + 3, "modulation")
                percent(start + 4, "diffusion"); curve(start + 5, "predelay", low: 0, high: 250)
                curve(start + 6, "highcut", low: 20000, high: 2000, logarithmic: true)
                curve(start + 7, "lowshelf", low: 0, high: -12); percent(2, "mix")
            }
            warn("Reverb time, EQ and predelay curves are approximated. The original wet-gain control is not applied.")
        default: return nil
        }
        let parameters: [[String: Any]] = values.keys.sorted().map { ["address": Int($0), "value": values[$0]!] }
        return ["id": "stfx:" + id, "parameters": parameters, "warnings": warnings]
    }

    private static func exactInteger(_ value: Double, range: ClosedRange<Int>) -> Int? {
        guard value.isFinite, value.rounded() == value,
              value >= Double(range.lowerBound), value <= Double(range.upperBound) else { return nil }
        return Int(value)
    }
}

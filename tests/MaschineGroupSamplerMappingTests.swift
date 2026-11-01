import Foundation

@main
struct SamplerMappingTests {
    static var checks = 0
    static func require(_ condition: @autoclosure () -> Bool, _ message: String) {
        checks += 1; if !condition() { fatalError(message) }
    }
    static func close(_ actual: Any?, _ expected: Double, _ message: String, tolerance: Double = 0.00001) {
        guard let actual = actual as? Double else { fatalError(message + " has no mapped value") }
        require(abs(actual - expected) <= max(tolerance, abs(expected) * tolerance), message)
    }
    static func main() throws {
        for (raw, expected) in [(0.0, 0.0), (0.25, 0.050305371172726154), (0.5, 0.26670458912849426), (0.75, 1.0), (1.0, 3.1622776985168457)] {
            close(MaschineGroupSamplerMapping.levelAmplitude(raw), expected, "native LevelScale golden point \(raw)", tolerance: 0.0000002)
        }
        let levels = (0...100).map { MaschineGroupSamplerMapping.levelAmplitude(Double($0) / 100)! }
        require(zip(levels, levels.dropFirst()).allSatisfy { $0 < $1 }, "native fader curve is strictly increasing")
        require([-0.01, 1.01, Double.infinity, Double.nan].allSatisfy { MaschineGroupSamplerMapping.levelAmplitude($0) == nil }, "invalid source gain cannot become an unsafe amplitude")
        var raw: [String: Double] = ["0xe8": 2, "0xf8": -0.25, "0xc8": 1, "0x148": 1, "0x150": 0.5, "0x158": 0.2, "0x190": 1]
        for prefix in ["ampEnvelope", "filterEnvelope"] {
            for i in 0..<9 { raw["\(prefix).\(i)"] = 0 }
        }
        raw["ampEnvelope.0"] = 0.5; raw["ampEnvelope.2"] = 1
        raw["ampEnvelope.3"] = 0.5; raw["ampEnvelope.4"] = 0.25
        raw["filterEnvelope.0"] = 0.25; raw["filterEnvelope.2"] = 0.5
        raw["filterEnvelope.3"] = 0.75; raw["filterEnvelope.4"] = 1
        let result = MaschineGroupSamplerMapping.map(parameters: raw), fields = result["instrument"] as! [String: Any]
        let physical = result["physical"] as! [String: Double]
        close(fields["tune"], -9, "main sampler tune is 36*u semitones")
        require(fields["voiceMode"] as? String == "mono", "polyphony1 maps to Mono")
        close(fields["sustain"], 0.125, "amp sustain uses cubic amplitude")
        close(fields["filterSustain"], 0.421875, "second nine-field envelope is independent")
        close(fields["attack"], 0.171032, "source attack midpoint is about171 ms", tolerance: 0.0001)
        close(fields["decay"], 12.31959, "source maximum decay is about12.32 s", tolerance: 0.0001)
        require((fields["release"] as! Double) > 0.05 && (fields["release"] as! Double) < 0.1, "quarter release uses the recovered curve, not seconds directly")
        require((fields["filterAttack"] as! Double) < (fields["attack"] as! Double), "filter attack never reads shifted amp modulation fields")
        require((fields["filterRelease"] as! Double) == (fields["decay"] as! Double), "same normalized decay/release uses same physical law")
        require(fields["volume"] == nil, "velocity-to-volume never overwrites instrument gain")
        require(fields["filterType"] as? String == "lowpass", "LP2 enum")
        require((fields["cutoff"] as! Double) > 950 && (fields["cutoff"] as! Double) < 1100, "LP cutoff midpoint is physically converted")
        close(fields["resonance"], 0.2, "resonance approximation is bounded")
        require((result["warnings"] as! [String]).contains { $0.contains("resonance") && $0.contains("approximation") }, "approximate resonance is disclosed")
        require(physical["ampEnvelopeHoldSeconds"] == 0, "zero Hold remains zero")
        for mode in [0.0, 1, 2] {
            raw["0xe8"] = mode
            let mapped = MaschineGroupSamplerMapping.map(parameters: raw), instrument = mapped["instrument"] as! [String: Any]
            if mode == 0 { close(instrument["attack"], 0, "One-shot ignores stored ADSR attack"); close(instrument["sustain"], 1, "One-shot stays full amplitude") }
            if mode == 1 { close(instrument["sustain"], 0, "AHD approximates AD without sustain") }
            require(!(mapped["warnings"] as! [String]).isEmpty, "mode limitations visible")
        }
        raw["0xe8"] = 2; raw["0x148"] = 3
        let hp = MaschineGroupSamplerMapping.map(parameters: raw)["instrument"] as! [String: Any]
        require(hp["filterType"] as? String == "highpass", "HP2 enum is3 not2")
        require((hp["cutoff"] as! Double) < (fields["cutoff"] as! Double), "HP uses its own source display factor")
        for mode in [2.0, 4.0] {
            raw["0x148"] = mode
            let unsupported = MaschineGroupSamplerMapping.map(parameters: raw)
            require((unsupported["instrument"] as! [String: Any])["cutoff"] == nil, "unverified BP/EQ mapping is not invented")
        }
        raw["0xc8"] = 0; raw["0xf0"] = 1; raw["filterEnvelope.8"] = 0.7
        raw["unrecognizedFutureField"] = 0.37
        let preserved = MaschineGroupSamplerMapping.map(parameters: raw)
        require((preserved["sourceParameters"] as! [String: Any])["filterEnvelope.8"] as? Double == 0.7, "ninth envelope field retained")
        require((preserved["sourceParameters"] as! [String: Any])["unrecognizedFutureField"] as? Double == 0.37, "unknown raw values preserved")
        require((preserved["instrument"] as! [String: Any])["voiceMode"] as? String == "legato", "Legato enum0 is not Mono")
        require((preserved["instrument"] as! [String: Any])["mode"] == nil, "reversed one-shot does not accidentally become reverse-loop mode")
        raw["0xf8"] = Double.infinity; raw["0x100"] = 1e300; raw["ampEnvelope.0"] = -0.2
        let invalid = MaschineGroupSamplerMapping.map(parameters: raw), invalidFields = invalid["instrument"] as! [String: Any]
        require(invalidFields["tune"] == nil && invalidFields["attack"] == nil, "invalid values keep destination defaults")
        require((invalid["physical"] as! [String: Double])["startOffsetFraction"] == nil, "extreme start cannot overflow a mapped scalar")
        require((invalid["sourceParameters"] as! [String: Any])["0xf8"] is String, "nonfinite source remains diagnostic data")
        require(JSONSerialization.isValidJSONObject(invalid), "rejected data remains safely serializable")
        print("Maschine sampler mapping: \(checks) checks passed")
    }
}

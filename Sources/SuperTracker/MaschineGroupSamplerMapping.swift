import Foundation

/// Pure mapping of verified native Sampler fields. Archive reading is separate.
/// Source evidence: docs/research/maschine-sampler-parameter-evidence.json.
/// Times and cutoff are recovered physical values; our envelope/filter DSP is
/// independent. Unsupported controls stay in sourceParameters, never guessed.
enum MaschineGroupSamplerMapping {
    /// Native LevelScale and sampler Zone gain both use this fifth-order law.
    /// 0.75 is unity and 1 is +10 dB; this is not a fourth-power fader curve.
    static func levelAmplitude(_ raw: Double) -> Double? {
        guard raw.isFinite, (0...1).contains(raw) else { return nil }
        let value = Float(raw), square = value * value
        let fourth = square * (square * Float(bitPattern: 0x40194494))
        return Double((fourth * value).addingProduct(square, Float(bitPattern: 0x3f4478b8)))
    }

    static func map(parameters raw: [String: Double]) -> [String: Any] {
        var fields: [String: Any] = [:], physical: [String: Double] = [:], warnings: [String] = []
        func warn(_ text: String) { if !warnings.contains(text) { warnings.append(text) } }
        func value(_ key: String, _ range: ClosedRange<Double> = 0...1) -> Double? {
            guard let v = raw[key] else { return nil }
            guard v.isFinite, range.contains(v) else {
                warn("Sampler control \(key) is outside its verified range; the destination default was retained."); return nil
            }
            return v
        }
        func nonzero(_ key: String) -> Bool { raw[key].map { $0.isFinite && abs($0) > 0.000001 } ?? false }
        if let v = value("0xf8", -1...1) { fields["tune"] = 36 * v; physical["tuneSemitones"] = 36 * v }
        if let voices = value("0xc8", 0...7), voices.rounded() == voices {
            fields["voiceMode"] = voices == 0 ? "legato" : voices == 1 ? "mono" : "poly"
            if voices > 1 { warn("Sampler polyphony uses SuperTracker's voice pool; the original per-Sound voice limit is retained in the import data.") }
        }
        if nonzero("0xe0") { warn("The original vintage sampler engine mode is not reproduced; standard sample playback is used.") }
        if nonzero("0xf0") { warn("Sampler Reverse is retained but not applied: SuperTracker's reverse mode also loops, unlike a reversed one-shot.") }
        if let start = value("0x100"), start > 0 { physical["startOffsetFraction"] = start * start; warn("The sampler-wide Start offset is retained; sample-zone trim positions are preserved without guessing how to combine them.") }
        if nonzero("0xd0") { warn("Sampler Glide is retained but was not mapped to a persistent portamento control.") }
        if nonzero("0xd8") { warn("The sampler Pitchbend range is retained; the original MIDI bend range is not reproduced.") }
        let envelopeMode = value("0xe8", 0...2)
        for (prefix, destination) in [("ampEnvelope", ""), ("filterEnvelope", "filter")] {
            for (index, field, curve) in [(0, "Attack", attackCurve), (1, "Hold", holdCurve), (2, "Decay", decayCurve), (4, "Release", decayCurve)] {
                if let v = value("\(prefix).\(index)") {
                    let seconds = envelopeSeconds(v, curve: curve, decay: index == 2 || index == 4)
                    physical[prefix + field + "Seconds"] = seconds
                    if index == 1 {
                        if seconds > 0.000001 && (prefix != "ampEnvelope" || envelopeMode != 0) { warn("\(prefix) Hold has no matching ADSR stage and was retained without applying it.") }
                    } else if prefix != "ampEnvelope" || envelopeMode == 1 || envelopeMode == 2 {
                        fields[destination.isEmpty ? field.lowercased() : destination + field] = seconds
                    }
                }
            }
            if let v = value(prefix + ".3") {
                physical[prefix + "Sustain"] = v * v * v
                if prefix != "ampEnvelope" || envelopeMode == 2 { fields[destination.isEmpty ? "sustain" : "filterSustain"] = v * v * v }
            }
            if (5...8).contains(where: { nonzero("\(prefix).\($0)") }) {
                warn("\(prefix) modulation to pitch, cutoff, drive or pan is retained; no unverified modulation-depth conversion was applied.")
            }
        }
        if envelopeMode == 0 {
            fields["attack"] = 0.0; fields["decay"] = 0.0; fields["sustain"] = 1.0
            warn("One-shot envelope timing follows SuperTracker's note gates; Maschine's key-release-independent behavior is not reproduced.")
        } else if envelopeMode == 1 {
            fields["sustain"] = 0.0
            warn("AHD is approximated by an AD envelope; Hold and key-release behavior differ.")
        } else if envelopeMode == 2 {
            warn("Sampler ADSR uses recovered physical stage times and sustain, with SuperTracker's independent envelope curves.")
        } else if raw["0xe8"] != nil { warn("Unknown sampler envelope type; the destination envelope was retained.") }
        if let mode = value("0x148", 0...4), mode.rounded() == mode {
            if mode == 0 {
                fields["cutoff"] = 22000.0; fields["resonance"] = 0.0; fields["filterType"] = "lowpass"
                warn("Sampler Filter Off is approximated by SuperTracker's fully open low-pass filter.")
            } else if mode == 1 || mode == 3 {
                fields["filterType"] = mode == 1 ? "lowpass" : "highpass"
                if let v = value("0x150") {
                    let hz = cutoffHertz(v, highpass: mode == 3)
                    fields["cutoff"] = min(22000, max(20, hz)); physical["cutoffHertz"] = hz
                }
                if let v = value("0x158") {
                    fields["resonance"] = min(0.99, v)
                    if v > 0 { warn("Filter resonance keeps its normalized amount as an approximation; the original filter's resonance response differs.") }
                }
                warn("Sampler filter cutoff follows the original physical display value; SuperTracker uses an independent filter implementation.")
            } else { warn("Sampler band-pass/EQ mode has no verified equivalent cutoff mapping and was retained without applying it.") }
        }
        for (key, name) in [("0x128","Compressor"),("0x130","Drive"),("0x138","sample-rate reduction"),("0x140","bit reduction")] where nonzero(key) {
            warn("Sampler \(name) is retained but its processing/control conversion was not verified.")
        }
        if ["0x178","0x180","0x188","0x198","0x1a0","0x1a8","0x1b0"].contains(where: nonzero) || raw["0x190"].map({ $0 != 1 }) == true {
            warn("Sampler velocity/mod-wheel routes are retained; SuperTracker's standard note-velocity behavior is used.")
        }
        if raw.values.contains(where: { !$0.isFinite }) { warn("Nonfinite sampler source controls were preserved as diagnostic strings and were not applied.") }
        let source: [String: Any] = raw.mapValues { $0.isFinite ? $0 as Any : String(describing: $0) }
        return ["instrument": fields, "physical": physical, "warnings": warnings, "sourceParameters": source]
    }
    private static func lookup(_ value: Double, _ table: [Float]) -> Float {
        let scaled = min(127, max(0, Float(value) * 127)), lower = Int(scaled)
        if lower == 127 { return table[lower] }
        return table[lower].addingProduct(table[lower + 1] - table[lower], scaled - Float(lower))
    }
    private static func envelopeSeconds(_ value: Double, curve: [Float], decay: Bool) -> Double {
        let exponent = Float(bitPattern: 0x3f317218).addingProduct(lookup(value, curve), Float(bitPattern: decay ? 0x4116efe0 : 0x410ec3c2))
        return max(0, Double(expf(exponent) - 2) / 1000)
    }
    private static func cutoffHertz(_ value: Double, highpass: Bool) -> Double {
        // Source's eight-term exponential approximation, not an assumed log-Hz law.
        let x = lookup(value, cutoffCurve) * Float(bitPattern: 0x40c4cccd)
        var polynomial: Float = 1 + x, power = x * x
        let coefficients: [Float] = [0.5, Float(bitPattern: 0x3e2aaaab), Float(bitPattern: 0x3d2aaaab), Float(bitPattern: 0x3c088889), Float(bitPattern: 0x3ab60b61), Float(bitPattern: 0x39500d01), Float(bitPattern: 0x37d00d01)]
        for coefficient in coefficients { polynomial = polynomial.addingProduct(power, coefficient); power *= x }
        let normalized = max(0, (polynomial - 1) / Float(bitPattern: 0x43c256a2))
        if normalized > 1 { return Double(Float(bitPattern: highpass ? 0x4691ce3e : 0x46aa8b00)) }
        let base = (normalized + Float(bitPattern: 0x3b03126f)) * 44100 * 0.5
        let scale = Float(bitPattern: highpass ? 0x3f6b851f : 0x3f7d70a4)
        return Double(highpass ? base * scale * scale : base * scale)
    }
    // Verified float32 remapping constants; interpolation preserves intermediate positions.
    private static let attackCurve: [Float] = [
        0, 0.00787402038, 0.0157479998, 0.0236220006, 0.0314961001, 0.0393701009, 0.0472441018, 0.0551180989,
        0.0629921034, 0.0708661005, 0.0787402019, 0.086614199, 0.0944882035, 0.102361999, 0.110235997, 0.118110001,
        0.125983998, 0.133857995, 0.141732007, 0.149606004, 0.157480001, 0.165353999, 0.173227996, 0.181101993,
        0.188976005, 0.196850002, 0.204723999, 0.212597996, 0.220471993, 0.228346005, 0.236220002, 0.244093999,
        0.25196901, 0.259842992, 0.267717004, 0.275590986, 0.283464998, 0.29133901, 0.299212992, 0.307087004,
        0.314960986, 0.322834998, 0.33070901, 0.338582993, 0.346457005, 0.354330987, 0.362204999, 0.370079011,
        0.377952993, 0.385827005, 0.393700987, 0.401574999, 0.409449011, 0.417322993, 0.425197005, 0.433070987,
        0.440944999, 0.448819011, 0.456692994, 0.464567006, 0.472440988, 0.480315, 0.488189012, 0.496062994,
        0.503638983, 0.510904014, 0.518150985, 0.525377989, 0.532586992, 0.539777994, 0.546949983, 0.554103017,
        0.561237991, 0.568354011, 0.575451016, 0.582530022, 0.589590013, 0.596632004, 0.603654981, 0.610659003,
        0.617645025, 0.624612987, 0.631560981, 0.638490975, 0.645403028, 0.652296007, 0.659169972, 0.666024983,
        0.672863007, 0.679681003, 0.686480999, 0.693261981, 0.700025022, 0.70676899, 0.713494003, 0.720201015,
        0.726889014, 0.733559012, 0.740209997, 0.74684298, 0.75345701, 0.760052025, 0.766628027, 0.773186982,
        0.779726028, 0.786247015, 0.792748988, 0.799233019, 0.805697978, 0.812143981, 0.818571985, 0.824980974,
        0.831372023, 0.837743998, 0.844097972, 0.850431979, 0.856748998, 0.86304599, 0.869324982, 0.875585973,
        0.88182801, 0.888050973, 0.894255996, 0.900442004, 0.906608999, 0.912757993, 0.918887973, 0.925000012,
    ]
    // Verified float32 remapping constants; interpolation preserves intermediate positions.
    private static let holdCurve: [Float] = [
        0, 0.0438422002, 0.0838845968, 0.120670997, 0.154203996, 0.184484005, 0.211520001, 0.235312998,
        0.256282002, 0.275599003, 0.293294013, 0.309365988, 0.323816001, 0.336649001, 0.347864985, 0.357930005,
        0.367193013, 0.375649005, 0.383293986, 0.390132993, 0.396164, 0.400911987, 0.403937012, 0.406982988,
        0.410046995, 0.413134009, 0.416242987, 0.419373989, 0.422526985, 0.425698012, 0.42888999, 0.432105005,
        0.435342014, 0.438600987, 0.441877991, 0.445176005, 0.448496997, 0.451839, 0.455199003, 0.458586007,
        0.461988986, 0.465413988, 0.468856007, 0.47232601, 0.475811005, 0.47931999, 0.482847989, 0.486400008,
        0.489973009, 0.493562996, 0.497173995, 0.500806987, 0.504462004, 0.508132994, 0.511829972, 0.515545011,
        0.519280016, 0.523033023, 0.526811004, 0.530605972, 0.534422994, 0.538260996, 0.54211998, 0.545994997,
        0.550346017, 0.555182993, 0.560055971, 0.564962983, 0.569899976, 0.574877024, 0.579886019, 0.584928989,
        0.590002, 0.595116973, 0.600266993, 0.605449021, 0.610665977, 0.615917981, 0.621201992, 0.626522005,
        0.631878018, 0.637270987, 0.642700016, 0.648160994, 0.653657973, 0.659191012, 0.664762974, 0.670369983,
        0.676008999, 0.681684971, 0.68739903, 0.693148971, 0.698938012, 0.704759002, 0.710615993, 0.716511011,
        0.722443998, 0.728415012, 0.734418988, 0.740459979, 0.74654001, 0.75265801, 0.758813977, 0.765003026,
        0.771230996, 0.77749598, 0.783801019, 0.790144026, 0.796521008, 0.802936018, 0.809391022, 0.815885007,
        0.822412014, 0.828984022, 0.835590005, 0.842235029, 0.848914981, 0.855639994, 0.862398982, 0.869195998,
        0.876034021, 0.882914007, 0.88983202, 0.896785021, 0.903778017, 0.91081202, 0.917885005, 0.925000012,
    ]
    // Verified float32 remapping constants; interpolation preserves intermediate positions.
    private static let decayCurve: [Float] = [
        0.0949999988, 0.110970996, 0.126539007, 0.141543001, 0.155862004, 0.169678003, 0.183089003, 0.196032003,
        0.208462, 0.220350996, 0.231671005, 0.242405996, 0.252546012, 0.262080014, 0.271005005, 0.279320002,
        0.287023008, 0.294234991, 0.301111013, 0.30764401, 0.313840985, 0.319707006, 0.325251013, 0.330478996,
        0.335397989, 0.340012014, 0.344327986, 0.348352998, 0.352086991, 0.355592996, 0.359102011, 0.362625986,
        0.366178006, 0.36975801, 0.373360991, 0.376996994, 0.380665004, 0.384373009, 0.388112009, 0.391894013,
        0.395713001, 0.399571002, 0.403472006, 0.407411993, 0.41139701, 0.415419012, 0.419485003, 0.423595995,
        0.427751005, 0.431946993, 0.436185002, 0.440468013, 0.444797009, 0.449167013, 0.453579992, 0.458041012,
        0.462543994, 0.467090011, 0.471684992, 0.476318985, 0.480998993, 0.485720992, 0.490491986, 0.495306998,
        0.500267029, 0.505380988, 0.510535002, 0.515747011, 0.521001995, 0.52631098, 0.531666994, 0.537066996,
        0.542524993, 0.548023999, 0.553580999, 0.559181988, 0.564833999, 0.570535004, 0.576290011, 0.582094014,
        0.587947011, 0.59385097, 0.599803984, 0.605809987, 0.611863971, 0.617974997, 0.624134004, 0.630342007,
        0.636604011, 0.64291501, 0.649280012, 0.655695021, 0.662162006, 0.668682992, 0.675253987, 0.681879997,
        0.688555002, 0.695284009, 0.702064991, 0.708899975, 0.715786994, 0.722728014, 0.729722977, 0.736765027,
        0.743866026, 0.751017988, 0.758225977, 0.765484989, 0.772803009, 0.78017199, 0.787591994, 0.795069993,
        0.802600026, 0.810154974, 0.817731977, 0.825414002, 0.833175004, 0.841041982, 0.84898901, 0.857053995,
        0.865212023, 0.873449981, 0.881796002, 0.890248001, 0.898781002, 0.907422006, 0.916170001, 0.925000012,
    ]
    // Verified float32 remapping constants; interpolation preserves intermediate positions.
    private static let cutoffCurve: [Float] = [
        0, 0.00749999983, 0.0149999997, 0.0225000009, 0.0299999993, 0.0375000015, 0.0450000018, 0.0524999984,
        0.0599999987, 0.0675000027, 0.075000003, 0.0825000033, 0.0900000036, 0.0974999964, 0.104999997, 0.112499997,
        0.119999997, 0.127499998, 0.135000005, 0.142499998, 0.150000006, 0.157499999, 0.165000007, 0.172499999,
        0.180000007, 0.1875, 0.194999993, 0.202500001, 0.209999993, 0.217500001, 0.224999994, 0.232500002,
        0.239999995, 0.247500002, 0.254999995, 0.262499988, 0.270000011, 0.277500004, 0.284999996, 0.292499989,
        0.300000012, 0.307500005, 0.314999998, 0.32249999, 0.330000013, 0.337500006, 0.344999999, 0.352499992,
        0.360000014, 0.367500007, 0.375, 0.382499993, 0.389999986, 0.397500008, 0.405000001, 0.412499994,
        0.419999987, 0.42750001, 0.435000002, 0.442499995, 0.449999988, 0.457500011, 0.465000004, 0.472499996,
        0.479999989, 0.487500012, 0.495000005, 0.502499998, 0.50999999, 0.517499983, 0.524999976, 0.532500029,
        0.540000021, 0.547500014, 0.555000007, 0.5625, 0.569999993, 0.577499986, 0.584999979, 0.592499971,
        0.600000024, 0.607500017, 0.61500001, 0.622500002, 0.629999995, 0.637499988, 0.644999981, 0.652499974,
        0.660000026, 0.667500019, 0.675000012, 0.682500005, 0.689999998, 0.69749999, 0.704999983, 0.712499976,
        0.720000029, 0.727500021, 0.735000014, 0.742500007, 0.75, 0.757499993, 0.764999986, 0.772499979,
        0.779999971, 0.787500024, 0.795000017, 0.80250001, 0.810000002, 0.817499995, 0.824999988, 0.832499981,
        0.839999974, 0.847500026, 0.855000019, 0.862500012, 0.870000005, 0.877499998, 0.88499999, 0.892499983,
        0.899999976, 0.911428988, 0.922856987, 0.934285998, 0.945713997, 0.957143009, 0.968571007, 0.980000019,
    ]
}

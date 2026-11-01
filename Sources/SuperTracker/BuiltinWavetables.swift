import Foundation

/// The wavetables that come with SuperTracker. They are generated (additive
/// synthesis, no recorded or third-party material) into the support folder the
/// first time they are needed, and regenerated whenever `version` changes.
///
///   Wavetables/Basic   single cycles: Sine, Triangle, Saw, Square, Pulse 25 %, Pulse 12.5 %
///                      plus Noise (a two-second loop, played in forward-loop mode)
///   Wavetables/Morph   32-frame tables for the Position control
enum BuiltinWavetables {
    struct Table {
        let group: String
        let name: String
        let frames: [[Float]]
        /// Noise is a loop, not a cycle table.
        var isWavetable: Bool { group != "Basic" || name != "Noise" }
        var fileName: String { name + ".wav" }
    }

    static let version = 1
    static let frameSize = WavetableFormat.defaultFrameSize
    static let morphFrames = 32

    /// SUPERTRACKER_SUPPORT_DIR keeps test and development runs out of the user's folder.
    static var directory: URL {
        let environment = ProcessInfo.processInfo.environment
        let support = environment["SUPERTRACKER_SUPPORT_DIR"].flatMap { $0.hasPrefix("/") ? URL(fileURLWithPath: $0, isDirectory: true) : nil }
            ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("SuperTracker", isDirectory: true)
        return support.appendingPathComponent("Wavetables", isDirectory: true)
    }

    /// The single-cycle table that replaces a retired synth waveform, as a project reference.
    static func basicPath(waveform: String) -> String {
        let names = ["sine": "Sine", "triangle": "Triangle", "saw": "Saw", "square": "Square", "noise": "Noise"]
        return BuiltinSamples.wavetable(group: "Basic", name: names[waveform] ?? "Sine")
    }

    private static let installLock = NSLock()
    private static var installedDirectory: String?

    /// Writes any missing or outdated table. Cheap after the first call per folder.
    @discardableResult static func installIfNeeded() throws -> URL {
        let root = directory
        installLock.lock(); defer { installLock.unlock() }
        if installedDirectory == root.path { return root }
        let marker = root.appendingPathComponent(".version")
        let current = (try? String(contentsOf: marker, encoding: .utf8)) == String(version)
        for table in catalogue() {
            let folder = root.appendingPathComponent(table.group, isDirectory: true)
            let file = folder.appendingPathComponent(table.fileName)
            if current && FileManager.default.fileExists(atPath: file.path) { continue }
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            let samples = table.frames.flatMap { $0 }
            try WavetableFormat.wavData(samples: samples, frameSize: table.isWavetable ? frameSize : nil).write(to: file, options: .atomic)
        }
        try String(version).write(to: marker, atomically: true, encoding: .utf8)
        installedDirectory = root.path
        return root
    }

    /// Listing for the sampler's wavetable picker.
    static func list() throws -> [[String: Any]] {
        try installIfNeeded()
        return catalogue().map { table in
            ["group": table.group, "name": table.name, "path": BuiltinSamples.wavetable(group: table.group, name: table.name),
             "frames": table.isWavetable ? table.frames.count : 0, "frameSize": table.isWavetable ? frameSize : 0,
             "wavetable": table.isWavetable]
        }
    }

    // MARK: - Generation

    static func catalogue() -> [Table] {
        [
            Table(group: "Basic", name: "Sine", frames: [additive { $0 == 1 ? 1 : 0 }]),
            Table(group: "Basic", name: "Triangle", frames: [additive { n in n % 2 == 1 ? (n % 4 == 1 ? 1 : -1) / Double(n * n) : 0 }]),
            Table(group: "Basic", name: "Saw", frames: [additive { 1 / Double($0) }]),
            Table(group: "Basic", name: "Square", frames: [additive { $0 % 2 == 1 ? 1 / Double($0) : 0 }]),
            Table(group: "Basic", name: "Pulse 25", frames: [pulse(width: 0.25)]),
            Table(group: "Basic", name: "Pulse 12.5", frames: [pulse(width: 0.125)]),
            Table(group: "Basic", name: "Noise", frames: [noise(frames: 96_000)]),
            Table(group: "Morph", name: "Sine to Saw", frames: morph { t in additive { n in n == 1 ? 1 : t / Double(n) } }),
            Table(group: "Morph", name: "Pulse Width", frames: morph { t in pulse(width: 0.5 - 0.47 * t) }),
            Table(group: "Morph", name: "Harmonic Sweep", frames: morph { t in
                let position = 1 + t * 31, lower = Int(position), blend = position - Double(lower)
                return additive { n in n == lower ? 1 - blend : n == lower + 1 ? blend : 0 }
            }),
            Table(group: "Morph", name: "Vowels", frames: morph { t in vowel(t) }),
            Table(group: "Morph", name: "Wavefold", frames: morph { t in fold(gain: 1 + 7 * t) }),
            Table(group: "Morph", name: "Sync Sweep", frames: morph { t in sync(ratio: 1 + 7 * t) }),
        ]
    }

    /// Band-limited cycle from harmonic amplitudes (sine phase), Lanczos-smoothed
    /// against ringing and normalised to a 0.9 peak.
    private static func additive(harmonics: Int = 256, _ amplitude: (Int) -> Double) -> [Float] {
        var values = [Double](repeating: 0, count: frameSize)
        for n in 1...harmonics {
            let a = amplitude(n)
            guard a != 0 else { continue }
            let x = Double.pi * Double(n) / Double(harmonics + 1)
            let weight = a * (n == 1 ? 1 : sin(x) / x)
            for i in 0..<frameSize { values[i] += weight * sineTable[(n * i) % frameSize] }
        }
        return normalized(values)
    }

    /// One sine cycle; harmonic n at sample i is sineTable[(n * i) % frameSize].
    private static let sineTable = (0..<frameSize).map { sin(2 * Double.pi * Double($0) / Double(frameSize)) }

    private static func pulse(width: Double) -> [Float] {
        // Pulse = saw(phase) - saw(phase + width), the width rounded to one table sample.
        let shift = Int((width * Double(frameSize)).rounded())
        var values = [Double](repeating: 0, count: frameSize)
        for n in 1...256 {
            let x = Double.pi * Double(n) / 257, a = (n == 1 ? 1 : sin(x) / x) / Double(n)
            for i in 0..<frameSize { values[i] += a * (sineTable[(n * i) % frameSize] - sineTable[(n * (i + shift)) % frameSize]) }
        }
        let mean = values.reduce(0, +) / Double(frameSize)
        return normalized(values.map { $0 - mean })
    }

    /// Spoken-vowel formants (A, E, I, O, U) as Gaussian bumps over harmonics of 110 Hz.
    private static func vowel(_ t: Double) -> [Float] {
        let formants: [(Double, Double)] = [(800, 1150), (400, 1600), (300, 2300), (500, 900), (350, 700)]
        let position = t * Double(formants.count - 1), lower = min(formants.count - 2, Int(position)), blend = position - Double(lower)
        let f1 = formants[lower].0 + (formants[lower + 1].0 - formants[lower].0) * blend
        let f2 = formants[lower].1 + (formants[lower + 1].1 - formants[lower].1) * blend
        return additive(harmonics: 64) { n in
            let frequency = 110 * Double(n)
            func bump(_ center: Double, _ width: Double) -> Double { exp(-pow((frequency - center) / width, 2)) }
            return (bump(f1, 120) + 0.6 * bump(f2, 160) + 0.05) / sqrt(Double(n))
        }
    }

    private static func fold(gain: Double) -> [Float] {
        normalized((0..<frameSize).map { i in sin(gain * sin(2 * Double.pi * Double(i) / Double(frameSize)) * Double.pi / 2) })
    }

    /// Hard-synced saw: a slave saw at `ratio` times the cycle, reset every cycle.
    private static func sync(ratio: Double) -> [Float] {
        let raw = (0..<frameSize).map { i -> Double in
            let phase = Double(i) / Double(frameSize) * ratio
            return 2 * (phase - floor(phase)) - 1
        }
        // A short moving average softens the reset step a little.
        let smoothed = raw.indices.map { i in (raw[(i + frameSize - 1) % frameSize] + 2 * raw[i] + raw[(i + 1) % frameSize]) / 4 }
        let mean = smoothed.reduce(0, +) / Double(frameSize)
        return normalized(smoothed.map { $0 - mean })
    }

    private static func noise(frames: Int) -> [Float] {
        var state: UInt32 = 0x1234_5678
        let values = (0..<frames).map { _ -> Double in
            state ^= state << 13; state ^= state >> 17; state ^= state << 5
            return Double(state) / Double(UInt32.max) * 2 - 1
        }
        return normalized(values, peak: 0.7)
    }

    private static func morph(_ frame: (Double) -> [Float]) -> [[Float]] {
        (0..<morphFrames).map { frame(Double($0) / Double(morphFrames - 1)) }
    }

    private static func normalized(_ values: [Double], peak: Double = 0.9) -> [Float] {
        let maximum = values.map(abs).max() ?? 0
        let gain = maximum > 0 ? peak / maximum : 0
        return values.map { Float($0 * gain) }
    }
}

/// Project-portable references to samples that come with the app rather than
/// with the project: `builtin:wavetables/<group>/<name>.wav` and
/// `builtin:drums/<model>/<eight parameters as hex>` (a rendered drum-model hit).
/// They are never collected, relinked or treated as external media; `resolve`
/// returns a local file, generating it on first use.
enum BuiltinSamples {
    static let prefix = "builtin:"

    /// Renders a drum-model hit (model 1...6, eight normalised controls, NaN = default)
    /// as mono 48 kHz PCM. Installed by the audio host (SampleReferences).
    nonisolated(unsafe) static var drumRenderer: ((_ model: Int, _ parameters: [Float]) -> [Float])?

    static func isBuiltin(_ path: String) -> Bool { path.hasPrefix(prefix) }

    static func wavetable(group: String, name: String) -> String { prefix + "wavetables/\(group)/\(name).wav" }

    static func drum(model: String, parameters: [Float]) -> String {
        prefix + "drums/\(model)/" + parameters.map { String(format: "%08x", $0.bitPattern) }.joined()
    }

    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    static func resolve(_ path: String) throws -> String {
        guard isBuiltin(path) else { return path }
        let parts = path.dropFirst(prefix.count).split(separator: "/").map(String.init)
        if parts.count == 3, parts[0] == "wavetables" {
            let root = try BuiltinWavetables.installIfNeeded()
            let file = root.appendingPathComponent(parts[1], isDirectory: true).appendingPathComponent(parts[2])
            guard FileManager.default.fileExists(atPath: file.path) else { throw Failure(message: "Unknown built-in wavetable: \(parts[1])/\(parts[2]).") }
            return file.path
        }
        if parts.count == 3, parts[0] == "drums", let model = ProjectMigration.drumModels[parts[1]], parts[2].count == 64 {
            let folder = BuiltinWavetables.directory.deletingLastPathComponent().appendingPathComponent("Generated/Drums", isDirectory: true)
            let file = folder.appendingPathComponent("\(parts[1])-\(parts[2]).wav")
            if FileManager.default.fileExists(atPath: file.path) { return file.path }
            guard let render = drumRenderer else { throw Failure(message: "Built-in drum samples need the audio engine.") }
            let hex = Array(parts[2])
            let parameters = stride(from: 0, to: 64, by: 8).map { Float(bitPattern: UInt32(String(hex[$0..<$0 + 8]), radix: 16) ?? 0) }
            let samples = render(model, parameters)
            guard !samples.isEmpty else { throw Failure(message: "The built-in \(parts[1]) drum could not be rendered.") }
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            try WavetableFormat.wavData(samples: samples).write(to: file, options: .atomic)
            return file.path
        }
        throw Failure(message: "Unknown built-in sample: \(path).")
    }
}

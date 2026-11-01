import Foundation
import AVFoundation

/// Archive/Sound-version coverage for Maschine Groups written by Maschine 1.8 up to
/// Maschine 3. Every fixture is an original, synthetic archive built to the decoded
/// member layout; no Native Instruments file, sample or code is included.
enum MaschineGroupVersionTests {
    static var checks = 0
    static func check(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
        checks += 1
        if try !condition() { throw NSError(domain: "MaschineGroupVersionTests", code: 1, userInfo: [NSLocalizedDescriptionKey: message]) }
    }
    static func reject(_ action: () throws -> Void, _ message: String) throws {
        do { try action() } catch { checks += 1; return }
        throw NSError(domain: "MaschineGroupVersionTests", code: 2, userInfo: [NSLocalizedDescriptionKey: message])
    }
    static func e(_ n: Int) -> [UInt8] { MaschineGroupArchive.encoded(n) }
    static func string(_ text: String) -> [UInt8] { e(text.utf8.count) + Array(text.utf8) }
    static func float(_ x: Float) -> [UInt8] { (0..<4).map { UInt8((x.bitPattern >> ($0 * 8)) & 255) } }
    /// A numeric parameter whose final tracked ID is `id`: three IDs (legacy) or four IDs
    /// plus the base-tracking byte (Maschine 2+), then value/default/modulation floats.
    static func numeric(_ id: Int, _ value: Float, modern: Bool, trailer: [Float] = [0, 0], flag: UInt8 = 0) -> [UInt8] {
        var bytes: [UInt8] = []
        for n in stride(from: modern ? 3 : 2, through: 0, by: -1) { bytes += e(id - n) }
        if modern { bytes += [0] }
        return bytes + float(value) + float(trailer[0]) + float(trailer[1]) + [flag] + float(0)
    }
    /// A WStringParameter (Sound or Group name).
    static func name(_ id: Int, _ text: String, modern: Bool, trailer: [Float] = [0, 0], flag: UInt8 = 0) -> [UInt8] {
        let width = modern ? 4 : 3
        let tail = numeric(id + width, 0, modern: modern, trailer: trailer, flag: flag)
        return e(id) + (modern ? [0] : []) + string(text) + tail
    }

    struct Options {
        var archiveVersion = 12, groupVersion = 9, soundVersion = 6
        /// Every Sound owns its tracked members instead of referencing the first Sound's.
        var perSoundCopies = false
        var names: [String] = (1...16).map { "Sound \($0)" }
        /// Sounds without any insert Slot (e.g. audio-input or MIDI-only pads).
        var slotless: Set<Int> = []
        var moduleType = "Iceverb"
        /// Register a Module class even though no Slot exists (must be refused).
        var orphanModuleClass = false
        var groupName = "Synthetic Group"
        var groupTrailer: [Float] = [0, 0], groupFlag: UInt8 = 0
        /// Module body written after the Slot for non-Iceverb types; receives the module ID.
        var body: ((Int, Int) -> [UInt8])? = nil
    }

    static func fixture(_ o: Options) -> Data {
        let modern = o.archiveVersion >= 13
        let layout = MaschineGroupArchive.layout(soundVersion: o.soundVersion, modern: modern)!
        var archive: [UInt8] = [1, 22]
        for part in [Array("serialization::archive".utf8), e(o.archiveVersion), e(0), e(1), e(o.groupVersion)] { archive += part }
        archive += [1, 16, 1, 1, 0, 1, 1]
        var slotDeclared = false, moduleDeclared = false
        for index in 0..<16 {
            let id = 100 + index * 1000
            let delta = index == 0 || o.perSoundCopies ? layout.firstNameDelta : layout.nameDelta
            let nameID = id + delta
            archive += e(21)
            if index == 0 { archive += e(1) + e(o.soundVersion) }
            archive += e(id) + e(2) + e(id + 1)
            archive += name(nameID, o.names[index], modern: modern)
            guard !o.slotless.contains(index) else { continue }
            let slot = nameID + (modern ? 18 : 14), inherited = modern ? 4 : 3, module = slot + inherited + 1
            archive += e(32)
            if !slotDeclared { archive += e(1) + e(0); slotDeclared = true }
            for n in 0...inherited { archive += e(slot + n) }
            if modern { archive += [0] }
            for part in [e(1), e(1), e(0), e(0), e(0)] { archive += part }
            archive += e(40)
            if !moduleDeclared { archive += string("NI::MASCHINE::DATA::" + o.moduleType) + e(1) + e(0); moduleDeclared = true }
            archive += e(module)
            if let body = o.body { archive += body(module, index) }
            else { for n in 1...4 { archive += numeric(module + n * (modern ? 4 : 3), Float(n) / 4, modern: modern) } }
        }
        archive += [UInt8](repeating: 0, count: 24)
        archive += name(17000, o.groupName, modern: modern, trailer: o.groupTrailer, flag: o.groupFlag)
        if o.orphanModuleClass { archive += e(40) + string("NI::MASCHINE::DATA::" + o.moduleType) + e(1) + e(0) }
        var container = [UInt8](repeating: 0, count: 32)
        container.replaceSubrange(12..<16, with: Array("hsin".utf8)); container.replaceSubrange(20..<28, with: Array("2SAM PRG".utf8))
        container += (0..<4).map { UInt8((archive.count >> ($0 * 8)) & 255) }
        container += archive
        for i in 0..<8 { container[i] = UInt8((container.count >> (i * 8)) & 255) }
        return Data(container)
    }

    static func writeWAV(_ url: URL, frames: Int) throws {
        let format = AVAudioFormat(standardFormatWithSampleRate: 44100, channels: 1)!
        let file = try AVAudioFile(forWriting: url, settings: format.settings)
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(frames))!
        buffer.frameLength = AVAudioFrameCount(frames)
        try file.write(from: buffer)
    }

    static func run() throws -> Int {
        // Every Sound version / archive generation seen in the installed libraries.
        let variants: [(Int, Int, Int)] = [(10,1,1),(10,2,3),(10,4,4),(10,9,5),(10,9,6),(12,9,6),(13,13,6),(14,14,6),
                                           (14,15,8),(15,20,8),(16,22,8),(17,22,9),(18,23,9),(19,23,9)]
        for (av, gv, sv) in variants {
            var o = Options(); o.archiveVersion = av; o.groupVersion = gv; o.soundVersion = sv
            let label = "archive \(av), Group \(gv), Sound \(sv)"
            let archive = try MaschineGroupArchive(data: fixture(o))
            try check(archive.sounds.map(\.name) == o.names, "\(label): Sound names keep their pad order")
            try check(archive.modules.count == 16 && archive.modules.allSatisfy { $0.type == "Iceverb" }, "\(label): one insert per Sound")
            try check(archive.sounds.allSatisfy { archive.modules(for: $0).count == 1 }, "\(label): inserts belong to their own Sound")
            try check(archive.groupName == "Synthetic Group" && archive.groupModules.isEmpty, "\(label): Group name and boundary")
            for module in archive.modules { try check(try archive.parameters(for: module) == [0.25, 0.5, 0.75, 1], "\(label): typed parameters") }
            // Each version's object-ID layout is distinct: decoding with a neighbouring
            // Sound version must be refused rather than misread.
            var data = fixture(o)
            let signature = data.range(of: Data("serialization::archive".utf8))!.upperBound
            let vector = data.range(of: Data([1, 16, 1, 1, 0, 1, 1]), in: signature..<data.count)!.upperBound
            let versionOffset = vector + e(21).count + e(1).count + 1
            try check(data[versionOffset] == UInt8(sv), "\(label): fixture Sound version position")
            for other in [1, 3, 4, 6, 8, 9] where MaschineGroupArchive.layout(soundVersion: other, modern: av >= 13) != nil
                && MaschineGroupArchive.layout(soundVersion: other, modern: av >= 13) != MaschineGroupArchive.layout(soundVersion: sv, modern: av >= 13) {
                data[versionOffset] = UInt8(other)
                try reject({ _ = try MaschineGroupArchive(data: data) }, "\(label) misread as Sound version \(other)")
            }
        }
        // Unknown archive, Group and Sound versions stay refused.
        for (av, gv, sv) in [(11, 9, 6), (20, 23, 9), (19, 24, 9), (19, 23, 10), (12, 9, 7), (17, 22, 7), (10, 1, 8)] {
            var o = Options(); o.archiveVersion = av; o.groupVersion = gv; o.soundVersion = min(sv, 9)
            if MaschineGroupArchive.layout(soundVersion: o.soundVersion, modern: av >= 13) == nil { o.soundVersion = av >= 13 ? 9 : 6 }
            var data = fixture(o)
            if sv != o.soundVersion {
                let signature = data.range(of: Data("serialization::archive".utf8))!.upperBound
                let vector = data.range(of: Data([1, 16, 1, 1, 0, 1, 1]), in: signature..<data.count)!.upperBound
                data[vector + e(21).count + e(1).count + 1] = UInt8(sv)
            }
            try reject({ _ = try MaschineGroupArchive(data: data) }, "archive \(av), Group \(gv), Sound \(sv) must be refused")
        }
        // External-MIDI templates: every Sound owns its members (all at the first-Sound distance).
        var copies = Options(); copies.archiveVersion = 14; copies.groupVersion = 15; copies.soundVersion = 8; copies.perSoundCopies = true
        try check(try MaschineGroupArchive(data: fixture(copies)).sounds.map(\.name) == copies.names, "Per-Sound member copies")
        // Empty pads have an empty name; Maschine shows them by pad number.
        var empty = Options(); empty.archiveVersion = 18; empty.groupVersion = 22; empty.soundVersion = 9
        empty.names = (1...16).map { $0 > 8 ? "" : "Pad \($0)" }
        let emptyArchive = try MaschineGroupArchive(data: fixture(empty))
        try check(emptyArchive.sounds.map(\.name) == empty.names, "Empty Sound names are decoded")
        try check(emptyArchive.sounds.map(MaschineGroupImporter.displayName) == (1...16).map { $0 > 8 ? "Sound \($0)" : "Pad \($0)" }, "Unnamed Sounds display their pad number")
        var legacyEmpty = empty; legacyEmpty.archiveVersion = 10; legacyEmpty.groupVersion = 4; legacyEmpty.soundVersion = 4
        try check(try MaschineGroupArchive(data: fixture(legacyEmpty)).sounds.map(\.name) == empty.names, "Empty legacy Sound names")
        // An input Sound without Slots first: the Slot class is declared by the next Sound.
        var input = Options(); input.archiveVersion = 14; input.groupVersion = 14; input.slotless = [0, 5]
        let inputArchive = try MaschineGroupArchive(data: fixture(input))
        try check(inputArchive.modules.count == 14 && inputArchive.modules(for: inputArchive.sounds[0]).isEmpty
                  && inputArchive.modules(for: inputArchive.sounds[5]).isEmpty && inputArchive.modules(for: inputArchive.sounds[1]).count == 1,
                  "Slotless Sounds keep their pads; later Slots are found")
        // MIDI-only Groups have no Slot and no Module class at all.
        var midi = Options(); midi.archiveVersion = 14; midi.groupVersion = 15; midi.soundVersion = 8; midi.slotless = Set(0..<16)
        let midiArchive = try MaschineGroupArchive(data: fixture(midi))
        try check(midiArchive.modules.isEmpty && midiArchive.slotClassID == nil && midiArchive.sounds.count == 16, "MIDI-only Group")
        midi.orphanModuleClass = true
        try reject({ _ = try MaschineGroupArchive(data: fixture(midi)) }, "A registered Module class without any Slot must be refused")
        // A Group name whose parameter trailer carries data is still unique.
        var trailer = Options(); trailer.archiveVersion = 19; trailer.groupVersion = 23; trailer.soundVersion = 9
        trailer.groupTrailer = [10, 9]; trailer.groupFlag = 1
        try check(try MaschineGroupArchive(data: fixture(trailer)).groupName == "Synthetic Group", "Group name with a non-zero trailer")
        trailer.groupTrailer = [.nan, 9]
        try reject({ _ = try MaschineGroupArchive(data: fixture(trailer)) }, "A non-finite Group name trailer must be refused")
        // Truncation and corruption of a newly supported variant never crash and are refused.
        var old = Options(); old.archiveVersion = 10; old.groupVersion = 1; old.soundVersion = 1
        let oldData = fixture(old)
        for length in stride(from: 0, to: oldData.count, by: max(1, oldData.count / 97)) {
            try reject({ _ = try MaschineGroupArchive(data: oldData.prefix(length)) }, "truncated legacy archive accepted at \(length)")
        }
        var flipped = 0
        for offset in stride(from: 40, to: oldData.count, by: 7) {
            var bad = oldData; bad[offset] ^= 0x5a
            if (try? MaschineGroupArchive(data: bad)) == nil { flipped += 1 }
        }
        try check(flipped > 0, "corrupted legacy archives are refused")

        let temporary = FileManager.default.temporaryDirectory.appendingPathComponent("SuperTracker-MaschineVersions-" + UUID().uuidString, isDirectory: true)
        let library = temporary.appendingPathComponent("Renamed Expansion", isDirectory: true)
        try FileManager.default.createDirectory(at: library.appendingPathComponent("Groups/Kits"), withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: library.appendingPathComponent("Samples/Loops"), withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: temporary) }
        try Data().write(to: library.appendingPathComponent("Renamed Expansion.nicnt"))
        let groupURL = library.appendingPathComponent("Groups/Kits/Synthetic.mxgrp")
        try check(MaschineGroupImporter.expansionRoot(groupURL, root: temporary).path == library.path, "An expansion without a \"Library\" suffix is found by its .nicnt")
        // Library-relative references with leading slashes stay inside the expansion.
        for path in ["//Samples/Loops/Beat.wav", "/Samples/Loops/Beat.wav", "Samples/Loops/Beat.wav"] {
            try check(try MaschineGroupImporter.resolve(path, library: library).path == library.resolvingSymlinksInPath().appendingPathComponent("Samples/Loops/Beat.wav").path, "relative form \(path)")
        }
        for path in ["/Users/Shared/Beat.wav", "//Samples/../../Beat.wav", "///Samples/Beat.wav", "/tmp/Samples/Beat.wav"] {
            try reject({ _ = try MaschineGroupImporter.resolve(path, library: library) }, "unsafe path \(path) accepted")
        }
        // Audio modules: their single sample reference becomes a whole-file zone.
        try writeWAV(library.appendingPathComponent("Samples/Loops/Beat.wav"), frames: 4410)
        var audio = Options(); audio.archiveVersion = 17; audio.groupVersion = 22; audio.soundVersion = 9; audio.moduleType = "AudioModule"
        audio.body = { module, index in
            e(module + 1) + (index == 0 ? [0, 0] : []) + e(2) + string("//Samples/Loops/Beat.wav") + string("C:/Expansions/Renamed Expansion/") + string("Renamed")
        }
        let audioArchive = try MaschineGroupArchive(data: fixture(audio))
        try check(try audioArchive.audioModuleSample(for: audioArchive.modules[3]).path == "//Samples/Loops/Beat.wav", "Audio module sample reference")
        try fixture(audio).write(to: groupURL)
        let audioImport = try MaschineGroupImporter.read(at: groupURL, root: temporary)
        let audioSounds = (((audioImport["project"] as! [String: Any])["kits"] as! [[String: Any]])[0]["sounds"] as! [[String: Any]])
        let firstZone = ((audioSounds[0]["instrument"] as! [String: Any])["zones"] as! [[String: Any]])[0]
        try check((firstZone["path"] as? String)?.hasSuffix("Renamed Expansion/Samples/Loops/Beat.wav") == true && firstZone["unavailable"] == nil
                  && (firstZone["end"] as? Double) == 1 && (firstZone["start"] as? Double) == 0, "Audio module zone covers its whole file")
        try check((audioImport["warnings"] as! [String]).contains { $0.contains("Audio module") && $0.contains("tempo sync") }, "Audio module limits are reported per Sound")
        audio.body = { module, _ in e(module + 1) + e(2) + string("Samples/Loops/Beat.exe") + string("") + string("") }
        let badAudio = try MaschineGroupArchive(data: fixture(audio))
        try reject({ _ = try badAudio.audioModuleSample(for: badAudio.modules[0]) }, "a non-audio Audio module reference accepted")
        // MIDI-only Groups import as sixteen empty pads with a Group-level note.
        midi.orphanModuleClass = false; midi.names[3] = ""
        try fixture(midi).write(to: groupURL)
        let midiImport = try MaschineGroupImporter.read(at: groupURL, root: temporary)
        let midiKit = ((midiImport["project"] as! [String: Any])["kits"] as! [[String: Any]])[0]
        try check((midiKit["sounds"] as! [[String: Any]]).map { $0["name"] as! String }[3] == "Sound 4", "Unnamed pad imports as Sound 4")
        try check((try KitProject(project: midiImport["project"] as! [String: Any])).sounds.count == 16, "MIDI-only Group is a canonical kit")
        try check((midiImport["warnings"] as! [String]).contains { $0.contains("no audio modules") }, "MIDI-only Group is explained")
        // REX2 slices play from the bundled WAV rendering only when every slice fits.
        try Data("CAT REX2".utf8).write(to: library.appendingPathComponent("Samples/Loops/Beat.rx2"))
        func slice(_ start: Double, _ end: Double) -> MaschineGroupArchive.Zone {
            MaschineGroupArchive.Zone(objectID: 1, path: "Samples/Loops/Beat.rx2", origin: "", library: "", values:
                ["keyLow": 60, "keyHigh": 60, "velocityLow": 0, "velocityHigh": 127, "root": 60, "pan": 0, "tune": 0, "gain": 0.75,
                 "loopEnabled": 0, "startFrame": start, "endFrame": end, "loopStartFrame": start, "loopEndFrame": end])
        }
        var warnings: [String] = [], cache: [String: Int64] = [:]
        let rex = MaschineGroupImporter.mapZones([slice(0, 2204), slice(2205, 4409)], library: library, frameCache: &cache) { warnings.append($0) }
        try check(rex.count == 2 && rex.allSatisfy { ($0["path"] as? String)?.hasSuffix("Beat.wav") == true && $0["unavailable"] == nil }
                  && (rex[1]["start"] as? Double) == 2205.0 / 4410, "REX2 slices use the bundled WAV")
        try check(warnings.contains { $0.contains("REX2") }, "REX2 substitution is reported")
        warnings = []; cache = [:]
        let tooLong = MaschineGroupImporter.mapZones([slice(0, 9000)], library: library, frameCache: &cache) { warnings.append($0) }
        try check(tooLong.count == 1 && tooLong[0]["unavailable"] as? Bool == true && warnings.contains { $0.contains("do not match") }, "Slices beyond the WAV rendering stay unavailable")
        cache = [:]
        let short = MaschineGroupImporter.mapZones([slice(0, 1000), slice(1001, 3000)], library: library, frameCache: &cache) { _ in }
        try check(short.allSatisfy { $0["unavailable"] as? Bool == true }, "Slices on a different timeline than the WAV stay unavailable")
        return checks
    }
}

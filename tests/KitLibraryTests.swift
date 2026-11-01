import Foundation

/// Fixture-only package tests: no user libraries, vendor executables, audio, or plug-ins.
@main
struct KitLibraryTests {
    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    static let files = FileManager.default

    static func main() throws {
        let scratch = files.temporaryDirectory.appendingPathComponent("SuperTracker-kit-library-\(UUID().uuidString)", isDirectory: true)
        try files.createDirectory(at: scratch, withIntermediateDirectories: true)
        defer { try? files.removeItem(at: scratch) }
        try collectedRoundTrip(scratch.appendingPathComponent("Collected", isDirectory: true))
        try externalReferencesAndTransactions(scratch.appendingPathComponent("External", isDirectory: true))
        try malformedPackages(scratch.appendingPathComponent("Malformed", isDirectory: true))
        try previewDiscovery(scratch.appendingPathComponent("Previews", isDirectory: true))
        try loadedSoundCount(scratch.appendingPathComponent("Count", isDirectory: true))
        try groupHeaders(scratch.appendingPathComponent("Headers", isDirectory: true))
        print("PASS kit library: opaque project state, copied root and zone media, portable relocation, external references, overwrite safety, cancellation, malformed manifests, bounded package paths, exact preview discovery, source preservation, Maschine group headers")
    }

    /// A synthetic NI SoundInfo item as found near the start of a .mxgrp (the archive itself is never read).
    static func groupHeaderBytes(name: String, chain: [String], types: [String], padding: Int = 256) -> Data {
        func u32(_ value: UInt32) -> [UInt8] { (0..<4).map { UInt8(truncatingIfNeeded: value >> ($0 * 8)) } }
        func text(_ value: String) -> [UInt8] { let units = Array(value.utf16); return u32(UInt32(units.count)) + units.flatMap { [UInt8($0 & 0xff), UInt8($0 >> 8)] } }
        var bytes = [UInt8](repeating: 0, count: 48) + Array("hsin".utf8) + [UInt8](repeating: 0, count: 12)
        bytes += Array("DSIN".utf8) + [0x6c, 0, 0, 0, 1, 0, 0, 0, 0x18, 0, 0, 0] + [UInt8](repeating: 0, count: 20)
        bytes += u32(1) + u32(2) + u32(1) + u32(0) + text(name) + text("Naitve Instruments") + text("Native Instruments") + text("")
        bytes += u32(0) + u32(.max) + u32(.max) + [UInt8](repeating: 0, count: 20) + u32(1)
        bytes += u32(UInt32(chain.count)) + chain.flatMap(text) + u32(UInt32(types.count)) + types.flatMap(text) + u32(0) + u32(0)
        return Data(bytes + [UInt8](repeating: 0, count: padding))
    }

    /// Maschine's browser fields come from the group header: bank chain, type paths (Maschine 3 names) and vendor.
    static func groupHeaders(_ directory: URL) throws {
        try files.createDirectory(at: directory, withIntermediateDirectories: true)
        let header = KitLibrary.groupHeader(groupHeaderBytes(name: "Acid Kit", chain: ["Maschine", "Maschine 1.8 Library"], types: ["\\:Kits", "\\:Kits\\:Urban Kit", "\\:Loops\\:Guitar\\:Wah"]))
        try check(header?.name == "Acid Kit" && header?.vendor == "Native Instruments" && header?.chain == ["Maschine", "Maschine 1.8 Library"], "Header name, vendor and bank chain")
        try check(header?.types == [["Kits", "Hip Hop"], ["Loops", "Guitar", "Wah"]], "Type paths keep the most specific entries and use Maschine 3's Hip Hop for Urban Kit")
        let full = groupHeaderBytes(name: "Truncated", chain: ["Orbit"], types: ["\\:Kits"], padding: 0)
        try check(KitLibrary.groupHeader(full.prefix(full.count - 12)) == nil, "A truncated header is rejected, not partially read")
        var hostile = [UInt8](groupHeaderBytes(name: "Huge", chain: [], types: []))
        let marker = Array("DSIN".utf8) + [0x6c, 0, 0, 0]
        let at = (0..<hostile.count - 8).first { Array(hostile[$0..<$0 + 8]) == marker }! + 36 + 16
        hostile.replaceSubrange(at..<at + 4, with: [0xff, 0xff, 0xff, 0x7f])
        try check(KitLibrary.groupHeader(Data(hostile)) == nil, "Oversized string lengths are rejected")
        try check(KitLibrary.groupHeader(Data("opaque".utf8)) == nil, "Files without a SoundInfo item have no header")
        // A library root inside an expansion must not name the expansion after its Groups/Kits folder.
        let expansion = directory.appendingPathComponent("Anima Ascent Library/Groups/Kits", isDirectory: true)
        try files.createDirectory(at: expansion, withIntermediateDirectories: true)
        let opaque = expansion.appendingPathComponent("Box Kit.mxgrp"), described = expansion.appendingPathComponent("Acid Kit.mxgrp")
        try put(Data("opaque NI group".utf8), at: opaque)
        try put(groupHeaderBytes(name: "Acid Kit", chain: ["Anima Ascent"], types: ["\\:Kits\\:Urban Kit"]), at: described)
        let root = directory.appendingPathComponent("Anima Ascent Library/Groups", isDirectory: true)
        try check(try KitLibrary.metadata(at: opaque, root: root)["expansion"] as? String == "Anima Ascent Library", "Fallback expansion is the Library folder, not Kits")
        let meta = try KitLibrary.metadata(at: described, root: root)
        try check(meta["expansion"] as? String == "Anima Ascent" && meta["product"] as? String == "Anima Ascent" && meta["subtypes"] as? [String] == ["Kits\u{1f}Hip Hop"], "Header metadata names product and sub-type")
    }

    /// The header's soundCount is what the kit browser shows; empty Sampler pads are not Sounds.
    static func loadedSoundCount(_ directory: URL) throws {
        try files.createDirectory(at: directory, withIntermediateDirectories: true)
        let sample = directory.appendingPathComponent("Kick.wav")
        try put(Data((0..<64).map(UInt8.init)), at: sample)
        var project = fixture(sample: sample.path, zone: sample.path)
        var kit = (project["kits"] as! [[String: Any]])[0], sounds = kit["sounds"] as! [[String: Any]]
        for index in 3..<16 { sounds[index]["instrument"] = ["type": "sample", "samplePath": ""] }
        sounds[2]["instrument"] = ["type": "sample", "zones": [["samplePath": sample.path, "rootNote": 40]]]
        kit["sounds"] = sounds; project["kits"] = [kit]
        let package = directory.appendingPathComponent("Three.stkit", isDirectory: true)
        _ = try KitLibrary.save(project: project, metadata: ["name": "Three"], to: package, copySamples: false)
        try check(try KitLibrary.metadata(at: package)["soundCount"] as? Int == 3, "soundCount counts the sample, zone and synth Sounds, not the 13 empty pads")
        try check(KitLibrary.loadedSoundCount(["kits": [["sounds": (0..<16).map { _ in ["instrument": ["type": "sample"]] }]]]) == 0, "A new empty kit has no loaded Sounds")
    }

    static func check(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
        guard try condition() else { throw Failure(message: message) }
    }

    static func rejects(_ message: String, _ body: () throws -> Void) throws {
        do { try body() }
        catch { return }
        throw Failure(message: message)
    }

    static func encode(_ value: Any) throws -> Data {
        try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys, .fragmentsAllowed])
    }

    static func put(_ data: Data, at url: URL) throws {
        try files.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: url)
    }

    static func object(at url: URL) throws -> [String: Any] {
        guard let value = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any] else {
            throw Failure(message: "Expected JSON object at \(url.lastPathComponent)")
        }
        return value
    }

    static func dictionary(_ value: Any?, _ label: String) throws -> [String: Any] {
        guard let value = value as? [String: Any] else { throw Failure(message: "Missing \(label)") }
        return value
    }

    static func list(_ value: Any?, _ label: String) throws -> [[String: Any]] {
        guard let value = value as? [[String: Any]] else { throw Failure(message: "Missing \(label)") }
        return value
    }

    static func samplePaths(_ project: [String: Any]) throws -> [String] {
        let kits = try list(project["kits"], "kits")
        let sounds = try list(kits.first?["sounds"], "sounds")
        let instrument = try dictionary(sounds.first?["instrument"], "Sound instrument")
        let zones = try list(instrument["zones"], "sample zones")
        guard let root = instrument["samplePath"] as? String,
              let first = zones.first?["samplePath"] as? String,
              let second = zones.last?["path"] as? String else {
            throw Failure(message: "Sample path fields must survive kit serialization")
        }
        return [root, first, second]
    }

    static func treeBytes(_ directory: URL) throws -> [String: Data] {
        var result: [String: Data] = [:]
        guard let items = files.enumerator(at: directory, includingPropertiesForKeys: [.isRegularFileKey], options: []) else {
            throw Failure(message: "Cannot inspect fixture directory")
        }
        for case let url as URL in items {
            if try url.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile == true {
                result[String(url.path.dropFirst(directory.path.count + 1))] = try Data(contentsOf: url)
            }
        }
        return result
    }

    static func fixture(sample: String, zone: String) -> [String: Any] {
        var sounds: [[String: Any]] = (0..<16).map { index in
            ["id": "sound-\(index)", "name": "Sound \(index + 1)",
             "instrument": ["type": "synth", "wave": "sine", "gain": 0.0],
             "channel": ["volume": 0.8, "pan": 0.0, "delaySend": 0.0, "reverbSend": 0.0]]
        }
        sounds[0] = ["id": "sound-0", "name": "Layered kick",
                     "instrument": ["type": "sample", "samplePath": sample, "rootNote": 36,
                                    "start": 0.03, "end": 0.91, "loopMode": "pingPong", "cutoff": 1532.0,
                                    "envelope": ["attack": 0.003, "decay": 0.4, "sustain": 0.27, "release": 0.7],
                                    "zones": [["samplePath": zone, "rootNote": 42, "keyLow": 36, "keyHigh": 60, "gain": 0.71],
                                              ["path": sample, "rootNote": 48, "velocityLow": 80, "velocityHigh": 127]]],
                     "channel": ["volume": 0.63, "pan": -0.23, "delaySend": 0.31, "reverbSend": 0.49,
                                 "muted": false, "effects": [["type": "filter", "cutoff": 3500.0]]]]
        return ["version": 1, "name": "Opaque kit fixture", "bpm": 123.5, "linesPerBeat": 4,
                "kits": [["id": "kit-fixture", "name": "Fixture kit", "busID": "kit-bus", "sounds": sounds]],
                "tracks": [["id": 0, "kind": "kit", "kitID": "kit-fixture", "soundID": "sound-0"]],
                "buses": [["id": "kit-bus", "name": "Fixture group", "volume": 0.83, "pan": -0.1]],
                "audioPlugins": [["id": "plugin-fixture", "owner": "sound:kit-fixture:sound-0",
                                  "format": "VST3", "state": "AAECAwQFAP/+", "parameters": ["0": 0.125, "70001": 0.87],
                                  "opaqueVendorState": ["samplePath": "/vendor/opaque/do-not-copy.wav", "path": "/vendor/data", "blob": [0, 255, 17]],
                                  "controllerPages": [["name": "Tone", "parameters": [["id": 0, "name": "Cutoff"]]]]]],
                "patterns": [["id": 0, "length": 4, "steps": [[["note": 36, "soundID": "sound-0", "velocity": 99], NSNull(), NSNull(), NSNull()]]]],
                "sequence": [0], "performance": ["chordSet": "major-sevenths", "root": 3],
                "automation": [["target": "sound:kit-fixture:sound-0:cutoff", "points": [["step": 0, "value": 0.21], ["step": 3, "value": 0.83]]]]]
    }

    static func collectedRoundTrip(_ directory: URL) throws {
        let source = directory.appendingPathComponent("Source", isDirectory: true)
        let song = source.appendingPathComponent("Song.supertracker")
        let kick = source.appendingPathComponent("Audio/Kick.wav")
        // Same basename but different bytes makes filename collisions observable.
        let zone = source.appendingPathComponent("Layers/Kick.wav")
        let kickBytes = Data((0..<97).map(UInt8.init)), zoneBytes = Data(repeating: 173, count: 103)
        try put(kickBytes, at: kick)
        try put(zoneBytes, at: zone)
        let project = fixture(sample: "Audio/Kick.wav", zone: "Layers/Kick.wav")
        try put(try encode(project), at: song)
        let originalSource = try treeBytes(source)
        let originalProject = try encode(project)
        let package = directory.appendingPathComponent("Destination/Portable.stkit", isDirectory: true)
        try files.createDirectory(at: package.deletingLastPathComponent(), withIntermediateDirectories: true)
        let metadata: [String: Any] = ["name": "Portable fixture", "author": "Fixture Author", "tags": ["Warm", "Drums"], "description": "Fixture description"]
        _ = try KitLibrary.save(project: project, metadata: metadata, to: package, copySamples: true, relativeTo: song)
        try check(try treeBytes(source) == originalSource, "Collecting a kit must not write the source project or source samples")
        try check(try encode(project) == originalProject, "Saving must not mutate the caller's project")

        let manifest = try object(at: package.appendingPathComponent("kit.json"))
        try check(manifest["format"] as? String == "supertracker-kit" && manifest["version"] as? Int == 1,
                  "Kit package must have a versioned format manifest")
        try check(manifest["projectFile"] as? String == "project.json", "Kit project must have an explicit package member")
        let storedProject = try object(at: package.appendingPathComponent("project.json"))
        let storedPaths = try samplePaths(storedProject)
        try check(storedPaths.allSatisfy { !$0.hasPrefix("/") && $0.hasPrefix("Samples/") && !$0.split(separator: "/").contains("..") },
                  "Copied samples must use portable package-relative paths")
        try check(storedPaths[0] == storedPaths[2], "Repeated references to one sample must reuse its copied member")
        try check(storedPaths[0] != storedPaths[1], "Different files with equal basenames must not collide")
        try check(try Data(contentsOf: package.appendingPathComponent(storedPaths[0])) == kickBytes, "Root sample copy must preserve exact bytes")
        try check(try Data(contentsOf: package.appendingPathComponent(storedPaths[1])) == zoneBytes, "Nested zone copy must preserve exact bytes")
        try check(try encode(storedProject["audioPlugins"]!) == encode(project["audioPlugins"]!), "Opaque plug-in state and parameter mapping must survive collection exactly")
        var expectedStored = project
        var expectedKits = try list(expectedStored["kits"], "expected kits")
        var expectedSounds = try list(expectedKits[0]["sounds"], "expected sounds")
        var expectedInstrument = try dictionary(expectedSounds[0]["instrument"], "expected instrument")
        var expectedZones = try list(expectedInstrument["zones"], "expected zones")
        expectedInstrument["samplePath"] = storedPaths[0]
        expectedZones[0]["samplePath"] = storedPaths[1]
        expectedZones[1]["path"] = storedPaths[2]
        expectedInstrument["zones"] = expectedZones
        expectedSounds[0]["instrument"] = expectedInstrument
        expectedKits[0]["sounds"] = expectedSounds
        expectedStored["kits"] = expectedKits
        try check(try encode(storedProject) == encode(expectedStored), "Collection must preserve every project field except explicit sample paths")

        // A move proves read resolves members from the opened package, not the save location.
        let moved = directory.appendingPathComponent("Relocated/Moved.stkit", isDirectory: true)
        try files.createDirectory(at: moved.deletingLastPathComponent(), withIntermediateDirectories: true)
        try files.moveItem(at: package, to: moved)
        let loaded = try KitLibrary.read(at: moved)
        let restored = try dictionary(loaded["project"], "loaded project")
        let loadedPaths = try samplePaths(restored)
        for (index, path) in loadedPaths.enumerated() {
            let resolved = path.hasPrefix("/") ? URL(fileURLWithPath: path) : moved.appendingPathComponent(path)
            try check(resolved.standardizedFileURL.path.hasPrefix(moved.path + "/Samples/"), "Loaded collected media must remain inside the moved kit")
            try check(try Data(contentsOf: resolved) == (index == 1 ? zoneBytes : kickBytes), "Relocated kit must resolve copied media")
        }
        try check(try encode(restored["audioPlugins"]!) == encode(project["audioPlugins"]!), "Reading must leave opaque plug-in paths untouched")
        let movedSnapshot = try treeBytes(moved)
        try rejects("Reference-only overwrite cannot discard its own referenced sample members") {
            _ = try KitLibrary.save(project: restored, metadata: metadata, to: moved, copySamples: false, overwrite: true)
        }
        try check(try treeBytes(moved) == movedSnapshot, "Unsafe self-reference replacement preserves the portable package")
        _ = try KitLibrary.save(project: restored, metadata: metadata, to: moved, copySamples: true, overwrite: true)
        try check((try KitLibrary.read(at: moved)["warnings"] as? [String])?.isEmpty == true, "Collected overwrite safely retains its own sample members")
        let summary = try KitLibrary.metadata(at: moved)
        try check(summary["name"] as? String == "Portable fixture", "Browser metadata must reflect the saved kit name")
        try check(loaded["warnings"] is [String], "Kit read must expose explicit warnings")
        try check(try treeBytes(source) == originalSource, "Reading and relocating a kit must not write original media")
    }

    static func externalReferencesAndTransactions(_ directory: URL) throws {
        let source = directory.appendingPathComponent("Source", isDirectory: true)
        let kick = source.appendingPathComponent("Kick.wav"), zone = source.appendingPathComponent("Zone.wav")
        try put(Data([1, 2, 3, 4]), at: kick)
        try put(Data([9, 8, 7, 6]), at: zone)
        let originalSource = try treeBytes(source)
        let project = fixture(sample: kick.path, zone: zone.path)
        let package = directory.appendingPathComponent("Destination/External.stkit", isDirectory: true)
        try files.createDirectory(at: package.deletingLastPathComponent(), withIntermediateDirectories: true)
        _ = try KitLibrary.save(project: project, metadata: ["name": "External fixture"], to: package, copySamples: false)
        let firstSnapshot = try treeBytes(package)
        let loaded = try KitLibrary.read(at: package)
        try check(try encode(try dictionary(loaded["project"], "external project")) == encode(project), "External-reference save/read must preserve all project settings exactly")
        try check(firstSnapshot.keys.allSatisfy { !$0.hasPrefix("Samples/") }, "Reference-only save must not collect samples")

        var edited = project
        edited["name"] = "Explicit replacement"
        try rejects("Save must refuse an existing destination unless overwrite is explicit") {
            _ = try KitLibrary.save(project: edited, metadata: ["name": "Replacement"], to: package, copySamples: false)
        }
        try check(try treeBytes(package) == firstSnapshot, "Refused overwrite must preserve every existing package byte")
        try rejects("Cancellation must reject an overwrite") {
            _ = try KitLibrary.save(project: edited, metadata: ["name": "Cancelled"], to: package, copySamples: true,
                                    overwrite: true, cancelled: { true })
        }
        try check(try treeBytes(package) == firstSnapshot, "Cancelled overwrite must preserve every existing package byte")
        var checks = 0
        try rejects("Cancellation during collection must reject an overwrite") {
            _ = try KitLibrary.save(project: edited, metadata: ["name": "Cancelled during copy"], to: package, copySamples: true,
                                    overwrite: true, cancelled: { checks += 1; return checks >= 3 })
        }
        try check(checks >= 3, "Collection cancellation fixture must reach the media-copy stage")
        try check(try treeBytes(package) == firstSnapshot, "Cancelled collection must preserve every existing package byte")
        try check(try files.contentsOfDirectory(atPath: package.deletingLastPathComponent().path) == [package.lastPathComponent],
                  "Cancelled collection must clean up its temporary staging package")
        let cancelledNew = directory.appendingPathComponent("Destination/Cancelled.stkit", isDirectory: true)
        try rejects("Cancellation must reject a new package") {
            _ = try KitLibrary.save(project: edited, metadata: ["name": "Cancelled"], to: cancelledNew, cancelled: { true })
        }
        try check(!files.fileExists(atPath: cancelledNew.path), "Cancelled save must not leave an apparently usable partial package")
        var missing = project
        var missingKits = try list(missing["kits"], "missing-sample kits")
        var missingSounds = try list(missingKits[0]["sounds"], "missing-sample sounds")
        var missingInstrument = try dictionary(missingSounds[0]["instrument"], "missing-sample instrument")
        missingInstrument["samplePath"] = source.appendingPathComponent("Absent.wav").path
        missingSounds[0]["instrument"] = missingInstrument
        missingKits[0]["sounds"] = missingSounds
        missing["kits"] = missingKits
        try rejects("A collection error must fail an overwrite") {
            _ = try KitLibrary.save(project: missing, metadata: ["name": "Missing sample"], to: package, overwrite: true)
        }
        try check(try treeBytes(package) == firstSnapshot, "A failed sample copy must preserve every existing package byte")

        _ = try KitLibrary.save(project: edited, metadata: ["name": "Replacement"], to: package, copySamples: false, overwrite: true)
        let replaced = try KitLibrary.read(at: package)
        try check(try encode(try dictionary(replaced["project"], "replaced project")) == encode(edited), "Explicit overwrite must commit the replacement project")
        try check(try KitLibrary.metadata(at: package)["name"] as? String == "Replacement", "Explicit overwrite must update metadata")
        try check(try treeBytes(source) == originalSource, "References, refusal, cancellation and overwrite must never modify source samples")
    }

    static func malformedPackages(_ directory: URL) throws {
        let package = directory.appendingPathComponent("Invalid.stkit", isDirectory: true)
        let manifest = package.appendingPathComponent("kit.json")
        let emptyProject: [String: Any] = ["kits": []]
        try put(try encode(emptyProject), at: package.appendingPathComponent("project.json"))
        func rejectManifest(_ value: Any, _ label: String) throws {
            try put(try encode(value), at: manifest)
            try rejects(label) { _ = try KitLibrary.read(at: package) }
        }
        let base: [String: Any] = ["format": "supertracker-kit", "version": 1, "projectFile": "project.json", "name": "Fixture"]
        try put(Data("{unclosed".utf8), at: manifest)
        try rejects("Malformed manifest JSON must fail") { _ = try KitLibrary.read(at: package) }
        try rejectManifest([], "Manifest array must not be accepted as a package")
        var bad = base
        bad["format"] = "some-other-format"
        try rejectManifest(bad, "Foreign manifest format must fail")
        for version: Any in [0, 2, -1, 1.5, true, "1"] {
            bad = base; bad["version"] = version
            try rejectManifest(bad, "Unsupported or malformed manifest version must fail: \(version)")
        }
        let outside = directory.appendingPathComponent("outside.json")
        try put(try encode(emptyProject), at: outside)
        for path in ["../outside.json", outside.path, "nested/../../outside.json"] {
            bad = base; bad["projectFile"] = path
            try rejectManifest(bad, "Manifest must not read a project outside its package")
        }
        // A lexically safe name is still unsafe when its file is a symlink out of the package.
        try files.createSymbolicLink(at: package.appendingPathComponent("link.json"), withDestinationURL: outside)
        bad = base; bad["projectFile"] = "link.json"
        try rejectManifest(bad, "A symlink must not bypass package membership checks")
        try put(try encode(base), at: manifest)
        try put(try encode([]), at: package.appendingPathComponent("project.json"))
        try rejects("Project member must be a JSON object") { _ = try KitLibrary.read(at: package) }
        try put(Data("{unclosed".utf8), at: package.appendingPathComponent("project.json"))
        try rejects("Malformed project JSON must fail") { _ = try KitLibrary.read(at: package) }
        try files.removeItem(at: package.appendingPathComponent("project.json"))
        try files.createSymbolicLink(at: package.appendingPathComponent("project.json"), withDestinationURL: outside)
        try rejects("A project member symlink must not escape its package") { _ = try KitLibrary.read(at: package) }
        try files.removeItem(at: package.appendingPathComponent("project.json"))
        try put(try encode(emptyProject), at: package.appendingPathComponent("project.json"))
        bad = base; bad["previewFile"] = "../outside.wav"
        try put(Data([1, 2, 3]), at: directory.appendingPathComponent("outside.wav"))
        try rejectManifest(bad, "A manifest preview path must remain inside its package")
        bad = base; bad["description"] = String(repeating: "x", count: 256 * 1024)
        try rejectManifest(bad, "Oversized kit metadata must fail before indexing")
        try put(try encode(base), at: manifest)
        let oversized = package.appendingPathComponent("project.json")
        let handle = try FileHandle(forWritingTo: oversized)
        try handle.truncate(atOffset: 64 * 1024 * 1024 + 1)
        try handle.close()
        try rejects("Oversized project state must fail before allocating the payload") { _ = try KitLibrary.read(at: package) }
        try check(try object(at: outside)["kits"] is [Any], "Rejected manifests must not modify outside files")
    }

    static func previewDiscovery(_ directory: URL) throws {
        let root = directory.appendingPathComponent("Library", isDirectory: true)
        let kit = root.appendingPathComponent("Expansion Name/Groups/Drums/Midnight.mxgrp")
        try put(Data("Opaque fixture only; never execute vendor code".utf8), at: kit)
        let original = try Data(contentsOf: kit)
        let parent = kit.deletingLastPathComponent()
        try put(Data([9]), at: parent.appendingPathComponent("Midnight Other.mxgrp.ogg"))
        try put(Data([9]), at: parent.appendingPathComponent(".previews/Midnight Other.ogg"))
        var details = try KitLibrary.metadata(at: kit, root: root)
        try check(details["previewPath"] == nil, "Preview lookup must not select a similarly named kit")
        try check(details["name"] as? String == "Midnight" && details["kind"] as? String == "kit", "Opaque Maschine group must be indexable without loading it")
        try check(details["expansion"] as? String == "Expansion Name", "Group metadata must preserve the indexed expansion folder")
        for relative in ["Midnight.mxgrp.ogg", "Midnight.ogg", ".previews/Midnight.mxgrp.ogg", ".previews/Midnight.ogg", "Previews/Midnight.mxgrp.ogg"] {
            let preview = parent.appendingPathComponent(relative)
            try put(Data([1, 2, 3]), at: preview)
            details = try KitLibrary.metadata(at: kit, root: root)
            try check(details["previewPath"] as? String == preview.path, "Exact preview candidate must be found: \(relative)")
            try files.removeItem(at: preview)
            details = try KitLibrary.metadata(at: kit, root: root)
            try check(details["previewPath"] == nil, "A deleted preview must not remain available")
        }
        let empty = parent.appendingPathComponent("Midnight.mxgrp.ogg")
        try put(Data(), at: empty)
        try check(try KitLibrary.metadata(at: kit)["previewPath"] == nil, "An empty preview must not be offered")
        try check(try Data(contentsOf: kit) == original, "Indexing and preview lookup must never modify a Maschine group")
        try rejects("Opaque Maschine groups require the dedicated importer") { _ = try KitLibrary.read(at: kit) }
    }
}

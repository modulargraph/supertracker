import Foundation
import CryptoKit

/// Exercises actual kit packages and temporary media only; never opens audio or vendor code.
@main
struct KitMediaPackageRecoveryTests {
    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }
    static let files = FileManager.default

    static func main() throws {
        let root = files.temporaryDirectory.appendingPathComponent("SuperTracker-kit-package-recovery-\(UUID().uuidString)", isDirectory: true)
        try files.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? files.removeItem(at: root) }
        try packageRecovery(root)
        print("PASS kit package recovery: actual save/read, 24-character collected SHA prefix, unprefixed original discovery, explicit verified apply, wrong-content rejection, package-only prefix trust, source preservation")
    }

    static func check(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
        guard try condition() else { throw Failure(message: message) }
    }
    static func rejects(_ message: String, _ body: () throws -> Void) throws {
        do { try body() } catch { return }
        throw Failure(message: message)
    }
    static func encoded(_ value: Any) throws -> Data { try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys, .fragmentsAllowed]) }
    static func object(_ value: Any?, _ label: String) throws -> [String: Any] {
        guard let value = value as? [String: Any] else { throw Failure(message: "Missing \(label)") }
        return value
    }
    static func rows(_ value: Any?, _ label: String) throws -> [[String: Any]] {
        guard let value = value as? [[String: Any]] else { throw Failure(message: "Missing \(label)") }
        return value
    }
    static func put(_ data: Data, at url: URL) throws {
        try files.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: url)
    }
    static func fixture(_ path: String) -> [String: Any] {
        var sounds: [[String: Any]] = (0..<16).map {
            ["id": "sound-\($0)", "name": "Sound \($0 + 1)", "instrument": ["type": "synth", "gain": 0.0],
             "channel": ["volume": 0.73, "pan": 0.0, "delaySend": 0.0, "reverbSend": 0.0]]
        }
        sounds[0]["instrument"] = ["type": "sample", "name": "Kick", "samplePath": path, "rootNote": 36,
                                    "zones": [["path": path, "samplePath": path, "rootNote": 42, "gain": 0.61]]]
        var outputs: [String: Any] = ["track:kit-lane": "master", "bus:kit-group": "master"]
        for index in 0..<16 { outputs["sound:kit-recovery:sound-\(index)"] = "bus:kit-group" }
        return ["version": 1, "name": "Kit recovery fixture", "bpm": 120.0, "linesPerBeat": 4, "masterVolume": 0.0,
                "kits": [["id": "kit-recovery", "name": "Recovery kit", "busID": "kit-group", "sounds": sounds]],
                "tracks": [["id": 0, "routingID": "kit-lane", "name": "Kit", "kind": "kit", "kitID": "kit-recovery", "soundID": "sound-0", "volume": 0.8, "pan": 0.0]],
                "instruments": [[String: Any]](),
                "patterns": [["id": 0, "length": 4, "steps": [[NSNull(), NSNull(), NSNull(), NSNull()]]]], "sequence": [0],
                "routing": ["version": 1, "mixModel": "standard", "buses": [["id": "kit-group", "name": "Kit group", "type": "group", "volume": 0.8, "pan": 0.0]],
                            "outputs": outputs, "sends": [[String: Any]](), "sidechains": [[String: Any]]()],
                "audioPlugins": [["instanceID": "opaque-fixture", "owner": "sound:kit-recovery:sound-0", "kind": "effect", "slot": 0,
                                  "state": "AAECAwQF", "opaqueVendorState": ["path": "/vendor/preserved.wav"]]]]
    }
    static func kitPaths(_ project: [String: Any]) throws -> [String] {
        let kits = try rows(project["kits"], "kits"), sounds = try rows(kits[0]["sounds"], "sounds")
        let instrument = try object(sounds[0]["instrument"], "sample instrument"), zones = try rows(instrument["zones"], "zones")
        guard let root = instrument["samplePath"] as? String, let path = zones[0]["path"] as? String,
              let alias = zones[0]["samplePath"] as? String else { throw Failure(message: "Kit media fields must survive") }
        return [root, path, alias]
    }

    static func packageRecovery(_ root: URL) throws {
        let source = root.appendingPathComponent("Original/Kick.wav"), wrong = root.appendingPathComponent("Wrong/Kick.wav")
        let bytes = Data((0..<97).map(UInt8.init)), wrongBytes = Data(repeating: 255, count: 97)
        try put(bytes, at: source); try put(wrongBytes, at: wrong)
        let project = fixture(source.path), originalProject = try encoded(project)
        _ = try KitProject(project: project)
        let package = root.appendingPathComponent("Kits/Collected.stkit", isDirectory: true)
        try files.createDirectory(at: package.deletingLastPathComponent(), withIntermediateDirectories: true)
        _ = try KitLibrary.save(project: project, metadata: ["name": "Collected kit"], to: package)
        let loaded = try object(KitLibrary.read(at: package)["project"], "loaded kit")
        let paths = try kitPaths(loaded)
        let prefix = String(SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined().prefix(24))
        try check(Set(paths).count == 1 && paths[0] == package.appendingPathComponent("Samples/\(prefix)-Kick.wav").path,
                  "Actual KitLibrary collection must supply the expected 24-character SHA-prefixed package filename")
        try check(loaded["mediaManifest"] == nil, "Recovery fixture must rely on the kit filename prefix rather than a full media manifest")
        let moved = root.appendingPathComponent("Moved.stkit", isDirectory: true)
        try files.moveItem(at: package, to: moved)
        try check(!files.fileExists(atPath: paths[0]), "Moved package must leave the imported absolute reference missing")
        let before = try encoded(loaded)
        let report = try MediaRelinker.findCandidates(loaded, searchRoots: [source.deletingLastPathComponent(), wrong.deletingLastPathComponent()])
        let missing = try rows(report["missing"], "missing collected media")
        try check(missing.count == 3, "Root and both zone aliases must be visible for recovery")
        var selections: [[String: Any]] = []
        for row in missing {
            try check(row["expectedHashPrefix"] as? String == prefix && row["expectedSHA256"] == nil,
                      "Canonical package media must expose its 24-character prefix without inventing a full hash")
            let candidates = try rows(row["candidates"], "package recovery candidates")
            guard let good = candidates.first(where: { $0["path"] as? String == source.path }),
                  let bad = candidates.first(where: { $0["path"] as? String == wrong.path }) else {
                throw Failure(message: "Unprefixed original and same-name wrong file must both be inspected")
            }
            try check(good["verified"] as? Bool == true && good["match"] as? String == "hash-prefix", "Original unprefixed sample must verify against the collected kit hash prefix")
            try check(bad["verified"] as? Bool == false && bad["contentMismatch"] as? Bool == true, "Wrong same-name bytes must be identified as a mismatch")
            try check(row["requiresSelection"] as? Bool == true, "A verified kit candidate still requires an explicit selection")
            selections.append(["id": row["id"]!, "from": row["path"]!, "to": source.path])
        }
        try check(try encoded(loaded) == before, "Discovery must not relink the project automatically")
        let unselected = try object(MediaRelinker.apply(loaded, selections: [])["project"], "unselected project")
        try check(try encoded(unselected) == before, "An empty selection must not apply the top verified candidate")
        var mismatch = selections[0]; mismatch["to"] = wrong.path
        try rejects("Kit-prefix mismatch must reject replacement without explicit different-content authorization") {
            _ = try MediaRelinker.apply(loaded, selections: [mismatch])
        }
        let applied = try MediaRelinker.apply(loaded, selections: selections)
        let repaired = try object(applied["project"], "repaired kit")
        try check(try kitPaths(repaired) == [source.path, source.path, source.path] && (applied["remaining"] as? [Any])?.isEmpty == true,
                  "Explicit verified apply must restore kit sample and both aliases")
        try check(try encoded(repaired["audioPlugins"]!) == encoded(project["audioPlugins"]!) && encoded(loaded) == before,
                  "Relink must preserve opaque state and its caller's project")

        for (index, directory) in ["Unrelated", "NotAKit/Samples", "Other.stkit/Elsewhere", "Nested.stkit/Inner/Samples"].enumerated() {
            let external = root.appendingPathComponent(directory).appendingPathComponent("\(prefix)-Kick.wav")
            try put(bytes, at: external)
            let referencePackage = root.appendingPathComponent("Kits/Reference-\(index).stkit", isDirectory: true)
            _ = try KitLibrary.save(project: fixture(external.path), metadata: ["name": "External reference"], to: referencePackage, copySamples: false)
            let imported = try object(KitLibrary.read(at: referencePackage)["project"], "external imported kit")
            try files.removeItem(at: external)
            let externalReport = try MediaRelinker.findCandidates(imported, searchRoots: [source.deletingLastPathComponent(), wrong.deletingLastPathComponent()])
            let externalMissing = try rows(externalReport["missing"], "external missing kit media")
            try check(externalMissing.count == 3, "External reference fixture must expose all missing usages")
            for row in externalMissing {
                let candidates = try rows(row["candidates"], "untrusted prefix candidates")
                try check(row["expectedHashPrefix"] == nil, "24-character prefix trust must require the direct .stkit/Samples layout: \(directory)")
                try check(candidates.contains { $0["path"] as? String == source.path }, "Prefix stripping may find an unprefixed name outside a kit")
                try check(candidates.allSatisfy { $0["verified"] as? Bool == false }, "A filename outside canonical kit media must not certify candidate content")
                try check(row["requiresSelection"] as? Bool == true, "Unverified external media must remain an explicit choice")
            }
        }
        try check(try encoded(project) == originalProject && Data(contentsOf: source) == bytes && Data(contentsOf: wrong) == wrongBytes,
                  "Save, inspection and recovery must not modify source project or candidate files")
        try check(try Data(contentsOf: moved.appendingPathComponent("Samples/\(prefix)-Kick.wav")) == bytes, "Moved package sample bytes must remain intact")
    }
}

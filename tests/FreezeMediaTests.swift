import Foundation
import CryptoKit

/// Pure filesystem integration: generated PCM bytes, private temporary files,
/// and the freeze policy only. No audio decoder, graph, device or plug-in runs.
@main struct FreezeMediaTests {
    typealias Freeze = AudioFreezeProject
    struct Failure: Error, CustomStringConvertible { let description: String }
    struct Fixture {
        let project: [String: Any]
        let source: URL, alternate: URL, print: URL, early: URL
        let contents: [String: Data]
    }
    static var groups = 0
    static func check(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
        if try !condition() { throw Failure(description: message) }
    }
    static func rejects(_ message: String, _ body: () throws -> Void) throws {
        do { try body() }
        catch is Freeze.Failure { return }
        catch is AudioFreezeDurationProof.Failure { return }
        catch is MediaRelinker.Failure { return }
        catch is TrackerError { return }
        throw Failure(description: "Expected rejection: " + message)
    }
    static func passed(_ message: String) { groups += 1; print("PASS freeze media: " + message) }
    static func json(_ value: Any) throws -> Data { try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys, .fragmentsAllowed]) }
    static func hash(_ data: Data) -> String { SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined() }
    static func folder(_ root: URL, _ name: String) throws -> URL {
        let result = root.appendingPathComponent(name)
        try FileManager.default.createDirectory(at: result, withIntermediateDirectories: true)
        return result
    }
    static func record(_ project: [String: Any]) -> [String: Any] { (project["tracks"] as! [[String: Any]])[0]["freeze"] as! [String: Any] }
    static func assets(_ project: [String: Any]) -> [[String: Any]] { record(project)["assets"] as! [[String: Any]] }
    static func sourceEvidence(_ project: [String: Any]) -> [String: String] {
        Dictionary(uniqueKeysWithValues: (record(project)["media"] as! [[String: Any]]).map { ($0["path"] as! String, $0["sha256"] as! String) })
    }
    static func verified(_ project: [String: Any]) -> [String: String] {
        Dictionary(uniqueKeysWithValues: assets(project).map { ($0["path"] as! String, $0["sha256"] as! String) })
    }
    static func clip(_ project: [String: Any]) -> [String: Any] { (project["audioClips"] as! [[String: Any]])[0] }
    static func takes(_ project: [String: Any]) -> [[String: Any]] { (clip(project)["takeSet"] as! [String: Any])["takes"] as! [[String: Any]] }
    static func references(_ project: [String: Any], url: URL? = nil) -> [[String: Any]] {
        MediaRelinker.inspect(project, projectURL: url)["references"] as! [[String: Any]]
    }
    static func stageReference(_ project: [String: Any], _ index: Int = 0) throws -> [String: Any] {
        let id = "/tracks/0/freeze/assets/\(index)/path"
        guard let row = references(project).first(where: { $0["id"] as? String == id }) else { throw Failure(description: "Missing frozen stage reference \(id)") }
        return row
    }
    static func selection(_ row: [String: Any], _ target: URL, different: Bool = false) -> [String: Any] {
        ["id": row["id"]!, "from": row["path"]!, "to": target.path, "allowDifferentContent": different]
    }
    static func applied(_ project: [String: Any], _ selections: [[String: Any]]) throws -> [String: Any] {
        try MediaRelinker.apply(project, selections: selections)["project"] as! [String: Any]
    }
    static func withoutPaths(_ value: Any) -> Any {
        if let object = value as? [String: Any] {
            return object.reduce(into: [String: Any]()) { result, entry in
                if !["path", "samplePath", "mediaManifest", "mediaRelinkHistory"].contains(entry.key) { result[entry.key] = withoutPaths(entry.value) }
            }
        }
        if let array = value as? [Any] { return array.map(withoutPaths) }
        return value
    }
    static func wav(_ value: Int16, frames: UInt32 = 1_016) -> Data {
        var result = Data()
        func text(_ value: String) { result.append(contentsOf: value.utf8) }
        func u16(_ value: UInt16) { result.append(UInt8(value & 255)); result.append(UInt8(value >> 8)) }
        func u32(_ value: UInt32) { for shift in stride(from: 0, to: 32, by: 8) { result.append(UInt8((value >> shift) & 255)) } }
        text("RIFF"); u32(36 + frames * 4); text("WAVEfmt "); u32(16); u16(1); u16(2)
        u32(8_000); u32(32_000); u16(4); u16(16); text("data"); u32(frames * 4)
        for _ in 0..<frames { u16(UInt16(bitPattern: value)); u16(UInt16(bitPattern: -value)) }
        return result
    }
    static func fixture(_ root: URL) throws -> Fixture {
        let library = try folder(root, "Original library")
        let source = library.appendingPathComponent("Source.wav"), alternate = library.appendingPathComponent("Alternate.wav")
        let print = library.appendingPathComponent("Frozen.wav"), early = library.appendingPathComponent("Early.wav")
        let contents = [source.path: wav(101), alternate.path: wav(202), print.path: wav(303), early.path: wav(404)]
        for (path, bytes) in contents { try bytes.write(to: URL(fileURLWithPath: path)) }
        let tracks: [[String: Any]] = [
            ["id": 0, "routingID": "source", "name": "Frozen track", "instrument": 0, "volume": 0.6, "pan": -0.1],
            ["id": 1, "routingID": "upstream", "instrument": 1, "volume": 0.7]]
        let zones: [[String: Any]] = [["path": source.path, "samplePath": source.path, "rootNote": 60]]
        let instruments: [[String: Any]] = [
            ["id": 0, "type": "sample", "samplePath": source.path, "zones": zones],
            ["id": 1, "type": "sample", "mode": "wavetable", "wavetablePosition": 0.0, "samplePath": "builtin:wavetables/Basic/Sine.wav"]]
        let steps: [[Any]] = [[["note": 60, "instrument": 0]], [["note": 48, "instrument": 1]]]
        let patterns: [[String: Any]] = [["id": 0, "length": 1, "steps": steps, "automation": []]]
        let takeRows: [[String: Any]] = [
            ["id": "active", "name": "Chosen", "path": source.path, "sourceOffset": 0.005],
            ["id": "alternate", "name": "Alternate", "path": alternate.path, "sourceOffset": 0.01],
            ["id": "trimmed", "name": "Alternate trimmed", "path": alternate.path, "sourceOffset": 0.02]]
        let takeSet: [String: Any] = ["version": 1, "activeID": "active", "takes": takeRows]
        let clips: [[String: Any]] = [["id": "clip", "name": "Original performance", "track": 0, "path": source.path, "samplePath": source.path,
            "startBeat": 0.0, "durationBeats": 0.20, "sourceOffset": 0.005, "gain": 0.8, "takeSet": takeSet]]
        let buses: [[String: Any]] = [["id": "return", "type": "return", "name": "Return"]]
        let sends: [[String: Any]] = [
            ["id": "early", "source": "track:source", "destination": "bus:return", "tap": "preInserts", "gain": 0.2],
            ["id": "incoming", "source": "track:upstream", "destination": "track:source", "tap": "postFader", "gain": 0.3]]
        let routing: [String: Any] = ["version": 1, "mixModel": "standard", "buses": buses,
            "outputs": ["track:source": "master", "track:upstream": "master", "bus:return": "master"], "sends": sends, "sidechains": []]
        let project: [String: Any] = ["version": 1, "name": "Freeze persistence", "bpm": 120.0, "linesPerBeat": 4, "swing": 0,
            "tracks": tracks, "instruments": instruments, "patterns": patterns, "sequence": [0],
            "audioPlugins": [], "audioClips": clips, "routing": routing]
        let evidence = Freeze.RuntimeEvidence(availablePluginIDs: [], verifiedMediaHashes: [source.path: hash(contents[source.path]!)], validatedSidechainIDs: [], liveInputOwners: [], recording: false)
        let plan = try Freeze.prepare(project: project, owner: "track:source", evidence: evidence)
        let captured = try Freeze.makeRecord(plan: plan, project: project, id: "capture", sampleRate: 8_000, musicalFrames: 1_000, tailFrames: 16,
            assets: [Freeze.Asset(stage: .preFader, path: print.path, sha256: hash(contents[print.path]!), frames: 1_016, trimmedLatencyFrames: 12),
                     Freeze.Asset(stage: .preInserts, path: early.path, sha256: hash(contents[early.path]!), frames: 1_016, trimmedLatencyFrames: 0)])
        return Fixture(project: try Freeze.attaching(captured, to: project), source: source, alternate: alternate, print: print, early: early, contents: contents)
    }
    static func portable(_ root: URL) throws {
        let f = try fixture(root), original = try json(f.project)
        let fingerprinted = try MediaRelinker.fingerprint(f.project)
        try check(fingerprinted["fingerprinted"] as? Int == 4, "All four unique source/take/printed files must be fingerprinted once")
        let project = fingerprinted["project"] as! [String: Any], store = try ProjectStore(directory: try folder(root, "Private support"))
        let before = try folder(root, "Before"), destination = before.appendingPathComponent("Session.supertracker")
        let saved = try store.save(to: destination, snapshot: project)
        let raw = try JSONSerialization.jsonObject(with: Data(contentsOf: destination)) as! [String: Any]
        let rawPaths = references(raw, url: destination).map { $0["path"] as! String }
        try check(rawPaths.allSatisfy { $0.hasPrefix("Session.media/") } && Set(rawPaths).count == 4, "Portable source, take and printed paths must all be relative and deduplicated")
        let manifest = raw["mediaManifest"] as! [[String: Any]]
        try check(Set(manifest.map { $0["path"] as! String }) == Set(rawPaths), "The generic manifest must follow all collected paths")
        try check(sourceEvidence(raw).keys.allSatisfy { $0.hasPrefix("Session.media/") }, "Recorded source evidence must relocate with canonical source paths")
        try check(try json(withoutPaths(raw)) == json(withoutPaths(project)), "Collection must preserve capture frames, hashes, fingerprints and canonical musical data")
        try check(try !Freeze.validate(project: raw)[0].needsRefresh && !Freeze.validate(project: saved)[0].needsRefresh, "Relative and live collected records must both remain fresh")
        let media = before.appendingPathComponent("Session.media")
        try check(try FileManager.default.contentsOfDirectory(atPath: media.path).count == 4, "Shared paths must be collected only once")
        let repeated = try store.save(to: destination)
        try check(try json(repeated) == json(saved), "Repeated save must not rewrite capture metadata or nest collected paths")
        try check(try FileManager.default.contentsOfDirectory(atPath: media.path).count == 4, "Repeated save must not duplicate printed stages")
        let after = root.appendingPathComponent("After")
        try FileManager.default.moveItem(at: before, to: after)
        let reopened = try store.read(after.appendingPathComponent(destination.lastPathComponent))
        try check(try !Freeze.validate(project: reopened)[0].needsRefresh, "Moving the portable project must preserve content-based capture validity")
        try check(MediaRelinker.inspect(reopened)["missingCount"] as? Int == 0, "Every relocated source, take and print must remain available")
        for row in references(reopened) {
            let path = row["path"] as! String, bytes = try Data(contentsOf: URL(fileURLWithPath: path))
            try check(path.hasPrefix(after.path + "/") && f.contents.values.contains(bytes), "Portable media bytes must match the generated originals exactly")
        }
        _ = try Freeze.playbackProjection(project: reopened, verifiedAssets: verified(reopened))
        for (path, bytes) in f.contents { try check(try Data(contentsOf: URL(fileURLWithPath: path)) == bytes, "Original library bytes must remain unchanged") }
        try check(try json(f.project) == original, "Persistence must not mutate the supplied canonical snapshot")
        passed("both stages, shared originals and inactive takes survive collect, repeated save and relocated read byte-for-byte")
    }
    static func exactPrintRelink(_ root: URL) throws {
        let f = try fixture(root), canonical = try json(f.project), row = try stageReference(f.project)
        try check(row["kind"] as? String == "freeze" && row["expectedSHA256"] as? String == hash(f.contents[f.print.path]!), "Frozen stage inventory must expose its inline full SHA without a generic manifest")
        let correct = try folder(root, "Relocated").appendingPathComponent(f.print.lastPathComponent)
        try FileManager.default.moveItem(at: f.print, to: correct)
        let decoy = try folder(root, "Decoy").appendingPathComponent(f.print.lastPathComponent)
        try wav(999).write(to: decoy)
        let report = try MediaRelinker.findCandidates(f.project, searchRoots: [correct.deletingLastPathComponent(), decoy.deletingLastPathComponent()])
        let missing = (report["missing"] as! [[String: Any]]).first { $0["id"] as? String == row["id"] as? String }!
        let candidates = missing["candidates"] as! [[String: Any]]
        try check(candidates.filter { $0["verified"] as? Bool == true }.map { $0["path"] as! String } == [correct.path], "Only exact printed bytes may verify, even with identical filename and size")
        try check(candidates.first { $0["path"] as? String == decoy.path }?["contentMismatch"] as? Bool == true, "Same-size wrong print must be identified as different content")
        for deliberate in [false, true] { try rejects("Frozen print cannot change identity") { _ = try applied(f.project, [selection(row, decoy, different: deliberate)]) } }
        for deliberate in [false, true] { try rejects("Frozen hash verification cannot exceed budget") { _ = try MediaRelinker.apply(f.project, selections: [selection(row, correct, different: deliberate)], maxHashBytes: 1) } }
        var stale = selection(row, correct); stale["from"] = "/stale/Frozen.wav"
        try rejects("Stale print selection") { _ = try applied(f.project, [stale]) }
        var conflicting = f.project
        conflicting["mediaManifest"] = [["path": f.print.path, "sha256": hash(f.contents[f.source.path]!), "size": f.contents[f.print.path]!.count]]
        try check(try stageReference(conflicting)["fingerprintConflict"] as? Bool == true, "Inline and generic print hash disagreement must be visible")
        try rejects("Conflicting manifest cannot override immutable print") { _ = try applied(conflicting, [selection(row, correct, different: true)]) }
        let repaired = try applied(f.project, [selection(row, correct)])
        try check(assets(repaired)[0]["path"] as? String == correct.path, "Exact repair must replace the selected private stage path")
        try check(try json(withoutPaths(repaired)) == json(withoutPaths(f.project)), "Print relink must preserve all canonical data and immutable capture metadata")
        try check(try !Freeze.validate(project: repaired)[0].needsRefresh, "Exact printed-stage relink must remain fresh")
        _ = try Freeze.playbackProjection(project: repaired, verifiedAssets: verified(repaired))
        try check(try json(f.project) == canonical, "Rejected and successful relinks must leave the original snapshot untouched")
        let earlyRow = try stageReference(f.project, 1), earlyTarget = correct.deletingLastPathComponent().appendingPathComponent(f.early.lastPathComponent)
        try FileManager.default.moveItem(at: f.early, to: earlyTarget)
        try rejects("Mixed valid and mismatched print selections must reject atomically") { _ = try applied(f.project, [selection(row, correct), selection(earlyRow, decoy, different: true)]) }
        try check(try json(f.project) == canonical, "Failed multi-stage relink must not partially publish its valid first selection")
        passed("inline full SHA, candidate identity, strict print repair, conflict/budget/stale rejection and atomic batches")
    }
    static func originalRelink(_ root: URL) throws {
        let f = try fixture(root), oldHash = hash(f.contents[f.source.path]!), oldPath = f.source.path
        let relocated = try folder(root, "Relocated"), replacement = relocated.appendingPathComponent("Moved source.wav")
        try FileManager.default.moveItem(at: f.source, to: replacement)
        let oldRows = references(f.project).filter { $0["path"] as? String == oldPath }
        let instrument = oldRows.first { $0["id"] as? String == "/instruments/0/samplePath" }!
        let partial = try applied(f.project, [selection(instrument, replacement)])
        try check(sourceEvidence(partial)[oldPath] == oldHash && sourceEvidence(partial)[replacement.path] == oldHash, "A partial shared-source relocation must retain old and new path evidence with the original hash")
        try check(try !Freeze.validate(project: partial)[0].needsRefresh, "Partial exact relink must preserve source fingerprint normalization")
        let remaining = references(partial).filter { $0["path"] as? String == oldPath }
        let repaired = try applied(partial, remaining.map { selection($0, replacement) })
        try check(sourceEvidence(repaired)[replacement.path] == oldHash, "Complete source relocation must preserve its capture hash")
        try check(references(repaired).allSatisfy { $0["path"] as? String != oldPath }, "All chosen canonical aliases must follow the replacement")
        try check(clip(repaired)["path"] as? String == replacement.path && clip(repaired)["samplePath"] as? String == replacement.path && takes(repaired)[0]["path"] as? String == replacement.path, "Relink must keep the active outer clip, legacy alias and embedded selected take aligned")
        try check(try !Freeze.validate(project: repaired)[0].needsRefresh, "Complete exact relink must preserve frozen source identity")
        try check(try json(record(repaired)["fingerprints"]!) == json(record(f.project)["fingerprints"]!), "Source relocation must never rewrite the baked content fingerprints")
        let different = relocated.appendingPathComponent("Different.wav"); try wav(777).write(to: different)
        try rejects("Different-content replacement of protected frozen source requires unfreeze") { _ = try applied(f.project, [selection(instrument, different, different: true)]) }
        let alternateTarget = relocated.appendingPathComponent(f.alternate.lastPathComponent)
        try FileManager.default.moveItem(at: f.alternate, to: alternateTarget)
        let alternateRows = references(repaired).filter { $0["path"] as? String == f.alternate.path }
        let evidenceBefore = try json(record(repaired)["media"]!)
        let alternativeRepaired = try applied(repaired, alternateRows.map { selection($0, alternateTarget) })
        try check(try json(record(alternativeRepaired)["media"]!) == evidenceBefore, "Inactive take relinks must not change captured source evidence")
        try check(try !Freeze.validate(project: alternativeRepaired)[0].needsRefresh, "Inactive alternatives must remain independent of the frozen selected source")
        passed("partial shared-source relocation preserves recorded hashes, active aliases and independent inactive takes")
    }
    static func corruptPrintRepair(_ root: URL) throws {
        let f = try fixture(root), wrong = wav(909)
        try wrong.write(to: f.print)
        let row = try stageReference(f.project)
        try check(row["available"] as? Bool == false && row["status"] as? String == "contentMismatch", "A present but altered printed file must be reported unavailable for playback")
        try rejects("Fingerprint cannot re-certify altered printed bytes") { _ = try MediaRelinker.fingerprint(f.project) }
        var limits = MediaRelinker.Limits(); limits.maxHashes = 0
        let limited = MediaRelinker.inspect(f.project, limits: limits)["references"] as! [[String: Any]]
        try check(limited.filter { $0["kind"] as? String == "freeze" }.allSatisfy { $0["status"] as? String == "unverified" && $0["available"] as? Bool == false }, "Existing prints must remain unverified when inspection cannot establish their hashes")
        let replacement = try folder(root, "Verified original").appendingPathComponent(f.print.lastPathComponent)
        try f.contents[f.print.path]!.write(to: replacement)
        let repaired = try applied(f.project, [selection(row, replacement)])
        try check(try stageReference(repaired)["available"] as? Bool == true, "A corrupted available print must be repairable with verified original bytes")
        try check(try Data(contentsOf: f.print) == wrong, "Repair must not overwrite the altered file")
        try check(assets(repaired)[0]["sha256"] as? String == hash(f.contents[f.print.path]!), "Repair must retain the immutable captured hash")
        _ = try Freeze.playbackProjection(project: repaired, verifiedAssets: verified(repaired))
        passed("present-but-corrupt print inspection, bounded verification, immutable fingerprints and exact repair")
    }
    static func saveFailures(_ root: URL) throws {
        let f = try fixture(root), support = try folder(root, "Private support"), store = try ProjectStore(directory: support)
        try store.update(f.project)
        let destination = root.appendingPathComponent("Existing.supertracker"), sentinel = Data("previous document bytes".utf8)
        try sentinel.write(to: destination)
        let before = try json(store.project!)
        try wav(999).write(to: f.print)
        try rejects("Corrupt available print cannot be collected") { _ = try store.save(to: destination) }
        try check(try Data(contentsOf: destination) == sentinel && json(store.project!) == before && store.url == nil && store.dirty, "Failed collection must preserve the previous document and store state")
        try check(!FileManager.default.fileExists(atPath: store.recoveryURL.path), "Failed save must not publish a recovery snapshot")
        try f.contents[f.print.path]!.write(to: f.print)
        let media = try folder(root, "Existing.media")
        let collision = media.appendingPathComponent(String(hash(f.contents[f.print.path]!).prefix(16)) + "-" + f.print.lastPathComponent)
        let wrong = wav(888); try wrong.write(to: collision)
        try rejects("A pre-existing collected filename must not substitute corrupt bytes") { _ = try store.save(to: destination) }
        try check(try Data(contentsOf: collision) == wrong && Data(contentsOf: destination) == sentinel && json(store.project!) == before, "Collision rejection must neither overwrite the existing media nor publish the project")
        var malformed = f.project, tracks = malformed["tracks"] as! [[String: Any]], raw = record(malformed)
        raw["version"] = 2; tracks[0]["freeze"] = raw; malformed["tracks"] = tracks
        try rejects("Persistence must reject malformed freeze metadata") { try store.validate(malformed) }
        passed("corrupted available and pre-existing collected prints reject before document publication, with strict freeze schema")
    }
    static func missingMedia(_ root: URL) throws {
        let f = try fixture(root), store = try ProjectStore(directory: try folder(root, "Private support"))
        try FileManager.default.removeItem(at: f.source)
        let destination = root.appendingPathComponent("MissingOriginal.supertracker")
        let saved = try store.save(to: destination, snapshot: f.project)
        let reopened = try store.read(destination)
        try check(try !Freeze.validate(project: reopened)[0].needsRefresh, "Pure persisted validation must not invent a changed source hash when originals are unavailable")
        let stale = try Freeze.playbackProjection(project: reopened, verifiedAssets: verified(reopened), currentMediaHashes: [:])
        try check(stale.statuses[0].needsRefresh && stale.substitutions.count == 1, "Verified print must remain usable when observed original media is missing")
        let savedAssets = assets(saved)
        try FileManager.default.removeItem(atPath: savedAssets[0]["path"] as! String)
        try Data().write(to: URL(fileURLWithPath: savedAssets[1]["path"] as! String))
        let missingURL = root.appendingPathComponent("Repairable.supertracker")
        _ = try store.save(to: missingURL, snapshot: saved)
        let missing = try store.read(missingURL)
        let unavailable = references(missing).filter { $0["kind"] as? String == "freeze" && $0["available"] as? Bool == false }
        try check(unavailable.count == 2, "Missing and empty printed files must persist and stay discoverable for repair")
        try check(try json(assets(missing).map(withoutPaths)) == json(assets(f.project).map(withoutPaths)), "Repairable prints must retain exact frames, trim metadata and recorded SHA values")
        try rejects("Persisted missing prints are not evidence of playable media") { _ = try Freeze.playbackProjection(project: missing, verifiedAssets: [:]) }
        let recoveredFolder = try folder(root, "Recovered originals")
        var selections: [[String: Any]] = []
        for row in unavailable {
            let original = (row["id"] as! String).contains("/assets/0/") ? f.print : f.early
            let target = recoveredFolder.appendingPathComponent(original.lastPathComponent)
            try f.contents[original.path]!.write(to: target)
            selections.append(selection(row, target))
        }
        let repaired = try applied(missing, selections)
        try check(references(repaired).filter { $0["kind"] as? String == "freeze" }.allSatisfy { $0["available"] as? Bool == true }, "Both missing and empty print references must be repairable by exact original bytes")
        _ = try Freeze.playbackProjection(project: repaired, verifiedAssets: verified(repaired))
        passed("missing originals preserve verified prints; missing and empty prints persist, remain gated and repair exactly")
    }
    static func changedOriginalSafety(_ root: URL) throws {
        let f = try fixture(root), originalHash = hash(f.contents[f.source.path]!), wrong = wav(515)
        let originalRecord = try json(record(f.project)), originalProject = try json(f.project)
        try wrong.write(to: f.source)
        let store = try ProjectStore(directory: try folder(root, "Private support"))
        let destination = root.appendingPathComponent("ChangedSource.supertracker")
        try rejects("Collect cannot attach a captured source SHA to newly different bytes") { _ = try store.save(to: destination, snapshot: f.project) }
        try check(!FileManager.default.fileExists(atPath: destination.path) && store.project == nil && store.url == nil, "Changed original collection must fail before publishing any document or store state")
        try check(try json(f.project) == originalProject, "Failed source collection must not recertify the caller's freeze metadata")
        let metadataURL = root.appendingPathComponent("RepairableMetadata.supertracker")
        let retained = try store.save(to: metadataURL, snapshot: f.project, collectSamples: false)
        let read = try store.read(metadataURL)
        try check(try json(record(retained)) == originalRecord && json(record(read)) == originalRecord, "Saving without collection must retain repairable metadata with exact original fingerprints")
        try check(sourceEvidence(read)[f.source.path] == originalHash, "Changed original bytes must never replace their recorded source SHA")
        let observed = [f.source.path: hash(wrong)]
        try check(try Freeze.validate(project: read, currentMediaHashes: observed)[0].needsRefresh, "Actual changed original bytes must be observable as stale capture evidence")
        let sourceRow = references(f.project).first { $0["id"] as? String == "/instruments/0/samplePath" }!
        try rejects("Protected same-path replacement must inspect identity, not just changed path spelling") { _ = try applied(f.project, [selection(sourceRow, f.source, different: true)]) }
        try rejects("Captured-source verification budget cannot be bypassed by explicit replacement") { _ = try MediaRelinker.apply(f.project, selections: [selection(sourceRow, f.source, different: true)], maxHashBytes: 0) }
        try check(try json(record(f.project)) == originalRecord && Data(contentsOf: f.source) == wrong, "Rejected same-path relink must preserve the capture evidence and current file")
        passed("changed protected source rejects collection and same-path replacement while metadata-only saves stay repairable")
    }
    static func upstreamSamePath(_ root: URL) throws {
        let f = try fixture(root), upstreamPath = root.appendingPathComponent("Upstream.wav"), upstreamBytes = wav(616)
        try upstreamBytes.write(to: upstreamPath)
        var canonical = try Freeze.removing(owner: "track:source", from: f.project)
        var instruments = canonical["instruments"] as! [[String: Any]]
        instruments[1] = ["id": 1, "type": "sample", "samplePath": upstreamPath.path]
        canonical["instruments"] = instruments
        let evidence = Freeze.RuntimeEvidence(availablePluginIDs: [],
            verifiedMediaHashes: [f.source.path: hash(f.contents[f.source.path]!), upstreamPath.path: hash(upstreamBytes)],
            validatedSidechainIDs: [], liveInputOwners: [], recording: false)
        let plan = try Freeze.prepare(project: canonical, owner: "track:source", evidence: evidence)
        let previous = try Freeze.parse(record(f.project))
        let capture = try Freeze.makeRecord(plan: plan, project: canonical, id: "upstream-capture", sampleRate: previous.sampleRate,
            musicalFrames: previous.musicalFrames, tailFrames: previous.tailFrames, assets: previous.assets)
        let frozen = try Freeze.attaching(capture, to: canonical), immutable = try json(record(frozen))
        let changed = wav(717); try changed.write(to: upstreamPath)
        let upstreamRow = references(frozen).first { $0["id"] as? String == "/instruments/1/samplePath" }!
        try rejects("Even upstream replacement must establish actual bytes before recording observed drift") { _ = try MediaRelinker.apply(frozen, selections: [selection(upstreamRow, upstreamPath, different: true)], maxHashBytes: 0) }
        let relinked = try applied(frozen, [selection(upstreamRow, upstreamPath, different: true)])
        try check(try json(record(relinked)) == immutable, "An upstream same-path replacement must preserve the immutable captured source hash")
        let manifest = relinked["mediaManifest"] as! [[String: Any]]
        let observed = Dictionary(uniqueKeysWithValues: manifest.compactMap { item -> (String, String)? in
            guard let path = item["path"] as? String, let hash = item["sha256"] as? String else { return nil }; return (path, hash)
        })
        try check(observed[upstreamPath.path] == hash(changed), "Generic observed media evidence must record actual replacement bytes")
        try check(try Freeze.validate(project: relinked)[0].needsRefresh, "Persisted observed media evidence must expose same-path upstream drift without recertifying its recorded SHA")
        var currentHashes = sourceEvidence(relinked); currentHashes.merge(observed) { _, current in current }
        try check(try Freeze.validate(project: relinked, currentMediaHashes: currentHashes)[0].needsRefresh, "Upstream same-path content changes must mark the retained print stale")
        let projection = try Freeze.playbackProjection(project: relinked, verifiedAssets: verified(relinked), currentMediaHashes: currentHashes)
        try check(projection.statuses[0].needsRefresh && projection.substitutions.count == 1, "An independently stale upstream source must leave the verified print available")
        passed("upstream same-path replacement records observed bytes without recertifying the capture and keeps its verified print")
    }
    static func inconsistentExtentRepair(_ root: URL) throws {
        let f = try fixture(root)
        let plan = try Freeze.prepare(project: f.project, owner: "track:upstream",
            evidence: Freeze.RuntimeEvidence(availablePluginIDs: [], verifiedMediaHashes: [:], validatedSidechainIDs: [], liveInputOwners: [], recording: false))
        let absent = root.appendingPathComponent("Missing second capture.wav")
        let second = try Freeze.makeRecord(plan: plan, project: f.project, id: "different-end", sampleRate: 8_000,
            musicalFrames: 1_001, tailFrames: 0,
            assets: [Freeze.Asset(stage: .preFader, path: absent.path, sha256: hash(wav(505, frames: 1_001)), frames: 1_001, trimmedLatencyFrames: 0)])
        let inconsistent = try Freeze.attaching(second, to: f.project), before = try json(inconsistent)
        try rejects("Mismatched common end cannot be admitted") { _ = try Freeze.commonCapturedExtent(project: inconsistent) }
        let store = try ProjectStore(directory: try folder(root, "Private support"))
        let destination = root.appendingPathComponent("Repairable endpoints.supertracker")
        _ = try store.save(to: destination, snapshot: inconsistent, collectSamples: false)
        let reopened = try store.read(destination)
        let retained = try Freeze.validate(project: reopened)
        try check(retained.map { $0.record.musicalFrames } == [1_000, 1_001], "Save/read must preserve both exact captured endpoints for repair")
        try rejects("Persistence cannot certify inconsistent endpoints") { _ = try Freeze.commonCapturedExtent(project: reopened) }
        let replacement = try folder(root, "Relocated").appendingPathComponent("Original print.wav")
        try FileManager.default.moveItem(at: f.print, to: replacement)
        let relinked = try applied(reopened, [selection(try stageReference(reopened), replacement)])
        try check(assets(relinked)[0]["path"] as? String == replacement.path, "An exact printed-stage relink must remain possible while another capture has a different endpoint")
        let repaired = try Freeze.removing(owner: "track:upstream", from: relinked)
        try check(try Freeze.commonCapturedExtent(project: repaired) == Freeze.CapturedExtent(sampleRate: 8_000, musicalFrames: 1_000), "Explicit Unfreeze must repair the common endpoint without reading the absent second print")
        _ = try store.save(to: destination, snapshot: repaired, collectSamples: false)
        try check(try Freeze.commonCapturedExtent(project: store.read(destination)) == Freeze.CapturedExtent(sampleRate: 8_000, musicalFrames: 1_000), "Repaired endpoint must survive another Save/Open")
        try check(try json(inconsistent) == before, "Admission, Save and repair must not mutate the caller's inconsistent canonical snapshot")
        passed("inconsistent endpoint admission stays separate from metadata Save/Open, exact print relink and missing-print Unfreeze repair")
    }
    static func legacyProofRepair(_ root: URL) throws {
        let f = try fixture(root), store = try ProjectStore(directory: try folder(root, "Private support"))
        let destination = root.appendingPathComponent("Legacy.supertracker")
        let saved = try store.save(to: destination, snapshot: f.project)
        let raw = try JSONSerialization.jsonObject(with: Data(contentsOf: destination)) as! [String: Any]
        let reopened = try store.read(destination)
        try check([f.project, saved, raw, reopened].allSatisfy { record($0)["durationProof"] == nil }, "Collect and Save/Open must not invent duration proof for a legacy capture")
        try check(try !activationEligible(reopened), "Save/Open must not admit a legacy capture without duration proof")
        let row = try stageReference(reopened), replacement = try folder(root, "Repaired print").appendingPathComponent("Original.wav")
        try FileManager.default.moveItem(atPath: row["path"] as! String, toPath: replacement.path)
        let relinked = try applied(reopened, [selection(row, replacement)])
        try check(record(relinked)["durationProof"] == nil, "Exact printed-media verification cannot retroactively certify legacy duration")
        _ = try store.save(to: destination, snapshot: relinked, collectSamples: false)
        try check(record(try store.read(destination))["durationProof"] == nil, "Saving repaired media must retain absent proof")
        let unfrozen = try Freeze.removing(owner: "track:source", from: relinked)
        try check((unfrozen["tracks"] as! [[String: Any]])[0]["freeze"] == nil, "Legacy duration proof must not be required to explicitly Unfreeze")
        try check(try json(unfrozen["audioClips"]!) == json(relinked["audioClips"]!), "Unfreeze repair must retain canonical clips")
        passed("legacy absent proof stays absent through collect, Save/Open, exact print repair and explicit Unfreeze")
    }
    static func opaqueProofRepair(_ root: URL) throws {
        for malformed in [false, true] {
            let scenario = try folder(root, malformed ? "Malformed v1" : "Unknown v99")
            let f = try fixture(scenario), evidenceOnly = scenario.appendingPathComponent("Opaque evidence only.wav")
            let evidenceBytes = wav(818); try evidenceBytes.write(to: evidenceOnly)
            let opaque: [String: Any] = [
                "version": malformed ? 1 : 99,
                "path": evidenceOnly.path,
                "samplePath": evidenceOnly.path,
                "sources": ["not a supported source", ["paths": [evidenceOnly.path], "path": evidenceOnly.path]],
                "nested": [["samplePath": evidenceOnly.path], ["path": "Relative evidence.wav"]],
                "future": ["array": [NSNull(), 7, true, "untouched"], "tag": "opaque\nmetadata"]]
            let opaqueBytes = try json(opaque)
            var project = f.project, tracks = project["tracks"] as! [[String: Any]], capture = record(project)
            capture["durationProof"] = opaque; tracks[0]["freeze"] = capture; project["tracks"] = tracks
            let original = try json(project), before = try folder(scenario, "Before")
            let destination = before.appendingPathComponent("Opaque.supertracker")
            let store = try ProjectStore(directory: try folder(scenario, "Private support"))
            let saved = try store.save(to: destination, snapshot: project)
            let raw = try JSONSerialization.jsonObject(with: Data(contentsOf: destination)) as! [String: Any]
            try check(try json(record(saved)["durationProof"]!) == opaqueBytes && json(record(raw)["durationProof"]!) == opaqueBytes,
                      "Collect must preserve unsupported or malformed proof as exact JSON, including apparent media fields")
            let collected = try FileManager.default.contentsOfDirectory(at: before.appendingPathComponent("Opaque.media"), includingPropertiesForKeys: nil)
            try check(collected.count == 4 && !collected.contains { $0.lastPathComponent.hasSuffix(evidenceOnly.lastPathComponent) },
                      "Opaque proof paths must not become collection candidates or copy evidence-only files")
            let after = scenario.appendingPathComponent("After")
            try FileManager.default.moveItem(at: before, to: after)
            let movedDocument = after.appendingPathComponent(destination.lastPathComponent), reopened = try store.read(movedDocument)
            try check(try json(record(reopened)["durationProof"]!) == opaqueBytes, "Project relocation and relative-path expansion must not reinterpret opaque proof")
            try check(try !activationEligible(reopened), "Unsupported or malformed proof must remain unavailable after Save/Open")
            let row = try stageReference(reopened), replacement = try folder(scenario, "Repaired print").appendingPathComponent("Verified.wav")
            try FileManager.default.moveItem(atPath: row["path"] as! String, toPath: replacement.path)
            let repaired = try applied(reopened, [selection(row, replacement)])
            try check(try json(record(repaired)["durationProof"]!) == opaqueBytes, "An exact print relink must preserve opaque proof without upgrading or recertifying it")
            _ = try store.save(to: movedDocument, snapshot: repaired, collectSamples: false)
            try check(try json(record(store.read(movedDocument))["durationProof"]!) == opaqueBytes, "Opaque repair metadata must survive a second Save/Open")
            let unfrozen = try Freeze.removing(owner: "track:source", from: repaired)
            try check((unfrozen["tracks"] as! [[String: Any]])[0]["freeze"] == nil, "Unsupported or malformed proof must remain explicitly unfreezable")
            try check(try json(unfrozen["audioClips"]!) == json(repaired["audioClips"]!), "Opaque proof removal must retain original canonical clip data")
            try check(try json(project) == original && Data(contentsOf: evidenceOnly) == evidenceBytes, "Persistence and repair must not mutate the caller or evidence-only original")
        }
        passed("unsupported and malformed duration proofs stay opaque through collect, project relocation, exact print repair and Unfreeze")
    }
    static func proof(_ project: [String: Any]) -> [String: Any] { record(project)["durationProof"] as! [String: Any] }
    static func proofSources(_ project: [String: Any]) -> [[String: Any]] { proof(project)["sources"] as! [[String: Any]] }
    static func proofAliases(_ project: [String: Any], hash: String) -> [String] {
        proofSources(project).first { $0["sha256"] as? String == hash }?["paths"] as? [String] ?? []
    }
    static func activationEligible(_ project: [String: Any], liveSources: [String: AudioFreezeDurationProof.DecodedSource] = [:]) throws -> Bool {
        if case .eligible = try Freeze.assessActivation(project: project, verifiedAssets: verified(project), liveSources: liveSources) { return true }
        return false
    }
    static func proofIdentity(_ project: [String: Any]) -> [String: Any] {
        var value = proof(project)
        value["sources"] = proofSources(project).map { source -> [String: Any] in
            var identity = source; identity.removeValue(forKey: "paths"); return identity
        }
        return value
    }
    /// These generated bytes and their exact PCM metadata exercise only the pure
    /// builder contract. No native capture or decoded-asset admission is claimed.
    static func durationFixture(_ root: URL, sharedLiveClip: Bool = false) throws -> (project: [String: Any], live: URL, bytes: Data) {
        let f = try fixture(root), live = f.source.deletingLastPathComponent().appendingPathComponent("Unrelated live clip.wav"), bytes = wav(919)
        try bytes.write(to: live)
        var canonical = try Freeze.removing(owner: "track:source", from: f.project)
        var routing = canonical["routing"] as! [String: Any]
        routing["sends"] = (routing["sends"] as! [[String: Any]]).filter { $0["id"] as? String != "incoming" }
        canonical["routing"] = routing
        var clips = canonical["audioClips"] as! [[String: Any]]
        clips.append(["id": "unrelated-live", "track": 1, "path": live.path, "startBeat": 0.1, "durationBeats": 0.1, "sourceOffset": 0.0])
        if sharedLiveClip { clips.append(["id": "shared-live", "track": 1, "path": live.path, "startBeat": 0.05, "durationBeats": 0.05, "sourceOffset": 0.0]) }
        canonical["audioClips"] = clips
        let sourceHash = hash(f.contents[f.source.path]!), liveHash = hash(bytes)
        let evidence = Freeze.RuntimeEvidence(availablePluginIDs: [], verifiedMediaHashes: [f.source.path: sourceHash, live.path: liveHash],
            validatedSidechainIDs: [], liveInputOwners: [], recording: false)
        let plan = try Freeze.prepare(project: canonical, owner: "track:source", evidence: evidence), previous = try Freeze.parse(record(f.project))
        let capture = try Freeze.makeRecord(plan: plan, project: canonical, id: "duration-proof", sampleRate: 8_000,
            musicalFrames: 1_000, tailFrames: 16, assets: previous.assets)
        let duration = try AudioFreezeDurationProof.make(project: canonical, sampleRate: 8_000, musicalFrames: 1_000, resolvedSources: [
            f.source.path: AudioFreezeDurationProof.DecodedSource(sha256: sourceHash, decodedFrames: 1_016, decodedSampleRate: 8_000),
            live.path: AudioFreezeDurationProof.DecodedSource(sha256: liveHash, decodedFrames: 1_016, decodedSampleRate: 8_000)])
        var project = try Freeze.attaching(capture, to: canonical), tracks = project["tracks"] as! [[String: Any]], raw = record(project)
        raw["durationProof"] = duration.dictionary; tracks[0]["freeze"] = raw; project["tracks"] = tracks
        _ = try Freeze.validate(project: project)
        try check(sourceEvidence(project)[live.path] == nil && proofAliases(project, hash: liveHash).contains(live.path),
                  "The unrelated live clip must be protected only by duration evidence, outside the frozen source/dependency media")
        return (project, live, bytes)
    }
    static func knownProofPortable(_ root: URL) throws {
        let f = try durationFixture(root), identity = try json(proofIdentity(f.project)), original = try json(f.project)
        let capturedSource = clip(f.project)["path"] as! String, sourceHash = sourceEvidence(f.project)[capturedSource]!
        let before = try folder(root, "Before"), destination = before.appendingPathComponent("Known.supertracker")
        let store = try ProjectStore(directory: try folder(root, "Private support"))
        let saved = try store.save(to: destination, snapshot: f.project)
        let raw = try JSONSerialization.jsonObject(with: Data(contentsOf: destination)) as! [String: Any]
        for value in [saved, raw] {
            let paths = (value["audioClips"] as! [[String: Any]]).map { $0["path"] as! String }
            for (index, digest) in [(0, sourceHash), (1, hash(f.bytes))] {
                try check(proofAliases(value, hash: digest).contains(paths[index]), "Collected canonical paths must retain their exact decoded-content proof identity")
            }
            try check(try json(proofIdentity(value)) == identity, "Collection must preserve temporal SHA and exact decoded frame/rate evidence")
        }
        try check(try FileManager.default.contentsOfDirectory(atPath: before.appendingPathComponent("Known.media").path).count == 5,
                  "Duration aliases must not duplicate collected source, take or print files")
        let repeated = try store.save(to: destination)
        try check(try json(repeated) == json(saved), "Repeated Save must not accumulate aliases or recertify duration evidence")
        let after = root.appendingPathComponent("After"); try FileManager.default.moveItem(at: before, to: after)
        let reopened = try store.read(after.appendingPathComponent(destination.lastPathComponent))
        let livePath = (reopened["audioClips"] as! [[String: Any]])[1]["path"] as! String
        try check(livePath.hasPrefix(after.path + "/") && proofAliases(reopened, hash: hash(f.bytes)).contains(livePath),
                  "Moving the portable project must resolve the selected live clip and its proof alias to the same local path")
        try check(try json(proofIdentity(reopened)) == identity && Data(contentsOf: URL(fileURLWithPath: livePath)) == f.bytes,
                  "Relocated proof must retain its exact temporal and decoded-content identity")
        try check(try activationEligible(reopened, liveSources: [livePath: .init(sha256: hash(f.bytes), decodedFrames: 1_016, decodedSampleRate: 8_000)]),
                  "The relocated proof must still pass pure temporal admission with matching current live-source evidence")
        try check(try json(f.project) == original && Data(contentsOf: f.live) == f.bytes, "Portable proof persistence must not alter the caller or original library")
        passed("known proof aliases follow verified collection and moved-project reads while temporal SHA, decoded bounds and repeated Save remain stable")
    }
    static func durationOnlyRelink(_ root: URL) throws {
        let f = try durationFixture(root), digest = hash(f.bytes), original = try json(f.project), identity = try json(proofIdentity(f.project))
        let row = references(f.project).first { $0["id"] as? String == "/audioClips/1/path" }!
        try check(row["expectedSHA256"] as? String == digest, "Relinker must expose duration-only source identity without a generic manifest or old freeze.media entry")
        let moved = try folder(root, "Moved").appendingPathComponent(f.live.lastPathComponent)
        try FileManager.default.moveItem(at: f.live, to: moved)
        let wrong = try folder(root, "Wrong").appendingPathComponent(f.live.lastPathComponent), wrongBytes = wav(929)
        try wrongBytes.write(to: wrong)
        for deliberate in [false, true] {
            try rejects("Same-length different bytes cannot recertify duration source identity") { _ = try applied(f.project, [selection(row, wrong, different: deliberate)]) }
        }
        try rejects("Duration-only captured identity needs full verification even with explicit replacement") {
            _ = try MediaRelinker.apply(f.project, selections: [selection(row, moved, different: true)], maxHashBytes: 0)
        }
        let report = try MediaRelinker.findCandidates(f.project, searchRoots: [moved.deletingLastPathComponent(), wrong.deletingLastPathComponent()])
        let missing = (report["missing"] as! [[String: Any]]).first { $0["id"] as? String == row["id"] as? String }!
        let candidates = missing["candidates"] as! [[String: Any]]
        try check(candidates.filter { $0["verified"] as? Bool == true }.map { $0["path"] as! String } == [moved.path],
                  "Duration-only relink candidates must verify full content identity, never equal frame counts or names")
        let repaired = try applied(f.project, [selection(row, moved)])
        try check(proofAliases(repaired, hash: digest).contains(f.live.path) && proofAliases(repaired, hash: digest).contains(moved.path),
                  "Verified relocation must append the new alias while retaining the historical source alias")
        try check(try json(proofIdentity(repaired)) == identity && sourceEvidence(repaired)[moved.path] == nil,
                  "Relinking duration-only evidence must preserve its decoded bounds and never promote it into the frozen owner's baked source evidence")
        try check(try activationEligible(repaired, liveSources: [moved.path: .init(sha256: digest, decodedFrames: 1_016, decodedSampleRate: 8_000)]),
                  "Exact alias repair must retain pure temporal admission with matching current live-source evidence")
        try check(try json(f.project) == original && Data(contentsOf: moved) == f.bytes && Data(contentsOf: wrong) == wrongBytes,
                  "Rejected and accepted proof relinks must preserve the original snapshot and every source file")
        let shared = try durationFixture(try folder(root, "Shared source"), sharedLiveClip: true)
        let sharedOriginal = try json(shared.project), sharedIdentity = try json(proofIdentity(shared.project)), sharedHash = hash(shared.bytes)
        let rows = references(shared.project), first = rows.first { $0["id"] as? String == "/audioClips/1/path" }!, second = rows.first { $0["id"] as? String == "/audioClips/2/path" }!
        let targets = try folder(root, "Split aliases"), a = targets.appendingPathComponent("A.wav"), b = targets.appendingPathComponent("B.wav")
        try FileManager.default.moveItem(at: shared.live, to: a); try FileManager.default.copyItem(at: a, to: b)
        let split = try applied(shared.project, [selection(first, a), selection(second, b)])
        let aliases = proofAliases(split, hash: sharedHash), clips = split["audioClips"] as! [[String: Any]]
        try check(clips[1]["path"] as? String == a.path && clips[2]["path"] as? String == b.path && [shared.live.path, a.path, b.path].allSatisfy(aliases.contains),
                  "A reference-level batch may append two independently verified destinations for one historical source path")
        try check(try json(proofIdentity(split)) == sharedIdentity && activationEligible(split, liveSources: [
            a.path: .init(sha256: sharedHash, decodedFrames: 1_016, decodedSampleRate: 8_000),
            b.path: .init(sha256: sharedHash, decodedFrames: 1_016, decodedSampleRate: 8_000)]),
                  "Split aliases must retain the one historical hash, exact decoded extent and pure temporal admission")
        try check(try json(shared.project) == sharedOriginal && Data(contentsOf: a) == shared.bytes && Data(contentsOf: b) == shared.bytes,
                  "Split reference relink must leave the supplied snapshot and replacement files intact")
        passed("duration-only live sources require exact-hash relink, reject override/budget bypass, and retain admission for shared-source split aliases")
    }
    static func changedDurationSourceCollect(_ root: URL) throws {
        let f = try durationFixture(root), originalHash = hash(f.bytes), identity = try json(proofIdentity(f.project)), changed = wav(939)
        try changed.write(to: f.live)
        let store = try ProjectStore(directory: try folder(root, "Private support")), destination = root.appendingPathComponent("Repairable source.supertracker")
        let saved = try store.save(to: destination, snapshot: f.project), reopened = try store.read(destination)
        for value in [saved, reopened] {
            let selected = (value["audioClips"] as! [[String: Any]])[1]["path"] as! String
            try check(selected != f.live.path && !proofAliases(value, hash: originalHash).contains(selected),
                      "Collecting changed live bytes must not carry a historical duration-source hash onto the new file")
            try check(try json(proofIdentity(value)) == identity, "A repairable Save must not mint fresh temporal or decoded evidence for changed source bytes")
            try check(try Data(contentsOf: URL(fileURLWithPath: selected)) == changed, "Saved canonical clip must point to the actual collected bytes")
            try check(try !activationEligible(value, liveSources: [selected: .init(sha256: hash(changed), decodedFrames: 1_016, decodedSampleRate: 8_000)]),
                      "Repairable Save cannot activate a print against changed live-source bytes")
        }
        try check(try Data(contentsOf: f.live) == changed, "Repairable collection must preserve the changed source file")
        passed("collecting changed unrelated live bytes preserves unavailable historical proof without granting the new path an old content identity")
    }
    static func main() throws {
        let root = try folder(FileManager.default.temporaryDirectory, "SuperTracker-freeze-media-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        try portable(try folder(root, "Portable"))
        try exactPrintRelink(try folder(root, "Printed relink"))
        try originalRelink(try folder(root, "Source relink"))
        try corruptPrintRepair(try folder(root, "Corrupt repair"))
        try saveFailures(try folder(root, "Save rejection"))
        try missingMedia(try folder(root, "Missing media"))
        try inconsistentExtentRepair(try folder(root, "Endpoint repair"))
        try legacyProofRepair(try folder(root, "Legacy proof repair"))
        try opaqueProofRepair(try folder(root, "Opaque proof repair"))
        try knownProofPortable(try folder(root, "Known proof portable"))
        try durationOnlyRelink(try folder(root, "Duration source relink"))
        try changedDurationSourceCollect(try folder(root, "Changed duration source"))
        try changedOriginalSafety(try folder(root, "Changed source"))
        try upstreamSamePath(try folder(root, "Changed upstream"))
        print("Audio freeze media: \(groups) pure-filesystem regression groups passed; no audio decoding, device, plug-in or graph opened.")
    }
}

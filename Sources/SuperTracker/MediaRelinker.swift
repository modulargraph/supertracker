import Foundation
import CryptoKit

/// Read-only discovery and explicit, transactional replacement of project media references.
/// Filesystem methods are synchronous: the bridge should call them on a utility worker.
enum MediaRelinker {
    struct Relocation { var from: String; var to: String }
    struct Limits {
        var maxFiles = 250_000
        var maxCandidates = 20_000
        var candidatesPerReference = 16
        var maxHashes = 128
        var maxHashBytes: Int64 = 1_073_741_824
        var maxFileHashBytes: Int64 = 268_435_456
        var seconds: Double = 30
    }
    enum Failure: LocalizedError {
        case invalid(String)
        case cancelled
        var errorDescription: String? {
            switch self { case .invalid(let message): return message; case .cancelled: return "Media search was cancelled." }
        }
    }
    private struct Reference {
        var id: String
        var kind: String
        var label: String
        var path: String
        var resolved: String?
        var instrument: Int?
        var kit: Int?
        var sound: Int?
        var kitID: String?
        var soundID: String?
        var zone: Int?
        var clip: Int?
        var take: Int?
        var freezeTrack: Int?
        var freezeAsset: Int?
        var currentFreeze: Bool = false
        var patternPrint: Int?
        var loopPrint: Bool = false
        var requiredSHA256: String?
        var field: String
    }
    private struct FileInfo {
        var path: String
        var size: Int64
        var modified: Double
        var identifier: UInt64
        var available: Bool
        var status: String
    }
    private struct Expected {
        var sha256: String?
        var prefix: String?
        var size: Int64?
        var conflict = false
    }
    private struct Fingerprint {
        var path: String
        var size: Int64
        var modified: Double
        var sha256: String?
        var dictionary: [String: Any] {
            var value: [String: Any] = ["path": path, "size": size, "modified": modified]
            if let sha256 { value["sha256"] = sha256 }
            return value
        }
    }
    private static let audioExtensions: Set<String> = ["wav", "wave", "aif", "aiff", "flac", "mp3", "ogg", "m4a", "caf"]

    static func inspect(_ project: [String: Any], projectURL: URL? = nil, limits: Limits = Limits()) -> [String: Any] {
        let references = references(project, projectURL: projectURL)
        let expected = expectations(project, references: references, projectURL: projectURL)
        var memo: [String: FileInfo] = [:]
        var warnings: [String] = []
        var freezeValid = true
        do { _ = try AudioFreezeProject.validate(project: project) }
        catch { freezeValid = false; warnings.append(error.localizedDescription) }
        var hashes: [String: Fingerprint] = [:], hashBytes: Int64 = 0, hashCount = 0
        let began = Date(), deadline = limits.seconds.isFinite ? max(0.1, min(300, limits.seconds)) : 30
        let rows = references.map { reference -> [String: Any] in
            var info = reference.resolved.map { path -> FileInfo in
                if let old = memo[path] { return old }
                let value = fileInfo(path); memo[path] = value; return value
            }
            let fingerprint = expected[reference.id] ?? Expected()
            if fingerprint.conflict {
                warnings.append(reference.kind == "freeze"
                    ? "Conflicting saved fingerprints for \(reference.label); repair its original capture metadata or Unfreeze. Different content cannot replace a print."
                    : "Conflicting saved fingerprints for \(reference.label); explicit replacement is required.")
            }
            if reference.kind == "freeze" {
                if !freezeValid || fingerprint.conflict || reference.requiredSHA256 == nil {
                    info?.available = false; info?.status = "invalidFreeze"
                } else if let current = info, current.available {
                    if hashes[current.path] == nil, hashCount < max(0, limits.maxHashes),
                       current.size <= max(0, limits.maxFileHashBytes), current.size <= max(0, limits.maxHashBytes - hashBytes),
                       Date().timeIntervalSince(began) < deadline {
                        hashCount += 1; hashBytes += current.size
                        hashes[current.path] = try? hashFile(current.path, maxBytes: current.size, cancelled: { Date().timeIntervalSince(began) >= deadline })
                    }
                    if let hash = hashes[current.path]?.sha256 {
                        if hash != reference.requiredSHA256 { info?.available = false; info?.status = "contentMismatch" }
                    } else {
                        info?.available = false; info?.status = "unverified"
                        warnings.append("Frozen print content could not be verified within the inspection budget: \(reference.label).")
                    }
                }
            }
            return row(reference, info: info, expected: fingerprint)
        }
        let missing = rows.filter { $0["available"] as? Bool != true }
        return ["references": rows, "missing": missing, "referenceCount": rows.count,
                "missingCount": missing.count, "presentCount": rows.count - missing.count,
                "warnings": Array(Set(warnings)).sorted()]
    }

    /// Searches only chosen roots. Root mappings preserve suffix paths; names never prove identity.
    static func findCandidates(_ project: [String: Any], projectURL: URL? = nil, searchRoots: [URL],
                               relocations: [Relocation] = [], limits: Limits = Limits(),
                               cancelled: @escaping () -> Bool = { false }, progress: (([String: Any]) -> Void)? = nil) throws -> [String: Any] {
        if cancelled() { throw Failure.cancelled }
        guard searchRoots.count <= 64, relocations.count <= 64 else { throw Failure.invalid("Search up to 64 media folders or root mappings at a time.") }
        var report = inspect(project, projectURL: projectURL, limits: limits)
        let allReferences = references(project, projectURL: projectURL)
        let expected = expectations(project, references: allReferences, projectURL: projectURL)
        let missingIDs = Set((report["missing"] as? [[String: Any]] ?? []).compactMap { $0["id"] as? String })
        let missing = allReferences.filter { missingIDs.contains($0.id) }
        if missing.isEmpty { report["scannedFiles"] = 0; report["truncated"] = false; return report }
        let start = Date(), deadline = limits.seconds.isFinite ? max(0.1, min(300, limits.seconds)) : 30
        var lastProgress = Date.distantPast
        var warnings = report["warnings"] as? [String] ?? []
        var truncated = false, scanned = 0, hashed = 0
        var hashBytes: Int64 = 0
        var candidateInfo: [String: FileInfo] = [:]
        var matches: [String: Set<String>] = [:]
        var hints: [String: [String: Set<String>]] = [:]
        let candidateLimit = max(1, min(100_000, limits.maxCandidates))
        func check() throws -> Bool {
            if cancelled() { throw Failure.cancelled }
            if Date().timeIntervalSince(start) >= deadline { truncated = true; return false }
            return true
        }
        func notify(_ phase: String, root: String = "") {
            if Date().timeIntervalSince(lastProgress) >= 0.25 || phase == "complete" {
                progress?(["phase": phase, "root": root, "scannedFiles": scanned, "candidates": candidateInfo.count, "hashedFiles": hashed, "hashBytes": hashBytes])
                lastProgress = Date()
            }
        }
        func add(_ path: String, to reference: Reference, reason: String, knownInfo: FileInfo? = nil) {
            let info = knownInfo ?? fileInfo(path)
            guard info.available else { return }
            let path = info.path
            if candidateInfo[path] == nil && candidateInfo.count >= candidateLimit { truncated = true; return }
            candidateInfo[path] = info
            matches[reference.id, default: []].insert(path)
            hints[reference.id, default: [:]][path, default: []].insert(reason)
        }
        var names: [String: [Reference]] = [:], sizes: [Int64: [Reference]] = [:], prefixes: [String: [Reference]] = [:]
        for reference in missing {
            let name = (reference.path as NSString).lastPathComponent
            names[fold(name), default: []].append(reference)
            let originalName = unhashedName(name)
            if originalName != name { names[fold(originalName), default: []].append(reference) }
            if let value = expected[reference.id], value.sha256 != nil, let size = value.size, size > 0 { sizes[size, default: []].append(reference) }
            if let hash = expected[reference.id]?.sha256 ?? expected[reference.id]?.prefix { prefixes[String(hash.prefix(16)), default: []].append(reference) }
        }
        for reference in missing {
            guard try check() else { break }
            if let path = reference.resolved {
                for relocation in relocations {
                    let from = normalize(relocation.from), to = normalize(relocation.to)
                    if inside(path, root: from) {
                        let suffix = String(path.dropFirst(from.count)).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
                        add(URL(fileURLWithPath: to).appendingPathComponent(suffix).path, to: reference, reason: "root relocation")
                    }
                }
            }
            if let projectURL {
                let fileName = (reference.path as NSString).lastPathComponent
                add(projectURL.deletingPathExtension().appendingPathExtension("media").appendingPathComponent(fileName).path, to: reference, reason: "project media folder")
            }
            for root in searchRoots { add(root.appendingPathComponent((reference.path as NSString).lastPathComponent).path, to: reference, reason: "selected folder") }
        }
        let roots = Array(Set(searchRoots.map { normalize($0.path) })).sorted { $0.count == $1.count ? $0 < $1 : $0.count < $1.count }
        var visitedRoots: [String] = []
        let keys: Set<URLResourceKey> = [.isRegularFileKey, .isDirectoryKey, .isSymbolicLinkKey, .fileSizeKey, .contentModificationDateKey]
        outer: for root in roots {
            guard try check() else { break }
            if visitedRoots.contains(where: { inside(root, root: $0) }) { continue }
            visitedRoots.append(root)
            var directory: ObjCBool = false
            guard FileManager.default.fileExists(atPath: root, isDirectory: &directory), directory.boolValue else { warnings.append("Search folder is unavailable: \(root)"); continue }
            guard let enumerator = FileManager.default.enumerator(at: URL(fileURLWithPath: root), includingPropertiesForKeys: Array(keys), options: [.skipsHiddenFiles, .skipsPackageDescendants], errorHandler: { url, error in
                if warnings.count < 30 { warnings.append("\(url.lastPathComponent): \(error.localizedDescription)") }
                return !cancelled()
            }) else { continue }
            while let url = enumerator.nextObject() as? URL {
                guard try check() else { break outer }
                let name = url.lastPathComponent
                if name.hasPrefix(".") || name.hasPrefix("._") { enumerator.skipDescendants(); continue }
                guard let values = try? url.resourceValues(forKeys: keys) else { continue }
                if values.isSymbolicLink == true { enumerator.skipDescendants(); continue }
                if values.isDirectory == true { continue }
                guard values.isRegularFile == true else { continue }
                scanned += 1
                if scanned > max(1, min(2_000_000, limits.maxFiles)) { truncated = true; break outer }
                let originalName = unhashedName(name)
                let nameMatches = (names[fold(name)] ?? []) + (originalName == name ? [] : names[fold(originalName)] ?? [])
                let prefixMatches = originalName == name ? [] : prefixes[String(name.prefix(16)).lowercased()] ?? []
                let sizeMatches = audioExtensions.contains(url.pathExtension.lowercased()) ? (sizes[Int64(values.fileSize ?? 0)] ?? []) : []
                if !nameMatches.isEmpty || !sizeMatches.isEmpty || !prefixMatches.isEmpty {
                    let info = fileInfo(url.path)
                    for reference in nameMatches { add(url.path, to: reference, reason: "matching filename", knownInfo: info) }
                    for reference in sizeMatches { add(url.path, to: reference, reason: "matching saved size", knownInfo: info) }
                    for reference in prefixMatches { add(url.path, to: reference, reason: "collected filename hash prefix", knownInfo: info) }
                }
                if scanned % 256 == 0 { Thread.sleep(forTimeInterval: 0.004) }
                notify("searching", root: root)
            }
        }
        var fingerprints: [String: Fingerprint] = [:], hashErrors: [String: String] = [:]
        var verificationPriority: [String: Int] = [:]
        for reference in missing where expected[reference.id]?.sha256 != nil || expected[reference.id]?.prefix != nil {
            for path in matches[reference.id] ?? [] {
                let reasons = hints[reference.id]?[path] ?? []
                let priority = reasons.contains("root relocation") ? 400 : reasons.contains("collected filename hash prefix") ? 300 : reasons.contains("matching filename") ? 200 : 0
                verificationPriority[path] = max(verificationPriority[path] ?? 0, priority)
            }
        }
        let verificationPaths = verificationPriority.keys.sorted { verificationPriority[$0] == verificationPriority[$1] ? $0 < $1 : verificationPriority[$0]! > verificationPriority[$1]! }
        let hashBudget = max(0, limits.maxHashBytes)
        for path in verificationPaths {
            guard try check() else { break }
            guard let info = candidateInfo[path] else { continue }
            guard hashed < max(0, limits.maxHashes), info.size <= max(0, limits.maxFileHashBytes), info.size <= max(0, hashBudget - hashBytes) else {
                hashErrors[path] = "Content verification budget reached"; truncated = true; continue
            }
            hashed += 1; hashBytes += info.size
            do {
                fingerprints[path] = try hashFile(path, maxBytes: max(0, limits.maxFileHashBytes), cancelled: cancelled)
            } catch Failure.cancelled { throw Failure.cancelled }
            catch { hashErrors[path] = error.localizedDescription }
            notify("verifying")
        }
        var updatedMissing: [[String: Any]] = []
        for reference in missing {
            let expectation = expected[reference.id] ?? Expected()
            var entry = (report["missing"] as? [[String: Any]])?.first(where: { $0["id"] as? String == reference.id })
                ?? row(reference, info: reference.resolved.map(fileInfo), expected: expectation)
            var candidates: [[String: Any]] = []
            for path in (matches[reference.id] ?? []).sorted() {
                guard let info = candidateInfo[path] else { continue }
                var reasons = Array(hints[reference.id]?[path] ?? []).sorted()
                let fingerprint = fingerprints[path]
                let requiredHash = expectation.sha256 ?? expectation.prefix
                let verified = !expectation.conflict && requiredHash.map { fingerprint?.sha256?.hasPrefix($0) == true } == true
                let mismatch = requiredHash != nil && fingerprint != nil && !verified
                let match = verified ? (expectation.sha256 != nil ? "sha256" : "hash-prefix") : reasons.contains("root relocation") ? "relocation" : "name"
                if verified { reasons.append(expectation.sha256 != nil ? "Exact SHA-256 content match" : "Matches collected-media SHA-256 prefix") }
                if mismatch { reasons.append("Content differs from the saved fingerprint") }
                if let failure = hashErrors[path] { reasons.append(failure) }
                if expectation.conflict { reasons.append(reference.kind == "freeze" ? "Frozen print fingerprints conflict; restore original capture metadata or Unfreeze" : "Conflicting project fingerprints require deliberate replacement") }
                let suffix = commonSuffix(reference.resolved ?? reference.path, path)
                let score = (verified ? 1000 : 0) + (match == "relocation" ? 400 : 0) + suffix * 20 + (mismatch ? -1000 : 0)
                var candidate: [String: Any] = ["path": path, "size": info.size, "match": match, "verified": verified,
                    "contentMismatch": mismatch || expectation.conflict, "score": score, "reasons": reasons]
                if let sha256 = fingerprint?.sha256 { candidate["sha256"] = sha256 }
                candidates.append(candidate)
            }
            candidates.sort {
                let left = $0["score"] as? Int ?? 0, right = $1["score"] as? Int ?? 0
                return left == right ? ($0["path"] as? String ?? "") < ($1["path"] as? String ?? "") : left > right
            }
            let verifiedCount = candidates.filter { $0["verified"] as? Bool == true }.count
            entry["candidateCount"] = candidates.count
            entry["candidates"] = Array(candidates.prefix(max(1, min(100, limits.candidatesPerReference))))
            entry["resolution"] = candidates.isEmpty ? "unresolved" : verifiedCount == 1 ? "verified" : candidates.count > 1 ? "ambiguous" : "suggested"
            entry["ambiguous"] = verifiedCount > 1 || verifiedCount == 0 && candidates.count > 1
            entry["requiresSelection"] = true
            updatedMissing.append(entry)
        }
        if truncated { warnings.append("Search or verification reached its limit. Narrow the chosen folders or increase the limits; omitted candidates are not assumed absent.") }
        report["missing"] = updatedMissing
        report["scannedFiles"] = scanned; report["hashedFiles"] = hashed; report["hashBytes"] = hashBytes
        report["truncated"] = truncated; report["warnings"] = Array(Set(warnings)).sorted()
        notify("complete")
        return report
    }

    /// Records full hashes while originals are available; returns a new project and never saves it.
    static func fingerprint(_ project: [String: Any], projectURL: URL? = nil, limits: Limits = Limits(),
                            cancelled: () -> Bool = { false }) throws -> [String: Any] {
        if cancelled() { throw Failure.cancelled }
        _ = try AudioFreezeProject.validate(project: project)
        let refs = references(project, projectURL: projectURL)
        let expected = expectations(project, references: refs, projectURL: projectURL)
        var manifest = project["mediaManifest"] as? [[String: Any]] ?? []
        var count = 0, bytes: Int64 = 0, warnings: [String] = []
        for path in Set(refs.compactMap(\.resolved)).sorted() {
            if cancelled() { throw Failure.cancelled }
            let info = fileInfo(path)
            guard info.available else { continue }
            guard count < max(0, limits.maxHashes), info.size <= max(0, limits.maxFileHashBytes), info.size <= max(0, max(0, limits.maxHashBytes) - bytes) else {
                warnings.append("Fingerprint budget skipped: \(path)"); continue
            }
            let value = try hashFile(path, maxBytes: limits.maxFileHashBytes, cancelled: cancelled)
            for reference in refs where reference.resolved == path && reference.kind == "freeze" {
                guard expected[reference.id]?.conflict != true, let required = reference.requiredSHA256, value.sha256 == required else {
                    throw Failure.invalid("Frozen print \(reference.label) changed. Fingerprinting cannot replace its captured SHA-256; relink the original print or Unfreeze.")
                }
            }
            manifest.removeAll { resolve($0["path"] as? String ?? "", projectURL: projectURL) == path }
            var entry = value.dictionary
            entry["path"] = path // Keep the reference spelling, including an intentional filesystem alias.
            manifest.append(entry); count += 1; bytes += info.size
        }
        var updated = project
        updated["mediaManifest"] = manifest.sorted { ($0["path"] as? String ?? "") < ($1["path"] as? String ?? "") }
        return ["project": updated, "fingerprinted": count, "hashBytes": bytes, "warnings": warnings]
    }

    /// Each selection is {id,from,to,allowDifferentContent?}. All validate before any project edit.
    static func apply(_ project: [String: Any], selections: [[String: Any]], projectURL: URL? = nil,
                      maxHashBytes: Int64 = 4_294_967_296, cancelled: () -> Bool = { false }) throws -> [String: Any] {
        if cancelled() { throw Failure.cancelled }
        let freezeStatuses = try AudioFreezeProject.validate(project: project)
        var capturedMediaPaths = Set(freezeStatuses.flatMap { $0.record.mediaHashes.keys })
        for status in freezeStatuses {
            if case .supported(let proof) = try AudioFreezeDurationProof.assess(status.record.durationProof) {
                capturedMediaPaths.formUnion(proof.sources.flatMap(\.paths))
            }
        }
        guard selections.count <= 16_384 else { throw Failure.invalid("Too many media replacements in one operation.") }
        if selections.isEmpty { return ["project": project, "relinked": [], "changed": 0, "remaining": inspect(project, projectURL: projectURL)["missing"] ?? [], "warnings": []] }
        let refs = references(project, projectURL: projectURL)
        let byID = Dictionary(uniqueKeysWithValues: refs.map { ($0.id, $0) })
        let expected = expectations(project, references: refs, projectURL: projectURL)
        var used = Set<String>(), memo: [String: Fingerprint] = [:], bytes: Int64 = 0
        var linkedTargets: [String: String] = [:]
        var prepared: [(Reference, Fingerprint, Bool)] = []
        var warnings: [String] = []
        for selection in selections {
            if cancelled() { throw Failure.cancelled }
            guard let id = selection["id"] as? String, let reference = byID[id], used.insert(id).inserted,
                  let from = selection["from"] as? String, from == reference.path,
                  let to = selection["to"] as? String, to.hasPrefix("/") || to.hasPrefix("~") else {
                throw Failure.invalid("A relink selection is stale or invalid. Refresh missing media and select a local replacement for its current reference.")
            }
            let target = fileInfo(normalize(to))
            guard target.available else { throw Failure.invalid("Replacement is not a readable, nonempty file: \(to)") }
            var linkedPaths: [String] = [], linkedOwner: String?
            if let clip = reference.clip, let items = project["audioClips"] as? [[String: Any]], items.indices.contains(clip) {
                let active = activeTakeIndex(items[clip])
                if reference.take == nil || reference.take == active {
                    linkedOwner = "/audioClips/\(clip)"
                    linkedPaths = [items[clip]["path"], items[clip]["samplePath"]].compactMap { $0 as? String }
                    if let active, let set = items[clip]["takeSet"] as? [String: Any], let takes = set["takes"] as? [[String: Any]],
                       let path = takes[active]["path"] as? String { linkedPaths.append(path) }
                }
            } else if let kit = reference.kit, let sound = reference.sound, let zone = reference.zone,
                      let kits = project["kits"] as? [[String: Any]], kits.indices.contains(kit),
                      let sounds = kits[kit]["sounds"] as? [[String: Any]], sounds.indices.contains(sound),
                      let instrument = sounds[sound]["instrument"] as? [String: Any],
                      let zones = instrument["zones"] as? [[String: Any]], zones.indices.contains(zone) {
                linkedOwner = "/kits/\(kit)/sounds/\(sound)/instrument/zones/\(zone)"
                linkedPaths = [zones[zone]["path"], zones[zone]["samplePath"]].compactMap { $0 as? String }
            } else if let instrument = reference.instrument, let zone = reference.zone,
                      let items = project["instruments"] as? [[String: Any]], items.indices.contains(instrument),
                      let zones = items[instrument]["zones"] as? [[String: Any]], zones.indices.contains(zone) {
                linkedOwner = "/instruments/\(instrument)/zones/\(zone)"
                linkedPaths = [zones[zone]["path"], zones[zone]["samplePath"]].compactMap { $0 as? String }
            }
            if let linkedOwner {
                guard linkedPaths.filter({ !$0.isEmpty }).allSatisfy({ resolve($0, projectURL: projectURL) == reference.resolved }) else {
                    throw Failure.invalid("The sample or audio clip aliases refer to different files. Repair the source selection before relinking.")
                }
                if let previous = linkedTargets[linkedOwner], previous != target.path {
                    throw Failure.invalid("Linked sample or audio clip references must use the same replacement file.")
                }
                linkedTargets[linkedOwner] = target.path
            }
            let expectation = expected[id] ?? Expected()
            let frozenPrint = reference.kind == "freeze"
            let deliberate = !frozenPrint && (selection["allowDifferentContent"] as? Bool ?? false)
            if let old = reference.resolved, fileInfo(old).available, fileInfo(old).path != target.path {
                if frozenPrint, let required = reference.requiredSHA256 {
                    let oldInfo = fileInfo(old)
                    guard oldInfo.size <= max(0, maxHashBytes - bytes) else { throw Failure.invalid("The existing frozen print exceeds the verification budget.") }
                    let original = try hashFile(old, maxBytes: maxHashBytes - bytes, cancelled: cancelled); bytes += oldInfo.size
                    guard original.sha256 != required else { throw Failure.invalid("\(reference.label) is available again. Refresh before replacing a reconnected original.") }
                } else { throw Failure.invalid("\(reference.label) is available again. Refresh the report before replacing a reconnected original.") }
            }
            var value = memo[target.path]
            if value == nil {
                if target.size <= max(0, maxHashBytes - bytes) {
                    value = try hashFile(target.path, maxBytes: maxHashBytes - bytes, cancelled: cancelled)
                    bytes += target.size
                } else {
                    if (expectation.sha256 != nil || expectation.prefix != nil) && !deliberate {
                        throw Failure.invalid(frozenPrint ? "The frozen print exceeds the verification budget. Increase the budget to verify its exact SHA-256 or Unfreeze." : "The replacement exceeds the verification budget. Increase the budget or explicitly choose a different-content replacement.")
                    }
                    value = Fingerprint(path: target.path, size: target.size, modified: target.modified, sha256: nil)
                    warnings.append("Replacement was selected explicitly but could not be fingerprinted within the budget: \(target.path)")
                }
                memo[target.path] = value
            }
            let required = expectation.sha256 ?? expectation.prefix
            let mismatch = expectation.conflict || required.map { value?.sha256?.hasPrefix($0) != true } == true
            if frozenPrint {
                guard !expectation.conflict, let exact = reference.requiredSHA256, value?.sha256 == exact else {
                    throw Failure.invalid("Frozen print \(reference.label) requires its exact original SHA-256. Different-content replacement is unavailable; relink the verified print or Unfreeze.")
                }
            }
            if mismatch && !deliberate { throw Failure.invalid("The selected file for \(reference.label) does not match its saved content fingerprint. Select the original file, or explicitly allow different content to replace the sound.") }
            if value?.sha256 == nil, capturedMediaPaths.contains(reference.path) {
                throw Failure.invalid("A captured source requires full content verification before relinking. Increase the verification budget or Unfreeze.")
            }
            if mismatch { warnings.append("Different content explicitly selected for \(reference.label).") }
            prepared.append((reference, value!, mismatch))
        }
        var updated = project
        var changes: [[String: Any]] = []
        for (reference, value, mismatch) in prepared {
            setPath(&updated, reference: reference, path: value.path)
            changes.append(["id": reference.id, "from": reference.path, "to": value.path, "differentContent": mismatch])
        }
        let relocations = prepared.compactMap { reference, value, _ -> AudioFreezeProject.MediaRelocation? in
            guard let hash = value.sha256 else { return nil }
            return AudioFreezeProject.MediaRelocation(from: reference.path, to: value.path, sha256: hash)
        }
        updated = try AudioFreezeProject.preservingRelocatedMedia(from: project, to: updated, relocations: relocations)
        var verifiedAliases: [String: String] = [:], ambiguousAliases = Set<String>()
        for (reference, value, mismatch) in prepared where !mismatch {
            // A hash-prefix/name match or deliberate replacement is insufficient
            // to preserve a current Track Freeze source fingerprint.
            guard let expectedHash = expected[reference.id]?.sha256, value.sha256 == expectedHash else { continue }
            if let previous = verifiedAliases[reference.path], previous != value.path { ambiguousAliases.insert(reference.path) }
            verifiedAliases[reference.path] = value.path
        }
        for path in ambiguousAliases { verifiedAliases.removeValue(forKey: path) }
        updated = try AudioFreezeTrackFingerprint.preservingRelocatedSources(from: project, to: updated, verifiedAliases: verifiedAliases)
        let activePaths = Set(references(updated, projectURL: projectURL).compactMap(\.resolved))
        var manifest = (project["mediaManifest"] as? [[String: Any]] ?? []).filter { entry in
            resolve(entry["path"] as? String ?? "", projectURL: projectURL).map { activePaths.contains($0) } ?? false
        }
        for value in memo.values where activePaths.contains(value.path) {
            manifest.removeAll { resolve($0["path"] as? String ?? "", projectURL: projectURL) == value.path }
            manifest.append(value.dictionary)
        }
        updated["mediaManifest"] = manifest.sorted { ($0["path"] as? String ?? "") < ($1["path"] as? String ?? "") }
        if !changes.isEmpty {
            var history = project["mediaRelinkHistory"] as? [[String: Any]] ?? []
            history.append(["date": ISO8601DateFormatter().string(from: Date()), "changes": changes])
            updated["mediaRelinkHistory"] = Array(history.suffix(64))
        }
        _ = try AudioFreezeProject.validate(project: updated)
        return ["project": updated, "relinked": changes, "changed": changes.count,
                "remaining": inspect(updated, projectURL: projectURL)["missing"] ?? [], "warnings": warnings]
    }

    private static func references(_ project: [String: Any], projectURL: URL?) -> [Reference] {
        var result: [Reference] = []
        for (track, item) in (project["tracks"] as? [[String: Any]] ?? []).enumerated() {
            let name = item["name"] as? String ?? "Track \(track + 1)"
            if let frozen = item["frozen"] as? [String: Any] {
                func append(_ entry: [String: Any], id: String, label: String, pattern: Int? = nil, loop: Bool = false) {
                    guard let path = entry["path"] as? String, !path.isEmpty else { return }
                    result.append(Reference(id: id, kind: "freeze", label: "\(name) · Frozen \(label)", path: path,
                        resolved: resolve(path, projectURL: projectURL), freezeTrack: track, currentFreeze: true,
                        patternPrint: pattern, loopPrint: loop, requiredSHA256: validHash(entry["sha256"] as? String, length: 64), field: "path"))
                }
                append(frozen, id: "/tracks/\(track)/frozen/path", label: "song")
                for (index, print) in (frozen["patternPrints"] as? [[String: Any]] ?? []).prefix(256).enumerated() {
                    let prefix = "/tracks/\(track)/frozen/patternPrints/\(index)"
                    append(print, id: prefix + "/path", label: "pattern \(index + 1) head", pattern: index)
                    if let loop = print["loop"] as? [String: Any] { append(loop, id: prefix + "/loop/path", label: "pattern \(index + 1) loop", pattern: index, loop: true) }
                }
            }
            guard let record = item["freeze"] as? [String: Any] else { continue }
            for (asset, entry) in (record["assets"] as? [[String: Any]] ?? []).prefix(2).enumerated() {
                guard let path = entry["path"] as? String, !path.isEmpty else { continue }
                let stage = entry["stage"] as? String ?? "capture"
                result.append(Reference(id: "/tracks/\(track)/freeze/assets/\(asset)/path", kind: "freeze", label: "\(name) · Frozen \(stage)", path: path,
                    resolved: resolve(path, projectURL: projectURL), freezeTrack: track, freezeAsset: asset,
                    requiredSHA256: validHash(entry["sha256"] as? String, length: 64), field: "path"))
            }
        }
        for (instrument, item) in (project["instruments"] as? [[String: Any]] ?? []).enumerated() {
            let label = item["name"] as? String ?? "Instrument \(instrument + 1)"
            if let path = item["samplePath"] as? String, !path.isEmpty {
                result.append(Reference(id: "/instruments/\(instrument)/samplePath", kind: "sample", label: label, path: path,
                                        resolved: resolve(path, projectURL: projectURL), instrument: instrument, field: "samplePath"))
            }
            for (zone, item) in (item["zones"] as? [[String: Any]] ?? []).prefix(128).enumerated() {
                for field in ["samplePath", "path"] {
                    if let path = item[field] as? String, !path.isEmpty {
                        result.append(Reference(id: "/instruments/\(instrument)/zones/\(zone)/\(field)", kind: "zone", label: "\(label) · Zone \(zone + 1)", path: path,
                            resolved: resolve(path, projectURL: projectURL), instrument: instrument, zone: zone, field: field))
                    }
                }
            }
        }
        for (kit, item) in (project["kits"] as? [[String: Any]] ?? []).prefix(4).enumerated() {
            let kitID = item["id"] as? String ?? "", kitName = item["name"] as? String ?? "Kit \(kit + 1)"
            for (sound, slot) in (item["sounds"] as? [[String: Any]] ?? []).prefix(16).enumerated() {
                guard let instrument = slot["instrument"] as? [String: Any] else { continue }
                let soundID = slot["id"] as? String ?? ""
                let soundName = slot["name"] as? String ?? instrument["name"] as? String ?? "Sound \(sound + 1)"
                let label = "\(kitName) · \(soundName)"
                // IDs are opaque to the UI. Keep a location plus identity discriminator so
                // replacing/reordering Sounds cannot apply a stale selection to a new owner.
                let identity = SHA256.hash(data: Data((kitID + "\u{0}" + soundID).utf8)).prefix(8).map { String(format: "%02x", $0) }.joined()
                let prefix = "/kits/\(kit)/sounds/\(sound)/instrument"
                if let path = instrument["samplePath"] as? String, !path.isEmpty {
                    result.append(Reference(id: prefix + "/samplePath#" + identity, kind: "sample", label: label, path: path,
                        resolved: resolve(path, projectURL: projectURL), kit: kit, sound: sound, kitID: kitID, soundID: soundID, field: "samplePath"))
                }
                for (zone, entry) in (instrument["zones"] as? [[String: Any]] ?? []).prefix(128).enumerated() {
                    for field in ["samplePath", "path"] {
                        guard let path = entry[field] as? String, !path.isEmpty else { continue }
                        result.append(Reference(id: prefix + "/zones/\(zone)/\(field)#" + identity, kind: "zone", label: "\(label) · Zone \(zone + 1)", path: path,
                            resolved: resolve(path, projectURL: projectURL), kit: kit, sound: sound, kitID: kitID, soundID: soundID, zone: zone, field: field))
                    }
                }
            }
        }
        for (clip, item) in (project["audioClips"] as? [[String: Any]] ?? []).prefix(256).enumerated() {
            let label = item["name"] as? String ?? "Audio clip \(clip + 1)"
            for field in ["path", "samplePath"] {
                if let path = item[field] as? String, !path.isEmpty {
                    result.append(Reference(id: "/audioClips/\(clip)/\(field)", kind: "clip", label: label, path: path,
                        resolved: resolve(path, projectURL: projectURL), clip: clip, field: field))
                }
            }
            if let set = item["takeSet"] as? [String: Any], set["version"] as? Int == 1 {
                for (take, entry) in (set["takes"] as? [[String: Any]] ?? []).prefix(32).enumerated() {
                    if let path = entry["path"] as? String, !path.isEmpty {
                        let name = entry["name"] as? String ?? "Take \(take + 1)"
                        result.append(Reference(id: "/audioClips/\(clip)/takeSet/takes/\(take)/path", kind: "take", label: "\(label) · \(name)", path: path,
                            resolved: resolve(path, projectURL: projectURL), clip: clip, take: take, field: "path"))
                    }
                }
            }
        }
        // Built-in sample references (BuiltinSamples) are app content, never missing media.
        return result.filter { !BuiltinSamples.isBuiltin($0.path) }
    }
    private static func expectations(_ project: [String: Any], references: [Reference], projectURL: URL?) -> [String: Expected] {
        var manifest: [String: Expected] = [:]
        for item in project["mediaManifest"] as? [[String: Any]] ?? [] {
            guard let path = item["path"] as? String, let resolved = resolve(path, projectURL: projectURL) else { continue }
            let hash = validHash(item["sha256"] as? String, length: 64)
            var value = manifest[resolved] ?? Expected()
            if let old = value.sha256, let hash, old != hash { value.conflict = true }
            if let hash { value.sha256 = hash }
            if let size = item["size"] as? NSNumber, size.int64Value >= 0 { value.size = size.int64Value }
            manifest[resolved] = value
        }
        // Captured source evidence is immutable even when a generic fingerprint
        // operation observes changed bytes at the same path.
        for track in project["tracks"] as? [[String: Any]] ?? [] {
            guard let freeze = track["freeze"] as? [String: Any] else { continue }
            for item in freeze["media"] as? [[String: Any]] ?? [] {
                guard let path = item["path"] as? String, let resolved = resolve(path, projectURL: projectURL),
                      let hash = validHash(item["sha256"] as? String, length: 64) else { continue }
                var value = manifest[resolved] ?? Expected()
                if let old = value.sha256, old != hash { value.conflict = true }
                value.sha256 = hash; manifest[resolved] = value
            }
            // Duration evidence also covers unrelated live clips. Only a
            // supported proof supplies immutable hash expectations; unknown
            // metadata is preserved for repair without interpreting its fields.
            if let assessment = try? AudioFreezeDurationProof.assess(freeze["durationProof"]),
               case .supported(let proof) = assessment {
                for source in proof.sources {
                    for path in source.paths {
                        guard let resolved = resolve(path, projectURL: projectURL) else { continue }
                        var value = manifest[resolved] ?? Expected()
                        if let old = value.sha256, old != source.decoded.sha256 { value.conflict = true }
                        value.sha256 = source.decoded.sha256; manifest[resolved] = value
                    }
                }
            }
        }
        var result: [String: Expected] = [:]
        for reference in references {
            var value = reference.resolved.flatMap { manifest[$0] } ?? Expected()
            if reference.kind == "freeze" {
                if let required = reference.requiredSHA256 {
                    if let saved = value.sha256, saved != required { value.conflict = true }
                    value.sha256 = required; value.prefix = nil
                } else { value.conflict = true }
            }
            if value.sha256 == nil {
                let url = URL(fileURLWithPath: reference.path)
                let directory = url.deletingLastPathComponent(), name = url.lastPathComponent
                let kitSamples = directory.lastPathComponent.lowercased() == "samples"
                    && directory.deletingLastPathComponent().pathExtension.lowercased() == "stkit"
                // Only our known collection layouts establish a saved fingerprint.
                // A hash-looking filename in an ordinary library remains unverified.
                let length = kitSamples ? 24 : directory.pathExtension.lowercased() == "media" ? 16 : 0
                if length > 0, name.count > length + 1, name.dropFirst(length).first == "-" {
                    value.prefix = validHash(String(name.prefix(length)), length: length)
                }
            }
            result[reference.id] = value
        }
        return result
    }
    private static func row(_ reference: Reference, info: FileInfo?, expected: Expected) -> [String: Any] {
        var value: [String: Any] = ["id": reference.id, "kind": reference.kind, "label": reference.label, "path": reference.path,
            "available": info?.available ?? false, "status": info?.status ?? "unresolvedRelativePath", "fingerprintConflict": expected.conflict]
        if let resolved = reference.resolved { value["resolvedPath"] = resolved }
        if let info, info.available { value["size"] = info.size }
        if let sha256 = expected.sha256 { value["expectedSHA256"] = sha256 }
        if let prefix = expected.prefix { value["expectedHashPrefix"] = prefix }
        if let size = expected.size { value["expectedSize"] = size }
        if let instrument = reference.instrument { value["instrument"] = instrument }
        if let kit = reference.kit { value["kit"] = kit }
        if let sound = reference.sound { value["sound"] = sound }
        if let kitID = reference.kitID, let soundID = reference.soundID {
            value["kitID"] = kitID; value["soundID"] = soundID; value["owner"] = "sound:\(kitID):\(soundID)"
        }
        if let zone = reference.zone { value["zone"] = zone }
        if let clip = reference.clip { value["clip"] = clip }
        if let take = reference.take { value["take"] = take }
        if let track = reference.freezeTrack { value["track"] = track; value["requiresExactContent"] = true }
        if let asset = reference.freezeAsset { value["freezeAsset"] = asset }
        let components = (reference.resolved ?? reference.path).split(separator: "/")
        if components.count > 1 && components[0] == "Volumes" {
            value["volumeName"] = String(components[1]); value["volumeAvailable"] = FileManager.default.fileExists(atPath: "/Volumes/\(components[1])")
        }
        return value
    }
    private static func setPath(_ project: inout [String: Any], reference: Reference, path: String) {
        if reference.currentFreeze, let track = reference.freezeTrack,
           var tracks = project["tracks"] as? [[String: Any]], tracks.indices.contains(track),
           var frozen = tracks[track]["frozen"] as? [String: Any] {
            if let index = reference.patternPrint, var prints = frozen["patternPrints"] as? [[String: Any]], prints.indices.contains(index) {
                if reference.loopPrint, var loop = prints[index]["loop"] as? [String: Any] { loop["path"] = path; prints[index]["loop"] = loop }
                else { prints[index]["path"] = path }
                frozen["patternPrints"] = prints
            } else { frozen["path"] = path }
            tracks[track]["frozen"] = frozen; project["tracks"] = tracks
        } else if let track = reference.freezeTrack, let asset = reference.freezeAsset,
           var tracks = project["tracks"] as? [[String: Any]], tracks.indices.contains(track),
           var record = tracks[track]["freeze"] as? [String: Any], var assets = record["assets"] as? [[String: Any]], assets.indices.contains(asset) {
            assets[asset]["path"] = path; record["assets"] = assets; tracks[track]["freeze"] = record; project["tracks"] = tracks
        } else if let kit = reference.kit, let sound = reference.sound,
                  var kits = project["kits"] as? [[String: Any]], kits.indices.contains(kit),
                  var sounds = kits[kit]["sounds"] as? [[String: Any]], sounds.indices.contains(sound),
                  var instrument = sounds[sound]["instrument"] as? [String: Any] {
            if let zone = reference.zone, var zones = instrument["zones"] as? [[String: Any]], zones.indices.contains(zone) {
                zones[zone][reference.field] = path
                for field in ["path", "samplePath"] where zones[zone][field] as? String != nil { zones[zone][field] = path }
                instrument["zones"] = zones
            } else { instrument[reference.field] = path }
            sounds[sound]["instrument"] = instrument; kits[kit]["sounds"] = sounds; project["kits"] = kits
        } else if let index = reference.instrument, var instruments = project["instruments"] as? [[String: Any]], instruments.indices.contains(index) {
            if let zone = reference.zone, var zones = instruments[index]["zones"] as? [[String: Any]], zones.indices.contains(zone) {
                zones[zone][reference.field] = path
                for field in ["path", "samplePath"] where zones[zone][field] as? String != nil { zones[zone][field] = path }
                instruments[index]["zones"] = zones
            } else { instruments[index][reference.field] = path }
            project["instruments"] = instruments
        } else if let clip = reference.clip, var clips = project["audioClips"] as? [[String: Any]], clips.indices.contains(clip) {
            let active = activeTakeIndex(clips[clip])
            if var set = clips[clip]["takeSet"] as? [String: Any], set["version"] as? Int == 1,
               var takes = set["takes"] as? [[String: Any]], reference.take != nil || active != nil {
                if let take = reference.take, takes.indices.contains(take) {
                    takes[take][reference.field] = path
                } else if let active { takes[active]["path"] = path }
                if reference.take == nil || reference.take == active {
                    clips[clip]["path"] = path
                    if clips[clip]["samplePath"] != nil { clips[clip]["samplePath"] = path }
                }
                set["takes"] = takes; clips[clip]["takeSet"] = set
            } else {
                clips[clip][reference.field] = path
                if reference.take == nil {
                    for field in ["path", "samplePath"] where clips[clip][field] as? String != nil { clips[clip][field] = path }
                }
            }
            project["audioClips"] = clips
        }
    }
    private static func activeTakeIndex(_ clip: [String: Any]) -> Int? {
        guard let set = clip["takeSet"] as? [String: Any], set["version"] as? Int == 1,
              let activeID = set["activeID"] as? String, let takes = set["takes"] as? [[String: Any]], takes.count <= 32 else { return nil }
        let matches = takes.indices.filter { takes[$0]["id"] as? String == activeID }
        return matches.count == 1 ? matches[0] : nil
    }
    private static func fileInfo(_ path: String) -> FileInfo {
        let url = URL(fileURLWithPath: normalize(path)).resolvingSymlinksInPath().standardizedFileURL
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: url.path) else {
            return FileInfo(path: url.path, size: 0, modified: 0, identifier: 0, available: false, status: "missing")
        }
        let regular = attributes[.type] as? FileAttributeType == .typeRegular
        let size = (attributes[.size] as? NSNumber)?.int64Value ?? 0
        let readable = FileManager.default.isReadableFile(atPath: url.path)
        return FileInfo(path: url.path, size: size, modified: (attributes[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0,
                        identifier: (attributes[.systemFileNumber] as? NSNumber)?.uint64Value ?? 0,
                        available: regular && readable && size > 0, status: !regular ? "notFile" : !readable ? "unreadable" : size <= 0 ? "empty" : "available")
    }
    private static func hashFile(_ path: String, maxBytes: Int64, cancelled: () -> Bool) throws -> Fingerprint {
        let before = fileInfo(path)
        guard before.available, before.size <= maxBytes else { throw Failure.invalid("Cannot fingerprint this missing, unreadable or oversized file: \(path)") }
        let handle = try FileHandle(forReadingFrom: URL(fileURLWithPath: before.path))
        defer { try? handle.close() }
        var hash = SHA256(), bytes: Int64 = 0
        while true {
            if cancelled() { throw Failure.cancelled }
            guard let data = try handle.read(upToCount: 1024 * 1024), !data.isEmpty else { break }
            bytes += Int64(data.count)
            guard bytes <= maxBytes else { throw Failure.invalid("Media grew beyond the verification budget while reading.") }
            hash.update(data: data)
        }
        let after = fileInfo(before.path)
        guard after.available, before.size == after.size, bytes == before.size,
              before.modified == after.modified, before.identifier == after.identifier else { throw Failure.invalid("Media changed during content verification; search again.") }
        return Fingerprint(path: after.path, size: after.size, modified: after.modified,
                           sha256: hash.finalize().map { String(format: "%02x", $0) }.joined())
    }
    private static func resolve(_ path: String, projectURL: URL?) -> String? {
        guard !path.isEmpty else { return nil }
        if path.hasPrefix("/") || path.hasPrefix("~") { return normalize(path) }
        guard let projectURL else { return nil }
        return projectURL.deletingLastPathComponent().appendingPathComponent(path).standardizedFileURL.path
    }
    private static func normalize(_ path: String) -> String { URL(fileURLWithPath: (path as NSString).expandingTildeInPath).standardizedFileURL.path }
    private static func inside(_ path: String, root: String) -> Bool { path == root || path.hasPrefix(root == "/" ? "/" : root + "/") }
    private static func fold(_ value: String) -> String { value.folding(options: [.caseInsensitive], locale: Locale(identifier: "en_US_POSIX")) }
    private static func validHash(_ value: String?, length: Int) -> String? {
        guard let value, value.count == length, value.allSatisfy({ $0.isHexDigit && $0.isASCII }) else { return nil }
        return value.lowercased()
    }
    private static func unhashedName(_ value: String) -> String {
        for length in [24, 16] where value.count > length + 1 {
            if value.dropFirst(length).first == "-", validHash(String(value.prefix(length)), length: length) != nil {
                return String(value.dropFirst(length + 1))
            }
        }
        return value
    }
    private static func commonSuffix(_ left: String, _ right: String) -> Int {
        var count = 0
        for (a, b) in zip(left.split(separator: "/").reversed(), right.split(separator: "/").reversed()) {
            if fold(String(a)) != fold(String(b)) { break }; count += 1
        }
        return count
    }
}

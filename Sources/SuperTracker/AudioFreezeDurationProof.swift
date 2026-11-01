import Foundation
import CoreFoundation
import CryptoKit

/// Control-thread historical duration evidence, independent of rendering and
/// file I/O. A caller must bind each DecodedSource to the actual immutable PCM
/// and full file hash it verified. This module cannot authenticate that claim,
/// observe natural end, validate prints, or make a frozen project playable.
enum AudioFreezeDurationProof {
    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { "Freeze duration: " + message }
    }
    struct DecodedSource: Equatable {
        let sha256: String
        let decodedFrames: Int64
        let decodedSampleRate: Double
    }
    struct Source: Equatable {
        let decoded: DecodedSource
        let paths: [String]
        var dictionary: [String: Any] {
            ["sha256": decoded.sha256, "decodedFrames": decoded.decodedFrames,
             "decodedSampleRate": decoded.decodedSampleRate, "paths": paths.sorted()]
        }
    }
    struct Proof: Equatable {
        let temporalSHA256: String
        let sources: [Source]
        var dictionary: [String: Any] {
            ["version": 1, "temporalSHA256": temporalSHA256,
             "sources": sources.sorted { $0.decoded.sha256 < $1.decoded.sha256 }.map(\.dictionary)]
        }
    }
    enum Assessment {
        case absent
        case supported(Proof)
        /// Repairable JSON remains with its owning record, unchanged. This is
        /// deliberately not a save/open error or permission to ignore evidence.
        case unavailable(String)
    }
    struct Relocation {
        let from: String
        let to: String
        let sha256: String
    }

    static let maximumStorageBytes = 2 * 1_024 * 1_024
    static let maximumSources = 256
    static let maximumAliases = 1_024
    static let maximumPathBytes = 16_384
    static let maximumAggregatePathBytes = 1_024 * 1_024

    /// Only unrepresentable/over-budget storage throws. Unknown versions and
    /// malformed supported versions remain opaque and unavailable for playback.
    /// The caller retains `raw`, not a lossy reconstruction of unavailable JSON.
    static func assess(_ raw: Any?) throws -> Assessment {
        guard let raw else { return .absent }
        try validateStorage(raw)
        do { return .supported(try parse(raw)) }
        catch { return .unavailable(error.localizedDescription) }
    }

    /// Build only after the caller observed the exact canonical natural endpoint
    /// with no extent override. musicalFrames is that supplied integer; it is
    /// never derived from source lengths or converted through seconds here.
    static func make(project: [String: Any], sampleRate: Double, musicalFrames: Int64,
                     resolvedSources: [String: DecodedSource]) throws -> Proof {
        guard resolvedSources.count <= maximumAliases else { throw failure("Too many resolved source paths.") }
        var used: [String: DecodedSource] = [:]
        let bytes = try encode(project: project, sampleRate: sampleRate, musicalFrames: musicalFrames) { path, _ in
            guard let source = resolvedSources[path] else { throw failure("No verified decoded source for \(path).") }
            try validate(source)
            used[path] = source
            return source
        }
        var sources: [String: Source] = [:]
        for (path, decoded) in used {
            if let previous = sources[decoded.sha256] {
                guard previous.decoded == decoded else { throw failure("One source hash has contradictory decoded extents.") }
                sources[decoded.sha256] = Source(decoded: decoded, paths: previous.paths + [path])
            } else { sources[decoded.sha256] = Source(decoded: decoded, paths: [path]) }
        }
        let proof = Proof(temporalSHA256: digest(bytes), sources: sources.values.sorted { $0.decoded.sha256 < $1.decoded.sha256 })
        try validateStorage(proof.dictionary)
        return try parse(proof.dictionary)
    }

    /// Rechecks the entire canonical temporal layout. Dormant owners may use
    /// captured aliases without their originals. Every remaining live unmuted
    /// clip requires freshly admitted immutable PCM evidence, including clips
    /// unrelated to the track being frozen. No duration-only or hash-prefix match.
    static func verify(proof: Proof, project: [String: Any], sampleRate: Double, musicalFrames: Int64,
                       dormantOwners: Set<String>, liveSources: [String: DecodedSource]) throws {
        try validateStorage(proof.dictionary)
        let proof = try parse(proof.dictionary)
        guard liveSources.count <= maximumAliases else { throw failure("Too many live source paths.") }
        let owners = try trackOwners(project)
        guard dormantOwners.isSubset(of: Set(owners)) else { throw failure("A dormant owner is not a canonical track.") }
        let byPath = Dictionary(uniqueKeysWithValues: proof.sources.flatMap { source in source.paths.map { ($0, source.decoded) } })
        let bytes = try encode(project: project, sampleRate: sampleRate, musicalFrames: musicalFrames) { path, owner in
            guard let historical = byPath[path] else { throw failure("The selected source is absent from captured duration evidence: \(path).") }
            if !dormantOwners.contains(owner) {
                guard let live = liveSources[path] else { throw failure("A live clip needs verified decoded media: \(path).") }
                try validate(live)
                guard live == historical else { throw failure("A live clip's bytes or decoded extent changed: \(path). Unfreeze or recapture.") }
            }
            return historical
        }
        guard digest(bytes) == proof.temporalSHA256 else {
            throw failure("Canonical timing or clip layout changed. Unfreeze or recapture all prints sharing this proof.")
        }
    }

    /// Pure alias operation. The caller proves identical full-file content for
    /// an external relink, or resolves the same stored relative reference while
    /// reading a package. Unknown/malformed proof is returned verbatim. Replacing
    /// is for portable rebasing; appending preserves historical relink aliases.
    static func relocating(_ raw: Any?, relocations: [Relocation], replacePaths: Bool = false) throws -> Any? {
        guard case let .supported(proof) = try assess(raw) else { return raw }
        guard relocations.count <= maximumAliases else { throw failure("Too many source relocations.") }
        var changes: [String: [Relocation]] = [:]
        for move in relocations {
            try validatePath(move.from); try validatePath(move.to); try validateHash(move.sha256)
            if let old = changes[move.from]?.first,
               old.sha256 != move.sha256 || (replacePaths && old.to != move.to) {
                throw failure("Conflicting relocations for one source path.")
            }
            changes[move.from, default: []].append(move)
        }
        let sources = try proof.sources.map { source -> Source in
            var paths = Set<String>()
            for path in source.paths {
                guard let moves = changes[path] else { paths.insert(path); continue }
                guard moves.allSatisfy({ $0.sha256 == source.decoded.sha256 }) else { throw failure("Relocation does not verify the captured full source hash.") }
                if !replacePaths { paths.insert(path) }
                for move in moves { paths.insert(move.to) }
            }
            return Source(decoded: source.decoded, paths: paths.sorted())
        }
        let result = Proof(temporalSHA256: proof.temporalSHA256, sources: sources).dictionary
        try validateStorage(result)
        return try parse(result).dictionary
    }

    private static func parse(_ raw: Any) throws -> Proof {
        guard let object = raw as? [String: Any] else { throw failure("Expected a duration-proof object.") }
        guard try integer(object["version"], "proof version", 1...Int64.max) == 1 else {
            throw failure("Unsupported duration-proof version.")
        }
        guard Set(object.keys) == ["version", "temporalSHA256", "sources"],
              let hash = object["temporalSHA256"] as? String,
              let rows = object["sources"] as? [[String: Any]], rows.count <= maximumSources else {
            throw failure("Malformed duration-proof fields.")
        }
        try validateHash(hash)
        var hashes = Set<String>(), allPaths = Set<String>(), pathBytes = 0
        let sources = try rows.map { row -> Source in
            guard Set(row.keys) == ["sha256", "decodedFrames", "decodedSampleRate", "paths"],
                  let hash = row["sha256"] as? String, let paths = row["paths"] as? [String],
                  !paths.isEmpty, paths.count <= maximumAliases else { throw failure("Malformed decoded source evidence.") }
            let source = DecodedSource(sha256: hash,
                decodedFrames: try integer(row["decodedFrames"], "decoded frames", 1...100_000_000),
                decodedSampleRate: try number(row["decodedSampleRate"], "decoded sample rate", 1_000...768_000))
            try validate(source)
            guard hashes.insert(hash).inserted else { throw failure("Repeated source hash in duration proof.") }
            for path in paths {
                try validatePath(path)
                guard allPaths.insert(path).inserted else { throw failure("Repeated or ambiguous source alias.") }
                pathBytes += path.utf8.count
                guard allPaths.count <= maximumAliases, pathBytes <= maximumAggregatePathBytes else {
                    throw failure("Duration source aliases exceed their storage budget.")
                }
            }
            return Source(decoded: source, paths: paths.sorted())
        }
        return Proof(temporalSHA256: hash, sources: sources.sorted { $0.decoded.sha256 < $1.decoded.sha256 })
    }

    /// Proof-v1 byte grammar, little endian throughout:
    /// length-prefixed domain UTF8; u32 version; f64 outputRate; u64 musicalFrames;
    /// u32 LPB; f32 swing; u8 mapped; tempo count + (f64 beat,bpm)*;
    /// meter count + (f64 beat,u32 numerator,denominator)*; owner count + strings;
    /// sorted pattern count + (u32 id,length)*; sequence count + u32 IDs;
    /// clip count + (owner string,f64 start,duration,offset,u8 muted,
    ///   if !muted: hash string,u64 decodedFrames,f64 decodedRate)*.
    /// Counts and string byte lengths are u32. +/-0 encode as positive zero.
    /// Map IDs, source aliases and non-temporal settings are intentionally absent.
    private static func encode(project: [String: Any], sampleRate: Double, musicalFrames: Int64,
                               resolve: (String, String) throws -> DecodedSource) throws -> Data {
        guard sampleRate.isFinite, (8_000...384_000).contains(sampleRate), (1...100_000_000).contains(musicalFrames) else {
            throw failure("Invalid captured output rate or exact musical frame count.")
        }
        var out = Encoder()
        out.string("SuperTracker.FreezeDurationProof"); out.u32(1); out.double(sampleRate); out.u64(UInt64(musicalFrames))
        out.u32(UInt32(try integer(project["linesPerBeat"] ?? 4, "lines per beat", 1...32)))
        let swing = Float(try number(project["swing"] ?? 0, "swing", 0...0.95))
        out.float(swing)
        // One fixed tempo and meter (format byte 0; byte 1 was the retired tempo map).
        out.byte(0); out.u32(1); out.double(0); out.double(try number(project["bpm"] ?? 120, "BPM", 20...999))
        var signature: [String: Any] = [:]
        if let raw = project["timeSignature"] {
            guard let value = raw as? [String: Any] else { throw failure("Expected a time signature object.") }
            signature = value
        }
        let numerator = try integer(signature["numerator"] ?? 4, "meter numerator", 1...16)
        let denominator = try integer(signature["denominator"] ?? 4, "meter denominator", 2...16)
        guard [2, 4, 8, 16].contains(denominator) else { throw failure("Unsupported meter denominator.") }
        out.u32(1); out.double(0); out.u32(UInt32(numerator)); out.u32(UInt32(denominator))
        let owners = try trackOwners(project)
        out.u32(UInt32(owners.count)); for owner in owners { out.string(owner) }
        guard let rawPatterns = project["patterns"] as? [[String: Any]], (1...256).contains(rawPatterns.count) else {
            throw failure("Provide 1–256 canonical patterns.")
        }
        var patterns: [Int64: Int64] = [:]
        for (index, pattern) in rawPatterns.enumerated() {
            let id = try integer(pattern["id"] ?? index, "pattern ID", 0...255)
            let length = try integer(pattern["length"] ?? 64, "pattern length", 1...256)
            guard patterns.updateValue(length, forKey: id) == nil else { throw failure("Duplicate pattern ID.") }
        }
        out.u32(UInt32(patterns.count))
        for id in patterns.keys.sorted() { out.u32(UInt32(id)); out.u32(UInt32(patterns[id]!)) }
        let rawSequence = project["sequence"] ?? [0]
        // Match AudioProject's whole-array cast before stricter per-value checks.
        // A Swift [Double] may otherwise hash IDs the mapper replaces with [0].
        guard let sequence = rawSequence as? [Any], rawSequence is [Int],
              (1...1_024).contains(sequence.count) else {
            throw failure("Provide 1–1024 canonical sequence entries.")
        }
        out.u32(UInt32(sequence.count))
        for value in sequence {
            let id = try integer(value, "sequence pattern", 0...255)
            guard patterns[id] != nil else { throw failure("Sequence references a missing pattern.") }
            out.u32(UInt32(id))
        }
        guard let clips = (project["audioClips"] ?? []) as? [[String: Any]], clips.count <= 256 else {
            throw failure("Provide at most 256 canonical audio clips.")
        }
        out.u32(UInt32(clips.count))
        for clip in clips {
            let track = try integer(clip["track"] ?? 0, "clip track", 0...Int64(owners.count - 1))
            let owner = owners[Int(track)]
            out.string(owner)
            out.double(try number(clip["startBeat"] ?? 0, "clip start", 0...1_000_000))
            out.double(try number(clip["durationBeats"] ?? 0, "clip duration", 0...1_000_000))
            out.double(try number(clip["sourceOffset"] ?? clip["offsetSeconds"] ?? 0, "clip source offset", 0...1_000_000))
            let muted = try boolean(clip["muted"] ?? clip["mute"] ?? false, "clip mute")
            out.byte(muted ? 1 : 0)
            if !muted {
                guard let path = (clip["path"] ?? clip["samplePath"]) as? String else { throw failure("An active clip needs a selected source path.") }
                try validatePath(path)
                let source = try resolve(path, owner)
                try validate(source)
                out.string(source.sha256); out.u64(UInt64(source.decodedFrames)); out.double(source.decodedSampleRate)
            }
        }
        return out.data
    }

    /// Sanity bound only (mirrors ST_TRACK_SANITY_LIMIT; this file builds standalone).
    private static let trackSanityLimit = 1024
    private static func trackOwners(_ project: [String: Any]) throws -> [String] {
        guard let tracks = project["tracks"] as? [[String: Any]], (1...trackSanityLimit).contains(tracks.count) else { throw failure("Provide 1–\(trackSanityLimit) canonical tracks.") }
        var owners = Set<String>()
        return try tracks.map { track in
            guard let id = track["routingID"] as? String, !id.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                  id.utf8.count <= 256, !id.contains("\0") else { throw failure("Each track needs a stable routing ID.") }
            let owner = "track:" + id
            guard owners.insert(owner).inserted else { throw failure("Duplicate stable track owner.") }
            return owner
        }
    }
    private static func validate(_ source: DecodedSource) throws {
        try validateHash(source.sha256)
        guard (1...100_000_000).contains(source.decodedFrames), source.decodedSampleRate.isFinite,
              (1_000...768_000).contains(source.decodedSampleRate) else { throw failure("Invalid actual decoded source extent.") }
    }
    private static func validateHash(_ value: String) throws {
        guard value.utf8.count == 64, value.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }) else {
            throw failure("Expected a full lowercase SHA256 digest.")
        }
    }
    private static func validatePath(_ value: String) throws {
        guard !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, value.utf8.count <= maximumPathBytes,
              !value.contains("\0"), !value.contains("://"), value == (value as NSString).standardizingPath else {
            throw failure("Expected a normalized local or package-relative source path.")
        }
    }
    private static func number(_ raw: Any?, _ label: String, _ range: ClosedRange<Double>) throws -> Double {
        guard let value = PayloadValue.number(raw, in: range) else { throw failure("\(label) must be a finite number in its supported range.") }
        return value == 0 ? 0 : value
    }
    private static func integer(_ raw: Any?, _ label: String, _ range: ClosedRange<Int64>) throws -> Int64 {
        let value = try number(raw, label, Double(range.lowerBound)...Double(range.upperBound))
        // Int64.max rounds to 2^63 as Double; reject that value before conversion.
        guard PayloadValue.isWhole(value) else { throw failure("\(label) must be an exact supported integer.") }
        let result = Int64(value)
        guard range.contains(result) else { throw failure("\(label) is outside its supported range.") }
        return result
    }
    private static func boolean(_ raw: Any, _ label: String) throws -> Bool {
        guard let value = PayloadValue.boolean(raw) else { throw failure("\(label) must be a Boolean.") }
        return value
    }
    private static func validateStorage(_ raw: Any) throws {
        // Iterative preflight has no independent nesting/node policy: future
        // representable JSON within the byte budget must remain repairable.
        // This lower bound rejects certainly oversized values before encoding.
        var pending: [Any] = [raw], minimumBytes = 0
        while let value = pending.popLast() {
            if let text = value as? String { minimumBytes += text.utf8.count + 2 }
            else if let object = value as? [String: Any] {
                guard object.count <= maximumStorageBytes else { throw failure("Duration proof exceeds its JSON storage budget.") }
                minimumBytes += 2 + max(0, object.count - 1)
                for (key, item) in object { minimumBytes += key.utf8.count + 3; pending.append(item) }
            } else if let array = value as? [Any] {
                guard array.count <= maximumStorageBytes else { throw failure("Duration proof exceeds its JSON storage budget.") }
                minimumBytes += 2 + max(0, array.count - 1)
                pending.append(contentsOf: array)
            } else if let number = value as? NSNumber {
                guard number.doubleValue.isFinite else { throw failure("Duration proof contains a non-JSON number.") }
                minimumBytes += 1
            } else if value is NSNull { minimumBytes += 4 }
            else { throw failure("Duration proof must contain only JSON values.") }
            guard minimumBytes <= maximumStorageBytes else { throw failure("Duration proof exceeds its JSON storage budget.") }
        }
        let encoded = try JSONSerialization.data(withJSONObject: raw, options: [.sortedKeys, .fragmentsAllowed, .withoutEscapingSlashes])
        guard encoded.count <= maximumStorageBytes else { throw failure("Duration proof exceeds its JSON storage budget.") }
    }
    private struct Encoder {
        var data = Data()
        mutating func byte(_ value: UInt8) { data.append(value) }
        mutating func u32(_ value: UInt32) { for n in 0..<4 { byte(UInt8(truncatingIfNeeded: value >> (8 * n))) } }
        mutating func u64(_ value: UInt64) { for n in 0..<8 { byte(UInt8(truncatingIfNeeded: value >> (8 * n))) } }
        mutating func double(_ value: Double) { u64((value == 0 ? 0.0 : value).bitPattern) }
        mutating func float(_ value: Float) { u32((value == 0 ? 0.0 : value).bitPattern) }
        mutating func string(_ value: String) { let bytes = Array(value.utf8); u32(UInt32(bytes.count)); data.append(contentsOf: bytes) }
    }
    private static func digest(_ bytes: Data) -> String { SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined() }
    private static func failure(_ message: String) -> Failure { Failure(message: message) }
}

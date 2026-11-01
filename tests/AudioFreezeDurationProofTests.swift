import Foundation

@main enum AudioFreezeDurationProofTests {
    typealias D = AudioFreezeDurationProof
    struct Failure: Error { let message: String }
    static let a = D.DecodedSource(sha256: String(repeating: "a", count: 64), decodedFrames: 6007, decodedSampleRate: 44_100)
    static let b = D.DecodedSource(sha256: String(repeating: "b", count: 64), decodedFrames: 24_000, decodedSampleRate: 48_000)
    static let path = "/source/a.wav", otherPath = "/source/b.wav"
    static var groups = 0
    static func check(_ value: @autoclosure () throws -> Bool, _ message: String) throws {
        if try !value() { throw Failure(message: message) }
    }
    static func rejects(_ name: String, _ body: () throws -> Void) throws {
        do { try body() } catch is D.Failure { return }
        throw Failure(message: "Expected rejection: " + name)
    }
    static func unavailable(_ raw: Any, _ name: String) throws {
        guard case .unavailable = try D.assess(raw) else { throw Failure(message: "Expected unavailable: " + name) }
    }
    static func supported(_ raw: Any?) throws -> D.Proof {
        guard case let .supported(proof) = try D.assess(raw) else { throw Failure(message: "Expected supported proof") }
        return proof
    }
    static func json(_ raw: Any) throws -> Data { try JSONSerialization.data(withJSONObject: raw, options: [.sortedKeys, .fragmentsAllowed]) }
    static func pass(_ name: String) { groups += 1; print("PASS duration proof: " + name) }
    static var minimal: [String: Any] { ["tracks": [["routingID": "alpha"]], "patterns": [["length": 4]]] }
    static var fixture: [String: Any] {
        ["tracks": [["routingID": "alpha"], ["routingID": "beta"]],
         "patterns": [["id": 0, "length": 4], ["id": 7, "length": 8]], "sequence": [0, 7, 0],
         "bpm": 123.5, "swing": 0.125, "timeSignature": ["numerator": 7, "denominator": 8],
         "audioClips": [["track": 0, "path": path, "startBeat": 1.5, "sourceOffset": 0.125],
                        ["track": 1, "startBeat": 9, "durationBeats": 4, "muted": true]]]
    }
    static func make(_ project: [String: Any] = fixture, frames: Int64 = 96_000,
                     sources: [String: D.DecodedSource]? = nil) throws -> D.Proof {
        try D.make(project: project, sampleRate: 48_000, musicalFrames: frames, resolvedSources: sources ?? [path: a, otherPath: b])
    }
    static func verify(_ proof: D.Proof, _ project: [String: Any] = fixture, frames: Int64 = 96_000,
                       dormant: Set<String> = [], sources: [String: D.DecodedSource]? = nil) throws {
        try D.verify(proof: proof, project: project, sampleRate: 48_000, musicalFrames: frames,
                     dormantOwners: dormant, liveSources: sources ?? [path: a, otherPath: b])
    }
    static func clips(_ project: inout [String: Any], _ body: (inout [[String: Any]]) -> Void) {
        var clips = project["audioClips"] as? [[String: Any]] ?? []; body(&clips); project["audioClips"] = clips
    }
    static func main() throws {
        // Independently produced using Python struct.pack little-endian grammar,
        // not values computed by the Swift encoder under test.
        let minimalProof = try make(minimal, frames: 6007, sources: [:])
        try check(minimalProof.temporalSHA256 == "a64f263f5444e6d463d7200b9414314c821dd92d469ad1ce1f947e009280ee7e", "Minimal typed encoding golden changed")
        let proof = try make()
        try check(proof.temporalSHA256 == "54240baf24007bd3ecab30fe7c4ff8a1ae550b53f57f0b3981598ad673f94a66", "Clip and swing encoding golden changed")
        try check(try supported(proof.dictionary) == proof, "Known proof must round-trip deterministically")
        try check(proof.sources == [D.Source(decoded: a, paths: [path])], "Only selected active clip sources belong in evidence")
        pass("two independently generated golden vectors: defaults and clips/swing")

        var equivalent = minimal
        equivalent["linesPerBeat"] = 4.0; equivalent["bpm"] = 120.0; equivalent["swing"] = -0.0
        equivalent["timeSignature"] = ["numerator": 4.0, "denominator": 4.0]
        equivalent["patterns"] = [["id": 0.0, "length": 4.0]]; equivalent["sequence"] = [NSNumber(value: 0.0)]; equivalent["audioClips"] = [[String: Any]]()
        try check(try make(equivalent, frames: 6007).temporalSHA256 == minimalProof.temporalSHA256, "Equivalent numeric/default/zero representations differ")
        var aliased = fixture
        clips(&aliased) { clips in
            clips[0].removeValue(forKey: "path"); clips[0]["samplePath"] = path
            clips[0].removeValue(forKey: "sourceOffset"); clips[0]["offsetSeconds"] = 0.125
            clips[0]["durationBeats"] = 0.0; clips[0]["muted"] = false
            clips[1].removeValue(forKey: "muted"); clips[1]["mute"] = true
        }
        try check(try make(aliased).temporalSHA256 == proof.temporalSHA256, "Accepted aliases must normalize identically")
        clips(&aliased) { $0[0]["path"] = path; $0[0]["samplePath"] = "/ignored.wav"; $0[0]["sourceOffset"] = 0.125; $0[0]["offsetSeconds"] = 77; $0[1]["muted"] = true; $0[1]["mute"] = false }
        try check(try make(aliased).temporalSHA256 == proof.temporalSHA256, "Primary field precedence must match native mapping")
        var swingMax = minimal; swingMax["swing"] = 0.95
        _ = try make(swingMax, sources: [:])
        pass("native alias precedence, default equivalence, exact integer types and Float32 swing")

        var benign = fixture
        benign["name"] = "Renamed"; benign["loopSong"] = true; benign["metronome"] = true; benign["masterVolume"] = 0.2
        benign["tracks"] = [["routingID": "alpha", "mute": true, "solo": true, "volume": 0.1], ["routingID": "beta", "pan": 1.0]]
        clips(&benign) { $0[0]["gain"] = 0.1; $0[0]["fadeIn"] = 2; $0[0]["name"] = "Clip"; $0[0]["takes"] = [["path": "/inactive.wav"]]; $0[1]["path"] = "/missing-muted.wav" }
        try verify(proof, benign)
        pass("duration-neutral names, mix, inactive takes and muted source changes")

        var changed = fixture
        clips(&changed) { $0.removeLast() }
        try rejects("unrelated clip removal") { try verify(proof, changed) }
        for (field, value) in [("startBeat", 9.5), ("durationBeats", 3.0), ("sourceOffset", 0.1), ("track", 0.0)] {
            changed = fixture; clips(&changed) { $0[1][field] = value }
            try rejects("unrelated clip \(field)") { try verify(proof, changed) }
        }
        changed = fixture; clips(&changed) { $0.reverse() }
        try rejects("clip order") { try verify(proof, changed) }
        changed = fixture; clips(&changed) { $0.append($0[0]) }
        try rejects("duplicate clip addition") { try verify(proof, changed) }
        changed = fixture; clips(&changed) { $0[1]["muted"] = false; $0[1]["path"] = path }
        try rejects("unmuting") { try verify(proof, changed) }
        for field in ["bpm", "linesPerBeat", "swing"] {
            changed = fixture; changed[field] = field == "swing" ? 0.25 : 24
            try rejects("timing \(field)") { try verify(proof, changed) }
        }
        changed = fixture; changed["sequence"] = [7, 0, 0]
        try rejects("sequence") { try verify(proof, changed) }
        changed = fixture; changed["patterns"] = [["id": 0, "length": 5], ["id": 7, "length": 8]]
        try rejects("pattern length") { try verify(proof, changed) }
        pass("conservative global clip geometry/order/mute and timing protection detects both shrink and extension")

        var twoLive = fixture
        clips(&twoLive) { $0[1]["muted"] = false; $0[1]["path"] = otherPath }
        let twoProof = try make(twoLive)
        for duration in [3.0, 5.0] {
            var resizedLastClip = twoLive; clips(&resizedLastClip) { $0[1]["durationBeats"] = duration }
            try rejects("last-ending unrelated live clip shrink/extension") { try verify(twoProof, resizedLastClip, dormant: ["track:alpha"]) }
        }
        var removedLastClip = twoLive; clips(&removedLastClip) { $0.removeLast() }
        try rejects("last-ending unrelated live clip removal") { try verify(twoProof, removedLastClip, dormant: ["track:alpha"]) }
        try verify(twoProof, twoLive, dormant: ["track:alpha"], sources: [otherPath: b])
        try rejects("unrelated live source missing") { try verify(twoProof, twoLive, dormant: ["track:alpha"], sources: [:]) }
        try rejects("frozen-only knowledge cannot bypass unrelated live PCM") { try verify(twoProof, twoLive, sources: [path: a]) }
        try verify(twoProof, twoLive, dormant: ["track:alpha", "track:beta"], sources: [:])
        try rejects("unknown dormant owner") { try verify(twoProof, twoLive, dormant: ["track:missing"]) }
        let changedBytes = D.DecodedSource(sha256: String(repeating: "c", count: 64), decodedFrames: b.decodedFrames, decodedSampleRate: b.decodedSampleRate)
        try rejects("same length changed bytes") { try verify(twoProof, twoLive, dormant: ["track:alpha"], sources: [otherPath: changedBytes]) }
        for source in [D.DecodedSource(sha256: b.sha256, decodedFrames: b.decodedFrames + 1, decodedSampleRate: b.decodedSampleRate),
                       D.DecodedSource(sha256: b.sha256, decodedFrames: b.decodedFrames, decodedSampleRate: 44_100)] {
            try rejects("decoded extent mismatch") { try verify(twoProof, twoLive, dormant: ["track:alpha"], sources: [otherPath: source]) }
        }
        pass("missing dormant versus live originals, unrelated live hashes and exact decoded frame/rate evidence")

        let movedRaw = try D.relocating(twoProof.dictionary, relocations: [.init(from: otherPath, to: "Session.media/b.wav", sha256: b.sha256)])
        let moved = try supported(movedRaw)
        try check(moved.temporalSHA256 == twoProof.temporalSHA256, "Relink must preserve temporal digest")
        changed = twoLive; clips(&changed) { $0[1]["path"] = "Session.media/b.wav" }
        try verify(moved, changed, dormant: ["track:alpha"], sources: ["Session.media/b.wav": b])
        try verify(moved, twoLive)
        let portable = try supported(D.relocating(twoProof.dictionary, relocations: [.init(from: path, to: "Session.media/a.wav", sha256: a.sha256), .init(from: otherPath, to: "Session.media/b.wav", sha256: b.sha256)], replacePaths: true))
        try check(portable.sources.flatMap(\.paths).allSatisfy { !$0.hasPrefix("/") }, "Portable replacement must not retain only machine-specific aliases")
        let loaded = try supported(D.relocating(portable.dictionary, relocations: [.init(from: "Session.media/a.wav", to: "/relocated/Session.media/a.wav", sha256: a.sha256), .init(from: "Session.media/b.wav", to: "/relocated/Session.media/b.wav", sha256: b.sha256)], replacePaths: true))
        changed = twoLive; clips(&changed) { $0[0]["path"] = "/relocated/Session.media/a.wav"; $0[1]["path"] = "/relocated/Session.media/b.wav" }
        try verify(loaded, changed, dormant: ["track:alpha"], sources: ["/relocated/Session.media/b.wav": b])
        try rejects("different-content alias") { _ = try D.relocating(twoProof.dictionary, relocations: [.init(from: otherPath, to: "/wrong.wav", sha256: a.sha256)]) }
        try rejects("alias collision") { _ = try D.relocating(twoProof.dictionary, relocations: [.init(from: path, to: otherPath, sha256: a.sha256)]) }
        let sharedMoves = [D.Relocation(from: path, to: "/one.wav", sha256: a.sha256), D.Relocation(from: path, to: "/two.wav", sha256: a.sha256)]
        let sharedMoved = try supported(D.relocating(proof.dictionary, relocations: sharedMoves))
        try check(sharedMoved.sources[0].paths == ["/one.wav", path, "/two.wav"].sorted(), "Independent same-content relinks must preserve both aliases")
        try rejects("conflicting portable relocation") { _ = try D.relocating(proof.dictionary, relocations: sharedMoves, replacePaths: true) }
        try check(try supported(D.relocating(proof.dictionary, relocations: [.init(from: "/unused.wav", to: "/other.wav", sha256: a.sha256)])) == proof, "Unbound alias must never mint source evidence")
        pass("exact-content append and portable rebasing for unrelated live clips, collision and mismatch rejection")

        guard case .absent = try D.assess(nil) else { throw Failure(message: "Old records must preserve absence") }
        let opaque: [String: Any] = ["version": 42, "sources": [["path": "opaque/value.wav"]], "extra": [true, 0, NSNull()] as [Any]]
        try unavailable(opaque, "future version")
        try check(try json(D.relocating(opaque, relocations: [.init(from: "opaque/value.wav", to: "/new.wav", sha256: a.sha256)])!) == json(opaque), "Unknown proof must remain opaque")
        for malformed: Any in [NSNull(), "opaque", 3, ["version": 1], ["version": true], ["version": 1, "temporalSHA256": "bad", "sources": []]] {
            try unavailable(malformed, "repairable malformed JSON")
            try check(try json(D.relocating(malformed, relocations: [])!) == json(malformed), "Malformed proof must round-trip without repair guesses")
        }
        try rejects("non-JSON") { _ = try D.assess(Date()) }
        try rejects("NaN JSON") { _ = try D.assess(["version": 9, "bad": Double.nan]) }
        try rejects("opaque bytes") { _ = try D.assess(["version": 9, "large": String(repeating: "x", count: D.maximumStorageBytes + 1)]) }
        var nested: Any = 0
        for _ in 0..<34 { nested = [nested] }
        try unavailable(nested, "deep representable opaque JSON must remain savable")
        try unavailable(Array(repeating: NSNull(), count: 70_000), "opaque node count has no extra save cap")
        pass("absent, malformed and unknown proof preservation; bounded JSON rejects only unsafe storage")

        let invalidFields: [(String, Any)] = [("bpm", true), ("linesPerBeat", 4.5), ("swing", Double.infinity), ("bpm", "120"), ("linesPerBeat", 33), ("tracks", []), ("patterns", []), ("sequence", []), ("audioClips", "invalid")]
        for (key, value) in invalidFields { changed = fixture; changed[key] = value; try rejects("typed \(key)") { _ = try make(changed) } }
        for (key, value): (String, Any) in [("startBeat", true), ("durationBeats", -1), ("sourceOffset", Double.nan), ("track", 0.5), ("muted", 1), ("path", 42)] {
            changed = fixture; clips(&changed) { $0[0][key] = value }
            try rejects("clip typed \(key)") { _ = try make(changed) }
        }
        changed = fixture; changed["tracks"] = [["routingID": "alpha"], ["routingID": "alpha"]]
        try rejects("stable owners") { _ = try make(changed) }
        changed = fixture; changed["patterns"] = [["id": 0], ["id": 0]]
        try rejects("duplicate patterns") { _ = try make(changed) }
        changed = fixture; changed["sequence"] = [99]
        try rejects("missing sequence target") { _ = try make(changed) }
        changed = fixture; changed["sequence"] = [Double(7)]
        try rejects("array representation that native replaces with pattern zero") { _ = try make(changed) }
        for invalid in ["", "relative//not-normalized.wav", "https://remote/a.wav", "bad\0path"] {
            changed = fixture; clips(&changed) { $0[0]["path"] = invalid }
            try rejects("invalid local path") { _ = try make(changed, sources: [invalid: a]) }
        }
        pass("strict finite non-Boolean numbers, canonical capacities/identities and local path validation")

        for wrong in [D.DecodedSource(sha256: "aa", decodedFrames: 1, decodedSampleRate: 48_000),
                      D.DecodedSource(sha256: a.sha256.uppercased(), decodedFrames: 1, decodedSampleRate: 48_000),
                      D.DecodedSource(sha256: a.sha256, decodedFrames: 0, decodedSampleRate: 48_000),
                      D.DecodedSource(sha256: a.sha256, decodedFrames: 100_000_001, decodedSampleRate: 48_000),
                      D.DecodedSource(sha256: a.sha256, decodedFrames: 1, decodedSampleRate: .nan)] {
            try rejects("source evidence bounds") { _ = try make(sources: [path: wrong]) }
        }
        changed = fixture; clips(&changed) { $0[1]["muted"] = false; $0[1]["path"] = otherPath }
        let conflict = D.DecodedSource(sha256: a.sha256, decodedFrames: a.decodedFrames + 1, decodedSampleRate: a.decodedSampleRate)
        try rejects("same hash contradictory decode") { _ = try make(changed, sources: [path: a, otherPath: conflict]) }
        var duplicated = proof.dictionary
        duplicated["sources"] = [proof.sources[0].dictionary, proof.sources[0].dictionary]
        try unavailable(duplicated, "duplicate source")
        var excessive = proof.sources[0].dictionary
        excessive["paths"] = (0...D.maximumAliases).map { "/a/\($0).wav" }
        duplicated["sources"] = [excessive]
        try unavailable(duplicated, "alias count")
        excessive["paths"] = (0..<70).map { "/a/\($0)/" + String(repeating: "x", count: 15_000) }
        duplicated["sources"] = [excessive]
        try unavailable(duplicated, "aggregate paths")
        excessive["paths"] = [String(repeating: "x", count: D.maximumPathBytes + 1)]
        duplicated["sources"] = [excessive]
        try unavailable(duplicated, "single path bytes")
        pass("hash/decode consistency and per-path, aggregate alias and source bounds")

        let observed = try make(minimal, frames: 6007), nextObserved = try make(minimal, frames: 6008)
        try check(observed.temporalSHA256 != nextObserved.temporalSHA256, "Neighboring observed endpoints must remain distinct")
        try rejects("no source-to-output frame inference") { try verify(observed, minimal, frames: 6008, sources: [:]) }
        let arbitraryObserved = try make(frames: 6008)
        try verify(arbitraryObserved, frames: 6008)
        try check(arbitraryObserved.sources[0].decoded.decodedFrames == 6007, "Source evidence must remain separate from observed output endpoint")
        for frames: Int64 in [0, -1, 100_000_001, Int64.max] { try rejects("extent cap") { _ = try make(minimal, frames: frames) } }
        for rate in [Double.nan, Double.infinity, 7_999, 384_001] { try rejects("rate cap") { _ = try D.make(project: minimal, sampleRate: rate, musicalFrames: 6007, resolvedSources: [:]) } }
        let maximum = try D.make(project: minimal, sampleRate: 384_000, musicalFrames: 100_000_000, resolvedSources: [:])
        try D.verify(proof: maximum, project: minimal, sampleRate: 384_000, musicalFrames: 100_000_000, dormantOwners: [], liveSources: [:])
        pass("observed integer musical endpoint is independent of decoded source extent and seconds rounding")

        var knownInvalid = proof.dictionary; knownInvalid["unexpected"] = "opaque"
        try unavailable(knownInvalid, "unknown supported keys")
        let before = try json(twoProof.dictionary)
        for _ in 0..<100 { try verify(twoProof, twoLive, dormant: ["track:alpha"], sources: [otherPath: b]) }
        try check(try json(twoProof.dictionary) == before, "Repeated admission must not mutate captured evidence")
        pass("unsupported fields and repeated immutable inheritance checks")
        print("PASS \(groups) freeze duration-proof groups; no files, audio, plug-ins or native rendering")
    }
}

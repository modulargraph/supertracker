import Foundation

@main struct AudioFreezeProjectTests {
    typealias Freeze = AudioFreezeProject
    struct Failure: Error, CustomStringConvertible { let description: String }
    static let owner = "track:t0"
    static let sourceHash = String(repeating: "a", count: 64)
    static let zoneHash = String(repeating: "b", count: 64)
    static let clipHash = String(repeating: "c", count: 64)
    static let printHash = String(repeating: "d", count: 64)
    static let earlyHash = String(repeating: "e", count: 64)
    static let maximumAddress = "18446744073709551615"
    static var groups = 0
    static func check(_ condition: @autoclosure () throws -> Bool, _ message: String) throws {
        if try !condition() { throw Failure(description: message) }
    }
    static func rejects(_ message: String, _ body: () throws -> Void) throws {
        do { try body() } catch is Freeze.Failure { return } catch is AudioRouting.RoutingError { return }
        throw Failure(description: "Expected rejection: " + message)
    }
    static func passed(_ message: String) { groups += 1; print("PASS freeze policy: " + message) }
    static func json(_ value: Any) throws -> Data { try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys, .fragmentsAllowed]) }
    static func copy(_ value: [String: Any]) throws -> [String: Any] { try JSONSerialization.jsonObject(with: json(value)) as! [String: Any] }
    static func rows(_ project: [String: Any]) -> [[Any]] { (project["patterns"] as! [[String: Any]])[0]["steps"] as! [[Any]] }
    static func editTrack(_ project: inout [String: Any], _ index: Int, _ body: (inout [String: Any]) -> Void) {
        var values = project["tracks"] as! [[String: Any]]; body(&values[index]); project["tracks"] = values
    }
    static func editInstrument(_ project: inout [String: Any], _ index: Int, _ body: (inout [String: Any]) -> Void) {
        var values = project["instruments"] as! [[String: Any]]; body(&values[index]); project["instruments"] = values
    }
    static func editPlugin(_ project: inout [String: Any], _ id: String, _ body: (inout [String: Any]) -> Void) {
        var values = project["audioPlugins"] as! [[String: Any]]; body(&values[values.firstIndex { $0["instanceID"] as? String == id }!]); project["audioPlugins"] = values
    }
    static func editStep(_ project: inout [String: Any], _ track: Int, _ row: Int, _ body: (inout [String: Any]) -> Void) {
        var patterns = project["patterns"] as! [[String: Any]], steps = patterns[0]["steps"] as! [[Any]], value = steps[track][row] as? [String: Any] ?? [:]
        body(&value); steps[track][row] = value; patterns[0]["steps"] = steps; project["patterns"] = patterns
    }
    static func editLane(_ project: inout [String: Any], _ id: String, _ body: (inout [String: Any]) -> Void) {
        var patterns = project["patterns"] as! [[String: Any]], lanes = patterns[0]["automation"] as! [[String: Any]]
        body(&lanes[lanes.firstIndex { $0["id"] as? String == id }!]); patterns[0]["automation"] = lanes; project["patterns"] = patterns
    }
    static func plugin(_ id: String, _ owner: String, _ kind: String, _ slot: Int) -> [String: Any] {
        ["instanceID": id, "owner": owner, "track": owner == Self.owner ? 14 : 1, "kind": kind, "slot": slot,
         "component": "original-fixture:" + id, "format": "AU", "state": "opaque:\u{0}:<" + id + ">", "metadata": ["keep": [1, 2, 3]]]
    }
    static func lock(_ id: String, value: Double = 0.25) -> [String: Any] {
        ["type": "plugin", "instanceID": id, "address": maximumAddress, "value": value, "kind": "effect", "slot": 2]
    }
    static func lane(_ id: String, _ owner: String, _ target: String) -> [String: Any] {
        ["id": id, "owner": owner, "track": owner == Self.owner ? 15 : 0, "target": target, "interpolation": "linear", "points": [["row": 0, "value": 0.4]]]
    }
    static func nativeLane(_ id: String, _ target: String) -> [String: Any] {
        ["id": id, "track": 0, "target": target, "interpolation": "linear", "points": [["row": 0, "value": 0.4]]]
    }
    static func fixture() -> [String: Any] {
        var tracks = (0..<16).map { ["id": $0, "routingID": "t\($0)", "name": "Track \($0)", "instrument": 2, "volume": 0.75, "pan": 0.1, "delaySend": 0.2, "reverbSend": 0.3] as [String: Any] }
        tracks[0]["instrument"] = 0; tracks[1]["instrument"] = 0
        var instruments = (0..<64).map { ["id": $0, "name": "Instrument \($0)", "type": "synth", "waveform": "sine", "volume": 0.6] as [String: Any] }
        instruments[0] = ["id": 0, "name": "Shared sample", "type": "sample", "samplePath": "/unread/source.wav", "volume": 0.6,
            "zones": [["samplePath": "/unread/zone.wav", "rootNote": 60]], "plugin": ["component": "dormant-source", "state": "must-not-reload"]]
        var steps = Array(repeating: Array<Any>(repeating: NSNull(), count: 6), count: 16)
        steps[0][0] = ["note": 60, "instrument": 0, "gate": 0.8, "fx1": ["type": "volume", "value": 0.5], "fx2": ["type": "cutoff", "value": 1400]]
        steps[0][1] = ["note": 64, "instrument": 1, "fx1": lock("source-fx"), "fx2": ["type": "pan", "value": 0.2]]
        steps[0][2] = ["note": -3, "fx1": ["type": "delay", "value": 0.3], "fx2": ["type": "reverb", "value": 0.4]]
        steps[0][3] = ["note": 67, "instrument": 0, "fx1": lock("other-fx"), "fx2": lock("unavailable")]
        steps[0][4] = ["note": -1, "fx1": ["type": "volume", "value": 0.01]]
        steps[0][5] = ["note": -2, "fx2": ["type": "pan", "value": -0.9]]
        steps[1][0] = ["note": 72, "instrument": 0, "fx1": lock("other-fx"), "fx2": lock("source-fx")]
        steps[1][1] = ["note": 74, "instrument": 0, "fx1": lock("same-slot-missing"), "fx2": ["type": "volume", "value": 0.4]]
        steps[2][0] = ["note": 48, "instrument": 2, "gate": 0.5]
        let lanes = [lane("source-volume", owner, "volume"), lane("source-pan", owner, "pan"), nativeLane("source-delay", "delay"), nativeLane("source-reverb", "reverb"),
            nativeLane("source-filter", "cutoff"), lane("source-instrument", owner, "plugin:instrument:0:42"), lane("source-insert", owner, "plugin:effect:6:42"),
            lane("other-insert", "track:t1", "plugin:effect:2:42"), lane("incoming-gain", "track:t3", "send:incoming"), lane("early-gain", owner, "send:early")]
        var outputs = Dictionary(uniqueKeysWithValues: (0..<16).map { ("track:t\($0)", "master") })
        outputs[owner] = "bus:group"; outputs["track:t2"] = owner; outputs["bus:group"] = "master"; outputs["bus:return"] = "master"
        return ["version": 1, "name": "Freeze policy fixture", "bpm": 120, "linesPerBeat": 4, "swing": 0, "activePattern": 0,
            "tracks": tracks, "instruments": instruments, "patterns": [["id": 0, "length": 6, "steps": steps, "automation": lanes]], "sequence": [0],
            "audioPlugins": [plugin("source-au", owner, "instrument", 0), plugin("source-fx", owner, "effect", 6), plugin("other-fx", "track:t1", "effect", 2), plugin("dependency-fx", "track:t2", "effect", 0)],
            "audioClips": [["id": "original-clip", "track": 0, "path": "/unread/clip.wav", "startBeat": 0, "durationBeats": 1, "sourceOffset": 0.1, "gain": 0.8]],
            "routing": ["version": 1, "mixModel": "standard", "buses": [["id": "group", "type": "group", "name": "Group"], ["id": "return", "type": "return", "name": "Return"]], "outputs": outputs,
                "sends": [["id": "incoming", "source": "track:t3", "destination": owner, "tap": "postFader", "gain": 0.3], ["id": "early", "source": owner, "destination": "bus:return", "tap": "preInserts", "gain": 0.2]],
                "sidechains": [["id": "key", "source": "track:t4", "tap": "preFader", "destination": ["instanceID": "source-fx", "inputBus": 1, "layoutSignature": ["name": "Detector", "channels": 2]]]]]]
    }
    static func evidence(plugins: Set<String> = ["source-au", "source-fx", "other-fx", "dependency-fx"], media: [String: String]? = nil,
                         sidechains: Set<String> = ["key"], inputs: Set<String> = [], recording: Bool = false) -> Freeze.RuntimeEvidence {
        Freeze.RuntimeEvidence(availablePluginIDs: plugins, verifiedMediaHashes: media ?? ["/unread/source.wav": sourceHash, "/unread/zone.wav": zoneHash, "/unread/clip.wav": clipHash], validatedSidechainIDs: sidechains, liveInputOwners: inputs, recording: recording)
    }
    static func assets(_ musical: Int64 = 48_000, tail: Int64 = 4_800) -> [Freeze.Asset] {
        [Freeze.Asset(stage: .preFader, path: "/not-created/print.wav", sha256: printHash, frames: musical + tail, trimmedLatencyFrames: 256),
         Freeze.Asset(stage: .preInserts, path: "/not-created/early.wav", sha256: earlyHash, frames: musical + tail, trimmedLatencyFrames: 0)]
    }
    static var verified: [String: String] { ["/not-created/print.wav": printHash, "/not-created/early.wav": earlyHash] }
    static func record(_ project: [String: Any], frames: Int64 = 48_000) throws -> Freeze.Record {
        let plan = try Freeze.prepare(project: project, owner: owner, evidence: evidence())
        return try Freeze.makeRecord(plan: plan, project: project, id: "freeze-0", sampleRate: 48_000, musicalFrames: frames, tailFrames: 4_800, assets: assets(frames))
    }
    static func unavailableInventory(_ assessment: Freeze.PlaybackAssessment) throws -> (statuses: [Freeze.Status], inventory: Set<String>) {
        guard case let .unavailable(statuses, assets) = assessment else { throw Failure(description: "Unverified prints must produce unavailable assessment") }
        let inventory = assets.map { [$0.owner, $0.stage.rawValue, $0.path, $0.expectedSHA256].joined(separator: "|") }
        try check(Set(inventory).count == assets.count, "Unverified inventory must not duplicate stages")
        return (statuses, Set(inventory))
    }
    static func assessmentAndBulkRemoval(_ project: [String: Any], frozen: [String: Any]) throws {
        let otherOwner = "track:t1", otherPath = "/not-created/other.wav", otherHash = String(repeating: "f", count: 64)
        let otherPlan = try Freeze.prepare(project: frozen, owner: otherOwner, evidence: evidence())
        let otherRecord = try Freeze.makeRecord(plan: otherPlan, project: frozen, id: "freeze-1", sampleRate: 48_000,
            musicalFrames: 48_000, tailFrames: 4_800,
            assets: [Freeze.Asset(stage: .preFader, path: otherPath, sha256: otherHash, frames: 52_800, trimmedLatencyFrames: 32)])
        let both = try Freeze.attaching(otherRecord, to: frozen), before = try json(both)
        let firstKey = [owner, "preFader", "/not-created/print.wav", printHash].joined(separator: "|")
        let earlyKey = [owner, "preInserts", "/not-created/early.wav", earlyHash].joined(separator: "|")
        let otherKey = [otherOwner, "preFader", otherPath, otherHash].joined(separator: "|")
        let none = try unavailableInventory(Freeze.assessPlayback(project: both, verifiedAssets: [:]))
        try check(none.statuses.count == 2 && none.statuses.allSatisfy { !$0.needsRefresh }, "Missing printed media must remain distinct from stale source metadata")
        try check(none.inventory == [firstKey, earlyKey, otherKey], "Typed assessment must name every unavailable owner, stage, path and immutable digest")
        let twoMissing = try unavailableInventory(Freeze.assessPlayback(project: both, verifiedAssets: ["/not-created/early.wav": earlyHash]))
        try check(twoMissing.inventory == [firstKey, otherKey], "Two independently missing owner prints must be repairable without a projection")
        var allHashes = verified; allHashes[otherPath] = otherHash
        var partial = allHashes; partial.removeValue(forKey: otherPath)
        try check(try unavailableInventory(Freeze.assessPlayback(project: both, verifiedAssets: partial)).inventory == [otherKey], "Partial verification must return only the unverified stage")
        var mismatch = allHashes; mismatch["/not-created/print.wav"] = sourceHash
        try check(try unavailableInventory(Freeze.assessPlayback(project: both, verifiedAssets: mismatch)).inventory == [firstKey], "Wrong bytes must retain the recorded expected digest in unavailable inventory")
        var prefixOnly = allHashes; prefixOnly[otherPath] = String(otherHash.prefix(16))
        try check(try unavailableInventory(Freeze.assessPlayback(project: both, verifiedAssets: prefixOnly)).inventory == [otherKey], "A matching filename hash prefix cannot establish print availability")
        guard case let .eligible(eligible) = try Freeze.assessPlayback(project: both, verifiedAssets: allHashes) else { throw Failure(description: "Every exact asset hash must produce an eligible projection") }
        let strict = try Freeze.playbackProjection(project: both, verifiedAssets: allHashes)
        try check(try json(eligible.project) == json(strict.project) && eligible.substitutions.count == 2 && eligible.statuses.count == 2, "Eligible assessment must preserve strict projection semantics for both owners")
        try check(eligible.dormantPluginIDs == ["source-au", "source-fx", "other-fx"], "Assessment must preserve exact dormant processor identities")
        guard case let .eligible(stale) = try Freeze.assessPlayback(project: both, verifiedAssets: allHashes, currentMediaHashes: [:]) else { throw Failure(description: "Missing originals must not hide verified prints") }
        try check(stale.statuses.allSatisfy { $0.needsRefresh } && stale.substitutions.count == 2, "Stale original evidence must remain separate from printed asset eligibility")
        try check(try json(both) == before && json(both["audioPlugins"]!) == json(project["audioPlugins"]!), "Assessment must preserve canonical fingerprints, source hashes and opaque plug-in states without file I/O")
        passed("typed missing-print assessment across two owners, partial and mismatched hashes, exact eligibility and independent source staleness")

        var malformed = both
        editTrack(&malformed, 1) { track in var metadata = track["freeze"] as! [String: Any]; metadata["version"] = 2; track["freeze"] = metadata }
        try rejects("Malformed metadata must not be downgraded to unavailable media") { _ = try Freeze.assessPlayback(project: malformed, verifiedAssets: [:]) }
        var changedSource = both; editStep(&changedSource, 0, 0) { $0["note"] = 61 }
        try rejects("Protected source mismatch must remain an error even when every print is missing") { _ = try Freeze.assessPlayback(project: changedSource, verifiedAssets: [:]) }
        try rejects("Strict projection must still reject missing prints") { _ = try Freeze.playbackProjection(project: both, verifiedAssets: [:]) }
        try rejects("Strict projection must still reject a wrong print hash") { _ = try Freeze.playbackProjection(project: both, verifiedAssets: mismatch) }
        var observedOnly = both
        observedOnly["mediaManifest"] = allHashes.map { ["path": $0.key, "sha256": $0.value] }
        try check(try unavailableInventory(Freeze.assessPlayback(project: observedOnly, verifiedAssets: [:])).inventory == none.inventory, "Historical manifest entries cannot substitute for explicit runtime print verification")
        passed("assessment rejects malformed or changed protected metadata and never weakens strict playback or runtime verification")

        let oneRemoved = try Freeze.removing(owners: [owner], from: both)
        var expectedOne = both; editTrack(&expectedOne, 0) { $0.removeValue(forKey: "freeze") }
        try check(try json(oneRemoved) == json(expectedOne), "Bulk removal must change exactly the explicitly selected frozen track")
        try check(try json(Freeze.removing(owner: owner, from: both)) == json(oneRemoved), "Singleton removal must use identical strict bulk semantics")
        try check(try unavailableInventory(Freeze.assessPlayback(project: oneRemoved, verifiedAssets: [:])).inventory == [otherKey], "Removing one missing owner must preserve the remaining owner's unavailable inventory")
        let allRemoved = try Freeze.removing(owners: [owner, otherOwner], from: both)
        try check(try json(allRemoved) == json(project), "Explicit bulk Unfreeze must restore exact canonical content without requiring either missing print")
        guard case let .eligible(unfrozen) = try Freeze.assessPlayback(project: allRemoved, verifiedAssets: [:]) else { throw Failure(description: "A completely unfrozen project needs no printed asset evidence") }
        try check(unfrozen.substitutions.isEmpty && unfrozen.statuses.isEmpty && unfrozen.dormantPluginIDs.isEmpty, "Bulk removal must not leave hidden print substitutions or dormant identities")
        let invalidSets: [Set<String>] = [[], ["track:missing"], ["track:t15"], ["bus:group"], [owner, "track:missing"], [owner, "track:t15"], [owner, otherOwner, "track:missing"]]
        for owners in invalidSets {
            try rejects("Every explicitly selected owner must exist and be frozen") { _ = try Freeze.removing(owners: owners, from: both) }
            try check(try json(both) == before, "Invalid owner sets must reject atomically without removing any valid selected freeze")
        }
        try rejects("Singleton removal of an unfrozen owner must not silently succeed") { _ = try Freeze.removing(owner: "track:t15", from: both) }
        try rejects("Ordinary mutation still cannot remove multiple freeze records") { _ = try Freeze.validateMutation(from: both, to: allRemoved) }
        try check(try json(both) == before && json(allRemoved["audioPlugins"]!) == json(project["audioPlugins"]!), "Bulk removal must preserve the input, original opaque state and every unrelated canonical field")
        passed("atomic exact-owner bulk Unfreeze, strict singleton parity and rejected empty, missing or unfrozen owner sets")
    }
    static func commonCaptureExtent(_ project: [String: Any], frozen: [String: Any]) throws {
        let expected = Freeze.CapturedExtent(sampleRate: 48_000, musicalFrames: 48_000)
        try check(try Freeze.commonCapturedExtent(project: project) == nil, "Unfrozen projects must not acquire a fabricated captured extent")
        try check(try Freeze.commonCapturedExtent(project: frozen) == expected, "One record must preserve its exact frame endpoint")
        let otherOwner = "track:t1", otherPlan = try Freeze.prepare(project: frozen, owner: otherOwner, evidence: evidence())
        func additional(rate: Double, frames: Int64) throws -> [String: Any] {
            let record = try Freeze.makeRecord(plan: otherPlan, project: frozen, id: "second-extent", sampleRate: rate,
                musicalFrames: frames, tailFrames: 9_600,
                assets: [Freeze.Asset(stage: .preFader, path: "/not-created/second-extent.wav", sha256: earlyHash,
                    frames: frames + 9_600, trimmedLatencyFrames: 321)])
            return try Freeze.attaching(record, to: frozen)
        }
        let equal = try additional(rate: 48_000, frames: 48_000), equalBytes = try json(equal)
        try check(try Freeze.commonCapturedExtent(project: equal) == expected, "Different tails and PDC trims must not change the shared musical endpoint")
        try check(try json(equal) == equalBytes, "Common-end admission must not mutate canonical records")
        passed("typed common musical extent, no-record absence and independent per-stage tails without media reads")

        for (rate, frames) in [(96_000.0, Int64(96_000)), (48_000.0, Int64(48_001)), (48_000.0, Int64(47_999))] {
            let mismatch = try additional(rate: rate, frames: frames), original = try json(mismatch)
            try check(try Freeze.validate(project: mismatch).count == 2, "Individually valid inconsistent captures must remain schema-valid for repair")
            try rejects("Equal seconds at another rate or one-frame endpoint drift cannot share a publication") {
                _ = try Freeze.commonCapturedExtent(project: mismatch)
            }
            _ = try unavailableInventory(Freeze.assessPlayback(project: mismatch, verifiedAssets: [:], currentMediaHashes: [:]))
            var renamed = mismatch; renamed["name"] = "Retained repairable project"
            try check(try Freeze.validateMutation(from: mismatch, to: renamed).count == 2, "Ordinary metadata edits must not be blocked by activation-only common-end admission")
            let repaired = try Freeze.removing(owner: otherOwner, from: mismatch)
            try check(try Freeze.commonCapturedExtent(project: repaired) == expected, "Explicit removal must repair a mismatched capture without requiring missing print files")
            try check(try json(mismatch) == original, "Rejected admission and repair proposals must leave the original untouched")
        }
        passed("rate and one-frame drift reject common-end admission while unavailable metadata and explicit repair remain intact")

        var extended = equal
        var clips = extended["audioClips"] as! [[String: Any]]
        clips.append(["id": "unrelated-last", "track": 15, "path": "/not-created/last.wav", "startBeat": 4, "durationBeats": 8])
        extended["audioClips"] = clips
        try check(try Freeze.commonCapturedExtent(project: extended) == expected && Freeze.commonCapturedExtent(project: equal) == expected,
            "Common captured extent alone must not claim to verify live clip extension or subsequent removal; Host canonical-end proof is separate")
        passed("common captured metadata is explicitly separate from canonical source-limited clip duration proof")
    }
    static func durationAdmission(_ original: [String: Any]) throws {
        typealias Duration = AudioFreezeDurationProof
        var project = original
        let livePath = "/not-created/unrelated-live.wav", liveHash = String(repeating: "f", count: 64)
        var clips = project["audioClips"] as! [[String: Any]]
        clips.append(["id": "last-ending-live", "track": 15, "path": livePath, "startBeat": 4, "durationBeats": 8])
        project["audioClips"] = clips
        let capturedClip = Duration.DecodedSource(sha256: clipHash, decodedFrames: 60_000, decodedSampleRate: 48_000)
        let liveClip = Duration.DecodedSource(sha256: liveHash, decodedFrames: 240_000, decodedSampleRate: 48_000)
        // Supplied frame count is synthetic control evidence for model tests,
        // not a claim that this fixture rendered or proved a native song end.
        let proof = try Duration.make(project: project, sampleRate: 48_000, musicalFrames: 48_000,
            resolvedSources: ["/unread/clip.wav": capturedClip, livePath: liveClip])
        let plan = try Freeze.prepare(project: project, owner: owner, evidence: evidence())
        let withProof = try Freeze.makeRecord(plan: plan, project: project, id: "duration-proof", sampleRate: 48_000,
            musicalFrames: 48_000, tailFrames: 4_800, assets: assets(), durationProof: proof.dictionary)
        let frozen = try Freeze.attaching(withProof, to: project), before = try json(frozen)
        func available(_ value: [String: Any], live: [String: Duration.DecodedSource], prints: [String: String] = verified) throws -> Bool {
            if case .eligible = try Freeze.assessActivation(project: value, verifiedAssets: prints, liveSources: live) { return true }
            return false
        }
        try check(try available(frozen, live: [livePath: liveClip]), "Verified prints and exact live evidence must admit absent dormant originals")
        try check(try !available(frozen, live: [:]), "Missing unrelated live clip evidence must reject the entire activation")
        let changed = Duration.DecodedSource(sha256: sourceHash, decodedFrames: liveClip.decodedFrames, decodedSampleRate: liveClip.decodedSampleRate)
        try check(try !available(frozen, live: [livePath: changed]), "Same-length different-content live source must not inherit duration proof")
        let differentDecode = Duration.DecodedSource(sha256: liveHash, decodedFrames: liveClip.decodedFrames + 1, decodedSampleRate: liveClip.decodedSampleRate)
        try check(try !available(frozen, live: [livePath: differentDecode]), "The full hash alone must not excuse a different decoded extent")
        if case let .unavailable(statuses, missing, reasons) = try Freeze.assessActivation(project: frozen, verifiedAssets: [:], liveSources: [livePath: liveClip]) {
            try check(statuses.count == 1 && missing.count == 2 && reasons.isEmpty, "Missing printed stages must remain distinct from valid duration evidence")
        } else { throw Failure(description: "No partial activation may be returned with missing stages") }
        passed("combined activation checks full live source identity and printed stages while absent dormant originals remain usable")

        for change in 0..<4 {
            var mutated = frozen, edited = clips
            if change == 0 { edited.removeLast() }
            if change == 1 { edited[1]["durationBeats"] = 7 }
            if change == 2 { edited[1]["muted"] = true }
            if change == 3 { edited[1]["sourceOffset"] = 0.01 }
            mutated["audioClips"] = edited
            try check(try Freeze.validate(project: mutated).count == 1, "A temporal proof mismatch must remain saveable canonical metadata")
            try check(try !available(mutated, live: [livePath: liveClip]), "Unrelated clip removal/trim/mute/offset change must invalidate activation")
            try rejects("Interactive edits cannot invalidate an admitted temporal proof") { _ = try Freeze.validateMutation(from: frozen, to: mutated) }
        }
        var neutral = frozen, neutralClips = clips
        neutralClips[1]["name"] = "Renamed"; neutralClips[1]["gain"] = 0.25; neutralClips[1]["fadeOut"] = 0.1
        neutral["audioClips"] = neutralClips
        _ = try Freeze.validateMutation(from: frozen, to: neutral)
        try check(try available(neutral, live: [livePath: liveClip]), "Duration-neutral edits to unrelated live clips must remain available")
        try check(try json(frozen) == before, "No assessment or rejected edit may mutate the source project")
        passed("canonical shrink and temporal edits reject activation/edit publication while live gain/fades/name remain editable")

        for raw in [nil, ["version": 99, "path": "/opaque.wav", "nested": ["keep": 1]], ["version": 1, "broken": true], NSNull()] as [Any?] {
            var repairable = frozen
            editTrack(&repairable, 0) { track in
                var metadata = track["freeze"] as! [String: Any]
                metadata["durationProof"] = raw
                track["freeze"] = metadata
            }
            let retained = try Freeze.validate(project: repairable)[0].record
            if let raw { try check(try json(retained.dictionary["durationProof"]!) == json(raw), "Unsupported/malformed opaque proof must round-trip exactly") }
            else { try check(retained.dictionary["durationProof"] == nil, "Old records must not be upgraded by validation") }
            try check(try !available(repairable, live: [livePath: liveClip]), "Unknown, absent or malformed proof must remain activation-unavailable")
            let unfrozen = try Freeze.removing(owner: owner, from: repairable)
            try check(try available(unfrozen, live: [:], prints: [:]), "Explicit Unfreeze must remain possible without understanding proof or verifying missing prints")
        }
        passed("optional and opaque proof round trips preserve repair and explicit Unfreeze without retroactive certification")
    }
    static func main() throws {
        let project = fixture(), originalBytes = try json(project), plan = try Freeze.prepare(project: project, owner: owner, evidence: evidence())
        try check(plan.trackIndex == 0 && plan.requiredStages == [.preFader, .preInserts], "Pre-insert outgoing tap requires a second capture")
        try check(plan.bakedPluginIDs == ["source-au", "source-fx"] && plan.protectedInstrumentIndices == [0, 1], "Capture must resolve explicit owners and per-step instruments")
        try check(plan.requiredPluginIDs == ["source-au", "source-fx", "dependency-fx"], "Capture availability must include exact upstream processors only")
        try check(plan.requiredInstrumentIndices == [0, 1, 2], "Runtime evidence must cover the source and every captured upstream instrument")
        try check(plan.dependencyOwners == ["track:t2", "track:t3", "track:t4"] && plan.incomingRouteIDs == ["incoming", "key"], "Incoming output, send, and sidechain dependencies must be found")
        try check(plan.mediaHashes.count == 3, "Active clip, shared sample and zone require explicit hashes")
        for unavailable in [evidence(recording: true), evidence(inputs: [owner]), evidence(inputs: ["track:t4"]), evidence(plugins: ["source-au", "source-fx"]), evidence(media: [:]), evidence(sidechains: [])] {
            try rejects("Missing runtime source evidence") { _ = try Freeze.prepare(project: project, owner: owner, evidence: unavailable) }
        }
        _ = try Freeze.prepare(project: project, owner: owner, evidence: evidence(plugins: ["source-au", "source-fx", "dependency-fx"], inputs: ["track:t15"]))
        for field in ["missing", "stateError", "state"] {
            var changed = project; editPlugin(&changed, "source-au") { $0[field] = field == "missing" ? true : field == "stateError" ? "unreadable" : ["not": "opaque string"] as Any }
            try rejects("Bad captured source state \(field)") { _ = try Freeze.prepare(project: changed, owner: owner, evidence: evidence()) }
        }
        var external = project; editInstrument(&external, 1) { $0["type"] = "midi" }
        try rejects("External MIDI source") { _ = try Freeze.prepare(project: external, owner: owner, evidence: evidence()) }
        try check(try json(project) == originalBytes, "Preparing must preserve canonical JSON")
        passed("runtime evidence, graph dependencies, source media and external-source rejection without I/O")

        let captured = try record(project), frozen = try Freeze.attaching(captured, to: project), frozenBytes = try json(frozen)
        try check(captured.musicalEndBeat == 2 && captured.assets.count == 2, "Record must derive exact flat-time musical end")
        try check(try json(Freeze.parse(captured.dictionary).dictionary) == json(captured.dictionary), "Strict record round trip")
        try check(try Freeze.validate(project: frozen).count == 1 && !Freeze.validate(project: frozen)[0].needsRefresh, "Fresh record must validate without stale dependencies")
        try check(try json(Freeze.removing(owner: owner, from: frozen)) == originalBytes, "Unfreeze must restore exact canonical content")
        try rejects("Double attach") { _ = try Freeze.attaching(captured, to: frozen) }
        try rejects("Ordinary edit cannot insert freeze metadata") { _ = try Freeze.validateMutation(from: project, to: frozen) }
        var stale = project; editStep(&stale, 0, 0) { $0["note"] = 61 }
        try rejects("Late source edit") { try Freeze.assertUnchanged(plan, project: stale, evidence: evidence()) }
        try rejects("Record from stale source") { _ = try Freeze.makeRecord(plan: plan, project: stale, id: "late", sampleRate: 48_000, musicalFrames: 48_000, tailFrames: 4_800, assets: assets()) }
        try rejects("Missing required early capture") { _ = try Freeze.makeRecord(plan: plan, project: project, id: "partial", sampleRate: 48_000, musicalFrames: 48_000, tailFrames: 4_800, assets: [assets()[0]]) }
        passed("record creation, attach/remove round trip and asynchronous source freshness")

        var oldObserved = project
        let unrelatedObservation: [String: Any] = ["path": "/unrelated.wav", "sha256": earlyHash, "size": 999, "modified": 42]
        let currentObservation: [String: Any] = ["path": "/unread/zone.wav", "sha256": zoneHash, "size": 101, "modified": 24]
        oldObserved["mediaManifest"] = [["path": "/unread/source.wav", "sha256": earlyHash, "size": 123, "modified": 99, "label": "Keep"], currentObservation, unrelatedObservation]
        let oldObservedBytes = try json(oldObserved)
        let freshlyObserved = try Freeze.attaching(captured, to: oldObserved)
        let observations = freshlyObserved["mediaManifest"] as! [[String: Any]]
        try check(observations[0]["sha256"] as? String == sourceHash && observations[0]["size"] == nil && observations[0]["modified"] == nil && observations[0]["label"] as? String == "Keep", "Explicit verified attach updates matching old hashes without retaining old file-stat hints")
        try check(try json(observations[1]) == json(currentObservation) && json(observations[2]) == json(unrelatedObservation), "Current and unrelated observations must remain byte-equivalent")
        try check(try !Freeze.validate(project: freshlyObserved)[0].needsRefresh && json(oldObserved) == oldObservedBytes, "Fresh attach must not inherit stale generic observations or mutate the caller")
        var observedDrift = freshlyObserved
        observedDrift["mediaManifest"] = [["path": "/unread/source.wav", "sha256": sourceHash], ["path": "/unread/source.wav", "sha256": earlyHash]]
        let driftBytes = try json(observedDrift)
        try check(try Freeze.validate(project: observedDrift)[0].needsRefresh && json(observedDrift) == driftBytes, "Ordinary validation detects conflicting observations without refreshing any hashes")
        passed("verified attach refreshes only captured generic observations; ordinary validation never re-certifies changed media")

        let topMutations: [(String, Any)] = [("version", 2), ("version", true), ("scope", "pattern"), ("startBeat", 1), ("sampleRate", 0), ("sampleRate", true), ("musicalFrames", 0), ("musicalFrames", 1.5), ("musicalFrames", 100_000_001), ("tailFrames", -1), ("tailFrames", 2_880_001), ("musicalEndBeat", 0), ("id", "unsafe/id"), ("owner", "bus:group"), ("bakedPluginIDs", ["source-au", "source-au"]), ("protectedInstrumentIndices", [1024]), ("protectedInstrumentIndices", [true]), ("dependencyOwners", [owner])]
        for (key, value) in topMutations { var raw = captured.dictionary; raw[key] = value; try rejects("Schema \(key)=\(value)") { _ = try Freeze.parse(raw) } }
        var unknown = captured.dictionary; unknown["surprise"] = 1; try rejects("Unknown schema field") { _ = try Freeze.parse(unknown) }
        for (key, value) in [("stage", "wrong"), ("path", "../escape.wav"), ("path", "https://example.invalid/a.wav"), ("path", "~/a.wav"), ("path", "relative/./a.wav"), ("path", "/a.mp3"), ("sha256", String(repeating: "A", count: 64)), ("frames", 48_000), ("channels", 1), ("trimmedLatencyFrames", -1), ("insertsBaked", 1)] as [(String, Any)] {
            var raw = captured.dictionary, entries = raw["assets"] as! [[String: Any]]; entries[0][key] = value; raw["assets"] = entries
            try rejects("Asset schema \(key)") { _ = try Freeze.parse(raw) }
        }
        var duplicateStage = captured.dictionary; duplicateStage["assets"] = [assets()[0].dictionary, assets()[0].dictionary]
        try rejects("Duplicate capture stage") { _ = try Freeze.parse(duplicateStage) }
        var relative = captured.dictionary, relativeAssets = relative["assets"] as! [[String: Any]]
        for index in relativeAssets.indices { relativeAssets[index]["path"] = "Portable.media/stage-\(index).wav" }; relative["assets"] = relativeAssets
        _ = try Freeze.parse(relative)
        passed("strict version, fields, integer/Boolean distinctions, frame bounds, PCM paths and stage schema")

        let protectedEdits: [(String, (inout [String: Any]) -> Void)] = [
            ("note", { editStep(&$0, 0, 0) { $0["note"] = 62 } }), ("gate", { editStep(&$0, 0, 0) { $0["gate"] = 0.25 } }),
            ("source effect", { editStep(&$0, 0, 0) { $0["fx2"] = ["type": "ratchet", "value": 3] } }),
            ("source lane", { editLane(&$0, "source-filter") { $0["points"] = [["row": 0, "value": 900]] } }),
            ("shared instrument", { editInstrument(&$0, 0) { $0["volume"] = 0.2 } }),
            ("source plug-in", { editPlugin(&$0, "source-fx") { $0["state"] = "changed opaque patch" } }),
            ("tempo", { $0["bpm"] = 130 }), ("sequence", { $0["sequence"] = [0, 0] }),
            ("source clip", { var clips = $0["audioClips"] as! [[String: Any]]; clips[0]["sourceOffset"] = 0.2; $0["audioClips"] = clips }),
            ("frozen record", { editTrack(&$0, 0) { $0.removeValue(forKey: "freeze") } })]
        for (name, edit) in protectedEdits { var changed = try copy(frozen); edit(&changed); try rejects(name) { _ = try Freeze.validateMutation(from: frozen, to: changed) } }
        let liveEdits: [(String, (inout [String: Any]) -> Void)] = [
            ("live strip", { editTrack(&$0, 0) { $0["volume"] = 0.25; $0["pan"] = -0.5; $0["delaySend"] = 0.7; $0["reverbSend"] = 0.8; $0["mute"] = true; $0["solo"] = true } }),
            ("live mix row", { editStep(&$0, 0, 0) { $0["fx1"] = ["type": "volume", "value": 0.2] } }),
            ("live mix lane", { editLane(&$0, "source-volume") { $0["points"] = [["row": 0, "value": 0.2]] } }),
            ("other processor", { editPlugin(&$0, "other-fx") { $0["state"] = "edited downstream" } }),
            ("unused instrument", { editInstrument(&$0, 63) { $0["waveform"] = "square" } }),
            ("cosmetic", { editTrack(&$0, 0) { $0["name"] = "Renamed" }; $0["name"] = "New project title" })]
        for (name, edit) in liveEdits { var changed = try copy(frozen); edit(&changed); let statuses = try Freeze.validateMutation(from: frozen, to: changed); try check(!statuses[0].needsRefresh, "Independent downstream change should remain fresh: \(name)") }
        var upstream = try copy(frozen); editTrack(&upstream, 2) { $0["volume"] = 0.2 }
        try check(try Freeze.validateMutation(from: frozen, to: upstream)[0].needsRefresh, "Upstream strip edit should mark print stale, not destroy it")
        var upstreamState = try copy(frozen); editPlugin(&upstreamState, "dependency-fx") { $0["state"] = "new upstream state" }
        try check(try Freeze.validateMutation(from: frozen, to: upstreamState)[0].needsRefresh, "Upstream opaque patch should mark dependencies stale")
        var upstreamMIDI = try copy(frozen); editInstrument(&upstreamMIDI, 2) { $0["type"] = "midi" }
        try check(try Freeze.validateMutation(from: frozen, to: upstreamMIDI)[0].needsRefresh, "An upstream MIDI change should stale the retained print without making it unavailable")
        var staleAttach = project; editTrack(&staleAttach, 2) { $0["volume"] = 0.2 }
        try rejects("Attach with stale dependency") { _ = try Freeze.attaching(captured, to: staleAttach) }
        passed("protected source edits versus editable live mix, unrelated content and independently stale upstream dependencies")

        var earlyDependency = project, earlyRouting = project["routing"] as! [String: Any]
        var earlyOutputs = earlyRouting["outputs"] as! [String: String]; earlyOutputs["track:t2"] = "master"; earlyRouting["outputs"] = earlyOutputs
        var earlySends = earlyRouting["sends"] as! [[String: Any]]
        earlySends.append(["id": "upstream-early", "source": "track:t2", "destination": owner, "tap": "preInserts", "gain": 0.5]); earlyRouting["sends"] = earlySends; earlyDependency["routing"] = earlyRouting
        let earlyPlan = try Freeze.prepare(project: earlyDependency, owner: owner, evidence: evidence(plugins: ["source-au", "source-fx"]))
        try check(!earlyPlan.requiredPluginIDs.contains("dependency-fx"), "Later inserts on an upstream early tap must not be required")
        let earlyFrozen = try Freeze.attaching(record(earlyDependency), to: earlyDependency)
        var afterEarlyTap = try copy(earlyFrozen)
        editTrack(&afterEarlyTap, 2) { $0["volume"] = 0.1; $0["pan"] = -0.8 }
        editPlugin(&afterEarlyTap, "dependency-fx") { $0["state"] = "uncaptured later insert" }
        try check(try !Freeze.validateMutation(from: earlyFrozen, to: afterEarlyTap)[0].needsRefresh, "Post-tap mix and insert changes cannot stale an early capture")
        editStep(&afterEarlyTap, 2, 0) { $0["note"] = 49 }
        try check(try Freeze.validateMutation(from: earlyFrozen, to: afterEarlyTap)[0].needsRefresh, "Pre-tap source changes must stale the dependent print")
        passed("pre-insert upstream dependency excludes later fader and insert state but tracks captured source edits")

        try rejects("Unverified print") { _ = try Freeze.playbackProjection(project: frozen, verifiedAssets: [:]) }
        var wrong = verified; wrong["/not-created/print.wav"] = sourceHash
        try rejects("Wrong print digest") { _ = try Freeze.playbackProjection(project: frozen, verifiedAssets: wrong) }
        let projection = try Freeze.playbackProjection(project: frozen, verifiedAssets: verified), projected = projection.project, projectedRows = rows(projected)
        try check((projected["tracks"] as! [[String: Any]]).count == 16 && (projected["instruments"] as! [[String: Any]]).count == 64, "Freeze cannot consume an extra track or instrument")
        try check(try json(projected["instruments"]!) == json(project["instruments"]!), "Shared instrument and dormant descriptor definitions must remain exact")
        try check(projection.dormantPluginIDs == ["source-au", "source-fx"], "Only captured processors may be unloaded")
        let projectedPlugins = projected["audioPlugins"] as! [[String: Any]], canonicalPlugins = project["audioPlugins"] as! [[String: Any]]
        try check(try json(projectedPlugins) == json(canonicalPlugins.filter { ["other-fx", "dependency-fx"].contains($0["instanceID"] as! String) }), "Live processor identities and opaque state must remain byte-exact")
        try check((projected["audioClips"] as! [[String: Any]]).isEmpty, "Captured source clips must not run alongside the private print")
        try check(try json(projectedRows[0][0]) == json(["fx1": ["type": "volume", "value": 0.5]]), "Source note and filter should be removed while live volume remains")
        try check(try json(projectedRows[0][1]) == json(["fx2": ["type": "pan", "value": 0.2]]), "Baked insert lock should be removed while live pan remains")
        try check(try json(projectedRows[0][2]) == json(["fx1": ["type": "delay", "value": 0.3], "fx2": ["type": "reverb", "value": 0.4]]), "Control-only send FX must stay live")
        try check(projectedRows[0][3] is NSNull && projectedRows[0][4] is NSNull && projectedRows[0][5] is NSNull, "Foreign/stale locks and native-ignored note-off/cut mix FX must not become live")
        let otherRow = projectedRows[1][0] as! [String: Any], unavailableRow = projectedRows[1][1] as! [String: Any]
        try check(try json(otherRow["fx1"]!) == json(lock("other-fx")) && otherRow["fx2"] == nil, "Exact live lock stays; foreign frozen-instance lock must remain inert")
        try check(unavailableRow["fx1"] == nil && unavailableRow["fx2"] != nil, "Same-slot unavailable ID must never infer the live replacement")
        let liveLaneIDs = Set(((projected["patterns"] as! [[String: Any]])[0]["automation"] as! [[String: Any]]).map { $0["id"] as! String })
        try check(liveLaneIDs == ["source-volume", "source-pan", "source-delay", "source-reverb", "other-insert", "early-gain"], "Source and disconnected incoming lanes should be removed, live strip/output lanes retained")
        let substitution = projection.substitutions[0], routing = projected["routing"] as! [String: Any]
        try check(substitution.trackIndex == 0 && substitution.removeNativeSource && substitution.removeIncomingRouteIDs == ["incoming", "key"] && substitution.assets.count == 2, "Host must receive complete explicit graph substitution instructions")
        try check((routing["outputs"] as! [String: Any])["track:t2"] is NSNull && (routing["outputs"] as! [String: Any])[owner] as? String == "bus:group", "Disconnect incoming output but preserve downstream routing")
        try check((routing["sends"] as! [[String: Any]]).map { $0["id"] as! String } == ["early"] && (routing["sidechains"] as! [[String: Any]]).isEmpty, "Incoming connections must not be double counted")
        try check(try json(frozen) == frozenBytes && json(project) == originalBytes, "Projection must never mutate canonical JSON")
        try check(try json(Freeze.playbackProjection(project: frozen, verifiedAssets: verified).project) == json(projected), "Repeated canonical projection must be deterministic")
        passed("verified-only projection, sixteen tracks, shared instruments, exact live locks, native note-off semantics and incoming graph substitution")

        let other = canonicalPlugins.first { $0["instanceID"] as? String == "other-fx" }!
        var updated = other; updated["state"] = "new live state"; updated["latencyFrames"] = 64
        let merged = try Freeze.mergingSnapshot([updated], into: frozen), mergedPlugins = merged["audioPlugins"] as! [[String: Any]]
        try check(mergedPlugins.first { $0["instanceID"] as? String == "other-fx" }?["state"] as? String == "new live state", "Merge must capture a matching live processor")
        try check(try json(mergedPlugins.filter { projection.dormantPluginIDs.contains($0["instanceID"] as! String) }) == json(canonicalPlugins.filter { projection.dormantPluginIDs.contains($0["instanceID"] as! String) }), "Merge must retain all dormant opaque source descriptors")
        try check(mergedPlugins.count == canonicalPlugins.count, "Missing live snapshot entries must not delete desired descriptors")
        try rejects("Dormant snapshot") { _ = try Freeze.mergingSnapshot([canonicalPlugins[0]], into: frozen) }
        try rejects("Duplicate snapshot ID") { _ = try Freeze.mergingSnapshot([updated, updated], into: frozen) }
        var missingState = updated; missingState.removeValue(forKey: "state")
        try rejects("Missing snapshot state") { _ = try Freeze.mergingSnapshot([missingState], into: frozen) }
        for (key, value) in [("instanceID", "stale"), ("owner", owner), ("kind", "instrument"), ("slot", 3), ("component", "replacement"), ("stateError", "capture failed")] as [(String, Any)] {
            var bad = updated; bad[key] = value; try rejects("Snapshot \(key)") { _ = try Freeze.mergingSnapshot([bad], into: frozen) }
        }
        passed("canonical snapshot merge preserves dormant states and rejects stale, moved, duplicate or unreadable instances")


        func relocated(_ value: Any) -> Any {
            if let dictionary = value as? [String: Any] {
                return dictionary.reduce(into: [String: Any]()) { output, item in
                    if ["path", "samplePath"].contains(item.key), let path = item.value as? String {
                        output[item.key] = path.replacingOccurrences(of: "/unread/", with: "Session.media/").replacingOccurrences(of: "/not-created/", with: "Session.media/")
                    } else { output[item.key] = relocated(item.value) }
                }
            }
            if let array = value as? [Any] { return array.map(relocated) }
            return value
        }
        let portable = relocated(frozen) as! [String: Any]
        try check(try !Freeze.validate(project: portable)[0].needsRefresh, "Portable media path rewriting must preserve content-based freshness")
        let portableProjection = try Freeze.playbackProjection(project: portable, verifiedAssets: ["Session.media/print.wav": printHash, "Session.media/early.wav": earlyHash])
        try check(portableProjection.substitutions[0].assets.allSatisfy { $0.path.hasPrefix("Session.media/") }, "Projection must use the relocated private capture paths")
        try check(try Freeze.validate(project: frozen, currentMediaHashes: [:])[0].needsRefresh, "Missing observed original media must mark the retained print stale")
        let currentMedia = ["/unread/source.wav": sourceHash, "/unread/zone.wav": zoneHash, "/unread/clip.wav": clipHash]
        try check(try !Freeze.validate(project: frozen, currentMediaHashes: currentMedia)[0].needsRefresh, "Matching observed original hashes should remain fresh")
        var changedMedia = currentMedia; changedMedia["/unread/clip.wav"] = sourceHash
        try check(try Freeze.validate(project: frozen, currentMediaHashes: changedMedia)[0].needsRefresh, "Changed observed original bytes should mark the retained print stale")
        let staleMediaProjection = try Freeze.playbackProjection(project: frozen, verifiedAssets: verified, currentMediaHashes: [:])
        try check(staleMediaProjection.statuses[0].needsRefresh && staleMediaProjection.substitutions.count == 1, "Verified print remains available when original media is missing")
        passed("portable content hashes and explicit current media evidence preserve prints while reporting stale originals")

        try assessmentAndBulkRemoval(project, frozen: frozen)
        try commonCaptureExtent(project, frozen: frozen)
        try durationAdmission(project)

        var legacy = project; legacy.removeValue(forKey: "routing")
        try check(try Freeze.validate(project: legacy).isEmpty, "Unfrozen legacy validation must pass through")
        try check(try json(Freeze.playbackProjection(project: legacy, verifiedAssets: [:]).project) == json(legacy), "Unfrozen legacy projection must stay byte-equivalent")
        try rejects("Legacy capture") { _ = try Freeze.prepare(project: legacy, owner: owner, evidence: evidence()) }
        passed("unfrozen legacy projects pass through unchanged while capture requires explicit standard routing")
        print("Audio freeze project: \(groups) pure-policy regression groups passed; no files, plug-ins, device or audio graph opened.")
    }
}

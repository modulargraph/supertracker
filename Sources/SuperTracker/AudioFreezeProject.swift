import Foundation
import CoreFoundation
import CryptoKit

/// Pure control-thread freeze policy. This does not render, read media, instantiate
/// plug-ins, or make a saved freeze playable. The host must verify runtime evidence
/// and atomically install every graph substitution in PlaybackProjection.
enum AudioFreezeProject {
    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { "Track freeze: " + message }
    }
    enum Stage: String, CaseIterable { case preFader, preInserts }
    struct RuntimeEvidence {
        let availablePluginIDs: Set<String>
        let verifiedMediaHashes: [String: String]
        let validatedSidechainIDs: Set<String>
        let liveInputOwners: Set<String>
        let recording: Bool
    }
    /// A file identity established externally by bounded, full SHA256 reading.
    /// A path/name match or a user's different-content override is insufficient.
    struct MediaRelocation { let from: String, to: String, sha256: String }
    struct Fingerprints: Equatable {
        let source: String, timing: String, arrangement: String, dependencies: String
        var dictionary: [String: Any] { ["source": source, "timing": timing, "arrangement": arrangement, "dependencies": dependencies] }
    }
    struct CapturePlan {
        let owner: String, trackIndex: Int
        let requiredStages: Set<Stage>
        let bakedPluginIDs: Set<String>
        let requiredPluginIDs: Set<String>
        let protectedInstrumentIndices: Set<Int>
        let requiredInstrumentIndices: Set<Int>
        let dependencyOwners: Set<String>
        let incomingRouteIDs: Set<String>
        let fingerprints: Fingerprints
        let mediaHashes: [String: String]
    }
    struct Asset {
        let stage: Stage, path: String, sha256: String
        let frames: Int64, trimmedLatencyFrames: Int64
        var dictionary: [String: Any] {
            ["stage": stage.rawValue, "path": path, "sha256": sha256, "frames": frames,
             "channels": 2, "trimmedLatencyFrames": trimmedLatencyFrames,
             "insertsBaked": stage == .preFader]
        }
    }
    struct Record {
        let id: String, owner: String
        let sampleRate: Double, musicalEndBeat: Double
        let musicalFrames: Int64, tailFrames: Int64
        let assets: [Asset]
        let bakedPluginIDs: Set<String>, protectedInstrumentIndices: Set<Int>, dependencyOwners: Set<String>
        let fingerprints: Fingerprints, mediaHashes: [String: String]
        /// Preserve absent, unsupported and malformed bounded proof as canonical
        /// intent. Only the explicit activation gate interprets its validity.
        let durationProof: Any?
        var dictionary: [String: Any] {
            var result: [String: Any] = ["version": 1, "id": id, "owner": owner, "scope": "song", "startBeat": 0,
             "sampleRate": sampleRate, "musicalEndBeat": musicalEndBeat,
             "musicalFrames": musicalFrames, "tailFrames": tailFrames,
             "assets": assets.sorted { $0.stage.rawValue < $1.stage.rawValue }.map(\.dictionary),
             "bakedPluginIDs": bakedPluginIDs.sorted(), "protectedInstrumentIndices": protectedInstrumentIndices.sorted(),
             "dependencyOwners": dependencyOwners.sorted(), "fingerprints": fingerprints.dictionary,
             "media": mediaHashes.keys.sorted().map { ["path": $0, "sha256": mediaHashes[$0]!] }]
            if let durationProof { result["durationProof"] = durationProof }
            return result
        }
    }
    struct Status {
        let record: Record
        /// An upstream edit leaves the old print intact, but must be surfaced as
        /// stale. Protected source/timing/identity edits instead throw.
        let needsRefresh: Bool
    }
    /// A common captured endpoint, not proof of today's canonical song duration.
    /// Tail lengths may differ: the native natural-end transition uses only this
    /// musical extent, and each printed stage owns its subsequent tail.
    struct CapturedExtent: Equatable {
        let sampleRate: Double
        let musicalFrames: Int64
    }
    struct UnverifiedAsset: Equatable {
        let owner: String, stage: Stage, path: String, expectedSHA256: String
    }
    /// Pure metadata admission, never a claim that PCM or a graph is ready.
    /// Unavailable intent remains canonical and can be saved or relinked while
    /// the host keeps playback stopped. No partial projection is returned.
    enum PlaybackAssessment {
        case eligible(PlaybackProjection)
        case unavailable(statuses: [Status], assets: [UnverifiedAsset])
    }
    /// Future Host activation must use this combined gate, not the metadata-only
    /// PlaybackAssessment/projection helpers. Eligible still requires Host graph,
    /// callback and publication checks; no runtime is enabled by this model.
    enum ActivationAssessment {
        case eligible(projection: PlaybackProjection, extent: CapturedExtent?)
        case unavailable(statuses: [Status], assets: [UnverifiedAsset], reasons: [String])
    }
    struct Substitution {
        let owner: String, trackIndex: Int, assets: [Asset]
        let removeIncomingRouteIDs: Set<String>
        /// Host must disconnect input->inserts->preFader, inject printed preFader,
        /// and inject preInserts only for outgoing early taps. Never pass the early
        /// capture through live inserts: all selected inserts are unloaded.
        let removeNativeSource = true
    }
    struct PlaybackProjection {
        let project: [String: Any]
        let substitutions: [Substitution]
        let dormantPluginIDs: Set<String>
        let statuses: [Status]
    }

    private static let mixTargets: Set<String> = ["volume", "pan", "delay", "reverb"]
    private static let sourceTargets: Set<String> = ["cutoff", "resonance", "pitch", "wavetablePosition", "attack", "decay", "sustain", "release", "drive", "bitDepth", "lfoRate", "lfoDepth", "start", "end", "loopStart", "loopEnd", "grainSize", "grainPosition", "sliceCount", "instrumentVolume", "instrumentPan", "tune", "filterAttack", "filterDecay", "filterSustain", "filterRelease", "filterEnvAmount", "lfo2Rate", "lfo2Depth", "lfoBeats", "lfo2Beats", "grainDensity", "grainSpray", "grainPitchSpread", "grainScan", "grainReverse", "grainStereo"]
    private static let cosmeticKeys: Set<String> = ["name", "color", "selected", "collapsed", "editorOpen", "windowFrame", "viewState"]
    private static let maximumFrames: Int64 = 100_000_000

    /// Availability is explicitly provided by the host after authoritative state
    /// capture. Desired descriptors alone never establish that a plug-in or file
    /// is available. The evidence should be rechecked at async completion.
    static func prepare(project: [String: Any], owner: String, evidence: RuntimeEvidence) throws -> CapturePlan {
        guard !evidence.recording else { throw fail("Finish recording before freezing.") }
        let model = try Model(project)
        let plan = try model.plan(owner: owner, mediaHashes: evidence.verifiedMediaHashes)
        let participating = plan.dependencyOwners.union([owner])
        guard participating.isDisjoint(with: evidence.liveInputOwners) else { throw fail("Record live inputs before freezing their dependent tracks.") }
        for index in plan.requiredInstrumentIndices where model.instruments[index]["type"] as? String == "midi" {
            throw fail("Record external MIDI instruments before freezing their dependent tracks.")
        }
        let requiredPlugins = model.plugins.filter { plan.requiredPluginIDs.contains($0.id) }
        for plugin in requiredPlugins {
            guard evidence.availablePluginIDs.contains(plugin.id), plugin.value["missing"] as? Bool != true,
                  plugin.value["stateError"] == nil, plugin.value["state"] is String else {
                throw fail("Capture a valid, available state for plug-in \(plugin.id) before freezing.")
            }
        }
        for route in model.definition.sidechains where route.enabled && plan.incomingRouteIDs.contains(route.id) {
            guard evidence.validatedSidechainIDs.contains(route.id) else { throw fail("Verify the actual input layout for sidechain \(route.id) before freezing.") }
        }
        return plan
    }

    /// No object identity or UI revision is hashed. Stable routing/instance IDs,
    /// musical content, and opaque plug-in state are hashed. Host identity and
    /// cancellation tokens remain additional transaction guards.
    static func assertUnchanged(_ plan: CapturePlan, project: [String: Any], evidence: RuntimeEvidence) throws {
        let current = try prepare(project: project, owner: plan.owner, evidence: evidence)
        guard current.trackIndex == plan.trackIndex, current.fingerprints == plan.fingerprints,
              current.requiredStages == plan.requiredStages, current.bakedPluginIDs == plan.bakedPluginIDs else {
            throw fail("The captured source or its dependencies changed. Discard this render and freeze again.")
        }
    }

    static func makeRecord(plan: CapturePlan, project: [String: Any], id: String,
                           sampleRate: Double, musicalFrames: Int64, tailFrames: Int64, assets: [Asset],
                           durationProof: Any? = nil) throws -> Record {
        let current = try Model(project).plan(owner: plan.owner, mediaHashes: plan.mediaHashes)
        guard current.fingerprints == plan.fingerprints, current.requiredStages == plan.requiredStages else {
            throw fail("The source changed before the render was committed.")
        }
        let end = try endingBeat(project: project, sampleRate: sampleRate, frames: musicalFrames)
        let value = Record(id: id, owner: plan.owner, sampleRate: sampleRate, musicalEndBeat: end,
            musicalFrames: musicalFrames, tailFrames: tailFrames, assets: assets,
            bakedPluginIDs: plan.bakedPluginIDs, protectedInstrumentIndices: plan.protectedInstrumentIndices,
            dependencyOwners: plan.dependencyOwners, fingerprints: plan.fingerprints, mediaHashes: plan.mediaHashes,
            durationProof: durationProof)
        let record = try parse(value.dictionary)
        guard Set(record.assets.map(\.stage)) == plan.requiredStages else { throw fail("The render does not contain every required capture stage.") }
        return record
    }

    static func attaching(_ record: Record, to project: [String: Any]) throws -> [String: Any] {
        var result = try copy(project), tracks = try objects(result["tracks"], "tracks")
        guard let index = tracks.firstIndex(where: { "track:" + ($0["routingID"] as? String ?? "") == record.owner }),
              tracks[index]["freeze"] == nil else { throw fail("The target track is missing or already frozen.") }
        tracks[index]["freeze"] = record.dictionary; result["tracks"] = tracks
        // Only an explicit successful capture may refresh older generic source
        // observations. The record carries the full hashes established by its
        // RuntimeEvidence; unrelated entries and canonical source data stay put.
        if let manifest = result["mediaManifest"] as? [[String: Any]] {
            result["mediaManifest"] = manifest.map { entry -> [String: Any] in
                guard let path = entry["path"] as? String, let hash = record.mediaHashes[path], entry["sha256"] as? String != hash else { return entry }
                var refreshed = entry
                refreshed["sha256"] = hash
                refreshed.removeValue(forKey: "size"); refreshed.removeValue(forKey: "modified")
                return refreshed
            }
        }
        let statuses = try validate(project: result)
        guard statuses.first(where: { $0.record.id == record.id })?.needsRefresh == false else { throw fail("The captured dependencies changed before publication.") }
        return result
    }

    /// Pure canonical edit; the coordinator must prepare original processors and
    /// only publish this result after the live graph can replace the print.
    static func removing(owner: String, from project: [String: Any]) throws -> [String: Any] {
        try removing(owners: [owner], from: project)
    }

    /// Remove exactly the explicitly selected records in one canonical edit.
    /// The coordinator must restore all selected endpoints before publication.
    static func removing(owners: Set<String>, from project: [String: Any]) throws -> [String: Any] {
        let frozen = Set(try validate(project: project).map { $0.record.owner })
        guard !owners.isEmpty, owners.isSubset(of: frozen) else {
            throw fail("Select one or more existing frozen tracks to Unfreeze; no unrelated track is included automatically.")
        }
        var result = try copy(project), tracks = try objects(result["tracks"], "tracks")
        for index in tracks.indices where owners.contains("track:" + (tracks[index]["routingID"] as? String ?? "")) {
            tracks[index].removeValue(forKey: "freeze")
        }
        result["tracks"] = tracks; return result
    }

    /// Schema and source validation only. A successful result does not establish
    /// asset readability/hash equality or make the record safe to play.
    static func validate(project: [String: Any], currentMediaHashes: [String: String]? = nil) throws -> [Status] {
        guard try objects(project["tracks"], "tracks").contains(where: { $0["freeze"] != nil }) else { return [] }
        let model = try Model(project)
        var observedMedia: [String: Set<String>] = [:]
        if currentMediaHashes == nil {
            for entry in project["mediaManifest"] as? [[String: Any]] ?? [] {
                guard let path = entry["path"] as? String, let hash = try? digest(entry["sha256"]) else { continue }
                observedMedia[path, default: []].insert(hash)
            }
        }
        var ids = Set<String>(), statuses: [Status] = []
        for track in model.tracks {
            guard let raw = track["freeze"] else { continue }
            guard let object = raw as? [String: Any] else { throw fail("Freeze metadata must be an object.") }
            let record = try parse(object)
            guard ids.insert(record.id).inserted, record.owner == "track:" + (track["routingID"] as? String ?? "") else { throw fail("Freeze identity does not match its track.") }
            let current = try model.plan(owner: record.owner, mediaHashes: record.mediaHashes, allowMissingDependencyMedia: true)
            guard record.bakedPluginIDs == current.bakedPluginIDs,
                  record.protectedInstrumentIndices == current.protectedInstrumentIndices,
                  record.fingerprints.source == current.fingerprints.source,
                  record.fingerprints.timing == current.fingerprints.timing,
                  record.fingerprints.arrangement == current.fingerprints.arrangement,
                  current.requiredStages.isSubset(of: Set(record.assets.map(\.stage))) else {
                throw fail("Unfreeze \(record.owner) before changing its source, timing, arrangement, instrument definitions, or captured stage requirements.")
            }
            let end = try endingBeat(project: project, sampleRate: record.sampleRate, frames: record.musicalFrames)
            guard abs(end - record.musicalEndBeat) < 0.000_001 else { throw fail("Freeze musical end does not match its exact frame count and timing.") }
            // A partial relink may retain old aliases while another reference
            // still uses them. Only currently referenced evidence affects status.
            let changedMedia = currentMediaHashes.map { hashes in current.mediaHashes.contains { hashes[$0.key] != $0.value } } ?? current.mediaHashes.contains { path, capturedHash in
                // A saved manifest is partial historical evidence, not proof of
                // current availability. Known different bytes still make a print
                // stale; missing observations need runtime verification later.
                observedMedia[path]?.contains(where: { $0 != capturedHash }) == true
            }
            statuses.append(Status(record: record, needsRefresh: changedMedia || record.fingerprints.dependencies != current.fingerprints.dependencies || record.dependencyOwners != current.dependencyOwners))
        }
        return statuses
    }

    /// Required preflight for a future frozen playback publication. Keep this
    /// separate from schema validation, Save and relinking: inconsistent captures
    /// must remain recoverable canonical intent, never a partially active graph.
    ///
    /// Equality does NOT establish the current canonical end. Before activation,
    /// the Host must separately prove that the sequence and source-length-limited
    /// clip ends still equal this extent and that the projected live end fits.
    /// For example, deleting an unrelated last-ending clip can shorten the song
    /// without changing these records. Missing dormant originals need previously
    /// verified captured-end evidence; this helper performs no media I/O and must
    /// not substitute a record's whole-song extent for an individual clip's end.
    static func commonCapturedExtent(project: [String: Any]) throws -> CapturedExtent? {
        let statuses = try validate(project: project)
        guard let first = statuses.first?.record else { return nil }
        let extent = CapturedExtent(sampleRate: first.sampleRate, musicalFrames: first.musicalFrames)
        for status in statuses.dropFirst() {
            let record = status.record
            guard record.sampleRate == extent.sampleRate, record.musicalFrames == extent.musicalFrames else {
                throw fail("Frozen tracks have different capture sample rates or song endpoints. Refresh or Unfreeze the differing captures before playback.")
            }
        }
        return extent
    }

    /// Extend captured source evidence only for verified identical bytes after a
    /// reference-level relink. Retain old aliases for partial/shared relinks, never
    /// replace recorded hashes or regenerate musical/source fingerprints. Printed
    /// stage paths may move only with the stage's exact full hash.
    static func preservingRelocatedMedia(from original: [String: Any], to updated: [String: Any],
                                         relocations: [MediaRelocation]) throws -> [String: Any] {
        let statuses = try validate(project: original)
        if statuses.isEmpty { return updated }
        for relocation in relocations { _ = try mediaPath(relocation.from); _ = try mediaPath(relocation.to); _ = try digest(relocation.sha256) }
        var result = try copy(updated), tracks = try objects(updated["tracks"], "tracks")
        let originalTracks = try objects(original["tracks"], "tracks")
        let oldClips = try objects(original["audioClips"], "audio clips"), newClips = try objects(updated["audioClips"], "audio clips")
        var proofMoves: [AudioFreezeDurationProof.Relocation] = []
        // Relinking an inactive take must not recertify the selected source. A
        // linked path/samplePath action may start from an equivalent alias, so
        // bind its verified target to the actual selected old/new clip paths.
        for (old, new) in zip(oldClips, newClips) where !(old["muted"] as? Bool ?? old["mute"] as? Bool ?? false) {
            guard let from = old["path"] as? String ?? old["samplePath"] as? String,
                  let to = new["path"] as? String ?? new["samplePath"] as? String,
                  let verified = relocations.first(where: { $0.to == to && (from != to || $0.from == from) }) else { continue }
            proofMoves.append(AudioFreezeDurationProof.Relocation(from: from, to: to, sha256: verified.sha256))
        }
        for status in statuses {
            let record = status.record
            guard let oldTrack = originalTracks.first(where: { "track:" + ($0["routingID"] as? String ?? "") == record.owner }),
                  let index = tracks.firstIndex(where: { "track:" + ($0["routingID"] as? String ?? "") == record.owner }),
                  var expected = oldTrack["freeze"] as? [String: Any], let actual = tracks[index]["freeze"] as? [String: Any] else {
                throw fail("Relinking cannot remove a frozen track or its capture record.")
            }
            var assets = try objects(expected["assets"], "freeze assets")
            let changedAssets = try objects(actual["assets"], "freeze assets")
            guard assets.count == changedAssets.count else { throw fail("Relinking cannot change capture stages.") }
            for i in assets.indices {
                let oldPath = assets[i]["path"] as? String ?? "", path = changedAssets[i]["path"] as? String ?? ""
                if oldPath != path {
                    guard relocations.contains(where: { $0.from == oldPath && $0.to == path && $0.sha256 == assets[i]["sha256"] as? String }) else {
                        throw fail("A frozen stage can only move to verified identical content.")
                    }
                    assets[i]["path"] = path
                }
            }
            expected["assets"] = assets
            guard try encoded(expected) == encoded(actual) else { throw fail("Relinking cannot re-certify freeze metadata or change its fingerprints.") }
            let updatedModel = try Model(result)
            let protectedPaths = try updatedModel.paths(owners: [record.owner], instruments: record.protectedInstrumentIndices)
            for relocation in relocations where protectedPaths.contains(relocation.to) {
                if let capturedHash = record.mediaHashes[relocation.to] ?? record.mediaHashes[relocation.from], capturedHash != relocation.sha256 {
                    throw fail("Unfreeze before replacing a captured source with different content, even when its path is unchanged.")
                }
            }
            var aliases = record.mediaHashes
            for relocation in relocations where record.mediaHashes[relocation.from] == relocation.sha256 {
                if let existing = aliases[relocation.to], existing != relocation.sha256 { throw fail("Relocation conflicts with a recorded source hash.") }
                aliases[relocation.to] = relocation.sha256
            }
            // Only add aliases used by the captured sources/dependencies. Relinking
            // an inactive alternative take must not modify the capture record.
            let current = try updatedModel.plan(owner: record.owner, mediaHashes: aliases, allowMissingDependencyMedia: true)
            for (path, hash) in current.mediaHashes { aliases[path] = hash }
            let usedNewPaths = Set(current.mediaHashes.keys)
            aliases = aliases.filter { record.mediaHashes[$0.key] != nil || usedNewPaths.contains($0.key) }
            expected["media"] = aliases.keys.sorted().map { ["path": $0, "sha256": aliases[$0]!] }
            if case .supported(let proof) = try AudioFreezeDurationProof.assess(record.durationProof) {
                for move in proofMoves {
                    for source in proof.sources where source.paths.contains(move.from) {
                        guard source.decoded.sha256 == move.sha256 else {
                            throw fail("Unfreeze before replacing a song-duration source with different content.")
                        }
                    }
                }
            }
            if let proof = try AudioFreezeDurationProof.relocating(record.durationProof, relocations: proofMoves) {
                expected["durationProof"] = proof
            }
            tracks[index]["freeze"] = expected
        }
        result["tracks"] = tracks
        _ = try validate(project: result)
        return result
    }

    /// Existing records cannot be silently replaced during an ordinary edit.
    /// Explicit attach/remove transactions use the dedicated methods above.
    static func validateMutation(from previous: [String: Any], to next: [String: Any]) throws -> [Status] {
        let oldTracks = try objects(previous["tracks"], "tracks"), newTracks = try objects(next["tracks"], "tracks")
        let oldFrozen = Set(oldTracks.filter { $0["freeze"] != nil }.compactMap { $0["routingID"] as? String })
        let newFrozen = Set(newTracks.filter { $0["freeze"] != nil }.compactMap { $0["routingID"] as? String })
        guard oldFrozen == newFrozen else { throw fail("Use an explicit Freeze or Unfreeze transaction to change frozen tracks.") }
        for track in oldTracks where track["freeze"] != nil {
            let id = track["routingID"] as? String
            guard let same = newTracks.first(where: { $0["routingID"] as? String == id }), let after = same["freeze"],
                  try encoded(track["freeze"]!) == encoded(after) else { throw fail("Use Unfreeze before removing a frozen track or changing its freeze record.") }
            guard let metadata = track["freeze"] as? [String: Any] else { throw fail("Freeze metadata must be an object.") }
            let record = try parse(metadata)
            if case .supported(let proof) = try AudioFreezeDurationProof.assess(record.durationProof) {
                // Historical-only comparison for the edit lock, NOT runtime
                // admission. Actual activation must verify every live source.
                let owners = Set(oldTracks.compactMap { ($0["routingID"] as? String).map { "track:" + $0 } })
                var previouslyMatched = false
                do {
                    try AudioFreezeDurationProof.verify(proof: proof, project: previous, sampleRate: record.sampleRate,
                        musicalFrames: record.musicalFrames, dormantOwners: owners, liveSources: [:])
                    previouslyMatched = true
                } catch { /* An already unavailable proof stays repairable. */ }
                if previouslyMatched {
                    do {
                        try AudioFreezeDurationProof.verify(proof: proof, project: next, sampleRate: record.sampleRate,
                            musicalFrames: record.musicalFrames, dormantOwners: owners, liveSources: [:])
                    } catch { throw fail("Unfreeze before changing the song's clip layout, timing or source media.") }
                }
            }
        }
        return try validate(project: next)
    }

    /// METADATA ONLY: enumerates every unverified printed stage for a valid canonical project.
    /// Missing proof never becomes a playable or partially activated projection.
    /// Schema or protected musical-data failures remain errors, not repair status.
    /// Host activation additionally requires assessActivation below.
    static func assessPlayback(project: [String: Any], verifiedAssets: [String: String],
                               currentMediaHashes: [String: String]? = nil) throws -> PlaybackAssessment {
        let statuses = try validate(project: project, currentMediaHashes: currentMediaHashes)
        let unverified = statuses.flatMap { status in
            status.record.assets.compactMap { asset -> UnverifiedAsset? in
                guard verifiedAssets[asset.path] != asset.sha256 else { return nil }
                return UnverifiedAsset(owner: status.record.owner, stage: asset.stage, path: asset.path, expectedSHA256: asset.sha256)
            }
        }.sorted { lhs, rhs in
            lhs.owner != rhs.owner ? lhs.owner < rhs.owner : lhs.stage.rawValue < rhs.stage.rawValue
        }
        guard unverified.isEmpty else { return .unavailable(statuses: statuses, assets: unverified) }
        return .eligible(try playbackProjection(project: project, verifiedAssets: verifiedAssets, currentMediaHashes: currentMediaHashes))
    }

    /// Check every frozen owner's duration proof, current live clip evidence and
    /// printed assets before returning any projected project. Unknown/missing or
    /// temporally stale proof remains a repairable unavailable assessment; ordinary
    /// Save/validate/relink never call this function. liveSources must describe
    /// immutable currently decoded assets with full source-file hashes verified
    /// by the Host; historical manifests do not qualify as runtime evidence.
    static func assessActivation(project: [String: Any], verifiedAssets: [String: String],
                                 liveSources: [String: AudioFreezeDurationProof.DecodedSource],
                                 currentMediaHashes: [String: String]? = nil) throws -> ActivationAssessment {
        let statuses = try validate(project: project, currentMediaHashes: currentMediaHashes)
        let unverified = statuses.flatMap { status in
            status.record.assets.compactMap { asset -> UnverifiedAsset? in
                guard verifiedAssets[asset.path] != asset.sha256 else { return nil }
                return UnverifiedAsset(owner: status.record.owner, stage: asset.stage, path: asset.path, expectedSHA256: asset.sha256)
            }
        }.sorted { $0.owner != $1.owner ? $0.owner < $1.owner : $0.stage.rawValue < $1.stage.rawValue }
        var reasons: [String] = [], extent: CapturedExtent?
        do { extent = try commonCapturedExtent(project: project) }
        catch { reasons.append(error.localizedDescription) }
        let dormantOwners = Set(statuses.map { $0.record.owner })
        var digest: String?
        for status in statuses {
            let record = status.record
            switch try AudioFreezeDurationProof.assess(record.durationProof) {
            case .absent:
                reasons.append("\(record.owner): Duration verification is unavailable; Unfreeze or recapture to enable this print.")
            case .unavailable(let reason):
                reasons.append("\(record.owner): \(reason)")
            case .supported(let proof):
                if let digest, digest != proof.temporalSHA256 {
                    reasons.append("Frozen tracks refer to different whole-song duration proofs. Unfreeze or recapture the differing prints.")
                }
                digest = digest ?? proof.temporalSHA256
                do {
                    try AudioFreezeDurationProof.verify(proof: proof, project: project, sampleRate: record.sampleRate,
                        musicalFrames: record.musicalFrames, dormantOwners: dormantOwners, liveSources: liveSources)
                } catch { reasons.append("\(record.owner): \(error.localizedDescription)") }
            }
        }
        guard reasons.isEmpty, unverified.isEmpty else {
            return .unavailable(statuses: statuses, assets: unverified, reasons: reasons)
        }
        return .eligible(projection: try playbackProjection(project: project, verifiedAssets: verifiedAssets,
            currentMediaHashes: currentMediaHashes), extent: extent)
    }

    /// METADATA ONLY: a derived view; canonical rows, clips, instruments and descriptors
    /// remain byte-equivalent JSON. No replacement public audio clip is inserted.
    /// This does not validate duration proof; Host activation uses assessActivation.
    static func playbackProjection(project: [String: Any], verifiedAssets: [String: String], currentMediaHashes: [String: String]? = nil) throws -> PlaybackProjection {
        let statuses = try validate(project: project, currentMediaHashes: currentMediaHashes)
        if statuses.isEmpty { return PlaybackProjection(project: try copy(project), substitutions: [], dormantPluginIDs: [], statuses: []) }
        let model = try Model(project)
        var result = try copy(project)
        let dormant = statuses.reduce(into: Set<String>()) { $0.formUnion($1.record.bakedPluginIDs) }
        let frozenOwners = Set(statuses.map { $0.record.owner })
        let indices = Set(model.definition.owners.filter { frozenOwners.contains($0.id) }.compactMap(\.trackIndex))
        var substitutions: [Substitution] = [], removedRoutes = Set<String>()
        for status in statuses {
            let record = status.record
            for asset in record.assets {
                guard verifiedAssets[asset.path] == asset.sha256 else { throw fail("Verify or relink frozen media \(asset.path) before playback, or Unfreeze.") }
            }
            let plan = try model.plan(owner: record.owner, mediaHashes: record.mediaHashes, allowMissingDependencyMedia: true)
            // Only direct incoming routes are disconnected. Upstream processors
            // still feed their other destinations and downstream owners stay live.
            let routes = Set(model.definition.sends.filter { $0.destination == record.owner }.map(\.id))
                .union(model.definition.sidechains.filter { record.bakedPluginIDs.contains($0.destination.instanceID) }.map(\.id))
            removedRoutes.formUnion(routes)
            substitutions.append(Substitution(owner: record.owner, trackIndex: plan.trackIndex, assets: record.assets, removeIncomingRouteIDs: routes))
        }
        result["audioPlugins"] = model.plugins.filter { !dormant.contains($0.id) }.map(\.value)
        result["audioClips"] = try objects(project["audioClips"], "audio clips").filter { !indices.contains(PayloadValue.integer($0["track"], 0)) }
        var patterns = try objects(project["patterns"], "patterns")
        for index in patterns.indices {
            var steps = patterns[index]["steps"] as? [[Any]] ?? []
            for track in steps.indices {
                for row in steps[track].indices {
                    guard var step = steps[track][row] as? [String: Any] else { continue }
                    if indices.contains(track) {
                        var live: [String: Any] = [:]
                        // Native note-off/cut rows return before built-in FX. Do
                        // not accidentally activate an ignored mix command when
                        // turning such a row into a control-only row.
                        let note = max(-3, min(127, PayloadValue.integer(step["note"], -3)))
                        for key in ["fx1", "fx2"] where note >= 0 || note == -3 {
                            if let fx = step[key] as? [String: Any], mixTargets.contains(fx["type"] as? String ?? "") { live[key] = fx }
                        }
                        step = live
                    } else {
                        for key in ["fx1", "fx2"] {
                            if let fx = step[key] as? [String: Any], fx["type"] as? String == "plugin",
                               !model.isLiveLock(fx, track: track, excluding: dormant) { step.removeValue(forKey: key) }
                        }
                    }
                    steps[track][row] = step.isEmpty ? NSNull() : step
                }
            }
            patterns[index]["steps"] = steps
            patterns[index]["automation"] = try objects(patterns[index]["automation"], "automation").filter { lane in
                let owner = try model.laneOwner(lane), target = lane["target"] as? String ?? "volume"
                if target.hasPrefix("send:"), removedRoutes.contains(String(target.dropFirst(5))) { return false }
                if let plugin = try model.lanePlugin(lane), dormant.contains(plugin.id) { return false }
                return !frozenOwners.contains(owner) || model.isMixLane(lane)
            }
        }
        result["patterns"] = patterns
        var routing = result["routing"] as! [String: Any]
        routing["sends"] = try objects(routing["sends"], "sends").filter { !removedRoutes.contains($0["id"] as? String ?? "") }
        routing["sidechains"] = try objects(routing["sidechains"], "sidechains").filter { !removedRoutes.contains($0["id"] as? String ?? "") }
        var outputs = routing["outputs"] as? [String: Any] ?? [:]
        for (source, destination) in model.definition.outputs where destination.map({ frozenOwners.contains($0) }) ?? false { outputs[source] = NSNull() }
        routing["outputs"] = outputs; result["routing"] = routing
        return PlaybackProjection(project: result, substitutions: substitutions, dormantPluginIDs: dormant, statuses: statuses)
    }

    /// Merge against canonical intent, never the derived descriptor list. Missing
    /// live instances retain their desired descriptors; stale/foreign snapshots
    /// cannot replace a dormant processor or retarget its stable identity.
    static func mergingSnapshot(_ live: [[String: Any]], into project: [String: Any]) throws -> [String: Any] {
        let statuses = try validate(project: project), model = try Model(project)
        let dormant = statuses.reduce(into: Set<String>()) { $0.formUnion($1.record.bakedPluginIDs) }
        var supplied: [String: [String: Any]] = [:]
        for value in live {
            let id = try identifier(value["instanceID"], "snapshot instance ID")
            guard supplied[id] == nil, !dormant.contains(id), let original = model.plugins.first(where: { $0.id == id }) else { throw fail("Snapshot contains a duplicate, dormant, or stale plug-in identity.") }
            let candidate = try Plugin(value, definition: model.definition)
            guard candidate.owner == original.owner, candidate.kind == original.kind, candidate.slot == original.slot,
                  candidate.value["component"] as? String == original.value["component"] as? String else { throw fail("Snapshot plug-in identity moved or changed component.") }
            guard value["stateError"] == nil, value["state"] is String else { throw fail("Snapshot contains an unreadable or missing plug-in state.") }
            supplied[id] = value
        }
        var result = try copy(project)
        result["audioPlugins"] = model.plugins.map { item -> [String: Any] in
            var value = item.value
            if let update = supplied[item.id] { value.merge(update) { _, new in new } }
            return value
        }
        return result
    }

    static func parse(_ raw: [String: Any]) throws -> Record {
        let required: Set<String> = ["version", "id", "owner", "scope", "startBeat", "sampleRate", "musicalEndBeat", "musicalFrames", "tailFrames", "assets", "bakedPluginIDs", "protectedInstrumentIndices", "dependencyOwners", "fingerprints", "media"]
        guard Set(raw.keys) == required || Set(raw.keys) == required.union(["durationProof"]) else {
            throw fail("Freeze record has missing or unknown fields.")
        }
        // Storage bounds only. A malformed/unsupported proof must survive Save,
        // relinking and explicit Unfreeze without becoming activation evidence.
        _ = try AudioFreezeDurationProof.assess(raw["durationProof"])
        guard try integer(raw["version"], "version", 1...1) == 1, raw["scope"] as? String == "song",
              try number(raw["startBeat"], "start beat", 0...0) == 0 else { throw fail("Only version 1 full-song freeze at beat zero is supported.") }
        let id = try identifier(raw["id"], "freeze ID"), owner = try ownerID(raw["owner"])
        guard owner.hasPrefix("track:") else { throw fail("Only track owners can be frozen.") }
        let rate = try number(raw["sampleRate"], "sample rate", 8_000...384_000)
        let musical = try integer(raw["musicalFrames"], "musical frames", 1...maximumFrames)
        let tail = try integer(raw["tailFrames"], "tail frames", 0...Int64(rate * 60))
        guard musical <= maximumFrames - tail else { throw fail("Freeze audio exceeds 100 million frames.") }
        let end = try number(raw["musicalEndBeat"], "musical end", Double.leastNonzeroMagnitude...1_000_000)
        let rows = try objects(raw["assets"], "freeze assets")
        guard (1...2).contains(rows.count) else { throw fail("Freeze requires one or two capture stages.") }
        var stages = Set<Stage>(), paths = Set<String>()
        let assets = try rows.map { value -> Asset in
            try keys(value, ["stage", "path", "sha256", "frames", "channels", "trimmedLatencyFrames", "insertsBaked"])
            guard let stage = Stage(rawValue: value["stage"] as? String ?? ""), stages.insert(stage).inserted,
                  try boolean(value["insertsBaked"], "insertsBaked") == (stage == .preFader),
                  try integer(value["channels"], "channels", 2...2) == 2 else { throw fail("Invalid, duplicate, or mislabeled capture stage.") }
            let path = try mediaPath(value["path"])
            guard paths.insert(path).inserted, ["wav", "aif", "aiff", "caf"].contains(URL(fileURLWithPath: path).pathExtension.lowercased()) else { throw fail("Each capture stage needs a distinct PCM audio path.") }
            let frames = try integer(value["frames"], "asset frames", 1...maximumFrames)
            guard frames == musical + tail else { throw fail("Capture frames must equal musical frames plus tail frames after latency trimming.") }
            return Asset(stage: stage, path: path, sha256: try digest(value["sha256"]), frames: frames,
                         trimmedLatencyFrames: try integer(value["trimmedLatencyFrames"], "trimmed latency", 0...1_048_576))
        }
        guard stages.contains(.preFader) else { throw fail("The post-insert pre-fader capture is required.") }
        let pluginIDs = try strings(raw["bakedPluginIDs"], "baked plug-ins", max: 9, transform: { try identifier($0, "instance ID") })
        let dependencyOwners = try strings(raw["dependencyOwners"], "dependency owners", max: 32, transform: ownerID)
        guard !dependencyOwners.contains(owner) else { throw fail("A frozen track cannot depend on itself.") }
        guard let indices = raw["protectedInstrumentIndices"] as? [Any], indices.count <= ProjectLayout.instrumentSanityLimit else { throw fail("Invalid protected instruments.") }
        let instruments = try indices.map { Int(try integer($0, "instrument index", 0...Int64(ProjectLayout.instrumentSanityLimit - 1))) }
        guard Set(instruments).count == instruments.count else { throw fail("Duplicate protected instrument index.") }
        guard let hashes = raw["fingerprints"] as? [String: Any] else { throw fail("Freeze fingerprints are required.") }
        try keys(hashes, ["source", "timing", "arrangement", "dependencies"])
        let fingerprints = try Fingerprints(source: digest(hashes["source"]), timing: digest(hashes["timing"]), arrangement: digest(hashes["arrangement"]), dependencies: digest(hashes["dependencies"]))
        var media: [String: String] = [:]
        let files = try objects(raw["media"], "media fingerprints")
        guard files.count <= 8_704 else { throw fail("Too many source media fingerprints.") }
        for file in files {
            try keys(file, ["path", "sha256"])
            let path = try mediaPath(file["path"])
            guard media[path] == nil else { throw fail("Duplicate source media fingerprint.") }
            media[path] = try digest(file["sha256"])
        }
        return Record(id: id, owner: owner, sampleRate: rate, musicalEndBeat: end, musicalFrames: musical,
            tailFrames: tail, assets: assets, bakedPluginIDs: pluginIDs, protectedInstrumentIndices: Set(instruments),
            dependencyOwners: dependencyOwners, fingerprints: fingerprints, mediaHashes: media, durationProof: raw["durationProof"])
    }

    private struct Plugin {
        let id: String, owner: String, kind: AudioRouting.PluginKind, slot: Int, value: [String: Any]
        init(_ value: [String: Any], definition: AudioRouting.Definition) throws {
            id = try identifier(value["instanceID"], "instance ID")
            if let raw = value["owner"] { owner = try ownerID(raw) }
            else {
                let track = PayloadValue.integer(value["track"], 0)
                guard let found = definition.owners.first(where: { $0.trackIndex == track }) else { throw fail("Plug-in track is unavailable.") }
                owner = found.id
            }
            guard definition.owner(owner) != nil, let kind = AudioRouting.PluginKind(rawValue: value["kind"] as? String ?? ""),
                  let component = value["component"] as? String, !component.isEmpty else { throw fail("Plug-in requires an existing owner, kind and component.") }
            self.kind = kind; slot = Int(try integer(value["slot"], "plug-in slot", 0...Int64(MixerChannel.insertSlotCount - 1))); self.value = value
            guard kind != .instrument || (slot == 0 && definition.owner(owner)?.kind == .track) else { throw fail("Invalid instrument location.") }
        }
    }
    private struct Model {
        let project: [String: Any], tracks: [[String: Any]], patterns: [[String: Any]], instruments: [[String: Any]]
        let definition: AudioRouting.Definition, plugins: [Plugin], graph: AudioRouting.Graph
        init(_ project: [String: Any]) throws {
            guard JSONSerialization.isValidJSONObject(project) else { throw fail("Project must contain finite JSON values.") }
            self.project = project; tracks = try objects(project["tracks"], "tracks"); patterns = try objects(project["patterns"], "patterns"); instruments = try objects(project["instruments"], "instruments")
            let routingDefinition = try AudioRouting.parse(project: project)
            definition = routingDefinition
            guard routingDefinition.enabled, routingDefinition.mixModel == .standard, !tracks.isEmpty else { throw fail("Full track freeze requires standard routing.") }
            guard project["audioPlugins"] != nil else { throw fail("Capture canonical audioPlugins descriptors with stable instance IDs before freezing.") }
            // Instrument insert chains (InstrumentEffects) are not baked by Freeze; see plan(owner:).
            let parsedPlugins = try objects(project["audioPlugins"], "plug-ins").filter { InstrumentEffects.instrumentIndex($0["owner"]) == nil }
                .map { try Plugin($0, definition: routingDefinition) }
            plugins = parsedPlugins
            guard parsedPlugins.count <= 512, Set(parsedPlugins.map(\.id)).count == parsedPlugins.count else { throw fail("Duplicate or excessive plug-in identities.") }
            let layouts = try parsedPlugins.map { plugin -> AudioRouting.PluginLayout in
                var buses: [Int: AudioRouting.InputBus] = [:]
                for route in routingDefinition.sidechains where route.destination.instanceID == plugin.id {
                    let d = route.destination
                    guard d.inputBus > 0 else { throw fail("Sidechain input must be an auxiliary bus.") }
                    let bus = AudioRouting.InputBus(index: d.inputBus, name: d.layoutSignature.name, channels: d.layoutSignature.channels, isAuxiliary: true)
                    guard buses[d.inputBus] == nil || buses[d.inputBus] == bus else { throw fail("Sidechain input layouts disagree.") }; buses[d.inputBus] = bus
                }
                return AudioRouting.PluginLayout(instanceID: plugin.id, owner: plugin.owner, kind: plugin.kind, slot: plugin.slot, inputBuses: Array(buses.values))
            }
            graph = try routingDefinition.graph(pluginLayouts: layouts)
            guard graph.unavailableSidechains.allSatisfy({ !$0.sidechain.enabled }) else { throw fail("An enabled sidechain is unavailable.") }
            for pattern in patterns {
                for lane in try objects(pattern["automation"], "automation") { _ = try lanePlugin(lane) }
            }
        }

        func plan(owner: String, mediaHashes: [String: String], allowMissingDependencyMedia: Bool = false) throws -> CapturePlan {
            guard let track = definition.owner(owner)?.trackIndex else { throw fail("Choose an existing track to freeze.") }
            let instrument = PayloadValue.integer(tracks.indices.contains(track) ? tracks[track]["instrument"] : nil, 0)
            guard !(try objects(project["audioPlugins"], "plug-ins")).contains(where: { InstrumentEffects.instrumentIndex($0["owner"]) == instrument }) else {
                throw fail("Freeze does not support instrument effects yet. Move the instrument's effects to the track's inserts to freeze it.")
            }
            let baked = Set(plugins.filter { $0.owner == owner }.map(\.id))
            var nodes: Set<String> = [AudioRouting.nodeID(owner: owner, stage: .preFader)]
            var changed = true
            while changed {
                changed = false
                for edge in graph.edges where nodes.contains(edge.destination) { if nodes.insert(edge.source).inserted { changed = true } }
            }
            let dependencies = Set(definition.owners.filter { candidate in
                candidate.id != owner && (AudioRouting.Stage.allFreezeStages.contains { nodes.contains(AudioRouting.nodeID(owner: candidate.id, stage: $0)) } || plugins.contains { $0.owner == candidate.id && nodes.contains(AudioRouting.pluginNodeID(instanceID: $0.id)) })
            }.map(\.id))
            let incomingIDs = Set(graph.edges.filter { nodes.contains($0.source) && nodes.contains($0.destination) }.compactMap { edge -> String? in
                if edge.id.hasPrefix("send:") { return String(edge.id.dropFirst(5)) }
                if edge.id.hasPrefix("sidechain:") { return String(edge.id.dropFirst(10)) }
                return nil
            })
            var required: Set<Stage> = [.preFader]
            if definition.sends.contains(where: { $0.source == owner && $0.tap == .preInserts }) || definition.sidechains.contains(where: { $0.source == owner && $0.tap == .preInserts }) { required.insert(.preInserts) }
            let ownInstruments = try referencedInstruments(track: track)
            var allInstruments = ownInstruments
            for dependency in dependencies { if let index = definition.owner(dependency)?.trackIndex { allInstruments.formUnion(try referencedInstruments(track: index)) } }
            let ownPaths = try paths(owners: [owner], instruments: ownInstruments)
            let allPaths = ownPaths.union(try paths(owners: dependencies, instruments: allInstruments.subtracting(ownInstruments), requireSources: !allowMissingDependencyMedia))
            var usedHashes: [String: String] = [:]
            for path in allPaths {
                if let hash = mediaHashes[path] { usedHashes[path] = try digest(hash) }
                else if !allowMissingDependencyMedia || ownPaths.contains(path) { throw fail("Verify source media \(path) before freezing.") }
            }
            let source = try sourcePayload(owner: owner, includeMix: false)
            let dependencyPayload: [String: Any] = ["owners": try dependencies.sorted().map {
                try sourcePayload(owner: $0, includeMix: nodes.contains(AudioRouting.nodeID(owner: $0, stage: .postFader)), capturedNodes: nodes)
            },
                "edges": graph.edges.filter { nodes.contains($0.source) && nodes.contains($0.destination) }.map { ["id": $0.id, "source": $0.source, "destination": $0.destination] }.sorted { $0["id"]! < $1["id"]! },
                "routes": routePayload(ids: incomingIDs)]
            let timingKeys = ["timing", "bpm", "linesPerBeat", "swing", "timeSignature"]
            let timing = project.filter { timingKeys.contains($0.key) }
            let arrangement: [String: Any] = ["sequence": project["sequence"] ?? [0], "owners": tracks.map { $0["routingID"] ?? NSNull() },
                "patterns": patterns.map { ["id": $0["id"] ?? 0, "length": $0["length"] ?? 64] }]
            let fingerprints = try Fingerprints(source: fingerprint(source, media: usedHashes), timing: fingerprint(timing), arrangement: fingerprint(arrangement), dependencies: fingerprint(dependencyPayload, media: usedHashes))
            return CapturePlan(owner: owner, trackIndex: track, requiredStages: required, bakedPluginIDs: baked,
                requiredPluginIDs: Set(plugins.filter { nodes.contains(AudioRouting.pluginNodeID(instanceID: $0.id)) }.map(\.id)),
                protectedInstrumentIndices: ownInstruments, requiredInstrumentIndices: allInstruments, dependencyOwners: dependencies, incomingRouteIDs: incomingIDs,
                fingerprints: fingerprints, mediaHashes: usedHashes)
        }

        func referencedInstruments(track: Int) throws -> Set<Int> {
            var indices: Set<Int> = [PayloadValue.integer(tracks[track]["instrument"], 0)]
            for pattern in patterns {
                let steps = pattern["steps"] as? [[Any]] ?? []
                if steps.indices.contains(track) { for raw in steps[track] { if let step = raw as? [String: Any], let value = step["instrument"] { let index = PayloadValue.integer(value, -1); if index >= 0 { indices.insert(index) } } } }
            }
            guard indices.allSatisfy({ (0..<ProjectLayout.instrumentSanityLimit).contains($0) && instruments.indices.contains($0) }) else { throw fail("A captured track references an unavailable instrument definition.") }
            return indices
        }
        func paths(owners: Set<String>, instruments indices: Set<Int>, requireSources: Bool = true) throws -> Set<String> {
            var result = Set<String>()
            for index in indices {
                let instrument = instruments[index]
                if instrument["type"] as? String == "sample" {
                    let path = instrument["samplePath"] as? String ?? ""
                    if requireSources, path.isEmpty && (instrument["zones"] as? [Any] ?? []).isEmpty { throw fail("A sampler has no source media.") }
                    // Built-in samples are app content, not hashed project media.
                    if !path.isEmpty && !BuiltinSamples.isBuiltin(path) { result.insert(try mediaPath(path)) }
                    for zone in try objects(instrument["zones"], "sample zones") { result.insert(try mediaPath(zone["path"] ?? zone["samplePath"])) }
                }
            }
            for clip in try objects(project["audioClips"], "clips") {
                let index = PayloadValue.integer(clip["track"], 0)
                if let owner = definition.owners.first(where: { $0.trackIndex == index }), owners.contains(owner.id) {
                    if !requireSources, (clip["path"] as? String ?? clip["samplePath"] as? String ?? "").isEmpty { continue }
                    result.insert(try mediaPath(clip["path"] ?? clip["samplePath"]))
                }
            }
            return result
        }
        func sourcePayload(owner: String, includeMix: Bool, capturedNodes: Set<String>? = nil) throws -> [String: Any] {
            var result: [String: Any] = ["owner": owner]
            if let index = definition.owner(owner)?.trackIndex {
                let excluded = cosmeticKeys.union(["id", "freeze", "plugins"]).union(includeMix ? [] : ["volume", "pan", "mute", "solo", "delaySend", "reverbSend"])
                result["track"] = tracks[index].filter { !excluded.contains($0.key) }
                result["instruments"] = try referencedInstruments(track: index).sorted().map { i in ["index": i, "definition": instruments[i].filter { !cosmeticKeys.contains($0.key) && $0.key != "id" }] as [String: Any] }
                result["clips"] = try objects(project["audioClips"], "clips").filter { PayloadValue.integer($0["track"], 0) == index }.map { clip -> [String: Any] in
                    // Only the selected source is rendered. Alternative take names,
                    // IDs and media paths remain canonical but are not baked.
                    ["path": clip["path"] ?? clip["samplePath"] ?? NSNull(), "track": index,
                     "startBeat": clip["startBeat"] ?? 0, "durationBeats": clip["durationBeats"] ?? 0,
                     "sourceOffset": clip["sourceOffset"] ?? clip["offsetSeconds"] ?? 0,
                     "gain": clip["gain"] ?? 1, "fadeIn": clip["fadeIn"] ?? clip["fadeInSeconds"] ?? 0.005,
                     "fadeOut": clip["fadeOut"] ?? clip["fadeOutSeconds"] ?? 0.005,
                     "muted": clip["muted"] ?? clip["mute"] ?? false]
                }
                result["patterns"] = try patterns.map { pattern -> [String: Any] in
                    let rows = pattern["steps"] as? [[Any]] ?? []
                    let steps: [Any] = rows.indices.contains(index) ? rows[index].map { raw in
                        guard var step = raw as? [String: Any] else { return NSNull() }
                        for key in ["fx1", "fx2"] {
                            guard let fx = step[key] as? [String: Any] else { continue }
                            if !includeMix && mixTargets.contains(fx["type"] as? String ?? "") { step.removeValue(forKey: key) }
                            if fx["type"] as? String == "plugin", !isLiveLock(fx, track: index, excluding: []) { step.removeValue(forKey: key) }
                            if fx["type"] as? String == "plugin", let nodes = capturedNodes,
                               let id = fx["instanceID"] as? String, !nodes.contains(AudioRouting.pluginNodeID(instanceID: id)) { step.removeValue(forKey: key) }
                        }
                        return step.isEmpty ? NSNull() : step
                    } : []
                    let lanes = try objects(pattern["automation"], "automation").filter { lane in
                        guard try laneOwner(lane) == owner, includeMix || !isMixLane(lane) else { return false }
                        if let nodes = capturedNodes, let plugin = try lanePlugin(lane) { return nodes.contains(AudioRouting.pluginNodeID(instanceID: plugin.id)) }
                        return true
                    }
                    return ["id": pattern["id"] ?? 0, "steps": steps, "automation": lanes.map(canonicalLane)]
                }
            } else if let bus = (project["routing"] as? [String: Any])?["buses"] as? [[String: Any]], let value = bus.first(where: { "bus:" + ($0["id"] as? String ?? "") == owner }) {
                let excluded = cosmeticKeys.union(includeMix ? [] : ["volume", "pan", "mute", "solo"])
                result["bus"] = value.filter { !excluded.contains($0.key) }
                result["automation"] = try patterns.map { pattern in
                    let lanes = try objects(pattern["automation"], "automation").filter { lane in
                        guard try laneOwner(lane) == owner, includeMix || !isMixLane(lane) else { return false }
                        if let nodes = capturedNodes, let plugin = try lanePlugin(lane) { return nodes.contains(AudioRouting.pluginNodeID(instanceID: plugin.id)) }
                        return true
                    }
                    return ["id": pattern["id"] ?? 0, "lanes": lanes.map(canonicalLane)] as [String: Any]
                }
            }
            result["plugins"] = plugins.filter {
                $0.owner == owner && (capturedNodes?.contains(AudioRouting.pluginNodeID(instanceID: $0.id)) ?? true)
            }.sorted { $0.id < $1.id }.map { plugin -> [String: Any] in
                var value = plugin.value.filter { !cosmeticKeys.contains($0.key) && !["track", "parameters", "latency", "latencyFrames", "tailTime", "stateError"].contains($0.key) }
                value["owner"] = plugin.owner
                return value
            }
            return result
        }
        func routePayload(ids: Set<String>) -> [[String: Any]] {
            let routing = project["routing"] as? [String: Any] ?? [:]
            return ((routing["sends"] as? [[String: Any]] ?? []) + (routing["sidechains"] as? [[String: Any]] ?? [])).filter { ids.contains($0["id"] as? String ?? "") }.map { $0.filter { !cosmeticKeys.contains($0.key) } }.sorted { ($0["id"] as? String ?? "") < ($1["id"] as? String ?? "") }
        }
        func canonicalLane(_ lane: [String: Any]) -> [String: Any] {
            lane.filter { !cosmeticKeys.contains($0.key) && $0.key != "id" && !(lane["owner"] != nil && $0.key == "track") }
        }
        func laneOwner(_ lane: [String: Any]) throws -> String {
            if let owner = lane["owner"] as? String { guard definition.owner(owner) != nil else { throw fail("Automation owner is unavailable.") }; return owner }
            guard let owner = definition.owners.first(where: { $0.trackIndex == PayloadValue.integer(lane["track"], 0) }) else { throw fail("Automation track is unavailable.") }; return owner.id
        }
        func isMixLane(_ lane: [String: Any]) -> Bool {
            let target = lane["target"] as? String ?? "volume"
            return mixTargets.contains(target) || target.hasPrefix("send:")
        }
        func lanePlugin(_ lane: [String: Any]) throws -> Plugin? {
            let target = lane["target"] as? String ?? "volume", owner = try laneOwner(lane)
            if lane["owner"] != nil, !["volume", "pan"].contains(target), !target.hasPrefix("send:"), !target.hasPrefix("plugin:") {
                throw fail("Owner-based automation supports mix, send and plug-in targets; native source controls require a numeric track lane.")
            }
            if lane["owner"] == nil, target.hasPrefix("send:") { throw fail("Send automation requires its stable source owner.") }
            if !target.hasPrefix("plugin:") {
                guard mixTargets.contains(target) || sourceTargets.contains(target) || target.hasPrefix("send:") else { throw fail("Unsupported automation target \(target).") }
                return nil
            }
            let parts = target.split(separator: ":", omittingEmptySubsequences: false).map(String.init)
            let kind: AudioRouting.PluginKind, slot: Int
            if parts.count == 4, let parsed = AudioRouting.PluginKind(rawValue: parts[1]), let number = Int(parts[2]), UInt64(parts[3]) != nil { kind = parsed; slot = number }
            else if parts.count == 3, let number = Int(parts[1]), UInt64(parts[2]) != nil { kind = .effect; slot = number }
            else { throw fail("Malformed plug-in automation target.") }
            guard (0..<MixerChannel.insertSlotCount).contains(slot), kind != .instrument || slot == 0,
                  lane["owner"] == nil || parts.count == 4 else { throw fail("Invalid plug-in automation slot or target format.") }
            return plugins.first { $0.owner == owner && $0.kind == kind && $0.slot == slot }
        }
        func isLiveLock(_ fx: [String: Any], track: Int, excluding: Set<String>) -> Bool {
            guard let id = fx["instanceID"] as? String, !excluding.contains(id), let plugin = plugins.first(where: { $0.id == id }),
                  definition.owner(plugin.owner)?.trackIndex == track, let address = fx["address"] as? String,
                  let parsed = UInt64(address), String(parsed) == address,
                  PayloadValue.number(fx["value"], in: 0...1) != nil else { return false }
            return true
        }
    }

    private static func endingBeat(project: [String: Any], sampleRate: Double, frames: Int64) throws -> Double {
        guard sampleRate.isFinite, (8_000...384_000).contains(sampleRate), (1...maximumFrames).contains(frames) else { throw fail("Invalid capture sample rate or musical frame count.") }
        let seconds = Double(frames) / sampleRate
        return seconds * (try number(project["bpm"] ?? 120, "tempo", 20...999)) / 60
    }
    private static func fingerprint(_ object: Any, media: [String: String] = [:]) throws -> String {
        func normalized(_ value: Any) -> Any {
            if let object = value as? [String: Any] {
                var result: [String: Any] = [:]
                for (key, value) in object {
                    if ["path", "samplePath"].contains(key), let path = value as? String, let hash = media[path] { result[key] = ["contentSHA256": hash] }
                    else { result[key] = normalized(value) }
                }
                return result
            }
            if let array = value as? [Any] { return array.map(normalized) }
            return value
        }
        return SHA256.hash(data: try encoded(normalized(object))).map { String(format: "%02x", $0) }.joined()
    }
    private static func encoded(_ object: Any) throws -> Data { try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .fragmentsAllowed]) }
    private static func copy(_ object: [String: Any]) throws -> [String: Any] { try JSONSerialization.jsonObject(with: encoded(object)) as! [String: Any] }
    private static func fail(_ message: String) -> Failure { Failure(message: message) }
    private static func objects(_ value: Any?, _ label: String) throws -> [[String: Any]] {
        guard let value else { return [] }; guard let objects = value as? [[String: Any]] else { throw fail("\(label) must be an array of objects.") }; return objects
    }
    private static func keys(_ object: [String: Any], _ expected: Set<String>) throws { guard Set(object.keys) == expected else { throw fail("Freeze record has missing or unknown fields.") } }
    private static func number(_ value: Any?, _ label: String, _ range: ClosedRange<Double>) throws -> Double {
        guard let result = PayloadValue.number(value, in: range) else { throw fail("Invalid \(label).") }; return result
    }
    private static func integer(_ value: Any?, _ label: String, _ range: ClosedRange<Int64>) throws -> Int64 {
        let n = try number(value, label, Double(range.lowerBound)...Double(range.upperBound)); guard PayloadValue.isWhole(n) else { throw fail("\(label) must be an integer.") }; return Int64(n)
    }
    private static func boolean(_ value: Any?, _ label: String) throws -> Bool {
        guard let result = PayloadValue.boolean(value) else { throw fail("\(label) must be Boolean.") }; return result
    }
    private static func identifier(_ value: Any?, _ label: String) throws -> String {
        let allowed = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_.")
        guard let value = value as? String, !value.isEmpty, value.utf8.count <= 128, value.unicodeScalars.allSatisfy({ allowed.contains($0) }) else { throw fail("Invalid \(label).") }; return value
    }
    private static func ownerID(_ value: Any?) throws -> String {
        guard let value = value as? String else { throw fail("Invalid owner ID.") }
        if value == "master" { return value }
        let parts = value.split(separator: ":", omittingEmptySubsequences: false)
        guard parts.count == 2, ["track", "bus"].contains(String(parts[0])) else { throw fail("Invalid owner ID.") }; _ = try identifier(String(parts[1]), "owner ID"); return value
    }
    private static func digest(_ value: Any?) throws -> String {
        guard let value = value as? String, value.utf8.count == 64, value.unicodeScalars.allSatisfy({ CharacterSet(charactersIn: "0123456789abcdef").contains($0) }) else { throw fail("Expected a lowercase SHA256 digest.") }; return value
    }
    private static func mediaPath(_ value: Any?) throws -> String {
        guard let path = value as? String, !path.isEmpty, path.utf8.count <= 16_384,
              !path.contains("://"), !path.hasPrefix("~"), !path.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }),
              !path.split(separator: "/").contains(where: { $0 == ".." || $0 == "." }) else { throw fail("Expected a local, normalized media path.") }; return path
    }
    private static func strings(_ value: Any?, _ label: String, max count: Int, transform: (Any?) throws -> String) throws -> Set<String> {
        guard let array = value as? [Any], array.count <= count else { throw fail("Invalid \(label).") }
        let values = try array.map(transform); guard Set(values).count == values.count else { throw fail("Duplicate \(label).") }; return Set(values)
    }
}

private extension AudioRouting.Stage {
    static var allFreezeStages: [AudioRouting.Stage] { [.source, .input, .preFader, .postFader] }
}

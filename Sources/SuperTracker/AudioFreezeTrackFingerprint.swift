import Foundation
import CryptoKit

/// Pure schema fingerprint for the user-facing tracks[].frozen record.
/// The hashing projection deliberately preserves the original v1 algorithm.
enum AudioFreezeTrackFingerprint {
    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }
    private static let mixTargets: Set<String> = ["volume", "pan", "delay", "reverb"]
    private static let recordKey = "frozen"
    private static let volatilePluginKeys: Set<String> = ["parameters", "latency", "latencyFrames", "tailTime", "stateError", "name", "editorOpen", "windowFrame"]
    private static let cosmeticTrackKeys: Set<String> = ["name", "color", "selected", "collapsed", "editorOpen", "windowFrame", "viewState", "id", "plugins", recordKey,
                                                         "volume", "pan", "mute", "solo", "delaySend", "reverbSend"]


    private static func owner(_ track: [String: Any]) -> String? {
        (track["routingID"] as? String).map { "track:" + $0 }
    }

    private static func pluginDescriptors(_ project: [String: Any]) -> [[String: Any]] {
        if let explicit = project["audioPlugins"] as? [[String: Any]] { return explicit }
        let instruments = project["instruments"] as? [[String: Any]] ?? []
        var result: [[String: Any]] = []
        for (track, item) in (project["tracks"] as? [[String: Any]] ?? []).enumerated() {
            let instrument = PayloadValue.integer(item["instrument"], 0)
            if instruments.indices.contains(instrument), var plugin = instruments[instrument]["plugin"] as? [String: Any], plugin["component"] != nil {
                plugin["track"] = track; plugin["kind"] = "instrument"; plugin["slot"] = 0; result.append(plugin)
            }
            for (slot, value) in (item["plugins"] as? [Any] ?? []).enumerated() {
                guard var plugin = value as? [String: Any], plugin["component"] != nil else { continue }
                plugin["track"] = track; plugin["kind"] = "effect"; plugin["slot"] = slot; result.append(plugin)
            }
        }
        return result
    }

    private static func pluginBelongs(_ plugin: [String: Any], index: Int, owner: String?) -> Bool {
        if let pluginOwner = plugin["owner"] as? String { return pluginOwner == owner }
        return PayloadValue.integer(plugin["track"], 0) == index
    }

    private static func plugins(of index: Int, in project: [String: Any]) -> [[String: Any]] {
        let tracks = project["tracks"] as? [[String: Any]] ?? []
        guard tracks.indices.contains(index) else { return [] }
        let owner = owner(tracks[index])
        return pluginDescriptors(project).filter { pluginBelongs($0, index: index, owner: owner) }
    }

    private static func laneBelongs(_ lane: [String: Any], index: Int, owner: String?) -> Bool {
        if let laneOwner = lane["owner"] as? String { return laneOwner == owner }
        return PayloadValue.integer(lane["track"], 0) == index
    }

    private static func isMixLane(_ lane: [String: Any]) -> Bool {
        let target = lane["target"] as? String ?? "volume"
        return mixTargets.contains(target) || target.hasPrefix("send:")
    }

    private static func isMixFX(_ fx: Any?) -> Bool {
        guard let fx = fx as? [String: Any] else { return false }
        return mixTargets.contains(fx["type"] as? String ?? "")
    }

    private static func trackUses(_ pattern: [String: Any], index: Int, owner: String?) -> Bool {
        let steps = pattern["steps"] as? [[Any]] ?? []
        if steps.indices.contains(index), steps[index].contains(where: { raw in
            guard var step = raw as? [String: Any] else { return false }
            for key in ["fx1", "fx2"] where isMixFX(step[key]) { step.removeValue(forKey: key) }
            return !step.isEmpty
        }) { return true }
        return (pattern["automation"] as? [[String: Any]] ?? []).contains { laneBelongs($0, index: index, owner: owner) && !isMixLane($0) }
    }


    static func fingerprint(_ project: [String: Any], index: Int) throws -> String {
        let tracks = project["tracks"] as? [[String: Any]] ?? []
        guard tracks.indices.contains(index) else { throw Failure(message: "Choose an existing track.") }
        let track = tracks[index], trackOwner = owner(track)
        let instruments = project["instruments"] as? [[String: Any]] ?? []
        var referenced = Set<Int>([PayloadValue.integer(track["instrument"], 0)])
        var patternPayload: [[String: Any]] = []
        // Patterns the song plays (song print) and patterns the track uses (pattern prints).
        let sequenced = Set((project["sequence"] as? [Any] ?? [0]).map { PayloadValue.integer($0, 0) })
        for (position, pattern) in (project["patterns"] as? [[String: Any]] ?? []).enumerated()
            where sequenced.contains(PayloadValue.integer(pattern["id"], position)) || trackUses(pattern, index: index, owner: trackOwner) {
            let steps = pattern["steps"] as? [[Any]] ?? []
            let rows: [Any] = steps.indices.contains(index) ? steps[index].map { raw -> Any in
                guard var step = raw as? [String: Any] else { return NSNull() }
                if let value = step["instrument"] { let i = PayloadValue.integer(value, -1); if i >= 0 { referenced.insert(i) } }
                for key in ["fx1", "fx2"] where isMixFX(step[key]) { step.removeValue(forKey: key) }
                return step.isEmpty ? NSNull() : step
            } : []
            let lanes = (pattern["automation"] as? [[String: Any]] ?? []).filter { laneBelongs($0, index: index, owner: trackOwner) && !isMixLane($0) }
                .map { $0.filter { !["id", "name"].contains($0.key) } }
            patternPayload.append(["id": pattern["id"] ?? 0, "length": pattern["length"] ?? 64, "steps": rows, "automation": lanes])
        }
        let payload: [String: Any] = [
            "track": track.filter { !cosmeticTrackKeys.contains($0.key) },
            "instruments": referenced.sorted().map { i -> Any in
                instruments.indices.contains(i) ? instruments[i].filter { !["name", "id", "plugin"].contains($0.key) } : NSNull()
            },
            "plugins": plugins(of: index, in: project).map { $0.filter { !volatilePluginKeys.contains($0.key) && $0.key != "state" } },
            "clips": (project["audioClips"] as? [[String: Any]] ?? []).filter { PayloadValue.integer($0["track"], 0) == index }
                .map { $0.filter { !["id", "name"].contains($0.key) } },
            "patterns": patternPayload,
            "timing": ["bpm": project["bpm"] ?? 120, "linesPerBeat": project["linesPerBeat"] ?? 4, "swing": project["swing"] ?? 0,
                       "timing": project["timing"] ?? NSNull(), "sequence": project["sequence"] ?? [0]]
        ]
        let data = try JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys, .fragmentsAllowed])
        return SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }

    /// Preserve freshness only across caller-verified, content-identical media
    /// aliases. This is not a way to certify stale or musically edited prints.
    /// Save supplies hash-verified copy aliases; Open supplies package-relative
    /// expansion of the same reference; Relink supplies exact full-hash matches.
    static func preservingRelocatedSources(from original: [String: Any], to rewritten: [String: Any],
                                            verifiedAliases: [String: String]) throws -> [String: Any] {
        guard !verifiedAliases.isEmpty else { return rewritten }
        let oldTracks = original["tracks"] as? [[String: Any]] ?? []
        var result = rewritten, tracks = rewritten["tracks"] as? [[String: Any]] ?? []
        let aliased = replacingSourcePaths(original, aliases: verifiedAliases)
        for index in tracks.indices where oldTracks.indices.contains(index) {
            guard let old = oldTracks[index]["frozen"] as? [String: Any],
                  var new = tracks[index]["frozen"] as? [String: Any],
                  old["version"] as? Int == 1, new["version"] as? Int == 1,
                  let id = old["id"] as? String, !id.isEmpty, new["id"] as? String == id,
                  let captured = old["fingerprint"] as? String, !captured.isEmpty,
                  new["fingerprint"] as? String == captured,
                  try fingerprint(original, index: index) == captured else { continue }
            let moved = try fingerprint(rewritten, index: index)
            guard try fingerprint(aliased, index: index) == moved else { continue }
            new["fingerprint"] = moved
            tracks[index]["frozen"] = new
        }
        result["tracks"] = tracks
        return result
    }

    /// Only declared source media references move. In particular, plugin opaque
    /// data/bundle paths and unknown extension fields remain byte-equivalent.
    private static func replacingSourcePaths(_ project: [String: Any], aliases: [String: String]) -> [String: Any] {
        func fields(_ object: [String: Any], _ keys: [String]) -> [String: Any] {
            var result = object
            for key in keys {
                if let path = object[key] as? String, !path.isEmpty,
                   let replacement = aliases[path], !replacement.isEmpty { result[key] = replacement }
            }
            return result
        }
        var result = project
        if let instruments = project["instruments"] as? [[String: Any]] {
            result["instruments"] = instruments.map { instrument in
                var moved = fields(instrument, ["samplePath"])
                if let zones = instrument["zones"] as? [[String: Any]] {
                    moved["zones"] = zones.map { fields($0, ["path", "samplePath"]) }
                }
                return moved
            }
        }
        if let clips = project["audioClips"] as? [[String: Any]] {
            result["audioClips"] = clips.map { clip in
                var moved = fields(clip, ["path", "samplePath"])
                if var set = clip["takeSet"] as? [String: Any], let takes = set["takes"] as? [[String: Any]] {
                    set["takes"] = takes.map { fields($0, ["path"]) }; moved["takeSet"] = set
                }
                if let takes = clip["takes"] as? [[String: Any]] {
                    moved["takes"] = takes.map { fields($0, ["path", "samplePath"]) }
                }
                return moved
            }
        }
        return result
    }

}

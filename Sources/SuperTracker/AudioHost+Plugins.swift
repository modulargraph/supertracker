import Foundation
import AppKit
import AVFoundation
import AudioToolbox
import CoreAudio
import TrackerEngine

/// Desired plug-in descriptors, their identity and signatures, snapshots and the AU mix mirror.
extension AudioHost {
    /// The descriptors the rack hosts: the project's plug-ins with each instrument effect
    /// replaced by its per-track instances (InstrumentEffects.expand).
    func pluginDescriptions(_ data:[String:Any]) -> [[String:Any]] {
        InstrumentEffects.expand(storedPluginDescriptions(data),project:data)
    }

    /// The project's own descriptors (instrument effects stay owned by `instrument:<n>`).
    func storedPluginDescriptions(_ data:[String:Any]) -> [[String:Any]] {
        func canonical(_ items:[[String:Any]])->[[String:Any]] {
            guard data["routing"] != nil else { return items }
            let tracks = data["tracks"] as? [[String:Any]] ?? []
            return items.map { value in
                var item = value
                if item["owner"] == nil {
                    let index = PayloadValue.integer(item,"track",0)
                    if tracks.indices.contains(index), let id = tracks[index]["routingID"] as? String { item["owner"] = "track:"+id }
                }
                return item
            }
        }
        if let explicit = data["audioPlugins"] as? [[String:Any]] { return canonical(explicit) }
        let instruments = data["instruments"] as? [[String:Any]] ?? []
        var result: [[String:Any]] = []
        for (track,item) in (data["tracks"] as? [[String:Any]] ?? []).enumerated() {
            let instrument = PayloadValue.integer(item,"instrument",0)
            if instruments.indices.contains(instrument), var plugin = instruments[instrument]["plugin"] as? [String:Any], plugin["component"] != nil {
                plugin["track"] = track; plugin["kind"] = "instrument"; plugin["slot"] = 0; result.append(plugin)
            }
            for (slot,value) in (item["plugins"] as? [Any] ?? []).enumerated() {
                guard var plugin = value as? [String:Any], plugin["component"] != nil else { continue }
                plugin["track"] = track; plugin["kind"] = "effect"; plugin["slot"] = slot; result.append(plugin)
            }
        }
        return canonical(result)
    }

    func pluginIdentityMatches(_ desired:[String:Any],_ active:[String:Any])->Bool {
        guard let component = desired["component"] as? String, component == active["component"] as? String else { return false }
        let expectedFormat = desired["format"] as? String ?? (component.lowercased().hasPrefix("vst3:") ? "VST3" : component.lowercased().hasPrefix("vst2:") ? "VST2" : component.hasPrefix("stfx:") ? "Builtin" : "AU")
        guard expectedFormat == active["format"] as? String else { return false }
        // Old descriptors may lack a resolved bundle/instance identity. Never
        // overwrite an explicit identity with a different live installation/unit.
        if let path = desired["bundlePath"] as? String, path != active["bundlePath"] as? String { return false }
        if let id = desired["instanceID"] as? String, id != active["instanceID"] as? String { return false }
        return true
    }

    func pluginSignature(_ plugins:[[String:Any]])->String {
        String(data:(try? JSONSerialization.data(withJSONObject:plugins,options:.sortedKeys)) ?? Data(),encoding:.utf8) ?? ""
    }

    func removeSidechains(to instanceIDs:Set<String>) {
        guard !instanceIDs.isEmpty, var routing = project["routing"] as? [String:Any],
              let sidechains = routing["sidechains"] as? [[String:Any]] else { return }
        routing["sidechains"] = sidechains.filter { sidechain in
            guard let destination = sidechain["destination"] as? [String:Any],
                  let instanceID = destination["instanceID"] as? String else { return true }
            return !instanceIDs.contains(instanceID)
        }
        project["routing"] = routing
    }

    func remapEffectAutomation(track:Int,owner:String? = nil,slots:[Int:Int]) {
        guard var patterns = project["patterns"] as? [[String:Any]] else { return }
        for pattern in patterns.indices {
            guard var lanes = patterns[pattern]["automation"] as? [[String:Any]] else { continue }
            for lane in lanes.indices {
                if let laneOwner = lanes[lane]["owner"] as? String {
                    guard laneOwner == owner else { continue }
                } else { guard track < projectTrackCount, PayloadValue.integer(lanes[lane],"track",0) == track else { continue } }
                guard let target = lanes[lane]["target"] as? String else { continue }
                var parts = target.split(separator:":",omittingEmptySubsequences:false).map(String.init)
                let position:Int
                if parts.count == 4 && parts[0] == "plugin" && parts[1] == "effect" { position = 2 }
                else if parts.count == 3 && parts[0] == "plugin" { position = 1 }
                else { continue }
                guard let old = Int(parts[position]), let next = slots[old] else { continue }
                parts[position] = String(next); lanes[lane]["target"] = parts.joined(separator:":")
            }
            patterns[pattern]["automation"] = lanes
        }
        project["patterns"] = patterns
    }

    /// A saved insert that has no live unit (still restoring, held by crash recovery or
    /// failed to load) gets a reason the parameter pages can show instead of the rack's
    /// generic empty-slot error.
    func explainingUnloadedPlugin(_ payload:[String:Any],_ body:() throws -> Any) throws -> Any {
        do { return try body() }
        catch {
            guard !rack.hasInstance(payload),
                  let desired = pluginDescriptions(project).first(where:{ pluginKey($0) == pluginKey(payload) }) else { throw error }
            let component = desired["component"] as? String ?? desired["id"] as? String ?? ""
            let name = (desired["name"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? (component.isEmpty ? "This plug-in" : component)
            if pendingPluginRestore != nil {
                throw AudioSampleError.invalid("\(name) is still loading. Its controls appear when the project's plug-ins have loaded.")
            }
            if !component.isEmpty, rack.requiresRecoveryReview(componentID:component,bundlePath:desired["bundlePath"] as? String) {
                throw AudioSampleError.invalid("\(name) was held back because SuperTracker quit unexpectedly while it was loaded. Choose Review interrupted plug-ins to load it again.")
            }
            throw AudioSampleError.invalid("\(name) is not loaded in the audio engine, so its parameters are unavailable. Replace the plug-in or reopen the project to load it again.")
        }
    }

    func pluginKey(_ descriptor:[String:Any])->String {
        "\(pluginOwner(descriptor)):\(descriptor["kind"] as? String ?? "effect"):\(PayloadValue.integer(descriptor,"slot",0))"
    }

    func pluginOwner(_ descriptor:[String:Any])->String {
        if let owner = descriptor["owner"] as? String { return owner }
        let index = PayloadValue.integer(descriptor,"track",0)
        let tracks = project["tracks"] as? [[String:Any]] ?? []
        if project["routing"] != nil, tracks.indices.contains(index), let id = tracks[index]["routingID"] as? String { return "track:"+id }
        return "legacy-track-\(index)"
    }

    /// fresh=false is for change notifications: it never reads plug-in state (see
    /// AudioPluginRack.snapshot), so publishing a project costs no capture.
    func pluginSnapshot(fresh:Bool = true)->[[String:Any]] {
        let desired = InstrumentEffects.expand(storedPluginDescriptions(project),project:project,keepUnused:true)
        // While a new project is loading, a same-component live instance may still
        // contain the previous project's patch. Its state cannot replace the user's
        // requested state in autosave or an immediate explicit Save.
        if pendingPluginRestore != nil { return freezeTrackRuntime.mergeDormant(collapsingInstrumentEffects(desired)) }
        let live = rack.snapshot(fresh:fresh)
        var result = desired
        for active in live {
            let key = pluginKey(active)
            if let index = result.firstIndex(where:{ pluginKey($0) == key }) {
                // Preserve a requested but unavailable replacement rather than silently
                // saving the old component which happens to remain in that slot.
                let migrated=rack.isVerifiedMigration(result[index],active:active)
                if migrated,active["state"] == nil {
                    // Never label a retained VST2 bank as VST3 document state.
                    // Keep its original binding and propagate capture failure so
                    // persistence cannot silently save over later live edits.
                    result[index]["stateError"]=active["stateError"] ?? "The migrated plug-in could not capture its canonical VST3 state."
                } else if pluginIdentityMatches(result[index],active) || migrated {
                    var merged = active
                    if active["state"] == nil { merged["state"] = result[index]["state"] }
                    for key in [InstrumentEffects.ownerKey,InstrumentEffects.slotKey,InstrumentEffects.instanceKey] where result[index][key] != nil {
                        merged[key] = result[index][key]
                    }
                    result[index] = merged
                }
            } else if !MixerChannel.isInstrumentSlot(PayloadValue.integer(active,"slot",0)) || active["kind"] as? String == "instrument" {
                // A live instrument-chain instance the project no longer describes is not a channel insert.
                result.append(active)
            }
        }
        return freezeTrackRuntime.mergeDormant(collapsingInstrumentEffects(result))
    }

    /// Each instrument effect once, owned by its instrument, with the live state of the
    /// instance on the first track that plays it.
    func collapsingInstrumentEffects(_ descriptors:[[String:Any]]) -> [[String:Any]] {
        var seen = Set<String>()
        return descriptors.compactMap { item in
            guard InstrumentEffects.isExpanded(item) else { return item }
            let collapsed = InstrumentEffects.collapse(item)
            return seen.insert(pluginKey(collapsed)).inserted ? collapsed : nil
        }
    }

    func updatePluginMix() {
        let tracks = ProjectLayout.channels(project)
        let instruments = ProjectLayout.instruments(project)
        let anySolo = tracks.values.contains { $0["solo"] as? Bool == true }
        for (index,track) in tracks {
            let instrument = PayloadValue.integer(track,"instrument",0)
            let item = instruments.indices.contains(instrument) ? instruments[instrument] : [:]
            let gain = PayloadValue.number(track,"volume",0.8)*PayloadValue.number(item,"volume",0.7)*PayloadValue.number(project,"masterVolume",0.7)
            rack.setTrackMix(track:index,gain:Float(gain),pan:Float(PayloadValue.number(track,"pan",0)),muted:(track["mute"] as? Bool ?? false) || (anySolo && track["solo"] as? Bool != true),baseGain:Float(PayloadValue.number(item,"volume",0.7)*PayloadValue.number(project,"masterVolume",0.7)),instrumentVolume:Float(PayloadValue.number(item,"volume",0.7)),masterVolume:Float(PayloadValue.number(project,"masterVolume",0.7)),instrumentPan:Float(PayloadValue.number(item,"pan",0)),trackVolume:Float(PayloadValue.number(track,"volume",0.8)))
            rack.setTrackSends(track:index,delay:Float(PayloadValue.number(track,"delaySend",0)),reverb:Float(PayloadValue.number(track,"reverbSend",0)))
        }
        let delay = project["delay"] as? [String:Any] ?? [:], reverb = project["reverb"] as? [String:Any] ?? [:]
        let signature = project["timeSignature"] as? [String:Any] ?? [:]
        rack.setTimeSignature(numerator:PayloadValue.integer(signature,"numerator",4),denominator:PayloadValue.integer(signature,"denominator",4))
        // A synced delay ("beats" > 0, quarter notes) follows the project tempo; "tone" (0...1) sets its low-pass.
        let delayBeats = PayloadValue.number(delay,"beats",0), delayTime = delayBeats > 0 ? delayBeats*60/max(1,PayloadValue.number(project,"bpm",120)) : PayloadValue.number(delay,"time",0.375)
        rack.configureSends(delayTime:delayTime,feedback:Float(PayloadValue.number(delay,"feedback",0.35)),reverbDecay:Float(PayloadValue.number(reverb,"size",0.7)),delayMix:Float(PayloadValue.number(delay,"mix",0.3)),reverbMix:Float(PayloadValue.number(reverb,"mix",0.25)),damping:Float(PayloadValue.number(reverb,"damping",0.4)),delayTone:delay["tone"] == nil ? nil : Float(PayloadValue.number(delay,"tone",1)))
    }
}

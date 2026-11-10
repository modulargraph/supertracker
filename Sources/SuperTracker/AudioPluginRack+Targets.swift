import AppKit
import AudioToolbox
import AVFoundation
import CoreAudioKit
import CoreFoundation
import Darwin

/// Owners and slot targets from payloads, unit summaries and parameter descriptions.
extension AudioPluginRack {
    func ownerIndex(_ payload:[String:Any]) throws -> Int {
        if let owner = payload["owner"] as? String {
            guard let index = ownerIndices[owner], routingDefinition?.owner(owner) != nil else {
                throw RackError.invalid("The selected mixer channel no longer exists.")
            }
            return index
        }
        guard let track = payload["track"] as? Int, (0..<flatTracks).contains(track) else {
            throw RackError.invalid("Choose a valid track, bus or master channel.")
        }
        return track
    }

    /// Main-thread identity configuration. Existing bus IDs retain their dense
    /// slots across array reordering; persistent IDs never reach render callbacks.
    func configureRoutingOwners(_ definition:AudioRouting.Definition) throws {
        var next:[String:Int] = [:]
        if definition.enabled {
            for owner in definition.owners where owner.trackIndex != nil {
                if let index = owner.trackIndex { next[owner.id] = index }
            }
            next["master"] = masterIndex
            let buses = definition.owners.filter { $0.kind == .group || $0.kind == .return }
            var occupied = Set<Int>()
            for bus in buses {
                if let old = ownerIndices[bus.id], (trackCount..<masterIndex).contains(old), occupied.insert(old).inserted { next[bus.id] = old }
            }
            for bus in buses where next[bus.id] == nil {
                guard let index = (trackCount..<masterIndex).first(where:{ !occupied.contains($0) }) else { throw RackError.invalid("This project exceeds \(Self.busCapacity) audio buses.") }
                occupied.insert(index); next[bus.id] = index
            }
        }
        ownerIndices = next
        ownersByIndex = Dictionary(uniqueKeysWithValues:next.map { ($0.value,$0.key) })
        routingDefinition = definition
        // Entering/leaving routed mixing on existing note tracks preserves their
        // live instrument/effect identity. Bus identity changes require restoration.
        // A track plug-in keeps its owner: when tracks move, the routing rebuild moves the plug-in
        // to its owner's new channel, and a deleted owner's plug-in is retired there. It must not
        // be adopted by whichever track shifts into its old channel, whose own plug-in moves in too.
        for item in instances.values where item.target.track < flatTrackCount {
            if let owner = item.ownerID, owner.hasPrefix("track:"), next[owner] != nil || definition.enabled { continue }
            item.ownerID = ownersByIndex[item.target.track]
        }
    }

    func target(_ payload: [String: Any]) throws -> Target {
        let track = try ownerIndex(payload)
        let kind = payload["kind"] as? String ?? "instrument"
        let slot = payload["slot"] as? Int ?? 0
        if track >= trackCount && routingDefinition?.mixModel != .standard {
            throw RackError.invalid("Enable standard routing to use bus or master insert effects.")
        }
        // Instrument-chain slots (16..<32) on track channels are reachable only through expanded
        // instrument effect descriptors (InstrumentEffects), never by a channel's own slot number.
        let slots = payload[InstrumentEffects.ownerKey] != nil && track < trackCount ? MixerChannel.instrumentSlotBase..<maximumEffects : 0..<MixerChannel.insertSlotCount
        guard ["instrument", "effect"].contains(kind), slots.contains(slot) || kind == "instrument" && slot == 0,
              kind != "instrument" || track < trackCount else {
            throw RackError.invalid("Choose a valid instrument or effect slot.")
        }
        return Target(track: track, kind: kind, slot: kind == "instrument" ? 0 : slot)
    }

    /// Whether the addressed slot holds a live unit (regardless of instance identity).
    func hasInstance(_ payload: [String: Any]) -> Bool {
        guard let target = try? target(payload) else { return false }
        return instances[target] != nil
    }

    /// The crash journal holds this exact component back until the user retries it.
    func requiresRecoveryReview(componentID: String, bundlePath: String?) -> Bool {
        guard let store = recoveryStore, Self.isRecoveryTracked(componentID:componentID,format:nil) else { return false }
        return store.recoveryReport().suspects.contains {
            $0.requiresReview && $0.identity.componentID.caseInsensitiveCompare(componentID) == .orderedSame &&
                (bundlePath == nil || $0.identity.bundlePath == nil || $0.identity.bundlePath == bundlePath)
        }
    }

    func instance(_ payload: [String: Any]) throws -> Instance {
        guard let item = instances[try target(payload)] else { throw RackError.invalid("There is no Audio Unit in this slot.") }
        if let requested = payload["instanceID"] {
            guard let identity = requested as? String, !identity.isEmpty, identity == item.instanceID else {
                throw RackError.invalid("The selected plug-in instance changed. Refresh the slot before editing or saving its state.")
            }
        }
        return item
    }

    func summary(_ item: Instance, includeParameters: Bool = true) -> [String: Any] {
        let unit = item.node.auAudioUnit
        synchronizeParameterValues(unit)
        // Third-party units may report a non-finite latency or tail (an "infinite tail" is common). JSON cannot
        // carry those: one such value made every snapshot, save and reply for the project fail.
        let reportedLatency: Double = unit.latency, reportedTail: Double = unit.tailTime
        let latency: Double = reportedLatency.isFinite ? max(0, reportedLatency) : 0
        let tailTime: Double = reportedTail.isFinite ? max(0, reportedTail) : (reportedTail > 0 ? 60 : 0)
        var result: [String: Any] = ["track": item.target.track, "kind": item.target.kind, "slot": item.target.slot, "instanceID":item.instanceID,
            "component": item.component.id, "name": item.component.name,
            "manufacturer": item.component.manufacturer, "format": item.component.format, "bypass": unit.shouldBypassEffect,
            "latency": latency, "tailTime": tailTime, "presets": presetInfo(unit)]
        if let owner = item.ownerID { result["owner"] = owner; if item.target.track >= flatTrackCount { result.removeValue(forKey:"track") } }
        if let path = item.component.bundlePath { result["bundlePath"] = path }
        if let nks = item.nks { result["nks"] = nks }
        if let migration=item.migration {
            result["migration"]=["sourceComponent":migration.component,"format":"VST2-bank-to-VST3","usedRawFallback":migration.usedRawFallback]
        }
        if let adapter = unit as? HostedPluginAdapter { result["hostStatus"] = adapter.status }
        if let adapter = unit as? VST3AudioUnit { result["basePatch"] = adapter.basePatchCapabilities }
        if includeParameters {
            let live = liveParameterValues(item)
            result["parameters"] = unit.parameterTree?.allParameters.map { parameterInfo($0,unit:unit,live:live) } ?? []
        }
        if let preset = unit.currentPreset { result["currentPreset"] = ["number": preset.number, "name": preset.name] }
        return result
    }

    func synchronizeParameterValues(_ unit:AUAudioUnit) {
        (unit as? HostedPluginAdapter)?.synchronizeParameters()
        (unit as? BuiltinEffectsAudioUnit)?.synchronizeParameterValues()
    }

    func parameterStepCount(_ parameter:AUParameter,unit:AUAudioUnit) -> Int? {
        if let adapter = unit as? BuiltinEffectsAudioUnit { return adapter.parameterStepCount(address:parameter.address) ?? 0 }
        if let adapter = unit as? VST3AudioUnit { return adapter.parameterStepCount(parameter.address) }
        // VST2 exposes no reliable discrete-domain metadata through this ABI.
        if unit is VST2AudioUnit { return 0 }
        return nil
    }

    func parameterInfo(_ parameter:AUParameter,unit:AUAudioUnit,live:((AUParameter) -> AUValue?)? = nil) -> [String:Any] {
        AudioPluginParameterMetadata.info(parameter,explicitStepCount:parameterStepCount(parameter,unit:unit),value:live?(parameter))
    }

    /// Apple's AUv2 bridge caches each AUParameter's value and refreshes it only when the plug-in posts a
    /// parameter-change notification. Some plug-ins (NI Raum) queue a write until their next render and post
    /// none, so the tree keeps the value it read right after the write: the old one, indefinitely. Read such
    /// units' global parameters from the plug-in itself. Our own adapters' trees are authoritative.
    func liveParameterValues(_ item:Instance) -> ((AUParameter) -> AUValue?)? {
        guard !(item.node.auAudioUnit is HostedPluginAdapter),!(item.node.auAudioUnit is BuiltinEffectsAudioUnit) else { return nil }
        let handle=item.node.audioUnit
        var size:UInt32=0
        guard AudioUnitGetPropertyInfo(handle,kAudioUnitProperty_ParameterList,kAudioUnitScope_Global,0,&size,nil) == noErr,size > 0 else { return nil }
        var ids=[AudioUnitParameterID](repeating:0,count:Int(size)/MemoryLayout<AudioUnitParameterID>.size)
        guard AudioUnitGetProperty(handle,kAudioUnitProperty_ParameterList,kAudioUnitScope_Global,0,&ids,&size) == noErr else { return nil }
        // The bridge gives other scopes' parameters encoded addresses; only global IDs read back unambiguously.
        let global=Set(ids.prefix(Int(size)/MemoryLayout<AudioUnitParameterID>.size).map { AUParameterAddress($0) })
        return { parameter in
            guard global.contains(parameter.address) else { return nil }
            var value:AudioUnitParameterValue=0
            guard AudioUnitGetParameter(handle,AudioUnitParameterID(parameter.address),kAudioUnitScope_Global,0,&value) == noErr,
                  value.isFinite else { return nil }
            return value
        }
    }

    func presetInfo(_ unit: AUAudioUnit) -> [[String: Any]] {
        (unit.factoryPresets ?? []).map { ["number": $0.number, "name": $0.name] }
    }
}

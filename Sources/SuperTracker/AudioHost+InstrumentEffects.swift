import Foundation

/// Plug-in commands addressed to an instrument's insert chain (`owner: "instrument:<n>"`).
///
/// The project keeps one descriptor per instrument effect; the rack runs one instance per
/// track that plays the instrument (`InstrumentEffects.expand`). Edits fan out to every
/// instance, reads (parameters, state, editor) use the instance on the first such track,
/// and the next project publication restores that state into the others. An instrument no
/// track plays keeps its descriptors without live instances.
extension AudioHost {
    /// Returns nil for payloads that do not address an instrument chain.
    @MainActor func handleInstrumentEffectCommand(_ command:String,_ payload:[String:Any]) async throws -> Any? {
        guard command.hasPrefix("plugin."), let instrument = InstrumentEffects.instrumentIndex(payload["owner"]) else { return nil }
        let owner = InstrumentEffects.owner(instrument)
        guard ProjectLayout.instruments(project).indices.contains(instrument) else {
            throw AudioSampleError.invalid("This instrument no longer exists.")
        }
        guard payload["kind"] as? String ?? "effect" == "effect" else {
            throw AudioSampleError.invalid("An instrument's insert chain holds effects only.")
        }
        switch command {
        case "plugin.load": return try await loadInstrumentEffect(payload,owner:owner)
        case "plugin.remove": return try await removeInstrumentEffect(payload,owner:owner)
        case "plugin.move": return try await moveInstrumentEffect(payload,owner:owner,instrument:instrument)
        case "plugin.parameter", "plugin.preset":
            let slot = try instrumentSlot(payload)
            let targets = try instances(of:owner,slot:slot,payload:payload)
            if payload["bypass"] is Bool || payload["number"] is Int, targets.isEmpty {
                // An instrument no track plays: the descriptor alone carries the change.
                return instrumentDescriptor(owner,slot:slot) ?? [:]
            }
            guard !targets.isEmpty else { throw notPlaying(instrument) }
            var first:Any?
            for target in targets {
                let result = command == "plugin.preset" ? try rack.preset(target) : try rack.parameter(target)
                if first == nil { first = result }
            }
            return collapsed(first,targets.first)
        case "plugin.parameters":
            guard let primary = try instances(of:owner,slot:try instrumentSlot(payload),payload:payload).first else { throw notPlaying(instrument) }
            return try rack.parameters(primary)
        case "plugin.editor":
            guard let primary = try instances(of:owner,slot:try instrumentSlot(payload),payload:payload).first else { throw notPlaying(instrument) }
            return try await rack.editor(primary)
        case "plugin.state":
            let slot = try instrumentSlot(payload)
            let targets = try instances(of:owner,slot:slot,payload:payload)
            guard let primary = targets.first else {
                guard var stored = instrumentDescriptor(owner,slot:slot) else { throw AudioSampleError.invalid("This insert effect is no longer present.") }
                if let state = payload["state"] as? String { stored["state"] = state; replaceInstrumentDescriptor(stored) }
                return stored
            }
            if let state = payload["state"] as? String {
                let identity = projectIdentity,generation = projectGeneration
                let release = try await acquireStateCaptureOperation()
                defer { release() }
                try requireInteractiveMutation()
                guard identity == projectIdentity, generation == projectGeneration else {
                    throw AudioSampleError.invalid("The project changed while plug-in state was waiting to restore. Retry the operation.")
                }
                var first:Any?
                for target in targets { var request = target; request["state"] = state; let result = try rack.state(request); if first == nil { first = result } }
                return collapsed(first,primary)
            }
            let release = try await preparePluginStateCapture(primary)
            defer { release() }
            if pendingPluginRestore != nil, let stored = instrumentDescriptor(owner,slot:slot) { return stored }
            return collapsed(try rack.state(primary),primary)
        default:
            throw AudioSampleError.invalid("This command is not available for instrument effects.")
        }
    }

    private func notPlaying(_ instrument:Int) -> Error {
        AudioSampleError.invalid("Instrument \(String(format:"%02d",instrument)) is not assigned to a track, so its effects are not running. Assign it to a track to edit them.")
    }

    private func instrumentSlot(_ payload:[String:Any]) throws -> Int {
        guard let slot = PayloadValue.integer(payload["slot"]), (0..<MixerChannel.insertSlotCount).contains(slot) else {
            throw AudioSampleError.invalid("Choose an insert slot from 1 to \(MixerChannel.insertSlotCount).")
        }
        return slot
    }

    func instrumentDescriptor(_ owner:String,slot:Int) -> [String:Any]? {
        storedPluginDescriptions(project).first { $0["owner"] as? String == owner && $0["kind"] as? String ?? "effect" == "effect" && PayloadValue.integer($0,"slot",0) == slot }
    }

    private func replaceInstrumentDescriptor(_ descriptor:[String:Any]) {
        let key = pluginKey(descriptor)
        project["audioPlugins"] = storedPluginDescriptions(project).map { pluginKey($0) == key ? descriptor : $0 }
    }

    /// The rack payloads of one instrument effect's instances, first track first. A payload
    /// instanceID must name the instrument's effect, not one of its per-track instances.
    private func instances(of owner:String,slot:Int,payload:[String:Any]) throws -> [[String:Any]] {
        guard let stored = instrumentDescriptor(owner,slot:slot) else { throw AudioSampleError.invalid("This insert effect is no longer present.") }
        if let requested = payload["instanceID"] as? String, requested != stored["instanceID"] as? String {
            throw AudioSampleError.invalid("The selected plug-in instance changed. Refresh the slot before editing or saving its state.")
        }
        return InstrumentEffects.expand([stored],project:project).map { expanded in
            var request = payload
            for key in ["owner","track","slot","kind","instanceID",InstrumentEffects.ownerKey,InstrumentEffects.slotKey,InstrumentEffects.instanceKey] {
                request[key] = expanded[key]
            }
            request["instanceID"] = expanded["instanceID"]
            return request
        }.filter { rack.hasInstance($0) }
    }

    /// A rack result for an expanded instance (`request`), as the instrument's descriptor.
    private func collapsed(_ value:Any?,_ request:[String:Any]?) -> Any {
        guard var item = value as? [String:Any], item["owner"] != nil || item["track"] != nil, let request else { return value ?? [:] }
        for key in [InstrumentEffects.ownerKey,InstrumentEffects.slotKey,InstrumentEffects.instanceKey] where request[key] != nil { item[key] = request[key] }
        return item[InstrumentEffects.ownerKey] != nil ? InstrumentEffects.collapse(item) : item
    }

    @MainActor private func loadInstrumentEffect(_ payload:[String:Any],owner:String) async throws -> Any {
        let slot = try instrumentSlot(payload)
        guard payload["component"] as? String ?? payload["id"] as? String != nil else {
            throw AudioSampleError.invalid("Choose an Audio Unit or VST3 plug-in to load.")
        }
        graphMutationsInFlight += 1
        defer { graphMutationsInFlight -= 1 }
        let identity = projectIdentity
        await awaitPluginRestoration()
        try requireInteractiveMutation()
        guard identity == projectIdentity else { throw AudioSampleError.invalid("Plug-in loading was superseded by another project.") }
        var descriptor = payload
        descriptor.removeValue(forKey:"track"); descriptor.removeValue(forKey:"path")
        descriptor["owner"] = owner; descriptor["kind"] = "effect"; descriptor["slot"] = slot
        let used = Set(storedPluginDescriptions(project).compactMap { $0["instanceID"] as? String })
        if (descriptor["instanceID"] as? String).map({ $0.isEmpty || used.contains($0) }) ?? true { descriptor["instanceID"] = UUID().uuidString }
        let original = project
        var first:[String:Any]?
        do {
            for expanded in InstrumentEffects.expand([descriptor],project:project) {
                let result = try await rack.load(expanded) as? [String:Any]
                try requireInteractiveMutation()
                guard identity == projectIdentity else { throw AudioSampleError.invalid("Plug-in loading was superseded by another project.") }
                if first == nil, var result {
                    for key in [InstrumentEffects.ownerKey,InstrumentEffects.slotKey,InstrumentEffects.instanceKey] { result[key] = expanded[key] }
                    first = result
                }
            }
        } catch {
            // The project still describes the previous chain: put its instances back.
            if identity == projectIdentity {
                project = original
                try? await rack.restore(pluginDescriptions(project))
            }
            throw error
        }
        var stored = descriptor
        if let first {
            let live = InstrumentEffects.collapse(first)
            for key in ["name","manufacturer","format","bundlePath","state","bypass"] where live[key] != nil { stored[key] = live[key] }
        } else if let id = stored["component"] as? String, id.hasPrefix("stfx:"),
                  let row = BuiltinEffectsAudioUnit.catalogue().first(where:{ $0["id"] as? String == id }) {
            stored["name"] = stored["name"] ?? row["name"]; stored["format"] = "Builtin"
        }
        stored.removeValue(forKey:"parameters")
        let removedIDs = Set(storedPluginDescriptions(project).filter { pluginKey($0) == pluginKey(stored) }.compactMap { $0["instanceID"] as? String })
        project["audioPlugins"] = storedPluginDescriptions(project).filter { pluginKey($0) != pluginKey(stored) } + [stored]
        removeSidechains(to:removedIDs)
        try refreshRoutingProject()
        appliedPluginSignature = ""
        if let first { return InstrumentEffects.collapse(first) }
        var result = stored; result["parameters"] = [Any](); result["inactive"] = true
        return result
    }

    @MainActor private func removeInstrumentEffect(_ payload:[String:Any],owner:String) async throws -> Any {
        let slot = try instrumentSlot(payload)
        graphMutationsInFlight += 1
        defer { graphMutationsInFlight -= 1 }
        let identity = projectIdentity
        await awaitPluginRestoration()
        try requireInteractiveMutation()
        guard identity == projectIdentity else { throw AudioSampleError.invalid("Plug-in removal was superseded by another project.") }
        guard let stored = instrumentDescriptor(owner,slot:slot) else { return ["removed":false,"owner":owner,"kind":"effect","slot":slot] }
        for expanded in InstrumentEffects.expand([stored],project:project) { _ = try rack.remove(expanded) }
        project["audioPlugins"] = storedPluginDescriptions(project).filter { pluginKey($0) != pluginKey(stored) }
        try refreshRoutingProject()
        appliedPluginSignature = ""
        var result:[String:Any] = ["removed":true,"owner":owner,"kind":"effect","slot":slot]
        if let id = stored["instanceID"] { result["instanceID"] = id }
        return result
    }

    /// Reorders an instrument's chain on every track that plays it in one transaction (no
    /// unit is reloaded; built-in chains update in place while playing).
    @MainActor private func moveInstrumentEffect(_ payload:[String:Any],owner:String,instrument:Int) async throws -> Any {
        guard graphMutationsInFlight == 0 else { throw AudioSampleError.invalid("Finish the pending audio graph change before moving an insert.") }
        guard let fromSlot = PayloadValue.integer(payload["fromSlot"]), let toSlot = PayloadValue.integer(payload["toSlot"]),
              (0..<MixerChannel.insertSlotCount).contains(fromSlot), (0..<MixerChannel.insertSlotCount).contains(toSlot) else {
            throw AudioSampleError.invalid("Choose valid source and destination insert slots on one track.")
        }
        graphMutationsInFlight += 1
        defer { graphMutationsInFlight -= 1 }
        let identity = projectIdentity, generation = projectGeneration
        await awaitPluginRestoration()
        try requireInteractiveMutation()
        guard identity == projectIdentity, graphMutationsInFlight == 1, directPluginLoads.isEmpty else {
            throw AudioSampleError.invalid("Insert movement was superseded by another project or plug-in change.")
        }
        let releaseCapture = try await preparePluginStateCapture()
        defer { releaseCapture() }
        guard identity == projectIdentity, generation == projectGeneration else {
            throw AudioSampleError.invalid("The project changed while insert states were being prepared. Retry moving the insert.")
        }
        let stored = storedPluginDescriptions(project)
        let chain = stored.filter { $0["owner"] as? String == owner && $0["kind"] as? String ?? "effect" == "effect" }
            .sorted { PayloadValue.integer($0,"slot",0) < PayloadValue.integer($1,"slot",0) }
        let slots = chain.map { PayloadValue.integer($0,"slot",0) }
        guard let fromIndex = slots.firstIndex(of:fromSlot), let toIndex = slots.firstIndex(of:toSlot) else {
            throw AudioSampleError.invalid("Both reorder positions must contain loaded insert effects.")
        }
        var order = Array(chain.indices)
        let moving = order.remove(at:fromIndex); order.insert(moving,at:toIndex)
        // chain[order[i]] moves to slots[i].
        var map:[Int:Int] = [:]
        for (position,index) in order.enumerated() { map[slots[index]] = slots[position] }
        // The current state of each effect: its first live instance, else the saved descriptor.
        let live = Dictionary(uniqueKeysWithValues:collapsingInstrumentEffects(
            InstrumentEffects.expand(chain,project:project).map { expanded -> [String:Any] in
                guard rack.hasInstance(expanded), var result = try? rack.state(expanded) as? [String:Any] else { return expanded }
                for key in [InstrumentEffects.ownerKey,InstrumentEffects.slotKey,InstrumentEffects.instanceKey] { result[key] = expanded[key] }
                result.removeValue(forKey:"parameters")
                return result
            }).map { (PayloadValue.integer($0,"slot",0),$0) })
        let previous = chain.map { item -> [String:Any] in
            var value = item
            if let current = live[PayloadValue.integer(item,"slot",0)] { for (key,entry) in current { value[key] = entry } }
            return value
        }
        let moved = previous.map { item -> [String:Any] in var value = item; value["slot"] = map[PayloadValue.integer(item,"slot",0)]!; return value }
        var plans:[Int:[Int:Int]] = [:]
        for track in InstrumentEffects.tracks(playing:instrument,project:project) {
            let channel = try rack.ownerIndex(track.owner.map { ["owner":$0] } ?? ["track":track.index])
            plans[channel] = Dictionary(uniqueKeysWithValues:map.map { (MixerChannel.instrumentSlotBase + $0.key,MixerChannel.instrumentSlotBase + $0.value) })
        }
        let original = project, oldSignature = appliedPluginSignature
        let commit = {
            self.project["audioPlugins"] = stored.filter { $0["owner"] as? String != owner || $0["kind"] as? String ?? "effect" != "effect" }
                .map { $0 } + moved.map { item -> [String:Any] in
                    var value = item; for key in ["parameters","track"] { value.removeValue(forKey:key) }; return value
                }
            self.remapEffectAutomation(track:-1,owner:owner,slots:map)
            try self.applyEngineProject(self.project)
            self.appliedPluginSignature = self.pluginSignature(self.pluginDescriptions(self.project))
        }
        let rollback = {
            self.project = original; self.appliedPluginSignature = oldSignature
            try self.applyEngineProject(original)
        }
        try rack.remapEffectSlots(plans.filter { !$0.value.isEmpty },commit:commit,rollback:rollback)
        try refreshRoutingProject()
        let strip = { (items:[[String:Any]]) in items.map { item -> [String:Any] in var value = item; value.removeValue(forKey:"parameters"); return value } }
        var result:[String:Any] = ["moved":fromSlot != toSlot,"owner":owner,"fromSlot":fromSlot,"toSlot":toSlot,
            "plugins":strip(moved),"previousPlugins":strip(previous),
            "slotMap":previous.map { ["fromSlot":PayloadValue.integer($0,"slot",0),"toSlot":map[PayloadValue.integer($0,"slot",0)]!,"instanceID":$0["instanceID"] ?? NSNull()] }
                .sorted { ($0["fromSlot"] as! Int) < ($1["fromSlot"] as! Int) },
            "audioPlugins":storedPluginDescriptions(project)]
        if let routing = project["routing"] { result["routing"] = routing }
        return result
    }
}

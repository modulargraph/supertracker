import AppKit
import AudioToolbox
import AVFoundation
import CoreAudioKit
import CoreFoundation
import Darwin

/// Removing plug-ins and reordering effect slots as one transaction.
extension AudioPluginRack {
    @MainActor func remove(_ payload: [String: Any]) throws -> Any {
        try removeTarget(target(payload))
    }

    @MainActor func removeTarget(_ target:Target) throws -> Any {
        notifyStateCaptureMutation()
        generations[target] = (generations[target] ?? 0) + 1
        guard let instance = instances[target] else { return ["removed":false] }
        let previousState = lastRestoredStates[target]
        routingTransactionDepth += 1
        defer { routingTransactionDepth -= 1 }
        let wasRunning = engine.isRunning
        // Removing a built-in insert usually only shrinks an insert-chain node: try in place first.
        let inPlace = wasRunning && insertChainCandidate(target:target,node:nil,previous:instance)
        if wasRunning && !inPlace { pauseEngine() }
        do { if !inPlace { try awaitRenderQuiescence() } }
        catch { if wasRunning { try? startEngine() }; throw error }
        instances.removeValue(forKey:target); lastRestoredStates.removeValue(forKey:target)
        publish(nil,for:target)
        if !inPlace { engine.disconnectNodeOutput(instance.node); engine.disconnectNodeOutput(instance.converter) }
        do {
            if inPlace, try applyInsertChainUpdate() {}
            else {
                if engine.isRunning { pauseEngine(); try awaitRenderQuiescence() }
                engine.disconnectNodeOutput(instance.node); engine.disconnectNodeOutput(instance.converter)
                try rebuildRemovingUnit(target.track)
            }
            if !refreshLatencyCompensation() { onLatencyPlanChanged?() }
            if wasRunning && !engine.isRunning { try startEngine() }
            try beginRecoveryRelease(instance.recovery)
        } catch {
            let original = error
            stopEngine(); try? awaitRenderQuiescence()
            instances[target] = instance; lastRestoredStates[target] = previousState
            publish(RenderEndpoint(instance),for:target)
            do { try rollbackRoutingMutation() }
            catch { throw RackError.invalid("Plug-in removal failed (\(original.localizedDescription)); restoring its previous routing also failed (\(error.localizedDescription)). Audio remains stopped.") }
            if wasRunning { try? startEngine() }
            throw original
        }
        if !engine.isRunning { try routingGraph.commit() } else { routingGraph.commitChainUpdate() }
        editorWindows.removeValue(forKey:target)?.close()
        retiredUnits.append(instance)
        detachUnit(instance)
        collectRetiredEndpoints()
        var result:[String:Any] = ["removed":true,"track":target.track,"kind":target.kind,"slot":target.slot,"instanceID":instance.instanceID]
        if let owner = instance.ownerID { result["owner"] = owner; if target.track >= flatTrackCount { result.removeValue(forKey:"track") } }
        return result
    }

    /// The channel's own loaded inserts (its instrument chain is not included).
    func loadedEffects(track:Int)->[[String:Any]] {
        instances.values.filter { $0.target.track == track && $0.target.kind == "effect" && !MixerChannel.isInstrumentSlot($0.target.slot) }.map {
            var result:[String:Any] = ["track":$0.target.track,"kind":"effect","slot":$0.target.slot,"component":$0.component.id,"instanceID":$0.instanceID,"format":$0.component.format]
            if let owner = $0.ownerID { result["owner"] = owner; if $0.target.track >= flatTrackCount { result.removeValue(forKey:"track") } }
            if let path = $0.component.bundlePath { result["bundlePath"] = path }
            return result
        }
    }

    /// Reorder occupied positions only. Numeric gaps stay gaps, and no unit or
    /// converter is detached, instantiated, restored, or destroyed by this operation.
    @MainActor func move(_ payload:[String:Any], commit:([String:Any]) throws->Void, rollback:() throws->Void) throws -> [String:Any] {
        let track = try ownerIndex(payload)
        guard let source = payload["fromSlot"] as? Int, let destination = payload["toSlot"] as? Int,
              (0..<maximumEffects).contains(source), (0..<maximumEffects).contains(destination) else {
            throw RackError.invalid("Choose valid source and destination insert slots on one track.")
        }
        // A channel's own inserts and its instrument chain reorder separately (see moveInstrumentEffects).
        let range = MixerChannel.isInstrumentSlot(source) ? MixerChannel.instrumentSlotBase..<maximumEffects : 0..<MixerChannel.insertSlotCount
        guard range.contains(destination) else { throw RackError.invalid("Choose valid source and destination insert slots on one track.") }
        let chain = instances.values.filter { $0.target.track == track && $0.target.kind == "effect" && range.contains($0.target.slot) }.sorted { $0.target.slot < $1.target.slot }
        guard let fromIndex = chain.firstIndex(where:{$0.target.slot == source}),
              let toIndex = chain.firstIndex(where:{$0.target.slot == destination}) else {
            throw RackError.invalid("Both reorder positions must contain loaded insert effects.")
        }
        let wasRunning = engine.isRunning
        // Reordering built-in inserts usually only reorders an insert-chain node's members.
        let inPlace = wasRunning && chain.allSatisfy { insertChainCandidate(target:$0.target,node:nil,previous:$0) }
        if wasRunning && !inPlace { pauseEngine() }
        do {
            if !inPlace { try awaitRenderQuiescence() }
            // Capture before touching routing. Serialization failure leaves the old
            // chain intact and does not manufacture an incomplete undo state.
            let previous = try chain.map { item -> [String:Any] in
                var descriptor = summary(item,includeParameters:false)
                descriptor["state"] = try encodedState(item.node.auAudioUnit, allowStoppedAudioAdvance: !wasRunning)
                return descriptor
            }
            var ordered = chain
            let moving = ordered.remove(at:fromIndex); ordered.insert(moving,at:toIndex)
            let slots = chain.map { $0.target.slot }
            var assignments:[Target:Instance] = [:], states:[String:String] = [:]
            var descriptions:[[String:Any]] = [], slotMap:[[String:Any]] = []
            for (index,item) in ordered.enumerated() {
                let newTarget = Target(track:track,kind:"effect",slot:slots[index])
                assignments[newTarget] = item
                var descriptor = previous.first { $0["instanceID"] as? String == item.instanceID }!
                descriptor["slot"] = newTarget.slot; descriptions.append(descriptor)
                states[item.instanceID] = descriptor["state"] as? String
                slotMap.append(["fromSlot":item.target.slot,"toSlot":newTarget.slot,"instanceID":item.instanceID])
            }
            var result:[String:Any] = ["moved":source != destination,"track":track,"fromSlot":source,"toSlot":destination,
                "plugins":descriptions,"previousPlugins":previous,"slotMap":slotMap.sorted { ($0["fromSlot"] as! Int) < ($1["fromSlot"] as! Int) }]
            if let owner = ownersByIndex[track] { result["owner"] = owner; if track >= flatTrackCount { result.removeValue(forKey:"track") } }
            try applyEffectOrders([track:assignments],states:states,resume:wasRunning && !inPlace,commit:{try commit(result)},rollback:rollback)
            return result
        } catch {
            if wasRunning && !engine.isRunning { try? startEngine() }
            throw error
        }
    }

    /// Moves effects to other slots on several channels in one transaction: an instrument chain
    /// reordered on every track that plays it. `plans[channel]` maps old slot to new slot; the
    /// channel's other effects keep theirs. Nothing is reloaded; built-in chains update in place.
    @MainActor func remapEffectSlots(_ plans:[Int:[Int:Int]],commit:() throws->Void,rollback:() throws->Void) throws {
        var assignments:[Int:[Target:Instance]] = [:]
        for (track,map) in plans {
            var plan:[Target:Instance] = [:]
            for item in instances.values where item.target.track == track && item.target.kind == "effect" {
                let target = Target(track:track,kind:"effect",slot:map[item.target.slot] ?? item.target.slot)
                guard plan[target] == nil else { throw RackError.invalid("Two effects would share one insert slot.") }
                plan[target] = item
            }
            if plan.contains(where:{ $0.key != $0.value.target }) { assignments[track] = plan }
        }
        guard !assignments.isEmpty else { try commit(); return }
        let wasRunning = engine.isRunning
        let inPlace = wasRunning && assignments.values.allSatisfy { $0.values.allSatisfy { insertChainCandidate(target:$0.target,node:nil,previous:$0) } }
        if wasRunning && !inPlace { pauseEngine() }
        do {
            if !inPlace { try awaitRenderQuiescence() }
            try applyEffectOrders(assignments,states:[:],resume:wasRunning && !inPlace,commit:commit,rollback:rollback)
        } catch { if wasRunning && !engine.isRunning { try? startEngine() }; throw error }
    }

    /// A complete, exact saved identity permutation is an undo/redo of ordering.
    /// Missing/duplicate IDs, changed components/bundles, or edited state use the
    /// normal restore path instead. Legacy projects remain valid without IDs.
    @MainActor @discardableResult func restoreEffectOrder(_ saved:[[String:Any]],commit:() throws->Void = {},rollback:() throws->Void = {}) throws -> Bool {
        let ids = saved.compactMap { $0["instanceID"] as? String }
        guard Set(ids).count == ids.count else { return false }
        var plans:[Int:[Target:Instance]] = [:]
        // Only strips that hold effects can need a reorder (the index space is sized for capacity).
        for track in Set(instances.values.filter { $0.target.kind == "effect" }.map(\.target.track)).sorted() {
            let current = instances.values.filter { $0.target.track == track && $0.target.kind == "effect" }
            let desired = saved.filter { (try? target($0).track) == track && $0["kind"] as? String == "effect" }
            guard !current.isEmpty, current.count == desired.count else { continue }
            let byID = Dictionary(uniqueKeysWithValues:current.map { ($0.instanceID,$0) })
            var assignments:[Target:Instance] = [:], valid = true
            for descriptor in desired {
                guard let id = descriptor["instanceID"] as? String, let item = byID[id],
                      descriptor["component"] as? String == item.component.id,
                      item.ownerID == ownersByIndex[track],
                      (descriptor["format"] as? String ?? item.component.format) == item.component.format,
                      descriptor["bundlePath"] as? String == item.component.bundlePath,
                      let state = descriptor["state"] as? String, let baseline = lastRestoredStates[item.target],
                      equivalentState(state,baseline), let destination = try? target(descriptor), assignments[destination] == nil else {
                    valid = false; break
                }
                assignments[destination] = item
            }
            guard valid, Set(assignments.values.map(\.instanceID)).count == current.count,
                  assignments.contains(where:{$0.key != $0.value.target}) else { continue }
            plans[track] = assignments
        }
        guard !plans.isEmpty else { return false }
        let wasRunning = engine.isRunning
        let inPlace = wasRunning && plans.values.allSatisfy { $0.values.allSatisfy { insertChainCandidate(target:$0.target,node:nil,previous:$0) } }
        if wasRunning && !inPlace { pauseEngine() }
        do {
            if !inPlace { try awaitRenderQuiescence() }
            try applyEffectOrders(plans,states:[:],resume:wasRunning && !inPlace,commit:commit,rollback:rollback)
            return true
        } catch { if wasRunning && !engine.isRunning { try? startEngine() }; throw error }
    }

    func equivalentState(_ lhs:String,_ rhs:String)->Bool {
        if lhs == rhs { return true }
        guard let a = Data(base64Encoded:lhs), let b = Data(base64Encoded:rhs) else { return false }
        return AudioAUStateReuse.equivalent(a,b)
    }

    /// Control-thread wait only; audio callbacks never acquire a control lock.
    /// An uncooperative in-flight vendor render denies the edit after 100 ms.
    func awaitRenderQuiescence() throws {
        let deadline = ProcessInfo.processInfo.systemUptime+0.1
        while endpointStore.activeReaders != 0 && ProcessInfo.processInfo.systemUptime < deadline { usleep(100) }
        guard endpointStore.activeReaders == 0 else {
            throw RackError.invalid("The audio render is still busy. Stop playback and retry the insert change.")
        }
    }

    @MainActor func applyEffectOrders(_ plans:[Int:[Target:Instance]],states:[String:String],resume:Bool,commit:() throws->Void,rollback:() throws->Void) throws {
        notifyStateCaptureMutation()
        routingTransactionDepth += 1
        defer { routingTransactionDepth -= 1 }
        let tracks = Set(plans.keys)
        let runningAtEntry = engine.isRunning
        let previous = instances.filter { tracks.contains($0.key.track) && $0.key.kind == "effect" }
        let oldStates = lastRestoredStates, oldWindows = editorWindows
        let statesByID = Dictionary(uniqueKeysWithValues:previous.compactMap { target,item in oldStates[target].map { (item.instanceID,$0) } })
        let windowsByID = Dictionary(uniqueKeysWithValues:previous.compactMap { target,item in oldWindows[target].map { (item.instanceID,$0) } })
        func install(_ assignments:[Target:Instance],restoring:Bool,reconnect:Bool = true) throws {
            for target in Array(instances.keys) where tracks.contains(target.track) && target.kind == "effect" {
                instances.removeValue(forKey:target); lastRestoredStates.removeValue(forKey:target); editorWindows.removeValue(forKey:target)
                generations[target,default:0] += 1
                publish(nil,for:target)
            }
            for (target,item) in assignments {
                item.target = target; instances[target] = item
                lastRestoredStates[target] = restoring ? oldStates[target] : (states[item.instanceID] ?? statesByID[item.instanceID])
                editorWindows[target] = restoring ? oldWindows[target] : windowsByID[item.instanceID]
                generations[target,default:0] += 1
                publish(RenderEndpoint(item),for:target)
            }
            if reconnect {
                // Built-in inserts reordered within their chain nodes need no rewiring.
                if routingDefinition?.enabled == true {
                    let inPlace = engine.isRunning ? try applyInsertChainUpdate() : false
                    if !inPlace { try configureRouting(routingProject,force:true) }
                }
                else { for track in tracks.sorted() { rebuildFlat(track) } }
                if !refreshLatencyCompensation() { onLatencyPlanChanged?() }
            }
            endpointStore.collectRetired()
        }
        let next = Dictionary(uniqueKeysWithValues:plans.values.flatMap { $0.map { ($0.key,$0.value) } })
        do {
            try install(next,restoring:false)
            try commit()
            try beforeEffectOrderResume?(); if resume { try startEngine() }
            else if !engine.isRunning { try routingGraph.commit() } else { routingGraph.commitChainUpdate() }
        }
        catch {
            let originalError = error
            stopEngine()
            try awaitRenderQuiescence()
            try install(previous,restoring:true,reconnect:false)
            try rollbackRoutingMutation(); try rollback()
            if resume || runningAtEntry { try? startEngine() }
            throw originalError
        }
    }
}

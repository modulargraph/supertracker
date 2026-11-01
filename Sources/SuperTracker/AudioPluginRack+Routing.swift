import AppKit
import AudioToolbox
import AVFoundation
import CoreAudioKit
import CoreFoundation
import Darwin
import VST2Host

/// Applying a routing definition: incremental changes while rendering, full rebuilds otherwise.
extension AudioPluginRack {
    /// A synchronous control-thread transaction. Rendering is stopped and its
    /// registered readers finish before any graph connection changes. The previous
    /// graph stays available until the replacement has started successfully.
    func configureRouting(_ data:[String:Any],force:Bool = false,commit:() throws->Void = {},rollback:() throws->Void = {}) throws {
        notifyStateCaptureMutation()
        let definition = try AudioRouting.parse(project:data)
        _ = try AudioProject.routingAutomation(data)
        let oldDefinition = routingDefinition, oldProject = routingProject, oldSignature = routingSignature
        let oldOwners = ownerIndices, oldIndices = ownersByIndex
        let oldItemOwners = instances.mapValues(\.ownerID)
        let oldInstances=instances,oldStates=lastRestoredStates,oldWindows=editorWindows
        var remappedPlugins=false
        var removedPlugins:[Instance]=[],releasedTracks:[Int]=[]
        func restorePluginAssignments() {
            guard remappedPlugins else { return }
            for target in Set(instances.keys).union(oldInstances.keys) { publish(nil,for:target) }
            instances=oldInstances;lastRestoredStates=oldStates;editorWindows=oldWindows
            for (target,item) in oldInstances { item.target=target;publish(RenderEndpoint(item),for:target) }
        }
        try configureRoutingOwners(definition)
        let signature:String
        do { signature = try makeRoutingSignature(data,definition:definition) }
        catch {
            routingDefinition = oldDefinition; ownerIndices = oldOwners; ownersByIndex = oldIndices
            for (target,owner) in oldItemOwners { instances[target]?.ownerID = owner }
            throw error
        }
        if !force && signature == routingSignature && (definition.enabled == hasRouting) {
            do {
                routingProject = data
                if let prepared = routingGraph.current {
                    updateRoutingDefaults(prepared,definition:definition,project:data)
                    try publishRoutingRender(prepared,project:data)
                }
                try commit(); return
            } catch {
                let failure = error
                routingProject = oldProject
                if let prepared = routingGraph.current, let previous = oldDefinition {
                    updateRoutingDefaults(prepared,definition:previous,project:oldProject)
                    try publishRoutingRender(prepared,project:oldProject)
                }
                try rollback(); throw failure
            }
        }
        if !force, try applyIncrementalRouting(data,definition:definition,previous:oldDefinition,commit:commit,rollback:{
            routingDefinition = oldDefinition; ownerIndices = oldOwners; ownersByIndex = oldIndices
            for (target,owner) in oldItemOwners { instances[target]?.ownerID = owner }
            try rollback()
        }) { return }
        // Same nodes, routes and timing (for example plug-in automation bindings, or built-in
        // inserts moved within their chain nodes): republish in place, without a rebuild.
        if !force, oldDefinition?.enabled == true, definition.enabled, oldDefinition?.mixModel == definition.mixModel,
           oldDefinition?.owners.map({ "\($0.id)/\($0.trackIndex ?? -1)" }) == definition.owners.map({ "\($0.id)/\($0.trackIndex ?? -1)" }),
           try applyInsertChainUpdate(data) {
            do {
                if let prepared = routingGraph.current { updateRoutingDefaults(prepared,definition:definition,project:data) }
                routingProject = data
                try commit()
                routingGraph.commitChainUpdate()
                return
            } catch {
                let failure = error
                try? routingGraph.rollbackChainUpdate()
                routingDefinition = oldDefinition; ownerIndices = oldOwners; ownersByIndex = oldIndices
                for (target,owner) in oldItemOwners { instances[target]?.ownerID = owner }
                routingProject = oldProject; routingSignature = oldSignature
                if let prepared = routingGraph.current, let previous = oldDefinition {
                    updateRoutingDefaults(prepared,definition:previous,project:oldProject)
                    try? publishRoutingRender(prepared,project:oldProject)
                }
                try rollback(); throw failure
            }
        }
        let wasRunning = engine.isRunning
        if wasRunning { pauseEngine() }
        fullRoutingRebuilds += 1
        var installed = false
        do {
            try awaitRenderQuiescence()
            // Replacing source connections can restart node clocks or resume past
            // the bounded retained range, also when the graph was not running.
            // Invalidate that range before the newly wired graph resumes, just as
            // a stop or pause does. Mix-only publications return above.
            onEngineStopped?()
            // Track plug-ins keep their stable owner when tracks are reordered (moving tracks
            // into, out of or within a group). Reassign their channel slots only while
            // callbacks are quiescent, with rollback ownership.
            let moving=oldInstances.filter { $0.value.ownerID?.hasPrefix("track:") == true }
            if moving.contains(where: { ownerIndices[$0.value.ownerID!] != $0.key.track }) {
                remappedPlugins=true
                for target in moving.keys {
                    generations[target,default:0] += 1
                    instances.removeValue(forKey:target);lastRestoredStates.removeValue(forKey:target);editorWindows.removeValue(forKey:target)
                    publish(nil,for:target)
                }
                var movedInstruments:[Int:Int?]=[:]
                for (previous,item) in moving {
                    guard let index=ownerIndices[item.ownerID!] else {
                        removedPlugins.append(item)
                        if previous.kind == "instrument" { movedInstruments[previous.track] = .some(nil) }
                        continue
                    }
                    let target=Target(track:index,kind:previous.kind,slot:previous.slot)
                    guard instances[target] == nil else { throw RackError.invalid("Two track plug-ins resolve to one live slot.") }
                    item.target=target;instances[target]=item;lastRestoredStates[target]=oldStates[previous];editorWindows[target]=oldWindows[previous]
                    generations[target,default:0] += 1
                    publish(RenderEndpoint(item),for:target)
                    // Held notes stay on the engine's old channel, whose release would now
                    // reach that channel's new plug-in. The host drops those voices now; the
                    // moved plug-in gets all-notes-off once the rewired graph runs again.
                    if previous.kind == "instrument", previous.track != index {
                        movedInstruments[previous.track]=index; releasedTracks.append(index)
                    }
                }
                if !movedInstruments.isEmpty { onInstrumentsMoved?(movedInstruments) }
            }
            removeRoutingMeters()
            if definition.enabled {
                let prepared = try prepareRouting(data,definition:definition)
                // These old master paths are rack-owned, so the assembler cannot
                // infer that they must stop producing retained delay/reverb tails.
                disconnectFlatMasterPaths()
                // Snapshot the old processor-input fanouts before source-policy
                // wiring can reconnect legacy wet-return inputs.
                var thrown: Error?
                if let refusal = catchingObjCException({ do { try self.routingGraph.install(prepared) } catch { thrown = error } }) {
                    let at = routingGraph.lastAttemptedConnection
                    NSLog("Routing refused at %@: %@", at, refusal)
                    throw RackError.invalid("The audio engine refused the routing change\(at.isEmpty ? "" : " (connecting \(at))"): \(refusal)")
                }
                if let thrown { throw thrown }
                installed = true
                wireCombinedSources(standard:definition.mixModel == .standard)
                updateRoutingDefaults(prepared,definition:definition,project:data)
                try publishRoutingRender(prepared,project:data)
                compensationLatencyFrames = prepared.latencyFrames
                latencyExceedsLimit = prepared.latencyFrames > nativeReturnCompensation.maximumFrames
            } else {
                try routingGraph.uninstall()
                publishRoutingValue(nil)
                try restoreFlatGraph()
            }
            routingProject = data; routingSignature = try makeRoutingSignature(data,definition:definition)
            installRoutingMeters()
            try commit()
            for track in 0..<realizedStrips { applyTrackMix(track) }
            // Removed plug-ins leave the graph while it is still stopped: AVAudioEngine raises on
            // disconnects in a running graph whose hardware input chain is connected (live inputs).
            for item in removedPlugins {
                if let previous=oldInstances.first(where: { $0.value === item })?.key { oldWindows[previous]?.close() }
                do { try beginRecoveryRelease(item.recovery) } catch { recordRecoveryError(error) }
                retiredUnits.append(item)
                if let refusal = catchingObjCException({ [self] in detachUnit(item) }) { recordRecoveryError(RackError.invalid("The audio engine refused to remove a plug-in: \(refusal)")) }
            }
            if wasRunning { try startEngine() }
            else if routingTransactionDepth == 0 { try routingGraph.commit() }
            routingRenderStore.collectRetired()
            onLatencyPlanChanged?()
            collectRetiredEndpoints()
            // AVAudioEngine may reset units while rewiring, discarding queued MIDI, so
            // release only after the new graph is published and resumed.
            for track in releasedTracks { sendMIDIMessage(track:track,status:0xB0,data1:123,data2:0) }
        } catch {
            let original = error
            stopEngine()
            try? awaitRenderQuiescence()
            removeRoutingMeters()
            if installed { try? routingGraph.rollback() }
            restorePluginAssignments()
            routingDefinition = oldDefinition; ownerIndices = oldOwners; ownersByIndex = oldIndices
            for (target,owner) in oldItemOwners { instances[target]?.ownerID = owner }
            routingProject = oldProject; routingSignature = oldSignature
            if let oldDefinition,oldDefinition.enabled {
                wireCombinedSources(standard:oldDefinition.mixModel == .standard)
                // Leaving routing detaches the old helper graph. Rebuild from the
                // preserved project if a device refused the proposed legacy graph.
                if routingGraph.current == nil,let previous = try? prepareRouting(oldProject,definition:oldDefinition) {
                    try? routingGraph.install(previous); try? routingGraph.commit()
                }
                if let previous = routingGraph.current {
                    updateRoutingDefaults(previous,definition:oldDefinition,project:oldProject)
                    try? publishRoutingRender(previous,project:oldProject)
                    compensationLatencyFrames = previous.latencyFrames
                }
            } else {
                try? routingGraph.uninstall(); publishRoutingValue(nil); try? restoreFlatGraph()
            }
            installRoutingMeters()
            try rollback()
            for track in 0..<realizedStrips { applyTrackMix(track) }
            if wasRunning { try? startEngine() }
            throw original
        }
    }

    /// Adding or removing owners (tracks, buses) with their own routes, while
    /// everything else keeps its exact topology and timing, changes the live graph without
    /// pausing: new strips attach and wire in, removed ones disconnect. Returns false when
    /// the change needs a full, stopped rebuild. Kits that add many tracks at once rely on this.
    func applyIncrementalRouting(_ data:[String:Any],definition:AudioRouting.Definition,previous:AudioRouting.Definition?,
                                         commit:() throws->Void,rollback:() throws->Void) throws -> Bool {
        guard definition.enabled, previous?.enabled == true, routingGraph.current != nil else { lastFullRebuildReason = "routing was off"; return false }
        guard routingCaptureOwners.isEmpty else { lastFullRebuildReason = "a stem capture is active"; return false }
        guard routingRebuildDeferral == 0 else { lastFullRebuildReason = "a rebuild is deferred"; return false }
        // Track plug-ins whose channel moves need the stopped reassignment below.
        guard instances.allSatisfy({ target,item in item.ownerID.map { ownerIndices[$0] == target.track } ?? true }) else {
            lastFullRebuildReason = "a track plug-in moved to another channel"; return false
        }
        let inputs = liveRoutingInputs(data,definition:definition)
        guard let change = try routingGraph.prepareIncremental(definition:definition,sources:inputs.sources,effects:routingEffects(definition),
            sourceLatencies:inputs.latencies,systemReturns:inputs.returns,extraMasterSources:inputs.extra,captureOwners:routingCaptureOwners) else {
            lastFullRebuildReason = routingGraph.lastIncrementalRefusal ?? "the change is not incremental"; return false
        }
        let prepared = change.prepared
        let standard = definition.mixModel == .standard
        do {
            // Meters are per owner index: removed owners lose theirs, moved tracks are re-tapped below.
            for owner in change.removedOwners + change.rewiredOwners {
                if let mix = routingGraph.current?.strips[owner]?.mix.automation { mix.meteringEnabled = false; routingMeterNodes.removeAll { $0 === mix } }
            }
            // A new track's channel source is wired before its strip joins the running graph:
            // AVAudioEngine does not render an input added later to a chain that joined empty
            // (a channel realized while playing, beyond the sixteen made at launch).
            let instrumentMaximum = Int(atomicLoad(automationOffsets.advanced(by:trackCount*Self.offsetStride)))
            for owner in change.addedOwners {
                guard let track = definition.owner(owner)?.trackIndex, track < realizedStrips else { continue }
                wireCombinedSource(track,standard:standard,latency:instrumentLatency(track),instrumentMaximum:instrumentMaximum)
            }
            // AVAudioEngine raises (rather than throws) for a live connection it refuses; the
            // change then falls back to the full rebuild below instead of ending the request.
            var thrown: Error?
            if let refusal = catchingObjCException({ do { try self.routingGraph.installIncremental(change) } catch { thrown = error } }) {
                routingGraph.abandonIncremental(change)
                lastFullRebuildReason = "the audio engine refused the live change: " + refusal + " (connecting " + routingGraph.lastAttemptedConnection + ")"
                return false
            }
            if let thrown { throw thrown }
            updateRoutingDefaults(prepared,definition:definition,project:data)
            try publishRoutingRender(prepared,project:data)
            routingProject = data; routingSignature = try makeRoutingSignature(data,definition:definition)
            for owner in change.addedOwners + change.rewiredOwners {
                guard let index = ownerIndices[owner], let mix = prepared.strips[owner]?.mix.automation else { continue }
                routingMeters[index].reset(); mix.meteringEnabled = true
                routingMeterNodes.append(mix)
            }
            try commit()
            for owner in change.addedOwners + change.rewiredOwners {
                if let track = definition.owner(owner)?.trackIndex, track < realizedStrips { applyTrackMix(track) }
            }
            if !engine.isRunning { try routingGraph.commit() }
            routingRenderStore.collectRetired()
            onLatencyPlanChanged?()
            incrementalRoutingChanges += 1
            return true
        } catch {
            // The live change could not complete: rebuild the previous graph while stopped.
            let original = error
            try rollback()
            routingSignature = ""
            throw original
        }
    }

    /// Inputs for a change made while rendering. A send route that is no longer needed stays
    /// until the next full rebuild: removing it live would cut its return's tail, and keeping
    /// a zero send costs only its gain stage.
    func liveRoutingInputs(_ data:[String:Any],definition:AudioRouting.Definition) -> RoutingInputs {
        let inputs = routingInputs(data,definition:definition)
        guard let current = routingGraph.current else { return inputs }
        let kept = Set(definition.owners.map(\.id))
        return RoutingInputs(sources:inputs.sources,latencies:inputs.latencies,returns:inputs.returns.map { item in
            let target = item.id == "delay" ? 4 : 5
            let installed = Set(current.builtinSendEndpoints.filter { $0.value[target] != nil && kept.contains($0.key) }.keys)
            return AudioRoutingGraph.SystemReturn(id:item.id,processor:item.processor,latencyFrames:item.latencyFrames,gains:item.gains,
                returnGain:item.returnGain,senders:item.senders.map { $0.union(installed) })
        },extra:inputs.extra)
    }

    /// Built-in inserts loaded, removed or reordered on routed strips (instances already
    /// updated) without changing the graph's nodes, routes or timing: each affected
    /// insert-chain node takes its new member list in place while audio keeps running.
    /// Returns false (nothing changed) when the change needs a rebuild. The caller commits
    /// (routingGraph.commitChainUpdate) or rolls back (rollbackRoutingMutation).
    @discardableResult func applyInsertChainUpdate(_ project:[String:Any]? = nil) throws -> Bool {
        guard routingDefinition?.enabled == true, let definition = routingDefinition, routingRebuildDeferral == 0, routingCaptureOwners.isEmpty else { return false }
        guard instances.allSatisfy({ target,item in item.ownerID.map { ownerIndices[$0] == target.track } ?? true }) else { return false }
        notifyStateCaptureMutation()
        let data = project ?? routingProject
        let inputs = liveRoutingInputs(data,definition:definition)
        guard let update = try routingGraph.prepareChainUpdate(definition:definition,sources:inputs.sources,effects:routingEffects(definition),
            sourceLatencies:inputs.latencies,systemReturns:inputs.returns,extraMasterSources:inputs.extra,captureOwners:routingCaptureOwners) else {
            lastFullRebuildReason = "insert chain: " + (routingGraph.lastChainRefusal ?? "not in place")
            return false
        }
        try routingGraph.applyChainUpdate(update)
        do {
            // Automation offsets and plug-in bindings follow the new insert positions.
            try publishRoutingRender(update.prepared,project:data)
            routingSignature = try makeRoutingSignature(data,definition:definition)
        } catch {
            let original = error
            try? routingGraph.rollbackChainUpdate()
            if let previous = routingGraph.current { try? publishRoutingRender(previous,project:data) }
            routingSignature = ""
            throw original
        }
        insertChainUpdates += 1
        onLatencyPlanChanged?()
        return true
    }

    func rollbackRoutingMutation() throws {
        try routingGraph.rollbackChainUpdate()
        if routingDefinition?.enabled == true {
            removeRoutingMeters()
            try routingGraph.rollback()
            guard let definition = routingDefinition,let previous = routingGraph.current else {
                throw RackError.invalid("The previous routing graph could not be recovered.")
            }
            wireCombinedSources(standard:definition.mixModel == .standard)
            try publishRoutingRender(previous,project:routingProject)
            routingSignature = try makeRoutingSignature(routingProject,definition:definition)
            compensationLatencyFrames = previous.latencyFrames
            installRoutingMeters()
            onLatencyPlanChanged?()
        } else { try restoreFlatGraph() }
    }
}

/// Runs body and returns the reason of an Objective-C exception it raised, or nil. An
/// exception skips Swift cleanups in the frames it unwinds, so their references leak;
/// this is only for failures that would otherwise end the process.
func catchingObjCException(_ body: @escaping () -> Void) -> String? {
    final class Body { let run: () -> Void; init(_ run: @escaping () -> Void) { self.run = run } }
    let context = Unmanaged.passRetained(Body(body)).toOpaque()
    defer { Unmanaged<Body>.fromOpaque(context).release() }
    var message = [CChar](repeating: 0, count: 512)
    let raised = st_objc_try({ pointer in Unmanaged<Body>.fromOpaque(pointer!).takeUnretainedValue().run() }, context, &message, message.count)
    return raised == 0 ? nil : String(cString: message)
}

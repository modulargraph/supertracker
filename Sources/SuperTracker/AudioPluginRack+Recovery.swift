import AppKit
import AudioToolbox
import AVFoundation
import CoreAudioKit
import CoreFoundation
import Darwin

/// Crash recovery records, preview state restore, render failures and closing every plug-in.
extension AudioPluginRack {
    func recoveryIdentity(_ component:Component)->PluginRecoveryStore.Identity {
        PluginRecoveryStore.Identity(format:component.format == "VST3" ? .vst3 : component.format == "VST2" ? .vst2 : .audioUnit,
            componentID:component.id,name:component.name,bundlePath:component.bundlePath)
    }

    /// First-party studio effects run SuperTracker's own DSP, not vendor code, so they
    /// never enter the crash journal. Quarantining them after an unexpected quit left
    /// kit, bus and track inserts silently unloaded ("Reading parameters…").
    static func isRecoveryTracked(componentID:String,format:String?) -> Bool {
        format != "Builtin" && !componentID.hasPrefix("stfx:")
    }

    func beginRecoveryLoad(_ component:Component,target:Target) throws -> RecoveryRecord? {
        guard let store = recoveryStore, Self.isRecoveryTracked(componentID:component.id,format:component.format) else { return nil }
        let identity = recoveryIdentity(component)
        let token = try store.beginLoad(identity,instanceHint:"track:\(target.track) \(target.kind):\(target.slot)")
        let record = RecoveryRecord(token:token,identity:identity)
        recoveryRecords[token] = record
        return record
    }

    func recoveryDidLoad(_ record:RecoveryRecord?) throws {
        guard let record, let store = recoveryStore else { return }
        try store.didLoad(record.token); record.phase = .active
    }

    /// Candidate resources have never been attached or published to readers.
    /// Call only after its asynchronous creation/state work has settled.
    @MainActor func abandonPreparedUnit(_ prepared:PreparedUnit) {
        guard !prepared.abandoned else { return }
        // A failed live rollback can still own graph-attached nodes; never
        // destroy their processor here.
        guard prepared.node.engine == nil else { return }
        prepared.abandoned=true
        auStateReuse.remove(ObjectIdentifier(prepared.node.auAudioUnit))
        if let record=prepared.recovery {
            record.abandoned=true
            if record.phase == .loading { record.phase = .failedLoad }
            else { do { try beginRecoveryRelease(record) } catch { recordRecoveryError(error) } }
        }
        if prepared.node.auAudioUnit.renderResourcesAllocated { prepared.node.auAudioUnit.deallocateRenderResources() }
        if let adapter=prepared.node.auAudioUnit as? HostedPluginAdapter {
            adapter.closePlugin(); prepared.recovery?.vendorClosed=true
        }
    }

    func beginRecoveryRelease(_ record:RecoveryRecord?) throws {
        guard let record, let store = recoveryStore, record.phase != .releasing else { return }
        try store.beginRelease(record.token); record.phase = .releasing
    }

    func recordRecoveryError(_ error:Error) {
        let message = "Plug-in recovery journal: "+error.localizedDescription
        if !recoveryWarnings.contains(message) {
            recoveryWarnings.append(message)
            if recoveryWarnings.count > 32 { recoveryWarnings.removeFirst() }
            onRecoveryError?(message)
        }
    }

    /// AU ownership is released on the control thread before this collection pass.
    /// Weak references detect framework-retained wrappers; VST3 also closes its
    /// processor explicitly. This does not claim to observe arbitrary vendor workers.
    func collectRecoveryRecords() {
        guard let store = recoveryStore else { return }
        for record in Array(recoveryRecords.values) where record.released {
            do {
                if record.abandoned && record.phase == .active { try beginRecoveryRelease(record) }
                if record.phase == .failedLoad { try store.loadFailed(record.token) }
                else if record.phase == .releasing { try store.didRelease(record.token) }
                else { continue }
                recoveryRecords.removeValue(forKey:record.token)
            } catch { recordRecoveryError(error) }
        }

    }

    func recoveryReport()->[String:Any] {
        collectRecoveryRecords()
        var report = recoveryStore?.recoveryReport().dictionary ?? ["suspects":[],"warnings":[],"liveSessionsIgnored":0]
        // Journals written before built-in effects were exempt may still list them.
        if let suspects = report["suspects"] as? [[String:Any]] {
            report["suspects"] = suspects.filter { suspect in
                let identity = suspect["identity"] as? [String:Any] ?? [:]
                return Self.isRecoveryTracked(componentID:identity["componentID"] as? String ?? "",format:nil)
            }
        }
        report["enabled"] = recoveryStore != nil
        report["warnings"] = (report["warnings"] as? [String] ?? [])+recoveryWarnings
        report["pendingInstances"] = recoveryRecords.count
        report["cleanShutdown"] = recoveryShutdownFinished
        return report
    }

    func beginRecoveryShutdown() {
        recoveryShuttingDown = true
        guard let store = recoveryStore else { return }
        do { try store.beginShutdown(); recoveryShutdownStarted = true }
        catch { recordRecoveryError(error) }
    }

    func beginRecoveryModuleCleanup() throws {
        collectRecoveryRecords()
        guard recoveryShuttingDown, recoveryRecords.isEmpty, instances.isEmpty,
              retiredUnits.isEmpty, endpointStore.activeReaders == 0 else {
            throw RackError.invalid("Plug-in resources are still being released; recovery cannot mark module cleanup complete.")
        }
        guard let store = recoveryStore else { return }
        if !recoveryShutdownStarted { try store.beginShutdown(); recoveryShutdownStarted = true }
        try store.beginModuleCleanup()
    }

    func finishRecoveryShutdown() throws {
        guard let store = recoveryStore else { return }
        try store.finishShutdown(); recoveryShutdownFinished = true
    }

    func restoreTrackedState(_ state:String,to instance:Instance) throws {
        notifyStateCaptureMutation()
        if let record = instance.recovery, let store = recoveryStore {
            _ = try store.beginLoad(record.identity,reusingToken:record.token)
            record.phase = .loading
        }
        try restoreState(state,to:instance.node.auAudioUnit)
        try recoveryDidLoad(instance.recovery)
    }

    /// A private preview must not leave automated parameter changes in the live
    /// processors. The caller holds the capture operation and has stopped/drained
    /// the graph; this restores the same instances without loading or replacing.
    @MainActor func restorePreviewStates(_ saved:[[String:Any]]) throws {
        guard !engine.isRunning else { throw RackError.invalid("Stop audio before restoring preview state.") }
        try awaitRenderQuiescence()
        let pairs = try saved.map { descriptor -> (Instance,String) in
            let item = try instance(descriptor)
            guard descriptor["component"] as? String == item.component.id,
                  descriptor["instanceID"] as? String == item.instanceID,
                  let state=descriptor["state"] as? String else {
                throw RackError.invalid("A preview plug-in changed before its state could be restored.")
            }
            return (item,state)
        }
        guard pairs.count == instances.count else { throw RackError.invalid("The preview plug-in graph changed before restoration.") }
        for (item,state) in pairs {
            try restoreTrackedState(state,to:item)
            if let descriptor=saved.first(where:{$0["instanceID"] as? String == item.instanceID}),let bypass=descriptor["bypass"] as? Bool {
                item.node.auAudioUnit.shouldBypassEffect=bypass
            }
            publish(RenderEndpoint(item),for:item.target)
        }
        refreshLatencyCompensation()
    }

    /// Begin a new render run after the graph has stopped/reset. No AU property is
    /// queried in a render observer; exact status retrieval is control-thread work.
    func resetRenderFailures() {
        precondition(Thread.isMainThread)
        let diagnosticsQuiescent = !engine.isRunning && endpointStore.activeReaders == 0
        for instance in instances.values {
            instance.renderMonitor.reset(resetDiagnostics:diagnosticsQuiescent)
        }
    }

    /// Returns the first unreported upstream failure. AVAudioEngine may absorb an
    /// AU error into silent output, so successful renderOffline alone is insufficient.
    func renderFailure() -> [String: Any]? {
        precondition(Thread.isMainThread)
        for instance in instances.values {
            guard let count = instance.renderMonitor.takeFailure() else { continue }
            var status: OSStatus = noErr
            var size = UInt32(MemoryLayout<OSStatus>.size)
            let propertyStatus = AudioUnitGetProperty(instance.node.audioUnit,
                kAudioUnitProperty_LastRenderError, kAudioUnitScope_Global, 0, &status, &size)
            let detail = propertyStatus == noErr && status != noErr ? " (OSStatus \(status))" : ""
            var result: [String: Any] = ["component": instance.component.id,
                "pluginName": instance.component.name, "track": instance.target.track,
                "kind": instance.target.kind, "slot": instance.target.slot, "errorCount": count,
                "propertyStatus": propertyStatus,
                "message": "\(instance.component.name) failed to render on track \(instance.target.track + 1)\(detail)."]
            if propertyStatus == noErr && status != noErr { result["osStatus"] = status }
            return result
        }
        return nil
    }

    /// Shutdown admission is owned by AudioHost. Stop the graph first, then call this
    /// after its registered readers drain. A busy graph is left intact for a later
    /// control-thread retry; no audio callback is blocked or asked to release a unit.
    @MainActor @discardableResult func closeAllPluginsAfterStop() -> Bool {
        // The host retries this stopped control-thread barrier until every
        // registered render callback has returned.
        guard !engine.isRunning, endpointStore.activeReaders == 0 else { return false }
        collectRecoveryRecords()
        notifyStateCaptureMutation()
        removeRoutingMeters()
        do { try routingGraph.uninstall() } catch { recordRecoveryError(error); return false }
        publishRoutingValue(nil); routingRenderStore.collectRetired()
        flatStepDestinations = []; flatStepSignature = []
        flatStepRenderStore.publish(nil,at:0); flatStepRenderStore.collectRetired()
        for target in generations.keys { generations[target, default: 0] += 1 }
        for (target, instance) in instances {
            do { try beginRecoveryRelease(instance.recovery) }
            catch { instance.recovery?.abandoned = true; recordRecoveryError(error) }
            publish(nil, for: target)
            editorWindows.removeValue(forKey: target)?.close()
            retiredUnits.append(instance)
            detachUnit(instance)
        }
        instances.removeAll(); lastRestoredStates.removeAll()
        collectRetiredEndpoints()
        auStateReuse.removeAll()
        return true
    }
}

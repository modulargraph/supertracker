import AppKit
import AudioToolbox
import AVFoundation
import CoreAudioKit
import CoreFoundation
import Darwin

/// Flat (unrouted) step-FX destinations and their published endpoints.
extension AudioPluginRack {
    /// New identity-bound tracker locks use host destinations even without custom
    /// routing. Existing slot-based legacy lanes keep their original event path.
    /// Pair a changed immutable endpoint map with the C project while callbacks
    /// are quiescent; unchanged maps can accept normal pattern edits directly.
    func configureFlatStepFX(_ data:[String:Any],commit:() throws->Void = {},rollback:() throws->Void = {}) throws {
        let destinations = try AudioProject.pluginStepFX(data).flatDestinations
        let signature = makeFlatStepSignature(destinations)
        if signature == flatStepSignature {
            do { try commit(); refreshFlatStepEndpoints() }
            catch { try rollback(); refreshFlatStepEndpoints(); throw error }
            return
        }
        notifyStateCaptureMutation()
        let previous = flatStepDestinations,wasRunning = engine.isRunning
        if wasRunning { pauseEngine() }
        do {
            try awaitRenderQuiescence()
            flatStepDestinations = destinations
            refreshFlatStepEndpoints()
            try commit()
            refreshFlatStepEndpoints()
            onLatencyPlanChanged?()
            if wasRunning { try startEngine() }
        } catch {
            let original = error
            stopEngine(); try? awaitRenderQuiescence()
            flatStepDestinations = previous
            try rollback()
            refreshFlatStepEndpoints()
            onLatencyPlanChanged?()
            if wasRunning { try? startEngine() }
            throw original
        }
    }

    func flatStepInstance(_ entry:AudioPluginStepFX.FlatDestination)->Instance? {
        guard (0..<trackCount).contains(entry.track), (-1..<maximumEffects).contains(entry.slot),
              let item = instances[Target(track:entry.track,kind:entry.slot < 0 ? "instrument" : "effect",slot:max(0,entry.slot))],
              item.instanceID == entry.instanceID else { return nil }
        return item
    }
    func makeFlatStepSignature(_ entries:[AudioPluginStepFX.FlatDestination])->[FlatStepIdentity] {
        entries.map { entry in
            let item = flatStepInstance(entry)
            let valid = (0..<trackCount).contains(entry.track) && (0..<maximumEffects).contains(entry.slot)
            let offset:Int64 = valid ? atomicLoad(automationOffsets.advanced(by:Self.offsetIndex(track:entry.track,slot:entry.slot))) : 0
            return FlatStepIdentity(destination:entry.destination,track:entry.track,slot:entry.slot,instanceID:entry.instanceID,
                                    unit:item.map { ObjectIdentifier($0.node.auAudioUnit) },offset:offset)
        }
    }
    func refreshFlatStepEndpoints() {
        flatStepSignature = makeFlatStepSignature(flatStepDestinations)
        guard !flatStepDestinations.isEmpty else { flatStepRenderStore.publish(nil,at:0); return }
        let count = (flatStepDestinations.map(\.destination).max() ?? -1)+1
        var bindings = [AudioRoutingRender.Binding](repeating:.unavailable,count:count)
        for (entry,signature) in zip(flatStepDestinations,flatStepSignature) {
            if let item = flatStepInstance(entry) {
                bindings[entry.destination] = .plugin(.init(unit:item.node.auAudioUnit,offset:Int(signature.offset),clock:item.clockOffset))
            }
        }
        flatStepRenderStore.publish(AudioRoutingRender(standard:false,bindings:bindings,tracks:[],delaySends:[],reverbSends:[],
                                                       pluginOffsets:[],controls:[]),at:0)
        flatStepRenderStore.collectRetired()
    }
}

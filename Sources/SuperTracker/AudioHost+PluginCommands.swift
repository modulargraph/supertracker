import Foundation
import AppKit
import AVFoundation
import AudioToolbox
import CoreAudio
import TrackerEngine

/// Plug-in loading, moves, state, parameters, presets and editors.
extension AudioHost {
    /// Returns nil for commands outside this family.
    @MainActor func handlePluginCommand(_ command: String, _ payload: [String: Any]) async throws -> Any? {
        if let result = try await handleInstrumentEffectCommand(command,payload) { return result }
        switch command {
        case "plugins.list": return rack.list()
        case "plugins.recovery": return rack.recoveryReport()
        case "plugins.recovery.retry": return try await retryRecoveredPlugins(payload)
        case "plugin.load", "plugin.loadNKS":
            graphMutationsInFlight += 1
            defer { graphMutationsInFlight -= 1 }
            let identity = projectIdentity
            await awaitPluginRestoration()
            try requireInteractiveMutation()
            guard identity == projectIdentity else { throw AudioSampleError.invalid("Plug-in loading was superseded by another project.") }
            let loadID = UUID()
            let task = Task { @MainActor in
                if command == "plugin.loadNKS" { return try await self.rack.loadNKS(payload) }
                return try await self.rack.load(payload)
            }
            directPluginLoads[loadID] = task
            defer { directPluginLoads.removeValue(forKey:loadID) }
            let result = try await withTaskCancellationHandler(operation:{
                try await task.value
            },onCancel:{ task.cancel() })
            // A candidate that committed before cancellation must still publish
            // its descriptor; cancellation inside load prevents earlier commits.
            try requireInteractiveMutation()
            guard identity == projectIdentity else { throw AudioSampleError.invalid("Plug-in loading was superseded by another project.") }
            if let descriptor = result as? [String:Any] {
                let previous = storedPluginDescriptions(project)
                let removedIDs = Set(previous.filter { pluginKey($0) == pluginKey(descriptor) && $0["instanceID"] as? String != descriptor["instanceID"] as? String }.compactMap { $0["instanceID"] as? String })
                var desired = previous.filter { pluginKey($0) != pluginKey(descriptor) }
                desired.append(descriptor); project["audioPlugins"] = desired
                removeSidechains(to:removedIDs)
                try refreshRoutingProject()
            }
            appliedPluginSignature = ""; return result
        case "plugin.move":
            guard graphMutationsInFlight == 0 else { throw AudioSampleError.invalid("Finish the pending audio graph change before moving an insert.") }
            graphMutationsInFlight += 1
            defer { graphMutationsInFlight -= 1 }
            let identity = projectIdentity,generation = projectGeneration
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
            let track = try rack.ownerIndex(payload)
            let owner = pluginOwner(payload)
            let original = project, oldSignature = appliedPluginSignature
            let desired = storedPluginDescriptions(project)
            let loaded = rack.loadedEffects(track:track)
            let requested = desired.filter { pluginOwner($0) == owner && $0["kind"] as? String == "effect" }
            guard requested.count == loaded.count, requested.allSatisfy({ item in
                loaded.contains { pluginKey($0) == pluginKey(item) && pluginIdentityMatches(item,$0) }
            }) else { throw AudioSampleError.invalid("Load or remove unavailable inserts on this track before changing their order.") }
            var result = try rack.move(payload,commit:{ change in
                let effects = change["plugins"] as? [[String:Any]] ?? []
                let map = Dictionary(uniqueKeysWithValues:(change["slotMap"] as? [[String:Any]] ?? []).compactMap { item -> (Int,Int)? in
                    guard let from = item["fromSlot"] as? Int, let to = item["toSlot"] as? Int else { return nil }; return (from,to)
                })
                self.project["audioPlugins"] = desired.filter { !(self.pluginOwner($0) == owner && $0["kind"] as? String == "effect") }+effects
                self.remapEffectAutomation(track:track,owner:payload["owner"] as? String,slots:map)
                try self.applyEngineProject(self.project)
                self.appliedPluginSignature = self.pluginSignature(self.pluginDescriptions(self.project))
            },rollback:{
                self.project = original; self.appliedPluginSignature = oldSignature
                try self.applyEngineProject(original)
            })
            result["audioPlugins"] = storedPluginDescriptions(project)
            try refreshRoutingProject()
            if let routing = project["routing"] { result["routing"] = routing }
            // The caller captures undo before applying this response. Do not publish
            // an intermediate plugins.changed event that would overwrite that baseline.
            return result
        case "plugin.remove":
            graphMutationsInFlight += 1
            defer { graphMutationsInFlight -= 1 }
            let identity = projectIdentity
            await awaitPluginRestoration()
            try requireInteractiveMutation()
            guard identity == projectIdentity else { throw AudioSampleError.invalid("Plug-in removal was superseded by another project.") }
            let removedIDs = Set(storedPluginDescriptions(project).filter { pluginKey($0) == pluginKey(payload) }.compactMap { $0["instanceID"] as? String })
            let result = try rack.remove(payload)
            project["audioPlugins"] = storedPluginDescriptions(project).filter { pluginKey($0) != pluginKey(payload) }
            removeSidechains(to:removedIDs)
            try refreshRoutingProject()
            appliedPluginSignature = ""; return result
        case "plugin.editor": return try await rack.editor(payload)
        case "plugin.parameters": return try explainingUnloadedPlugin(payload) { try rack.parameters(payload) }
        case "plugin.parameter": return try explainingUnloadedPlugin(payload) { try rack.parameter(payload) }
        case "plugin.captureBasePatch":
            let identity=projectIdentity,generation=projectGeneration
            let release=try await acquireStateCaptureOperation()
            defer { release() }
            try requireInteractiveMutation()
            guard identity == projectIdentity,generation == projectGeneration else {
                throw AudioSampleError.invalid("The project changed while base capture was waiting. Retry the operation.")
            }
            let captureTransport=st_engine_status(engine)
            guard captureTransport.playing == 0,captureTransport.transportPending == 0,
                  captureTransport.countIn == 0,recording == nil,!recordingStopping,!freezeTrackRuntime.busy else {
                throw AudioSampleError.invalid("Stop playback and recording before updating the base patch.")
            }
            return try rack.captureBasePatch(payload)
        case "plugin.state":
            var releaseCapture:(()->Void)?
            defer { releaseCapture?() }
            if payload["state"] != nil {
                guard payload["state"] is String, payload["owner"] != nil || payload["track"] != nil else {
                    throw AudioSampleError.invalid("Restoring plug-in state requires a serialized state string and a target owner or track.")
                }
                let identity = projectIdentity,generation = projectGeneration
                releaseCapture = try await acquireStateCaptureOperation()
                try requireInteractiveMutation()
                guard identity == projectIdentity, generation == projectGeneration,
                      routingFailure == nil, unavailableProject == nil else {
                    throw AudioSampleError.invalid("The project changed while plug-in state was waiting to restore. Retry the operation.")
                }
            } else { releaseCapture = try await preparePluginStateCapture(payload.isEmpty ? nil : payload) }
            if payload.isEmpty { return pluginSnapshot() }
            if pendingPluginRestore != nil {
                guard payload["state"] == nil else { throw AudioSampleError.invalid("Wait for the project's Audio Units to finish loading before changing their saved state.") }
                if let desired = pluginDescriptions(project).first(where:{pluginKey($0) == pluginKey(payload)}) { return desired }
                throw AudioSampleError.invalid("This Audio Unit is not part of the project being loaded.")
            }
            do { return try rack.state(payload) }
            catch {
                if payload["state"] == nil, var desired = pluginDescriptions(project).first(where:{pluginKey($0) == pluginKey(payload)}) {
                    desired["stateError"] = error.localizedDescription
                    return desired
                }
                throw error
            }
        case "plugin.preset": return try rack.preset(payload)
        default: return nil
        }
    }
}

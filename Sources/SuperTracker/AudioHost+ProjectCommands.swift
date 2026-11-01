import Foundation
import AppKit
import AVFoundation
import AudioToolbox
import CoreAudio
import TrackerEngine

/// Project publication, snapshots, routing and render diagnostics.
extension AudioHost {
    /// Returns nil for commands outside this family.
    @MainActor func handleProjectCommand(_ command: String, _ payload: [String: Any]) async throws -> Any? {
        switch command {
        case "engine.project":
            let data = payload["project"] as? [String: Any] ?? payload
            if payload["replace"] as? Bool == true, data["routing"] != nil {
                do { _ = try rack.validateRouting(data) }
                catch let error as AudioRouting.RoutingError {
                    try await retainUnavailableRouting(data,error:error)
                    return status()
                }
            }
            let forcePluginState = (payload["replace"] as? Bool ?? false) || (payload["forcePluginState"] as? Bool ?? false)
            try applyProject(data,forcePluginState:forcePluginState)
            if payload["awaitPlugins"] as? Bool == true {
                // A renderer sync must settle this publication before a mapped
                // control can write. Follow this task only: a replacement belongs
                // to its own request, and must not turn this acknowledgement into
                // success for a different project.
                let identity = projectIdentity, generation = projectGeneration
                let restoration = restoreTask
                await restoration?.value
                try Task.checkCancellation()
                guard !closed, identity == projectIdentity, generation == projectGeneration else {
                    throw AudioSampleError.invalid("The audio project update was superseded by a newer project.")
                }
            }
            return status()
        case "engine.snapshot":
            if let unavailableProject { return unavailableProject }
            let releaseCapture = try await preparePluginStateCapture()
            defer { releaseCapture() }
            var snapshot = exportOriginalProject ?? project; snapshot["audioPlugins"] = pluginSnapshot(); return snapshot
        case "audio.renderDiagnostics":
            var result:[String:Any]=["units":rack.renderDiagnostics()]
            result["inputCapture"]=inputCapture.diagnostics
            result["renderClock"]=renderClockDiagnostics?.snapshot() ?? ["enabled":false]
            result["inputArmGeneration"]=String(inputArmGeneration)
            result["graphRunning"]=graph.isRunning; result["manualRendering"]=graph.isInManualRenderingMode
            result["sampleRates"]=["native":sampleRate,"mainMixerOutput":graph.mainMixerNode.outputFormat(forBus:0).sampleRate,"outputInput":graph.outputNode.inputFormat(forBus:0).sampleRate,"firstSource":sources.first?.outputFormat(forBus:0).sampleRate ?? 0]
            result["projectGeneration"]=String(projectGeneration); result["projectIdentity"]=String(projectIdentity)
            result["exporting"]=exporting; result["transportPlaying"]=st_engine_status(engine).playing != 0
            let now=mach_absolute_time()
            if let snapshot=renderHeartbeat.snapshot() { lastRenderHeartbeat=snapshot }
            if let value=lastRenderHeartbeat {
                result["heartbeat"]=["began":value.began,"completed":value.completed,"failed":value.failed,
                    "lastSucceeded":value.lastCompletionSucceeded,"inFlight":value.began != value.completed,
                    "completionAge":value.completed > 0 && now >= value.lastCompletionTicks
                        ? Double(now-value.lastCompletionTicks)*secondsPerHostTick : NSNull(),
                    "nowTicks":String(now),"completionTicks":String(value.lastCompletionTicks)] as [String:Any]
            } else { result["heartbeat"]=NSNull() }
            return result
        case "routing.capabilities":
            var capabilities = routingCapabilities()
            capabilities["audioExport"] = AudioExportFormat.capabilities
            return capabilities
        case "routing.status": return routingStatus()
        case "routing.validate":
            guard let candidate = payload["project"] as? [String:Any] else {
                throw AudioSampleError.invalid("Routing validation requires the complete project.")
            }
            return try rack.validateRouting(candidate)
        case "routing.apply":
            guard graphMutationsInFlight == 0 else { throw AudioSampleError.invalid("Finish the pending plug-in or audio graph change before changing routing.") }
            guard let candidate = payload["project"] as? [String:Any] else {
                throw AudioSampleError.invalid("Routing changes require the complete project.")
            }
            graphMutationsInFlight += 1
            defer { graphMutationsInFlight -= 1 }
            let identity = projectIdentity, generation = projectGeneration
            await awaitPluginRestoration()
            try requireInteractiveMutation()
            guard identity == projectIdentity, generation == projectGeneration else {
                throw AudioSampleError.invalid("The project changed while routing was being prepared.")
            }
            _ = try rack.validateRouting(candidate)
            try applyProject(candidate)
            // Repairing retained routing can also load its desired plug-ins.
            // Keep the graph-edit gate held until that publication settles, so
            // mapped edits released by the UI cannot race the restored state.
            let appliedIdentity = projectIdentity, appliedGeneration = projectGeneration
            let restoration = restoreTask
            await restoration?.value
            try Task.checkCancellation()
            guard !closed, appliedIdentity == projectIdentity, appliedGeneration == projectGeneration else {
                throw AudioSampleError.invalid("The routing update was superseded by a newer project.")
            }
            return ["project":project,"routing":project["routing"] ?? NSNull(),"status":rack.routingStatus()]
        default: return nil
        }
    }
}

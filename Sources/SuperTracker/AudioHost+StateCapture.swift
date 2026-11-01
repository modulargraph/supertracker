import Foundation
import AppKit
import AVFoundation
import AudioToolbox
import CoreAudio
import TrackerEngine

/// Serialized plug-in state capture (saving and snapshots) against concurrent graph edits.
extension AudioHost {
    @MainActor func acquireStateCaptureOperation() async throws -> ()->Void {
        try Task.checkCancellation()
        let id = UUID()
        if stateCaptureOwner == nil { stateCaptureOwner = id }
        else {
            try await withTaskCancellationHandler(operation:{
                try await withCheckedThrowingContinuation { (continuation:CheckedContinuation<Void,Error>) in
                    if Task.isCancelled { continuation.resume(throwing:CancellationError()) }
                    else { stateCaptureWaiters.append((id,continuation)) }
                }
            },onCancel:{ [weak self] in
                Task { @MainActor in self?.cancelStateCaptureWaiter(id) }
            })
        }
        do { try Task.checkCancellation() }
        catch { releaseStateCaptureOperation(id); throw error }
        return { [weak self] in self?.releaseStateCaptureOperation(id) }
    }

    @MainActor func cancelStateCaptureWaiter(_ id:UUID) {
        guard let index = stateCaptureWaiters.firstIndex(where:{$0.id == id}) else { return }
        stateCaptureWaiters.remove(at:index).continuation.resume(throwing:CancellationError())
    }

    @MainActor func releaseStateCaptureOperation(_ id:UUID) {
        guard stateCaptureOwner == id else { return }
        stateCaptureOwner = nil
        if !stateCaptureWaiters.isEmpty {
            let next = stateCaptureWaiters.removeFirst()
            stateCaptureOwner = next.id; next.continuation.resume()
        }
    }

    /// Admission covers preparation and the final opaque copy. Overlapping
    /// autosaves therefore cannot issue newer tickets before an earlier caller
    /// consumes and releases its request-local preparation.
    @MainActor func preparePluginStateCapture(_ payload:[String:Any]? = nil) async throws -> ()->Void {
        let identity = projectIdentity, generation = projectGeneration
        let releaseOperation = try await acquireStateCaptureOperation()
        var preparation:AudioPluginRack.StateCapturePreparation?
        do {
            guard !closed, identity == projectIdentity, generation == projectGeneration,
                  routingFailure == nil, unavailableProject == nil else {
                throw AudioSampleError.invalid("The project changed while plug-in edits were being saved. Retry the save operation.")
            }
        // A pending restore's desired descriptors must remain authoritative. During
        // manual export its render loop owns state access and cannot be paused here.
            guard pendingPluginRestore == nil, !exporting else { return releaseOperation }
            let isCurrent = { [weak self] in
                guard let self else { return false }
                return !self.closed && identity == self.projectIdentity && generation == self.projectGeneration &&
                    self.routingFailure == nil && self.unavailableProject == nil && !self.exporting && self.pendingPluginRestore == nil
            }
            preparation = try await rack.prepareStateCapture(payload,isCurrent:isCurrent)
            try Task.checkCancellation()
            guard isCurrent() else {
                throw AudioSampleError.invalid("The project changed while plug-in edits were being saved. Retry the save operation.")
            }
            let cancelPreparation = preparation!.cancel
            return { cancelPreparation(); releaseOperation() }
        } catch {
            preparation?.cancel(); releaseOperation(); throw error
        }
    }
}

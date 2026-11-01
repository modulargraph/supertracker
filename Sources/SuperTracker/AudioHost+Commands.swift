import Foundation
import AppKit
import AVFoundation
import AudioToolbox
import CoreAudio
import TrackerEngine

/// Bridge command entry: admission rules shared by every command, then dispatch to the command families.
extension AudioHost {
    /// Inclusive pattern rows from `{start,end}`; nil (no loop) for anything else.
    static func patternLoop(_ value: Any?) -> (start: Int, end: Int)? {
        guard let loop = value as? [String: Any],
              let start = (loop["start"] as? NSNumber)?.intValue, let end = (loop["end"] as? NSNumber)?.intValue,
              start >= 0, end >= start, start < 256 else { return nil }
        return (start, min(end, 255))
    }

    /// Wrapped by `handle` in AudioFreezeTrack.swift, which projects frozen tracks.
    @MainActor func handleCore(_ command: String, payload: [String: Any]) async throws -> Any {
        guard !closed else { throw AudioSampleError.invalid("The audio engine is closed.") }
        // An explicit transport interruption also cancels Play or step audition
        // waiting for assets, including Stop/panic's early export return.
        if command == "transport.stop" || command == "engine.panic" || command == "transport.seek" {
            playRequestGeneration &+= 1
            previewGeneration &+= 1
        }
        if let routingFailure, !["engine.project","engine.snapshot","engine.panic","transport.stop","sample.stopPreview",
            "routing.apply","routing.validate","routing.status","routing.capabilities","transport.recordArm","transport.loop","audio.renderDiagnostics","audio.devices","audio.stopRecord","midi.devices","midi.clock","plugins.list","plugins.recovery"].contains(command) {
            throw AudioSampleError.invalid("Project routing is unavailable: "+routingFailure+" Repair the routing before playing or editing audio.")
        }
        if !lineInputs.isEmpty && ["audio.export","audio.bounceTrack","audio.kitPreview"].contains(command) {
            throw AudioSampleError.invalid("Record live line inputs to samples before offline export, or disable their live input routes.")
        }
        if exporting {
            if command == "transport.stop" || command == "engine.panic" { exportCancelled = true; return status() }
            if !["engine.snapshot","audio.devices","midi.devices","midi.clock","plugins.list","plugin.parameters","plugin.state","plugins.recovery","routing.capabilities","routing.status","transport.recordArm","audio.renderDiagnostics"].contains(command) {
                throw AudioSampleError.invalid("Wait for export to finish, or press Stop to cancel it.")
            }
        }
        if ["engine.project","engine.panic","transport.play","transport.stop","transport.seek","transport.loop","transport.queue","transport.recordArm","step.preview",
            "audio.configure","audio.export","audio.kitPreview","audio.bounceTrack","audio.record","audio.stopRecord",
            "sample.preview","sample.stopPreview"].contains(command) { rack.notifyStateCaptureMutation() }
        if let result = try await handleProjectCommand(command, payload) { return result }
        if let result = try await handleTransportCommand(command, payload) { return result }
        if let result = try await handleSampleCommand(command, payload) { return result }
        if let result = try await handlePluginCommand(command, payload) { return result }
        if let result = try await handleAudioCommand(command, payload) { return result }
        if let result = try await handleRenderSelectionCommand(command, payload) { return result }
        throw AudioSampleError.invalid("Unsupported audio command: \(command)")
    }
}

import Foundation
import AppKit
import AVFoundation
import AudioToolbox
import CoreAudio
import TrackerEngine

/// Audio devices, inputs, recording, export and bounces.
extension AudioHost {
    /// Returns nil for commands outside this family.
    @MainActor func handleAudioCommand(_ command: String, _ payload: [String: Any]) async throws -> Any? {
        switch command {
        case "audio.devices": return audioDevices()
        case "audio.inputs", "audio.inputs.configure":
            return try await configureLineInputs(payload)
        case "audio.configure":
            graphMutationsInFlight += 1
            defer { graphMutationsInFlight -= 1 }
            let result = try await configureAudio(payload)
            rememberAudioDevices(payload)
            refreshMIDIClock() // the automatic clock lead follows the interface and buffer size
            return result
        case "audio.record":
            graphMutationsInFlight += 1
            defer { graphMutationsInFlight -= 1 }
            return try await beginRecording(payload)
        case "audio.stopRecord": return try await endRecording()
        case "audio.export": return try await exportAudio(payload)
        case "audio.kitPreview": return try await exportKitPreview(payload)
        case "audio.bounceTrack":
            let tracks = project["tracks"] as? [[String:Any]] ?? [], index = PayloadValue.integer(payload,"track",midiTrack)
            guard tracks.indices.contains(index) else { throw AudioSampleError.invalid("Choose a track to bounce.") }
            _ = try AudioExportFormat.resolve(payload:payload,mode:"track")
            let destination:URL
            if let path = payload["path"] as? String { destination = URL(fileURLWithPath:path) }
            else {
                let folder = try FileManager.default.url(for:.applicationSupportDirectory,in:.userDomainMask,appropriateFor:nil,create:true).appendingPathComponent("SuperTracker/Renders",isDirectory:true)
                try FileManager.default.createDirectory(at:folder,withIntermediateDirectories:true)
                destination = folder.appendingPathComponent("Track-\(index+1)-\(UUID().uuidString.prefix(8)).wav")
            }
            var data = payload; data["path"] = destination.path; data["mode"] = "track"; data["track"] = index
            _ = try await exportAudio(data)
            let info = try AVAudioFile(forReading:destination)
            let duration = Double(info.length)/info.processingFormat.sampleRate
            let clip:[String:Any] = ["id":UUID().uuidString,"name":(tracks[index]["name"] as? String ?? "Track \(index+1)")+" bounce","path":destination.path,"track":index,"startBeat":0,"durationBeats":duration*PayloadValue.number(project,"bpm",120)/60,"sourceOffset":0,"gain":1,"fadeIn":0,"fadeOut":0,"muted":false]
            return ["path":destination.path,"clip":clip,"sourceTrack":tracks[index],"sourceTrackIndex":index,"duration":duration]
        default: return nil
        }
    }
}

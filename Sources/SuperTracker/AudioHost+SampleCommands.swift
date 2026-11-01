import Foundation
import AppKit
import AVFoundation
import AudioToolbox
import CoreAudio
import TrackerEngine

/// Sample loading, editing, analysis and browser previews.
extension AudioHost {
    /// Returns nil for commands outside this family.
    @MainActor func handleSampleCommand(_ command: String, _ payload: [String: Any]) async throws -> Any? {
        switch command {
        case "sample.load":
            guard let path = payload["path"] as? String else { throw AudioSampleError.invalid("No sample path was provided.") }
            let sampleIndex = try resolvedSampleInstrument(payload)
            let generation = (sampleRequestGeneration[sampleIndex] ?? 0) &+ 1
            sampleRequestGeneration[sampleIndex] = generation
            let projectToken = projectGeneration
            sampleTasks[sampleIndex]?.cancel(); pendingSamplePaths.removeValue(forKey:sampleIndex)
            let decoded = try await Task.detached(priority:.userInitiated) { try AudioSamples.decode(path) }.value
            try requireInteractiveMutation()
            guard !closed, projectGeneration == projectToken, sampleRequestGeneration[sampleIndex] == generation else { throw AudioSampleError.invalid("Sample load was superseded by a newer project or sample change.") }
            // An instrument the UI is adding beyond the current project has no native slot
            // until the project arrives: keep its sample for that publication.
            if sampleIndex >= ordinaryInstruments { stagedSamples[path] = decoded; return decoded.info() }
            try installSample(decoded,instrument:sampleIndex); return decoded.info()
        case "sample.analyzeSlices":
            let index = try resolvedSampleInstrument(payload)
            let generation = sampleRequestGeneration[index] ?? 0
            let projectToken = projectGeneration
            let original:DecodedSample
            if let path = payload["path"] as? String {
                original = try await Task.detached(priority:.userInitiated) { try AudioSamples.decode(path) }.value
            } else if let sample = samples[index] { original = sample }
            else { throw AudioSampleError.invalid("Load a sample before detecting transients.") }
            let result = try await Task.detached(priority:.userInitiated) { try SampleSlicing.analyze(original,payload:payload) }.value
            try requireInteractiveMutation()
            guard projectGeneration == projectToken, (sampleRequestGeneration[index] ?? 0) == generation else {
                throw AudioSampleError.invalid("Transient analysis was superseded by a newer project or sample change.")
            }
            return result
        case "sample.preview":
            guard let path = payload["path"] as? String, !path.isEmpty else { throw AudioSampleError.invalid("Choose a sample to preview.") }
            guard let previewPlayer else { throw AudioSampleError.invalid("Preview audio is unavailable.") }
            // Cached heads return at once; misses decode off the main and audio
            // threads at the graph rate. A newer preview or stop supersedes this one.
            guard let loaded = try await previewPlayer.load(path) else { return ["playing":false,"superseded":true] }
            guard !closed, !exporting else { throw AudioSampleError.invalid("Wait for export to finish before previewing.") }
            try start()
            previewPlayer.begin(path, pcm: loaded.pcm, volume: Float(PayloadValue.number(payload,"volume",0.35)), began: loaded.started)
            return ["playing":true,"duration":loaded.pcm.duration,"cached":loaded.cached,"startMs":previewPlayer.lastStartLatency]
        case "sample.stopPreview":
            previewPlayer?.stop(); return ["playing":false]
        case "sample.wavetables":
            // Built-in tables for the sampler's Wavetable mode; their paths are project-portable references.
            return try BuiltinWavetables.list()
        case "sample.prefetchPreviews":
            // Warms the preview cache for the items around a browser selection;
            // each request replaces (cancels) the previous queue.
            let requests = (payload["items"] as? [Any] ?? []).compactMap { item -> [String:Any]? in
                if let path = item as? String { return ["path":path] }
                return item as? [String:Any]
            }
            return previewPlayer?.prefetch(requests) ?? ["queued":0]
        case "sample.previewStatus":
            return previewPlayer?.status() ?? [:]
        case "sample.info":
            // Read cached PCM metadata without reinstalling the sample or
            // starting a decode. A new web session can outlive a native cache hit.
            let index = try resolvedSampleInstrument(payload, fallback:-1)
            guard (0..<instrumentCapacity).contains(index), let expectedPath = payload["path"] as? String,
                  let sample = samples[index], sample.path == expectedPath else {
                return ["available":false]
            }
            let generation = sampleRequestGeneration[index] ?? 0
            let result = await Task.detached(priority:.utility) { sample.info() }.value
            guard !closed, (sampleRequestGeneration[index] ?? 0) == generation,
                  samples[index]?.path == expectedPath,
                  samples[index]?.left.count == sample.left.count,
                  samples[index]?.sampleRate == sample.sampleRate else {
                return ["available":false]
            }
            return result
        case "sample.peaks":
            // Sample editor: zoomed waveform columns and zero-crossing snapping of the loaded instrument sample.
            guard let sample = samples[try resolvedSampleInstrument(payload)] else { throw AudioSampleError.invalid("Load a sample before editing it.") }
            if let path = payload["path"] as? String, path != sample.path { throw AudioSampleError.invalid("The sample is still loading. Try again in a moment.") }
            return await Task.detached(priority:.userInitiated) { sample.inspect(payload:payload) }.value
        case "sample.edit":
            let index = try resolvedSampleInstrument(payload)
            let generation = (sampleRequestGeneration[index] ?? 0) &+ 1
            sampleRequestGeneration[index] = generation
            let projectToken = projectGeneration
            let original: DecodedSample
            if let path = payload["path"] as? String { original = try await Task.detached(priority:.userInitiated) { try AudioSamples.decode(path) }.value }
            else if let sample = samples[index] { original = sample }
            else { throw AudioSampleError.invalid("Load a sample before editing it.") }
            let edited = try await Task.detached(priority:.userInitiated) { try AudioSamples.edit(original,payload:payload) }.value
            try requireInteractiveMutation()
            guard !closed, projectGeneration == projectToken, sampleRequestGeneration[index] == generation else { throw AudioSampleError.invalid("Sample edit was superseded by a newer project or sample change.") }
            try installSample(edited,instrument:index); return edited.info()
        case "audio.clipInfo":
            guard let path = payload["path"] as? String else { throw AudioSampleError.invalid("Choose an audio clip file.") }
            if let info = clipMetadata[path] { return info }
            let sample = try await Task.detached(priority:.userInitiated) { try AudioSamples.decode(path) }.value
            let info = sample.info(); clipMetadata[path] = info; return info
        default: return nil
        }
    }
}

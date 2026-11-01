import Foundation
import AppKit
import AVFoundation
import AudioToolbox
import CoreAudio
import TrackerEngine

/// Master and input audio recording.
extension AudioHost {
    @MainActor func publishRecordingIntent() {
        // Export is a separate playback transaction. UI arm requests may still
        // change current intent, but its generated audio always reports false.
        st_engine_set_recording(engine,!exporting && (noteRecordArmed || (recording != nil && !recordingStopping)) ? 1 : 0)
    }

    func requireNoActiveRecording() throws {
        guard recording == nil else { throw AudioSampleError.invalid("Finish recording before reconfiguring audio.") }
    }

    @MainActor func beginRecording(_ payload:[String:Any]) async throws -> [String:Any] {
        guard audioConfigurationsInFlight == 0 else { throw AudioSampleError.invalid("Wait for audio configuration before recording.") }
        guard recording == nil else { throw AudioSampleError.invalid("A recording is already running.") }
        if payload["owner"] != nil { _ = try noteDestination(payload) }
        let fromInput = payload["source"] as? String != "master"
        if fromInput { try await authorizeInput(); try requireInteractiveMutation() }
        guard recording == nil,audioConfigurationsInFlight == 0 else { throw AudioSampleError.invalid("Audio recording or configuration changed while input was being authorized.") }
        let wasRunning = graph.isRunning
        if fromInput && inputMixer == nil {
            // Wiring the input path reconnects the input node's fan-out, which also feeds live line
            // inputs. AVAudioEngine raises on that in a running graph (the exception crashes later in
            // unrelated code), so connect it stopped, as audio.configure does; start() below resumes.
            do {
                if wasRunning { graph.stop(); renderCacheNeedsReset = true; try rack.awaitRenderQuiescence() }
                try connectInput()
            } catch { if wasRunning { try? start() }; throw error }
        }
        let previousMonitor = inputMixer?.outputVolume
        if fromInput {
            // Honor monitor-off before an idle input graph can emit any audio.
            inputMixer?.outputVolume = payload["monitor"] as? Bool == true && lineInputs.isEmpty ? Float(PayloadValue.number(payload,"monitorVolume",0.5)) : 0
        }
        do {
            // AVAudioEngine negotiates the implicit main-mixer output format on
            // start. Admit that format before creating the file and publishing
            // the recording endpoint; setup callbacks precede the captured range.
            try start()
            let node:AVAudioNode = fromInput ? graph.inputNode : graph.mainMixerNode
            let format = node.outputFormat(forBus:0)
            let channels:AudioRecordingChannelPlan
            if fromInput {
                channels=try AudioRecordingChannelPlan(sourceFormat:format,sourceChannels:format.channelCount == 1 && selectedInputChannels == [0,1] ? [0] : selectedInputChannels)
            }
            else { channels=try AudioRecordingChannelPlan.master(sourceFormat:format) }
            let url = try AudioRecordingLocation.destination(payload)
            let settings:[String:Any] = [AVFormatIDKey:kAudioFormatLinearPCM,AVSampleRateKey:format.sampleRate,AVNumberOfChannelsKey:channels.outputFormat.channelCount,AVLinearPCMBitDepthKey:24,AVLinearPCMIsFloatKey:false,AVLinearPCMIsBigEndianKey:false]
            let file = try AVAudioFile(forWriting:url,settings:settings,commonFormat:.pcmFormatFloat32,interleaved:false)
            recordingEpoch &+= 1
            let boundary = try AudioRecordingBoundary(sampleRate:format.sampleRate,epoch:recordingEpoch)
            let capture = try AudioRecording(source:fromInput ? .input : .master,file:file,path:url.path,instrument:payload["instrument"] as? Int,channels:channels,
                boundary:boundary,manualClock:!fromInput && graph.isInManualRenderingMode,owner:payload["owner"] as? String)
            if fromInput { publishInputRecording(capture) }
            else { publishMasterRecording(capture) }
            recording = capture; recordingStopping = false; publishRecordingIntent()
            emit("audio.recording",["recording":true,"path":url.path,"source":fromInput ? "input" : "master","channels":capture.channelCount])
            return ["recording":true,"path":url.path,"channels":capture.channelCount]
        } catch {
            let original = error
            if !wasRunning {
                        graph.stop(); renderCacheNeedsReset = true
                do { try rack.awaitRenderQuiescence() }
                catch {
                    if fromInput { inputMixer?.outputVolume = previousMonitor ?? 0 }
                    throw AudioRecordingError.invalid("\(original.localizedDescription) Audio was stopped, but its render callback did not finish: \(error.localizedDescription)")
                }
            }
            if fromInput { inputMixer?.outputVolume = previousMonitor ?? 0 }
            throw original
        }
    }

    @MainActor func endRecording() async throws -> [String:Any] {
        if let task = recordingStopTask { return try await task.value }
        guard let capture = recording else { return ["recording":false] }
        recordingStopping = true; capture.requestStop(); publishRecordingIntent()
        let task = Task { @MainActor [self,capture] () throws -> [String:Any] in
            defer { recordingStopTask = nil }
            let deadline = ContinuousClock.now.advanced(by:.seconds(2))
            var interruption:Error?
            while true {
                let progress = await capture.progress()
                if progress.error != nil || progress.complete { break }
                if closed { interruption = AudioRecordingError.invalid("The audio host closed before the recording source tail arrived."); break }
                if ContinuousClock.now >= deadline {
                    interruption = AudioRecordingError.invalid(progress.endFrame == nil
                        ? "The recording source did not acknowledge Stop on its audio clock within two seconds."
                        : "The recording source did not deliver the complete stopped interval within two seconds.")
                    break
                }
                do { try await Task.sleep(for:.milliseconds(5)) }
                catch { interruption = error; break }
            }
            if recording === capture {
                if capture.source == .master { publishMasterRecording(nil) }
                else { publishInputRecording(nil) }
            }
            let final = await capture.finish(interruption:interruption)
            if recording === capture {
                recording = nil; recordingStopping = false
                if !closed { publishRecordingIntent() }
            }
            var result:[String:Any] = ["recording":false,"path":capture.path,"duration":Double(final.frames)/capture.sampleRate,
                "frames":final.frames,"channels":capture.channelCount,"partial":final.error != nil,
                "sourceClockEpoch":String(capture.boundary?.epoch ?? 0),
                "sourceStartFrame":final.startFrame.map { String($0) as Any } ?? NSNull(),
                "sourceEndFrame":final.endFrame.map { String($0) as Any } ?? NSNull(),
                "receivedThrough":final.receivedThrough.map { String($0) as Any } ?? NSNull()]
            if let instrument = capture.instrument { result["instrument"] = instrument }
            if let owner=capture.owner { result["owner"]=owner }
            if let error = final.error { result["error"] = error.localizedDescription }
            emit("audio.recording",result)
            if let error = final.error {
                throw AudioRecordingError.invalid("\(error.localizedDescription) Partial audio was preserved at \(capture.path).")
            }
            return result
        }
        recordingStopTask = task
        // Cancelling an individual caller must not abandon the owned tail/drain.
        return try await task.value
    }
}

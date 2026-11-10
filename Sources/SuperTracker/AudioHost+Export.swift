import Foundation
import AppKit
import AVFoundation
import AudioToolbox
import CoreAudio
import TrackerEngine

/// Offline export: song, pattern, track, stems and kit previews.
extension AudioHost {
    /// The pattern a pattern-mode export or preview renders, as a one-entry song.
    struct PatternExportTiming {
        let pattern:Int
        let sequence:[Int]
        let occurrence:Int
        let frames:Int64?
    }

    func patternExportTiming(_ data:[String:Any],payload:[String:Any]) throws -> PatternExportTiming {
        let active=PayloadValue.integer(payload,"pattern",PayloadValue.integer(data,"activePattern",0))
        if payload["pattern"] != nil,
           !(data["patterns"] as? [[String:Any]] ?? []).enumerated().contains(where:{PayloadValue.integer($0.element,"id",$0.offset) == active}) {
            throw AudioSampleError.invalid("The preview or export pattern does not exist.")
        }
        return PatternExportTiming(pattern:active,sequence:[active],occurrence:0,frames:nil)
    }

    struct ExportDeliveryItem {
        let entry:AudioExportTransaction.Entry
        let sourceFrames:Int64
        let musicalFrames:Int64
    }

    struct KitPreview {
        let kitID:String
        let owner:String
        let maximumFrames:Int64
        let projectGeneration:UInt64
        let warning:String?
    }

    /// Preview audio of a group (a kit): its bus stem of the selected pattern, at most eight bars or ten seconds.
    @MainActor func exportGroupPreview(_ payload:[String:Any],groupID:String) async throws -> [String:Any] {
        guard let group=(project["groups"] as? [[String:Any]] ?? []).first(where:{$0["id"] as? String == groupID}),
              let bus=group["busID"] as? String else { throw AudioSampleError.invalid("Choose an existing kit to preview.") }
        let patternID=PayloadValue.integer(payload,"pattern",PayloadValue.integer(project,"activePattern",0))
        guard let pattern=(project["patterns"] as? [[String:Any]] ?? []).enumerated().first(where:{PayloadValue.integer($0.element,"id",$0.offset) == patternID})?.element else {
            throw AudioSampleError.invalid("Choose an existing pattern for the kit preview.")
        }
        let rows=pattern["steps"] as? [[Any]] ?? [], instruments=project["instruments"] as? [[String:Any]] ?? []
        var native=false,external=false
        for (index,track) in (project["tracks"] as? [[String:Any]] ?? []).enumerated() where track["groupID"] as? String == groupID && rows.indices.contains(index) {
            for raw in rows[index] {
                guard let step=raw as? [String:Any], PayloadValue.integer(step,"note",-1) >= 0 else { continue }
                let instrument=instruments.indices.contains(PayloadValue.integer(step,"instrument",PayloadValue.integer(track,"instrument",0)))
                    ? instruments[PayloadValue.integer(step,"instrument",PayloadValue.integer(track,"instrument",0))] : [:]
                if instrument["type"] as? String == "midi" { external=true } else { native=true }
            }
        }
        guard native else { throw AudioSampleError.invalid(external
            ? "This kit preview uses only external MIDI instruments. Record the hardware audio into its tracks before saving an audio preview."
            : "Add a note for this kit in the selected pattern before generating its audio preview.") }
        let meter=project["timeSignature"] as? [String:Any] ?? [:]
        let beats=8*Double(max(1,min(16,PayloadValue.integer(meter,"numerator",4))))*4/Double(max(1,PayloadValue.integer(meter,"denominator",4)))
        let maximumFrames=min(Int64(sampleRate*10),Int64((beats*60/max(20,min(999,PayloadValue.number(project,"bpm",120)))*sampleRate).rounded(.down)))
        var request=payload;request["mode"]="pattern";request["pattern"]=patternID
        request["tail"]=1;request["encoding"]="float32";request["sampleRate"]=48000
        let preview=KitPreview(kitID:groupID,owner:"bus:"+bus,maximumFrames:max(1,maximumFrames),projectGeneration:projectGeneration,
            warning:external ? "External MIDI tracks are omitted from this offline kit preview; record their audio for a complete preview." : nil)
        return try await exportAudio(request,kitPreview:preview)
    }

    @MainActor func exportKitPreview(_ payload:[String:Any]) async throws -> [String:Any] {
        guard let groupID=payload["groupID"] as? String else { throw AudioSampleError.invalid("Choose an existing kit to preview.") }
        return try await exportGroupPreview(payload,groupID:groupID)
    }

    @MainActor func exportAudio(_ payload:[String:Any],kitPreview:KitPreview? = nil) async throws -> [String:Any] {
        guard lineInputs.isEmpty else { throw AudioSampleError.invalid("Record or disable live line inputs before offline rendering. Hardware inputs cannot be rendered offline.") }
        guard let path = payload["path"] as? String, !path.isEmpty else { throw AudioSampleError.invalid("Choose an export destination.") }
        let mode = payload["mode"] as? String ?? "song"
        let encoding = try AudioExportFormat.resolve(payload:payload,mode:mode)
        let delivery = try AudioExportDelivery.Settings(encoding:encoding,sampleRate:AudioExportFormat.resolveSampleRate(payload:payload,mode:mode))
        guard recording == nil, !exporting else { throw AudioSampleError.invalid("Finish recording or the current export first.") }
        guard graphMutationsInFlight == 0 else { throw AudioSampleError.invalid("Wait for plug-in loading or audio configuration to finish before exporting.") }
        await awaitProjectAssets()
        try requireInteractiveMutation()
        guard routingFailure == nil, unavailableProject == nil else {
            throw AudioSampleError.invalid("Repair the project's unavailable routing before exporting audio.")
        }
        guard graphMutationsInFlight == 0 else { throw AudioSampleError.invalid("Wait for plug-in loading or audio configuration to finish before exporting.") }
        var previewRelease:(()->Void)?,previewStates:[[String:Any]]?
        defer { previewRelease?() }
        if let kitPreview {
            previewRelease=try await preparePluginStateCapture()
            try requireInteractiveMutation()
            guard kitPreview.projectGeneration == projectGeneration, graphMutationsInFlight == 0 else {
                throw AudioSampleError.invalid("The project changed while preparing its kit preview.")
            }
            let states=rack.snapshot()
            guard states.allSatisfy({$0["state"] is String && $0["stateError"] == nil}) else {
                throw AudioSampleError.invalid("A plug-in could not provide restorable state for the kit preview. Retry after its edits finish processing.")
            }
            previewStates=states
        }
        // A recording, another export or a live input may have started while this request waited above.
        guard recording == nil, !exporting, lineInputs.isEmpty else {
            throw AudioSampleError.invalid("Finish recording or the current export, and disable live line inputs, before exporting.")
        }
        rack.notifyStateCaptureMutation()
        if project["routing"] == nil, payload["owners"] != nil {
            throw AudioSampleError.invalid("Selected channel stem export requires an enabled routing graph. Enable routing before selecting tracks, buses or master as export owners.")
        }
        let transaction = try AudioExportTransaction(destinationURL:URL(fileURLWithPath:path),kind:mode == "stems" ? .stems : .file)
        defer { try? transaction.discard() }
        let song = mode != "pattern"
        let tail = max(0,min(30,PayloadValue.number(payload,"tail",3)))
        let originalProject = project, exportIdentity = projectIdentity
        let patternExport = !song ? try patternExportTiming(originalProject,payload:payload) : nil
        exportOriginalProject = originalProject
        defer { exportOriginalProject = nil }
        let wasRunning = graph.isRunning
        let originalManual:(AVAudioEngineManualRenderingMode,AVAudioFormat,AVAudioFrameCount)? = graph.isInManualRenderingMode ? (graph.manualRenderingMode,graph.manualRenderingFormat,graph.manualRenderingMaximumFrameCount) : nil
        let monitorVolume = inputMixer?.outputVolume ?? 0
        graph.stop()
        do { try rack.awaitRenderQuiescence() }
        catch { if wasRunning { try? start() }; throw error }
        // Capture flushes pending Play/Stop/Seek at zero time while no render owns
        // the engine. An earlier queued Stop must win over stale rendered status.
        let originalState = st_engine_capture_transport(engine)
        inputMixer?.outputVolume = 0
        previewPlayer?.stop(fade: false); panicExternal(); st_engine_panic(engine)
        meters.reset()
        if let inputMixer { graph.disconnectNodeOutput(inputMixer) }
        let ioDevices = currentIODevices()
        exporting = true
        publishRecordingIntent()
        exportCancelled = false
        let format = AVAudioFormat(standardFormatWithSampleRate:sampleRate,channels:2)!
        var deliveries:[ExportDeliveryItem] = []
        var cleaned = false
        func cleanup() throws {
            guard !cleaned else { return }
            rack.notifyStateCaptureMutation()
            defer { exporting = false }
            graph.stop()
            try rack.awaitRenderQuiescence()
            graph.disableManualRenderingMode(); renderCacheNeedsReset = true
            restoreIODevices(ioDevices)
            if originalProject["routing"] != nil || rack.hasRouting {
                try rack.configureRoutingCapture(owners:[])
                try rack.configureRouting(originalProject,commit:{
                    try self.applyEngineProject(originalProject)
                })
            } else { try applyEngineProject(originalProject) }
            project = originalProject; updatePluginMix(); panicExternal()
            if let previewStates { try rack.restorePreviewStates(previewStates) }
            // Restores exact fractional cursor and Song/Pattern mode, consumes the
            // restored project now, and never carries offline notes into live audio.
            var restoredState = originalState
            // Restore the latest current-session intent, not an arm value that
            // may have been explicitly changed while offline rendering yielded.
            restoredState.recording = exportIdentity == projectIdentity && (noteRecordArmed || recording != nil) ? 1 : 0
            st_engine_restore_transport(engine,restoredState,exportCancelled ? 0 : 1)
            inputMixer?.outputVolume = monitorVolume
            if let inputMixer { graph.connect(inputMixer,to:graph.mainMixerNode,format:AVAudioFormat(standardFormatWithSampleRate:engineInputFormat.sampleRate,channels:2)) }
            if let manual = originalManual { try graph.enableManualRenderingMode(manual.0,format:manual.1,maximumFrameCount:manual.2) }
            cleaned = true
            if wasRunning { try start() }
        }
        func finishDelivery(_ items:[ExportDeliveryItem],result initial:[String:Any]) async throws -> [String:Any] {
            guard !items.isEmpty else { throw AudioSampleError.invalid("Add a track before exporting stems.") }
            for (index,item) in items.enumerated() {
                if exportCancelled { throw AudioSampleError.invalid("Export cancelled. Existing destination files were preserved.") }
                let job = try AudioExportDelivery.Job(sourceURL:item.entry.sourceURL,expectedInputFrames:item.sourceFrames,destinationURL:item.entry.destinationURL,settings:delivery)
                defer { job.discard() }
                while job.result == nil {
                    if exportCancelled { throw AudioSampleError.invalid("Export cancelled. Existing destination files were preserved.") }
                    let progress = try job.advance(maximumCalls:8)
                    emit("audio.exportProgress",["progress":0.8+0.2*(Double(index)+Double(progress.outputFrames)/Double(progress.totalOutputFrames))/Double(items.count),"path":path,"phase":"delivery"])
                    await Task.yield()
                }
                if exportCancelled { throw AudioSampleError.invalid("Export cancelled. Existing destination files were preserved.") }
                _ = try job.publish()
            }
            var result = initial
            result.merge(delivery.metadata) { _,new in new }
            if let first = items.first {
                result["sourceFrames"] = first.sourceFrames
                let total = try AudioExportDelivery.outputFrames(inputFrames:first.sourceFrames,sampleRate:delivery.sampleRate)
                let musical = try AudioExportDelivery.outputFrames(inputFrames:first.musicalFrames,sampleRate:delivery.sampleRate)
                result["frames"] = total; result["musicalFrames"] = musical
                result["tailFrames"] = total-musical
            }
            if midiRoutes.contains(where:{$0.external}) { result["warning"] = "External hardware synths must be recorded to audio first; their live input is not available in offline export." }
            // Restore transport before committing any user-visible destination.
            // This synchronous publication cannot race another MainActor edit.
            try cleanup()
            result["paths"] = try transaction.publish().map(\.path)
            emit("audio.exported",result)
            return result
        }
        do {
            if originalProject["routing"] != nil {
                let rendered = try await renderRoutingExport(payload,project:originalProject,mode:mode,path:path,tail:tail,format:format,delivery:delivery,transaction:transaction,kitPreview:kitPreview)
                return try await finishDelivery(rendered.items,result:rendered.result)
            }
            let tracks = originalProject["tracks"] as? [[String:Any]] ?? []
            let selectedTrack = PayloadValue.integer(payload,"track",0)
            if mode == "track", !tracks.indices.contains(selectedTrack) { throw AudioSampleError.invalid("The selected bounce track does not exist.") }
            let iterations:[Int?] = mode == "stems" ? tracks.indices.map { Optional($0) } : mode == "track" ? [selectedTrack] : [nil]
            if graph.isInManualRenderingMode { graph.disableManualRenderingMode() }
            try graph.enableManualRenderingMode(.offline,format:format,maximumFrameCount:4096)
            nativeSourceUsesHostClock = false
            guard let buffer = AVAudioPCMBuffer(pcmFormat:format,frameCapacity:4096) else { throw AudioSampleError.invalid("Cannot allocate export buffer.") }
            guard let writeBuffer = AVAudioPCMBuffer(pcmFormat:format,frameCapacity:4096) else { throw AudioSampleError.invalid("Cannot allocate export trim buffer.") }
            for (iteration,soloTrack) in iterations.enumerated() {
                var bounceProject = originalProject; bounceProject["loopSong"] = false; bounceProject["metronome"] = false
                // A one-pattern, non-looping song cannot retrigger the first row when an
                // AU requests a look-ahead frame past the musical export boundary.
                if let patternExport { bounceProject["sequence"] = patternExport.sequence; bounceProject["audioClips"] = []; bounceProject["patternExport"] = true }
                // Reusable track bounces exclude the master fader; their new audio track will
                // pass through it on playback. This avoids applying the master gain twice.
                if mode == "track" { bounceProject["masterVolume"] = 1 }
                if let soloTrack {
                    var isolated = tracks
                    // A track bounce renders its track regardless of mute/solo. Stems follow
                    // the mix: keep the user's mute and solo flags and only mute the other
                    // tracks, so a muted (or solo-excluded) track exports a silent stem and the
                    // stems sum to the master mix. Silent files keep one file per track, as
                    // the UI expects and as routed post-fader stems already behave.
                    for index in isolated.indices {
                        if mode == "stems" { if index != soloTrack { isolated[index]["mute"] = true } }
                        else { isolated[index]["solo"] = index == soloTrack; isolated[index]["mute"] = index != soloTrack }
                    }
                    bounceProject["tracks"] = isolated
                }
                project = bounceProject; try applyEngineProject(bounceProject); updatePluginMix()
                graph.reset(); try resetRenderCache(); st_engine_panic(engine); st_engine_seek(engine,Int32(clamping:patternExport?.pattern ?? PayloadValue.integer(originalProject,"activePattern",0)),0,Int32(patternExport?.occurrence ?? 0))
                st_engine_play(engine,1)
                let duration = (patternExport?.frames).map { Double($0)/sampleRate } ?? st_engine_duration(engine,1)
                guard duration > 0, duration < 86400 else { throw AudioSampleError.invalid("Export duration is invalid or exceeds 24 hours.") }
                let musicalFrames = AVAudioFramePosition(originalProject["timing"] == nil ? duration*sampleRate : (duration*sampleRate).rounded())
                let outputFrames = originalProject["timing"] == nil ? AVAudioFramePosition((duration+tail)*sampleRate) : musicalFrames+AVAudioFramePosition(tail*sampleRate)
                let url:URL
                if let soloTrack, mode == "stems" {
                    let name = (tracks[soloTrack]["name"] as? String ?? "Track \(soloTrack+1)").replacingOccurrences(of:"/",with:"-").replacingOccurrences(of:":",with:"-")
                    url = URL(fileURLWithPath:path).appendingPathComponent(String(format:"%02d",soloTrack+1)+"-"+name+".wav")
                } else { url = URL(fileURLWithPath:path) }
                _ = try AudioExportDelivery.preflight(inputFrames:outputFrames,settings:delivery)
                let entry = try transaction.register(filename:url.lastPathComponent)
                var file:AVAudioFile? = try AVAudioFile(forWriting:entry.sourceURL,settings:AudioExportFormat.float32.settings,commonFormat:.pcmFormatFloat32,interleaved:false)
                graph.prepare(); rack.refreshLatencyCompensation()
                guard !rack.latencyExceedsLimit else { throw AudioSampleError.invalid("This plug-in chain exceeds the two-second latency compensation limit. Reduce the chain's reported latency before exporting.") }
                let compensationFrames = AVAudioFramePosition(rack.compensationLatencyFrames)
                let totalFrames = outputFrames + compensationFrames
                rack.resetRenderFailures()
                try graph.start()
                var rendered:AVAudioFramePosition = 0; var retries = 0; var stoppedForTail = false
                while rendered < totalFrames {
                    if exportCancelled { throw AudioSampleError.invalid("Export cancelled. Existing destination files were preserved.") }
                    if rendered >= musicalFrames && !stoppedForTail { st_engine_stop(engine); stoppedForTail = true }
                    let untilBoundary = rendered < musicalFrames ? musicalFrames-rendered : totalFrames-rendered
                    let count = AVAudioFrameCount(min(4096,min(totalFrames-rendered,untilBoundary)))
                    var result = AVAudioEngineManualRenderingStatus.error
                    var renderError:Error?
                    do {
                        result = try renderOfflineQuantum(frames:count,to:buffer)
                    }
                    catch { renderError = error }
                    if let failure = rack.renderFailure() {
                        let message = failure["message"] as? String ?? "A plug-in failed to render."
                        throw AudioSampleError.invalid(message+" Export stopped; existing destination files were preserved.")
                    }
                    if let renderError { throw renderError }
                    switch result {
                    case .success:
                        let skip = Int(max(0,min(AVAudioFramePosition(buffer.frameLength),compensationFrames-rendered)))
                        let framesToWrite = Int(buffer.frameLength)-skip
                        if framesToWrite > 0 {
                            if skip == 0 { try file!.write(from:buffer) }
                            else {
                                writeBuffer.frameLength = AVAudioFrameCount(framesToWrite)
                                for channel in 0..<2 { writeBuffer.floatChannelData![channel].update(from:buffer.floatChannelData![channel].advanced(by:skip),count:framesToWrite) }
                                try file!.write(from:writeBuffer)
                            }
                        }
                        rendered += AVAudioFramePosition(buffer.frameLength); retries = 0
                    case .cannotDoInCurrentContext, .insufficientDataFromInputNode:
                        retries += 1
                        if retries > 100 { throw AudioSampleError.invalid("An Audio Unit could not render offline. Try recording the master output in real time.") }
                    case .error: throw AudioSampleError.invalid("Audio Unit rendering failed during export.")
                    @unknown default: throw AudioSampleError.invalid("Unknown offline rendering status.")
                    }
                    if rendered % 65536 < 4096 {
                        emit("audio.exportProgress",["progress":0.8*(Double(iteration)+Double(rendered)/Double(totalFrames))/Double(max(1,iterations.count)),"path":url.path])
                        await Task.yield()
                    }
                }
                graph.stop()
                if #available(macOS 15.0, *) { file?.close() }
                file = nil
                deliveries.append(ExportDeliveryItem(entry:entry,sourceFrames:outputFrames,musicalFrames:musicalFrames))
            }
            let result:[String:Any] = ["path":path,"mode":mode,"latencyCompensation":rack.latencyStatus()]
            return try await finishDelivery(deliveries,result:result)
        } catch {
            let originalError = error
            do { try cleanup() }
            catch { emit("audio.error",["error":"Audio transport could not be restored after export: "+error.localizedDescription]) }
            throw originalError
        }
    }

    /// Leaving manual rendering recreates the HAL units on the system default devices
    /// (the input drops from a 10-channel interface to the built-in microphone). The
    /// input unit is only read while it is in use, so an output-only session never
    /// instantiates it.
    func currentIODevices() -> (input: AudioDeviceID?, output: AudioDeviceID?) {
        func device(_ unit: AudioUnit?) -> AudioDeviceID? {
            guard let unit else { return nil }
            var device: AudioDeviceID = 0; var bytes = UInt32(MemoryLayout<AudioDeviceID>.size)
            guard AudioUnitGetProperty(unit,kAudioOutputUnitProperty_CurrentDevice,kAudioUnitScope_Global,0,&device,&bytes) == noErr, device != 0 else { return nil }
            return device
        }
        guard !graph.isInManualRenderingMode else { return (nil, nil) }
        let input = hardwareInputActive || selectedInputDevice != nil ? device(graph.inputNode.audioUnit) : nil
        return (input, device(graph.outputNode.audioUnit))
    }

    func restoreIODevices(_ devices: (input: AudioDeviceID?, output: AudioDeviceID?)) {
        let current = currentIODevices()
        for (wanted, now, unit) in [(devices.output, current.output, graph.outputNode.audioUnit),
                                    (devices.input, current.input, devices.input == nil ? nil : graph.inputNode.audioUnit)] {
            guard var wanted, wanted != now, let unit else { continue }
            AudioUnitSetProperty(unit,kAudioOutputUnitProperty_CurrentDevice,kAudioUnitScope_Global,0,&wanted,UInt32(MemoryLayout<AudioDeviceID>.size))
        }
    }
}

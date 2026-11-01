import Foundation
import AppKit
import AVFoundation
import AudioToolbox
import CoreAudio
import TrackerEngine

/// Offline export through the routed mixer graph (stems and captures per owner).
extension AudioHost {
    /// Routed exports advance the entire graph once. Selected strip captures are
    /// passive processors in that render, so detector inputs, shared returns and
    /// nonlinear master processors keep exactly the same signal as normal playback.
    @MainActor func renderRoutingExport(_ payload:[String:Any],project original:[String:Any],mode:String,path:String,tail:Double,format:AVAudioFormat,delivery:AudioExportDelivery.Settings,transaction:AudioExportTransaction,kitPreview:KitPreview? = nil) async throws -> (result:[String:Any],items:[ExportDeliveryItem]) {
        let definition = try AudioRouting.parse(project:original)
        let requestedOwners:[String]
        if let kitPreview { requestedOwners=[kitPreview.owner] }
        else if mode == "stems" {
            if let raw = payload["owners"] {
                guard let owners = raw as? [String], !owners.isEmpty else { throw AudioSampleError.invalid("Select one or more channels for stem export.") }
                requestedOwners = owners
            } else {
                // One stem per track; group members are tracks (their group buses are explicit owners).
                requestedOwners=definition.owners.filter { $0.kind == .track }.map(\.id)
            }
        } else if mode == "track" {
            let index = PayloadValue.integer(payload,"track",0)
            guard let owner = definition.owners.first(where:{$0.trackIndex == index}) else { throw AudioSampleError.invalid("The selected bounce track does not exist.") }
            requestedOwners = [owner.id]
        } else { requestedOwners = [] }
        let selected = Set(requestedOwners)
        guard selected.count == requestedOwners.count,
              selected.isSubset(of:Set(definition.owners.map(\.id))) else {
            throw AudioSampleError.invalid("Stem selection contains a duplicate or unavailable channel.")
        }
        if mode == "stems", selected.isEmpty { throw AudioSampleError.invalid("Add a track or select a bus before exporting stems.") }
        let owners = definition.owners.filter { selected.contains($0.id) }
        let patternExport = mode == "pattern" ? try patternExportTiming(original,payload:payload) : nil
        var bounceProject = original
        bounceProject["loopSong"] = false; bounceProject["metronome"] = false
        if mode == "pattern" {
            bounceProject["sequence"] = patternExport?.sequence ?? [PayloadValue.integer(original,"activePattern",0)]
            bounceProject["audioClips"] = []; bounceProject["patternExport"] = true
        }
        project = bounceProject
        try rack.configureRouting(bounceProject,commit:{
            try self.applyEngineProject(bounceProject)
        })
        try rack.configureRoutingCapture(owners:selected)
        updatePluginMix()
        if graph.isInManualRenderingMode { graph.disableManualRenderingMode() }
        try graph.enableManualRenderingMode(.offline,format:format,maximumFrameCount:4096)
            nativeSourceUsesHostClock = false
        guard let buffer = AVAudioPCMBuffer(pcmFormat:format,frameCapacity:4096),
              let writeBuffer = AVAudioPCMBuffer(pcmFormat:format,frameCapacity:4096),
              let captureBuffer = AVAudioPCMBuffer(pcmFormat:format,frameCapacity:4096) else {
            throw AudioSampleError.invalid("Cannot allocate routing export buffers.")
        }
        graph.reset(); try resetRenderCache(); st_engine_panic(engine)
        st_engine_seek(engine,Int32(clamping:patternExport?.pattern ?? PayloadValue.integer(original,"activePattern",0)),0,Int32(patternExport?.occurrence ?? 0)); st_engine_play(engine,1)
        let duration = (patternExport?.frames).map { Double($0)/sampleRate } ?? st_engine_duration(engine,1)
        guard duration > 0, duration < 86400 else { throw AudioSampleError.invalid("Export duration is invalid or exceeds 24 hours.") }
        let fullMusicalFrames = AVAudioFramePosition(original["timing"] == nil ? duration*sampleRate : (duration*sampleRate).rounded())
        let musicalFrames = min(fullMusicalFrames,kitPreview?.maximumFrames ?? fullMusicalFrames)
        let outputFrames = kitPreview != nil || original["timing"] != nil
            ? musicalFrames+AVAudioFramePosition(tail*sampleRate) : AVAudioFramePosition((duration+tail)*sampleRate)
        graph.prepare(); rack.refreshLatencyCompensation()
        guard !rack.latencyExceedsLimit else { throw AudioSampleError.invalid("This routing graph exceeds the two-second latency compensation limit. Reduce its latency before exporting.") }
        try rack.resetRoutingCaptures(startFrame:0,frameLimit:outputFrames)
        let captures = rack.routingCaptures
        guard Set(captures.keys) == selected else { throw AudioSampleError.invalid("The routing graph did not prepare all selected stem captures.") }
        let compensationFrames = AVAudioFramePosition(rack.compensationLatencyFrames)
        let captureLatency = AVAudioFramePosition(rack.routingCaptureArrivalFrames.values.max() ?? 0)
        let totalFrames = outputFrames+max(compensationFrames,captureLatency)
        let deliveredFrames = try AudioExportDelivery.preflight(inputFrames:outputFrames,settings:delivery)
        let settings = AudioExportFormat.float32.settings
        var items:[ExportDeliveryItem] = []
        var files:[String:AVAudioFile] = [:], paths:[String] = [], exportedOwners:[[String:Any]] = []
        var writtenFrames:[String:Int64] = [:]
        for (index,owner) in owners.enumerated() {
            let url:URL
            if mode == "stems" {
                let name = owner.name.replacingOccurrences(of:"/",with:"-").replacingOccurrences(of:":",with:"-")
                url = URL(fileURLWithPath:path).appendingPathComponent(String(format:"%02d",index+1)+"-"+name+".wav")
            } else { url = URL(fileURLWithPath:path) }
            let entry = try transaction.register(filename:url.lastPathComponent)
            files[owner.id] = try AVAudioFile(forWriting:entry.sourceURL,settings:settings,commonFormat:.pcmFormatFloat32,interleaved:false)
            items.append(ExportDeliveryItem(entry:entry,sourceFrames:outputFrames,musicalFrames:musicalFrames))
            writtenFrames[owner.id] = 0; paths.append(entry.finalURL.path)
            exportedOwners.append(["owner":owner.id,"name":owner.name,"path":url.path,
                "boundary":owner.kind == .master ? "masterOutput" : "postFaderPreParent",
                "frames":deliveredFrames,"sourceFrames":outputFrames,"sampleRate":delivery.sampleRate,
                "arrivalFrames":rack.routingCaptureArrivalFrames[owner.id] ?? 0,"arrivalSampleRate":48_000])
        }
        var masterFile:AVAudioFile?
        if owners.isEmpty {
            let entry = try transaction.register(filename:URL(fileURLWithPath:path).lastPathComponent)
            masterFile = try AVAudioFile(forWriting:entry.sourceURL,settings:settings,commonFormat:.pcmFormatFloat32,interleaved:false)
            items.append(ExportDeliveryItem(entry:entry,sourceFrames:outputFrames,musicalFrames:musicalFrames))
            paths.append(entry.finalURL.path)
        }
        func drainCaptures() throws {
            for owner in owners {
                guard let capture = captures[owner.id], let file = files[owner.id] else { continue }
                if let error = capture.error {
                    let diagnostics = (try? JSONSerialization.data(withJSONObject:capture.diagnostics,options:.sortedKeys))
                        .flatMap { String(data:$0,encoding:.utf8) } ?? "unavailable"
                    throw AudioSampleError.invalid("Stem capture for \(owner.name) (\(owner.id)) failed: \(error.localizedDescription) Diagnostics: \(diagnostics)")
                }
                while capture.availableFrames > 0 {
                    let chunk = try capture.drain(into:captureBuffer)
                    guard chunk.frameCount > 0, chunk.startFrame == writtenFrames[owner.id],
                          chunk.startFrame+Int64(chunk.frameCount) <= outputFrames else {
                        throw AudioSampleError.invalid("Stem capture for \(owner.name) lost its timeline alignment. The export is incomplete.")
                    }
                    try file.write(from:captureBuffer)
                    writtenFrames[owner.id] = chunk.startFrame+Int64(chunk.frameCount)
                }
            }
        }
        rack.resetRenderFailures(); try graph.start()
        var rendered:AVAudioFramePosition = 0, masterFrames:AVAudioFramePosition = 0
        var retries = 0, stoppedForTail = false
        while rendered < totalFrames {
            if exportCancelled { throw AudioSampleError.invalid("Export cancelled. Existing destination files were preserved.") }
            if rendered >= musicalFrames && !stoppedForTail { st_engine_stop(engine); stoppedForTail = true }
            let boundary = rendered < musicalFrames ? musicalFrames-rendered : totalFrames-rendered
            let count = AVAudioFrameCount(min(4096,min(totalFrames-rendered,boundary)))
            var result = AVAudioEngineManualRenderingStatus.error
            var renderError:Error?
            do {
                let quantumStart = Int64(graph.manualRenderingSampleTime)
                for capture in captures.values { try capture.beginRenderQuantum(sampleTime:quantumStart,frameCount:Int(count)) }
                result = try renderOfflineQuantum(frames:count,to:buffer)
            } catch { renderError = error }
            // AVAudioEngine may omit the sample-time validity flag while pulling
            // auxiliary buses. The capture can use this explicit master-render
            // window only while that synchronous render request is active.
            for capture in captures.values { capture.endRenderQuantum() }
            // Drain after every master request, including retry statuses. A node
            // already rendered during a retry deduplicates by its sample timestamp.
            try drainCaptures()
            if let failure = rack.renderFailure() {
                throw AudioSampleError.invalid((failure["message"] as? String ?? "A plug-in failed to render.")+" Export stopped; existing destination files were preserved.")
            }
            if let renderError { throw renderError }
            switch result {
            case .success:
                if let masterFile {
                    let skip = Int(max(0,min(AVAudioFramePosition(buffer.frameLength),compensationFrames-rendered)))
                    let frames = Int(min(AVAudioFramePosition(Int(buffer.frameLength)-skip),outputFrames-masterFrames))
                    if frames > 0 {
                        writeBuffer.frameLength = AVAudioFrameCount(frames)
                        for channel in 0..<2 { writeBuffer.floatChannelData![channel].update(from:buffer.floatChannelData![channel].advanced(by:skip),count:frames) }
                        try masterFile.write(from:writeBuffer); masterFrames += AVAudioFramePosition(frames)
                    }
                }
                guard buffer.frameLength > 0 else { throw AudioSampleError.invalid("The audio graph returned no frames during export.") }
                rendered += AVAudioFramePosition(buffer.frameLength); retries = 0
            case .cannotDoInCurrentContext, .insufficientDataFromInputNode:
                retries += 1
                if retries > 100 { throw AudioSampleError.invalid("A plug-in could not render offline. Try recording the master output in real time.") }
            case .error: throw AudioSampleError.invalid("Audio rendering failed during routing export.")
            @unknown default: throw AudioSampleError.invalid("Unknown offline rendering status.")
            }
            if rendered % 65536 < 4096 {
                emit("audio.exportProgress",["progress":0.8*Double(rendered)/Double(totalFrames),"path":path])
                await Task.yield()
            }
        }
        graph.stop(); try rack.awaitRenderQuiescence(); try drainCaptures()
        guard writtenFrames.values.allSatisfy({$0 == outputFrames}), masterFile == nil || masterFrames == outputFrames else {
            throw AudioSampleError.invalid("The routing export ended before every selected channel reached the requested duration.")
        }
        if #available(macOS 15.0, *) { masterFile?.close(); for file in files.values { file.close() } }
        masterFile = nil; files.removeAll()
        var result:[String:Any] = ["paths":paths,"path":path,"mode":mode,
            "latencyCompensation":rack.latencyStatus(),"stems":exportedOwners,"renderPasses":1,"stemPolicy":"fullGraph",
            "captureDiagnostics":captures.mapValues { $0.diagnostics }]
        if let kitPreview {
            result["kitID"]=kitPreview.kitID;result["previewOwner"]=kitPreview.owner
            result["previewBoundary"]="postFaderPreParent";result["previewMaximumSeconds"]=10
            if let warning=kitPreview.warning { result["warning"]=warning }
        }
        return (result,items)
    }
}

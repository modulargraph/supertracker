import Foundation
import AppKit
import AVFoundation
import AudioToolbox
import CoreAudio
import TrackerEngine

/// Live line inputs routed into tracks.
extension AudioHost {
    func lineInputStatus()->[[String:Any]] {
        ((project["tracks"] as? [[String:Any]]) ?? []).enumerated().compactMap { track,item in
            guard let input=item["audioInput"] as? [String:Any] else { return nil }
            var status:[String:Any]=["track":track,"input":input,"active":lineInputs[track] != nil,
                "state":lineInputs[track] != nil ? "live" : lineInputPaused[track] != nil ? "paused" : "off"]
            if let reason=lineInputPaused[track] { status["status"]=reason }
            status["monitoring"]=lineInputs[track] != nil && input["monitor"] as? Bool != false
            if let mapped=lineInputMapped[track],lineInputs[track] != nil { status["engineChannels"]=mapped.channels;status["via"]=mapped.via }
            return status
        }
    }

    /// Whether the project asks for any live input. Device changes re-resolve these routes even when none
    /// is running yet (for example when they could not start at load because the input was not available).
    var projectHasLineInputs:Bool {
        ((project["tracks"] as? [[String:Any]]) ?? []).contains { ($0["audioInput"] as? [String:Any])?["enabled"] as? Bool == true }
    }

    func hardwareInputDevice()->AudioDeviceID {
        selectedInputDevice ?? AudioDeviceSelection.defaultDevice(input:true)
    }

    /// Paused routes stay saved; report the condition once and watch for the interface.
    @MainActor func publishLineInputPause(_ paused:[Int:String],report:Bool) {
        let changed=paused != lineInputPaused
        lineInputPaused=paused
        let summary=LineInputProblemReporter.summary(paused)
        lineInputError=summary
        if !paused.isEmpty && lineInputDeviceObserver == nil {
            lineInputDeviceObserver=AudioDeviceListObserver { [weak self] in self?.lineInputRecheckUntil=Date().addingTimeInterval(3) }
        } else if paused.isEmpty { lineInputDeviceObserver=nil }
        guard report else { return }
        if let message=lineInputReporter.update(summary) { emit("audio.error",["error":message]) }
        if changed { emit("audio.devices",audioDevices()) }
    }

    /// Hotplug or interface change: re-resolve the routes for a few seconds (the engine's input format
    /// follows the HAL a moment later, so a route first resolved against the old channel count is
    /// rebuilt once the new one appears). No-op when unchanged.
    @MainActor func recheckPausedLineInputs() {
        guard let until=lineInputRecheckUntil,!lineInputPaused.isEmpty || projectHasLineInputs else { return }
        guard Date() < until else { lineInputRecheckUntil=nil;return }
        guard !closed,!exporting,!graph.isInManualRenderingMode,!deviceRecoveryPending,
              audioConfigurationsInFlight == 0,graphMutationsInFlight == 0,pendingPluginRestore == nil,
              stateCaptureOwner == nil,!freezeTrackRuntime.busy,recording == nil,!recordingStopping else { return }
        do { try synchronizeLineInputs(project) }
        catch {
            lineInputError=error.localizedDescription
            if let message=lineInputReporter.update(error.localizedDescription) { emit("audio.error",["error":message]) }
        }
    }

    /// Starts audio for live inputs (or a MIDI route published meanwhile) once the project and its
    /// plug-ins have finished loading.
    @MainActor func startLiveInputsWhenIdle() {
        guard lineInputAutoStart || midiAutoStart else { return }
        guard !lineInputs.isEmpty || midiAutoStart,!graph.isRunning else { lineInputAutoStart=false;midiAutoStart=false;return }
        guard !closed,!exporting,!graph.isInManualRenderingMode,!deviceRecoveryPending,
              audioConfigurationsInFlight == 0,graphMutationsInFlight == 0,pendingPluginRestore == nil,
              stateCaptureOwner == nil,!freezeTrackRuntime.busy,recording == nil,!recordingStopping else { return }
        let forInputs=lineInputAutoStart && !lineInputs.isEmpty
        lineInputAutoStart=false;midiAutoStart=false
        // A project opened with unavailable routing reports that itself; MIDI alone does not retry.
        guard forInputs || (routingFailure == nil && unavailableProject == nil) else { return }
        do { try start() } catch { emit("audio.error",["error":"Could not start audio\(forInputs ? " for live inputs" : ""): \(error.localizedDescription)"]) }
    }

    /// Re-resolves the project's routes for the next few seconds, after the input interface changed.
    func recheckLineInputsSoon() {
        lineInputRecheckUntil=Date().addingTimeInterval(3)
    }

    /// Hardware output has one fan-out connection, so metering and all lanes see
    /// the same source quantum. No tap/ring-buffer delay is introduced per lane.
    func removeInputMonitorSelector() {
        if let selector=inputMonitorSelector {
            graph.disconnectNodeInput(selector.node);graph.disconnectNodeOutput(selector.node);graph.detach(selector.node)
        }
        inputMonitorSelector=nil
    }

    func updateInputMonitorSelector() throws {
        guard let mixer=inputMixer else { return }
        let format=engineInputFormat
        let channels=format.channelCount == 1 && selectedInputChannels == [0,1] ? [0] : selectedInputChannels
        let next=try AudioLineInput(format:format,channels:channels)
        let gain=mixer.outputVolume
        removeInputMonitorSelector()
        graph.attach(next.node);inputMonitorSelector=next
        graph.connect(next.node,to:mixer,format:next.node.outputFormat(forBus:0))
        wireLineInputFanout()
        mixer.outputVolume=gain
    }

    func wireLineInputFanout() {
        var points=lineInputs.sorted { $0.key < $1.key }.map { AVAudioConnectionPoint(node:$0.value.node,bus:0) }
        if let selector=inputMonitorSelector { points.append(AVAudioConnectionPoint(node:selector.node,bus:0)) }
        let input=graph.inputNode
        graph.disconnectNodeOutput(input)
        if !points.isEmpty { graph.connect(input,to:points,fromBus:0,format:engineInputFormat) }
    }

    @MainActor func synchronizeLineInputs(_ candidate:[String:Any],force:Bool=false,report:Bool=true) throws {
        let tracks=(candidate["tracks"] as? [[String:Any]]) ?? []
        // Only enabled routes, keyed by track index, define the live graph.
        // Adding or removing tracks without inputs must never restart audio.
        let enabled=tracks.enumerated().compactMap { index,track -> (Int,[String:Any])? in
            guard let item=track["audioInput"] as? [String:Any],item["enabled"] as? Bool == true else { return nil }
            return (index,item)
        }
        let keyed=Dictionary(uniqueKeysWithValues:enabled.map { (String($0.0),$0.1) })
        var signature=(try JSONSerialization.data(withJSONObject:keyed,options:.sortedKeys)).base64EncodedString()
        // Routes name the physical interface; resolve them against the running input
        // device (directly, or as a member of an aggregate). Absent interfaces pause
        // their routes without failing the others or the project edit.
        var plan:[Int:LineInputTopology.Resolution]=[:],format:AVAudioFormat?,topology:LineInputTopology?
        if !enabled.isEmpty {
            guard AVCaptureDevice.authorizationStatus(for:.audio) == .authorized else {
                throw AudioSampleError.invalid("Saved line inputs are inactive. Enable them in Line inputs to allow microphone access.")
            }
            let input=engineInputFormat
            let current=LineInputTopology.current(device:hardwareInputDevice(),inputChannels:input.sampleRate > 0 ? Int(input.channelCount) : 0)
            for (track,item) in enabled {
                plan[track]=current.resolve(deviceUID:item["deviceUID"] as? String ?? "",channels:item["channels"] as? [Int] ?? [])
            }
            format=input;topology=current
            signature += "|\(current.signature)@\(input.sampleRate)|"+plan.keys.sorted().map { "\($0):\(plan[$0]!)" }.joined(separator:";")
        }
        guard force || signature != lineInputSignature else { return }
        var paused:[Int:String]=[:],mapped:[Int:(channels:[Int],via:String)]=[:]
        for (track,item) in enabled {
            switch plan[track]! {
            case .unavailable(let reason): paused[track]=reason
            case .live(let channels): if channels != (item["channels"] as? [Int]) { mapped[track]=(channels,topology?.name ?? "") }
            }
        }
        // Nothing to prepare and nothing live: the graph already matches. This
        // precedes the recording guard so master takes survive track edits.
        if paused.count == enabled.count && lineInputs.isEmpty {
            lineInputSignature=signature;lineInputMapped=[:];publishLineInputPause(paused,report:report);return
        }
        guard recording == nil else { throw AudioSampleError.invalid("Finish recording before changing live input routes.") }
        // Validate and allocate before touching the audible graph. A bad UI
        // selection must leave its previous live routes intact.
        var prepared:[Int:AudioLineInput]=[:]
        if let format {
            for (track,item) in enabled {
                guard case .live(let channels)=plan[track]! else { continue }
                // Monitor off keeps the route (and its saved channels) but silences it in the track.
                let gain=item["monitor"] as? Bool == false ? 0 : Float(PayloadValue.number(item,"gain",1))
                prepared[track]=try AudioLineInput(format:format,channels:channels,gain:gain)
            }
        }
        let wasRunning=graph.isRunning,oldInputs=lineInputs,oldSignature=lineInputSignature
        let previousMonitor=inputMixer?.outputVolume
        lineInputGraphRebuilds &+= 1
        graph.stop();renderCacheNeedsReset=true
        try rack.awaitRenderQuiescence()
        let touchesHardware = !oldInputs.isEmpty || !prepared.isEmpty || inputMixer != nil
        do {
            try rack.setLineInputSources([:])
            for line in oldInputs.values { graph.disconnectNodeInput(line.node);graph.detach(line.node) }
            for line in prepared.values { graph.attach(line.node) }
            lineInputs=prepared
            if !prepared.isEmpty { inputMixer?.outputVolume=0 }
            if touchesHardware { wireLineInputFanout() }
            refreshHardwareReturnLatency(replan:false)
            try rack.setLineInputSources(prepared.mapValues { $0.node })
            refreshMIDIClock()
            if wasRunning { try start() }
            // Live inputs are heard without pressing Play. Audio starts from the timer once nothing is
            // loading (plug-ins restore right after this, and must never be processing while they do).
            lineInputAutoStart = !prepared.isEmpty && !wasRunning
            lineInputSignature=signature;lineInputMapped=mapped
            publishLineInputPause(paused,report:report)
        } catch {
            graph.stop();renderCacheNeedsReset=true
            try? rack.setLineInputSources([:])
            for line in prepared.values where line.node.engine === graph { graph.disconnectNodeInput(line.node);graph.detach(line.node) }
            for line in oldInputs.values where line.node.engine == nil { graph.attach(line.node) }
            lineInputs=oldInputs;lineInputSignature=oldSignature
            if let previousMonitor { inputMixer?.outputVolume=previousMonitor }
            if touchesHardware { wireLineInputFanout() }
            try? rack.setLineInputSources(oldInputs.mapValues { $0.node })
            if wasRunning { try? start() }
            throw error
        }
    }

    @MainActor func configureLineInputs(_ payload:[String:Any]) async throws -> [String:Any] {
        guard audioConfigurationsInFlight == 0,graphMutationsInFlight == 0,recording == nil else { throw AudioSampleError.invalid("Finish recording or audio configuration before changing line inputs.") }
        let changes:[[String:Any]]
        if let values=payload["inputs"] as? [[String:Any]] { changes=values }
        else if let track=payload["track"] as? Int {
            var item=payload["input"] as? [String:Any] ?? ["enabled":false];item["track"]=track;changes=[item]
        } else { return audioDevices() }
        let requestIdentity=projectIdentity,requestGeneration=projectGeneration
        var candidate=project,tracks=project["tracks"] as? [[String:Any]] ?? []
        for var item in changes {
            guard let track=item.removeValue(forKey:"track") as? Int,tracks.indices.contains(track) else { throw AudioSampleError.invalid("Choose an existing track for each line input.") }
            tracks[track]["audioInput"]=item
        }
        candidate["tracks"]=tracks
        audioConfigurationsInFlight += 1
        defer { audioConfigurationsInFlight -= 1 }
        if changes.contains(where: { $0["enabled"] as? Bool == true }) { try await authorizeInput() }
        try requireInteractiveMutation();try requireNoActiveRecording()
        guard requestIdentity == projectIdentity,requestGeneration == projectGeneration else { throw AudioSampleError.invalid("The project changed while line inputs were being enabled. Try again.") }
        do {
            try synchronizeLineInputs(candidate,force:true)
            if !lineInputs.isEmpty { try start() }
        } catch {
            try? synchronizeLineInputs(project,force:true)
            throw error
        }
        project=candidate
        updatePluginMix()
        return audioDevices()
    }
}

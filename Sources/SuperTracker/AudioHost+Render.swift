import Foundation
import AppKit
import AVFoundation
import AudioToolbox
import CoreAudio
import TrackerEngine

/// The render path: native sources, the main-mixer preflight, render-cache epochs and engine events.
extension AudioHost {
    /// A native source for one engine channel (-1: the return bus). It is keyed by
    /// channel, so its audio follows the channel when the return bus moves.
    func makeNativeSource(channel:Int32) -> AVAudioSourceNode {
        AVAudioSourceNode(format: nativeFormat) { [weak self] isSilence, time, frameCount, audioBufferList -> OSStatus in
                guard let self else { return noErr }
                self.rack.beginRenderRead()
                defer { self.rack.endRenderRead() }
                isSilence.pointee = false
                let buffers = UnsafeMutableAudioBufferListPointer(audioBufferList)
                guard buffers.count >= 2, let l = buffers[0].mData?.assumingMemoryBound(to: Float.self), let r = buffers[1].mData?.assumingMemoryBound(to: Float.self) else { return kAudio_ParamError }
                guard let start=Self.nativeGraphFrame(time.pointee,frames:frameCount) else { return kAudio_ParamError }
                self.renderLock.lock(); defer { self.renderLock.unlock() }
                self.renderClockDiagnostics?.stamp(master:false,sampleTime:time.pointee.mSampleTime,
                    flags:time.pointee.mFlags.rawValue,frames:frameCount,bus:channel,hostTime:AudioMIDITimebase.hostTime(time.pointee))
                // The same cache/event consumer also serves mixer preflight,
                // per-unit timing and frozen replay. One call supplies dry PCM.
                let hostTime=AudioMIDITimebase.hostTime(time.pointee)
                let alignedStart = self.nativeSourceUsesHostClock
                    ? self.midiTimebase.graphFrame(atHostTime:hostTime) ?? start : start
                return self.prepareCachedRangeLocked(graphFrame:alignedStart,frames:frameCount,
                    hostTime:AudioMIDITimebase.hostTime(time.pointee),channel:channel,left:l,right:r)
        }
    }

    func publishMasterRecording(_ capture:AudioRecording?) { publishRecording(capture,endpoint:masterRecordingEndpoint) }

    func publishInputRecording(_ capture:AudioRecording?) { publishRecording(capture,endpoint:inputRecordingEndpoint) }

    func publishRecording(_ capture:AudioRecording?,endpoint:UnsafeMutablePointer<Int64>) {
        let value:Int64
        if let capture {
            recordingHistory.append(capture)
            value = Int64(bitPattern:UInt64(UInt(bitPattern:Unmanaged.passUnretained(capture).toOpaque())))
        } else { value = 0 }
        var old = OSAtomicAdd64Barrier(0,endpoint)
        while !OSAtomicCompareAndSwap64Barrier(old,value,endpoint) { old = OSAtomicAdd64Barrier(0,endpoint) }
    }

    /// The main mixer is the native engine's render-clock authority. Leaving its
    /// implicit output at 44.1 kHz while our sources run at 48 kHz causes their
    /// sample coordinates to diverge until the shared PCM cache returns silence.
    /// Configure this engine connection, never the device's global nominal rate.
    func ensureProcessingOutputFormat() throws {
        nativeSourceUsesHostClock = !graph.isInManualRenderingMode
        guard nativeSourceUsesHostClock else { return }
        let current=graph.mainMixerNode.outputFormat(forBus:0)
        guard current.sampleRate != sampleRate || current.channelCount != 2 else { return }
        guard let format=AVAudioFormat(standardFormatWithSampleRate:sampleRate,channels:2) else {
            throw AudioSampleError.invalid("Cannot configure the processing output format.")
        }
        graph.connect(graph.mainMixerNode,to:graph.outputNode,format:format)
        renderCacheNeedsReset=true
    }

    func start() throws {
        guard !closed else { throw AudioSampleError.invalid("The audio engine is closed.") }
        guard routingFailure == nil, unavailableProject == nil else {
            throw AudioSampleError.invalid("Repair the project's unavailable routing before starting audio.")
        }
        if !graph.isRunning {
            try ensureProcessingOutputFormat()
            rack.notifyStateCaptureMutation()
            // Core Audio may stop the graph without going through the rack. Its
            // restarted sample clock must never reuse the previous cache epoch.
            renderCacheNeedsReset = true
            try resetRenderCacheIfNeeded()
            raiseHardwareSliceLimits()
            // Rewire for the current latency plan while the graph is uninitialized: a routed
            // rewire of a prepared graph whose hardware input chain is connected (live line
            // inputs) raises in AVAudioEngine (WillDisconnectFromMixer). prepare() allocates
            // render resources, so a plug-in may only report its latency then; release them
            // again for that second rewire.
            rack.refreshLatencyCompensation()
            graph.prepare()
            if !rack.lineInputSources.isEmpty && rack.routingRewirePending {
                graph.stop(); rack.refreshLatencyCompensation(); graph.prepare()
            } else { rack.refreshLatencyCompensation() }
            rack.resetRenderFailures()
            try resetRenderCacheIfNeeded()
            try graph.start()
        }
        if !graph.isInManualRenderingMode { realtimeAudioStarted = true }
    }

    /// The HAL input unit defaults to 512 frames per slice. With input and output on one
    /// device at a 1024 or 2048 buffer the engine asks it for 1025/2049 frames, the render
    /// fails and the device stops cycling (playback freezes). Raise both units' limit while
    /// the graph is stopped. The input node is only touched when it is already in use.
    func raiseHardwareSliceLimits() {
        // In manual rendering the output unit is the offline renderer: its limit is the
        // export/freeze quantum the caller chose, not a device slice.
        guard !graph.isInManualRenderingMode else { return }
        var frames: UInt32 = 8192
        let size = UInt32(MemoryLayout<UInt32>.size)
        if let unit = graph.outputNode.audioUnit {
            AudioUnitSetProperty(unit,kAudioUnitProperty_MaximumFramesPerSlice,kAudioUnitScope_Global,0,&frames,size)
        }
        if hardwareInputActive || selectedInputDevice != nil, let unit = graph.inputNode.audioUnit {
            AudioUnitSetProperty(unit,kAudioUnitProperty_MaximumFramesPerSlice,kAudioUnitScope_Global,0,&frames,size)
        }
    }

    @MainActor func recoverDeviceIfNeeded() {
        guard deviceRecoveryPending, !closed, !exporting, !graph.isInManualRenderingMode,
              audioConfigurationsInFlight == 0, graphMutationsInFlight == 0,
              pendingPluginRestore == nil, stateCaptureOwner == nil, !freezeTrackRuntime.busy,
              recording == nil, !recordingStopping else { return }
        deviceRecoveryPending = false
        guard !graph.isRunning else { return }
        do {
            // A new device may run at another rate: rebuild the monitor path
            // at the input's current format before line inputs are re-prepared.
            if inputMixer != nil { try rebuildInputMonitorPath() }
            if !lineInputs.isEmpty || !lineInputPaused.isEmpty || projectHasLineInputs { try synchronizeLineInputs(project,force:true) }
            try start()
            emit("audio.devices",audioDevices())
        } catch {
            graph.stop();renderCacheNeedsReset=true
            try? rack.setLineInputSources([:])
            for line in lineInputs.values { graph.disconnectNodeInput(line.node);graph.detach(line.node) }
            lineInputs.removeAll();lineInputSignature="";lineInputMapped=[:];lineInputError=error.localizedDescription
            // Playback must survive a lost or re-clocked input. Restart without
            // line inputs, then also without the monitor path if that still fails.
            var restarted=(try? start()) != nil
            if !restarted && inputMixer != nil { removeInputMonitorPath();restarted=(try? start()) != nil }
            emit("audio.error",["error":(restarted ? "Live inputs stopped after a device change: " : "Audio stopped after a device change: ")+error.localizedDescription])
            if restarted { emit("audio.devices",audioDevices()) }
        }
    }

    /// A device can stop delivering cycles while the graph still reports running
    /// (seen after a buffer-size change on a USB interface): no configuration
    /// change arrives, so recoverDeviceIfNeeded never runs. Stop the graph and
    /// let the normal recovery restart it.
    @MainActor func restartStalledDevice() {
        let eligible = graph.isRunning && realtimeAudioStarted && !closed && !exporting && !graph.isInManualRenderingMode &&
            audioConfigurationsInFlight == 0 && graphMutationsInFlight == 0 && recording == nil && !recordingStopping
        guard stallWatch.stalled(now:ProcessInfo.processInfo.systemUptime,quanta:OSAtomicAdd64Barrier(0,renderQuanta),eligible:eligible) else { return }
        NSLog("SuperTracker: the audio device stopped delivering buffers; restarting the engine")
        graph.stop(); renderCacheNeedsReset = true
        // Large buffers are what stall USB interfaces here: restart at a size they keep up with.
        let lowered = lowerStalledBufferSize()
        deviceRecoveryPending = true
        emit("audio.error",["error":lowered.map { "The audio interface stopped responding at a buffer of \($0) samples; switched to \(Self.stallFallbackBuffer) and restarted audio." }
            ?? "The audio interface stopped responding; audio was restarted. If this repeats, choose a smaller buffer size."])
    }

    static let stallFallbackBuffer: UInt32 = 1024

    /// Sets the output device to `stallFallbackBuffer` frames when it is larger, and saves that
    /// choice. Returns the previous size, or nil when nothing changed.
    @MainActor func lowerStalledBufferSize() -> UInt32? {
        guard let unit = graph.outputNode.audioUnit else { return nil }
        var device: AudioDeviceID = 0; var bytes = UInt32(MemoryLayout<AudioDeviceID>.size)
        guard AudioUnitGetProperty(unit,kAudioOutputUnitProperty_CurrentDevice,kAudioUnitScope_Global,0,&device,&bytes) == noErr, device != 0 else { return nil }
        var property = AudioObjectPropertyAddress(mSelector:kAudioDevicePropertyBufferFrameSize,mScope:kAudioObjectPropertyScopeGlobal,mElement:kAudioObjectPropertyElementMain)
        var current: UInt32 = 0; bytes = UInt32(MemoryLayout<UInt32>.size)
        guard AudioObjectGetPropertyData(device,&property,0,nil,&bytes,&current) == noErr, current > Self.stallFallbackBuffer else { return nil }
        var frames = Self.stallFallbackBuffer
        guard AudioObjectSetPropertyData(device,&property,0,nil,UInt32(MemoryLayout<UInt32>.size),&frames) == noErr else { return nil }
        if let url = devicePreferencesURL {
            var saved = AudioDevicePreferences.load(url); saved.bufferSize = Int(frames); try? saved.save(url)
        }
        refreshHardwareReturnLatency()
        emit("audio.devices",audioDevices())
        return current
    }

    func emit(_ name: String,_ data:[String:Any]) {
        if Thread.isMainThread { onEvent?(name,data) }
        else { DispatchQueue.main.async { [weak self] in self?.onEvent?(name,data) } }
    }

    /// Preserve the existing source timestamp contract (manual converter stamps
    /// may be unflagged), but reject unsafe casts and fractional cache positions.
    static func nativeGraphFrame(_ stamp:AudioTimeStamp,frames:UInt32)->Int64? {
        let value=stamp.mSampleTime
        guard frames > 0, frames <= 65_536, value.isFinite, value.rounded(.towardZero)==value,
              value >= -Double(Int64(1)<<52), value <= Double((Int64(1)<<52)-Int64(frames)) else { return nil }
        return Int64(value)
    }

    /// Caller holds renderLock exactly once. Offsets from the native event ring
    /// are relative to THIS request, including its cached prefix, never to a
    /// guessed newly rendered suffix. This helper does not invoke child DSP.
    func prepareCachedRangeLocked(graphFrame:Int64,frames:UInt32,hostTime:UInt64,
        channel:Int32=0,left:UnsafeMutablePointer<Float>?=nil,right:UnsafeMutablePointer<Float>?=nil,
        updateTransport:Bool=false)->OSStatus {
        guard frames > 0, frames <= 65_536, graphFrame >= -(Int64(1)<<52),
              graphFrame <= (Int64(1)<<52)-Int64(frames) else { return kAudio_ParamError }
        st_engine_render_channel(engine,channel,UInt64(max(0,graphFrame)),left,right,frames)
        if let renderClockDiagnostics {
            var clock=STClockSpan()
            let clockStatus=st_engine_clock_span(engine,graphFrame,1,0,&clock)
            renderClockDiagnostics.prepared(frame:graphFrame,status:clockStatus,epoch:clock.context.epoch,flags:Int32(clock.context.flags))
        }
        if updateTransport {
            let state=st_engine_status(engine),controls=renderControls.read()
            // Plug-ins expect song (timeline) position, not the graph's free-running frame.
            // The prepared clock at this quantum's first frame is exact, including after
            // seeks and stopped graph time; status time is the fallback (end of the render).
            var clock=STClockSpan()
            let samplePosition:Double,beatPosition:Double
            if st_engine_clock_span(engine,graphFrame,1,0,&clock) == ST_CLOCK_OK {
                samplePosition=Double(clock.context.songFrame); beatPosition=clock.context.beat
            } else {
                let seconds=max(0,state.seconds-Double(frames)/sampleRate)
                samplePosition=(seconds*sampleRate).rounded(); beatPosition=seconds*controls.tempo/60
            }
            rack.setTransport(bpm:controls.tempo,playing:state.playing != 0,recording:state.recording != 0,
                samplePosition:samplePosition,beatPosition:beatPosition)
            if !exporting {
                midiClock.render(beat:beatPosition,playing:state.playing != 0,tempo:controls.tempo,frames:frames,sampleRate:sampleRate,
                    hostTime:hostTime,ticksPerFrame:hostTicksPerFrame)
            }
            drainEvents(hostTime:hostTime,sampleTime:graphFrame,controls:controls)
        } else { drainEvents(hostTime:hostTime,sampleTime:graphFrame) }
        return noErr
    }

    /// Main-mixer preflight is the transport anchor for every render quantum.
    func prepareRender(_ time:UnsafePointer<AudioTimeStamp>,frames:AUAudioFrameCount) {
        guard let graphFrame=Self.nativeGraphFrame(time.pointee,frames:frames) else { return }
        OSAtomicIncrement64Barrier(renderQuanta)
        atomicStore(graphFrame, at: rack.graphQuantum)
        renderLock.lock(); defer { renderLock.unlock() }
        renderClockDiagnostics?.stamp(master:true,sampleTime:time.pointee.mSampleTime,
            flags:time.pointee.mFlags.rawValue,frames:frames,hostTime:AudioMIDITimebase.hostTime(time.pointee))
        midiTimebase.observe(time.pointee)
        if OSAtomicCompareAndSwap32Barrier(1,0,routingAutomationReset) {
            rack.resetAutomation(); st_engine_refresh_automation(engine)
        }
        _ = prepareCachedRangeLocked(graphFrame:graphFrame,frames:frames,hostTime:AudioMIDITimebase.hostTime(time.pointee),updateTransport:true)
    }

    func resetRenderCacheIfNeeded() throws {
        guard renderCacheNeedsReset else { return }
        try resetRenderCache()
        renderCacheNeedsReset = false
    }

    func resetRenderCache() throws {
        try rack.awaitRenderQuiescence()
        st_engine_reset_render_cache(engine);rack.resetAutomation()
        midiTimebase.reset()
    }

    func drainEvents(hostTime:UInt64,sampleTime:Int64,controls:AudioRenderControlSnapshot? = nil) {
        var event = STEvent()
        guard st_engine_pop_event(engine,&event) != 0 else { return }
        let snapshot = controls ?? renderControls.read()
        repeat {
            let absoluteTime = sampleTime+Int64(event.frameOffset)
            let route = snapshot.routes[max(0,min(AudioRenderControl.routeCount-1,Int(event.instrument)))]
            switch event.type {
            case Int32(ST_EVENT_NOTE_ON), Int32(ST_EVENT_NOTE_OFF):
                let on = event.type == ST_EVENT_NOTE_ON
                if route.external && !exporting {
                    let legacyTimestamp = hostTime > 0 ? Self.externalNoteTimestamp(hostTime:hostTime,frameOffset:event.frameOffset,
                        ticksPerFrame:hostTicksPerFrame,leadTicks:route.leadTicks) : 0
                    route.output?.send(status:(on ? 0x90 : 0x80) | route.channel,data1:UInt8(max(0,min(127,event.note))),data2:UInt8(max(0,min(127,event.velocity))),timestamp:legacyTimestamp)
                } else {
                    rack.sendMIDI(track:Int(event.track),note:Int(event.note),velocity:Int(event.velocity),on:on,sampleOffset:Int(event.frameOffset),sampleTime:absoluteTime,sequenced:event.slot == 1)
                }
            case Int32(ST_EVENT_PARAMETER):
                if event.target == ST_TARGET_PLUGIN {
                    rack.automate(track:Int(event.track),slot:max(0,Int(event.slot)),address:event.address,value:Float(event.value),sampleOffset:Int(event.frameOffset),kind:event.slot < 0 ? "instrument" : "effect",sampleTime:absoluteTime)
                } else { rack.automateTrack(track:Int(event.track),target:Int(event.target),value:Float(event.value),sampleTime:absoluteTime) }
            case Int32(ST_EVENT_HOST_PARAMETER):
                rack.automateHost(destination:Int(event.target),address:event.address,value:event.value,sampleTime:absoluteTime,map:event.hostMap)
            case Int32(ST_EVENT_ALL_NOTES_OFF):
                rack.allNotesOff(sampleTime:absoluteTime)
                if !exporting {
                    for index in 0..<128 {
                        let route = snapshot.routes[index]
                        if route.external {
                            route.output?.send(status:0xB0 | route.channel,data1:64,data2:0)
                            route.output?.send(status:0xB0 | route.channel,data1:123,data2:0)
                        }
                    }
                }
            default: break
            }
        } while st_engine_pop_event(engine,&event) != 0
    }

    /// Host time for a note to an External MIDI instrument: the render's host time plus the
    /// event's frame offset, less the route's lead (never at or before the clock's origin).
    @inline(__always)
    static func externalNoteTimestamp(hostTime:UInt64,frameOffset:Int32,ticksPerFrame:Double,leadTicks:UInt32)->UInt64 {
        let stamp = hostTime + UInt64(max(0,Double(frameOffset)*ticksPerFrame))
        return stamp > UInt64(leadTicks) ? stamp - UInt64(leadTicks) : stamp
    }

    func panicExternal() {
        cancelMIDICaptures(reason:"transport interrupted")
        rack.allNotesOff()
        var sent = Set<String>()
        for route in midiRoutes where route.external {
            let key = "\(route.destination ?? 0):\(route.channel)"
            if sent.insert(key).inserted {
                midi.send(status:0xB0 | route.channel,data1:64,data2:0,destination:route.destination)
                midi.send(status:0xB0 | route.channel,data1:123,data2:0,destination:route.destination)
                midi.send(status:0xB0 | route.channel,data1:120,data2:0,destination:route.destination)
            }
        }
        heldNotes.removeAll(); midiSustain.clear()
        midiNoteDestinations.removeAll(); midiExpandedNotes.removeAll()
    }

    /// The single manual-render entry point owns timestamp authority for the
    /// exact synchronous call, including valid unflagged converter subrequests.
    /// It never starts a device or changes transport; callers own their render
    /// transaction and buffers. Internal fixtures use this same production path.
    @MainActor func renderOfflineQuantum(frames:AUAudioFrameCount,to buffer:AVAudioPCMBuffer) throws -> AVAudioEngineManualRenderingStatus {
        guard graph.isInManualRenderingMode, graph.isRunning, frames > 0,
              frames <= graph.manualRenderingMaximumFrameCount, frames <= buffer.frameCapacity else {
            throw AudioSampleError.invalid("Offline rendering requires a running manual graph and a bounded output buffer.")
        }
        let sourceFrame = Int64(graph.manualRenderingSampleTime)
        let capture = recording?.source == .master ? recording : nil
        capture?.observeManualQuantum(frame:sourceFrame,frames:frames)
        let result:AVAudioEngineManualRenderingStatus
        do { result = try graph.renderOffline(frames,to:buffer) }
        catch { capture?.observeManualQuantum(frame:sourceFrame,frames:frames,failed:true); throw error }
        if result != .success { capture?.observeManualQuantum(frame:sourceFrame,frames:frames,failed:true) }
        if let failure=rack.renderFailure() {
            capture?.observeManualQuantum(frame:sourceFrame,frames:frames,failed:true)
            throw AudioSampleError.invalid(failure["message"] as? String ?? "A plug-in failed to render.")
        }
        return result
    }
}

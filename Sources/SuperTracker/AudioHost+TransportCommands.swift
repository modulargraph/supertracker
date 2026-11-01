import Foundation
import AppKit
import AVFoundation
import AudioToolbox
import CoreAudio
import TrackerEngine

/// Transport, auditions, live notes and MIDI configuration.
extension AudioHost {
    /// Returns nil for commands outside this family.
    @MainActor func handleTransportCommand(_ command: String, _ payload: [String: Any]) async throws -> Any? {
        switch command {
        case "transport.recordArm":
            guard let armed = PayloadValue.boolean(payload["armed"]) else {
                throw AudioSampleError.invalid("Record arm requires an armed Boolean.")
            }
            if noteRecordArmed != armed {
                inputArmGeneration &+= 1
                cancelMIDICaptures(reason:"record arm changed")
            }
            noteRecordArmed = armed
            publishRecordingIntent()
            return ["armed":noteRecordArmed]
        case "transport.play":
            playRequestGeneration &+= 1
            let playRequest = playRequestGeneration, identity = projectIdentity
            playPreparations += 1
            defer { playPreparations -= 1 }
            await awaitProjectAssets()
            try Task.checkCancellation()
            guard !closed, playRequest == playRequestGeneration, identity == projectIdentity else {
                throw AudioSampleError.invalid("The Play request was superseded by a transport command or project change.")
            }
            try requireInteractiveMutation()
            rack.notifyStateCaptureMutation()
            rack.resetRenderFailures()
            let song = payload["mode"] as? String != "pattern" && payload["songMode"] as? Bool != false
            if payload["fromStart"] as? Bool == true {
                st_engine_seek(engine,Int32(clamping:PayloadValue.integer(project,"activePattern",0)),0,0)
            }
            try start()
            songModeRequested = song
            // Pattern loop range rides with each Play; a Play without one clears it,
            // so ordinary playback is unchanged.
            let loop = song ? nil : Self.patternLoop(payload["loop"])
            st_engine_set_pattern_loop(engine,Int32(clamping:loop?.start ?? -1),Int32(clamping:loop?.end ?? -1))
            // Optional count-in (bars): the engine clicks one bar per count, then starts Play sample-accurately.
            let countIn = max(0,min(4,PayloadValue.integer(payload,"countIn",0)))
            if countIn > 0 { st_engine_play_count_in(engine,song ? 1 : 0,Int32(countIn)) }
            else { st_engine_play(engine,song ? 1 : 0) }
            return status()
        case "transport.loop":
            let loop = Self.patternLoop(payload["loop"] ?? payload)
            st_engine_set_pattern_loop(engine,Int32(clamping:loop?.start ?? -1),Int32(clamping:loop?.end ?? -1))
            return status()
        case "transport.queue":
            // Queued scene/pattern change, honoured by the engine at the next pattern end while playing.
            // {pattern} (Pattern mode, or first occurrence in Song mode), {sequence} (Song mode index), {clear:true}.
            // {mode:"song"|"pattern"} switches the playback mode at that same boundary: Pattern mode loops
            // {pattern}; Song mode continues at {sequence}.
            let clear = payload["clear"] as? Bool == true
            let pattern = clear ? -1 : PayloadValue.integer(payload,"pattern",-1), sequence = clear ? -1 : PayloadValue.integer(payload,"sequence",-1)
            guard clear || pattern >= 0 || sequence >= 0 else { throw AudioSampleError.invalid("Queue a pattern or sequence position.") }
            var transport = st_engine_status(engine)
            let playing = st_status_transport_playing(&transport) != 0
            if !clear, let mode = payload["mode"] as? String {
                guard mode == "song" || mode == "pattern" else { throw AudioSampleError.invalid("Queue mode must be song or pattern.") }
                guard mode == "song" ? sequence >= 0 : pattern >= 0 else { throw AudioSampleError.invalid("Queue a sequence position for Song mode or a pattern for Pattern mode.") }
                st_engine_queue_mode(engine,mode == "song" ? 1 : 0,Int32(clamping:pattern),Int32(clamping:sequence))
                songModeRequested = mode == "song"
                return ["queued":playing,"mode":mode,"pattern":pattern,"sequence":sequence]
            }
            st_engine_queue_jump(engine,Int32(clamping:pattern),Int32(clamping:sequence))
            return ["queued":playing && !clear,"pattern":pattern,"sequence":sequence]
        case "transport.stop":
            st_engine_stop(engine); panicExternal(); return status()
        case "transport.seek":
            let pattern=PayloadValue.integer(payload,"pattern",PayloadValue.integer(project,"activePattern",0)), sequence=PayloadValue.integer(payload,"sequence",0)
            st_engine_seek(engine,Int32(clamping:pattern),Int32(clamping:PayloadValue.integer(payload,"row",0)),Int32(clamping:sequence)); return status()
        case "engine.panic":
            lastExpression.removeAll()
            st_engine_panic(engine); panicExternal(); rack.resetLatencyCompensation(); previewPlayer?.stop(); return status()
        case "step.preview":
            guard let data = payload["step"] as? [String:Any] else { throw AudioSampleError.invalid("Choose a tracker step to audition.") }
            var step = AudioProject.step(data)
            let target=payload
            let destination=try noteDestination(target)
            let track=Int32(destination.channel)
            previewGeneration &+= 1
            let generation = previewGeneration
            if step.note < 0 {
                st_engine_preview_step(engine,track,step,nil,0,0.03)
                return ["auditioning":false]
            }
            previewPreparations += 1
            defer { previewPreparations -= 1 }
            let identity = projectIdentity
            await awaitProjectAssets()
            try Task.checkCancellation()
            try requireInteractiveMutation()
            let currentDestination=try noteDestination(target)
            guard currentDestination.channel == destination.channel,currentDestination.instrument == destination.instrument,
                  identity == projectIdentity, generation == previewGeneration else {
                throw AudioSampleError.invalid("Step audition was superseded by a newer audition or project.")
            }
            rack.notifyStateCaptureMutation()
            let chord = (data["chord"] as? [Int] ?? []).prefix(8).map { Int32(clamping:$0) }
            try start()
            chord.withUnsafeBufferPointer {
                st_engine_preview_step(engine,track,step,$0.baseAddress,Int32(chord.count),Float(PayloadValue.number(payload,"duration",0.35)))
            }
            return ["auditioning":true]
        case "note.on":
            let destination = try noteDestination(payload)
            try start()
            let track = destination.channel, instrument = destination.instrument
            midiTrack = track; midiInstrument = instrument
            midiSustain.retrigger(source:Self.appNoteSource,channel:1,track:track,note:max(0,min(127,PayloadValue.integer(payload,"note",60))))
            if payload["captureInput"] as? Bool == true {
                return await captureKeyboardInput(payload,on:true,track:track,instrument:instrument)
            }
            st_engine_note_on(engine,Int32(track),Int32(instrument),Int32(clamping:PayloadValue.integer(payload,"note",60)),Int32(clamping:PayloadValue.integer(payload,"velocity",100)))
            return ["ok":true]
        case "note.off":
            // A held sustain pedal (midi.expression) defers the sounding release; the key release is still captured.
            let releaseNote = max(0,min(127,PayloadValue.integer(payload,"note",60)))
            let destination=try noteDestination(payload)
            let releaseTrack=destination.channel
            let sustained = (payload["applyNote"] as? Bool ?? true) && midiSustain.deferNoteOff(source:Self.appNoteSource,channel:1,track:releaseTrack,note:releaseNote)
            if payload["captureInput"] as? Bool == true {
                var release = payload; if sustained { release["applyNote"] = false }
                return await captureKeyboardInput(release,on:false,track:releaseTrack,
                    instrument:destination.instrument)
            }
            if sustained { return ["ok":true,"sustained":true] }
            st_engine_note_off(engine,Int32(releaseTrack),Int32(releaseNote)); return ["ok":true]
        case "engine.instrumentParameter":
            // A knob drag heard on sounding voices at once, without republishing the project (the commit does that).
            let instrument = PayloadValue.integer(payload,"instrument",-1), name = payload["target"] as? String ?? ""
            let value = PayloadValue.number(payload,"value",.nan)
            guard instrument >= 0, instrument < ordinaryInstruments, value.isFinite, let target = Self.liveInstrumentTarget(name) else {
                throw AudioSampleError.invalid("Unsupported live instrument parameter \(name).")
            }
            return ["ok": st_engine_set_instrument_parameter(engine,Int32(instrument),target,Float(value)) != 0]
        case "sampler.playheads":
            // The sample editor shows where the instrument's voices play while it is visible.
            if PayloadValue.boolean(payload["enabled"]) == true { startPlayheads(instrument: PayloadValue.integer(payload,"instrument",-1)) } else { stopPlayheads() }
            return ["ok": true, "watching": playheadWatch]
        case "midi.expression":
            return try expression(payload)
        case "midi.devices": return midi.devices()
        case "midi.clock": return try configureMIDIClock(payload)
        case "midi.performance":
            setMIDIPerformanceMap(payload["map"])
            return ["active": midiPerformanceMap != nil]
        case "midi.configure":
            // Release held input notes only when the input route actually changes, not
            // whenever a payload merely repeats the current input/channel/enable values.
            let inputRouteFields = ["input","inputID","inputIDs","inputs","inputChannel","enabled"]
            let routeBefore = payload.keys.contains(where:inputRouteFields.contains) ? midiInputRoute(midi.devices()) : nil
            if payload["track"] != nil || payload["instrument"] != nil {
                midiTrack=lastTrack(PayloadValue.integer(payload,"track",0)); midiInstrument=max(0,min(ordinaryInstruments-1,PayloadValue.integer(payload,"instrument",0)))
            }
            let previousInputChannel = midiInputChannel
            midiInputChannel = PayloadValue.integer(payload,"inputChannel",midiInputChannel)
            let settings:[String:Any]
            do { settings = try midi.configure(payload:payload) } catch { midiInputChannel = previousInputChannel; throw error }
            if let routeBefore, previousInputChannel != midiInputChannel || routeBefore != midiInputRoute(settings) { releaseInputNotes() }
            if payload["enabled"] as? Bool != false {
                // The interface publishes its MIDI route as soon as it is ready, while a just-opened
                // project's plug-ins are still restoring (and must not be processing then). Start once
                // they have loaded, as live inputs do.
                if pendingPluginRestore != nil && !graph.isRunning { midiAutoStart = true } else { try start() }
            }
            return settings

        default: return nil
        }
    }

    /// Engine target for a live instrument parameter: the instrument's filter, or an instrument automation target.
    static func liveInstrumentTarget(_ name: String) -> Int32? {
        if name == "cutoff" { return 2 }
        if name == "resonance" { return 3 }
        guard let target = AudioProject.nativeAutomationTargets[name], target >= 8 else { return nil }
        return target
    }

    /// Polls the engine's voice snapshot at display rate while the sample editor is visible; events only on change.
    func startPlayheads(instrument: Int) {
        playheadWatch = max(-1, instrument)
        st_engine_watch_voice_positions(engine, Int32(playheadWatch))
        guard playheadTimer == nil else { return }
        var buffer = [STVoicePosition](repeating: STVoicePosition(), count: Int(ST_VOICE_POSITION_CAPACITY))
        var lastSerial: UInt64 = 0
        playheadTimer = Timer.scheduledTimer(withTimeInterval: 1.0 / 30, repeats: true) { [weak self] _ in
            guard let self, !self.closed, !self.engineDestroyed else { return }
            var serial: UInt64 = 0
            let count = Int(buffer.withUnsafeMutableBufferPointer { st_engine_voice_positions(self.engine, $0.baseAddress, Int32($0.count), &serial) })
            if count == 0 && !self.playheadsShown { return }
            if serial == lastSerial && count > 0 { return }
            lastSerial = serial; self.playheadsShown = count > 0
            self.emit("sampler.playheads", ["instrument": self.playheadWatch, "voices": buffer.prefix(count).map(Self.playheadPayload)])
        }
    }

    func stopPlayheads() {
        playheadTimer?.invalidate(); playheadTimer = nil; playheadWatch = -2; playheadsShown = false
        if !engineDestroyed { st_engine_watch_voice_positions(engine, -2) }
    }

    static func playheadPayload(_ v: STVoicePosition) -> [String: Any] {
        var grains: [[Double]] = []
        withUnsafeBytes(of: v.grainPositions) { positions in
            withUnsafeBytes(of: v.grainLevels) { levels in
                let p = positions.bindMemory(to: Double.self), l = levels.bindMemory(to: Float.self)
                for k in 0..<min(Int(v.grainCount), p.count) { grains.append([p[k], Double(l[k])]) }
            }
        }
        return ["instrument": Int(v.instrument), "track": Int(v.track), "note": Int(v.note), "mode": Int(v.mode), "direction": Int(v.direction),
                "releasing": v.flags & Int32(ST_VOICE_POSITION_RELEASING) != 0, "zone": v.flags & Int32(ST_VOICE_POSITION_ZONE) != 0,
                "preview": v.flags & Int32(ST_VOICE_POSITION_PREVIEW) != 0,
                "position": v.position, "windowStart": v.windowStart, "windowEnd": v.windowEnd, "frames": v.sampleFrames,
                "level": Double(v.level), "grains": grains]
    }
}

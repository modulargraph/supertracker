import Foundation
import AppKit
import AVFoundation
import AudioToolbox
import CoreAudio
import TrackerEngine

/// MIDI and computer-keyboard input: routing, live notes, input capture for recording, expression.
extension AudioHost {
    func refreshMIDIRoutes() {
        guard !closed else { return }
        let instruments = ProjectLayout.instruments(project)
        var routes = Array(repeating:MIDIRoute(),count:AudioRenderControl.routeCount)
        // Synths played from a track and heard back through that track's line input.
        let tracks = project["tracks"] as? [[String:Any]] ?? []
        let returned = Set(lineInputs.keys.compactMap { tracks.indices.contains($0) ? PayloadValue.integer(tracks[$0],"instrument",0) : nil })
        let leadTicks = UInt32(clamping:AVAudioTime.hostTime(forSeconds:max(0,min(0.5,returnedNoteLeadSeconds))))
        for (index,item) in instruments.prefix(AudioRenderControl.routeCount).enumerated() {
            routes[index].external = item["type"] as? String == "midi"
            if let value = item["midiOutput"] as? NSNumber { routes[index].destination = value.int32Value }
            else if let value = item["midiOutput"] as? String { routes[index].destination = Int32(value) }
            if routes[index].destination == 0 { routes[index].destination = nil }
            routes[index].channel = UInt8(max(0,min(15,PayloadValue.integer(item,"midiChannel",1)-1)))
            if routes[index].external { routes[index].output = midi.prepareDestination(routes[index].destination) }
            if routes[index].external && returned.contains(index) { routes[index].leadTicks = leadTicks }
        }
        let previous = midiRoutes
        for index in previous.indices where previous[index].external {
            let old = previous[index], next = routes[index]
            if old.output == nil, let resumed=next.output, next.external,
               old.destination == next.destination, old.channel == next.channel {
                // A returning interface can still drive a synth whose prior release
                // was lost while unplugged. Clear that channel before new notes.
                resumed.send(status:0xB0 | next.channel,data1:64,data2:0)
                resumed.send(status:0xB0 | next.channel,data1:123,data2:0)
            }
            if old.channel != next.channel || old.output?.endpoint != next.output?.endpoint || !next.external {
                lastExpression.removeAll(keepingCapacity:true)
                old.output?.send(status:0xB0 | old.channel,data1:64,data2:0)
                old.output?.send(status:0xB0 | old.channel,data1:123,data2:0)
                old.output?.send(status:0xB0 | old.channel,data1:123,data2:0,timestamp:mach_absolute_time()+AVAudioTime.hostTime(forSeconds:0.1))
            }
        }
        midiRoutes = routes
        renderControls.publish(tempo:tempo,routes:routes)
    }

    @MainActor func captureKeyboardInput(_ payload:[String:Any],on:Bool,track:Int,instrument:Int) async -> [String:Any] {
        let note=max(0,min(127,PayloadValue.integer(payload,"note",60))),velocity=max(0,min(127,PayloadValue.integer(payload,"velocity",on ? 100 : 0)))
        let owner=payload["inputOwner"] as? String ?? ""
        let applyNote=payload["applyNote"] as? Bool ?? true
        return await withCheckedContinuation { continuation in
            let accepted=enqueueInputCapture(owner:owner,on:on,track:track,instrument:instrument,note:note,velocity:velocity,
                applyNote:applyNote) { acknowledgement in
                    continuation.resume(returning:["ok":true,"inputAcknowledgement":acknowledgement])
                }
            if accepted == nil && applyNote {
                if on { st_engine_note_on(engine,Int32(track),Int32(instrument),Int32(note),Int32(velocity)) }
                else { st_engine_note_off(engine,Int32(track),Int32(note)) }
            }
        }
    }

    @discardableResult
    func enqueueInputCapture(owner:String,on:Bool,track:Int,instrument:Int,note:Int,velocity:Int,applyNote:Bool,
        completion:@escaping ([String:Any])->Void) -> UInt64? {
        let identity=projectIdentity,arm=inputArmGeneration
        let context=AudioInputCapture.recordingContext(project)
        let patterns=(project["patterns"] as? [[String:Any]] ?? []).enumerated().map { PayloadValue.integer($0.element,"id",$0.offset) }
        let tracks=liveTracks,instruments=liveInstruments
        let lane=track
        let trackID=tracks.indices.contains(lane) ? tracks[lane]["routingID"] as? String : nil
        let finish:([String:Any])->Void = completion
        let instrumentID=instruments.indices.contains(instrument) ? instruments[instrument]["id"] as? String : nil
        let request=AudioInputCapture.Request(owner:owner,on:on,track:track,instrument:instrument,note:note,velocity:velocity,
            applyNote:applyNote,context:context,patternIDs:Set(patterns))
        guard !owner.isEmpty,owner.utf8.count <= 256 else {
            finish(["version":1,"inputOwner":owner,"status":"unavailable","applied":false,"playing":false,"recording":false,
                "mapped":false,"on":on,"track":track,"instrument":instrument,"note":note,"velocity":velocity,"applyNote":applyNote,
                "recordingContext":context,"reason":"Input recording requires a bounded correlation ID."])
            return nil
        }
        return inputCapture.enqueue(request,isCurrent:{ [weak self] in
            guard let self,!self.closed,identity == self.projectIdentity,arm == self.inputArmGeneration else { return false }
            let currentTracks=self.liveTracks
            let currentInstruments=self.liveInstruments
            guard currentTracks.indices.contains(lane),currentInstruments.indices.contains(instrument) else { return false }
            return (trackID == nil || currentTracks[lane]["routingID"] as? String == trackID)
                && (instrumentID == nil || currentInstruments[instrument]["id"] as? String == instrumentID)
        },completion:finish)
    }

    func cancelMIDICaptures(reason:String) {
        let old=midiCaptureOwners; midiCaptureOwners.removeAll()
        for value in old.values {
            emit("midi.note",["on":false,"inputOwner":value.owner,"recordingOnly":true,"recordingCancelled":true,"reason":reason])
        }
    }

    /// Immediate MIDI delivery remains performance/highlight only when a capture
    /// marker is pending. A separate session-fenced ACK event owns recording.
    func captureMIDIInput(key:String,on:Bool,track:Int,instrument:Int,note:Int,velocity:Int,channel:Int,source:Int32,applyNote:Bool) -> Bool {
        let owner:String,onsetID:UInt64?
        if on {
            guard noteRecordArmed else { return false }
            if let previous=midiCaptureOwners.removeValue(forKey:key) {
                emit("midi.note",["on":false,"inputOwner":previous.owner,"recordingOnly":true,"recordingCancelled":true,"reason":"retriggered"])
            }
            owner="midi:"+UUID().uuidString; onsetID=nil
        } else {
            guard let previous=midiCaptureOwners.removeValue(forKey:key) else { return false }
            owner=previous.owner; onsetID=previous.onsetID
        }
        let identity=projectIdentity,arm=inputArmGeneration
        var immediate:[String:Any]=["on":on,"note":note,"velocity":velocity,"channel":channel,"source":source,
            "track":track,"instrument":instrument,"inputOwner":owner,"recordingPending":true]
        if let onsetID { immediate["onsetRequestID"]=String(onsetID) }
        emit("midi.note",immediate)
        let id=enqueueInputCapture(owner:owner,on:on,track:track,instrument:instrument,note:note,velocity:velocity,applyNote:applyNote) { [weak self] ack in
            guard let self,!self.closed,identity == self.projectIdentity,arm == self.inputArmGeneration else { return }
            var event=immediate; event.removeValue(forKey:"recordingPending")
            event["recordingOnly"]=true; event["inputAcknowledgement"]=ack
            self.emit("midi.note",event)
        }
        if on { midiCaptureOwners[key]=(owner,id) }
        if id == nil && applyNote {
            if on { st_engine_note_on(engine,Int32(track),Int32(instrument),Int32(note),Int32(velocity)) }
            else { st_engine_note_off(engine,Int32(track),Int32(note)) }
        }
        return true
    }

    @MainActor func setMIDIPerformanceMap(_ value: Any?) {
        guard let rows = value as? [[Any]], rows.count == 128 else { midiPerformanceMap = nil; return }
        let map = rows.map { row in Array(row.compactMap { ($0 as? NSNumber)?.intValue }.filter { (0...127).contains($0) }.prefix(8)) }
        midiPerformanceMap = map.enumerated().allSatisfy { $0.element == [$0.offset] } ? nil : map
    }

    // CoreMIDI dispatches here on the main queue. Internal visibility permits
    // offline ingress tests without broadcasting notes to other open apps.
    @MainActor func receiveMIDI(_ status:UInt8,_ data1:UInt8,_ data2:UInt8,source:Int32) {
        let kind = status & 0xf0
        guard kind == 0x90 || kind == 0x80, data1 < 128 else {
            receiveMIDIDirect(status,data1,data2,source:source); return
        }
        let key = "\(source):\(status & 0x0f):\(data1)"
        if kind == 0x90 && data2 > 0 {
            if let previous = midiExpandedNotes.removeValue(forKey:key) {
                for note in previous { receiveMIDIDirect(0x80 | (status & 0x0f),UInt8(note),0,source:source) }
            }
            guard let map = midiPerformanceMap else {
                // Preserve ordinary raw-note retrigger/legato behavior. A raw
                // key has no expansion to retain; its unmatched release is raw.
                receiveMIDIDirect(status,data1,data2,source:source); return
            }
            let notes = map[Int(data1)]
            midiExpandedNotes[key] = notes
            for note in notes { receiveMIDIDirect(status,UInt8(note),data2,source:source) }
        } else {
            // A later mode/map change does not change ownership of held pitches.
            // Without an owned onset, release only the actual incoming pitch.
            for note in midiExpandedNotes.removeValue(forKey:key) ?? [Int(data1)] { receiveMIDIDirect(status,UInt8(note),data2,source:source) }
        }
    }

    /// Live performance expression for one track's instrument (Maschine strips, pad pressure, pedal), using the
    /// same destinations as hardware MIDI input: AU/VST plug-ins get the MIDI message, external MIDI instruments
    /// get it on their output channel, built-in instruments get pitch bend (+/-2 semitones) and CC1 as the
    /// engine mod wheel (filter close). Sustain (CC64) also defers the app's own note releases on any instrument.
    /// value: pitchBend -1...1, cc/aftertouch/polyAftertouch 0...1. Identical repeated values are not re-sent.
    @MainActor func expression(_ payload:[String:Any]) throws -> [String:Any] {
        guard !closed, routingFailure == nil, unavailableProject == nil else { return ["sent":false] }
        let tracks = project["tracks"] as? [[String:Any]] ?? []
        var selected=payload
        let lane=lastTrack(PayloadValue.integer(payload,"track",midiTrack))
        selected["instrument"]=tracks.indices.contains(lane) ? PayloadValue.integer(tracks[lane],"instrument",midiInstrument) : midiInstrument
        let destination=try noteDestination(selected),track=destination.channel,instrument=destination.instrument
        let value = PayloadValue.number(payload,"value",0)
        guard value.isFinite else { throw AudioSampleError.invalid("Expression value must be a finite number.") }
        let seven = UInt8(max(0,min(127,Int((max(0,min(1,value))*127).rounded()))))
        var kind = payload["kind"] as? String ?? ""
        var controller = max(0,min(127,PayloadValue.integer(payload,"controller",1)))
        if kind == "sustain" { kind = "cc"; controller = 64 }
        let message:(status:UInt8,data1:UInt8,data2:UInt8)
        switch kind {
        case "pitchBend":
            let wheel = max(0,min(16383,Int(((max(-1,min(1,value))+1)*8192).rounded())))
            message = (0xE0,UInt8(wheel & 0x7f),UInt8(wheel >> 7))
        case "cc":
            guard controller < 120 else { throw AudioSampleError.invalid("Use engine.panic for channel-mode messages.") }
            message = (0xB0,UInt8(controller),seven)
        case "aftertouch": message = (0xD0,seven,0)
        case "polyAftertouch":
            guard let note = (payload["note"] as? NSNumber)?.intValue, (0...127).contains(note) else { throw AudioSampleError.invalid("Poly aftertouch needs a note.") }
            message = (0xA0,UInt8(note),seven)
        default: throw AudioSampleError.invalid("Unsupported expression kind: \(kind).")
        }
        let key = "\(track):\(instrument):\(message.status):\(message.status == 0xB0 || message.status == 0xA0 ? Int(message.data1) : -1)"
        let packed = Int(message.data1) << 8 | Int(message.data2)
        if lastExpression[key] == packed { return ["sent":false,"track":track,"instrument":instrument] }
        lastExpression[key] = packed
        if lastExpression.count > 512 { lastExpression.removeAll(keepingCapacity:true); lastExpression[key] = packed }
        try start()
        let route = midiRoutes[instrument]
        if route.external { route.output?.send(status:message.status | route.channel,data1:message.data1,data2:message.data2) }
        else { rack.sendMIDIMessage(track:track,status:message.status,data1:message.data1,data2:message.data2) }
        switch (message.status,message.data1) {
        case (0xE0,_):
            let wheel = Int(message.data1) | Int(message.data2) << 7
            st_engine_pitch_bend(engine,Int32(track),Float(wheel-8192)/8192*2)
        case (0xB0,1): st_engine_mod_wheel(engine,Int32(track),Float(seven)/127)
        case (0xB0,64):
            if seven >= 64 { midiSustain.setPedal(source:Self.appNoteSource,channel:1,down:true) }
            else { for held in midiSustain.setPedal(source:Self.appNoteSource,channel:1,down:false) { st_engine_note_off(engine,Int32(held.track),Int32(held.note)) } }
        default: break
        }
        return ["sent":true,"track":track,"instrument":instrument,"status":Int(message.status),"data1":Int(message.data1),"data2":Int(message.data2)]
    }

    @MainActor func receiveMIDIDirect(_ status:UInt8,_ data1:UInt8,_ data2:UInt8,source:Int32) {
        guard !closed, !exporting, routingFailure == nil, unavailableProject == nil else { return }
        // System real-time bytes (clock 0xF8, active sensing 0xFE, ...) arrive dozens of times a second, often
        // echoed back by a device SuperTracker clocks. Only Start/Continue/Stop mean anything here, and not while
        // SuperTracker sends clock itself: an echoed Start would restart its own transport.
        if status >= 0xF8 && (status < 0xFA || status > 0xFC || midiClock.isActive) { return }
        let channel = Int(status & 0x0f)+1
        let kind = status & 0xf0
        let noteKey = "\(source):\(channel):\(data1)"
        // Releases belong to the original input even after the selected
        // instrument changes to one that rejects new notes from that input.
        if kind == 0xB0 && data1 == 64 && data2 < 64 {
            for held in midiSustain.setPedal(source:source,channel:channel,down:false) {
                st_engine_note_off(engine,Int32(held.track),Int32(held.note))
            }
            return
        }
        if kind == 0x80 || (kind == 0x90 && data2 == 0) {
            // A release this input never started here (its note-on was filtered, frozen,
            // or already released by a route change) owns nothing: dropping it keeps it
            // from cutting the same pitch held on the selected track by another device.
            guard let previous = midiNoteDestinations.removeValue(forKey:noteKey) else { return }
            heldNotes.remove(Int(data1))
            let deferred = midiSustain.deferNoteOff(source:source,channel:channel,track:previous.track,note:Int(data1))
            if !captureMIDIInput(key:noteKey,on:false,track:previous.track,instrument:previous.instrument,note:Int(data1),
                velocity:Int(data2),channel:channel,source:source,applyNote:!deferred) {
                if !deferred { st_engine_note_off(engine,Int32(clamping:previous.track),Int32(data1)) }
                emit("midi.note",["on":false,"note":Int(data1),"velocity":Int(data2),"channel":channel,"source":source,"track":previous.track,"instrument":previous.instrument])
            }
            return
        }
        if midiInputChannel != 0 && midiInputChannel != channel { return }
        let instruments = liveInstruments
        let tracks = liveTracks
        var destination = (track:midiTrack,instrument:midiInstrument)
        let routeMatch = instruments.enumerated().first { index,item in
            guard item["type"] as? String == "midi", let input = item["midiInput"] else { return false }
            let identifier = String(describing:input)
            let inputChannel = PayloadValue.integer(item,"midiInputChannel",0)
            return identifier == String(source) && (inputChannel == 0 || inputChannel == channel)
        }
        if let match = routeMatch {
            destination.instrument = match.offset
            destination.track = tracks.firstIndex { PayloadValue.integer($0,"instrument",0) == match.offset } ?? midiTrack
        } else if instruments.indices.contains(midiInstrument) {
            let item = instruments[midiInstrument]
            if let input = item["midiInput"], !["all","0",String(source)].contains(String(describing:input)) { return }
            let requestedChannel = PayloadValue.integer(item,"midiInputChannel",0)
            if requestedChannel != 0 && requestedChannel != channel { return }
        }
        if kind == 0x90 {
            // Releases returned above; only note-ons reach this branch. Match the keyboard
            // audition guard after resolving the real MIDI destination.
            if tracks.indices.contains(destination.track), tracks[destination.track][AudioFreezeTrack.recordKey] != nil {
                emit("audio.error",["error":AudioFreezeTrack.auditionMessage(tracks[destination.track],index:destination.track)])
                return
            }
            midiNoteDestinations[noteKey] = destination
            heldNotes.insert(Int(data1))
            midiSustain.retrigger(source:source,channel:channel,track:destination.track,note:Int(data1))
            if !captureMIDIInput(key:noteKey,on:true,track:destination.track,instrument:destination.instrument,note:Int(data1),
                velocity:Int(data2),channel:channel,source:source,applyNote:true) {
                st_engine_note_on(engine,Int32(destination.track),Int32(destination.instrument),Int32(data1),Int32(data2))
                let state = st_engine_status(engine)
                emit("midi.note",["on":true,"note":Int(data1),"velocity":Int(data2),"channel":channel,"source":source,
                    "track":destination.track,"instrument":destination.instrument,"pattern":Int(state.pattern),"row":Int(state.row),"position":state.position])
            }
        } else if kind == 0xB0 && data1 == 64 {
            midiSustain.setPedal(source:source,channel:channel,down:true)
        } else if kind == 0xB0 && (data1 == 120 || data1 == 123) { rack.notifyStateCaptureMutation(); st_engine_panic(engine); panicExternal() }
        else if status == 0xFA || status == 0xFB { rack.notifyStateCaptureMutation(); st_engine_play(engine,1) }
        else if status == 0xFC { rack.notifyStateCaptureMutation(); st_engine_stop(engine); panicExternal() }
        else {
            if kind == 0xB0 || kind == 0xC0 || kind == 0xD0 || kind == 0xE0 || kind == 0xA0 {
                let route = midiRoutes[max(0,min(midiRoutes.count-1,destination.instrument))]
                if route.external { route.output?.send(status:kind | route.channel,data1:data1,data2:data2) }
                else { rack.sendMIDIMessage(track:destination.track,status:kind,data1:data1,data2:data2) }
                if kind == 0xE0 {
                    let wheel = Int(data1) | Int(data2)<<7
                    st_engine_pitch_bend(engine,Int32(destination.track),Float(wheel-8192)/8192*2)
                }
            }
            // The interface uses control changes only (MIDI learn, CC-mapped controls). Pitch bend and
            // (poly) aftertouch stream at hundreds of messages a second and were each a discarded bridge call.
            if kind == 0xB0 { emit("midi.message",["status":Int(status),"data1":Int(data1),"data2":Int(data2),"source":source]) }
        }
    }

    /// Runs inside the rack's quiescent routing transaction after reordered tracks moved their
    /// instrument plug-ins (the rack already released their notes). Held keys follow their
    /// track to its new channel, so later releases cannot reach the plug-in now on the old channel.
    func forgetMovedInstrumentVoices(_ moves:[Int:Int?]) {
        for channel in moves.keys { _ = st_engine_forget_external_voices(engine,Int32(channel)) }
        midiSustain.remapTracks(moves)
        for (key,destination) in midiNoteDestinations {
            guard let moved = moves[destination.track] else { continue }
            if let next = moved { midiNoteDestinations[key] = (next,destination.instrument) } else { midiNoteDestinations.removeValue(forKey:key) }
        }
    }

    /// Input-side MIDI route: enablement, requested and connected sources and channel filter.
    func midiInputRoute(_ settings:[String:Any]) -> String {
        let requested = (settings["requestedInputIDs"] as? [Any])?.map { String(describing:$0) }.sorted()
        let connected = (settings["inputIDs"] as? [Any] ?? []).map { String(describing:$0) }.sorted()
        return "\(settings["enabled"] as? Bool ?? true)|\(requested?.joined(separator:",") ?? "*")|\(connected.joined(separator:","))|\(settings["inputChannel"] ?? 0)"
    }

    func releaseInputNotes() {
        cancelMIDICaptures(reason:"MIDI input route changed")
        for (key,destination) in midiNoteDestinations {
            if let part = key.split(separator:":").last, let note = Int32(part) { st_engine_note_off(engine,Int32(clamping:destination.track),note) }
        }
        for held in midiSustain.releaseAll() { st_engine_note_off(engine,Int32(held.track),Int32(held.note)) }
        midiNoteDestinations.removeAll(); midiExpandedNotes.removeAll(); heldNotes.removeAll()
    }

    func noteDestination(_ payload:[String:Any]) throws -> (channel:Int,instrument:Int) {
        let last = ordinaryInstruments-1
        return (lastTrack(PayloadValue.integer(payload,"track",midiTrack)),max(0,min(last,PayloadValue.integer(payload,"instrument",min(last,midiInstrument)))))
    }
}

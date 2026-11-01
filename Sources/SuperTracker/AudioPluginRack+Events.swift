import AppKit
import AudioToolbox
import AVFoundation
import CoreAudioKit
import CoreFoundation
import Darwin

/// MIDI, parameter automation and host transport for hosted units (render-thread safe).
extension AudioPluginRack {
    func sendMIDI(track: Int, note: Int, velocity: Int, on: Bool, sampleOffset: Int = 0, sampleTime: Int64? = nil, sequenced: Bool = false) {
        sendMIDIMessage(track: track, status: on ? 0x90 : 0x80,
            data1: UInt8(clamping: max(0, min(127, note))), data2: UInt8(clamping: max(0, min(127, velocity))), sampleOffset: sampleOffset, sampleTime: sampleTime, sequenced: sequenced)
    }

    /// Forwards pitch bend, modulation, sustain, aftertouch and program changes in
    /// addition to notes. A tracker track addresses channel 1 within its AU instrument.
    func sendMIDIMessage(track: Int, status: UInt8, data1: UInt8, data2: UInt8, sampleOffset: Int = 0, sampleTime: Int64? = nil, sequenced: Bool = false) {
        endpointStore.beginRead()
        defer { endpointStore.endRead() }
        guard (0..<trackCount).contains(track) else { return }
        let type = status & 0xF0
        guard status >= 0x80, status != 0xF0, status != 0xF7 else { return }
        let length: Int
        if status >= 0xF8 || status == 0xF6 { length = 1 }
        else if type == 0xC0 || type == 0xD0 || status == 0xF1 || status == 0xF3 { length = 2 }
        else { length = 3 }
        endpointStore.withValue(at: track * Self.endpointStride) { endpoint in
            guard let midi = endpoint.midi else { return }
            var bytes: (UInt8, UInt8, UInt8) = (status < 0xF0 ? type : status, data1 & 0x7F, data2 & 0x7F)
            withUnsafePointer(to: &bytes) { pointer in
                pointer.withMemoryRebound(to: UInt8.self, capacity: 3) {
                    let time = sampleTime.map { $0 + atomicLoad(endpoint.clockOffset.frames) } ?? (AUEventSampleTimeImmediate + AUEventSampleTime(max(0, sampleOffset)))
                    midi(time, sequenced ? endpoint.sequencedCable : 0, length, $0)
                }
            }
        }
    }

    func allNotesOff(track: Int? = nil, sampleTime: Int64? = nil) {
        endpointStore.beginRead()
        defer { endpointStore.endRead() }
        let lower = track ?? 0, upper = track.map { $0 + 1 } ?? trackCount
        guard lower >= 0, upper <= trackCount, lower < upper else { return }
        for index in lower..<upper {
            endpointStore.withValue(at: index * Self.endpointStride) { endpoint in
                guard let midi = endpoint.midi else { return }
                var bytes: (UInt8, UInt8, UInt8) = (0xB0, 123, 0)
                withUnsafePointer(to: &bytes) { pointer in
                    pointer.withMemoryRebound(to: UInt8.self, capacity: 3) {
                        midi(sampleTime.map { $0 + atomicLoad(endpoint.clockOffset.frames) } ?? AUEventSampleTimeImmediate, 0, 3, $0)
                    }
                }
            }
        }
    }

    /// Values are normalized [0, 1], matching automation lane storage and the JS API.
    func automate(track: Int, slot: Int, address: UInt64, value: Float, sampleOffset: Int = 0, kind: String = "effect", sampleTime: Int64? = nil) {
        endpointStore.beginRead()
        defer { endpointStore.endRead() }
        guard (0..<stripCapacity).contains(track), (0..<maximumEffects).contains(slot), value.isFinite else { return }
        var time = sampleTime ?? (AUEventSampleTimeImmediate + AUEventSampleTime(max(0, sampleOffset)))
        if sampleTime != nil, kind == "effect" {
            let routed = routingRenderStore.withValue(at:0) { state in
                let index = Self.offsetIndex(track:track,slot:slot)
                if state.pluginOffsets.indices.contains(index) { time += state.pluginOffsets[index] }
            }
            if !routed, track < trackCount { time += atomicLoad(automationOffsets.advanced(by: Self.offsetIndex(track: track, slot: slot))) }
        }
        endpointStore.withValue(at: Self.endpointIndex(track: track, kind: kind, slot: slot)) { endpoint in
            guard let range = endpoint.ranges[address] else { return }
            if sampleTime != nil { time += atomicLoad(endpoint.clockOffset.frames) }
            let raw = range.0 + max(0, min(1, value)) * (range.1 - range.0)
            endpoint.parameter(time, 0, address, raw)
        }
    }

    /// `map` is the event's STEvent.hostMap: the destination numbering it was emitted with.
    func automateHost(destination:Int,address:UInt64,value:Double,sampleTime:Int64,map:UInt64 = 0) {
        endpointStore.beginRead()
        defer { endpointStore.endRead() }
        let routed = routingRenderStore.withValue(at:0) { $0.automate(destination:destination,address:address,value:value,sampleTime:sampleTime,map:map) }
        // Flat step destinations belong to unrouted projects, whose events carry map 0.
        if !routed, map == 0 { flatStepRenderStore.withValue(at:0) { $0.automate(destination:destination,address:address,value:value,sampleTime:sampleTime) } }
    }

    func setTransport(bpm: Double, playing: Bool, recording: Bool = false, samplePosition: Double, beatPosition: Double) {
        endpointStore.beginRead()
        defer { endpointStore.endRead() }
        atomicStore(Int64(bitPattern: max(1, bpm).bitPattern), at: transport)
        atomicStore(Int64(bitPattern: samplePosition.bitPattern), at: transport.advanced(by: 1))
        atomicStore(Int64(bitPattern: beatPosition.bitPattern), at: transport.advanced(by: 2))
        var flags:AUHostTransportStateFlags = []
        if playing { flags.insert(.moving) }
        if recording { flags.insert(.recording) }
        atomicStore(Int64(flags.rawValue), at: transport.advanced(by: 3))
    }

    func setTimeSignature(numerator: Int, denominator: Int) {
        endpointStore.beginRead()
        defer { endpointStore.endRead() }
        atomicStore(Int64(max(1, min(32, numerator))), at: transport.advanced(by: 4))
        let supported = [1, 2, 4, 8, 16, 32]
        atomicStore(Int64(supported.contains(denominator) ? denominator : 4), at: transport.advanced(by: 5))
    }
}

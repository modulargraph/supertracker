import AppKit
import AudioToolbox
import AVFoundation
import CoreAudioKit
import CoreFoundation
import Darwin

/// Track mix values, mix automation and the built-in send effects.
extension AudioPluginRack {
    /// The native sampler already applies its own track gain and pan. This controls only
    /// the Audio Unit instrument so both kinds of instrument follow the same track fader.
    func setTrackMix(track: Int, gain: Float, pan: Float, muted: Bool, baseGain: Float? = nil,
                     instrumentVolume: Float = 1, masterVolume: Float = 1,
                     instrumentPan: Float = 0, trackVolume: Float? = nil) {
        endpointStore.beginRead()
        defer { endpointStore.endRead() }
        guard instrumentMixers.indices.contains(track) else { return }
        let base = baseGain ?? (instrumentVolume * masterVolume)
        let rawTrack = trackVolume ?? (base > 0 ? gain / base : trackValue(track, 0))
        // Preserve compatibility for callers that provide only the historical baseGain.
        let master = instrumentVolume > 0 ? base / instrumentVolume : masterVolume
        storeTrackValue(track, 0, max(0, min(2, rawTrack)))
        atomicStore(muted ? 1 : 0, at: trackControl.advanced(by: track * 6 + 1))
        storeTrackValue(track, 2, max(0, min(2, instrumentVolume)))
        storeTrackValue(track, 3, max(0, min(2, master)))
        storeTrackValue(track, 4, max(-1, min(1, pan)))
        storeTrackValue(track, 5, max(-1, min(1, instrumentPan)))
        applyTrackMix(track)
    }

    /// Audio-thread scheduling. The zero-latency processors apply each event to its
    /// exact sample; native voices have already received matching automation in C++.
    func automateTrack(track: Int, target: Int, value: Float, sampleOffset: Int = 0, sampleTime: Int64? = nil) {
        endpointStore.beginRead()
        defer { endpointStore.endRead() }
        var routed = false
        if let sampleTime {
            routingRenderStore.withValue(at:0) { routed = $0.automateTrack(track:track,target:target,value:value,sampleTime:sampleTime) }
        }
        if routed { return }
        // Native tracks already apply these events in the original DSP. Their AU
        // mix nodes are disconnected and must not accumulate unconsumed events.
        guard (0..<realizedStrips).contains(track), value.isFinite, hasInstrument(track: track) else { return }
        let time = sampleTime ?? (AUEventSampleTimeImmediate + AUEventSampleTime(max(0, sampleOffset)))
        switch target {
        case 0, 1, 23, 24:
            let offset = sampleTime == nil ? 0 : atomicLoad(automationOffsets.advanced(by: track * Self.offsetStride))
            instrumentAutomation[track].schedule(target: target, value: value, sampleTime: time + offset)
        case 4, 5:
            let offset = sampleTime == nil ? 0 : atomicLoad(automationOffsets.advanced(by: trackCount * Self.offsetStride))
            if target == 4 { delayAutomation[track].schedule(target: target, value: value, sampleTime: time + offset) }
            else { reverbAutomation[track].schedule(target: target, value: value, sampleTime: time + offset) }
        default: break
        }
    }

    func trackValue(_ track: Int, _ index: Int) -> Float {
        Float(Double(bitPattern: UInt64(bitPattern: atomicLoad(trackControl.advanced(by: track * 6 + index)))))
    }

    func storeTrackValue(_ track: Int, _ index: Int, _ value: Float) {
        atomicStore(Int64(bitPattern: Double(value).bitPattern), at: trackControl.advanced(by: track * 6 + index))
    }

    func applyTrackMix(_ track: Int) {
        let muted = atomicLoad(trackControl.advanced(by: track * 6 + 1)) != 0
        instrumentAutomation[track].setDefaults(trackVolume: standardMixing ? 1 : trackValue(track, 0), instrumentVolume: trackValue(track, 2),
            masterVolume: standardMixing ? 1 : trackValue(track, 3), trackPan: standardMixing ? 0 : trackValue(track, 4), instrumentPan: trackValue(track, 5), muted: standardMixing ? false : muted)
    }

    /// Native voices use the custom DSP returns; AU instruments have equivalent sends
    /// on the AVAudioEngine graph, so their audio does not cross into a render callback.
    func setTrackSends(track: Int, delay: Float, reverb: Float) {
        endpointStore.beginRead()
        defer { endpointStore.endRead() }
        guard (0..<realizedStrips).contains(track) else { return }
        delayAutomation[track].setGain(max(0, min(1, delay)))
        reverbAutomation[track].setGain(max(0, min(1, reverb)))
    }

    func configureSends(delayTime: Double, feedback: Float, reverbDecay: Float,
                        delayMix: Float = 0.3, reverbMix: Float = 0.25, damping: Float = 0.4, delayTone: Float? = nil) {
        endpointStore.beginRead()
        defer { endpointStore.endRead() }
        delayReturn.delayTime = max(0.001, min(2, delayTime))
        delayReturn.feedback = max(0, min(95, feedback * 100))
        // Explicit delay tone: 200 Hz ... 20 kHz (log); older projects keep the damping-derived cutoff.
        delayReturn.lowPassCutoff = delayTone.map { 200 * pow(100, max(0, min(1, $0))) } ?? 20_000 * pow(0.04, max(0, min(1, damping)))
        delayInput.outputVolume = max(0, min(1, delayMix))
        reverbInput.outputVolume = max(0, min(1, reverbMix))
        // The Apple reverb offers room presets rather than a continuous decay control.
        // Match the custom return's decay range with increasing acoustic room sizes.
        let presets: [AVAudioUnitReverbPreset] = [.smallRoom, .mediumRoom, .largeRoom, .mediumHall, .largeHall, .cathedral]
        let room = max(0, min(5, Int(reverbDecay * 5.999)))
        if room != reverbRoom { reverbRoom = room; reverbReturn.loadFactoryPreset(presets[room]) }
    }

    func hasInstrument(track: Int) -> Bool {
        guard (0..<trackCount).contains(track) else { return false }
        return endpointStore.hasValue(at: track * Self.endpointStride)
    }
}

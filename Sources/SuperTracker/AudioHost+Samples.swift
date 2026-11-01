import Foundation
import AppKit
import AVFoundation
import AudioToolbox
import CoreAudio
import TrackerEngine

/// Instrument samples, multisample zones and timeline clip assets in the native engine.
extension AudioHost {
    func loadSample(_ payload: [String:Any]) throws -> [String:Any] {
        guard let path = payload["path"] as? String else { throw AudioSampleError.invalid("No sample path was provided.") }
        let sample = try AudioSamples.decode(path)
        try installSample(sample,instrument:PayloadValue.integer(payload,"instrument",0)); return sample.info()
    }

    /// Decoded built-in samples (BuiltinSamples), kept for the session.
    func builtinSample(_ path:String) throws -> DecodedSample {
        if let cached = builtinSamples[path] { return cached }
        let decoded = try AudioSamples.decode(path)
        builtinSamples[path] = decoded
        return decoded
    }

    func installSample(_ sample: DecodedSample,instrument: Int) throws {
        guard (0..<instrumentCapacity).contains(instrument) else { throw AudioSampleError.invalid("Instrument index is outside this project's instruments.") }
        let result = sample.left.withUnsafeBufferPointer { left in sample.right.withUnsafeBufferPointer { right in
            st_engine_set_sample(engine,Int32(instrument),left.baseAddress,right.baseAddress,UInt64(sample.left.count),sample.sampleRate)
        }}
        guard result != 0 else { throw AudioSampleError.invalid("The engine could not load this sample.") }
        _ = st_engine_set_sample_cycle(engine,Int32(instrument),Int32(sample.cycleFrames ?? 0))
        samples[instrument] = sample
    }

    func scheduleZones(_ zones:[[String:Any]],instrument:Int) {
        let encoded = (try? JSONSerialization.data(withJSONObject:zones,options:.sortedKeys)) ?? Data()
        let signature = encoded.base64EncodedString()
        guard zoneSignatures[instrument] != signature else { return }
        let hadZones = zoneSignatures[instrument] != nil
        zoneSignatures[instrument] = signature; zoneTasks[instrument]?.cancel()
        if zones.isEmpty { if hadZones { st_engine_clear_zones(engine,Int32(instrument)); st_engine_commit_update(engine) }; return }
        zoneTasks[instrument] = Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let decoded = try await Task.detached(priority:.userInitiated) { () -> [(DecodedSample,[String:Any])] in
                    try zones.prefix(128).map { zone in
                        guard let path = zone["path"] as? String ?? zone["samplePath"] as? String, !path.isEmpty else { throw AudioSampleError.invalid("A multisample zone has no sample path.") }
                        return (try AudioSamples.decode(path),zone)
                    }
                }.value
                guard !Task.isCancelled, self.zoneSignatures[instrument] == signature else { return }
                st_engine_begin_update(self.engine); st_engine_clear_zones(self.engine,Int32(instrument))
                for (zoneIndex,entry) in decoded.enumerated() {
                    let (sample,zone)=entry
                    let accepted = sample.left.withUnsafeBufferPointer { left in sample.right.withUnsafeBufferPointer { right in
                        st_engine_add_sample_zone(self.engine,Int32(instrument),left.baseAddress,right.baseAddress,UInt64(sample.left.count),sample.sampleRate,
                            Int32(clamping:PayloadValue.integer(zone,"keyLow",0)),Int32(clamping:PayloadValue.integer(zone,"keyHigh",127)),Int32(clamping:PayloadValue.integer(zone,"velocityLow",PayloadValue.integer(zone,"velLow",0))),Int32(clamping:PayloadValue.integer(zone,"velocityHigh",PayloadValue.integer(zone,"velHigh",127))),Int32(clamping:PayloadValue.integer(zone,"rootNote",PayloadValue.integer(zone,"root",60))),Float(PayloadValue.number(zone,"tune",0)),Float(PayloadValue.number(zone,"volume",1)))
                    }}
                    guard accepted != 0 else { throw AudioSampleError.invalid("The engine rejected a multisample zone.") }
                    if ["start","end","loopStart","loopEnd","pan"].contains(where: { zone[$0] != nil }) {
                        guard st_engine_set_sample_zone_region(self.engine,Int32(instrument),Int32(zoneIndex),
                            Float(PayloadValue.number(zone,"start",0)),Float(PayloadValue.number(zone,"end",1)),Float(PayloadValue.number(zone,"loopStart",0)),
                            Float(PayloadValue.number(zone,"loopEnd",1)),Float(PayloadValue.number(zone,"pan",0))) != 0 else {
                            throw AudioSampleError.invalid("A multisample zone has invalid playback bounds.")
                        }
                    }
                    if let looping=zone["loopEnabled"] as? Bool {
                        guard st_engine_set_sample_zone_loop_enabled(self.engine,Int32(instrument),Int32(zoneIndex),looping ? 1 : 0) != 0 else {
                            throw AudioSampleError.invalid("A multisample zone has an invalid loop mode.")
                        }
                    }
                }
                st_engine_commit_update(self.engine)
                self.emit("sample.zonesLoaded",["instrument":instrument,"count":decoded.count])
            } catch {
                guard !Task.isCancelled, self.zoneSignatures[instrument] == signature else { return }
                self.zoneSignatures.removeValue(forKey:instrument)
                st_engine_clear_zones(self.engine,Int32(instrument)); st_engine_commit_update(self.engine)
                self.emit("sample.error",["instrument":instrument,"error":"Multisample zones: "+error.localizedDescription])
            }
        }
    }

    @MainActor func syncClipAssets(_ clips:[[String:Any]]) throws {
        guard clips.count <= 256 else { throw AudioSampleError.invalid("A project supports at most 256 timeline audio clips.") }
        let paths = Set(clips.compactMap { $0["path"] as? String ?? $0["samplePath"] as? String }.filter { !$0.isEmpty })
        guard paths.count <= 256 else { throw AudioSampleError.invalid("A project supports at most 256 distinct audio clip files.") }
        for path in Array(clipAssetSlots.keys) where !paths.contains(path) {
            clipTasks[path]?.cancel(); clipTasks.removeValue(forKey:path)
            if let slot = clipAssetSlots.removeValue(forKey:path) { _ = st_engine_set_audio_asset(engine,Int32(slot),nil,nil,0,sampleRate) }
        }
        for path in paths.sorted() where clipAssetSlots[path] == nil {
            let used = Set(clipAssetSlots.values)
            guard let slot = (0..<256).first(where:{ !used.contains($0) }) else { continue }
            clipAssetSlots[path] = slot
            clipTasks[path] = Task { @MainActor [weak self] in
                guard let self else { return }
                do {
                    let sample = try await Task.detached(priority:.userInitiated) { try AudioSamples.decode(path) }.value
                    guard !Task.isCancelled, self.clipAssetSlots[path] == slot else { return }
                    let result = sample.left.withUnsafeBufferPointer { left in sample.right.withUnsafeBufferPointer { right in
                        st_engine_set_audio_asset(self.engine,Int32(slot),left.baseAddress,right.baseAddress,UInt64(sample.left.count),sample.sampleRate)
                    }}
                    guard result != 0 else {
                        st_engine_cancel_update(self.engine)
                        throw AudioSampleError.invalid("Cannot load audio clip into the engine.")
                    }
                    let info = sample.info(); self.clipMetadata[path] = info
                    self.emit("audio.clipLoaded",info)
                } catch {
                    guard !Task.isCancelled, self.clipAssetSlots[path] == slot else { return }
                    self.clipAssetSlots.removeValue(forKey:path)
                    _ = st_engine_set_audio_asset(self.engine,Int32(slot),nil,nil,0,self.sampleRate)
                    self.emit("audio.clipError",["path":path,"error":error.localizedDescription])
                }
            }
        }
    }

    func resolvedSampleInstrument(_ payload:[String:Any],fallback:Int=0) throws -> Int {
        let index=PayloadValue.integer(payload,"instrument",fallback)
        guard index == -1 || (0..<ProjectLayout.instrumentSanityLimit).contains(index) else { throw AudioSampleError.invalid("Choose an instrument.") }
        return index
    }
}

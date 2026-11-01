import AppKit
import AudioToolbox
import AVFoundation
import CoreAudioKit
import CoreFoundation
import Darwin

/// Plug-in delay compensation and mix automation status.
extension AudioPluginRack {
    /// VST3 kLatencyChanged: reactivate each requesting component while rendering is
    /// paused and drained (never on the render thread), then recompute compensation.
    /// Returns true when a restart ran; a busy render is retried on a later tick.
    @discardableResult func applyPendingLatencyRestarts() -> Bool {
        guard routingTransactionDepth == 0 else { return false }
        let pending = instances.values.compactMap { $0.node.auAudioUnit as? VST3AudioUnit }.filter(\.latencyRestartPending)
        guard !pending.isEmpty else { return false }
        let wasRunning = engine.isRunning
        if wasRunning { pauseEngine() }
        do { try awaitRenderQuiescence() } catch { if wasRunning { try? startEngine() }; return false }
        notifyStateCaptureMutation()
        for adapter in pending {
            do { try adapter.restartForLatencyChange() } catch { recordRecoveryError(error) }
        }
        if wasRunning { do { try startEngine() } catch { recordRecoveryError(error) } }
        if !refreshLatencyCompensation() { onLatencyPlanChanged?() }
        return true
    }

    /// AVAudioEngine does not align unequal mixer branches automatically. A deterministic
    /// reported-64-frame test AU yields dry/wet pulse offsets of 64 without these delays.
    /// Compensate algorithmic latency only; creative delay/reverb durations remain audible.
    /// True when refreshLatencyCompensation would rebuild the routed graph.
    var routingRewirePending: Bool {
        guard routingDefinition?.enabled == true, routingRebuildDeferral == 0, let definition = routingDefinition,
              let signature = try? makeRoutingSignature(routingProject,definition:definition) else { return false }
        return signature != routingSignature
    }

    @discardableResult func refreshLatencyCompensation() -> Bool {
        if routingDefinition?.enabled == true {
            if routingRebuildDeferral > 0 { routingRebuildPending = true; return false }
            guard let definition = routingDefinition,
                  let signature = try? makeRoutingSignature(routingProject,definition:definition),
                  signature != routingSignature else { return false }
            do { try configureRouting(routingProject,force:true); return true }
            catch { recordRecoveryError(error); return false }
        }
        func frames(_ unit:AUAudioUnit?)->Int {
            guard let seconds = unit?.latency, seconds.isFinite, seconds > 0 else { return 0 }
            return Int((min(60,seconds)*format.sampleRate).rounded())
        }
        let strips = realizedStrips
        var instruments = [Int](repeating:0,count:strips), effects = instruments
        var insertLatencies = [Int](repeating: 0, count: strips * maximumEffects)
        for track in 0..<strips {
            instruments[track] = sourceLatency(track:track,plugin:frames(instances[Target(track:track,kind:"instrument",slot:0)]?.node.auAudioUnit))
            for slot in 0..<maximumEffects {
                let latency = frames(instances[Target(track:track,kind:"effect",slot:slot)]?.node.auAudioUnit)
                insertLatencies[track * maximumEffects + slot] = latency
                effects[track] += latency
            }
        }
        let delayLatency = frames(delayReturn.auAudioUnit), reverbLatency = frames(reverbReturn.auAudioUnit)
        // Equal total latency can still have a different distribution between slots.
        // Parameter timestamps must follow the upstream latency of each individual insert.
        let plan = instruments + insertLatencies + [delayLatency,reverbLatency]
        guard plan != lastLatencyPlan else { return false }
        notifyStateCaptureMutation()
        lastLatencyPlan = plan
        let instrumentMaximum = instruments.max() ?? 0
        let dryMaximum = zip(instruments,effects).map(+).max() ?? 0
        let total = max(dryMaximum,instrumentMaximum + max(delayLatency,reverbLatency))
        atomicStore(Int64(instrumentMaximum), at: automationOffsets.advanced(by: trackCount * Self.offsetStride))
        compensationLatencyFrames = total
        latencyExceedsLimit = total > nativeReturnCompensation.maximumFrames
        for track in 0..<strips {
            atomicStore(Int64(instruments[track]), at: automationOffsets.advanced(by: track * Self.offsetStride))
            nativeCompensators[track].frames = instruments[track]
            trackCompensators[track].frames = total-instruments[track]-effects[track]
            sendCompensators[track].frames = instrumentMaximum-instruments[track]
            if let instrument = instances[Target(track:track,kind:"instrument",slot:0)]?.node.auAudioUnit {
                for index in 0..<instrument.outputBusses.count { instrument.outputBusses[index].contextPresentationLatency = Double(total-instruments[track])/format.sampleRate }
            }
            var elapsed = instruments[track]
            for slot in 0..<maximumEffects {
                atomicStore(Int64(elapsed), at: automationOffsets.advanced(by: Self.offsetIndex(track: track, slot: slot)))
                guard let effect = instances[Target(track:track,kind:"effect",slot:slot)]?.node.auAudioUnit else { continue }
                for index in 0..<effect.inputBusses.count { effect.inputBusses[index].contextPresentationLatency = Double(elapsed)/format.sampleRate }
                elapsed += frames(effect)
                for index in 0..<effect.outputBusses.count { effect.outputBusses[index].contextPresentationLatency = Double(max(0,total-elapsed))/format.sampleRate }
            }
        }
        delayReturnCompensation.frames = total-instrumentMaximum-delayLatency
        reverbReturnCompensation.frames = total-instrumentMaximum-reverbLatency
        nativeReturnCompensation.frames = total
        refreshFlatStepEndpoints()
        onLatencyPlanChanged?()
        return true
    }

    func latencyStatus()->[String:Any] {
        ["frames":compensationLatencyFrames,"milliseconds":Double(compensationLatencyFrames)*1000/format.sampleRate,
         "maximumFrames":nativeReturnCompensation.maximumFrames,"limited":latencyExceedsLimit]
    }

    func resetLatencyCompensation() {
        for delay in nativeCompensators + trackCompensators + sendCompensators { delay.reset() }
        delayReturnCompensation.reset(); reverbReturnCompensation.reset(); nativeReturnCompensation.reset()
        routingGraph.current?.resetDelays()
    }

    /// Called while stopped at a seek/manual-render epoch boundary. Each processor
    /// discards old scheduled events and starts from the latest live control defaults.
    func resetAutomation() {
        endpointStore.beginRead()
        defer { endpointStore.endRead() }
        // Routing/PDC refresh re-emits parameter values; live and queued MIDI is preserved.
        for track in 0..<realizedStrips {
            instrumentAutomation[track].reset()
            delayAutomation[track].reset()
            reverbAutomation[track].reset()
        }
        routingRenderStore.withValue(at:0) { $0.resetAutomation() }
    }

    /// A bounded queue preserves render deadlines under hostile automation traffic.
    /// Surface the cumulative loss count instead of silently hiding an overload.
    func automationStatus() -> [String: Any] {
        var dropped = 0
        for track in 0..<realizedStrips {
            dropped += instrumentAutomation[track].droppedEvents
            dropped += delayAutomation[track].droppedEvents
            dropped += reverbAutomation[track].droppedEvents
        }
        routingRenderStore.withValue(at:0) { dropped += $0.droppedEvents }
        return ["droppedEvents": dropped, "overloaded": dropped > 0]
    }
}

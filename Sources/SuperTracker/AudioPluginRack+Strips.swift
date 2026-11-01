import AppKit
import AudioToolbox
import AVFoundation
import CoreAudioKit
import CoreFoundation
import Darwin

/// Rebuilding a track strip, unit formats, host context and endpoint publication.
extension AudioPluginRack {
    func rebuild(_ track: Int) throws {
        if routingDefinition?.enabled == true {
            // A batch restore (e.g. a kit with sixteen Sounds and many inserts) installs
            // the routed graph once at its end instead of once per loaded unit.
            if routingRebuildDeferral > 0 { routingRebuildPending = true; return }
            try configureRouting(routingProject,force:true)
        }
        else { rebuildFlat(track) }
    }

    /// A unit leaving the strip is detached right after the graph resumes, so it must leave the
    /// routed graph now, also while a batch restore defers rebuilds: a deferred rebuild left its
    /// input wired, the detach then disconnected it in the running graph (AVAudioEngine raises on
    /// that with live inputs) and the strip stayed silent until the batch ended.
    func rebuildRemovingUnit(_ track: Int) throws {
        if routingDefinition?.enabled == true { try configureRouting(routingProject,force:true) }
        else { rebuildFlat(track) }
    }

    /// Runs `body` with routed-graph rebuilds coalesced into one final install.
    @MainActor func coalescingRoutingRebuilds(_ body: () async throws -> Void) async throws {
        routingRebuildDeferral += 1
        var failure: Error?
        do { try await body() } catch { failure = error }
        routingRebuildDeferral -= 1
        if routingRebuildDeferral == 0, routingRebuildPending {
            routingRebuildPending = false
            if routingDefinition?.enabled == true {
                do { try configureRouting(routingProject,force:true) }
                catch { if failure == nil { failure = error } else { recordRecoveryError(error) } }
            }
        }
        if let failure { throw failure }
    }

    func rebuildFlat(_ track: Int) {
        guard trackInputs.indices.contains(track) else { return }
        notifyStateCaptureMutation()
        let input = trackInputs[track], output = trackOutputs[track], instrument = instrumentMixers[track]
        engine.disconnectNodeOutput(input); engine.disconnectNodeOutput(output)
        engine.disconnectNodeOutput(instrument)
        engine.disconnectNodeOutput(instrumentAutomation[track].node)
        engine.disconnectNodeOutput(nativeCompensators[track].node)
        engine.disconnectNodeOutput(trackCompensators[track].node)
        engine.disconnectNodeOutput(sendCompensators[track].node)
        if nativeSources.indices.contains(track) {
            engine.disconnectNodeOutput(nativeSources[track])
            engine.connect(nativeSources[track],to:nativeCompensators[track].node,format:format)
            engine.connect(nativeCompensators[track].node, to: input, fromBus: 0, toBus: 0, format: format)
        }
        let target = Target(track: track, kind: "instrument", slot: 0)
        if let unit = instances[target]?.node {
            engine.disconnectNodeOutput(unit)
            engine.connect(unit, to: instrument,fromBus:0,toBus:0, format: unit.outputFormat(forBus: 0))
        }
        if let line=lineInputSources[track] {
            engine.disconnectNodeOutput(line)
            engine.connect(line,to:instrument,fromBus:0,toBus:1,format:line.outputFormat(forBus:0))
        }
        if instances[target] != nil || lineInputSources[track] != nil {
            engine.connect(instrument, to: instrumentAutomation[track].node, format: format)
            let connections = [AVAudioConnectionPoint(node: input, bus: 1),AVAudioConnectionPoint(node:sendCompensators[track].node,bus:0)]
            engine.connect(instrumentAutomation[track].node, to: connections, fromBus: 0, format: format)
            engine.connect(sendCompensators[track].node,to:[AVAudioConnectionPoint(node:delaySends[track],bus:0),AVAudioConnectionPoint(node:reverbSends[track],bus:0)],fromBus:0,format:format)
        }
        var previous: AVAudioNode = input
        for slot in MixerChannel.slotsInChainOrder {
            if let instance = instances[Target(track: track, kind: "effect", slot: slot)] {
                let effect = instance.node
                engine.disconnectNodeOutput(effect)
                engine.disconnectNodeOutput(instance.converter)
                let main = max(0,(instance.node.auAudioUnit as? VST3AudioUnit)?.mainInputBus ?? 0)
                engine.connect(previous,to:effect,fromBus:0,toBus:AVAudioNodeBus(main),format:effect.inputFormat(forBus:AVAudioNodeBus(main)))
                engine.connect(effect, to: instance.converter, format: effect.outputFormat(forBus: 0))
                previous = instance.converter
            }
        }
        engine.connect(previous,to:trackCompensators[track].node,format:format)
        engine.connect(trackCompensators[track].node,to:output,format:format)
        engine.connect(output, to: engine.mainMixerNode, format: format)
    }

    func configureFormat(_ unit: AUAudioUnit) throws {
        // Auxiliary sidechain and multi-output buses can have different formats and
        // must not be forced to the primary stereo layout when they are unconnected.
        let main = (unit as? VST3AudioUnit)?.mainInputBus ?? 0
        let inputs = main >= 0 && main < unit.inputBusses.count ? [unit.inputBusses[main]] : []
        let outputs = unit.outputBusses.count > 0 ? [unit.outputBusses[0]] : []
        for bus in inputs + outputs {
            do { try bus.setFormat(format) }
            catch {
                let mono = AVAudioFormat(standardFormatWithSampleRate: format.sampleRate, channels: 1)!
                do { try bus.setFormat(mono) }
                catch { throw RackError.invalid("This Audio Unit cannot process mono or stereo audio at \(Int(format.sampleRate)) Hz.") }
            }
        }
    }

    func configureContext(_ unit: AUAudioUnit) {
        let values = transport, sampleRate = format.sampleRate
        unit.musicalContextBlock = { tempo, numerator, denominator, beat, offset, downbeat in
            let bpm = Double(bitPattern: UInt64(bitPattern: atomicLoad(values)))
            let position = Double(bitPattern: UInt64(bitPattern: atomicLoad(values.advanced(by: 2))))
            let top = Double(atomicLoad(values.advanced(by: 4)))
            let bottom = Int(atomicLoad(values.advanced(by: 5)))
            let beatsPerBar = max(0.125, top * 4 / Double(max(1, bottom)))
            tempo?.pointee = bpm; numerator?.pointee = top; denominator?.pointee = bottom
            beat?.pointee = position
            offset?.pointee = Int((ceil(position) - position) * 60 / max(1, bpm) * sampleRate)
            downbeat?.pointee = floor(position / beatsPerBar) * beatsPerBar
            return true
        }
        unit.transportStateBlock = { flags, sample, start, end in
            flags?.pointee = AUHostTransportStateFlags(rawValue: UInt(atomicLoad(values.advanced(by: 3))))
            sample?.pointee = Double(bitPattern: UInt64(bitPattern: atomicLoad(values.advanced(by: 1))))
            start?.pointee = 0; end?.pointee = 0
            return true
        }
        (unit as? HostedPluginAdapter)?.captureRenderContext()
        (unit as? BuiltinEffectsAudioUnit)?.refreshHostContext()
    }

    func publish(_ endpoint: RenderEndpoint?, for target: Target) {
        let index = Self.endpointIndex(track: target.track, kind: target.kind, slot: target.slot)
        endpointStore.publish(endpoint, at: index)
        refreshFlatStepEndpoints()
    }
}

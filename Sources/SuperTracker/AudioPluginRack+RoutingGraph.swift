import AppKit
import AudioToolbox
import AVFoundation
import CoreAudioKit
import CoreFoundation
import Darwin

/// Routing graph inputs: signatures, defaults, combined sources, effects, latencies and render state.
extension AudioPluginRack {
    func makeRoutingSignature(_ data:[String:Any],definition:AudioRouting.Definition,using supplied:[Target:Instance]? = nil) throws -> String {
        let instances = supplied ?? self.instances
        guard definition.enabled else { return "legacy-absent" }
        var route = data["routing"] as? [String:Any] ?? [:]
        if let buses = route["buses"] as? [[String:Any]] {
            route["buses"] = buses.map { item in item.filter { !["volume","pan","name","mute","solo"].contains($0.key) } }
        }
        for key in ["sends","sidechains"] {
            if let values = route[key] as? [[String:Any]] { route[key] = values.map { $0.filter { $0.key != "gain" } } }
        }
        let owners:[[String:Any]] = definition.owners.map { ["id":$0.id,"kind":$0.kind.rawValue,"index":$0.trackIndex ?? -1] }
        let plugins:[[String:Any]] = instances.values.sorted { $0.instanceID < $1.instanceID }.map { item in
            let unit = item.node.auAudioUnit
            let buses:[[String:Any]] = (0..<unit.inputBusses.count).map { index in
                ["name":unit.inputBusses[index].name ?? "Input \(index + 1)","channels":Int(unit.inputBusses[index].format.channelCount)]
            }
            return ["id":item.instanceID,"owner":item.ownerID ?? "","track":item.target.track,"kind":item.target.kind,"slot":item.target.slot,
             "latency":unit.latency,"buses":buses]
        }
        let mapping = try AudioProject.routingAutomation(data)
        let destinations:[[String:Any]] = mapping.entries.map { entry in
            ["destination":entry.destination,"owner":entry.owner,"kind":String(describing:entry.kind),
             "route":entry.routeID ?? "","slot":entry.slot ?? -1,"pluginKind":entry.pluginKind?.rawValue ?? "",
             "instanceID":entry.instanceID ?? "","requiresExactIdentity":entry.requiresExactIdentity]
        }
        let senders = Self.builtinSenders(data,definition:definition)
        let value:[String:Any] = ["routing":route,"owners":owners,"plugins":plugins,"capture":routingCaptureOwners.sorted(),"destinations":destinations,
            "delaySenders":senders.delay.sorted(),"reverbSenders":senders.reverb.sorted()]
        return String(data:try JSONSerialization.data(withJSONObject:value,options:[.sortedKeys]),encoding:.utf8)!
    }

    func updateRoutingDefaults(_ prepared:AudioRoutingGraph.Prepared,definition:AudioRouting.Definition,project:[String:Any]) {
        prepared.updateSolo(definition)
        for owner in definition.owners {
            let legacySource = definition.mixModel == .legacy && (owner.kind == .track || owner.kind == .master)
            prepared.strips[owner.id]?.setMix(volume:legacySource ? 1 : Float(owner.volume),pan:legacySource ? 0 : Float(owner.pan),muted:legacySource ? false : owner.mute)
        }
        for route in definition.sends { prepared.sendEndpoints[route.id]?.setGain(route.enabled ? Float(route.gain) : 0) }
        for route in definition.sidechains { prepared.sidechainEndpoints[route.id]?.setGain(route.enabled ? Float(route.gain) : 0) }
        let tracks = ProjectLayout.channels(project)
        for owner in definition.owners {
            guard let index = owner.trackIndex, let channel = tracks[index] else { continue }
            prepared.builtinSendEndpoints[owner.id]?[4]?.setGain(Float(PayloadValue.number(channel,"delaySend",0)))
            prepared.builtinSendEndpoints[owner.id]?[5]?.setGain(Float(PayloadValue.number(channel,"reverbSend",0)))
        }
        prepared.returnEndpoints["delay"]?.setGain(Float(PayloadValue.number(project["delay"] as? [String:Any] ?? [:],"mix",0.3)))
        prepared.returnEndpoints["reverb"]?.setGain(Float(PayloadValue.number(project["reverb"] as? [String:Any] ?? [:],"mix",0.25)))
        // Constant fallback lanes from AudioRoutingAutomation restore removed
        // mix/send lanes at their musical timestamps. Keep queued PDC events intact.
    }

    func disconnectFlatMasterPaths() {
        for node in trackOutputs { engine.disconnectNodeOutput(node) }
        for node in [delayReturnCompensation.node,reverbReturnCompensation.node,nativeReturnCompensation.node] { engine.disconnectNodeOutput(node) }
    }

    func instrumentLatency(_ track:Int)->Int {
        Int((max(0,min(60,instances[Target(track:track,kind:"instrument",slot:0)]?.node.auAudioUnit.latency ?? 0))*format.sampleRate).rounded())
    }

    /// Connects every realized channel's native source, AU instrument and line input into
    /// its strip input, the routed graph's source for that channel.
    func wireCombinedSources(standard:Bool) {
        let instrumentLatencies = (0..<realizedStrips).map(instrumentLatency)
        let instrumentMaximum = instrumentLatencies.max() ?? 0
        atomicStore(Int64(instrumentMaximum),at:automationOffsets.advanced(by:trackCount*Self.offsetStride))
        for track in 0..<realizedStrips {
            wireCombinedSource(track,standard:standard,latency:instrumentLatencies[track],instrumentMaximum:instrumentMaximum)
        }
        if !standard {
            engine.connect(delayInput,to:delayReturn,format:format); engine.connect(reverbInput,to:reverbReturn,format:format)
        }
    }

    func wireCombinedSource(_ track:Int,standard:Bool,latency sourceLatency:Int,instrumentMaximum:Int) {
        let input = trackInputs[track], instrument = instrumentMixers[track]
        for node in [instrument,instrumentAutomation[track].node,nativeCompensators[track].node,sendCompensators[track].node] as [AVAudioNode] { engine.disconnectNodeOutput(node) }
        // A routed track's direct native source belongs to the routing graph (see hasDirectSource).
        if ownersByIndex[track] != nil, hasDirectSource(track) {
            nativeCompensators[track].frames = sourceLatency
            sendCompensators[track].frames = max(0,instrumentMaximum-sourceLatency)
            atomicStore(Int64(sourceLatency),at:automationOffsets.advanced(by:track*Self.offsetStride))
            return
        }
        if nativeSources.indices.contains(track) {
            engine.disconnectNodeOutput(nativeSources[track])
            engine.connect(nativeSources[track],to:nativeCompensators[track].node,format:format)
            engine.connect(nativeCompensators[track].node,to:input,fromBus:0,toBus:0,format:format)
        }
        let unit = instances[Target(track:track,kind:"instrument",slot:0)]?.node
        nativeCompensators[track].frames = sourceLatency
        sendCompensators[track].frames = max(0,instrumentMaximum-sourceLatency)
        atomicStore(Int64(sourceLatency),at:automationOffsets.advanced(by:track*Self.offsetStride))
        if let unit {
            engine.disconnectNodeOutput(unit)
            engine.connect(unit,to:instrument,fromBus:0,toBus:0,format:unit.outputFormat(forBus:0))
        }
        if let line=lineInputSources[track] {
            engine.disconnectNodeOutput(line)
            engine.connect(line,to:instrument,fromBus:0,toBus:1,format:line.outputFormat(forBus:0))
        }
        if unit != nil || lineInputSources[track] != nil {
            engine.connect(instrument,to:instrumentAutomation[track].node,format:format)
            if standard { engine.connect(instrumentAutomation[track].node,to:input,fromBus:0,toBus:1,format:format) }
            else {
                engine.connect(instrumentAutomation[track].node,to:[AVAudioConnectionPoint(node:input,bus:1),AVAudioConnectionPoint(node:sendCompensators[track].node,bus:0)],fromBus:0,format:format)
                engine.connect(sendCompensators[track].node,to:[AVAudioConnectionPoint(node:delaySends[track],bus:0),AVAudioConnectionPoint(node:reverbSends[track],bus:0)],fromBus:0,format:format)
            }
        }
    }

    func restoreFlatGraph() throws {
        for item in instances.values where item.target.kind == "effect" {
            let unit = item.node.auAudioUnit
            if let adapter = unit as? VST3AudioUnit {
                for bus in adapter.inputBusMetadata() where bus.isAuxiliary && (1...2).contains(bus.channels) {
                    try adapter.configureAuxiliaryInput(bus:bus.index,enabled:false)
                    unit.inputBusses[bus.index].isEnabled = false
                    item.node.auAudioUnit.inputBusses[bus.index].isEnabled = false
                }
            } else {
                for index in 1..<max(1,unit.inputBusses.count) { unit.inputBusses[index].isEnabled = false; item.node.auAudioUnit.inputBusses[index].isEnabled = false }
            }
        }
        for track in 0..<flatTracks { rebuildFlat(track) }
        engine.connect(delayInput,to:delayReturn,format:format)
        engine.connect(delayReturn,to:delayReturnCompensation.node,format:format)
        engine.connect(delayReturnCompensation.node,to:engine.mainMixerNode,format:format)
        engine.connect(reverbInput,to:reverbReturn,format:format)
        engine.connect(reverbReturn,to:reverbReturnCompensation.node,format:format)
        engine.connect(reverbReturnCompensation.node,to:engine.mainMixerNode,format:format)
        engine.connect(nativeReturnInput,to:nativeReturnCompensation.node,format:format)
        engine.connect(nativeReturnCompensation.node,to:engine.mainMixerNode,format:format)
        lastLatencyPlan = []; refreshLatencyCompensation()
    }

    func routingEffects(_ definition:AudioRouting.Definition, using supplied:[Target:Instance]? = nil) -> [AudioRoutingGraph.Effect] {
        (supplied ?? instances).values.compactMap { item in
            guard item.target.kind == "effect", let owner = item.ownerID, definition.owner(owner) != nil else { return nil }
            if let adapter = item.node.auAudioUnit as? VST3AudioUnit {
                return AudioRoutingGraph.Effect(owner:owner,slot:item.target.slot,instanceID:item.instanceID,node:item.node,
                    mainInputBus:adapter.mainInputBus,inputBuses:adapter.inputBusMetadata(),
                    configureAuxiliary:{ bus,enabled in
                        try adapter.configureAuxiliaryInput(bus:bus,enabled:enabled)
                        item.node.auAudioUnit.inputBusses[bus].isEnabled = enabled
                    })
            }
            return AudioRoutingGraph.Effect(owner:owner,slot:item.target.slot,instanceID:item.instanceID,node:item.node,
                chainMember:(item.node.auAudioUnit as? BuiltinEffectsAudioUnit)?.chainMember,configureAuxiliary:{ bus,enabled in
                item.node.auAudioUnit.inputBusses[bus].isEnabled = enabled
            })
        }
    }

    func routingSourceLatencies(_ definition:AudioRouting.Definition, using supplied:[Target:Instance]? = nil)->[String:Int] {
        let instances = supplied ?? self.instances
        var result:[String:Int] = [:]
        for owner in definition.owners {
            guard let track = owner.trackIndex else { continue }
            let seconds = instances[Target(track:track,kind:"instrument",slot:0)]?.node.auAudioUnit.latency ?? 0
            result[owner.id] = sourceLatency(track:track,plugin:seconds.isFinite ? Int((max(0,min(60,seconds))*format.sampleRate).rounded()) : 0)
        }
        return result
    }

    /// Track owners whose built-in delay/reverb send can be audible: a nonzero send level,
    /// a delay/reverb step FX or an automation lane for that send. Only they get a send
    /// route (a zero send adds nothing); a change of either set re-plans routing, which
    /// adds or removes that route live (AudioRoutingGraph.prepareIncremental).
    static func builtinSenders(_ data:[String:Any],definition:AudioRouting.Definition) -> (delay:Set<String>,reverb:Set<String>) {
        var delay = Set<Int>(), reverb = Set<Int>()
        for (index,track) in ProjectLayout.channels(data) {
            if PayloadValue.number(track,"delaySend",0) != 0 { delay.insert(index) }
            if PayloadValue.number(track,"reverbSend",0) != 0 { reverb.insert(index) }
        }
        for usage in sendFXUsage(data) { if usage.delay { delay.insert(usage.track) } else { reverb.insert(usage.track) } }
        for pattern in data["patterns"] as? [[String:Any]] ?? [] {
            for lane in pattern["automation"] as? [[String:Any]] ?? [] {
                let target = lane["target"] as? String ?? "volume"
                guard target == "delay" || target == "reverb" else { continue }
                let track = (lane["owner"] as? String).map { definition.owner($0)?.trackIndex ?? -1 } ?? PayloadValue.integer(lane,"track",0)
                if target == "delay" { delay.insert(track) } else { reverb.insert(track) }
            }
        }
        var owners:[Int:String] = [:]
        for owner in definition.owners { if let index = owner.trackIndex { owners[index] = owner.id } }
        return (Set(delay.compactMap { owners[$0] }),Set(reverb.compactMap { owners[$0] }))
    }

    /// Tracks with a delay or reverb step FX anywhere in the patterns (sorted, unique).
    static func sendFXUsage(_ data:[String:Any]) -> [(track:Int,delay:Bool)] {
        var found = Set<Int>()   // track * 2 + (reverb ? 1 : 0)
        for pattern in data["patterns"] as? [[String:Any]] ?? [] {
            for (track,rows) in (pattern["steps"] as? [[Any]] ?? []).enumerated() {
                for raw in rows {
                    guard let step = raw as? [String:Any] else { continue }
                    for key in ["fx1","fx2"] {
                        switch (step[key] as? [String:Any])?["type"] as? String {
                        case "delay": found.insert(track * 2)
                        case "reverb": found.insert(track * 2 + 1)
                        default: break
                        }
                    }
                }
            }
        }
        return found.sorted().map { ($0 / 2,$0 % 2 == 0) }
    }

    /// What the routing graph is assembled from: track sources, their instrument
    /// latencies, the delay/reverb returns or legacy wet paths, and the metronome.
    struct RoutingInputs {
        let sources:[String:AVAudioNode]
        let latencies:[String:Int]
        let returns:[AudioRoutingGraph.SystemReturn]
        let extra:[AudioRoutingGraph.Source]
    }

    func prepareRouting(_ data:[String:Any],definition:AudioRouting.Definition,using supplied:[Target:Instance]? = nil) throws -> AudioRoutingGraph.Prepared {
        let inputs = routingInputs(data,definition:definition,using:supplied)
        return try routingGraph.prepare(definition:definition,sources:inputs.sources,effects:routingEffects(definition,using:supplied),
            sourceLatencies:inputs.latencies,systemReturns:inputs.returns,extraMasterSources:inputs.extra,captureOwners:routingCaptureOwners)
    }

    /// A routed track with no AU instrument and no line input feeds its native source straight
    /// into its routing strip: no zero-delay compensator and no one-input combining mixer.
    func hasDirectSource(_ track:Int, using supplied:[Target:Instance]? = nil) -> Bool {
        nativeSources.indices.contains(track) && lineInputSources[track] == nil &&
            (supplied ?? instances)[Target(track:track,kind:"instrument",slot:0)] == nil
    }

    func routingInputs(_ data:[String:Any],definition:AudioRouting.Definition,using supplied:[Target:Instance]? = nil) -> RoutingInputs {
        let sources = Dictionary(uniqueKeysWithValues:definition.owners.compactMap { owner -> (String,AVAudioNode)? in
            guard let track = owner.trackIndex, trackInputs.indices.contains(track) else { return nil }
            return (owner.id,hasDirectSource(track,using:supplied) ? nativeSources[track] : trackInputs[track])
        })
        let latencies = routingSourceLatencies(definition,using:supplied)
        var returns:[AudioRoutingGraph.SystemReturn] = [], extra:[AudioRoutingGraph.Source] = []
        if definition.mixModel == .standard {
            let tracks = ProjectLayout.channels(data)
            func gains(_ key:String)->[String:Float] {
                Dictionary(uniqueKeysWithValues:definition.owners.compactMap { owner in
                    guard let index = owner.trackIndex, let channel = tracks[index] else { return nil }
                    return (owner.id,Float(PayloadValue.number(channel,key,0)))
                })
            }
            let delay = data["delay"] as? [String:Any] ?? [:], reverb = data["reverb"] as? [String:Any] ?? [:]
            let senders = Self.builtinSenders(data,definition:definition)
            returns = [AudioRoutingGraph.SystemReturn(id:"delay",processor:delayReturn,
                latencyFrames:Int((max(0,delayReturn.auAudioUnit.latency)*format.sampleRate).rounded()),gains:gains("delaySend"),returnGain:Float(PayloadValue.number(delay,"mix",0.3)),
                senders:senders.delay),
                AudioRoutingGraph.SystemReturn(id:"reverb",processor:reverbReturn,
                latencyFrames:Int((max(0,reverbReturn.auAudioUnit.latency)*format.sampleRate).rounded()),gains:gains("reverbSend"),returnGain:Float(PayloadValue.number(reverb,"mix",0.25)),
                senders:senders.reverb)]
            extra = [.init(id:"metronome",node:nativeReturnInput)]
        } else {
            let upstream = latencies.values.max() ?? 0
            extra = [.init(id:"native-returns",node:nativeReturnInput),
                .init(id:"legacy-delay",node:delayReturn,latencyFrames:upstream+Int((max(0,delayReturn.auAudioUnit.latency)*format.sampleRate).rounded())),
                .init(id:"legacy-reverb",node:reverbReturn,latencyFrames:upstream+Int((max(0,reverbReturn.auAudioUnit.latency)*format.sampleRate).rounded()))]
        }
        return RoutingInputs(sources:sources,latencies:latencies,returns:returns,extra:extra)
    }

    func makeRoutingRender(_ prepared:AudioRoutingGraph.Prepared,project:[String:Any],using supplied:[Target:Instance]? = nil) throws -> AudioRoutingRender {
        let instances = supplied ?? self.instances
        let mapping = try AudioProject.routingAutomation(project)
        var bindings:[AudioRoutingRender.Binding] = []
        for entry in mapping.entries {
            switch entry.kind {
            case .mix:
                if let control = prepared.mixControls[entry.owner] {
                    bindings.append(.mix(.init(control:control,offset:Int64(prepared.mixInputArrival[entry.owner] ?? 0))))
                } else { bindings.append(.unavailable) }
            case .send:
                if let route = entry.routeID, let control = prepared.sendControls[route] {
                    bindings.append(.send(.init(control:control,offset:Int64(prepared.sendInputArrival[route] ?? 0))))
                } else { bindings.append(.unavailable) }
            case .plugin:
                if let index = ownerIndices[entry.owner], let slot = entry.slot,
                   let item = instances[Target(track:index,kind:entry.pluginKind?.rawValue ?? "effect",slot:slot)],
                   !entry.requiresExactIdentity || item.instanceID == entry.instanceID {
                    bindings.append(.plugin(.init(unit:item.node.auAudioUnit,offset:prepared.pluginInputArrival[item.instanceID] ?? 0,clock:item.clockOffset)))
                } else { bindings.append(.unavailable) }
            }
        }
        var tracks = [AudioRoutingRender.MixEndpoint?](repeating:nil,count:trackCount)
        var delays = tracks, reverbs = tracks, offsets = [Int64](repeating:0,count:stripCapacity*Self.offsetStride)
        for (owner,index) in ownerIndices {
            if index < trackCount {
                if let control = prepared.mixControls[owner] { tracks[index] = .init(control:control,offset:Int64(prepared.mixInputArrival[owner] ?? 0)) }
                if let control = prepared.builtinSendControls[owner]?[4] { delays[index] = .init(control:control,offset:Int64(prepared.builtinSendInputArrival[owner]?[4] ?? 0)) }
                if let control = prepared.builtinSendControls[owner]?[5] { reverbs[index] = .init(control:control,offset:Int64(prepared.builtinSendInputArrival[owner]?[5] ?? 0)) }
            }
            for slot in 0..<maximumEffects {
                if let item = instances[Target(track:index,kind:"effect",slot:slot)] { offsets[Self.offsetIndex(track:index,slot:slot)] = Int64(prepared.pluginInputArrival[item.instanceID] ?? 0) }
            }
        }
        let controls = Array(prepared.mixControls.values)+Array(prepared.sendControls.values)+prepared.builtinSendControls.values.flatMap { $0.values }+prepared.returnEndpoints.values.map(\.automation)
        return AudioRoutingRender(standard:prepared.definition.mixModel == .standard,hostMap:mapping.hostMap,bindings:bindings,
            tracks:tracks,delaySends:delays,reverbSends:reverbs,pluginOffsets:offsets,controls:controls)
    }

    func publishRoutingRender(_ prepared:AudioRoutingGraph.Prepared,project:[String:Any]) throws {
        publishRoutingValue(try makeRoutingRender(prepared,project:project))
        routingRenderStore.collectRetired()
    }

    func publishRoutingValue(_ value:AudioRoutingRender?) {
        routingRenderStore.publish(value,at:0)
        // A new numbering drops events of the engine project numbered for the old
        // one. When that project is already the matching one (it was published
        // first), nothing else makes the engine re-send its current lane values.
        let map = value?.hostMap ?? 0
        if map != publishedHostMap { publishedHostMap = map; onHostAutomationMapChanged?() }
    }
}

extension BuiltinEffectsChain: AudioInsertChain {}

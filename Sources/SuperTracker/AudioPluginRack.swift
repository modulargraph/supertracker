import AppKit
import AudioToolbox
import AVFoundation
import CoreAudioKit
import CoreFoundation
import Darwin

/// Audio Unit hosting. Graph, discovery and editor operations belong to the main thread.
/// `sendMIDI`, `automate` and `setTransport` are also safe from the audio render thread.
/// Endpoint publication uses macOS's lock-free atomic primitives so rendering never waits
/// for a UI lock. Retired endpoints are reclaimed on the control thread only after
/// registered graph renders and endpoint readers have completed.
final class AudioPluginRack {
    enum RackError: LocalizedError {
        case invalid(String)
        var errorDescription: String? {
            switch self { case .invalid(let message): return message }
        }
    }

    struct Target: Hashable {
        let track: Int
        let kind: String
        let slot: Int
    }
    struct Component {
        let id: String
        let name: String
        let manufacturer: String
        let format: String
        let bundlePath: String?
        init(_ component: AVAudioUnitComponent) {
            let description = component.audioComponentDescription
            id = "\(description.componentType):\(description.componentSubType):\(description.componentManufacturer)"
            name = component.name; manufacturer = component.manufacturerName
            format = "AU"; bundlePath = nil
        }
        init(id:String,name:String,format:String = "AU") {
            self.id = id; self.name = name; manufacturer = format == "Builtin" ? "SuperTracker" : ""; self.format = format; bundlePath = nil
        }
        init(_ descriptor: VST3PluginDescriptor) {
            id = descriptor.id; name = descriptor.name; manufacturer = descriptor.manufacturer
            format = "VST3"; bundlePath = descriptor.bundlePath
        }
        init(_ descriptor: VST2PluginDescriptor) {
            id = descriptor.id; name = descriptor.name; manufacturer = descriptor.manufacturer
            format = "VST2"; bundlePath = descriptor.bundlePath
        }
    }
    final class RecoveryRecord {
        enum Phase { case loading, active, releasing, failedLoad }
        let token:String
        let identity:PluginRecoveryStore.Identity
        weak var node:AVAudioUnit?
        weak var unit:AUAudioUnit?
        var phase:Phase = .loading
        var vendorClosed = false
        var abandoned = false
        init(token:String,identity:PluginRecoveryStore.Identity) { self.token = token; self.identity = identity }
        var released:Bool { vendorClosed || (node == nil && unit == nil) }
    }
    struct LegacyMigration {
        let component:String,state:String,usedRawFallback:Bool
    }
    final class PreparedUnit {
        let component:Component
        let node:AVAudioUnit
        let nks:[String:Any]?
        let recovery:RecoveryRecord?
        let baseline:String
        let migration:LegacyMigration?
        var abandoned = false
        init(component:Component,node:AVAudioUnit,nks:[String:Any]?,recovery:RecoveryRecord?,baseline:String,migration:LegacyMigration? = nil) {
            self.component=component; self.node=node
            self.nks=nks; self.recovery=recovery; self.baseline=baseline
            self.migration=migration
        }
    }

    /// A unit's render sample time minus the main mixer's for the same quantum. An engine
    /// pause restarts the mixer's sample clock at zero while units keep device time, so a
    /// note stamped with the graph frame alone lands seconds in the unit's past; some
    /// units (Arturia's) then drop it while their keyboard still shows the key.
    final class RenderClockOffset {
        let frames = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
        init() { frames.initialize(to: 0) }
        deinit { frames.deinitialize(count: 1); frames.deallocate() }
    }

    final class Instance {
        var target: Target
        var ownerID: String?
        let instanceID: String
        let component: Component
        let node: AVAudioUnit
        let converter = AVAudioMixerNode()
        let renderMonitor = AudioPluginRenderMonitor()
        let clockOffset = RenderClockOffset()
        var renderObserverToken: Int?
        var nks: [String: Any]?
        var migration:LegacyMigration?
        let recovery:RecoveryRecord?
        init(target: Target, component: Component, node: AVAudioUnit,
             leaseStore: AudioEndpointStore<RenderEndpoint>, graphQuantum: UnsafeMutablePointer<Int64>? = nil,
             nks: [String: Any]? = nil, recovery:RecoveryRecord? = nil, instanceID:String = UUID().uuidString) {
            self.instanceID = instanceID
            self.recovery = recovery
            self.target = target; self.component = component; self.node = node; self.nks = nks
            let monitor = renderMonitor
            let reads = leaseStore.readScope
            let offset = clockOffset
            renderObserverToken = node.auAudioUnit.token(byAddingRenderObserver: { flags, time, frames, _ in
                if flags.contains(.unitRenderAction_PreRender) {
                    reads.beginRead()
                    let sample = time.pointee.mSampleTime
                    if let graphQuantum, sample.isFinite, abs(sample) < 1e15 {
                        atomicStore(Int64(sample) - atomicLoad(graphQuantum), at: offset.frames)
                    }
                }
                if flags.contains(.unitRenderAction_PostRenderError) {
                    monitor.recordFailure(flags:flags.rawValue,sampleTime:time.pointee.mSampleTime,frames:frames)
                }
                if flags.contains(.unitRenderAction_PostRender) { reads.endRead() }
            })
        }
        /// After detach and registered render quiescence, on the control thread.
        func removeRenderObserver() {
            if let token = renderObserverToken {
                node.auAudioUnit.removeRenderObserver(token)
                renderObserverToken = nil
            }
        }
    }
    final class RenderEndpoint {
        let unit: AUAudioUnit
        let midi: AUScheduleMIDIEventBlock?
        let parameter: AUScheduleParameterBlock
        let ranges: [UInt64: (Float, Float)]
        let clockOffset: RenderClockOffset
        /// Our VST3 adapter reads cable 1 as "sequenced" and clears kIsLive; every other
        /// unit keeps cable 0 for all notes.
        let sequencedCable: UInt8
        init(_ instance: Instance) {
            let unit = instance.node.auAudioUnit
            self.unit = unit
            clockOffset = instance.clockOffset
            sequencedCable = unit is VST3AudioUnit ? VST3AudioUnit.sequencedCable : 0
            midi = unit.scheduleMIDIEventBlock; parameter = unit.scheduleParameterBlock
            // A third-party tree may list one address twice; the first entry wins instead of trapping.
            ranges = Dictionary((unit.parameterTree?.allParameters ?? []).map {
                ($0.address, ($0.minValue, $0.maxValue))
            }, uniquingKeysWith: { first, _ in first })
        }
    }

    let engine: AVAudioEngine
    /// Channel index capacity (tracks; see st_channel_count). It sizes
    /// only small index tables; strip nodes exist just for realized channels.
    let trackCount: Int
    /// Routing group/return buses occupy trackCount..<trackCount+busCapacity; master follows.
    static let busCapacity = ProjectLayout.trackSanityLimit
    var masterIndex: Int { trackCount + Self.busCapacity }
    /// Channel strips with live nodes; grows on demand (ensureStrips), never shrinks.
    var realizedStrips: Int { trackInputs.count }
    /// Ordinary tracks wired as flat strips when routing is off (the project's track count).
    var flatTrackCount = ProjectLayout.minimumReturnBus
    var flatTracks: Int { min(flatTrackCount, trackInputs.count) }
    /// Control-thread notification after latency offsets or plug-in routing change.
    /// The host orders an automation reset and value refresh at its next pre-render.
    var onLatencyPlanChanged: (() -> Void)?
    /// The published routing table now numbers host destinations differently: the
    /// host asks the engine to re-send its lane values (st_engine_refresh_automation).
    var onHostAutomationMapChanged: (() -> Void)?
    /// AudioRoutingRender.hostMap of the table last published (0 = none).
    var publishedHostMap: UInt64 = 0
    /// Control-thread notification inside a quiescent routing transaction: track
    /// instrument plug-ins moved from old channel to new channel (nil: removed). The
    /// host must end the engine's external voices on the old channels without emitting
    /// releases; the rack sends each moved plug-in all-notes-off once the graph resumes.
    var onInstrumentsMoved: (([Int:Int?]) -> Void)?
    /// Nonzero while a batch restore coalesces routed-graph rebuilds; see coalescingRoutingRebuilds.
    var routingRebuildDeferral = 0
    var routingRebuildPending = false
    /// Control-thread lifecycle handoff. A stop or a pause restarts the main
    /// mixer's sample clock at zero (source nodes keep device time); its owning
    /// host must invalidate retained ranges before any resume, or the engine cache
    /// stays ahead of the new clock and the transport freezes until it catches up.
    var onEngineStopped: (() -> Void)?
    var beforeEngineStart: (() throws -> Void)?
    func stopEngine() { engine.stop(); onEngineStopped?() }
    /// Stops the graph for rewiring. With live inputs connected the hardware input chain stays
    /// initialized across a pause, and AVAudioEngine raises on disconnects in it; stop fully then.
    func pauseEngine() {
        enginePauses &+= 1
        if lineInputSources.isEmpty { engine.pause() }
        else if let refusal = catchingObjCException({ [engine] in engine.stop() }) {
            // Stopping re-validates every connection; a refusal must not unwind through Swift. Pausing still
            // halts rendering for the rewire.
            recordRecoveryError(RackError.invalid("The audio engine refused to stop for a routing change: \(refusal)"))
            engine.pause()
        }
        onEngineStopped?()
    }

    /// Removes a plug-in unit (and its converter) from the graph: both directions are disconnected first, since
    /// a detached node that is still some node's input destination leaves a dangling connection behind that
    /// fails the engine's next stop or reconfiguration.
    func detachUnit(_ item: Instance) {
        for node in [item.node, item.converter] where node.engine === engine {
            engine.disconnectNodeInput(node); engine.disconnectNodeOutput(node); engine.detach(node)
        }
    }
    func startEngine() throws { try beforeEngineStart?(); try engine.start() }
    /// Internal verification hook. Production leaves this nil; tests can suspend a
    /// real component load before instantiation without replacing the Audio Unit.
    var beforeInstantiation: (() async throws -> Void)?
    /// Original-fixture fault injection after reconnection/project publication.
    /// Production leaves this nil; it never runs on an audio callback.
    var beforeEffectOrderResume: (() throws -> Void)?
    var onRecoveryError:((String)->Void)?
    let recoveryStore:PluginRecoveryStore?
    let vst2Catalogue:VST2Catalogue
    let vst3Catalogue:VST3Catalogue
    var stateCaptureEpoch:UInt64 = 0
    var recoveryRecords:[String:RecoveryRecord] = [:]
    var recoveryWarnings:[String] = []
    var recoveryShuttingDown = false
    var recoveryShutdownStarted = false
    var recoveryShutdownFinished = false
    /// Channel slots the host addresses: a channel's own inserts and, on track channels, its instrument's chain.
    let maximumEffects = MixerChannel.channelSlotCount
    /// Outstanding state-capture leases; each lease cancels its captures once.
    var capturePreparationTokens = Set<UUID>()
    /// Targets with a published render endpoint (the endpoint index space is sized for capacity).
    // Endpoint and automation-offset layouts per channel: see MixerChannel.
    static let endpointStride = MixerChannel.endpointStride
    static let offsetStride = MixerChannel.offsetStride
    @inline(__always) static func endpointIndex(track: Int, kind: String, slot: Int) -> Int {
        MixerChannel.endpointIndex(channel: track, kind: kind, slot: slot)
    }
    @inline(__always) static func offsetIndex(track: Int, slot: Int) -> Int { MixerChannel.offsetIndex(track: track, slot: slot) }
    let stripCapacity: Int // channel capacity, routing buses, master
    var routingDefinition: AudioRouting.Definition?
    let routingGraph:AudioRoutingGraph
    var routingProject:[String:Any] = [:]
    var routingSignature = ""
    var routingTransactionDepth = 0
    var routingCaptureOwners = Set<String>()
    let routingMeters: [AudioMeters]
    /// Strip mix processors currently metering their output for routingMeters.
    var routingMeterNodes:[AudioMixAutomation] = []
    var ownerIndices: [String:Int] = [:]
    var ownersByIndex: [Int:String] = [:]
    let format: AVAudioFormat
    var nativeSources: [AVAudioNode] = []
    var trackInputs: [AVAudioMixerNode] = []
    var trackOutputs: [AVAudioMixerNode] = []
    var instrumentMixers: [AVAudioMixerNode] = []
    var delaySends: [AVAudioMixerNode] = []
    var reverbSends: [AVAudioMixerNode] = []
    var instrumentAutomation: [AudioMixAutomation] = []
    var delayAutomation: [AudioMixAutomation] = []
    var reverbAutomation: [AudioMixAutomation] = []
    var lineInputSources:[Int:AVAudioNode] = [:]
    let delayInput = AVAudioMixerNode()
    let reverbInput = AVAudioMixerNode()
    let delayReturn = AVAudioUnitDelay()
    let reverbReturn = AVAudioUnitReverb()
    let nativeReturnInput = AVAudioMixerNode()
    let delayReturnCompensation:AudioDelayCompensator
    let reverbReturnCompensation:AudioDelayCompensator
    let nativeReturnCompensation:AudioDelayCompensator
    var nativeCompensators:[AudioDelayCompensator] = []
    var trackCompensators:[AudioDelayCompensator] = []
    var sendCompensators:[AudioDelayCompensator] = []
    var lastLatencyPlan:[Int] = []
    var compensationLatencyFrames = 0
    /// Hardware played from a track and heard back through its line input arrives this many
    /// frames after the track's own output would (one IO buffer plus the interface's input
    /// latency). Counted as that track's source latency so the rest of the mix waits for it.
    var hardwareReturnFrames = 0
    func sourceLatency(track:Int, plugin:Int) -> Int { plugin + (lineInputSources[track] != nil ? hardwareReturnFrames : 0) }
    var latencyExceedsLimit = false
    /// Routing graph changes applied live (incremental) and by a stopped full rebuild.
    var incrementalRoutingChanges = 0, fullRoutingRebuilds = 0
    /// Built-in insert changes applied in place to insert-chain nodes (no rewiring, no pause).
    var insertChainUpdates = 0
    /// Why the last routing change rebuilt the graph instead of extending it (diagnostics).
    var lastFullRebuildReason: String?
    var reverbRoom = -1
    var instances: [Target: Instance] = [:]
    var generations: [Target: Int] = [:]
    // Baselines change only when state is applied. Reading a snapshot must not make a
    // stale UI copy appear newly authoritative over edits made in a vendor's editor.
    var lastRestoredStates: [Target: String] = [:]
    /// Encoded states most recently read from each live instance. A desired state
    /// equal to one of these is that instance's own output: restoring it again could
    /// only revert edits made since, so reconciliation leaves the instance alone.
    var lastCapturedStates: [ObjectIdentifier: CapturedState] = [:]
    struct CapturedState { weak var unit: AUAudioUnit?; let state: String }
    /// Identity-checked: a retired unit's address can be reused by a new one.
    func capturedState(_ unit: AUAudioUnit) -> String? {
        guard let entry = lastCapturedStates[ObjectIdentifier(unit)], entry.unit === unit else { return nil }
        return entry.state
    }
    /// Diagnostics: graph pauses issued by the rack, and VST3 captures taken
    /// while rendering continued (neither is consulted by production logic).
    var enginePauses: UInt64 = 0
    var concurrentStateCaptures: UInt64 = 0
    // Encoding stability is independent of reconciliation baselines. Every
    // capture still reads and serializes fresh AU state before comparison.
    let auStateReuse = AudioAUStateReuse()
    var editorWindows: [Target: NSWindow] = [:]
    /// Targets whose editor view is being created (requestViewController may complete later).
    var editorsOpening = Set<Target>()
    let endpointStore: AudioEndpointStore<RenderEndpoint>
    let routingRenderStore = AudioEndpointStore<AudioRoutingRender>(capacity:1)
    let flatStepRenderStore = AudioEndpointStore<AudioRoutingRender>(capacity:1)
    var flatStepDestinations:[AudioPluginStepFX.FlatDestination] = []
    struct FlatStepIdentity:Equatable {
        let destination:Int,track:Int,slot:Int,instanceID:String
        let unit:ObjectIdentifier?
        let offset:Int64
    }
    var flatStepSignature:[FlatStepIdentity] = []
    var retiredUnits: [Instance] = []
    let transport: UnsafeMutablePointer<Int64>
    /// Main-mixer sample time of the quantum being rendered (see RenderClockOffset).
    let graphQuantum: UnsafeMutablePointer<Int64>
    let trackControl: UnsafeMutablePointer<Int64>
    // Layout: see `offsetStride`. Final entry is the common send time.
    let automationOffsets: UnsafeMutablePointer<Int64>

    /// trackCount is the channel index capacity; realizedStrips (default: the legacy 16
    /// tracks plus 64 Sounds, capped by capacity) get nodes now, the rest on demand.
    init(engine: AVAudioEngine, trackCount: Int = 16, realizedStrips: Int? = nil, sampleRate: Double? = nil, recoveryStore:PluginRecoveryStore? = nil, vst3Catalogue:VST3Catalogue = .shared, vst2Catalogue:VST2Catalogue = .shared) throws {
        self.recoveryStore = recoveryStore
        self.vst3Catalogue = vst3Catalogue
        self.vst2Catalogue = vst2Catalogue
        self.engine = engine
        self.trackCount = max(1, trackCount)
        stripCapacity = max(1, trackCount) + Self.busCapacity + 1
        routingMeters = (0..<(max(1, trackCount) + Self.busCapacity + 1)).map { _ in AudioMeters(trackCapacity: 0) }
        let rate = sampleRate ?? engine.outputNode.outputFormat(forBus: 0).sampleRate
        format = AVAudioFormat(standardFormatWithSampleRate: rate > 0 ? rate : 48_000, channels: 2)!
        routingGraph = try AudioRoutingGraph(engine:engine,sampleRate:format.sampleRate)
        let chainRate = format.sampleRate
        routingGraph.makeInsertChain = { try BuiltinEffectsChain(sampleRate:chainRate) }
        routingGraph.collapsesTrackInputs = true
        delayReturnCompensation = try AudioDelayCompensator(sampleRate:format.sampleRate)
        reverbReturnCompensation = try AudioDelayCompensator(sampleRate:format.sampleRate)
        nativeReturnCompensation = try AudioDelayCompensator(sampleRate:format.sampleRate)
        endpointStore = AudioEndpointStore(capacity: (max(1, trackCount) + Self.busCapacity + 1) * Self.endpointStride)
        transport = .allocate(capacity: 6)
        transport.initialize(repeating: 0, count: 6)
        graphQuantum = .allocate(capacity: 1)
        graphQuantum.initialize(to: 0)
        trackControl = .allocate(capacity: max(1, trackCount) * 6)
        trackControl.initialize(repeating: 0, count: max(1, trackCount) * 6)
        automationOffsets = .allocate(capacity: max(1, trackCount) * Self.offsetStride + 1)
        automationOffsets.initialize(repeating: 0, count: max(1, trackCount) * Self.offsetStride + 1)
        for track in 0..<self.trackCount {
            for index in [0, 2, 3] {
                atomicStore(Int64(bitPattern: Double(1).bitPattern), at: trackControl.advanced(by: track * 6 + index))
            }
        }
        atomicStore(Int64(bitPattern: Double(120).bitPattern), at: transport)
        atomicStore(4, at: transport.advanced(by: 4)); atomicStore(4, at: transport.advanced(by: 5))
        for node in [delayInput, reverbInput, delayReturn, reverbReturn,nativeReturnInput,delayReturnCompensation.node,reverbReturnCompensation.node,nativeReturnCompensation.node] as [AVAudioNode] { engine.attach(node) }
        delayReturn.wetDryMix = 100
        reverbReturn.wetDryMix = 100
        reverbReturn.loadFactoryPreset(.mediumHall)
        engine.connect(delayInput, to: delayReturn, format: format)
        engine.connect(delayReturn,to:delayReturnCompensation.node,format:format)
        engine.connect(delayReturnCompensation.node, to: engine.mainMixerNode, format: format)
        engine.connect(reverbInput, to: reverbReturn, format: format)
        engine.connect(reverbReturn,to:reverbReturnCompensation.node,format:format)
        engine.connect(reverbReturnCompensation.node, to: engine.mainMixerNode, format: format)
        engine.connect(nativeReturnInput,to:nativeReturnCompensation.node,format:format)
        engine.connect(nativeReturnCompensation.node,to:engine.mainMixerNode,format:format)
        for track in 0..<min(self.trackCount, realizedStrips ?? ProjectLayout.channelCount(trackCount:ProjectLayout.minimumReturnBus)) { try realizeStrip(track) }
    }

    /// Main thread. Attaches one channel strip's nodes. Safe while the graph runs: new
    /// nodes join only through the shared send mixers' fresh input buses.
    func realizeStrip(_ track: Int) throws {
            precondition(track == trackInputs.count)
            let input = AVAudioMixerNode(), output = AVAudioMixerNode(), instrument = AVAudioMixerNode()
            let delay = AVAudioMixerNode(), reverb = AVAudioMixerNode()
            let nativeCompensation = try AudioDelayCompensator(sampleRate:format.sampleRate)
            let trackCompensation = try AudioDelayCompensator(sampleRate:format.sampleRate)
            let sendCompensation = try AudioDelayCompensator(sampleRate:format.sampleRate)
            let mixControl = try AudioMixAutomation(sampleRate: format.sampleRate)
            // A send has one automated value, versus four independent mix targets.
            // At the fixed 48 kHz engine rate, 4096 events cover 2 s PDC plus lookahead.
            let sendEvents = max(4096, Int(ceil((ceil(2 * format.sampleRate) + 16_384) / 32)))
            let delayControl = try AudioMixAutomation(sampleRate: format.sampleRate, eventCapacity: sendEvents)
            let reverbControl = try AudioMixAutomation(sampleRate: format.sampleRate, eventCapacity: sendEvents)
            engine.attach(input); engine.attach(output); engine.attach(instrument)
            engine.attach(delay); engine.attach(reverb)
            for node in [nativeCompensation.node,trackCompensation.node,sendCompensation.node,
                         mixControl.node,delayControl.node,reverbControl.node] { engine.attach(node) }
            delayControl.setGain(0); reverbControl.setGain(0)
            engine.connect(delay, to: delayControl.node, format: format)
            engine.connect(delayControl.node, to: delayInput, format: format)
            engine.connect(reverb, to: reverbControl.node, format: format)
            engine.connect(reverbControl.node, to: reverbInput, format: format)
            if trackInputs.isEmpty {
                // Render-side readers index these arrays; never let growth reallocate them.
                for list in [\AudioPluginRack.trackInputs,\.trackOutputs,\.instrumentMixers,\.delaySends,\.reverbSends] { self[keyPath:list].reserveCapacity(trackCount) }
                for list in [\AudioPluginRack.instrumentAutomation,\.delayAutomation,\.reverbAutomation] { self[keyPath:list].reserveCapacity(trackCount) }
                for list in [\AudioPluginRack.nativeCompensators,\.trackCompensators,\.sendCompensators] { self[keyPath:list].reserveCapacity(trackCount) }
                nativeSources.reserveCapacity(trackCount)
            }
            trackInputs.append(input); trackOutputs.append(output); instrumentMixers.append(instrument)
            delaySends.append(delay); reverbSends.append(reverb)
            instrumentAutomation.append(mixControl); delayAutomation.append(delayControl); reverbAutomation.append(reverbControl)
            nativeCompensators.append(nativeCompensation); trackCompensators.append(trackCompensation); sendCompensators.append(sendCompensation)
            // Strips that are not flat tracks (Sounds, unused) stay disconnected from the
            // legacy master, but need a negotiated source output before routing is
            // prepared. This private dormant branch is replaced transactionally by
            // AudioRoutingGraph or rebuildFlat just like the ordinary flat strip branch.
            engine.connect(input,to:trackCompensation.node,format:format)
            engine.connect(trackCompensation.node,to:output,format:format)
    }

    /// Main thread. Realize strips for `channels` channels (st_channel_count of the
    /// project's track count) and wire `tracks` flat tracks when routing is off. Adding
    /// tracks never stops or pauses the graph: only new nodes are attached and wired.
    /// `sources` supplies the native source node for each newly realized strip.
    func ensureStrips(channels: Int, tracks: Int, sources: (Int) -> AVAudioNode) throws {
        guard channels <= trackCount, tracks >= 0, tracks <= channels else { throw RackError.invalid("The project needs more mixer channels than the audio host supports.") }
        let first = realizedStrips
        while realizedStrips < channels {
            let index = realizedStrips
            try realizeStrip(index)
            let source = sources(index)
            if source.engine == nil { engine.attach(source) }
            nativeSources.append(source)
            // A dormant connection negotiates the source's format before routing can use it
            // directly (hasDirectSource); its compensator branch is not rendered.
            if engine.outputConnectionPoints(for:source,outputBus:0).isEmpty {
                engine.connect(source,to:nativeCompensators[index].node,format:format)
            }
        }
        let previousFlat = flatTrackCount
        flatTrackCount = max(ProjectLayout.minimumReturnBus, tracks)
        guard first < realizedStrips || previousFlat != flatTrackCount else { return }
        if routingDefinition?.enabled != true {
            for track in 0..<flatTracks where track >= first || track >= previousFlat { rebuildFlat(track) }
            // Former flat tracks beyond the new count stop feeding the master.
            for track in flatTracks..<max(flatTracks,min(previousFlat,realizedStrips)) { engine.disconnectNodeOutput(trackOutputs[track]) }
        }
        refreshLatencyCompensation()
    }

    deinit {
        // The owning audio host must stop the engine before releasing the rack.
        transport.deinitialize(count: 6); transport.deallocate()
        graphQuantum.deinitialize(count: 1); graphQuantum.deallocate()
        trackControl.deinitialize(count: trackCount * 6); trackControl.deallocate()
        automationOffsets.deinitialize(count: trackCount * Self.offsetStride + 1); automationOffsets.deallocate()
    }

    /// Legacy form: one source per realized strip, then the native return sources.
    func installNativeSources(_ sources: [AVAudioNode]) {
        installNativeSources(Array(sources.prefix(realizedStrips)), returns: Array(sources.dropFirst(realizedStrips)))
    }

    /// `sources[i]` feeds realized strip i; `returns` are the native return buses.
    func installNativeSources(_ sources: [AVAudioNode], returns: [AVAudioNode]) {
        nativeSources = sources
        for source in sources + returns where source.engine == nil { engine.attach(source) }
        for track in 0..<flatTracks { rebuildFlat(track) }
        // Additional native return buses (delay/reverb) bypass track insert effects.
        for (index,source) in returns.enumerated() {
            engine.connect(source, to:nativeReturnInput,fromBus:0,toBus:AVAudioNodeBus(index),format:format)
        }
        refreshLatencyCompensation()
    }

    /// Call only with the engine stopped and callbacks drained. Selectors are owned by AudioHost.
    func setLineInputSources(_ sources:[Int:AVAudioNode]) throws {
        guard !engine.isRunning else { throw RackError.invalid("Stop audio before changing line-input routes.") }
        try awaitRenderQuiescence()
        for node in lineInputSources.values { engine.disconnectNodeOutput(node) }
        lineInputSources=sources
        // A line input changes whether a routed track's native source feeds its strip directly.
        if routingDefinition?.enabled == true { try configureRouting(routingProject,force:true) }
        else { for track in 0..<flatTracks { rebuildFlat(track) } }
        refreshLatencyCompensation()
    }

    func outputNode(track: Int) -> AVAudioMixerNode? {
        guard trackOutputs.indices.contains(track) else { return nil }
        return trackOutputs[track]
    }
}

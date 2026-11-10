import Foundation
import AVFoundation
import AudioToolbox

/// One graph node that renders several consecutive in-process inserts of a strip back to back
/// (BuiltinEffectsChain). The assembler builds one for every run of chainable inserts
/// (AudioRoutingGraph.Effect.chainMember) and can replace its members in place.
protocol AudioInsertChain: AnyObject {
    var node: AVAudioUnitEffect { get }
    /// Control thread; realtime-safe handoff to the renderer.
    func setMembers(_ members: [AnyObject]) throws
    var members: [AnyObject] { get }
    func collectRetired()
}

/// The control-thread assembler for the validated routing DAG. It owns only helper
/// mixers, gain processors and delays. Sources, plug-ins and wet-return processors
/// remain the rack's responsibility. Callers stop AND drain render readers before
/// install/commit/rollback/uninstall; engine.isRunning alone is not a drain barrier.
final class AudioRoutingGraph {
    enum GraphError: LocalizedError {
        case invalid(String)
        var errorDescription: String? { if case .invalid(let message) = self { return message }; return nil }
    }

    enum CaptureStage: String, CaseIterable {
        case preInserts, preFader, postFader
        fileprivate var graphStage: AudioRouting.Stage {
            switch self { case .preInserts: return .input; case .preFader: return .preFader; case .postFader: return .postFader }
        }
    }
    struct StageKey: Hashable {
        let owner: String
        let stage: CaptureStage
        var id: String { owner + "/" + stage.rawValue }
        fileprivate var nodeID: String { AudioRouting.nodeID(owner: owner, stage: stage.graphStage) }
    }

    /// Independently verified immutable PCM sources prepared by the host from a
    /// playbackProjection. They start at full-song frame zero with capture PDC
    /// already trimmed; neither node may introduce latency or have audio inputs.
    /// This assembler validates topology, not files, hashes or transport spans.
    struct FrozenStages {
        let owner: String
        let preFader: AVAudioNode
        let preInserts: AVAudioNode?
        init(owner: String, preFader: AVAudioNode, preInserts: AVAudioNode? = nil) {
            self.owner = owner; self.preFader = preFader; self.preInserts = preInserts
        }
    }

    struct Effect {
        let owner: String
        let slot: Int
        let instanceID: String
        let node: AVAudioUnit
        /// Set for in-process inserts that an insert-chain node can render (built-in effects):
        /// consecutive ones on a strip share one chain node instead of a node each.
        let chainMember: AnyObject?
        let mainInputBus: Int
        /// VST adapters provide their actual auxiliary classification and main
        /// ordinal. Native AUs default to main input 0 and additional key buses.
        let inputBuses: [AudioRouting.InputBus]?
        /// Optional adapter bridge for activating a vendor's auxiliary bus. The
        /// assembler also enables the corresponding AU bus. Control thread only.
        let configureAuxiliary: ((Int, Bool) throws -> Void)?
        init(owner: String, slot: Int, instanceID: String, node: AVAudioUnit, mainInputBus: Int = 0,
             inputBuses: [AudioRouting.InputBus]? = nil, chainMember: AnyObject? = nil,
             configureAuxiliary: ((Int, Bool) throws -> Void)? = nil) {
            self.owner = owner; self.slot = slot; self.instanceID = instanceID; self.node = node; self.chainMember = chainMember
            self.mainInputBus = mainInputBus
            self.inputBuses = inputBuses; self.configureAuxiliary = configureAuxiliary
        }
    }

    struct SystemReturn {
        let id: String
        let processor: AVAudioNode
        let latencyFrames: Int
        let gains: [String: Float]
        let returnGain: Float
        /// Tracks that get a send route to this return; nil: every track. A track whose
        /// send can never be audible (no level, step FX or lane) needs no route.
        let senders: Set<String>?
        init(id: String, processor: AVAudioNode, latencyFrames: Int = 0,
             gains: [String: Float], returnGain: Float = 1, senders: Set<String>? = nil) {
            self.id = id; self.processor = processor; self.latencyFrames = latencyFrames
            self.gains = gains; self.returnGain = returnGain; self.senders = senders
        }
    }

    struct Source {
        let id: String
        let node: AVAudioNode
        let latencyFrames: Int
        init(id: String, node: AVAudioNode, latencyFrames: Int = 0) {
            self.id = id; self.node = node; self.latencyFrames = latencyFrames
        }
    }

    /// Obtain endpoints during graph preparation and keep a dense array in the host
    /// render snapshot. Scheduling then performs no string lookup or allocation.
    final class AutomationEndpoint {
        let automation: AudioMixAutomation
        let inputArrival: Int64
        init(_ automation: AudioMixAutomation, inputArrival: Int) {
            self.automation = automation; self.inputArrival = Int64(inputArrival)
        }
        @inline(__always) func schedule(target: Int, value: Float, sampleTime: Int64) {
            let (time, overflow) = sampleTime.addingReportingOverflow(inputArrival)
            guard !overflow else { return }
            automation.schedule(target: target, value: value, sampleTime: time)
        }
        /// Route gains share the existing send processor's raw gain target.
        @inline(__always) func scheduleGain(_ value: Float, sampleTime: Int64) {
            schedule(target: 4, value: value, sampleTime: sampleTime)
        }
        func setGain(_ value: Float) { automation.setGain(value) }
    }

    final class Strip {
        let owner: AudioRouting.Owner
        /// The input stage: a summing mixer, or (collapsesTrackInputs) the track's own source.
        let input: AVAudioNode
        let preFader: AVAudioNode
        let postFader: AVAudioNode
        let mix: AutomationEndpoint
        init(owner: AudioRouting.Owner, input: AVAudioNode, preFader: AVAudioNode,
             mix: AutomationEndpoint, postFader: AVAudioNode? = nil) {
            self.owner = owner; self.input = input; self.preFader = preFader
            self.mix = mix; self.postFader = postFader ?? mix.automation.node
        }
        func setMix(volume: Float, pan: Float, muted: Bool) {
            mix.automation.setDefaults(trackVolume: volume, instrumentVolume: 1, masterVolume: 1,
                trackPan: pan, instrumentPan: 0, muted: muted)
        }
    }

    private static func audibleSoloEdges(definition:AudioRouting.Definition,graph:AudioRouting.Graph,captureAnchors:Set<String>)->Set<String>? {
        let soloStages = Set(definition.owners.filter(\.solo).map { AudioRouting.nodeID(owner: $0.id, stage: .postFader) })
        // Solo controls audible output/send edges, never the signal used by a
        // detector. A soloed bus includes its audio ancestors; a soloed track
        // includes its downstream groups/returns without unmuting their peers.
        let audioEdges = graph.edges.filter { !$0.id.hasPrefix("sidechain:") && !captureAnchors.contains($0.id) }
        var soloEdges = Set<String>()
        if !soloStages.isEmpty {
            // Downstream starts at every stage of a soloed strip, so its pre-insert and
            // pre-fader sends stay audible just like its post-fader sends.
            let soloed = definition.owners.filter(\.solo).map(\.id)
            var upstream = soloStages, downstream = soloStages.union(soloed.flatMap { owner in
                [AudioRouting.Stage.input, .preFader].map { AudioRouting.nodeID(owner: owner, stage: $0) } })
            var changed = true
            while changed {
                changed = false
                for edge in audioEdges where upstream.contains(edge.destination) {
                    soloEdges.insert(edge.id)
                    if upstream.insert(edge.source).inserted { changed = true }
                }
            }
            changed = true
            while changed {
                changed = false
                for edge in audioEdges where downstream.contains(edge.source) {
                    soloEdges.insert(edge.id)
                    if downstream.insert(edge.destination).inserted { changed = true }
                }
            }
            // A detector bus may itself sum other strips or process another
            // detector. Preserve that complete computational ancestry as well;
            // its independent downstream audible outputs remain solo-gated.
            var detectors = Set(graph.edges.filter { $0.id.hasPrefix("sidechain:") }.map(\.source))
            changed = true
            while changed {
                changed = false
                for edge in graph.edges where !captureAnchors.contains(edge.id) && detectors.contains(edge.destination) {
                    soloEdges.insert(edge.id)
                    if detectors.insert(edge.source).inserted { changed = true }
                }
            }
        }
        return soloStages.isEmpty ? nil : soloEdges
    }

    final class Prepared {
        let definition: AudioRouting.Definition
        private let soloGates:[String:AudioMixAutomation]
        private let captureAnchors:Set<String>
        let graph: AudioRouting.Graph
        let plan: AudioRouting.Plan
        let strips: [String: Strip]
        let ownerOrder: [String]
        let mixEndpoints: [AutomationEndpoint]
        let sendEndpoints: [String: AutomationEndpoint]
        let sidechainEndpoints: [String: AutomationEndpoint]
        let builtinSendEndpoints: [String: [Int: AutomationEndpoint]]
        let returnEndpoints: [String: AutomationEndpoint]
        let pluginInputArrival: [String: Int]
        let mixControls: [String: AudioMixAutomation]
        let mixInputArrival: [String: Int]
        let sendControls: [String: AudioMixAutomation]
        let sendInputArrival: [String: Int]
        let builtinSendControls: [String: [Int: AudioMixAutomation]]
        let builtinSendInputArrival: [String: [Int: Int]]
        let stripOutputs: [String: AVAudioNode]
        let captures: [String: AudioRoutingCapture]
        let captureArrivalFrames: [String: Int]
        /// In-path captures keyed by exact owner and boundary. The old owner-only
        /// properties above are aliases for these same post-fader processors.
        let stageCaptures: [StageKey: AudioRoutingCapture]
        let stageCaptureArrivalFrames: [StageKey: Int]
        let frozenStages: [String: FrozenStages]
        let masterOutput: AVAudioNode
        let latencyFrames: Int
        /// Existing stage outputs for optional host metering taps.
        /// These are existing render paths, never independent source-node pulls.
        let capturePoints: [String: AVAudioNode]
        fileprivate let engineIdentity: ObjectIdentifier
        fileprivate let sampleRate: Double
        fileprivate let connections: [Connection]
        fileprivate let helpers: [AVAudioNode]
        fileprivate let effects: [Effect]
        fileprivate let delays: [AudioDelayCompensator]
        fileprivate let busConfigurations: [BusConfiguration]
        fileprivate let processingMixes: [AudioMixAutomation]
        /// Build state for incremental changes (strips, ports, mixer inputs per owner and edge).
        fileprivate let assembly: Assembly
        fileprivate init(definition: AudioRouting.Definition, graph: AudioRouting.Graph, plan: AudioRouting.Plan,
            strips: [String: Strip], ownerOrder: [String], sendEndpoints: [String: AutomationEndpoint],
            sidechainEndpoints: [String: AutomationEndpoint], builtinSendEndpoints: [String: [Int: AutomationEndpoint]],
            returnEndpoints: [String: AutomationEndpoint], pluginInputArrival: [String: Int],
            masterOutput: AVAudioNode, capturePoints: [String: AVAudioNode], stageCaptures: [StageKey: AudioRoutingCapture], frozenStages: [String: FrozenStages], engine: AVAudioEngine,
            sampleRate: Double, connections: [Connection], helpers: [AVAudioNode], effects: [Effect],
            delays: [AudioDelayCompensator], busConfigurations: [BusConfiguration], mixes: [AudioMixAutomation],
            soloGates:[String:AudioMixAutomation],captureAnchors:Set<String>, assembly: Assembly) {
            self.assembly = assembly
            self.definition = definition; self.graph = graph; self.plan = plan; self.strips = strips
            self.soloGates=soloGates;self.captureAnchors=captureAnchors
            self.ownerOrder = ownerOrder; mixEndpoints = ownerOrder.compactMap { strips[$0]?.mix }
            self.sendEndpoints = sendEndpoints; self.sidechainEndpoints = sidechainEndpoints
            self.builtinSendEndpoints = builtinSendEndpoints; self.returnEndpoints = returnEndpoints
            self.pluginInputArrival = pluginInputArrival; self.masterOutput = masterOutput
            mixControls = strips.mapValues { $0.mix.automation }
            mixInputArrival = strips.mapValues { Int($0.mix.inputArrival) }
            let routes = sendEndpoints.merging(sidechainEndpoints) { current, _ in current }
            sendControls = routes.mapValues(\.automation); sendInputArrival = routes.mapValues { Int($0.inputArrival) }
            builtinSendControls = builtinSendEndpoints.mapValues { $0.mapValues(\.automation) }
            builtinSendInputArrival = builtinSendEndpoints.mapValues { $0.mapValues { Int($0.inputArrival) } }
            stripOutputs = strips.mapValues(\.postFader); self.stageCaptures = stageCaptures
            self.frozenStages = frozenStages
            stageCaptureArrivalFrames = Dictionary(uniqueKeysWithValues: stageCaptures.keys.map { ($0, plan.arrival[$0.nodeID] ?? 0) })
            captures = Dictionary(uniqueKeysWithValues: stageCaptures.filter { $0.key.stage == .postFader }.map { ($0.key.owner, $0.value) })
            captureArrivalFrames = Dictionary(uniqueKeysWithValues: stageCaptureArrivalFrames.filter { $0.key.stage == .postFader }.map { ($0.key.owner, $0.value) })
            self.capturePoints = capturePoints; engineIdentity = ObjectIdentifier(engine); self.sampleRate = sampleRate
            self.connections = connections; self.helpers = helpers; self.effects = effects; self.delays = delays
            self.busConfigurations = busConfigurations; processingMixes = mixes
            latencyFrames = plan.arrival[AudioRouting.nodeID(owner: "master", stage: .postFader)] ?? 0
        }
        /// Solo changes gain on permanent audible-edge gates. Detector paths
        /// retain their upstream signal, and the graph/plug-ins keep rendering.
        func updateSolo(_ definition:AudioRouting.Definition) {
            let allowed=AudioRoutingGraph.audibleSoloEdges(definition:definition,graph:graph,captureAnchors:captureAnchors)
            for (edge,gate) in soloGates { gate.setSoloGain(allowed == nil || allowed!.contains(edge) ? 1 : 0) }
        }

        /// Insert-chain node of each chained insert, by instance ID.
        var insertChains: [String: AudioInsertChain] { assembly.chains }

        func resetAutomation() { for mix in processingMixes { mix.reset() } }
        /// Control thread after a graph epoch change. Delay reset currently calls
        /// the underlying AU reset and is deliberately not a render-thread API.
        func resetDelays() { for delay in delays { delay.reset() } }
        /// Quiescent control thread before a new offline pass. The host drains
        /// each capture after every master render and never pulls these nodes.
        func configureCaptures(enabled: Bool = true, startFrame: Int64 = 0, frameLimit: Int64? = nil) throws {
            for (key, capture) in stageCaptures {
                try capture.configureCapture(enabled: enabled, arrivalFrames: Int64(stageCaptureArrivalFrames[key] ?? 0),
                    startFrame: startFrame, frameLimit: frameLimit)
            }
        }
        func scheduleMix(owner: String, target: Int, value: Float, sampleTime: Int64) {
            strips[owner]?.mix.schedule(target: target, value: value, sampleTime: sampleTime)
        }
        func scheduleBuiltinSend(owner: String, target: Int, value: Float, sampleTime: Int64) {
            builtinSendEndpoints[owner]?[target]?.scheduleGain(value, sampleTime: sampleTime)
        }
        var status: [String: Any] {
            ["owners": ownerOrder, "latencyFrames": latencyFrames,
             "latencyMilliseconds": Double(latencyFrames) * 1000 / sampleRate,
             "pluginInputArrival": pluginInputArrival, "edgeDelays": plan.edgeDelays,
             "captureArrivalFrames": captureArrivalFrames,
             "stageCaptureArrivalFrames": Dictionary(uniqueKeysWithValues: stageCaptureArrivalFrames.map { ($0.key.id, $0.value) }),
             "frozenOwners": frozenStages.keys.sorted(),
             "droppedAutomationEvents": processingMixes.reduce(0) { $0 + $1.droppedEvents },
             "insertChains": Set(assembly.chains.values.map { ObjectIdentifier($0) }).count,
             "chainedInserts": assembly.chains.count, "foldedSoloGates": assembly.foldedGates.count,
             "unavailableSidechains": graph.unavailableSidechains.map {
                 ["id": $0.sidechain.id, "reason": $0.reason,
                  "instanceID": $0.sidechain.destination.instanceID,
                  "inputBus": $0.sidechain.destination.inputBus] as [String: Any]
             }]
        }
    }

    fileprivate struct Destination {
        let node: AVAudioNode
        let bus: AVAudioNodeBus
    }
    fileprivate struct Connection {
        let source: AVAudioNode
        let bus: AVAudioNodeBus
        let destinations: [Destination]
        let format: AVAudioFormat
    }
    private struct OutputKey: Hashable { let node: ObjectIdentifier; let bus: AVAudioNodeBus }
    fileprivate struct InputKey: Hashable { let node: ObjectIdentifier; let bus: AVAudioNodeBus }
    fileprivate struct BusConfiguration {
        let effect: Effect
        let bus: Int
        let enabled: Bool
        let format: AVAudioFormat?
        let auxiliary: Bool
    }
    fileprivate struct BusSnapshot {
        let configuration: BusConfiguration
        let enabled: Bool
        let format: AVAudioFormat
    }
    fileprivate struct PresentationSnapshot {
        let bus: AUAudioUnitBus
        let latency: TimeInterval
    }
    fileprivate struct PendingChange {
        let previous: Prepared?
        let installed: Prepared
        let connections: [Connection]
        let busStates: [BusSnapshot]
        let presentation: [PresentationSnapshot]
        let newlyAttached: [AVAudioNode]
    }
    fileprivate struct Port {
        let output: AVAudioNode
        let input: AVAudioNode?
        let inputChannels: AVAudioChannelCount
        let mixer: Bool
        var defaultBus: AVAudioNodeBus = 0
    }

    let engine: AVAudioEngine
    let sampleRate: Double
    let maximumDelayFrames: Int
    /// Builds insert-chain nodes; nil renders every insert as its own node (as in tests that
    /// assemble graphs from plain Audio Units).
    var makeInsertChain: (() throws -> AudioInsertChain)?
    /// A track whose input stage has only its own source (no routes into it, no capture or
    /// frozen print) uses that source as the stage instead of a one-input mixer. A rewired
    /// track (its channel moved) then reconnects its first processor to the new source live.
    var collapsesTrackInputs = false
    /// An owner's output route uses the owner's mix processor as its solo gate where that is
    /// equivalent (see foldableGate) instead of a separate gate node.
    var foldsSoloGates = true
    private let stereo: AVAudioFormat
    private(set) var current: Prepared?
    private var pending: PendingChange?
    /// Helpers disconnected by incremental changes, detached at the next quiescent commit.
    private var retiredHelpers: [AVAudioNode] = []
    /// Why the last change could not be made while running (diagnostics; nil when it could).
    private(set) var lastIncrementalRefusal: String?
    private var sinkBus: AVAudioNodeBus?

    init(engine: AVAudioEngine, sampleRate: Double = 48_000, maximumDelayFrames: Int? = nil) throws {
        guard sampleRate.isFinite, sampleRate >= 8_000, sampleRate <= 768_000,
              let format = AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: 2) else {
            throw GraphError.invalid("Routing requires a supported stereo sample rate.")
        }
        self.engine = engine; self.sampleRate = sampleRate; stereo = format
        self.maximumDelayFrames = max(0, min(1_536_000, maximumDelayFrames ?? Int(ceil(sampleRate * 2))))
    }

    /// Validated topology and timing for one routing definition; pure apart from
    /// reading the supplied nodes' formats. Shared by full and incremental preparation.
    private struct Analysis {
        let definition: AudioRouting.Definition
        let graph: AudioRouting.Graph
        let plan: AudioRouting.Plan
        let trackIDs: Set<String>
        let frozen: [String: FrozenStages]
        let selectedCaptures: Set<StageKey>
        let captureAnchors: Set<String>
        let soloEdges: Set<String>?
        let enabledKeys: [AudioRouting.ResolvedSidechain]
        let sidechainsByEdge: [String: AudioRouting.ResolvedSidechain]
        let sendByEdge: [String: AudioRouting.Send]
        let actualBuses: [String: [AudioRouting.InputBus]]
        let auxiliaryIndices: [String: Set<Int>]
        /// Incoming edges per destination node.
        let incoming: [String: [AudioRouting.Edge]]
        /// Outgoing edges per source node.
        let outgoing: [String: [AudioRouting.Edge]]
    }

    private func analyze(definition: AudioRouting.Definition, sources: [String: AVAudioNode], effects: [Effect],
                         sourceLatencies: [String: Int], systemReturns: [SystemReturn], extraMasterSources: [Source],
                         captureOwners: Set<String>, captureStages: Set<StageKey>, frozenStages: [FrozenStages]) throws -> Analysis {
        precondition(Thread.isMainThread)
        guard definition.enabled else { throw GraphError.invalid("Legacy projects without routing retain their existing graph.") }
        // Sanity bound only: tracks and buses up to the sanity limit, master.
        guard definition.owners.count <= 2 * ProjectLayout.trackSanityLimit + 65, definition.sends.count + definition.sidechains.count <= 128,
              definition.owners.filter({ $0.kind == .master }).map(\.id) == ["master"] else {
            throw GraphError.invalid("Routing exceeds its owner or route limits.")
        }
        if definition.mixModel == .legacy,
           definition.sends.contains(where: { $0.tap != .postFader }) || definition.sidechains.contains(where: { $0.tap != .postFader }) {
            throw GraphError.invalid("Pre-insert and pre-fader routing require the standard mix model; legacy track sources already include their faders.")
        }
        guard definition.mixModel == .standard || systemReturns.isEmpty else {
            throw GraphError.invalid("Legacy routing must supply existing wet returns as extra master sources.")
        }
        let trackIDs = Set(definition.owners.filter { $0.trackIndex != nil }.map(\.id))
        let frozenOwners = Set(frozenStages.map(\.owner))
        guard frozenOwners.count == frozenStages.count, frozenOwners.isSubset(of: trackIDs),
              frozenOwners.isEmpty || definition.mixModel == .standard else {
            throw GraphError.invalid("Frozen stages require unique existing tracks in the standard mix model.")
        }
        let frozen = Dictionary(uniqueKeysWithValues: frozenStages.map { ($0.owner, $0) })
        let selectedCaptures = captureStages.union(captureOwners.map { StageKey(owner: $0, stage: .postFader) })
        guard Set(selectedCaptures.map(\.owner)).isSubset(of: Set(definition.owners.map(\.id))) else {
            throw GraphError.invalid("Capture selection contains an unavailable owner.")
        }
        guard definition.mixModel == .standard || selectedCaptures.allSatisfy({ $0.stage == .postFader }) else {
            throw GraphError.invalid("Pre-insert and pre-fader captures require the standard mix model.")
        }
        let liveTrackIDs = trackIDs.subtracting(frozenOwners)
        guard Set(sources.keys) == liveTrackIDs, Set(sourceLatencies.keys).isSubset(of: liveTrackIDs) else {
            throw GraphError.invalid("Every live routing track requires exactly one source; frozen tracks cannot retain an original source or its latency.")
        }
        guard effects.allSatisfy({ !frozenOwners.contains($0.owner) }) else {
            throw GraphError.invalid("Remove baked inserts before substituting a frozen track.")
        }
        for item in frozenStages {
            // Disabled saved routes count too: later enablement must never route
            // a baked late print through an early tap by accident.
            let needsEarly = definition.sends.contains { $0.source == item.owner && $0.tap == .preInserts }
                || definition.sidechains.contains { $0.source == item.owner && $0.tap == .preInserts }
                || selectedCaptures.contains(StageKey(owner: item.owner, stage: .preInserts))
            guard !needsEarly || item.preInserts != nil else {
                throw GraphError.invalid("Frozen track \(item.owner) needs its separate pre-insert print for the saved early routes.")
            }
        }
        var externalIDs = Set<ObjectIdentifier>()
        func external(_ node: AVAudioNode, label: String) throws {
            guard node.engine == nil || node.engine === engine,
                  externalIDs.insert(ObjectIdentifier(node)).inserted,
                  node.numberOfOutputs > 0 else {
                throw GraphError.invalid("\(label) is duplicated, has no output, or belongs to another engine.")
            }
            try validateFormat(node.outputFormat(forBus: 0), label: label + " output")
        }
        for (owner, node) in sources { try external(node, label: owner) }
        for item in frozenStages {
            var stages: [(CaptureStage, AVAudioNode)] = [(.preFader, item.preFader)]
            if let early = item.preInserts { stages.append((.preInserts, early)) }
            for (stage, node) in stages {
                try external(node, label: item.owner + "/" + stage.rawValue)
                guard node.numberOfInputs == 0, node.outputFormat(forBus: 0).channelCount == 2,
                      node.auAudioUnit.latency == 0 else {
                    throw GraphError.invalid("Frozen stage sources must be stereo generators with no new algorithmic latency.")
                }
            }
        }
        var layouts: [AudioRouting.PluginLayout] = [], actualBuses: [String: [AudioRouting.InputBus]] = [:]
        var auxiliaryIndices: [String: Set<Int>] = [:]
        for effect in effects {
            try external(effect.node, label: effect.instanceID)
            guard effect.mainInputBus >= 0, effect.mainInputBus < effect.node.auAudioUnit.inputBusses.count else { throw GraphError.invalid("An insert has no matching main input.") }
            try validateFormat(effect.node.inputFormat(forBus: AVAudioNodeBus(effect.mainInputBus)), label: effect.instanceID + " main input")
            let unit = effect.node.auAudioUnit
            let metadata = effect.inputBuses ?? (0..<unit.inputBusses.count).map {
                AudioRouting.InputBus(index: $0, name: unit.inputBusses[$0].name ?? "Input \($0 + 1)",
                    channels: Int(unit.inputBusses[$0].format.channelCount), isAuxiliary: $0 > 0)
            }
            auxiliaryIndices[effect.instanceID] = Set(metadata.filter {
                $0.isAuxiliary && $0.index >= 0 && $0.index < unit.inputBusses.count && $0.index != effect.mainInputBus &&
                    (1...2).contains($0.channels)
            }.map(\.index))
            // Unsupported, missing or changed auxiliary layouts remain unresolved
            // in the pure plan. They never fall through to the main audio input.
            let supported = metadata.filter { bus in
                guard bus.index >= 0, bus.index != effect.mainInputBus, bus.index < unit.inputBusses.count, bus.isAuxiliary,
                      (1...2).contains(bus.channels) else { return false }
                let actual = unit.inputBusses[bus.index]
                return actual.format.channelCount == bus.channels &&
                    (actual.name ?? "Input \(bus.index + 1)") == bus.name
            }
            actualBuses[effect.instanceID] = supported
            let seconds = unit.latency
            guard seconds.isFinite, seconds >= 0, seconds * sampleRate <= Double(Int.max) else {
                throw GraphError.invalid("The insert reported an invalid latency.")
            }
            layouts.append(AudioRouting.PluginLayout(instanceID: effect.instanceID, owner: effect.owner,
                slot: effect.slot, latencyFrames: Int((seconds * sampleRate).rounded()), inputBuses: supported))
        }
        let parsed = try definition.graph(pluginLayouts: layouts)
        if !frozenOwners.isEmpty {
            guard parsed.unavailableSidechains.allSatisfy({ !$0.sidechain.enabled }) else {
                throw GraphError.invalid("Resolve enabled sidechains before preparing a frozen playback projection.")
            }
            for owner in frozenOwners {
                let input = AudioRouting.nodeID(owner: owner, stage: .input)
                let pre = AudioRouting.nodeID(owner: owner, stage: .preFader)
                guard parsed.edges.allSatisfy({ edge in
                    (edge.destination != input || edge.id == "native:" + owner)
                        && (edge.destination != pre || edge.id == "preFader:" + owner)
                }) else {
                    throw GraphError.invalid("Remove baked incoming routes before substituting frozen track \(owner).")
                }
            }
        }
        var nodes = parsed.nodes.map { node -> AudioRouting.Node in
            for owner in trackIDs where node.id == AudioRouting.nodeID(owner: owner, stage: .source) {
                return AudioRouting.Node(id: node.id, latencyFrames: sourceLatencies[owner] ?? 0)
            }
            return node
        }
        var edges = parsed.edges
        // The early and late prints are independent generators. In particular,
        // the pre-insert print must never feed the already-baked pre-fader path.
        let removedSources = Set(frozenOwners.map { AudioRouting.nodeID(owner: $0, stage: .source) })
        nodes.removeAll { removedSources.contains($0.id) }
        let removedEdges = Set(frozenOwners.flatMap { ["native:" + $0, "preFader:" + $0] })
        edges.removeAll { removedEdges.contains($0.id) }
        var returnIDs = Set<String>(), extraIDs = Set<String>()
        let masterInput = AudioRouting.nodeID(owner: "master", stage: .input)
        for item in systemReturns {
            guard ["delay", "reverb"].contains(item.id), returnIDs.insert(item.id).inserted,
                  Set(item.gains.keys).isSubset(of: trackIDs), item.returnGain.isFinite,
                  (0...2).contains(item.returnGain), item.gains.values.allSatisfy({ $0.isFinite && (0...2).contains($0) }) else {
                throw GraphError.invalid("Invalid fixed delay/reverb return configuration.")
            }
            try external(item.processor, label: "System return " + item.id)
            guard item.processor.numberOfInputs > 0 else { throw GraphError.invalid("A wet return requires an audio input.") }
            try validateFormat(item.processor.inputFormat(forBus: 0), label: "System return input")
            let input = "system:\(item.id)/input", processor = "system:\(item.id)/processor", output = "system:\(item.id)/output"
            nodes += [AudioRouting.Node(id: input), AudioRouting.Node(id: processor, latencyFrames: item.latencyFrames), AudioRouting.Node(id: output)]
            for owner in trackIDs.sorted() where item.senders?.contains(owner) ?? true {
                edges.append(AudioRouting.Edge(id: builtinEdge(owner: owner, id: item.id),
                    source: AudioRouting.nodeID(owner: owner, stage: .postFader), destination: input))
            }
            edges += [AudioRouting.Edge(id: "system:\(item.id)/processor", source: input, destination: processor),
                      AudioRouting.Edge(id: "system:\(item.id)/gain", source: processor, destination: output),
                      AudioRouting.Edge(id: "system:\(item.id)/master", source: output, destination: masterInput)]
        }
        for source in extraMasterSources {
            guard !source.id.isEmpty, extraIDs.insert(source.id).inserted else { throw GraphError.invalid("Duplicate extra master source.") }
            try external(source.node, label: source.id)
            let id = "extra:\(source.id)"
            nodes.append(AudioRouting.Node(id: id, latencyFrames: source.latencyFrames))
            edges.append(AudioRouting.Edge(id: id + "/master", source: id, destination: masterInput))
        }
        // An otherwise disconnected stage is rendered only when requested for a
        // capture. Its silent root edge keeps the original graph's single pull;
        // no independent render call is allowed to advance a source twice.
        let beforeCapture = AudioRouting.Graph(nodes: nodes, edges: edges, resolvedSidechains: parsed.resolvedSidechains,
                                              unavailableSidechains: parsed.unavailableSidechains)
        let masterDependencies = try beforeCapture.dependencies(of: [AudioRouting.nodeID(owner: "master", stage: .postFader)])
        var captureAnchors = Set<String>()
        for key in selectedCaptures.sorted(by: { $0.id < $1.id }) {
            let stage = key.nodeID
            if !masterDependencies.contains(stage) {
                let edgeID = "captureAnchor:" + (key.stage == .postFader ? key.owner : key.id)
                edges.append(AudioRouting.Edge(id: edgeID, source: stage, destination: masterInput))
                captureAnchors.insert(edgeID)
            }
        }
        let graph = AudioRouting.Graph(nodes: nodes, edges: edges, resolvedSidechains: parsed.resolvedSidechains,
                                       unavailableSidechains: parsed.unavailableSidechains)
        let plan = try graph.plan(maximumFrames: maximumDelayFrames)
        let enabledKeys = graph.resolvedSidechains.filter { $0.sidechain.enabled }
        return Analysis(definition: definition, graph: graph, plan: plan, trackIDs: trackIDs, frozen: frozen,
            selectedCaptures: selectedCaptures, captureAnchors: captureAnchors,
            soloEdges: Self.audibleSoloEdges(definition: definition, graph: graph, captureAnchors: captureAnchors),
            enabledKeys: enabledKeys,
            sidechainsByEdge: Dictionary(uniqueKeysWithValues: enabledKeys.map { ("sidechain:" + $0.sidechain.id, $0) }),
            sendByEdge: Dictionary(uniqueKeysWithValues: definition.sends.filter(\.enabled).map { ("send:" + $0.id, $0) }),
            actualBuses: actualBuses, auxiliaryIndices: auxiliaryIndices,
            incoming: Dictionary(grouping: graph.edges, by: \.destination),
            outgoing: Dictionary(grouping: graph.edges, by: \.source))
    }

    /// Everything one preparation builds, kept with the prepared graph so a later
    /// incremental change can reuse the strips, ports and mixer inputs it owns.
    fileprivate final class Assembly {
        var helpers: [AVAudioNode] = [], mixes: [AudioMixAutomation] = [], delays: [AudioDelayCompensator] = []
        var ports: [String: Port] = [:], strips: [String: Strip] = [:], capturePoints: [String: AVAudioNode] = [:]
        var rawConnections: [Connection] = [], mixerInputs: [ObjectIdentifier: AVAudioNodeBus] = [:]
        var sendEndpoints: [String: AutomationEndpoint] = [:], keyEndpoints: [String: AutomationEndpoint] = [:]
        var builtinEndpoints: [String: [Int: AutomationEndpoint]] = [:], returnEndpoints: [String: AutomationEndpoint] = [:]
        var pluginArrival: [String: Int] = [:]
        var stageCaptures: [StageKey: AudioRoutingCapture] = [:]
        var soloGates: [String: AudioMixAutomation] = [:]
        var keyInputs: [InputKey: AVAudioMixerNode] = [:]
        /// Edges realized without a connection: an owner's pre-fader stage that would be a
        /// one-input copy mixer is the input stage's output itself (see buildOwner).
        var collapsedEdges: Set<String> = []
        /// Insert-chain node of each chained insert (by instance ID).
        var chains: [String: AudioInsertChain] = [:]
        /// Output edges whose solo gate is the source strip's own mix processor (no gate node).
        var foldedGates: [String: AudioMixAutomation] = [:]
        /// Track owners whose input stage is their source node (collapsesTrackInputs).
        var collapsedInputs: Set<String> = []
        /// Helper nodes and connections owned by each owner strip and each edge.
        var ownerHelpers: [String: [AVAudioNode]] = [:], edgeHelpers: [String: [AVAudioNode]] = [:]
        var edgeConnections: [String: [Connection]] = [:]
        /// Set while building one owner or edge, so its helpers and connections are recorded.
        var currentOwner: String?, currentEdge: String?

        func copy() -> Assembly {
            let value = Assembly()
            value.helpers = helpers; value.mixes = mixes; value.delays = delays; value.ports = ports; value.strips = strips
            value.capturePoints = capturePoints; value.rawConnections = rawConnections; value.mixerInputs = mixerInputs
            value.sendEndpoints = sendEndpoints; value.keyEndpoints = keyEndpoints; value.builtinEndpoints = builtinEndpoints
            value.returnEndpoints = returnEndpoints; value.pluginArrival = pluginArrival; value.stageCaptures = stageCaptures
            value.soloGates = soloGates; value.keyInputs = keyInputs; value.collapsedEdges = collapsedEdges; value.ownerHelpers = ownerHelpers
            value.edgeHelpers = edgeHelpers; value.edgeConnections = edgeConnections
            value.chains = chains; value.foldedGates = foldedGates; value.collapsedInputs = collapsedInputs
            return value
        }
        func helper(_ node: AVAudioNode) {
            helpers.append(node)
            if let owner = currentOwner { ownerHelpers[owner, default: []].append(node) }
            if let edge = currentEdge { edgeHelpers[edge, default: []].append(node) }
        }
        func connection(_ source: AVAudioNode, _ destination: AVAudioNode, bus: AVAudioNodeBus = 0, format: AVAudioFormat) {
            let value = Connection(source: source, bus: 0, destinations: [Destination(node: destination, bus: bus)], format: format)
            rawConnections.append(value)
            if let edge = currentEdge { edgeConnections[edge, default: []].append(value) }
        }
        func mixer() -> AVAudioMixerNode { let node = AVAudioMixerNode(); helper(node); return node }
        func nextBus(_ node: AVAudioNode) -> AVAudioNodeBus {
            let id = ObjectIdentifier(node), bus = mixerInputs[id, default: 0]; mixerInputs[id] = bus + 1; return bus
        }
    }

    /// An owner's output edge can use the owner's post-fader mix processor as its solo gate
    /// when everything else leaving that stage is a fixed delay/reverb send (whose own gates
    /// are never open while this one is closed) and nothing captures the stage. A compensation
    /// delay on the route then follows the gate instead of preceding it, which only moves a
    /// solo change by that delay. The strip meters before the gate (AudioMixAutomation).
    private func foldableGate(_ edge: AudioRouting.Edge, analysis: Analysis, assembly: Assembly) -> AudioMixAutomation? {
        guard foldsSoloGates, edge.id.hasPrefix("output:"), !analysis.captureAnchors.contains(edge.id),
              let slash = edge.source.lastIndex(of: "/"), edge.source == AudioRouting.nodeID(owner: String(edge.source[..<slash]), stage: .postFader),
              let strip = assembly.strips[String(edge.source[..<slash])], assembly.ports[edge.source]?.output === strip.mix.automation.node,
              (analysis.outgoing[edge.source] ?? []).allSatisfy({ $0.id == edge.id || $0.id.hasPrefix("builtin:") }) else { return nil }
        return strip.mix.automation
    }

    /// Insert ports. Runs of consecutive chainable inserts on one strip (no sidechain into
    /// them, no compensation delay between them) become one insert-chain node; the pure
    /// graph keeps one node per insert, so latency, PDC and automation offsets stay per insert.
    /// One strip's inserts in slot order: runs that share an insert-chain node, and inserts
    /// that render as their own node.
    fileprivate enum InsertSegment {
        case chain([Effect])
        case single(Effect)
    }

    /// Runs of consecutive chainable inserts on one strip (no sidechain into them, no
    /// compensation delay between them) share an insert-chain node; the pure graph keeps one
    /// node per insert, so latency, PDC and automation offsets stay per insert.
    private func insertSegments(_ effects: [Effect], analysis: Analysis) -> [String: [InsertSegment]] {
        let keyed = Set(analysis.enabledKeys.map(\.plugin.instanceID))
        func chainable(_ effect: Effect) -> Bool {
            makeInsertChain != nil && effect.chainMember != nil && !keyed.contains(effect.instanceID) && effect.mainInputBus == 0 &&
                isGraphStereo(effect.node.outputFormat(forBus: 0)) && isGraphStereo(effect.node.inputFormat(forBus: 0))
        }
        var result: [String: [InsertSegment]] = [:]
        for owner in Set(effects.map(\.owner)) {
            var segments: [InsertSegment] = [], run: [Effect] = []
            for effect in effects.filter({ $0.owner == owner }).sorted(by: { MixerChannel.chainOrder($0.slot) < MixerChannel.chainOrder($1.slot) }) {
                if chainable(effect) {
                    if !run.isEmpty && (analysis.plan.edgeDelays["insert:" + effect.instanceID] ?? 0) != 0 { segments.append(.chain(run)); run = [] }
                    run.append(effect); continue
                }
                if !run.isEmpty { segments.append(.chain(run)); run = [] }
                segments.append(.single(effect))
            }
            if !run.isEmpty { segments.append(.chain(run)) }
            result[owner] = segments
        }
        return result
    }

    private func isGraphStereo(_ format: AVAudioFormat) -> Bool {
        format.channelCount == 2 && format.sampleRate == sampleRate && format.commonFormat == .pcmFormatFloat32 && !format.isInterleaved
    }

    /// Insert ports: one insert-chain node per run of chainable inserts, otherwise the insert's own node.
    private func buildEffects(_ effects: [Effect], analysis: Analysis, assembly: Assembly) throws {
        let segments = insertSegments(effects, analysis: analysis)
        for owner in segments.keys.sorted() {
            assembly.currentOwner = owner; defer { assembly.currentOwner = nil }
            for segment in segments[owner] ?? [] {
                switch segment {
                case .chain(let run):
                    guard let makeInsertChain else { continue }
                    let chain = try makeInsertChain()
                    try chain.setMembers(run.compactMap(\.chainMember))
                    assembly.helper(chain.node)
                    assignChain(chain, to: run, analysis: analysis, assembly: assembly)
                case .single(let effect):
                    let id = AudioRouting.pluginNodeID(instanceID: effect.instanceID)
                    assembly.pluginArrival[effect.instanceID] = analysis.plan.inputArrival[id] ?? 0
                    // A built-in effect's output is already the graph's stereo format.
                    let output = effect.chainMember != nil && isGraphStereo(effect.node.outputFormat(forBus: 0)) ? effect.node : canonicalOutput(effect.node, assembly)
                    assembly.ports[id] = Port(output: output, input: effect.node,
                        inputChannels: effect.node.inputFormat(forBus: AVAudioNodeBus(effect.mainInputBus)).channelCount,
                        mixer: false, defaultBus: AVAudioNodeBus(effect.mainInputBus))
                }
            }
        }
    }

    /// Ports of a chain's members: every member's pure node is the chain node; the edges
    /// between members have no physical connection.
    private func assignChain(_ chain: AudioInsertChain, to run: [Effect], analysis: Analysis, assembly: Assembly) {
        for (index, effect) in run.enumerated() {
            let id = AudioRouting.pluginNodeID(instanceID: effect.instanceID)
            assembly.pluginArrival[effect.instanceID] = analysis.plan.inputArrival[id] ?? 0
            assembly.ports[id] = Port(output: chain.node, input: chain.node, inputChannels: 2, mixer: false, defaultBus: 0)
            if index > 0 { assembly.collapsedEdges.insert("insert:" + effect.instanceID) }
            assembly.chains[effect.instanceID] = chain
        }
    }

    private func busConfigurations(_ effects: [Effect], analysis: Analysis) -> [BusConfiguration] {
        var busConfigurations: [BusConfiguration] = []
        for effect in effects {
            busConfigurations.append(BusConfiguration(effect: effect, bus: effect.mainInputBus, enabled: true, format: nil, auxiliary: false))
            // Disable every unused physical input, including an auxiliary whose
            // saved layout no longer matches. Leaving it enabled would allow an
            // old key connection to influence an unavailable saved route.
            for index in 0..<effect.node.auAudioUnit.inputBusses.count where index != effect.mainInputBus {
                let bus = analysis.actualBuses[effect.instanceID]?.first { $0.index == index }
                let enabled = bus != nil && analysis.enabledKeys.contains { $0.plugin.instanceID == effect.instanceID && $0.inputBus.index == index }
                busConfigurations.append(BusConfiguration(effect: effect, bus: index, enabled: enabled,
                    format: enabled ? AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: AVAudioChannelCount(bus!.channels)) : nil,
                    auxiliary: analysis.auxiliaryIndices[effect.instanceID]?.contains(index) ?? false))
            }
        }
        return busConfigurations
    }

    private func canonicalOutput(_ node: AVAudioNode, _ assembly: Assembly) -> AVAudioNode {
        let converter = assembly.mixer()
        assembly.connection(node, converter, format: node.outputFormat(forBus: 0))
        return converter
    }

    private func captureOutput(_ node: AVAudioNode, owner: String, stage: CaptureStage, analysis: Analysis,
                               assembly: Assembly, capacityFrames: Int) throws -> AVAudioNode {
        let key = StageKey(owner: owner, stage: stage)
        guard analysis.selectedCaptures.contains(key) else { return node }
        let capture = try AudioRoutingCapture(sampleRate: sampleRate, capacityFrames: capacityFrames)
        try capture.configureCapture(enabled: true, arrivalFrames: Int64(analysis.plan.arrival[key.nodeID] ?? 0))
        assembly.helper(capture.node); assembly.connection(node, capture.node, format: stereo)
        assembly.stageCaptures[key] = capture
        return capture.node
    }

    private func buildOwner(_ owner: AudioRouting.Owner, sources: [String: AVAudioNode], analysis: Analysis,
                            assembly: Assembly, capacityFrames: Int) throws {
        assembly.currentOwner = owner.id; defer { assembly.currentOwner = nil }
        let definition = analysis.definition, plan = analysis.plan
        let inputID = AudioRouting.nodeID(owner: owner.id, stage: .input), preID = AudioRouting.nodeID(owner: owner.id, stage: .preFader)
        // The pre-fader stage's only input is the input stage or the last insert (routes only
        // ever target input stages), so its mixer would be a pure per-quantum copy. Use that
        // stage's output instead, unless a capture or frozen print needs the stage. Insert
        // ports are built before the owners (see prepare).
        let preEdges = analysis.incoming[preID] ?? []
        var collapsePre = analysis.frozen[owner.id] == nil && preEdges.count == 1 && preEdges[0].id == "preFader:" + owner.id &&
            (plan.edgeDelays[preEdges[0].id] ?? 0) == 0 &&
            !analysis.selectedCaptures.contains(StageKey(owner: owner.id, stage: .preFader)) &&
            !analysis.selectedCaptures.contains(StageKey(owner: owner.id, stage: .preInserts))
        let insertOutput = collapsePre && preEdges[0].source != inputID ? assembly.ports[preEdges[0].source]?.output : nil
        if collapsePre && preEdges[0].source != inputID && insertOutput == nil { collapsePre = false }
        let replacement = analysis.frozen[owner.id]
        let inputEdges = analysis.incoming[inputID] ?? []
        let directSource: AVAudioNode? = collapsesTrackInputs && replacement == nil && owner.trackIndex != nil && inputEdges.count == 1 &&
            inputEdges[0].id == "native:" + owner.id && (plan.edgeDelays[inputEdges[0].id] ?? 0) == 0 &&
            !analysis.selectedCaptures.contains(StageKey(owner: owner.id, stage: .preInserts)) &&
            sources[owner.id].map { $0.outputFormat(forBus: 0).channelCount == 2 } == true ? sources[owner.id] : nil
        let input: AVAudioNode = directSource ?? assembly.mixer(), mix = try AudioMixAutomation(sampleRate: sampleRate)
        if directSource != nil { assembly.collapsedEdges.insert(inputEdges[0].id); assembly.collapsedInputs.insert(owner.id) }
        // AVAudioEngine's prepare() disconnects the output of a node fed only by a mixer with no inputs. A bus
        // nothing is routed to yet (a fresh return bus) would then never render its inserts: a plug-in that applies
        // parameter writes on its next render (NI Raum) ignored every edit, and later edits reconnect nothing.
        // Buses and master keep a silent source on their input mixer, so their strips stay in the pull.
        if owner.trackIndex == nil, replacement == nil {
            let silence = AVAudioSourceNode(format: stereo) { isSilence, _, _, output in
                isSilence.pointee = true
                for buffer in UnsafeMutableAudioBufferListPointer(output) { if let data = buffer.mData { memset(data, 0, Int(buffer.mDataByteSize)) } }
                return noErr
            }
            assembly.helper(silence); assembly.connection(silence, input, bus: assembly.nextBus(input), format: stereo)
        }
        let pre: AVAudioNode = collapsePre ? insertOutput ?? input : assembly.mixer()
        if collapsePre { assembly.collapsedEdges.insert(preEdges[0].id) }
        assembly.helper(mix.node); assembly.mixes.append(mix)
        let postID = AudioRouting.nodeID(owner: owner.id, stage: .postFader)
        let endpoint = AutomationEndpoint(mix, inputArrival: plan.inputArrival[postID] ?? 0)
        let inputOutput = try captureOutput(replacement?.preInserts ?? input, owner: owner.id, stage: .preInserts, analysis: analysis, assembly: assembly, capacityFrames: capacityFrames)
        let preOutput = collapsePre ? insertOutput ?? inputOutput : try captureOutput(replacement?.preFader ?? pre, owner: owner.id, stage: .preFader, analysis: analysis, assembly: assembly, capacityFrames: capacityFrames)
        let postOutput = try captureOutput(mix.node, owner: owner.id, stage: .postFader, analysis: analysis, assembly: assembly, capacityFrames: capacityFrames)
        let strip = Strip(owner: owner, input: input, preFader: pre, mix: endpoint, postFader: postOutput)
        let legacyEmbedded = definition.mixModel == .legacy && (owner.kind == .track || owner.kind == .master)
        strip.setMix(volume: legacyEmbedded ? 1 : Float(owner.volume), pan: legacyEmbedded ? 0 : Float(owner.pan),
                     muted: legacyEmbedded ? false : owner.mute)
        assembly.strips[owner.id] = strip
        assembly.ports[inputID] = Port(output: inputOutput, input: replacement == nil && directSource == nil ? input : nil, inputChannels: 2, mixer: directSource == nil)
        assembly.ports[preID] = Port(output: preOutput, input: replacement == nil && !collapsePre ? pre : nil, inputChannels: 2, mixer: !collapsePre)
        assembly.ports[postID] = Port(output: postOutput, input: mix.node, inputChannels: 2, mixer: false)
        assembly.capturePoints[inputID] = inputOutput; assembly.capturePoints[preID] = preOutput; assembly.capturePoints[postID] = postOutput
        if let source = sources[owner.id] {
            let output = source.outputFormat(forBus: 0).channelCount == 2 ? source : canonicalOutput(source, assembly)
            assembly.ports[AudioRouting.nodeID(owner: owner.id, stage: .source)] = Port(output: output, input: nil, inputChannels: 2, mixer: false)
        }
    }

    private func buildEdge(_ edge: AudioRouting.Edge, systemReturns: [SystemReturn], analysis: Analysis, assembly: Assembly) throws {
        assembly.currentEdge = edge.id; defer { assembly.currentEdge = nil }
        if assembly.collapsedEdges.contains(edge.id) { return }
        let plan = analysis.plan
        guard let source = assembly.ports[edge.source], let destination = assembly.ports[edge.destination], let input = destination.input else {
            throw GraphError.invalid("The validated routing stage has no physical audio port: \(edge.id).")
        }
        var previous = source.output
        let delayFrames = plan.edgeDelays[edge.id] ?? 0
        if delayFrames > 0 {
            let delay = try AudioDelayCompensator(sampleRate: sampleRate, maximumFrames: delayFrames)
            delay.frames = delayFrames; delay.reset(); assembly.delays.append(delay); assembly.helper(delay.node)
            assembly.connection(previous, delay.node, format: stereo); previous = delay.node
        }
        var gain: Float?, routeID: String?, key = false, builtin: (String, Int)?
        if let send = analysis.sendByEdge[edge.id] { gain = Float(send.gain); routeID = send.id }
        if let item = analysis.sidechainsByEdge[edge.id] { gain = Float(item.sidechain.gain); routeID = item.sidechain.id; key = true }
        for item in systemReturns {
            for owner in analysis.trackIDs where edge.id == builtinEdge(owner: owner, id: item.id) {
                gain = item.gains[owner] ?? 0; builtin = (owner, item.id == "delay" ? 4 : 5)
            }
        }
        var gainStage: AudioMixAutomation?
        if let gain {
            let mix = try AudioMixAutomation(sampleRate: sampleRate, eventCapacity: 4096)
            mix.setGain(gain); assembly.helper(mix.node); assembly.mixes.append(mix)
            assembly.connection(previous, mix.node, format: stereo); previous = mix.node
            let endpoint = AutomationEndpoint(mix, inputArrival: plan.inputArrival[edge.destination] ?? 0)
            if let routeID { if key { assembly.keyEndpoints[routeID] = endpoint } else { assembly.sendEndpoints[routeID] = endpoint } }
            if let builtin { assembly.builtinEndpoints[builtin.0, default: [:]][builtin.1] = endpoint }
            gainStage = mix
        }
        let audibleBoundary = edge.id.hasPrefix("output:") || edge.id.hasPrefix("send:") || edge.id.hasPrefix("builtin:")
        if analysis.captureAnchors.contains(edge.id) || audibleBoundary {
            // The solo gate is a separate gain that no scheduled user-send gain can
            // override. A route with a gain stage carries it there (one node instead
            // of two); others get a gate node. Its zero output still pulls the
            // capture/key source and preserves the shared graph render cache.
            let gate: AudioMixAutomation
            if let gainStage { gate = gainStage }
            else if let folded = foldableGate(edge, analysis: analysis, assembly: assembly) {
                // The source strip's own mix processor carries the gate: one node fewer per strip.
                gate = folded; assembly.foldedGates[edge.id] = folded
            } else {
                gate = try AudioMixAutomation(sampleRate: sampleRate, eventCapacity: 256)
                assembly.helper(gate.node); assembly.mixes.append(gate)
                assembly.connection(previous, gate.node, format: stereo); previous = gate.node
            }
            let soloed = analysis.soloEdges.map { $0.contains(edge.id) } ?? true
            gate.setSoloGain(analysis.captureAnchors.contains(edge.id) || !soloed ? 0 : 1)
            if audibleBoundary && !analysis.captureAnchors.contains(edge.id) { assembly.soloGates[edge.id] = gate }
        }
        let bus: AVAudioNodeBus, channels: AVAudioChannelCount, physicalInput: AVAudioNode
        if let sidechain = analysis.sidechainsByEdge[edge.id] {
            let key = InputKey(node: ObjectIdentifier(input), bus: AVAudioNodeBus(sidechain.inputBus.index))
            guard let sum = assembly.keyInputs[key] else { throw GraphError.invalid("The resolved sidechain has no input summing stage.") }
            bus = assembly.nextBus(sum); physicalInput = sum; channels = 2
        } else if destination.mixer {
            bus = assembly.nextBus(input); physicalInput = input; channels = 2
        } else { physicalInput = input; bus = destination.defaultBus; channels = destination.inputChannels }
        if channels == 1 {
            let converter = assembly.mixer(); assembly.connection(previous, converter, format: stereo); previous = converter
            assembly.connection(previous, physicalInput, bus: bus, format: AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: 1)!)
        } else { assembly.connection(previous, physicalInput, bus: bus, format: stereo) }
    }

    private func finish(_ analysis: Analysis, _ assembly: Assembly, effects: [Effect],
                        busConfigurations: [BusConfiguration]) throws -> Prepared {
        guard let master = assembly.strips["master"] else { throw GraphError.invalid("Routing has no master strip.") }
        // A normalized plan has exactly one destination per input bus. Validate this
        // before AVAudioEngine's nonthrowing macOS14 connection API can be invoked.
        let connections = try groupConnections(assembly.rawConnections)
        return Prepared(definition: analysis.definition, graph: analysis.graph, plan: analysis.plan, strips: assembly.strips,
            ownerOrder: analysis.definition.owners.map(\.id), sendEndpoints: assembly.sendEndpoints,
            sidechainEndpoints: assembly.keyEndpoints, builtinSendEndpoints: assembly.builtinEndpoints, returnEndpoints: assembly.returnEndpoints,
            pluginInputArrival: assembly.pluginArrival, masterOutput: master.postFader, capturePoints: assembly.capturePoints,
            stageCaptures: assembly.stageCaptures, frozenStages: analysis.frozen,
            engine: engine, sampleRate: sampleRate, connections: connections, helpers: assembly.helpers, effects: effects,
            delays: assembly.delays, busConfigurations: busConfigurations, mixes: assembly.mixes, soloGates: assembly.soloGates,
            captureAnchors: analysis.captureAnchors, assembly: assembly)
    }

    /// Builds and validates a candidate without attaching nodes or changing any
    /// live connection. Combined track sources already align native/AU instruments;
    /// their algorithmic arrival is supplied separately, never counted twice.
    /// For frozen tracks, the caller supplies the verified playbackProjection's
    /// definition/effects and omits its baked original sources, source latencies,
    /// inserts and incoming routes. External PCM nodes remain host-owned through
    /// rollback/retirement, just like existing sources and plug-ins.
    func prepare(definition: AudioRouting.Definition, sources: [String: AVAudioNode], effects: [Effect],
                 sourceLatencies: [String: Int] = [:], systemReturns: [SystemReturn] = [],
                 extraMasterSources: [Source] = [], captureOwners: Set<String> = [],
                 captureStages: Set<StageKey> = [],
                 frozenStages: [FrozenStages] = [],
                 captureCapacityFrames: Int = 131_072) throws -> Prepared {
        let analysis = try analyze(definition: definition, sources: sources, effects: effects, sourceLatencies: sourceLatencies,
            systemReturns: systemReturns, extraMasterSources: extraMasterSources, captureOwners: captureOwners,
            captureStages: captureStages, frozenStages: frozenStages)
        let assembly = Assembly()
        try buildEffects(effects, analysis: analysis, assembly: assembly)
        for owner in definition.owners {
            try buildOwner(owner, sources: sources, analysis: analysis, assembly: assembly, capacityFrames: captureCapacityFrames)
        }
        for item in systemReturns {
            let input = assembly.mixer(), processorOutput = canonicalOutput(item.processor, assembly)
            let mix = try AudioMixAutomation(sampleRate: sampleRate, eventCapacity: 4096)
            mix.setGain(item.returnGain); assembly.helper(mix.node); assembly.mixes.append(mix)
            let inputID = "system:\(item.id)/input", processorID = "system:\(item.id)/processor", outputID = "system:\(item.id)/output"
            assembly.ports[inputID] = Port(output: input, input: input, inputChannels: 2, mixer: true)
            assembly.ports[processorID] = Port(output: processorOutput, input: item.processor,
                                               inputChannels: item.processor.inputFormat(forBus: 0).channelCount, mixer: false)
            assembly.ports[outputID] = Port(output: mix.node, input: mix.node, inputChannels: 2, mixer: false)
            assembly.returnEndpoints[item.id] = AutomationEndpoint(mix, inputArrival: analysis.plan.inputArrival[outputID] ?? 0)
        }
        for source in extraMasterSources {
            let output = source.node.outputFormat(forBus: 0).channelCount == 2 ? source.node : canonicalOutput(source.node, assembly)
            assembly.ports["extra:\(source.id)"] = Port(output: output, input: nil, inputChannels: 2, mixer: false)
        }
        // Several routes may feed one detector bus. Sum them before the single
        // physical AU input connection; each edge retains its own gain and PDC.
        for item in analysis.enabledKeys {
            guard let effect = effects.first(where: { $0.instanceID == item.plugin.instanceID }) else { continue }
            let bus = AVAudioNodeBus(item.inputBus.index), key = InputKey(node: ObjectIdentifier(effect.node), bus: bus)
            if assembly.keyInputs[key] == nil {
                let sum = assembly.mixer(); assembly.keyInputs[key] = sum
                assembly.connection(sum, effect.node, bus: bus,
                    format: AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: AVAudioChannelCount(item.inputBus.channels))!)
            }
        }
        for edge in analysis.graph.edges.sorted(by: { $0.id < $1.id }) {
            try buildEdge(edge, systemReturns: systemReturns, analysis: analysis, assembly: assembly)
        }
        return try finish(analysis, assembly, effects: effects, busConfigurations: busConfigurations(effects, analysis: analysis))
    }

    /// Built-in inserts added, removed or reordered between the same physical neighbours only
    /// change which effects existing insert-chain nodes run: no node, connection, delay or
    /// timing changes. Such a change is applied in place while rendering (applyChainUpdate).
    struct ChainUpdate {
        let prepared: Prepared
        fileprivate let base: Prepared
        fileprivate let members: [(chain: AudioInsertChain, members: [AnyObject])]
        fileprivate let added: Set<String>
    }

    /// Why the last chain update could not be made in place (diagnostics).
    private(set) var lastChainRefusal: String?
    /// The graph and member lists an applied chain update replaced, until committed.
    private var chainUndo: (previous: Prepared, members: [(chain: AudioInsertChain, members: [AnyObject])])?

    func prepareChainUpdate(definition: AudioRouting.Definition, sources: [String: AVAudioNode], effects: [Effect],
                            sourceLatencies: [String: Int] = [:], systemReturns: [SystemReturn] = [],
                            extraMasterSources: [Source] = [], captureOwners: Set<String> = []) throws -> ChainUpdate? {
        func refuse(_ reason: String) -> ChainUpdate? { lastChainRefusal = reason; return nil }
        lastChainRefusal = nil
        if let settled = pending, engine.isRunning {
            if let previous = settled.previous { retiredHelpers += previous.helpers }
            pending = nil
        }
        guard pending == nil else { return refuse("a previous graph is not committed yet") }
        guard makeInsertChain != nil, let current, captureOwners.isEmpty, current.stageCaptures.isEmpty, current.frozenStages.isEmpty,
              current.definition.mixModel == definition.mixModel else { return refuse("captures, frozen stages or the mix model changed") }
        guard definition.owners.map({ "\($0.id)/\($0.kind.rawValue)/\($0.trackIndex ?? -1)" }) ==
                current.definition.owners.map({ "\($0.id)/\($0.kind.rawValue)/\($0.trackIndex ?? -1)" }) else { return refuse("owners changed") }
        for (owner, source) in sources {
            guard current.assembly.ports[AudioRouting.nodeID(owner: owner, stage: .source)]?.output === source else { return refuse("a track source changed") }
        }
        let analysis = try analyze(definition: definition, sources: sources, effects: effects, sourceLatencies: sourceLatencies,
            systemReturns: systemReturns, extraMasterSources: extraMasterSources, captureOwners: [], captureStages: [], frozenStages: [])
        guard analysis.plan.arrival.filter({ !$0.key.hasPrefix("plugin:") }) == current.plan.arrival.filter({ !$0.key.hasPrefix("plugin:") }),
              analysis.plan.inputArrival.filter({ !$0.key.hasPrefix("plugin:") }) == current.plan.inputArrival.filter({ !$0.key.hasPrefix("plugin:") })
        else { return refuse("compensation timing changed") }
        // The installed segments, read from the chains the installed graph built.
        var old: [String: [InsertSegment]] = [:]
        for owner in Set(current.effects.map(\.owner)) {
            var segments: [InsertSegment] = []
            for effect in current.effects.filter({ $0.owner == owner }).sorted(by: { MixerChannel.chainOrder($0.slot) < MixerChannel.chainOrder($1.slot) }) {
                if let chain = current.assembly.chains[effect.instanceID] {
                    if case .chain(let run)? = segments.last, let first = run.first, current.assembly.chains[first.instanceID] === chain {
                        segments[segments.count - 1] = .chain(run + [effect]); continue
                    }
                    segments.append(.chain([effect]))
                } else { segments.append(.single(effect)) }
            }
            old[owner] = segments
        }
        let new = insertSegments(effects, analysis: analysis)
        // Same physical shape: the same single inserts (same nodes) and chains at the same places.
        var runNames: [String: String] = [:], newRunNames: [String: String] = [:], internalOld = Set<String>(), internalNew = Set<String>()
        var pairs: [(chain: AudioInsertChain, old: [Effect], new: [Effect])] = []
        for owner in Set(old.keys).union(new.keys) {
            let before = old[owner] ?? [], after = new[owner] ?? []
            guard before.count == after.count else { return refuse("an insert chain was created or removed") }
            for (index, (a, b)) in zip(before, after).enumerated() {
                switch (a, b) {
                case (.single(let x), .single(let y)):
                    guard x.instanceID == y.instanceID, x.node === y.node else { return refuse("a hosted insert changed") }
                case (.chain(let x), .chain(let y)):
                    guard let chain = current.assembly.chains[x[0].instanceID] else { return refuse("an installed chain is missing") }
                    let name = "run:\(owner)#\(index)"
                    for effect in x { runNames[AudioRouting.pluginNodeID(instanceID: effect.instanceID)] = name }
                    for effect in y { newRunNames[AudioRouting.pluginNodeID(instanceID: effect.instanceID)] = name }
                    internalOld.formUnion(x.dropFirst().map { "insert:" + $0.instanceID })
                    internalNew.formUnion(y.dropFirst().map { "insert:" + $0.instanceID })
                    runNames["insert:" + x[0].instanceID] = "into:" + name; newRunNames["insert:" + y[0].instanceID] = "into:" + name
                    pairs.append((chain, x, y))
                default: return refuse("an insert moved between a chain and its own node")
                }
            }
        }
        // Every other route is the same, with the same compensation delay.
        func normalized(_ graph: AudioRouting.Graph, _ plan: AudioRouting.Plan, names: [String: String], hidden: Set<String>) -> Set<String> {
            Set(graph.edges.filter { !hidden.contains($0.id) }.map { edge in
                [names[edge.id] ?? edge.id, names[edge.source] ?? edge.source, names[edge.destination] ?? edge.destination,
                 String(plan.edgeDelays[edge.id] ?? 0)].joined(separator: "|")
            })
        }
        guard normalized(current.graph, current.plan, names: runNames, hidden: internalOld) ==
                normalized(analysis.graph, analysis.plan, names: newRunNames, hidden: internalNew) else { return refuse("a route changed") }
        guard current.latencyFrames == (analysis.plan.arrival[AudioRouting.nodeID(owner: "master", stage: .postFader)] ?? 0) else { return refuse("the master latency changed") }
        let assembly = current.assembly.copy()
        for pair in pairs {
            for effect in pair.old {
                assembly.ports.removeValue(forKey: AudioRouting.pluginNodeID(instanceID: effect.instanceID))
                assembly.chains.removeValue(forKey: effect.instanceID); assembly.pluginArrival.removeValue(forKey: effect.instanceID)
                assembly.collapsedEdges.remove("insert:" + effect.instanceID)
            }
            // The physical connection into the chain now belongs to its new first member's edge.
            let oldEntry = "insert:" + pair.old[0].instanceID, newEntry = "insert:" + pair.new[0].instanceID
            if oldEntry != newEntry, let connections = assembly.edgeConnections.removeValue(forKey: oldEntry) { assembly.edgeConnections[newEntry] = connections }
            assignChain(pair.chain, to: pair.new, analysis: analysis, assembly: assembly)
        }
        for effect in effects where assembly.chains[effect.instanceID] == nil {
            assembly.pluginArrival[effect.instanceID] = analysis.plan.inputArrival[AudioRouting.pluginNodeID(instanceID: effect.instanceID)] ?? 0
        }
        let prepared = try finish(analysis, assembly, effects: effects, busConfigurations: busConfigurations(effects, analysis: analysis))
        let installed = Set(current.effects.map(\.instanceID))
        return ChainUpdate(prepared: prepared, base: current, members: pairs.map { ($0.chain, $0.new.compactMap(\.chainMember)) },
                           added: Set(effects.map(\.instanceID)).subtracting(installed))
    }

    /// Applies a chain update while the graph may be rendering: new members' unused inputs
    /// are disabled, every insert's presentation latency follows the plan, and each chain
    /// takes its new member list at its next render. rollbackChainUpdate restores the
    /// previous lists until the next commit.
    func applyChainUpdate(_ update: ChainUpdate) throws {
        precondition(Thread.isMainThread)
        guard pending == nil, current === update.base else { throw GraphError.invalid("The insert chain update no longer matches the installed graph.") }
        let candidate = update.prepared
        for configuration in candidate.busConfigurations where update.added.contains(configuration.effect.instanceID) {
            let unit = configuration.effect.node.auAudioUnit
            guard configuration.bus < unit.inputBusses.count else { continue }
            if configuration.auxiliary { try configuration.effect.configureAuxiliary?(configuration.bus, configuration.enabled) }
            unit.inputBusses[configuration.bus].isEnabled = configuration.enabled
        }
        // Only chained (in-process) inserts move; hosted inserts keep their timing (checked above).
        for effect in candidate.effects where candidate.assembly.chains[effect.instanceID] != nil {
            let unit = effect.node.auAudioUnit
            let inputArrival = candidate.pluginInputArrival[effect.instanceID] ?? 0
            let outputArrival = candidate.plan.arrival[AudioRouting.pluginNodeID(instanceID: effect.instanceID)] ?? inputArrival
            for index in 0..<unit.inputBusses.count { unit.inputBusses[index].contextPresentationLatency = Double(inputArrival) / sampleRate }
            for index in 0..<unit.outputBusses.count {
                unit.outputBusses[index].contextPresentationLatency = Double(max(0, candidate.latencyFrames - outputArrival)) / sampleRate
            }
        }
        let previous = update.members.map { ($0.chain, $0.chain.members) }
        for item in update.members { try item.chain.setMembers(item.members) }
        chainUndo = (update.base, previous)
        current = candidate
    }

    /// Restores the member lists and graph an applied chain update replaced.
    func rollbackChainUpdate() throws {
        guard let undo = chainUndo else { return }
        for item in undo.members { try item.chain.setMembers(item.members) }
        current = undo.previous; chainUndo = nil
    }

    /// Ends the rollback window of an applied chain update; retired member lists are released
    /// once no render can still use them.
    func commitChainUpdate() {
        chainUndo = nil
        var seen = Set<ObjectIdentifier>()
        for chain in current?.assembly.chains.values ?? [:].values where seen.insert(ObjectIdentifier(chain)).inserted { chain.collectRetired() }
    }

    /// A live, gap-free change of the installed graph: new owner strips and their
    /// routes are attached and wired while the engine keeps rendering, removed owners
    /// are disconnected (and detached at the next quiescent commit).
    struct IncrementalChange {
        let prepared: Prepared
        fileprivate let attach: [AVAudioNode]
        fileprivate let connections: [Connection]
        fileprivate let removedHelpers: [AVAudioNode]
        fileprivate let removedSources: [AVAudioNode]
        /// Strip mix processors that stop being a removed route's solo gate.
        fileprivate let releasedGates: [AudioMixAutomation]
        let addedOwners: [String]
        let removedOwners: [String]
        /// Kept tracks whose channel (source) moved.
        let rewiredOwners: [String]
    }

    /// Prepares an incremental change from the installed graph when the only
    /// difference is owners added or removed (tracks, Sounds, buses) together with
    /// their own routes, and every remaining node keeps its exact timing. Anything
    /// else (an existing route, insert, return, latency, capture or frozen stage
    /// changing) returns nil: the caller then installs a full graph while stopped.
    func prepareIncremental(definition: AudioRouting.Definition, sources: [String: AVAudioNode], effects: [Effect],
                            sourceLatencies: [String: Int] = [:], systemReturns: [SystemReturn] = [],
                            extraMasterSources: [Source] = [], captureOwners: Set<String> = []) throws -> IncrementalChange? {
        func refuse(_ reason: String) -> IncrementalChange? { lastIncrementalRefusal = reason; return nil }
        lastIncrementalRefusal = nil
        // A full install made while running stays pending (its previous graph kept for
        // rollback) until a stopped commit. Once the engine runs it again, that install
        // stands: the previous graph's helpers retire, and live changes may follow.
        if let settled = pending, engine.isRunning {
            if let previous = settled.previous { retiredHelpers += previous.helpers }
            pending = nil
        }
        guard pending == nil else { return refuse("a previous graph is not committed yet") }
        guard let current, captureOwners.isEmpty, current.stageCaptures.isEmpty, current.frozenStages.isEmpty,
              current.definition.mixModel == definition.mixModel else { return refuse("captures, frozen stages or the mix model changed") }
        guard Set(current.effects.map(\.instanceID)) == Set(effects.map(\.instanceID)),
              current.effects.allSatisfy({ old in effects.contains { $0.instanceID == old.instanceID && $0.node === old.node && $0.owner == old.owner && $0.slot == old.slot } })
        else { return refuse("inserts changed") }
        let analysis = try analyze(definition: definition, sources: sources, effects: effects, sourceLatencies: sourceLatencies,
            systemReturns: systemReturns, extraMasterSources: extraMasterSources, captureOwners: [],
            captureStages: [], frozenStages: [])
        let oldOwners = Set(current.definition.owners.map(\.id)), newOwners = Set(definition.owners.map(\.id))
        let added = definition.owners.filter { !oldOwners.contains($0.id) }
        let removed = current.definition.owners.filter { !newOwners.contains($0.id) }.map(\.id)
        let sendRoutesChanged = Set(current.graph.edges.map(\.id).filter { $0.hasPrefix("builtin:") }) !=
            Set(analysis.graph.edges.map(\.id).filter { $0.hasPrefix("builtin:") })
        guard !added.isEmpty || !removed.isEmpty || sendRoutesChanged || definition.owners.contains(where: { item in
            item.trackIndex != nil && current.definition.owner(item.id)?.trackIndex != item.trackIndex }) else { return refuse("no owner was added or removed") }
        let addedIDs = Set(added.map(\.id)), removedIDs = Set(removed)
        func owner(of node: String) -> String? {
            guard let slash = node.lastIndex(of: "/") else { return nil }
            return String(node[..<slash])
        }
        // Kept owners keep their strip and kind. A track whose channel moved (an earlier
        // track was removed) keeps its strip and only takes its new channel's source.
        var rewired: [String] = []
        for item in definition.owners where oldOwners.contains(item.id) {
            guard let old = current.definition.owner(item.id), old.kind == item.kind, (old.trackIndex == nil) == (item.trackIndex == nil) else { return refuse("an owner changed kind") }
            if item.trackIndex != nil {
                guard let source = sources[item.id], source.outputFormat(forBus: 0).channelCount == 2,
                      let port = current.assembly.ports[AudioRouting.nodeID(owner: item.id, stage: .source)] else { return refuse("a kept track has no stereo source") }
                if port.output !== source {
                    // A direct native source and a combined input mixer are wired differently by the host.
                    guard type(of: port.output) == type(of: source) else { return refuse("a kept track's source changed kind") }
                    rewired.append(item.id)
                }
            }
        }
        let oldEdges = Dictionary(uniqueKeysWithValues: current.graph.edges.map { ($0.id, $0) })
        let newEdges = Dictionary(uniqueKeysWithValues: analysis.graph.edges.map { ($0.id, $0) })
        func touchesAdded(_ edge: AudioRouting.Edge) -> Bool {
            [edge.source, edge.destination].contains { owner(of: $0).map(addedIDs.contains) ?? false }
        }
        func touchesRemoved(_ edge: AudioRouting.Edge) -> Bool {
            [edge.source, edge.destination].contains { owner(of: $0).map(removedIDs.contains) ?? false }
        }
        // A rewired track's source connection is rebuilt; its strip and routes stay.
        var rebuilt = Set(rewired.map { "native:" + $0 })
        // A rewired track whose input stage is its source node: the routes leaving that node
        // (its first insert, or the fader when the pre-fader stage collapsed onto it) are
        // rebuilt from the new source. Taps leaving the stage would need their own rebuild.
        for id in rewired where current.assembly.collapsedInputs.contains(id) {
            let inputID = AudioRouting.nodeID(owner: id, stage: .input), preID = AudioRouting.nodeID(owner: id, stage: .preFader)
            let preOnInput = current.assembly.ports[preID]?.output === current.assembly.ports[inputID]?.output
            let leaving = current.graph.edges.filter { $0.source == inputID || (preOnInput && $0.source == preID) }
            guard leaving.allSatisfy({ $0.id == "fader:" + id || $0.id == "preFader:" + id || $0.id.hasPrefix("insert:") }) else {
                return refuse("a moved track's direct source has taps")
            }
            rebuilt.formUnion(leaving.map(\.id).filter { !current.assembly.collapsedEdges.contains($0) })
        }
        for (id, edge) in newEdges where !rebuilt.contains(id) {
            if let old = oldEdges[id] {
                // A kept route must keep its endpoints, delay and gain stage.
                guard old == edge, (current.plan.edgeDelays[id] ?? 0) == (analysis.plan.edgeDelays[id] ?? 0),
                      (current.definition.sends.contains { "send:" + $0.id == id }) == (analysis.sendByEdge[id] != nil) else { return refuse("an existing route changed") }
            } else if !touchesAdded(edge) && !id.hasPrefix("builtin:") { return refuse("a new route connects existing owners") }
            // AVAudioEngine does not render an input added later to a chain that joined empty.
            if oldEdges[id] == nil, id.hasPrefix("builtin:"), !current.graph.edges.contains(where: { $0.destination == edge.destination }) {
                return refuse("a delay/reverb return gains its first send")
            }
            // Sidechains and send routes only change together with a full rebuild.
            if oldEdges[id] == nil, id.hasPrefix("sidechain:") { return refuse("a sidechain was added") }
            // A stage whose output gate is its own mix processor gains no other audible route live.
            if oldEdges[id] == nil, !id.hasPrefix("builtin:"),
               current.assembly.foldedGates.keys.contains(where: { oldEdges[$0]?.source == edge.source }) { return refuse("a route was added to a stage with a folded solo gate") }
        }
        for (id, edge) in oldEdges where newEdges[id] == nil {
            // A kept track's delay/reverb send route may come and go with its send usage.
            guard touchesRemoved(edge) || id.hasPrefix("builtin:") else { return refuse("a route of a kept owner was removed") }
        }
        // Every remaining node keeps its timing, so no existing delay or automation offset moves.
        for node in current.plan.arrival.keys where !(owner(of: node).map(removedIDs.contains) ?? false) {
            guard current.plan.arrival[node] == analysis.plan.arrival[node],
                  current.plan.inputArrival[node] == analysis.plan.inputArrival[node] else { return refuse("an existing node's timing changed") }
        }
        guard current.latencyFrames == (analysis.plan.arrival[AudioRouting.nodeID(owner: "master", stage: .postFader)] ?? 0) else { return refuse("the master latency changed") }

        let assembly = current.assembly.copy()
        // Forget what the removed owners and routes built.
        var removedHelpers: [AVAudioNode] = [], removedSources: [AVAudioNode] = []
        for id in removedIDs {
            removedHelpers += assembly.ownerHelpers.removeValue(forKey: id) ?? []
            assembly.strips.removeValue(forKey: id)
            if let source = assembly.ports[AudioRouting.nodeID(owner: id, stage: .source)]?.output { removedSources.append(source) }
            for stage in [AudioRouting.Stage.input, .preFader, .postFader, .source] {
                let key = AudioRouting.nodeID(owner: id, stage: stage)
                assembly.ports.removeValue(forKey: key); assembly.capturePoints.removeValue(forKey: key)
            }
            assembly.builtinEndpoints.removeValue(forKey: id)
        }
        var releasedGates: [AudioMixAutomation] = []
        for id in oldEdges.keys where newEdges[id] == nil || rebuilt.contains(id) {
            if newEdges[id] == nil { assembly.collapsedEdges.remove(id) }
            if let gate = assembly.foldedGates.removeValue(forKey: id), let source = oldEdges[id]?.source,
               let slash = source.lastIndex(of: "/"), !removedIDs.contains(String(source[..<slash])) { releasedGates.append(gate) }
            removedHelpers += assembly.edgeHelpers.removeValue(forKey: id) ?? []
            assembly.edgeConnections.removeValue(forKey: id)
            assembly.soloGates.removeValue(forKey: id)
            if newEdges[id] == nil, id.hasPrefix("builtin:") {
                for item in systemReturns {
                    for (owner, targets) in assembly.builtinEndpoints where id == builtinEdge(owner: owner, id: item.id) {
                        var remaining = targets; remaining.removeValue(forKey: item.id == "delay" ? 4 : 5)
                        assembly.builtinEndpoints[owner] = remaining.isEmpty ? nil : remaining
                    }
                }
            }
        }
        // Rewired tracks: drop the old source connection, point the port at the new source.
        var previousSources: [AVAudioNode] = []
        for id in rewired {
            let key = AudioRouting.nodeID(owner: id, stage: .source)
            guard let old = assembly.ports[key], let source = sources[id] else { continue }
            previousSources.append(old.output)
            assembly.ports[key] = Port(output: source, input: nil, inputChannels: 2, mixer: false)
            if assembly.collapsedInputs.contains(id) {
                // The stage (and a pre-fader stage collapsed onto it) is the new source.
                for stage in [AudioRouting.Stage.input, .preFader] {
                    let stageID = AudioRouting.nodeID(owner: id, stage: stage)
                    guard let port = assembly.ports[stageID], port.output === old.output else { continue }
                    assembly.ports[stageID] = Port(output: source, input: nil, inputChannels: 2, mixer: false)
                    assembly.capturePoints[stageID] = source
                }
            }
            if let strip = assembly.strips[id], let owner = definition.owner(id) {
                let collapsed = assembly.collapsedInputs.contains(id)
                assembly.strips[id] = Strip(owner: owner, input: collapsed ? source : strip.input,
                    preFader: collapsed && strip.preFader === old.output ? source : strip.preFader, mix: strip.mix, postFader: strip.postFader)
            }
        }
        let keptSources = Set(sources.values.map(ObjectIdentifier.init))
        removedSources = (removedSources + previousSources).filter { !keptSources.contains(ObjectIdentifier($0)) }
        let rewiredSources = Set(previousSources.map(ObjectIdentifier.init))
        // Every connection a rewired source made belonged to its old track's strip (rebuilt below).
        assembly.rawConnections.removeAll { rewiredSources.contains(ObjectIdentifier($0.source)) }
        let removedSet = Set(removedHelpers.map(ObjectIdentifier.init)), removedSourceSet = Set(removedSources.map(ObjectIdentifier.init))
        assembly.helpers.removeAll { removedSet.contains(ObjectIdentifier($0)) }
        assembly.rawConnections.removeAll { connection in
            removedSet.contains(ObjectIdentifier(connection.source)) || removedSourceSet.contains(ObjectIdentifier(connection.source))
                || connection.destinations.contains { removedSet.contains(ObjectIdentifier($0.node)) }
        }
        let removedMixes = Set(removedHelpers.map(ObjectIdentifier.init))
        assembly.mixes.removeAll { removedMixes.contains(ObjectIdentifier($0.node)) }
        assembly.delays.removeAll { removedMixes.contains(ObjectIdentifier($0.node)) }
        // Build the added owners and every new route.
        let firstHelper = assembly.helpers.count, firstConnection = assembly.rawConnections.count
        for item in added {
            try buildOwner(item, sources: sources, analysis: analysis, assembly: assembly, capacityFrames: 131_072)
        }
        for edge in analysis.graph.edges.sorted(by: { $0.id < $1.id }) where oldEdges[edge.id] == nil || rebuilt.contains(edge.id) {
            try buildEdge(edge, systemReturns: systemReturns, analysis: analysis, assembly: assembly)
        }
        // Existing strips keep their mix endpoints; their solo gates follow the new definition later (updateSolo).
        let prepared = try finish(analysis, assembly, effects: current.effects, busConfigurations: current.busConfigurations)
        return IncrementalChange(prepared: prepared, attach: Array(assembly.helpers[firstHelper...]),
            connections: try groupConnections(Array(assembly.rawConnections[firstConnection...])),
            removedHelpers: removedHelpers, removedSources: removedSources, releasedGates: releasedGates,
            addedOwners: added.map(\.id), removedOwners: removed, rewiredOwners: rewired)
    }

    /// After AVAudioEngine refused part of an incremental change: its new nodes are retired
    /// (detached by the next stopped commit); the installed graph stays the previous one.
    func abandonIncremental(_ change: IncrementalChange) {
        for node in change.attach where node.engine === engine {
            engine.disconnectNodeOutput(node)
        }
        retiredHelpers += change.attach
    }

    /// Applies an incremental change while the engine may be rendering: AVAudioEngine
    /// accepts attaching nodes and connecting fresh mixer inputs live. Removed nodes are
    /// only disconnected here; they are detached by the next quiescent commit or install.
    func installIncremental(_ change: IncrementalChange) throws {
        precondition(Thread.isMainThread)
        guard pending == nil, current != nil, change.prepared.engineIdentity == ObjectIdentifier(engine) else {
            throw GraphError.invalid("The incremental routing change no longer matches the installed graph.")
        }
        let nodes = uniqueNodes(change.attach + change.connections.flatMap { [$0.source] + $0.destinations.map(\.node) })
        for node in nodes where node.engine == nil { engine.attach(node) }
        let added = Set(change.attach.map(ObjectIdentifier.init))
        // An added track's source drops its dormant connection; any other existing
        // node keeps its current destinations and gains the new branch.
        let replaced = Set((change.addedOwners + change.rewiredOwners).compactMap { change.prepared.assembly.ports[AudioRouting.nodeID(owner: $0, stage: .source)].map { ObjectIdentifier($0.output) } })
        var internalConnections: [Connection] = [], entryConnections: [Connection] = []
        for connection in change.connections {
            if added.contains(ObjectIdentifier(connection.source)) { internalConnections.append(connection); continue }
            var destinations = connection.destinations
            if !replaced.contains(ObjectIdentifier(connection.source)) {
                destinations += engine.outputConnectionPoints(for: connection.source, outputBus: connection.bus).compactMap { point in
                    point.node.map { Destination(node: $0, bus: point.bus) }
                }
            }
            entryConnections.append(Connection(source: connection.source, bus: connection.bus, destinations: destinations, format: connection.format))
        }
        // Build the new branches completely, then join them to the live graph: AVAudioEngine
        // renders a branch added while running only if it is complete when it joins.
        let all = internalConnections + entryConnections
        let joins = all.filter { connection in connection.destinations.contains { !added.contains(ObjectIdentifier($0.node)) } }
        connect(all.filter { connection in connection.destinations.allSatisfy { added.contains(ObjectIdentifier($0.node)) } })
        // A change AVAudioEngine still refuses falls back to a full rebuild
        // (AudioPluginRack.applyIncrementalRouting).
        connect(joins)
        for node in change.removedSources where node.engine === engine { engine.disconnectNodeOutput(node) }
        for node in change.removedHelpers where node.engine === engine {
            engine.disconnectNodeOutput(node); engine.disconnectNodeInput(node)
        }
        retiredHelpers += change.removedHelpers
        for gate in change.releasedGates { gate.setSoloGain(1) }
        current = change.prepared; chainUndo = nil
    }
    /// Retains one prior graph and complete fan-outs until commit. If subsequent
    /// engine.start fails, the caller stops/drains again and calls rollback(). A new
    /// installation first commits the previous successful change, bounding retention.
    func install(_ candidate: Prepared, validate: (() throws -> Void)? = nil) throws {
        try requireStopped()
        guard candidate.engineIdentity == ObjectIdentifier(engine), candidate !== current else {
            throw GraphError.invalid("The routing candidate belongs to another engine or is already installed.")
        }
        try commit()
        chainUndo = nil
        let bus = sinkBus ?? engine.mainMixerNode.nextAvailableInputBus
        sinkBus = bus
        let terminal = Connection(source: candidate.masterOutput, bus: 0,
            destinations: [Destination(node: engine.mainMixerNode, bus: bus)], format: stereo)
        let connections = candidate.connections + [terminal]
        let oldConnections = current.map { $0.connections + [Connection(source: $0.masterOutput, bus: 0,
            destinations: [Destination(node: engine.mainMixerNode, bus: bus)], format: stereo)] } ?? []
        let saved = captureConnections(changing: connections + oldConnections)
        var states: [BusSnapshot] = [], presentation: [PresentationSnapshot] = [], attached: [AVAudioNode] = []
        let previous = current
        do {
            var allNodes = candidate.helpers + connections.flatMap { [$0.source] + $0.destinations.map(\.node) }
            allNodes = uniqueNodes(allNodes)
            for node in allNodes where node.engine == nil { engine.attach(node); attached.append(node) }
            for configuration in candidate.busConfigurations {
                let unit = configuration.effect.node.auAudioUnit
                guard configuration.bus < unit.inputBusses.count else { throw GraphError.invalid("An auxiliary bus disappeared before routing commit.") }
                let input = unit.inputBusses[configuration.bus]
                states.append(BusSnapshot(configuration: configuration, enabled: input.isEnabled, format: input.format))
                if configuration.auxiliary { try configuration.effect.configureAuxiliary?(configuration.bus, configuration.enabled) }
                if let format = configuration.format, input.format != format { try input.setFormat(format) }
                input.isEnabled = configuration.enabled
            }
            for effect in candidate.effects {
                let unit = effect.node.auAudioUnit
                let inputArrival = candidate.pluginInputArrival[effect.instanceID] ?? 0
                let outputArrival = candidate.plan.arrival[AudioRouting.pluginNodeID(instanceID: effect.instanceID)] ?? inputArrival
                for index in 0..<unit.inputBusses.count {
                    let input = unit.inputBusses[index]
                    presentation.append(PresentationSnapshot(bus: input, latency: input.contextPresentationLatency))
                    input.contextPresentationLatency = Double(inputArrival) / sampleRate
                }
                for index in 0..<unit.outputBusses.count {
                    let output = unit.outputBusses[index]
                    presentation.append(PresentationSnapshot(bus: output, latency: output.contextPresentationLatency))
                    output.contextPresentationLatency = Double(max(0, candidate.latencyFrames - outputArrival)) / sampleRate
                }
            }
            disconnectOutputs(connections + oldConnections)
            connect(connections)
            current = candidate
            pending = PendingChange(previous: previous, installed: candidate, connections: saved,
                busStates: states, presentation: presentation, newlyAttached: attached)
            try validate?()
        } catch {
            // The caller's validation must stop and drain any test render before
            // throwing. We do not guess that pause/stop itself drains callbacks.
            guard !engine.isRunning else {
                throw GraphError.invalid("Routing validation failed while rendering was still running. Stop and drain the engine, then roll back: \(error.localizedDescription)")
            }
            if pending == nil {
                pending = PendingChange(previous: previous, installed: candidate, connections: saved,
                    busStates: states, presentation: presentation, newlyAttached: attached)
            }
            let original = error
            do { try rollback() }
            catch { throw GraphError.invalid("Routing failed (\(original.localizedDescription)); rollback also failed (\(error.localizedDescription)).") }
            throw original
        }
    }

    func commit() throws {
        try requireStopped()
        detachHelpers(retiredHelpers); retiredHelpers.removeAll()
        guard let pending else { return }
        if let previous = pending.previous { detachHelpers(previous.helpers) }
        self.pending = nil
    }

    func rollback() throws {
        try requireStopped()
        guard let pending else { return }
        let candidate = pending.installed
        disconnectOutputs(candidate.connections + [Connection(source: candidate.masterOutput, bus: 0,
            destinations: [], format: stereo)])
        for state in pending.busStates.reversed() {
            let configuration = state.configuration, unit = configuration.effect.node.auAudioUnit
            guard configuration.bus < unit.inputBusses.count else { throw GraphError.invalid("A bus disappeared during routing rollback.") }
            if configuration.auxiliary { try configuration.effect.configureAuxiliary?(configuration.bus, state.enabled) }
            let bus = unit.inputBusses[configuration.bus]
            if bus.format != state.format { try bus.setFormat(state.format) }
            bus.isEnabled = state.enabled
        }
        for item in pending.presentation { item.bus.contextPresentationLatency = item.latency }
        detachHelpers(candidate.helpers)
        let helperIDs = Set(candidate.helpers.map(ObjectIdentifier.init))
        for node in pending.newlyAttached where !helperIDs.contains(ObjectIdentifier(node)) && node.engine === engine {
            engine.disconnectNodeInput(node); engine.disconnectNodeOutput(node); engine.detach(node)
        }
        connect(pending.connections)
        current = pending.previous; self.pending = nil
    }

    func uninstall() throws {
        try requireStopped()
        try commit()
        guard let current else { return }
        disconnectOutputs(current.connections + [Connection(source: current.masterOutput, bus: 0, destinations: [], format: stereo)])
        detachHelpers(current.helpers)
        self.current = nil
    }

    private func requireStopped() throws {
        precondition(Thread.isMainThread)
        guard !engine.isRunning else { throw GraphError.invalid("Stop and drain audio rendering before changing routing.") }
    }
    private func validateFormat(_ format: AVAudioFormat, label: String) throws {
        guard format.sampleRate == sampleRate, (1...2).contains(format.channelCount),
              format.commonFormat == .pcmFormatFloat32, !format.isInterleaved else {
            throw GraphError.invalid("\(label) must be mono or stereo float32 at \(Int(sampleRate)) Hz.")
        }
    }
    private func builtinEdge(owner: String, id: String) -> String { "builtin:\(id):\(owner)" }
    private func uniqueNodes(_ nodes: [AVAudioNode]) -> [AVAudioNode] {
        var seen = Set<ObjectIdentifier>()
        return nodes.filter { seen.insert(ObjectIdentifier($0)).inserted }
    }
    private func groupConnections(_ raw: [Connection]) throws -> [Connection] {
        var order: [OutputKey] = [], groups: [OutputKey: Connection] = [:], inputs = Set<InputKey>()
        for connection in raw {
            let key = OutputKey(node: ObjectIdentifier(connection.source), bus: connection.bus)
            for destination in connection.destinations {
                guard inputs.insert(InputKey(node: ObjectIdentifier(destination.node), bus: destination.bus)).inserted else {
                    throw GraphError.invalid("More than one connection targets the same physical input bus.")
                }
            }
            if let previous = groups[key] {
                guard previous.format == connection.format else {
                    throw GraphError.invalid("Fan-out branches require one source format; channel conversion belongs on each branch.")
                }
                groups[key] = Connection(source: connection.source, bus: connection.bus,
                    destinations: previous.destinations + connection.destinations, format: connection.format)
            } else { groups[key] = connection; order.append(key) }
        }
        return order.compactMap { groups[$0] }
    }
    private func captureConnections(changing connections: [Connection]) -> [Connection] {
        var sources: [(AVAudioNode, AVAudioNodeBus)] = connections.map { ($0.source, $0.bus) }
        for connection in connections {
            for destination in connection.destinations where destination.node.engine === engine && destination.bus < destination.node.numberOfInputs {
                if let parent = engine.inputConnectionPoint(for: destination.node, inputBus: destination.bus), let node = parent.node {
                    sources.append((node, parent.bus))
                }
            }
        }
        var seen = Set<OutputKey>(), result: [Connection] = []
        for (node, bus) in sources where node.engine === engine && bus < node.numberOfOutputs {
            let key = OutputKey(node: ObjectIdentifier(node), bus: bus)
            guard seen.insert(key).inserted else { continue }
            let destinations = engine.outputConnectionPoints(for: node, outputBus: bus).compactMap { point -> Destination? in
                guard let node = point.node else { return nil }
                return Destination(node: node, bus: point.bus)
            }
            result.append(Connection(source: node, bus: bus, destinations: destinations, format: node.outputFormat(forBus: bus)))
        }
        return result
    }
    private func disconnectOutputs(_ connections: [Connection]) {
        var seen = Set<OutputKey>()
        for connection in connections where connection.source.engine === engine {
            let key = OutputKey(node: ObjectIdentifier(connection.source), bus: connection.bus)
            if seen.insert(key).inserted { engine.disconnectNodeOutput(connection.source, bus: connection.bus) }
        }
    }
    /// The connection being made when AVAudioEngine last raised (diagnostics).
    private var lastConnection: Connection?
    var lastAttemptedConnection: String {
        guard let connection = lastConnection else { return "" }
        func describe(_ node: AVAudioNode) -> String { (node as? AVAudioUnit)?.auAudioUnit.componentName ?? "\(type(of: node))" }
        return describe(connection.source) + " -> " + connection.destinations.map { "\(describe($0.node)) bus \($0.bus) of \($0.node.numberOfInputs)" }.joined(separator: ", ")
    }
    private func connect(_ connections: [Connection]) {
        for connection in connections where connection.source.engine === engine {
            lastConnection = connection
            if connection.destinations.isEmpty { engine.disconnectNodeOutput(connection.source, bus: connection.bus) }
            else {
                engine.connect(connection.source, to: connection.destinations.map { AVAudioConnectionPoint(node: $0.node, bus: $0.bus) },
                    fromBus: connection.bus, format: connection.format)
            }
        }
        lastConnection = nil
    }
    private func detachHelpers(_ nodes: [AVAudioNode]) {
        for node in nodes where node.engine === engine {
            engine.disconnectNodeInput(node); engine.disconnectNodeOutput(node)
        }
        for node in nodes where node.engine === engine { engine.detach(node) }
    }
}

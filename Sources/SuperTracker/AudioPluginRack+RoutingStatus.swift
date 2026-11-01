import AppKit
import AudioToolbox
import AVFoundation
import CoreAudioKit
import CoreFoundation
import Darwin

/// Routing meters, capture configuration, validation, capabilities and status.
extension AudioPluginRack {
    var hasRouting:Bool { routingGraph.current != nil }
    var standardMixing:Bool { routingDefinition?.enabled == true && routingDefinition?.mixModel == .standard }
    var routingCaptures:[String:AudioRoutingCapture] { routingGraph.current?.captures ?? [:] }

    var routingCaptureArrivalFrames:[String:Int] { routingGraph.current?.captureArrivalFrames ?? [:] }

    func routingMeterSnapshot()->[String:[Float]] {
        var result:[String:[Float]] = [:]
        // Strip mix processors measure their own output peaks (no taps, no buffer copies).
        let strips = routingGraph.current?.strips ?? [:]
        for (owner,index) in ownerIndices {
            if let mix = strips[owner]?.mix.automation, mix.meteringEnabled {
                let peaks = mix.takePeaks(); routingMeters[index].accumulateMaster(left:peaks.left,right:peaks.right)
            }
            let levels = routingMeters[index].snapshot()
            result[owner] = [levels.left,levels.right]
        }
        return result
    }

    func removeRoutingMeters() {
        for mix in routingMeterNodes { mix.meteringEnabled = false }
        routingMeterNodes.removeAll()
    }

    func installRoutingMeters() {
        removeRoutingMeters()
        guard let prepared = routingGraph.current else { return }
        for (owner,strip) in prepared.strips {
            guard let index = ownerIndices[owner] else { continue }
            routingMeters[index].reset()
            strip.mix.automation.meteringEnabled = true
            routingMeterNodes.append(strip.mix.automation)
        }
    }


    func configureRoutingCapture(owners:Set<String>) throws {
        let previous = routingCaptureOwners
        routingCaptureOwners = owners
        do { try configureRouting(routingProject,force:true) }
        catch { routingCaptureOwners = previous; throw error }
    }

    func resetRoutingCaptures(startFrame:Int64 = 0,frameLimit:Int64? = nil) throws {
        try routingGraph.current?.configureCaptures(startFrame:startFrame,frameLimit:frameLimit)
    }

    func routingLayouts(_ definition:AudioRouting.Definition) -> [AudioRouting.PluginLayout] {
        routingEffects(definition).map { effect in
            let unit = effect.node.auAudioUnit
            let buses = effect.inputBuses ?? (0..<unit.inputBusses.count).map { index in
                let bus = unit.inputBusses[index]
                return AudioRouting.InputBus(index:index,name:bus.name ?? "Input \(index + 1)",channels:Int(bus.format.channelCount),isAuxiliary:index != effect.mainInputBus)
            }
            return .init(instanceID:effect.instanceID,owner:effect.owner,slot:effect.slot,
                latencyFrames:Int((max(0,min(60,unit.latency))*format.sampleRate).rounded()),inputBuses:buses)
        }
    }

    func validateRouting(_ data:[String:Any]) throws -> [String:Any] {
        let definition = try AudioRouting.parse(project:data)
        _ = try AudioProject.routingAutomation(data)
        let layouts = routingLayouts(definition).map { item in
            AudioRouting.PluginLayout(instanceID:item.instanceID,owner:item.owner,kind:item.kind,slot:item.slot,
                latencyFrames:item.latencyFrames,inputBuses:item.inputBuses.filter { (0..<32).contains($0.index) && (1...2).contains($0.channels) })
        }
        let graph = try definition.graph(pluginLayouts:layouts)
        let plan = try AudioRouting.plan(nodes:graph.nodes,edges:graph.edges,maximumFrames:nativeReturnCompensation.maximumFrames)
        let unavailable = graph.unavailableSidechains.map { ["id":$0.sidechain.id,"reason":$0.reason] }
        return ["valid":true,"routing":data["routing"] ?? NSNull(),"status":["unavailable":unavailable,"latencyFrames":plan.total]]
    }

    func routingCapabilities() -> [String:Any] {
        let definition = routingDefinition
        let layouts = definition.map(routingLayouts) ?? []
        let names = Dictionary(uniqueKeysWithValues:instances.values.map { ($0.instanceID,$0.component.name) })
        let inputs:[[String:Any]] = layouts.flatMap { item in
            item.inputBuses.filter(\.isAuxiliary).map { bus in
                ["instanceID":item.instanceID,"owner":item.owner,"pluginName":names[item.instanceID] ?? "Plug-in",
                 "inputBus":bus.index,"layoutSignature":["name":bus.name,"channels":bus.channels],
                 "available":(1...2).contains(bus.channels)]
            }
        }
        let targets:[[String:Any]] = (definition?.owners ?? []).map { owner in
            ["owner":owner.id,"name":owner.name,"kind":owner.kind.rawValue,
             "boundary":owner.kind == .master ? "masterOutput" : "postFaderPreParent","available":true]
        }
        return ["supported":true,"mixModels":["legacy","standard"],"outputs":true,
            "buses":["group":true,"return":true,"max":Self.busCapacity],
            "sends":["enabled":definition?.mixModel == .standard,"taps":definition?.mixModel == .legacy ? ["postFader"] : ["preFader","postFader"],"max":128],
            "sidechains":["enabled":definition?.mixModel == .standard,"inputs":inputs,"taps":definition?.mixModel == .legacy ? ["postFader"] : ["preInserts","preFader","postFader"]],"exportTargets":targets,"fullGraphStems":definition?.enabled == true && hasRouting,
            "requiresStandardForCustomRouting":true]
    }


    /// Instance IDs of the inserts each insert-chain node renders, in processing order.
    func insertChainLayout() -> [[String]] {
        guard let chains = routingGraph.current?.insertChains else { return [] }
        var names: [ObjectIdentifier: String] = [:]
        for item in instances.values { if let member = (item.node.auAudioUnit as? BuiltinEffectsAudioUnit)?.chainMember { names[ObjectIdentifier(member)] = item.instanceID } }
        var seen = Set<ObjectIdentifier>(), result: [[String]] = []
        for chain in chains.values where seen.insert(ObjectIdentifier(chain)).inserted {
            result.append(chain.members.map { names[ObjectIdentifier($0)] ?? "?" })
        }
        return result.sorted { ($0.first ?? "") < ($1.first ?? "") }
    }

    func routingStatus()->[String:Any] {
        var value = routingGraph.current?.status ?? ["enabled":false]
        value["incrementalChanges"] = incrementalRoutingChanges; value["fullRebuilds"] = fullRoutingRebuilds; value["insertChainUpdates"] = insertChainUpdates
        if let lastFullRebuildReason { value["lastFullRebuildReason"] = lastFullRebuildReason }
        value["enabled"] = hasRouting
        value["mixModel"] = routingDefinition?.mixModel.rawValue ?? "legacy"
        value["captureArrivalFrames"] = routingCaptureArrivalFrames
        value["limited"] = latencyExceedsLimit
        value["unavailable"] = value["unavailableSidechains"] ?? []
        return value
    }
}

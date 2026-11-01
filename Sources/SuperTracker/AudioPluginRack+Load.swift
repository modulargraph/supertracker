import AppKit
import AudioToolbox
import AVFoundation
import CoreAudioKit
import CoreFoundation
import Darwin

/// Listing and loading plug-ins (including NKS presets) into a slot.
extension AudioPluginRack {
    func list() -> [[String: Any]] {
        let types: [OSType] = [kAudioUnitType_MusicDevice, kAudioUnitType_Effect, kAudioUnitType_MusicEffect]
        var result: [[String: Any]] = []
        for type in types {
            let description = AudioComponentDescription(componentType: type, componentSubType: 0,
                componentManufacturer: 0, componentFlags: 0, componentFlagsMask: 0)
            result.append(contentsOf: AVAudioUnitComponentManager.shared().components(matching: description).filter {
                let value = $0.audioComponentDescription
                return !VST3AudioUnit.isAdapterDescription(value) && !VST2AudioUnit.isAdapterDescription(value) && !BuiltinEffectsAudioUnit.isAdapterDescription(value) &&
                    (value.componentManufacturer != 0x53705472 || ![0x73747064, 0x73746d78].contains(value.componentSubType)) &&
                    (value.componentManufacturer != 0x5374546d || ![0x73747467, 0x73747465].contains(value.componentSubType))
            }.map { component in
                ["id": componentID(component.audioComponentDescription), "name": component.name,
                 "manufacturer": component.manufacturerName, "version": component.versionString, "format": "AU",
                 "type": type == kAudioUnitType_MusicDevice ? "instrument" : "effect"]
            })
        }
        result.append(contentsOf: BuiltinEffectsAudioUnit.catalogue().map { row in var row=row;row["type"]="effect";return row })
        result.append(contentsOf: vst3Catalogue.records.map(\.dictionary))
        result.append(contentsOf: vst2Catalogue.records.map(\.dictionary))
        return result.sorted {
            let lhs = ($0["manufacturer"] as? String ?? "") + ($0["name"] as? String ?? "")
            let rhs = ($1["manufacturer"] as? String ?? "") + ($1["name"] as? String ?? "")
            return lhs.localizedStandardCompare(rhs) == .orderedAscending
        }
    }

    @MainActor func load(_ payload: [String: Any]) async throws -> Any {
        try await load(payload, nksPreset: nil)
    }

    /// Native NKS loading is transactional: exact identity and component state are
    /// validated in a candidate node before any existing rack node is replaced.
    @MainActor func loadNKS(_ payload: [String: Any]) async throws -> Any {
        guard let path = payload["path"] as? String else { throw RackError.invalid("Choose an NKS preset to load.") }
        let preset = try NKSHostPreset(url: URL(fileURLWithPath: path))
        var request = payload
        let isInstrument = (payload["kind"] as? String ?? "instrument") == "instrument"
        let hints = [preset.identity.name, preset.metadata["deviceType"] as? String].compactMap { $0 } +
            URL(fileURLWithPath: path).deletingLastPathComponent().pathComponents.suffix(4).reversed()
        let vst2 = vst2Catalogue, vst3 = vst3Catalogue
        request["component"] = try await VST3Catalogue.resolveNKSComponent(
            vst3ClassID: preset.identity.vst3ClassID, vst2Magic: preset.identity.vst2Magic,
            exactVST3: { try await vst3.resolveOrScan(identifier: $0, preferredNames: hints).id },
            nativeVST2: { try await vst2.resolveOrScan(uniqueID: $0, isInstrument: isInstrument, preferredNames: hints)?.id },
            compatibleVST3: { try await vst3.resolveCompatibleOrScan(vst2Magic: $0, isInstrument: isInstrument, preferredNames: hints).id })
        return try await load(request, nksPreset: preset)
    }

    /// Unattached preparation of one plug-in: instantiation, saved or NKS state,
    /// format and the baseline capture. Nothing is attached or published here.
    @MainActor func prepareUnit(_ payload:[String:Any], target:Target, nksPreset:NKSHostPreset?,
                                       isRequestCurrent:@escaping ()->Bool) async throws -> PreparedUnit {
        func trace(_ message:String) { if getenv("SUPERTRACKER_VST3_TRACE") != nil { fputs("VST3 rack: \(message)\n",stderr) } }
        guard var identifier = payload["component"] as? String ?? payload["id"] as? String else {
            throw RackError.invalid("Choose an Audio Unit or VST3 plug-in to load.")
        }
        let sourceIdentifier=identifier
        var legacyBank:AudioLegacyVST2Bank?, migratedDescriptor:VST3PluginDescriptor?
        var usedRawFallback=false
        if nksPreset == nil,identifier.lowercased().hasPrefix("vst2:"),
           let saved=payload["state"] as? String,let magic=UInt32(identifier.dropFirst(5),radix:16) {
            let native=try await vst2Catalogue.resolveOrScan(uniqueID:magic,isInstrument:target.kind == "instrument",
                preferredNames:[payload["name"] as? String].compactMap { $0 })
            try Task.checkCancellation()
            guard isRequestCurrent() else { throw RackError.invalid("A newer request replaced this plug-in loading request.") }
            if native == nil {
                let bank=try AudioLegacyVST2Bank(component:identifier,state:saved)
                let replacement=try await vst3Catalogue.resolveCompatibleOrScan(vst2Magic:bank.uniqueID,isInstrument:target.kind == "instrument",
                    preferredNames:[payload["name"] as? String].compactMap { $0 })
                try Task.checkCancellation()
                guard isRequestCurrent(),VST3Catalogue.isVST2Compatible(classID:replacement.classID,vst2Magic:bank.uniqueID) else {
                    throw RackError.invalid("The VST3 replacement does not match this saved VST2 bank.")
                }
                legacyBank=bank;migratedDescriptor=replacement;identifier=replacement.id
            }
        }
        var recovery:RecoveryRecord?
        var candidate:AVAudioUnit?
        var committed = false
        defer {
            if !committed {
                if let unit = candidate?.auAudioUnit { auStateReuse.remove(ObjectIdentifier(unit)) }
                if let record = recovery {
                    record.abandoned = true
                    if record.phase == .loading { record.phase = .failedLoad }
                    else { do { try beginRecoveryRelease(record) } catch { recordRecoveryError(error) } }
                }
                if let unit=candidate?.auAudioUnit,unit.renderResourcesAllocated { unit.deallocateRenderResources() }
                if let adapter = candidate?.auAudioUnit as? HostedPluginAdapter {
                    adapter.closePlugin(); recovery?.vendorClosed = true
                }
                candidate = nil
                collectRecoveryRecords()
            }
        }
        func prepareCandidate(_ value:Component) async throws {
            recovery = try beginRecoveryLoad(value,target:target)
            if let beforeInstantiation { try await beforeInstantiation() }
            try Task.checkCancellation()
            guard isRequestCurrent() else {
                throw RackError.invalid("A newer request replaced this plug-in loading request.")
            }
        }
        let component: Component
        let node: AVAudioUnit
        var nks = payload["nks"] as? [String: Any]
        if identifier.hasPrefix("stfx:") {
            guard target.kind == "effect", nksPreset == nil, BuiltinEffectsAudioUnit.has(id:identifier) else {
                throw RackError.invalid("Choose a supported built-in effect for an insert slot.")
            }
            let metadata = BuiltinEffectsAudioUnit.catalogue().first { $0["id"] as? String == identifier }
            component = Component(id:identifier,name:metadata?["name"] as? String ?? identifier,format:"Builtin")
            try await prepareCandidate(component)
            node = try BuiltinEffectsAudioUnit.makeNode(id:identifier,sampleRate:format.sampleRate)
            candidate = node; recovery?.node = node; recovery?.unit = node.auAudioUnit
        } else if identifier.lowercased().hasPrefix("vst3:") {
            if let suspect = recoveryStore?.recoveryReport().suspects.first(where: {
                $0.requiresReview && $0.identity.componentID == identifier &&
                (payload["bundlePath"] == nil || payload["bundlePath"] as? String == $0.identity.bundlePath)
            }) { throw PluginRecoveryStore.RecoveryError.reviewRequired(suspect.identity) }
            trace("resolve class")
            let descriptor:VST3PluginDescriptor
            if let migratedDescriptor { descriptor=migratedDescriptor }
            else { descriptor = try await vst3Catalogue.resolveOrScan(identifier: identifier,
                preferredNames: [nksPreset?.identity.name, nksPreset?.metadata["deviceType"] as? String].compactMap { $0 },
                preferredBundlePath: payload["bundlePath"] as? String) }
            try Task.checkCancellation()
            guard isRequestCurrent() else {
                throw RackError.invalid("A newer request replaced this plug-in loading request.")
            }
            guard descriptor.isInstrument == (target.kind == "instrument") else {
                throw RackError.invalid("This VST3 plug-in has the wrong type for the selected slot.")
            }
            trace("resolved class")
            component = Component(descriptor)
            try await prepareCandidate(component)
            node = try VST3AudioUnit.makeNode(descriptor: descriptor, sampleRate: format.sampleRate)
            candidate = node; recovery?.node = node; recovery?.unit = node.auAudioUnit
            if let adapter = node.auAudioUnit as? VST3AudioUnit {
                // Set the final rack format before release-specific initialization.
                // Later AU resource allocation preserves the same C++ configuration.
                try configureFormat(adapter)
                trace("prepare candidate state")
                try await adapter.prepareForInitialState()
                try Task.checkCancellation()
                guard isRequestCurrent() else {
                    throw RackError.invalid("A newer request replaced this plug-in loading request.")
                }
                if let preset = nksPreset {
                    try adapter.restoreComponentState(preset.state, classID: descriptor.classID)
                } else if let bank=legacyBank {
                    usedRawFallback=try adapter.restoreLegacyVST2Bank(bank.payload,uniqueID:bank.uniqueID,program:bank.program)
                }
            }
        } else if identifier.lowercased().hasPrefix("vst2:") {
            if let suspect = recoveryStore?.recoveryReport().suspects.first(where: {
                $0.requiresReview && $0.identity.componentID.uppercased() == identifier.uppercased()
            }) { throw PluginRecoveryStore.RecoveryError.reviewRequired(suspect.identity) }
            var descriptor = vst2Catalogue.resolve(identifier: identifier)
            if descriptor == nil, let value = UInt32(identifier.dropFirst(5), radix: 16) {
                descriptor = try await vst2Catalogue.resolveOrScan(uniqueID: value, isInstrument: target.kind == "instrument",
                    preferredNames: [payload["name"] as? String].compactMap { $0 })
            }
            guard let descriptor else { throw RackError.invalid("The VST2 plug-in is not installed or cannot run on this Mac: \(identifier)") }
            try Task.checkCancellation()
            guard isRequestCurrent() else { throw RackError.invalid("A newer request replaced this plug-in loading request.") }
            guard descriptor.isInstrument == (target.kind == "instrument") else {
                throw RackError.invalid("This VST2 plug-in has the wrong type for the selected slot.")
            }
            component = Component(descriptor)
            try await prepareCandidate(component)
            node = try VST2AudioUnit.makeNode(descriptor: descriptor, sampleRate: format.sampleRate)
            candidate = node; recovery?.node = node; recovery?.unit = node.auAudioUnit
            if let adapter = node.auAudioUnit as? VST2AudioUnit {
                try configureFormat(adapter)
                if let preset = nksPreset {
                    guard preset.identity.vst2Magic == descriptor.uniqueID else {
                        throw RackError.invalid("The NKS preset does not identify this VST2 plug-in.")
                    }
                    try adapter.restoreChunk(preset.state, preset: false)
                    nks = ["name": preset.name, "path": preset.url.path, "identity": preset.identity.dictionary,
                        "pages": preset.validatedMappings(vst2UniqueID: descriptor.uniqueID, parameterIDs: adapter.parameterIDs),
                        "warnings": preset.warnings]
                }
            }
        } else {
            guard nksPreset == nil else { throw RackError.invalid("Direct NKS loading requires a matching VST3 plug-in.") }
            let description = try componentDescription(identifier)
            let isInstrument = description.componentType == kAudioUnitType_MusicDevice
            guard isInstrument == (target.kind == "instrument") else {
                throw RackError.invalid("This Audio Unit has the wrong type for the selected slot.")
            }
            let installed = AVAudioUnitComponentManager.shared().components(matching: description).first
            component = installed.map(Component.init) ?? Component(id:identifier,name:payload["name"] as? String ?? identifier)
            try await prepareCandidate(component)
            guard installed != nil else { throw RackError.invalid("The Audio Unit is not installed: \(identifier)") }
            node = try await withCheckedThrowingContinuation { continuation in
                AVAudioUnit.instantiate(with: description, options: []) { unit, error in
                    if let unit { continuation.resume(returning: unit) }
                    else { continuation.resume(throwing: error ?? RackError.invalid("The Audio Unit could not be opened.")) }
                }
            }
            candidate = node; recovery?.node = node; recovery?.unit = node.auAudioUnit
        }
        try Task.checkCancellation()
        guard isRequestCurrent() else {
            throw RackError.invalid("A newer request replaced this plug-in loading request.")
        }
        if legacyBank == nil,let saved = payload["state"] as? String { try restoreState(saved, to: node.auAudioUnit, inGraph: false) }
        if let adapter = node.auAudioUnit as? VST3AudioUnit {
            trace("finish candidate state")
            try await adapter.finishInitialStateRestore(componentState: nksPreset != nil || legacyBank != nil)
            try Task.checkCancellation()
            guard isRequestCurrent() else {
                throw RackError.invalid("A newer request replaced this plug-in loading request.")
            }
            if let preset = nksPreset, let classID = adapter.classID {
                nks = ["name": preset.name, "path": preset.url.path, "identity": preset.identity.dictionary,
                    "pages": preset.validatedMappings(vst3ClassID: classID, parameterIDs: adapter.parameterIDs),
                    "warnings": preset.warnings]
            }
        }
        trace("set bypass")
        node.auAudioUnit.shouldBypassEffect = payload["bypass"] as? Bool ?? false
        trace("configure context")
        configureContext(node.auAudioUnit)
        guard node.auAudioUnit.outputBusses.count > 0 else {
            throw RackError.invalid("The plug-in does not provide an audio output.")
        }
        if target.kind == "effect", node.auAudioUnit.inputBusses.count == 0 || (node.auAudioUnit as? VST3AudioUnit)?.mainInputBus == -1 {
            throw RackError.invalid("The plug-in does not provide an audio input.")
        }
        trace("configure format")
        try configureFormat(node.auAudioUnit)
        func captureBaseline() async throws -> String {
            // Capture the initial state before committing: a plug-in that cannot save
            // its state must not replace a working instrument with an unsavable patch.
            // Vendor initialization can enqueue controls before a user ever touches
            // the editor. This candidate has no graph callbacks yet, so settle its
            // own state without suspending playback through the current instrument.
            var cancelCandidateCapture: (() -> Void)?
            defer { cancelCandidateCapture?() }
            if (payload["state"] as? String == nil || legacyBank != nil), let adapter = node.auAudioUnit as? VST3AudioUnit,
               adapter.requiredStateFlushFrames > 0 {
                let owner = payload["owner"] as? String
                let isCurrent = {
                    isRequestCurrent() && node.engine == nil && (owner.map { self.ownerIndices[$0] == target.track } ?? true)
                }
                trace("prepare candidate capture")
                if let preparation = try await VST3StatePreparation.prepareCapture(
                    snapshot: { adapter.stateCaptureSnapshot }, isCurrent: isCurrent,
                    isGraphRunning: { false },
                    beginCapture: { try adapter.beginStateCapture(allowStoppedAudioAdvance: $0) },
                    cancelCapture: { adapter.cancelStateCapture(token: $0) }) {
                    cancelCandidateCapture = { adapter.cancelStateCapture(token: preparation.token) }
                    try Task.checkCancellation()
                    guard isCurrent() else {
                        throw RackError.invalid("A newer request replaced this plug-in loading request.")
                    }
                    try adapter.acceptStateCapturePreparation(preparation)
                }
            }
            try Task.checkCancellation()
            guard isRequestCurrent() else {
                throw RackError.invalid("A newer request replaced this plug-in loading request.")
            }
            trace("encode initial state")
            notifyStateCaptureMutation()
            return try legacyBank != nil ? encodedState(node.auAudioUnit) : (payload["state"] as? String ?? encodedState(node.auAudioUnit))
        }
        let restoredBaseline=try await captureBaseline()
        // Hosted VST3/VST2 adapters start processing here, after their startup recipe
        // and saved state, instead of implicitly inside the graph's next start. There a
        // failed activation surfaced only as AVAudioEngine's opaque -10867
        // (kAudioUnitErr_Uninitialized) and the slot rendered silence; here the vendor's
        // own error is reported and the previous plug-in stays in place.
        if node.auAudioUnit is HostedPluginAdapter, !node.auAudioUnit.renderResourcesAllocated {
            do { try node.auAudioUnit.allocateRenderResources() }
            catch { throw RackError.invalid("\(component.name) could not start audio processing: \(error.localizedDescription)") }
        }
        try recoveryDidLoad(recovery)
        if let owner = payload["owner"] as? String, ownerIndices[owner] != target.track {
            throw RackError.invalid("The mixer channel changed while this plug-in was loading.")
        }
        try Task.checkCancellation()
        guard isRequestCurrent() else {
            throw RackError.invalid("The plug-in preparation request no longer owns its candidate.")
        }
        committed = true
        return PreparedUnit(component:component,node:node,nks:nks,
            recovery:recovery,baseline:restoredBaseline,migration:legacyBank == nil ? nil : LegacyMigration(component:sourceIdentifier,state:payload["state"] as! String,usedRawFallback:usedRawFallback))
    }

    @MainActor func load(_ payload: [String: Any], nksPreset: NKSHostPreset?) async throws -> Any {
        func trace(_ message: String) { if getenv("SUPERTRACKER_VST3_TRACE") != nil { fputs("VST3 rack: \(message)\n", stderr) } }
        trace("load start")
        try Task.checkCancellation()
        notifyStateCaptureMutation()
        let target = try target(payload)
        guard (payload["component"] as? String ?? payload["id"] as? String) != nil else {
            throw RackError.invalid("Choose an Audio Unit or VST3 plug-in to load.")
        }
        let generation = (generations[target] ?? 0) + 1
        generations[target] = generation
        let prepared = try await prepareUnit(payload,target:target,nksPreset:nksPreset,
            isRequestCurrent:{ self.generations[target] == generation && (payload["owner"] == nil || self.ownerIndices[payload["owner"] as? String ?? ""] == target.track) })
        let node=prepared.node, component=prepared.component, recovery=prepared.recovery
        let nks=prepared.nks, restoredBaseline=prepared.baseline
        var committed = false
        defer { if !committed { abandonPreparedUnit(prepared) } }
        let previous = instances[target], previousState = lastRestoredStates[target]
        routingTransactionDepth += 1
        defer { routingTransactionDepth -= 1 }
        let wasRunning = engine.isRunning
        // A built-in insert replacing nothing or another built-in on a routed strip usually only
        // changes an insert-chain node's members: try that in place before pausing anything.
        let inPlace = wasRunning && insertChainCandidate(target:target,node:node,previous:previous)
        if wasRunning && !inPlace { pauseEngine() }
        do {
            if !inPlace { try awaitRenderQuiescence() }
            // getState and recovery bookkeeping are synchronous vendor/control
            // operations; cancellation can arrive from another thread meanwhile.
            try Task.checkCancellation()
            guard generations[target] == generation else {
                throw RackError.invalid("A newer request replaced this plug-in loading request.")
            }
        }
        catch { if wasRunning && !engine.isRunning { try? startEngine() }; throw error }
        trace("attach node")
        let requestedID = payload["instanceID"] as? String
        let stableID = requestedID.flatMap { value in
            !value.isEmpty && value.count <= 128 && !instances.values.contains(where:{ $0.target != target && $0.instanceID == value }) ? value : nil
        } ?? UUID().uuidString
        let instance = Instance(target:target,component:component,node:node,leaseStore:endpointStore,graphQuantum:graphQuantum,nks:nks,recovery:recovery,instanceID:stableID)
        instance.migration=prepared.migration
        instance.ownerID = ownersByIndex[target.track]
        engine.attach(instance.node)
        engine.attach(instance.converter)
        instances[target] = instance; lastRestoredStates[target] = restoredBaseline
        // The previous unit is disconnected below, once the graph is paused (never while it runs).
        publish(RenderEndpoint(instance),for:target)
        do {
            trace("rebuild graph")
            if inPlace, try applyInsertChainUpdate() { trace("insert chain updated in place") }
            else {
                if engine.isRunning { pauseEngine(); try awaitRenderQuiescence() }
                if let previous { engine.disconnectNodeOutput(previous.node); engine.disconnectNodeOutput(previous.converter) }
                if previous != nil { try rebuildRemovingUnit(target.track) } else { try rebuild(target.track) }
            }
            if !refreshLatencyCompensation() { onLatencyPlanChanged?() }
            if wasRunning && !engine.isRunning { try startEngine() }
            // Only a fully prepared and successfully started candidate can retire
            // the preceding instance. Failed routes preserve its exact live state.
            if let previous { try beginRecoveryRelease(previous.recovery) }
        } catch {
            let original = error
            stopEngine(); try? awaitRenderQuiescence()
            instances[target] = previous; lastRestoredStates[target] = previousState
            publish(previous.map { RenderEndpoint($0) },for:target)
            engine.disconnectNodeOutput(instance.node); engine.disconnectNodeOutput(instance.converter)
            do { try rollbackRoutingMutation() }
            catch {
                throw RackError.invalid("Plug-in change failed (\(original.localizedDescription)); restoring its previous routing also failed (\(error.localizedDescription)). Audio remains stopped.")
            }
            instance.removeRenderObserver()
            detachUnit(instance)
            endpointStore.collectRetired(); routingRenderStore.collectRetired()
            if wasRunning { try? startEngine() }
            throw original
        }
        if !engine.isRunning { try routingGraph.commit() } else { routingGraph.commitChainUpdate() }
        committed = true
        if let previous {
            editorWindows.removeValue(forKey:target)?.close()
            retiredUnits.append(previous)
            detachUnit(previous)
        }
        collectRetiredEndpoints()
        trace("make summary")
        var result = summary(instance)
        result["state"] = restoredBaseline
        trace("load return")
        return result
    }
}

extension AudioPluginRack {
    /// A routed strip's built-in insert slot whose change can be an in-place insert-chain
    /// update (applyInsertChainUpdate decides; otherwise the usual paused rebuild follows).
    func insertChainCandidate(target:Target,node:AVAudioUnit?,previous:Instance?) -> Bool {
        guard target.kind == "effect", routingDefinition?.enabled == true, routingRebuildDeferral == 0, routingCaptureOwners.isEmpty else { return false }
        if let node, !(node.auAudioUnit is BuiltinEffectsAudioUnit) { return false }
        if let previous, !(previous.node.auAudioUnit is BuiltinEffectsAudioUnit) { return false }
        return node != nil || previous != nil
    }
}

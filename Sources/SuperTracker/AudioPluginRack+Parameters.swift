import AppKit
import AudioToolbox
import AVFoundation
import CoreAudioKit
import CoreFoundation
import Darwin

/// Parameters, base patches, state, presets, snapshots, session restore and editors.
extension AudioPluginRack {
    func parameters(_ payload: [String: Any]) throws -> Any {
        let item = try instance(payload), unit = item.node.auAudioUnit
        synchronizeParameterValues(unit)
        let live = liveParameterValues(item)
        let requested = (payload["addresses"] as? [String]).map { Set($0) }
        let diagnostic = payload["diagnostics"] as? Bool == true ? (unit as? VST3AudioUnit)?.parameterDiagnostics() : nil
        return (unit.parameterTree?.allParameters ?? []).filter {
            requested?.contains(String($0.address)) ?? true
        }.map { parameter -> [String: Any] in
            var result = parameterInfo(parameter,unit:unit,live:live)
            if let diagnostic, let watchedID = diagnostic["parameterID"] as? UInt32,
               AUParameterAddress(watchedID) == parameter.address { result["diagnostic"] = diagnostic }
            return result
        }
    }

    func parameter(_ payload: [String: Any]) throws -> Any {
        let item = try instance(payload)
        if let bypass = payload["bypass"] as? Bool {
            notifyStateCaptureMutation()
            item.node.auAudioUnit.shouldBypassEffect = bypass
            refreshLatencyCompensation()
            return summary(item)
        }
        guard let address = address(payload["address"]),
              let parameter = item.node.auAudioUnit.parameterTree?.parameter(withAddress: address) else {
            throw RackError.invalid("The Audio Unit parameter does not exist.")
        }
        var written: AUValue?
        if let number = payload["value"] as? NSNumber {
            guard number.doubleValue.isFinite else { throw RackError.invalid("Parameter value must be finite.") }
            let raw: Float
            if let grid = AudioPluginParameterMetadata.grid(parameter,explicitStepCount:parameterStepCount(parameter,unit:item.node.auAudioUnit)) {
                raw = grid.raw(number.doubleValue,normalized:payload["normalized"] as? Bool ?? true)
            } else if payload["normalized"] as? Bool ?? true {
                raw = parameter.minValue + max(0, min(1, number.floatValue)) * (parameter.maxValue - parameter.minValue)
            } else { raw = max(parameter.minValue, min(parameter.maxValue, number.floatValue)) }
            notifyStateCaptureMutation()
            parameter.setValue(raw, originator: nil)
            written = raw
        }
        refreshLatencyCompensation()
        synchronizeParameterValues(item.node.auAudioUnit)
        // A plug-in may apply the write only on its next render, so its value here can still be the old one.
        // "written" is the position sent (snapped to the parameter's steps), for callers to verify against.
        var result = parameterInfo(parameter,unit:item.node.auAudioUnit,live:liveParameterValues(item))
        if let written {
            let width = parameter.maxValue - parameter.minValue
            result["written"] = width > 0 ? (written - parameter.minValue) / width : 0
        }
        return result
    }

    @MainActor func captureBasePatch(_ payload:[String:Any]) throws -> [String:Any] {
        let item=try instance(payload)
        guard let expected=payload["instanceID"] as? String,expected == item.instanceID else {
            throw RackError.invalid("The selected plug-in changed. Select it again before updating its base patch.")
        }
        guard let adapter=item.node.auAudioUnit as? VST3AudioUnit else {
            throw RackError.invalid("This plug-in does not support baseline-safe patch capture.")
        }
        notifyStateCaptureMutation()
        // Capture the baseline and the project state in one drained access, so
        // no render or edit can land between them. If encoding still fails, the
        // plug-in holds a baseline the project has not saved: say so plainly.
        let wasRunning=engine.isRunning
        let state=try withVST3StateAccess { () throws -> String in
            try adapter.captureBasePatch()
            do { return try encodedState(item.node.auAudioUnit,allowStoppedAudioAdvance:!wasRunning) }
            catch {
                throw RackError.invalid("The plug-in updated its base patch, but the project could not save it: \(error.localizedDescription) Save the project or update the base patch again.")
            }
        }
        var result=summary(item,includeParameters:false)
        result["state"]=state
        return result
    }

    @MainActor func state(_ payload: [String: Any]) throws -> Any {
        if payload["track"] == nil && payload["owner"] == nil { return snapshot() }
        let item = try instance(payload)
        if let saved = payload["state"] as? String {
            do { try restoreTrackedState(saved,to:item) }
            catch { _ = try? remove(payload); throw error }
            lastRestoredStates[item.target] = saved
            publish(RenderEndpoint(item), for: item.target)
        }
        var result = summary(item)
        let saved = try encodedState(item.node.auAudioUnit)
        result["state"] = saved
        return result
    }

    @MainActor func preset(_ payload: [String: Any]) throws -> Any {
        let item = try instance(payload)
        guard let number = payload["number"] as? Int else { return presetInfo(item.node.auAudioUnit) }
        guard let preset = item.node.auAudioUnit.factoryPresets?.first(where: { $0.number == number }) else {
            throw RackError.invalid("The factory preset does not exist.")
        }
        if let record = item.recovery, let store = recoveryStore {
            _ = try store.beginLoad(record.identity,reusingToken:record.token); record.phase = .loading
        }
        notifyStateCaptureMutation()
        lastCapturedStates.removeValue(forKey:ObjectIdentifier(item.node.auAudioUnit))
        item.node.auAudioUnit.currentPreset = preset
        do { try recoveryDidLoad(item.recovery) }
        catch { _ = try? remove(payload); throw error }
        refreshLatencyCompensation()
        publish(RenderEndpoint(item), for: item.target)
        return summary(item)
    }

    /// fresh=false reports the state this rack last restored into or read from each
    /// instance, without touching the plug-ins: a descriptor list for the interface
    /// after reconciliation. Saving (fresh=true) reads every instance's actual state.
    func snapshot(fresh: Bool = true) -> [[String: Any]] {
        instances.values.sorted {
            if $0.target.track != $1.target.track { return $0.target.track < $1.target.track }
            if $0.target.kind != $1.target.kind { return $0.target.kind < $1.target.kind }
            return $0.target.slot < $1.target.slot
        }.map { item in
            var result = summary(item, includeParameters: false)
            if !fresh, let known = capturedState(item.node.auAudioUnit) ?? lastRestoredStates[item.target] {
                result["state"] = known
                return result
            }
            do {
                let state = try encodedState(item.node.auAudioUnit)
                result["state"] = state
            }
            catch { result["stateError"] = error.localizedDescription }
            return result
        }
    }

    /// This authority comes from a successfully prepared live instance, never
    /// from a caller-supplied compatibility flag. The source bank must still be
    /// byte-exact and bound to the same saved identity and stable owner/slot.
    func isVerifiedMigration(_ desired:[String:Any],item:Instance)->Bool {
        guard let migration=item.migration,
              desired["component"] as? String == migration.component,
              desired["state"] as? String == migration.state,
              desired["instanceID"] as? String == item.instanceID,
              desired["owner"] as? String == item.ownerID,
              desired["kind"] as? String == item.target.kind,
              desired["slot"] as? Int == item.target.slot,
              (try? target(desired)) == item.target else { return false }
        return true
    }

    func isVerifiedMigration(_ desired:[String:Any],active:[String:Any])->Bool {
        guard let target=try? target(desired),let item=instances[target],
              active["instanceID"] as? String == item.instanceID,
              active["component"] as? String == item.component.id,
              active["format"] as? String == item.component.format,
              active["owner"] as? String == item.ownerID else { return false }
        return isVerifiedMigration(desired,item:item)
    }

    @MainActor func restore(_ saved: [[String: Any]], forceStates: Bool = false) async throws {
        try Task.checkCancellation()
        notifyStateCaptureMutation()
        // Reconcile instead of rebuilding: unrelated edits to a tracker project must not
        // interrupt instrument voices, reset vendor editors, or reload gigabyte libraries.
        let desiredTargets = try Set(saved.map(target))
        for target in Array(generations.keys) where !desiredTargets.contains(target) {
            generations[target] = (generations[target] ?? 0) + 1
        }
        for target in Array(instances.keys) where !desiredTargets.contains(target) {
            _ = try removeTarget(target)
        }
        var failures: [String] = []
        try await coalescingRoutingRebuilds {
        for item in saved {
            try Task.checkCancellation()
            do {
                let target = try target(item)
                if let existing=instances[target],isVerifiedMigration(item,item:existing),!forceStates {
                    // An ordinary project edit may still carry the original
                    // import descriptor. Keep any subsequent live VST3 edits;
                    // the next snapshot promotes its actual canonical state.
                    if let bypass=item["bypass"] as? Bool { existing.node.auAudioUnit.shouldBypassEffect=bypass }
                    continue
                }
                if let existing = instances[target],
                   item["component"] as? String == existing.component.id,
                   (item["instanceID"] == nil || item["instanceID"] as? String == existing.instanceID),
                   (item["format"] as? String ?? existing.component.format) == existing.component.format,
                   item["bundlePath"] as? String == existing.component.bundlePath,
                   existing.ownerID == ownersByIndex[target.track] {
                    if let state = item["state"] as? String, !forceStates,
                       lastRestoredStates[target] != state, capturedState(existing.node.auAudioUnit) == state {
                        // The interface adopted a state read from this very instance.
                        // Re-applying it would pause audio and could only revert edits
                        // made since; it becomes the reconciliation baseline instead.
                        lastRestoredStates[target] = state
                    } else if let state = item["state"] as? String, forceStates || lastRestoredStates[target] != state {
                        do {
                            try restoreTrackedState(state,to:existing)
                        } catch {
                            // This is a committed project state, so a failed patch must
                            // leave the target silent. Keeping the prior live patch would
                            // let a later snapshot overwrite the failed desired state.
                            // Direct load() still preserves its old instance on failure.
                            _ = try? removeTarget(target)
                            throw error
                        }
                        lastRestoredStates[target] = state
                        publish(RenderEndpoint(existing), for: target)
                    }
                    existing.nks = item["nks"] as? [String: Any]
                    if let bypass = item["bypass"] as? Bool { existing.node.auAudioUnit.shouldBypassEffect = bypass }
                } else {
                    // Reconciliation applies an already committed project selection.
                    // A missing replacement must leave silence rather than playing an
                    // unrelated old instrument. Direct load() remains transactional so
                    // a failed, uncommitted browser choice preserves the current unit.
                    if instances[target] != nil {
                        _ = try removeTarget(target)
                    }
                    _ = try await load(item)
                }
            }
            catch is CancellationError { throw CancellationError() }
            catch PluginRecoveryStore.RecoveryError.reviewRequired {
                // Quarantined slots remain in the desired project and are explained
                // by the recovery dialog. They are expected skips, not load failures.
                continue
            }
            catch { failures.append("\(item["name"] as? String ?? "Audio Unit"): \(error.localizedDescription)") }
        }
        }
        try Task.checkCancellation()
        refreshLatencyCompensation()
        if !failures.isEmpty { throw RackError.invalid(failures.joined(separator: "\n")) }
    }

    @MainActor func editor(_ payload: [String: Any]) async throws -> Any {
        let item = try instance(payload)
        if let window = editorWindows[item.target] {
            window.makeKeyAndOrderFront(nil)
            return ["shown": true, "name": item.component.name]
        }
        // One editor per slot: a second request must not start while the first is still creating its view.
        let opening = item.target
        guard editorsOpening.insert(opening).inserted else { return ["shown": true, "name": item.component.name] }
        defer { editorsOpening.remove(opening) }
        let controller: NSViewController? = await withCheckedContinuation { continuation in
            item.node.auAudioUnit.requestViewController { controller in continuation.resume(returning: controller) }
        }
        // An AU may have been removed while its editor was being instantiated.
        guard instances[item.target] === item else { throw RackError.invalid("This Audio Unit was removed while its editor was opening.") }
        let viewController = controller ?? GenericAudioUnitEditor(unit: item.node.auAudioUnit)
        let window = NSWindow(contentViewController: viewController)
        window.title = "\(item.component.name) · Track \(item.target.track + 1)"
        window.styleMask.formUnion([.titled, .closable, .miniaturizable, .resizable])
        window.isReleasedWhenClosed = false
        window.setContentSize(NSSize(width: max(500, viewController.view.frame.width),
                                     height: max(360, viewController.view.frame.height)))
        window.center(); window.makeKeyAndOrderFront(nil)
        editorWindows[item.target] = window
        return ["shown": true, "custom": controller != nil, "name": item.component.name] as [String: Any]
    }
}

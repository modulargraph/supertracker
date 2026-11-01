import AppKit
import AudioToolbox
import AVFoundation
import CoreAudioKit
import CoreFoundation
import Darwin

/// Serialized unit state: capture preparation, encoding, restoring and VST3 state access.
extension AudioPluginRack {
    /// Each caller owns only its own tokens. A cancelled older request cannot
    /// revoke a newer preparation on the same processor.
    struct StateCapturePreparation {
        let cancel: () -> Void
    }

    /// Control-thread graph/transport edits invalidate a suspended capture even
    /// when a stop/restart finishes with the same isRunning value. The state
    /// transaction's own bounded pause/resume deliberately does not call this.
    func notifyStateCaptureMutation() {
        precondition(Thread.isMainThread)
        stateCaptureEpoch &+= 1
    }

    /// Profiled dirty processors share one event-loop settlement interval. The
    /// helper owns no render lease across its suspension, and opaque capture still
    /// occurs synchronously under withVST3StateAccess after these checks succeed.
    @MainActor func prepareStateCapture(_ payload: [String:Any]? = nil,
                                        isCurrent: @escaping () -> Bool = { true }) async throws -> StateCapturePreparation {
        let selected: [Instance]
        if let payload, !payload.isEmpty { selected = instances[try target(payload)].map { [$0] } ?? [] }
        else { selected = Array(instances.values) }
        let profiled = selected.compactMap { item -> (Instance,VST3AudioUnit)? in
            guard let adapter = item.node.auAudioUnit as? VST3AudioUnit,
                  adapter.requiredStateFlushFrames > 0 else { return nil }
            return (item,adapter)
        }
        guard !profiled.isEmpty else { return StateCapturePreparation(cancel:{}) }
        let epoch = stateCaptureEpoch
        let targets = profiled.map { item,adapter in
            VST3StatePreparation.CaptureTarget(snapshot:{ adapter.stateCaptureSnapshot },
                isCurrent:{ self.stateCaptureEpoch == epoch && self.instances[item.target] === item && isCurrent() },
                beginCapture:{ try adapter.beginStateCapture(allowStoppedAudioAdvance:$0) },
                cancelCapture:{ adapter.cancelStateCapture(token:$0) })
        }
        let prepared = try await VST3StatePreparation.prepareCaptures(targets:targets,
            isGraphRunning:{ self.engine.isRunning },
            performBegins:{ operations in try self.withVST3StateAccess(operations) })
        let captureToken=UUID()
        capturePreparationTokens.insert(captureToken)
        let lease = StateCapturePreparation(cancel:{
            guard self.capturePreparationTokens.remove(captureToken) != nil else { return }
            for entry in prepared { profiled[entry.index].1.cancelStateCapture(token:entry.preparation.token) }
        })
        do {
            try Task.checkCancellation()
            guard isCurrent(), stateCaptureEpoch == epoch,
                  profiled.allSatisfy({ self.instances[$0.0.target] === $0.0 }) else {
                throw RackError.invalid("The project or audio graph changed while plug-in edits were being saved. Retry saving.")
            }
            for entry in prepared { try profiled[entry.index].1.acceptStateCapturePreparation(entry.preparation) }
            return lease
        } catch {
            lease.cancel()
            throw error
        }
    }

    func encodedState(_ unit: AUAudioUnit, allowStoppedAudioAdvance:Bool? = nil) throws -> String {
        let state = try readEncodedState(unit, allowStoppedAudioAdvance: allowStoppedAudioAdvance)
        lastCapturedStates = lastCapturedStates.filter { $0.value.unit != nil }
        lastCapturedStates[ObjectIdentifier(unit)] = CapturedState(unit: unit, state: state)
        return state
    }

    private func readEncodedState(_ unit: AUAudioUnit, allowStoppedAudioAdvance:Bool?) throws -> String {
        if let adapter = unit as? BuiltinEffectsAudioUnit {
            // Like native AU plists, first-party dictionaries can serialize in
            // a different key order. Always capture actual DSP state, then
            // reuse bytes only when the bounded typed comparison proves equality.
            let capture = { try PropertyListSerialization.data(fromPropertyList:adapter.documentState(),format:.binary,options:0) }
            return try auStateReuse.capture(for:ObjectIdentifier(unit),readFreshEncoded:capture).data.base64EncodedString()
        }
        if let adapter = unit as? VST3AudioUnit {
            let wasRunning = engine.isRunning
            // Saving while audio runs is ordinary DAW behaviour: VST3 getState is a
            // UI-thread call that plug-ins serve concurrently with process(). Pausing
            // the graph here made every autosave and project publication an audible
            // gap (one IO restart per VST3 instance). Only queued UI edits need the
            // render thread first; give it a few blocks, and fall back to the drained
            // capture only when audio is not delivering them.
            if wasRunning {
                let deadline = ProcessInfo.processInfo.systemUptime + 0.05
                repeat {
                    if let state = try adapter.documentStateConcurrent() {
                        concurrentStateCaptures &+= 1
                        return try PropertyListSerialization.data(fromPropertyList: state, format: .binary, options: 0).base64EncodedString()
                    }
                    usleep(500)
                } while ProcessInfo.processInfo.systemUptime < deadline && engine.isRunning
            }
            let state = try withVST3StateAccess { try adapter.documentState(allowStoppedAudioAdvance: allowStoppedAudioAdvance ?? !wasRunning) }
            return try PropertyListSerialization.data(fromPropertyList: state, format: .binary, options: 0).base64EncodedString()
        }
        if let adapter = unit as? VST2AudioUnit {
            // effGetChunk / getParameter are what every host calls from its UI thread while
            // processing runs. Pausing here stopped the whole engine (a full IO restart while
            // line inputs are open) on every 20 s recovery save: a regular audible dropout.
            let state = try adapter.documentState()
            return try PropertyListSerialization.data(fromPropertyList:state,format:.binary,options:0).base64EncodedString()
        }
        return try auStateReuse.capture(for:ObjectIdentifier(unit)) {
            let state = unit.fullStateForDocument ?? unit.fullState ?? [:]
            return try PropertyListSerialization.data(fromPropertyList:state,format:.binary,options:0)
        }.data.base64EncodedString()
    }

    /// `inGraph` false: an unattached candidate (prepareUnit). Nothing renders it, so restoring
    /// its state never needs to suspend the graph that keeps playing the current plug-ins.
    func restoreState(_ text: String, to unit: AUAudioUnit, inGraph: Bool = true) throws {
        notifyStateCaptureMutation()
        lastCapturedStates.removeValue(forKey: ObjectIdentifier(unit))
        guard let data = Data(base64Encoded: text), data.count <= 128 * 1024 * 1024,
              let state = try PropertyListSerialization.propertyList(from: data, options: [], format: nil) as? [String: Any] else {
            throw RackError.invalid("The saved Audio Unit state is not a valid property list.")
        }
        if let adapter = unit as? BuiltinEffectsAudioUnit {
            try adapter.restoreDocumentState(state)
            auStateReuse.seed(data,for:ObjectIdentifier(unit))
        } else if let adapter = unit as? VST3AudioUnit {
            if inGraph { try withVST3StateAccess { try adapter.restoreDocumentState(state) } }
            else { try adapter.restoreDocumentState(state) }
        } else if let adapter = unit as? VST2AudioUnit {
            if inGraph { try withVST3StateAccess { try adapter.restoreDocumentState(state) } }
            else { try adapter.restoreDocumentState(state) }
        } else {
            unit.fullStateForDocument = state
            // The AU setter has no throwing acknowledgment. Seeding preserves
            // the saved encoding only if the NEXT fresh getter proves equality;
            // an ignored or normalized restore therefore returns actual state.
            auStateReuse.seed(data,for:ObjectIdentifier(unit))
        }
    }

    /// VST3 processor state is a main-thread operation. Suspend graph rendering for
    /// the state transaction instead of letting a vendor serialize its live DSP concurrently.
    func withVST3StateAccess<T>(_ body: () throws -> T) throws -> T {
        let wasRunning = engine.isRunning
        if wasRunning { pauseEngine() }
        do {
            guard endpointStore.activeReaders == 0 else {
                throw RackError.invalid("The audio render is still finishing. Retry the plug-in state operation.")
            }
            let value = try body()
            if wasRunning { try startEngine() }
            return value
        } catch {
            if wasRunning { try? startEngine() }
            throw error
        }
    }

    func componentID(_ description: AudioComponentDescription) -> String {
        "\(description.componentType):\(description.componentSubType):\(description.componentManufacturer)"
    }

    func componentDescription(_ identifier: String) throws -> AudioComponentDescription {
        let values = identifier.split(separator: ":").compactMap { UInt32($0) }
        guard values.count == 3,
              [kAudioUnitType_MusicDevice, kAudioUnitType_Effect, kAudioUnitType_MusicEffect].contains(values[0]) else {
            throw RackError.invalid("The Audio Unit identifier is invalid.")
        }
        return AudioComponentDescription(componentType: values[0], componentSubType: values[1],
            componentManufacturer: values[2], componentFlags: 0, componentFlagsMask: 0)
    }

    func address(_ value: Any?) -> UInt64? {
        if let text = value as? String { return UInt64(text) }
        return (value as? NSNumber)?.uint64Value
    }
}

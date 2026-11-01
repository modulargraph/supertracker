import Foundation

/// MIDI clock output (MIDIClockOutput): settings, persistence and the per-output targets.
extension AudioHost {
    /// Rebuilds the clock targets from the saved choice and the MIDI outputs present now.
    func refreshMIDIClock() {
        guard !closed else { return }
        let preferences = MIDIClockPreferences.load(midiClockPreferencesURL)
        var targets: [MIDIClockOutput.Target] = []
        if preferences.enabled {
            for output in (midi.devices()["outputs"] as? [[String: Any]]) ?? [] {
                guard let id = (output["id"] as? NSNumber)?.int32Value ?? (output["id"] as? Int).map(Int32.init),
                      output["offline"] as? Bool != true else { continue }
                let device = preferences.device(id)
                guard device.clock, let route = midi.prepareDestination(id) else { continue }
                targets.append(.init(route: route, transport: device.transport))
            }
        }
        midiClock.setTargets(targets)
        let lead = (preferences.leadMilliseconds ?? automaticClockLeadMilliseconds()) / 1000
        midiClock.setLead(lead)
        // Notes to an External MIDI instrument carry the same render host-time stamp as the
        // clock. Heard back through a compensated line input, they need the same lead, or
        // they land that much (about one IO buffer) behind the rest of the mix.
        returnedNoteLeadSeconds = rack.hardwareReturnFrames > 0 && !lineInputs.isEmpty ? lead : 0
        refreshMIDIRoutes()
    }

    /// The live inputs' return path: what a clocked device's audio needs to reach the engine.
    /// While hardware returns are compensated (the mix waits `hardwareReturnSeconds` for
    /// them), the clock is needed that much less early.
    func automaticClockLeadMilliseconds() -> Double {
        let device = hardwareInputDevice()
        let compensated = rack.hardwareReturnFrames > 0 && !lineInputs.isEmpty ? AudioDeviceSelection.hardwareReturnSeconds(device) : 0
        return (max(0, AudioDeviceSelection.inputPathSeconds(device) - compensated) * 1000).rounded()
    }

    /// Recomputes the hardware-return compensation for the current input device and buffer,
    /// then the plug-in delay plan and the clock lead that depend on it.
    func refreshHardwareReturnLatency(replan: Bool = true) {
        let frames = Int((AudioDeviceSelection.hardwareReturnSeconds(hardwareInputDevice()) * sampleRate).rounded())
        guard frames != rack.hardwareReturnFrames else { return }
        rack.hardwareReturnFrames = frames
        if replan { rack.refreshLatencyCompensation() }
        refreshMIDIClock()
    }

    func midiClockStatus() -> [String: Any] {
        let preferences = MIDIClockPreferences.load(midiClockPreferencesURL)
        let outputs = (midi.devices()["outputs"] as? [[String: Any]]) ?? []
        return ["enabled": preferences.enabled, "running": graph.isRunning,
                "leadMilliseconds": preferences.leadMilliseconds as Any? ?? NSNull(), "automaticLeadMilliseconds": automaticClockLeadMilliseconds(), "devices": outputs.compactMap { output -> [String: Any]? in
            guard let id = (output["id"] as? NSNumber)?.int32Value ?? (output["id"] as? Int).map(Int32.init) else { return nil }
            let device = preferences.device(id)
            return ["id": Int(id), "name": output["name"] ?? "MIDI output", "offline": output["offline"] ?? false,
                    "clock": device.clock, "transport": device.transport]
        }]
    }

    /// `midi.clock`: no payload reads the settings; `enabled` and/or `devices` [{id, clock, transport}] change them.
    @MainActor func configureMIDIClock(_ payload: [String: Any]) throws -> [String: Any] {
        if payload["enabled"] != nil || payload["devices"] != nil || payload["leadMilliseconds"] != nil {
            var preferences = MIDIClockPreferences.load(midiClockPreferencesURL)
            preferences.apply(payload)
            try preferences.save(midiClockPreferencesURL)
            refreshMIDIClock()
            // The clock runs with the audio engine, also while the transport is stopped.
            if preferences.enabled && !graph.isRunning && !exporting && !graph.isInManualRenderingMode { try? start() }
        }
        return midiClockStatus()
    }
}

import Foundation
import CoreAudio

extension AudioHost {
    /// Applies the saved interface at launch, then re-resolves the project's live inputs against it.
    @MainActor func restoreSavedAudioDevices() async {
        guard let url = devicePreferencesURL else { return }
        let saved = AudioDevicePreferences.load(url)
        let devices = LineInputTopology.objectList(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDevices)
        let payload = saved.payload(present: devices)
        watchSavedAudioDevices()
        guard !payload.isEmpty else { return }
        restoringDevicePreferences = true
        defer { restoringDevicePreferences = false }
        do {
            _ = try await handle("audio.configure", payload: payload)
            try synchronizeLineInputs(project, force: true)
            recheckLineInputsSoon()
        } catch {
            emit("audio.error", ["error": "Could not restore your audio interface: \(error.localizedDescription) Choose it again in Settings."])
        }
        emit("audio.devices", audioDevices())
    }

    /// The saved interface may be off at launch (the system default plays) or come back after
    /// it was switched off. Each time the device list changes, apply the saved choice again
    /// when one of its devices is present but not the one the engine uses.
    @MainActor func watchSavedAudioDevices() {
        guard savedDeviceObserver == nil else { return }
        savedDeviceObserver = AudioDeviceListObserver { [weak self] in
            Task { @MainActor [weak self] in
                // A switched-on interface publishes its streams a moment after it appears.
                try? await Task.sleep(nanoseconds: 1_000_000_000)
                await self?.restoreSavedAudioDevicesIfDifferent()
            }
        }
    }

    @MainActor func restoreSavedAudioDevicesIfDifferent() async {
        guard let url = devicePreferencesURL, !closed, !restoringDevicePreferences, !exporting,
              !graph.isInManualRenderingMode, recording == nil, !recordingStopping, !freezeTrackRuntime.busy else { return }
        let devices = LineInputTopology.objectList(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDevices)
        let payload = AudioDevicePreferences.load(url).payload(present: devices)
        let output = (payload["outputDevice"] as? NSNumber)?.uint32Value, input = (payload["inputDevice"] as? NSNumber)?.uint32Value
        let current = currentIODevices()
        // A separate input and output run as one private aggregate on the shared HAL unit, which then reports
        // the aggregate instead of either device: compare with the selection the aggregate was built from.
        // (Comparing the IDs alone restarted audio on every device-list change for such a pair.)
        func applied(_ saved: AudioDeviceID?, engine: AudioDeviceID?, selected: AudioDeviceID?) -> Bool {
            guard let saved else { return true }
            if saved == engine { return true }
            guard saved == selected, let engine else { return false }
            return AudioDeviceSelection.isAggregate(engine, containing: saved)
        }
        guard !applied(output, engine: current.output, selected: selectedOutputDevice)
            || !applied(input, engine: current.input ?? selectedInputDevice, selected: selectedInputDevice) else { return }
        await restoreSavedAudioDevices()
    }

    /// Records a successful `audio.configure` request (not the launch restore itself).
    func rememberAudioDevices(_ payload: [String: Any]) {
        guard let url = devicePreferencesURL, !restoringDevicePreferences else { return }
        var saved = AudioDevicePreferences.load(url)
        saved.record(payload, output: selectedOutputDevice, input: selectedInputDevice, channels: selectedInputChannels)
        do { try saved.save(url) } catch { emit("audio.error", ["error": "Could not save the audio interface choice: \(error.localizedDescription)"]) }
    }
}

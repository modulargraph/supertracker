import Foundation

/// A live snapshot may retain an older opaque state when capture fails. Such a
/// descriptor is useful for display, but must never be mistaken for a saved patch.
enum PluginSnapshotCapture {
    static func states(from response: Any) throws -> [[String: Any]] {
        guard let snapshot = response as? [String: Any],
              let states = snapshot["audioPlugins"] as? [[String: Any]] else {
            throw TrackerError.message("The audio engine did not return a valid plug-in state snapshot. Retry saving; the existing project file has not been replaced.")
        }
        for (index, descriptor) in states.enumerated() {
            let name = (descriptor["name"] as? String).flatMap { $0.isEmpty ? nil : $0 }
                ?? (descriptor["component"] as? String).flatMap { $0.isEmpty ? nil : $0 }
                ?? "Plug-in \(index + 1)"
            if let error = descriptor["stateError"] {
                let reason = (error as? String).flatMap { $0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : $0 }
                    ?? "The current state could not be captured."
                throw TrackerError.message("Could not capture \(name): \(reason) Retry saving before closing the project; its previous saved state has not been replaced.")
            }
            // State is intentionally opaque. In particular, unavailable or recovery-
            // blocked desired instances must retain their exact existing bytes.
            if let state = descriptor["state"], !(state is String) {
                throw TrackerError.message("\(name) returned an invalid plug-in state. Retry saving; the existing project file has not been replaced.")
            }
        }
        guard JSONSerialization.isValidJSONObject(states) else {
            throw TrackerError.message("The audio engine returned plug-in state that cannot be saved. Retry saving; the existing project file has not been replaced.")
        }
        return states
    }
}

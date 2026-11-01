import Foundation
import CoreAudio

/// The audio interface chosen in Settings, kept across launches. Device IDs change between launches and
/// reconnections, so the choice is stored by Core Audio UID. A device that is absent at launch is skipped
/// (the system default plays) and the choice stays saved for the next launch.
struct AudioDevicePreferences: Codable, Equatable {
    var outputUID: String?
    var inputUID: String?
    var inputChannels: [Int]?
    var bufferSize: Int?

    static func load(_ url: URL) -> AudioDevicePreferences {
        (try? JSONDecoder().decode(AudioDevicePreferences.self, from: Data(contentsOf: url))) ?? AudioDevicePreferences()
    }
    func save(_ url: URL) throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try JSONEncoder().encode(self).write(to: url, options: .atomic)
    }

    /// Folds a successful `audio.configure` request into the saved choice.
    /// `outputSystemDefault` / `inputSystemDefault` mean the user chose "System default".
    mutating func record(_ payload: [String: Any], output: AudioDeviceID?, input: AudioDeviceID?, channels: [Int]) {
        if payload["outputSystemDefault"] as? Bool == true { outputUID = nil }
        else if payload["outputDevice"] != nil || payload["output"] != nil, let output { outputUID = AudioDeviceSelection.uid(output) }
        if payload["inputSystemDefault"] as? Bool == true { inputUID = nil }
        else if payload["inputDevice"] != nil || payload["input"] != nil, let input { inputUID = AudioDeviceSelection.uid(input) }
        if payload["inputChannels"] != nil { inputChannels = channels }
        if let frames = payload["bufferSize"] as? NSNumber { bufferSize = frames.intValue }
    }

    /// The `audio.configure` payload that restores this choice with the devices present now.
    func payload(present devices: [AudioDeviceID]) -> [String: Any] {
        func device(_ uid: String?) -> AudioDeviceID? {
            guard let uid, !uid.isEmpty else { return nil }
            return devices.first { AudioDeviceSelection.uid($0) == uid }
        }
        var result: [String: Any] = [:]
        if let output = device(outputUID) { result["outputDevice"] = NSNumber(value: output) }
        if let input = device(inputUID) {
            result["inputDevice"] = NSNumber(value: input)
            if let inputChannels, !inputChannels.isEmpty,
               inputChannels.allSatisfy({ $0 >= 0 && $0 < AudioDeviceSelection.channels(input, input: true) }) { result["inputChannels"] = inputChannels }
        }
        if let bufferSize { result["bufferSize"] = NSNumber(value: bufferSize) }
        return result
    }
}

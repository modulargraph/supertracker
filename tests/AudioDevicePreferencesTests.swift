import Foundation

// The Settings audio interface is remembered by UID across launches (AudioDevicePreferences).
@main
struct AudioDevicePreferencesTests {
    static func main() throws {
        func require(_ ok: Bool, _ message: String) { if !ok { print("FAIL audio device preferences: \(message)"); exit(1) } }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("st-audio-devices-\(UUID().uuidString).json")
        defer { try? FileManager.default.removeItem(at: url) }
        require(AudioDevicePreferences.load(url) == AudioDevicePreferences(), "a missing file is an empty choice")

        var saved = AudioDevicePreferences(outputUID: "AudioBox", inputUID: "AudioBox", inputChannels: [4, 5], bufferSize: 512)
        try saved.save(url)
        require(AudioDevicePreferences.load(url) == saved, "the choice survives a save and load")

        saved.record(["outputDevice": 7, "outputSystemDefault": true, "bufferSize": 256], output: 7, input: nil, channels: [4, 5])
        require(saved.outputUID == nil && saved.inputUID == "AudioBox" && saved.bufferSize == 256, "System default clears only the output choice: \(saved)")
        saved.record(["midi": true], output: 7, input: 9, channels: [0, 1])
        require(saved.inputUID == "AudioBox" && saved.inputChannels == [4, 5], "a request without devices keeps the saved ones: \(saved)")

        // An absent interface is skipped (system default plays) and only the buffer size is restored.
        let absent = AudioDevicePreferences(outputUID: "Gone:1,2", inputUID: "Gone:1,2", inputChannels: [4, 5], bufferSize: 512)
        let payload = absent.payload(present: [])
        require(payload.count == 1 && (payload["bufferSize"] as? NSNumber)?.intValue == 512, "an absent interface was restored: \(payload)")
        print("PASS audio device preferences: saved by UID, System default clears it, absent devices are skipped")
    }
}

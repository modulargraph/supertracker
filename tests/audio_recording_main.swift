// Generated fixtures only; this runner opens no audio engine or hardware input.
// Runs the native recording checks (also part of `--self-test`) without the app; see scripts/test-steps.sh.
import Foundation

@main
struct AudioRecordingCheck {
    static func main() async throws {
        let output = URL(fileURLWithPath: CommandLine.arguments.dropFirst().first ?? "/tmp/supertracker-recording-tests", isDirectory: true)
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        let report = output.appendingPathComponent("recording-test-report.json")
        do {
            let passed = try await NativeAudioRecordingTests.run(output: output)
            for name in passed { print("PASS " + name) }
            try JSONSerialization.data(withJSONObject: ["passed": true, "checks": passed], options: [.prettyPrinted, .sortedKeys])
                .write(to: report, options: .atomic)
        } catch {
            try? JSONSerialization.data(withJSONObject: ["passed": false, "error": error.localizedDescription], options: [.prettyPrinted, .sortedKeys])
                .write(to: report, options: .atomic)
            throw error
        }
        print("Recording channel tests passed: " + output.path)
    }
}

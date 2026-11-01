import AppKit

/// Original/Apple-only native audio acceptance. This entry never scans or opens
/// an installed VST3, opens a live output device or creates a user window.
@main enum NativeRoutingDiagnostic {
    @MainActor static func main() {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        // Catalogue discovery invokes this same executable in an isolated child.
        // Dispatch before the regression runner so a successful child exit means
        // the requested atomic factory report was actually written. This runner
        // admits only its original fixture, never an installed vendor module.
        if let index = CommandLine.arguments.firstIndex(of:"--vst3-scan") {
            guard CommandLine.arguments.indices.contains(index+1),
                  let outputIndex=CommandLine.arguments.firstIndex(of:"--output"),
                  CommandLine.arguments.indices.contains(outputIndex+1) else {
                fputs("Usage: --vst3-scan <original timing fixture> --output <report.json>\n",stderr)
                exit(64)
            }
            let fixture=URL(fileURLWithPath:CommandLine.arguments[index+1]).standardizedFileURL
            let originalBundles = ["studio.supertracker.tests.timing-probe", "studio.supertracker.tests.fixture"]
            guard let bundleID=Bundle(url:fixture)?.bundleIdentifier,originalBundles.contains(bundleID) else {
                fputs("This diagnostic scanner accepts only the original timing or deterministic lifecycle fixture.\n",stderr)
                exit(64)
            }
            exit(VST3Catalogue.runScanCommand(bundle:fixture,
                output:URL(fileURLWithPath:CommandLine.arguments[outputIndex+1])))
        }
        Task { @MainActor in
            let outputIndex = CommandLine.arguments.firstIndex(of: "--output")
            let output = outputIndex.flatMap { CommandLine.arguments.indices.contains($0 + 1) ? URL(fileURLWithPath: CommandLine.arguments[$0 + 1]) : nil }
                ?? URL(fileURLWithPath:FileManager.default.currentDirectoryPath).appendingPathComponent("test-output")
            let inputAck = CommandLine.arguments.contains("--input-ack")
            let recordingFence = CommandLine.arguments.contains("--recording-fence")
            let full = CommandLine.arguments.contains("--regression")
            let auState = CommandLine.arguments.contains("--au-state-reuse")
            let stepFX = CommandLine.arguments.contains("--plugin-step-fx")
            let mixTiming = CommandLine.arguments.contains("--mix-timing")
            let name = inputAck ? "input-ack-report.json" : recordingFence ? "recording-fence-report.json" : mixTiming ? "mix-timing-report.json" : auState ? "au-state-reuse-report.json" : stepFX ? "plugin-step-fx-report.json" : full ? "audio-host-regression-report.json" : "routing-host-regression-report.json"
            let reportURL = output.appendingPathComponent(name)
            var checks:[String] = []
            do {
                try FileManager.default.createDirectory(at:output,withIntermediateDirectories:true)
                try? FileManager.default.removeItem(at:reportURL)
                if inputAck { checks = try await NativeInputAcknowledgementTests.run(output:output) }
                else if recordingFence { checks = try await NativeRecordingFenceTests.run(output:output) }
                else if mixTiming { checks = try await NativeAUMixTimingTests.run(output:output) }
                else if auState { checks = try await NativeAUStateReuseTests.run(output:output) }
                else if stepFX { checks = try await NativePluginStepFXTests.run(output:output) }
                else { checks = full ? try await NativeAudioTests.run(output:output) : try await NativeRoutingHostTests.run(output:output) }
                for check in checks { print("PASS \(check)") }
                if full && !inputAck && !recordingFence && !auState && !stepFX && !mixTiming {
                    let recovery = try await NativePluginRecoveryTests.run(output:output)
                    for check in recovery { print("PASS \(check)") }
                    checks += recovery
                }
                let report:[String:Any] = ["passed":true,"checks":checks,"generatedAt":ISO8601DateFormatter().string(from:Date()),
                    "scope":"production audio source with original and Apple fixtures","thirdPartyPlugins":false,"physicalPlayback":false]
                try JSONSerialization.data(withJSONObject:report,options:[.prettyPrinted,.sortedKeys]).write(to:reportURL,options:.atomic)
                exit(0)
            } catch {
                let failure:[String:Any] = ["passed":false,"checks":checks,"error":error.localizedDescription,
                    "generatedAt":ISO8601DateFormatter().string(from:Date()),"thirdPartyPlugins":false,"physicalPlayback":false]
                if let data = try? JSONSerialization.data(withJSONObject:failure,options:[.prettyPrinted,.sortedKeys]) {
                    try? data.write(to:reportURL,options:.atomic)
                }
                fputs("FAIL \(error.localizedDescription)\n",stderr); exit(1)
            }
        }
        app.run()
    }
}

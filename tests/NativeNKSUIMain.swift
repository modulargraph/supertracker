import AppKit
import Foundation
import VST3Host

@_silgen_name("st_install_nks_diagnostic_guard")
private func installDiagnosticGuard() -> Int32

/// Dedicated installed-NKS UI acceptance entry; never linked into SuperTracker.app.
@main enum NativeNKSUIMain {
    @MainActor static func main() {
        guard installDiagnosticGuard() == 1 else {
            fputs("Unable to install diagnostic fatal-signal guard.\n", stderr); exit(2)
        }
        if CommandLine.arguments.contains("--diagnostic-guard-check") {
            print("SUPERTRACKER_NKS_UI_DIAGNOSTIC_GUARD=1"); exit(0)
        }
        let app = NSApplication.shared; app.setActivationPolicy(.accessory)
        func argument(_ name: String) -> String? {
            guard let index = CommandLine.arguments.firstIndex(of: name), CommandLine.arguments.count > index + 1 else { return nil }
            return CommandLine.arguments[index + 1]
        }
        if let bundle = argument("--vst3-scan"), let output = argument("--output") {
            exit(VST3Catalogue.runScanCommand(bundle: URL(fileURLWithPath: bundle), output: URL(fileURLWithPath: output)))
        }
        guard let output = argument("--nks-ui-self-test"), let webRoot = argument("--web-root"),
              let runID = argument("--run-id"), UUID(uuidString: runID) != nil else {
            fputs("Use the bounded installed-NKS UI diagnostic runner.\n", stderr); exit(2)
        }
        Task { @MainActor in
            do {
                try await NativeNKSUIAcceptance.run(output: URL(fileURLWithPath: output),
                    webRoot: URL(fileURLWithPath: webRoot), runID: runID)
                st_vst3_release_unused_modules()
                exit(0)
            } catch {
                fputs("FAIL \(error.localizedDescription)\n", stderr)
                st_vst3_release_unused_modules()
                exit(1)
            }
        }
        app.run()
    }
}

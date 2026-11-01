import AppKit
import Foundation
import VST3Host

@_silgen_name("st_install_nks_diagnostic_guard")
private func installDiagnosticGuard() -> Int32

/// Standalone diagnostic entry point. This source and its fatal-signal guard are
/// excluded from Package.swift and every production app build.
@main enum NativeNKSMain {
    @MainActor static func main() {
        guard installDiagnosticGuard() == 1 else {
            fputs("The diagnostic fatal-signal guard could not be installed.\n", stderr)
            exit(2)
        }
        if CommandLine.arguments.contains("--diagnostic-guard-check") {
            print("SUPERTRACKER_NKS_DIAGNOSTIC_GUARD=1")
            exit(0)
        }
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        if let index = CommandLine.arguments.firstIndex(of: "--vst3-scan"),
           CommandLine.arguments.count > index + 1,
           let outputIndex = CommandLine.arguments.firstIndex(of: "--output"),
           CommandLine.arguments.count > outputIndex + 1 {
            exit(VST3Catalogue.runScanCommand(bundle: URL(fileURLWithPath: CommandLine.arguments[index + 1]),
                                             output: URL(fileURLWithPath: CommandLine.arguments[outputIndex + 1])))
        }
        if let index = CommandLine.arguments.firstIndex(of: "--vst3-fixture-self-test"),
           CommandLine.arguments.count > index + 1,
           let fixtureIndex = CommandLine.arguments.firstIndex(of: "--fixture-bundle"),
           CommandLine.arguments.count > fixtureIndex + 1 {
            let output = URL(fileURLWithPath: CommandLine.arguments[index + 1])
            let fixture = URL(fileURLWithPath: CommandLine.arguments[fixtureIndex + 1])
            Task { @MainActor in
                do {
                    _ = try await NativeVST3FixtureTests.run(output: output, fixtureBundle: fixture)
                    st_vst3_release_unused_modules()
                    exit(0)
                } catch {
                    st_vst3_release_unused_modules()
                    fputs("FAIL \(error.localizedDescription)\n", stderr)
                    exit(1)
                }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--vst3-capture-self-test"),
           CommandLine.arguments.count > index + 1,
           let fixtureIndex = CommandLine.arguments.firstIndex(of: "--fixture-bundle"),
           CommandLine.arguments.count > fixtureIndex + 1 {
            let output = URL(fileURLWithPath: CommandLine.arguments[index + 1])
            let fixture = URL(fileURLWithPath: CommandLine.arguments[fixtureIndex + 1])
            Task { @MainActor in
                do {
                    _ = try await NativeVST3CaptureTests.run(output: output, fixtureBundle: fixture)
                    st_vst3_release_unused_modules()
                    exit(0)
                } catch {
                    st_vst3_release_unused_modules()
                    fputs("FAIL \(error.localizedDescription)\n", stderr)
                    exit(1)
                }
            }
            app.run()
            return
        }
        guard let index = CommandLine.arguments.firstIndex(of: "--vst3-self-test"),
              CommandLine.arguments.count > index + 1 else {
            fputs("Use the bounded NKS diagnostic runner with an output directory.\n", stderr)
            exit(2)
        }
        let output = URL(fileURLWithPath: CommandLine.arguments[index + 1])
        let includeNKS = CommandLine.arguments.contains("--include-nks")
        Task { @MainActor in
            do {
                _ = try await NativeVST3Tests.run(output: output, includeNKS: includeNKS)
                st_vst3_release_unused_modules()
                exit(0)
            } catch {
                st_vst3_release_unused_modules()
                fputs("FAIL \(error.localizedDescription)\n", stderr)
                exit(1)
            }
        }
        app.run()
    }
}

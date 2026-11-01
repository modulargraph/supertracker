import AppKit
@main enum NativeHostingDebugMain {
    @MainActor static func main() {
        guard CommandLine.arguments.count == 3 else { fputs("Usage: NativeHostingDebug <original-bundle> <private-output>\n",stderr);exit(64) }
        let app=NSApplication.shared;app.setActivationPolicy(.prohibited)
        Task { @MainActor in
            let output=URL(fileURLWithPath:CommandLine.arguments[2],isDirectory:true)
            do {
                let report:[String:Any]
                if CommandLine.arguments[1] == "--midi-performance" {
                    report=try await NativeMIDIPerformanceDebugTests.run(output:output)
                } else if CommandLine.arguments[1] == "--freeze-checkpoint" {
                    report=try await NativeFreezeCheckpointDebugTests.run(output:output)
                } else {
                    report=try await NativeHostingDebugTests.run(output:output,bundle:URL(fileURLWithPath:CommandLine.arguments[1],isDirectory:true))
                }
                try JSONSerialization.data(withJSONObject:report,options:[.prettyPrinted,.sortedKeys]).write(to:output.appendingPathComponent("report.json"),options:.atomic)
                exit(report["passed"] as? Bool == true ? 0:1)
            } catch { fputs("FAIL \(error)\n",stderr);exit(1) }
        }
        app.run()
    }
}

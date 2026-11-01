import AppKit
import WebKit

#if SELFTESTS
/// Development-only UI driver. With SUPERTRACKER_DEV_SCRIPT=<steps.json> the app runs
/// each step after the interface is ready: {"js": "...", "wait": seconds, "size": [w, h], "snapshot": "name"}.
/// Results and PNG snapshots go to SUPERTRACKER_DEV_OUTPUT. Absent variables: no effect.
@MainActor
enum DevHarness {
    static var menuLog: [[String: Any]] = []

    static func startIfRequested(webView: WKWebView) {
        let environment = ProcessInfo.processInfo.environment
        guard let script = environment["SUPERTRACKER_DEV_SCRIPT"], let output = environment["SUPERTRACKER_DEV_OUTPUT"],
              let data = FileManager.default.contents(atPath: script),
              let steps = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]] else { return }
        let folder = URL(fileURLWithPath: output, isDirectory: true)
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        Task { @MainActor in
            var results: [[String: Any]] = []
            for (index, step) in steps.enumerated() {
                if let wait = (step["wait"] as? NSNumber)?.doubleValue { try? await Task.sleep(nanoseconds: UInt64(wait * 1e9)) }
                var record: [String: Any] = ["step": index]
                // {"size": [width, height]} resizes the content area (for layout checks at several window sizes).
                // The scripted window stops autosaving its frame first so the user's saved window size is untouched.
                if let size = step["size"] as? [NSNumber], size.count == 2, let window = webView.window {
                    window.setFrameAutosaveName("")
                    window.setContentSize(NSSize(width: size[0].doubleValue, height: size[1].doubleValue))
                    try? await Task.sleep(nanoseconds: 300_000_000)
                    record["size"] = [webView.bounds.width, webView.bounds.height]
                }
                if let js = step["js"] as? String {
                    do {
                        let value = try await webView.callAsyncJavaScript(js, arguments: [:], in: nil, contentWorld: .page)
                        record["value"] = jsonSafe(value)
                    } catch { record["error"] = error.localizedDescription }
                }
                if let name = step["snapshot"] as? String {
                    let configuration = WKSnapshotConfiguration()
                    if let image = try? await webView.takeSnapshot(configuration: configuration),
                       let tiff = image.tiffRepresentation, let bitmap = NSBitmapImageRep(data: tiff),
                       let png = bitmap.representation(using: .png, properties: [:]) {
                        try? png.write(to: folder.appendingPathComponent(name + ".png"))
                        record["snapshot"] = name + ".png"
                    }
                }
                if !menuLog.isEmpty { record["menus"] = menuLog; menuLog = [] }
                results.append(record)
                if let json = try? JSONSerialization.data(withJSONObject: results, options: [.prettyPrinted]) {
                    try? json.write(to: folder.appendingPathComponent("results.json"))
                }
            }
            if environment["SUPERTRACKER_DEV_QUIT"] != nil { _exit(0) }
        }
    }

    /// When scripted, context menus are recorded and answered without showing them.
    static var scriptedMenuPick: String?? {
        let environment = ProcessInfo.processInfo.environment
        guard environment["SUPERTRACKER_DEV_SCRIPT"] != nil else { return nil }
        return .some(environment["SUPERTRACKER_MENU_PICK"])
    }

    private static func jsonSafe(_ value: Any?) -> Any {
        guard let value else { return NSNull() }
        if JSONSerialization.isValidJSONObject(["v": value]) { return value }
        return String(describing: value)
    }
}
#endif

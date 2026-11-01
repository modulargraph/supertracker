import AppKit
import WebKit
import VST3Host

/// Clicks act immediately, even when SuperTracker is not the active window (a Mac app, not a web page).
/// The view fills the window, title bar included, so it also moves the window: window-drag.js reports
/// when the pointer is over empty top bar space, and a mouse-down there drags the window instead.
final class AppWebView: WKWebView {
    var windowDragRegion = false

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func mouseDown(with event: NSEvent) {
        guard windowDragRegion, let window else { return super.mouseDown(with: event) }
        if event.clickCount == 2 { Self.titleBarDoubleClick(window) } else { window.performDrag(with: event) }
    }

    /// System Settings → Desktop & Dock → "Double-click a window's title bar to" (zoom, minimise or nothing).
    private static func titleBarDoubleClick(_ window: NSWindow) {
        switch UserDefaults.standard.string(forKey: "AppleActionOnDoubleClick") {
        case "Minimize": window.performMiniaturize(nil)
        case "None": break
        default: window.performZoom(nil)
        }
    }
}

/// Receives window-drag.js reports. Weak, so the content controller does not keep the view alive.
final class WindowDragHandler: NSObject, WKScriptMessageHandler {
    weak var view: AppWebView?
    init(view: AppWebView) { self.view = view }
    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        view?.windowDragRegion = message.body as? Bool ?? false
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
    var window: NSWindow!
    var webView: WKWebView!
    var bridge: Bridge!
    var agentControl: AgentControlServer?
    private var terminationPending = false
    private var pendingProjectURL: URL?
    private let keyboardAcceptance: NativeKeyboardAcceptanceSession?
    private let requestTermination: @MainActor () -> Void

    init(keyboardAcceptance: NativeKeyboardAcceptanceSession? = nil,
         requestTermination: @escaping @MainActor () -> Void = { NSApp.terminate(nil) }) {
        self.keyboardAcceptance = keyboardAcceptance
        self.requestTermination = requestTermination
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        if let keyboardAcceptance {
            Task { @MainActor in
                do {
                    let services = try await keyboardAcceptance.prepare()
                    try createWindow(audio:services.audio,library:services.library,store:services.store,policy:services.policy)
                } catch { showStartupError(error) }
            }
            return
        }
        do {
            let store = try ProjectStore()
            let recovery = try PluginRecoveryStore(directory: store.support.appendingPathComponent("Plugins/Recovery", isDirectory: true))
            try recovery.startSession()
            let audio = try AudioHost(recoveryStore: recovery)
            audio.devicePreferencesURL = store.support.appendingPathComponent("audio-devices.json")
            audio.midiClockPreferencesURL = store.support.appendingPathComponent("midi-clock.json")
            audio.refreshMIDIClock()
            let library = LibraryService()
            try createWindow(audio:audio,library:library,store:store)
            Task { @MainActor in await audio.restoreSavedAudioDevices() }
        } catch { showStartupError(error) }
    }

    private func showStartupError(_ error: Error) {
        let alert = NSAlert(); alert.messageText = "SuperTracker could not start"; alert.informativeText = error.localizedDescription; alert.runModal(); NSApp.terminate(nil)
    }

    private func createWindow(audio: AudioHost, library: LibraryService, store: ProjectStore, policy: KeyboardAcceptancePolicy? = nil) throws {
        bridge = Bridge(audio: audio, library: library, store: store, keyboardAcceptance:policy)
        if let pendingProjectURL { bridge.requestOpenProject(pendingProjectURL); self.pendingProjectURL = nil }
        let configuration = WKWebViewConfiguration()
        if keyboardAcceptance != nil { configuration.websiteDataStore = .nonPersistent() }
        configuration.userContentController.add(bridge, name: "supertracker")
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
        configuration.preferences.tabFocusesLinks = true
        let appWebView = AppWebView(frame: .zero, configuration: configuration)
        configuration.userContentController.add(WindowDragHandler(view: appWebView), name: "windowDrag")
        webView = appWebView
        webView.navigationDelegate = bridge
        webView.isInspectable = true
        webView.allowsLinkPreview = false
        webView.allowsMagnification = false
        webView.allowsBackForwardNavigationGestures = false
        webView.setValue(false, forKey: "drawsBackground")
        bridge.webView = webView
        if keyboardAcceptance == nil { bridge.startMaschine() }
        if keyboardAcceptance == nil {
            // Local AI control (MCP): user-only socket, toggled in Settings.
            let control = AgentControlServer(webView: webView, settingsURL: store.support.appendingPathComponent("ai-control.json"), projectPath: { [weak bridge] in bridge?.store.url?.path })
            bridge.agentControl = control; agentControl = control
            control.start()
        }
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1500, height: 940), styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView], backing: .buffered, defer: false)
        window.title = keyboardAcceptance != nil ? NativeKeyboardAcceptanceSession.windowTitle : "SuperTracker"
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.backgroundColor = NSColor(calibratedRed: 0.055, green: 0.066, blue: 0.085, alpha: 1)
        window.appearance = NSAppearance(named: .darkAqua)
        window.minSize = NSSize(width: 1080, height: 700)
        window.contentView = webView
        window.delegate = self
        window.setFrameAutosaveName(keyboardAcceptance != nil ? "SuperTrackerKeyboardAcceptance" : "SuperTrackerMain")
        window.center()
        createMenus()
        let resource = Bundle.main.resourceURL?.appendingPathComponent("Web/index.html")
        let fallback = URL(fileURLWithPath: FileManager.default.currentDirectoryPath).appendingPathComponent("Resources/Web/index.html")
        let html = resource.flatMap { FileManager.default.fileExists(atPath: $0.path) ? $0 : nil } ?? fallback
        guard FileManager.default.fileExists(atPath: html.path) else { throw TrackerError.message("Interface resources are missing. Run scripts/build.sh and open build/SuperTracker.app.") }
        webView.loadFileURL(html, allowingReadAccessTo: html.deletingLastPathComponent())
        if ProcessInfo.processInfo.environment["SUPERTRACKER_DEV_SCRIPT"] != nil {
            // Scripted development runs stay invisible and never take keyboard focus from the user.
            window.alphaValue = 0.01; window.ignoresMouseEvents = true
            window.orderFrontRegardless()
            window.makeFirstResponder(webView)
        } else {
            window.makeKeyAndOrderFront(nil)
            window.makeFirstResponder(webView)
            NSApp.activate(ignoringOtherApps: true)
        }
    }

    /// Native menu bar. Items forward a command name to the web layer (app.js perform()).
    /// Key equivalents are only ones the web keyboard handler also owns globally: WKWebView offers
    /// each key to the page first, and a key the page handles (preventDefault) never reaches the menu,
    /// so a shortcut runs exactly once. Context-dependent keys (Space, ⌥ letters, ⌘D…) stay without
    /// equivalents here; they are listed in the help sheet and the command palette instead.
    private func createMenus() {
        let bar = NSMenu()
        func submenu(_ title: String) -> NSMenu {
            let menu = NSMenu(title: title); let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
            item.submenu = menu; bar.addItem(item); return menu
        }
        let appMenu = NSMenu(); let appItem = NSMenuItem(); appItem.submenu = appMenu; bar.addItem(appItem)
        appMenu.addItem(withTitle: "About SuperTracker", action: #selector(about), keyEquivalent: "")
        appMenu.addItem(.separator())
        add(appMenu, "Settings…", "settings", ",")
        appMenu.addItem(.separator())
        appMenu.addItem(withTitle: "Hide SuperTracker", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h")
        let hideOthers = appMenu.addItem(withTitle: "Hide Others", action: #selector(NSApplication.hideOtherApplications(_:)), keyEquivalent: "h")
        hideOthers.keyEquivalentModifierMask = [.command, .option]
        appMenu.addItem(withTitle: "Show All", action: #selector(NSApplication.unhideAllApplications(_:)), keyEquivalent: "")
        appMenu.addItem(.separator())
        appMenu.addItem(withTitle: "Quit SuperTracker", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")

        let file = submenu("File")
        add(file, "New Project", "new", "n"); add(file, "Open Project…", "open", "o"); file.addItem(.separator())
        add(file, "Save Project", "save", "s"); add(file, "Save Project As…", "saveAs", "s", modifiers: [.command, .shift])
        add(file, "Rename Session…", "renameProject", ""); file.addItem(.separator())
        add(file, "Import MIDI File…", "importMIDI", ""); add(file, "Import Audio Clip…", "importAudio", ""); add(file, "Load Sample…", "loadSample", "")
        file.addItem(.separator())
        add(file, "Export Audio…", "export", "e", modifiers: [.command, .shift]); add(file, "Export MIDI File…", "exportMIDI", "")
        file.addItem(.separator())
        add(file, "Record Audio Input…", "recordInput", "r", modifiers: [.command, .shift])
        add(file, "Relink Missing Media…", "relinkMedia", ""); add(file, "Review Interrupted Plug-ins…", "pluginRecovery", "")
        file.addItem(.separator())
        file.addItem(withTitle: "Close Window", action: #selector(NSWindow.performClose(_:)), keyEquivalent: "w")

        let edit = submenu("Edit")
        add(edit, "Undo", "undo", "z"); add(edit, "Redo", "redo", "z", modifiers: [.command, .shift]); edit.addItem(.separator())
        edit.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        edit.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        edit.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        add(edit, "Select All", "selectAll", "a"); edit.addItem(.separator())
        add(edit, "Fill Selected Phrase Down", "fill", ""); add(edit, "Clear Step Effects", "clearEffects", "")

        let view = submenu("View")
        for (index, (command, title)) in [("pattern", "Pattern"), ("instrument", "Instrument"), ("browser", "Browser"), ("mixer", "Mixer"), ("automation", "Automation"), ("song", "Song"), ("recording", "Recording")].enumerated() {
            add(view, title, "view:" + command, String(index + 1))
        }
        view.addItem(.separator())
        let f6 = String(Character(UnicodeScalar(NSF6FunctionKey)!))
        add(view, "Next Section", "nextPane", f6, modifiers: []); add(view, "Previous Section", "previousPane", f6, modifiers: [.shift])
        view.addItem(.separator())
        add(view, "Command Palette…", "palette", "k"); add(view, "Find Instrument Control…", "findControl", "")
        add(view, "Browse NKS Instruments", "browseNKS", "b", modifiers: [.command, .shift]); add(view, "Browse NKS Effects", "browseNKSEffects", "")
        add(view, "Browse Plug-ins", "browsePlugins", ""); add(view, "Send Effects…", "sendEffects", "x", modifiers: [.command, .shift])
        view.addItem(.separator())
        let fullScreen = view.addItem(withTitle: "Enter Full Screen", action: #selector(NSWindow.toggleFullScreen(_:)), keyEquivalent: "f")
        fullScreen.keyEquivalentModifierMask = [.command, .control]

        let transport = submenu("Transport")
        add(transport, "Play / Stop", "play", ""); add(transport, "Stop and Silence", "stop", "")
        add(transport, "Arm Note Recording", "record", "r"); add(transport, "Computer Keyboard Piano", "keyboard", "p")
        add(transport, "Metronome", "metronome", "m", modifiers: [.command, .shift]); transport.addItem(.separator())
        add(transport, "Song Playback", "modeSong", ""); add(transport, "Pattern Playback", "modePattern", "")
        add(transport, "Loop Selected Rows", "loopSelection", ""); add(transport, "Follow Playback", "follow", "")
        transport.addItem(.separator())
        add(transport, "Set Tempo…", "setTempo", "")
        transport.addItem(.separator())
        add(transport, "Automation Write On / Off", "automationWriteToggle", ""); add(transport, "Cycle Automation Write Mode", "automationWriteCycle", "")
        add(transport, "Disarm All Automation Parameters", "automationClearArms", "")

        let pattern = submenu("Pattern")
        add(pattern, "New Pattern", "addPattern", "n", modifiers: [.command, .shift]); add(pattern, "Duplicate Pattern", "duplicatePattern", "")
        add(pattern, "Rename Pattern…", "renamePattern", ""); add(pattern, "Set Pattern Length…", "patternLength", "")
        add(pattern, "Remove Pattern", "removePattern", ""); pattern.addItem(.separator())
        add(pattern, "Previous Pattern", "previousPattern", "["); add(pattern, "Next Pattern", "nextPattern", "]")
        pattern.addItem(.separator())
        add(pattern, "Insert Row", "insertRow", ""); add(pattern, "Delete Row", "deleteRow", ""); pattern.addItem(.separator())
        add(pattern, "Choose Step Effect…", "stepEffect", ""); add(pattern, "Automate Parameter…", "automateParameter", "")
        pattern.addItem(.separator())
        add(pattern, "Interpolate Velocity / FX", "interpolate", ""); add(pattern, "Humanize Velocity", "humanize", "")
        add(pattern, "Reverse Steps", "reverseSteps", ""); add(pattern, "Stretch ×2", "stretch", ""); add(pattern, "Compress ½", "compress", "")
        add(pattern, "Double Pattern Length", "doublePattern", ""); add(pattern, "Halve Pattern Length", "halvePattern", "")
        pattern.addItem(.separator())
        add(pattern, "Octave Up", "octaveUp", ""); add(pattern, "Octave Down", "octaveDown", "")

        let track = submenu("Track")
        add(track, "Add Track", "addTrack", "t"); add(track, "Duplicate Track", "duplicateTrack", "t", modifiers: [.command, .shift])
        add(track, "Rename Track…", "renameTrack", ""); add(track, "Delete Track…", "deleteTracks", ""); track.addItem(.separator())
        add(track, "Move Track Left", "moveTrackLeft", ""); add(track, "Move Track Right", "moveTrackRight", "")
        add(track, "Group Selected Tracks", "groupTracks", ""); add(track, "Take Track Out of Group", "ungroupTrack", "")
        add(track, "Hide / Show Empty Tracks", "hideEmptyTracks", ""); track.addItem(.separator())
        add(track, "Mute / Unmute", "muteTrack", ""); add(track, "Solo / Unsolo", "soloTrack", ""); add(track, "Unmute All and Clear Solo", "clearMuteSolo", "")
        track.addItem(.separator())
        add(track, "Freeze Track…", "freezeTrack", ""); add(track, "Unfreeze Track…", "unfreezeTrack", ""); add(track, "Bounce Track to Audio…", "bounceTrack", "")
        track.addItem(.separator())
        add(track, "Route Mixer Channel…", "routing", ""); add(track, "Add Insert Effect…", "addEffect", "")
        add(track, "New Group Bus…", "addGroup", ""); add(track, "New Return Bus…", "addReturn", "")

        let instrument = submenu("Instrument")
        add(instrument, "Add Instrument", "addInstrument", ""); add(instrument, "Add Wavetable Synth", "addWavetable", "")
        add(instrument, "Rename Instrument…", "renameInstrument", "")
        add(instrument, "Assign to Selected Track", "assignInstrument", ""); add(instrument, "Remove Instrument", "removeInstrument", "")
        instrument.addItem(.separator())
        add(instrument, "Previous Instrument", "previousInstrument", ""); add(instrument, "Next Instrument", "nextInstrument", "")
        instrument.addItem(.separator())
        add(instrument, "Load Sample…", "loadSample", ""); add(instrument, "Show Sample in Finder", "revealSample", "")
        add(instrument, "Sample Editor", "sample:editor", ""); add(instrument, "Detect Transients", "sample:detectTransients", "")
        add(instrument, "Slices to New Pattern", "sample:slicesToPattern", "")

        let windowMenu = submenu("Window")
        windowMenu.addItem(withTitle: "Minimize", action: #selector(NSWindow.performMiniaturize(_:)), keyEquivalent: "m")
        windowMenu.addItem(withTitle: "Zoom", action: #selector(NSWindow.performZoom(_:)), keyEquivalent: "")
        windowMenu.addItem(.separator())
        windowMenu.addItem(withTitle: "Bring All to Front", action: #selector(NSApplication.arrangeInFront(_:)), keyEquivalent: "")
        NSApp.windowsMenu = windowMenu

        let help = submenu("Help")
        add(help, "Keyboard Shortcuts", "help", "/"); add(help, "Command Palette…", "palette", "")
        NSApp.helpMenu = help
        NSApp.mainMenu = bar
    }

    private func add(_ menu: NSMenu, _ title: String, _ command: String, _ key: String, modifiers: NSEvent.ModifierFlags = .command) {
        let item = NSMenuItem(title: title, action: #selector(menuCommand(_:)), keyEquivalent: key)
        item.keyEquivalentModifierMask = modifiers; item.representedObject = command; item.target = self; menu.addItem(item)
    }
    @objc func menuCommand(_ sender: NSMenuItem) { if let command = sender.representedObject as? String { bridge?.command(command) } }
    func application(_ application: NSApplication, open urls: [URL]) {
        guard let url = urls.last(where: { $0.isFileURL && $0.pathExtension.lowercased() == "supertracker" }) else { return }
        if let bridge { bridge.requestOpenProject(url) }
        else { pendingProjectURL = url }
        window?.makeKeyAndOrderFront(nil)
        application.activate(ignoringOtherApps: true)
    }
    @objc func about() { NSApp.orderFrontStandardAboutPanel(options: [.applicationName: "SuperTracker", .applicationVersion: "0.1.0", .credits: NSAttributedString(string: "Keyboard-first tracker studio. Original audio engine. Audio Unit, VST3 and VST2 hosting.")]) }
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        guard sender === window else { return true }
        // Keep the workspace alive while Save/Discard/Cancel and state capture run.
        // Returning false also retains it when termination is rejected or cancelled.
        if !terminationPending { requestTermination() }
        return false
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard let bridge else { return .terminateNow }
        if terminationPending { return .terminateCancel }
        terminationPending = true
        agentControl?.suspendAdmissionForTermination()
        Task { @MainActor in
            var quitting = false
            do {
                try await agentControl?.settleForTermination()
                try await bridge.prepareForTermination()
                if !bridge.store.dirty { await finishTermination(sender); return }
                let alert = NSAlert(); alert.messageText = "Save your project before quitting?"; alert.informativeText = "A recovery copy is also kept automatically."
                alert.addButton(withTitle: "Save"); alert.addButton(withTitle: "Don't Save"); alert.addButton(withTitle: "Cancel")
                switch alert.runModal() {
                case .alertFirstButtonReturn:
                    if await bridge.saveForTermination() { quitting = true; await finishTermination(sender) }
                    else { sender.reply(toApplicationShouldTerminate: false) }
                case .alertSecondButtonReturn: quitting = true; await finishTermination(sender)
                default: sender.reply(toApplicationShouldTerminate: false)
                }
            } catch {
                let alert = NSAlert(); alert.messageText = "Your latest changes could not be secured"
                alert.informativeText = error.localizedDescription + "\n\nThe last automatic recovery copy is kept either way."
                alert.addButton(withTitle: "Keep Working"); alert.addButton(withTitle: "Quit Anyway")
                if alert.runModal() == .alertSecondButtonReturn { quitting = true; await finishTermination(sender) }
                else { sender.reply(toApplicationShouldTerminate: false) }
            }
            if !quitting { await agentControl?.resumeAfterTermination() }
            terminationPending = false
        }
        return .terminateLater
    }
    private func finishTermination(_ sender: NSApplication) async {
        await bridge.shutdownAndWait()
        do {
            try bridge.audio.beginRecoveryModuleCleanup()
            st_vst3_release_unused_modules()
            try bridge.audio.finishRecoveryShutdown()
        } catch {
            // Always release the modules even if journal persistence fails. Leave
            // the journal unclean so the next launch can explain the interruption.
            st_vst3_release_unused_modules()
            NSLog("Plug-in shutdown journal: %@", error.localizedDescription)
        }
        sender.reply(toApplicationShouldTerminate: true)
    }
    func applicationWillTerminate(_ notification: Notification) { agentControl?.stop(); bridge?.shutdown() }
}

@main
struct SuperTrackerMain {
    @MainActor static func main() {
        // MCP proxy and control CLI: no window, no audio, no AppKit application.
        if CommandLine.arguments.contains("--mcp") { exit(MCPServer.runStdio()) }
        if let index = CommandLine.arguments.firstIndex(of: "--ctl") { exit(MCPServer.runControl(arguments: Array(CommandLine.arguments.dropFirst(index + 1)))) }
        let app = NSApplication.shared
        let explicitKeyboardSession = CommandLine.arguments.firstIndex(of:"--ui-keyboard-session")
        let bundledKeyboardSession = Bundle.main.bundleIdentifier == NativeKeyboardAcceptanceSession.bundleID && Bundle.main.object(forInfoDictionaryKey:NativeKeyboardAcceptanceSession.bundleOptInKey) as? Bool == true
        if explicitKeyboardSession != nil || bundledKeyboardSession {
            let output: URL
            if let index = explicitKeyboardSession {
                guard CommandLine.arguments.count > index + 1 else {
                    fputs("Usage: SuperTracker --ui-keyboard-session <private-output-directory>\n",stderr); exit(64)
                }
                output = URL(fileURLWithPath:CommandLine.arguments[index + 1],isDirectory:true)
            } else if let path = Bundle.main.object(forInfoDictionaryKey:"SuperTrackerKeyboardAcceptanceOutput") as? String, path.hasPrefix("/") {
                output = URL(fileURLWithPath:path,isDirectory:true)
            } else {
                output = FileManager.default.temporaryDirectory.appendingPathComponent("SuperTrackerKeyboardAcceptance",isDirectory:true)
            }
            app.setActivationPolicy(.regular)
            let delegate = AppDelegate(keyboardAcceptance:NativeKeyboardAcceptanceSession(output:output))
            app.delegate = delegate
            withExtendedLifetime(delegate) { app.run() }
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--mk3-probe") {
            // Hardware diagnostic: no window. Shared mode unless --exclusive (stops NI's agents, restores them).
            app.setActivationPolicy(.accessory)
            let seconds = CommandLine.arguments.count > index + 1 ? Double(CommandLine.arguments[index + 1]) ?? 10 : 10
            let arguments = Set(CommandLine.arguments)
            Task { @MainActor in
                exit(await MaschineMK3Probe.run(seconds: seconds, exclusive: arguments.contains("--exclusive"), raw: arguments.contains("--raw")))
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--vst2-scan") {
            app.setActivationPolicy(.accessory)
            guard CommandLine.arguments.count > index + 1,
                  let outputIndex = CommandLine.arguments.firstIndex(of: "--output"),
                  CommandLine.arguments.count > outputIndex + 1 else {
                fputs("Usage: SuperTracker --vst2-scan <bundle> --output <report.json>\n", stderr)
                exit(64)
            }
            // Plug-in static destructors can crash at process exit; the envelope is already written.
            _exit(VST2Catalogue.runScanCommand(
                bundle: URL(fileURLWithPath: CommandLine.arguments[index + 1]),
                output: URL(fileURLWithPath: CommandLine.arguments[outputIndex + 1])))
        }
        if let index = CommandLine.arguments.firstIndex(of: "--vst3-scan") {
            app.setActivationPolicy(.accessory)
            guard CommandLine.arguments.count > index + 1,
                  let outputIndex = CommandLine.arguments.firstIndex(of: "--output"),
                  CommandLine.arguments.count > outputIndex + 1 else {
                fputs("Usage: SuperTracker --vst3-scan <bundle> --output <report.json>\n", stderr)
                exit(64)
            }
            exit(VST3Catalogue.runScanCommand(
                bundle: URL(fileURLWithPath: CommandLine.arguments[index + 1]),
                output: URL(fileURLWithPath: CommandLine.arguments[outputIndex + 1])))
        }
        #if SELFTESTS
        if let index = CommandLine.arguments.firstIndex(of: "--ui-workflow-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/workflow-ui"
            Task { @MainActor in
                do { _ = try await NativeUIBridgeSelfTest.run(output: URL(fileURLWithPath: path), workflowOnly: true); exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--ui-kits-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/kits-ui"
            Task { @MainActor in
                do { _ = try await NativeUIBridgeSelfTest.run(output: URL(fileURLWithPath: path), kitOnly: true); exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--ui-bridge-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output"
            Task { @MainActor in
                do { _ = try await NativeUIBridgeSelfTest.run(output: URL(fileURLWithPath: path)); exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--ui-plugin-recovery-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output"
            Task { @MainActor in
                do { _ = try await NativeUIRecoveryAcceptanceTests.run(output: URL(fileURLWithPath: path)); exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--vst2-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output"
            Task { @MainActor in
                do { let checks = try await NativeVST2Tests.run(output: URL(fileURLWithPath: path)); print("PASS \(checks.count) checks"); _exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); _exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--vst3-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output"
            Task { @MainActor in
                do {
                    _ = try await NativeVST3Tests.run(output: URL(fileURLWithPath: path))
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
        if let index = CommandLine.arguments.firstIndex(of: "--freeze-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/freeze"
            Task { @MainActor in
                do { _ = try await NativeFreezeTests.run(output: URL(fileURLWithPath: path)); exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--maschine-kits-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/maschine-kits"
            Task { @MainActor in
                do {
                    let checks = try await NativeMaschineKitTests.run(output: URL(fileURLWithPath: path))
                    st_vst3_release_unused_modules()
                    print("PASS \(checks.count) Maschine kit checks")
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
        if let index = CommandLine.arguments.firstIndex(of: "--many-tracks-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/many-tracks"
            Task { @MainActor in
                do { let checks = try await NativeManyTracksTests.run(output: URL(fileURLWithPath: path)); print("PASS \(checks.count) many-track checks"); exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--kits-performance") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/kits-performance"
            Task { @MainActor in
                do { let checks = try await NativeKitPerformanceTests.run(output: URL(fileURLWithPath: path)); print("PASS \(checks.count) kit performance checks"); exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--render-performance") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/render-performance"
            Task { @MainActor in
                do { let checks = try await NativeRenderPerformanceTests.run(output: URL(fileURLWithPath: path)); print("PASS \(checks.count) render performance checks"); exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--audio-input-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/hardware-input"
            Task { @MainActor in
                do { let checks = try await NativeHardwareInputTests.run(output: URL(fileURLWithPath: path)); print("PASS \(checks.count) hardware input checks"); exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run(); return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--line-input-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/line-input"
            Task { @MainActor in
                do { let checks = try await NativeLineInputTests.run(output: URL(fileURLWithPath: path)); print("PASS \(checks.count) line input checks"); exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--midi-output-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/midi-output"
            Task { @MainActor in
                do { let checks = try await NativeMIDIOutputTests.run(output: URL(fileURLWithPath: path)); print("PASS \(checks.count) MIDI output checks"); exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--plugin-state-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/plugin-state"
            Task { @MainActor in
                do { let checks = try await NativePluginStateTests.run(output: URL(fileURLWithPath: path)); print("PASS \(checks.count) plug-in state checks"); exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--kit-dsp-measurement"), CommandLine.arguments.count > index + 3 {
            app.setActivationPolicy(.accessory)
            let arguments = CommandLine.arguments
            Task { @MainActor in
                do { _ = try await NativeKitDSPMeasurement.run(group: URL(fileURLWithPath: arguments[index + 1]), library: URL(fileURLWithPath: arguments[index + 2]),
                                                                output: URL(fileURLWithPath: arguments[index + 3])); exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--insert-chain-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/insert-chain"
            Task { @MainActor in
                do { let checks = try await NativeInsertChainTests.run(output: URL(fileURLWithPath: path)); print("PASS \(checks.count) insert chain checks"); for check in checks { print("  " + check) }; exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--bus-insert-automation-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/bus-insert-automation"
            Task { @MainActor in
                // _exit: it hosts an Audio Unit; no plug-in teardown runs in static destructors at exit().
                do { let checks = try await NativeBusInsertAutomationTests.run(output: URL(fileURLWithPath: path)); print("PASS \(checks.count) bus insert automation checks"); for check in checks { print("  " + check) }; fflush(stdout); _exit(0) }
                catch { fflush(stdout); fputs("FAIL \(error.localizedDescription)\n", stderr); _exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--instrument-effects-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/instrument-effects"
            Task { @MainActor in
                do { let checks = try await NativeInstrumentEffectsTests.run(output: URL(fileURLWithPath: path)); print("PASS \(checks.count) instrument effect checks"); for check in checks { print("  " + check) }; exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--render-selection-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/render-selection"
            Task { @MainActor in
                do { let checks = try await NativeRenderSelectionTests.run(output: URL(fileURLWithPath: path)); print("PASS \(checks.count) render selection checks"); for check in checks { print("  " + check) }; exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--live-publish-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/live-publish"
            Task { @MainActor in
                do { let checks = try await NativeLivePublishTests.run(output: URL(fileURLWithPath: path)); print("PASS \(checks.count) live publish checks"); exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--render-restart-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/render-restart"
            Task { @MainActor in
                do { let checks = try await NativeRenderRestartTests.run(output: URL(fileURLWithPath: path)); print("PASS \(checks.count) render restart checks"); exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--deferred-parameters-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/deferred-parameters"
            Task { @MainActor in
                // _exit: NI plug-ins crash in their static destructors at exit(), which would raise a crash dialog.
                do { let checks = try await NativeDeferredParameterTests.run(output: URL(fileURLWithPath: path)); print("PASS \(checks.count) deferred parameter checks"); for check in checks { print("  " + check) }; fflush(stdout); _exit(0) }
                catch { fflush(stdout); fputs("FAIL \(error.localizedDescription)\n", stderr); _exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--inserts-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/inserts"
            Task { @MainActor in
                do { let checks = try await NativeInsertLimitTests.run(output: URL(fileURLWithPath: path)); print("PASS \(checks.count) insert limit checks"); exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--audio-host-fixes-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/audio-host-fixes"
            Task { @MainActor in
                do { let checks = try await NativeAudioHostFixTests.run(output: URL(fileURLWithPath: path)); print("PASS \(checks.count) audio host checks"); exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--kits-self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output/kits"
            Task { @MainActor in
                do { let checks = try await NativeKitHostTests.run(output: URL(fileURLWithPath: path)); print("PASS \(checks.count) kit host checks"); exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        if let index = CommandLine.arguments.firstIndex(of: "--self-test") {
            app.setActivationPolicy(.accessory)
            let path = CommandLine.arguments.count > index + 1 ? CommandLine.arguments[index + 1] : FileManager.default.currentDirectoryPath + "/test-output"
            Task { @MainActor in
                do { try await NativeSelfTest.run(output: URL(fileURLWithPath: path)); exit(0) }
                catch { fputs("FAIL \(error.localizedDescription)\n", stderr); exit(1) }
            }
            app.run()
            return
        }
        #else
        // Self-test modes are compiled only into SELFTESTS builds (debug, or scripts/build.sh selftest).
        // Refuse them here instead of falling through to a normal, visible launch.
        if let flag = CommandLine.arguments.dropFirst().first(where: { $0.hasSuffix("-self-test") || $0 == "--kits-performance" || $0 == "--render-performance" }) {
            fputs("\(flag) is not available in this build. Self-tests are compiled only into debug builds or scripts/build.sh selftest.\n", stderr)
            exit(64)
        }
        #endif
        // Scripted development runs have no Dock icon or menu bar presence.
        app.setActivationPolicy(ProcessInfo.processInfo.environment["SUPERTRACKER_DEV_SCRIPT"] != nil ? .accessory : .regular)
        let delegate = AppDelegate()
        app.delegate = delegate
        withExtendedLifetime(delegate) { app.run() }
    }
}

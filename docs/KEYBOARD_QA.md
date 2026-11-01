# Isolated native keyboard session

Build without launching:

```sh
bash scripts/build-keyboard-qa.sh debug
```

The output is `build/KeyboardQA/SuperTrackerKeyboardQA.app`. Its bundle identifier is `studio.supertracker.keyboardqa`, its application name is **SuperTracker Keyboard QA**, and its window title is **SuperTracker Keyboard Acceptance**. It does not register as a handler for project files.

When the app is deliberately opened later, it automatically starts the private session without command-line arguments. CUA can target the exact `.app` path or bundle identifier. The automatic entry requires both the exact QA identifier and the explicit `SuperTrackerKeyboardAcceptance` Info.plist flag; the normal app ignores that flag.

An alternative explicit launch, only when authorized to open the QA window, is:

```sh
build/KeyboardQA/SuperTrackerKeyboardQA.app/Contents/MacOS/SuperTrackerKeyboardQA --ui-keyboard-session "$PWD/test-output/keyboard-native"
```

Each launch creates a fresh UUID session inside `test-output/keyboard-native`. `latest-session.json` records the private project, recovery, and generated sample locations. Previous sessions remain available for inspecting saved results. Nothing reads or replaces the normal SuperTracker recovery project or sample index. The browser scans only four original generated WAV fixtures. The project has eight tracks, three instruments, two patterns, two clips, and an automation lane.

The production AppDelegate supplies the same menus, WKWebView preferences, JavaScript, native Bridge, focus routing, save/quit flow, and native engine. Web storage is ephemeral. The QA-only Bridge policy keeps engine master output and sample preview muted, disables MIDI, hides the plug-in catalogue, and rejects plug-in loading/scanning before native resolution. It also confines media and save paths to the private session and blocks external project replacement, file pickers, external applications, recording/input configuration, and exports. This session is for keyboard routing and layout acceptance; it does **not** establish acceptance of plug-ins, microphone capture, hardware, file dialogs, or audible playback.

Suggested physical checks after an authorized launch:

- Command-1 through Command-6 enter the six workspaces; Tab and Shift-Tab traverse real native WebKit focus.
- In Mixer, Left/Right keeps the same control on the next track; Up/Down adjusts the focused range. Rapid arrows reach successive tracks. The last track stays visible beside Master at 1080×700.
- Sidebar arrows select patterns/instruments without editing the song sequence. Right enters the relevant workspace.
- Instrument-type and piano arrows move focus; activation remains explicit. Browser search, facets, and result navigation remain confined to their controls.
- Clip toolbar arrows/Delete do not edit clips; timeline keys edit the focused clip and Command-Z restores it.
- Escape closes a local chooser before invoking global Stop. Opening the recording dialog is read-only; capture and input configuration are deliberately unavailable here.

Do not identify or operate the normal `SuperTracker.app` during this QA session. No GUI is launched by the build script.

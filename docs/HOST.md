# The host

The design is in [ARCHITECTURE.md](ARCHITECTURE.md). This file is the reference: what runs today, how to
run and test it, and every bridge command with the crate that owns it and its state.

## What runs

| Piece | Crate | State |
|---|---|---|
| Bridge core: protocol, command table, events, transports' queue | `st-host` | done, tested on Linux |
| Session: publish, new, open, save, autosave and recovery, templates, MIDI files, missing media | `st-host` on `st-project` | done, tested on Linux |
| Settings file | `st-host` | done |
| `--serve`: HTTP + WebSocket on 127.0.0.1 | `st-app` | done, tested end to end in Chromium |
| Window: tao + wry + muda, menus, native context menus, file choosers, quit handshake | `st-app` | runs on Linux (GTK/WebKitGTK, checked in a virtual display); macOS code type-checks, **not yet run on a Mac** |
| Audio playback core: project to engine, the mixer graph (strips, inserts with built-in effects, sends, sidechains, buses, master, delay compensation, automation, meters), transport, live notes, devices (cpal and the null device), offline render | `st-audio` on `tracker-engine`, registered in `st-host` (`areas/audio.rs`) | done, tested on Linux (offline renders, allocation-free render path, null device, Chromium end to end); CoreAudio only type-checked |
| Plug-ins: instances in container processes as inserts and instruments of the mixer (latency into delay compensation, sidechains, automation and step locks with frame offsets and a return to the base value the host keeps per parameter, notes of plug-in instruments, live controllers and pressure for them, state in the project), the catalogue (scanned by container processes, blocklist), crash recovery (`plugin.*`, `plugins.*`) | `st-audio` (`plugins`) on the container client (`crates/plugin-host`), registered in `st-host` (`areas/plugins.rs`) | done, tested on Linux with the CLAP and VST3 test plug-ins and the real container process (offline renders compared bit for bit, allocation-free render path with plug-ins, the bridge on the null device, Chromium end to end). Audio Units, editor windows of VST3 and Audio Units, and yielding activation to a container before its editor is focused are macOS only: type-checked, **not yet run on a Mac**. No third-party plug-in has been loaded yet |
| Samples (decoding on worker threads, edits, slices, sampler playheads), browser previews, export / bounce / render selection / Track Freeze on a private offline engine, recording of the master output or an input, live line inputs into track strips, Track Freeze by recording on the live engine | `st-audio` (with `st-samples`), registered in `st-host` (`areas/samples.rs`, `areas/export.rs`, `areas/recording.rs`) | done, tested on Linux (offline renders, the null device and the null input, Chromium end to end for sample loading and export); `cpal` input streams only type-checked for macOS and never opened on a real device, so line inputs and freeze by recording have only met the null input |
| Sound library: folders, index and search, favorites and labels, NKS catalogue, kits with the audio preview they are saved with (`library.*`, `kit.*`) | `st-library`, registered in `st-host` (`areas/library.rs`); the preview is rendered by `st_audio::kit_preview` on a private offline engine | done, tested on Linux (bridge tests on generated audio files and generated kits; the Browser workspace in Chromium end to end). The standard content folders and NKS registrations of macOS are named in code that has not run on a Mac |
| MIDI: devices with hot-plug, input to the engine (with recording captures), controllers, the pedals, pressure and pitch bend from input and from `midi.expression` to the instrument a track plays (an external device, a plug-in, the engine), output of external-MIDI instruments from the render thread, MIDI clock and transport out (`midi.*`) | `st-midi`, connected to `st-audio` in `st-host` (`areas/midi.rs`; the audio side's seam is `st_audio::midi`) | done, tested on Linux with in-memory ports (note times, clock against tempo, input acknowledgements, expression and the pedals to every kind of instrument). **No real MIDI device has been opened** (ALSA sequencer, CoreMIDI) |
| Maschine MK3 (`mk3.*`), off unless turned on in Settings; `--mk3-probe` | `st-mk3`, registered in `st-host` (`areas/mk3.rs`) | commands tested through the bridge against the in-memory controller; **never run with a controller** (see `cargo doc -p st-mk3`) |
| AI control: the local control socket, `supertracker --mcp` and `--ctl` (`agent.*`) | `st-agent`, registered in `st-host` (`areas/agent.rs`); the page answers `agent.call` events | done, tested on Linux (bridge tests; MCP session and `--ctl` against the page in Chromium end to end). Unix sockets only. Screenshots (`app.screenshot`, the MCP tool `screenshot`) are taken by the window's web view (`st-app`, `screenshot.rs`, installed through `areas::agent::Agent::set_screenshot_provider`): tested on Linux in the real window under Xvfb; the macOS path is type-checked only; `--serve` has no window and answers with a message |

The interface (`ui/`) reaches its host through `host-transport.js` only.

## Building and running

```sh
cargo build -p st-app                      # the `supertracker` executable
# The plug-in container is a workspace of its own; built into the same folder it lands next to `supertracker`,
# where the host looks for it (else: `$STPC_CONTAINER_PATH`, or `container` in the settings section `plugins`).
# Without it the host runs, lists and loads built-in effects, and says why a plug-in cannot be loaded.
CARGO_TARGET_DIR=target cargo build --manifest-path crates/plugin-host/Cargo.toml -p stpc-container
cargo test -p st-host -p st-app            # no display or audio hardware needed
target/debug/supertracker                  # the window; the interface is found in ui/
target/debug/supertracker --serve          # no window: prints http://127.0.0.1:<port>/?token=<token>
target/debug/supertracker --mcp            # MCP server on stdin/stdout for an AI client (needs the application running)
target/debug/supertracker --ctl summary    # one call through the application's control socket; `--ctl help` lists the methods
target/debug/supertracker --mk3-probe 10   # Maschine MK3 hardware diagnostic ([--exclusive] [--raw])
target/debug/supertracker --help
```

Linux build packages (Ubuntu 24.04): `libgtk-3-dev libwebkit2gtk-4.1-dev libasound2-dev pkg-config`
(and a C toolchain; `libasound2-dev` is for `cpal`, the audio device layer of `st-audio`, and for
`midir`, the MIDI device layer of `st-midi`). At run time MIDI devices need the ALSA sequencer
(`/dev/snd/seq`); without it the host runs without MIDI devices and `midi.devices` says why. The
Maschine MK3 needs access to its `hidraw` and USB device nodes (a udev rule). To run the
window without a display: `xvfb`; the window smoke test also needs `xdotool`.

Type-checking the macOS code from Linux (no SDK needed; one dependency compiles a small Objective-C
file, for which clang must be named):

```sh
cargo check -p st-audio -p st-host --target aarch64-apple-darwin
CC_aarch64_apple_darwin=clang cargo check -p st-app --target aarch64-apple-darwin
```

Environment: `SUPERTRACKER_SUPPORT_DIR` (absolute path) replaces the support folder
(`~/Library/Application Support/SuperTracker`, `$XDG_DATA_HOME/SuperTracker` or
`~/.local/share/SuperTracker`); `SUPERTRACKER_UI_DIR` is `--ui`; `SUPERTRACKER_DIALOG_ANSWERS` is
`--dialog-answers`; `SUPERTRACKER_TRACE=1` logs every request and its outcome to standard error;
`SUPERTRACKER_AUDIO_DEVICE=null` makes the host render to the null audio device whatever is connected
and whatever the settings say (tests, servers). Without it the host uses the output chosen in
Settings, else the system's default output, and the null device while none is connected.
`SUPERTRACKER_MIDI=off` opens no MIDI device. `SUPERTRACKER_CONTROL_SOCKET` (absolute path) replaces
the AI control socket, which is otherwise `control.sock` in the support folder (or, when that path is
too long for a socket, `SuperTracker-<hash>.sock` in the user's temporary folder); the application
and `--mcp` / `--ctl` find it by the same rule, so they meet whenever they agree on the support folder.
`SUPERTRACKER_NO_MK3` keeps the host from connecting to the controller at launch;
`SUPERTRACKER_MK3_SCREEN_DUMP=<folder>` writes every controller screen frame as PNG. `SUPERTRACKER_PLUGIN_PATH` names folders (separated like `$PATH`) searched for
plug-ins besides the settings' and the standard ones; `SUPERTRACKER_PLUGIN_STANDARD_FOLDERS=0` leaves
the standard folders out.

### Tests

| What | How |
|---|---|
| Bridge guarantees, session and settings commands, catalogue; the audio commands on the null device (`tests/audio.rs`) | `cargo test -p st-host` |
| The audio core: unit tests, offline renders against the mixer's rules (`tests/offline.rs`), the demo song, transport, devices (`tests/playback.rs`), no allocation on the render path (`tests/realtime.rs`), kit previews of generated kits with a built-in effect and a plug-in in a container (`tests/kit_preview.rs`) | `cargo test -p st-audio` |
| Line inputs: a manual input through a track's inserts, fader and meter, delay compensation of its latency, paused routes, the tap on a strip (`tests/line_inputs.rs`), no allocation on the input and render paths with line inputs and a tap (`tests/realtime.rs`); an input on a clock of its own in simulated time: the converter's distortion and frame count (unit tests of `asrc.rs`), ten minutes at +200 ppm, -200 ppm and from 44.1 kHz without a gap or a skip, a stall with a burst, lost input, a stalled output, a 44.1 kHz input into a track and into a take at the engine's rate (`tests/two_clocks.rs`), no allocation in any of that (`tests/realtime.rs`); Freeze by recording in real time on the null device, a recorded print against the offline print of the same track, cancelled and disturbed takes (`tests/freeze_recording.rs`) | `cargo test -p st-audio` |
| Line inputs and Freeze by recording through the bridge, on the null device with the null input's test tone: a route in the project heard on its track's meter and reported in `audio.devices`, paused routes, the remembered input, a hardware track frozen by recording, played from its print, exported and unfrozen (`tests/line_inputs.rs`) | `cargo test -p st-host` |
| Render cost of the demo song and of a 64-track project | `cargo run --release -p st-audio --example bench` |
| The library through the bridge, on generated audio files: add a folder, scan, search, favorites and browser state across a restart, kits, a kit saved with the preview of its group bus and auditioned (the file's format, length and where it has audio), the seam to the plug-in list (`tests/library.rs`) | `cargo test -p st-host` |
| MIDI with in-memory ports: a pattern's notes for an external instrument at their times, all-notes-off on stop and panic, clock against the tempo, a keyboard note that sounds and is acknowledged for recording, MIDI Start/Stop, hot-plug, shutdown (`tests/midi.rs`); no allocation on the render path with MIDI output connected (`tests/midi_realtime.rs`); controllers, channel and poly pressure and pitch bend from `midi.expression` and from a keyboard to an external instrument, the sampler, and the CLAP and VST3 test instruments (`tests/expression.rs`); the sustain pedal from the interface and from keyboards: passed on with its value to the external instrument and to the CLAP and VST3 test instruments (which sustain by themselves), held by the host for the sampler and for a plug-in the pedal cannot reach, following the keyboard to another track, on Stop, at the end of a song and on panic, across a publication, a replaced instrument, a plug-in restored after a crash and reordered tracks, with several keyboards and channels, and the length of notes recorded under it (`tests/pedal.rs`) | `cargo test -p st-host` |
| The audio core's MIDI seam with a recording hook: blocks with the transport position, notes of external instruments at their frames, all-notes-off on stop and panic, nothing during an offline render (`tests/midi.rs`) | `cargo test -p st-audio` |
| The Maschine MK3 through the bridge against `st_mk3::fake` (`tests/mk3.rs`); AI control through the bridge with a control-socket client, the setting and the quit fence (`tests/agent.rs`) | `cargo test -p st-host` |
| Command line, WebSocket framing, file serving, menus, window helpers | `cargo test -p st-app` |
| The page's bridge, transport and quit handshake | `node tests/host_transport_tests.js`, `node tests/host_quit_tests.js`, and every `tests/*_tests.js` (they play the host through `tests/helpers/host.js`) |
| The real page against the real host, in Chromium | `node tests/host_e2e/run.js` ([README](../tests/host_e2e/README.md)) |
| The real window in a virtual display | `tests/host_e2e/window_smoke.sh` |

File choosers cannot open in a test. `--dialog-answers <file>` answers them from a JSON array instead:
each chooser takes the next element, a path string, or `null` for Cancel. The file is read again for
every chooser, so a test may append answers while the host runs. When the answers run out the
command fails with a message that says so.

## The bridge in `st-host`

One command is one registration in `st-host` (see the crate documentation, `cargo doc -p st-host`):

```rust
bridge.commands().with(&service)
    .register("transport.stop", Transport::stop)            // quick: bridge thread, in arrival order
    .register_blocking("audio.export", Exporter::export);   // may take long: its own worker thread
```

- Handlers are plain functions `Fn(&Service, Payload) -> Result<Output, E>`; `Payload` and `Output` are
  serde types, `E` is anything whose `Display` is a message for the user.
- `register_deferred` hands the handler a `Reply` to answer later from any thread.
- Events: `EventSink::emit(name, payload)` from any thread; `emit_latest` for snapshots (`status`,
  meters, playheads) of which only the newest matters while the page is busy.
- One area can call another through the table (`Calls::call`), which is how `ready` collects
  `library.roots`, `plugins.list`, `audio.devices`, `midi.devices` and `plugins.recovery`, and how
  `project.save` captures plug-in state (`engine.snapshot`). Registering those commands is all an area
  has to do to appear there.
- The audio engine attaches to project publications with `Session::attach_audio` (`ProjectAudio`);
  `st_host::areas::audio` does (the engine is `Host::audio`, for the areas that build on it:
  samples through `st_audio::SampleSource`, plug-in inserts through `st_audio::graph::insert::InsertFactory`,
  export and freeze through `AudioEngine::render_offline`).
- Areas that follow the project without being part of the audio engine observe publications with
  `Session::observe_projects` (MIDI routes do). Work that must not start while a quit is being
  decided hangs on `Lifecycle::set_fence` (AI control does).
- The library, MIDI, the Maschine MK3 and AI control are installed together
  (`st_host::areas::services`, options in `HostOptions::services`, the running services in
  `Host::services`). The executable calls `host.services.start_agent()` once its transport is up and
  `host.services.shutdown()` when it quits and on SIGINT, SIGTERM and SIGHUP (`st-app/src/signals.rs`):
  the control socket is closed, the controller is left dark with Native Instruments' agents restored,
  external MIDI instruments are silenced, the library's index is written.
- `st_host::testing::TestHost` runs commands as the page would, for each area's tests.
- After adding or finishing a command, change its entry in `crates/st-host/src/catalog_table.rs` and
  run `UPDATE_HOST_DOC=1 cargo test -p st-host --test host_doc`, which rewrites the tables
  below. `cargo test` fails while the catalogue, the registrations and this file disagree.

### Transports

| | Window | `--serve` |
|---|---|---|
| Page URL | `supertracker://localhost/index.html` (files from the interface folder) | `http://127.0.0.1:<port>/?token=<token>` |
| Page to host | `window.ipc.postMessage(text)` | WebSocket `ws://127.0.0.1:<port>/bridge?token=<token>`, one JSON text per message |
| Host to page | `STHostTransport.receive(...)` by script, in acknowledged batches | the same socket |
| Protection | the web view only loads `supertracker:` URLs | bound to 127.0.0.1; random 128-bit token per run (constant-time compare); `Host` must be this server; the WebSocket's `Origin` must be this server |
| File choosers | native (NSOpenPanel through `rfd`; GTK file chooser) | none: commands fail with an explanation unless the request carries a `path` or `--dialog-answers` is given |
| Context menus | native (`ui.contextMenu`, muda) | the page's own menu (the command is not registered) |

`--serve` serves every file of the interface folder to any local client (they are not secret); only the
bridge needs the token. Port: free port by default, `--serve <port>` to choose. It stops on `quit` on
standard input (recovery copy written, pending requests answered, sockets closed); killed, it leaves
the recovery copy of at most 20 seconds ago.

### The page's side

- `host-transport.js` (first script after `util.js` and `busy.js`) chooses the channel and exposes
  `STHostTransport.post(message)` / `onMessage(handler)`; `available` is false on a page opened with
  no host, where `app-bridge.js` keeps its browser-preview answers.
- `app-bridge.js` posts through it and receives replies and events from it (events go to
  `window.nativeEvent`). `ST.longRequest` has no time limit (`freeze.js` and `render-selection.js` use it);
  `ST.hostImplements(name)` says whether the host registered a command.
- `ready` returns `commands`, the list of implemented commands; `context-menu.js` uses its own
  menu when `ui.contextMenu` is not among them.
- `window-drag.js` asks the host to move the window (`window.titleBar`) on a press in empty top bar
  space.
- Quit: the host sends the event `app.quitRequested`; `app-session.js` flushes pending edits, asks
  about unsaved changes with the page's own Save / Discard / Cancel dialog, and answers with
  `app.quitAnswer`.
  AI control is fenced for the duration: the page calls `STAgent.suspendForTermination` first (a
  command that does not finish within three seconds answers `stay`) and
  `STAgent.resumeAfterTermination` with every `stay`; the host refuses new control-socket requests
  between `app.quitRequested` and `stay`.
- AI control: `agent-api.js` answers the event `agent.call` `{id, method, params}` by running
  `STAgent.dispatch` and sending the outcome with the command `agent.reply` `{id, reply}`, and passes
  `agent.status` events to `STAgent.setConnection`.

## Commands

State is **implemented**, **partial** (registered; the note says what is missing) or **not yet** (the
request is answered with "`<name>` is not implemented yet (it belongs to
`<crate>`)."). The crate is the one that owns the behaviour; every handler is registered in `st-host`
(or `st-app` for the window's own) and delegates there. Payload and result are the JSON shapes in
brief; `?` marks optional fields. Where a shape is abbreviated ("…"), the handler's payload type in
`crates/st-host/src/areas/` is the reference.

<!-- commands:begin (generated from crates/st-host/src/catalog_table.rs; see crates/st-host/tests/host_doc.rs) -->
| Crate | Commands | Implemented | Partial | Not yet |
|---|---:|---:|---:|---:|
| `st-host` | 16 | 16 | 0 | 0 |
| `st-app` | 2 | 2 | 0 | 0 |
| `st-project` | 10 | 10 | 0 | 0 |
| `st-audio` | 49 | 49 | 0 | 0 |
| `st-plugin-*` | 4 | 4 | 0 | 0 |
| `st-library` | 21 | 21 | 0 | 0 |
| `st-midi` | 4 | 4 | 0 | 0 |
| `st-mk3` | 6 | 6 | 0 | 0 |
| `st-agent` | 3 | 3 | 0 | 0 |
| **all** | **115** | **115** | **0** | **0** |

### `st-host`

| Command | Payload | Result | State | Notes |
|---|---|---|---|---|
| `app.external` | `{url} or {path, reveal: true}` | {opened: true} | implemented | Only http(s) links; reveal shows an existing file in the file manager. |
| `app.quitAnswer` | `{state: "deciding" or "stay" or "proceed", reason?, project?, force?}` | {} or {quitting: true} | implemented | the page's answer to the app.quitRequested event (st_host::Lifecycle). |
| `dialog.export` | `{mode?: "stems", name?}` | {path} or {path, mode: "stems"} or {cancelled: true} | implemented | Stems: choose a folder; a new "<name> stems" folder is reserved in it. |
| `dialog.openSample` | `{}` | {path} or {cancelled: true} | implemented | Open panel for audio files. |
| `engine.project` | `{project, replace?}` | the status (as in status events); {audio: false} in a host without an audio engine | implemented | The reply comes when the mixer is built and the project's plug-ins are loaded. replace: the project replaces what the engine holds (restored, undone), so plug-in state is loaded from it even into instances that already run. Validation, dirty flag and the st-project session (st-host), then the engine and the mixer through st_host::ProjectAudio (st-audio): unchanged patterns are not sent again, and a mixer that did not change is not recompiled. Samples that cannot be loaded are reported in sample.error events. The tracks' audioInput routes line inputs: the input device is opened when an enabled route names the input in use (closed when none does), and an audio.devices event says what became of the routes when that changed. |
| `library.chooseRoot` | `{}` | library.scan result, or {cancelled: true} | implemented | The folder chooser, then library.scan with the chosen folder. |
| `project.autosave` | `{project?}` | {saved: true} or {saved: false, deferred: true} | implemented | Recovery copy; deferred while a save or session change is in flight. The host also writes it every 20 s. |
| `project.media.chooseRoot` | `{}` | {path} or {cancelled: true} | implemented | Folder chooser. |
| `project.new` | `{templateID?, blank?}` | {project} from a template, or null (the page uses its blank project) | implemented | Uses the default template unless blank. |
| `project.open` | `{path?}` | {project, path, dirty} or {cancelled: true} | implemented | Open panel without a path. Projects in a retired layout are refused with st-project's message. |
| `project.save` | `{project?, path?, collectSamples?}` | {project, path, saved: true, recoveryWarning?} or {cancelled: true} | implemented | The document and its media folder. Plug-in state is captured through engine.snapshot first; a plug-in whose state cannot be read fails the save with its name. |
| `project.saveAs` | `{project?, path?, collectSamples?}` | as project.save | implemented | Always asks where unless a path is given. |
| `ready` | `{}` | {version, commands, project, dirty, path, roots, plugins, audio, midi, pluginRecovery?, warning?, audioError?, openPath?} | implemented | The handshake. roots, plugins, audio, midi and pluginRecovery come from library.roots, plugins.list, audio.devices, midi.devices and plugins.recovery when those are registered, and are empty until then. |
| `routing.apply` | `{project}` | {project, capabilities, status} (the two are absent in a host without an audio engine) | implemented | Publishes the project like engine.project (the mixer is rebuilt from it; an invalid project is refused and nothing changes) and answers with the project as the session holds it, routing.capabilities and routing.status of the rebuilt mixer. |
| `settings.get` | `{key?}` | {value} or {values: {…}} | implemented | interface preferences in settings.json (section `interface`). Not used by the page yet. |
| `settings.set` | `{key, value} (null removes)` | {} | implemented | See settings.get. |

### `st-app`

| Command | Payload | Result | State | Notes |
|---|---|---|---|---|
| `ui.contextMenu` | `{x, y, items: [{title, id?, enabled?, checked?, key?, items?} or {separator: true} or {header: true, title}]}` | {id: string or null} | implemented | Native menu through muda, in the window only. Under --serve the command is absent and the page shows its own menu (it checks ready.commands). |
| `window.titleBar` | `{doubleClick?: bool}` | null | implemented | macOS window only: a press on empty top bar space starts the window drag (double click zooms). Replaces the windowDrag message handler. |

### `st-project`

| Command | Payload | Result | State | Notes |
|---|---|---|---|---|
| `project.exportMIDI` | `{project?, path?}` | {path, noteCount, trackCount, ticksPerQuarter, warnings} or {cancelled: true} | implemented | st_project::midi::export_file. |
| `project.importMIDI` | `{path?}` | {project, path, warnings} or {cancelled: true} | implemented | st_project::midi::import_file; the import becomes a new unsaved session. |
| `project.media.apply` | `{project?, selections: [{id, from, to, allowDifferentContent?}]}` | {project, relinked, changed, remaining, warnings} | implemented | st_project::relink::apply. The session changes when the page publishes the returned project. |
| `project.media.cancel` | `{}` | {cancelled: true, operations: number} | implemented | Stops the running search, inspections and applications. |
| `project.media.inspect` | `{project?}` | {references, missing, referenceCount, missingCount, presentCount, warnings} | implemented | st_project::relink::inspect. |
| `project.media.search` | `{project?, roots?: [path], relocations?: [{from, to}], requestId?}` | inspect result with candidates + {scannedFiles, hashedFiles, truncated, requestId, operationId} | implemented | st_project::relink::find_candidates on a worker; progress in project.media.progress events; a newer search replaces the running one. |
| `project.templates.list` | `{}` | {templates: [{id, name, path}], defaultID} \| | implemented |  |
| `project.templates.save` | `{project?, name, id?}` | {id, name, path} | implemented | Does not touch the open session. |
| `project.templates.setDefault` | `{id: string or null}` | {defaultID} | implemented | The id field must be present. |
| `routing.validate` | `{project}` | {valid: true} or {valid: false, error} | implemented | st_project::validate on the proposed project; nothing is changed. |

### `st-audio`

| Command | Payload | Result | State | Notes |
|---|---|---|---|---|
| `audio.bounceTrack` | `{track, path?, tail?}` | {path, clip: {id, name, path, track, startBeat, durationBeats, sourceOffset, gain, fadeIn, fadeOut, muted}, sourceTrack, sourceTrackIndex, duration} | implemented | A track-mode export (the track after its fader, 32-bit float at 48 kHz) to path, or to Renders in the support folder. |
| `audio.clipInfo` | `{path}` | sample info: {path, frames, sampleRate, channels, duration, peaks…} | implemented | Decodes an audio clip's file on the command's worker thread (once: the engine's sample cache keeps it). |
| `audio.configure` | `{output?, outputDevice?, outputSystemDefault?, bufferSize?, sampleRate?, input?, inputDevice?, inputChannels?, monitor?, monitorVolume?, inputMeter?}` | audio.devices result | implemented | Selects the output (by id, by uid in `output`, or outputSystemDefault), the buffer size and the sample rate, and the input (`input`: uid, or `inputDevice`: id); all remembered in the settings section audio.devices. A device that cannot be opened leaves the null device running (fallback: true, deviceError). Also `inputChannels` (one for mono, two for stereo), `monitor` + `monitorVolume` (the input mixed into the output) and `inputMeter`, which are not remembered. There is one input stream, at the engine's sample rate, open only while the meter, monitoring, a take or a line input of the project uses it. The project's line inputs are resolved again against the devices chosen. Refused for the output while a take runs. |
| `audio.devices` | `{}` | {devices, inputs, outputs: [{id, uid, name, inputChannels, outputChannels, sampleRate, bufferSize}], defaultInput, defaultOutput, outputDevice, outputUid, sampleRate, bufferSize, bufferSizes, running, fallback, deviceError?, lineInputs: [{track, state: "live" / "paused" / "off", active, monitoring, status?, latencyFrames?, clock?: "shared" / "converted", driftPpm?, underruns?, overruns?}], lineInputError, …} | implemented | Part of the ready handshake (`audio`). Devices come from cpal; the null device (id 0, uid "null") is always among the outputs and is what runs while no output is connected; while it runs, a null input (silence) is offered too. Also carries the input's state (inputDevice, inputUid, inputChannels, inputMetering, monitoring, monitorVolume, inputRunning, inputError, inputSampleRate: the rate the open input delivers at, which is the engine's where the device supports it and the device's nearest one otherwise). lineInputs has one entry per track of the published project that has an audioInput: live (its channels feed the track's strip; latencyFrames is what the mixer compensates for the hardware's round trip; clock is shared when the input is the output's own device and its frames pass through unchanged, converted when it is another device and is resampled onto the output's clock, with driftPpm the measured difference of the two clocks; underruns counts the times the input did not deliver in time, overruns the times audio was dropped because too much waited, neither of which happens while both devices run), paused with the reason in status (its device is not the input in use, could not be opened or lacks the channel; the track is frozen), or off; lineInputError is the reason of the first paused one. Line inputs have no command: tracks[i].audioInput {enabled, deviceUID, channels, gain, monitor?} in the project is the routing, applied by every publication. Also sent as an event when the output changes by itself, the input is lost or returns, or a publication changes the state of a line input. |
| `audio.export` | `{path, mode?: "song" (default) or "pattern" or "stems" or "track", owners?: [owner], track?, pattern?, tail? (0..30 s, default 3), encoding?: "pcm16" / "pcm24" / "float32", sampleRate?: 44100 / 48000 / 96000, dither? ("tpdf" by default for the integer encodings, or "none")}` | {path, paths, mode, frames, musicalFrames, tailFrames, sourceFrames, duration, container, encoding, sampleRate, bitDepth, isFloat, dither, renderSampleRate, latencyCompensation, stems: [{owner, name, path, boundary, frames, arrivalFrames, …}], renderPasses, stemPolicy}; progress in audio.exportProgress {progress, path, phase} events, audio.exported at the end | implemented | st_audio::export on a private offline engine at 48 kHz (live playback is not paused): one pass through the complete graph, stems captured after each chosen channel's fader and delay-compensated; files through st-samples' transaction (published with one rename; stems in a folder). transport.stop or engine.panic cancels it and nothing is published. Refused while a track that is not frozen plays a live line input (freeze it by recording, or switch the input off). |
| `audio.record` | `{source?: "master" or "input" (default), path?, name?, instrument?, owner?, monitor?, monitorVolume?}` | {recording: true, path, channels}; also an audio.recording event | implemented | Starts a take into a new 24-bit WAV (path, or Recordings in the support folder): the master output (what the device gets), or the chosen channels of the input device as the output's clock hears them (at the engine's sample rate; resampled when the input is another device than the output), with the latency of the output, the input and the input's feed left out of the start. st_audio::recording: the render thread copies into a ring, a writer thread writes the file. |
| `audio.stopRecord` | `{}` | {recording: false, path, duration, frames, channels, sampleRate, partial, source, sourceStartFrame, sourceEndFrame, latencyCompensation, instrument?, owner?} (also an audio.recording event); {recording: false} when no take runs | implemented | Ends the take on the output's clock: the file holds one frame per output frame between Record and Stop, for an input take as for a master take, so a take stays in place over its length when it is put into the song. sourceStartFrame and sourceEndFrame are graph frames (the renderer's count of output frames). |
| `builtin.drumModels` | `{}` | [{id, model, name, parameters: [{index, id, name, unit, min, max, defaultValue, defaultPhysical, logarithmic, stepCount}]}] | implemented | Not sent by the page today. Metadata of the built-in drum models (tracker-engine). |
| `builtin.effects` | `{}` | [{id: "stfx:<effect>", name, category, sidechain, parameters: [{address, id, name, unit, min, max, defaultValue, logarithmic, integer, toggle, labels}]}] | implemented | Metadata of the built-in effects (tracker-engine); a parameter's index is its address. |
| `engine.instrumentParameter` | `{instrument, target, value}` | {ok: bool} | implemented | Live change of one built-in instrument parameter (cutoff, resonance, automation targets). |
| `engine.panic` | `{}` | transport status | implemented | All sound off: voices, effect tails, plug-ins and external MIDI instruments (pedals up, all notes off). The pedals played live are forgotten: one that is still held down counts as up until it is pressed again. project.new calls it when present. |
| `engine.snapshot` | `{}` | the project as the engine holds it, with audioPlugins: [{…, state}] captured now | implemented | Built-in effects are captured from the mixer (base64 JSON, format SuperTracker.BuiltinFX). Every hosted plug-in is asked for its state (base64 of the container's state bytes; an unchanged plug-in gives the identical text) and gets latency and tailTime; a descriptor whose state could not be read carries stateError; one without a hosted instance (not installed, held back) keeps what it carries, a lost one its last known state. |
| `freeze.status` | `{}` | {tracks: [{track, id, available, stale, path, dormantPlugins, patternPrints, patternPlayback, unprintedPatterns}], busy} | implemented | stale: the fingerprint of what the print was made from changed (media files by SHA-256 of their content, so Save As and relinking do not make a print stale); available: every print file is readable. |
| `freeze.track` | `{track, tail? (fixed, 0..30 s), maxTail? (auto tail bound, default 10 s), method?: "offline" (default) or "record", patterns? (record: also the pattern prints, default true), monitor? (record: hear the mix during the takes, default true)}` | {track, record (the object the page stores as tracks[i].frozen: version, id, path, sha256, sampleRate, frames, musicalFrames, tailFrames, musicalEndBeat, tailSeconds, dormantPluginIDs, fingerprint, createdAt, patternPrints, unprintedPatterns, tailMode, method: "offline" or "recorded"), audioPlugins, releasedPlugins, unprintedPatterns, notes}; progress in freeze.progress {track, progress, phase: "song" / "patterns" / "done", recording} events | implemented | Long: the page sends it with ST.longRequest. Offline (st_audio::freeze on a private engine): the song print and a head and loop print of every pattern the track uses, 32-bit float WAV at 48 kHz in Freezes of the support folder; refused for a track with a live line input or an external MIDI instrument. method "record" (st_audio::freeze_recording) is for those: the live engine plays the song once in real time (and, with patterns, each pattern for its head and its loop print) while the track's strip is recorded through a tap after its inserts and before its fader, each frame placed at its song frame (the strip's delay compensation, which holds the hardware return's latency, taken out); the prints are at the engine's sample rate. It needs a running device and a stopped transport, publishes a project of its own for the takes (no song loop, no metronome, the master silent without monitor) and the page's project again afterwards. transport.stop cancels either method; a take also fails when playback stops early or jumps, the strip delivers nothing, audio was lost, or a project is published meanwhile. Nothing is kept of a failed freeze. The fingerprint counts media by content hash, not by path. |
| `freeze.unfreeze` | `{track}` | {track, audioPlugins} | implemented | The page removes tracks[i].frozen and publishes; the engine then plays the live chain again and the track's plug-ins load from their descriptors. |
| `midi.expression` | `{kind: "pitchBend" or "cc" or "aftertouch" or "polyAftertouch" or "sustain", owner? (track:<routingID>), controller? (cc; default 1), note (polyAftertouch), value (-1..1 for pitchBend, else 0..1)}` | {sent: bool, owner, instrument} | implemented | Live expression for the instrument of the track whose channel is owner (the track of the last note without one; an owner that is no track of the project is refused), to where a MIDI keyboard's controllers go: an external MIDI instrument gets the message on its device and channel; a plug-in instrument gets it with its next block, in its format's terms (CLAP: MIDI, or note expressions for pressure and pitch bend when its note port takes no MIDI; VST3: the parameters it maps controllers, channel pressure and pitch bend to, and poly pressure events; Audio Units: MIDI); the engine's own instruments take pitch bend (±2 semitones) and the mod wheel (cc 1) and ignore the rest (sent: false). The pedals (kind sustain, which is cc 64, and cc 66 sostenuto and cc 67 soft) are the interface's own pedals and act on the notes the interface plays: a plug-in or external MIDI instrument gets the pedal with its value (half-pedalling is the instrument's) and its note-offs as played, and sustains by itself; the engine's own instruments have no pedal, so the host holds back their note-offs while sustain is at 0.5 or more (sostenuto and soft: sent false), and so it does for a plug-in that controller 64 cannot reach (a CLAP note port without MIDI, a VST3 plug-in that maps no parameter to it). A pedal that is down follows the interface's notes to other tracks (each instrument gets it with the first note played to it) and its release reaches every instrument it reached, whatever owner the message names; an instrument that several pedals reach (the interface's, a MIDI keyboard's) hears the highest. transport.stop sends pedal-up to every instrument; engine.panic forgets the pedals. Live input only: patterns carry no pedal. Controllers 120 and up are refused (engine.panic silences). |
| `note.off` | `{note, track?, instrument?, applyNote?, captureInput?, …}` | {ok: true, sustained?, inputAcknowledgement?} | implemented | With captureInput the reply waits for the audio thread and carries inputAcknowledgement. sustained: true when the interface's sustain pedal is down (midi.expression): the note sounds on until the pedal comes up. A plug-in or external MIDI instrument still gets the note-off (it sustains by itself); the note-off of one of the engine's own instruments is held back by the host. The interface keeps the recording of such a note open and closes it with a capture-only edge (applyNote: false) when its pedal comes up. |
| `note.on` | `{note, velocity?, track?, instrument?, captureInput?, …}` | {ok: true, inputAcknowledgement?} | implemented | Quick command: order with note.off matters. With captureInput (and inputOwner) the reply waits for the audio thread and carries inputAcknowledgement {status, applied, pattern, row, sequence, recordingClock?, …}. |
| `plugin.editor` | `{owner, kind, slot, instanceID?, open?: false to close}` | {shown, open, name, custom, width, height, windowID} | implemented | Opens the plug-in's own window in its container process, or brings the open one to the front. Built-in effects have none. On Linux only CLAP plug-ins with floating windows have editors; macOS windows (and yielding activation to the container) are type-checked only. |
| `plugin.load` | `{owner, kind: "instrument" or "effect", slot, component (or id): "clap:<id>", "vst3:<class id>", "au:<type:subtype:manufacturer>" or "stfx:<effect>", bundlePath?, state?, name?, bypass?, instanceID?, nks?}` | the descriptor {owner, kind, slot, instanceID, component, format, bundlePath, name, manufacturer, bypass, latency, tailTime, presets, hostStatus, parameters} | implemented | Long. Creates the instance in a container (or the built-in effect), replaces what the slot held in the engine's project, and answers once it is audible; the interface stores the descriptor and publishes. A failed load leaves the engine as it was. |
| `plugin.loadNKS` | `plugin.load target + {path of the .nksf / .nksfx preset}` | the descriptor, with nks: {name, path, identity, pages, warnings} | implemented | Long. The preset's VST3 class (or the VST3 a vendor ships for its VST2 identity) is looked up in the catalogue, loaded with the preset's exact state bytes, and the controller pages are bound to the parameters the plug-in has. |
| `plugin.move` | `{owner, fromSlot, toSlot}` | {moved: true, owner, fromSlot, toSlot, previousPlugins, plugins, slotMap: [{fromSlot, toSlot, instanceID}], audioPlugins, routing} | implemented | The effect in fromSlot takes the chain position of toSlot; the effects between shift, the slot numbers in use stay. Automation lanes of the owner follow their effect. previousPlugins carry the states captured before the move. No instance is reloaded. |
| `plugin.parameter` | `{owner, kind, slot, address, value, normalized?: true} or {…, bypass: bool}` | the parameter {address, name, value, normalized, display, written, …}; for bypass the descriptor | implemented | Values are normalized 0..1 unless normalized is false. An effect of an instrument's chain is set on every track that plays it. Bypass is applied to the engine's project at once. |
| `plugin.parameters` | `{owner, kind, slot, instanceID?, addresses?: [string]}` | {parameters: [{address, id, name, group, min, max, value, normalized, display, unitName, writable, automatable, hidden, stepCount?, normalizedStep?, valueStrings?}]} | implemented | address is the parameter id as a decimal string. display is the plug-in's own text (asked for the first 512 parameters). A lost plug-in answers with its last known values; one that is not loaded with the reason. |
| `plugin.preset` | `{owner, kind, slot, number?}` | the descriptor with parameters and currentPreset; without number [{number, name}] | implemented | Factory presets of the plug-in (VST3 program lists, Audio Unit presets). |
| `plugin.remove` | `{owner, kind, slot}` | {removed: bool, owner, kind, slot, instanceIDs} | implemented | Removes the descriptor and the sidechains into it from the engine's project; the instance is destroyed off the audio thread once no render plan uses it. |
| `plugin.state` | `{} for all, or a target, or a target + {state: string} to restore` | [descriptors with state] or one descriptor (stateError when capture failed) | implemented | The page rejects a reply whose result has stateError. Only the targeted plug-in is asked for its state. |
| `render.cancelSelection` | `{}` | {cancelled: bool} | implemented | Cancels the running render.selection, which then fails with "The render was cancelled." |
| `render.selection` | `{pattern, tracks: [index], firstRow, lastRow, mode?: "master" or "track", tail? (maximum, 0..60 s, default 10), thresholdDB?, holdSeconds?, sampleRate?, channels?, normalize?, name?, path?}` | {path, name, frames, sampleRate, channels, musicalFrames, renderedFrames, tailFrames, tailEnd: "silence" or "maximum", peak, normalizeGain, mode, latencyFrames, plugins, rootNote, warnings?}; progress in render.selectionProgress {job, phase, progress} events | implemented | Long: sent with ST.longRequest. st_audio::render_selection on a private engine while the live one plays; the sample is a 32-bit float WAV in Recordings of the support folder (or path). |
| `routing.capabilities` | `{}` | {buses: {max}, sends: {taps, max}, sidechains: {taps, max, inputs: [{instanceID, owner, slot, pluginName, inputBus, layoutSignature, available}]}, exportTargets: [{owner, name, kind, boundary, available}], audioExport} | implemented | What the page needs before it edits routing or exports; there is one mix model, so nothing here says whether routing is supported. The limits and taps are the project format's. exportTargets are the channels of the mixer in use (stems are taken after the fader, before the parent bus; master is the complete mix). sidechains.inputs are the inserts that read a sidechain: built-in effects with a key input and plug-ins with a second input bus. audioExport is st_samples::export::format::capabilities. |
| `routing.status` | `{}` | {strips: [owner], inserts: [{owner, slot, instanceID, component, available, reason?, latencyFrames, instrument?, format?, lost?}], unavailableSidechains: [{id, reason}], latencyFrames} | implemented | The mixer as compiled from the published project, with the live state of its plug-ins: an insert whose plug-in could not be loaded, is held back or was lost while running is unavailable with the reason and passes its signal through. The page gets the same object (without the plug-ins' live state) as routing in every status event and in the reply of routing.apply; the command is for asking at a moment of one's own (the host's tests and AI clients do). A project the mixer cannot render is refused at publication, so there is no failed state to report. |
| `sample.analyzeSlices` | `{instrument, path?, sensitivity?, minimumGapSeconds?, maximumSlices?, start?, end?}` | {method, slicePoints, slices, sliceCount, …} | implemented | Transient detection (st-samples analyze_slices) on the command's worker thread. |
| `sample.edit` | `{instrument, path?, operation, start, end, peak?, decibels?, curve?, ratio?, sourceBPM?, targetBPM?, outputDirectory?}` | sample info of the edited sample (its path is the new file) | implemented | st-samples edit_to_file: a new 24-bit WAV in Edits of the support folder; the source file is never modified. The result is in the sample cache at once. |
| `sample.info` | `{instrument, path}` | sample info, or {available: false} | implemented | Answers from the cache when the published instrument plays that path; never decodes. |
| `sample.load` | `{path, instrument}` | sample info {path, name, duration, sampleRate, channels, frames, peaks (512), wavetableFrameSize, loops, rootNote} | implemented | Decodes on the command's worker thread (st-samples load_sample; builtin: references too) into the engine's sample cache; the publication that names the path installs it. A newer load for the instrument supersedes one still decoding. |
| `sample.peaks` | `{instrument, path?, start?, end?, bins?, snap?: [position], direction?, window?}` | {path, frames, sampleRate, start, end, min: [...], max: [...], snapped?} | implemented | Waveform columns and zero-crossing snapping for the sample editor (DecodedSample::inspect). Fails with "still loading" while the sample is not decoded. |
| `sample.prefetchPreviews` | `{items: [path or {path}]}` | {queued: number} | implemented | Warms the preview cache (at most 8 files; each request replaces the previous queue). {kit} items are skipped until the library resolves a kit's preview file. |
| `sample.preview` | `{path, volume? (default 0.35)}` | {playing: true, duration, cached, startMs} or {playing: false, superseded: true} | implemented | Browser audition on the engine's preview mixer (st-audio preview): head from st-samples' PreviewCache, the tail of a long file streamed; consecutive previews crossfade. Mixed after the master fader. sample.previewEnded {path} when it has played to its end. |
| `sample.previewStatus` | `{}` | {cachedPreviews, cacheBytes, cacheHits, cacheMisses, voices, lastStartMs, underruns, leakedVoices} | implemented | Not sent by the page. |
| `sample.stopPreview` | `{}` | {playing: false} \| | implemented | Fades the preview out. engine.panic stops it too. |
| `sample.wavetables` | `{}` | [{group, name, path, frames, frameSize, wavetable}] built-in wavetables \| | implemented | From st-samples (builtin::wavetable_list). |
| `sampler.playheads` | `{enabled: bool, instrument?}` | {ok: true, watching: instrument (-1 all, -2 off)}; positions in sampler.playheads events {instrument, voices: [{instrument, track, note, mode, direction, releasing, zone, preview, position, windowStart, windowEnd, frames, level, grains}]} (30 per second, emit_latest, only when the engine's snapshot changed) \| | implemented |  |
| `step.preview` | `{step: {note, instrument, velocity, chord?…}, track?, duration?}` | {auditioning: bool} | implemented | Audition of one tracker step. |
| `transport.loop` | `{loop: {start, end} or null} (or the range itself)` | transport status \| | implemented | Pattern-mode row loop; anything that is not a range clears it. |
| `transport.play` | `{mode?: "pattern" or "song", songMode?, fromStart?, loop?, countIn?}` | transport status | implemented | mode "pattern" or songMode: false plays the active pattern; loop is a row range for Pattern mode; countIn is bars (0..4). Then status events, 20 per second (emit_latest). |
| `transport.queue` | `{pattern?, sequence?, mode?, clear?}` | {queued, pattern, sequence, mode?} | implemented | Queues a jump at the next boundary. |
| `transport.recordArm` | `{armed: bool}` | {armed} | implemented | The engine stamps input acknowledgements with the armed state. project.new and project.open disarm through it when present. |
| `transport.seek` | `{pattern?, row?, sequence?}` | transport status \| | implemented |  |
| `transport.stop` | `{}` | transport status | implemented | Stops the transport. Every instrument that was sent a pedal played live gets pedal-up (external MIDI instruments before their all-notes-off) and the notes the host holds for a sustain pedal end; a pedal still held down reaches an instrument again with the next note. (Cancelling an export belongs to audio.export.) |

### `st-plugin-*`

| Command | Payload | Result | State | Notes |
|---|---|---|---|---|
| `plugins.list` | `{details?: bool}` | [{id, name, manufacturer, vendor, version, format: "CLAP" or "VST3" or "AU" or "Builtin", kind, type, category, isInstrument, bundlePath?}]; with details {plugins, scanning, progress, blocklist: [{format, path, status, reason, blocklisted}], folders, error} | implemented | Part of the ready handshake (`plugins`). Answers from the scan cache and the built-in effects: no plug-in is loaded and no process started. |
| `plugins.recovery` | `{}` | {suspects: [{identity: {key, format, componentID, name, bundlePath?}, reasons, requiresReview, retryAuthorized, loadCrashes, lostInstances, heldBack}], warnings, pendingInstances, …} | implemented | Part of the ready handshake (`pluginRecovery`). Lists plug-ins held back after crashing twice in a row while loading (plugin-recovery.json) and plug-ins lost while running; also sent as the event plugins.recovery when a plug-in is lost. |
| `plugins.recovery.retry` | `{keys?: [identity key]}` | the recovery report + retryErrors: [{name, message}] | implemented | Long. Restores lost instances with their last state and loads held-back plug-ins of the project again. |
| `plugins.scan` | `{rescan?, retryFailed?, folders?: [path], wait?}` | {scanning: true}; with wait {scanning: false, result, plugins, blocklist}; progress in plugins.catalogue events {scanning, completed, total, path, scanned, cached, failed, plugins, blocklist} | implemented | Each plug-in file is read by a short-lived container process; one that crashes or hangs is blocklisted. folders replaces the user's folders (settings section plugins). Also runs once at start for what changed. |

### `st-library`

| Command | Payload | Result | State | Notes |
|---|---|---|---|---|
| `kit.detail` | `{path, availableComponents?}` | {name, sounds: [{slot, id, name, type, plugin, empty, missing}], compatibility: {summary, loadable, warnings, missingPlugins}, path, previewPath?} | implemented | Reads a .stkit package or a Maschine group. The installed plug-ins come from plugins.list (st-plugin-*) when that is registered; without it plug-in availability is not judged. |
| `kit.read` | `{path}` | {path, metadata, groupProject, sounds, warnings, report?} | implemented | A kit as a track group to insert. |
| `kit.save` | `{project, groupID, name?, path?, overwrite?, copySamples?}` | {path, name, warnings} or {exists: true, path, name} or {needsName: true, path} | implemented | The project is the one in the payload (the page publishes with plug-in state captured first), the session's file resolves relative sample paths, the kit folder is rescanned. The kit's audio preview is rendered into the package while it is written (st_audio::kit_preview on a private offline engine: the group's bus after its fader in the selected pattern, with its built-in effects and plug-ins; at most ten seconds or eight bars and a second of tail; Preview.wav, 16-bit stereo at 48 kHz). A preview that cannot be rendered (no pad plays a note in the pattern, only external MIDI instruments do, a live line input is on) is one of the warnings; a host without an audio engine saves kits without previews. |
| `library.cancel` | `{}` | {scanning: false, count, cancelled: true, errors, rootReports} | implemented | Stops the running scan. |
| `library.diagnostics` | `{roots?: [path]}` | [{path, readable, …}] per root | implemented | Not sent by the page. |
| `library.favorite` | `{path, favorite?: bool}` | {path, favorite}; also a library.favorite event | implemented | favorites.json in the Library folder of the support directory. |
| `library.kit` | `{path}` | kit metadata | implemented | Not sent by the page. |
| `library.kits.browse` | `{query?, offset?, limit?, filters?, favorites?, pack?, user?}` | {items: [...], total, facets…} | implemented | Kit browser page. |
| `library.metadata.edit` | `{paths: [path], changes: {…}}` | {updated, items?} | implemented | Edits library metadata of samples. |
| `library.nks` | `{path}` | NKS preset metadata: {name, vendor, product, bank, types, modes, plugin id, …} | implemented | .nksf / .nksfx only. |
| `library.nks.artwork` | `{keys: [string], variant?: "tile"}` | {images: {key: data URL}, variant} | implemented | Up to 400 keys. |
| `library.nks.browse` | `{query?, product?, bank?, subBank?, kind?, filters?, favorites?, offset?, limit?, hideOffline?}` | {items, total, products, banks, …} | implemented | NKS browser page. |
| `library.nks.rescan` | `{}` | NKS catalogue status | implemented | Progress in library.nks.progress events. |
| `library.nks.status` | `{}` | NKS catalogue status | implemented | Not sent by the page. |
| `library.removeRoot` | `{path}` | library.roots result | implemented | Also drops the root's items from the index. |
| `library.roots` | `{}` | [{path, name, source, available, scanStatus, scanErrors, indexedCount}] | implemented | Part of the ready handshake (`roots`). |
| `library.scan` | `{roots?: [path]}` | {scanning: true, …}; progress in library.progress and library.results events | implemented | Returns at once. library.chooseRoot calls it. |
| `library.search` | `{query?, category?, kind?, roots?, favorites?, offset?, limit?, filters?}` | {items: [...], total, categories, scanning} | implemented | On a worker thread; searches run one at a time and one that a newer search overtook answers {cancelled: true}. |
| `library.state.get` | `{}` | the browser's remembered state object, or {} | implemented | browser-state.json in the Library folder (256 KiB limit). |
| `library.state.set` | `{state: object}` | {saved: true} | implemented | See library.state.get. |
| `library.status` | `{}` | {scanning, count, errors, rootReports} | implemented | Not sent by the page. |

### `st-midi`

| Command | Payload | Result | State | Notes |
|---|---|---|---|---|
| `midi.clock` | `{enabled?, devices?: [{id, clock?, transport?}], leadMilliseconds?} or {} to read` | {enabled, running, leadMilliseconds, automaticLeadMilliseconds, devices: [{id, name, offline, clock, transport}]} | implemented | MIDI clock and Start/Stop/Continue from the transport position, generated on the render thread; settings section midi.clock (was midi-clock.json). |
| `midi.configure` | `{enabled?, inputID?, inputIDs?, inputChannel?, outputID?, channel?, track?, instrument?}` | midi.devices result | implemented | Device selection, and the track and instrument MIDI input plays. Input reaches the engine through st_host::areas::midi (recording captures included); notes of instruments of type midi leave through their output. |
| `midi.devices` | `{}` | {inputs: [{id, name, manufacturer, offline, connected}], outputs: [{id, name, manufacturer, offline, selected}], enabled, inputIDs, allInputs, outputID, requestedInputIDs, requestedOutputID, channel, inputChannel, error?} | implemented | Part of the ready handshake (`midi`). `error` says why no device can be used (no MIDI service on this system). Changes arrive as midi.status events. |
| `midi.performance` | `{map: [[note]] (128 rows) or null}` | {active: bool} | implemented | Chord/scale map applied to incoming MIDI notes. |

### `st-mk3`

| Command | Payload | Result | State | Notes |
|---|---|---|---|---|
| `mk3.configure` | `{enabled: bool}` | controller status + {enabled} | implemented | Settings section mk3 (was maschine-mk3.json); off by default. |
| `mk3.connect` | `{exclusive?: bool}` | controller status + {enabled} | implemented | Refused while support is off in Settings. |
| `mk3.disconnect` | `{}` | controller status + {enabled} | implemented | Releases the controller to other software. |
| `mk3.leds` | `LED state delta` | {ok: true} | implemented |  |
| `mk3.screen` | `{left?, right?} display lists` | {screens: bool} | implemented | screens is false while the displays are not taken over (mk3.connect {exclusive: true}). |
| `mk3.status` | `{diagnostics?: bool}` | {connected, exclusive, screens, …, enabled, diagnostics?} | implemented | Input arrives as mk3.input events, changes as mk3.status events. |

### `st-agent`

| Command | Payload | Result | State | Notes |
|---|---|---|---|---|
| `agent.configure` | `{enabled?: bool}` | {available, enabled, listening, clients, socket, suspendedForTermination, error?} (same as agent.status) | implemented | Turns the local AI control socket on or off (settings section agent, was ai-control.json). Off closes every client connection at once. |
| `agent.reply` | `{id, reply}` | null | implemented | the page's answer to the event agent.call {id, method, params}; reply is what STAgent.dispatch returned (its JSON text or the object). |
| `agent.status` | `{}` | {available: true, enabled, listening, clients, socket, suspendedForTermination, error?} | implemented | Also sent as the event agent.status on every change (the page passes it to STAgent.setConnection). The socket opens once the executable has its transport up. |

<!-- commands:end -->

## Events (host to page)

Events the host sends and the page handles, and who emits them.

| Event | Emitter |
|---|---|
| `command` `{name}` | `st-app`: a menu item was chosen |
| `app.quitRequested` | `st-host`: the window is closing (see `Lifecycle`) |
| `project.openRequested` `{path}` | `st-host`: the system asked to open a project file |
| `project.media.progress` | `st-host` (`st-project` search) |
| `error` `{message}` | any area; `st-host` uses it for a failed recovery save |
| `status` (sent 20 times a second with `emit_latest`), `audio.devices` (the output changed by itself, the input was lost or is back, or a publication changed the state of a line input), `sample.error`, `audio.clipError` (a sample or clip file of the published project could not be loaded), `sample.loaded`, `sample.zonesLoaded`, `audio.clipLoaded` (a background decode finished), `sampler.playheads` (`emit_latest`), `sample.previewEnded`, `audio.exportProgress`, `audio.exported`, `render.selectionProgress`, `freeze.progress`, `audio.recording` | `st-audio`: sent today |
| `plugins.catalogue` (scan progress `{scanning, completed, total, path, scanned, cached, failed, plugins}`, and `blocklist` when done), `plugin.parameter` `{owner, kind, slot, instanceID, address, value (normalized), plainValue, source: "editor"}` and `plugin.gesture` `{…, address, begin}` (a value changed, a control grabbed or released, in a plug-in's editor; changes are coalesced, values a plug-in only reports are not sent), `plugin.changed` `{…, what: "parameters" or "latency" or "state" or "editor" or "restored"}`, `plugin.error` `{error, name, lost?}` (a plug-in could not be loaded, or stopped working), `plugins.recovery` (the recovery report, when a plug-in was lost while running: the page opens the review) | `st_host::areas::plugins` from `st_audio::plugins`: sent today |
| `library.progress`, `library.results`, `library.metadata`, `library.favorite`, `library.error`, `library.kits.progress`, `library.nks.progress` | `st-library`: sent today (from the library's worker threads) |
| `midi.note` (a key played on a device; with `inputOwner` and `recordingPending` / `recordingOnly` + `inputAcknowledgement` while note recording is armed, `recordingCancelled` when a capture ends without its release), `midi.message` (control changes), `midi.status` (`midiConfiguration`, `midiDevicesChanged`, `midiError`) | `st-midi` and `st_host::areas::midi`: sent today |
| `mk3.input`, `mk3.status` | `st-mk3`: sent today |
| `agent.call` `{id, method, params}` (answered with the command `agent.reply`), `agent.status` | `st-agent`: sent today |

## Settings

`settings.json` in the support folder, one section per area (`st_host::Settings`, typed
`get`/`set`/`update`, written atomically):

| Section | Owner |
|---|---|
| `window` | `st-app` (size, position, maximised) |
| `interface` | the page, through `settings.get` / `settings.set` (the page still keeps most preferences in `localStorage`, which a `--serve` page loses with each new port) |
| `audio.devices` | `st-audio` (`{output: uid, absent for the system default; bufferSize; sampleRate}`) |
| `plugins` | `st_host::areas::plugins` (`{folders: [path], standardFolders, grouping: "plugin" or "manufacturer" or "shared" or "instance", container?: path}`) |
| `midi.clock` | `st-midi` |
| `mk3` | `st-mk3` |
| `agent` | `st-agent` |

`st-project` keeps its own files in the same folder: `Recovery.supertracker`, `Recovery-session.json`,
`Recovery Backups/`, `Templates/`, `ProjectTemplates.json`. The library keeps its files in `Library/`
there (`roots.json`, `index.bin`, `metadata.json`, `favorites.json`, `browser-state.json`,
`diagnostics.json` and the NKS and kit caches; see `cargo doc -p st-library`). Kits are saved to
`Documents/SuperTracker/Kits`, or to `Kits/` in the support folder when `SUPERTRACKER_SUPPORT_DIR`
names it. The AI control socket is `control.sock`; on macOS `st-mk3` keeps the record of stopped
Native Instruments agents there too.

## What has and has not been exercised

Exercised on Linux (Ubuntu 24.04, x86-64):

- `cargo test -p st-host -p st-app -p st-audio`. The audio tests render offline or on the null device;
  the container has no sound card, so no `cpal` output stream has been opened anywhere yet (device
  listing, the fallback to the null device and the refusal of an unknown device are exercised).
- `tests/host_e2e/run.js`: Chromium against `supertracker --serve` (boot against the real host, save
  through a scripted Save chooser, reload, New, open through a scripted Open chooser, unimplemented
  commands, token and origin checks, clean quit, session and settings restored by a second process,
  and audio: the demo song played from the page on the null device, a note edited while playing;
  the library: a folder of generated WAV files added from the Browser through the scripted folder
  chooser, scanned, searched, shown in the result list, a favorite set and found again after the
  host restarted; AI control: `supertracker --ctl` and an MCP session over `supertracker --mcp` read
  the song and write steps that appear in the page, turning AI control off in Settings disconnects
  them, turning it on lets the session reconnect).
- Plug-ins, with the container process and the CLAP and VST3 test plug-ins of `crates/plugin-host`
  (`cargo test -p st-audio --test plugins --test plugins_realtime`, `cargo test -p st-host --test
  plugins`; the tests build the container and the plug-ins when they are missing): an effect in
  the graph bit for bit, delay compensation following a latency change, automation lanes and step
  locks on their frames, a locked parameter back at the value the user set (after its row, at
  the next start or seek, and before a state is captured while stopped), a plug-in
  instrument playing a pattern, controllers and pressure
  played live reaching it with the next block (as MIDI, as CLAP note expressions, as the
  parameters a VST3 plug-in maps them to), the sustain pedal reaching it before the notes of
  that block and coming up with a panic, a sidechain, state into
  `engine.snapshot` and back from a reopened project, an offline copy equal to the live render, a
  crash inside `process` during playback (pass-through, report, restore with state), a plug-in
  that crashes while loading held back at the next start, no allocation on the render path; and
  in Chromium (`run.js`): a folder scanned, the plug-in added from the mixer's rack, its gain
  heard on its track's meter, the project saved and opened again with the state intact.
- Line inputs and Freeze by recording, on the null output device with the null input (which the
  output's thread delivers, so both share one clock; `cargo test -p st-audio --test line_inputs
  --test freeze_recording`, `cargo test -p st-host --test line_inputs`): an input through its
  track's inserts, fader and meter and in the delay compensation, paused routes, a print recorded
  in real time that equals the offline print of the track's notes plus the input frame for frame
  (song, head and loop prints), cancelled and disturbed takes, the frozen track played and
  exported; an input on a second clock (the test tone from a timer thread of its own at
  44.1 kHz under the null output at 48 kHz): the route converted and reported so, the tone at
  its level on the track and at its pitch in a take. What is exact about two clocks (no gap in
  ten minutes, the pitch to a few parts per million, the delay to a few frames, recovery from
  stalls) is measured in simulated time, where the test places every callback of both devices
  (`cargo test -p st-audio --test two_clocks`): real threads on a busy machine cannot promise
  it; and in Chromium (`run.js`): Live input ticked in the mixer, the tone on the track's
  meter, the route gone with Undo.
- Termination: `supertracker --serve` ends on SIGTERM with its services shut down (checked by hand:
  the control socket is removed, exit status 143).
- `tests/host_e2e/window_smoke.sh` under Xvfb: the GTK window with WebKitGTK loads the page from the
  custom scheme, the message channel works both ways, a menu shortcut reaches the page, a native
  context menu and the native file chooser open and are answered, Quit runs the handshake and exits 0.
  Screenshots for AI clients: `supertracker --ctl app.screenshot` and the MCP tool `screenshot` over
  `supertracker --mcp` return a PNG file of the web view (1500×915 in the 1500×940 window, whose
  menu bar takes the rest), scaled to a requested width with its proportions kept, with the
  interface in it (the script decodes the files and counts colours) and following what the window
  shows (the command palette); a request made while the native file chooser is open fails after ten
  seconds with a message; `supertracker --serve` refuses with "running without a window". Checked
  by hand with `GDK_SCALE=2` (two pixels per layout unit): the picture is 1500 wide by default and
  2560 wide on request.

Not exercised anywhere yet:

- **Audio on a real device**: `cpal` output streams (ALSA and CoreAudio), device loss and return,
  and the restart of the engine at a device's sample rate have only run against the null device and
  missing devices.
- **Audio from a real device**: no input device has been opened. `cpal` input streams, line inputs
  from hardware (the latency they are given in the delay compensation is an estimate from what the
  devices report; the microphone permission on macOS, which the first opened input asks for) and
  Freeze by recording of a real return have only run on the null devices. An input on another
  device than the output is resampled onto the output's clock; that has run against simulated
  clocks and a timer thread only. What hardware has to show: how punctual the callbacks of
  `cpal`'s Core Audio and ALSA streams are (the converter follows the two clocks by the time each
  callback begins, read from the system's monotonic clock, not by the devices' own timestamps;
  callbacks later than the feed's margin of 1 to 5 ms cost a counted gap, and if one device's
  callbacks are later on average than the other's, the real delay differs from the reported one
  by that much), whether a device
  delivers the block size it was asked for (the feeds are made for the largest block seen on
  either side, and made anew when a larger one arrives), how a USB device behaves when it drops
  out, and that two streams on one device really run in step where the host assumes one clock
  (the same device name on macOS, the same `CARD=` name on Linux).
- **Everything macOS**: the AppKit window (transparent title bar, full-size content), `window.titleBar`
  dragging, the application menu and its standard items, `rfd` panels and the alert for a hung page,
  context menus through `show_context_menu_for_nsview`, `Event::Opened` (files opened from Finder),
  screenshots for AI clients (`takeSnapshotWithConfiguration:completionHandler:`, drawn into an
  `NSBitmapImageRep` of the wanted size and encoded as PNG by AppKit; the code takes the snapshot's
  size to be in points and the window's scale factor to be its pixels per point, which only a
  Retina display can confirm),
  and whether a key the page handles also triggers its menu shortcut. The code type-checks for `aarch64-apple-darwin`; it has not been
  linked or run.
- **Plug-ins beyond the test plug-ins**: no third-party plug-in has been loaded through the host.
  Audio Units (macOS only), the editor windows of VST3 and Audio Units, yielding activation to
  the container before an editor is focused (`yieldActivationToApplication:`, called by the window
  through `st_audio::plugins::manager::yield_activation`), and the short socket directory chosen on
  macOS are type-checked only. Real-time deadlines have only met the null device: a block may
  take what is left of its render call, at least half a millisecond.
- **MIDI devices**: no port of the ALSA sequencer or of CoreMIDI has been opened (the container has
  no sequencer; the host then runs without devices and says so in `midi.devices`). Everything above
  the device layer is tested with in-memory ports. Controllers and pressure for Audio Units are
  type-checked and unit-tested as far as the scheduling of their MIDI bytes; no Audio Unit has
  received them. The timing of scheduled MIDI against the audio
  output is unverified: a block's messages are delayed by the output latency the device reports
  with each callback (`st_audio::midi::RenderMidi::set_playback_delay`), which has only ever been
  zero (the null device).
- **The Maschine MK3**: never run with a controller (`cargo doc -p st-mk3` lists what that leaves
  unverified); `--mk3-probe` ran only as far as "no Maschine MK3 found". A signal during
  `--mk3-probe --exclusive` ends the process without restoring Native Instruments' agents; the next
  launch of the application restores them from the record.
- **AI control on macOS and Windows**: Unix sockets on Linux only so far; Windows has no transport
  (named pipes are not written), and no window, so no screenshots.
- The hung-page path of the quit handshake in a real window (covered in `st-host` tests only).
- Windows: `--serve` should work; the window is not implemented there.

Behaviour worth knowing:

- Quitting from the Dock or at logout ends the event loop without the handshake (tao gives no veto):
  the host then writes the recovery copy from the last project the page published.
- A double click on the top bar always zooms; the system setting "double-click a window's title bar
  to" is not consulted.
- The page reserves room for the traffic lights (`body.native .topbar`) under every host, also where
  the window has its own title bar and in a browser.
- One project layout. `st-project` refuses older layouts with its own message; `st-library` refuses
  kit packages of the retired kit-lane layout. Nothing is migrated.
- A termination signal (SIGINT, SIGTERM, SIGHUP) ends the host through the same shutdown as Quit,
  without asking about unsaved changes (the recovery copy is written first). What `midir` cannot
  do is listed in `cargo doc -p st-midi` (`midir_backend`).
- AI control: the socket is `control.sock` in the support folder. A screenshot is a picture of the
  web view (the interface), taken by the web view itself: the menu bar, native menus, file choosers
  and plug-in editor windows are not in it. It is as wide as the window's content in layout units
  (not doubled on a high-density display) unless the client asks for a `width` (320 to 2560 pixels):
  a wider window is scaled down to that, a narrower one is never enlarged. The request waits for the
  window's event loop for at most ten seconds; while a native menu or dialog is open the loop does
  not serve it, and the client gets an error saying so. A host without a window (`--serve`) answers
  that it has none.
- Audio: one mix model. Every project has a `routing` table and every track a `routingID`; a table
  without buses is tracks into the master. A project without either is refused. The delay and
  reverb returns are the mixer's own (`st-audio`, `graph/sends.rs`), built on the comb and
  all-pass filters of `tracker_engine::effects`. Built-in effect state is base64 JSON
  (`SuperTracker.BuiltinFX`); a state in any other
  encoding is refused per effect and the effect starts from its defaults. Play does not wait for
  samples (they are loaded when the project is published). See `cargo doc -p st-audio` for the
  design and the seams.
- Line inputs have no command. A track's `audioInput` `{enabled, deviceUID, channels, gain,
  monitor?}` in the project is the routing, applied by every publication; `audio.devices` (also
  sent as an event) says which routes are live and why the others are paused. The input device
  chosen with `audio.configure` is remembered; a route is live only while its `deviceUID` is that
  input. Opening a project with enabled routes opens the input device.
- The input and the output may be two devices with two clocks and two sample rates. Everything
  that brings input audio into the mix (line inputs, monitoring, the source of Freeze by
  recording, takes of the input) is then resampled onto the output's clock by a ratio the host
  measures (`st_audio::asrc`, `st_audio::inputs`); one device with one clock passes its frames
  through unchanged. A take of the input is a file at the engine's rate on the output's clock.
- Plug-ins: every plug-in runs in a `supertracker-plugin-container` process (CLAP, VST3, Audio
  Units; no VST2, no in-process hosting). Components are named `clap:<id>`, `vst3:<class id>`,
  `au:<type>:<subtype>:<manufacturer>`; a descriptor with any other identity or state encoding is
  reported as not installed or not readable, per plug-in, and keeps its bytes. The interface owns
  the project: `plugin.load`, `.remove` and `.move` change the engine's copy and answer with
  descriptors; the host pushes no plug-in list of its own. A descriptor, a plug-in command and a
  plug-in event name the channel by `owner` (`track:<routingID>`, `bus:<id>`, `master`,
  `instrument:<n>`), never by track index. A publication loads its plug-ins
  before it answers. The recovery store holds a plug-in back after two crashes at load in a row
  (or a load the host did not survive); a crash while playing costs one container and is restored
  with Retry. Editors report parameter changes with the events `plugin.parameter` and
  `plugin.gesture`.
- Plug-in parameters and automation: the value the user leaves a parameter at (loaded with the
  state, set from the interface, changed in the plug-in's editor) is its base value, which the
  host keeps for every plug-in of every format (`st_audio::plugins::manager::ParamTable`). A
  lane or a step lock moves the parameter only while it applies: it is back at its base value
  at the end of a locked row where no lane takes over, and when playback starts or seeks.
  Stopping returns nothing (tails ring out with the values the song left); a state captured
  while the song does not play (`engine.snapshot`, `plugin.state`, `project.save`) brings the
  parameters back first, so it is the patch as the user made it. There is no command to
  capture a base patch: nothing needs capturing. A state captured while the song plays
  contains what the lanes and locks hold at that moment.
- Expression: controllers, channel and poly pressure, pitch bend and program changes played
  live (`midi.expression`, a MIDI keyboard) go to the instrument of their track. An external
  MIDI instrument gets them on its device and channel at once. A plug-in instrument gets them
  at the first frame of its next block: as MIDI (CLAP note ports that take it, Audio Units),
  as note expressions for pressure and pitch bend (CLAP note ports that take CLAP events
  only; control changes have no CLAP event), or as the parameters it maps controllers,
  channel pressure and pitch bend to plus poly pressure events (VST3). The engine's own
  instruments take pitch bend (two semitones either way) and the mod wheel.
- The pedals: the sustain pedal belongs to the instrument. A plug-in instrument and an
  external MIDI instrument get it (controller 64, with its value, so half-pedalling and
  re-pedalling are the instrument's; likewise sostenuto, 66, and soft, 67) and their
  note-offs as played: the instrument sustains. A plug-in gets a pedal at the first frame of
  its next block, before the notes of that block, so a key released after the pedal went
  down is released after it for the plug-in as well (the other way round, a pedal pressed
  less than one audio block after a key came up can still catch the note; the same holds for
  a device, whose notes are timed to the audio while its pedal is sent at once). The engine's
  own instruments have no pedal: while a sustain pedal is at 64 or more the host holds back
  the note-offs of the keys its source releases. So it does for a plug-in instrument that
  controller 64 cannot reach (a CLAP note port that takes CLAP note events only; a VST3
  plug-in that maps no parameter to the controller): the container says which plug-ins the
  pedal can reach (`INFO_SUSTAIN_PEDAL`). One record keeps the pedals of every source
  (`st_midi::SustainState` in `areas/audio.rs`): each MIDI device and channel has its own
  pedals, the interface has one (`midi.expression`). A pedal acts on what its own source and
  channel play; it follows them to another track (the first instrument keeps sustaining, the
  next gets the pedal with the first note played to it, every instrument it reached is told
  when it comes up); an instrument several pedals reach is sent the highest of them. Stop
  sends pedal-up to every instrument (to a device before its all-notes-off) and ends what the
  host holds; a pedal still down reaches an instrument again with the next note. A panic
  silences everything (plug-ins and devices are sent the pedals up with all-notes-off) and
  forgets the pedals. An instrument replaced while a pedal is down (another plug-in, one
  restored after a crash, another device or channel, a device that came back) gets the pedal
  again with the next note; a device or channel an instrument leaves is sent pedal-up, and so
  is a plug-in whose track a publication moves or removes (before the publication, which also
  lets go of the notes the host holds on such a track). A keyboard that is unplugged, and an
  input route that changes, lift their pedals. When a song ends by itself the render thread
  silences the external instruments as on Stop; a pedal still down is sent again with the
  next note.
  Patterns carry notes with gates and no pedal data, so all of this is live input only; while
  a live pedal is down on a plug-in or a device, the notes the song plays on that instrument
  are sustained by it too, as on the instrument itself.
- Recording under the pedal: a recorded note is as long as it sounded. A key released while
  the sustain pedal of its source is down keeps its capture open until the note ends (the
  pedal comes up or goes below half, the same key is struck again on that instrument, Stop),
  whether the host holds the note or the instrument does. For MIDI keys the host does this
  (`areas/midi.rs`); for its own keys the interface does (`ui/app-notes.js`, told about its
  pedal by the controller code). A note that another source's pedal keeps sounding on a
  plug-in or device is recorded with the length of its key.

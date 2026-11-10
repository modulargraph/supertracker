# Rust host: port status

The target design is [RUST_ARCHITECTURE.md](RUST_ARCHITECTURE.md). This file is the work list: what
runs today, how to run and test it, and every bridge command with the crate that owns it and its state.

## What runs

| Piece | Crate | State |
|---|---|---|
| Bridge core: protocol, command table, events, transports' queue | `st-host` | done, tested on Linux |
| Session: publish, new, open, save, autosave and recovery, templates, MIDI files, missing media | `st-host` on `st-project` | done, tested on Linux |
| Settings file | `st-host` | done |
| `--serve`: HTTP + WebSocket on 127.0.0.1 | `st-app` | done, tested end to end in Chromium |
| Window: tao + wry + muda, menus, native context menus, file choosers, quit handshake | `st-app` | runs on Linux (GTK/WebKitGTK, checked in a virtual display); macOS code type-checks, **not yet run on a Mac** |
| Audio playback core: project to engine, the mixer graph (strips, inserts with built-in effects, sends, sidechains, buses, master, delay compensation, automation, meters), transport, live notes, devices (cpal and the null device), offline render | `st-audio` on `tracker-engine`, registered in `st-host` (`areas/audio.rs`) | done, tested on Linux (offline renders, allocation-free render path, null device, Chromium end to end); CoreAudio only type-checked |
| Sample commands, recording and line inputs, export / bounce / freeze / render selection, plug-in instances, library, MIDI devices, Maschine MK3, AI control | `st-audio` (with `st-samples`), `st-plugin-*`, `st-library`, `st-midi`, `st-mk3`, `st-agent` | no commands registered in the host yet: they answer "not implemented" |

The interface (`Resources/Web`) reaches its host through `host-transport.js`. It no longer talks to
the Swift host: the page does not use `window.webkit.messageHandlers` and does not define
`window.nativeReceive` any more.

## Building and running

```sh
cd rust
cargo build -p st-app                      # the `supertracker` executable
cargo test -p st-host -p st-app            # no display or audio hardware needed
target/debug/supertracker                  # the window; the interface is found in ../Resources/Web
target/debug/supertracker --serve          # no window: prints http://127.0.0.1:<port>/?token=<token>
target/debug/supertracker --help
```

Linux build packages (Ubuntu 24.04): `libgtk-3-dev libwebkit2gtk-4.1-dev libasound2-dev pkg-config`
(and a C toolchain; `libasound2-dev` is for `cpal`, the audio device layer of `st-audio`). To run the
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

### Tests

| What | How |
|---|---|
| Bridge guarantees, session and settings commands, catalogue; the audio commands on the null device (`tests/audio.rs`) | `cargo test -p st-host` |
| The audio core: unit tests, offline renders against the mixer's rules (`tests/offline.rs`), the demo song, transport, devices (`tests/playback.rs`), no allocation on the render path (`tests/realtime.rs`) | `cargo test -p st-audio` |
| Render cost of the demo song and of a 64-track project | `cargo run --release -p st-audio --example bench` |
| Command line, WebSocket framing, file serving, menus, window helpers | `cargo test -p st-app` |
| The page's bridge, transport and quit handshake | `node tests/host_transport_tests.js`, `node tests/host_quit_tests.js`, and every `tests/*_tests.js` (they play the host through `tests/helpers/host.js`) |
| The real page against the real host, in Chromium | `node tests/host_e2e/run.js` ([README](../tests/host_e2e/README.md)) |
| The real window in a virtual display | `tests/host_e2e/window_smoke.sh` |

File choosers cannot open in a test. `--dialog-answers <file>` answers them from a JSON array instead:
each chooser takes the next element, a path string, or `null` for Cancel. The file is read again for
every chooser, so a test may append answers while the host runs. When the answers run out the
command fails with a message that says so.

## The bridge in the Rust host

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
- `st_host::testing::TestHost` runs commands as the page would, for each area's tests.
- After adding or finishing a command, change its entry in `rust/st-host/src/catalog_table.rs` and
  run `UPDATE_PORT_STATUS=1 cargo test -p st-host --test port_status`, which rewrites the tables
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

### What changed in the page

- New `host-transport.js` (first script after `util.js` and `busy.js`): chooses the channel and exposes
  `STHostTransport.post(message)` / `onMessage(handler)`; `available` is false on a page opened with
  no host, where `app-bridge.js` keeps its browser-preview answers.
- `app-bridge.js` posts through it and receives replies and events from it (`window.nativeReceive` is
  gone; events still go to `window.nativeEvent`). New: `ST.longRequest` (no time limit; `freeze.js`
  and `render-selection.js` use it instead of posting to WebKit themselves) and `ST.hostImplements`.
- `ready` now returns `commands`, the list of implemented commands; `context-menu.js` uses its own
  menu when `ui.contextMenu` is not among them.
- `window-drag.js` asks the host to move the window (`window.titleBar`) on a press in empty top bar
  space, instead of reporting hover regions to a WebKit message handler.
- Quit: the host sends the event `app.quitRequested`; `app-session.js` flushes pending edits, asks
  about unsaved changes with the page's own Save / Discard / Cancel dialog, and answers with
  `app.quitAnswer`. The host no longer evaluates scripts in the page to collect the project.

## Commands

State is **implemented**, **partial** (registered; the note says what is missing) or **not yet** (the
request is answered with "`<name>` is not implemented in the Rust host yet (it belongs to
`<crate>`)."). The crate is the one that owns the behaviour; every handler is registered in `st-host`
(or `st-app` for the window's own) and delegates there. Payload and result are the JSON shapes in
brief; `?` marks optional fields. Where a shape is abbreviated ("…"), the Swift handlers
(`Sources/SuperTracker/Bridge.swift`, `AudioHost+*Commands.swift`, `LibraryService.swift`,
`MaschineMK3.swift`) and the page's call sites are the reference until the area is ported.

<!-- commands:begin (generated from rust/st-host/src/catalog_table.rs; see rust/st-host/tests/port_status.rs) -->
| Crate | Commands | Implemented | Partial | Not yet |
|---|---:|---:|---:|---:|
| `st-host` | 15 | 12 | 3 | 0 |
| `st-app` | 2 | 2 | 0 | 0 |
| `st-project` | 9 | 9 | 0 | 0 |
| `st-audio` | 56 | 13 | 4 | 39 |
| `st-plugin-*` | 4 | 0 | 0 | 4 |
| `st-library` | 21 | 0 | 0 | 21 |
| `st-midi` | 4 | 0 | 0 | 4 |
| `st-mk3` | 6 | 0 | 0 | 6 |
| `st-agent` | 2 | 0 | 0 | 2 |
| **all** | **119** | **36** | **7** | **76** |

### `st-host`

| Command | Payload | Result | State | Notes |
|---|---|---|---|---|
| `app.external` | `{url} or {path, reveal: true}` | {opened: true} | implemented | Only http(s) links; reveal shows an existing file in the file manager. |
| `app.quitAnswer` | `{state: "deciding" or "stay" or "proceed", reason?, project?, force?}` | {} or {quitting: true} | implemented | New in the Rust host: the page's answer to the app.quitRequested event (st_host::Lifecycle). |
| `dialog.export` | `{mode?: "stems", name?}` | {path} or {path, mode: "stems"} or {cancelled: true} | implemented | Stems: choose a folder; a new "<name> stems" folder is reserved in it. |
| `dialog.openSample` | `{}` | {path} or {cancelled: true} | implemented | Open panel for audio files. |
| `engine.project` | `{project, replace?, forcePluginState?, awaitPlugins?}` | the status (as in status events); {audio: false} in a host without an audio engine | implemented | Validation, dirty flag and the st-project session (st-host), then the engine and the mixer through st_host::ProjectAudio (st-audio): unchanged patterns are not sent again, and a mixer that did not change is not recompiled. Samples that cannot be loaded are reported in sample.error events. |
| `library.chooseRoot` | `{}` | library.scan result, or {cancelled: true} | partial | The folder chooser is done; the scan it starts is library.scan (st-library), so it fails with that command's "not implemented" until then. |
| `project.autosave` | `{project?}` | {saved: true} or {saved: false, deferred: true} | implemented | Recovery copy; deferred while a save or session change is in flight. The host also writes it every 20 s. |
| `project.media.chooseRoot` | `{}` | {path} or {cancelled: true} | implemented | Folder chooser. |
| `project.new` | `{templateID?, blank?}` | {project} from a template, or null (the page uses its blank project) | implemented | Uses the default template unless blank. |
| `project.open` | `{path?}` | {project, path, dirty} or {cancelled: true} | implemented | Open panel without a path. Projects in a retired layout are refused with st-project's message. |
| `project.save` | `{project?, path?, collectSamples?}` | {project, path, saved: true, recoveryWarning?} or {cancelled: true} | partial | Complete for the document and its media folder. Plug-in state is captured through engine.snapshot once st-audio registers it; until then the project keeps the states it carries. |
| `project.saveAs` | `{project?, path?, collectSamples?}` | as project.save | partial | Always asks where unless a path is given. Same plug-in state note as project.save. |
| `ready` | `{}` | {version, commands, project, dirty, path, roots, plugins, audio, midi, pluginRecovery?, warning?, audioError?, openPath?} | implemented | The handshake. roots, plugins, audio, midi and pluginRecovery come from library.roots, plugins.list, audio.devices, midi.devices and plugins.recovery when those are registered, and are empty until then. |
| `settings.get` | `{key?}` | {value} or {values: {…}} | implemented | New in the Rust host: interface preferences in settings.json (section `interface`). Not used by the page yet. |
| `settings.set` | `{key, value} (null removes)` | {} | implemented | See settings.get. |

### `st-app`

| Command | Payload | Result | State | Notes |
|---|---|---|---|---|
| `ui.contextMenu` | `{x, y, items: [{title, id?, enabled?, checked?, key?, items?} or {separator: true} or {header: true, title}]}` | {id: string or null} | implemented | Native menu through muda, in the window only. Under --serve the command is absent and the page shows its own menu (it checks ready.commands). |
| `window.titleBar` | `{doubleClick?: bool}` | null | implemented | New in the Rust host, macOS window only: a press on empty top bar space starts the window drag (double click zooms). Replaces the windowDrag message handler. |

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

### `st-audio`

| Command | Payload | Result | State | Notes |
|---|---|---|---|---|
| `audio.bounceTrack` | `{track, path?, format fields of audio.export}` | {path, clip: {id, name, path, track, startBeat, durationBeats, …}, sourceTrack, sourceTrackIndex, duration} | not yet | Offline render of one track to a file (default: Renders folder in the support directory). |
| `audio.clipInfo` | `{path}` | sample info: {path, frames, sampleRate, channels, duration, peaks…} | not yet | Decodes an audio clip's file once and caches its metadata. File work in st-samples (decode_file, DecodedSample::info). |
| `audio.configure` | `{output?, outputDevice?, outputSystemDefault?, bufferSize?, sampleRate?, input?, inputDevice?, inputChannels?, monitor?, inputMeter?}` | audio.devices result | partial | Selects the output (by id, by uid in `output`, or outputSystemDefault), the buffer size and the sample rate; remembered in the settings section audio.devices. A device that cannot be opened leaves the null device running (fallback: true, deviceError). Input fields (input, inputDevice, inputChannels, monitor, inputMeter) are ignored until recording is ported. |
| `audio.devices` | `{}` | {devices, inputs, outputs: [{id, uid, name, inputChannels, outputChannels, sampleRate, bufferSize}], defaultInput, defaultOutput, outputDevice, outputUid, sampleRate, bufferSize, bufferSizes, running, fallback, deviceError?, …} | implemented | Part of the ready handshake (`audio`). Devices come from cpal; the null device (id 0, uid "null") is always among the outputs and is what runs while no output is connected. Also sent as an event when the output changes by itself. |
| `audio.export` | `{path, mode: "mix" or "stems" or "track", owners?, tail?, format fields (sampleRate, bitDepth, …)}` | {path or paths, duration, …}; progress in audio.exportProgress events | not yet | Offline render through the same graph; Stop cancels it. |
| `audio.inputs` | `{inputs?: [...], input?, track?}` | line input configuration and status | not yet | Same handler as audio.inputs.configure. |
| `audio.inputs.configure` | `{inputs?: [...], input?, track?}` | line input configuration and status | not yet | Live line inputs routed into tracks. |
| `audio.kitPreview` | `{groupID, path}` | {path, warning?} | not yet | Not sent by the page: kit.save calls it to render a kit's audio preview. |
| `audio.record` | `{source, instrument?, owner?, monitor?, monitorVolume?}` | recording status; progress in audio.recording events | not yet | Starts recording an input to a sample. |
| `audio.renderDiagnostics` | `{}` | {units, inputCapture, renderClock, heartbeat, sampleRates, …} | not yet | Not sent by the page: diagnostics for tests and the agent. |
| `audio.stopRecord` | `{}` | {path, duration, …} of the recorded file | not yet | Ends audio.record. |
| `builtin.drumModels` | `{}` | [{id, name, parameters…}] | not yet | Not sent by the page today. Metadata of the built-in drum models (tracker-engine). |
| `builtin.effects` | `{}` | [{id: "stfx:<effect>", name, category, sidechain, parameters: [{address, id, name, unit, min, max, defaultValue, logarithmic, integer, toggle, labels}]}] | implemented | Metadata of the built-in effects (tracker-engine); a parameter's index is its address. |
| `engine.instrumentParameter` | `{instrument, target, value}` | {ok: bool} | implemented | Live change of one built-in instrument parameter (cutoff, resonance, automation targets). |
| `engine.panic` | `{}` | transport status | implemented | All sound off: voices, effect tails, held sustain. project.new calls it when present. |
| `engine.snapshot` | `{}` | the project as the engine holds it, with audioPlugins: [{…, state}] captured now | partial | The published project with the state of built-in effects captured (base64 JSON, format SuperTracker.BuiltinFX). Plug-in instances are not hosted yet: their descriptors keep the state they carry. |
| `freeze.status` | `{}` | {tracks: [{track, state, stale?, …}]} | not yet | Freeze state of every track. |
| `freeze.track` | `{track, tail?, maxTail?, method?, patterns?, monitor?}` | {project or track freeze record, media: [...]}; progress in freeze.progress events | not yet | Long: the page sends it with ST.longRequest (no time limit). |
| `freeze.unfreeze` | `{track}` | {track, project?} | not yet | Long, like freeze.track. |
| `midi.expression` | `{kind: pitch bend, controller, aftertouch or sustain; track, note?, controller?, value}` | {sent: bool, track, instrument} | partial | Reaches the engine: pitch bend (value -1..1 is ±2 semitones), the mod wheel (cc 1) and sustain (cc 64 or kind sustain). Other controllers and aftertouch answer sent: false until plug-in instruments and MIDI outputs exist. |
| `note.off` | `{note, track?, instrument?, applyNote?, captureInput?, …}` | {ok: true, sustained?, inputAcknowledgement?} | implemented | With captureInput the reply waits for the audio thread and carries inputAcknowledgement. A held sustain pedal defers the release (sustained: true). |
| `note.on` | `{note, velocity?, track?, instrument?, captureInput?, …}` | {ok: true, inputAcknowledgement?} | implemented | Quick command: order with note.off matters. With captureInput (and inputOwner) the reply waits for the audio thread and carries inputAcknowledgement {status, applied, pattern, row, sequence, recordingClock?, …}. |
| `plugin.captureBasePatch` | `{owner or track, kind, slot, instanceID?}` | the plug-in descriptor with its new base patch | not yet | Only while stopped. |
| `plugin.editor` | `{owner or track, kind, slot, instanceID?, open?}` | {open: bool, …} | not yet | Opens the plug-in's own window (in its container process). |
| `plugin.load` | `{owner or track, kind: "instrument" or "effect", slot, id/component, state?, …}` | the loaded plug-in's descriptor {instanceID, name, kind, owner, slot, parameters…} | not yet | Long. The instance lives in a container (st-plugin-*). |
| `plugin.loadNKS` | `plugin.load target + {path of the NKS preset}` | the loaded plug-in's descriptor | not yet | Long. |
| `plugin.move` | `{owner or track, fromSlot, toSlot, …}` | {plugins, slotMap, audioPlugins, routing?} | not yet | Reorders a strip's insert effects and remaps their automation. |
| `plugin.parameter` | `{owner or track, kind, slot, address, value, normalized?}` | {address, value, display?} \| | not yet |  |
| `plugin.parameters` | `{owner or track, kind, slot, instanceID?}` | {parameters: [{address, name, value, min, max, unit, …}]} \| | not yet |  |
| `plugin.preset` | `{owner or track, kind, slot, preset or index}` | the plug-in's descriptor after the change \| | not yet |  |
| `plugin.remove` | `{owner or track, kind, slot}` | {removed: bool, …} | not yet | Also removes sidechains into the instance. |
| `plugin.state` | `{} for all, or a target, or a target + {state: string} to restore` | [descriptors with state] or one descriptor (stateError when capture failed) | not yet | The page rejects a reply whose result has stateError. |
| `render.cancelSelection` | `{}` | {cancelled: bool} | not yet | Cancels render.selection. |
| `render.selection` | `{tracks, startBeat, endBeat or rows, tail?, destination…}` | {path, clip or sample, duration}; progress in render.selectionProgress events | not yet | Long: sent with ST.longRequest. |
| `routing.apply` | `{project}` | {project, routing, status} | not yet | Validates and applies a routing change with the complete project. |
| `routing.capabilities` | `{}` | {maxBuses, sidechain support, …, audioExport: formats} \| | not yet |  |
| `routing.status` | `{}` | {projectUnavailable?: bool, failure?, strips…} | partial | The compiled mixer: {standard, strips, inserts: [{owner, slot, instanceID, component, available, reason?, latencyFrames}], unavailableSidechains, latencyFrames}. A project the mixer cannot render is refused at publication, so projectUnavailable is always false and failure null. |
| `routing.validate` | `{project}` | validation report {valid, errors, latency…} \| | not yet |  |
| `sample.analyzeSlices` | `{instrument, path?, sensitivity…}` | {slices: [frame], …} | not yet | Transient detection; up to 180 s. File work in st-samples (analyze_slices). |
| `sample.edit` | `{instrument, path?, operation, start, end, …}` | sample info of the edited sample | not yet | Writes edits into the support folder's Edits directory. File work in st-samples (edit_to_file). |
| `sample.info` | `{instrument, path}` | sample info, or {available: false} \| | not yet |  |
| `sample.load` | `{path, instrument}` | sample info {path, frames, sampleRate, channels, duration, …} | not yet | Decoding on a worker; a newer load for the instrument supersedes it. File work in st-samples (decode_file); st-audio installs the result in the engine. |
| `sample.peaks` | `{instrument, path?, start?, end?, width?}` | {peaks: [...], frames, …} | not yet | Waveform overview for the sample editor. File work in st-samples (DecodedSample::inspect). |
| `sample.prefetchPreviews` | `{items: [path or {path}]}` | {queued: number} | not yet | Warms the preview cache. |
| `sample.preview` | `{path, volume?}` | {playing: true, duration, cached, startMs} or {playing: false, superseded: true} | not yet | Browser audition. |
| `sample.previewStatus` | `{}` | preview player status | not yet | Not sent by the page. |
| `sample.stopPreview` | `{}` | {playing: false} \| | not yet |  |
| `sample.wavetables` | `{}` | [{name, path, frames…}] built-in wavetables \| | not yet | From st-samples (builtin::wavetable_list). |
| `sampler.playheads` | `{enabled: bool, instrument?}` | {ok: true, watching: instrument}; positions in sampler.playheads events (30 per second, emit_latest) \| | not yet |  |
| `step.preview` | `{step: {note, instrument, velocity, chord?…}, track?, duration?}` | {auditioning: bool} | implemented | Audition of one tracker step. |
| `transport.loop` | `{loop: {start, end} or null} (or the range itself)` | transport status \| | implemented | Pattern-mode row loop; anything that is not a range clears it. |
| `transport.play` | `{mode?: "pattern" or "song", songMode?, fromStart?, loop?, countIn?}` | transport status | implemented | mode "pattern" or songMode: false plays the active pattern; loop is a row range for Pattern mode; countIn is bars (0..4). Then status events, 20 per second (emit_latest). |
| `transport.queue` | `{pattern?, sequence?, mode?, clear?}` | {queued, pattern, sequence, mode?} | implemented | Queues a jump at the next boundary. |
| `transport.recordArm` | `{armed: bool}` | {armed} | implemented | The engine stamps input acknowledgements with the armed state. project.new and project.open disarm through it when present. |
| `transport.seek` | `{pattern?, row?, sequence?}` | transport status \| | implemented |  |
| `transport.stop` | `{}` | transport status | implemented | Stops the transport. (Cancelling an export belongs to audio.export.) |

### `st-plugin-*`

| Command | Payload | Result | State | Notes |
|---|---|---|---|---|
| `plugins.list` | `{}` | [{id, name, vendor, format, kind, …}] | not yet | Part of the ready handshake (`plugins`). Formats: CLAP, VST3, Audio Units (macOS). |
| `plugins.recovery` | `{}` | {suspects: [{id, name, requiresReview, …}]} | not yet | Part of the ready handshake (`pluginRecovery`) when registered. |
| `plugins.recovery.retry` | `{ids?: [...]}` | recovery report after the retry | not yet | Long. |
| `plugins.scan` | `{}` | {scanning: true}; progress in plugins.catalogue events | not yet | Discovery runs in isolated scanner processes. |

### `st-library`

| Command | Payload | Result | State | Notes |
|---|---|---|---|---|
| `kit.detail` | `{path}` | {name, sounds: [{slot, id, name, type, plugin, empty, missing}], compatibility: {summary, loadable, warnings, missingPlugins}, path, previewPath?} | not yet | Reads a .stkit package (Maschine .mxgrp import is not ported: no legacy). |
| `kit.read` | `{path}` | {project, metadata, warnings, groupProject} | not yet | A kit as a track group to insert. |
| `kit.save` | `{groupID, name?, path?, overwrite?, copySamples?, previewPath?, generatePreview?}` | {path, name, warnings} or {exists: true, path, name} or {needsName: true, path} | not yet | Needs engine.snapshot (plug-in state) and audio.kitPreview. |
| `library.cancel` | `{}` | {scanning: false, count, cancelled: true, errors, rootReports} | not yet | Stops the running scan. |
| `library.diagnostics` | `{roots?: [path]}` | [{path, readable, …}] per root | not yet | Not sent by the page. |
| `library.favorite` | `{path, favorite?: bool}` | {path, favorite}; also a library.favorite event | not yet | Favourites were favorites.json in the library cache. |
| `library.kit` | `{path}` | kit metadata | not yet | Not sent by the page. |
| `library.kits.browse` | `{query?, offset?, limit?, filters?, favorites?, pack?, user?}` | {items: [...], total, facets…} | not yet | Kit browser page. |
| `library.metadata.edit` | `{paths: [path], changes: {…}}` | {updated, items?} | not yet | Edits library metadata of samples. |
| `library.nks` | `{path}` | NKS preset metadata: {name, vendor, product, bank, types, modes, plugin id, …} | not yet | .nksf / .nksfx only. |
| `library.nks.artwork` | `{keys: [string], variant?: "tile"}` | {images: {key: data URL}, variant} | not yet | Up to 400 keys. |
| `library.nks.browse` | `{query?, product?, bank?, subBank?, kind?, filters?, favorites?, offset?, limit?, hideOffline?}` | {items, total, products, banks, …} | not yet | NKS browser page. |
| `library.nks.rescan` | `{}` | NKS catalogue status | not yet | Progress in library.nks.progress events. |
| `library.nks.status` | `{}` | NKS catalogue status | not yet | Not sent by the page. |
| `library.removeRoot` | `{path}` | library.roots result | not yet | Also drops the root's items from the index. |
| `library.roots` | `{}` | [{path, name, count, available, …}] | not yet | Part of the ready handshake (`roots`). |
| `library.scan` | `{roots?: [path]}` | {scanning: true, …}; progress in library.progress events | not yet | library.chooseRoot calls it. |
| `library.search` | `{query?, category?, kind?, roots?, favorites?, offset?, limit?, filters?}` | {items: [...], total, categories, scanning} | not yet | Newer searches supersede queued ones. |
| `library.state.get` | `{}` | the browser's remembered state object, or {} | not yet | Was browser-state.json (256 KiB limit). |
| `library.state.set` | `{state: object}` | {saved: true} | not yet | See library.state.get. |
| `library.status` | `{}` | {scanning, count, errors, rootReports} | not yet | Not sent by the page. |

### `st-midi`

| Command | Payload | Result | State | Notes |
|---|---|---|---|---|
| `midi.clock` | `{enabled?, devices?: [id], leadMilliseconds?} or {} to read` | {enabled, devices, leadMilliseconds, …} | not yet | MIDI clock output; was midi-clock.json (settings section midi.clock). |
| `midi.configure` | `{enabled?, input?, inputID?, inputIDs?, inputs?, inputChannel?, track?, instrument?, output fields}` | MIDI settings and device lists | not yet | Routes MIDI input to a track/instrument; starts the audio engine when enabled. |
| `midi.devices` | `{}` | {inputs: [...], outputs: [...], settings…} | not yet | Part of the ready handshake (`midi`). |
| `midi.performance` | `{map: object or null}` | {active: bool} | not yet | Chord/scale map applied to incoming MIDI notes. |

### `st-mk3`

| Command | Payload | Result | State | Notes |
|---|---|---|---|---|
| `mk3.configure` | `{enabled: bool}` | controller status + {enabled} | not yet | Was maschine-mk3.json (settings section mk3). |
| `mk3.connect` | `{exclusive?: bool}` | controller status + {enabled} | not yet | Refused while support is off in Settings. |
| `mk3.disconnect` | `{}` | controller status + {enabled} \| | not yet |  |
| `mk3.leds` | `LED state delta` | {ok: true} \| | not yet |  |
| `mk3.screen` | `{left?, right?} screen images` | {screens: …} \| | not yet |  |
| `mk3.status` | `{diagnostics?: bool}` | {connected, exclusive, …, enabled} | not yet | Input arrives as mk3.input events. |

### `st-agent`

| Command | Payload | Result | State | Notes |
|---|---|---|---|---|
| `agent.configure` | `{enabled?: bool}` | {available, enabled, clients, …} (same as agent.status) | not yet | Turns the local AI control socket on or off; the setting was ai-control.json. |
| `agent.status` | `{}` | {available: bool, enabled: bool, clients: number, …} | not yet | The old host answered {available:false, enabled:false, clients:0} when the control server was absent. |

<!-- commands:end -->

## Events (host to page)

Events the page handles today, and who will emit them.

| Event | Emitter |
|---|---|
| `command` `{name}` | `st-app`: a menu item was chosen |
| `app.quitRequested` | `st-host`: the window is closing (see `Lifecycle`) |
| `project.openRequested` `{path}` | `st-host`: the system asked to open a project file |
| `project.media.progress` | `st-host` (`st-project` search) |
| `error` `{message}` | any area; `st-host` uses it for a failed recovery save |
| `status` (sent 20 times a second with `emit_latest`), `audio.devices` (the output changed by itself), `sample.error` (a sample of the published project could not be loaded) | `st-audio`: sent today |
| `transport.status`, `sampler.playheads` (`emit_latest`), `audio.latency`, `audio.recording`, `audio.exportProgress`, `audio.clipLoaded`, `audio.clipsLoaded`, `audio.clipError`, `sample.loaded`, `sample.previewEnded`, `sample.preview.stopped`, `freeze.progress`, `render.selectionProgress`, `routing.changed`, `routing.error`, `plugin.changed`, `plugin.error`, `plugins.changed`, `audioPlugins.changed` | `st-audio`: not sent yet |
| `plugins.catalogue` | `st-plugin-*` (scan progress) |
| `library.progress`, `library.results`, `library.metadata`, `library.favorite`, `library.error`, `library.kits.progress`, `library.nks.progress` | `st-library` |
| `midi.note`, `midi.message`, `midi.status` | `st-midi` |
| `mk3.input`, `mk3.status` | `st-mk3` |
| `project.loaded`, `project.changed`, `project.sequence` | `st-agent` (a client replaced or edited the project) |

## Settings

`settings.json` in the support folder, one section per area (`st_host::Settings`, typed
`get`/`set`/`update`, written atomically). The Swift host kept the same information in separate files,
which are not read (no migration):

| Section | Owner | Was |
|---|---|---|
| `window` | `st-app` (size, position, maximised) | the window frame autosave in UserDefaults (`SuperTrackerMain`) |
| `interface` | the page, through `settings.get` / `settings.set` | (new; the page still uses `localStorage`, which a `--serve` page loses with each new port) |
| `audio.devices` | `st-audio` (`{output: uid, absent for the system default; bufferSize; sampleRate}`) | `audio-devices.json` |
| `midi.clock` | `st-midi` | `midi-clock.json` |
| `mk3` | `st-mk3` | `maschine-mk3.json` (`{enabled}`) |
| `agent` | `st-agent` | `ai-control.json` |

`st-project` keeps its own files in the same folder: `Recovery.supertracker`, `Recovery-session.json`,
`Recovery Backups/`, `Templates/`, `ProjectTemplates.json`. The library's `browser-state.json` and
`favorites.json` belong to `st-library` (`library.state.*`, `library.favorite`).

## What has and has not been exercised

Exercised on Linux (Ubuntu 24.04, x86-64):

- `cargo test -p st-host -p st-app -p st-audio`. The audio tests render offline or on the null device;
  the container has no sound card, so no `cpal` output stream has been opened anywhere yet (device
  listing, the fallback to the null device and the refusal of an unknown device are exercised).
- `tests/host_e2e/run.js`: Chromium against `supertracker --serve` (boot against the real host, save
  through a scripted Save chooser, reload, New, open through a scripted Open chooser, unimplemented
  commands, token and origin checks, clean quit, session and settings restored by a second process,
  and audio: the demo song played from the page on the null device, a note edited while playing).
- `tests/host_e2e/window_smoke.sh` under Xvfb: the GTK window with WebKitGTK loads the page from the
  custom scheme, the message channel works both ways, a menu shortcut reaches the page, a native
  context menu and the native file chooser open and are answered, Quit runs the handshake and exits 0.

Not exercised anywhere yet:

- **Audio on a real device**: `cpal` output streams (ALSA and CoreAudio), device loss and return,
  and the restart of the engine at a device's sample rate have only run against the null device and
  missing devices.
- **Everything macOS**: the AppKit window (transparent title bar, full-size content), `window.titleBar`
  dragging, the application menu and its standard items, `rfd` panels and the alert for a hung page,
  context menus through `show_context_menu_for_nsview`, `Event::Opened` (files opened from Finder),
  and whether a key the page handles also triggers its menu shortcut (in the Swift host WKWebView
  offered keys to the page first). The code type-checks for `aarch64-apple-darwin`; it has not been
  linked or run.
- The hung-page path of the quit handshake in a real window (covered in `st-host` tests only).
- Windows: `--serve` should work; the window is not implemented there.

Known differences from the Swift host:

- Quitting from the Dock or at logout ends the event loop without the handshake (tao gives no veto):
  the host then writes the recovery copy from the last project the page published.
- A double click on the top bar always zooms; the system setting "double-click a window's title bar
  to" is not consulted.
- The page reserves room for the traffic lights (`body.native .topbar`) under every host, also where
  the window has its own title bar and in a browser.
- No migration of old project layouts (`st-project` refuses them with its own message), no Maschine
  group (`.mxgrp`) import, no VST2.
- Audio: one mix model (every project is mixed as "standard" routing; a project without `routing` is
  the same mixer without buses). The delay and reverb returns are the engine's own DSP run as mixer
  returns, not Apple's delay and reverb units. Built-in effect state is base64 JSON
  (`SuperTracker.BuiltinFX`), not a binary property list: states saved by the Swift host are refused
  per effect and the effect starts from its defaults. Play does not wait for samples (they are
  loaded when the project is published). See `cargo doc -p st-audio` for the design and the seams.
- The self-test modes, the hidden UI script runner (`DevHarness.swift`) and the keyboard acceptance
  session of the Swift executable have no counterpart; the end-to-end tests above take their place
  for what is ported.

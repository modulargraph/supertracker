# SuperTracker architecture (Rust host)

This is the target architecture. It replaces the Swift/AppKit host, the C++ engine and the Objective-C++ plug-in
hosts described in [ARCHITECTURE.md](ARCHITECTURE.md), which stay in the tree only until the Rust host has taken
over each area. There is no compatibility layer: no Swift, no C++, no C API, no VST2, no migration of old project
formats, no "legacy" mix path. Platform code is Rust behind `cfg`, written against the platform's C/Objective-C APIs
only where no cross-platform crate exists (Audio Units on macOS).

## Shape

```
ui (web, unchanged in spirit)  <-- JSON messages -->  host process (Rust)  <-- shared memory + socket -->  plug-in containers (Rust)
```

- **The web interface** (`Resources/Web`, later `ui/`) owns the document: the open project lives in the page, with
  its undo history. It talks to the host only through the bridge below.
- **The host** is one Rust process: window and web view, audio and MIDI devices, the engine and mixer graph,
  files, the sound library, hardware controllers, and the AI control server.
- **Plug-ins never run in the host.** Every plug-in instance lives in a `supertracker-plugin-container` process
  (grouped by policy: one container for all, per manufacturer, per plug-in, or per instance). A container that
  crashes or hangs costs its own plug-ins a restart; the host keeps playing.

## Workspace

One cargo workspace (`rust/` for now, the repository root once the Swift tree is gone).

| Crate | Responsibility | May depend on |
|---|---|---|
| `tracker-engine` | Sequencer, sampler/synth voices, clock and timing map, built-in effects and drum models. Pure DSP and timing, no I/O. | std |
| `st-project` | The project document: typed model of the JSON the interface publishes, validation, project files and their media folder, autosave and recovery files, Standard MIDI File import/export. | serde |
| `st-samples` | Audio files: decoding to float PCM, resampling, sample edits (trim, fades, normalize, reverse, gain, time stretch), transient detection and slicing, wavetable files, waveform peaks, encoding for export. No devices, no engine. | |
| `st-audio` | The mixer graph around the engine (strips, inserts, sends, buses, sidechains, delay compensation, automation of mix parameters, meters), audio device I/O, offline rendering and export, recording, sample preview. | `tracker-engine`, `st-project`, `st-samples`, plug-in client |
| `st-plugin-*` (`rust/plugin-host`) | Shared-memory transport, host-side client and supervisor, the container binary and its format backends (CLAP, VST3, Audio Units on macOS). | libc |
| `st-library` | Sound library index and search, NKS metadata and presets, kits. | `st-project` |
| `st-midi` | MIDI device I/O and clock output. | |
| `st-mk3` | Maschine MK3 controller driver. | |
| `st-agent` | Local control socket and MCP server for AI clients. | |
| `st-host` | The application core: session state, the command table of the bridge, events, settings. Everything the interface can ask for is a command handler here that delegates to the crates above. No window code. | all of the above |
| `st-app` | The executable: window, web view and menus (tao/wry/muda), file dialogs, and a headless mode that serves the same bridge over a local WebSocket. | `st-host` |

Rules for every crate:

- **Tests run on Linux.** Each crate has unit and integration tests that run with `cargo test` on Linux without
  audio hardware or a display. Code that only exists on one platform is behind `cfg`, type-checked for
  `aarch64-apple-darwin` from Linux, and listed as such in the crate's docs until it has run on that platform.
- **Real-time code** (anything called from an audio callback) does not allocate, lock a mutex another thread can hold
  for long, perform I/O or panic. State crosses to it through lock-free queues or atomically published immutable
  snapshots, and is reclaimed on a non-real-time thread.
- **Errors** are `Result`s with messages a user can read; the bridge turns them into the reply's `error` string.
  No panics for bad input; `panic = "abort"` in release makes a panic a crash.
- **Dependencies** are few, maintained and permissively licensed (MIT/Apache-2.0/BSD/ISC/Zlib/MPL-2.0; the app is
  GPL-3.0-or-later).
- **No legacy.** If the old host carried a second code path for old projects or old plug-in formats, the Rust host
  has only the current one.

## The bridge

One protocol, two transports.

- Request (interface to host): `{"id": <number>, "command": "<name>", "payload": {...}}`
- Reply (host to interface): `{"id": <number>, "ok": true, "result": ...}` or `{"id": ..., "ok": false, "error": "<message>"}`.
  Every request gets exactly one reply.
- Event (host to interface, unsolicited): `{"type": "<name>", "payload": {...}}`

Transports: the web view's message channel inside the app; a WebSocket on 127.0.0.1 (random port, token in the URL)
when the host runs with `--serve`. The second one is how the interface is tested against the real host in an ordinary
browser, and what a remote control surface would use. The interface picks the transport in one module
(`host-transport.js`); nothing else in the page knows which is in use.

Command names keep the form `area.verb` the interface already uses (`project.save`, `transport.play`,
`sample.load`, `plugin.load`, `library.search`, ...). `st-host` registers each command with a typed payload
(`serde` structs, unknown fields ignored, missing optional fields defaulted) and a handler that returns a
serializable result. A command the host does not implement yet answers with an error that names it; nothing is
silently ignored.

## Audio

- The engine renders one stereo stream per track ("channel") plus the return bus; it knows nothing about plug-ins.
- `st-audio` builds an immutable graph from the project (track strip: instrument insert chain, track inserts,
  fader/pan, sends; group and return buses; master) with plug-in delay compensation, publishes it to the audio
  thread atomically, and renders it block by block: engine channel, then each insert (a built-in effect or a
  plug-in instance in a container, reached through the client's real-time `process` call), then mixing.
- The same graph renders offline (export, freeze, render selection, tests) with the containers' deadline disabled.
- Devices come from `cpal`; a null device (render to nowhere at a fixed rate) exists for tests and headless use.

## What "done" means for an area

An area is ported when its commands are implemented in `st-host`, its behaviour is covered by Rust tests, the
interface runs against it in a browser through `--serve`, and the Swift/C++ code it replaces is deleted in the same
change. `docs/RUST_PORT_STATUS.md` lists every command and area with its state.

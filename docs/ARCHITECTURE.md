# SuperTracker architecture

SuperTracker is a web interface on a Rust host. Everything native is Rust: the engine, the mixer, devices, files,
the library, controllers and plug-in hosting. There is one code path for each thing: one project layout, one mix
model, one way to host a plug-in. Platform code is Rust behind `cfg`, written against the platform's C/Objective-C
APIs only where no cross-platform crate exists (Audio Units on macOS).

[HOST.md](HOST.md) is the reference that goes with this design: how to run the host, every bridge command with its
state, events, settings, and what has and has not been exercised. [PLUGIN_CONTAINER.md](PLUGIN_CONTAINER.md)
describes the plug-in containers.

## Shape

```
ui (web)  <-- JSON messages -->  host process (Rust)  <-- shared memory + socket -->  plug-in containers (Rust)
```

- **The web interface** (`ui/`, plain scripts, no build step) owns the document: the open project lives in the page, with
  its undo history. It talks to the host only through the bridge below.
- **The host** is one Rust process: window and web view, audio and MIDI devices, the engine and mixer graph,
  files, the sound library, hardware controllers, and the AI control server.
- **Plug-ins never run in the host.** Every plug-in instance lives in a `supertracker-plugin-container` process
  (grouped by policy: one container for all, per manufacturer, per plug-in, or per instance). A container that
  crashes or hangs costs its own plug-ins a restart; the host keeps playing.

## Workspace

One cargo workspace at the repository root, with the crates in `crates/`. The plug-in container is a second
workspace in `crates/plugin-host` (its own dependencies and binaries), built into the same target folder.

| Crate | Responsibility | May depend on |
|---|---|---|
| `tracker-engine` | Sequencer, sampler/synth voices, built-in effects and drum models. Pure DSP and timing, no I/O. | std |
| `st-project` | The project document: typed model of the JSON the interface publishes, validation, project files and their media folder, autosave and recovery files, Standard MIDI File import/export. | serde |
| `st-samples` | Audio files: decoding to float PCM, resampling, sample edits (trim, fades, normalize, reverse, gain, time stretch), transient detection and slicing, wavetable files, waveform peaks, encoding for export. No devices, no engine. | |
| `st-audio` | The mixer graph around the engine (strips, inserts, sends, buses, sidechains, delay compensation, automation of mix parameters, meters), audio device I/O, offline rendering and export, recording, sample preview. | `tracker-engine`, `st-project`, `st-samples`, plug-in client |
| `st-plugin-*` (`crates/plugin-host`) | Shared-memory transport, host-side client and supervisor, the container binary and its format backends (CLAP, VST3, Audio Units on macOS). | libc |
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
- **No legacy.** A change of format or behaviour replaces the old one. There are no second code paths for older
  projects, kits or plug-in formats, and no migrations.

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

## Engine

`tracker-engine` is pure Rust with no dependencies. It holds what the host uses and nothing else: a project has
one tempo and one meter, and the engine renders one block after another, each bus once. Comments in the source say
where an operand type or an order of evaluation matters for bit-identical PCM; many of its tests compare recorded
PCM hashes.

- `tracker_engine::engine`: `Engine::new(sample_rate)` and its three handles (`ControlHalf` for the control thread,
  `RenderHalf` for the audio thread, the clonable `StatusHandle` for everyone). `engine/host.rs` states why they may
  cross threads. The audio thread calls `render_block` and reads the track buses and the return bus from `block()`,
  the events of the block from `pop_event` and, when it records, the song frame of the block from
  `block_song_frame`. [AUDIO_ENGINE.md](AUDIO_ENGINE.md) describes the behaviour.
- `tracker_engine::effects`: the built-in effect catalogue, `Effect` (an instance with lock-free parameter
  mailboxes; `EffectControls` for other threads) and the drum models.

Run its tests from the repository root:

```sh
cargo test -p tracker-engine              # debug build: overflow and bounds checks on
cargo test -p tracker-engine --release    # the build the recorded PCM hashes and CPU budgets refer to
cargo test -p tracker-engine --release --test engine_benchmark -- --ignored --nocapture   # one of the benchmarks
```

The benchmarks (`engine_benchmark`, `engine_dsp_benchmark`, `engine_render_budget`, `engine_many_tracks`,
`effects_builtin_perf`) are `#[ignore]`d tests that print timings; they are run one at a time as above.

The support modules of the integration tests in `crates/tracker-engine/tests/` install a counting allocator that
fails a test when a render call allocates or frees.

## Adding to the host

A feature the interface needs from the host is a command: a typed payload and a handler in `st-host`
(`crates/st-host/src/areas/`) that delegates to the crate that owns the behaviour, an entry in the command
catalogue (`crates/st-host/src/catalog_table.rs`, from which the tables in [HOST.md](HOST.md) are generated),
Rust tests for the behaviour, and, where the interface is involved, a check in the browser test
(`tests/host_e2e/run.js`). [TESTING.md](TESTING.md) lists the suites.

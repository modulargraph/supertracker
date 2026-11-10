# Feature status

What is built, how it was verified, and what has not been verified. Version 0.2 replaced the whole native side
(a Swift application with a C++ engine) by a Rust host; the interface is the same one. So the question for every
feature is twice: does the interface do it, and does the new host carry it.

## How features are verified

| Method | What it means | What it does not mean |
|---|---|---|
| **Interface tests** | `tests/*_tests.js` (156 files) load the interface's scripts into a simulated page and check keyboard handling, state changes, Undo and rendering. | No browser, no host, no audio. |
| **Engine tests** | `cargo test -p tracker-engine` renders the engine offline and measures the audio (note timing, loop windows, echo times and so on). The engine was translated from the 0.1 C++ engine and gave bit-identical output to it on x86-64 before that engine was removed. | Offline only. Nobody has listened. On arm64 the output can differ from x86-64 in the last bits (fused multiply-add); the arm64 build has never run. |
| **Host tests** | `cargo test --workspace` and the plug-in container's tests: every bridge command against the real crates, offline renders through the mixer graph, plug-ins in container processes (CLAP and VST3 test plug-ins), files, library, MIDI with in-memory ports. | No audio device, no MIDI device, no third-party plug-in. |
| **Browser test** | `tests/host_e2e/run.js`: the real page in Chromium against the real host over its local WebSocket. | Chromium on Linux, not the app's own web view on macOS; the null audio device. |

[TESTING.md](TESTING.md) has the commands.

## Not verified (applies across the app)

- **macOS at run time.** The host has only run on Linux. The Mac window and menus, Core Audio, CoreMIDI, Audio Unit
  hosting, plug-in editor windows and the container's real-time thread priority compile for Apple Silicon and have
  never been started. First-run checklists: [PLUGIN_CONTAINER.md](PLUGIN_CONTAINER.md).
- **A real audio device.** Every render so far went to a file, to memory or to the null device.
- **Real MIDI hardware**, including the timing of MIDI output against the audio output.
- **Third-party plug-ins.** Only the test plug-ins built in this repository have been loaded. Crash isolation is
  tested with those: a plug-in that crashes or hangs costs its container, the host keeps playing and offers Retry.
- **The Maschine MK3** (off unless turned on in Settings): never run with a controller.
- **Listening**, and a complete pass by a person at the keyboard.

## What the host does not do yet

[HOST.md](HOST.md) has the full command table (108 of 120 commands implemented, 4 more in part). Missing:

| Feature | State |
|---|---|
| Live line inputs routed into track strips (`audio.inputs`, `audio.inputs.configure`) | Not implemented. Recording an input to a file works. |
| Freeze by recording (for tracks whose sound comes from outside the computer) | Not implemented; offline freeze works. |
| Routing checks before a change is made (`routing.capabilities`, `routing.validate`, `routing.apply`) | Not implemented; the mixer is rebuilt from each published project and reports what it could not connect in `routing.status`. |
| Audio preview file for a saved kit (`audio.kitPreview`) | Not implemented; kits save without a preview. |
| Controllers other than pitch bend, mod wheel and sustain, and aftertouch, to plug-in and external instruments (`midi.expression`) | Not implemented. |
| Base patch capture for plug-in parameter locks (`plugin.captureBasePatch`) | Not implemented. |
| Screenshots for AI clients (`app.screenshot`) | Not implemented. |
| Windows | `--serve` should work; there is no window and no control socket. |

## Removed in 0.2

- VST2 hosting. NKS presets and Maschine groups that name a VST2 plug-in are still read: a preset loads into the
  vendor's VST2-compatible VST3 when that is installed; a group keeps the plug-in's state and says so.
- Opening projects, kits, built-in effect states and plug-in states saved by 0.1. Nothing is migrated.
- The second mixer ("legacy" routing). Every project uses the routed mixer.
- The in-app self-test modes and the hidden script runner. The host tests and the browser test take their place.

## Features of the interface

These are built in the interface and covered by interface tests. Where the host is involved, the last column says
how far the Rust host has been checked.

| Feature | Built | Host |
|---|---|---|
| Tracker patterns | Note, instrument, velocity, gate, two step-effect columns, up to eight note columns per track, selection, clipboard, transpose, pattern tools (interpolate, humanize, reverse, nudge, stretch), each one Undo step. | Engine tests for every step effect; a note edited while playing in the browser test. |
| Step effects | Probability, ratchet, microtiming, glide, reverse, offset, arpeggio, slice, mixer locks, plug-in parameter locks. | Engine tests; plug-in locks at their frames in host tests. |
| Playing | 15 scales, chord modes, 31 chord sets, note repeat and arpeggiator, from the computer keyboard and MIDI. | Live notes and recording acknowledgements in host tests with in-memory MIDI. |
| Instruments | Synth, sampler (1-shot, loops, slice, wavetable, granular), six drum models, multisample zones, filter envelope, two LFOs, plug-in and external MIDI instruments. | Engine tests per mode; plug-in instrument playing a pattern in host tests. |
| Sample editor | Zoom, markers, zero-crossing snap, trim, fades, normalize, reverse, gain, slicing (equal, beat, transient), time stretch; edits write new files. | `st-samples` tests; `sample.*` commands in host tests. |
| Built-in effects | 34 effects as inserts on tracks, instruments, buses and Master. | Engine tests per effect; inserts in the graph in host tests. |
| Mixer | Tracks, groups, returns, Master, sends, sidechains, sixteen inserts per channel, plug-in delay compensation, meters. | Offline renders against the mixer's rules; delay compensation following a plug-in's latency change. |
| Automation | Lanes per pattern and across the song, linear and step, Read/Touch/Latch/Write recording, MIDI CC learn. | Engine tests; mix and plug-in automation at their frames in host tests. |
| Song | Arranger with repeats, section loop, song scroll, audio clips and takes. | Engine tests for song order and loops; clip loading in host tests. |
| Browsers | Samples, NKS presets, kits and plug-ins with search, filters, favorites and previews. | Library through the bridge on generated files; a folder scanned and searched in the browser test. Not run on a real library. |
| Kits | Groups of tracks behind a group bus; save and load `.stkit`; Maschine group import. | `st-library` tests on generated archives. Not run on installed Native Instruments content. |
| Plug-ins | Audio Unit, VST3 and CLAP instruments and effects in container processes; parameters, state in the project, editors, eight-knob pages, crash recovery. | CLAP and VST3 test plug-ins end to end, including a crash while playing. Audio Units and all editors: compiled only. |
| Track freeze, bounce, render selection | Offline prints of a track, a stem or a selection. | Host tests compare frozen and live renders. |
| Recording | Master output or an input to WAV. | Host tests with generated input. No real input device. |
| Projects | JSON project with a media folder, atomic save, autosave and recovery, templates, missing-media repair, Undo. | `st-project` tests; save, open and recovery in the browser test. |
| Export | WAV (16/24-bit, float) at 44.1/48/96 kHz: song, pattern, stems; Standard MIDI File import and export. | `st-samples` and `st-project` tests; export through the bridge. |
| MIDI | Input with channel filter and sustain, external MIDI instruments, MIDI clock out. | In-memory ports only. |
| AI control | Local MCP server (`supertracker --mcp`) and `--ctl`; can be turned off in Settings. | Browser test: an MCP session reads the song and writes steps that appear in the page. |

## Outstanding product requirements

- Punch-in/out, loop recording with take splitting, automatic take alignment.
- Tempo and meter maps in the interface (the engine's clock has them; projects carry one tempo and meter).
- Multi-output instruments.
- Warp markers, realtime stretch, round robin, zone crossfades, a modulation matrix.
- MPE, full controller recording, external clock sync.
- Maschine import of group mix levels, auxiliary routing, modulation, patterns and macro pages.

Capacities: any number of tracks and buses (1024 sanity bound each), 64 instruments, 256 patterns of up to 256
rows, 256 audio clips, 128 zones/slices per instrument, eight notes per step chord, sixteen inserts per channel,
128 custom sends/sidechain connections.

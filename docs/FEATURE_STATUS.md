# Feature status

What is built, how it was verified, and what has not been verified. Version 0.2 replaced the whole native side
(a Swift application with a C++ engine) by a Rust host; the interface is the same one. So the question for every
feature is twice: does the interface do it, and does the new host carry it.

## How features are verified

| Method | What it means | What it does not mean |
|---|---|---|
| **Interface tests** | `tests/*_tests.js` (155 files) load the interface's scripts into a simulated page and check keyboard handling, state changes, Undo and rendering. | No browser, no host, no audio. |
| **Engine tests** | `cargo test -p tracker-engine` renders the engine offline and measures the audio (note timing, loop windows, echo times and so on). The engine was translated from the 0.1 C++ engine and gave bit-identical output to it on x86-64 before that engine was removed. Since then its own mixer was removed too: the recorded audio hashes are of the per-track buses the host reads, recorded while the old mixed-output hashes still passed on the same engine. | Offline only. Nobody has listened. On arm64 the output can differ from x86-64 in the last bits (fused multiply-add); the arm64 build has never run. |
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

Every command the interface sends is implemented ([HOST.md](HOST.md) has the table). What is still missing:

| Feature | State |
|---|---|
| Control changes to CLAP plug-ins that accept only CLAP note events | Dropped: CLAP has no event for them. Pressure and pitch bend arrive as note expressions. |
| Windows | `--serve` should work; there is no window and no control socket. |

## Added after the first Rust host

Each of these was tested on Linux with the silent test devices, in-memory MIDI ports and the test plug-ins only.

| Feature | How it was verified |
|---|---|
| Live line inputs into track strips (`tracks[i].audioInput`; state reported in `audio.devices`) | Mixer-graph and bridge tests with the test tone: through the track's inserts, fader and meters, with delay compensation; a check in the browser test. No real input device has been opened. |
| An input on a different device than the output (two clocks, any pair of sample rates) is resampled onto the output's clock, for line inputs, monitoring, input takes and Freeze by recording; one device for both passes audio through unchanged | Simulated clocks only: ten simulated minutes at ±200 ppm and 44.1 into 48 kHz with jittered callbacks, no gap or skip, tone within about 15 ppm, delay within 3 frames of the reported latency; stalls and bursts recover and are counted. Costs latency (27 ms instead of 11 ms at 512-frame blocks). How regular real devices' callbacks are is unverified. |
| Freeze by recording, for tracks whose sound comes from outside the computer (song and pattern prints) | A recorded print equals the offline print of the same track frame for frame in the tests. Real hardware returns and their latency are unverified. |
| Kit previews: a kit is saved with `Preview.wav` (its group bus in the selected pattern, up to 10 s or 8 bars) | Bridge test on generated kits, including a built-in effect and a plug-in in a container. Nobody has listened to one. |
| Controllers, channel and poly pressure and pitch bend to the instrument a track plays (external device, plug-in, engine), from MIDI input and the interface | In-memory MIDI ports; CLAP and VST3 test plug-ins, frame-exact. Audio Units: type-checked only. |
| The sustain, sostenuto and soft pedals passed on to plug-in and external instruments (the host holds notes for built-in instruments and for plug-ins the pedal cannot reach); a note recorded under the pedal is as long as it sounded | Bridge tests with in-memory ports and the CLAP and VST3 test plug-ins, including a keyboard moved to another track, Stop, panic, a replaced or restored plug-in and several keyboards. No real pedal, synth or piano plug-in. |
| Live input keeps its order and is not lost: notes, pedals, controllers, pressure and bend reach an instrument in the order played, also within one audio block; what was played into a block a plug-in misses arrives with the next block (after two seconds away the plug-in is silenced cleanly instead); a key held while its track is moved or removed is released before the change | Container tests that make a plug-in miss blocks on purpose; bridge tests driving the audio blocks by hand, for CLAP, VST3, the sampler and an external device. A held note ends when its track moves; it does not follow the track. |
| Plug-in parameters return to the value you set after a locked row, and at Play and Seek, for every plug-in format | Tests with CLAP and VST3 test plug-ins. A project saved while the song plays stores what lanes and locks hold at that moment. |
| Screenshots of the window for AI clients (`screenshot` tool, `--ctl app.screenshot`) | Linux: the real window under a virtual display. macOS: type-checked only. `--serve` has no window and says so. |
| Export dither (TPDF by default for 16- and 24-bit, or none), chosen in the export dialog | `st-samples` tests; interface tests of the dialog. |
| Shortcut labels as Ctrl / Alt / Shift away from a Mac | Checked in Chromium on Linux. |

## Removed in 0.2

- VST2 hosting. NKS presets and Maschine groups that name a VST2 plug-in are still read: a preset loads into the
  vendor's VST2-compatible VST3 when that is installed; a group keeps the plug-in's state and says so.
- Opening projects, kits, built-in effect states and plug-in states saved by 0.1 or by earlier builds of 0.2 (project format 2, kit package format 3; any other number is refused with one message). Nothing is migrated.
- The second mixer ("legacy" mix) and the dialog that switched a song to standard routing. Every project uses the routed mixer.
- Tempo and meter maps: the engine's clock module and the interface's reading of `project.timing`. A project has one tempo and one meter.
- Engine features the host never called: its own mixer (faders, mute/solo, delay and reverb sends; the host mixes), plug-in automation lanes and track/slot step locks, the frozen-song provider, the unit range cache, the timestamped bus history, transport checkpoints, its own meters, zone regions.
- Base patch capture (the host keeps every plug-in parameter's base value itself), "Bounce track" (Render stem remains), the "Routing needs repair" state (a project the mixer cannot render is refused when published), and the mapping of former factory chord-set IDs.
- The in-app self-test modes and the hidden script runner. The host tests and the browser test take their place.
- Looser spellings of project data: the instrument type `au` (it is `plugin`), a plug-in entry without an `instanceID` or a `format`, a choke group on the instrument (it is the track's), a recording input saved by device name only, and the earlier shape of the kit browser's saved settings. The host refuses the first four; the interface reads a loaded project's parts without checking for them ([HOST.md](HOST.md) lists what every project has).

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
| Mixer | Tracks, groups, returns, Master, sends, sidechains, sixteen inserts per channel, plug-in delay compensation, meters. | Offline renders against the mixer's rules; delay compensation following a plug-in's latency change; in the browser test a return bus and a send made in the mixer carry signal. |
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
- Tempo and meter maps (a project has one tempo and one meter; nothing in the engine or the interface supports more).
- Multi-output instruments.
- Warp markers, realtime stretch, round robin, zone crossfades, a modulation matrix.
- MPE, recording of controllers, pressure and the pedal itself (recorded notes get the length they sounded), external clock sync.
- Maschine import of group mix levels, auxiliary routing, modulation, patterns and macro pages.

Capacities: any number of tracks and buses (1024 sanity bound each), 64 instruments, 256 patterns of up to 256
rows, 256 audio clips, 128 zones/slices per instrument, eight notes per step chord, sixteen inserts per channel,
128 custom sends/sidechain connections.

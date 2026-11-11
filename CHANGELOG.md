# Release notes

## 0.2.0 — Rust host (in development)

The native side of SuperTracker was rewritten in Rust. The Swift application, the C++ engine and the
Objective-C++ plug-in hosts are gone; the interface is the same web interface, now in `ui/`.

**Not yet run on a Mac.** Everything below was built and tested on Linux without audio hardware. The macOS code
(window, Core Audio, CoreMIDI, Audio Units) compiles for Apple Silicon and has never been started. Use 0.1 for
making music until this has had its first run on a Mac ([checklists](docs/PLUGIN_CONTAINER.md)).

### New

- **Plug-ins run in separate processes.** Every Audio Unit, VST3 and CLAP plug-in lives in a
  `supertracker-plugin-container` process: one per plug-in by default, or one for all, one per manufacturer or one
  per instance (Settings section `plugins`). A plug-in that crashes or hangs costs its container; the song keeps
  playing and the plug-in can be restored with its state.
- **CLAP plug-ins.**
- **One host for every platform.** The same Rust host runs the Mac window and a Linux window, and
  `supertracker --serve` serves the interface to a browser over a token-protected local WebSocket.
- **Audio engine in Rust**, translated from the 0.1 engine and bit-identical to it on x86-64 at the time of the
  switch.

- **Exports to 16- and 24-bit WAV are dithered (TPDF) by default**; the export dialog has a Dither choice.
- **Live line inputs into tracks**, with latency compensation, stored with the track in the project.
- **Freeze by recording** for tracks whose sound comes from outside the computer.
- **An input on a different device than the output stays in sync.** It is resampled continuously onto the output's
  clock (also across different sample rates), for line inputs, monitoring, recording and Freeze by recording. One
  interface for both passes audio through unchanged. Recordings of an input are written at the project's sample
  rate on the output's clock.
- **Kits are saved with an audio preview.**
- **Controllers, pressure and pitch bend** reach the plug-in or external instrument a track plays.
- **The sustain pedal belongs to the instrument.** Plug-in and external MIDI instruments receive the pedal with its
  value (also sostenuto and soft) and sustain by themselves; built-in instruments are held by the host. Notes
  recorded under the pedal get the length they sounded.
- **Plug-in parameters return to the value you set** after a locked row and at Play or Seek, for every plug-in
  format (this replaces base patch capture).
- **AI clients can take screenshots of the window** (`screenshot` tool, `supertracker --ctl app.screenshot`).
- **Shortcuts are shown as Ctrl / Alt / Shift away from a Mac.**

### Removed, without migration

- **Projects and kits saved by 0.1, or by earlier builds of 0.2, do not open.** The project format is version 2 and
  the kit package format version 3; a file with any other number (projects, templates, recovery copies, kits) is
  refused with one message. Nothing is converted.
- **Plug-in and built-in effect states saved by 0.1 are not read.** Plug-ins are identified as `clap:…`, `vst3:…`
  and `au:type:subtype:manufacturer`.
- **VST2 hosting.** NKS presets made for a VST2 plug-in load into the vendor's VST2-compatible VST3 when installed.
- The second ("legacy") mix model: every song uses the routed mixer (buses, sends, sidechains, Master inserts).
- **Tempo and meter maps.** A project has one tempo and one meter; the engine's clock module is gone.
- Engine features the host never used: the engine's own mixer and sends (it renders one bus per track and the host
  mixes), plug-in automation lanes, the frozen-song provider, unit range cache, timestamped bus history, transport
  checkpoints and meters.
- "Bounce track" (Render stem remains), the "Routing needs repair" state, base patch capture, the `audio.inputs`
  commands (a track's input is part of the project), former factory chord-set IDs.
- The in-app self-test modes and the keyboard QA build.
- **Looser project shapes.** A project must have a routing table and a `routingID` on every track; plug-ins and
  their automation are addressed by mixer channel (`track:<id>`, `bus:<id>`, `master`), never by track number;
  `routing.version`, `routing.mixModel` and the freeze record's `mixMode` are gone. Frozen tracks read as stale once.
  A sidechain's destination is the insert itself (one stereo key input per insert); built-in effects are named
  `stfx:<id>` only; a plug-in instrument's descriptor lives on the tracks that play it, not also on the instrument.
- The page's second way of recording notes (timed by the wall clock). Every recorded note is placed by the engine's
  acknowledgement.

### Fixed along the way

- Automation recorded for an instrument plug-in on a track now plays (it was silently dropped).
- A plug-in loaded before the start-up scan had listed it is named properly.
- Offline Track Freeze refuses a song longer than a print can be before rendering it.

### Known gaps

Nothing has been run on a Mac, with a real audio or MIDI device, or with a
third-party plug-in. [docs/FEATURE_STATUS.md](docs/FEATURE_STATUS.md) and [docs/HOST.md](docs/HOST.md) have the details.

### Build

Rust (stable) and, on Linux, `libgtk-3-dev libwebkit2gtk-4.1-dev libasound2-dev pkg-config`.
`./scripts/build.sh` builds `build/SuperTracker.app` on macOS and `build/supertracker/` elsewhere;
`./scripts/test.sh` runs the tests.

## 0.1.0 — first public preview

The notes below describe 0.1 as it was: a Swift and C++ application for macOS. Its build steps, scripts and file
paths no longer exist in this tree.

SuperTracker is a keyboard-first music tracker and studio for macOS: a Polyend Tracker-style pattern workflow with Maschine-style sound browsing, chords and performance tools, plus mixing, automation, recording and export. It is a native AppKit app with a bundled WebKit interface and an original C++ audio engine, and it hosts Audio Unit, VST3 and VST2 plug-ins.

This is the first public source release. Expect rough edges, and keep backups of projects you care about.

### Platform

- **macOS only.** macOS 14 Sonoma or later.
- **Apple Silicon (arm64).** Intel Macs are untested and no Intel or universal build is provided. Intel-only plug-ins cannot load.
- No Windows or Linux version.
- Built from source and ad-hoc signed; not notarized.
- Licensed under the GNU General Public License v3.0 or later (see `LICENSE`).

### Highlights

- **Tracker patterns from the keyboard:** note, instrument, velocity, two step-effect columns and an optional gate per row; up to 256 patterns of 256 rows; step entry and live recording.
- **Pattern navigation:** an instrument list over a pattern matrix (all patterns, or the song's blocks with basic song editing), *Song scroll* to run the grid through the song in order, and an automation panel under the grid whose rows link to the grid and whose song timeline lets you draw across pattern boundaries.
- **Step effects:** probability, ratchet, microtiming, glide, reverse, sample offset, arpeggio, slice selection, mixer locks and plug-in parameter locks. Pattern tools (interpolate, humanize, reverse, nudge, stretch) are single Undo steps.
- **Playing:** 15 scales, chord modes, 31 chord sets (16 harmony and 15 style sets), note repeat and an arpeggiator, also from a MIDI keyboard.
- **Sound design:** built-in synth and sampler (1-shot, loops, slice, wavetable, granular), six DrumSynth models, 34 built-in effects, a keyboard sample editor with slicing, multisample zones and time stretch.
- **Browsers:** samples, NKS presets, kits and plug-ins (AU, VST3, VST2) with search, filters, previews, favorites and tags. Standard locations are found on their own (Apple Loops, `~/Music/Loopcloud`, Native Instruments content installed by Native Access, `~/Documents/Native Instruments`); any other folder can be added.
- **Kits:** groups of ordinary tracks routed through a group bus; save your own kits and import supported Maschine expansion groups.
- **Plug-in control:** eight-knob pages from NKS data or generated for any plug-in.
- **Arrange, mix and automate:** song arranger with audio clips and takes, mixer with groups, returns, sends, sidechains and up to sixteen inserts per channel, automation lanes with Read/Touch/Latch/Write recording and MIDI CC learn, plug-in delay compensation, track freeze.
- **Finish:** autosave and recovery, WAV export of song, pattern or stems, Standard MIDI file import/export.
- **AI control (optional):** a local MCP server lets Claude and other MCP clients read and edit the open song; it can be turned off in Settings.

### In development

- **Native Instruments Maschine MK3 controller support is in development and off by default.** Turn it on in Settings → *Maschine MK3 (in development)*. It has had limited testing with a physical controller; taking over the screens pauses Native Instruments' background agents until you release the controller or quit.

### Known limitations

- Not yet tested with real MIDI hardware, audio-interface recording, sustained live playback or by ear for the newest features; verification so far is automated tests and offline renders.
- Plug-ins run in the app's process: a crashing plug-in can take the app down. Suspect plug-ins are disabled on the next launch, but there is no crash isolation.
- Only a few vendor plug-ins have been exercised end to end.
- Maschine kit import is partial: patterns, modulation, group level and some routing are not translated.
- Track freeze covers standard routing only.
- One tempo and meter per project; internal processing at 48 kHz; no punch-in or loop recording; no multi-output instruments; no MPE.

See [docs/FEATURE_STATUS.md](docs/FEATURE_STATUS.md) for what each feature was verified with.

### Build and install

Requirements: an Apple Silicon Mac with macOS 14 or later and Apple's command-line developer tools (`xcode-select --install`) or Xcode.

```sh
git clone --recurse-submodules <repository-url> supertracker && cd supertracker
git submodule update --init --recursive    # the pinned Steinberg VST3 SDK
./scripts/build.sh release build/SuperTracker.app
open build/SuperTracker.app
```

`./scripts/install.sh` builds a release app and installs it as `/Applications/SuperTracker.app` (an existing copy is kept as `build/SuperTracker-Applications-backup.zip`). `./scripts/test.sh` runs the engine, storage, DSP and UI tests.

### Changes since the private builds

- Maschine MK3 support is opt-in (Settings) and labelled as in development.
- Library, NKS and kit browsers no longer assume the author's drive and folders; they use standard locations and folders you add.
- The chord sets imported from the Maschine 3 software were replaced with 16 original harmony sets. Projects that used one open on the harmony set in the same position.
- The private hardware-editor project template (Minilogue XD, Circuit Mono Station and Summit editor plug-ins) and its test tooling were removed.
- Test fixtures use neutral device names and paths; tests that need installed content (Maschine expansions, a DX7 V NKS preset) take their location from `SUPERTRACKER_NI_CONTENT_DIR`, `SUPERTRACKER_1988_KIT` and `SUPERTRACKER_DX7_NKS_PRESET`.

### Fixes from a full code review

The web interface and engine changes are covered by the automated tests; the Swift changes were reviewed but not compiled on the review machine, so run `scripts/test.sh --native` on a Mac before relying on them.

- **Keyboard:** the pattern list and the automation panel under the grid no longer swallow F6, Tab and the workspace keys; Enter in a rename prompt no longer reopens it; Escape in Tempo and Pattern length restores the value; dialogs open on their default button.
- **Editing:** stale selections after New/Open, a new pattern or track, Stop and song edits; moving a track past the edge; clips in the audio timeline can be left with the arrow keys; edits that change nothing no longer add Undo steps; an emptied number field is ignored instead of read as 0; a refused edit on a frozen track keeps Redo.
- **Layout:** the mixer's Live input panel and strip height in a new project, long names in every workspace, dialogs taller than the window, the key reference at the minimum window size, the Recording page header.
- **Sound:** the tempo-synced send delay could read past its buffer; automating sample Start/End on a held note; a note swallowed after Seek or Panic; ratchets across a live tempo change.
- **Sampler and browsers:** live playheads after switching pages, a stuck marker drag, background refreshes that threw the Kits/NKS selection back, your own control pages on built-in effects.
- **AI control and MK3:** an NKS effect load could replace an existing effect; automation targets and step edits are validated; slow turns of stepped MK3 encoders; pattern-end overruns from pads.
- **Projects and hosting:** saving a kit over itself broke its sample paths; old kit projects with out-of-range numbers or lane routing; plug-ins reporting an infinite tail made saves fail; audio restarting on every device change with separate input and output devices; text pasted or loaded from a file could add markup to the grid.

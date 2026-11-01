# Release notes

## 0.1.0 — first public preview (draft)

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

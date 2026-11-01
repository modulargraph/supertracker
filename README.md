# SuperTracker

SuperTracker is a keyboard-first music tracker and studio for macOS. It combines a Polyend Tracker-style pattern workflow with Maschine-style sound browsing, chords and performance tools, plus the mixing, automation, recording and export features you expect from a DAW.

It is a native app: an AppKit window with a bundled WebKit interface, and an original C++ audio engine. It hosts Audio Unit, VST3 and VST2 instruments and effects. It does not use JUCE or Electron.

This is active development software. It runs on **macOS only** (Apple Silicon), is built from source and ad-hoc signed (not notarized), and some workflows have not yet been tested on real hardware (see [Limitations](#limitations)).

## Release 0.1 (first public preview)

SuperTracker 0.1 is the first public source release. It is a preview for people who are comfortable building a Mac app from source and who expect rough edges. What's in it, what's known not to work yet and how to build it are in the [release notes](CHANGELOG.md).

- **Platform:** macOS 14 Sonoma or later, on Apple Silicon (arm64). Intel Macs are untested and no Intel or universal build is provided. There is no Windows or Linux version.
- **Maschine MK3:** controller support is **in development** and off by default. Turn it on in Settings if you want to try it (see [Maschine MK3](#maschine-mk3-in-development)).
- **Your libraries:** the browsers find standard locations on their own (Apple Loops, `~/Music/Loopcloud`, Native Instruments content installed by Native Access, `~/Documents/Native Instruments`) and you can add any other folder, such as a sample drive.
- **Feedback:** bug reports with steps to reproduce are very welcome. Please include your macOS version, Mac model and the plug-ins involved.

## What you can do

**Write patterns from the keyboard**
- Tracker grid with note, instrument, velocity, two step-effect columns and an optional gate per row (notes play until the next note, OFF or CUT, like a classic tracker); any number of tracks, 256 patterns of up to 256 rows.
- Play notes from the computer keyboard (two chromatic octaves), with octave, velocity and edit-step control. Record while stopped (step entry) or during playback (note lengths are captured).
- Step effects: probability, ratchet, microtiming, glide, reverse, sample offset, arpeggio, slice selection, mixer locks and exact plug-in parameter locks.
- Pattern tools: interpolate velocity or effect ramps, humanize, reverse, nudge, stretch/compress, double/halve pattern length, quick step effects and clear effects. Each one is a single Undo step.
- Track mute/solo from the grid, and a pattern loop range for working on a few rows.

**Play musically**
- Scale mode with 15 scales: snap to key, or play scale degrees (one note in key per key).
- Chords: fixed chord shapes, or chords built from the current key (triads, 7ths, 9ths, sus4, open voicings).
- 31 chord sets (16 harmony sets for keys and modes, 15 style sets): each key plays the next chord. They also work from a MIDI keyboard.
- Note repeat and an arpeggiator (up, down, up-down, random, as played) at 1/4 to 1/32 rates, including triplets.

**Sound design**
- Built-in synth and sampler. Sampler play modes: 1-shot, forward loop, backward loop, ping-pong, slice, wavetable and granular.
- Six native DrumSynth models and 34 built-in studio effects, including Ice, modulation, dynamics, granular delays and performance effects. These are independent implementations with documented approximation limits.
- Instrument page in Polyend style: grouped controls (play mode and loop, volume/pan/tune/fine, filter, amp envelope, drive/bit depth, filter envelope and two LFOs) with readable units and arrow-key editing.
- Keyboard sample editor: zoomable waveform, markers with zero-crossing snap, audition, and trim, fades, normalize, reverse and gain. Edits always write a new file. Includes slicing (equal, beat grid, transient detection) and a "slices to pattern" command.
- Multisample zones, offline time stretch, per-track mono/stereo line inputs and sample recording of hardware synths. See [hardware inputs and MIDI output](docs/HARDWARE_INPUT_OUTPUT.md).

**Browse sounds**
- Sample browser over local folders, Native Instruments content and Loopcloud libraries, with text search, filters, previews, favorites and editable tags.
- NKS browser over your installed NKS catalogue. Filter by product, bank, type and character, preview presets, then load instruments onto a track or NKS effects into insert slots.
- Plug-in browser for AU, VST3 and VST2. Plug-ins are scanned in the background after launch.

**Build and play kits**
- A kit is a group of ordinary tracks, one per pad, each with its own source, inserts and mixer channel. The tracks are routed automatically through the kit's group bus. Collapse a group to one overview column, or expand it to edit its tracks like any other. **Pad keys** play and enter the pads from the computer keyboard. Older kit-lane projects convert on open.
- Kit browser with expansion/source/tag filters, text search, favorites and group previews. Save homemade kits with collected samples and a generated preview; import supported Maschine expansion groups with visible compatibility notes.
- With the Maschine MK3 (in development, off by default), group buttons select kits and pads play their tracks, with Sound / Group / Master editing and insert-chain selection. See the [kit guide](docs/KITS.md).

**Control plug-ins**
- Eight-knob control pages: NKS pages for NKS presets, and generated pages for any other plug-in, with pinning, assignment and user pages.
- Native VST2 hosting for arm64 builds, including shell plug-ins. VST2-based NKS presets load into the native VST2 plug-in, or into the vendor's VST2-compatible VST3 release when the VST2 is Intel-only.

**Arrange, mix and automate**
- Song arranger: pattern blocks with repeat counts, colours, play from block, a looping section and follow-playhead, all editable from the keyboard. Audio clips play alongside patterns, with takes and comping.
- Mixer with groups, returns, pre/post sends, sidechains and up to sixteen inserts per channel. Built-in send delay (tempo-synced or free, with tone control) and reverb (with pre-delay), edited in one keyboard dialog.
- Automation lanes per pattern, a searchable "automate parameter" picker that lists automated parameters first, and live automation recording in Read, Touch, Latch and Write modes, with parameter arming and MIDI CC learn.
- Plug-in delay compensation.

**Finish**
- Save/open with autosave, recovery and Undo. Referenced media is collected beside the project.
- WAV export of the song, the active pattern or stems at 44.1/48/96 kHz (16-bit, 24-bit or 32-bit float). Standard MIDI file import and export. Track bounce and stem render.

**Feels like a Mac app**
- Native menus, native right-click menus (also opened with Shift+F10 or the Menu key), and no accidental web-style text selection.
- Every main action has a key, a command palette entry (`Cmd+K`) and an entry in the keyboard reference (`?`).

## Build and run

Requirements: an Apple Silicon Mac, macOS 14 or later, Apple's command-line developer tools (Swift 5.9+ and Clang: `xcode-select --install`, or Xcode), and the Steinberg VST3 SDK checked out at `Vendor/vst3sdk` (a git submodule).

```sh
git clone --recurse-submodules <repository-url> supertracker && cd supertracker
git submodule update --init --recursive                  # fetch the pinned VST3 SDK and its dependencies
./scripts/build.sh                                       # release build to build/SuperTracker.app
./scripts/build.sh release build/SourcePreview/SuperTracker.app   # build to a separate location
./scripts/install.sh                                     # release build installed as /Applications/SuperTracker.app
open build/SourcePreview/SuperTracker.app
```

`./scripts/build.sh [debug|release] [path/to/App.app]` compiles the Swift/C++ package, copies the web interface into the app bundle and ad-hoc signs it. `./scripts/run.sh` makes a debug build at `build/SuperTracker.app` and opens it.

Builds are incremental: every configuration reuses the shared `.build` folder, and the app is split into a few Swift modules (`SuperTrackerCore`, `SuperTrackerLibrary`, `SuperTrackerMK3`, `SuperTrackerAgent` and the `SuperTracker` app) so an edit only recompiles the module it touches. Release builds leave out the in-app self-tests; debug builds and `./scripts/build.sh selftest` (optimised, self-tests included) keep them. See [docs/TESTING.md](docs/TESTING.md#builds).

`./scripts/build.sh` with no arguments also refreshes `/Applications/SuperTracker.app` if you installed it there before with `./scripts/install.sh`; build to another path (second argument) to leave the installed copy alone.

All builds share the same recovery storage, so save and quit any running copy before you open another build. The default `build/SuperTracker.app` path may already contain an older build you want to keep; build to a separate path if so.

Press `Cmd+N` for a new project, or `Cmd+K` → **Load Midnight transmission demo** for the included demo song. Press `?` for the keyboard reference and `Cmd+K` to search every command.

To enter notes, select a pattern lane and click **Arm & enter notes** (or press `Cmd+R`), then play `Z X C V…` on the grid. **Keys on** by itself previews the instrument without writing notes. `Space` plays from the current position; `Shift+Space` plays from the pattern or song start. Drag an occupied selected cell to move it, or hold Option while dragging to copy. Loading a plug-in while a sample/synth is selected adds a new lane and keeps your existing sample lanes.

## Maschine MK3 (in development)

SuperTracker can drive a Native Instruments Maschine MK3 directly over USB: pads, buttons, encoders, LEDs and both screens follow the tracker. This support is **in development**: it is off by default and has had limited testing with a physical controller. To try it, open Settings (`Cmd+,`) and tick **Enable Maschine MK3 controller support (experimental)** under *Maschine MK3 (in development)*. Pads and buttons then work alongside Native Instruments software; **Maschine MK3 (in development): take over screens…** in the command palette pauses NI's background agents to draw the screens, and **release controller** hands the controller back. Turning the setting off releases the controller. Details: [docs/MASCHINE_MK3.md](docs/MASCHINE_MK3.md).

Importing Maschine expansion groups as kits and the chord sets do not need the controller and are available to everyone.

## Tests

```sh
./scripts/test.sh            # engine (C++), storage, DSP, MIDI-file, library and UI tests (node)
./scripts/test.sh --native   # also builds the app and renders real Audio Units through the native graph
```

Debug builds of the app also have built-in self-tests for installed plug-ins (`--vst2-self-test`, `--vst3-self-test`), and a development harness that drives the real interface from a script. See [docs/FEATURE_STATUS.md](docs/FEATURE_STATUS.md) for what each feature has been verified with.

## Limitations

These are honest current limits, not a complete list.

- **macOS only, Apple Silicon only.** No Windows or Linux version; Intel Macs are untested. The app is ad-hoc signed and not notarized.
- **Maschine MK3 support is in development** and off by default (see above). [docs/FEATURE_STATUS.md](docs/FEATURE_STATUS.md) has the details.

- **Not tested with real hardware or by ear.** Features are verified by automated tests, offline renders and hidden in-app runs. Real MIDI controllers, audio-interface recording, sustained live playback and listening tests have not been done for the newest features.
- **Intel-only plug-ins cannot load** (for example Super 8). SuperTracker runs natively on Apple Silicon and does not host x86 plug-ins. An NKS preset for an Intel-only VST2 only loads if the vendor ships a compatible VST3.
- **Plug-ins run in the app's process.** A crashing plug-in can take the app down. Plug-in recovery keeps suspect plug-ins disabled on the next launch, but this is not crash isolation.
- **Vendor compatibility is narrow.** Only a few installed products (for example Massive X and Arturia DX7 V) have been exercised end to end.
- **Maschine kit import is partial.** Imported effects and DrumSynths use original SuperTracker processors. Patterns, modulation, Group level and source/auxiliary routing are retained in the source archive but are not fully translated. Import notes identify unsupported controls and dependencies; this is not a Maschine project player.
- **Track freeze covers standard routing only.** Freeze/Unfreeze prints a track with its inserts; some routing shapes are refused (see [Track Freeze](docs/TRACK_FREEZE_UX.md)).
- **Sample browser on real libraries is in progress.** Indexing works; previewing, loading and favorites across real libraries are still being verified.
- One project tempo and meter (no tempo maps); internal processing at 48 kHz; no punch-in or loop recording; no multi-output instruments; no MPE.

## More documentation

Using the app:
- [User guide](docs/USER_GUIDE.md): every workspace and its keyboard shortcuts.
- [Feature status](docs/FEATURE_STATUS.md): what is built, how it was verified, and the outstanding work.
- [Release notes](CHANGELOG.md).
- [Kits](docs/KITS.md), [kit library](docs/KIT_LIBRARY.md), [Maschine group import](docs/MASCHINE_GROUP_IMPORT.md), [Maschine MK3 controller (in development)](docs/MASCHINE_MK3.md).
- [Automation](docs/AUTOMATION.md), [audio takes](docs/AUDIO_TAKES.md), [track freeze](docs/TRACK_FREEZE_UX.md), [track duplication](docs/TRACK_DUPLICATION.md), [MIDI files](docs/MIDI_FILES.md), [media relinking](docs/MEDIA_RELINKING.md), [project templates](docs/PROJECT_TEMPLATES.md), [library metadata](docs/LIBRARY_METADATA.md).
- [Hardware inputs and MIDI output](docs/HARDWARE_INPUT_OUTPUT.md), [built-in effects](docs/BUILTIN_EFFECTS.md), [AI control (MCP)](docs/MCP.md).

Development:
- [Running the tests](docs/TESTING.md) and [keyboard QA builds](docs/KEYBOARD_QA.md).
- [Architecture](docs/ARCHITECTURE.md), [audio engine](docs/AUDIO_ENGINE.md), [VST3 host](docs/VST3_HOST.md), [DSP performance](docs/DSP_PERFORMANCE.md), [workflow repairs](docs/REPAIR_NOTES.md).
- Design: [design brief](docs/DESIGN.md), [reference research](docs/REFERENCE.md), [pattern layout](docs/PATTERN_LAYOUT.md), [instrument layout](docs/INSTRUMENT_LAYOUT.md), [MK3 design study](docs/MK3_DESIGN_STUDY.md), [kits as track groups](docs/KIT_GROUPS_DESIGN.md).
- Engine internals: [tracker clock](docs/TRACKER_CLOCK.md), [clock transport context](docs/CLOCK_TRANSPORT_CONTEXT.md), [input acknowledgement](docs/INPUT_CAPTURE_HOST.md), [input-edge recording](docs/INPUT_EDGE_RECORDING.md).
- Engine capabilities with no app feature on top (native tests only): [stepped tempo/meter map](docs/ENGINE_TIMING.md), [frozen-song extent](docs/AUDIO_FREEZE_EXTENT.md) and [frozen-stage timeline](docs/AUDIO_FREEZE_TIMELINE.md). The app plays one tempo and meter per project.
- Maschine MK3 protocol notes in [docs/hardware](docs/hardware/README.md); parameter-mapping research in `docs/research/`.
- [Cleanup audit](docs/CLEANUP_AUDIT.md) (25 October 2026).
- [Harbor Circuit](Examples/Harbor%20Circuit/README.md): an original example project and export.

## License

SuperTracker is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License or (at your option) any later version. It is distributed WITHOUT ANY WARRANTY; see [LICENSE](LICENSE) for details.

The Steinberg VST3 SDK in `Vendor/vst3sdk` (a git submodule) is under its own MIT license. The Maschine MK3 protocol notes in `docs/hardware` credit their MIT-licensed source.

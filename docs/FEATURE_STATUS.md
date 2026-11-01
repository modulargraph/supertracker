# Feature status

What is built, how each feature was verified, and what has not been verified. The "Not verified" column matters as much as the "Built" column. Dated review notes and per-candidate evidence logs from October 2026 were removed in the 26 October cleanup; they remain in git history. Run records under `test-output/` and `build/` are local and are not kept.

## How features are verified

| Method | What it means | What it does not mean |
| --- | --- | --- |
| **Node UI tests** | `tests/*_tests.js` (94 files at the 11 October review) load the real interface scripts with a simulated page and check keyboard handling, state changes, Undo and rendering. Run by `./scripts/test.sh`. | Not the real WebKit window, not real key presses, no audio. |
| **Engine PCM tests** | C++ tests render the original engine offline and measure the audio (note timing, loop windows, echo times, pre-delay onset, and so on). | Offline rendering only; no audio device, no listening. |
| **Swift unit tests** | Storage, sample editing, slicing, stretch, MIDI files, library index, NKS parsing, plug-in catalogues. | Pieces in isolation, not the running app. |
| **Native self-tests** | A debug (or `scripts/build.sh selftest`) build of the app run with `--self-test`, `--vst2-self-test`, `--vst3-self-test` and similar flags. These host real installed plug-ins and render through the real native audio graph offline. | Offline export graph; a narrow set of installed plug-ins; no live device. |
| **Hidden in-app harness runs** | The real app, started invisibly with `SUPERTRACKER_DEV_SCRIPT`, drives the actual interface by script and saves snapshots. Earlier milestones also used hidden WebKit/Bridge test runs. | Synthetic events, not a person at a keyboard; no listening. |
| **OS-keyboard QA** | Bounded CUA keyboard sessions on review candidates, with audio muted; checks rapid note entry/transpose, view switching, mixer arrows, chord search and disk Save. | No listening, physical MIDI/MK3, or exhaustive native-menu/dialog coverage. |

## Not verified (applies across the app)

- **Real MIDI hardware.** MIDI behaviour (chord/scale mapping, chord sets on a MIDI keyboard, MIDI CC learn, sustain) is tested with simulated or virtual MIDI only. No physical controller, pedal or external clock has been tested.
- **Listening tests.** Nobody has listened to the new features (sampler mode fixes, synced delay/tone, pre-delay, note repeat, automation recording) on speakers. Engine measurements say the audio is correct; they are not a hearing test.
- **Live audio device playback.** Offline rendering is verified. A previous live test received no audio callbacks from the device, so sustained realtime playback, dropouts and audio-interface recording remain untested.
- **Intel-only plug-ins cannot load.** SuperTracker is arm64 only. Intel-only plug-ins (for example Super 8, and about 70 VST2 bundles on the test Mac) cannot be hosted. A VST2-based NKS preset for such a plug-in only works when the vendor's VST2-compatible VST3 is installed.
- **Plug-in crash isolation.** Plug-ins run inside the app's process. Background *scanning* happens in separate helper processes, but a plug-in that crashes while playing or in its editor takes the app down. Plug-in recovery disables suspects on the next launch; it does not contain crashes.
- **Broad vendor compatibility.** End-to-end checks cover a few installed products (Massive X, Arturia DX7 V, Apple AUs, original test plug-ins). Other vendors and presets may behave differently.
- **Complete physical keyboard and accessibility pass** of the whole app, including every dialog.

## Kits

Kits are track groups: one ordinary track per loaded pad plus an automatically routed group bus, with a collapsible group header in the pattern editor, group strips in the mixer, Pad keys, per-track choke groups and voice modes, the Kits library (load, replace, save `.stkit`), Maschine group import, 34 original effects and six DrumSynth families ([kit guide](KITS.md), [data model](KIT_GROUPS_DESIGN.md), [Maschine import](MASCHINE_GROUP_IMPORT.md)). Older kit-lane projects convert on open. Loading a sixteen-pad kit while playing extends the running graph without a rebuild (native offline and real-time WebKit tests, October 2026). Physical MK3, live-device and OS-keyboard kit acceptance remain unverified.

## Status by area

### Added on 10 October

| Feature | Built | How it was verified | Not verified / limits |
| --- | --- | --- | --- |
| VST2 hosting | Clean-room VST2 host for native arm64 VST2 instruments and effects: scanning, shell plug-ins, MIDI, transport, chunk/parameter state, vendor editor. VST2 appears in the plug-in browser with its own format filter. | Native VST2 self-test with installed plug-ins: discovers 104 VST2 classes, plays sequenced MIDI through Arturia DX7 V and restores its state into a fresh instance (identical render level), all offline. | Shell plug-ins are supported in code but no installed shell bundle was checked end to end. Intel-only bundles cannot load. Vendor editors not tested by hand. No crash isolation. |
| VST2-identity NKS presets | NKS presets made for a VST2 load into the native VST2 when it runs, otherwise into the vendor's VST2-compatible VST3 class, with mapped controls. | Same self-test: a DX7 V NKS preset loads into native VST2 with 102 mapped controls and renders; a Massive X VST2-identity preset (Intel-only VST2) loads into the Massive X VST3 and renders non-silent audio. Swift NKS preset tests cover the identity matching. | Other products not checked. Not heard. |
| Background plug-in scanning | VST2 and VST3 catalogues refresh after launch in isolated helper processes; the browser shows scan progress and a Refresh button. | Exercised by the native VST2 self-test scan; catalogue unit tests. | No dedicated UI test of the progress display. |
| Native context menus | Right-click, `Shift+F10` and the Menu key open AppKit menus built for the element (pattern cells, track headers, automatable controls, browser rows, plug-ins, song blocks, sample editor, text fields). No web-style text selection outside fields. | Menu contents are covered indirectly by feature UI tests (pattern tools, song arranger, automation arming). The in-app harness can log native menus. | Not checked with a real right-click or Menu key by a person. |
| Sampler play modes | 1-shot, forward loop, backward loop, ping-pong, slice, wavetable, granular. Fixed backward loop; declicked one-shot/slice edges; seamless loop seams; Hermite interpolation; cycle-aligned wavetables. | Engine PCM tests for each mode (loop windows, seam continuity, interpolation error, wavetable pitch/morph, granular spread). | Not heard. |
| Keyboard sample editor | Zoomable waveform, keyboard markers, zero-crossing snap, hold-to-audition, undoable trim/fades/normalize/reverse/gain/DC/silence that write new files, add/remove slices, slices to new pattern. | Node UI tests for keys and Undo; Swift tests for overview/zoom, zero-crossing snap and 11 source-preserving region edits. | Not used with a real keyboard; audition not heard. |
| Instrument page (Polyend style) | Grouped controls with readable units (ms/s, Hz/kHz, dB, pan, st, ct, notes, bits, beats) and fine/coarse arrow editing in musical steps. | Node UI tests for focus, value formatting and arrow editing. | Not checked by a person. |
| Chord and scale mode | 15 scales, snap-to-key or scale degrees, fixed chords and chords in key, keyboard velocity (127–40). Saved with the song; chords record as step chords. | Node UI tests (pattern performance, command palette). | Not played by a person. |
| Chord sets | 16 harmony sets (192 chords) and 15 style sets (186 chords), all original; grouped selector, Option+G cycling, searchable Cmd+K. Former factory-set IDs in older projects open on the matching harmony set. | Node tests record chords through the keyboard handler, check the MIDI map, palette search and the legacy-ID mapping. The native Save/Open self-test uses the new sets. | Physical MIDI and listening are not covered. |
| Chord/scale/chord sets on MIDI keyboards | Host-side note map applies the same performance settings to MIDI input; can be turned off. | Node tests with simulated MIDI notes. | **No real MIDI keyboard tested.** |
| Note repeat / arpeggiator | Repeat, Arp up/down/up-down/random/as played; 1/4 to 1/32 including triplets; live playing only. | Node UI tests. | Timing not measured against audio; not heard. |
| Pattern mute/solo and pattern tools | Mute/solo from the grid; interpolate, humanize/randomize, louder/softer, reverse, nudge, stretch/compress, double/halve, quick step effects, clear effects. One Undo step each. | Node UI tests (pattern performance). | Not checked by a person. |
| Automate-parameter picker | `Option+P` / context menu; searchable; automated parameters first. | Node UI tests. | — |
| Note columns | Up to eight notes per row on any track (`noteColumns`, `step.columns`): each column its own voice, row FX shared; header + / −, typed entry, OFF/CUT, Delete, transpose, clipboard, recording into free columns, MIDI import/export, agent API. | Engine tests (independent lanes, gates, row FX, copy-on-write); Swift publisher test; MIDI file tests; node UI tests (`note_columns_tests.js`). | Not checked by a person or heard; not covered by the native WebKit runs. |
| Pattern loop range | `Option+Shift+L` loops selected rows (or the current bar) during Pattern playback, sample-accurately. Not saved with the project. | Engine tests (loop timing, swing, tempo map); node playback-mode tests; looping observed in a hidden in-app run. | Not heard. |
| NKS browser | Maschine-style browser over the full installed NKS catalogue (about 23,600 presets): Instruments/Effects, product, bank, type/sub-type, character, search, favorites, audio previews. | Node UI tests; Swift NKS metadata tests; catalogue read on the test Mac. | Preview playback not heard; loading every product not tested. |
| NKS effects into inserts | `.nksfx` presets load into a chosen insert slot of the selected track. | Node loader tests. | Only checked through the loader logic and a few presets. |
| 8-knob control pages | NKS pages for NKS presets; generated, assignable pages with user pages for any AU/VST3/VST2 plug-in. | Node UI tests (plug-in pages, NKS controls). | Large plug-ins and unusual parameter types not checked. |
| Song arranger | Pattern blocks with repeats, colours, insert/duplicate/clone/remove, play from block, follow, section loop; all by keyboard. | Node UI tests; engine PCM tests for song order with repeats and a sample-accurate section loop (including across a tempo map). | Not checked by a person. |
| Synced delay, tone and reverb pre-delay | Delay tempo sync or free time, feedback tone low-pass; reverb pre-delay 0–250 ms. | Engine PCM tests: echo times follow tempo divisions and BPM, tone darkens echoes, pre-delay shifts the wet onset. | Apple-return path (plug-in tracks, standard routing) does not apply pre-delay/damping. Not heard. |
| Send-effects dialog | `Cmd+Shift+X` keyboard dialog for both returns and all track sends. | Node UI tests. | — |
| Live automation recording | Read/Touch/Latch/Write (`Option+W`, `Option+Shift+W`), per-parameter and per-channel arming, MIDI CC learn, one Undo step per pass, thinning. | Node UI tests (writing, Touch return, Latch hold, Write overwrite, Undo, thinning, MIDI learn with simulated CCs). | **Not heard with live audio; no real MIDI controller.** |

### Additional implementation and acceptance

| Feature | Status |
| --- | --- |
| Track freeze | Implemented user-facing Freeze/Unfreeze with Song and Pattern prints. 36 native checks pass, including collected-media Save/Open and exact frozen/live comparisons; nine filesystem suites cover relocation and repair. [Current scope and refused shapes](TRACK_FREEZE_UX.md). Live hardware/listening remains unverified. |
| Render selection to sample | ⌥B / palette / right-click on a pattern selection renders those tracks' notes in the selected rows in a private offline host (live playback is not paused): clean start at the first row, notes held at the end released, tail kept until −70 dBFS for 200 ms (max tail 0–60 s, default 10). Through master or Track only (pre-master); 44.1/48/96 kHz, mono/stereo, normalize. Result: a 1-Shot sampler instrument (root C-4) in `<support>/Recordings`, collected on Save; optional bounce in place (clear or mute originals, C-4 trigger on a new or the first selected track), one Undo step. Refused: frozen tracks, line inputs, external MIDI, hardware editor plug-ins; audio clips are not included. Checks: `tests/AudioRenderSelectionTests.swift`, `tests/render_selection_ui_tests.js`, `--render-selection-self-test` (sample-accurate start, exclusion, tail, cap, master vs pre-master, render while playing). Listening unverified. |
| Sample browser on real libraries | Earlier native indexing covered about 164,000 Native Access/Loopcloud entries. The current review adds 150,000-entry stress coverage and 11 mounted filtering/preview/load stages on private generated samples. Current removable-drive listening/performance remains a separate acceptance task. |

### Established before 10 October

| Area | Built | How it was verified | Not verified / limits |
| --- | --- | --- | --- |
| macOS app | AppKit window, bundled WebKit interface, native menus, no JUCE. | Hidden WebKit runs at 1080 × 700 and 1500 × 940; bounded physical keyboard QA on earlier builds. | Full accessibility and dialog pass. |
| Original engine | C++ scheduling, synth/sample voices, pattern/song clock, step effects, modulation, mix, built-in delay/reverb. | Deterministic engine tests (also under sanitizers in earlier runs); native export renders. | Whole-host realtime robustness on a live device. |
| Tracker | Note/instrument/velocity/gate, chords, microtiming, two FX columns, selection, clipboard, transpose, fill, live note lengths, track duplication. | Engine and UI tests; physical note entry and transpose on an earlier build. | Full musical session by a person. |
| Instruments and sampler | Synth, sampler, AU/VST3/VST2, external MIDI; filter envelope, two tempo-syncable LFOs; slicing (equal, beat, transient), Slice FX, multisample zones, choke/legato; offline time stretch. | Engine PCM and UI tests; detector suites; stretch frequency measurements. | Round robin, zone crossfades, warp markers and formant correction are not implemented. |
| Step effects | Probability, ratchet, microtiming, glide, reverse, offset, arpeggio, slice, mixer locks, exact plug-in parameter locks. | Engine and native tests in both routing modes. | — |
| Audio clips, takes, bounce | Song timeline clips, takes and manual comping, Render stem, Bounce track. | Native renders and UI tests. | Punch-in/out and loop-record take splitting are not implemented. |
| Browser | Root indexing, text and multi-select filters, favorites, preview, local metadata editing. | 163,968-entry native scan of real libraries; 150,000-entry synthetic stress test; UI tests. | See "Sample browser on real libraries" above. |
| AU and VST3 hosting | Instruments and effects, vendor editors, parameters, state save/restore, unavailable-state retention. | Native tests with Apple AUs, original fixtures and installed Massive X (VST3 and NKS). | Broad vendor compatibility, crash isolation. |
| Automation | Pattern lanes, fractional playhead, hold/linear interpolation, `Option+A`, keyboard point editing, exact plug-in targets. | Engine/native PCM tests; hidden WebKit runs; physical envelope editing on an earlier build. | Assignable macros are not implemented. |
| Mixing | Track/group/return/Master, custom sends, sidechains, up to sixteen inserts per channel, insert reordering, latency compensation (2 s capacity). | Native routing and aligned-stem tests; latency PCM tests with zero difference. | Multi-output instruments are not supported; incorrectly reported plug-in latency not tested. |
| MIDI | CoreMIDI input/output, channel filtering, sustain per source/channel; Standard MIDI File import/export. | Virtual CoreMIDI loopback test; production MIDI ingress tests with offline audio; file byte tests. | Physical devices, MPE, full expression recording, external clock. |
| Recording | Input or master capture, hardware sampling profiles, mono/stereo WAV, new-track capture. | Generated-PCM and UI tests. | Real audio-interface recording and roundtrip latency. |
| Projects and export | Versioned JSON, atomic save, autosave/recovery, Undo, collected media, relinking; WAV export (16/24-bit, float) at 44.1/48/96 kHz, stems. | Native export and file-format tests; hidden save/open/relink runs. | Finder/close-dialog workflows by a person; device loss. |
| Plug-in recovery | Session journal, suspect gating, explicit retry. | Native and hidden WebKit tests. | Not crash isolation. |

## Outstanding product requirements

These are limits of the current product, not a reduced definition of done.

- Track freeze live-device/listening acceptance and broader routing shapes beyond the documented supported cases.
- Punch-in/out, loop recording with take splitting, automatic take alignment.
- Tempo and meter maps (one project tempo and meter today; MIDI tempo changes are converted).
- Multi-output instruments.
- Configurable internal sample rate (internal processing is 48 kHz; export supports 44.1/48/96 kHz).
- Warp markers, realtime stretch, round robin, zone crossfades, an assignable modulation matrix.
- Full MIDI CC/pitch/aftertouch recording, MPE, external clock sync.
- Plug-in crash isolation and broader vendor compatibility.
- A complete keyboard-only, accessibility and listening pass on real hardware.
- Maschine MK3 support is **in development** and off by default (Settings turns it on). Still to do: a physical Maschine MK3 pass: pads, encoders, Sound/group/master navigation, disconnect/reconnect and release back to Native Instruments software. Choke, Replace and Section modes still use provisional mappings ([MASCHINE_MK3.md](MASCHINE_MK3.md)).
- Reference comparisons of the built-in effects against Native Instruments beyond Ice. The effects are independent approximations, not NI-identical.
- Maschine import of group mix levels, auxiliary routing, modulation, patterns and macro pages.
- An intermittent live input-acknowledgement timeout was seen once in October and has not been reproduced since. If it returns, keep the session and the input-ack diagnostics before changing behaviour.

Capacities: any number of tracks and additional buses (1024 sanity bound each), 64 instruments, 256 patterns of up to 256 rows, 256 audio clips, 128 zones/slices per instrument, eight notes per step chord, sixteen inserts per channel, 128 custom sends/sidechain connections. Audio is decoded into RAM, up to 100 million frames per file.


# MIDI files

Use **File → Import MIDI File…** to turn a `.mid` or `.midi` file into a new SuperTracker song. Changed sessions first offer Save, Discard or Cancel; a canceled/failed Save stops replacement. The native import path also writes recovery data. Use **File → Export MIDI File…** to save the current arrangement as a Standard MIDI File. Export writes the sequence once and respects track mute and solo.

## Import behavior

- SMF type 0 and type 1 files with musical PPQN timing are supported. Type 2 and SMPTE timing are rejected with an explanation.
- Notes are placed at four rows per beat. Sub-row positions become microtiming step FX. Simultaneous notes with equal velocity and duration share a chord; overlapping notes with different durations or velocities on one source track and channel go into extra note columns of the same tracker track (up to eight note columns), and into additional tracker tracks when the columns are full or their microtiming differs from the row's.
- Note-offs, velocity-zero note-offs, running status, sustain pedal, program changes, bank select, tempo and time signatures are read. Sustained notes retain gates up to 256 rows. Longer gates are shortened and counted in the import warnings. With microtiming, the gate compensates for the remaining fraction of the first row; the same 256 gate limit still applies.
- A changing tempo map is converted to one tracker tempo while preserving elapsed event timing. The original tempo events remain in `midiImport.tempoChanges`. Meter changes remain in metadata; the project displays the initial meter.
- Controller 7 becomes volume automation, controller 10 becomes pan, and controller 91 becomes reverb send. Values carry across pattern boundaries. Other controllers remain in metadata with a warning where they are not played. Sustain is represented in note durations.
- Instruments use audible native synth placeholders. MIDI channel, program and bank information is retained for export; choose the intended samples or Audio Units after import.

Imports create as many tracks as needed (1024 sanity bound), up to 64 instruments, 256 patterns and 65,536 rows. Chords contain up to eight notes; larger chords use extra tracks when available. Notes that cannot fit are counted in warnings rather than silently discarded. The browser does not need to index a library before importing MIDI.

Pitch bend, aftertouch and SysEx device state are not converted into tracker playback. Warnings describe unsupported events. Onsets in the final 5% of a row move slightly earlier because the engine's microtiming range ends at 0.95. Source trailing silence is retained, rounded up to the containing pattern.

## Export behavior

Export creates SMF type 1 at 960 ticks per quarter note. A note without a gate (gate 0) gets its note-off where the next note on its track starts (after microtiming; a probability-dropped note does not end it), at an OFF/CUT, or at the song end, as in playback; a stored step without a gate key exports with the legacy 0.8-row gate. It preserves the project tempo and meter, including every tempo-map change inside the song, song order, note velocities, gates, chords, swing, microtiming, ratchets and instrument MIDI channel routing. Imported program and bank selections are written before their notes. Ratchet repeats continue across empty rows and pattern boundaries at the last count, use the latest note/chord, and stop on another effect, count 1, zero velocity or note-off/cut, matching engine playback. A ratchet-only row can restart the last note/chord. Note-off and cut steps stop notes on their tracker track. Extra note columns (up to the track's `noteColumns`) export as independent voices: a new note, OFF or CUT in a column ends only that column's notes, each with its own gate, and the row's microtiming, ratchet and probability apply to them. Notes are released at the end of the finite arrangement.

Volume, pan and reverb automation export as CC 7, 10 and 91. Linear curves are sampled at quarter-row intervals. External instrument mappings can specify other controls in project data:

```json
{
  "type": "midi",
  "midiChannel": 5,
  "midiProgram": 10,
  "midiCCMap": { "cutoff": 74 },
  "midiCCRanges": { "cutoff": [20, 20000] }
}
```

Channels use 1–16; programs and controller numbers use 0–127. `midiCCRanges` gives the parameter's minimum and maximum for conversion to controller values 0–127. These explicit mappings also apply to step FX. Sample processing, plugin state and unmapped automation are not MIDI data. Probabilistic steps export deterministically as notes and report that difference. Use audio export to retain the rendered sound.

## Verification

`tests/MIDIFileTests.swift` generates independent SMF fixtures and inspects exported event bytes. Passing checks cover running status, velocity-zero note-offs, type 1 metadata, changing tempo, sustain, independent chord durations, microtiming gate compensation, controller carry across patterns, channel/program routing, sequence order, mapped controller curves, exact pattern boundaries, trailing silence, 17-row sustained notes, 256-row gate roundtrip, capacity warnings, and malformed lengths/status/VLQ/SMPTE rejection.

```sh
swiftc -module-cache-path /tmp/supertracker-swift-cache \
  Sources/SuperTracker/MIDIFile.swift tests/MIDIFileTests.swift \
  -o /tmp/supertracker-midi-tests
/tmp/supertracker-midi-tests
```

The reader bounds files to 64 MiB and one million events, validates chunk boundaries and four-byte variable-length quantities, and does not execute or send imported SysEx data.

## Virtual port verification

`tests/MIDILoopbackTests.swift` compiles against the current production `MIDIHost.swift`. It creates a temporary CoreMIDI source and destination and explicitly selects their unique IDs; it never selects a physical output. It verifies running status, realtime messages, note/sustain/pitch bytes, source identity, input-channel filtering, output status lengths/channel preservation, note-off/CC123 bytes, clock/transport/program bytes, invalid-channel rejection and disabled output. The endpoint client is disposed when the test completes.

```sh
mkdir -p .build/test-bin .build/clang-module-cache
swiftc -swift-version 5 -module-cache-path .build/clang-module-cache \
  Sources/SuperTracker/MIDIHost.swift tests/MIDILoopbackTests.swift \
  -o .build/test-bin/midi-loopback
.build/test-bin/midi-loopback test-output/midi-loopback-report.json
```

Run with access to the macOS CoreMIDI service. A restricted command environment may be unable to create a client: the test then writes `status: skipped`, exits 77 and does not claim success. This service-dependent test is separate from the default file/DSP regression command.

The recorded current-source run passed on 9 October 2026 with nine input and nine output messages. This establishes virtual CoreMIDI transport and byte handling, not physical-controller playing, hardware roundtrip timing, clock synchronization, sound-engine all-notes-off behavior or the complete application's MIDI-recording workflow. The native AU timing fixtures are separate evidence and do not substitute for external MIDI timing.

# Duplicate Track

Duplicate Track appends an independently editable copy of the selected track to the end of the project. Existing track numbers stay in place.

1. Select the source track. Press **⌘K**, search for **Duplicate selected track**, and press Return. In Pattern view, **Duplicate track** is also available beside **+ Track**, or from the track header’s context menu.
2. Wait for preparation to finish. The new track is named “Source name copy” and becomes selected; Pattern and Mixer return focus to the copied track’s workspace.
3. Edit the copy. **⌘Z** removes the complete duplication in one undo operation; **⌘⇧Z** restores the same copied identities and contents.

The copy retains volume, pan, mute, solo and sends. If the source is audible, the copy is audible too. Mute either track when comparing variations.

| Content | Copy behavior |
| --- | --- |
| Pattern data | Copies the source column in every pattern, including notes, chords, gates, velocities and both step FX. Song order stays unchanged. |
| Instruments | Creates new instrument slots for the track assignment and every valid instrument referenced in its pattern cells. Other tracks keep their original instruments. |
| Hosted instruments and FX | Creates fresh instances for the source track’s active plug-in descriptors, retaining captured opaque state and sparse insert positions. References in copied plug-in step FX point to the new instances. |
| Automation | Copies source-owned lanes with new lane identities and the new track/owner. Parameter addresses and envelopes are preserved; copied send controls follow the copied sends. |
| Samples and clips | Copies sample settings and timeline clip edits, with new clip identities. Existing audio files remain shared; ordinary project Save collects portable media. |
| Routing | Preserves the same output destination and duplicates the track’s sends. Shared buses and Master stay shared. Incoming sidechains to copied inserts are copied; a self-sidechain follows the new track. Sidechains feeding unrelated inserts remain attached to their original source. |

External MIDI instruments retain their saved ports, channels and recording-input preferences. Both copies therefore address the same hardware until those settings are changed. Duplicating a track adds processing and uses another track; freeze/unfreeze remains separate work.

Preparation captures pending mapped-control/native edits and checks that each new plug-in instance actually loaded before committing the single undoable edit. A capture/load failure leaves the duplicate uncommitted. Edits or project replacement during preparation cancel a stale result and restore the latest project to the audio engine. If restoration itself fails, the error is shown explicitly. Save waits for the pending publication; quit asks for it to finish.

The resulting project must fit **1024 tracks (sanity bound), 64 instruments, 256 audio clips, and 128 sends plus sidechains combined**. Multiple instruments used by one track can consume several free instrument slots. Unsupported routing versions or ambiguous active identities are rejected. Dormant metadata and unrelated parameter references are retained without activating them.

The 2026-10-09 native duplication report passed four focused groups through the actual hidden WebKit UI and Bridge. An original in-process AU instrument and effect in slot 6 preserved exact opaque bytes, including typed metadata. The original/copy instrument parameters remained independently at **0.625 / 0.75**, and effect parameters at **0.375 / 0.875**. Undo unloaded the copied AU; Redo retained its IDs. After saving and clearing the entire native rack, Open restored all four instances and their independent state. The generated source sample remained unchanged.

The run manifest records **47 passing WebKit groups**, a passing separate recovery run, and exact binary/resource hashes; the build report records a successful 125.341-second build. The [19 proposal tests](../tests/track_duplicate_project_tests.js) and [12 application-workflow tests](../tests/track_duplicate_ui_tests.js) cover capacity, reference remapping, stale results, failure recovery, save ordering and undo/redo.

Acceptance limits: this duplication run verified live native state and persistence, not duplicate-track PCM output or real-time playback. Installed vendor/VST3 duplication, hardware MIDI, native file panels and physical keyboard interaction were not exercised. Sidechain remapping has focused project-data coverage; copied-sidechain audio was not measured in this native fixture. The Mac was locked during this acceptance run, so the hidden DOM focus result does not establish physical keyboard behavior.

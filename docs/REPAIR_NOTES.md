# Workflow repairs — October 2026

This pass follows concrete editing, loading, routing and playback failures. It does
not certify every third-party plug-in or physical audio interface as bug-free.

## Corrected behavior

- Browser kit replacement keeps the selected kit, group and sixteen Sound identities,
  preserving its lanes, pattern notes, locks and automation. Compatible processors in
  the same slots keep instance identities; different processors do not receive guessed
  parameter mappings. References to missing processors remain saved and unavailable.
  External routes and shared return chains survive; failed preparation restores the
  previous graph, and Undo restores the former kit. Loading a new kit remains separate.

- Kit pattern lanes have Notes, Full and Summary views. Notes shows every Sound in a
  narrow note column; Full adds velocity and both FX columns; Summary lists every hit.
  These are views of the existing kit events, not extra tracks or instruments. Selecting
  a Sound keeps edits, phrase tools, copy/paste and row shifts on that Sound while
  retaining simultaneous hits from its neighbors.

- Removing a kit keeps unrelated sample instruments, sample files and plug-ins in place.
  Native graph publications during preparation are matched to the staged transaction
  instead of being mistaken for concurrent edits. Surviving lane indices commit only
  after acknowledgement; failure restores the current graph and Undo restores the kit.

- Recording paints the changed grid cell immediately. Keys and Record buttons move
  focus out of text inputs when entering keyboard performance/note-entry mode.
  Keys alone previews; Record / Command-R enters notes. The status bar states this.
- A single selected note can be dragged across rows/tracks. Selection updates keep
  the pointer target mounted; cancelled gestures and session changes cannot commit
  stale drops. Moves/copies retain independent note data and Undo.
- Shift-Space plays from the current pattern's start or the first song block.
  Existing enabled loop ranges still apply.
- Loading an instrument plug-in from a sample/synth destination creates a new
  instrument and lane. Existing sample objects, notes and lanes remain usable.
  Failed loading/capture/publication rolls back the staged destination.
- Reassigning a plug-in creates an independent destination instance without sending
  the source lane's owner ID to the host.
- Removing tracks/kits respects explicit owner IDs before obsolete numeric track
  caches. Unrelated bus/Sound effects and automation survive. Track duplication
  also works in projects containing kit plug-ins.
- A late browser search response preserves navigation made while it was pending.
  Kit compatibility notes have a compact summary and expandable full details.
- Audio graphs stopped externally restart with a new render-cache epoch. Device
  changes request recovery after conflicting graph operations finish.
- AU parameter formatting always passes an explicit value pointer. Three local
  crash reports identified null-pointer dereferences in Digitalis and ARP ODYSSEY
  through the AUv2 bridge; both installed AUs passed the opt-in metadata probe.
- Recovery offers per-plug-in Enable / Retry. Interrupted load/release remains
  disabled until retried; merely active or previously released neighbours are
  diagnostic entries rather than automatic blocks. Failed retries remain disabled.
- Effect-only reverse/offset changes the current sequenced sample voice without
  retriggering its envelope. Ratchets continue through empty rows/patterns and live
  edits until another effect, OFF/CUT, zero velocity or transport reset. MIDI
  export follows the same repeats. Granular/wavetable use their position parameter.

## Repeatable verification

`./scripts/test.sh --native` runs the regular suite and isolated native modes,
including real WebKit note visibility/focus/drag, fixture AU loading after two
samples, independent assignment, recovery, and stopped-graph restart. Where reports and
screenshots go is described in [TESTING.md](TESTING.md).

For the installed AUs involved in the crash reports, run:

```sh
./scripts/probe-au-parameter-metadata.sh Digitalis 'ARP ODYSSEY'
```

The probe is opt-in and does not render audio or open plug-in editors. Passing
metadata inspection does not guarantee all vendor processing/editor paths.
Physical keyboard defaults, device hotplug and listening still need hardware
acceptance; synthetic DOM tests explicitly record that boundary.

## Verification results

- Complete regular suite: 267 jobs, zero failures.
- Final JavaScript rerun after the source/destination transaction repair: 175 jobs,
  zero failures.
- Native audio integration: 187 checks passed. The initial five-minute run timed
  out during framework teardown under parallel load; the ten-minute rerun passed.
- Full WebKit bridge/interaction run: 75 checks passed; final kit/browser destination
  run: 17 checks passed, including two samples followed by a plug-in and independent
  reassignment. Recovery, kit hosting, insert limits and render restart modes passed.
- Installed vendor metadata probes: Digitalis 162 parameters and ARP ODYSSEY 276
  parameters, both exited successfully.
- Release installed at `/Applications/SuperTracker.app`; final signature verified.

The isolated `--plugin-state-self-test` debug mode runs the 17 plug-in state checks
independently when investigating process-history-dependent native test slowdown.

## Solo and Mute during playback

Pattern Solo/Mute previously changed the routing topology signature and rebuilt
running audio nodes. They now update permanent gain controls in the existing graph.
The regression checks unchanged node identity and exact transport advancement for
repeated Solo/Mute/clear in standard and legacy routing with active effects. A
separate two-input PCM fixture checks isolation/restoration, and routing graph tests
verify dry output, sends, returns and sidechain dependencies. The previous routing
policy fails the node-identity test; the updated implementation passes.

## Reverse continuity and note endings

Direction changes now smooth each playing sample voice over 2 ms while preserving
its position and envelope. This includes overlapping voices and release tails;
reverse note starts also fade in. Deterministic PCM tests cover loop crossfades,
one-shot end fades, rapid direction changes and block-size independence. The
reproduced adjacent-sample jump fell from 0.703 to 0.0148.

In the pattern note column, `1` enters OFF and `Shift+1` enters CUT with Keys either
on or off. The keyboard regression exercises both modes without triggering notes.

## Recording workspace and follow-up verification

The Recording main tab (Cmd+7) exposes capture source, interface details and live
input assignments. Ordinary recording defaults to a new sampler instrument and
new lane in every view. Replacement requires selecting the named instrument or
using the explicit Kit Sound action. Legacy implicit lane reassignment is no
longer offered, and generic capture cannot replace an instrument through a Kit lane.

Follow-up verification: full regular suite 276 jobs passed; final JavaScript suite
183 jobs passed; packaged native WebKit run 19 checks passed, including Recording
tab defaults and source propagation, kit deletion with sample PCM and unrelated
plug-in preservation, and Undo. Native line input fixtures passed all 4 checks.

## Embedded recorder and physical microphone repair

The Recording tab now contains the capture controls directly, using the app's
sidebar and panel layout. Record/Stop stays visible in a fixed footer; optional
playback and monitoring settings are collapsed. Draft settings survive repaint,
and active capture prevents navigation or unrelated edits from removing its controls.

The reported Core Audio -10851 failure came from setting an input-only microphone
on the engine's shared duplex HAL unit. Device selection now preserves the input/
output pair, recognizes its existing aggregate, and reports physical device IDs.
A separate bug muted the input mixer before attachment, but attachment reset its
gain to one. The mute is now applied after attachment and wiring, and explicit
monitor settings are applied after meter setup.

The opt-in physical microphone test captured 23,552 mono frames at 44.1 kHz on
this Mac, verified the output device stayed unchanged, checked repeated selection
and restoration, and asserted actual monitor gain zero. Independent native UI
coverage records master audio using Record/Stop and verifies new-lane attachment,
existing instrument preservation and Undo. External interface switching still
requires testing with those interfaces attached.

Final follow-up checks: 184 JavaScript test jobs, 21 native WebKit checks, 4 live
input routing fixtures and 5 graph restart checks passed. The two-size screenshots
were visually inspected; optional controls collapse so both meters and the fixed
capture action remain visible at 1100×760. The physical microphone check above
also passed with monitoring disabled.

## Capture clock and stereo restoration

Input recording now uses the timestamp from the same input tap that supplies its
PCM. The shared duplex unit's output render callbacks are no longer interpreted
as the microphone's clock. A deterministic before/after fixture reproduces the
reported skipped-render-quantum error in the former implementation, accepts
contiguous variable-sized input packets in the new implementation, and still
rejects a genuine missing input packet.

Input selection no longer writes the shared HAL unit's channel map. Selected
channels are extracted in the capture and monitoring paths, with mono monitoring
duplicated to both output channels. This preserves ordinary stereo playback after
a mono recording. Final recording events retain frame counts and partial-file
metadata: useful partial takes can load, empty takes offer retry, and events from
a previous capture cannot replace the current take.

## First-click Keys and independent kit effects

A numeric FX field previously refreshed the interface synchronously on blur,
removing the Keys button between pointer-down and click. Deferred field repaint
preserves the click. The regression dispatches that event sequence, then verifies
audition, note release, and armed note entry. FX edits retain their selected Sound
identity; editing an empty Sound creates a separate effect event without changing
its neighbor, and switching Sounds invalidates an open picker. Native PCM fixtures
check independent reverse, offset, volume, cutoff and ratchet behavior for stacked
Sounds, including effect-only rows.

The extended physical-input fixture now captures twice for more than 12 seconds
per take, loads both WAVs successfully, keeps monitoring disabled, and verifies
the output unit channel map is unchanged. The latest run captured 628,800 and
609,600 mono frames at 48 kHz.

## Live playback clock regression

The expanded native kit test exposed an additional mismatch: AVAudioEngine's
implicit main-mixer connection used 44.1 kHz while the native sample sources used
48 kHz. Those sample timestamps eventually exceeded the shared render cache's
retained range, leaving a moving transport with silent Sound outputs. A standalone
probe verified that explicitly connecting the mixer at the processing rate aligns
the graph without changing the physical device's nominal rate. Topology changes
also invalidate the previous render-cache origin before playback resumes.

Stopped note-entry acknowledgements now retain their captured pattern, row, lane
and Sound when the user navigates before the native reply arrives. The delayed
write leaves the new cursor/focus untouched. Tests cover quick release followed
by an immediate lane or Sound switch; target replacement, conflicting cell edits
and disarming still invalidate unsafe pending writes.

Source nodes also retained independent sample-time origins after reconnection.
Realtime source timestamps are now translated through the master clock's valid
host-time anchor; offline rendering keeps its existing exact sample coordinates.
The control thread publishes the realtime/manual mode while stopped, so the audio
callback does not query AVAudioEngine or introduce a graph-lock inversion.
The final native kit run passes all 22 combined checks, including live track
addition with advancing transport and nonzero Sound PCM, aligned stems, per-Sound
and group effects, persistence, and unrelated-instrument preservation on removal.

## Repeated-save media stability

The full workflow's exact save/reopen comparison found that macOS symlink aliases
(`/var` versus `/private/var`) caused already-collected audio to be collected again,
adding another hash prefix to its filename. ProjectStore now compares resolved
media directories and keeps paths in the destination directory's spelling. A
symlink-alias regression fails on the old comparison and passes with the repair.

Final verification includes 185 passing JavaScript jobs and 190 passing native
integration checks, including the genuine mono-WAV stereo-output regression,
recording boundaries, AU state, routing, render restarts, delivery formats and MIDI.
The default native runner now includes the continuous project-building workflow
and captures process stacks automatically before terminating a timed-out test.

The continuous workflow passes all 9 acceptance groups, including rapid-tap
navigation and exact musical-data persistence. The final line-input suite passes
all 4 checks. The release was installed to `/Applications/SuperTracker.app`;
code-signature verification and exact packaged-resource/binary comparisons pass.

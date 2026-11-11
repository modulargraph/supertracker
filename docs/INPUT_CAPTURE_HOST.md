# Input acknowledgement and source-clock recording

A note played while recording is armed is placed where the engine applied it, not where the page last saw the
playhead. The engine's side (`Control::input_note`, `STInputAck`, the run and the distance it counts) is described
in [AUDIO_ENGINE.md](AUDIO_ENGINE.md#transport); this page is the contract between the host and the interface.

Every note that is recorded takes this path: the interface has no other way to place one. A page opened without
a host answers its own requests the same way, at the stopped transport (`previewRequest` in `ui/app-bridge.js`).

## The acknowledgement

A `note.on` or `note.off` request with `captureInput: true` and a bounded `inputOwner` is answered with
`inputAcknowledgement`:

- `status`: `applied`; `discontinuity` (the timeline changed between the edge and its block); or `unavailable`,
  with a `reason` when the edge could not be queued, the engine did not answer within a second, or the recording
  session or the arm intent changed meanwhile. Only an applied edge may be inserted.
- `applied`, `playing`, `recording`: what the block that took the edge was doing.
- `pattern`, `row`, `sequence`: where the edge landed, from that block, never from published status. A note-on in
  the second half of a row is placed on the next row.
- `early` (the edge was placed on the next row), and the engine's frame counts as decimal strings: `renderFrame`
  and, when the engine has one, `graphFrame`.
- `recordingClock: {distanceBeats, runID}`, only for an applied edge while playing: the quarter beats played in the
  current run, counted through loops, and the run's identity as a decimal string. The run changes whenever the
  timeline does (Play, Stop, Seek, Panic, a tempo or arrangement change).
- `recordingContext`: the project the edge was recorded against, as the engine holds it: `{linesPerBeat,
  swing, loopSong, bpm, patterns: [{id, length}], sequence, clips}`. The interface builds the same object from its
  own project (`inputRecordingContext` in `ui/app-input-capture.js`) and compares the two field by field, so a
  page with an edit the engine has not received cannot use an acknowledgement of the older project.
- `requestID` (a decimal string), `inputOwner`, `on`, `track`, `instrument`, `note`, `velocity`,
  `applyNote`: the request, echoed. `ownershipCurrent: false` marks a result that expired after the session
  changed.

Accepted edges are never retried. The one-second deadline expires only the recording result: a note the engine
already accepted still plays once. An edge that cannot be queued is played through the ordinary note path when
`applyNote` asks for it and reported as unavailable. `applyNote: false` records a shared-key or sustain release
without changing what sounds. Session and record-arm changes invalidate pending results.

## Note lengths

A recorded note's gate is the distance between its two endpoints. Both need a `recordingClock` of the same run,
and the later distance must not be smaller; otherwise the note keeps no gate and the interface says that the exact
duration was unavailable (`ui/app-live-record.js`). The distance in beats is divided by the onset row's spacing,
`(1 ± swing) / linesPerBeat × (1 − microtiming) / ratchet`, with the engine's clamping, to give rows. Notes of one
chord use the first onset and the last release.

## MIDI

When MIDI capture is armed, the immediate `midi.note` event has `recordingPending: true`, and a later
`recordingOnly: true` event carries the acknowledgement. Each onset gets a unique owner reused by its physical
release. A repeated source, channel and pitch cancels the earlier recording owner before the new onset, without
an extra audible note-off. What is recorded is the frame at which the engine applied the note, not the arrival
time of the hardware's MIDI packet.

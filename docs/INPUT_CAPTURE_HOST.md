# Input acknowledgement and source-clock recording

The optional `inputAcknowledgements: {version: 1, supported: true}` capability in
`audio.devices` and native status enables note-edge recording. Without it the
renderer keeps the legacy note protocol. This capability is independent of the
still-disabled-by-default per-unit tempo-map graph.

A `note.on` or `note.off` request with `captureInput: true` and a bounded
`inputOwner` receives `inputAcknowledgement`. The host returns the C engine's
applied pattern ID, row, sequence, playing/recording state and musical distance;
it never reconstructs those values from periodically published status. Only an
applied, playing edge has a `recordingClock`. Request/run/revision identifiers
are decimal strings. Every result also contains the native request's canonical
`recordingContext`, so a renderer with an unsubmitted grid/map/arrangement edit
cannot use an acknowledgement from the old native domain.

Accepted edges are never retried. A one-second native acknowledgement deadline
expires only the recording result; performance commands already accepted by the
engine still execute once. Queue rejection may use one ordinary performance
fallback. `applyNote: false` records a shared-key or sustain release marker
without changing audible ownership. Session and record-arm changes invalidate
pending results. A note already applied before Stop may still be inserted at its
native onset; an invalid or stopped release cannot invent a live duration.

When MIDI capture is armed, the immediate `midi.note` event has
`recordingPending: true`, and a later `recordingOnly: true` event carries the
acknowledgement. Each onset gets a unique owner reused by its physical release.
A repeated source/channel/pitch cancels the earlier recording owner before the
new onset, without inserting an extra audible note-off. This records the frame
at which the engine applies MIDI, not the hardware packet's arrival timestamp.

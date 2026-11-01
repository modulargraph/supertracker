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

Audio recording uses a different source-clock fence. Start is accepted at the
first observed source-render quantum; Stop at the next forward quantum after
its request. Timestamped tap packets are trimmed to that exact half-open
interval, including packets delayed or batched by AVAudioEngine. The recorder
retains its tap endpoint until the end watermark arrives and awaits its serial
file writer before returning success. Mono/stereo channel extraction remains the
existing channel-map output extraction, not a second hardware remap.

Missing source-clock/tap progress fails after a two-second deadline and preserves
the written prefix. This deadline does not cancel synchronous filesystem I/O:
a wedged `AVAudioFile.write` can delay the writer barrier and shutdown. Epoch,
rate, invalid timestamp and writer errors fail explicitly; a device/host
interruption after the complete interval was already written cannot retroactively
mark it partial. The observer pairing and generated/manual fixtures do not prove
live hardware input operation; physical input validation remains separate.

Validation is recorded by `NativeRecordingFenceTests`,
`NativeInputAcknowledgementTests`, the pure boundary/window suites, and actual-app
frontend acknowledgement tests. See each run's immutable source manifest and
report for executed results; source presence alone is not a passing native gate.

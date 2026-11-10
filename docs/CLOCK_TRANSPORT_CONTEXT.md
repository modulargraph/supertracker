# Retained musical context

> The function names in this document are methods of the Rust engine (`tracker_engine::engine`: `ControlHalf` for edits and commands, `RenderHalf` for rendering and span queries; `cargo doc -p tracker-engine`). Errors are `Result`s.

The native clock distinguishes an actual reachable quarter-beat boundary from
the distance to the next line of the unwrapped musical grid:

- `nextQuarterBeatFrame` is a graph-frame prediction along the active transport,
  including loop wrapping. It remains `INT64_MAX` when stopped or when a loop
  cannot reach an integer quarter beat.
- `nextQuarterGridFrame` is a graph-frame distance metric through the immutable
  tempo map, ignoring stopped/loop constraints. Its difference from `graphFrame`
  is the number of samples from the current musical position to the next integer
  quarter beat. Zero is valid at the sampled beat boundary. It does not predict
  that a held or looping transport will reach that beat.
- `ST_CLOCK_GRID_PROJECTION` identifies contexts whose strict reachable value is
  unavailable. Adapters can use the separate grid metric under the convention
  below, without changing transport playing/cycling flags or song position.

Both values use the same first-not-earlier sample quantization as sequencing and
include future tempo changes in the retained map. For example, with 120 BPM
until beat 0.375 and 60 BPM afterward, the stopped beat-0.25 cursor at 48 kHz is
33,000 samples from grid beat 1. Computing solely from its current 120 BPM would
incorrectly give 18,000 samples. Upstream latency translates both graph-frame
values equally, so their distance from the callback frame is preserved.

The agreed AU adapter convention uses the reachable value when available and
otherwise uses the projected-grid distance. Apple's
[musical-context callback documentation](https://developer.apple.com/documentation/audiotoolbox/auhostmusicalcontextblock)
defines an optional next-beat sample offset, but no unavailable-value sentinel.
The projection convention keeps valid tempo, meter and beat context available to
an AU requesting that output. It is explicitly a musical-grid interpretation in
these two states, not a claim that playback advances while stopped. There is no
arbitrary zero or latest-status/tempo fallback. VST3 adapters can retain their
existing context behavior without using this AU-specific fallback.

## Recording intent

`set_recording(enabled)` queues transient host recording intent
on the normal single-producer control queue. It applies at the next uncached
render/control-flush boundary. This flag can be true while stopped: the host
defines it as active audio capture or armed note recording. Play and Stop do not
disarm it. Session replacement and shutdown must explicitly clear it.

The engine exposes `ST_CLOCK_RECORDING` in retained contexts and `recording` in
the atomic status and ephemeral transport checkpoint. A changed value advances
the context epoch, so a retained span stops exactly at the change; queued intent
cannot rewrite previously generated or latency-delayed context. Re-publishing
the same value creates no extra seam. Startup preroll preserves the immutable
origin's flag rather than borrowing a newer recording state.

Checkpoint restoration restores recording intent independently of its `resume`
playing override. Export must capture first, explicitly disable recording for
the export, and restore the captured/current intended arm only if the host
session still matches. These fields are transient, never project persistence.

`STClockOrigin` is unchanged; standalone immutable timelines default to no
recording flag, and the engine adds its queued recording state. Clock and engine
regressions cover stopped fractional/integer positions, tiny loops, future tempo
changes, unchanged reachable loop boundaries, delayed retained recording, Stop,
redundant requests, and checkpoint restore, with zero render/query allocations.

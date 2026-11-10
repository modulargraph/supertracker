# Stepped tempo/meter maps in the engine

> The function names in this document are methods of the Rust engine (`tracker_engine::engine`: `ControlHalf` for edits and commands, `RenderHalf` for rendering and span queries; `cargo doc -p tracker-engine`). Errors are `Result`s.

The engine runs on the clock of [TRACKER_CLOCK.md](TRACKER_CLOCK.md) when a project carries a tempo/meter map. **The host does not publish maps yet**: projects carry one tempo and meter, and `st-audio` clears the map with every publication. Project loading, the editor, the musical context given to plug-ins and export must be connected and tested together before this is a feature.

## Publication contract

The planned optional saved field is:

```json
{
  "timing": {
    "version": 1,
    "tempos": [{"id": "t0", "beat": 0, "bpm": 120}],
    "meters": [{"id": "m0", "beat": 0, "numerator": 4, "denominator": 4}]
  }
}
```

Positions are quarter-note beats. IDs belong to project/UI identity and are not sent to C. The host supplies a stable content-derived map revision. The engine accepts the same limits and strict boundary validation as `TrackerClock`: explicit zero events, unique ascending positions, at most 1024 events per kind, 20–400 BPM, beats through 1,000,000, and supported meters. Stepped changes are the only supported tempo curve. Absence of `timing` retains the original fixed-tempo path, including its wider legacy BPM range and nearest-frame gate/microtiming behavior.

The control thread begins a draft, applies its ordinary project edits, calls `set_timing_map` with complete copied arrays, optionally sets `set_pattern_timing_origin`, and calls `commit`. The map setter validates without replacing the old draft map on failure. Checked commit compiles sequence occurrence prefixes and clip endpoints and rejects arrangement, pattern-origin, or clip bounds beyond the map domain before publication. Failure leaves the published project intact and the draft available for correction. `cancel_update` discards that draft. The legacy void commit uses the same caught validation path; new mapped adapters must use the checked result and diagnostic.

The immutable map and compiled data live in the normal project snapshot. Construction, copying, allocation, and destruction remain on the control thread. The render callback adopts snapshots and retires old ones using the existing engine mechanism. `clear_timing_map` explicitly restores the legacy path on the next successful publication. Identical arrays and revision reuse the prior map identity.

## Scheduling and sound

Song occurrences use the existing swing law: a pattern has `(length + swing when odd) / linesPerBeat` quarter beats, and row `r` begins at `(r + swing when odd) / linesPerBeat`. Swing phase restarts in each pattern occurrence. Native row boundaries use absolute mapped frames; rounded row lengths are never accumulated.

Microtiming and ratchet onsets use the row's musical beat span. A gate uses the post-microtiming ratchet spacing multiplied by the step gate value. Onset and release have separate absolute musical deadlines, so a note held across a tempo change gets the correct duration. The deadline comparison uses the clock's authoritative sample inverse. Mapped scheduling rounds to the first sample not earlier than the musical boundary; it does not borrow the legacy nearest-sample rule or use an epsilon to swallow a deliberately late point. Existing float-valued step parameters retain their represented values.

Native instruments, AU/MIDI note events, native/plug-in/host automation, clips, metronome, synced LFO rates, and the native arpeggio use the effective mapped clock. This proves the engine's emitted event times; it does not prove that a hosted plug-in receives matching per-process context yet. Linear automation remains a 32-frame event stream; stepped automation uses exact beat comparisons, including fractional rows. Native continuous parameter evaluation remains per sample.

Clips remain unwarped source-time PCM. Their starts and optional beat-duration ends map to absolute frames; source offset and fades remain seconds. Available source length limits the end. Seeking into a clip reads the corresponding elapsed source time. Meter changes affect bar numbering and metronome accents, not audio speed or position. Every meter event begins a new bar, including a repeated meter value.

Map edits preserve the musical beat at the publication boundary. Active mapped note releases and pending ratchet/microtiming onsets retain musical deadlines. Free-Hz LFOs retain their units; synced LFOs change effective rate while preserving voice phase. Structural arrangement changes still clear pending step scheduling. No-map projects retain the old scheduling branch.

## Pattern origin, loops, and checkpoints

Song mode uses occurrence prefixes. Pattern audition uses an explicit song-beat origin, default zero, and repeats that same map span each loop. A selected occurrence can therefore audition the tempo of that occurrence. Mapped pattern loops reset the visible song beat while unwrapped musical gate deadlines can continue across the loop. Song wraps release sequenced voices under the existing behavior; live keyboard/MIDI-input ownership remains separate.

Mapped `STStatus.seconds` is absolute song-map seconds, including a pattern audition origin. `duration(..., 0)` returns the length of the active pattern's origin span. Additive status fields expose effective `beat`, `bpm`, numerator/denominator, one-based bar, bar-start beat, and timing revision. Status is the existing set of atomic UI snapshots; render consumers must use retained contexts for a specific frame.

`STTransportState` appends mapped version, revision, beat, pattern origin, loop offset, and iteration. Version zero represents legacy state; version one represents mapped state. This is an ephemeral native ABI requiring recompilation, not a serialized project format. Same-revision restore preserves the exact frame cursor and fractional row. A changed map revision resolves the captured beat in the current map. The original project must still be restored before this stopped-graph operation. Existing cancellation, discarded temporary events, and no-voice/tail-restoration semantics remain: a mid-row resume waits for the next row to produce new notes.

## Retained native context

Bus preparation records an immutable `STClockContext` for each retained frame in a separate 262,144-frame clock ring; the PCM ring remains 65,536 frames. The clock window covers the current 48 kHz host's two-second upstream latency allowance plus its largest pull. A future host with a higher sample rate or longer latency must validate its requirements against `ST_ENGINE_CLOCK_HISTORY_FRAMES`. `clock_span(graphFrame, requestedFrames, upstreamLatencyFrames, ...)` reads that prepared history, subtracts upstream latency, and ends at a musical boundary, available prefix, or transport/revision transition. Old prepared PCM and context retain their old revision after a live edit. A cached query never renders, drains an event, applies a command, or advances status.

`ST_ENGINE_CLOCK_NOT_PREPARED` identifies future or unprepared content. `ST_ENGINE_CLOCK_NOT_RETAINED` identifies expired post-origin history or a query more than 2^30 frames before the startup origin. Invalid inputs fail without modifying output storage. Queries are serialized with bus rendering and accept 1–65,536 frames, graph coordinates within ±2^52, and upstream latency through 2^30 frames. Direct mixed rendering deliberately invalidates bus history.

Contexts include actual quarter beat, effective BPM/meter, bar start/end, cycle bounds, loop iteration, revision/epoch, and `nextQuarterBeatFrame`. The latter uses graph coordinates and returns the current frame on a quantized integer quarter beat; it follows tempo changes and loop wrap. A stopped clock or a short loop with no reachable integer beat returns `INT64_MAX`.

The first prepared frame captures the transport origin after queued play/seek commands and the current project have actually been adopted. Queries preceding that graph frame use this immutable origin and its original map, for up to 2^30 frames of startup prehistory. Playing prehistory is marked `ST_CLOCK_PREROLL`, follows earlier tempo/meter segments or extrapolates the initial segment into negative song time, and never loops backwards. A stopped origin holds its seek position. Spans stop at the startup origin. Later edits do not change startup prehistory; a graph-cache reset captures a new origin on the next prepared frame. Legacy projects use their captured BPM/meter and fractional source-frame origin directly, supporting the legacy 999 BPM limit without constructing a clock on the audio thread.

The per-frame context ring is preallocated with the engine (approximately 38 MiB of clock metadata). Queries allocate nothing and take no locks. A span may scan at most its requested retained prefix for an epoch/revision transition. Startup retains only the immutable map using an atomic render-side pin and a control-owned map pool; it does not retain an entire old project or its PCM. Map destruction remains on the control thread. Per-unit AU/VST caches, context installation, auxiliary-bus ownership, and graph workload admission remain host integration work. Native map density acceptance does not guarantee that a particular host's bounded subdivision budget can process it; that host must explicitly validate its chosen limit before enabling maps.

## Validation scope

`crates/tracker-engine/tests/engine_timing_playback.rs` and `engine_timing_clock.rs` contain focused native suites covering exact tempo-crossing note/gate timing, microtimed ratchets, odd swung repeated patterns/song loops, pattern origins, live map edits, source-time clips/seeks, exact PCM/event equality across 1/7/127/512/4096-frame blocks and overlapping 4097/4096 bus pulls, atomic drafts, and retained context/history. Additional provider regressions cover mapped startup preroll across earlier tempo/meter changes, immutable origin-map lifetime after project retirement, explicit epoch reset, legacy 999 BPM fractional/stopped startup positions, 96,000-frame upstream latency after PCM eviction, independent 262,144-frame clock eviction, and fractional legacy song-end/wrap partitioning. A render/query allocation interceptor reports zero allocations. Thirteen timing/provider groups passed optimized and ASan/UBSan builds. A separate fourteenth group stresses concurrent map publication/collection against origin pin acquisition/release; its targeted ThreadSanitizer run passed with 64 publications and 128 render-reset/preroll cycles. The full fourteen-group suite has not been rerun under each sanitizer.

`crates/tracker-engine/tests/engine_core_*.rs` retain its 37 pre-map suites and adds mapped checkpoint/cancellation/revision restore, exact stepped and continuous automation, meter-change click PCM, both synced LFOs, and failed-publication/cancel regressions, for 41 suites. `crates/tracker-engine/tests/clock.rs` has eleven immutable-clock groups, including quarter-beat/cycle metadata and the negative-preroll boundary regression. The dedicated plug-in-step FX suite remains twelve groups.

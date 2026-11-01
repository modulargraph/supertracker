# Immutable native clock foundation

`TrackerClock.h` provides a C ABI for an immutable tempo/meter map and a graph-time origin. The native engine now adopts it through an optional checked map API. **The saved project adapter, hosted plug-in callbacks, export UI, and tempo-map interface have not enabled the feature yet.** Existing projects without a map retain the legacy scheduling path. See `ENGINE_TIMING.md` for the engine integration contract.

The implementation consists of `TrackerClock.cpp` and the internal `ClockTimingMap.hpp/.cpp`. The C layer adds sample quantization, timeline origins, preroll, loops, latency-adjusted contexts, and bounded span queries.

## Coordinates and rounding

All musical positions are quarter-note beats. Tempo is quarter notes per minute, including in meters such as 5/8. Tempo changes are stepped. A meter event begins a new bar at its exact position and may shorten the preceding bar. A repeated meter value still starts a new bar. Bars are one-based in the song.

Seconds integrate the tempo map from beat zero. Event positions are converted to absolute sample frames with ceil-to-first-not-earlier rounding, rather than rounding and adding each interval. The inverse sample lookup is authoritative at compiled event boundaries. Exact neighboring comparisons correct floating-point inverse rounding and make sample → beat → sample round trips stable; no tolerance snaps a deliberately late event to an earlier sample. For example, at 48 kHz and 120 BPM, beat 0.25 is frame 6000, while the next representable beat above 0.25 is frame 6001.

A continuous beat usually falls between samples. Its returned frame can represent a beat less than one sample later. The inclusive maximum endpoint is beat 1,000,000 at its ceil-quantized frame. A playing span there has zero frames and the `ST_CLOCK_DOMAIN_END` flag. Stopped timelines may hold that position and return a full stopped span.

Tempo and meter events at the same exact beat take effect atomically. Distinct event positions that collapse onto one audio sample are rejected with a diagnostic. The map does not silently discard either event.

## Creating and publishing clocks

Create a map and timeline on a control thread:

```c
STClockTempoEvent tempos[] = {{0, 120}, {16, 90}};
STClockMeterEvent meters[] = {{0, 4, 4}, {32, 5, 8}};
char error[256];
STClockMap *map = st_clock_map_create(48000, tempos, 2, meters, 2,
                                     1, error, sizeof error);
if (!map) { /* Report error and return on the control thread. */ return; }

STClockOrigin origin = {0};
origin.graphOriginFrame = 100000;
origin.startBeat = 8;
origin.playing = 1;
origin.epoch = 1;
STClockTimeline *timeline = st_clock_timeline_create(map, origin,
                                                    error, sizeof error);
st_clock_map_destroy(map); /* The timeline owns its map reference. */
```

After successful creation, publish the immutable timeline through the host's render-safe ownership mechanism. Queries do not acquire ownership or perform reference-count operations. The owner must retain each timeline while readers or cached ranges can still refer to it. Destroy it on a control thread only after those readers are quiescent. Replacing a map or changing a seek/loop/play origin creates a new object; there are no mutable setters.

`revision` identifies the map and `epoch` identifies the transport origin. They are caller-assigned opaque values returned unchanged by every context. The layer neither publishes objects nor supplies a reclamation protocol.

## Seek, loop, stopped state, and preroll

`graphOriginFrame` identifies the continuous graph timestamp corresponding to the once-quantized `startBeat`. A playing context advances by the graph-frame difference from that origin. A stopped context always holds its start position, regardless of query time or latency.

An enabled loop repeats `[loopStartFrame, loopEndFrame)` exactly after the initial pass. Seeking before the loop start creates an intro; seeking at or after its end is rejected. `loopIteration` is zero during the initial pass and increases at each wrap. Quantized loop or seek positions that collapse onto the same endpoint are rejected.

Queries before the origin are marked `ST_CLOCK_PREROLL` and extrapolate backward without looping. They can still have a positive song position when the seek starts later in the song. Negative song frames extrapolate the first tempo and meter; the bar immediately before song bar 1 is bar 0, then bars -1, -2, and so on. Frame inverses and timeline contexts accept this preroll; the continuous beat/seconds conversion functions retain the nonnegative song domain.

## Per-unit context and spans

`st_clock_context(timeline, graphFrame, upstreamLatencyFrames, &context)` maps a graph timestamp to the musical time of the audio entering a processing unit. It subtracts the accumulated upstream input-path latency before applying the origin and loop. Do not subtract that unit's own output latency again. `contentGraphFrame` exposes the adjusted coordinate; `nextBoundaryFrame` remains in the original graph coordinate. `loopStartBeat`/`loopEndBeat` carry the active cycle bounds. `nextQuarterBeatFrame` identifies the next reachable integer quarter beat in graph coordinates, including offset zero on that beat's quantized sample. It respects tempo changes and loop wrap; `INT64_MAX` means none is reachable (for example a stopped timeline or a short loop containing no integer beat).

`st_clock_span` returns the largest prefix of the requested range that does not cross a tempo, meter, bar, loop, origin, or domain boundary. The context belongs to the first sample of that span. It combines boundary flags when several changes share a frame. Advance by the returned `frames` and query again for the remainder. A zero result at the domain endpoint terminates playback; callers must not retry the same zero-length span indefinitely. Requested lengths may be 1 through 65,536, including 4097 and other non-power-of-two pulls.

This is a read-only timing query. It does not render audio, advance a sequencer, drain events, apply pending commands, inspect mutable transport status, or retain PCM. In particular, it does not solve overlapping AVAudioEngine pulls by itself. The host still needs timestamp-range output caches so a stateful AU/VST processes only each uncached suffix. Every unit's context and MIDI/parameter offsets must use the same timeline revision and latency coordinate as that suffix. Main and sidechain pulls need consistent context ownership.

An earlier Audio Unit experiment showed why splitting only the outer graph request is insufficient: a plug-in can ask for 4097 input frames during a 4096-frame output pull. Per-unit splitting plus range caching remains a later integration step.

## Bounds and render-thread behavior

Each map accepts 1–1024 ascending, unique events of each kind, both beginning at beat zero. Beats are 0–1,000,000, tempo is 20–400 BPM, sample rate is 8–384 kHz, and meter uses numerator 1–16 with denominator 2, 4, 8, or 16. These are the new map's explicit bounds; they do not change the legacy engine's tempo limits.

Graph timestamps are limited to ±2^52 frames and upstream latency to 0–2^30 frames. Invalid pointers, nonfinite values, unordered events, unsupported meters, and out-of-domain queries fail explicitly. Query output storage is unchanged on failure. Creation copies input arrays, validates distinguishability, and may allocate; failure returns no partial object and copies a bounded diagnostic into optional caller storage.

Query work uses immutable vectors, bounded binary searches, and a bounded number of exact floating-point corrections. It performs no allocation, locking, sorting, mutation, command processing, or per-query shared ownership operations. A single span query is bounded independently of span length. Processing a whole block can still require many spans for a very dense map; the future host must budget or validate that workload and the corresponding plug-in call count. These tests make no whole-host realtime-performance claim.

## Focused validation

Run `bash scripts/test-clock.sh` or `bash scripts/test-clock.sh --sanitize`. Both passed all eleven groups on the implementation snapshot:

- Tempo/seconds/sample conversion and shortened 5/8 bars.
- Exact and adjacent-representable ceil boundaries; atomic tempo/meter events.
- Negative preroll and delayed graph origin.
- Exact negative bar boundaries, including the 8 kHz/400 BPM regression and opposite graph-origin extremes with maximum latency.
- Seek, intro, loop wrap, distant iteration, and per-unit latency equivalence.
- Stopped contexts and map lifetime retained by independent immutable timelines.
- Input rejection, collapsing-event diagnostics, and untouched failed outputs.
- Sample-rate/tempo/domain extremes and zero-length end spans.
- Span partitioning across changing tempo, meter, loops, latency, and pull sizes.
- Next-quarter timing across tempo changes, fractional ceil frames, loops, stopped state, and loops with no quarter-note boundary.
- 30,000 randomized round trips on 1024-event maps and two concurrent readers performing 200,000 context/span pairs with zero observed query allocations.

The sanitizer variant enables AddressSanitizer and UndefinedBehaviorSanitizer. This is a focused native clock result, not a new device, vendor plug-in, integrated host, or ThreadSanitizer acceptance run.

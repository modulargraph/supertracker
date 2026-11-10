# Native frozen-stage timeline provider

Status: implemented, default disabled, pure native tests only. No Host/Rack/JavaScript activation is included. The existing input ACK structs, functions and recording-run semantics are unchanged.

## Control publication and lifetime

`st_engine_set_frozen_song_policy(engine, enabled, publicationID, error, capacity)` edits the immutable project draft. Enable accepts exactly `1` and a nonzero publication ID; disable accepts exactly `0`. Checked project commit remains the atomic publication boundary. The setter catches allocation errors and leaves the draft policy unchanged on failure. Enabling lazily allocates one fixed 262,144-frame metadata history on the control thread, retained until quiescent engine destruction. It uses `sizeof(STFreezeContext) * 262144` bytes (22 MiB with the current ABI), outside the decoded freeze-PCM budget. Discarding a draft can leave this bounded reusable history allocated. No history allocation or destruction occurs on render/query paths.

The host supplies a fresh publication identity for the verified set of stage assets and retains their session owner separately. Publication ID alone is not a user-session identity. Graph installation, policy changes, rollback and source retirement require host quiescence/lifetime fencing. The provider neither owns decoded assets nor validates their SHA256 metadata.

## Preparation authority, including an entirely frozen song

`st_engine_freeze_span(engine, graphFrame, requestedFrames, upstreamLatencyFrames, &span)` is a read-only query, serialized with `st_engine_render_bus`. It never processes commands, advances transport, renders audio or reads periodic status. It returns unavailable when the requested range has not been prepared.

The graph must retain a silent transport anchor or an equivalent serialized preflight that calls `st_engine_render_bus` for every needed new graph range. Null left/right output pointers and any valid bus are supported. This requirement remains when **every track is frozen or muted and no original source node is pulled**. The existing shared timestamp cache is the sole advancement authority: overlapping requests such as `(0,4097)`, `(0,4096)` and `(4096,4096)` generate each suffix once. A query cannot replace this preparation step. Preparing all dependent ranges must precede child/source rendering, with bounded retained PDC history; do not add a second independent clock or infer playback from status.

## Exact contiguous spans

A successful query returns a positive contiguous `frames` count no larger than the request, within graph coordinates ±2^52. Requests accept 1…65,536 frames and integer upstream latency 0…2^30. The content coordinate is `graphFrame - upstreamLatencyFrames`. A caller repeats the query for any remaining block suffix.

Each context includes immutable publication, playback and transition IDs; graph/content/project integer frames; transition content/project frames; sample rate; phase; cause and flags. It splits at every phase, mapping, publication, playback or transition change. Every whole-song loop starts a new playback ID, even if hundreds of loops occur inside one pull. These IDs are independent of the input ACK run, which remains continuous through ordinary loops. Identity exhaustion fails closed with an invalid-position phase instead of reusing an ID.

In `SONG`, project frame advances by one per content frame. In `STOPPED`, it holds. `UNSUPPORTED` marks Pattern mode. `DISABLED` and `INVALID_POSITION` are also silent substitution states. A graph reset/seek/restore/replacement establishes a fresh playback identity; normal downstream mix edits preserve it. Same-policy semantic timing/arrangement/loop-domain changes invalidate the old playback. A host must still reject protected frozen-source edits according to its project model.

History retains 262,144 frames independently of the 65,536-frame PCM cache. Expired post-origin frames return `ST_ENGINE_CLOCK_NOT_RETAINED`; future frames return `ST_ENGINE_CLOCK_NOT_PREPARED`. The query never extrapolates old post-origin transitions. A newly allocated history cannot retroactively describe frames rendered before allocation. Output is untouched on error. With no allocated history the result is `ST_FREEZE_NOT_CONFIGURED`.

Before the first prepared graph frame, the immutable origin permits bounded PDC preroll up to 2^30 frames. It explicitly reports `STOPPED | ST_FREEZE_PREROLL`, even when the origin is a seek to a positive song position. Its held project cursor is irrelevant to playback because this phase is silent. The span ends at the origin or graph-coordinate limit. After graph reset, a new first prepared frame replaces the startup origin.

## Natural completion and printed tails

Only `phase == ST_FREEZE_NATURAL_TAIL` **and** `cause == ST_FREEZE_CAUSE_NATURAL_END` represents authoritative natural completion. Its `transitionProjectFrame` is the exact ceiled full-song musical length, including decoded clip extensions. Its `transitionContentGraphFrame` is the original graph-content frame where completion occurred. It retains the playback ID that reached the end; transition ID identifies this one event.

After natural completion, the native transport is stopped, but the retained replay cursor is:

```
projectFrame = transitionProjectFrame
             + contentGraphFrame - transitionContentGraphFrame
```

A source must validate that endpoint against its captured `musicalFrames`, sample rate, publication and session, then use the typed `AudioFreezeSource.NaturalTailToken`. Stage PCM bounds decide when the printed tail ends. The provider does not know a particular stage's tail duration. Capture latency has already been trimmed from stage files; query upstream latency selects retained original content and must not trim the file again.

An applied explicit Stop immediately produces `STOPPED` and a new transition/playback identity, even one frame before natural end or during a printed tail. No future printed musical notes are replayed. Seek, restore, replacement and graph reset never synthesize natural completion. Panic follows existing engine transport semantics, but reports its own cause and cancels any prior natural-tail authorization. A mix-only edit retains an ongoing natural tail. Host cancellation must apply its explicit Stop intent and session/publication fencing, not merely inspect `playing == false`.

## Validation

Run `cargo test -p tracker-engine --release --test engine_freeze_timeline` in `rust/` (without `--release` for a build with overflow and bounds checks). Ten groups cover allocation failure and invalid-policy rollback; fractional swung seek boundaries; pending/mid-row checkpoint restoration; exact no-freeze rollback; natural end versus Stop; tempo maps; clips extending by both whole samples and less than the legacy tolerance; 700+ loop wraps per pull; retained publication/PDC contexts; pattern exclusion; bounded history and coordinates; and transport preparation with all computation bits disabled. Render and query paths measure zero C++ allocations.

`rust/tracker-engine/tests/engine_freeze_timeline.rs` holds the pure tests. A separate legacy comparison checked nine fixtures against the original pre-timing engine, including overlapping bus pulls. These are bounded pure-engine proofs, not end-to-end production Freeze validation.

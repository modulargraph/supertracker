# Exact frozen song extent

Status: implemented in the original C++ engine, **opt-in and not wired into Host/project playback**. Ordinary projects and the original frozen policy setter without an explicit extent retain their previous arithmetic and behavior. Existing C structures, input ACKs, event layout and checkpoint layout are unchanged.

Control publishes the following in the same immutable project transaction as the projected musical data:

```c
st_engine_set_frozen_song_policy(engine, 1, publicationID, error, capacity);
st_engine_set_frozen_song_extent(engine, publicationID, musicalFrames, error, capacity);
st_engine_commit_update_checked(engine, error, capacity);
```

The extent setter requires the same enabled nonzero publication ID and a positive integer in `1…100,000,000`. Repeating the identical value succeeds; changing an already assigned value for that publication rejects without altering the draft. A new publication ID, disabling the policy or `clear_project` clears the override. Failed checked commit preserves the published project and leaves the rejected draft available for cancellation/correction. The old void commit still suppresses exceptions, but integration must use checked commit to observe rejection.

Checked commit compiles timing/clips, then rejects an extent before the projected live sequence or a non-muted source-limited clip. Mapped sequence/end positions use compiled integer frames. Legacy admission compares the requested integer output boundary against sampled musical/source positions, avoiding a source-frames → seconds → frames roundtrip. Positive tiny durations remain significant even when adding them to a large beat position would round back to the same start. Explicit-extent mapped publications also correct a source-limited clip's end by bounded inverse sample comparisons. There is no epsilon snapping. Default-off mapped clip compilation is unchanged.

The effective playback end uses the integer extent directly. This matters at 48 kHz: `6007 / 48000 * 48000` can exceed 6007, while the analogous 6005 roundtrip can fall below it. Sequencing, ordinary song loops, natural-tail transition coordinates, retained clock loop bounds and input-recording loop distance all use the same effective end. `st_engine_duration(engine, 1)` returns `musicalFrames / sampleRate` seconds; it is informational floating-point output, so the Host must retain the admitted integer value instead of recovering it with `ceil(seconds * rate)`. Pattern audition/duration ignores the full-song override and remains unsupported by frozen source substitution.

This allows a projected song to remove a printed owner's original clips and PCM while retaining the captured full-song end. The native sequence may finish before that end; transport continues through the remaining extension, then emits an authoritative natural completion at the exact declared frame. A mid-extension checkpoint resumes that cursor without retriggering notes. Explicit Stop still suppresses subsequent printed notes and never authorizes a natural tail.

For this explicit-extent policy only, a replacement or restored Song cursor at or beyond the declared end is clamped to that end and stopped. It retains the replacement/restore cause; it does not loop, emit future sequence notes or manufacture natural completion. Host integration must pre-admit compatible checkpoints and inspect the actual restored play state before reporting resumption. Rollback republishes the original policy/extent **before** restoring the original checkpoint; no policy fields were added to the checkpoint ABI.

## Host admission remains separate

Native lower-bound validation proves only that the installed projected arrangement fits. It cannot prove the printed media represents the current canonical project. The Host must verify source hashes, sample rate, exact agreement among active print ends and historical canonical duration evidence. Equal print ends alone do not detect deletion/shortening of an unrelated clip that previously determined the full-song end. `AudioFreezeProject`'s existing arrangement fingerprint omits that unrelated clip geometry. Before activation, require explicit temporal evidence or conservative edit protection that rejects canonical duration changes in either direction until Refresh/Unfreeze. Preserve unavailable canonical source data for Save/repair while verified prints remain independently playable.

No native audio, plugin, device or GUI execution is part of this change. Focused commands:

```sh
cd rust
cargo test -p tracker-engine --release --test engine_freeze_extent
cargo test -p tracker-engine --test engine_freeze_extent    # debug: overflow and bounds checks
```

The nine focused groups cover API validation, failed allocation/publication, exact 6005/6007 boundaries, removed source clips, later arrangement rejection, tiny positive duration, loops/ACK identity, mid-extension restore/Stop, incompatible cursor clamp, original-policy rollback, Pattern exclusion and maximum extent across 8/48/384 kHz. Render/query allocation and deallocation counters must remain zero. `rust/tracker-engine/tests/engine_freeze_extent.rs` holds them; a separate comparison against the old engine checked PCM/event parity.

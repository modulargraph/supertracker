//! Port of `tests/clock_tests.cpp`: the sample-accurate musical clock through its safe Rust API
//! (`STClockMap`, `STClockTimeline`): conversion, exact post-tempo frame inverse, projected
//! stopped/short-loop grid, ceil boundaries, preroll, loop/seek/latency, immutable lifetime,
//! validation, extremes, span partitioning and concurrent zero-allocation queries.
//!
//! Dropped from the C++ program (C pointer mechanics, see `validation_and_untouched_outputs`):
//! the null tempo list, the one-byte error buffer, and the "output left untouched on error"
//! halves of five assertions (a `Result`/`Option` has no output to touch).
#![cfg(feature = "clock")]

mod engine_support_b;

use engine_support_b::alloc::{realtime, render_allocations};
use tracker_engine::clock::*;

fn tempo(beat: f64, bpm: f64) -> STClockTempoEvent {
    STClockTempoEvent { beat, bpm }
}

fn meter(beat: f64, numerator: i32, denominator: i32) -> STClockMeterEvent {
    STClockMeterEvent { beat, numerator, denominator }
}

/// C++ `struct Map`: creation must succeed.
#[track_caller]
fn map_with(tempos: &[STClockTempoEvent], meters: &[STClockMeterEvent], rate: f64, revision: u64) -> STClockMap {
    let result = STClockMap::new(rate, tempos, meters, revision);
    if let Err(error) = &result {
        eprintln!("Map: {error}");
    }
    assert!(result.is_ok());
    result.unwrap()
}

/// `Map map(tempos, meters)`: 48 kHz, revision 1.
#[track_caller]
fn map_48k(tempos: &[STClockTempoEvent], meters: &[STClockMeterEvent]) -> STClockMap {
    map_with(tempos, meters, 48000.0, 1)
}

/// `Map map;`: 120 BPM in 4/4.
#[track_caller]
fn plain_map() -> STClockMap {
    map_48k(&[tempo(0.0, 120.0)], &[meter(0.0, 4, 4)])
}

fn origin(
    graph_origin_frame: i64,
    start_beat: f64,
    playing: i32,
    loop_enabled: i32,
    loop_start_beat: f64,
    loop_end_beat: f64,
    epoch: u64,
) -> STClockOrigin {
    STClockOrigin { graph_origin_frame, start_beat, playing, loop_enabled, loop_start_beat, loop_end_beat, epoch }
}

/// C++ `struct Timeline`: creation must succeed.
#[track_caller]
fn timeline_at(map: &STClockMap, origin: STClockOrigin) -> STClockTimeline {
    let result = STClockTimeline::new(map, origin);
    if let Err(error) = &result {
        eprintln!("Timeline: {error}");
    }
    assert!(result.is_ok());
    result.unwrap()
}

/// `Timeline timeline(map)`: origin `{0,0,1,0,0,0,1}`.
#[track_caller]
fn default_timeline(map: &STClockMap) -> STClockTimeline {
    timeline_at(map, origin(0, 0.0, 1, 0, 0.0, 0.0, 1))
}

#[track_caller]
fn frame(m: &STClockMap, beat: f64) -> i64 {
    let result = m.frame_at_beat(beat);
    assert!(result.is_some());
    result.unwrap()
}

#[track_caller]
fn beat(m: &STClockMap, frame: i64) -> f64 {
    let result = m.beat_at_frame(frame);
    assert!(result.is_some());
    result.unwrap()
}

#[track_caller]
fn context_at(t: &STClockTimeline, frame: i64, latency: i64) -> STClockContext {
    let result = t.context(frame, latency);
    if let Err(error) = &result {
        eprintln!("context frame={} latency={} status={}", frame, latency, error.status());
    }
    assert!(result.is_ok());
    let result = result.unwrap();
    assert!(
        result.beat.is_finite() && result.bpm.is_finite() && result.bar_start_beat.is_finite() && result.bar_end_beat.is_finite()
    );
    result
}

#[track_caller]
fn context(t: &STClockTimeline, frame: i64) -> STClockContext {
    context_at(t, frame, 0)
}

#[track_caller]
fn span_at(t: &STClockTimeline, frame: i64, count: u32, latency: i64) -> STClockSpan {
    let result = t.span(frame, count, latency);
    assert!(result.is_ok());
    result.unwrap()
}

#[track_caller]
fn span(t: &STClockTimeline, frame: i64, count: u32) -> STClockSpan {
    span_at(t, frame, count, 0)
}

/// The program's final `assert(queryAllocations.load()==0)`.
fn zero_query_allocations() {
    assert!(render_allocations() == 0);
}

#[test]
fn conversions_and_meters() {
    let map = map_48k(&[tempo(0.0, 120.0), tempo(4.0, 60.0)], &[meter(0.0, 4, 4), meter(6.0, 5, 8)]);
    assert!(frame(&map, 0.0) == 0 && frame(&map, 4.0) == 96000 && frame(&map, 6.0) == 192000 && frame(&map, 8.5) == 312000);
    assert!(beat(&map, 192000) == 6.0);
    let timeline = default_timeline(&map);
    let a = context(&timeline, 96000);
    assert!(
        a.bpm == 60.0 && a.beat == 4.0 && a.numerator == 4 && a.bar == 2 && a.bar_start_beat == 4.0 && a.bar_end_beat == 6.0
    );
    let b = context(&timeline, 192000);
    assert!(
        b.beat == 6.0
            && b.numerator == 5
            && b.denominator == 8
            && b.bar == 3
            && b.bar_start_beat == 6.0
            && b.bar_end_beat == 8.5
    );
    let c = context(&timeline, 312000);
    assert!(c.bar == 4 && c.bar_start_beat == 8.5);
    let seconds = map.seconds_at_beat(6.0);
    assert!(seconds == Some(4.0));
    let roundtrip = map.beat_at_seconds(seconds.unwrap());
    assert!(roundtrip == Some(6.0));
    zero_query_allocations();
}

#[test]
fn ceil_and_atomic_boundaries() {
    let plain = plain_map();
    assert!(frame(&plain, 0.25) == 6000);
    assert!(frame(&plain, 0.25f64.next_up()) == 6001);
    assert!(frame(&plain, 0.25f64.next_down()) == 6000);
    let event_beat = 123.25 / 24000.0;
    let map = map_48k(&[tempo(0.0, 120.0), tempo(event_beat, 60.0)], &[meter(0.0, 4, 4), meter(event_beat, 5, 8)]);
    let timeline = default_timeline(&map);
    assert!(frame(&map, event_beat) == 124);
    let (before, after) = (context(&timeline, 123), context(&timeline, 124));
    assert!(
        before.bpm == 120.0
            && before.numerator == 4
            && after.bpm == 60.0
            && after.numerator == 5
            && after.denominator == 8
            && after.beat >= event_beat
    );
    let part = span(&timeline, 0, 4096);
    assert!(
        part.frames == 124
            && (part.context.next_boundary_kinds & ST_CLOCK_BOUNDARY_TEMPO) != 0
            && (part.context.next_boundary_kinds & ST_CLOCK_BOUNDARY_METER) != 0
    );
    assert!(after.beat - event_beat < 60.0 / (60.0 * 48000.0));
    zero_query_allocations();
}

#[test]
fn preroll() {
    let map = plain_map();
    let timeline = default_timeline(&map);
    let a = context(&timeline, -1);
    assert!(a.flags & ST_CLOCK_PREROLL != 0);
    assert!(a.flags & ST_CLOCK_PLAYING != 0);
    assert!(a.song_frame == -1 && a.bar == 0 && a.bar_start_beat == -4.0 && a.bar_end_beat == 0.0 && a.beat < 0.0);
    let b = context(&timeline, -96001);
    assert!(b.bar == -1 && b.bar_start_beat == -8.0 && b.bar_end_beat == -4.0);
    let latency = span_at(&timeline, 0, 4096, 64);
    assert!(
        latency.frames == 64
            && latency.context.song_frame == -64
            && latency.context.next_boundary_frame == 64
            && (latency.context.next_boundary_kinds & ST_CLOCK_BOUNDARY_ORIGIN) != 0
    );
    let at_zero = context_at(&timeline, 64, 64);
    assert!((at_zero.flags & ST_CLOCK_PREROLL) == 0 && at_zero.beat == 0.0 && at_zero.content_graph_frame == 0);
    zero_query_allocations();
}

#[test]
fn preroll_bar_boundaries() {
    let exact = map_with(&[tempo(0.0, 400.0)], &[meter(0.0, 1, 16)], 8000.0, 1);
    let timeline = default_timeline(&exact);
    let at = context(&timeline, -156300);
    assert!(at.bar_start_beat == -130.25 && at.beat >= at.bar_start_beat);
    let part = span(&timeline, -156300, 65536);
    assert!(part.frames > 0 && part.frames <= 301);
    assert!(context(&timeline, -156300i64 + part.frames as i64 - 1).bar == at.bar);
    assert!(context(&timeline, -156300i64 + part.frames as i64).bar != at.bar);
    for rate in [8000.0, 44100.0, 48000.0, 384000.0] {
        for bpm in [20.0, 119.99, 123.0, 400.0] {
            let map = map_with(&[tempo(0.0, bpm)], &[meter(0.0, 1, 16)], rate, 1);
            for graph_origin in [0i64, 1i64 << 52] {
                let current = timeline_at(&map, origin(graph_origin, 0.0, 1, 0, 0.0, 0.0, 1));
                for query in [-156301i64, -156300, -156299, -1, -(1i64 << 52), -(1i64 << 52) + 2048] {
                    let range = span_at(&current, query, 65536, 1i64 << 30);
                    assert!(range.frames > 0);
                    let last = context_at(&current, query + range.frames as i64 - 1, 1i64 << 30);
                    assert!(last.bar == range.context.bar && last.bar_start_beat == range.context.bar_start_beat);
                    if range.context.next_boundary_kinds & ST_CLOCK_BOUNDARY_BAR != 0 {
                        assert!(
                            context_at(&current, range.context.next_boundary_frame, 1i64 << 30).bar != range.context.bar
                        );
                    }
                }
            }
        }
    }
    zero_query_allocations();
}

#[test]
fn seek_loop_and_latency() {
    let map = map_with(&[tempo(0.0, 120.0), tempo(4.0, 60.0)], &[meter(0.0, 4, 4), meter(6.0, 5, 8)], 48000.0, 71);
    let timeline = timeline_at(&map, origin(100000, 6.0, 1, 1, 4.0, 8.0, 22));
    let start = context(&timeline, 100000);
    let delayed = context_at(&timeline, 100064, 64);
    assert!(start.song_frame == 192000 && start.beat == 6.0 && start.epoch == 22 && start.revision == 71);
    assert!(
        delayed.song_frame == start.song_frame
            && delayed.beat == start.beat
            && delayed.bar_start_beat == start.bar_start_beat
            && delayed.content_graph_frame == start.content_graph_frame
    );
    let wrap = context(&timeline, 196000);
    assert!(wrap.song_frame == 96000 && wrap.beat == 4.0 && wrap.loop_iteration == 1 && wrap.bpm == 60.0);
    let second = context(&timeline, 388000);
    assert!(second.song_frame == 96000 && second.loop_iteration == 2);
    let before = span(&timeline, 195999, 4096);
    assert!(before.frames == 1 && (before.context.next_boundary_kinds & ST_CLOCK_BOUNDARY_LOOP) != 0);
    let historical = context(&timeline, 99999);
    assert!(
        historical.loop_iteration == 0 && (historical.flags & ST_CLOCK_PREROLL) != 0 && historical.song_frame == 191999
    );
    let intro = timeline_at(&map, origin(0, 0.0, 1, 1, 4.0, 8.0, 23));
    assert!(context(&intro, 0).beat == 0.0 && context(&intro, 288000).beat == 4.0);
    let far = context(&timeline, 1i64 << 52);
    assert!(far.loop_iteration > 1000000 && far.song_frame >= 96000 && far.song_frame < 288000);
    zero_query_allocations();
}

#[test]
fn stopped_and_immutable_lifetime() {
    let tempos = [tempo(0.0, 120.0)];
    let meters = [meter(0.0, 4, 4)];
    let map = STClockMap::new(48000.0, &tempos, &meters, 9);
    assert!(map.is_ok());
    let map = map.unwrap();
    let paused = timeline_at(&map, origin(1000, 3.125, 0, 0, 0.0, 0.0, 41));
    // The timeline owns a map reference: the caller's map may go away.
    drop(map);
    let (a, b) = (context_at(&paused, -1000, 64), context_at(&paused, 500000, 257));
    assert!(
        a.song_frame == b.song_frame
            && a.beat == 3.125
            && b.beat == a.beat
            && (b.flags & ST_CLOCK_PLAYING) == 0
            && (b.flags & ST_CLOCK_PREROLL) == 0
            && a.revision == 9
            && a.epoch == 41
    );
    assert!(span(&paused, 1000, 4096).frames == 4096);
    let newer = map_with(&[tempo(0.0, 60.0)], &[meter(0.0, 7, 8)], 48000.0, 10);
    let next = timeline_at(&newer, origin(1000, 3.125, 1, 0, 0.0, 0.0, 42));
    assert!(
        context(&next, 1100).revision == 10 && context(&paused, 1100).revision == 9 && context(&paused, 1100).bpm == 120.0
    );
    zero_query_allocations();
}

#[test]
fn validation_and_untouched_outputs() {
    let tempo_list = [tempo(0.0, 120.0)];
    let meter_list = [meter(0.0, 4, 4)];
    // C++ `invalid`: creation fails with an error text; the last text is kept for `strstr`.
    let mut error = String::new();
    let mut invalid = |rate: f64, t: &[STClockTempoEvent], m: &[STClockMeterEvent]| {
        let p = STClockMap::new(rate, t, m, 0);
        error = p.as_ref().err().map_or(String::new(), |failure| failure.message().to_string());
        assert!(p.is_err() && !error.is_empty());
    };
    invalid(f64::NAN, &tempo_list, &meter_list);
    invalid(7999.0, &tempo_list, &meter_list);
    // (C++ also passed a null tempo pointer here: dropped.)
    invalid(48000.0, &[], &meter_list);
    let too_many: Vec<STClockTempoEvent> = (0..1025).map(|n| tempo(n as f64, 120.0)).collect();
    invalid(48000.0, &too_many, &meter_list);
    let order = [tempo(0.0, 120.0), tempo(2.0, 60.0), tempo(1.0, 90.0)];
    invalid(48000.0, &order, &meter_list);
    let value = [tempo(0.0, f64::INFINITY)];
    invalid(48000.0, &value, &meter_list);
    let malformed = [meter(0.0, 5, 3)];
    invalid(48000.0, &tempo_list, &malformed);
    let collapsed = [tempo(0.0, 120.0), tempo(1e-8, 100.0), tempo(2e-8, 80.0)];
    invalid(48000.0, &collapsed, &meter_list);
    assert!(error.contains("collapse"));
    // C++: a one-byte error buffer is still terminated; here: the creation fails.
    assert!(STClockMap::new(0.0, &[], &[], 0).is_err());
    let good = plain_map();
    let mut o = origin(0, 0.0, 1, 1, 1.0, 1.0, 0);
    assert!(STClockTimeline::new(&good, o).is_err());
    o = origin(0, 0.0, 1, 1, 1e-8, 2e-8, 0);
    assert!(STClockTimeline::new(&good, o).is_err_and(|failure| failure.message().contains("collapse")));
    o = origin(0, f64::NAN, 0, 0, 0.0, 0.0, 0);
    assert!(STClockTimeline::new(&good, o).is_err());
    let timeline = default_timeline(&good);
    assert!(timeline.context(0, -1) == Err(ClockQueryError::Invalid));
    assert!(timeline.context(i64::MAX, 0) == Err(ClockQueryError::OutsideDomain));
    assert!(good.frame_at_beat(f64::NAN).is_none());
    assert!(good.beat_at_seconds(-1.0).is_none());
    assert!(timeline.span(0, 0, 0) == Err(ClockQueryError::Invalid));
    assert!(timeline.span(0, 65537, 0) == Err(ClockQueryError::Invalid));
    zero_query_allocations();
}

#[test]
fn domain_extremes() {
    for rate in [8000.0, 384000.0] {
        for bpm in [20.0, 400.0] {
            let map = map_with(&[tempo(0.0, bpm)], &[meter(0.0, 1, 16)], rate, 1);
            let end = frame(&map, 1000000.0);
            assert!(beat(&map, end) == 1000000.0);
            let timeline = default_timeline(&map);
            let last = span(&timeline, end, 4096);
            assert!(last.frames == 0 && (last.context.flags & ST_CLOCK_DOMAIN_END) != 0);
            assert!(timeline.context(end + 1, 0) == Err(ClockQueryError::OutsideDomain));
            let extreme = context_at(&timeline, -(1i64 << 52), 1i64 << 30);
            assert!(extreme.beat.is_finite() && extreme.bar <= 0 && extreme.next_boundary_frame > extreme.graph_frame);
            let paused = timeline_at(&map, origin(0, 1000000.0, 0, 0, 0.0, 0.0, 0));
            assert!(span(&paused, 0, 4096).frames == 4096);
        }
    }
    zero_query_allocations();
}

#[test]
fn span_partitioning() {
    let map = map_48k(
        &[tempo(0.0, 120.0), tempo(0.17066666666666666, 60.0), tempo(0.6, 180.0)],
        &[meter(0.0, 4, 4), meter(0.17066666666666666, 5, 8), meter(0.4, 3, 4), meter(0.6, 7, 8)],
    );
    let timeline = timeline_at(&map, origin(1000, 0.0, 1, 1, 0.0, 0.9, 51));
    for size in [1u32, 7, 127, 512, 4096, 65536] {
        let mut cursor: i64 = 0;
        while cursor < 200000 {
            let piece = span_at(&timeline, cursor, (size as i64).min(200000 - cursor) as u32, 257);
            assert!(piece.frames > 0 && piece.frames <= size);
            let first = &piece.context;
            let last = context_at(&timeline, cursor + piece.frames as i64 - 1, 257);
            assert!(
                first.bpm == last.bpm
                    && first.numerator == last.numerator
                    && first.denominator == last.denominator
                    && first.bar_start_beat == last.bar_start_beat
                    && first.loop_iteration == last.loop_iteration
                    && ((first.flags ^ last.flags) & ST_CLOCK_PREROLL) == 0
            );
            let expected = first.beat + (piece.frames - 1) as f64 * first.bpm / (60.0 * first.sample_rate);
            assert!((last.beat - expected).abs() < 2e-9);
            cursor += piece.frames as i64;
        }
    }
    zero_query_allocations();
}

#[test]
fn round_trips_and_render_queries() {
    let (mut tempos, mut meters) = (Vec::new(), Vec::new());
    for n in 0..1024 {
        tempos.push(tempo(n as f64 * 900.0, 20.0 + (n % 381) as f64));
        meters.push(meter(
            n as f64 * 900.0,
            1 + n % 16,
            if n % 4 == 0 {
                2
            } else if n % 4 == 1 {
                4
            } else if n % 4 == 2 {
                8
            } else {
                16
            },
        ));
    }
    let map = map_with(&tempos, &meters, 384000.0, 8);
    let timeline = timeline_at(&map, origin(0, 0.0, 1, 1, 0.0, 1000000.0, 99));
    let mut random: u64 = 0x8715692;
    realtime(|| {
        for _ in 0..30000 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let original = (random >> 11) as f64 / 9007199254740992.0 * 1000000.0;
            let sample = frame(&map, original);
            let after = beat(&map, sample);
            assert!(after >= original && after - original <= 400.0 / (60.0 * 384000.0) + 2e-9);
            let sample_again = frame(&map, after);
            assert!(sample_again == sample);
            let seconds = map.seconds_at_beat(original);
            assert!(seconds.is_some());
            assert!(sample as f64 / 384000.0 - seconds.unwrap() < 1.0 / 384000.0 + 2e-9);
            let c = context(&timeline, sample);
            assert!(c.revision == 8);
            let range = span(&timeline, sample, 4096);
            assert!(range.frames > 0);
        }
    });
    // Two concurrent readers of the same immutable timeline; each reports the allocations its
    // own queries made.
    let reader = || {
        realtime(|| {
            for n in 0..100000i64 {
                let c = context_at(&timeline, n * 13, 64);
                assert!(c.epoch == 99);
                let range = span_at(&timeline, n * 13, 127, 64);
                assert!(range.frames > 0);
            }
        });
        render_allocations()
    };
    let (a, b) = std::thread::scope(|scope| {
        let (a, b) = (scope.spawn(reader), scope.spawn(reader));
        (a.join().expect("reader"), b.join().expect("reader"))
    });
    assert!(a + b + render_allocations() == 0);
    zero_query_allocations();
}

#[test]
fn quarter_beat_and_loop_context() {
    let map = map_48k(&[tempo(0.0, 120.0), tempo(0.375, 60.0)], &[meter(0.0, 4, 4), meter(0.375, 5, 8)]);
    let timeline = default_timeline(&map);
    assert!(context(&timeline, 0).next_quarter_beat_frame == 0);
    assert!(context(&timeline, 1).next_quarter_beat_frame == 39000);
    assert!(context(&timeline, 7500).next_quarter_beat_frame == 39000);
    assert!(context(&timeline, 39000).next_quarter_beat_frame == 39000);
    assert!(context(&timeline, -1).next_quarter_beat_frame == 0);
    let looped = timeline_at(&map, origin(100, 0.5, 1, 1, 0.25, 1.25, 4));
    let first = context(&looped, 100);
    assert!(first.loop_start_beat == 0.25 && first.loop_end_beat == 1.25 && first.next_quarter_beat_frame == 24100);
    let near = context(&looped, 36099);
    assert!(near.next_quarter_beat_frame == 69100);
    let wrapped = context(&looped, 36100);
    assert!(wrapped.loop_iteration == 1 && wrapped.next_quarter_beat_frame == 69100);
    let empty = timeline_at(&map, origin(0, 0.1, 1, 1, 0.1, 0.2, 5));
    assert!(context(&empty, 0).next_quarter_beat_frame == i64::MAX);
    let stopped = timeline_at(&map, origin(0, 0.2, 0, 0, 0.0, 0.0, 6));
    assert!(context(&stopped, 0).next_quarter_beat_frame == i64::MAX);
    let fractional = map_with(&[tempo(0.0, 123.0)], &[meter(0.0, 4, 4)], 48000.0, 1);
    let precise = default_timeline(&fractional);
    let one = frame(&fractional, 1.0);
    assert!(context(&precise, one).next_quarter_beat_frame == one);
    zero_query_allocations();
}

#[test]
fn exact_frame_inverse_after_tempo_change() {
    let map = map_48k(&[tempo(0.0, 120.0), tempo(0.5, 60.0), tempo(1.5, 180.0)], &[meter(0.0, 4, 4), meter(0.75, 3, 4)]);
    let timeline = default_timeline(&map);
    let expected: [i64; 9] = [0, 6000, 12000, 24000, 36000, 48000, 60000, 64000, 68000];
    for n in 0..9 {
        let position = n as f64 * 0.25;
        let sample = expected[n];
        assert!(frame(&map, position) == sample && beat(&map, sample) == position);
        if n != 0 {
            // No epsilon snapping: the immediately later representable beat
            // must still wait until the next sample, unlike the exact boundary.
            assert!(frame(&map, position.next_up()) == sample + 1);
            assert!(frame(&map, position.next_down()) == sample);
            assert!(beat(&map, sample - 1) < position);
        }
    }
    for sample in 63980..=64020i64 {
        assert!(frame(&map, beat(&map, sample)) == sample);
    }
    let at = context(&timeline, 64000);
    assert!(at.beat == 1.75 && at.bpm == 180.0 && at.next_quarter_beat_frame == 68000);
    zero_query_allocations();
}

#[test]
fn projected_quarter_grid() {
    let map = map_48k(&[tempo(0.0, 120.0), tempo(0.375, 60.0)], &[meter(0.0, 4, 4)]);
    let stopped = timeline_at(&map, origin(100, 0.25, 0, 0, 0.0, 0.0, 1));
    // Beat1 is at39000 samples, so the held .25 cursor is33000 samples from
    // that grid line. A latest-BPM guess would incorrectly return18000.
    for graph in [-1000i64, 100, 8192] {
        let c = context_at(&stopped, graph, 64);
        assert!(c.beat == 0.25 && c.song_frame == 6000);
        assert!(c.next_quarter_beat_frame == i64::MAX);
        assert!(c.next_quarter_grid_frame == graph + 33000 && (c.flags & ST_CLOCK_GRID_PROJECTION) != 0);
        assert!((c.flags & (ST_CLOCK_PLAYING | ST_CLOCK_RECORDING)) == 0);
    }
    let integer = timeline_at(&map, origin(0, 0.0, 0, 0, 0.0, 0.0, 2));
    assert!(context(&integer, 712).next_quarter_grid_frame == 712);
    let tiny = timeline_at(&map, origin(0, 0.1, 1, 1, 0.1, 0.2, 3));
    for graph in [0i64, 17, 2399, 2400, 4800] {
        let c = context(&tiny, graph);
        assert!(c.next_quarter_beat_frame == i64::MAX);
        assert!(c.next_quarter_grid_frame == graph + 39000 - c.song_frame);
        assert!(c.next_quarter_grid_frame > graph && (c.flags & ST_CLOCK_GRID_PROJECTION) != 0);
    }
    // Reachable next-cycle beats keep their existing transport meaning, even
    // when the separate unwrapped-grid distance is different.
    let reachable = timeline_at(&map, origin(0, 1.125, 1, 1, 0.25, 1.25, 4));
    let c = context(&reachable, 0);
    assert!(c.next_quarter_beat_frame == 39000 && c.next_quarter_grid_frame == 42000);
    assert!((c.flags & ST_CLOCK_GRID_PROJECTION) == 0);
    zero_query_allocations();
}

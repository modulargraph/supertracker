//! Unit tests for the clock.
//!
//! `tests/clock.rs` is the acceptance test. These tests cover what it leaves out: every creation
//! error and its text, domain limits, non-finite input and the query errors.

use super::*;

const TEMPO: [ClockTempoEvent; 1] = [ClockTempoEvent {
    beat: 0.0,
    bpm: 120.0,
}];
const METER: [ClockMeterEvent; 1] = [ClockMeterEvent {
    beat: 0.0,
    numerator: 4,
    denominator: 4,
}];
const PLAYING: ClockOrigin = ClockOrigin {
    graph_origin_frame: 0,
    start_beat: 0.0,
    playing: 1,
    loop_enabled: 0,
    loop_start_beat: 0.0,
    loop_end_beat: 0.0,
    epoch: 1,
};

fn tempo(beat: f64, bpm: f64) -> ClockTempoEvent {
    ClockTempoEvent { beat, bpm }
}

fn meter(beat: f64, numerator: i32, denominator: i32) -> ClockMeterEvent {
    ClockMeterEvent {
        beat,
        numerator,
        denominator,
    }
}

fn plain_map() -> ClockMap {
    ClockMap::new(48000.0, &TEMPO, &METER, 1).expect("valid map")
}

fn map_error(rate: f64, tempos: &[ClockTempoEvent], meters: &[ClockMeterEvent]) -> ClockError {
    ClockMap::new(rate, tempos, meters, 0)
        .err()
        .expect("invalid map")
}

fn timeline_error(map: &ClockMap, origin: ClockOrigin) -> ClockError {
    ClockTimeline::new(map, origin)
        .err()
        .expect("invalid timeline")
}

#[test]
fn maps_and_timelines_are_shareable_between_threads() {
    fn shareable<T: Send + Sync>() {}
    shareable::<ClockMap>();
    shareable::<ClockTimeline>();
}

#[test]
fn map_creation_reports_each_error() {
    for rate in [f64::NAN, f64::INFINITY, -48000.0, 0.0, 7999.999, 384000.5] {
        assert_eq!(map_error(rate, &TEMPO, &METER), ClockError::SampleRate);
    }
    assert!(ClockMap::new(8000.0, &TEMPO, &METER, 0).is_ok());
    assert!(ClockMap::new(384000.0, &TEMPO, &METER, 0).is_ok());
    // The sample rate is reported before anything about the events.
    assert_eq!(map_error(1.0, &[], &[]), ClockError::SampleRate);
    assert_eq!(map_error(48000.0, &[], &METER), ClockError::EventCount);
    assert_eq!(map_error(48000.0, &TEMPO, &[]), ClockError::EventCount);
    assert_eq!(
        map_error(48000.0, &[tempo(0.0, 120.0); 1025], &METER),
        ClockError::EventCount
    );
    assert_eq!(
        map_error(48000.0, &TEMPO, &[meter(0.0, 4, 4); 1025]),
        ClockError::EventCount
    );
    assert_eq!(
        map_error(48000.0, &[tempo(0.0, f64::NAN)], &METER),
        ClockError::Map(TimingMapError::Tempo)
    );
    assert_eq!(
        map_error(48000.0, &TEMPO, &[meter(0.0, 5, 3)]),
        ClockError::Map(TimingMapError::Meter)
    );
    assert_eq!(
        map_error(48000.0, &[tempo(1.0, 120.0)], &METER),
        ClockError::Map(TimingMapError::FirstPosition)
    );
    assert_eq!(
        map_error(
            48000.0,
            &[tempo(0.0, 120.0), tempo(f64::NAN, 120.0)],
            &METER
        ),
        ClockError::Map(TimingMapError::Positions)
    );
    // Distinct in seconds, but on the same sample: tempo against tempo and tempo against meter.
    assert_eq!(
        map_error(
            48000.0,
            &[tempo(0.0, 120.0), tempo(1e-8, 100.0), tempo(2e-8, 80.0)],
            &METER
        ),
        ClockError::BoundariesCollapse
    );
    assert_eq!(
        map_error(
            48000.0,
            &[tempo(0.0, 120.0), tempo(1.0, 100.0)],
            &[meter(0.0, 4, 4), meter(1.0 - 1e-9, 3, 4)]
        ),
        ClockError::BoundariesCollapse
    );
    assert_eq!(
        ClockError::Map(TimingMapError::Meter).to_string(),
        "Meter requires numerator 1..16 and denominator 2, 4, 8 or 16."
    );
    assert!(ClockError::BoundariesCollapse
        .message()
        .contains("collapse"));
    assert!(ClockError::LoopCollapse.message().contains("collapse"));
}

#[test]
fn conversions_cover_the_closed_domain_only() {
    let map = plain_map();
    let end = map.frame_at_beat(1_000_000.0).unwrap();
    assert_eq!(end, 24_000_000_000);
    assert_eq!(map.beat_at_frame(end), Some(1_000_000.0));
    assert_eq!(map.beat_at_frame(end + 1), None);
    assert_eq!(
        map.beat_at_frame(end - 1),
        Some(1_000_000.0 - 1.0 / 24000.0)
    );
    for bad in [
        f64::NAN,
        f64::INFINITY,
        -f64::INFINITY,
        -1e-300,
        1_000_000.000_000_1,
    ] {
        assert_eq!(map.frame_at_beat(bad), None);
        assert_eq!(map.seconds_at_beat(bad), None);
    }
    assert_eq!(map.frame_at_beat(-0.0), Some(0));
    assert_eq!(map.beat_at_seconds(-1.0), None);
    assert_eq!(map.beat_at_seconds(f64::NAN), None);
    assert_eq!(map.beat_at_seconds(500_000.0), Some(1_000_000.0));
    assert_eq!(map.seconds_at_beat(1_000_000.0), Some(500_000.0));
    // Negative frames are initial-tempo preroll, down to the graph limit and no further.
    assert_eq!(map.beat_at_frame(-24000), Some(-1.0));
    assert_eq!(
        map.beat_at_frame(-(1 << 52)),
        Some(-((1u64 << 52) as f64) / 24000.0)
    );
    assert_eq!(map.beat_at_frame(-(1 << 52) - 1), None);
    assert_eq!(map.beat_at_frame(i64::MIN), None);
    assert_eq!(map.beat_at_frame(i64::MAX), None);
    // No epsilon snapping: the next representable beat waits for the next sample.
    assert_eq!(map.frame_at_beat(0.25), Some(6000));
    assert_eq!(map.frame_at_beat(0.25f64.next_up()), Some(6001));
    assert_eq!(map.frame_at_beat(0.25f64.next_down()), Some(6000));
}

#[test]
fn timeline_creation_reports_each_error() {
    let map = plain_map();
    for frame in [(1 << 52) + 1, -(1 << 52) - 1, i64::MAX, i64::MIN] {
        assert_eq!(
            timeline_error(
                &map,
                ClockOrigin {
                    graph_origin_frame: frame,
                    ..PLAYING
                }
            ),
            ClockError::Origin
        );
    }
    for beat in [f64::NAN, f64::INFINITY, -1.0, 1_000_000.5] {
        assert_eq!(
            timeline_error(
                &map,
                ClockOrigin {
                    start_beat: beat,
                    ..PLAYING
                }
            ),
            ClockError::Origin
        );
    }
    let looping = |start: f64, end: f64, seek: f64| ClockOrigin {
        loop_enabled: 1,
        loop_start_beat: start,
        loop_end_beat: end,
        start_beat: seek,
        ..PLAYING
    };
    for (start, end, seek) in [
        (1.0, 1.0, 0.0),
        (2.0, 1.0, 0.0),
        (f64::NAN, 4.0, 0.0),
        (0.0, f64::NAN, 0.0),
        (0.0, 1_000_000.5, 0.0),
        (-1.0, 4.0, 0.0),
        (0.0, 4.0, 4.0),
        (0.0, 4.0, 5.0),
    ] {
        assert_eq!(
            timeline_error(&map, looping(start, end, seek)),
            ClockError::Loop,
            "{start}..{end} from {seek}"
        );
    }
    assert_eq!(
        timeline_error(&map, looping(1e-8, 2e-8, 0.0)),
        ClockError::LoopCollapse
    );
    assert_eq!(
        timeline_error(&map, looping(0.0, 1.0, 1.0 - 1e-9)),
        ClockError::LoopCollapse
    );
    // Loop beats are ignored, whatever they are, while looping is off.
    let ignored = ClockOrigin {
        loop_start_beat: f64::NAN,
        loop_end_beat: -5.0,
        ..PLAYING
    };
    let context = ClockTimeline::new(&map, ignored)
        .unwrap()
        .context(0, 0)
        .unwrap();
    assert_eq!((context.loop_start_beat, context.loop_end_beat), (0.0, 0.0));
    // The whole domain may loop, and both graph limits are valid origins.
    assert!(ClockTimeline::new(&map, looping(0.0, 1_000_000.0, 0.0)).is_ok());
    assert!(ClockTimeline::new(
        &map,
        ClockOrigin {
            graph_origin_frame: 1 << 52,
            ..PLAYING
        }
    )
    .is_ok());
    assert!(ClockTimeline::new(
        &map,
        ClockOrigin {
            graph_origin_frame: -(1 << 52),
            ..PLAYING
        }
    )
    .is_ok());
}

#[test]
fn any_nonzero_flag_means_true() {
    let map = plain_map();
    let origin = ClockOrigin {
        playing: -7,
        loop_enabled: 1 << 20,
        loop_start_beat: 0.0,
        loop_end_beat: 4.0,
        ..PLAYING
    };
    let timeline = ClockTimeline::new(&map, origin).unwrap();
    let context = timeline.context(96000 + 10, 0).unwrap();
    assert_eq!(context.flags, CLOCK_PLAYING | CLOCK_LOOPING);
    assert_eq!((context.loop_iteration, context.song_frame), (1, 10));
}

#[test]
fn context_validates_latency_before_the_frame() {
    let map = plain_map();
    let timeline = ClockTimeline::new(&map, PLAYING).unwrap();
    let limit = 1i64 << 52;
    for latency in [-1, (1 << 30) + 1, i64::MIN, i64::MAX] {
        assert_eq!(timeline.context(0, latency), Err(ClockQueryError::Invalid));
        // A bad latency wins over a bad frame.
        assert_eq!(
            timeline.context(i64::MAX, latency),
            Err(ClockQueryError::Invalid)
        );
    }
    for frame in [limit + 1, -limit - 1, i64::MAX, i64::MIN] {
        assert_eq!(
            timeline.context(frame, 0),
            Err(ClockQueryError::OutsideDomain)
        );
        assert_eq!(
            timeline.span(frame, 16, 0),
            Err(ClockQueryError::OutsideDomain)
        );
    }
    for requested in [0, 65537, u32::MAX] {
        assert_eq!(
            timeline.span(0, requested, 0),
            Err(ClockQueryError::Invalid)
        );
        // A bad request size wins over a bad frame.
        assert_eq!(
            timeline.span(i64::MAX, requested, 0),
            Err(ClockQueryError::Invalid)
        );
    }
    assert_eq!(ClockQueryError::Invalid.status(), CLOCK_INVALID);
    assert_eq!(
        ClockQueryError::OutsideDomain.status(),
        CLOCK_OUTSIDE_DOMAIN
    );
    // Both limits are inside the domain; the far preroll is finite and still has a boundary.
    let far = timeline.context(-limit, 1 << 30).unwrap();
    assert_eq!(far.content_graph_frame, -limit - (1 << 30));
    assert_eq!(far.song_frame, far.content_graph_frame);
    assert!(far.beat.is_finite() && far.bar < 0 && far.next_boundary_frame > far.graph_frame);
    assert_eq!(far.flags, CLOCK_PLAYING | CLOCK_PREROLL);
    // The upper graph limit is a valid query; here it lies beyond the end of the song.
    assert_eq!(
        timeline.context(limit, 0),
        Err(ClockQueryError::OutsideDomain)
    );
    let late = ClockTimeline::new(
        &map,
        ClockOrigin {
            graph_origin_frame: limit,
            ..PLAYING
        },
    )
    .unwrap();
    assert_eq!(late.context(limit, 0).unwrap().song_frame, 0);
}

#[test]
fn spans_stop_at_the_graph_limit_and_the_domain_end() {
    let map = plain_map();
    let limit = 1i64 << 52;
    // Stopped: nothing ends a span except the end of graph time itself.
    let stopped = ClockTimeline::new(
        &map,
        ClockOrigin {
            playing: 0,
            start_beat: 3.125,
            ..PLAYING
        },
    )
    .unwrap();
    assert_eq!(stopped.span(limit, 4096, 0).unwrap().frames, 1);
    assert_eq!(stopped.span(limit - 9, 4096, 0).unwrap().frames, 10);
    assert_eq!(stopped.span(-limit, 65536, 0).unwrap().frames, 65536);
    let held = stopped.context(limit, 0).unwrap();
    assert_eq!(
        (held.beat, held.song_frame, held.next_boundary_frame),
        (3.125, 75000, i64::MAX)
    );
    assert_eq!(held.flags, CLOCK_GRID_PROJECTION);
    assert_eq!(held.next_quarter_grid_frame, limit + 21000);
    // Playing into the end of the map: the last span is empty and flagged.
    let end = map.frame_at_beat(1_000_000.0).unwrap();
    let playing = ClockTimeline::new(&map, PLAYING).unwrap();
    let last = playing.span(end - 3, 4096, 0).unwrap();
    assert_eq!((last.frames, last.context.next_boundary_frame), (3, end));
    assert_ne!(
        last.context.next_boundary_kinds & CLOCK_BOUNDARY_DOMAIN,
        0
    );
    let at_end = playing.span(end, 4096, 0).unwrap();
    assert_eq!(at_end.frames, 0);
    assert_eq!(at_end.context.beat, 1_000_000.0);
    assert_ne!(at_end.context.flags & CLOCK_DOMAIN_END, 0);
    assert_eq!(
        playing.context(end + 1, 0),
        Err(ClockQueryError::OutsideDomain)
    );
    // A seek to the very end is valid, stopped or playing.
    let parked = ClockTimeline::new(
        &map,
        ClockOrigin {
            playing: 0,
            start_beat: 1_000_000.0,
            ..PLAYING
        },
    )
    .unwrap();
    let context = parked.context(12345, 0).unwrap();
    assert_eq!((context.song_frame, context.beat), (end, 1_000_000.0));
    assert_eq!(
        context.flags,
        CLOCK_DOMAIN_END | CLOCK_GRID_PROJECTION
    );
    assert_eq!(context.next_quarter_grid_frame, 12345);
}

#[test]
fn an_event_at_the_end_of_the_domain_is_a_boundary_on_the_last_sample() {
    let map = ClockMap::new(
        48000.0,
        &[tempo(0.0, 120.0), tempo(1_000_000.0, 60.0)],
        &[meter(0.0, 4, 4), meter(1_000_000.0, 3, 8)],
        7,
    )
    .unwrap();
    let end = map.frame_at_beat(1_000_000.0).unwrap();
    let timeline = ClockTimeline::new(&map, PLAYING).unwrap();
    let before = timeline.context(end - 1, 0).unwrap();
    assert_eq!(
        (before.bpm, before.numerator, before.next_boundary_frame),
        (120.0, 4, end)
    );
    assert_eq!(
        before.next_boundary_kinds,
        CLOCK_BOUNDARY_TEMPO
            | CLOCK_BOUNDARY_METER
            | CLOCK_BOUNDARY_BAR
            | CLOCK_BOUNDARY_DOMAIN
    );
    let last = timeline.context(end, 0).unwrap();
    assert_eq!(
        (last.bpm, last.numerator, last.denominator, last.beat),
        (60.0, 3, 8, 1_000_000.0)
    );
    assert_eq!(last.revision, 7);
}

#[test]
fn a_timeline_outlives_its_map_and_queries_report_their_errors() {
    let map = plain_map();
    assert_eq!((map.frame_at_beat(f64::NAN), map.beat_at_frame(i64::MAX)), (None, None));
    assert_eq!((map.seconds_at_beat(-1.0), map.beat_at_seconds(f64::INFINITY)), (None, None));
    assert_eq!((map.frame_at_beat(1.0), map.beat_at_frame(12000)), (Some(24000), Some(0.5)));
    assert_eq!((map.seconds_at_beat(3.0), map.beat_at_seconds(1.5)), (Some(1.5), Some(3.0)));
    let timeline = ClockTimeline::new(&map, PLAYING).expect("valid timeline");
    // The timeline keeps the compiled map alive.
    drop(map);
    assert_eq!(timeline.context(0, -1), Err(ClockQueryError::Invalid));
    assert_eq!(timeline.context(i64::MAX, 0), Err(ClockQueryError::OutsideDomain));
    assert_eq!(timeline.span(0, 0, 0), Err(ClockQueryError::Invalid));
    assert_eq!(timeline.span(0, 65537, 0), Err(ClockQueryError::Invalid));
    assert_eq!(timeline.span(0, 16, (1 << 30) + 1), Err(ClockQueryError::Invalid));
    assert_eq!(timeline.span(i64::MIN, 16, 0), Err(ClockQueryError::OutsideDomain));
    assert_eq!((ClockQueryError::Invalid.status(), ClockQueryError::OutsideDomain.status()), (CLOCK_INVALID, CLOCK_OUTSIDE_DOMAIN));
    assert!(!ClockQueryError::Invalid.to_string().is_empty());
    let span = timeline.span(95990, 4096, 0).expect("a span before the bar line");
    assert_eq!((span.frames, span.context.bar, span.context.next_boundary_kinds), (10, 1, CLOCK_BOUNDARY_BAR));
    let context = timeline.context(96000, 0).expect("the bar line");
    assert_eq!((context.bar, context.beat, context.bpm, context.sample_rate), (2, 4.0, 120.0, 48000.0));
}

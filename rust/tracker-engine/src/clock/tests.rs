//! Unit tests for the clock's C API and its safe wrappers.
//!
//! `tests/clock_tests.cpp` remains the acceptance test of the C API and is run against this
//! library unchanged. These tests cover what it leaves out: null and capacity handling of every
//! entry point, error texts, domain limits, non-finite input and the header's layout.

use std::ffi::{c_char, CStr};
use std::mem::{align_of, offset_of, size_of, size_of_val};
use std::ptr::{null, null_mut};

use super::*;

const TEMPO: [STClockTempoEvent; 1] = [STClockTempoEvent {
    beat: 0.0,
    bpm: 120.0,
}];
const METER: [STClockMeterEvent; 1] = [STClockMeterEvent {
    beat: 0.0,
    numerator: 4,
    denominator: 4,
}];
const PLAYING: STClockOrigin = STClockOrigin {
    graph_origin_frame: 0,
    start_beat: 0.0,
    playing: 1,
    loop_enabled: 0,
    loop_start_beat: 0.0,
    loop_end_beat: 0.0,
    epoch: 1,
};

fn tempo(beat: f64, bpm: f64) -> STClockTempoEvent {
    STClockTempoEvent { beat, bpm }
}

fn meter(beat: f64, numerator: i32, denominator: i32) -> STClockMeterEvent {
    STClockMeterEvent {
        beat,
        numerator,
        denominator,
    }
}

fn plain_map() -> STClockMap {
    STClockMap::new(48000.0, &TEMPO, &METER, 1).expect("valid map")
}

fn map_error(rate: f64, tempos: &[STClockTempoEvent], meters: &[STClockMeterEvent]) -> ClockError {
    STClockMap::new(rate, tempos, meters, 0)
        .err()
        .expect("invalid map")
}

fn timeline_error(map: &STClockMap, origin: STClockOrigin) -> ClockError {
    STClockTimeline::new(map, origin)
        .err()
        .expect("invalid timeline")
}

/// The output of a C++ program printing `sizeof`, `alignof` and `offsetof` for every struct of
/// `TrackerClock.h` (identical for x86-64 and arm64).
const CXX_LAYOUT: &str = "\
STClockTempoEvent size 16 align 8
STClockTempoEvent.beat offset 0 size 8
STClockTempoEvent.bpm offset 8 size 8
STClockMeterEvent size 16 align 8
STClockMeterEvent.beat offset 0 size 8
STClockMeterEvent.numerator offset 8 size 4
STClockMeterEvent.denominator offset 12 size 4
STClockOrigin size 48 align 8
STClockOrigin.graphOriginFrame offset 0 size 8
STClockOrigin.startBeat offset 8 size 8
STClockOrigin.playing offset 16 size 4
STClockOrigin.loopEnabled offset 20 size 4
STClockOrigin.loopStartBeat offset 24 size 8
STClockOrigin.loopEndBeat offset 32 size 8
STClockOrigin.epoch offset 40 size 8
STClockContext size 160 align 8
STClockContext.revision offset 0 size 8
STClockContext.epoch offset 8 size 8
STClockContext.graphFrame offset 16 size 8
STClockContext.contentGraphFrame offset 24 size 8
STClockContext.songFrame offset 32 size 8
STClockContext.loopIteration offset 40 size 8
STClockContext.bar offset 48 size 8
STClockContext.sampleRate offset 56 size 8
STClockContext.beat offset 64 size 8
STClockContext.bpm offset 72 size 8
STClockContext.barStartBeat offset 80 size 8
STClockContext.barEndBeat offset 88 size 8
STClockContext.numerator offset 96 size 4
STClockContext.denominator offset 100 size 4
STClockContext.flags offset 104 size 4
STClockContext.nextBoundaryFrame offset 112 size 8
STClockContext.nextBoundaryKinds offset 120 size 4
STClockContext.loopStartBeat offset 128 size 8
STClockContext.loopEndBeat offset 136 size 8
STClockContext.nextQuarterBeatFrame offset 144 size 8
STClockContext.nextQuarterGridFrame offset 152 size 8
STClockSpan size 168 align 8
STClockSpan.context offset 0 size 160
STClockSpan.frames offset 160 size 4
constants 1 0 -1 | 1 2 4 8 16 32 | 1 2 4 8 16 32 | 1024 65536
";

#[test]
fn layout_matches_the_c_header() {
    let mut report = String::new();
    macro_rules! layout {
        ($type:ident { $($c_name:literal => $field:ident),+ $(,)? }) => {{
            let name = stringify!($type);
            report += &format!("{name} size {} align {}\n", size_of::<$type>(), align_of::<$type>());
            let value = $type::default();
            $(report += &format!(
                "{name}.{} offset {} size {}\n", $c_name, offset_of!($type, $field), size_of_val(&value.$field)
            );)+
        }};
    }
    layout!(STClockTempoEvent { "beat" => beat, "bpm" => bpm });
    layout!(STClockMeterEvent { "beat" => beat, "numerator" => numerator, "denominator" => denominator });
    layout!(STClockOrigin {
        "graphOriginFrame" => graph_origin_frame,
        "startBeat" => start_beat,
        "playing" => playing,
        "loopEnabled" => loop_enabled,
        "loopStartBeat" => loop_start_beat,
        "loopEndBeat" => loop_end_beat,
        "epoch" => epoch,
    });
    layout!(STClockContext {
        "revision" => revision,
        "epoch" => epoch,
        "graphFrame" => graph_frame,
        "contentGraphFrame" => content_graph_frame,
        "songFrame" => song_frame,
        "loopIteration" => loop_iteration,
        "bar" => bar,
        "sampleRate" => sample_rate,
        "beat" => beat,
        "bpm" => bpm,
        "barStartBeat" => bar_start_beat,
        "barEndBeat" => bar_end_beat,
        "numerator" => numerator,
        "denominator" => denominator,
        "flags" => flags,
        "nextBoundaryFrame" => next_boundary_frame,
        "nextBoundaryKinds" => next_boundary_kinds,
        "loopStartBeat" => loop_start_beat,
        "loopEndBeat" => loop_end_beat,
        "nextQuarterBeatFrame" => next_quarter_beat_frame,
        "nextQuarterGridFrame" => next_quarter_grid_frame,
    });
    layout!(STClockSpan { "context" => context, "frames" => frames });
    report += &format!(
        "constants {ST_CLOCK_OK} {ST_CLOCK_INVALID} {ST_CLOCK_OUTSIDE_DOMAIN} | {ST_CLOCK_PLAYING} {ST_CLOCK_LOOPING} \
         {ST_CLOCK_PREROLL} {ST_CLOCK_DOMAIN_END} {ST_CLOCK_RECORDING} {ST_CLOCK_GRID_PROJECTION} | \
         {ST_CLOCK_BOUNDARY_TEMPO} {ST_CLOCK_BOUNDARY_METER} {ST_CLOCK_BOUNDARY_BAR} {ST_CLOCK_BOUNDARY_LOOP} \
         {ST_CLOCK_BOUNDARY_ORIGIN} {ST_CLOCK_BOUNDARY_DOMAIN} | {ST_CLOCK_MAX_EVENTS} {ST_CLOCK_MAX_SPAN}\n"
    );
    print!("{report}");
    assert_eq!(report, CXX_LAYOUT);
}

#[test]
fn maps_and_timelines_are_shareable_between_threads() {
    fn shareable<T: Send + Sync>() {}
    shareable::<STClockMap>();
    shareable::<STClockTimeline>();
}

#[test]
fn map_creation_reports_each_error() {
    for rate in [f64::NAN, f64::INFINITY, -48000.0, 0.0, 7999.999, 384000.5] {
        assert_eq!(map_error(rate, &TEMPO, &METER), ClockError::SampleRate);
    }
    assert!(STClockMap::new(8000.0, &TEMPO, &METER, 0).is_ok());
    assert!(STClockMap::new(384000.0, &TEMPO, &METER, 0).is_ok());
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
                STClockOrigin {
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
                STClockOrigin {
                    start_beat: beat,
                    ..PLAYING
                }
            ),
            ClockError::Origin
        );
    }
    let looping = |start: f64, end: f64, seek: f64| STClockOrigin {
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
    let ignored = STClockOrigin {
        loop_start_beat: f64::NAN,
        loop_end_beat: -5.0,
        ..PLAYING
    };
    let context = STClockTimeline::new(&map, ignored)
        .unwrap()
        .context(0, 0)
        .unwrap();
    assert_eq!((context.loop_start_beat, context.loop_end_beat), (0.0, 0.0));
    // The whole domain may loop, and both graph limits are valid origins.
    assert!(STClockTimeline::new(&map, looping(0.0, 1_000_000.0, 0.0)).is_ok());
    assert!(STClockTimeline::new(
        &map,
        STClockOrigin {
            graph_origin_frame: 1 << 52,
            ..PLAYING
        }
    )
    .is_ok());
    assert!(STClockTimeline::new(
        &map,
        STClockOrigin {
            graph_origin_frame: -(1 << 52),
            ..PLAYING
        }
    )
    .is_ok());
}

#[test]
fn any_nonzero_flag_means_true() {
    let map = plain_map();
    let origin = STClockOrigin {
        playing: -7,
        loop_enabled: 1 << 20,
        loop_start_beat: 0.0,
        loop_end_beat: 4.0,
        ..PLAYING
    };
    let timeline = STClockTimeline::new(&map, origin).unwrap();
    let context = timeline.context(96000 + 10, 0).unwrap();
    assert_eq!(context.flags, ST_CLOCK_PLAYING | ST_CLOCK_LOOPING);
    assert_eq!((context.loop_iteration, context.song_frame), (1, 10));
}

#[test]
fn context_validates_latency_before_the_frame() {
    let map = plain_map();
    let timeline = STClockTimeline::new(&map, PLAYING).unwrap();
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
    assert_eq!(ClockQueryError::Invalid.status(), ST_CLOCK_INVALID);
    assert_eq!(
        ClockQueryError::OutsideDomain.status(),
        ST_CLOCK_OUTSIDE_DOMAIN
    );
    // Both limits are inside the domain; the far preroll is finite and still has a boundary.
    let far = timeline.context(-limit, 1 << 30).unwrap();
    assert_eq!(far.content_graph_frame, -limit - (1 << 30));
    assert_eq!(far.song_frame, far.content_graph_frame);
    assert!(far.beat.is_finite() && far.bar < 0 && far.next_boundary_frame > far.graph_frame);
    assert_eq!(far.flags, ST_CLOCK_PLAYING | ST_CLOCK_PREROLL);
    // The upper graph limit is a valid query; here it lies beyond the end of the song.
    assert_eq!(
        timeline.context(limit, 0),
        Err(ClockQueryError::OutsideDomain)
    );
    let late = STClockTimeline::new(
        &map,
        STClockOrigin {
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
    let stopped = STClockTimeline::new(
        &map,
        STClockOrigin {
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
    assert_eq!(held.flags, ST_CLOCK_GRID_PROJECTION);
    assert_eq!(held.next_quarter_grid_frame, limit + 21000);
    // Playing into the end of the map: the last span is empty and flagged.
    let end = map.frame_at_beat(1_000_000.0).unwrap();
    let playing = STClockTimeline::new(&map, PLAYING).unwrap();
    let last = playing.span(end - 3, 4096, 0).unwrap();
    assert_eq!((last.frames, last.context.next_boundary_frame), (3, end));
    assert_ne!(
        last.context.next_boundary_kinds & ST_CLOCK_BOUNDARY_DOMAIN,
        0
    );
    let at_end = playing.span(end, 4096, 0).unwrap();
    assert_eq!(at_end.frames, 0);
    assert_eq!(at_end.context.beat, 1_000_000.0);
    assert_ne!(at_end.context.flags & ST_CLOCK_DOMAIN_END, 0);
    assert_eq!(
        playing.context(end + 1, 0),
        Err(ClockQueryError::OutsideDomain)
    );
    // A seek to the very end is valid, stopped or playing.
    let parked = STClockTimeline::new(
        &map,
        STClockOrigin {
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
        ST_CLOCK_DOMAIN_END | ST_CLOCK_GRID_PROJECTION
    );
    assert_eq!(context.next_quarter_grid_frame, 12345);
}

#[test]
fn an_event_at_the_end_of_the_domain_is_a_boundary_on_the_last_sample() {
    let map = STClockMap::new(
        48000.0,
        &[tempo(0.0, 120.0), tempo(1_000_000.0, 60.0)],
        &[meter(0.0, 4, 4), meter(1_000_000.0, 3, 8)],
        7,
    )
    .unwrap();
    let end = map.frame_at_beat(1_000_000.0).unwrap();
    let timeline = STClockTimeline::new(&map, PLAYING).unwrap();
    let before = timeline.context(end - 1, 0).unwrap();
    assert_eq!(
        (before.bpm, before.numerator, before.next_boundary_frame),
        (120.0, 4, end)
    );
    assert_eq!(
        before.next_boundary_kinds,
        ST_CLOCK_BOUNDARY_TEMPO
            | ST_CLOCK_BOUNDARY_METER
            | ST_CLOCK_BOUNDARY_BAR
            | ST_CLOCK_BOUNDARY_DOMAIN
    );
    let last = timeline.context(end, 0).unwrap();
    assert_eq!(
        (last.bpm, last.numerator, last.denominator, last.beat),
        (60.0, 3, 8, 1_000_000.0)
    );
    assert_eq!(last.revision, 7);
}

fn text(buffer: &[u8]) -> &str {
    CStr::from_bytes_until_nul(buffer)
        .expect("terminated")
        .to_str()
        .expect("ASCII")
}

#[test]
fn c_api_handles_null_pointers_and_error_capacity() {
    // SAFETY: every pointer is null or addresses live local storage of the stated size.
    unsafe {
        let mut error = [b'x'; 256];
        let capacity = error.len() as u32;
        let message = error.as_mut_ptr().cast::<c_char>();
        let create =
            |tempos: *const STClockTempoEvent, count: u32, meters: *const STClockMeterEvent| {
                st_clock_map_create(48000.0, tempos, count, meters, 1, 0, message, capacity)
            };
        assert!(create(null(), 1, METER.as_ptr()).is_null());
        assert_eq!(
            text(&error),
            "Provide 1..1024 tempo and meter events, beginning at quarter beat zero."
        );
        assert!(create(TEMPO.as_ptr(), 1, null()).is_null());
        assert!(create(TEMPO.as_ptr(), 0, METER.as_ptr()).is_null());
        assert!(create(TEMPO.as_ptr(), 1025, METER.as_ptr()).is_null());
        assert_eq!(
            text(&error),
            "Provide 1..1024 tempo and meter events, beginning at quarter beat zero."
        );
        // A bad sample rate is reported even when the event pointers are null.
        assert!(
            st_clock_map_create(f64::NAN, null(), 0, null(), 0, 0, message, capacity).is_null()
        );
        assert_eq!(
            text(&error),
            "Sample rate must be finite and within 8000..384000 Hz."
        );
        // The text is truncated to the capacity and always terminated; storage beyond is untouched.
        let mut small = [b'x'; 12];
        assert!(
            st_clock_map_create(1.0, null(), 0, null(), 0, 0, small.as_mut_ptr().cast(), 8)
                .is_null()
        );
        assert_eq!(&small, b"Sample \0xxxx");
        let mut tiny = [b'x'; 2];
        assert!(
            st_clock_map_create(1.0, null(), 0, null(), 0, 0, tiny.as_mut_ptr().cast(), 1)
                .is_null()
        );
        assert_eq!(&tiny, b"\0x");
        assert!(
            st_clock_map_create(1.0, null(), 0, null(), 0, 0, tiny.as_mut_ptr().cast(), 0)
                .is_null()
        );
        assert_eq!(&tiny, b"\0x");
        assert!(st_clock_map_create(1.0, null(), 0, null(), 0, 0, null_mut(), 99).is_null());

        // Success clears the error text.
        error.fill(b'x');
        let map = create(TEMPO.as_ptr(), 1, METER.as_ptr());
        assert!(!map.is_null());
        assert_eq!(text(&error), "");

        let (mut frame, mut beat, mut seconds) = (42i64, 42.0f64, 42.0f64);
        assert_eq!(
            st_clock_frame_at_beat(null(), 1.0, &mut frame),
            ST_CLOCK_INVALID
        );
        assert_eq!(
            st_clock_frame_at_beat(map, 1.0, null_mut()),
            ST_CLOCK_INVALID
        );
        assert_eq!(
            st_clock_frame_at_beat(map, f64::NAN, &mut frame),
            ST_CLOCK_OUTSIDE_DOMAIN
        );
        assert_eq!(
            st_clock_beat_at_frame(null(), 1, &mut beat),
            ST_CLOCK_INVALID
        );
        assert_eq!(st_clock_beat_at_frame(map, 1, null_mut()), ST_CLOCK_INVALID);
        assert_eq!(
            st_clock_beat_at_frame(map, i64::MAX, &mut beat),
            ST_CLOCK_OUTSIDE_DOMAIN
        );
        assert_eq!(
            st_clock_seconds_at_beat(null(), 1.0, &mut seconds),
            ST_CLOCK_INVALID
        );
        assert_eq!(
            st_clock_seconds_at_beat(map, 1.0, null_mut()),
            ST_CLOCK_INVALID
        );
        assert_eq!(
            st_clock_seconds_at_beat(map, -1.0, &mut seconds),
            ST_CLOCK_OUTSIDE_DOMAIN
        );
        assert_eq!(
            st_clock_beat_at_seconds(null(), 1.0, &mut beat),
            ST_CLOCK_INVALID
        );
        assert_eq!(
            st_clock_beat_at_seconds(map, 1.0, null_mut()),
            ST_CLOCK_INVALID
        );
        assert_eq!(
            st_clock_beat_at_seconds(map, f64::INFINITY, &mut beat),
            ST_CLOCK_OUTSIDE_DOMAIN
        );
        // On error, output values are left untouched.
        assert_eq!((frame, beat, seconds), (42, 42.0, 42.0));
        assert_eq!(st_clock_frame_at_beat(map, 1.0, &mut frame), ST_CLOCK_OK);
        assert_eq!(st_clock_beat_at_frame(map, 12000, &mut beat), ST_CLOCK_OK);
        assert_eq!(
            st_clock_seconds_at_beat(map, 3.0, &mut seconds),
            ST_CLOCK_OK
        );
        assert_eq!((frame, beat, seconds), (24000, 0.5, 1.5));
        assert_eq!(st_clock_beat_at_seconds(map, 1.5, &mut beat), ST_CLOCK_OK);
        assert_eq!(beat, 3.0);

        error.fill(b'x');
        assert!(st_clock_timeline_create(null(), PLAYING, message, capacity).is_null());
        assert_eq!(text(&error), "A timing map is required.");
        let bad_origin = STClockOrigin {
            start_beat: f64::NAN,
            ..PLAYING
        };
        assert!(st_clock_timeline_create(map, bad_origin, message, capacity).is_null());
        assert_eq!(
            text(&error),
            "Graph origin must be within +/-2^52 frames and seek beat within 0..1000000."
        );
        let bad_loop = STClockOrigin {
            loop_enabled: 1,
            ..PLAYING
        };
        assert!(st_clock_timeline_create(map, bad_loop, message, capacity).is_null());
        assert_eq!(
            text(&error),
            "Loop requires 0<=start<end<=1000000 and a seek position before its end."
        );
        let collapsed = STClockOrigin {
            loop_enabled: 1,
            loop_start_beat: 1e-8,
            loop_end_beat: 2e-8,
            ..PLAYING
        };
        assert!(st_clock_timeline_create(map, collapsed, null_mut(), 0).is_null());
        assert!(st_clock_timeline_create(map, collapsed, message, capacity).is_null());
        assert_eq!(
            text(&error),
            "Loop/seek positions collapse onto the same audio sample."
        );
        let timeline = st_clock_timeline_create(map, PLAYING, message, capacity);
        assert!(!timeline.is_null());
        assert_eq!(text(&error), "");
        // The timeline keeps the compiled map alive.
        st_clock_map_destroy(map);

        let untouched = STClockContext {
            beat: 123.0,
            ..STClockContext::default()
        };
        let mut context = untouched;
        assert_eq!(
            st_clock_context(null(), 0, 0, &mut context),
            ST_CLOCK_INVALID
        );
        assert_eq!(
            st_clock_context(timeline, 0, 0, null_mut()),
            ST_CLOCK_INVALID
        );
        assert_eq!(
            st_clock_context(timeline, 0, -1, &mut context),
            ST_CLOCK_INVALID
        );
        assert_eq!(
            st_clock_context(timeline, i64::MAX, 0, &mut context),
            ST_CLOCK_OUTSIDE_DOMAIN
        );
        assert_eq!(context, untouched);
        let mut span = STClockSpan {
            context: untouched,
            frames: 99,
        };
        assert_eq!(st_clock_span(null(), 0, 16, 0, &mut span), ST_CLOCK_INVALID);
        assert_eq!(
            st_clock_span(timeline, 0, 16, 0, null_mut()),
            ST_CLOCK_INVALID
        );
        assert_eq!(
            st_clock_span(timeline, 0, 0, 0, &mut span),
            ST_CLOCK_INVALID
        );
        assert_eq!(
            st_clock_span(timeline, 0, 65537, 0, &mut span),
            ST_CLOCK_INVALID
        );
        assert_eq!(
            st_clock_span(timeline, 0, 16, (1 << 30) + 1, &mut span),
            ST_CLOCK_INVALID
        );
        assert_eq!(
            st_clock_span(timeline, i64::MIN, 16, 0, &mut span),
            ST_CLOCK_OUTSIDE_DOMAIN
        );
        assert_eq!(
            span,
            STClockSpan {
                context: untouched,
                frames: 99
            }
        );
        assert_eq!(
            st_clock_span(timeline, 95990, 4096, 0, &mut span),
            ST_CLOCK_OK
        );
        assert_eq!(
            (
                span.frames,
                span.context.bar,
                span.context.next_boundary_kinds
            ),
            (10, 1, ST_CLOCK_BOUNDARY_BAR)
        );
        assert_eq!(
            st_clock_context(timeline, 96000, 0, &mut context),
            ST_CLOCK_OK
        );
        assert_eq!(
            (context.bar, context.beat, context.bpm, context.sample_rate),
            (2, 4.0, 120.0, 48000.0)
        );

        st_clock_timeline_destroy(timeline);
        st_clock_timeline_destroy(null_mut());
        st_clock_map_destroy(null_mut());
    }
}

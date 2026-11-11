//! `RenderHalf::block_song_frame`: the transport frame the first frame of a block plays.
//!
//! A host that records what it renders writes each block at this frame. The expected values
//! are those the engine gave before it had this query (through the clock history it had then),
//! for the same scenes.
#![cfg(feature = "engine")]

mod engine_support_b;

use engine_support_b::alloc::{realtime, render_allocations};
use engine_support_b::engine::Engine;

/// One track, one pattern of 4 rows in the song, 120 BPM at 4 lines per beat: a row lasts 6000
/// frames at 48 kHz and the song 24000.
fn fixture(loop_song: bool, swing: f32) -> Engine {
    let mut e = Engine::new();
    e.begin_update();
    e.set_track_count(1);
    e.set_timing(120.0, 4, swing, 1.0);
    e.set_pattern(0, 4);
    e.set_sequence(&[0], loop_song, 0);
    e.commit_update();
    e
}

/// Renders `count` blocks of `block` frames from `time` and collects the song frame of each.
fn blocks(e: &mut Engine, time: &mut u64, block: u32, count: usize) -> Vec<Option<i64>> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(realtime(|| {
            e.render_block(*time, block);
            e.block_song_frame()
        }));
        *time += block as u64;
    }
    out
}

fn some(frames: &[i64]) -> Vec<Option<i64>> {
    frames.iter().map(|frame| Some(*frame)).collect()
}

#[test]
fn a_stopped_transport_has_no_song_frame() {
    let mut e = fixture(false, 0.0);
    assert_eq!(e.block_song_frame(), None, "before the first block");
    let mut time = 1000;
    assert_eq!(blocks(&mut e, &mut time, 4096, 2), [None, None]);
    e.play(0);
    assert_eq!(blocks(&mut e, &mut time, 4096, 1), [Some(0)]);
    e.stop();
    assert_eq!(blocks(&mut e, &mut time, 4096, 2), [None, None]);
    assert_eq!(render_allocations(), 0);
}

/// Pattern mode counts on through the pattern's wraps (the frames since Play); the host's own
/// timestamps (here starting at 1000) do not enter.
#[test]
fn pattern_mode_counts_from_play_through_seek_and_tempo_change() {
    let mut e = fixture(false, 0.0);
    let mut time = 1000;
    e.play(0);
    assert_eq!(blocks(&mut e, &mut time, 4096, 8), some(&[0, 4096, 8192, 12288, 16384, 20480, 24576, 28672]));
    // A seek applies at the start of the next block: row 2 starts at frame 12000.
    e.seek(0, 2, 0);
    assert_eq!(blocks(&mut e, &mut time, 4096, 2), some(&[12000, 16096]));
    // Halving the tempo doubles the frame of the same musical position (20192 frames in).
    e.begin_update();
    e.set_timing(60.0, 4, 0.0, 1.0);
    e.commit_update();
    assert_eq!(blocks(&mut e, &mut time, 4096, 2), some(&[40384, 44480]));
    assert_eq!(render_allocations(), 0);
}

#[test]
fn a_looping_song_restarts_at_frame_zero() {
    let mut e = fixture(true, 0.0);
    let mut time = 0;
    e.play(1);
    // The song wraps at frame 24000, 576 frames before the seventh block starts.
    assert_eq!(blocks(&mut e, &mut time, 4096, 8), some(&[0, 4096, 8192, 12288, 16384, 20480, 576, 4672]));
    // A block that starts exactly on the wrap starts the song again.
    let mut e = fixture(true, 0.0);
    let mut time = 0;
    e.play(1);
    assert_eq!(blocks(&mut e, &mut time, 4000, 8), some(&[0, 4000, 8000, 12000, 16000, 20000, 0, 4000]));
    assert_eq!(render_allocations(), 0);
}

#[test]
fn a_song_that_ended_has_no_song_frame() {
    let mut e = fixture(false, 0.0);
    let mut time = 0;
    e.play(1);
    let frames = blocks(&mut e, &mut time, 4096, 8);
    assert_eq!(frames[..6], some(&[0, 4096, 8192, 12288, 16384, 20480]));
    assert_eq!(frames[6..], [None, None]);
    assert_eq!(render_allocations(), 0);
}

/// One bar of count-in at 120 BPM in 4/4 lasts 96000 frames; Play starts on the frame after it.
#[test]
fn a_count_in_has_no_song_frame_until_play_starts() {
    let mut e = fixture(false, 0.0);
    let mut time = 0;
    e.play_count_in(false, 1);
    let frames = blocks(&mut e, &mut time, 4000, 26);
    assert!(frames[..24].iter().all(Option::is_none));
    assert_eq!(frames[24..], some(&[0, 4000]), "the block that starts with Play");
    // Play starts inside a block: that block still began in the count-in.
    let mut e = fixture(false, 0.0);
    let mut time = 0;
    e.play_count_in(true, 1);
    let frames = blocks(&mut e, &mut time, 4096, 26);
    assert!(frames[..24].iter().all(Option::is_none));
    assert_eq!(frames[24..], some(&[2304, 6400]));
    assert_eq!(render_allocations(), 0);
}

/// With swing a row can start between two frames: the song frame is the whole frame below.
#[test]
fn a_fractional_start_is_truncated() {
    let mut e = fixture(false, 0.3);
    let mut time = 0;
    e.seek(0, 1, 0);
    e.play(0);
    assert_eq!(blocks(&mut e, &mut time, 4096, 2), some(&[7799, 11895]));
    assert_eq!(render_allocations(), 0);
}

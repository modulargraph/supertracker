//! A tap on a strip for a recording that must line up with the song: what the strip carries
//! after its inserts and before its fader, block by block, with each block's place in graph
//! time and in the song.
//!
//! ```text
//!  render thread (Plan::process, at the strip's fader)        the thread that records
//!  --------------------------------------------------        ------------------------
//!  StripTap::capture(block, signal)   ~~ two rings ~~>        StripTap::pop(left, right)
//! ```
//!
//! The render side copies the block's frames into a preallocated ring and its [`TapBlock`]
//! into a second one, and does nothing else: no allocation, no lock, no I/O. A silent block
//! sends its header only. When the reader falls behind and a ring is full, the block is
//! counted as dropped instead of waited for; the reader sees the hole in the blocks' times.
//!
//! [`TapBlock::arrival`] is how many frames the strip's signal lags the engine at the tap (the
//! delay compensation of the plan that rendered the block), and [`TapBlock::song_frame`] is
//! the song position of the engine's frame at the block's start. Together they say where in
//! the song every captured frame belongs, which is what Freeze by recording needs
//! ([`crate::freeze_recording`]).

use crate::recording::AudioRing;
use std::sync::atomic::{AtomicU64, Ordering};
use tracker_engine::engine::util::Ring;

/// [`TapBlock::song_frame`] of a block the transport did not play.
pub const NOT_PLAYING: i64 = i64::MIN;

/// One captured block.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TapBlock {
    /// Graph time of the block's first frame.
    pub time: u64,
    pub frames: u32,
    /// Frames the strip's signal lags the engine at the tap.
    pub arrival: u32,
    /// The song frame the engine rendered at `time`, or [`NOT_PLAYING`].
    pub song_frame: i64,
    /// The strip carried nothing: no audio was queued for this block.
    pub silent: bool,
}

/// The tap. One render thread captures, one other thread pops.
pub struct StripTap {
    audio: AudioRing,
    blocks: Ring<TapBlock>,
    dropped: AtomicU64,
}

impl StripTap {
    /// A tap that holds `frames` frames while its reader is away.
    pub fn new(frames: usize) -> StripTap {
        // Blocks are at least 16 frames in practice; one header per 16 frames is plenty.
        StripTap { audio: AudioRing::new(frames.max(8192)), blocks: Ring::new(frames.max(8192) / 16 + 64), dropped: AtomicU64::new(0) }
    }

    /// Render thread: one block of the strip; `signal` is `None` for silence. Real-time safe.
    pub(crate) fn capture(&self, mut block: TapBlock, signal: Option<(&[f32], &[f32])>) {
        let frames = block.frames as usize;
        block.silent = signal.is_none();
        let room = self.blocks.available() > 0 && signal.is_none_or(|_| self.audio.capacity() - self.audio.available() >= frames);
        if !room {
            self.dropped.fetch_add(frames as u64, Ordering::Relaxed);
            return;
        }
        if let Some((left, right)) = signal {
            self.audio.push(&left[..frames], &right[..frames]);
        }
        // The header follows its audio, so a header the reader sees has all its frames.
        self.blocks.push(&block);
    }

    /// Reader: the next block, its audio appended to `left` and `right` (zeros for a silent
    /// block). `None` when nothing waits.
    pub fn pop(&self, left: &mut Vec<f32>, right: &mut Vec<f32>) -> Option<TapBlock> {
        let block = self.blocks.pop()?;
        let (frames, start) = (block.frames as usize, left.len());
        left.resize(start + frames, 0.0);
        right.resize(start + frames, 0.0);
        if !block.silent {
            self.audio.pop(&mut left[start..], &mut right[start..]);
        }
        Some(block)
    }

    /// Reader: forgets everything that waits.
    pub fn clear(&self) {
        while self.blocks.pop().is_some() {}
        self.audio.clear();
    }

    /// Frames lost because the reader fell behind.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_arrive_in_order_with_their_audio_and_a_full_tap_counts_what_it_drops() {
        let tap = StripTap::new(8192);
        let ramp: Vec<f32> = (0..512).map(|frame| frame as f32).collect();
        let block = |time: u64, frames: u32| TapBlock { time, frames, arrival: 7, song_frame: time as i64 - 100, silent: false };
        tap.capture(block(0, 512), Some((&ramp, &ramp)));
        tap.capture(block(512, 256), None);
        tap.capture(block(768, 100), Some((&ramp[100..], &ramp[200..])));
        let (mut left, mut right) = (Vec::new(), Vec::new());
        assert_eq!(tap.pop(&mut left, &mut right), Some(block(0, 512)));
        assert_eq!((left.len(), left[511], right[1]), (512, 511.0, 1.0));
        assert_eq!(tap.pop(&mut left, &mut right), Some(TapBlock { silent: true, ..block(512, 256) }));
        assert_eq!((left.len(), left[600]), (768, 0.0), "a silent block is zeros");
        assert_eq!(tap.pop(&mut left, &mut right).map(|block| block.time), Some(768));
        assert_eq!((left.len(), left[768], right[768], left[867]), (868, 100.0, 200.0, 199.0));
        assert_eq!(tap.pop(&mut left, &mut right), None);
        // Seventeen blocks of 512 do not fit 8192 frames: the last one is dropped whole.
        for index in 0..17 {
            tap.capture(block(index * 512, 512), Some((&ramp, &ramp)));
        }
        assert_eq!(tap.dropped(), 512);
        tap.clear();
        assert_eq!(tap.pop(&mut left, &mut right), None);
        tap.capture(block(0, 512), Some((&ramp, &ramp)));
        assert_eq!(tap.pop(&mut left, &mut right).map(|block| block.frames), Some(512), "the tap takes blocks again once there is room");
    }
}

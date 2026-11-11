//! Recording to a file: the master output, or an input device.
//!
//! ```text
//!  audio thread (render or input callback)        writer thread ("audio-recording")
//!  ---------------------------------------        ---------------------------------
//!  Tap::capture(time, left, right)   ~~ ring ~~>   drain -> WavWriter (24-bit PCM) -> file
//! ```
//!
//! A [`Recording`] is one take. Its [`Tap`] is given to whoever has the audio: the live output
//! stage for the master ([`crate::live`]), the input stream for an input ([`crate::inputs`]).
//! The tap copies frames into a preallocated single-producer/single-consumer ring and does
//! nothing else: no allocation, no lock, no I/O, and when the writer falls behind (the ring
//! holds four seconds) frames are counted as dropped instead of waiting. The writer thread
//! drains the ring every few milliseconds into the file.
//!
//! Start and stop happen on the source's clock: the tap notes the frame of the first block it
//! captures and, once a stop was requested, the frame at which it stopped, so the file holds
//! exactly the frames `start..end` of that clock ([`Finished::start_frame`]). For the master
//! that clock is the renderer's graph time, which is what makes a take line up with the song:
//! frame `n` of the file is what the mixer put out at graph frame `start + n`.
//!
//! Latency compensation of an input take: what arrives from an input device was played
//! against what was heard, so it is late by the output's and the input's latency. The first
//! `skip_frames` frames of the take are therefore left out of the file ([`Recording::start`]),
//! which moves the take that far ahead.

use st_samples::wav::{WavEncoding, WavSpec, WavWriter};
use std::cell::UnsafeCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A single-producer/single-consumer ring of stereo frames. Neither side blocks or allocates.
pub struct AudioRing {
    left: Box<[UnsafeCell<f32>]>,
    right: Box<[UnsafeCell<f32>]>,
    /// Frames written and read so far (never wrapped; the index is the count modulo capacity).
    written: AtomicUsize,
    read: AtomicUsize,
}

// SAFETY: one thread writes the cells between `read` and `read + capacity` and publishes them
// by advancing `written`; one other thread reads the cells between `read` and `written` and
// releases them by advancing `read`. The two ranges never overlap.
unsafe impl Sync for AudioRing {}
unsafe impl Send for AudioRing {}

impl AudioRing {
    pub fn new(frames: usize) -> AudioRing {
        let cells = |count: usize| (0..count).map(|_| UnsafeCell::new(0.0f32)).collect::<Vec<_>>().into_boxed_slice();
        AudioRing { left: cells(frames.max(1)), right: cells(frames.max(1)), written: AtomicUsize::new(0), read: AtomicUsize::new(0) }
    }

    pub fn capacity(&self) -> usize {
        self.left.len()
    }

    /// Frames waiting to be read.
    pub fn available(&self) -> usize {
        self.written.load(Ordering::Acquire).wrapping_sub(self.read.load(Ordering::Acquire))
    }

    /// Producer: appends as many frames as fit and returns how many.
    pub fn push(&self, left: &[f32], right: &[f32]) -> usize {
        let written = self.written.load(Ordering::Relaxed);
        let free = self.capacity() - written.wrapping_sub(self.read.load(Ordering::Acquire));
        let count = left.len().min(right.len()).min(free);
        for index in 0..count {
            let at = written.wrapping_add(index) % self.capacity();
            // SAFETY: see the `Sync` implementation; these cells are the producer's.
            unsafe {
                *self.left[at].get() = left[index];
                *self.right[at].get() = right[index];
            }
        }
        self.written.store(written.wrapping_add(count), Ordering::Release);
        count
    }

    /// Consumer: takes up to `left.len()` frames and returns how many.
    pub fn pop(&self, left: &mut [f32], right: &mut [f32]) -> usize {
        let read = self.read.load(Ordering::Relaxed);
        let count = left.len().min(right.len()).min(self.written.load(Ordering::Acquire).wrapping_sub(read));
        for index in 0..count {
            let at = read.wrapping_add(index) % self.capacity();
            // SAFETY: see the `Sync` implementation; these cells are the consumer's.
            unsafe {
                left[index] = *self.left[at].get();
                right[index] = *self.right[at].get();
            }
        }
        self.read.store(read.wrapping_add(count), Ordering::Release);
        count
    }

    /// Consumer: drops the oldest `count` frames waiting (all of them when fewer wait).
    pub fn skip(&self, count: usize) {
        let read = self.read.load(Ordering::Relaxed);
        let count = count.min(self.written.load(Ordering::Acquire).wrapping_sub(read));
        self.read.store(read.wrapping_add(count), Ordering::Release);
    }

    /// Consumer: drops everything waiting.
    pub fn clear(&self) {
        self.read.store(self.written.load(Ordering::Acquire), Ordering::Release);
    }
}

const ARMED: u8 = 0;
const RECORDING: u8 = 1;
const STOPPING: u8 = 2;
const STOPPED: u8 = 3;

struct Shared {
    ring: AudioRing,
    state: AtomicU8,
    /// Frames of the source to leave out at the start (latency compensation).
    skip: AtomicU64,
    start_frame: AtomicI64,
    end_frame: AtomicI64,
    captured: AtomicU64,
    dropped: AtomicU64,
    /// Peak of the captured audio since the last reading (`f32` bits), per channel.
    peaks: [AtomicU64; 2],
}

/// The audio thread's end of a recording. Cheap to clone; every clone feeds the same take, so
/// only one source may use it.
#[derive(Clone)]
pub struct Tap {
    shared: Arc<Shared>,
}

impl Tap {
    /// The next block of the source: `left.len()` frames starting at frame `time` of the
    /// source's clock. Real-time safe.
    pub fn capture(&self, time: u64, left: &[f32], right: &[f32]) {
        let shared = &*self.shared;
        match shared.state.load(Ordering::Acquire) {
            ARMED => {
                shared.start_frame.store(time as i64, Ordering::Release);
                // The take may have been ended before its first block: then nothing is captured.
                if shared.state.compare_exchange(ARMED, RECORDING, Ordering::AcqRel, Ordering::Acquire).is_err() {
                    return;
                }
            }
            RECORDING => {}
            STOPPING => {
                shared.end_frame.store(time as i64, Ordering::Release);
                shared.state.store(STOPPED, Ordering::Release);
                return;
            }
            _ => return,
        }
        let frames = left.len().min(right.len());
        let skip = shared.skip.load(Ordering::Relaxed).min(frames as u64) as usize;
        if skip > 0 {
            shared.skip.fetch_sub(skip as u64, Ordering::Relaxed);
        }
        let pushed = shared.ring.push(&left[skip..frames], &right[skip..frames]);
        shared.captured.fetch_add(pushed as u64, Ordering::Relaxed);
        if pushed < frames - skip {
            shared.dropped.fetch_add((frames - skip - pushed) as u64, Ordering::Relaxed);
        }
        for (channel, samples) in [&left[skip..frames], &right[skip..frames]].into_iter().enumerate() {
            let peak = samples.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()));
            if peak > f32::from_bits(shared.peaks[channel].load(Ordering::Relaxed) as u32) {
                shared.peaks[channel].store(peak.to_bits() as u64, Ordering::Relaxed);
            }
        }
    }

    /// True once the take has ended on the source's clock: the source can let go of the tap.
    pub fn is_finished(&self) -> bool {
        self.shared.state.load(Ordering::Acquire) == STOPPED
    }
}

/// What a finished take is.
#[derive(Debug, Clone, PartialEq)]
pub struct Finished {
    pub path: PathBuf,
    pub frames: u64,
    pub sample_rate: u32,
    pub channels: u16,
    /// The source's frame of the first frame in the file (after the skipped ones).
    pub start_frame: Option<i64>,
    /// The source's frame after the last one captured.
    pub end_frame: Option<i64>,
    /// Frames lost because the writer fell behind, or the reason the take is incomplete.
    pub dropped: u64,
    pub problem: Option<String>,
}

/// One take being written. Control threads only.
pub struct Recording {
    shared: Arc<Shared>,
    path: PathBuf,
    sample_rate: u32,
    channels: u16,
    skipped: u64,
    started: Instant,
    stop_writer: Arc<AtomicBool>,
    writer: Option<std::thread::JoinHandle<Result<u64, String>>>,
}

impl Recording {
    /// Creates the file at `path` (24-bit PCM at `sample_rate`; `channels` 1 writes the mean of
    /// both captured channels) and starts the writer thread. The take begins with the first
    /// block the returned [`Tap`] is given; its first `skip_frames` frames are left out.
    pub fn start(path: &Path, sample_rate: u32, channels: u16, skip_frames: u64) -> Result<(Recording, Tap), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("The recording folder could not be created: {error}"))?;
        }
        let channels = channels.clamp(1, 2);
        let spec = WavSpec { sample_rate, channels, encoding: WavEncoding::Pcm24, dither: Default::default() };
        let mut file = WavWriter::create_new(path, spec, &[]).map_err(|error| error.to_string())?;
        let shared = Arc::new(Shared {
            ring: AudioRing::new(sample_rate as usize * 4),
            state: AtomicU8::new(ARMED),
            skip: AtomicU64::new(skip_frames),
            start_frame: AtomicI64::new(i64::MIN),
            end_frame: AtomicI64::new(i64::MIN),
            captured: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            peaks: [AtomicU64::new(0), AtomicU64::new(0)],
        });
        let stop_writer = Arc::new(AtomicBool::new(false));
        let (source, stop) = (shared.clone(), stop_writer.clone());
        let writer = std::thread::Builder::new()
            .name("audio-recording".to_string())
            .spawn(move || -> Result<u64, String> {
                let (mut left, mut right) = (vec![0.0f32; 8192], vec![0.0f32; 8192]);
                let mut mono = vec![0.0f32; 8192];
                loop {
                    // Read the flag first: what is in the ring after it was set is all there is.
                    let last = stop.load(Ordering::Acquire);
                    loop {
                        let count = source.ring.pop(&mut left, &mut right);
                        if count == 0 {
                            break;
                        }
                        let written = if channels == 1 {
                            for index in 0..count {
                                mono[index] = (left[index] + right[index]) * 0.5;
                            }
                            file.write_planar(&[&mono[..count]])
                        } else {
                            file.write_planar(&[&left[..count], &right[..count]])
                        };
                        written.map_err(|error| error.to_string())?;
                    }
                    if last {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                let (out, frames) = file.finalize().map_err(|error| error.to_string())?;
                out.into_inner().map_err(|error| format!("The recording could not be finished: {}", error.error()))?;
                Ok(frames)
            })
            .map_err(|error| format!("The recording thread could not start: {error}"))?;
        let recording = Recording { shared: shared.clone(), path: path.to_path_buf(), sample_rate, channels, skipped: skip_frames, started: Instant::now(), stop_writer, writer: Some(writer) };
        Ok((recording, Tap { shared }))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Seconds captured so far.
    pub fn duration(&self) -> f64 {
        self.shared.captured.load(Ordering::Relaxed) as f64 / self.sample_rate as f64
    }

    /// Time since the take was started.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// The peaks captured since the last call, left and right.
    pub fn take_peaks(&self) -> (f32, f32) {
        let take = |peak: &AtomicU64| f32::from_bits(peak.swap(0, Ordering::Relaxed) as u32);
        (take(&self.shared.peaks[0]), take(&self.shared.peaks[1]))
    }

    /// Ends the take: asks the source to stop at its next block, waits up to `patience` for it
    /// to do so (a source that no longer runs is not waited for longer), then drains and closes
    /// the file.
    pub fn finish(mut self, patience: Duration) -> Finished {
        let shared = self.shared.clone();
        let mut problem = None;
        // A take whose source never delivered a block has nothing to acknowledge.
        if shared.state.compare_exchange(ARMED, STOPPED, Ordering::AcqRel, Ordering::Acquire).is_err() {
            let _ = shared.state.compare_exchange(RECORDING, STOPPING, Ordering::AcqRel, Ordering::Acquire);
            let deadline = Instant::now() + patience;
            while shared.state.load(Ordering::Acquire) != STOPPED {
                if Instant::now() >= deadline {
                    problem = Some("The recording source did not acknowledge Stop on its audio clock in time.".to_string());
                    shared.state.store(STOPPED, Ordering::Release);
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        self.stop_writer.store(true, Ordering::Release);
        let written = match self.writer.take().map(|writer| writer.join()) {
            Some(Ok(Ok(frames))) => frames,
            Some(Ok(Err(error))) => {
                problem = Some(error);
                0
            }
            _ => {
                problem = Some("The recording thread ended unexpectedly.".to_string());
                0
            }
        };
        let dropped = shared.dropped.load(Ordering::Relaxed);
        if dropped > 0 && problem.is_none() {
            problem = Some(format!("{dropped} frames were lost because the disk could not keep up."));
        }
        let frame = |value: &AtomicI64| Some(value.load(Ordering::Acquire)).filter(|frame| *frame != i64::MIN);
        Finished {
            path: self.path.clone(),
            frames: written,
            sample_rate: self.sample_rate,
            channels: self.channels,
            start_frame: frame(&shared.start_frame).map(|start| start + self.skipped as i64),
            end_frame: frame(&shared.end_frame),
            dropped,
            problem,
        }
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        // A take that was not finished is closed as it is.
        self.shared.state.store(STOPPED, Ordering::Release);
        self.stop_writer.store(true, Ordering::Release);
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_carries_frames_in_order_and_refuses_what_does_not_fit() {
        let ring = AudioRing::new(8);
        assert_eq!(ring.push(&[1.0, 2.0, 3.0], &[-1.0, -2.0, -3.0]), 3);
        let (mut left, mut right) = ([0.0; 2], [0.0; 2]);
        assert_eq!((ring.pop(&mut left, &mut right), left, right), (2, [1.0, 2.0], [-1.0, -2.0]));
        // Seven fit (one is still waiting), across the wrap.
        let ramp: Vec<f32> = (10..20).map(|value| value as f32).collect();
        assert_eq!(ring.push(&ramp, &ramp), 7);
        assert_eq!(ring.available(), 8);
        let (mut left, mut right) = ([0.0; 16], [0.0; 16]);
        assert_eq!(ring.pop(&mut left, &mut right), 8);
        assert_eq!(left[..8], [3.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0]);
        assert_eq!(right[0], -3.0);
        ring.push(&[1.0], &[1.0]);
        ring.clear();
        assert_eq!(ring.available(), 0);
        // Skipping drops the oldest frames, and no more than there are.
        ring.push(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]);
        ring.skip(2);
        assert_eq!((ring.pop(&mut left, &mut right), left[0]), (1, 3.0));
        ring.skip(5);
        assert_eq!(ring.available(), 0);
    }

    #[test]
    fn a_take_holds_exactly_the_frames_between_start_and_stop_of_its_source() {
        let path = std::env::temp_dir().join(format!("st-audio-take-{}.wav", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let (recording, tap) = Recording::start(&path, 48_000, 2, 100).unwrap();
        // The source delivers blocks of 256 frames from its frame 1 000 on: a ramp by frame.
        let block = |start: u64| -> (Vec<f32>, Vec<f32>) { ((start..start + 256).map(|frame| frame as f32 / 100_000.0).collect(), (start..start + 256).map(|frame| -(frame as f32) / 100_000.0).collect()) };
        let source = std::thread::spawn({
            let tap = tap.clone();
            move || {
                let mut time = 1_000u64;
                while !tap.is_finished() {
                    let (left, right) = block(time);
                    tap.capture(time, &left, &right);
                    time += 256;
                    std::thread::sleep(Duration::from_micros(300));
                }
            }
        });
        std::thread::sleep(Duration::from_millis(60));
        assert!(recording.duration() > 0.0);
        let (left, right) = recording.take_peaks();
        assert!(left > 0.0 && right > 0.0);
        let finished = recording.finish(Duration::from_secs(2));
        source.join().unwrap();
        assert_eq!((finished.problem.clone(), finished.dropped, finished.channels, finished.sample_rate), (None, 0, 2, 48_000));
        // The first 100 frames were skipped: the file starts at source frame 1 100.
        let (start, end) = (finished.start_frame.unwrap(), finished.end_frame.unwrap());
        assert_eq!((start, finished.frames as i64), (1_100, end - start));
        let file = crate::samples::read_wav(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(file.frames() as u64, finished.frames);
        for (index, (left, right)) in file.left.iter().zip(&file.right).enumerate() {
            let expected = (1_100 + index) as f32 / 100_000.0;
            assert!((left - expected).abs() < 1e-6 && (right + expected).abs() < 1e-6, "frame {index}");
        }
        // A take whose source never ran is an empty file, without waiting.
        let empty = std::env::temp_dir().join(format!("st-audio-take-empty-{}.wav", std::process::id()));
        let _ = std::fs::remove_file(&empty);
        let (recording, _tap) = Recording::start(&empty, 44_100, 1, 0).unwrap();
        let began = Instant::now();
        let finished = recording.finish(Duration::from_secs(2));
        assert!(began.elapsed() < Duration::from_secs(1));
        assert_eq!((finished.frames, finished.start_frame, finished.problem), (0, None, None));
        assert!(Recording::start(&empty, 44_100, 1, 0).is_err(), "an existing file is never overwritten");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&empty);
    }
}

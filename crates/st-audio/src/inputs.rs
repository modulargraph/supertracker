//! Audio inputs: one input stream per engine, and everything that listens to it.
//!
//! ```text
//!  input thread (cpal callback, the null input's timer thread, or the null output's thread)
//!     InputHub::deliver(frames)
//!        the chosen channels as stereo    -->  meter peaks (atomics)
//!                                         -->  their LineFeed  ~~~>  render thread (crate::live): monitoring, the take
//!        the channels of each line input  -->  its LineFeed    ~~~>  render thread (crate::graph::plan), the input of a track strip
//! ```
//!
//! [`Inputs`] is the control side (any control thread): which device and channels, whether the
//! input is metered or monitored, and which line inputs the published project routes into its
//! tracks ([`Inputs::set_line_routes`]). There is one stream, and it is open only while
//! something uses it: the meter, monitoring, a take, or a line input of the device. Devices come
//! from `cpal` (ALSA on Linux; CoreAudio on macOS, type-checked only). The *null input* (`uid`
//! "null") needs no hardware: it delivers silence (or, for tests, a sine) in real time. It is
//! offered while the engine runs on the null output device, and then that device's thread
//! delivers it, block for block before it renders: one device with one clock, as an audio
//! interface is ([`NullDuplex`]). Without a null output it has a timer thread of its own.
//!
//! # Line inputs
//!
//! A line input is a track's `audioInput` in the project: one or two channels of the input
//! device, mixed into the track's strip ahead of its inserts. The project is the only
//! description of the routing; [`Inputs::set_line_routes`] is told the enabled routes of every
//! publication and answers, per route, with the [`LineFeed`] the mixer reads or with the reason
//! the route is paused (its device is not the input in use, the device could not be opened, a
//! channel does not exist). A paused route stays in the project; it is resolved again with
//! every publication and every change of the devices.
//!
//! # The input's sample rate
//!
//! The input is opened at the engine's sample rate when the device supports it, and otherwise
//! at the rate nearest to it that the device has (a 44.1 kHz microphone under a 48 kHz
//! engine). In the second case, and whenever the input is another device than the output, the
//! audio is resampled on its way to the render thread; see the next section.
//!
//! # Two clocks
//!
//! Everything that mixes the input into the output crosses from the input's thread to the
//! render thread through a [`LineFeed`]: a ring, and a reader that knows how the input's clock
//! relates to the output's ([`FeedClock`]). The mixer compensates for a feed's delay
//! ([`LineFeed::latency`], constant for the feed's life), so each kind of feed says exactly
//! what it does.
//!
//! **One clock** ([`FeedClock::Shared`]): the input and the output are one device at one rate,
//! or the null input delivered by the null output's thread, or the manual input of a test that
//! delivers and renders on one thread. This is assumed only where it is certain
//! ([`crate::device::same_clock`]). The frames pass through unchanged, bit for bit:
//!
//! * **Start.** The reader plays silence until `block` frames wait beyond the block it is about
//!   to read, drops what is older, and starts. From then on a frame is mixed `block` frames
//!   after it was delivered (plus the phase between the two callbacks, less than a block).
//!   `block` is one block of the device: the cushion for the callbacks' jitter, and the feed's
//!   latency.
//! * **Underrun** (less waits than the block needs: the input stalled): the block gets what is
//!   there and silence after it, and the feed starts over. That is a gap of at least `block`
//!   frames; [`LineFeed::underruns`] counts them.
//! * **Overrun** (more than three times `block` plus 1024 frames wait beyond the block: the
//!   output stalled): the oldest frames are dropped down to `block`. That is a skip;
//!   [`LineFeed::overruns`] counts them.
//!
//! **Two clocks** ([`FeedClock::Separate`]): every other pair of devices, at the same nominal
//! rate or not. Their crystals differ by tens to hundreds of parts per million, so the reader
//! resamples by a ratio it measures ([`crate::asrc`]):
//!
//! * **Time.** Both threads read one monotonic clock when their callback begins (the
//!   system's, not the devices' own timestamps: `cpal` does not promise that the timestamps
//!   of two streams count from one origin). With every block the input thread publishes when
//!   it arrived and how many frames have arrived in all; the reader extends that to the moment of its own callback: `head = frames + (now -
//!   then) * input rate`, the frame the input has reached if frames came one by one instead
//!   of in blocks. The feed's delay is the distance from the read position to that head.
//!   Counting the frames in the ring instead would jump by a whole block each time the two
//!   callbacks slip past each other, and the pitch with it. What the reader cannot see is a
//!   constant: if one device's callbacks are later on average than the other's, the delay
//!   is off by the difference.
//! * **Delay.** The delay is held at `output block * ratio + input block + half the filter +
//!   margin` input frames: an output block is read in one instant, the input arrives a block
//!   at a time, the filter reads 44 frames ahead, and the margin (half the duration of the
//!   longer block, at least 1 ms and at most 5 ms) is for callbacks that come late.
//!   [`LineFeed::latency`] is that in output frames. For two 48 kHz devices with 512-frame
//!   blocks it is 1 310 frames (27 ms), with 64-frame blocks 222 frames (4.6 ms); the
//!   one-clock feed's is 512 and 64. What varies is how well the loop holds it. With
//!   callbacks that come up to half a millisecond late (`tests/two_clocks.rs`) the delay
//!   stays within 3 frames of the figure once the loop has the ratio and within 4 while it
//!   finds it; without that jitter a device 200 ppm off moves it by 5 frames at most in the
//!   first seconds (23 frames for 1 000 ppm) and by a tenth of a frame after a minute.
//! * **Start.** The reader is silent for a tenth of a second while it measures where the
//!   input is, and starts at the mean of what it measured: one callback's time is only as
//!   exact as that callback was punctual.
//! * **Ratio.** [`crate::asrc::RateLoop`] turns the error of the delay into the ratio of each
//!   block; [`LineFeed::drift_ppm`] is the difference of the clocks it has measured. A feed
//!   made while another of the same stream has run for twenty seconds starts from that
//!   one's measurement (a take begins at the right pitch).
//! * **Late input** (frames a block needs have not arrived: a USB hiccup): the block gets
//!   silence in their place and keeps its place in time. When the frames arrive after all,
//!   those whose time has passed are dropped, and the delay is what it was: no offset
//!   remains. One incident counts as one underrun.
//! * **A jump** (the delay is away from its target by more than half a block and the margin
//!   for a tenth of a second: one side lost frames or stood still): the read position is put
//!   back on the target at once, by the mean of the error in that tenth of a second, keeping
//!   the measured ratio. Too little waiting means silence
//!   until the input has caught up, and counts as an underrun unless the incident was counted
//!   already; too much waiting drops the oldest frames, and counts as an overrun. So does an
//!   input ring that ran full because nobody read it: what waited in it is stale and dropped.
//! * **No input** for half a second: the feed is silent and starts over when blocks arrive
//!   again.
//!
//! The take of an input is recorded behind the chosen channels' feed, on the render thread
//! ([`crate::live`]): the file is on the output's clock, like everything else in the song.
//!
//! Real-time rules: [`InputHub::deliver`], [`InputHub::listen`] and [`LineFeed::read`] do not
//! allocate, block or perform I/O. The hub's mutexes (the list of feeds for the input thread,
//! the chosen channels' feed for the render thread) are only ever `try_lock`ed on those
//! threads, each by one of them, and held elsewhere for the time of an assignment; a block
//! that finds one taken skips that consumer.

use crate::asrc::{Converter, Kernel, RateLoop, RATIO_RANGE};
use crate::device::{device_id, DeviceInfo, NULL_DEVICE};
use crate::graph::insert::monotonic_ns;
use crate::recording::AudioRing;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The `uid` of the null input fed by a test tone (tests only; listed only while it is chosen).
pub const NULL_TONE_INPUT: &str = "null:tone";
/// The `uid` of a null input that delivers nothing by itself: a test calls
/// [`InputHub::deliver`] (tests only; never listed). It has eight channels, and shares the
/// clock of whoever renders: the test is both threads. With a rate after it
/// (`null:manual:44100`, and optionally its block size, `null:manual:44100:441`) it is a device
/// with a clock of its own at that nominal rate: the test says when each block arrives
/// ([`InputHub::deliver_at`]) and when each render call happens.
pub const NULL_MANUAL_INPUT: &str = "null:manual";
/// The prefix of the `uid` of a null input that runs free (tests only; never listed):
/// `null:free:44100` delivers the test tone from a timer thread of its own at 44.1 kHz,
/// whatever the output is, and `null:free:48000:200` at 48 kHz from a clock 200 ppm fast.
pub const NULL_FREE_INPUT: &str = "null:free:";
/// Frames of scratch per delivery step.
const SCRATCH: usize = 4096;
/// How long a failed attempt to open the input for line inputs is not repeated.
const RETRY: Duration = Duration::from_secs(2);
/// Output frames a feed that resamples renders in one step.
const CONVERT_FRAMES: usize = 1024;
/// A feed that resamples is silent and starts over when its input has delivered nothing for
/// this long.
const STALLED_SECONDS: f64 = 0.5;
/// How long the delay of a feed that resamples may be out of range before its read position
/// jumps, and how long such a feed measures where its input is before it starts.
const ASTRAY_SECONDS: f64 = 0.1;
/// How long a feed's loop has run when later feeds of the same stream start from the clock
/// difference it has measured.
const KNOWN_SECONDS: f64 = 20.0;

/// How the clock of a feed's input relates to the output's. See the module documentation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FeedClock {
    /// One clock: frames pass through unchanged, `block` frames after they were delivered.
    Shared { block: u32 },
    /// A clock of its own: the feed resamples. The rates are the nominal ones, the blocks the
    /// largest each side moves at once (in its own frames).
    Separate { input_rate: f64, output_rate: f64, input_block: u32, output_block: u32 },
}

/// The parts of a feed that resamples.
struct Converted {
    input_rate: f64,
    output_rate: f64,
    /// The delay the reader keeps between the input's head and its read position, in input frames.
    target: f64,
    /// How far the delay may be from the target for longer than a moment before the read
    /// position jumps: more than late callbacks and the loop's search for the ratio explain.
    limit: f64,
    /// The input thread's last block: `written` frames had arrived in all at `stamp`
    /// (nanoseconds of the monotonic clock). `sequence` is odd while the two are being changed.
    sequence: AtomicU64,
    written: AtomicU64,
    stamp: AtomicU64,
    /// The ring was full when the input thread came with a block.
    overflowed: AtomicBool,
    /// A reader is at work (there is never more than one).
    reading: AtomicBool,
    reader: UnsafeCell<Reader>,
    /// The measured difference of the clocks in parts per million (`f32` bits).
    drift: AtomicU32,
    /// The same for every feed of the stream, once a feed has measured it for a while (NaN
    /// until then): a feed made later starts from it.
    known_drift: Arc<AtomicU32>,
    /// Input frames the delay was away from its target at the last read (`f32` bits).
    error: AtomicU32,
}

// SAFETY: `reader` is only touched by the thread that set `reading`, one at a time.
unsafe impl Sync for Converted {}

/// The render thread's state of a feed that resamples.
struct Reader {
    converter: Converter,
    rate: RateLoop,
    /// The ring's next frame, as its index in the input's count of frames.
    taken: i64,
    /// The input thread's last block as this reader saw it last: frames and time.
    seen: Option<(u64, u64)>,
    running: bool,
    /// Seconds left of the silent start, in which the reader only measures where the input is.
    aligning: f64,
    /// The last read lacked frames (one incident is counted once).
    starving: bool,
    /// Seconds the delay has been out of range.
    astray: f64,
    /// The sum and the number of the delay errors measured while starting or out of range.
    errors: (f64, u32),
}

impl Reader {
    /// Gives the converter the input frames `from .. from + count`. True when some of them
    /// have not arrived yet (they are silence).
    fn supply(&mut self, ring: &AudioRing, from: i64, count: usize) -> bool {
        let (left, right) = self.converter.input(count);
        let count = left.len();
        let mut at = 0;
        // Frames the ring no longer has (before the stream began, or behind a read position
        // that was put back) are silence.
        if from < self.taken {
            at = ((self.taken - from) as usize).min(count);
            left[..at].fill(0.0);
            right[..at].fill(0.0);
        }
        if at == count {
            return false;
        }
        let next = from + at as i64;
        if next > self.taken {
            // Frames that arrived after their time has passed.
            self.taken += ring.skip((next - self.taken) as usize) as i64;
        }
        if self.taken == next {
            let got = ring.pop(&mut left[at..], &mut right[at..]);
            self.taken += got as i64;
            at += got;
        }
        if at < count {
            left[at..].fill(0.0);
            right[at..].fill(0.0);
            return true;
        }
        false
    }

    fn read(&mut self, feed: &LineFeed, shared: &Converted, clock_ns: u64, left: &mut [f32], right: &mut [f32]) -> bool {
        let frames = left.len().min(right.len());
        // The input thread's last block. It changes the pair in a few instructions; a reader
        // that keeps finding it at work goes on with the pair it saw last.
        for _ in 0..4 {
            let sequence = shared.sequence.load(Ordering::SeqCst);
            if sequence & 1 == 0 {
                let pair = (shared.written.load(Ordering::SeqCst), shared.stamp.load(Ordering::SeqCst));
                if shared.sequence.load(Ordering::SeqCst) == sequence {
                    if sequence > 0 {
                        self.seen = Some(pair);
                    }
                    break;
                }
            }
            std::hint::spin_loop();
        }
        let Some((written, stamp)) = self.seen else { return false };
        let since = (clock_ns as f64 - stamp as f64) * 1e-9;
        if since > STALLED_SECONDS {
            if self.running && !self.starving {
                feed.underruns.fetch_add(1, Ordering::Relaxed);
            }
            (self.running, self.starving) = (false, false);
            return false;
        }
        if shared.overflowed.swap(false, Ordering::Relaxed) {
            // Nobody read while the input went on: what waits is old, and what came after it
            // was lost.
            self.taken += feed.ring.skip(usize::MAX) as i64;
            if self.running {
                feed.overruns.fetch_add(1, Ordering::Relaxed);
            }
            self.running = false;
        }
        // Where the input is now, counted in its frames.
        let head = written as f64 + since * shared.input_rate;
        if !self.running {
            self.converter.seek(head - shared.target);
            self.rate.settle();
            (self.running, self.aligning, self.starving, self.astray, self.errors) = (true, ASTRAY_SECONDS, false, 0.0, (0.0, 0));
        }
        let mut error = head - self.converter.position() - shared.target;
        let seconds = frames as f64 / shared.output_rate;
        if self.aligning > 0.0 {
            // One callback's time is only as exact as the callback was punctual: the start
            // measures for a tenth of a second, in silence, and begins at the mean.
            self.errors = (self.errors.0 + error, self.errors.1 + 1);
            self.aligning -= seconds;
            if self.aligning > 0.0 {
                self.converter.seek(self.converter.position() + self.rate.held() * frames as f64);
                return false;
            }
            let mean = self.errors.0 / self.errors.1 as f64;
            self.converter.seek(self.converter.position() + mean);
            self.errors = (0.0, 0);
            error -= mean;
        }
        let ratio = if error.abs() > shared.limit {
            // Callbacks that come late explain this for a block or two; if it stays so, frames
            // were lost on one side or the other.
            self.astray += seconds;
            self.errors = (self.errors.0 + error, self.errors.1 + 1);
            if self.astray >= ASTRAY_SECONDS {
                if error > 0.0 {
                    feed.overruns.fetch_add(1, Ordering::Relaxed);
                } else if !self.starving {
                    feed.underruns.fetch_add(1, Ordering::Relaxed);
                    self.starving = true;
                }
                // By the mean of what was measured meanwhile, as at the start.
                let mean = self.errors.0 / self.errors.1 as f64;
                self.converter.seek(self.converter.position() + mean);
                self.rate.settle();
                (self.astray, self.errors) = (0.0, (0.0, 0));
                error -= mean;
            }
            self.rate.held()
        } else {
            (self.astray, self.errors) = (0.0, (0.0, 0));
            self.rate.update(error, seconds)
        };
        let step = crate::asrc::step(ratio);
        let (mut at, mut starved) = (0, false);
        while at < frames {
            let count = (frames - at).min(self.converter.max_frames());
            let (from, wanted) = self.converter.wanted(count, step);
            starved |= self.supply(&feed.ring, from, wanted);
            self.converter.render(step, &mut left[at..at + count], &mut right[at..at + count]);
            at += count;
        }
        if starved && !self.starving {
            feed.underruns.fetch_add(1, Ordering::Relaxed);
        }
        self.starving = starved;
        shared.drift.store((self.rate.drift_ppm() as f32).to_bits(), Ordering::Relaxed);
        if self.rate.age() > KNOWN_SECONDS {
            shared.known_drift.store((self.rate.drift_ppm() as f32).to_bits(), Ordering::Relaxed);
        }
        shared.error.store((error as f32).to_bits(), Ordering::Relaxed);
        true
    }
}

/// One feed: the ring between the input thread and the render thread, and the reader's side
/// of the two clocks. See the module documentation.
pub struct LineFeed {
    /// Device channels of left and right (the same for a mono input).
    channels: [u32; 2],
    ring: AudioRing,
    clock: FeedClock,
    /// One clock: the frames kept waiting, and the most that may wait.
    target: usize,
    limit: usize,
    /// One clock: the reader has started (render thread only).
    primed: AtomicBool,
    /// Two clocks: the converter.
    converted: Option<Converted>,
    /// Output frames between a frame's delivery and its place in the mix.
    latency: u32,
    underruns: AtomicU64,
    overruns: AtomicU64,
}

impl LineFeed {
    /// A feed of `channels` for an input on `clock`; one that resamples is given the filter
    /// of its two rates and what the stream's feeds know of the clocks' difference. Control
    /// thread.
    fn new(channels: [u32; 2], clock: FeedClock, converter: Option<(&Arc<Kernel>, &Arc<AtomicU32>)>) -> LineFeed {
        let (underruns, overruns) = (AtomicU64::new(0), AtomicU64::new(0));
        match (clock, converter) {
            (FeedClock::Separate { input_rate, output_rate, input_block, output_block }, Some((kernel, known_drift))) => {
                let known = f32::from_bits(known_drift.load(Ordering::Relaxed)) as f64;
                let rate = RateLoop::new(input_rate, output_rate).starting_at(if known.is_finite() { known } else { 0.0 });
                // Callbacks that come late: half the duration of the longer block, 1 to 5 ms.
                let margin = (0.5 * (input_block as f64 / input_rate).max(output_block as f64 / output_rate)).clamp(0.001, 0.005) * input_rate;
                let target = (output_block as f64 * rate.nominal() + input_block as f64 + kernel.half() as f64 + 2.0 + margin).ceil();
                let limit = 0.5 * (input_block as f64).min(output_block as f64 * rate.nominal()) + margin;
                let rate_drift = rate.drift_ppm();
                let converter = Converter::new(kernel.clone(), rate.max_ratio(), CONVERT_FRAMES);
                // Room for the delay and as much again before a jump, the largest block the
                // input moves at once, and a margin.
                let capacity = (3.0 * target) as usize + 3 * 8192 * (rate.nominal().ceil() as usize).max(1);
                LineFeed {
                    channels,
                    ring: AudioRing::new(capacity.next_power_of_two()),
                    clock,
                    target: 0,
                    limit: 0,
                    primed: AtomicBool::new(false),
                    latency: (target / rate.nominal()).round() as u32,
                    converted: Some(Converted {
                        input_rate,
                        output_rate,
                        target,
                        limit,
                        sequence: AtomicU64::new(0),
                        written: AtomicU64::new(0),
                        stamp: AtomicU64::new(0),
                        overflowed: AtomicBool::new(false),
                        reading: AtomicBool::new(false),
                        reader: UnsafeCell::new(Reader { converter, rate, taken: 0, seen: None, running: false, aligning: 0.0, starving: false, astray: 0.0, errors: (0.0, 0) }),
                        drift: AtomicU32::new((rate_drift as f32).to_bits()),
                        known_drift: known_drift.clone(),
                        error: AtomicU32::new(0f32.to_bits()),
                    }),
                    underruns,
                    overruns,
                }
            }
            (FeedClock::Shared { block }, _) | (FeedClock::Separate { output_block: block, .. }, None) => {
                let target = (block as usize).max(16);
                let limit = 3 * target + 1024;
                // Room for the limit, the largest block either side moves at once, and a margin.
                LineFeed { channels, ring: AudioRing::new((limit + 3 * 8192).next_power_of_two()), clock, target, limit, primed: AtomicBool::new(false), converted: None, latency: target as u32, underruns, overruns }
            }
        }
    }

    /// How the input's clock relates to the output's.
    pub fn clock(&self) -> FeedClock {
        self.clock
    }

    /// Output frames between a frame's delivery and its place in the mix: the feed's part of
    /// the input's latency. Constant for the feed's life.
    pub fn latency(&self) -> u32 {
        self.latency
    }

    /// Times the input did not deliver in time (a gap in the input).
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    /// Times audio was dropped because too much waited (a skip in the input).
    pub fn overruns(&self) -> u64 {
        self.overruns.load(Ordering::Relaxed)
    }

    /// A feed that resamples: the difference of the two clocks as measured so far, in parts
    /// per million (positive when the input's clock is the faster one). `None` for one clock.
    pub fn drift_ppm(&self) -> Option<f64> {
        self.converted.as_ref().map(|converted| f32::from_bits(converted.drift.load(Ordering::Relaxed)) as f64)
    }

    /// A feed that resamples: how far the delay was from [`LineFeed::latency`] at the last
    /// read, in output frames (positive: later).
    pub fn latency_error(&self) -> Option<f64> {
        self.converted.as_ref().map(|converted| f32::from_bits(converted.error.load(Ordering::Relaxed)) as f64 * converted.output_rate / converted.input_rate)
    }

    /// Frames waiting in the ring.
    pub fn waiting(&self) -> usize {
        self.ring.available()
    }

    /// Input thread: the next frames. `time_ns` is when the delivery they are part of
    /// arrived, and `rest` how many frames of that delivery follow these.
    fn push(&self, time_ns: u64, rest: usize, left: &[f32], right: &[f32]) {
        let pushed = self.ring.push(left, right);
        if let Some(converted) = &self.converted {
            if pushed < left.len().min(right.len()) {
                converted.overflowed.store(true, Ordering::Relaxed);
            }
            // The frames that follow arrive, on the input's clock, that much later.
            let stamp = time_ns.saturating_sub((rest as f64 / converted.input_rate * 1e9) as u64).max(1);
            converted.sequence.fetch_add(1, Ordering::SeqCst);
            converted.written.fetch_add(pushed as u64, Ordering::SeqCst);
            converted.stamp.store(stamp, Ordering::SeqCst);
            converted.sequence.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Render thread: the next `left.len()` frames, for a block whose callback began at
    /// `clock_ns` of the monotonic clock (plus the duration of the frames the callback
    /// rendered before these). False for silence (the buffers are then untouched). Real-time
    /// safe.
    pub fn read(&self, clock_ns: u64, left: &mut [f32], right: &mut [f32]) -> bool {
        if let Some(converted) = &self.converted {
            if converted.reading.swap(true, Ordering::Acquire) {
                return false;
            }
            // SAFETY: `reading` admits one reader at a time.
            let reader = unsafe { &mut *converted.reader.get() };
            let heard = reader.read(self, converted, clock_ns, left, right);
            converted.reading.store(false, Ordering::Release);
            return heard;
        }
        let frames = left.len().min(right.len());
        let available = self.ring.available();
        if !self.primed.load(Ordering::Relaxed) {
            if available < self.target + frames {
                return false;
            }
            self.ring.skip(available - (self.target + frames));
            self.primed.store(true, Ordering::Relaxed);
        } else if available > self.limit + frames {
            self.ring.skip(available - (self.target + frames));
            self.overruns.fetch_add(1, Ordering::Relaxed);
        }
        let count = self.ring.pop(&mut left[..frames], &mut right[..frames]);
        if count < frames {
            left[count..frames].fill(0.0);
            right[count..frames].fill(0.0);
            self.primed.store(false, Ordering::Relaxed);
            self.underruns.fetch_add(1, Ordering::Relaxed);
        }
        true
    }
}

/// What the input thread and everybody else share.
pub struct InputHub {
    /// Source channels of left and right (the same for a mono input).
    channels: [AtomicU32; 2],
    peaks: [AtomicU32; 2],
    metering: AtomicBool,
    /// The chosen channels are mixed into the output.
    monitoring: AtomicBool,
    /// Monitor gain (`f32` bits).
    monitor_volume: AtomicU32,
    /// What the input thread feeds: the live line inputs and, while monitoring or a take
    /// listens, the chosen channels.
    feeds: Mutex<Arc<Vec<Arc<LineFeed>>>>,
    /// The chosen channels' feed, for the render thread.
    listen: Mutex<Option<Arc<LineFeed>>>,
    /// Estimated input latency in the input's frames, as the device reports it per callback.
    latency: AtomicU32,
    /// The largest block the open stream has delivered, in frames.
    block: AtomicU32,
}

impl InputHub {
    fn new() -> InputHub {
        InputHub {
            channels: [AtomicU32::new(0), AtomicU32::new(1)],
            peaks: [AtomicU32::new(0), AtomicU32::new(0)],
            metering: AtomicBool::new(false),
            monitoring: AtomicBool::new(false),
            monitor_volume: AtomicU32::new(0.5f32.to_bits()),
            feeds: Mutex::default(),
            listen: Mutex::default(),
            latency: AtomicU32::new(0),
            block: AtomicU32::new(0),
        }
    }

    /// Input thread: `frames` frames that arrived now, of which `sample(frame, channel)` reads
    /// one sample. Real-time safe.
    pub fn deliver(&self, frames: usize, device_channels: usize, sample: impl Fn(usize, usize) -> f32) {
        self.deliver_at(monotonic_ns(), frames, device_channels, sample);
    }

    /// As [`InputHub::deliver`], for frames that arrived at `time_ns` of the monotonic clock
    /// (a test with a clock of its own says when).
    pub fn deliver_at(&self, time_ns: u64, frames: usize, device_channels: usize, sample: impl Fn(usize, usize) -> f32) {
        let (mut left, mut right) = ([0.0f32; SCRATCH], [0.0f32; SCRATCH]);
        let last = device_channels.saturating_sub(1);
        let (from_left, from_right) = ((self.channels[0].load(Ordering::Relaxed) as usize).min(last), (self.channels[1].load(Ordering::Relaxed) as usize).min(last));
        if frames > self.block.load(Ordering::Relaxed) as usize {
            self.block.store(frames.min(u32::MAX as usize) as u32, Ordering::Relaxed);
        }
        let feeds = self.feeds.try_lock();
        let mut done = 0;
        while done < frames {
            let count = (frames - done).min(SCRATCH);
            if self.metering.load(Ordering::Relaxed) && device_channels > 0 {
                for index in 0..count {
                    left[index] = sample(done + index, from_left);
                    right[index] = sample(done + index, from_right);
                }
                for (channel, samples) in [&left[..count], &right[..count]].into_iter().enumerate() {
                    let peak = samples.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()));
                    if peak > f32::from_bits(self.peaks[channel].load(Ordering::Relaxed)) {
                        self.peaks[channel].store(peak.to_bits(), Ordering::Relaxed);
                    }
                }
            }
            // Each feed with its own channels. A feed whose ring is full (the render thread is
            // not reading) loses the newest audio here; its reader starts over when it reads
            // again.
            if let (Ok(feeds), true) = (&feeds, device_channels > 0) {
                for feed in feeds.iter() {
                    let (from_left, from_right) = ((feed.channels[0] as usize).min(last), (feed.channels[1] as usize).min(last));
                    for index in 0..count {
                        left[index] = sample(done + index, from_left);
                        right[index] = sample(done + index, from_right);
                    }
                    feed.push(time_ns, frames - done - count, &left[..count], &right[..count]);
                }
            }
            done += count;
        }
    }

    /// Render thread: the next `left.len()` frames of the chosen channels, for a block whose
    /// callback began at `clock_ns` (see [`LineFeed::read`]). False for silence (nobody
    /// listens, or nothing has arrived); the buffers are then untouched. Real-time safe.
    pub fn listen(&self, clock_ns: u64, left: &mut [f32], right: &mut [f32]) -> bool {
        let Ok(feed) = self.listen.try_lock() else { return false };
        feed.as_ref().is_some_and(|feed| feed.read(clock_ns, left, right))
    }

    /// The gain at which the chosen channels are mixed into the output, while they are.
    pub fn monitor_gain(&self) -> Option<f32> {
        self.monitoring.load(Ordering::Relaxed).then(|| f32::from_bits(self.monitor_volume.load(Ordering::Relaxed)))
    }
}

/// A running input. Dropping it stops the device and waits for its thread.
pub trait InputStream: Send {
    fn uid(&self) -> &str;
    fn channels(&self) -> u32;
    /// The nominal rate the device delivers at.
    fn sample_rate(&self) -> f64;
    /// Whether the input is certain to run on the clock of the output device `output`.
    fn shares_clock(&self, output: &str) -> bool;
    /// False once the device has failed or disappeared.
    fn is_alive(&self) -> bool;
    /// Frames per delivery, where the stream knows them.
    fn block(&self) -> Option<u32> {
        None
    }
    /// Whether the device's latency and block size are only known from its callbacks.
    fn reports_latency(&self) -> bool {
        false
    }
}

/// One sample of the null input: silence, or with `tone` a 440 Hz sine at a quarter of full
/// scale on channel 0 and an octave higher on each channel after it.
fn null_sample(tone: bool, sample_rate: f64, position: u64, channel: usize) -> f32 {
    if !tone {
        return 0.0;
    }
    let phase = position as f64 * 440.0 * (channel + 1) as f64 / sample_rate;
    (phase * std::f64::consts::TAU).sin() as f32 * 0.25
}

struct DuplexSource {
    hub: Arc<InputHub>,
    tone: bool,
    sample_rate: f64,
    position: u64,
}

/// The link between the null output device and the null input of the same engine: while both
/// run, the output's thread delivers the input, block for block before it renders, so the two
/// share one clock as the input and the output of one audio interface do.
#[derive(Default)]
pub struct NullDuplex {
    /// A null output device is running and calls [`NullDuplex::tick`].
    driven: AtomicBool,
    source: Mutex<Option<DuplexSource>>,
}

impl NullDuplex {
    /// The null output device started (or stopped) calling [`NullDuplex::tick`].
    pub(crate) fn set_driven(&self, driven: bool) {
        self.driven.store(driven, Ordering::Release);
    }

    fn is_driven(&self) -> bool {
        self.driven.load(Ordering::Acquire)
    }

    /// The null output's thread, before it renders `frames` frames.
    pub(crate) fn tick(&self, frames: usize) {
        let Ok(mut source) = self.source.try_lock() else { return };
        let Some(source) = source.as_mut() else { return };
        let (tone, rate, base) = (source.tone, source.sample_rate, source.position);
        source.hub.deliver(frames, 2, |frame, channel| null_sample(tone, rate, base + frame as u64, channel));
        source.position += frames as u64;
    }
}

enum NullDriver {
    /// A timer thread of the input's own, delivering blocks of this many frames.
    Thread { stop: Arc<AtomicBool>, thread: Option<std::thread::JoinHandle<()>>, block: u32 },
    /// The null output device's thread.
    Duplex(Arc<NullDuplex>),
    /// Nobody: a test delivers, on the clock of whoever renders or on one of its own (then
    /// in blocks of this many frames, if it says).
    Manual { own_clock: bool, block: Option<u32> },
}

/// The null input: silence (or a 440 Hz sine at a quarter of full scale on channel 0, an
/// octave higher on channel 1) delivered in real time.
pub struct NullInput {
    uid: String,
    sample_rate: f64,
    driver: NullDriver,
}

impl NullInput {
    /// A null input named `uid` with a timer thread of its own: blocks of `block` frames at
    /// `pace` frames per second of the system's clock, which the input calls `sample_rate`.
    pub fn start(hub: Arc<InputHub>, uid: &str, sample_rate: f64, pace: f64, block: usize, tone: bool) -> Result<NullInput, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let block = block.clamp(16, SCRATCH);
        let thread = std::thread::Builder::new()
            .name("audio-null-input".to_string())
            .spawn(move || {
                let period = Duration::from_secs_f64(block as f64 / pace);
                let (mut start, mut blocks, mut position) = (Instant::now(), 0u32, 0u64);
                while !flag.load(Ordering::Acquire) {
                    let base = position;
                    // A tone of 440 Hz as a device with this clock samples it.
                    hub.deliver(block, 2, |frame, channel| null_sample(tone, pace, base + frame as u64, channel));
                    position += block as u64;
                    blocks += 1;
                    let deadline = start + period * blocks;
                    let now = Instant::now();
                    if deadline > now {
                        std::thread::sleep(deadline - now);
                    } else if now - deadline > Duration::from_secs(1) || blocks > u32::MAX / 2 {
                        start = now;
                        blocks = 0;
                    }
                }
            })
            .map_err(|error| format!("The null audio input could not start: {error}"))?;
        Ok(NullInput { uid: uid.to_string(), sample_rate, driver: NullDriver::Thread { stop, thread: Some(thread), block: block as u32 } })
    }

    /// A null input delivered by the null output device's thread.
    fn duplex(duplex: Arc<NullDuplex>, hub: Arc<InputHub>, sample_rate: f64, tone: bool) -> NullInput {
        *duplex.source.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(DuplexSource { hub, tone, sample_rate, position: 0 });
        NullInput { uid: if tone { NULL_TONE_INPUT } else { NULL_DEVICE }.to_string(), sample_rate, driver: NullDriver::Duplex(duplex) }
    }
}

impl InputStream for NullInput {
    fn uid(&self) -> &str {
        &self.uid
    }
    fn channels(&self) -> u32 {
        if matches!(self.driver, NullDriver::Manual { .. }) { 8 } else { 2 }
    }
    fn sample_rate(&self) -> f64 {
        self.sample_rate
    }
    fn shares_clock(&self, _output: &str) -> bool {
        match &self.driver {
            NullDriver::Thread { .. } => false,
            NullDriver::Duplex(_) => true,
            NullDriver::Manual { own_clock, .. } => !own_clock,
        }
    }
    fn is_alive(&self) -> bool {
        match &self.driver {
            NullDriver::Thread { thread, .. } => thread.as_ref().is_some_and(|thread| !thread.is_finished()),
            NullDriver::Duplex(duplex) => duplex.is_driven(),
            NullDriver::Manual { .. } => true,
        }
    }
    fn block(&self) -> Option<u32> {
        match &self.driver {
            NullDriver::Thread { block, .. } => Some(*block),
            NullDriver::Duplex(_) => None,
            NullDriver::Manual { block, .. } => *block,
        }
    }
}

impl Drop for NullInput {
    fn drop(&mut self) {
        match &mut self.driver {
            NullDriver::Thread { stop, thread, .. } => {
                stop.store(true, Ordering::Release);
                if let Some(thread) = thread.take() {
                    let _ = thread.join();
                }
            }
            NullDriver::Duplex(duplex) => *duplex.source.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None,
            NullDriver::Manual { .. } => {}
        }
    }
}

/// The numbers after `prefix` in the `uid` of a test input: `null:free:44100:200` has 44100
/// and 200 after `null:free:`.
fn uid_numbers(uid: &str, prefix: &str) -> Option<Vec<f64>> {
    uid.strip_prefix(prefix)?.split(':').map(|part| part.parse::<f64>().ok().filter(|number| number.is_finite())).collect()
}

/// Whether audio at `input_rate` can be resampled for an engine at `output_rate`.
fn convertible(input_rate: f64, output_rate: f64) -> bool {
    (8_000.0..=384_000.0).contains(&input_rate) && RATIO_RANGE.contains(&(input_rate / output_rate))
}

/// A system input through `cpal`. Like the output, the stream lives on a thread of its own.
pub struct SystemInput {
    uid: String,
    channels: u32,
    sample_rate: f64,
    lost: Arc<AtomicBool>,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn build_input<T>(device: &cpal::Device, config: &cpal::StreamConfig, hub: Arc<InputHub>, lost: Arc<AtomicBool>) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = config.channels.max(1) as usize;
    let rate = config.sample_rate.0 as f64;
    device.build_input_stream(
        config,
        move |data: &[T], info: &cpal::InputCallbackInfo| {
            let timestamp = info.timestamp();
            if let Some(latency) = timestamp.callback.duration_since(&timestamp.capture) {
                hub.latency.store((latency.as_secs_f64() * rate) as u32, Ordering::Relaxed);
            }
            hub.deliver(data.len() / channels, channels, |frame, channel| <f32 as cpal::FromSample<T>>::from_sample_(data[frame * channels + channel]));
        },
        move |_error| lost.store(true, Ordering::Release),
        None,
    )
}

/// The configuration an input is opened with, and its rate: `preferred` Hz where the device
/// supports it, else the rate nearest to it; among those the most channels (a multi-channel
/// interface offers them all), float before integers.
fn choose_input(configs: impl IntoIterator<Item = cpal::SupportedStreamConfigRange>, preferred: u32) -> Option<(cpal::SupportedStreamConfigRange, u32)> {
    let distance = |rate: u32| (rate.max(1) as f64 / preferred.max(1) as f64).ln().abs();
    configs.into_iter().map(|config| (preferred.clamp(config.min_sample_rate().0, config.max_sample_rate().0), config)).map(|(rate, config)| (config, rate)).min_by(|(a, a_rate), (b, b_rate)| {
        distance(*a_rate)
            .total_cmp(&distance(*b_rate))
            .then(b.channels().cmp(&a.channels()))
            .then((a.sample_format() != cpal::SampleFormat::F32).cmp(&(b.sample_format() != cpal::SampleFormat::F32)))
    })
}

impl SystemInput {
    /// Opens the input named `uid` at `sample_rate` if the device supports it, else at its
    /// nearest rate, with blocks as long as `buffer_size` frames at `sample_rate`.
    pub fn start(hub: Arc<InputHub>, uid: &str, sample_rate: f64, buffer_size: u32) -> Result<SystemInput, String> {
        let lost = Arc::new(AtomicBool::new(false));
        let (stop, stopped) = mpsc::channel::<()>();
        let (ready, opened) = mpsc::channel::<Result<(u32, f64), String>>();
        let (name, flag) = (uid.to_string(), lost.clone());
        let thread = std::thread::Builder::new()
            .name("audio-input".to_string())
            .spawn(move || {
                let open = || -> Result<(cpal::Stream, u32, f64), String> {
                    let host = cpal::default_host();
                    let device = host.input_devices().map_err(|error| format!("The audio inputs could not be listed: {error}"))?.find(|device| device.name().is_ok_and(|device| device == name)).ok_or_else(|| format!("The audio input {name} is not connected."))?;
                    let configs = device.supported_input_configs().map_err(|error| format!("{name} cannot be used for input: {error}"))?;
                    let (range, rate) = choose_input(configs, sample_rate as u32).ok_or_else(|| format!("{name} offers no input format."))?;
                    if !convertible(rate as f64, sample_rate) {
                        return Err(format!("{name} records at {rate} Hz, which cannot be converted to {sample_rate} Hz. Choose another sample rate."));
                    }
                    // Blocks of the same duration as the output's.
                    let frames = (buffer_size as f64 * rate as f64 / sample_rate).round() as u32;
                    let fixed = match range.buffer_size() {
                        cpal::SupportedBufferSize::Range { min, max } => cpal::BufferSize::Fixed(frames.clamp(*min, *max)),
                        cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Default,
                    };
                    let (format, channels) = (range.sample_format(), range.channels());
                    let config = cpal::StreamConfig { channels, sample_rate: cpal::SampleRate(rate), buffer_size: fixed };
                    let stream = match format {
                        cpal::SampleFormat::F32 => build_input::<f32>(&device, &config, hub, flag),
                        cpal::SampleFormat::I16 => build_input::<i16>(&device, &config, hub, flag),
                        cpal::SampleFormat::U16 => build_input::<u16>(&device, &config, hub, flag),
                        cpal::SampleFormat::I32 => build_input::<i32>(&device, &config, hub, flag),
                        other => return Err(format!("{name} uses a sample format this version cannot read ({other}).")),
                    }
                    .map_err(|error| format!("{name} could not be opened for input: {error}"))?;
                    stream.play().map_err(|error| format!("{name} could not be started: {error}"))?;
                    Ok((stream, channels as u32, rate as f64))
                };
                match open() {
                    Ok((stream, channels, rate)) => {
                        let _ = ready.send(Ok((channels, rate)));
                        let _ = stopped.recv();
                        drop(stream);
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                    }
                }
            })
            .map_err(|error| format!("The audio input thread could not start: {error}"))?;
        match opened.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok((channels, sample_rate))) => Ok(SystemInput { uid: uid.to_string(), channels, sample_rate, lost, stop: Some(stop), thread: Some(thread) }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => Err(format!("{uid} did not answer within ten seconds.")),
        }
    }
}

impl InputStream for SystemInput {
    fn uid(&self) -> &str {
        &self.uid
    }
    fn channels(&self) -> u32 {
        self.channels
    }
    fn sample_rate(&self) -> f64 {
        self.sample_rate
    }
    fn shares_clock(&self, output: &str) -> bool {
        crate::device::same_clock(output, &self.uid)
    }
    fn is_alive(&self) -> bool {
        !self.lost.load(Ordering::Acquire) && self.thread.as_ref().is_some_and(|thread| !thread.is_finished())
    }
    fn reports_latency(&self) -> bool {
        true
    }
}

impl Drop for SystemInput {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// `audio.configure`: what to change about the input.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InputRequest {
    /// Select the input with this `uid` ("null" for the null input).
    pub uid: Option<String>,
    /// Select the input with this id ([`DeviceInfo::id`]; 0 is the null input).
    pub id: Option<u32>,
    /// Source channels: one for mono (heard on both sides), two for stereo.
    pub channels: Option<Vec<u32>>,
    pub monitor: Option<bool>,
    pub monitor_volume: Option<f32>,
    pub meter: Option<bool>,
}

/// One enabled line input of the published project.
#[derive(Debug, Clone, PartialEq)]
pub struct LineRoute {
    /// The track's index in the project.
    pub track: usize,
    /// The `uid` of the device the route was chosen on.
    pub device_uid: String,
    /// Device channels of left and right (the same for a mono input).
    pub channels: [u32; 2],
}

/// What became of a [`LineRoute`].
#[derive(Clone)]
pub enum LineState {
    /// The input is open and feeds the route. `input_latency` is what the device reported when
    /// it was opened, in the engine's frames.
    Live { feed: Arc<LineFeed>, input_latency: u32 },
    /// Nothing feeds the route, for this reason. It stays in the project.
    Paused(String),
}

impl PartialEq for LineState {
    fn eq(&self, other: &LineState) -> bool {
        match (self, other) {
            (LineState::Live { feed, input_latency }, LineState::Live { feed: other_feed, input_latency: other_latency }) => Arc::ptr_eq(feed, other_feed) && input_latency == other_latency,
            (LineState::Paused(reason), LineState::Paused(other)) => reason == other,
            _ => false,
        }
    }
}

impl std::fmt::Debug for LineState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LineState::Live { feed, input_latency } => write!(formatter, "Live(channels {:?}, {:?}, input latency {input_latency})", feed.channels, feed.clock),
            LineState::Paused(reason) => write!(formatter, "Paused({reason})"),
        }
    }
}

/// What the feeds of the open stream are made for: fixed while the stream is open, so that
/// the mixer's compensation does not move.
#[derive(Clone, Copy)]
struct StreamFormat {
    /// The latency the device reported when it was opened, in the engine's frames.
    latency: u32,
    clock: FeedClock,
}

struct State {
    stream: Option<Box<dyn InputStream>>,
    /// The chosen input; `None` follows the system's default input.
    wanted: Option<String>,
    channels: Vec<u32>,
    monitor: bool,
    monitor_volume: f32,
    meter: bool,
    /// A take is being recorded from the input.
    recording: bool,
    error: Option<String>,
    sample_rate: f64,
    buffer_size: u32,
    /// The `uid` of the output device the engine renders to (empty without one).
    output: String,
    /// The largest block the output device has rendered at once, where it was measured.
    output_block: u32,
    /// The enabled line inputs of the published project.
    lines: Vec<LineRoute>,
    /// Line inputs may use the system's default input when none was chosen (an engine with a
    /// device; an offline engine never opens an input by itself).
    follow_default: bool,
    /// The feeds of the live routes.
    feeds: Vec<(LineRoute, Arc<LineFeed>)>,
    /// The feed of the chosen channels, while monitoring or a take listens to them.
    listen: Option<Arc<LineFeed>>,
    /// What the feeds of the open stream are made for, once something asked.
    format: Option<StreamFormat>,
    /// The format changed under the routes' feeds: they are to be resolved again.
    lines_outdated: bool,
    /// The filter of feeds that resample, for the two rates it was made for.
    kernel: Option<(f64, f64, Arc<Kernel>)>,
    /// What the feeds of the open stream have measured of the clocks' difference (`f32` bits
    /// of parts per million; NaN while they have not).
    known_drift: Arc<AtomicU32>,
    /// When opening the input for line inputs alone failed last.
    line_failed: Option<Instant>,
    /// The system's default input and when it was asked for.
    default_input: Option<(Instant, Option<String>)>,
}

/// The control side of the input. See the module documentation.
pub struct Inputs {
    hub: Arc<InputHub>,
    duplex: Arc<NullDuplex>,
    state: Mutex<State>,
}

impl Default for Inputs {
    fn default() -> Inputs {
        Inputs {
            hub: Arc::new(InputHub::new()),
            duplex: Arc::default(),
            state: Mutex::new(State {
                stream: None,
                wanted: None,
                channels: vec![0, 1],
                monitor: false,
                monitor_volume: 0.5,
                meter: false,
                recording: false,
                error: None,
                sample_rate: 48_000.0,
                buffer_size: 512,
                output: String::new(),
                output_block: 0,
                lines: Vec::new(),
                follow_default: false,
                feeds: Vec::new(),
                listen: None,
                format: None,
                lines_outdated: false,
                kernel: None,
                known_drift: Arc::new(AtomicU32::new(f32::NAN.to_bits())),
                line_failed: None,
                default_input: None,
            }),
        }
    }
}

impl Inputs {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The hub, for the render stage's monitoring.
    pub fn hub(&self) -> &Arc<InputHub> {
        &self.hub
    }

    /// What the engine gives its null output device, so that it delivers the null input.
    pub(crate) fn null_duplex(&self) -> &Arc<NullDuplex> {
        &self.duplex
    }

    /// The engine's format changed (or its device did): the input follows. `output` is the
    /// `uid` of the device the engine renders to, empty for an engine without one.
    pub fn set_format(&self, sample_rate: f64, buffer_size: u32, output: &str) {
        let mut state = self.lock();
        if state.sample_rate == sample_rate && state.buffer_size == buffer_size && state.output == output {
            return;
        }
        state.sample_rate = sample_rate;
        state.buffer_size = buffer_size;
        state.output = output.to_string();
        state.output_block = 0;
        if state.stream.is_some() {
            // Another rate, another block or another clock to keep up with: the stream and
            // its feeds are made anew.
            state.stream = None;
            state.format = None;
            self.reconcile(&mut state);
            state.lines_outdated = !state.lines.is_empty();
        }
    }

    /// The largest block the output device has rendered in one callback (a device may use
    /// another size than it was asked for). The feeds are made for it.
    pub fn set_output_block(&self, frames: u32) {
        let mut state = self.lock();
        if frames <= state.output_block.max(state.buffer_size) {
            return;
        }
        state.output_block = frames;
        if state.format.take().is_some() {
            self.listen_follows(&mut state);
            state.lines_outdated = !state.lines.is_empty();
        }
    }

    /// The system's default input, without enumerating devices on a machine that has no sound
    /// card, and at most every two seconds.
    fn system_default_input(state: &mut State) -> Option<String> {
        if let Some((asked, uid)) = &state.default_input {
            if asked.elapsed() < RETRY {
                return uid.clone();
            }
        }
        let uid = crate::device::system::may_have_outputs().then(|| crate::device::system::list().default_input).flatten();
        state.default_input = Some((Instant::now(), uid.clone()));
        uid
    }

    /// Whether the null input is offered: the engine runs on the null output.
    fn null_available(state: &State) -> bool {
        state.output == NULL_DEVICE
    }

    /// The `uid` of the input in effect: the chosen one, else the system's default, else the
    /// null input where it is offered.
    fn effective(state: &mut State) -> Option<String> {
        state.wanted.clone().or_else(|| Self::system_default_input(state)).or_else(|| Self::null_available(state).then(|| NULL_DEVICE.to_string()))
    }

    /// The `uid` of the input line inputs may use.
    fn line_device(state: &mut State) -> Option<String> {
        if state.wanted.is_some() || state.follow_default { Self::effective(state) } else { None }
    }

    /// Opens the input named `uid`.
    fn open(&self, state: &State, uid: &str) -> Result<Box<dyn InputStream>, String> {
        let (rate, block) = (state.sample_rate, state.buffer_size as usize);
        let boxed = |stream: NullInput| Box::new(stream) as Box<dyn InputStream>;
        let unusable = || format!("The test input {uid} has no rate that can be converted to {rate} Hz.");
        if uid == NULL_DEVICE || uid == NULL_TONE_INPUT {
            let tone = uid == NULL_TONE_INPUT;
            if self.duplex.is_driven() {
                return Ok(boxed(NullInput::duplex(self.duplex.clone(), self.hub.clone(), rate, tone)));
            }
            return NullInput::start(self.hub.clone(), uid, rate, rate, block, tone).map(boxed);
        }
        if uid == NULL_MANUAL_INPUT {
            return Ok(boxed(NullInput { uid: uid.to_string(), sample_rate: rate, driver: NullDriver::Manual { own_clock: false, block: None } }));
        }
        if let Some(numbers) = uid_numbers(uid, "null:manual:") {
            let own = numbers.first().copied().filter(|own| convertible(*own, rate)).ok_or_else(unusable)?;
            let block = numbers.get(1).map(|block| block.clamp(1.0, 8192.0) as u32);
            return Ok(boxed(NullInput { uid: uid.to_string(), sample_rate: own, driver: NullDriver::Manual { own_clock: true, block } }));
        }
        if let Some(numbers) = uid_numbers(uid, NULL_FREE_INPUT) {
            let own = numbers.first().copied().filter(|own| convertible(*own, rate)).ok_or_else(unusable)?;
            let pace = own * (1.0 + numbers.get(1).copied().unwrap_or(0.0).clamp(-10_000.0, 10_000.0) * 1e-6);
            return NullInput::start(self.hub.clone(), uid, own, pace, (block as f64 * own / rate).round() as usize, true).map(boxed);
        }
        // Without a sound card there is nothing to enumerate (and ALSA is slow to say so).
        if !crate::device::system::may_have_outputs() {
            return Err(format!("The audio input {uid} is not connected."));
        }
        SystemInput::start(self.hub.clone(), uid, rate, state.buffer_size).map(|stream| Box::new(stream) as Box<dyn InputStream>)
    }

    /// Opens the stream when something uses the input, closes it when nothing does, and
    /// gives the chosen channels a feed while monitoring or a take listens.
    fn reconcile(&self, state: &mut State) {
        let listened = state.meter || state.monitor || state.recording;
        self.hub.metering.store(state.meter, Ordering::Relaxed);
        self.hub.monitor_volume.store(state.monitor_volume.to_bits(), Ordering::Relaxed);
        let channels = [state.channels.first().copied().unwrap_or(0), state.channels.get(1).or(state.channels.first()).copied().unwrap_or(0)];
        self.hub.channels[0].store(channels[0], Ordering::Relaxed);
        self.hub.channels[1].store(channels[1], Ordering::Relaxed);
        let routed = !state.lines.is_empty() && Self::line_device(state).is_some_and(|uid| state.lines.iter().any(|route| route.device_uid == uid));
        if !listened && !routed {
            state.stream = None;
            state.format = None;
            self.listen_follows(state);
            return;
        }
        let wanted = Self::effective(state);
        if state.stream.as_ref().is_some_and(|stream| stream.is_alive() && Some(stream.uid()) == wanted.as_deref()) {
            self.listen_follows(state);
            return;
        }
        // An input that only line inputs want is not tried again at every publication.
        if !listened && state.stream.is_none() && state.line_failed.is_some_and(|failed| failed.elapsed() < RETRY) {
            self.listen_follows(state);
            return;
        }
        state.stream = None;
        state.format = None;
        self.hub.block.store(0, Ordering::Relaxed);
        self.hub.latency.store(0, Ordering::Relaxed);
        let opened = match wanted.as_deref() {
            None => Err("No audio input device is connected.".to_string()),
            Some(uid) => self.open(state, uid),
        };
        match opened {
            Ok(stream) => {
                state.stream = Some(stream);
                state.error = None;
                state.line_failed = None;
                // Another stream may be another device: its clock is measured anew.
                state.known_drift = Arc::new(AtomicU32::new(f32::NAN.to_bits()));
            }
            Err(error) => {
                state.error = Some(error);
                state.line_failed = Some(Instant::now());
            }
        }
        self.listen_follows(state);
    }

    /// What the feeds of the open stream are made for. The first call after the stream was
    /// opened waits for a device's first callbacks, which say how late it is and how large
    /// its blocks are.
    fn format(&self, state: &mut State) -> Option<StreamFormat> {
        let stream = state.stream.as_ref().filter(|stream| stream.is_alive())?;
        if let Some(format) = state.format {
            return Some(format);
        }
        let measured = stream.reports_latency();
        if measured {
            let deadline = Instant::now() + Duration::from_millis(300);
            while self.hub.block.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let delivered = self.hub.block.load(Ordering::Relaxed);
        let (input_rate, output_rate) = (stream.sample_rate(), state.sample_rate);
        let output_block = state.buffer_size.max(state.output_block);
        let clock = if input_rate == output_rate && stream.shares_clock(&state.output) {
            FeedClock::Shared { block: if measured { delivered.max(output_block) } else { state.buffer_size }.clamp(16, 8192) }
        } else {
            // The block the input was asked for lasts as long as the output's.
            let asked = (state.buffer_size as f64 * input_rate / output_rate).round() as u32;
            let input_block = stream.block().unwrap_or(if measured && delivered > 0 { delivered } else { asked });
            FeedClock::Separate { input_rate, output_rate, input_block: input_block.clamp(1, 65_536), output_block: output_block.clamp(16, 8192) }
        };
        let latency = (self.hub.latency.load(Ordering::Relaxed) as f64 * output_rate / input_rate).round() as u32;
        state.format = Some(StreamFormat { latency, clock });
        state.format
    }

    /// Whether the open stream delivers larger blocks than its feeds were made for.
    fn outgrown(&self, state: &State) -> bool {
        let delivered = self.hub.block.load(Ordering::Relaxed);
        let measured = state.stream.as_ref().is_some_and(|stream| stream.reports_latency() && stream.block().is_none());
        measured
            && match state.format.map(|format| format.clock) {
                Some(FeedClock::Shared { block }) => delivered.min(8192) > block,
                Some(FeedClock::Separate { input_block, .. }) => delivered.min(65_536) > input_block,
                None => false,
            }
    }

    /// A feed of `channels` on `clock`.
    fn feed(state: &mut State, channels: [u32; 2], clock: FeedClock) -> Arc<LineFeed> {
        let kernel = match clock {
            FeedClock::Shared { .. } => None,
            FeedClock::Separate { input_rate, output_rate, .. } => {
                if !state.kernel.as_ref().is_some_and(|(input, output, _)| (*input, *output) == (input_rate, output_rate)) {
                    state.kernel = Some((input_rate, output_rate, Arc::new(Kernel::new(input_rate, output_rate))));
                }
                state.kernel.as_ref().map(|(_, _, kernel)| kernel.clone())
            }
        };
        Arc::new(LineFeed::new(channels, clock, kernel.as_ref().map(|kernel| (kernel, &state.known_drift))))
    }

    /// Gives the chosen channels a feed while monitoring or a take listens to them and the
    /// stream is open, and takes it away otherwise.
    fn listen_follows(&self, state: &mut State) {
        let clock = if state.monitor || state.recording { self.format(state).map(|format| format.clock) } else { None };
        let channels = [self.hub.channels[0].load(Ordering::Relaxed), self.hub.channels[1].load(Ordering::Relaxed)];
        let kept = state.listen.clone().filter(|feed| feed.channels == channels && Some(feed.clock) == clock);
        let listen = match (clock, kept) {
            (None, _) => None,
            (Some(_), Some(feed)) => Some(feed),
            (Some(clock), None) => Some(Self::feed(state, channels, clock)),
        };
        let same = match (&listen, &state.listen) {
            (Some(new), Some(old)) => Arc::ptr_eq(new, old),
            (None, None) => true,
            _ => false,
        };
        state.listen = listen;
        if !same {
            self.hand_over(state);
        }
        self.hub.monitoring.store(state.monitor && state.listen.is_some(), Ordering::Relaxed);
    }

    /// Gives the input thread the feeds as they are now, and the render thread the chosen
    /// channels' feed.
    fn hand_over(&self, state: &State) {
        let list = Arc::new(state.feeds.iter().map(|(_, feed)| feed.clone()).chain(state.listen.clone()).collect::<Vec<_>>());
        let previous = std::mem::replace(&mut *self.hub.feeds.lock().unwrap_or_else(|poisoned| poisoned.into_inner()), list);
        // The list the input thread read until now is freed here, not on its thread; and so
        // is the feed the render thread read.
        drop(previous);
        let previous = std::mem::replace(&mut *self.hub.listen.lock().unwrap_or_else(|poisoned| poisoned.into_inner()), state.listen.clone());
        drop(previous);
    }

    /// `audio.configure`: the input device, its channels, monitoring and metering.
    pub fn configure(&self, request: &InputRequest) -> Result<(), String> {
        let mut state = self.lock();
        let chosen = match (&request.uid, request.id) {
            (Some(uid), _) => Some(uid.clone()),
            (None, Some(0)) => Some(NULL_DEVICE.to_string()),
            (None, Some(id)) if id == device_id(NULL_TONE_INPUT) && state.wanted.as_deref() == Some(NULL_TONE_INPUT) => Some(NULL_TONE_INPUT.to_string()),
            (None, Some(id)) => Some(crate::device::system::list().inputs.into_iter().find(|device| device.id == id).ok_or("That audio input is not connected.")?.uid),
            (None, None) => None,
        };
        if let Some(uid) = chosen {
            if state.recording && Some(&uid) != Self::effective(&mut state).as_ref() {
                return Err("Finish recording before reconfiguring audio.".to_string());
            }
            if state.wanted.as_ref() != Some(&uid) {
                // Another device is asked for now, not after the retry pause.
                state.line_failed = None;
            }
            state.wanted = Some(uid);
        }
        if let Some(channels) = &request.channels {
            if channels.is_empty() || channels.len() > 2 || channels.iter().any(|channel| *channel >= 256) {
                return Err("Choose one input channel (mono) or two (stereo).".to_string());
            }
            state.channels = channels.clone();
        }
        if let Some(volume) = request.monitor_volume.filter(|volume| volume.is_finite()) {
            state.monitor_volume = volume.clamp(0.0, 2.0);
        }
        state.monitor = request.monitor.unwrap_or(state.monitor);
        state.meter = request.meter.unwrap_or(state.meter);
        self.reconcile(&mut state);
        match (&state.error, state.meter || state.monitor) {
            (Some(error), true) => Err(error.clone()),
            _ => Ok(()),
        }
    }

    /// The `uid` of the chosen input, if one was chosen (for the settings file).
    pub fn chosen(&self) -> Option<String> {
        self.lock().wanted.clone()
    }

    /// The enabled line inputs of the project that was just published, in any order: opens the
    /// input when a route needs it (closes it when nothing does any more) and answers with what
    /// became of each route, in the order given. `follow_default` lets the routes use the
    /// system's default input when no input was chosen.
    pub fn set_line_routes(&self, routes: &[LineRoute], follow_default: bool) -> Vec<LineState> {
        let mut state = self.lock();
        let had = !state.lines.is_empty();
        state.lines = routes.to_vec();
        state.follow_default = follow_default;
        state.lines_outdated = false;
        if routes.is_empty() && !had {
            return Vec::new();
        }
        self.reconcile(&mut state);
        let device = Self::line_device(&mut state);
        let stream = state.stream.as_ref().filter(|stream| stream.is_alive()).map(|stream| (stream.uid().to_string(), stream.channels()));
        let open = stream.as_ref().filter(|(uid, _)| Some(uid) == device.as_ref());
        let format = if open.is_some() { self.format(&mut state) } else { None };
        let known = std::mem::take(&mut state.feeds);
        let mut feeds = Vec::new();
        let states: Vec<LineState> = routes
            .iter()
            .map(|route| {
                let Some(uid) = device.as_deref() else { return LineState::Paused("No audio input device is connected.".to_string()) };
                if route.device_uid != uid {
                    return LineState::Paused(format!("{} is not the audio input in use. Its input is paused until it is.", route.device_uid));
                }
                let (Some((_, channels)), Some(format)) = (open, format) else { return LineState::Paused(state.error.clone().unwrap_or_else(|| format!("The audio input {uid} could not be opened."))) };
                if let Some(missing) = route.channels.iter().find(|channel| **channel >= *channels) {
                    return LineState::Paused(format!("{uid} has {channels} input channel{}: input {} does not exist.", if *channels == 1 { "" } else { "s" }, missing + 1));
                }
                // The same route keeps its feed, and with it the audio in flight.
                let feed = match known.iter().find(|(known, feed)| known == route && feed.clock == format.clock) {
                    Some((_, feed)) => feed.clone(),
                    None => Self::feed(&mut state, route.channels, format.clock),
                };
                feeds.push((route.clone(), feed.clone()));
                LineState::Live { feed, input_latency: format.latency }
            })
            .collect();
        // The input thread gets a new list only when the feeds changed: most publications (a
        // note edit) leave them as they are, and then its list is not touched at all.
        let same = feeds.len() == known.len() && feeds.iter().zip(&known).all(|((_, feed), (_, known))| Arc::ptr_eq(feed, known));
        state.feeds = feeds;
        if !same {
            self.hand_over(&state);
        }
        states
    }

    /// Whether the line inputs are to be resolved again: one waits for its device and opening
    /// it may be tried again, or the open stream turned out to need other feeds.
    pub fn line_retry_due(&self) -> bool {
        let mut state = self.lock();
        if state.lines.is_empty() {
            return false;
        }
        if state.lines_outdated {
            return true;
        }
        if state.stream.is_some() || state.line_failed.is_none_or(|failed| failed.elapsed() < RETRY) {
            return false;
        }
        Self::line_device(&mut state).is_some_and(|uid| state.lines.iter().any(|route| route.device_uid == uid))
    }

    /// A take from the input begins: opens the stream if need be and has the chosen channels
    /// fed to the render thread, which records them ([`crate::live`]). Returns how late they
    /// are there, in the engine's frames: the latency the device reported and the feed's.
    pub fn begin_take(&self, monitor: Option<(bool, f32)>) -> Result<u32, String> {
        let mut state = self.lock();
        state.recording = true;
        if let Some((monitor, volume)) = monitor {
            state.monitor = monitor;
            state.monitor_volume = if volume.is_finite() { volume.clamp(0.0, 2.0) } else { state.monitor_volume };
        }
        self.reconcile(&mut state);
        let (Some(format), Some(feed)) = (state.format, state.listen.clone()) else {
            let error = state.error.clone().unwrap_or_else(|| "The audio input could not be opened.".to_string());
            state.recording = false;
            self.reconcile(&mut state);
            return Err(error);
        };
        Ok(format.latency + feed.latency())
    }

    /// The take has ended: the input lets go of it.
    pub fn end_take(&self) {
        let mut state = self.lock();
        state.recording = false;
        self.reconcile(&mut state);
    }

    /// Channels a take of the current selection has: 1 when one source channel is chosen.
    pub fn take_channels(&self) -> u16 {
        if self.lock().channels.len() == 1 { 1 } else { 2 }
    }

    /// Frames the input lags by, as the device reported last, in the engine's frames (0 for
    /// the null input).
    pub fn latency_frames(&self) -> u32 {
        let state = self.lock();
        let scale = state.stream.as_ref().map_or(1.0, |stream| state.sample_rate / stream.sample_rate());
        (self.hub.latency.load(Ordering::Relaxed) as f64 * scale).round() as u32
    }

    /// The meter's peaks since the last call, left and right.
    pub fn take_peaks(&self) -> (f32, f32) {
        (f32::from_bits(self.hub.peaks[0].swap(0, Ordering::Relaxed)), f32::from_bits(self.hub.peaks[1].swap(0, Ordering::Relaxed)))
    }

    /// Device loss: an input that died is closed and its loss noted (a take from it goes on
    /// in silence; the meter and monitoring come back when the device does, at the next
    /// change of configuration; line inputs are paused and tried again every two seconds).
    /// Returns true when the input in use changed. Also looks after a device whose blocks
    /// turned out larger than its feeds were made for.
    pub fn watch(&self) -> bool {
        let mut state = self.lock();
        if state.stream.as_ref().is_some_and(|stream| !stream.is_alive()) {
            let uid = state.stream.take().map(|stream| stream.uid().to_string()).unwrap_or_default();
            state.format = None;
            state.line_failed = Some(Instant::now());
            state.error = Some(format!("The audio input {uid} was disconnected."));
            self.listen_follows(&mut state);
            return true;
        }
        if self.outgrown(&state) {
            state.format = None;
            self.listen_follows(&mut state);
            state.lines_outdated = !state.lines.is_empty();
        }
        false
    }

    /// The null input's entry for the device list, where it is offered.
    pub fn null_input(&self) -> Option<DeviceInfo> {
        let state = self.lock();
        Self::null_available(&state).then(|| DeviceInfo { id: 0, uid: NULL_DEVICE.to_string(), name: "No audio input (silence)".to_string(), input_channels: 2, output_channels: 0, sample_rate: state.sample_rate, buffer_size: state.buffer_size })
    }

    /// The test tone input's entry for the device list, while it is the chosen input.
    pub fn tone_input(&self) -> Option<DeviceInfo> {
        let state = self.lock();
        (state.wanted.as_deref() == Some(NULL_TONE_INPUT)).then(|| DeviceInfo {
            id: device_id(NULL_TONE_INPUT),
            uid: NULL_TONE_INPUT.to_string(),
            name: "Test tone input".to_string(),
            input_channels: 2,
            output_channels: 0,
            sample_rate: state.sample_rate,
            buffer_size: state.buffer_size,
        })
    }

    /// What `audio.devices` reports about the input.
    pub fn status(&self) -> serde_json::Value {
        let mut state = self.lock();
        let effective = Self::effective(&mut state);
        let stream = state.stream.as_ref().filter(|stream| stream.is_alive());
        serde_json::json!({
            "inputDevice": effective.as_deref().map(device_id), "inputUid": effective,
            "inputChannels": state.channels, "inputMetering": state.meter, "monitoring": state.monitor, "monitor": state.monitor, "monitorVolume": state.monitor_volume,
            "inputRunning": stream.is_some(), "inputError": state.error,
            "inputDeviceChannels": state.stream.as_ref().map(|stream| stream.channels()),
            "inputSampleRate": stream.map(|stream| stream.sample_rate()),
        })
    }

    /// Whether the meter is on (the status then carries the input's peaks).
    pub fn metering(&self) -> bool {
        self.lock().meter
    }

    /// Closes the input (the engine shuts down).
    pub fn close(&self) {
        let mut state = self.lock();
        state.meter = false;
        state.monitor = false;
        state.recording = false;
        state.lines.clear();
        state.feeds.clear();
        self.reconcile(&mut state);
        self.hand_over(&state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ramp by frame, so every frame of a feed says where it came from.
    fn deliver(hub: &InputHub, from: usize, frames: usize) {
        hub.deliver(frames, 8, |frame, channel| (from + frame) as f32 + channel as f32 * 0.25);
    }

    fn live(inputs: &Inputs, routes: &[LineRoute]) -> Vec<Arc<LineFeed>> {
        inputs.set_line_routes(routes, false).into_iter().map(|state| match state { LineState::Live { feed, input_latency: 0 } => feed, other => panic!("not live: {other:?}") }).collect()
    }

    fn route(track: usize, channels: [u32; 2]) -> LineRoute {
        LineRoute { track, device_uid: NULL_MANUAL_INPUT.to_string(), channels }
    }

    #[test]
    fn a_feed_starts_a_target_behind_and_says_what_it_does_when_the_clocks_disagree() {
        let inputs = Inputs::default();
        inputs.set_format(48_000.0, 64, "");
        inputs.configure(&InputRequest { uid: Some(NULL_MANUAL_INPUT.to_string()), ..InputRequest::default() }).unwrap();
        let feeds = live(&inputs, &[route(0, [2, 2]), route(1, [0, 1])]);
        let (mono, stereo) = (&feeds[0], &feeds[1]);
        assert_eq!((mono.clock(), mono.latency(), mono.limit, mono.drift_ppm()), (FeedClock::Shared { block: 64 }, 64, 1216, None));
        let hub = inputs.hub();
        let (mut left, mut right) = ([9.0f32; 32], [9.0f32; 32]);
        // Nothing yet, then less than the target beyond a block: silence, buffers untouched.
        assert!(!mono.read(0, &mut left, &mut right));
        deliver(hub, 0, 80);
        assert!(!mono.read(0, &mut left, &mut right) && left[0] == 9.0);
        // 100 frames wait: the reader starts at frame 100 - 64 - 32 = 4, a target behind.
        deliver(hub, 80, 20);
        assert!(mono.read(0, &mut left, &mut right));
        assert_eq!((left[0], left[31], right[0]), (4.5, 35.5, 4.5), "channel 2 on both sides");
        assert!(stereo.read(0, &mut left, &mut right));
        assert_eq!((left[0], right[0]), (4.0, 4.25), "channels 0 and 1");
        // In step, block for block, nothing is lost and nothing is counted.
        for block in 0..50 {
            deliver(hub, 100 + block * 32, 32);
            assert!(mono.read(0, &mut left, &mut right));
            assert_eq!(left[0], 36.5 + (block * 32) as f32);
        }
        assert_eq!((mono.underruns(), mono.overruns()), (0, 0));
        // The input stalls: the block gets what waits (64 frames, so two blocks), then a
        // partly silent block, and the feed starts over once a target waits again.
        assert!(mono.read(0, &mut left, &mut right) && mono.read(0, &mut left, &mut right));
        assert_eq!(mono.underruns(), 0);
        deliver(hub, 1_700, 8);
        assert!(mono.read(0, &mut left, &mut right));
        assert_eq!((left[7], left[8], mono.underruns()), (1_707.5, 0.0, 1));
        deliver(hub, 1_708, 90);
        assert!(!mono.read(0, &mut left, &mut right), "90 frames are less than a target and a block");
        deliver(hub, 1_798, 10);
        assert!(mono.read(0, &mut left, &mut right));
        assert_eq!(left[0], 1_712.5, "started over a target behind");
        // The output stalls: more than the limit waits, and the reader drops down to a target.
        deliver(hub, 1_808, 2_000);
        assert!(mono.read(0, &mut left, &mut right));
        assert_eq!((left[0], mono.overruns()), ((3_808 - 64 - 32) as f32 + 0.5, 1));
        // Each feed has its own reader: the other one still holds everything since its read.
        assert_eq!((stereo.underruns(), stereo.overruns()), (0, 0));
    }

    #[test]
    fn routes_are_live_on_the_input_in_use_and_paused_with_a_reason_otherwise() {
        let inputs = Inputs::default();
        inputs.set_format(48_000.0, 128, "");
        // An engine without a device never opens the system's default input for a route.
        let states = inputs.set_line_routes(&[route(0, [0, 1])], false);
        assert_eq!(states, [LineState::Paused("No audio input device is connected.".to_string())]);
        assert_eq!(inputs.status()["inputRunning"], false);
        inputs.set_line_routes(&[], false);
        inputs.configure(&InputRequest { uid: Some(NULL_MANUAL_INPUT.to_string()), ..InputRequest::default() }).unwrap();
        assert_eq!(inputs.status()["inputRunning"], false, "choosing an input opens nothing while no route uses it");
        let other = LineRoute { track: 2, device_uid: "Interface".to_string(), channels: [0, 1] };
        let states = inputs.set_line_routes(&[route(0, [0, 1]), route(1, [7, 8]), other.clone()], false);
        assert!(matches!(&states[0], LineState::Live { feed, input_latency: 0 } if feed.latency() == 128));
        assert_eq!(states[1], LineState::Paused("null:manual has 8 input channels: input 9 does not exist.".to_string()));
        assert_eq!(states[2], LineState::Paused("Interface is not the audio input in use. Its input is paused until it is.".to_string()));
        assert_eq!(inputs.status()["inputRunning"], true, "a live route opens the input");
        // The same route keeps its feed (and with it the audio in flight); another block size
        // is another feed.
        let again = inputs.set_line_routes(&[route(0, [0, 1])], false);
        assert_eq!(again[0], states[0]);
        inputs.set_format(48_000.0, 256, "");
        assert!(inputs.line_retry_due(), "the routes are to be resolved for the new format");
        let resized = inputs.set_line_routes(&[route(0, [0, 1])], false);
        assert!(resized[0] != states[0] && matches!(&resized[0], LineState::Live { feed, .. } if feed.latency() == 256));
        // Routes of another device only: the input closes.
        inputs.set_line_routes(&[other], false);
        assert_eq!(inputs.status()["inputRunning"], false);
        inputs.set_line_routes(&[route(0, [0, 1])], false);
        inputs.set_line_routes(&[], false);
        assert_eq!(inputs.status()["inputRunning"], false, "no route, no input");
        assert!(!inputs.line_retry_due());
    }

    #[test]
    fn an_input_opens_at_the_engines_rate_or_at_its_nearest_and_one_clock_is_only_assumed_where_it_is_certain() {
        use cpal::SampleFormat::{F32, I16};
        let range = |channels, min, max, format| cpal::SupportedStreamConfigRange::new(channels, cpal::SampleRate(min), cpal::SampleRate(max), cpal::SupportedBufferSize::Unknown, format);
        let chosen = |configs: Vec<cpal::SupportedStreamConfigRange>, preferred| choose_input(configs, preferred).map(|(config, rate)| (rate, config.channels(), config.sample_format()));
        // A device that has the engine's rate: with the most channels, float before integers.
        let interface = vec![range(2, 44_100, 44_100, I16), range(2, 48_000, 48_000, F32), range(8, 48_000, 48_000, I16), range(8, 48_000, 48_000, F32), range(8, 96_000, 96_000, F32)];
        assert_eq!(chosen(interface, 48_000), Some((48_000, 8, F32)));
        // A microphone that has not: its nearest rate, whichever side it is on.
        assert_eq!(chosen(vec![range(1, 16_000, 16_000, I16), range(1, 44_100, 44_100, I16)], 48_000), Some((44_100, 1, I16)));
        assert_eq!(chosen(vec![range(2, 22_050, 22_050, F32), range(2, 88_200, 192_000, F32)], 48_000), Some((88_200, 2, F32)));
        assert_eq!(chosen(vec![range(2, 8_000, 32_000, F32)], 44_100), Some((32_000, 2, F32)));
        assert_eq!(chosen(Vec::new(), 48_000), None);
        assert!(convertible(44_100.0, 48_000.0) && convertible(8_000.0, 96_000.0) && convertible(192_000.0, 44_100.0));
        assert!(!convertible(1_000.0, 48_000.0) && !convertible(384_000.0, 44_100.0) && !convertible(8_000.0, 192_000.0));

        // Two names are one clock only when they are the same and name one device for certain.
        use crate::device::same_clock;
        assert!(!same_clock("", "") && !same_clock("Scarlett 2i2 USB", "USB Microphone") && !same_clock("null", "Scarlett 2i2 USB"));
        assert_eq!(same_clock("hw:CARD=USB,DEV=0", "hw:CARD=USB,DEV=0"), cfg!(any(target_os = "linux", target_os = "macos")));
        assert!(!same_clock("hw:CARD=USB,DEV=0", "plughw:CARD=USB,DEV=0"));
        // A sound server's door on Linux may lead to different hardware on each side.
        assert_eq!(same_clock("default", "default"), cfg!(target_os = "macos"));
        assert_eq!(same_clock("pipewire", "pipewire"), cfg!(target_os = "macos"));
    }

    /// An input with a clock of its own at `rate` in blocks of `block`, routed as one stereo
    /// line input of an engine at 48 kHz with blocks of `output_block`.
    fn separate(rate: u32, block: u32, output_block: u32) -> (Inputs, Arc<LineFeed>) {
        let inputs = Inputs::default();
        inputs.set_format(48_000.0, output_block, "Speakers");
        let uid = format!("null:manual:{rate}:{block}");
        inputs.configure(&InputRequest { uid: Some(uid.clone()), ..InputRequest::default() }).unwrap();
        let states = inputs.set_line_routes(&[LineRoute { track: 0, device_uid: uid, channels: [0, 1] }], false);
        let LineState::Live { feed, input_latency: 0 } = &states[0] else { panic!("not live: {states:?}") };
        (inputs, feed.clone())
    }

    #[test]
    fn a_feed_of_a_second_clock_says_its_delay_and_keeps_it() {
        // What a feed keeps waiting: an output block, an input block, half the filter (and
        // two frames), and the margin for late callbacks.
        let (_inputs, feed) = separate(48_000, 512, 512);
        assert_eq!(feed.clock(), FeedClock::Separate { input_rate: 48_000.0, output_rate: 48_000.0, input_block: 512, output_block: 512 });
        assert_eq!(feed.latency(), 512 + 512 + 44 + 2 + 240);
        assert_eq!(separate(48_000, 64, 64).1.latency(), 64 + 64 + 44 + 2 + 48);
        // From 44.1 kHz in blocks of 441 frames (10 ms): 256 output frames are 235.2 input
        // frames, the margin is 5 ms; in output frames the whole is 1.088 times as much.
        assert_eq!(separate(44_100, 441, 256).1.latency(), ((235.2f64 + 441.0 + 46.0 + 220.5).ceil() * 48_000.0 / 44_100.0).round() as u32);
        // An output that turns out to ask for larger blocks than it was opened with: the
        // routes are resolved again, and the feed is one for that block.
        let (inputs, feed) = separate(48_000, 512, 512);
        inputs.set_output_block(512);
        assert!(!inputs.line_retry_due());
        inputs.set_output_block(1_024);
        assert!(inputs.line_retry_due());
        let states = inputs.set_line_routes(&[LineRoute { track: 0, device_uid: "null:manual:48000:512".to_string(), channels: [0, 1] }], false);
        let LineState::Live { feed: larger, .. } = &states[0] else { panic!("not live: {states:?}") };
        assert!(!Arc::ptr_eq(&feed, larger) && larger.latency() == 1_024 + 512 + 44 + 2 + 240 && !inputs.line_retry_due());
        // A rate that cannot be converted is refused with a reason.
        let inputs = Inputs::default();
        inputs.set_format(48_000.0, 256, "Speakers");
        inputs.configure(&InputRequest { uid: Some("null:manual:1000".to_string()), ..InputRequest::default() }).unwrap();
        let states = inputs.set_line_routes(&[LineRoute { track: 0, device_uid: "null:manual:1000".to_string(), channels: [0, 1] }], false);
        assert_eq!(states, [LineState::Paused("The test input null:manual:1000 has no rate that can be converted to 48000 Hz.".to_string())]);

        // The input runs 250 ppm fast; each side's callbacks come when its clock says. The
        // signal is a ramp that wraps, so every output frame says which input position it is.
        let (inputs, feed) = separate(48_000, 480, 512);
        let hub = inputs.hub();
        let input_rate = 48_000.0 * 1.00025;
        let (mut left, mut right) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        assert!(!feed.read(1, &mut left, &mut right), "nothing has arrived: silence");
        let (mut delivered, mut worst, mut heard) = (0u64, 0.0f64, 0usize);
        for block in 0..5_700u64 {
            let now = 0.02 + block as f64 * 512.0 / 48_000.0;
            while (delivered + 1) as f64 * 480.0 / input_rate <= now {
                delivered += 1;
                let base = (delivered - 1) * 480;
                hub.deliver_at((delivered as f64 * 480.0 / input_rate * 1e9) as u64, 480, 8, |frame, channel| ((base + frame as u64) % 4_800) as f32 / 4_800.0 + channel as f32);
            }
            assert_eq!(feed.read((now * 1e9) as u64, &mut left, &mut right), block >= 9, "silent while it measures, for a tenth of a second");
            // After 45 seconds: the input position of the block's first frame, from the ramp
            // (which is interpolated exactly except where it wraps), against where the input
            // is at this callback.
            if block > 4_200 && left[0] > 0.05 && left[0] < 0.95 {
                let delay = (now * input_rate - left[0] as f64 * 4_800.0).rem_euclid(4_800.0);
                worst = worst.max((delay - feed.latency() as f64).abs());
                heard += 1;
                assert!((right[0] - left[0] - 1.0).abs() < 1e-3, "the second channel is the first one's neighbour");
            }
        }
        println!("250 ppm: measured {:+.1} ppm, the delay stays within {worst:.2} frames of {}, {} frames wait", feed.drift_ppm().unwrap(), feed.latency(), feed.waiting());
        assert!(heard > 1_000 && worst < 1.0, "{heard} blocks, {worst} frames");
        assert!((feed.drift_ppm().unwrap() - 250.0).abs() < 2.0 && feed.latency_error().unwrap().abs() < 1.0);
        assert_eq!((feed.underruns(), feed.overruns()), (0, 0));
    }
}

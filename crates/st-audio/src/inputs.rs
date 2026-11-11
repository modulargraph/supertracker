//! Audio inputs: one input stream per engine, and everything that listens to it.
//!
//! ```text
//!  input thread (cpal callback, the null input's timer thread, or the null output's thread)
//!     InputHub::deliver(frames)
//!        the chosen channels as stereo    -->  meter peaks (atomics)
//!                                         -->  the take's Tap      (crate::recording)
//!                                         -->  monitor ring  ~~~>  render thread (crate::live), mixed into the output
//!        the channels of each line input  -->  its LineFeed  ~~~>  render thread (crate::graph::plan), the input of a track strip
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
//! # Two clocks
//!
//! The input runs at the engine's sample rate; a device that cannot is refused with a message.
//! There is no resampling between the input's clock and the output's, so each consumer on the
//! render thread reads through a ring and says what it does when the two disagree. The monitor
//! ring drops the oldest audio when it runs ahead and plays silence when it runs dry. A
//! [`LineFeed`] is stricter, because the mixer compensates for its delay:
//!
//! * **Start.** The reader plays silence until `target` frames wait beyond the block it is about
//!   to read, drops what is older, and starts. From then on a frame is mixed `target` frames
//!   after it was delivered (plus the phase between the two callbacks, less than a block).
//!   `target` is one block of the device: the cushion for the callbacks' jitter.
//! * **Underrun** (less waits than the block needs: the input's clock is the slower one, or the
//!   input stalled): the block gets what is there and silence after it, and the feed starts
//!   over. That is a gap of at least `target` frames; [`LineFeed::underruns`] counts them.
//! * **Overrun** (more than three times `target` plus 1024 frames wait beyond the block: the
//!   input's clock is the faster one, or the output stalled): the oldest frames are dropped
//!   down to `target`. That is a skip; [`LineFeed::overruns`] counts them.
//!
//! An interface used for input and for output has one clock, and neither ever happens. Two
//! separate devices drift apart by a few frames per second at worst, which at 512-frame blocks
//! means a gap or a skip every few minutes. Both are counted and reported with the route.
//!
//! Real-time rules: [`InputHub::deliver`], [`InputHub::monitor`] and [`LineFeed::read`] do not
//! allocate, block or perform I/O. The hub's mutexes (the take's tap, the list of line feeds)
//! are only ever `try_lock`ed on the input thread and held elsewhere for the time of an
//! assignment; a block that finds one taken skips that consumer.

use crate::device::{device_id, DeviceInfo, NULL_DEVICE};
use crate::recording::{AudioRing, Tap};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The `uid` of the null input fed by a test tone (tests only; listed only while it is chosen).
pub const NULL_TONE_INPUT: &str = "null:tone";
/// The `uid` of a null input that delivers nothing by itself: a test calls
/// [`InputHub::deliver`] (tests only; never listed). It has eight channels.
pub const NULL_MANUAL_INPUT: &str = "null:manual";
/// Frames of scratch per delivery step.
const SCRATCH: usize = 4096;
/// How long a failed attempt to open the input for line inputs is not repeated.
const RETRY: Duration = Duration::from_secs(2);

/// One line input: the ring between the input thread and the render thread. See the module
/// documentation for what the reader does when the two clocks disagree.
pub struct LineFeed {
    /// Device channels of left and right (the same for a mono input).
    channels: [u32; 2],
    ring: AudioRing,
    target: usize,
    limit: usize,
    /// The reader has started (render thread only).
    primed: AtomicBool,
    underruns: AtomicU64,
    overruns: AtomicU64,
}

impl LineFeed {
    /// A feed of `channels` whose reader keeps `block` frames waiting.
    fn new(channels: [u32; 2], block: usize) -> LineFeed {
        let target = block.max(16);
        let limit = 3 * target + 1024;
        LineFeed {
            channels,
            // Room for the limit, the largest block either side moves at once, and a margin.
            ring: AudioRing::new((limit + 3 * 8192).next_power_of_two()),
            target,
            limit,
            primed: AtomicBool::new(false),
            underruns: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
        }
    }

    /// Frames between a frame's delivery and its place in the mix: the feed's part of the
    /// input's latency.
    pub fn target(&self) -> usize {
        self.target
    }

    /// Times the reader ran dry and started over (a gap in the input).
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    /// Times the reader dropped audio because too much waited (a skip in the input).
    pub fn overruns(&self) -> u64 {
        self.overruns.load(Ordering::Relaxed)
    }

    /// Render thread: the next `left.len()` frames. False for silence (the buffers are then
    /// untouched). Real-time safe.
    pub fn read(&self, left: &mut [f32], right: &mut [f32]) -> bool {
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
    monitoring: AtomicBool,
    /// Monitor gain (`f32` bits).
    monitor_volume: AtomicU32,
    monitor: AudioRing,
    tap: Mutex<Option<Tap>>,
    /// The feeds of the live line inputs.
    lines: Mutex<Arc<Vec<Arc<LineFeed>>>>,
    /// Frames delivered so far: the input's clock.
    frames: AtomicU64,
    /// Estimated input latency in frames, as the device reports it per callback.
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
            monitor: AudioRing::new(48_000),
            tap: Mutex::new(None),
            lines: Mutex::default(),
            frames: AtomicU64::new(0),
            latency: AtomicU32::new(0),
            block: AtomicU32::new(0),
        }
    }

    /// Input thread: `frames` frames, of which `sample(frame, channel)` reads one sample.
    /// Real-time safe.
    pub fn deliver(&self, frames: usize, device_channels: usize, sample: impl Fn(usize, usize) -> f32) {
        let (mut left, mut right) = ([0.0f32; SCRATCH], [0.0f32; SCRATCH]);
        let last = device_channels.saturating_sub(1);
        let (from_left, from_right) = ((self.channels[0].load(Ordering::Relaxed) as usize).min(last), (self.channels[1].load(Ordering::Relaxed) as usize).min(last));
        if frames > self.block.load(Ordering::Relaxed) as usize {
            self.block.store(frames.min(u32::MAX as usize) as u32, Ordering::Relaxed);
        }
        let lines = self.lines.try_lock();
        let mut done = 0;
        while done < frames {
            let count = (frames - done).min(SCRATCH);
            if device_channels == 0 {
                left[..count].fill(0.0);
                right[..count].fill(0.0);
            } else {
                for index in 0..count {
                    left[index] = sample(done + index, from_left);
                    right[index] = sample(done + index, from_right);
                }
            }
            let time = self.frames.fetch_add(count as u64, Ordering::Relaxed);
            if self.metering.load(Ordering::Relaxed) {
                for (channel, samples) in [&left[..count], &right[..count]].into_iter().enumerate() {
                    let peak = samples.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()));
                    if peak > f32::from_bits(self.peaks[channel].load(Ordering::Relaxed)) {
                        self.peaks[channel].store(peak.to_bits(), Ordering::Relaxed);
                    }
                }
            }
            if let Ok(tap) = self.tap.try_lock() {
                if let Some(tap) = tap.as_ref() {
                    tap.capture(time, &left[..count], &right[..count]);
                }
            }
            if self.monitoring.load(Ordering::Relaxed) {
                // A full ring means the output is not reading (or reads slower): the newest
                // audio is dropped here, the render side skips ahead when it reads again.
                self.monitor.push(&left[..count], &right[..count]);
            }
            // The line inputs, each with its own channels. A feed whose ring is full (the
            // render thread is not reading) loses the newest audio here; its reader drops
            // down to its target when it reads again.
            if let (Ok(lines), true) = (&lines, device_channels > 0) {
                for feed in lines.iter() {
                    let (from_left, from_right) = ((feed.channels[0] as usize).min(last), (feed.channels[1] as usize).min(last));
                    for index in 0..count {
                        left[index] = sample(done + index, from_left);
                        right[index] = sample(done + index, from_right);
                    }
                    feed.ring.push(&left[..count], &right[..count]);
                }
            }
            done += count;
        }
    }

    /// Render thread: adds the monitored input to `left`/`right`. `scratch` holds at least
    /// `left.len()` frames per channel. Real-time safe.
    pub fn monitor(&self, left: &mut [f32], right: &mut [f32], scratch: (&mut [f32], &mut [f32])) {
        if !self.monitoring.load(Ordering::Relaxed) {
            return;
        }
        let frames = left.len().min(right.len()).min(scratch.0.len()).min(scratch.1.len());
        // More than a few blocks waiting is latency that would never go away: skip ahead.
        let keep = (frames * 3).max(2_048);
        while self.monitor.available() > keep + frames {
            let excess = (self.monitor.available() - keep).min(frames);
            self.monitor.pop(&mut scratch.0[..excess], &mut scratch.1[..excess]);
        }
        let count = self.monitor.pop(&mut scratch.0[..frames], &mut scratch.1[..frames]);
        let volume = f32::from_bits(self.monitor_volume.load(Ordering::Relaxed));
        for index in 0..count {
            left[index] += scratch.0[index] * volume;
            right[index] += scratch.1[index] * volume;
        }
    }
}

/// A running input. Dropping it stops the device and waits for its thread.
pub trait InputStream: Send {
    fn uid(&self) -> &str;
    fn channels(&self) -> u32;
    /// False once the device has failed or disappeared.
    fn is_alive(&self) -> bool;
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
    /// A timer thread of the input's own.
    Thread { stop: Arc<AtomicBool>, thread: Option<std::thread::JoinHandle<()>> },
    /// The null output device's thread.
    Duplex(Arc<NullDuplex>),
    /// Nobody: a test delivers.
    Manual,
}

/// The null input: silence (or a 440 Hz sine at a quarter of full scale on channel 0, an
/// octave higher on channel 1) delivered in real time.
pub struct NullInput {
    uid: String,
    driver: NullDriver,
}

impl NullInput {
    /// A null input with a timer thread of its own.
    pub fn start(hub: Arc<InputHub>, sample_rate: f64, block: usize, tone: bool) -> Result<NullInput, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("audio-null-input".to_string())
            .spawn(move || {
                let block = block.clamp(16, SCRATCH);
                let period = Duration::from_secs_f64(block as f64 / sample_rate);
                let (mut start, mut blocks, mut position) = (Instant::now(), 0u32, 0u64);
                while !flag.load(Ordering::Acquire) {
                    let base = position;
                    hub.deliver(block, 2, |frame, channel| null_sample(tone, sample_rate, base + frame as u64, channel));
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
        Ok(NullInput { uid: if tone { NULL_TONE_INPUT } else { NULL_DEVICE }.to_string(), driver: NullDriver::Thread { stop, thread: Some(thread) } })
    }

    /// A null input delivered by the null output device's thread.
    fn duplex(duplex: Arc<NullDuplex>, hub: Arc<InputHub>, sample_rate: f64, tone: bool) -> NullInput {
        *duplex.source.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(DuplexSource { hub, tone, sample_rate, position: 0 });
        NullInput { uid: if tone { NULL_TONE_INPUT } else { NULL_DEVICE }.to_string(), driver: NullDriver::Duplex(duplex) }
    }
}

impl InputStream for NullInput {
    fn uid(&self) -> &str {
        &self.uid
    }
    fn channels(&self) -> u32 {
        if matches!(self.driver, NullDriver::Manual) { 8 } else { 2 }
    }
    fn is_alive(&self) -> bool {
        match &self.driver {
            NullDriver::Thread { thread, .. } => thread.as_ref().is_some_and(|thread| !thread.is_finished()),
            NullDriver::Duplex(duplex) => duplex.is_driven(),
            NullDriver::Manual => true,
        }
    }
}

impl Drop for NullInput {
    fn drop(&mut self) {
        match &mut self.driver {
            NullDriver::Thread { stop, thread } => {
                stop.store(true, Ordering::Release);
                if let Some(thread) = thread.take() {
                    let _ = thread.join();
                }
            }
            NullDriver::Duplex(duplex) => *duplex.source.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None,
            NullDriver::Manual => {}
        }
    }
}

/// A system input through `cpal`. Like the output, the stream lives on a thread of its own.
pub struct SystemInput {
    uid: String,
    channels: u32,
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

impl SystemInput {
    /// Opens the input named `uid` at `sample_rate`, which the device must support.
    pub fn start(hub: Arc<InputHub>, uid: &str, sample_rate: f64, buffer_size: u32) -> Result<SystemInput, String> {
        let lost = Arc::new(AtomicBool::new(false));
        let (stop, stopped) = mpsc::channel::<()>();
        let (ready, opened) = mpsc::channel::<Result<u32, String>>();
        let (name, flag) = (uid.to_string(), lost.clone());
        let thread = std::thread::Builder::new()
            .name("audio-input".to_string())
            .spawn(move || {
                let open = || -> Result<(cpal::Stream, u32), String> {
                    let host = cpal::default_host();
                    let device = host.input_devices().map_err(|error| format!("The audio inputs could not be listed: {error}"))?.find(|device| device.name().is_ok_and(|device| device == name)).ok_or_else(|| format!("The audio input {name} is not connected."))?;
                    let rate = cpal::SampleRate(sample_rate as u32);
                    let range = device
                        .supported_input_configs()
                        .map_err(|error| format!("{name} cannot be used for input: {error}"))?
                        .filter(|config| config.min_sample_rate() <= rate && rate <= config.max_sample_rate())
                        // The most channels first (a multi-channel interface offers them all), float before integers.
                        .min_by_key(|config| (std::cmp::Reverse(config.channels()), config.sample_format() != cpal::SampleFormat::F32))
                        .ok_or_else(|| format!("{name} does not record at {sample_rate} Hz. Choose a sample rate the input and the output both support."))?;
                    let fixed = match range.buffer_size() {
                        cpal::SupportedBufferSize::Range { min, max } => cpal::BufferSize::Fixed(buffer_size.clamp(*min, *max)),
                        cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Default,
                    };
                    let (format, channels) = (range.sample_format(), range.channels());
                    let config = cpal::StreamConfig { channels, sample_rate: rate, buffer_size: fixed };
                    let stream = match format {
                        cpal::SampleFormat::F32 => build_input::<f32>(&device, &config, hub, flag),
                        cpal::SampleFormat::I16 => build_input::<i16>(&device, &config, hub, flag),
                        cpal::SampleFormat::U16 => build_input::<u16>(&device, &config, hub, flag),
                        cpal::SampleFormat::I32 => build_input::<i32>(&device, &config, hub, flag),
                        other => return Err(format!("{name} uses a sample format this version cannot read ({other}).")),
                    }
                    .map_err(|error| format!("{name} could not be opened for input: {error}"))?;
                    stream.play().map_err(|error| format!("{name} could not be started: {error}"))?;
                    Ok((stream, channels as u32))
                };
                match open() {
                    Ok((stream, channels)) => {
                        let _ = ready.send(Ok(channels));
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
            Ok(Ok(channels)) => Ok(SystemInput { uid: uid.to_string(), channels, lost, stop: Some(stop), thread: Some(thread) }),
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
    /// it was opened, in frames.
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
            LineState::Live { feed, input_latency } => write!(formatter, "Live(channels {:?}, target {}, input latency {input_latency})", feed.channels, feed.target),
            LineState::Paused(reason) => write!(formatter, "Paused({reason})"),
        }
    }
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
    /// The null input is offered (the engine runs on the null output).
    null_available: bool,
    /// The enabled line inputs of the published project.
    lines: Vec<LineRoute>,
    /// Line inputs may use the system's default input when none was chosen (an engine with a
    /// device; an offline engine never opens an input by itself).
    follow_default: bool,
    /// The feeds of the live routes.
    feeds: Vec<(LineRoute, Arc<LineFeed>)>,
    /// Input latency and block size of the open stream as line inputs count them, in frames:
    /// measured once per stream, so the mixer's compensation does not move while it is open.
    line_format: Option<(u32, u32)>,
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
                null_available: false,
                lines: Vec::new(),
                follow_default: false,
                feeds: Vec::new(),
                line_format: None,
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

    /// The engine's format changed (or its device did): the input follows.
    pub fn set_format(&self, sample_rate: f64, buffer_size: u32, null_available: bool) {
        let mut state = self.lock();
        let reopen = state.stream.is_some() && (state.sample_rate != sample_rate || state.buffer_size != buffer_size);
        state.sample_rate = sample_rate;
        state.buffer_size = buffer_size;
        state.null_available = null_available;
        if reopen {
            state.stream = None;
            self.reconcile(&mut state);
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

    /// The `uid` of the input in effect: the chosen one, else the system's default, else the
    /// null input where it is offered.
    fn effective(state: &mut State) -> Option<String> {
        state.wanted.clone().or_else(|| Self::system_default_input(state)).or_else(|| state.null_available.then(|| NULL_DEVICE.to_string()))
    }

    /// The `uid` of the input line inputs may use.
    fn line_device(state: &mut State) -> Option<String> {
        if state.wanted.is_some() || state.follow_default { Self::effective(state) } else { None }
    }

    /// Opens the stream when something uses the input, closes it when nothing does.
    fn reconcile(&self, state: &mut State) {
        let listened = state.meter || state.monitor || state.recording;
        self.hub.metering.store(state.meter, Ordering::Relaxed);
        self.hub.monitor_volume.store(state.monitor_volume.to_bits(), Ordering::Relaxed);
        let channels = [state.channels.first().copied().unwrap_or(0), state.channels.get(1).or(state.channels.first()).copied().unwrap_or(0)];
        self.hub.channels[0].store(channels[0], Ordering::Relaxed);
        self.hub.channels[1].store(channels[1], Ordering::Relaxed);
        let routed = !state.lines.is_empty() && Self::line_device(state).is_some_and(|uid| state.lines.iter().any(|route| route.device_uid == uid));
        if !listened && !routed {
            self.hub.monitoring.store(false, Ordering::Relaxed);
            state.stream = None;
            state.line_format = None;
            return;
        }
        let wanted = Self::effective(state);
        if state.stream.as_ref().is_some_and(|stream| stream.is_alive() && Some(stream.uid()) == wanted.as_deref()) {
            self.hub.monitoring.store(state.monitor, Ordering::Relaxed);
            return;
        }
        // An input that only line inputs want is not tried again at every publication.
        if !listened && state.stream.is_none() && state.line_failed.is_some_and(|failed| failed.elapsed() < RETRY) {
            return;
        }
        state.stream = None;
        state.line_format = None;
        self.hub.monitor.clear();
        self.hub.block.store(0, Ordering::Relaxed);
        self.hub.latency.store(0, Ordering::Relaxed);
        let null = |tone: bool| -> Result<Box<dyn InputStream>, String> {
            if self.duplex.is_driven() {
                return Ok(Box::new(NullInput::duplex(self.duplex.clone(), self.hub.clone(), state.sample_rate, tone)));
            }
            NullInput::start(self.hub.clone(), state.sample_rate, state.buffer_size as usize, tone).map(|stream| Box::new(stream) as Box<dyn InputStream>)
        };
        let opened: Result<Box<dyn InputStream>, String> = match wanted.as_deref() {
            None => Err("No audio input device is connected.".to_string()),
            Some(NULL_DEVICE) => null(false),
            Some(NULL_TONE_INPUT) => null(true),
            Some(NULL_MANUAL_INPUT) => Ok(Box::new(NullInput { uid: NULL_MANUAL_INPUT.to_string(), driver: NullDriver::Manual })),
            // Without a sound card there is nothing to enumerate (and ALSA is slow to say so).
            Some(uid) if !crate::device::system::may_have_outputs() => Err(format!("The audio input {uid} is not connected.")),
            Some(uid) => SystemInput::start(self.hub.clone(), uid, state.sample_rate, state.buffer_size).map(|stream| Box::new(stream) as Box<dyn InputStream>),
        };
        match opened {
            Ok(stream) => {
                state.stream = Some(stream);
                state.error = None;
                state.line_failed = None;
                self.hub.monitoring.store(state.monitor, Ordering::Relaxed);
            }
            Err(error) => {
                state.error = Some(error);
                state.line_failed = Some(Instant::now());
                self.hub.monitoring.store(false, Ordering::Relaxed);
            }
        }
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
        if routes.is_empty() && !had {
            return Vec::new();
        }
        self.reconcile(&mut state);
        let device = Self::line_device(&mut state);
        let stream = state.stream.as_ref().filter(|stream| stream.is_alive()).map(|stream| (stream.uid().to_string(), stream.channels(), stream.reports_latency()));
        let open = stream.as_ref().filter(|(uid, _, _)| Some(uid) == device.as_ref());
        if let (Some((_, _, measured)), None) = (open, state.line_format) {
            if *measured {
                // The first callbacks say how late the device is and how large its blocks are.
                let deadline = Instant::now() + Duration::from_millis(300);
                while self.hub.block.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            let block = if *measured { self.hub.block.load(Ordering::Relaxed).max(state.buffer_size) } else { state.buffer_size };
            state.line_format = Some((self.hub.latency.load(Ordering::Relaxed), block.clamp(16, 8192)));
        }
        let (input_latency, block) = state.line_format.unwrap_or((0, state.buffer_size));
        let mut feeds = Vec::new();
        let states: Vec<LineState> = routes
            .iter()
            .map(|route| {
                let Some(uid) = device.as_deref() else { return LineState::Paused("No audio input device is connected.".to_string()) };
                if route.device_uid != uid {
                    return LineState::Paused(format!("{} is not the audio input in use. Its input is paused until it is.", route.device_uid));
                }
                let Some((_, channels, _)) = open else { return LineState::Paused(state.error.clone().unwrap_or_else(|| format!("The audio input {uid} could not be opened."))) };
                if let Some(missing) = route.channels.iter().find(|channel| **channel >= *channels) {
                    return LineState::Paused(format!("{uid} has {channels} input channel{}: input {} does not exist.", if *channels == 1 { "" } else { "s" }, missing + 1));
                }
                let feed = state.feeds.iter().find(|(known, feed)| known == route && feed.target == block as usize).map(|(_, feed)| feed.clone()).unwrap_or_else(|| Arc::new(LineFeed::new(route.channels, block as usize)));
                feeds.push((route.clone(), feed.clone()));
                LineState::Live { feed, input_latency }
            })
            .collect();
        // The input thread gets a new list only when the feeds changed: most publications (a
        // note edit) leave them as they are, and then its list is not touched at all.
        let same = feeds.len() == state.feeds.len() && feeds.iter().zip(&state.feeds).all(|((_, feed), (_, known))| Arc::ptr_eq(feed, known));
        state.feeds = feeds;
        if !same {
            let list = Arc::new(state.feeds.iter().map(|(_, feed)| feed.clone()).collect::<Vec<_>>());
            let previous = std::mem::replace(&mut *self.hub.lines.lock().unwrap_or_else(|poisoned| poisoned.into_inner()), list);
            // The list the input thread read until now is freed here, not on its thread.
            drop(previous);
        }
        states
    }

    /// Whether a line input waits for its device and opening it may be tried again.
    pub fn line_retry_due(&self) -> bool {
        let mut state = self.lock();
        if state.lines.is_empty() || state.stream.is_some() || state.line_failed.is_none_or(|failed| failed.elapsed() < RETRY) {
            return false;
        }
        Self::line_device(&mut state).is_some_and(|uid| state.lines.iter().any(|route| route.device_uid == uid))
    }

    /// A take from the input begins: opens the stream if need be and gives it the tap.
    /// Returns the take's channel count (1 for a mono selection) on success.
    pub fn begin_take(&self, tap: Tap, monitor: Option<(bool, f32)>) -> Result<(), String> {
        let mut state = self.lock();
        state.recording = true;
        if let Some((monitor, volume)) = monitor {
            state.monitor = monitor;
            state.monitor_volume = if volume.is_finite() { volume.clamp(0.0, 2.0) } else { state.monitor_volume };
        }
        self.reconcile(&mut state);
        if let Some(error) = state.error.clone().filter(|_| state.stream.is_none()) {
            state.recording = false;
            self.reconcile(&mut state);
            return Err(error);
        }
        *self.hub.tap.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(tap);
        Ok(())
    }

    /// The take has ended (its tap is finished): the input lets go of it.
    pub fn end_take(&self) {
        *self.hub.tap.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        let mut state = self.lock();
        state.recording = false;
        self.reconcile(&mut state);
    }

    /// Channels a take of the current selection has: 1 when one source channel is chosen.
    pub fn take_channels(&self) -> u16 {
        if self.lock().channels.len() == 1 { 1 } else { 2 }
    }

    /// Frames the input lags by, as the device reported last (0 for the null input).
    pub fn latency_frames(&self) -> u32 {
        self.hub.latency.load(Ordering::Relaxed)
    }

    /// The meter's peaks since the last call, left and right.
    pub fn take_peaks(&self) -> (f32, f32) {
        (f32::from_bits(self.hub.peaks[0].swap(0, Ordering::Relaxed)), f32::from_bits(self.hub.peaks[1].swap(0, Ordering::Relaxed)))
    }

    /// Device loss: an input that died is closed and its loss noted (a take from it ends with
    /// what it has; the meter and monitoring come back when the device does, at the next
    /// change of configuration; line inputs are paused and tried again every two seconds).
    /// Returns true when the input in use changed.
    pub fn watch(&self) -> bool {
        let mut state = self.lock();
        if state.stream.as_ref().is_some_and(|stream| !stream.is_alive()) {
            let uid = state.stream.take().map(|stream| stream.uid().to_string()).unwrap_or_default();
            self.hub.monitoring.store(false, Ordering::Relaxed);
            state.line_format = None;
            state.line_failed = Some(Instant::now());
            state.error = Some(format!("The audio input {uid} was disconnected."));
            return true;
        }
        false
    }

    /// The null input's entry for the device list, where it is offered.
    pub fn null_input(&self) -> Option<DeviceInfo> {
        let state = self.lock();
        state.null_available.then(|| DeviceInfo { id: 0, uid: NULL_DEVICE.to_string(), name: "No audio input (silence)".to_string(), input_channels: 2, output_channels: 0, sample_rate: state.sample_rate, buffer_size: state.buffer_size })
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
        serde_json::json!({
            "inputDevice": effective.as_deref().map(device_id), "inputUid": effective,
            "inputChannels": state.channels, "inputMetering": state.meter, "monitoring": state.monitor, "monitor": state.monitor, "monitorVolume": state.monitor_volume,
            "inputRunning": state.stream.as_ref().is_some_and(|stream| stream.is_alive()), "inputError": state.error,
            "inputDeviceChannels": state.stream.as_ref().map(|stream| stream.channels()),
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
        let previous = std::mem::take(&mut *self.hub.lines.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
        drop(previous);
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
        inputs.set_format(48_000.0, 64, false);
        inputs.configure(&InputRequest { uid: Some(NULL_MANUAL_INPUT.to_string()), ..InputRequest::default() }).unwrap();
        let feeds = live(&inputs, &[route(0, [2, 2]), route(1, [0, 1])]);
        let (mono, stereo) = (&feeds[0], &feeds[1]);
        assert_eq!((mono.target(), mono.limit), (64, 1216));
        let hub = inputs.hub();
        let (mut left, mut right) = ([9.0f32; 32], [9.0f32; 32]);
        // Nothing yet, then less than the target beyond a block: silence, buffers untouched.
        assert!(!mono.read(&mut left, &mut right));
        deliver(hub, 0, 80);
        assert!(!mono.read(&mut left, &mut right) && left[0] == 9.0);
        // 100 frames wait: the reader starts at frame 100 - 64 - 32 = 4, a target behind.
        deliver(hub, 80, 20);
        assert!(mono.read(&mut left, &mut right));
        assert_eq!((left[0], left[31], right[0]), (4.5, 35.5, 4.5), "channel 2 on both sides");
        assert!(stereo.read(&mut left, &mut right));
        assert_eq!((left[0], right[0]), (4.0, 4.25), "channels 0 and 1");
        // In step, block for block, nothing is lost and nothing is counted.
        for block in 0..50 {
            deliver(hub, 100 + block * 32, 32);
            assert!(mono.read(&mut left, &mut right));
            assert_eq!(left[0], 36.5 + (block * 32) as f32);
        }
        assert_eq!((mono.underruns(), mono.overruns()), (0, 0));
        // The input stalls: the block gets what waits (64 frames, so two blocks), then a
        // partly silent block, and the feed starts over once a target waits again.
        assert!(mono.read(&mut left, &mut right) && mono.read(&mut left, &mut right));
        assert_eq!(mono.underruns(), 0);
        deliver(hub, 1_700, 8);
        assert!(mono.read(&mut left, &mut right));
        assert_eq!((left[7], left[8], mono.underruns()), (1_707.5, 0.0, 1));
        deliver(hub, 1_708, 90);
        assert!(!mono.read(&mut left, &mut right), "90 frames are less than a target and a block");
        deliver(hub, 1_798, 10);
        assert!(mono.read(&mut left, &mut right));
        assert_eq!(left[0], 1_712.5, "started over a target behind");
        // The output stalls: more than the limit waits, and the reader drops down to a target.
        deliver(hub, 1_808, 2_000);
        assert!(mono.read(&mut left, &mut right));
        assert_eq!((left[0], mono.overruns()), ((3_808 - 64 - 32) as f32 + 0.5, 1));
        // Each feed has its own reader: the other one still holds everything since its read.
        assert_eq!((stereo.underruns(), stereo.overruns()), (0, 0));
    }

    #[test]
    fn routes_are_live_on_the_input_in_use_and_paused_with_a_reason_otherwise() {
        let inputs = Inputs::default();
        inputs.set_format(48_000.0, 128, false);
        // An engine without a device never opens the system's default input for a route.
        let states = inputs.set_line_routes(&[route(0, [0, 1])], false);
        assert_eq!(states, [LineState::Paused("No audio input device is connected.".to_string())]);
        assert_eq!(inputs.status()["inputRunning"], false);
        inputs.set_line_routes(&[], false);
        inputs.configure(&InputRequest { uid: Some(NULL_MANUAL_INPUT.to_string()), ..InputRequest::default() }).unwrap();
        assert_eq!(inputs.status()["inputRunning"], false, "choosing an input opens nothing while no route uses it");
        let other = LineRoute { track: 2, device_uid: "Interface".to_string(), channels: [0, 1] };
        let states = inputs.set_line_routes(&[route(0, [0, 1]), route(1, [7, 8]), other.clone()], false);
        assert!(matches!(&states[0], LineState::Live { feed, input_latency: 0 } if feed.target() == 128));
        assert_eq!(states[1], LineState::Paused("null:manual has 8 input channels: input 9 does not exist.".to_string()));
        assert_eq!(states[2], LineState::Paused("Interface is not the audio input in use. Its input is paused until it is.".to_string()));
        assert_eq!(inputs.status()["inputRunning"], true, "a live route opens the input");
        // The same route keeps its feed (and with it the audio in flight); another block size
        // is another feed.
        let again = inputs.set_line_routes(&[route(0, [0, 1])], false);
        assert_eq!(again[0], states[0]);
        inputs.set_format(48_000.0, 256, false);
        let resized = inputs.set_line_routes(&[route(0, [0, 1])], false);
        assert!(resized[0] != states[0] && matches!(&resized[0], LineState::Live { feed, .. } if feed.target() == 256));
        // Routes of another device only: the input closes.
        inputs.set_line_routes(&[other], false);
        assert_eq!(inputs.status()["inputRunning"], false);
        inputs.set_line_routes(&[route(0, [0, 1])], false);
        inputs.set_line_routes(&[], false);
        assert_eq!(inputs.status()["inputRunning"], false, "no route, no input");
        assert!(!inputs.line_retry_due());
    }
}

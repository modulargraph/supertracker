//! Audio inputs: a device's input stream, its meter, monitoring, and where a take of it comes
//! from.
//!
//! ```text
//!  input device thread (cpal callback, or the null input's timer thread)
//!     InputHub::deliver(frames)  -- chosen channels to stereo -->  meter peaks (atomics)
//!                                                              -->  the take's Tap        (crate::recording)
//!                                                              -->  monitor ring  ~~~>  render thread (crate::live), mixed into the output
//! ```
//!
//! [`Inputs`] is the control side (any control thread): which device and channels, whether the
//! input is metered or monitored. The stream is open only while something uses it: the meter,
//! monitoring, or a take. Devices come from `cpal` (ALSA on Linux; CoreAudio on macOS,
//! type-checked only); the *null input* (`uid` "null") needs no hardware: a timer thread that
//! delivers silence (or, for tests, a sine) in real time. It is offered while the engine runs on
//! the null output device.
//!
//! The input runs at the engine's sample rate; a device that cannot is refused with a message
//! (there is no resampling between the two clocks). The two devices' clocks still drift:
//! monitoring reads through a small ring that drops the oldest audio when it runs ahead and
//! plays silence when it runs dry.
//!
//! Real-time rules: [`InputHub::deliver`] and [`InputHub::monitor`] do not allocate, block or
//! perform I/O; the hub's two mutexes (the take's tap, the channel choice) are only ever
//! `try_lock`ed there and held elsewhere for the time of an assignment.

use crate::device::{device_id, DeviceInfo, NULL_DEVICE};
use crate::recording::{AudioRing, Tap};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The `uid` of the null input fed by a test tone (tests only; not listed).
pub const NULL_TONE_INPUT: &str = "null:tone";
/// Frames of scratch per delivery step.
const SCRATCH: usize = 4096;

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
    /// Frames delivered so far: the input's clock.
    frames: AtomicU64,
    /// Estimated input latency in frames, as the device reports it per callback.
    latency: AtomicU32,
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
            frames: AtomicU64::new(0),
            latency: AtomicU32::new(0),
        }
    }

    /// Input thread: `frames` frames, of which `sample(frame, channel)` reads one sample.
    /// Real-time safe.
    pub fn deliver(&self, frames: usize, device_channels: usize, sample: impl Fn(usize, usize) -> f32) {
        let (mut left, mut right) = ([0.0f32; SCRATCH], [0.0f32; SCRATCH]);
        let last = device_channels.saturating_sub(1);
        let (from_left, from_right) = ((self.channels[0].load(Ordering::Relaxed) as usize).min(last), (self.channels[1].load(Ordering::Relaxed) as usize).min(last));
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
}

/// The null input: silence (or a 440 Hz sine at a quarter of full scale on channel 0, an
/// octave higher on channel 1) delivered in real time by a timer thread.
pub struct NullInput {
    uid: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl NullInput {
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
                    hub.deliver(block, 2, |frame, channel| {
                        if !tone {
                            return 0.0;
                        }
                        let phase = (base + frame as u64) as f64 * 440.0 * (channel + 1) as f64 / sample_rate;
                        (phase * std::f64::consts::TAU).sin() as f32 * 0.25
                    });
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
        Ok(NullInput { uid: if tone { NULL_TONE_INPUT } else { NULL_DEVICE }.to_string(), stop, thread: Some(thread) })
    }
}

impl InputStream for NullInput {
    fn uid(&self) -> &str {
        &self.uid
    }
    fn channels(&self) -> u32 {
        2
    }
    fn is_alive(&self) -> bool {
        self.thread.as_ref().is_some_and(|thread| !thread.is_finished())
    }
}

impl Drop for NullInput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
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
}

impl Drop for SystemInput {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The system's default input, without enumerating devices on a machine that has no sound card.
fn system_default_input() -> Option<String> {
    crate::device::system::may_have_outputs().then(|| crate::device::system::list().default_input).flatten()
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
}

/// The control side of the input. See the module documentation.
pub struct Inputs {
    hub: Arc<InputHub>,
    state: Mutex<State>,
}

impl Default for Inputs {
    fn default() -> Inputs {
        Inputs {
            hub: Arc::new(InputHub::new()),
            state: Mutex::new(State { stream: None, wanted: None, channels: vec![0, 1], monitor: false, monitor_volume: 0.5, meter: false, recording: false, error: None, sample_rate: 48_000.0, buffer_size: 512, null_available: false }),
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

    /// The `uid` of the input in effect: the chosen one, else the system's default, else the
    /// null input where it is offered.
    fn effective(state: &State) -> Option<String> {
        state.wanted.clone().or_else(system_default_input).or_else(|| state.null_available.then(|| NULL_DEVICE.to_string()))
    }

    /// Opens the stream when something uses the input, closes it when nothing does.
    fn reconcile(&self, state: &mut State) {
        let needed = state.meter || state.monitor || state.recording;
        self.hub.metering.store(state.meter, Ordering::Relaxed);
        self.hub.monitor_volume.store(state.monitor_volume.to_bits(), Ordering::Relaxed);
        let channels = [state.channels.first().copied().unwrap_or(0), state.channels.get(1).or(state.channels.first()).copied().unwrap_or(0)];
        self.hub.channels[0].store(channels[0], Ordering::Relaxed);
        self.hub.channels[1].store(channels[1], Ordering::Relaxed);
        if !needed {
            self.hub.monitoring.store(false, Ordering::Relaxed);
            state.stream = None;
            return;
        }
        let wanted = Self::effective(state);
        if state.stream.as_ref().is_some_and(|stream| stream.is_alive() && Some(stream.uid()) == wanted.as_deref()) {
            self.hub.monitoring.store(state.monitor, Ordering::Relaxed);
            return;
        }
        state.stream = None;
        self.hub.monitor.clear();
        let opened: Result<Box<dyn InputStream>, String> = match wanted.as_deref() {
            None => Err("No audio input device is connected.".to_string()),
            Some(NULL_DEVICE) => NullInput::start(self.hub.clone(), state.sample_rate, state.buffer_size as usize, false).map(|stream| Box::new(stream) as Box<dyn InputStream>),
            Some(NULL_TONE_INPUT) => NullInput::start(self.hub.clone(), state.sample_rate, state.buffer_size as usize, true).map(|stream| Box::new(stream) as Box<dyn InputStream>),
            Some(uid) => SystemInput::start(self.hub.clone(), uid, state.sample_rate, state.buffer_size).map(|stream| Box::new(stream) as Box<dyn InputStream>),
        };
        match opened {
            Ok(stream) => {
                state.stream = Some(stream);
                state.error = None;
                self.hub.monitoring.store(state.monitor, Ordering::Relaxed);
            }
            Err(error) => {
                state.error = Some(error);
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
            (None, Some(id)) => Some(crate::device::system::list().inputs.into_iter().find(|device| device.id == id).ok_or("That audio input is not connected.")?.uid),
            (None, None) => None,
        };
        if let Some(uid) = chosen {
            if state.recording && Some(&uid) != Self::effective(&state).as_ref() {
                return Err("Finish recording before reconfiguring audio.".to_string());
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
    /// change of configuration). Returns true when the input in use changed.
    pub fn watch(&self) -> bool {
        let mut state = self.lock();
        if state.stream.as_ref().is_some_and(|stream| !stream.is_alive()) {
            let uid = state.stream.take().map(|stream| stream.uid().to_string()).unwrap_or_default();
            self.hub.monitoring.store(false, Ordering::Relaxed);
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

    /// What `audio.devices` reports about the input.
    pub fn status(&self) -> serde_json::Value {
        let state = self.lock();
        let effective = Self::effective(&state);
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
        self.reconcile(&mut state);
    }
}

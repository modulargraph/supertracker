//! The system's audio devices, through `cpal`.
//!
//! `cpal` streams are not `Send` on every platform, so a stream lives on a thread of its own
//! that builds it, starts it and then waits to be told to stop ([`SystemOutput`]). The audio
//! callback runs on the system's real-time thread: it renders into planar scratch buffers
//! allocated before the stream starts and interleaves them into the device's buffer in the
//! device's sample format; the first two device channels carry left and right (a mono device
//! gets their mean), further channels are silent. Output is clamped to full scale here, at the
//! device, and nowhere before.
//!
//! Devices have no identity beyond their name in `cpal`; the name is the `uid`.

use super::{device_id, DeviceInfo, OutputStream, SharedRenderer};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

/// The devices of the default host.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SystemDevices {
    pub inputs: Vec<DeviceInfo>,
    pub outputs: Vec<DeviceInfo>,
    /// `uid`s of the system's default devices.
    pub default_input: Option<String>,
    pub default_output: Option<String>,
}

fn describe(device: &cpal::Device, output: bool) -> Option<DeviceInfo> {
    let name = device.name().ok()?;
    let input = device.default_input_config().ok();
    let out = device.default_output_config().ok();
    let config = if output { out.as_ref() } else { input.as_ref() }?;
    let buffer_size = match config.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => 512u32.clamp(*min, *max),
        cpal::SupportedBufferSize::Unknown => 512,
    };
    Some(DeviceInfo {
        id: device_id(&name),
        uid: name.clone(),
        name,
        input_channels: input.as_ref().map_or(0, |config| config.channels() as u32),
        output_channels: out.as_ref().map_or(0, |config| config.channels() as u32),
        sample_rate: config.sample_rate().0 as f64,
        buffer_size,
    })
}

/// Lists the system's devices. A host without audio gives empty lists.
pub fn list() -> SystemDevices {
    let host = cpal::default_host();
    let mut devices = SystemDevices::default();
    if let Ok(outputs) = host.output_devices() {
        devices.outputs = outputs.filter_map(|device| describe(&device, true)).collect();
    }
    if let Ok(inputs) = host.input_devices() {
        devices.inputs = inputs.filter_map(|device| describe(&device, false)).collect();
    }
    devices.default_output = host.default_output_device().and_then(|device| device.name().ok()).filter(|name| devices.outputs.iter().any(|device| device.uid == *name));
    devices.default_input = host.default_input_device().and_then(|device| device.name().ok()).filter(|name| devices.inputs.iter().any(|device| device.uid == *name));
    devices
}

/// A cheap look at whether the system can have an output at all, for the periodic check that
/// brings a device back: on Linux, whether any sound card is registered (enumerating ALSA
/// devices on a machine without one is slow and writes to standard error each time).
pub fn may_have_outputs() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/asound/cards").is_ok_and(|cards| cards.contains('['))
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

/// The sample rate an output would run at when asked for `preferred`: that rate if the device
/// supports it, else the device's own default.
pub fn output_rate(uid: &str, preferred: f64) -> Result<f64, String> {
    let device = find_output(uid)?;
    let wanted = cpal::SampleRate(preferred as u32);
    let supported = device
        .supported_output_configs()
        .map(|mut configs| configs.any(|config| config.min_sample_rate() <= wanted && wanted <= config.max_sample_rate()))
        .unwrap_or(false);
    if supported {
        return Ok(preferred);
    }
    device.default_output_config().map(|config| config.sample_rate().0 as f64).map_err(|error| format!("{uid} cannot be used for output: {error}"))
}

fn find_output(uid: &str) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    let mut devices = host.output_devices().map_err(|error| format!("The audio devices could not be listed: {error}"))?;
    devices.find(|device| device.name().is_ok_and(|name| name == uid)).ok_or_else(|| format!("The audio device {uid} is not connected."))
}

/// A running system output.
pub struct SystemOutput {
    uid: String,
    sample_rate: f64,
    buffer_size: u32,
    /// The largest callback so far, in frames.
    largest: Arc<AtomicU32>,
    lost: Arc<AtomicBool>,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn write_interleaved<T: cpal::SizedSample + cpal::FromSample<f32>>(data: &mut [T], channels: usize, offset: usize, left: &[f32], right: &[f32]) {
    for (frame, (l, r)) in data[offset * channels..].chunks_mut(channels).zip(left.iter().zip(right)) {
        let (l, r) = (l.clamp(-1.0, 1.0), r.clamp(-1.0, 1.0));
        match frame {
            [mono] => *mono = T::from_sample((l + r) * 0.5),
            [first, second, rest @ ..] => {
                *first = T::from_sample(l);
                *second = T::from_sample(r);
                rest.fill(T::from_sample(0.0));
            }
            [] => {}
        }
    }
}

fn build<T: cpal::SizedSample + cpal::FromSample<f32>>(device: &cpal::Device, config: &cpal::StreamConfig, renderer: SharedRenderer, lost: Arc<AtomicBool>, largest: Arc<AtomicU32>) -> Result<cpal::Stream, cpal::BuildStreamError> {
    let channels = config.channels.max(1) as usize;
    // Scratch for the largest block rendered in one go; larger callbacks are rendered in parts.
    let (mut left, mut right) = (vec![0.0f32; 8192], vec![0.0f32; 8192]);
    device.build_output_stream(
        config,
        move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
            let frames = data.len() / channels;
            largest.fetch_max(frames.min(u32::MAX as usize) as u32, Ordering::Relaxed);
            // The time from this callback to the moment its first frame leaves the device.
            let timestamp = info.timestamp();
            let playback_delay = timestamp.playback.duration_since(&timestamp.callback).unwrap_or_default();
            super::render_blocks(&renderer, &mut left, &mut right, frames, playback_delay, |offset, l, r| write_interleaved(data, channels, offset, l, r));
        },
        move |_error| lost.store(true, Ordering::Release),
        None,
    )
}

impl SystemOutput {
    /// Opens the output named `uid` at `sample_rate` (which the device must support, see
    /// [`output_rate`]) with blocks of `buffer_size` frames where the device allows it.
    pub fn start(renderer: SharedRenderer, uid: &str, sample_rate: f64, buffer_size: u32) -> Result<SystemOutput, String> {
        let lost = Arc::new(AtomicBool::new(false));
        let (stop, stopped) = mpsc::channel::<()>();
        let (ready, opened) = mpsc::channel::<Result<(), String>>();
        let (name, flag) = (uid.to_string(), lost.clone());
        let largest = Arc::new(AtomicU32::new(0));
        let measured = largest.clone();
        let thread = std::thread::Builder::new()
            .name("audio-device".to_string())
            .spawn(move || {
                let open = || -> Result<cpal::Stream, String> {
                    let device = find_output(&name)?;
                    let rate = cpal::SampleRate(sample_rate as u32);
                    let range = device
                        .supported_output_configs()
                        .map_err(|error| format!("{name} cannot be used for output: {error}"))?
                        .filter(|config| config.min_sample_rate() <= rate && rate <= config.max_sample_rate())
                        // Stereo first, then anything wider, then mono; float before integer formats.
                        .min_by_key(|config| (if config.channels() == 2 { 0 } else if config.channels() > 2 { 1 } else { 2 }, config.sample_format() != cpal::SampleFormat::F32))
                        .ok_or_else(|| format!("{name} does not support {sample_rate} Hz."))?;
                    let fixed = match range.buffer_size() {
                        cpal::SupportedBufferSize::Range { min, max } => cpal::BufferSize::Fixed(buffer_size.clamp(*min, *max)),
                        cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Default,
                    };
                    let format = range.sample_format();
                    let config = cpal::StreamConfig { channels: range.channels(), sample_rate: rate, buffer_size: fixed };
                    let stream = match format {
                        cpal::SampleFormat::F32 => build::<f32>(&device, &config, renderer, flag, measured),
                        cpal::SampleFormat::I16 => build::<i16>(&device, &config, renderer, flag, measured),
                        cpal::SampleFormat::U16 => build::<u16>(&device, &config, renderer, flag, measured),
                        cpal::SampleFormat::I32 => build::<i32>(&device, &config, renderer, flag, measured),
                        other => return Err(format!("{name} uses a sample format this version cannot write ({other}).")),
                    }
                    .map_err(|error| format!("{name} could not be opened: {error}"))?;
                    stream.play().map_err(|error| format!("{name} could not be started: {error}"))?;
                    Ok(stream)
                };
                match open() {
                    Ok(stream) => {
                        let _ = ready.send(Ok(()));
                        // Keeps the stream alive until the owner drops its end of the channel.
                        let _ = stopped.recv();
                        drop(stream);
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                    }
                }
            })
            .map_err(|error| format!("The audio device thread could not start: {error}"))?;
        match opened.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(SystemOutput { uid: uid.to_string(), sample_rate, buffer_size, largest, lost, stop: Some(stop), thread: Some(thread) }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => Err(format!("{uid} did not answer within ten seconds.")),
        }
    }
}

impl OutputStream for SystemOutput {
    fn uid(&self) -> &str {
        &self.uid
    }
    fn sample_rate(&self) -> f64 {
        self.sample_rate
    }
    fn buffer_size(&self) -> u32 {
        self.buffer_size
    }
    fn largest_block(&self) -> u32 {
        self.largest.load(Ordering::Relaxed).max(self.buffer_size)
    }
    fn is_alive(&self) -> bool {
        !self.lost.load(Ordering::Acquire) && self.thread.as_ref().is_some_and(|thread| !thread.is_finished())
    }
}

impl Drop for SystemOutput {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

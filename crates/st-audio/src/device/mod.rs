//! Audio devices: listing, opening an output on the renderer, and the null device.
//!
//! An output is a thread (the system's audio callback thread, or the null device's timer thread)
//! that calls [`Renderer::render`] once per block. The renderer sits in a mutex shared with the
//! control side, which takes it only for exclusive work on a stopped graph (replacing the engine
//! after a sample-rate change, an offline render through the live engine). The audio thread
//! never waits for it: it `try_lock`s, and a block that finds the mutex taken is silence.
//!
//! Real devices come from `cpal` ([`system`]): ALSA on Linux (the build needs `libasound2-dev`),
//! CoreAudio on macOS (type-checked from Linux; not yet run on a Mac). The [`null`] device needs
//! nothing: it renders to nowhere at a fixed rate and block size, which is what a machine
//! without an audio device, the tests and a `--serve` run on a server use.

pub mod null;
pub mod system;

use crate::graph::render::Renderer;
use serde::Serialize;
use std::sync::{Arc, Mutex};

/// The renderer as devices share it with the control side.
pub type SharedRenderer = Arc<Mutex<Renderer>>;

/// The `uid` of the null device.
pub const NULL_DEVICE: &str = "null";

/// One device as the interface lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    /// A number that stays the same for a device name (the interface selects by it).
    pub id: u32,
    /// The device's name as the system reports it; `null` for the null device.
    pub uid: String,
    pub name: String,
    pub input_channels: u32,
    pub output_channels: u32,
    pub sample_rate: f64,
    pub buffer_size: u32,
}

/// The id of a device name: FNV-1a, 31 bits, never 0 (0 is the null device).
pub fn device_id(uid: &str) -> u32 {
    if uid == NULL_DEVICE {
        return 0;
    }
    let mut hash: u32 = 0x811c_9dc5;
    for byte in uid.bytes() {
        hash = (hash ^ byte as u32).wrapping_mul(0x0100_0193);
    }
    (hash & 0x7fff_ffff).max(1)
}

/// The null device's entry in the output list.
pub fn null_device_info(sample_rate: f64, buffer_size: u32) -> DeviceInfo {
    DeviceInfo { id: 0, uid: NULL_DEVICE.to_string(), name: "No audio device (silent)".to_string(), input_channels: 0, output_channels: 2, sample_rate, buffer_size }
}

/// A running output. Dropping it stops the device and waits for its thread.
pub trait OutputStream: Send {
    /// The `uid` of the device.
    fn uid(&self) -> &str;
    fn sample_rate(&self) -> f64;
    /// Frames per block (the device may deliver other sizes; this is what was asked for).
    fn buffer_size(&self) -> u32;
    /// The largest block the device has asked for in one callback so far, where that is
    /// measured: a device may use another size than it was asked for.
    fn largest_block(&self) -> u32 {
        self.buffer_size()
    }
    /// False once the device has failed or disappeared.
    fn is_alive(&self) -> bool;
}

/// Whether the input device `input` and the output device `output` (their `uid`s) are certain
/// to run on one clock, so that audio can pass between them frame for frame. Only then: any
/// other pair is treated as two clocks and resampled ([`crate::inputs`]), which costs latency
/// but is right in both cases.
///
/// `cpal` knows a device by its name only. On macOS the same name on both sides is one Core
/// Audio device (an aggregate device included, whose members Core Audio keeps together). On
/// Linux the same name is one clock only when it names a sound card (`hw:CARD=...`,
/// `front:CARD=...`, `sysdefault:CARD=...`): `default`, `pulse` or `pipewire` are a sound
/// server's doors, behind which the input and the output may be different hardware.
pub fn same_clock(output: &str, input: &str) -> bool {
    if output.is_empty() || output != input {
        return false;
    }
    if cfg!(target_os = "macos") {
        true
    } else if cfg!(target_os = "linux") {
        output.contains("CARD=")
    } else {
        false
    }
}

/// Buffer sizes the interface offers.
pub const BUFFER_SIZES: [u32; 6] = [64, 128, 256, 512, 1024, 2048];

/// Renders `frames` frames into planar scratch buffers and hands them to `write`, block by
/// block. A renderer that is busy elsewhere gives silence. `playback_delay` is what the device
/// says lies between this callback and the moment its first frame is heard; scheduled MIDI is
/// delayed by it so external instruments sound with the engine. Call it as the device's
/// callback begins: it reads the monotonic clock for the renderer, which an input device with
/// a clock of its own is followed by ([`Renderer::render_at`]). Real-time safe.
#[inline]
pub(crate) fn render_blocks(
    renderer: &SharedRenderer,
    left: &mut [f32],
    right: &mut [f32],
    frames: usize,
    playback_delay: std::time::Duration,
    mut write: impl FnMut(usize, &[f32], &[f32]),
) {
    let capacity = left.len().min(right.len()).max(1);
    let began = crate::graph::insert::monotonic_ns();
    let mut done = 0;
    while done < frames {
        let count = (frames - done).min(capacity);
        match renderer.try_lock() {
            Ok(mut renderer) => {
                renderer.midi().set_playback_delay(playback_delay);
                let clock_ns = began + (done as f64 / renderer.sample_rate() * 1e9) as u64;
                renderer.render_at(clock_ns, &mut left[..count], &mut right[..count]);
            }
            Err(_) => {
                left[..count].fill(0.0);
                right[..count].fill(0.0);
            }
        }
        write(done, &left[..count], &right[..count]);
        done += count;
    }
}

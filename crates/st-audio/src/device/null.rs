//! The null device: a timer-driven thread that renders to nowhere.
//!
//! It keeps the engine's clock, sequencer, meters and status running in real time on a machine
//! without an audio device. Blocks are rendered at their real-time pace (`buffer_size /
//! sample_rate` seconds apart, without drift: each deadline is computed from the start); when
//! the thread falls more than a second behind (a suspended machine), it starts over from now
//! instead of rendering the backlog.

use super::{OutputStream, SharedRenderer, NULL_DEVICE};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct NullDevice {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    sample_rate: f64,
    buffer_size: u32,
}

impl NullDevice {
    /// Starts rendering `buffer_size` frames at a time at the pace of `sample_rate`.
    pub fn start(renderer: SharedRenderer, sample_rate: f64, buffer_size: u32) -> Result<NullDevice, String> {
        let buffer_size = buffer_size.clamp(16, 8192);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("audio-null-device".to_string())
            .spawn(move || {
                let frames = buffer_size as usize;
                let (mut left, mut right) = (vec![0.0f32; frames], vec![0.0f32; frames]);
                let period = Duration::from_secs_f64(frames as f64 / sample_rate);
                let mut start = Instant::now();
                let mut blocks: u32 = 0;
                while !flag.load(Ordering::Acquire) {
                    super::render_blocks(&renderer, &mut left, &mut right, frames, |_, _, _| {});
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
            .map_err(|error| format!("The null audio device could not start: {error}"))?;
        Ok(NullDevice { stop, thread: Some(thread), sample_rate, buffer_size })
    }
}

impl OutputStream for NullDevice {
    fn uid(&self) -> &str {
        NULL_DEVICE
    }
    fn sample_rate(&self) -> f64 {
        self.sample_rate
    }
    fn buffer_size(&self) -> u32 {
        self.buffer_size
    }
    fn is_alive(&self) -> bool {
        self.thread.as_ref().is_some_and(|thread| !thread.is_finished())
    }
}

impl Drop for NullDevice {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

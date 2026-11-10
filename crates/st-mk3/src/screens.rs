//! The screen session: owns the claimed display interface, renders the display lists the interface
//! submits (coalesced to at most thirty frames a second) and sends each screen's changed region.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::screen::encode::{self, DEVICE_FORMAT};
use crate::screen::font::Fonts;
use crate::screen::ops::DrawOp;
use crate::screen::render::Renderer;
use crate::screen::{png, HEIGHT, PIXEL_COUNT, WIDTH};
use crate::transport::{require_complete, ScreenPipe, ScreenTransport};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// How the session is set up.
pub(crate) struct ScreenOptions {
    pub transport: Arc<dyn ScreenTransport>,
    pub fonts: Arc<Fonts>,
    /// Every rendered frame is also written here as PNG files (see [`crate::Mk3Options::dump_dir`]).
    pub dump_dir: Option<PathBuf>,
    /// Also keep a numbered copy of every dumped frame.
    pub dump_all: bool,
    /// The shortest time between two frames.
    pub frame_interval: Duration,
    /// How long opening keeps trying while the previous owner lets go of the interface.
    pub open_timeout: Duration,
    /// Called from the render thread when the screens stop working (three failed transfers in a
    /// row).
    pub on_failure: Box<dyn Fn(String) + Send + Sync>,
    pub log: Box<dyn Fn(&str) + Send + Sync>,
}

#[derive(Default)]
struct Queue {
    pending: [Option<Vec<DrawOp>>; 2],
    tags: [Option<String>; 2],
    last_frame: Option<Instant>,
    stop: bool,
}

#[derive(Default)]
struct Device {
    pipe: Option<Box<dyn ScreenPipe>>,
    /// What each screen shows, as far as transfers were confirmed.
    sent: [Option<Vec<u16>>; 2],
    failures: u32,
    frames_sent: usize,
    bytes_sent: usize,
    // Dump state: the device's picture as decoded back from the packets.
    decoded: [Option<Vec<u16>>; 2],
    dump_sent: [Option<Vec<u16>>; 2],
    dump_count: usize,
    dump_mismatches: usize,
}

/// Counters for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ScreenStats {
    pub frames_sent: usize,
    pub bytes_sent: usize,
    pub dump_mismatches: usize,
}

struct Shared {
    options: ScreenOptions,
    open: AtomicBool,
    queue: Mutex<Queue>,
    wake: Condvar,
    device: Mutex<Device>,
}

pub(crate) struct ScreenSession {
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

impl ScreenSession {
    pub fn new(options: ScreenOptions) -> Self {
        let shared = Arc::new(Shared { options, open: AtomicBool::new(false), queue: Mutex::new(Queue::default()), wake: Condvar::new(), device: Mutex::new(Device::default()) });
        let for_thread = shared.clone();
        let thread = thread::Builder::new().name("mk3-screens".into()).spawn(move || render_loop(for_thread)).ok();
        ScreenSession { shared, thread: Mutex::new(thread) }
    }

    /// Frames are rendered for dumps even while the screens are not open.
    pub fn dumps(&self) -> bool {
        self.shared.options.dump_dir.is_some()
    }

    pub fn fonts(&self) -> &Fonts {
        &self.shared.options.fonts
    }

    pub fn is_present(&self) -> bool {
        self.shared.options.transport.is_present()
    }

    /// Claims the display interface, retrying while its previous owner lets go.
    pub fn open(&self) -> Result<(), String> {
        let mut device = lock(&self.shared.device);
        if device.pipe.is_some() {
            return Ok(());
        }
        let deadline = Instant::now() + self.shared.options.open_timeout;
        loop {
            match self.shared.options.transport.open() {
                Ok(pipe) => {
                    device.pipe = Some(pipe);
                    device.sent = [None, None];
                    device.failures = 0;
                    self.shared.open.store(true, Ordering::SeqCst);
                    return Ok(());
                }
                Err(error) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(format!("Could not open the Maschine MK3 screens: {error}"));
                    }
                    thread::sleep(remaining.min(Duration::from_millis(250)));
                }
            }
        }
    }

    /// Makes both screens black and so establishes what they show, before the screens are
    /// announced as usable. A failure leaves ownership with the caller, which closes.
    pub fn initialize_displays(&self) -> Result<(), String> {
        let mut device = lock(&self.shared.device);
        let black = vec![0u16; PIXEL_COUNT];
        for screen in 0..2usize {
            let packet = encode::packet(screen as u8, None, &black).ok_or("Could not initialize the Maschine MK3 screen packet.")?;
            write(&mut device, &packet)?;
            device.sent[screen] = Some(black.clone());
            device.frames_sent += 1;
        }
        Ok(())
    }

    /// Releases the display interface, after blanking both screens when asked.
    pub fn close(&self, blank: bool) {
        let mut device = lock(&self.shared.device);
        self.shared.open.store(false, Ordering::SeqCst);
        if blank && device.pipe.is_some() {
            let black = vec![0u16; PIXEL_COUNT];
            for screen in 0..2usize {
                if let Some(packet) = encode::packet(screen as u8, device.sent[screen].as_deref(), &black) {
                    let _ = write(&mut device, &packet);
                }
            }
        }
        {
            let mut queue = lock(&self.shared.queue);
            queue.pending = [None, None];
            queue.tags = [None, None];
        }
        device.sent = [None, None];
        device.pipe = None;
    }

    /// Replaces the display list waiting for a screen (0 left, 1 right). Never waits for a
    /// transfer.
    pub fn submit(&self, screen: usize, ops: Vec<DrawOp>, tag: Option<String>) {
        if screen > 1 || !(self.shared.open.load(Ordering::SeqCst) || self.dumps()) {
            return;
        }
        let mut queue = lock(&self.shared.queue);
        queue.pending[screen] = Some(ops);
        if tag.is_some() {
            queue.tags[screen] = tag;
        }
        self.shared.wake.notify_all();
    }

    pub fn stats(&self) -> ScreenStats {
        let device = lock(&self.shared.device);
        ScreenStats { frames_sent: device.frames_sent, bytes_sent: device.bytes_sent, dump_mismatches: device.dump_mismatches }
    }
}

impl Drop for ScreenSession {
    fn drop(&mut self) {
        lock(&self.shared.queue).stop = true;
        self.shared.wake.notify_all();
        if let Some(thread) = lock(&self.thread).take() {
            let _ = thread.join();
        }
    }
}

fn write(device: &mut Device, packet: &[u8]) -> Result<(), String> {
    let pipe = device.pipe.as_mut().ok_or("Screens are closed.")?;
    let transferred = pipe.write(packet, WRITE_TIMEOUT)?;
    require_complete(packet.len(), transferred)?;
    device.bytes_sent += transferred;
    Ok(())
}

fn render_loop(shared: Arc<Shared>) {
    let mut renderer = Renderer::new(shared.options.fonts.clone());
    loop {
        let (work, tags) = {
            let mut queue = lock(&shared.queue);
            loop {
                if queue.stop {
                    return;
                }
                if queue.pending.iter().all(Option::is_none) {
                    queue = shared.wake.wait(queue).unwrap_or_else(|poisoned| poisoned.into_inner());
                    continue;
                }
                let due = queue.last_frame.map(|last| last + shared.options.frame_interval);
                match due.map(|due| due.saturating_duration_since(Instant::now())).filter(|wait| !wait.is_zero()) {
                    Some(wait) => queue = shared.wake.wait_timeout(queue, wait).unwrap_or_else(|poisoned| poisoned.into_inner()).0,
                    None => break,
                }
            }
            queue.last_frame = Some(Instant::now());
            (std::mem::take(&mut queue.pending), std::mem::take(&mut queue.tags))
        };
        let mut failed: Option<String> = None;
        {
            let mut device = lock(&shared.device);
            if device.pipe.is_none() && shared.options.dump_dir.is_none() {
                continue;
            }
            for (screen, ops) in work.into_iter().enumerate() {
                let Some(ops) = ops else { continue };
                renderer.render(&ops);
                let frame = renderer.framebuffer(DEVICE_FORMAT);
                if shared.options.dump_dir.is_some() {
                    dump(&shared, &mut device, screen, &renderer, &frame, tags[screen].as_deref());
                }
                if device.pipe.is_none() {
                    continue;
                }
                let Some(packet) = encode::packet(screen as u8, device.sent[screen].as_deref(), &frame) else { continue };
                match write(&mut device, &packet) {
                    Ok(()) => {
                        device.sent[screen] = Some(frame);
                        device.failures = 0;
                        device.frames_sent += 1;
                    }
                    Err(error) => {
                        device.sent[screen] = None;
                        device.failures += 1;
                        (shared.options.log)(&format!("screen write failed: {error}"));
                        if device.failures >= 3 {
                            failed = Some(error);
                            break;
                        }
                        // The interface sends a display list only when it changes: try this frame
                        // again (in full, the baseline is gone) unless a newer one waits already.
                        let mut queue = lock(&shared.queue);
                        if queue.pending[screen].is_none() {
                            queue.pending[screen] = Some(ops);
                        }
                    }
                }
            }
        }
        if let Some(error) = failed {
            (shared.options.on_failure)(error);
        }
    }
}

/// Writes `<dir>/left.png` (the rendered bitmap) and `left-device.png` (the 16-bit picture decoded
/// back from the packets, exactly what the device would show), `<tag>-left*.png` when the frame is
/// tagged, and numbered copies when asked for.
fn dump(shared: &Shared, device: &mut Device, screen: usize, renderer: &Renderer, frame: &[u16], tag: Option<&str>) {
    let Some(folder) = &shared.options.dump_dir else { return };
    let _ = std::fs::create_dir_all(folder);
    let mut decoded = device.decoded[screen].take().unwrap_or_else(|| vec![0u16; PIXEL_COUNT]);
    if let Some(packet) = encode::packet(screen as u8, device.dump_sent[screen].as_deref(), frame) {
        if !encode::apply(&packet, &mut decoded) {
            (shared.options.log)(&format!("dump: the packet for screen {screen} does not decode"));
        }
    }
    if decoded != frame {
        device.dump_mismatches += 1;
        (shared.options.log)(&format!("dump: the decoded picture differs from the rendered frame on screen {screen}"));
    }
    let side = if screen == 0 { "left" } else { "right" };
    let mut names = vec![side.to_string()];
    if let Some(tag) = tag.filter(|tag| !tag.is_empty()) {
        let safe: String = tag.chars().map(|character| if character.is_alphanumeric() || character == '-' || character == '_' { character } else { '_' }).collect();
        names.push(format!("{safe}-{side}"));
    }
    if shared.options.dump_all {
        device.dump_count += 1;
        names.push(format!("frame-{:05}-{side}", device.dump_count));
    }
    let device_rgb: Vec<u8> = decoded.iter().flat_map(|&pixel| <[u8; 3]>::from(DEVICE_FORMAT.unpack(pixel))).collect();
    let pictures = [(String::new(), png::encode_rgb(WIDTH, HEIGHT, &renderer.rgb())), ("-device".to_string(), png::encode_rgb(WIDTH, HEIGHT, &device_rgb))];
    for name in &names {
        for (suffix, picture) in &pictures {
            if let Some(picture) = picture {
                // Written under another name first: a viewer watching the folder never sees half a picture.
                let (target, partial) = (folder.join(format!("{name}{suffix}.png")), folder.join(format!("{name}{suffix}.png.part")));
                if std::fs::write(&partial, picture).is_ok() {
                    let _ = std::fs::rename(&partial, &target);
                }
            }
        }
    }
    device.decoded[screen] = Some(decoded);
    device.dump_sent[screen] = Some(frame.to_vec());
}

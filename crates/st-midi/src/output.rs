//! Output to devices: the sender thread that holds each message until its time, and the handle the
//! audio thread uses to hand messages over.
//!
//! `midir` sends a message at the moment of the call and nothing else, so sample-accurate output is
//! built here. The audio callback computes, for every event of a render block, the device time at
//! which the event's frame plays ([`crate::schedule::note_timestamp`]) and pushes the message into a
//! bounded lock-free queue ([`RenderOutput`]: no allocation, no lock, no system call). The sender
//! thread drains the queue at least every millisecond, keeps the messages in time order and sends
//! each one when its time comes, sleeping until shortly before and spinning through the last
//! fraction of a millisecond. The timing error at the device is that of a thread wake-up (tens of
//! microseconds on an idle machine), not of an audio buffer.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::clock::{ClockBlock, ClockGenerator};
use crate::message::Message;
use crate::ports::OutputPort;
use crate::schedule::{self, note_timestamp, Consumer, Producer, Schedule, TimedMessage};

/// Output devices that can be open at once.
pub const MAX_OUTPUTS: usize = 64;
/// Instruments that can have a route (the engine's instrument limit).
pub const ROUTE_COUNT: usize = 1024;
/// Messages the audio thread can have in flight to the sender thread.
const QUEUE_CAPACITY: usize = 4096;
/// Messages the sender thread holds for later.
const BACKLOG_CAPACITY: usize = 16_384;
/// How often the sender thread looks at the audio thread's queue.
const POLL: Duration = Duration::from_millis(1);
/// The sender wakes this long before a message is due and spins until its time.
const SPIN: Duration = Duration::from_micros(150);
/// Messages to send at once that can wait for the sender's attention.
const URGENT_CAPACITY: usize = 4096;
/// The furthest a message may be scheduled ahead, in ticks. An audio block is never further than
/// this from being heard, so a later time is an error upstream; the message is sent after this long
/// instead of being held indefinitely.
const MAXIMUM_AHEAD: u64 = 2_000_000_000;

/// The time base of scheduled output: nanoseconds on the system's monotonic clock, counted from
/// the creation of the clock. Reading it is safe on the audio thread. A time is never zero (zero
/// means "now" wherever a time is expected).
#[derive(Debug, Clone, Copy)]
pub struct MidiClock {
    origin: Instant,
}

impl Default for MidiClock {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiClock {
    /// The first second of the scale is left out, so that leads subtracted from a time never
    /// reach zero.
    const EPOCH: u64 = 1_000_000_000;
    /// Device ticks (nanoseconds) per second.
    pub const TICKS_PER_SECOND: f64 = 1e9;

    /// A clock that starts now.
    pub fn new() -> Self {
        MidiClock { origin: Instant::now() }
    }

    /// The current time.
    pub fn now(&self) -> u64 {
        self.at(Instant::now())
    }

    /// The time of `instant` (the audio device's playback time of a block, for instance).
    pub fn at(&self, instant: Instant) -> u64 {
        Self::EPOCH + instant.saturating_duration_since(self.origin).as_nanos().min(u128::from(u64::MAX / 2)) as u64
    }

    /// The time `delay` from now.
    pub fn after(&self, delay: Duration) -> u64 {
        self.now() + delay.as_nanos().min(u128::from(u64::MAX / 4)) as u64
    }

    /// Ticks per audio frame at `sample_rate`.
    pub fn ticks_per_frame(sample_rate: f64) -> f64 {
        Self::TICKS_PER_SECOND / sample_rate
    }
}

/// Where an instrument's notes go, as the audio thread reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Route {
    /// The instrument is an external MIDI instrument: its notes go to a device, not to the engine.
    pub external: bool,
    /// The open output's slot, when the device is present.
    pub output: Option<u8>,
    /// MIDI channel, 0 to 15.
    pub channel: u8,
    /// Nanoseconds its notes are sent ahead of their render time (a synth heard back through a
    /// latency-compensated line input).
    pub lead: u32,
}

impl Route {
    fn pack(self) -> u64 {
        u64::from(self.external) << 63 | u64::from(self.output.is_some()) << 62 | u64::from(self.output.unwrap_or(0)) << 48 | u64::from(self.channel & 0x0F) << 40 | u64::from(self.lead)
    }

    fn unpack(bits: u64) -> Self {
        Route {
            external: bits >> 63 != 0,
            output: (bits >> 62 & 1 != 0).then_some((bits >> 48 & 0xFF) as u8),
            channel: (bits >> 40 & 0x0F) as u8,
            lead: bits as u32,
        }
    }
}

/// State the control side publishes and the audio thread reads, one atomic word at a time.
pub(crate) struct Published {
    routes: Box<[AtomicU64]>,
    /// Per output slot: bit 0 clock, bit 1 transport.
    clock_targets: [AtomicU8; MAX_OUTPUTS],
    clock_lead_bits: AtomicU64,
    playing: AtomicBool,
    dropped: AtomicU64,
}

impl Published {
    fn new() -> Self {
        Published {
            routes: (0..ROUTE_COUNT).map(|_| AtomicU64::new(0)).collect(),
            clock_targets: std::array::from_fn(|_| AtomicU8::new(0)),
            clock_lead_bits: AtomicU64::new(0f64.to_bits()),
            playing: AtomicBool::new(false),
            dropped: AtomicU64::new(0),
        }
    }
}

enum Command {
    Open { slot: u8, port: Box<dyn OutputPort> },
    Close { slot: u8 },
    Send(TimedMessage),
    Attach(Consumer<TimedMessage>),
    Stop,
}

/// Counters of the output path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OutputStats {
    /// Messages handed to devices.
    pub sent: u64,
    /// Messages lost: a full queue, or a device that was no longer open.
    pub dropped: u64,
    /// Sends a device refused.
    pub failed: u64,
}

struct Counters {
    sent: AtomicU64,
    failed: AtomicU64,
}

type ErrorCallback = Arc<dyn Fn(u8, String) + Send + Sync>;

/// The sender thread and the state it shares with the audio thread.
pub(crate) struct OutputEngine {
    commands: Mutex<Sender<Command>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    published: Arc<Published>,
    counters: Arc<Counters>,
    clock: MidiClock,
}

impl OutputEngine {
    /// Starts the sender thread. `on_error(slot, message)` is called from it when a device refuses
    /// a message (once per run of failures).
    pub fn start(clock: MidiClock, on_error: ErrorCallback) -> Result<Self, String> {
        let (commands, receiver) = mpsc::channel();
        let published = Arc::new(Published::new());
        let counters = Arc::new(Counters { sent: AtomicU64::new(0), failed: AtomicU64::new(0) });
        let (for_thread, dropped) = (counters.clone(), published.clone());
        let thread = thread::Builder::new()
            .name("midi-out".into())
            .spawn(move || SenderLoop::new(receiver, clock, for_thread, dropped, on_error).run())
            .map_err(|error| format!("Could not start the MIDI output thread: {error}."))?;
        Ok(OutputEngine { commands: Mutex::new(commands), thread: Mutex::new(Some(thread)), published, counters, clock })
    }

    fn command(&self, command: Command) {
        let _ = self.commands.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).send(command);
    }

    pub fn clock(&self) -> MidiClock {
        self.clock
    }

    pub fn open(&self, slot: u8, port: Box<dyn OutputPort>) {
        self.command(Command::Open { slot, port });
    }

    pub fn close(&self, slot: u8) {
        self.command(Command::Close { slot });
    }

    /// Sends from a control thread: at `time`, or now when it is 0.
    pub fn send(&self, slot: u8, message: Message, time: u64) {
        self.command(Command::Send(TimedMessage { time, output: slot, message }));
    }

    pub fn set_route(&self, instrument: usize, route: Route) {
        if let Some(cell) = self.published.routes.get(instrument) {
            cell.store(route.pack(), Ordering::Release);
        }
    }

    pub fn route(&self, instrument: usize) -> Route {
        self.published.routes.get(instrument).map(|cell| Route::unpack(cell.load(Ordering::Acquire))).unwrap_or_default()
    }

    /// Publishes which outputs get clock (`(slot, transport)`) and the lead in seconds. An output
    /// that loses transport (or the clock) while the song plays gets a Stop, so its sequencer does
    /// not run on.
    pub fn set_clock_targets(&self, targets: &[(u8, bool)], lead_seconds: f64) {
        let playing = self.published.playing.load(Ordering::Acquire);
        for (slot, cell) in self.published.clock_targets.iter().enumerate() {
            let next = targets.iter().find(|(target, _)| usize::from(*target) == slot).map_or(0, |(_, transport)| 1 | u8::from(*transport) << 1);
            let previous = cell.swap(next, Ordering::AcqRel);
            if playing && previous & 2 != 0 && next & 2 == 0 {
                self.send(slot as u8, Message::new(0xFC, 0, 0), 0);
            }
        }
        let lead = if lead_seconds.is_finite() { lead_seconds.clamp(0.0, crate::clock::MAXIMUM_LEAD_SECONDS) } else { 0.0 };
        self.published.clock_lead_bits.store(lead.to_bits(), Ordering::Release);
    }

    /// True while any output gets clock.
    pub fn clock_active(&self) -> bool {
        self.published.clock_targets.iter().any(|cell| cell.load(Ordering::Acquire) != 0)
    }

    /// A handle for an audio thread. Each handle has its own queue; one that is dropped is drained
    /// and forgotten.
    pub fn render_output(&self) -> RenderOutput {
        let (producer, consumer) = schedule::queue(QUEUE_CAPACITY);
        self.command(Command::Attach(consumer));
        RenderOutput {
            queue: producer,
            published: self.published.clone(),
            generator: ClockGenerator::new(),
            host_time: 0,
            frames: 0,
            sample_rate: 0.0,
            ticks_per_frame: 0.0,
        }
    }

    pub fn stats(&self) -> OutputStats {
        OutputStats {
            sent: self.counters.sent.load(Ordering::Relaxed),
            dropped: self.published.dropped.load(Ordering::Relaxed),
            failed: self.counters.failed.load(Ordering::Relaxed),
        }
    }

    /// Sends what is due, closes every device and ends the thread.
    pub fn shutdown(&self) {
        self.command(Command::Stop);
        if let Some(thread) = self.thread.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() {
            let _ = thread.join();
        }
    }
}

impl Drop for OutputEngine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct SenderLoop {
    commands: Receiver<Command>,
    clock: MidiClock,
    counters: Arc<Counters>,
    published: Arc<Published>,
    on_error: ErrorCallback,
    ports: Vec<Option<Box<dyn OutputPort>>>,
    failing: [bool; MAX_OUTPUTS],
    queues: Vec<Consumer<TimedMessage>>,
    schedule: Schedule,
    /// Messages to send now. Kept apart from the backlog so that a backlog full of scheduled
    /// messages can never hold up a panic.
    urgent: std::collections::VecDeque<TimedMessage>,
}

impl SenderLoop {
    fn new(commands: Receiver<Command>, clock: MidiClock, counters: Arc<Counters>, published: Arc<Published>, on_error: ErrorCallback) -> Self {
        SenderLoop {
            commands,
            clock,
            counters,
            published,
            on_error,
            ports: (0..MAX_OUTPUTS).map(|_| None).collect(),
            failing: [false; MAX_OUTPUTS],
            queues: Vec::new(),
            schedule: Schedule::new(BACKLOG_CAPACITY),
            urgent: std::collections::VecDeque::with_capacity(URGENT_CAPACITY),
        }
    }

    fn run(mut self) {
        loop {
            loop {
                match self.commands.try_recv() {
                    Ok(command) => {
                        if !self.handle(command) {
                            return;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }
            self.drain_queues();
            self.send_due();
            // Wait for whichever comes first: a command, the next look at the audio thread's
            // queues, or the next message's time.
            let now = self.clock.now();
            let due_in = self.schedule.next_time().map(|time| Duration::from_nanos(time.saturating_sub(now)));
            let poll = (!self.queues.is_empty()).then_some(POLL);
            let wait = match (due_in, poll) {
                (Some(due), Some(poll)) => Some(due.min(poll)),
                (due, poll) => due.or(poll),
            };
            let received = match wait {
                None => self.commands.recv().map_err(|_| RecvTimeoutError::Disconnected),
                Some(wait) if due_in == Some(wait) && wait <= SPIN => {
                    // The last stretch before a message is due: a sleep could overshoot it.
                    let until = Instant::now() + wait;
                    while Instant::now() < until {
                        std::hint::spin_loop();
                    }
                    continue;
                }
                Some(wait) if due_in == Some(wait) => self.commands.recv_timeout(wait - SPIN),
                Some(wait) => self.commands.recv_timeout(wait),
            };
            match received {
                Ok(command) => {
                    if !self.handle(command) {
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    /// Returns false when the thread should end.
    fn handle(&mut self, command: Command) -> bool {
        match command {
            Command::Open { slot, port } => {
                self.ports[usize::from(slot) % MAX_OUTPUTS] = Some(port);
                self.failing[usize::from(slot) % MAX_OUTPUTS] = false;
            }
            Command::Close { slot } => {
                self.ports[usize::from(slot) % MAX_OUTPUTS] = None;
                let before = self.urgent.len();
                self.urgent.retain(|message| message.output != slot);
                let lost = self.schedule.remove_output(slot) + (before - self.urgent.len());
                self.published.dropped.fetch_add(lost as u64, Ordering::Relaxed);
            }
            Command::Send(message) => self.enqueue(message),
            Command::Attach(queue) => self.queues.push(queue),
            Command::Stop => {
                self.drain_queues();
                self.send_due();
                return false;
            }
        }
        true
    }

    fn enqueue(&mut self, mut message: TimedMessage) {
        let accepted = if message.time == 0 {
            let room = self.urgent.len() < URGENT_CAPACITY;
            if room {
                self.urgent.push_back(message);
            }
            room
        } else {
            message.time = message.time.min(self.clock.now() + MAXIMUM_AHEAD);
            self.schedule.push(message)
        };
        if !accepted {
            self.published.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn drain_queues(&mut self) {
        for index in 0..self.queues.len() {
            while let Some(message) = self.queues[index].pop() {
                self.enqueue(message);
            }
        }
        self.queues.retain(|queue| !queue.is_finished());
    }

    fn send_due(&mut self) {
        while let Some(timed) = self.urgent.pop_front().or_else(|| self.schedule.pop_due(self.clock.now())) {
            let slot = usize::from(timed.output) % MAX_OUTPUTS;
            let Some(port) = self.ports[slot].as_mut() else {
                self.published.dropped.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            let (bytes, length) = timed.message.encode();
            match port.send(&bytes[..length]) {
                Ok(()) => {
                    self.counters.sent.fetch_add(1, Ordering::Relaxed);
                    self.failing[slot] = false;
                }
                Err(error) => {
                    self.counters.failed.fetch_add(1, Ordering::Relaxed);
                    if !std::mem::replace(&mut self.failing[slot], true) {
                        (self.on_error)(timed.output, error);
                    }
                }
            }
        }
    }
}

/// The audio thread's handle to MIDI output. Every method is real-time safe: no allocation, no
/// lock, no system call; a full queue drops the message and counts it.
///
/// One render block goes like this:
///
/// ```
/// # use st_midi::{MidiClock, MidiService, MidiOptions, memory::MemoryPorts};
/// # let service = MidiService::new(MidiOptions::in_memory(&MemoryPorts::new())).unwrap();
/// let mut midi = service.render_output();          // once, when the audio stream starts
/// let clock = service.clock();
/// // in the audio callback:
/// let playback_time = clock.now();                 // device time of the block's first frame
/// midi.begin_block(playback_time, 512, 48_000.0);
/// midi.clock(0.0, true, 120.0);                    // transport position: beat, playing, tempo
/// let external = midi.note(3, true, 60, 100, 128); // instrument 3, note-on at frame 128
/// if !external { /* the engine or a plug-in plays it */ }
/// ```
pub struct RenderOutput {
    queue: Producer<TimedMessage>,
    published: Arc<Published>,
    generator: ClockGenerator,
    host_time: u64,
    frames: u32,
    sample_rate: f64,
    ticks_per_frame: f64,
}

impl RenderOutput {
    /// Starts a render block: `host_time` is the device time ([`MidiClock`]) at which the block's
    /// first frame plays, or 0 when unknown (messages are then sent as soon as possible).
    pub fn begin_block(&mut self, host_time: u64, frames: u32, sample_rate: f64) {
        self.host_time = host_time;
        self.frames = frames;
        self.sample_rate = sample_rate;
        self.ticks_per_frame = if sample_rate > 0.0 { MidiClock::ticks_per_frame(sample_rate) } else { 0.0 };
    }

    fn push(&mut self, message: TimedMessage) {
        if !self.queue.push(message) {
            self.published.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn route(&self, instrument: usize) -> Route {
        Route::unpack(self.published.routes[instrument.min(ROUTE_COUNT - 1)].load(Ordering::Acquire))
    }

    /// A note of `instrument` at `frame_offset` frames into the block. Returns true when the
    /// instrument is an external MIDI instrument: the note was scheduled for its device (or
    /// dropped because the device is absent) and must not be played by the engine or a plug-in.
    pub fn note(&mut self, instrument: usize, on: bool, note: u8, velocity: u8, frame_offset: i32) -> bool {
        let route = self.route(instrument);
        if !route.external {
            return false;
        }
        if let Some(output) = route.output {
            let time = note_timestamp(self.host_time, frame_offset, self.ticks_per_frame, route.lead);
            let status = if on { 0x90 } else { 0x80 } | route.channel;
            self.push(TimedMessage { time, output, message: Message::new(status, note.min(127), velocity.min(127)) });
        }
        true
    }

    /// The engine silenced everything: pedals up (sustain, sostenuto, soft) and all-notes-off on
    /// every external instrument's device and channel, now.
    pub fn all_notes_off(&mut self) {
        let mut done = [0u16; MAX_OUTPUTS];
        for instrument in 0..ROUTE_COUNT {
            let route = self.route(instrument);
            let (true, Some(output)) = (route.external, route.output) else { continue };
            let seen = &mut done[usize::from(output) % MAX_OUTPUTS];
            if *seen & 1 << route.channel != 0 {
                continue;
            }
            *seen |= 1 << route.channel;
            for controller in crate::sustain::PEDALS {
                self.push(TimedMessage { time: 0, output, message: Message::new(0xB0 | route.channel, controller, 0) });
            }
            self.push(TimedMessage { time: 0, output, message: Message::new(0xB0 | route.channel, 123, 0) });
        }
    }

    /// The transport position at the block's first frame: `beat` in quarter notes, whether the
    /// song plays, and the tempo. Generates the block's clock pulses and transport messages for
    /// the outputs that want them. Call once per block; not while rendering offline.
    pub fn clock(&mut self, beat: f64, playing: bool, tempo: f64) {
        let targets: [u8; MAX_OUTPUTS] = std::array::from_fn(|slot| self.published.clock_targets[slot].load(Ordering::Acquire));
        let active = targets.iter().any(|flags| *flags != 0);
        let lead = f64::from_bits(self.published.clock_lead_bits.load(Ordering::Acquire));
        let block =
            ClockBlock { beat, playing, tempo, frames: self.frames, sample_rate: self.sample_rate, host_time: self.host_time, ticks_per_frame: self.ticks_per_frame };
        let (queue, published) = (&mut self.queue, &self.published);
        self.generator.render(&block, lead, active, |event| {
            let wanted = if event.transport { 2 } else { 1 };
            for (slot, flags) in targets.iter().enumerate() {
                if flags & wanted != 0 {
                    let message = TimedMessage { time: event.time, output: slot as u8, message: Message::new(event.status, event.data1, event.data2) };
                    if !queue.push(message) {
                        published.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        });
        if self.host_time > 0 {
            published.playing.store(self.generator.was_playing(), Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_survive_packing() {
        for route in [
            Route::default(),
            Route { external: true, output: Some(0), channel: 0, lead: 0 },
            Route { external: true, output: Some(63), channel: 15, lead: 500_000_000 },
            Route { external: true, output: None, channel: 9, lead: u32::MAX },
            Route { external: false, output: Some(7), channel: 3, lead: 1 },
        ] {
            assert_eq!(Route::unpack(route.pack()), route);
        }
        assert_eq!(Route::unpack(0), Route::default());
    }

    #[test]
    fn the_clock_never_reads_zero_and_moves_forward() {
        let clock = MidiClock::new();
        let first = clock.now();
        assert!(first >= 1_000_000_000);
        assert!(clock.after(Duration::from_millis(5)) >= first + 5_000_000);
        assert!(clock.at(Instant::now() + Duration::from_secs(1)) > first + 900_000_000);
        assert!((MidiClock::ticks_per_frame(48_000.0) - 20_833.333).abs() < 0.001);
    }
}

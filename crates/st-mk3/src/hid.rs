//! The HID session: finds the controller, reads its input reports, and sends LED reports (only
//! changed ones, at most about sixty times a second).

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::protocol::{normalize_unidentified, InputEvent, InputParser};
use crate::transport::{HidTransport, HidWriter};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What the session tells its owner, from its own threads.
pub(crate) struct HidCallbacks {
    /// Input events; the flag is true for releases made up because the controller went away.
    pub events: Box<dyn Fn(Vec<InputEvent>, bool) + Send + Sync>,
    /// The controller was found (true) or lost (false).
    pub connection: Box<dyn Fn(bool) + Send + Sync>,
    /// Something to log.
    pub log: Box<dyn Fn(&str) + Send + Sync>,
    /// Every normalized input report, for diagnostics.
    pub raw: crate::driver::RawReportCallback,
}

/// Counters for `mk3.status {diagnostics: true}`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HidStats {
    pub report_counts: BTreeMap<u8, usize>,
    pub first_reports: BTreeMap<u8, Vec<u8>>,
    pub led_reports_sent: usize,
    pub led_report_failures: Vec<String>,
}

#[derive(Default)]
struct Queue {
    pending: BTreeMap<u8, Vec<u8>>,
    force: bool,
    last_flush: Option<Instant>,
}

#[derive(Default)]
struct Device {
    writer: Option<Box<dyn HidWriter>>,
    last_sent: HashMap<u8, Vec<u8>>,
}

struct Shared {
    transport: Arc<dyn HidTransport>,
    callbacks: HidCallbacks,
    stop: AtomicBool,
    queue: Mutex<Queue>,
    wake: Condvar,
    device: Mutex<Device>,
    stats: Mutex<HidStats>,
    poll_interval: Duration,
    output_interval: Duration,
}

/// The running session. Dropping it stops it.
pub(crate) struct HidSession {
    shared: Arc<Shared>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl HidSession {
    pub fn new(transport: Arc<dyn HidTransport>, callbacks: HidCallbacks, poll_interval: Duration) -> Self {
        HidSession {
            shared: Arc::new(Shared {
                transport,
                callbacks,
                stop: AtomicBool::new(true),
                queue: Mutex::new(Queue::default()),
                wake: Condvar::new(),
                device: Mutex::new(Device::default()),
                stats: Mutex::new(HidStats::default()),
                poll_interval,
                output_interval: Duration::from_micros(16_667),
            }),
            threads: Mutex::new(Vec::new()),
        }
    }

    /// Starts looking for the controller. Returns an error text when the threads cannot start.
    pub fn start(&self) -> Option<String> {
        let mut threads = lock(&self.threads);
        if !threads.is_empty() {
            return None;
        }
        self.shared.stop.store(false, Ordering::SeqCst);
        let spawn = |name: &str, body: fn(Arc<Shared>)| {
            let shared = self.shared.clone();
            thread::Builder::new().name(name.into()).spawn(move || body(shared))
        };
        match (spawn("mk3-hid-in", read_loop), spawn("mk3-hid-out", write_loop)) {
            (Ok(reader), Ok(writer)) => {
                threads.extend([reader, writer]);
                None
            }
            (reader, writer) => {
                self.shared.stop.store(true, Ordering::SeqCst);
                self.shared.wake.notify_all();
                for handle in [reader, writer].into_iter().flatten() {
                    let _ = handle.join();
                }
                Some("The Maschine MK3 input threads could not be started.".into())
            }
        }
    }

    /// Stops the session and closes the controller. Held controls are released first (as cancelled
    /// events) and the loss of the connection is reported.
    pub fn stop(&self) {
        let threads: Vec<JoinHandle<()>> = lock(&self.threads).drain(..).collect();
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.wake.notify_all();
        for handle in threads {
            let _ = handle.join();
        }
        lock(&self.shared.queue).pending.clear();
    }

    /// Queues output reports (byte 0 is the report id). With `force` every report is sent even if
    /// it equals what was sent last (the controller's LEDs may have been changed by other software).
    pub fn send(&self, reports: Vec<Vec<u8>>, force: bool) {
        let mut queue = lock(&self.shared.queue);
        queue.force |= force;
        for report in reports {
            if let Some(&id) = report.first() {
                queue.pending.insert(id, report);
            }
        }
        self.shared.wake.notify_all();
    }

    /// Sends reports now, on the calling thread, whatever was sent before (used at release and
    /// shutdown to switch the LEDs off).
    pub fn send_now(&self, reports: Vec<Vec<u8>>) {
        let pending: BTreeMap<u8, Vec<u8>> = {
            let mut queue = lock(&self.shared.queue);
            let mut pending = std::mem::take(&mut queue.pending);
            queue.force = false;
            for report in reports {
                if let Some(&id) = report.first() {
                    pending.insert(id, report);
                }
            }
            pending
        };
        self.shared.flush(pending, true);
    }

    pub fn stats(&self) -> HidStats {
        lock(&self.shared.stats).clone()
    }
}

impl Drop for HidSession {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Shared {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// Waits up to `duration`, or until the session is stopped.
    fn pause(&self, duration: Duration) {
        let deadline = Instant::now() + duration;
        let mut queue = lock(&self.queue);
        while !self.stopped() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            queue = self.wake.wait_timeout(queue, remaining).unwrap_or_else(|poisoned| poisoned.into_inner()).0;
        }
    }

    fn flush(&self, pending: BTreeMap<u8, Vec<u8>>, force: bool) {
        let mut device = lock(&self.device);
        if force {
            device.last_sent.clear();
        }
        let device = &mut *device;
        let Some(writer) = device.writer.as_mut() else { return };
        for (id, report) in pending {
            if device.last_sent.get(&id) == Some(&report) {
                continue;
            }
            match writer.write(&report) {
                Ok(()) => {
                    device.last_sent.insert(id, report);
                    lock(&self.stats).led_reports_sent += 1;
                }
                Err(error) => {
                    let mut stats = lock(&self.stats);
                    stats.led_report_failures.push(format!("{id:#04x}: {error}"));
                    let excess = stats.led_report_failures.len().saturating_sub(8);
                    stats.led_report_failures.drain(..excess);
                    drop(stats);
                    (self.callbacks.log)(&format!("LED report {id:#04x} failed: {error}"));
                }
            }
        }
    }
}

fn read_loop(shared: Arc<Shared>) {
    let started = Instant::now();
    let mut open_error: Option<String> = None;
    while !shared.stopped() {
        let connection = match shared.transport.open() {
            Ok(Some(connection)) => connection,
            Ok(None) => {
                open_error = None;
                shared.pause(shared.poll_interval);
                continue;
            }
            Err(error) => {
                // Said once, not at every attempt.
                if open_error.as_ref() != Some(&error) {
                    (shared.callbacks.log)(&format!("the controller could not be opened: {error}"));
                    open_error = Some(error);
                }
                shared.pause(shared.poll_interval);
                continue;
            }
        };
        open_error = None;
        let mut reader = connection.reader;
        {
            let mut device = lock(&shared.device);
            device.writer = Some(connection.writer);
            device.last_sent.clear();
        }
        let mut parser = InputParser::new();
        (shared.callbacks.connection)(true);
        while !shared.stopped() {
            match reader.read(Duration::from_millis(100)) {
                Ok(Some(bytes)) => {
                    let report = normalize_unidentified(&bytes);
                    if let Some(&id) = report.first() {
                        let mut stats = lock(&shared.stats);
                        *stats.report_counts.entry(id).or_insert(0) += 1;
                        stats.first_reports.entry(id).or_insert_with(|| report.clone());
                    }
                    (shared.callbacks.raw)(&report);
                    let events = parser.parse(&report, started.elapsed().as_secs_f64());
                    if !events.is_empty() {
                        (shared.callbacks.events)(events, false);
                    }
                }
                Ok(None) => {}
                Err(_) => break,
            }
        }
        lock(&shared.device).writer = None;
        // The controller can no longer report releases: make them up so that nothing stays held.
        let released = parser.release_all();
        if !released.is_empty() {
            (shared.callbacks.events)(released, true);
        }
        (shared.callbacks.connection)(false);
    }
}

fn write_loop(shared: Arc<Shared>) {
    loop {
        let (pending, force) = {
            let mut queue = lock(&shared.queue);
            loop {
                if shared.stopped() {
                    return;
                }
                if queue.pending.is_empty() {
                    queue = shared.wake.wait(queue).unwrap_or_else(|poisoned| poisoned.into_inner());
                    continue;
                }
                // Coalesce: at most one flush per output interval.
                let due = queue.last_flush.map(|last| last + shared.output_interval);
                match due.map(|due| due.saturating_duration_since(Instant::now())).filter(|wait| !wait.is_zero()) {
                    Some(wait) => queue = shared.wake.wait_timeout(queue, wait).unwrap_or_else(|poisoned| poisoned.into_inner()).0,
                    None => break,
                }
            }
            queue.last_flush = Some(Instant::now());
            (std::mem::take(&mut queue.pending), std::mem::take(&mut queue.force))
        };
        shared.flush(pending, force);
    }
}

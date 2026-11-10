//! A Maschine MK3 that exists only in memory, for tests of this crate and of the host: plug it in
//! and out, press its controls by sending input reports, and read the LED reports and screen
//! packets it received.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::transport::{AgentRecord, HidConnection, HidLoss, HidReader, HidTransport, HidWriter, ScreenPipe, ScreenTransport, VendorAgents};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Default)]
struct HidState {
    present: bool,
    generation: u64,
    input: VecDeque<Vec<u8>>,
    written: Vec<Vec<u8>>,
    opens: usize,
    open_error: Option<String>,
    write_error: Option<String>,
}

/// The controller's HID side.
#[derive(Clone, Default)]
pub struct FakeHid {
    state: Arc<(Mutex<HidState>, Condvar)>,
}

impl FakeHid {
    /// A controller that is not plugged in.
    pub fn new() -> Self {
        Self::default()
    }

    /// Plugs the controller in.
    pub fn plug(&self) {
        let mut state = lock(&self.state.0);
        state.present = true;
        state.generation += 1;
        state.input.clear();
        self.state.1.notify_all();
    }

    /// Unplugs it: an open connection reports the loss.
    pub fn unplug(&self) {
        let mut state = lock(&self.state.0);
        state.present = false;
        state.generation += 1;
        self.state.1.notify_all();
    }

    /// The controller sends an input report.
    pub fn send(&self, report: &[u8]) {
        lock(&self.state.0).input.push_back(report.to_vec());
        self.state.1.notify_all();
    }

    /// Every output report received so far.
    pub fn written(&self) -> Vec<Vec<u8>> {
        lock(&self.state.0).written.clone()
    }

    /// The last output report with this id.
    pub fn last_report(&self, id: u8) -> Option<Vec<u8>> {
        lock(&self.state.0).written.iter().rev().find(|report| report.first() == Some(&id)).cloned()
    }

    /// Forgets the output reports received.
    pub fn clear_written(&self) {
        lock(&self.state.0).written.clear();
    }

    /// How often the controller was opened.
    pub fn opens(&self) -> usize {
        lock(&self.state.0).opens
    }

    /// Makes opening fail with this message (`None`: work again).
    pub fn fail_open(&self, error: Option<&str>) {
        lock(&self.state.0).open_error = error.map(str::to_string);
    }

    /// Makes output reports fail with this message (`None`: work again).
    pub fn fail_writes(&self, error: Option<&str>) {
        lock(&self.state.0).write_error = error.map(str::to_string);
    }
}

struct FakeHidEnd {
    hid: FakeHid,
    generation: u64,
}

impl HidTransport for FakeHid {
    fn open(&self) -> Result<Option<HidConnection>, String> {
        let mut state = lock(&self.state.0);
        if let Some(error) = &state.open_error {
            return Err(error.clone());
        }
        if !state.present {
            return Ok(None);
        }
        state.opens += 1;
        let generation = state.generation;
        Ok(Some(HidConnection {
            reader: Box::new(FakeHidEnd { hid: self.clone(), generation }),
            writer: Box::new(FakeHidEnd { hid: self.clone(), generation }),
        }))
    }
}

impl HidReader for FakeHidEnd {
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, HidLoss> {
        let deadline = Instant::now() + timeout;
        let mut state = lock(&self.hid.state.0);
        loop {
            if state.generation != self.generation || !state.present {
                return Err(HidLoss::Disconnected);
            }
            if let Some(report) = state.input.pop_front() {
                return Ok(Some(report));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            state = self.hid.state.1.wait_timeout(state, remaining).unwrap_or_else(|poisoned| poisoned.into_inner()).0;
        }
    }
}

impl HidWriter for FakeHidEnd {
    fn write(&mut self, report: &[u8]) -> Result<(), String> {
        let mut state = lock(&self.hid.state.0);
        if state.generation != self.generation || !state.present {
            return Err("the controller is not connected".into());
        }
        if let Some(error) = &state.write_error {
            return Err(error.clone());
        }
        state.written.push(report.to_vec());
        Ok(())
    }
}

/// A gate a test closes to hold a call inside a fake, and opens to let it go on.
#[derive(Clone, Default)]
pub struct Gate {
    state: Arc<(Mutex<GateState>, Condvar)>,
}

#[derive(Default)]
struct GateState {
    closed: bool,
    waiting: usize,
    entered: usize,
}

impl Gate {
    /// Closes the gate: the next calls that pass it wait.
    pub fn close(&self) {
        lock(&self.state.0).closed = true;
    }

    /// Opens the gate and lets every waiting call continue.
    pub fn open(&self) {
        lock(&self.state.0).closed = false;
        self.state.1.notify_all();
    }

    /// Waits until a call is held at the gate; false after five seconds.
    pub fn wait_entered(&self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut state = lock(&self.state.0);
        while state.waiting == 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            state = self.state.1.wait_timeout(state, remaining).unwrap_or_else(|poisoned| poisoned.into_inner()).0;
        }
        true
    }

    /// How many calls have reached the gate so far.
    pub fn entered(&self) -> usize {
        lock(&self.state.0).entered
    }

    fn pass(&self) {
        let mut state = lock(&self.state.0);
        state.entered += 1;
        if !state.closed {
            return;
        }
        state.waiting += 1;
        self.state.1.notify_all();
        let deadline = Instant::now() + Duration::from_secs(10);
        while state.closed && Instant::now() < deadline {
            state = self.state.1.wait_timeout(state, Duration::from_millis(50)).unwrap_or_else(|poisoned| poisoned.into_inner()).0;
        }
        state.waiting -= 1;
    }
}

#[derive(Default)]
struct ScreenState {
    absent: bool,
    open_failures: usize,
    write_failures: usize,
    short_writes: usize,
    packets: Vec<Vec<u8>>,
    open_now: usize,
}

/// The controller's display side.
#[derive(Clone, Default)]
pub struct FakeScreens {
    state: Arc<Mutex<ScreenState>>,
    opens: Arc<AtomicUsize>,
    /// Every write passes this gate (close it to hold a transfer, or a blanking at release).
    pub write_gate: Gate,
}

impl FakeScreens {
    /// Screens that are present and work.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a controller is connected at all.
    pub fn set_present(&self, present: bool) {
        lock(&self.state).absent = !present;
    }

    /// The next `count` attempts to open fail.
    pub fn fail_opens(&self, count: usize) {
        lock(&self.state).open_failures = count;
    }

    /// The next `count` transfers fail.
    pub fn fail_writes(&self, count: usize) {
        lock(&self.state).write_failures = count;
    }

    /// The next `count` transfers report one byte less than the packet.
    pub fn shorten_writes(&self, count: usize) {
        lock(&self.state).short_writes = count;
    }

    /// How often opening was attempted.
    pub fn opens(&self) -> usize {
        self.opens.load(Ordering::SeqCst)
    }

    /// Whether the display interface is claimed right now.
    pub fn is_open(&self) -> bool {
        lock(&self.state).open_now > 0
    }

    /// Every packet received so far.
    pub fn packets(&self) -> Vec<Vec<u8>> {
        lock(&self.state).packets.clone()
    }

    /// Forgets the packets received.
    pub fn clear_packets(&self) {
        lock(&self.state).packets.clear();
    }
}

struct FakePipe {
    screens: FakeScreens,
}

impl ScreenTransport for FakeScreens {
    fn is_present(&self) -> bool {
        !lock(&self.state).absent
    }

    fn open(&self) -> Result<Box<dyn ScreenPipe>, String> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        let mut state = lock(&self.state);
        if state.absent {
            return Err("Maschine MK3 screen interface not found.".into());
        }
        if state.open_failures > 0 {
            state.open_failures -= 1;
            return Err("fixture USB open failure".into());
        }
        state.open_now += 1;
        Ok(Box::new(FakePipe { screens: self.clone() }))
    }
}

impl ScreenPipe for FakePipe {
    fn write(&mut self, packet: &[u8], _timeout: Duration) -> Result<usize, String> {
        self.screens.write_gate.pass();
        let mut state = lock(&self.screens.state);
        if state.write_failures > 0 {
            state.write_failures -= 1;
            return Err("fixture USB transfer failure".into());
        }
        if state.short_writes > 0 {
            state.short_writes -= 1;
            return Ok(packet.len().saturating_sub(1));
        }
        state.packets.push(packet.to_vec());
        Ok(packet.len())
    }
}

impl Drop for FakePipe {
    fn drop(&mut self) {
        let mut state = lock(&self.screens.state);
        state.open_now = state.open_now.saturating_sub(1);
    }
}

#[derive(Default)]
struct AgentState {
    recorded: Vec<AgentRecord>,
    stop_calls: usize,
    restore_calls: usize,
    recover_calls: usize,
    restore_failures: usize,
    stop_error: Option<String>,
}

/// Vendor agents that are "running": stopping and restoring them is counted.
#[derive(Clone, Default)]
pub struct FakeAgents {
    state: Arc<Mutex<AgentState>>,
    /// Stopping passes this gate (close it to hold a take-over in the middle).
    pub stop_gate: Gate,
}

impl FakeAgents {
    /// Agents that stop and restore without trouble.
    pub fn new() -> Self {
        Self::default()
    }

    /// How often agents were stopped.
    pub fn stop_calls(&self) -> usize {
        lock(&self.state).stop_calls
    }

    /// How often agents were restored.
    pub fn restore_calls(&self) -> usize {
        lock(&self.state).restore_calls
    }

    /// How often crash recovery ran.
    pub fn recover_calls(&self) -> usize {
        lock(&self.state).recover_calls
    }

    /// The next `count` restores fail (the agents do not come back).
    pub fn fail_restores(&self, count: usize) {
        lock(&self.state).restore_failures = count;
    }

    /// Makes stopping fail with this message (`None`: work again).
    pub fn fail_stop(&self, error: Option<&str>) {
        lock(&self.state).stop_error = error.map(str::to_string);
    }

    /// Puts a record "on disk", as a crashed earlier run would have left it.
    pub fn set_recorded(&self, records: Vec<AgentRecord>) {
        lock(&self.state).recorded = records;
    }

    fn record() -> AgentRecord {
        AgentRecord { name: "fake".into(), ..AgentRecord::default() }
    }
}

impl VendorAgents for FakeAgents {
    fn recover(&self, _log: &dyn Fn(&str)) {
        lock(&self.state).recover_calls += 1;
    }

    fn stop(&self, log: &dyn Fn(&str)) -> Result<Vec<AgentRecord>, String> {
        lock(&self.state).stop_calls += 1;
        self.stop_gate.pass();
        let mut state = lock(&self.state);
        // The durable record is written before anything is stopped.
        state.recorded = vec![Self::record()];
        if let Some(error) = &state.stop_error {
            return Err(error.clone());
        }
        log("stopped fake");
        Ok(vec![Self::record()])
    }

    fn restore(&self, _records: &[AgentRecord], _log: &dyn Fn(&str)) -> bool {
        let mut state = lock(&self.state);
        state.restore_calls += 1;
        if state.restore_failures > 0 {
            state.restore_failures -= 1;
            return false;
        }
        state.recorded.clear();
        true
    }

    fn read_state(&self) -> Vec<AgentRecord> {
        lock(&self.state).recorded.clone()
    }
}

//! The controller's driver: who owns the device when.
//!
//! - **Shared mode** (automatic when support is on): the HID interface only. Buttons, pads,
//!   encoders and LEDs work alongside Native Instruments' software.
//! - **Exclusive mode** (only on `mk3.connect {exclusive: true}`): the vendor's background agents
//!   are stopped, the display interface is claimed and both screens are drawn. Releasing the
//!   controller, quitting, any failure on the way, and (through a record on disk) the next launch
//!   after a crash all give the screens back and start the agents again.
//!
//! Changes of ownership run one at a time, in the order they were asked for, on the caller's
//! thread: a release can never finish in the middle of an older take-over and then have that
//! take-over reopen the screens. [`Mk3Driver::shutdown`] is the exception: it does not queue behind
//! a take-over that is still stopping agents, it makes it give up.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::hid::{HidCallbacks, HidSession};
use crate::protocol::{InputEvent, LedState};
use crate::screen::font::Fonts;
use crate::screen::ops::DrawOp;
use crate::screens::{ScreenOptions, ScreenSession};
use crate::transport::{AgentRecord, HidTransport, ScreenTransport, VendorAgents};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The controller's state, as `mk3.status` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverStatus {
    /// The controller is connected and open.
    pub connected: bool,
    /// Both screens are open and drawn by this application.
    pub screens: bool,
    /// The vendor's agents are stopped for this application.
    pub exclusive: bool,
    /// The controller has sent input since it was connected.
    pub input_seen: bool,
    /// The driver looks for the controller.
    pub running: bool,
    /// What went wrong last (a failed take-over, a lost controller).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Receives input reports, report id first.
pub type RawReportCallback = Box<dyn Fn(&[u8]) + Send + Sync>;

/// What the driver tells its owner. Called from the driver's threads.
pub struct DriverCallbacks {
    /// An `mk3.input` payload.
    pub input: Box<dyn Fn(Map<String, Value>) + Send + Sync>,
    /// The status changed.
    pub status: Box<dyn Fn(DriverStatus) + Send + Sync>,
    /// A line for the log.
    pub log: Box<dyn Fn(&str) + Send + Sync>,
    /// Every input report, with its report id first (diagnostics; the probe's `--raw`).
    pub raw_report: RawReportCallback,
}

impl Default for DriverCallbacks {
    fn default() -> Self {
        DriverCallbacks { input: Box::new(|_| {}), status: Box::new(|_| {}), log: Box::new(|_| {}), raw_report: Box::new(|_| {}) }
    }
}

/// What a driver is built from.
pub struct DriverOptions {
    /// The HID interface.
    pub hid: Arc<dyn HidTransport>,
    /// The display interface.
    pub screens: Arc<dyn ScreenTransport>,
    /// The vendor software that must step aside for the screens.
    pub agents: Arc<dyn VendorAgents>,
    /// Fonts for the screens' text.
    pub fonts: Arc<Fonts>,
    /// Receives input, status changes and log lines.
    pub callbacks: DriverCallbacks,
    /// Render every frame to PNG files in this folder as well, also without the screens.
    pub dump_dir: Option<PathBuf>,
    /// Keep a numbered copy of every dumped frame.
    pub dump_all: bool,
    /// How often an absent controller is looked for.
    pub poll_interval: Duration,
    /// How long claiming the display interface is retried.
    pub screen_open_timeout: Duration,
    /// The shortest time between two frames on a screen.
    pub frame_interval: Duration,
}

#[derive(Default)]
struct State {
    running: bool,
    connected: bool,
    screens: bool,
    exclusive: bool,
    input_seen: bool,
    last_error: Option<String>,
    leds: LedState,
    stopped_agents: Vec<AgentRecord>,
}

struct Inner {
    hid: HidSession,
    screen: ScreenSession,
    agents: Arc<dyn VendorAgents>,
    callbacks: DriverCallbacks,
    state: Mutex<State>,
    /// Held for the whole of one change of ownership.
    operation: Mutex<()>,
    operations: AtomicUsize,
    shutting_down: AtomicBool,
    recovered: AtomicBool,
}

/// The Maschine MK3 driver.
pub struct Mk3Driver {
    inner: Arc<Inner>,
}

impl Mk3Driver {
    /// Creates the driver. It touches no hardware until [`connect`](Self::connect).
    pub fn new(options: DriverOptions) -> Self {
        let inner = Arc::new_cyclic(|weak: &Weak<Inner>| {
            let (for_events, for_connection, for_failure) = (weak.clone(), weak.clone(), weak.clone());
            let (for_log, for_screen_log, for_raw) = (weak.clone(), weak.clone(), weak.clone());
            let hid = HidSession::new(
                options.hid,
                HidCallbacks {
                    events: Box::new(move |events, cancelled| {
                        if let Some(inner) = for_events.upgrade() {
                            inner.deliver(events, cancelled);
                        }
                    }),
                    connection: Box::new(move |connected| {
                        if let Some(inner) = for_connection.upgrade() {
                            inner.connection_changed(connected);
                        }
                    }),
                    log: Box::new(move |message| {
                        if let Some(inner) = for_log.upgrade() {
                            (inner.callbacks.log)(message);
                        }
                    }),
                    raw: Box::new(move |report| {
                        if let Some(inner) = for_raw.upgrade() {
                            (inner.callbacks.raw_report)(report);
                        }
                    }),
                },
                options.poll_interval,
            );
            let screen = ScreenSession::new(ScreenOptions {
                transport: options.screens,
                fonts: options.fonts,
                dump_dir: options.dump_dir,
                dump_all: options.dump_all,
                frame_interval: options.frame_interval,
                open_timeout: options.screen_open_timeout,
                on_failure: Box::new(move |message| {
                    if let Some(inner) = for_failure.upgrade() {
                        inner.screen_failed(message);
                    }
                }),
                log: Box::new(move |message| {
                    if let Some(inner) = for_screen_log.upgrade() {
                        (inner.callbacks.log)(message);
                    }
                }),
            });
            Inner {
                hid,
                screen,
                agents: options.agents,
                callbacks: options.callbacks,
                state: Mutex::new(State::default()),
                operation: Mutex::new(()),
                operations: AtomicUsize::new(0),
                shutting_down: AtomicBool::new(false),
                recovered: AtomicBool::new(false),
            }
        });
        Mk3Driver { inner }
    }

    /// The current status.
    pub fn status(&self) -> DriverStatus {
        self.inner.status()
    }

    /// Starts agents a crashed earlier run left stopped. Runs once per driver; `connect` does it
    /// too.
    pub fn recover_agents(&self) {
        self.inner.recover_agents();
    }

    /// `mk3.connect`: starts looking for the controller, and with `exclusive` takes over the
    /// screens (without it, gives them back if this driver holds them). May take seconds while
    /// agents stop; the status tells how it ended.
    pub fn connect(&self, exclusive: bool) -> DriverStatus {
        self.inner.operate(|inner| inner.perform_connect(exclusive));
        self.inner.status()
    }

    /// `mk3.disconnect`: releases the controller entirely (LEDs off, screens blank) and restores
    /// the vendor's agents if they were stopped.
    pub fn disconnect(&self) -> DriverStatus {
        self.inner.operate(Inner::perform_disconnect);
        self.inner.status()
    }

    /// The application quits: releases the controller and restores the agents, now, on this
    /// thread. A take-over in progress is made to give up; nothing is left half done.
    pub fn shutdown(&self) {
        self.inner.shutdown();
    }

    /// `mk3.leds`: a partial LED update (see [`LedState::apply`]).
    pub fn set_leds(&self, payload: &Value) {
        let (reports, running) = {
            let mut state = lock(&self.inner.state);
            state.leds.apply(payload);
            (state.leds.reports(), state.running)
        };
        if running {
            self.inner.hid.send(reports, false);
        }
    }

    /// `mk3.screen`: display lists for the `left` and `right` screen. Ignored unless the screens
    /// are taken over (or frames are dumped). Returns whether the screens are open.
    pub fn set_screens(&self, payload: &Value) -> bool {
        let screens = lock(&self.inner.state).screens;
        if !screens && !self.inner.screen.dumps() {
            return false;
        }
        let tag = payload.get("dumpTag").and_then(Value::as_str);
        for (index, side) in ["left", "right"].into_iter().enumerate() {
            if let Some(list) = payload.get(side).and_then(Value::as_array) {
                self.inner.screen.submit(index, DrawOp::parse(list), tag.map(str::to_string));
            }
        }
        screens
    }

    /// Counters and first reports, for `mk3.status {diagnostics: true}` and the probe.
    pub fn diagnostics(&self) -> Value {
        let hid = self.inner.hid.stats();
        let screens = self.inner.screen.stats();
        let hex = |id: &u8| format!("{id:#04x}");
        json!({
            "reports": hid.report_counts.iter().map(|(id, count)| (hex(id), json!(count))).collect::<Map<String, Value>>(),
            "firstReports": hid.first_reports.iter().map(|(id, report)| (hex(id), json!(report.iter().map(|byte| format!("{byte:02x}")).collect::<Vec<_>>().join(" ")))).collect::<Map<String, Value>>(),
            "ledReportsSent": hid.led_reports_sent,
            "ledReportFailures": hid.led_report_failures,
            "framesSent": screens.frames_sent,
            "screenBytes": screens.bytes_sent,
            "dumpMismatches": screens.dump_mismatches,
            "fonts": self.inner.screen.fonts().description(),
        })
    }
}

impl Drop for Mk3Driver {
    fn drop(&mut self) {
        self.inner.shutdown();
    }
}

impl Inner {
    fn shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    fn status(&self) -> DriverStatus {
        let state = lock(&self.state);
        DriverStatus { connected: state.connected, screens: state.screens, exclusive: state.exclusive, input_seen: state.input_seen, running: state.running, error: state.last_error.clone() }
    }

    fn publish(&self) {
        (self.callbacks.status)(self.status());
    }

    fn log(&self, message: &str) {
        (self.callbacks.log)(message);
    }

    fn fail(&self, message: String) {
        self.log(&message);
        lock(&self.state).last_error = Some(message);
    }

    /// Runs one change of ownership after the ones asked for before it.
    fn operate(&self, operation: impl FnOnce(&Inner)) {
        if self.shutting_down() {
            return;
        }
        self.operations.fetch_add(1, Ordering::SeqCst);
        {
            let _turn = lock(&self.operation);
            if !self.shutting_down() {
                operation(self);
            }
        }
        self.operations.fetch_sub(1, Ordering::SeqCst);
    }

    fn deliver(&self, events: Vec<InputEvent>, cancelled: bool) {
        if self.shutting_down() {
            return;
        }
        let first = {
            let mut state = lock(&self.state);
            !std::mem::replace(&mut state.input_seen, true)
        };
        if first {
            self.publish();
        }
        for event in events {
            let mut payload = event.payload();
            if cancelled {
                payload.insert("cancelled".into(), Value::Bool(true));
            }
            (self.callbacks.input)(payload);
        }
    }

    fn connection_changed(self: &Arc<Self>, connected: bool) {
        if self.shutting_down() {
            return;
        }
        let (reports, was_exclusive) = {
            let mut state = lock(&self.state);
            state.connected = connected;
            if !connected {
                state.input_seen = false;
            }
            (state.leds.reports(), state.exclusive)
        };
        if connected {
            // A reconnected controller shows our LEDs again. It does not clear a failed take-over:
            // the HID interface is independent of the screens.
            self.hid.send(reports, true);
        } else if was_exclusive {
            // Not on this thread: it belongs to the HID session, which a release may have to stop.
            let inner = self.clone();
            let _ = std::thread::Builder::new().name("mk3-release".into()).spawn(move || inner.operate(|inner| inner.leave_exclusive(Some("The Maschine MK3 was disconnected.".into()))));
        }
        self.publish();
    }

    fn screen_failed(self: &Arc<Self>, message: String) {
        if !lock(&self.state).exclusive {
            return;
        }
        let inner = self.clone();
        let _ = std::thread::Builder::new()
            .name("mk3-release".into())
            .spawn(move || inner.operate(|inner| inner.leave_exclusive(Some(format!("Maschine MK3 screens stopped: {message}")))));
    }

    fn recover_agents(&self) {
        if !self.recovered.swap(true, Ordering::SeqCst) {
            self.agents.recover(&|message| self.log(message));
        }
    }

    fn perform_connect(&self, want_exclusive: bool) {
        self.recover_agents();
        if self.shutting_down() {
            return;
        }
        let start = {
            let mut state = lock(&self.state);
            !std::mem::replace(&mut state.running, true)
        };
        if start {
            if let Some(error) = self.hid.start() {
                self.fail(error);
            }
        }
        if self.shutting_down() {
            return;
        }
        let (exclusive, owns_agents) = {
            let state = lock(&self.state);
            (state.exclusive, !state.stopped_agents.is_empty())
        };
        if want_exclusive && !exclusive {
            self.enter_exclusive();
        } else if !want_exclusive && (exclusive || owns_agents) {
            self.leave_exclusive(None);
        }
        self.publish();
    }

    fn perform_disconnect(&self) {
        let (exclusive, owns_agents, running) = {
            let state = lock(&self.state);
            (state.exclusive, !state.stopped_agents.is_empty(), state.running)
        };
        if exclusive || owns_agents {
            self.leave_exclusive(None);
        }
        if running {
            self.hid.send_now(LedState::new().reports());
            self.hid.stop();
            lock(&self.state).running = false;
        }
        {
            let mut state = lock(&self.state);
            state.connected = false;
            state.input_seen = false;
        }
        self.publish();
    }

    fn enter_exclusive(&self) {
        lock(&self.state).last_error = None;
        let present = self.screen.is_present();
        if self.shutting_down() {
            return;
        }
        if !present {
            self.fail(
                "Maschine MK3 screens were not found. Connect and power on the MK3, then retry Take over screens. Native Instruments agents were not paused."
                    .into(),
            );
            return;
        }
        let log = |message: &str| self.log(message);
        let stopped = self.agents.stop(&log);
        if self.shutting_down() {
            // Shutdown restores from the record on disk.
            return;
        }
        match stopped {
            Err(error) => {
                self.fail(error);
                let records = self.agents.read_state();
                let restored = self.agents.restore(&records, &log);
                if !restored && !self.shutting_down() {
                    lock(&self.state).stopped_agents = records;
                }
                return;
            }
            Ok(records) => {
                let mut state = lock(&self.state);
                state.stopped_agents = records;
                state.exclusive = true;
            }
        }
        let opened = self.screen.open().and_then(|()| self.screen.initialize_displays());
        if self.shutting_down() {
            return;
        }
        if let Err(error) = opened {
            self.log(&error);
            self.leave_exclusive(Some(error));
            return;
        }
        let reports = {
            let mut state = lock(&self.state);
            state.screens = true;
            state.leds.reports()
        };
        // The agents may have rewritten the LEDs while they shut down.
        self.hid.send(reports, true);
        self.log("exclusive mode: screens open");
    }

    fn leave_exclusive(&self, reason: Option<String>) {
        {
            let state = lock(&self.state);
            if !state.exclusive && state.stopped_agents.is_empty() {
                return;
            }
        }
        self.screen.close(true);
        if self.shutting_down() {
            return;
        }
        let records = {
            let mut state = lock(&self.state);
            state.screens = false;
            state.exclusive = false;
            std::mem::take(&mut state.stopped_agents)
        };
        let records = if records.is_empty() { self.agents.read_state() } else { records };
        let restored = self.agents.restore(&records, &|message| self.log(message));
        if !restored && !self.shutting_down() {
            // Kept, so that the next release tries again.
            lock(&self.state).stopped_agents = records;
        }
        if let Some(reason) = reason {
            lock(&self.state).last_error = Some(reason);
        }
        self.publish();
    }

    fn shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::SeqCst) {
            return;
        }
        let had_operation = self.operations.load(Ordering::SeqCst) > 0;
        let (screens, exclusive, running) = {
            let state = lock(&self.state);
            (state.screens, state.exclusive, state.running)
        };
        if screens || exclusive {
            self.screen.close(true);
        }
        if running {
            self.hid.send_now(LedState::new().reports());
            self.hid.stop();
        }
        // A take-over may still be stopping agents. Wait for it to notice the shutdown and return
        // (it does so after its current step), then restore what it stopped: from our own records,
        // or from the record on disk when the take-over never got to report them.
        let _turn = lock(&self.operation);
        // Whatever that operation still opened before it noticed is closed now.
        self.screen.close(true);
        self.hid.stop();
        let owned = std::mem::take(&mut lock(&self.state).stopped_agents);
        if had_operation || exclusive || !owned.is_empty() {
            let records = if owned.is_empty() { self.agents.read_state() } else { owned };
            if !records.is_empty() {
                self.agents.restore(&records, &|message| self.log(message));
            }
        }
        let mut state = lock(&self.state);
        state.exclusive = false;
        state.screens = false;
        state.running = false;
        state.connected = false;
        state.input_seen = false;
    }
}

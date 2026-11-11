//! Ownership of the controller in every order, with the hardware and the vendor's agents
//! replaced by `st_mk3::fake`. No hardware and no Native Instruments process is touched.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use st_mk3::fake::{FakeAgents, FakeHid, FakeScreens};
use st_mk3::screen::font::Fonts;
use st_mk3::transport::AgentRecord;
use st_mk3::{DriverCallbacks, DriverOptions, DriverStatus, Mk3Driver};

struct Fixture {
    hid: FakeHid,
    screens: FakeScreens,
    agents: FakeAgents,
    driver: Arc<Mk3Driver>,
    statuses: Arc<Mutex<Vec<DriverStatus>>>,
    log: Arc<Mutex<Vec<String>>>,
}

fn fixture() -> Fixture {
    let (hid, screens, agents) = (FakeHid::new(), FakeScreens::new(), FakeAgents::new());
    hid.plug();
    let statuses: Arc<Mutex<Vec<DriverStatus>>> = Arc::default();
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let (for_status, for_log) = (statuses.clone(), log.clone());
    let driver = Mk3Driver::new(DriverOptions {
        hid: Arc::new(hid.clone()),
        screens: Arc::new(screens.clone()),
        agents: Arc::new(agents.clone()),
        fonts: Arc::new(Fonts::builtin()),
        callbacks: DriverCallbacks {
            status: Box::new(move |status| for_status.lock().unwrap().push(status)),
            log: Box::new(move |message| for_log.lock().unwrap().push(message.to_string())),
            ..DriverCallbacks::default()
        },
        dump_dir: None,
        dump_all: false,
        poll_interval: Duration::from_millis(2),
        // One attempt: a failed open is a failed take-over.
        screen_open_timeout: Duration::ZERO,
        frame_interval: Duration::from_millis(2),
    });
    Fixture { hid, screens, agents, driver: Arc::new(driver), statuses, log }
}

impl Fixture {
    /// Starts a call on another thread, as two commands from the interface arrive.
    fn spawn(&self, call: impl FnOnce(&Mk3Driver) -> DriverStatus + Send + 'static) -> thread::JoinHandle<DriverStatus> {
        let driver = self.driver.clone();
        thread::spawn(move || call(&driver))
    }

    fn wait_connected(&self) {
        wait_for("the controller", || self.driver.status().connected);
    }
}

fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(2));
    }
}

/// Lets queued calls reach the point where they wait for their turn.
fn settle() {
    thread::sleep(Duration::from_millis(40));
}

#[test]
fn release_during_a_takeover_wins() {
    let f = fixture();
    f.agents.stop_gate.close();
    let connecting = f.spawn(|driver| driver.connect(true));
    assert!(f.agents.stop_gate.wait_entered(), "the take-over is stopping agents");
    let disconnecting = f.spawn(|driver| driver.disconnect());
    settle();
    f.agents.stop_gate.open();
    connecting.join().unwrap();
    let status = disconnecting.join().unwrap();
    assert!(!status.exclusive && !status.screens && !status.running, "{status:?}");
    assert_eq!((f.agents.stop_calls(), f.agents.restore_calls()), (1, 1));
    assert!(!f.screens.is_open());
}

#[test]
fn duplicate_takeovers_stop_the_agents_once() {
    let f = fixture();
    f.agents.stop_gate.close();
    let connecting = f.spawn(|driver| driver.connect(true));
    assert!(f.agents.stop_gate.wait_entered());
    let again = f.spawn(|driver| driver.connect(true));
    settle();
    f.agents.stop_gate.open();
    connecting.join().unwrap();
    let status = again.join().unwrap();
    assert!(status.exclusive && status.screens && status.error.is_none(), "{status:?}");
    assert_eq!((f.agents.stop_calls(), f.screens.opens()), (1, 1));
    assert!(f.log.lock().unwrap().iter().any(|line| line == "exclusive mode: screens open"));
    f.driver.disconnect();
    assert_eq!(f.agents.restore_calls(), 1);
}

#[test]
fn reconnect_in_shared_mode_after_a_release_keeps_running() {
    let f = fixture();
    f.agents.stop_gate.close();
    let connecting = f.spawn(|driver| driver.connect(true));
    assert!(f.agents.stop_gate.wait_entered());
    let disconnecting = f.spawn(|driver| driver.disconnect());
    settle();
    let reconnecting = f.spawn(|driver| driver.connect(false));
    settle();
    f.agents.stop_gate.open();
    for call in [connecting, disconnecting, reconnecting] {
        call.join().unwrap();
    }
    let status = f.driver.status();
    assert!(!status.exclusive && !status.screens && status.running, "{status:?}");
    assert_eq!((f.agents.stop_calls(), f.agents.restore_calls()), (1, 1));
    f.wait_connected();
}

fn shutdown_during_takeover(queued: bool) {
    let f = fixture();
    f.agents.stop_gate.close();
    let connecting = f.spawn(|driver| driver.connect(true));
    assert!(f.agents.stop_gate.wait_entered());
    let second = queued.then(|| f.spawn(|driver| driver.connect(true)));
    settle();
    let gate = f.agents.stop_gate.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(40));
        gate.open();
    });
    f.driver.shutdown();
    connecting.join().unwrap();
    if let Some(second) = second {
        second.join().unwrap();
    }
    settle();
    let status = f.driver.status();
    assert!(!status.running && !status.connected && !status.exclusive && !status.screens, "{status:?}");
    assert_eq!((f.agents.stop_calls(), f.agents.restore_calls(), f.screens.opens()), (1, 1, 0), "the agents are restored from the record on disk and the screens never opened");
    assert!(f.agents.read_state_is_empty());
    // After shutdown nothing starts again.
    assert!(!f.driver.connect(true).running);
    assert_eq!(f.agents.stop_calls(), 1);
}

#[test]
fn shutdown_interrupts_a_takeover() {
    shutdown_during_takeover(false);
}

#[test]
fn shutdown_interrupts_a_takeover_with_another_one_queued() {
    shutdown_during_takeover(true);
}

#[test]
fn shutdown_during_a_release_restores_once() {
    let f = fixture();
    assert!(f.driver.connect(true).screens);
    // Something is on a screen, so that releasing has to blank it.
    assert!(f.driver.set_screens(&serde_json::json!({"left": [{"op": "clear", "color": "#ffffff"}]})));
    wait_for("the frame", || f.screens.packets().len() == 3);
    // The release hangs while it blanks the screens.
    f.screens.write_gate.close();
    let releasing = f.spawn(|driver| driver.disconnect());
    assert!(f.screens.write_gate.wait_entered(), "the release is blanking the screens");
    let gate = f.screens.write_gate.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(40));
        gate.open();
    });
    f.driver.shutdown();
    releasing.join().unwrap();
    settle();
    let status = f.driver.status();
    assert!(!status.running && !status.exclusive && !status.screens, "{status:?}");
    assert_eq!(f.agents.restore_calls(), 1);
    assert!(!f.screens.is_open());
}

#[test]
fn shared_shutdown_leaves_the_agents_alone() {
    let f = fixture();
    f.driver.connect(false);
    f.wait_connected();
    // A record some other run left on disk must not make a shared-only shutdown restore agents.
    f.agents.set_recorded(vec![AgentRecord::default()]);
    f.driver.shutdown();
    settle();
    let status = f.driver.status();
    assert!(!status.running && !status.connected, "{status:?}");
    assert_eq!((f.agents.stop_calls(), f.agents.restore_calls()), (0, 0));
    // The LEDs were switched off on the way out.
    assert_eq!(f.hid.last_report(0x80).unwrap()[1..], [0u8; 62]);
    assert_eq!(f.hid.last_report(0x81).unwrap()[1..], [0u8; 41]);
}

#[test]
fn a_failed_restore_is_retried_at_the_next_release() {
    let f = fixture();
    f.driver.connect(true);
    f.agents.fail_restores(1);
    f.driver.disconnect();
    f.driver.disconnect();
    assert!(!f.driver.status().exclusive);
    assert_eq!(f.agents.restore_calls(), 2);
    assert!(f.agents.read_state_is_empty());
    f.driver.disconnect();
    assert_eq!(f.agents.restore_calls(), 2, "nothing is left to restore");
}

#[test]
fn a_missing_device_fails_before_any_agent_is_stopped() {
    let f = fixture();
    f.screens.set_present(false);
    let reply = f.driver.connect(true);
    let original = reply.error.clone().expect("an explanation");
    assert!(original.contains("Connect and power on"), "{original}");
    // The controller's HID side reconnecting does not clear the diagnosis.
    f.wait_connected();
    f.hid.unplug();
    wait_for("the unplug", || !f.driver.status().connected);
    f.hid.plug();
    f.wait_connected();
    let status = f.driver.status();
    assert!(!status.screens && !status.exclusive && status.error.as_deref() == Some(original.as_str()), "{status:?}");
    assert_eq!((f.agents.stop_calls(), f.screens.opens()), (0, 0));

    f.screens.set_present(true);
    let status = f.driver.connect(true);
    assert!(status.screens && status.exclusive && status.error.is_none(), "{status:?}");
    assert_eq!(f.agents.stop_calls(), 1);
    f.driver.disconnect();
    assert_eq!(f.agents.restore_calls(), 1);
}

fn failed_takeover_then_retry(arrange: impl FnOnce(&FakeScreens), expected: &str) {
    let f = fixture();
    arrange(&f.screens);
    let reply = f.driver.connect(true);
    let original = reply.error.clone().expect("an explanation");
    assert!(original.contains(expected), "{original}");
    f.wait_connected();
    f.hid.unplug();
    wait_for("the unplug", || !f.driver.status().connected);
    f.hid.plug();
    f.wait_connected();
    let status = f.driver.status();
    assert!(!status.screens && !status.exclusive && status.error.as_deref() == Some(original.as_str()), "{status:?}");
    assert_eq!(f.agents.restore_calls(), 1, "the failed take-over gave the screens back");
    assert!(f.agents.read_state_is_empty());
    assert!(!f.screens.is_open());

    let status = f.driver.connect(true);
    assert!(status.screens && status.exclusive && status.error.is_none(), "{status:?}");
    assert_eq!(f.screens.opens(), 2);
    f.driver.disconnect();
    assert_eq!(f.agents.restore_calls(), 2);
}

#[test]
fn a_failed_open_restores_the_agents_and_can_be_retried() {
    failed_takeover_then_retry(|screens| screens.fail_opens(1), "fixture USB open failure");
}

#[test]
fn a_failed_initialization_restores_the_agents_and_can_be_retried() {
    failed_takeover_then_retry(|screens| screens.fail_writes(1), "fixture USB transfer failure");
}

#[test]
fn an_incomplete_first_transfer_is_a_failed_takeover() {
    failed_takeover_then_retry(|screens| screens.shorten_writes(1), "screen transfer was incomplete (31 of 32 bytes)");
}

#[test]
fn agents_that_cannot_be_stopped_are_restored_and_reported() {
    let f = fixture();
    f.agents.fail_stop(Some("Could not stop NIHardwareAgent."));
    let status = f.driver.connect(true);
    assert!(!status.exclusive && !status.screens && status.error.as_deref() == Some("Could not stop NIHardwareAgent."), "{status:?}");
    assert_eq!((f.agents.stop_calls(), f.agents.restore_calls(), f.screens.opens()), (1, 1, 0));
    assert!(f.agents.read_state_is_empty());
    // Whatever could not be restored then is restored at the next release.
    f.agents.fail_restores(1);
    f.driver.connect(true);
    assert_eq!(f.agents.restore_calls(), 2);
    f.agents.fail_stop(None);
    f.driver.disconnect();
    assert_eq!(f.agents.restore_calls(), 3);
    assert!(f.agents.read_state_is_empty());
}

#[test]
fn crash_recovery_runs_once_and_shared_mode_gives_back_the_screens() {
    let f = fixture();
    f.driver.recover_agents();
    f.driver.connect(false);
    f.driver.connect(false);
    assert_eq!(f.agents.recover_calls(), 1, "once per driver");
    assert!(f.driver.connect(true).screens);
    // Asking for shared mode releases the screens but keeps the controller.
    let status = f.driver.connect(false);
    assert!(!status.exclusive && !status.screens && status.running, "{status:?}");
    assert_eq!(f.agents.restore_calls(), 1);
    assert!(!f.screens.is_open());
    // Every change was announced.
    let statuses = f.statuses.lock().unwrap();
    assert!(statuses.iter().any(|status| status.screens) && !statuses.last().unwrap().screens);
}

#[test]
fn opening_the_screens_waits_for_their_previous_owner() {
    let (hid, screens, agents) = (FakeHid::new(), FakeScreens::new(), FakeAgents::new());
    screens.fail_opens(2);
    let driver = Mk3Driver::new(DriverOptions {
        hid: Arc::new(hid),
        screens: Arc::new(screens.clone()),
        agents: Arc::new(agents),
        fonts: Arc::new(Fonts::builtin()),
        callbacks: DriverCallbacks::default(),
        dump_dir: None,
        dump_all: false,
        poll_interval: Duration::from_millis(2),
        screen_open_timeout: Duration::from_secs(4),
        frame_interval: Duration::from_millis(2),
    });
    let status = driver.connect(true);
    assert!(status.screens && status.error.is_none(), "{status:?}");
    assert_eq!(screens.opens(), 3, "retried while the interface was still held");
    drop(driver);
    assert!(!screens.is_open(), "dropping the driver releases the screens");
}

trait AgentStateExt {
    fn read_state_is_empty(&self) -> bool;
}

impl AgentStateExt for FakeAgents {
    fn read_state_is_empty(&self) -> bool {
        use st_mk3::transport::VendorAgents;
        self.read_state().is_empty()
    }
}

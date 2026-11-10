//! The service as `st-host` uses it, against the in-memory controller: commands with the page's
//! JSON, input and status events, LED and screen output, hot-plug, failures, dumps and the probe.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use st_mk3::fake::{FakeAgents, FakeHid, FakeScreens};
use st_mk3::protocol::{led, LedState};
use st_mk3::screen::encode::{self, DEVICE_FORMAT};
use st_mk3::screen::font::Fonts;
use st_mk3::screen::ops::DrawOp;
use st_mk3::screen::render::Renderer;
use st_mk3::screen::PIXEL_COUNT;
use st_mk3::{Configure, Connect, Mk3Options, Mk3Service, Mk3Settings, StatusRequest};

fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(2));
    }
}

struct Fixture {
    hid: FakeHid,
    screens: FakeScreens,
    agents: FakeAgents,
    service: Arc<Mk3Service>,
    events: Arc<Mutex<Vec<(String, Value)>>>,
    saved: Arc<Mutex<Vec<Mk3Settings>>>,
}

fn fixture_with(enabled: bool, adjust: impl FnOnce(&mut Mk3Options)) -> Fixture {
    let (hid, screens, agents) = (FakeHid::new(), FakeScreens::new(), FakeAgents::new());
    let events: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    let saved: Arc<Mutex<Vec<Mk3Settings>>> = Arc::default();
    let (for_events, for_saved) = (events.clone(), saved.clone());
    let mut options = Mk3Options {
        agents: Arc::new(agents.clone()),
        settings: Mk3Settings { enabled },
        save_settings: Box::new(move |settings| {
            for_saved.lock().unwrap().push(settings.clone());
            Ok(())
        }),
        events: Box::new(move |name, payload| for_events.lock().unwrap().push((name.to_string(), payload))),
        ..Mk3Options::with(Arc::new(hid.clone()), Arc::new(screens.clone()))
    };
    adjust(&mut options);
    Fixture { hid, screens, agents, service: Mk3Service::new(options), events, saved }
}

fn fixture(enabled: bool) -> Fixture {
    fixture_with(enabled, |_| {})
}

impl Fixture {
    fn take(&self, name: &str) -> Vec<Value> {
        let mut events = self.events.lock().unwrap();
        let taken: Vec<Value> = events.iter().filter(|(event, _)| event == name).map(|(_, payload)| payload.clone()).collect();
        events.retain(|(event, _)| event != name);
        taken
    }

    fn status(&self) -> Value {
        json!(self.service.status(StatusRequest::default()).unwrap())
    }

    fn connected(&self) {
        wait_for("the controller", || self.status()["connected"] == true);
    }

    fn screen_frames(&self) -> [Vec<u16>; 2] {
        // Replays every packet the fake received, as the device would.
        let mut frames = [vec![0x1234u16; PIXEL_COUNT], vec![0x1234u16; PIXEL_COUNT]];
        for packet in self.screens.packets() {
            assert!(encode::apply(&packet, &mut frames[usize::from(packet[2])]), "every packet decodes");
        }
        frames
    }
}

fn button_report(edit: impl FnOnce(&mut [u8])) -> Vec<u8> {
    let mut report = vec![0u8; 42];
    report[0] = 0x01;
    edit(&mut report);
    report
}

fn rendered(ops: Value) -> Vec<u16> {
    let mut renderer = Renderer::new(Arc::new(Fonts::builtin()));
    renderer.render(&DrawOp::parse(ops.as_array().unwrap()));
    renderer.framebuffer(DEVICE_FORMAT)
}

#[test]
fn support_is_off_until_the_user_turns_it_on() {
    let f = fixture(false);
    f.hid.plug();
    f.service.start();
    assert_eq!(f.agents.recover_calls(), 1, "crash recovery runs even while support is off");
    assert_eq!(f.take("mk3.status"), [json!({"connected": false, "screens": false, "exclusive": false, "inputSeen": false, "running": false, "enabled": false})]);
    thread::sleep(Duration::from_millis(30));
    assert_eq!(f.hid.opens(), 0, "the controller is not touched");
    assert_eq!(
        f.service.connect(Connect { exclusive: true }).unwrap_err().to_string(),
        "Maschine MK3 support is in development and off. Turn it on in Settings first."
    );
    assert_eq!(f.service.configure(Configure::default()).unwrap_err().to_string(), "Choose whether Maschine MK3 support is on.");
    assert_eq!(json!(f.service.leds(json!({"pads": ["#ff0000"]})).unwrap()), json!({"ok": true}), "LED state is kept for later");
    assert_eq!(json!(f.service.screen(json!({"left": []})).unwrap()), json!({"screens": false}));

    // Turning it on connects in shared mode and stores the choice.
    let on = json!(f.service.configure(serde_json::from_value(json!({"enabled": true})).unwrap()).unwrap());
    assert!(on["enabled"] == true && on["running"] == true && on["exclusive"] == false, "{on}");
    assert_eq!(f.saved.lock().unwrap().last(), Some(&Mk3Settings { enabled: true }));
    f.connected();
    wait_for("the LEDs set while off", || f.hid.last_report(0x81).is_some_and(|report| report[26 + 12] == led::indexed(st_mk3::protocol::Rgb::from_hex("#ff0000"))));
    let statuses = f.take("mk3.status");
    assert!(statuses.iter().any(|status| status["connected"] == true && status["enabled"] == true), "{statuses:?}");

    // Turning it off releases the controller: LEDs off, nothing running.
    let off = json!(f.service.configure(Configure { enabled: Some(false) }).unwrap());
    assert!(off["enabled"] == false && off["running"] == false && off["connected"] == false, "{off}");
    assert_eq!(f.hid.last_report(0x81).unwrap()[1..], [0u8; 41]);
    assert_eq!(f.agents.stop_calls(), 0, "shared mode never touches the vendor's agents");
    assert_eq!(serde_json::from_value::<Mk3Settings>(json!({})).unwrap(), Mk3Settings { enabled: false }, "off by default");
    // A setting that cannot be stored changes nothing.
    let failing = fixture_with(false, |options| options.save_settings = Box::new(|_| Err("the disk is full.".into())));
    assert_eq!(failing.service.configure(Configure { enabled: Some(true) }).unwrap_err().to_string(), "The Maschine MK3 setting could not be saved: the disk is full.");
    assert_eq!(failing.status()["enabled"], false);
}

#[test]
fn input_reports_become_events_and_an_unplug_releases_what_was_held() {
    let f = fixture(true);
    f.hid.plug();
    f.service.start();
    f.connected();
    assert_eq!(f.status()["inputSeen"], false);
    f.take("mk3.status");

    f.hid.send(&button_report(|_| {}));
    // Play and Shift down, the pedal down, encoder 1 touched; delivered without the report id.
    f.hid.send(
        &button_report(|report| {
            report[6] = 0x20;
            report[1] = 0x40 | 0x02;
            report[10] = 0x80;
        })[1..],
    );
    let mut pads = vec![0u8; 64];
    pads[0] = 0x02;
    pads[1..4].copy_from_slice(&[12, 0x1A, 0x00]);
    f.hid.send(&pads);
    wait_for("the events", || f.events.lock().unwrap().iter().filter(|(name, _)| name == "mk3.input").count() >= 8);
    let input = f.take("mk3.input");
    assert_eq!(input[..3].iter().map(|event| event["type"].as_str().unwrap()).collect::<Vec<_>>(), ["knob", "knob", "knob"], "the first report carries the analog knobs");
    assert!(input.contains(&json!({"type": "touch", "target": "encoder1", "touched": true})));
    assert!(input.contains(&json!({"type": "button", "name": "play", "pressed": true})));
    assert!(input.contains(&json!({"type": "button", "name": "shift", "pressed": true})));
    assert!(input.contains(&json!({"type": "pedal", "down": true})));
    assert_eq!(input.last().unwrap(), &json!({"type": "pad", "pad": 1, "phase": "down", "pressure": 0.625, "velocity": 96}));
    // A status from before the first report may still be on its way; what counts is one announcement.
    let seen = f.take("mk3.status");
    assert_eq!(seen.iter().filter(|status| status["inputSeen"] == true).count(), 1, "input being seen is announced once: {seen:?}");

    // Unplugged with everything held: the releases are made up and marked as cancelled.
    f.hid.unplug();
    wait_for("the unplug", || f.status()["connected"] == false);
    let cancelled = f.take("mk3.input");
    for expected in [
        json!({"type": "pad", "pad": 1, "phase": "up", "pressure": 0.0, "cancelled": true}),
        json!({"type": "pedal", "down": false, "cancelled": true}),
        json!({"type": "touch", "target": "encoder1", "touched": false, "cancelled": true}),
        json!({"type": "button", "name": "play", "pressed": false, "cancelled": true}),
        json!({"type": "button", "name": "shift", "pressed": false, "cancelled": true}),
    ] {
        assert!(cancelled.contains(&expected), "{expected} in {cancelled:?}");
    }
    assert_eq!(cancelled.len(), 5);
    assert_eq!(f.status()["inputSeen"], false);
    // Plugged in again: found by itself, with fresh baselines.
    f.hid.plug();
    f.connected();
    f.hid.send(&button_report(|report| report[11] = 9));
    f.hid.send(&button_report(|report| report[11] = 10));
    wait_for("the main encoder", || f.take("mk3.input").contains(&json!({"type": "main", "delta": 1})));
    let diagnostics = json!(f.service.status(StatusRequest { diagnostics: true }).unwrap());
    assert_eq!(diagnostics["diagnostics"]["reports"], json!({"0x01": 4, "0x02": 1}));
    assert!(diagnostics["diagnostics"]["firstReports"]["0x02"].as_str().unwrap().starts_with("02 0c 1a 00"));
    assert_eq!(diagnostics["diagnostics"]["fonts"], "built-in bitmap font");
    assert_eq!(f.hid.opens(), 2);
}

#[test]
fn leds_are_sent_when_they_change_and_again_after_a_reconnect() {
    let f = fixture(true);
    f.hid.plug();
    f.service.start();
    f.connected();
    wait_for("the initial LED reports", || f.hid.written().len() == 2);
    assert_eq!(f.hid.written(), LedState::new().reports(), "a newly found controller is told the whole LED state");

    f.hid.clear_written();
    f.service.leds(json!({"buttons": {"play": 1}, "pads": ["#00ff00"]})).unwrap();
    wait_for("both reports", || f.hid.written().len() == 2);
    assert_eq!(f.hid.last_report(0x80).unwrap()[42], 0x7F);
    assert_eq!(f.hid.last_report(0x81).unwrap()[26 + 12], 7 << 2 | 3);
    // Only the report that changed is sent; an update that changes nothing sends nothing.
    f.hid.clear_written();
    f.service.leds(json!({"buttons": {"play": 0.5}})).unwrap();
    wait_for("the button report", || f.hid.written().len() == 1);
    f.service.leds(json!({"buttons": {"play": 0.5}})).unwrap();
    thread::sleep(Duration::from_millis(60));
    assert_eq!(f.hid.written().iter().map(|report| report[0]).collect::<Vec<_>>(), [0x80]);
    // A burst is coalesced: far fewer reports than updates, and the last state wins.
    f.hid.clear_written();
    for step in 0..200 {
        f.service.leds(json!({"buttons": {"rec": f64::from(step % 2)}, "pads": [if step % 2 == 0 { "#ff0000" } else { "#0000ff" }]})).unwrap();
    }
    f.service.leds(json!({"buttons": {"rec": 1}, "pads": ["#ffffff"]})).unwrap();
    wait_for("the final state", || f.hid.last_report(0x80).is_some_and(|report| report[43] == 0x7F) && f.hid.last_report(0x81).is_some_and(|report| report[26 + 12] == 17 << 2 | 3));
    assert!(f.hid.written().len() < 100, "{} reports for 201 updates", f.hid.written().len());

    // A failed write is counted and reported, and the next change is sent again.
    f.hid.fail_writes(Some("the pipe stalled"));
    f.service.leds(json!({"buttons": {"stop": 1}})).unwrap();
    wait_for("the failure", || json!(f.service.status(StatusRequest { diagnostics: true }).unwrap())["diagnostics"]["ledReportFailures"] == json!(["0x80: the pipe stalled"]));
    f.hid.fail_writes(None);
    f.service.leds(json!({"buttons": {"stop": 0.9}})).unwrap();
    wait_for("the retried report", || f.hid.last_report(0x80).is_some_and(|report| report[44] == 0x7F));

    // After a reconnect the controller gets the whole state again without being asked.
    f.hid.unplug();
    wait_for("the unplug", || f.status()["connected"] == false);
    f.hid.clear_written();
    f.hid.plug();
    wait_for("the state after reconnecting", || f.hid.written().len() == 2);
    assert!(f.hid.last_report(0x80).unwrap()[43] == 0x7F && f.hid.last_report(0x81).unwrap()[26 + 12] == 17 << 2 | 3);
}

#[test]
fn screens_show_the_display_lists_once_taken_over() {
    let f = fixture(true);
    f.hid.plug();
    f.service.start();
    f.connected();
    let left = json!([{"op": "clear", "color": "#101820"}, {"op": "text", "x": 12, "y": 10, "text": "Pattern 01", "size": 20, "color": "#ffffff", "weight": "bold"}, {"op": "bar", "x": 12, "y": 250, "w": 456, "h": 8, "value": 0.4, "color": "#5ad19a", "background": "#2a2f3a"}]);
    let right = json!([{"op": "clear", "color": "#000000"}, {"op": "arc", "x": 240, "y": 136, "r": 60, "value": 0.6, "color": "#ffaa00", "background": "#333333", "width": 8}]);
    assert_eq!(json!(f.service.screen(json!({"left": left, "right": right})).unwrap()), json!({"screens": false}), "ignored until the screens are taken over");
    thread::sleep(Duration::from_millis(20));
    assert!(f.screens.packets().is_empty());

    let status = json!(f.service.connect(Connect { exclusive: true }).unwrap());
    assert!(status["exclusive"] == true && status["screens"] == true && status["enabled"] == true && status.get("error").is_none(), "{status}");
    assert_eq!(f.agents.stop_calls(), 1);
    assert_eq!(f.screens.packets().len(), 2, "both screens were made black first");
    assert!(f.screen_frames().iter().all(|frame| frame.iter().all(|pixel| *pixel == 0)));

    assert_eq!(json!(f.service.screen(json!({"left": left, "right": right})).unwrap()), json!({"screens": true}));
    wait_for("both frames", || f.screens.packets().len() == 4);
    let frames = f.screen_frames();
    assert!(frames[0] == rendered(left.clone()) && frames[1] == rendered(right.clone()), "the device shows exactly what was drawn");
    // Only the side that changed is sent, as the region that changed.
    let moved = json!([{"op": "clear", "color": "#101820"}, {"op": "text", "x": 12, "y": 10, "text": "Pattern 02", "size": 20, "color": "#ffffff", "weight": "bold"}, {"op": "bar", "x": 12, "y": 250, "w": 456, "h": 8, "value": 0.4, "color": "#5ad19a", "background": "#2a2f3a"}]);
    f.service.screen(json!({"left": moved})).unwrap();
    wait_for("the changed frame", || f.screens.packets().len() == 5);
    let update = f.screens.packets().pop().unwrap();
    assert_eq!(update[2], 0, "the left screen");
    let (x, y, width, height) = (usize::from(update[9]), usize::from(update[11]), usize::from(update[13]), usize::from(update[15]));
    assert!(x >= 100 && y >= 8 && width <= 24 && height <= 24, "just the digit that changed: {x},{y} {width}×{height}");
    assert_eq!(f.screen_frames()[0], rendered(moved.clone()));
    f.service.screen(json!({"left": moved})).unwrap();
    thread::sleep(Duration::from_millis(30));
    assert_eq!(f.screens.packets().len(), 5, "an identical frame sends nothing");

    // A burst of frames is coalesced; the last one is what the screen ends up showing.
    for step in 0..100 {
        f.service.screen(json!({"right": [{"op": "bar", "x": 0, "y": 0, "w": 480, "h": 272, "value": f64::from(step) / 100.0, "color": "#ffffff"}]})).unwrap();
    }
    let last = json!([{"op": "rect", "x": 0, "y": 0, "w": 480, "h": 272, "color": "#ff0000"}]);
    let started = Instant::now();
    f.service.screen(json!({"right": last, "dumpTag": "ignored without a dump folder"})).unwrap();
    assert!(started.elapsed() < Duration::from_millis(100), "submitting never waits for a transfer");
    wait_for("the last frame", || f.screen_frames()[1] == rendered(last.clone()));
    assert!(f.screens.packets().len() < 60, "{} packets for 101 frames", f.screens.packets().len());
    let diagnostics = json!(f.service.status(StatusRequest { diagnostics: true }).unwrap());
    assert_eq!(diagnostics["diagnostics"]["framesSent"], json!(f.screens.packets().len()));
    assert!(diagnostics["diagnostics"]["screenBytes"].as_u64().unwrap() > 1000);

    // Release: both screens are blanked, the interface is given back and the agents restored.
    let released = json!(f.service.disconnect(serde::de::IgnoredAny).unwrap());
    assert!(released["exclusive"] == false && released["screens"] == false && released["running"] == false, "{released}");
    assert!(f.screen_frames().iter().all(|frame| frame.iter().all(|pixel| *pixel == 0)), "blank at release");
    assert!(!f.screens.is_open());
    assert_eq!(f.agents.restore_calls(), 1);
    assert_eq!(json!(f.service.screen(json!({"left": left})).unwrap()), json!({"screens": false}));
}

#[test]
fn a_failed_transfer_is_retried_in_full_and_three_in_a_row_give_the_screens_back() {
    let f = fixture(true);
    f.hid.plug();
    f.service.start();
    f.service.connect(Connect { exclusive: true }).unwrap();
    let first = json!([{"op": "rect", "x": 100, "y": 100, "w": 40, "h": 20, "color": "#00ff00"}]);
    f.service.screen(json!({"left": first})).unwrap();
    wait_for("the first frame", || f.screens.packets().len() == 3);
    // One transfer fails, the next is cut short: the frame is not counted as shown, and is sent
    // again as a whole screen because what the device shows is no longer known.
    f.screens.fail_writes(1);
    f.screens.shorten_writes(1);
    let second = json!([{"op": "rect", "x": 100, "y": 100, "w": 40, "h": 20, "color": "#0000ff"}]);
    f.service.screen(json!({"left": second})).unwrap();
    wait_for("the retried frame", || f.screens.packets().len() == 4);
    let retried = f.screens.packets().pop().unwrap();
    assert_eq!(retried[8..16], [0, 0, 0, 0, 0x01, 0xE0, 0x01, 0x10], "a full-screen packet");
    assert_eq!(f.screen_frames()[0], rendered(second));
    assert_eq!(f.status()["screens"], true, "two failures are survived");

    // Three in a row: the screens are given back, the agents restored, and the status says why.
    f.take("mk3.status");
    f.screens.fail_writes(3);
    f.service.screen(json!({"left": [{"op": "clear", "color": "#ffffff"}]})).unwrap();
    wait_for("the screens to be released", || f.status()["exclusive"] == false);
    let status = f.status();
    assert!(status["screens"] == false && status["connected"] == true && status["running"] == true, "{status}");
    assert_eq!(status["error"], "Maschine MK3 screens stopped: fixture USB transfer failure");
    assert_eq!(f.agents.restore_calls(), 1);
    assert!(!f.screens.is_open());
    assert!(f.take("mk3.status").iter().any(|event| event["error"] == "Maschine MK3 screens stopped: fixture USB transfer failure"), "the interface is told");
    // A new take-over starts clean.
    let again = json!(f.service.connect(Connect { exclusive: true }).unwrap());
    assert!(again["screens"] == true && again.get("error").is_none(), "{again}");
}

#[test]
fn unplugging_during_exclusive_mode_gives_the_screens_back() {
    let f = fixture(true);
    f.hid.plug();
    f.service.start();
    f.connected();
    f.service.connect(Connect { exclusive: true }).unwrap();
    f.hid.unplug();
    wait_for("the release", || f.status()["exclusive"] == false);
    assert_eq!(f.status()["error"], "The Maschine MK3 was disconnected.");
    assert_eq!(f.agents.restore_calls(), 1);
    assert!(!f.screens.is_open());
    f.service.shutdown();
    assert_eq!(f.agents.restore_calls(), 1, "shutdown has nothing left to restore");
    assert_eq!(f.status()["running"], false);
}

#[test]
fn frames_can_be_dumped_as_pictures_without_a_controller() {
    let folder = std::env::temp_dir().join(format!("st-mk3-dump-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    let dump = folder.clone();
    let f = fixture_with(true, move |options| {
        options.dump_dir = Some(dump);
        options.dump_all = true;
    });
    // No controller at all: the frames are still rendered, for looking at layouts.
    let result = f.service.screen(json!({"left": [{"op": "clear", "color": "#203040"}, {"op": "text", "x": 10, "y": 10, "text": "Dump", "size": 24, "color": "#ffffff"}], "dumpTag": "home page"})).unwrap();
    assert!(!result.screens);
    let expected = ["left.png", "left-device.png", "home_page-left.png", "home_page-left-device.png", "frame-00001-left.png", "frame-00001-left-device.png"];
    wait_for("the pictures", || expected.iter().all(|name| folder.join(name).exists()));
    for name in expected {
        let picture = std::fs::read(folder.join(name)).unwrap();
        assert_eq!(picture[..8], [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A], "{name}");
        assert!(picture.len() > 480 * 272 * 3, "{name} holds every pixel");
    }
    assert!(!folder.join("right.png").exists());
    assert_eq!(json!(f.service.status(StatusRequest { diagnostics: true }).unwrap())["diagnostics"]["dumpMismatches"], 0);
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn the_probe_exercises_leds_screens_and_reports_what_it_saw() {
    use st_mk3::probe::{self, ProbeOptions};
    use st_mk3::{DriverCallbacks, DriverOptions};

    #[derive(Clone, Default)]
    struct Output(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let options = |hid: &FakeHid, screens: &FakeScreens, agents: &FakeAgents| DriverOptions {
        hid: Arc::new(hid.clone()),
        screens: Arc::new(screens.clone()),
        agents: Arc::new(agents.clone()),
        fonts: Arc::new(Fonts::builtin()),
        callbacks: DriverCallbacks::default(),
        dump_dir: None,
        dump_all: false,
        poll_interval: Duration::from_millis(2),
        screen_open_timeout: Duration::ZERO,
        frame_interval: Duration::from_millis(2),
    };
    let (hid, screens, agents) = (FakeHid::new(), FakeScreens::new(), FakeAgents::new());
    let output = Output::default();
    assert_eq!(probe::run(options(&hid, &screens, &agents), ProbeOptions { seconds: 0.2, exclusive: false, raw: false }, output.clone()), 2);
    assert!(String::from_utf8(output.0.lock().unwrap().clone()).unwrap().contains("no Maschine MK3 (17cc:1600) found"));

    hid.plug();
    let presser = hid.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        presser.send(&button_report(|_| {}));
        presser.send(&button_report(|report| report[6] = 0x20));
    });
    let output = Output::default();
    assert_eq!(probe::run(options(&hid, &screens, &agents), ProbeOptions { seconds: 0.5, exclusive: true, raw: true }, output.clone()), 0);
    let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    for expected in [
        "mk3-probe: EXCLUSIVE (NI agents will be stopped and restored) mode, 0 s",
        "leds pads rainbow",
        "leds all button backlights level 0.25 → byte 0x0c",
        r#"input {"name":"play","pressed":true,"type":"button"}"#,
        "raw 0x01 00 00 00 00 00 20",
        "log exclusive mode: screens open",
        "event counts {\"button\":1,\"knob\":3}",
        r#"final status {"connected":false,"exclusive":false,"inputSeen":false,"running":false,"screens":false}"#,
    ] {
        assert!(text.contains(expected), "{expected:?} in:\n{text}");
    }
    assert!(text.contains("\"framesSent\":") && !text.contains("\"framesSent\":2,"), "the test pattern was drawn: {text}");
    assert_eq!((agents.stop_calls(), agents.restore_calls()), (1, 1), "the agents are back");
    assert!(!screens.is_open());
    // The rainbow reached the pads and the pattern the screens.
    assert!(hid.written().iter().any(|report| report[0] == 0x81 && report[26 + 12] == 1 << 2 | 3), "pad 1 was red");
    assert_eq!(probe::hue_hex(0.0, 1.0), "#ff0000");
    assert_eq!(probe::hue_hex(120.0, 1.0), "#00ff00");
    assert_eq!(probe::hue_hex(240.0, 1.0), "#0000ff");
    assert_eq!(DrawOp::parse(&probe::screen_ops("left", 3, 10)).len(), probe::screen_ops("left", 3, 10).len(), "every operation of the pattern is valid");
}

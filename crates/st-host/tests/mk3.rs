//! The Maschine MK3 area through the bridge, against `st-mk3`'s in-memory controller.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use st_host::areas::mk3::{Mk3Devices, Mk3Options};
use st_host::areas::services::ServiceOptions;
use st_host::testing::{TempDir, TestHost};
use st_host::{install, Host, HostOptions, NoDialogs};
use st_mk3::fake::{FakeHid, FakeScreens};

struct Fixture {
    host: TestHost,
    services: Host,
    hid: FakeHid,
    screens: FakeScreens,
}

/// A host with the controller only, made of fakes, on `support`.
fn start(support: &std::path::Path) -> Fixture {
    let (hid, screens) = (FakeHid::new(), FakeScreens::new());
    hid.plug();
    let mut services = None;
    let host = TestHost::start(|bridge| {
        let mut options = HostOptions::new(support.to_path_buf(), Arc::new(NoDialogs));
        options.autosave_every = None;
        options.audio = None;
        options.services = ServiceOptions { mk3: Some(Mk3Options { devices: Mk3Devices::Parts { hid: Arc::new(hid.clone()), screens: Arc::new(screens.clone()) } }), ..ServiceOptions::none() };
        services = Some(install(bridge, options).expect("install the host"));
    });
    host.request("ready", json!({})).expect("the handshake");
    Fixture { host, services: services.unwrap(), hid, screens }
}

fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn status(host: &TestHost) -> Value {
    host.request("mk3.status", json!({})).unwrap()
}

#[test]
fn the_controller_is_off_by_default_and_driven_through_the_bridge_once_enabled() {
    let folder = TempDir::new("mk3");
    let support = folder.path().join("support");
    {
        let fixture = start(&support);
        let host = &fixture.host;
        let ready = host.request("ready", json!({})).unwrap();
        for command in ["mk3.configure", "mk3.connect", "mk3.disconnect", "mk3.status", "mk3.leds", "mk3.screen"] {
            assert!(ready["commands"].as_array().unwrap().contains(&json!(command)), "{command} is implemented");
        }

        // Off by default: the service started at launch and did not touch the controller.
        std::thread::sleep(Duration::from_millis(50));
        let off = status(host);
        assert_eq!((off["enabled"].clone(), off["connected"].clone(), off["running"].clone()), (json!(false), json!(false), json!(false)), "{off}");
        assert_eq!(fixture.hid.opens(), 0, "the controller is not touched while support is off");
        assert_eq!(host.request("mk3.connect", json!({})).unwrap_err(), "Maschine MK3 support is in development and off. Turn it on in Settings first.");
        assert_eq!(host.request("mk3.configure", json!({})).unwrap_err(), "Choose whether Maschine MK3 support is on.");

        // Turned on in Settings: connected in shared mode, the choice stored in settings.json.
        let on = host.request("mk3.configure", json!({ "enabled": true })).unwrap();
        assert_eq!((on["enabled"].clone(), on["running"].clone(), on["exclusive"].clone()), (json!(true), json!(true), json!(false)), "{on}");
        wait_for("the controller", || status(host)["connected"] == true);
        assert_eq!(fixture.services.settings.get_raw("mk3"), Some(json!({ "enabled": true })));
        assert!(host.wait_for_event("mk3.status", Duration::from_secs(5)).is_some(), "status changes are events");

        // Input: a report from the controller becomes mk3.input events.
        let mut first = vec![0u8; 42];
        first[0] = 0x01;
        fixture.hid.send(&first);
        let mut play = first.clone();
        play[6] = 0x20;
        fixture.hid.send(&play);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let event = host.wait_for_event("mk3.input", deadline.saturating_duration_since(Instant::now())).expect("the Play button arrives as an event");
            if event == json!({ "type": "button", "name": "play", "pressed": true }) {
                break;
            }
        }

        // Output: LEDs through mk3.leds; the screens only once they are taken over.
        assert_eq!(host.request("mk3.leds", json!({ "buttons": { "play": 1.0 } })), Ok(json!({ "ok": true })));
        wait_for("an LED report", || fixture.hid.last_report(0x80).is_some_and(|report| report[1..].iter().any(|byte| *byte != 0)));
        assert_eq!(host.request("mk3.screen", json!({ "left": [{ "op": "clear", "color": "#ff0000" }] })), Ok(json!({ "screens": false })));
        let exclusive = host.request("mk3.connect", json!({ "exclusive": true })).unwrap();
        assert_eq!((exclusive["exclusive"].clone(), exclusive["screens"].clone()), (json!(true), json!(true)), "{exclusive}");
        assert_eq!(host.request("mk3.screen", json!({ "left": [{ "op": "clear", "color": "#ff0000" }] })), Ok(json!({ "screens": true })));
        wait_for("a frame on the screen", || !fixture.screens.packets().is_empty());
        assert!(status(host)["diagnostics"].is_null());
        assert!(host.request("mk3.status", json!({ "diagnostics": true })).unwrap()["diagnostics"]["reports"].is_object());

        // Quitting leaves the controller dark and released.
        fixture.services.services.shutdown();
        assert!(!fixture.screens.is_open(), "the display interface is released");
        assert!(fixture.hid.last_report(0x80).unwrap()[1..].iter().all(|byte| *byte == 0), "button LEDs are off");
        assert!(fixture.hid.last_report(0x81).unwrap()[1..].iter().all(|byte| *byte == 0), "pad LEDs are off");
        let after = status(host);
        assert_eq!((after["connected"].clone(), after["running"].clone()), (json!(false), json!(false)), "{after}");
    }

    // The next launch finds support on and connects by itself.
    let fixture = start(&support);
    wait_for("the controller at launch", || status(&fixture.host)["connected"] == true);
    assert_eq!(status(&fixture.host)["enabled"], true);
    let off = fixture.host.request("mk3.disconnect", json!({})).unwrap();
    assert_eq!((off["connected"].clone(), off["enabled"].clone()), (json!(false), json!(true)));
    fixture.services.services.shutdown();
}

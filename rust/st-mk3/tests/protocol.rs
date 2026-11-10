//! The Maschine MK3 protocol on synthetic reports, no device: ported from
//! tests/MaschineMK3ProtocolTests.swift.

use std::sync::Arc;

use serde_json::{json, Value};
use st_mk3::protocol::{
    hardware_index, led, normalize, normalize_unidentified, pad_number, InputEvent, InputParser, LedState, PadPhase, Rgb, BUTTON_BITS,
};
use st_mk3::screen::encode::{self, PixelFormat, Region};
use st_mk3::screen::font::Fonts;
use st_mk3::screen::ops::DrawOp;
use st_mk3::screen::render::Renderer;
use st_mk3::screen::{HEIGHT, PIXEL_COUNT, WIDTH};

fn button_report(edit: impl FnOnce(&mut [u8])) -> Vec<u8> {
    let mut report = vec![0u8; 42];
    report[0] = 0x01;
    edit(&mut report);
    report
}

fn set_knob(report: &mut [u8], index: usize, value: u16) {
    report[12 + index * 2] = value as u8;
    report[13 + index * 2] = (value >> 8) as u8;
}

fn pad_set(tuples: &[(u8, u8, u8)]) -> Vec<u8> {
    let mut set = vec![0u8; 64];
    set[0] = 0x02;
    for (index, tuple) in tuples.iter().enumerate() {
        set[1 + index * 3] = tuple.0;
        set[2 + index * 3] = tuple.1;
        set[3 + index * 3] = tuple.2;
    }
    set
}

fn pads(events: &[InputEvent]) -> Vec<(u8, PadPhase)> {
    events.iter().filter_map(|event| if let InputEvent::Pad { pad, phase, .. } = event { Some((*pad, *phase)) } else { None }).collect()
}

fn button(name: &'static str, pressed: bool) -> InputEvent {
    InputEvent::Button { name, pressed }
}

fn touch(target: &str, touched: bool) -> InputEvent {
    InputEvent::Touch { target: target.into(), touched }
}

fn is_encoder(event: &InputEvent) -> bool {
    matches!(event, InputEvent::Encoder { .. })
}

#[test]
fn disconnect_releases_pedal_and_all_touch_sensors_and_reconnect_takes_new_baselines() {
    let mut parser = InputParser::new();
    parser.parse(
        &button_report(|r| {
            r[1] = 0x02;
            r[9] = 0x80;
            r[10] = 0x81;
            r[31] = 0x02;
        }),
        0.0,
    );
    let released = parser.release_all();
    for event in [InputEvent::Pedal { down: false }, touch("main", false), touch("encoder1", false), touch("encoder8", false), touch("strip", false)] {
        assert!(released.contains(&event), "disconnect releases {event:?}");
    }
    assert!(parser.release_all().is_empty(), "the release is idempotent");
    let fresh = parser.parse(
        &button_report(|r| {
            r[11] = 11;
            set_knob(r, 0, 800);
        }),
        1.0,
    );
    assert!(!fresh.iter().any(|event| is_encoder(event) || matches!(event, InputEvent::Main { .. })), "reconnect establishes fresh encoder baselines");
}

#[test]
fn buttons_edges_and_touch_sensors() {
    let mut parser = InputParser::new();
    let first = parser.parse(&button_report(|_| {}), 0.0);
    assert!(first.len() == 3 && first.iter().all(|event| matches!(event, InputEvent::Knob { .. })), "an idle first report only reports the analog knobs: {first:?}");
    let mut events = parser.parse(
        &button_report(|r| {
            r[6] = 0x20;
            r[1] = 0x40;
        }),
        0.01,
    );
    assert!(events.contains(&button("play", true)) && events.contains(&button("shift", true)));
    events = parser.parse(&button_report(|r| r[1] = 0x40), 0.02);
    assert_eq!(events, [button("play", false)], "play up only");
    events = parser.parse(
        &button_report(|r| {
            r[1] = 0x80 | 0x01;
            r[2] = 0x81;
            r[8] = 0x01;
            r[9] = 0x01;
            r[7] = 0x04;
        }),
        0.03,
    );
    assert!(events.contains(&button("shift", false)));
    for name in ["display8", "encoderPush", "groupA", "groupH", "channel", "display1", "arrowRight"] {
        assert!(events.contains(&button(name, true)), "{name} down");
    }
    events = parser.parse(
        &button_report(|r| {
            r[9] = 0x80;
            r[10] = 0x81;
        }),
        0.04,
    );
    assert!(events.contains(&touch("main", true)));
    assert!(events.contains(&touch("encoder1", true)), "encoder 1 is bit 7");
    assert!(events.contains(&touch("encoder8", true)), "encoder 8 is bit 0");

    let contract = "channel plugin arranger mixer browser sampling arrowLeft arrowRight file settings auto macro display1 display2 display3 display4 display5 display6 display7 display8 notes volume swing tempo noteRepeat lock pitch mod perform padMode keyboard chords step fixedVel scene pattern events variation duplicate select solo mute groupA groupB groupC groupD groupE groupF groupG groupH restart erase tap follow play rec stop shift encoderPush encoderUp encoderDown encoderLeft encoderRight";
    let decoded: std::collections::BTreeSet<&str> = BUTTON_BITS.iter().map(|(_, _, name)| *name).collect();
    assert_eq!(decoded, contract.split(' ').collect(), "all 63 contract buttons are decoded");
    assert_eq!(BUTTON_BITS.len(), 63);
    let locations: std::collections::BTreeSet<(usize, u8)> = BUTTON_BITS.iter().map(|(byte, bit, _)| (*byte, *bit)).collect();
    assert_eq!(locations.len(), 63, "no two buttons share a bit");
}

#[test]
fn main_encoder_and_knob_wrap_around_deltas() {
    let mut parser = InputParser::new();
    parser.parse(
        &button_report(|r| {
            r[11] = 14;
            r[10] = 0xFF;
            set_knob(r, 0, 990);
            set_knob(r, 1, 500);
        }),
        0.0,
    );
    let mut events = parser.parse(
        &button_report(|r| {
            r[11] = 1;
            r[10] = 0xFF;
            set_knob(r, 0, 10);
            set_knob(r, 1, 480);
        }),
        0.01,
    );
    assert!(events.contains(&InputEvent::Main { delta: 3 }), "the main encoder wraps 14 → 1 as +3: {events:?}");
    assert!(events.contains(&InputEvent::Encoder { index: 0, delta: 20.0 / 1000.0 }), "the knob wraps 990 → 10 as +20 counts");
    assert!(events.contains(&InputEvent::Encoder { index: 1, delta: -20.0 / 1000.0 }));
    events = parser.parse(
        &button_report(|r| {
            r[11] = 15;
            r[10] = 0xFF;
            set_knob(r, 0, 995);
            set_knob(r, 1, 480);
        }),
        0.02,
    );
    assert!(events.contains(&InputEvent::Main { delta: -2 }), "the main encoder wraps 1 → 15 as -2");
    assert!(events.contains(&InputEvent::Encoder { index: 0, delta: -15.0 / 1000.0 }), "the knob wraps 10 → 995 as -15 counts");
    // Untouched jitter is ignored but accumulates.
    events = parser.parse(
        &button_report(|r| {
            r[11] = 15;
            set_knob(r, 0, 996);
            set_knob(r, 1, 480);
        }),
        0.03,
    );
    assert!(!events.iter().any(is_encoder), "one count of untouched jitter is ignored");
    events = parser.parse(
        &button_report(|r| {
            r[11] = 15;
            set_knob(r, 0, 998);
            set_knob(r, 1, 480);
        }),
        0.04,
    );
    assert!(events.contains(&InputEvent::Encoder { index: 0, delta: 3.0 / 1000.0 }), "the accumulated untouched turn is reported");
}

#[test]
fn touch_strip_analog_knobs_and_pedal() {
    let mut parser = InputParser::new();
    parser.parse(&button_report(|_| {}), 0.0);
    let mut events = parser.parse(
        &button_report(|r| {
            r[30] = 0x00;
            r[31] = 0x02;
            r[40] = 0xFF;
            r[41] = 0x0F;
        }),
        0.01,
    );
    assert!(events.contains(&touch("strip", true)));
    assert!(events.contains(&InputEvent::Strip { value: 0.5 }), "512 of 1024");
    assert!(events.contains(&InputEvent::Knob { name: "master", value: 1.0 }));
    events = parser.parse(
        &button_report(|r| {
            r[40] = 0xFF;
            r[41] = 0x0F;
            r[1] = 0x02;
        }),
        0.02,
    );
    assert!(events.contains(&touch("strip", false)));
    assert!(events.contains(&InputEvent::Pedal { down: true }));
}

#[test]
fn pad_hit_pressure_hysteresis_and_release() {
    let mut parser = InputParser::new();
    // Hardware index 12 (bottom left) is pad 1; index 3 (top right) is pad 16.
    assert!(pad_number(12) == 1 && pad_number(3) == 16 && pad_number(0) == 13);
    for pad in 1..=16 {
        assert_eq!(pad_number(hardware_index(pad)), pad, "the pad map round-trips");
    }
    let mut events = parser.parse(&pad_set(&[(12, 0x1A, 0x00)]), 0.0);
    match events.first() {
        Some(InputEvent::Pad { pad: 1, phase: PadPhase::Down, pressure, velocity: Some(velocity) }) => {
            assert!((pressure - 2560.0 / 4095.0).abs() < 0.001 && *velocity > 90 && *velocity <= 127, "velocity from the strike's pressure");
        }
        other => panic!("a hit must press pad 1: {other:?}"),
    }
    events = parser.parse(&pad_set(&[(12, 0x4B, 0x00), (12, 0x4B, 0x80)]), 0.01);
    assert_eq!(pads(&events), [(1, PadPhase::Pressure)], "pressure is coalesced per report");
    events = parser.parse(&pad_set(&[(12, 0x40, 0x10)]), 0.02);
    assert!(events.is_empty(), "a release is suppressed during the 30 ms rebound window");
    events = parser.parse(&pad_set(&[(12, 0x40, 0x10)]), 0.05);
    assert_eq!(pads(&events), [(1, PadPhase::Up)], "aftertouch below 16 releases after the window");
    // Switch-on hysteresis: below 32 does not press.
    events = parser.parse(&pad_set(&[(5, 0x00, 20)]), 0.1);
    assert!(events.is_empty(), "the dead band does not press");
    events = parser.parse(&pad_set(&[(5, 0x00, 40)]), 0.11);
    assert_eq!(pads(&events), [(pad_number(5), PadPhase::Down)], "switch-on at 32 or more presses");
    events = parser.parse(&pad_set(&[(5, 0x30, 0x00)]), 0.12);
    assert_eq!(pads(&events), [(pad_number(5), PadPhase::Up)], "an explicit off releases at once");
}

#[test]
fn index_zero_low_pressure_edge_case_and_terminator() {
    // A low-pressure tuple of hardware index 0 (first data byte zero) must not end the list.
    let mut parser = InputParser::new();
    let events = parser.parse(&pad_set(&[(0, 0x00, 0x30), (7, 0x10, 0x90)]), 0.0);
    assert_eq!(pads(&events), [(13, PadPhase::Down), (12, PadPhase::Down)], "{events:?}");
    let mut tail = InputParser::new();
    assert!(tail.parse(&pad_set(&[(0, 0x00, 0x00), (7, 0x10, 0x90)]), 0.0).is_empty(), "an all-zero tuple ends the set");
}

#[test]
fn double_pumped_pad_sets() {
    // Set A still holds the pad, set B releases it; both are decoded in order.
    let mut parser = InputParser::new();
    parser.parse(&pad_set(&[(9, 0x10, 0x80)]), 0.0);
    let packet = [pad_set(&[(9, 0x41, 0x75)]), pad_set(&[(9, 0x30, 0x00)])].concat();
    let events = parser.parse(&packet, 0.2);
    assert_eq!(pads(&events).last(), Some(&(pad_number(9), PadPhase::Up)), "set B's release is honoured: {events:?}");
    assert!(!parser.pads()[9].pressed);
    // A second set that reports the hit again does not trigger twice.
    let mut again = InputParser::new();
    let twice = again.parse(&[pad_set(&[(2, 0x10, 0x80)]), pad_set(&[(2, 0x10, 0x82)])].concat(), 0.0);
    assert_eq!(pads(&twice).iter().filter(|(_, phase)| *phase == PadPhase::Down).count(), 1);
    let released = again.release_all();
    assert_eq!(pads(&released), [(pad_number(2), PadPhase::Up)], "release_all lifts held pads");
}

#[test]
fn report_id_normalisation() {
    assert_eq!(normalize(&[0x00, 0x01], 2), [0x02, 0x00, 0x01], "a missing report id is prepended");
    let full = button_report(|_| {});
    assert_eq!(normalize(&full, 1), full, "a report with its id is kept");
    assert_eq!(normalize(&full[1..], 1), full, "a 41-byte report gets its id back");
    // The first pad index can equal the report id: the size, not that byte, tells whether the id
    // is missing, also when two pad sets arrive together.
    let pumped = [pad_set(&[(2, 0x10, 0x80)]), pad_set(&[(2, 0x30, 0x00)])].concat();
    let normalized = normalize(&pumped[1..], 2);
    assert_eq!(normalized, pumped, "a double-pumped report gets its id back before hardware pad index 2");
    assert_eq!(normalize(&pumped, 2), pumped, "and keeps an id it has");
    let mut parser = InputParser::new();
    assert_eq!(pads(&parser.parse(&normalized, 0.0)), [(15, PadPhase::Down), (15, PadPhase::Up)], "both the press and the release of pad 15 survive");
    assert_eq!(normalize(&[1, 2, 3], 0), [1, 2, 3], "no report id: nothing to restore");
    // A layer that does not say which report it delivers: the size tells.
    assert_eq!(normalize_unidentified(&full[1..]), full);
    assert_eq!(normalize_unidentified(&full), full);
    assert_eq!(normalize_unidentified(&pumped[1..]), pumped);
    assert_eq!(normalize_unidentified(&pumped), pumped);
    assert_eq!(normalize_unidentified(&pad_set(&[(2, 0x10, 0x80)])[1..]), pad_set(&[(2, 0x10, 0x80)]));
    assert_eq!(normalize_unidentified(&[0x20, 1, 2]), [0x20, 1, 2], "other reports pass unchanged");
}

fn rgb(hex: &str) -> Option<Rgb> {
    Rgb::from_hex(hex)
}

#[test]
fn led_reports_and_palette_mapping() {
    assert_eq!(led::indexed(rgb("#ff0000")), 1 << 2 | 3, "red full");
    assert_eq!(led::indexed(rgb("#00ff00")), 7 << 2 | 3, "green full");
    assert_eq!(led::indexed(rgb("#0000ff")), 11 << 2 | 3, "blue full");
    assert_eq!(led::indexed(rgb("#ffff00")), 5 << 2 | 3, "yellow");
    assert_eq!(led::indexed(rgb("#00ffff")), 9 << 2 | 3, "cyan");
    assert_eq!(led::indexed(rgb("#ff00ff")), 13 << 2 | 3, "magenta");
    assert_eq!(led::indexed(rgb("#ffffff")), 17 << 2 | 3, "white");
    assert_eq!(led::indexed(rgb("#400000")), 1 << 2, "dim red");
    assert!(led::indexed(rgb("#000000")) == 0 && led::indexed(None) == 0, "off");
    assert_eq!(rgb("#f00"), rgb("#ff0000"));
    assert_eq!(rgb("#ff000080"), rgb("#ff0000"), "alpha is ignored for LEDs");
    assert!(rgb("red").is_none() && rgb("#12345").is_none());
    // The interface's muted default track palette, at full value: every colour keeps a hue (not
    // white), and the eight groups are not all the same palette entry.
    let palette: Vec<u8> = ["#bba374", "#a0adb1", "#b0af89", "#c58b69", "#b79583", "#9da688", "#94a39a", "#aaa0ad"]
        .iter()
        .map(|hex| {
            let colour = rgb(hex).unwrap();
            let scale = 255.0 / f64::from(colour.r.max(colour.g).max(colour.b));
            let full = |channel: u8| (f64::from(channel) * scale).round() as u8;
            led::indexed(Some(Rgb { r: full(colour.r), g: full(colour.g), b: full(colour.b) }))
        })
        .collect();
    assert!(palette.iter().all(|byte| byte >> 2 != 17 && byte & 3 == 3), "track colours light in their hue at full intensity: {palette:?}");
    assert!(palette.iter().map(|byte| byte >> 2).collect::<std::collections::BTreeSet<_>>().len() >= 5, "track colours stay distinguishable: {palette:?}");
    assert_eq!(led::indexed(rgb("#0f0f0f")), 17 << 2, "a 6 % white marker lights dimly");
    assert_eq!(led::indexed(rgb("#f4f4f2")) >> 2, 17, "near-white stays white");
    assert!(led::single(0.0) == 0 && led::single(1.0) == 0x7F && led::single(0.2) & 3 == 0 && led::single(0.6) & 3 == 2, "single levels");

    let mut leds = LedState::new();
    let mut pads_payload: Vec<Value> = vec![json!("#00ff00")];
    pads_payload.extend(std::iter::repeat_n(Value::Null, 14));
    pads_payload.push(json!("#0000ff"));
    leds.apply(&json!({
        "buttons": {"play": 1, "channel": 0.5, "bogus": 1},
        "groups": ["#ff0000", null],
        "pads": pads_payload,
        "strip": [1, 0, "#ff0000"],
        "nav": {"up": "#ffffff"},
    }));
    let (mut buttons, mut pad_bytes) = (leds.button_report(), leds.pad_report());
    assert!(buttons.len() == 63 && buttons[0] == 0x80 && pad_bytes.len() == 42 && pad_bytes[0] == 0x81, "report sizes");
    assert!(buttons[42] == 0x7F && buttons[1] == led::single(0.5), "the play and channel LED offsets");
    assert!(buttons[30] == (1 << 2 | 3) && buttons[31] == 0 && buttons[59] == (17 << 2 | 3), "group A and nav up");
    assert_eq!(pad_bytes[26 + 12], 7 << 2 | 3, "pad 1 is hardware index 12");
    assert_eq!(pad_bytes[26 + 3], 11 << 2 | 3, "pad 16 is hardware index 3");
    assert!(pad_bytes[1] == (17 << 2 | 3) && pad_bytes[2] == 0 && pad_bytes[3] == (1 << 2 | 3), "strip: brightness, off, colour");
    leds.apply(&json!({"buttons": {"play": 0}}));
    buttons = leds.button_report();
    pad_bytes = leds.pad_report();
    assert!(buttons[42] == 0 && buttons[1] == led::single(0.5) && pad_bytes[26 + 12] == (7 << 2 | 3), "a partial update keeps the rest");
    assert_eq!(LedState::new().button_report()[1..], [0u8; 62], "a fresh state is all off");
    assert_eq!(st_mk3::protocol::led_button_names().len(), 50, "every button with an LED");
}

#[test]
fn input_events_have_the_contracts_payloads() {
    let payload = |event: InputEvent| Value::Object(event.payload());
    assert_eq!(payload(button("play", true)), json!({"type": "button", "name": "play", "pressed": true}));
    assert_eq!(
        payload(InputEvent::Pad { pad: 3, phase: PadPhase::Down, pressure: 0.62515, velocity: Some(96) }),
        json!({"type": "pad", "pad": 3, "phase": "down", "pressure": 0.625, "velocity": 96})
    );
    assert_eq!(payload(InputEvent::Pad { pad: 3, phase: PadPhase::Up, pressure: 0.0, velocity: None }), json!({"type": "pad", "pad": 3, "phase": "up", "pressure": 0.0}));
    assert_eq!(payload(InputEvent::Encoder { index: 7, delta: -0.01234567 }), json!({"type": "encoder", "index": 7, "delta": -0.0123}));
    assert_eq!(payload(InputEvent::Main { delta: -2 }), json!({"type": "main", "delta": -2}));
    assert_eq!(payload(touch("encoder1", true)), json!({"type": "touch", "target": "encoder1", "touched": true}));
    assert_eq!(payload(InputEvent::Strip { value: 0.33349 }), json!({"type": "strip", "value": 0.333}));
    assert_eq!(payload(InputEvent::Knob { name: "mic", value: 1.0 }), json!({"type": "knob", "name": "mic", "value": 1.0}));
    assert_eq!(payload(InputEvent::Pedal { down: true }), json!({"type": "pedal", "down": true}));
}

fn ops(list: Value) -> Vec<DrawOp> {
    DrawOp::parse(list.as_array().unwrap())
}

#[test]
fn screen_rendering_diff_regions_and_packet_encoding() {
    let mut renderer = Renderer::new(Arc::new(Fonts::builtin()));
    renderer.render(&ops(json!([{"op": "clear", "color": "#000000"}, {"op": "rect", "x": 10, "y": 20, "w": 30, "h": 10, "color": "#ff0000"}])));
    let frame = renderer.framebuffer(PixelFormat::Rgb565);
    assert!(frame[25 * 480 + 20] == 0xF800 && frame[0] == 0, "a red rectangle with the origin at the top left (RGB565)");
    assert_eq!(renderer.framebuffer(PixelFormat::Bgr565)[25 * 480 + 20], 0x001F, "BGR565 swaps the channels");
    assert_eq!(frame.iter().filter(|pixel| **pixel != 0).count(), 300, "exactly the rectangle's pixels");
    // The full first frame round-trips through the packet decoder.
    let full = encode::packet(1, None, &frame).expect("a full packet");
    assert_eq!(full[..16], [0x84, 0, 1, 0x60, 0, 0, 0, 0, 0, 0, 0, 0, 0x01, 0xE0, 0x01, 0x10], "the full header");
    assert_eq!(full[full.len() - 8..], [0x03, 0, 0, 0, 0x40, 0, 0, 0], "the footer");
    assert!(full.len() < 2000, "a mostly black frame compresses with repeat commands: {}", full.len());
    let mut decoded = vec![0x1234u16; PIXEL_COUNT];
    assert!(encode::apply(&full, &mut decoded) && decoded == frame, "the full packet decodes to the frame");
    assert!(encode::packet(0, Some(&frame), &frame).is_none(), "an unchanged frame sends nothing");

    // A partial update: text in a corner.
    renderer.render(&ops(json!([
        {"op": "clear", "color": "#000000"},
        {"op": "rect", "x": 10, "y": 20, "w": 30, "h": 10, "color": "#ff0000"},
        {"op": "text", "x": 301, "y": 201, "text": "SuperTracker", "size": 18, "color": "#ffffff", "weight": "bold"},
    ])));
    let next = renderer.framebuffer(PixelFormat::Rgb565);
    let region: Region = encode::dirty_region(Some(&frame), &next).expect("text changes pixels");
    assert!(region.x % 4 == 0 && region.y % 2 == 0 && region.width % 4 == 0 && region.height % 2 == 0, "4 × 2 alignment: {region:?}");
    assert!(region.x >= 296 && region.y >= 200 && region.x + region.width <= 480, "the region bounds the text: {region:?}");
    let partial = encode::packet(0, Some(&frame), &next).expect("a partial packet");
    let mut screen = frame.clone();
    assert!(encode::apply(&partial, &mut screen) && screen == next, "the partial packet reproduces the frame");
    assert!(partial.len() < full.len() + 40_000 && partial[2] == 0, "the partial packet is small and for the left screen");

    // Every operation renders; the arc and the bar draw their colour.
    renderer.render(&ops(json!([
        {"op": "clear", "color": "#101010"},
        {"op": "frame", "x": 0, "y": 0, "w": 480, "h": 272, "color": "#ffffff", "width": 2},
        {"op": "line", "x1": 0, "y1": 136, "x2": 480, "y2": 136, "color": "#00ff00"},
        {"op": "bar", "x": 20, "y": 100, "w": 100, "h": 10, "value": 0.5, "color": "#00ff00", "background": "#333333"},
        {"op": "arc", "x": 240, "y": 200, "r": 30, "value": 0.75, "color": "#ffaa00", "background": "#333333", "width": 4},
        {"op": "text", "x": 240, "y": 10, "text": "Centered", "size": 14, "color": "#ffffff", "align": "center", "mono": true},
        {"op": "text", "x": 20, "y": 40, "text": "A very long label that must be truncated", "size": 14, "color": "#ffffff", "width": 80},
        {"op": "nonsense"},
    ])));
    let drawn = renderer.framebuffer(PixelFormat::Rgb565);
    assert!(drawn[105 * 480 + 30] == 0x07E0 && drawn[105 * 480 + 110] != 0x07E0, "the bar fills half");
    assert_eq!(drawn[0], 0xFFFF, "the frame's border");
    assert_eq!(drawn[5 * 480 + 5], PixelFormat::Rgb565.pack(0x10, 0x10, 0x10), "and its inside is left alone");
    let ring = drawn.iter().filter(|pixel| *pixel >> 11 > 25 && (*pixel >> 5 & 0x3F) > 30 && (*pixel & 0x1F) < 6).count();
    assert!(ring > 50, "the arc draws its value portion: {ring}");
    let outside = (40..60).flat_map(|y| (104..460).map(move |x| y * 480 + x)).filter(|index| drawn[*index] > 0x8000).count();
    assert_eq!(outside, 0, "truncated text stays inside its width");
    let inside = (40..60).flat_map(|y| (20..101).map(move |x| y * 480 + x)).filter(|index| drawn[*index] > 0x8000).count();
    assert!(inside > 40, "and is drawn: {inside}");
    assert_eq!(renderer.rgb().len(), WIDTH * HEIGHT * 3);
}

#[test]
fn shapes_are_placed_and_anti_aliased_as_the_canvas_would() {
    let mut renderer = Renderer::new(Arc::new(Fonts::builtin()));
    let at = |frame: &[u16], x: usize, y: usize| PixelFormat::Rgb565.unpack(frame[y * 480 + x]);
    renderer.render(&ops(json!([
        // Half-covered pixels at fractional edges.
        {"op": "rect", "x": 10.5, "y": 10, "w": 10, "h": 10, "color": "#ffffff"},
        // A one-pixel horizontal line on a pixel boundary covers two rows by half.
        {"op": "line", "x1": 100, "y1": 50, "x2": 200, "y2": 50, "color": "#ffffff", "width": 1},
        // The same line through pixel centres is one crisp row.
        {"op": "line", "x1": 100, "y1": 60.5, "x2": 200, "y2": 60.5, "color": "#ffffff", "width": 1},
        // A rounded rectangle: corners cut, centre full.
        {"op": "rect", "x": 300, "y": 10, "w": 40, "h": 40, "color": "#ffffff", "radius": 12},
        // Translucent colour over a background.
        {"op": "rect", "x": 400, "y": 10, "w": 20, "h": 20, "color": "#ff0000"},
        {"op": "rect", "x": 400, "y": 10, "w": 20, "h": 20, "color": "#0000ff80"},
        // A vertical bar fills from the bottom.
        {"op": "bar", "x": 440, "y": 100, "w": 10, "h": 100, "value": 0.25, "color": "#00ff00"},
        // A diagonal line.
        {"op": "line", "x1": 20, "y1": 200, "x2": 60, "y2": 240, "color": "#ffffff", "width": 3},
        // A full ring: the gap of the 270° sweep is at the bottom.
        {"op": "arc", "x": 240, "y": 200, "r": 30, "value": 1, "color": "#ffffff", "width": 4},
        // Right-aligned and centred text without a width, around their anchor.
        {"op": "text", "x": 200, "y": 100, "text": "RIGHT", "size": 10, "color": "#ffffff", "align": "right"},
        {"op": "text", "x": 200, "y": 120, "text": "MID!", "size": 10, "color": "#ffffff", "align": "center"},
    ])));
    let frame = renderer.framebuffer(PixelFormat::Rgb565);
    let half = |value: u8| (115..=140).contains(&value);
    assert!(half(at(&frame, 10, 15).0) && at(&frame, 11, 15).0 == 255 && half(at(&frame, 20, 15).0) && at(&frame, 21, 15).0 == 0);
    assert!(half(at(&frame, 150, 49).0) && half(at(&frame, 150, 50).0) && at(&frame, 150, 48).0 == 0 && at(&frame, 150, 51).0 == 0);
    assert!(at(&frame, 150, 60).0 == 255 && at(&frame, 150, 59).0 == 0 && at(&frame, 150, 61).0 == 0);
    assert!(at(&frame, 300, 10).0 == 0 && at(&frame, 339, 49).0 == 0 && at(&frame, 320, 30).0 == 255 && at(&frame, 320, 10).0 == 255 && at(&frame, 300, 30).0 == 255);
    let mixed = at(&frame, 410, 20);
    assert!(half(mixed.0) && half(mixed.2) && mixed.1 == 0, "half blue over red: {mixed:?}");
    assert!(at(&frame, 445, 190).1 == 255 && at(&frame, 445, 170).1 == 0, "the vertical bar fills its lowest quarter");
    assert!(at(&frame, 40, 220).0 == 255 && at(&frame, 60, 200).0 == 0 && at(&frame, 20, 240).0 == 0, "the diagonal runs from top left to bottom right");
    assert!(at(&frame, 210, 200).0 == 255 && at(&frame, 270, 200).0 == 255 && at(&frame, 240, 170).0 == 255, "left, right and top of the ring");
    assert_eq!(at(&frame, 240, 230).0, 0, "the bottom of the ring is open");
    assert_eq!(at(&frame, 240, 200).0, 0, "the ring is hollow");
    let lit = |x0: usize, x1: usize, y0: usize, y1: usize| (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|(x, y)| at(&frame, *x, *y).0 > 128).count();
    assert!(lit(170, 200, 100, 112) > 20 && lit(200, 240, 100, 112) == 0, "right-aligned text ends at its anchor");
    assert!(lit(188, 200, 120, 132) > 5 && lit(200, 212, 120, 132) > 5 && lit(213, 240, 120, 132) == 0 && lit(160, 187, 120, 132) == 0, "centred text straddles its anchor");
    // A translucent clear shows over black; everything outside the screen is ignored.
    renderer.render(&ops(json!([{"op": "clear", "color": "#ffffff80"}, {"op": "rect", "x": -50, "y": -50, "w": 60, "h": 60, "color": "#ff0000"}, {"op": "rect", "x": 470, "y": 262, "w": 500, "h": 500, "color": "#00ff00"}, {"op": "line", "x1": -1e9, "y1": 150, "x2": 1e9, "y2": 150.5, "color": "#0000ff"}, {"op": "arc", "x": 0, "y": 0, "r": 1e7, "value": 1, "color": "#ffffff"}])));
    let frame = renderer.framebuffer(PixelFormat::Rgb565);
    assert!(half(at(&frame, 100, 100).0) && at(&frame, 5, 5) == (255, 0, 0) && at(&frame, 479, 271) == (0, 255, 0));
}

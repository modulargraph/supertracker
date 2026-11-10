//! The Maschine MK3's HID protocol: input reports to events, LED state to output reports. Pure
//! data transformations, no device access.
//!
//! Sources (protocol facts only): `docs/hardware/MaschineMK3-*.md` and the device's own HID report
//! descriptor:
//!
//! - `0x01` in, 42 bytes: 80 one-bit buttons (bytes 1–10), the main encoder's 4-bit position (byte
//!   11, low nibble), 8 × 16-bit knobs with the logical range 0…999 (bytes 12–27), the touch strip
//!   (bytes 28–33, position at 30–31), and three 12-bit analog knobs (bytes 36–41: mic, headphone,
//!   master).
//! - `0x02` in, 64 bytes: pad tuples; a delivery may carry several such sets.
//! - `0x80` out, 63 bytes: 62 button LEDs. `0x81` out, 42 bytes: 25 strip and 16 pad LEDs.
//!
//! Everything that is not confirmed on real hardware is a named constant or a field with a default,
//! so that it can be changed in one place.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

/// USB vendor id (Native Instruments).
pub const VENDOR_ID: u16 = 0x17CC;
/// USB product id (Maschine MK3).
pub const PRODUCT_ID: u16 = 0x1600;
/// Report id of the button, encoder and strip report.
pub const BUTTON_REPORT_ID: u8 = 0x01;
/// Report id of the pad report.
pub const PAD_REPORT_ID: u8 = 0x02;
/// Length of the button report, with its id.
pub const BUTTON_REPORT_LENGTH: usize = 42;
/// Length of one pad set, with its id.
pub const PAD_SET_LENGTH: usize = 64;
/// Report id of the button LED report.
pub const BUTTON_LED_REPORT_ID: u8 = 0x80;
/// Length of the button LED report, with its id.
pub const BUTTON_LED_REPORT_LENGTH: usize = 63;
/// Report id of the pad and strip LED report.
pub const PAD_LED_REPORT_ID: u8 = 0x81;
/// Length of the pad and strip LED report, with its id.
pub const PAD_LED_REPORT_LENGTH: usize = 42;
/// Knobs report 0…999: one revolution is 1000 counts.
pub const KNOB_MODULUS: i32 = 1000;
/// Counts that make an encoder delta of 1.0 ("a full sweep of a parameter").
pub const KNOB_COUNTS_PER_UNIT: f64 = 1000.0;
/// Analog knobs are 12-bit.
pub const ANALOG_FULL_SCALE: f64 = 4095.0;
/// The touch strip's position is normalized by this maximum.
pub const STRIP_FULL_SCALE: f64 = 1024.0;
/// Byte offset of the touch strip's position in the button report.
pub const STRIP_POSITION_OFFSET: usize = 30;

/// Phase of a pad event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadPhase {
    /// The pad was struck.
    Down,
    /// The pressure on a held pad changed.
    Pressure,
    /// The pad was released.
    Up,
}

impl PadPhase {
    /// The phase's name in `mk3.input` events.
    pub fn name(self) -> &'static str {
        match self {
            PadPhase::Down => "down",
            PadPhase::Pressure => "pressure",
            PadPhase::Up => "up",
        }
    }
}

/// One thing the player did, as the interface receives it in an `mk3.input` event.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    /// A button changed.
    Button {
        /// Contract name (`play`, `groupA`, `display1`, ...).
        name: &'static str,
        /// Down or up.
        pressed: bool,
    },
    /// A pad was struck, pressed harder or softer, or released.
    Pad {
        /// Pad number, 1 (bottom left) to 16 (top right).
        pad: u8,
        /// What happened.
        phase: PadPhase,
        /// Pressure, 0 to 1.
        pressure: f64,
        /// Velocity 1 to 127 of a strike.
        velocity: Option<u8>,
    },
    /// One of the eight encoders turned.
    Encoder {
        /// 0 to 7.
        index: u8,
        /// Turn as a fraction of a revolution; negative is counter-clockwise.
        delta: f64,
    },
    /// The main (4-D) encoder turned by this many detents.
    Main {
        /// Detents; negative is counter-clockwise.
        delta: i32,
    },
    /// A touch sensor changed: `main`, `encoder1` … `encoder8`, or `strip`.
    Touch {
        /// The sensor.
        target: String,
        /// Touched or let go.
        touched: bool,
    },
    /// The finger's position on the touch strip, 0 to 1.
    Strip {
        /// Position.
        value: f64,
    },
    /// An analog knob of the audio interface: `mic`, `headphone` or `master`, 0 to 1.
    Knob {
        /// The knob.
        name: &'static str,
        /// Position.
        value: f64,
    },
    /// The foot pedal.
    Pedal {
        /// Down or up.
        down: bool,
    },
}

fn rounded(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

impl InputEvent {
    /// The event as the payload of an `mk3.input` event.
    pub fn payload(&self) -> Map<String, Value> {
        let value = match self {
            InputEvent::Button { name, pressed } => json!({"type": "button", "name": name, "pressed": pressed}),
            InputEvent::Pad { pad, phase, pressure, velocity } => {
                let mut value = json!({"type": "pad", "pad": pad, "phase": phase.name(), "pressure": rounded(*pressure, 3)});
                if let Some(velocity) = velocity {
                    value["velocity"] = json!(velocity);
                }
                value
            }
            InputEvent::Encoder { index, delta } => json!({"type": "encoder", "index": index, "delta": rounded(*delta, 4)}),
            InputEvent::Main { delta } => json!({"type": "main", "delta": delta}),
            InputEvent::Touch { target, touched } => json!({"type": "touch", "target": target, "touched": touched}),
            InputEvent::Strip { value } => json!({"type": "strip", "value": rounded(*value, 3)}),
            InputEvent::Knob { name, value } => json!({"type": "knob", "name": name, "value": rounded(*value, 3)}),
            InputEvent::Pedal { down } => json!({"type": "pedal", "down": down}),
        };
        match value {
            Value::Object(payload) => payload,
            _ => unreachable!("every payload is an object"),
        }
    }
}

/// Every button of the button report: `(byte, bit, contract name)`. Byte 1 bit 1 is the pedal and
/// byte 5 bit 0 "microphone connected"; neither is a button.
pub const BUTTON_BITS: [(usize, u8, &str); 63] = [
    (1, 0, "encoderPush"),
    (1, 2, "encoderUp"),
    (1, 3, "encoderRight"),
    (1, 4, "encoderDown"),
    (1, 5, "encoderLeft"),
    (1, 6, "shift"),
    (1, 7, "display8"),
    (3, 0, "notes"),
    (3, 1, "volume"),
    (3, 2, "swing"),
    (3, 3, "tempo"),
    (3, 4, "noteRepeat"),
    (3, 5, "lock"),
    (4, 0, "padMode"),
    (4, 1, "keyboard"),
    (4, 2, "chords"),
    (4, 3, "step"),
    (4, 4, "fixedVel"),
    (4, 5, "scene"),
    (4, 6, "pattern"),
    (4, 7, "events"),
    (5, 1, "variation"),
    (5, 2, "duplicate"),
    (5, 3, "select"),
    (5, 4, "solo"),
    (5, 5, "mute"),
    (5, 6, "pitch"),
    (5, 7, "mod"),
    (6, 0, "perform"),
    (6, 1, "restart"),
    (6, 2, "erase"),
    (6, 3, "tap"),
    (6, 4, "follow"),
    (6, 5, "play"),
    (6, 6, "rec"),
    (6, 7, "stop"),
    (7, 0, "macro"),
    (7, 1, "settings"),
    (7, 2, "arrowRight"),
    (7, 3, "sampling"),
    (7, 4, "mixer"),
    (7, 5, "plugin"),
    (8, 0, "channel"),
    (8, 1, "arranger"),
    (8, 2, "browser"),
    (8, 3, "arrowLeft"),
    (8, 4, "file"),
    (8, 5, "auto"),
    (2, 0, "groupA"),
    (2, 1, "groupB"),
    (2, 2, "groupC"),
    (2, 3, "groupD"),
    (2, 4, "groupE"),
    (2, 5, "groupF"),
    (2, 6, "groupG"),
    (2, 7, "groupH"),
    (9, 0, "display1"),
    (9, 1, "display2"),
    (9, 2, "display3"),
    (9, 3, "display4"),
    (9, 4, "display5"),
    (9, 5, "display6"),
    (9, 6, "display7"),
];

const PEDAL_BIT: (usize, u8) = (1, 1);
const MAIN_TOUCH_BIT: (usize, u8) = (9, 7);
/// Byte 10: bit 7 is encoder 1, bit 0 encoder 8.
fn encoder_touch_bit(index: usize) -> (usize, u8) {
    (10, 7 - index as u8)
}
const ANALOG_KNOBS: [(usize, &str); 3] = [(36, "mic"), (38, "headphone"), (40, "master")];

fn bit(bytes: &[u8], location: (usize, u8)) -> bool {
    bytes.get(location.0).is_some_and(|byte| byte & (1 << location.1) != 0)
}

fn u16_at(bytes: &[u8], offset: usize) -> Option<i32> {
    Some(i32::from(*bytes.get(offset)?) | i32::from(*bytes.get(offset + 1)?) << 8)
}

/// The Maschine pad number (1 = bottom left) of a hardware index (0 = top left, row by row).
pub fn pad_number(hardware_index: usize) -> u8 {
    ((3 - hardware_index / 4) * 4 + hardware_index % 4 + 1) as u8
}

/// The hardware index of a Maschine pad number.
pub fn hardware_index(pad: u8) -> usize {
    let pad = usize::from(pad.clamp(1, 16)) - 1;
    (3 - pad / 4) * 4 + pad % 4
}

/// Velocity 1…127 from a 12-bit strike pressure, on a gentle curve: light hits still register.
pub fn velocity(raw: i32) -> u8 {
    let normalized = (f64::from(raw) / 4095.0).clamp(0.0, 1.0);
    ((normalized.powf(0.6) * 127.0).round() as i32).clamp(1, 127) as u8
}

/// Restores the report id of a delivered report: a system may or may not include it.
///
/// `report_id` is the id the system reported with the delivery. A double-pumped pad delivery holds
/// several 64-byte sets and the first pad index can itself be `0x02`, so the size, not the first
/// byte, decides whether the id is missing.
pub fn normalize(bytes: &[u8], report_id: u32) -> Vec<u8> {
    if report_id == 0 || report_id > 0xFF {
        return bytes.to_vec();
    }
    let id = report_id as u8;
    let expected = match id {
        BUTTON_REPORT_ID => Some(BUTTON_REPORT_LENGTH),
        PAD_REPORT_ID => Some(PAD_SET_LENGTH),
        _ => None,
    };
    if Some(bytes.len()) == expected {
        return bytes.to_vec();
    }
    let with_id = || [&[id], bytes].concat();
    if id == PAD_REPORT_ID && bytes.len() >= PAD_SET_LENGTH - 1 && bytes.len() % PAD_SET_LENGTH == PAD_SET_LENGTH - 1 {
        return with_id();
    }
    if expected.is_some_and(|expected| bytes.len() + 1 == expected) || bytes.first() != Some(&id) {
        return with_id();
    }
    bytes.to_vec()
}

/// [`normalize`] for a delivery that does not say which report it is (some HID layers hand over
/// only the bytes): the size tells.
pub fn normalize_unidentified(bytes: &[u8]) -> Vec<u8> {
    let id = match bytes.len() {
        length if length == BUTTON_REPORT_LENGTH - 1 => BUTTON_REPORT_ID,
        length if length >= PAD_SET_LENGTH - 1 && length % PAD_SET_LENGTH == PAD_SET_LENGTH - 1 => PAD_REPORT_ID,
        _ => return bytes.to_vec(),
    };
    normalize(bytes, u32::from(id))
}

/// One pad's state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PadState {
    /// The pad counts as held.
    pub pressed: bool,
    /// Last raw pressure, 0 to 4095.
    pub pressure: i32,
    /// When it was struck, in the caller's seconds.
    pub strike_time: f64,
    /// The pressure last reported in an event.
    pub reported_pressure: i32,
}

impl Default for PadState {
    fn default() -> Self {
        PadState { pressed: false, pressure: 0, strike_time: -1.0, reported_pressure: 0 }
    }
}

/// Turns input reports into events, remembering what is held.
#[derive(Debug, Clone)]
pub struct InputParser {
    buttons: Option<Vec<u8>>,
    knobs: [Option<i32>; 8],
    main_position: Option<i32>,
    strip_touched: bool,
    strip_value: Option<i32>,
    analog: [Option<i32>; 3],
    pads: [PadState; 16],
    /// Pressure changes smaller than this (raw 0…4095) are not reported.
    pub pressure_step: i32,
    /// The smallest analog-knob change (raw) that is reported.
    pub analog_step: i32,
    /// An encoder that is not touched needs this many counts before a turn is reported (jitter).
    pub untouched_knob_threshold: i32,
}

impl Default for InputParser {
    fn default() -> Self {
        InputParser {
            buttons: None,
            knobs: [None; 8],
            main_position: None,
            strip_touched: false,
            strip_value: None,
            analog: [None; 3],
            pads: [PadState::default(); 16],
            pressure_step: 24,
            analog_step: 12,
            untouched_knob_threshold: 3,
        }
    }
}

impl InputParser {
    /// A parser that knows nothing about the controller's state yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The pads' state, by hardware index.
    pub fn pads(&self) -> &[PadState; 16] {
        &self.pads
    }

    /// Parses a report that starts with its report id. `now` is a monotonic time in seconds.
    pub fn parse(&mut self, report: &[u8], now: f64) -> Vec<InputEvent> {
        match report.first() {
            Some(&BUTTON_REPORT_ID) => self.parse_buttons(report),
            Some(&PAD_REPORT_ID) => self.parse_pads(report, now),
            _ => Vec::new(),
        }
    }

    fn parse_buttons(&mut self, report: &[u8]) -> Vec<InputEvent> {
        if report.len() < 12 {
            return Vec::new();
        }
        let mut events = Vec::new();
        let previous = self.buttons.take();
        // An edge: a change against the last report, or "down" in the very first one.
        let edge = |location: (usize, u8)| -> Option<bool> {
            let now = bit(report, location);
            match &previous {
                Some(previous) => (bit(previous, location) != now).then_some(now),
                None => now.then_some(true),
            }
        };
        // Touch first, so that a touch-and-turn in one report arrives in a useful order.
        if let Some(touched) = edge(MAIN_TOUCH_BIT) {
            events.push(InputEvent::Touch { target: "main".into(), touched });
        }
        for index in 0..8 {
            if let Some(touched) = edge(encoder_touch_bit(index)) {
                events.push(InputEvent::Touch { target: format!("encoder{}", index + 1), touched });
            }
        }
        for (byte, bit_index, name) in BUTTON_BITS {
            if let Some(pressed) = edge((byte, bit_index)) {
                events.push(InputEvent::Button { name, pressed });
            }
        }
        if let Some(down) = edge(PEDAL_BIT) {
            events.push(InputEvent::Pedal { down });
        }
        self.buttons = Some(report[..report.len().min(BUTTON_REPORT_LENGTH)].to_vec());

        // Main encoder: a 4-bit position that wraps from 15 to 0.
        let position = i32::from(report[11] & 0x0F);
        if let Some(last) = self.main_position {
            let delta = ((position - last + 8) & 15) - 8;
            if delta != 0 {
                events.push(InputEvent::Main { delta });
            }
        }
        self.main_position = Some(position);

        // Knobs: absolute 0…999 and endless; the delta wraps in either direction.
        for index in 0..8 {
            let Some(raw) = u16_at(report, 12 + index * 2) else { continue };
            let value = raw % KNOB_MODULUS;
            let Some(last) = self.knobs[index] else {
                self.knobs[index] = Some(value);
                continue;
            };
            let mut delta = (value - last) % KNOB_MODULUS;
            if delta >= KNOB_MODULUS / 2 {
                delta -= KNOB_MODULUS;
            }
            if delta < -KNOB_MODULUS / 2 {
                delta += KNOB_MODULUS;
            }
            if delta == 0 {
                continue;
            }
            let touched = bit(report, encoder_touch_bit(index));
            if !touched && delta.abs() < self.untouched_knob_threshold {
                // The baseline stays, so a slow untouched turn still accumulates.
                continue;
            }
            self.knobs[index] = Some(value);
            events.push(InputEvent::Encoder { index: index as u8, delta: f64::from(delta) / KNOB_COUNTS_PER_UNIT });
        }

        // Touch strip: its position is zero while nobody touches it.
        if let Some(raw) = u16_at(report, STRIP_POSITION_OFFSET) {
            let touched = raw != 0;
            if touched != self.strip_touched {
                self.strip_touched = touched;
                events.push(InputEvent::Touch { target: "strip".into(), touched });
            }
            if touched && Some(raw) != self.strip_value {
                events.push(InputEvent::Strip { value: (f64::from(raw) / STRIP_FULL_SCALE).clamp(0.0, 1.0) });
            }
            self.strip_value = touched.then_some(raw);
        }

        // Analog knobs of the audio interface: 12-bit absolute.
        for (slot, (offset, name)) in ANALOG_KNOBS.into_iter().enumerate() {
            let Some(raw) = u16_at(report, offset) else { continue };
            let value = raw & 0x0FFF;
            if let Some(last) = self.analog[slot] {
                if (last - value).abs() < self.analog_step && value != 0 && value != 0x0FFF {
                    continue;
                }
            }
            if self.analog[slot] == Some(value) {
                continue;
            }
            self.analog[slot] = Some(value);
            events.push(InputEvent::Knob { name, value: f64::from(value) / ANALOG_FULL_SCALE });
        }
        events
    }

    /// Report `0x02`: one or more 64-byte sets, each the id followed by 3-byte tuples and ended by
    /// an all-zero tuple.
    fn parse_pads(&mut self, report: &[u8], now: f64) -> Vec<InputEvent> {
        let mut events = Vec::new();
        for set in report.chunks(PAD_SET_LENGTH) {
            if set[0] == PAD_REPORT_ID {
                self.decode_set(set, now, &mut events);
            }
        }
        // Of several pressure events for one pad in one report only the latest is kept.
        let mut seen = [false; 17];
        let mut kept: Vec<InputEvent> = Vec::with_capacity(events.len());
        for event in events.into_iter().rev() {
            if let InputEvent::Pad { pad, phase: PadPhase::Pressure, .. } = &event {
                if std::mem::replace(&mut seen[usize::from(*pad)], true) {
                    continue;
                }
            }
            kept.push(event);
        }
        kept.reverse();
        kept
    }

    fn decode_set(&mut self, set: &[u8], now: f64, events: &mut Vec<InputEvent>) {
        for tuple in set[1..].chunks_exact(3) {
            let (index, d1, d2) = (usize::from(tuple[0]), tuple[1], tuple[2]);
            // The list ends only when all three bytes are zero: pad index 0 can send a zero first
            // data byte at low pressure.
            if index == 0 && d1 == 0 && d2 == 0 {
                break;
            }
            if index >= 16 {
                continue;
            }
            let raw = i32::from(d1 & 0x0F) << 8 | i32::from(d2);
            self.apply(index, d1 & 0xF0, raw, now, events);
        }
    }

    fn apply(&mut self, index: usize, tag: u8, raw: i32, now: f64, events: &mut Vec<InputEvent>) {
        let pad = pad_number(index);
        let step = self.pressure_step;
        let state = &mut self.pads[index];
        enum Step {
            Press,
            Release,
            Update,
            Hold,
            Nothing,
        }
        let step_kind = match tag {
            // Hit on: always pressed.
            0x10 => {
                if state.pressed {
                    Step::Update
                } else {
                    Step::Press
                }
            }
            // Explicit off.
            0x20 | 0x30 => {
                if state.pressed {
                    Step::Release
                } else {
                    Step::Nothing
                }
            }
            // Switch on / aftertouch: hysteresis, and a rebound guard right after a strike.
            0x00 | 0x40 => {
                if !state.pressed {
                    if raw >= 32 {
                        Step::Press
                    } else {
                        Step::Nothing
                    }
                } else if raw <= 16 {
                    if now - state.strike_time >= 0.030 {
                        Step::Release
                    } else {
                        Step::Hold
                    }
                } else {
                    Step::Update
                }
            }
            _ => {
                if !state.pressed {
                    if raw >= 64 {
                        Step::Press
                    } else {
                        Step::Nothing
                    }
                } else if raw <= 48 {
                    Step::Release
                } else {
                    Step::Update
                }
            }
        };
        match step_kind {
            Step::Press => {
                *state = PadState { pressed: true, pressure: raw, reported_pressure: raw, strike_time: now };
                events.push(InputEvent::Pad { pad, phase: PadPhase::Down, pressure: f64::from(raw) / 4095.0, velocity: Some(velocity(raw)) });
            }
            Step::Release => {
                state.pressed = false;
                state.pressure = 0;
                state.reported_pressure = 0;
                events.push(InputEvent::Pad { pad, phase: PadPhase::Up, pressure: 0.0, velocity: None });
            }
            Step::Update => {
                state.pressure = raw;
                if (raw - state.reported_pressure).abs() >= step {
                    state.reported_pressure = raw;
                    events.push(InputEvent::Pad { pad, phase: PadPhase::Pressure, pressure: f64::from(raw) / 4095.0, velocity: None });
                }
            }
            Step::Hold => state.pressure = raw,
            Step::Nothing => {}
        }
    }

    /// The controller is gone: releases every held pad, button, pedal and touch sensor, and
    /// forgets all baselines (the next reports establish new ones).
    pub fn release_all(&mut self) -> Vec<InputEvent> {
        let mut events = Vec::new();
        for index in 0..16 {
            if self.pads[index].pressed {
                events.push(InputEvent::Pad { pad: pad_number(index), phase: PadPhase::Up, pressure: 0.0, velocity: None });
            }
        }
        if let Some(buttons) = &self.buttons {
            if bit(buttons, PEDAL_BIT) {
                events.push(InputEvent::Pedal { down: false });
            }
            if bit(buttons, MAIN_TOUCH_BIT) {
                events.push(InputEvent::Touch { target: "main".into(), touched: false });
            }
            for index in 0..8 {
                if bit(buttons, encoder_touch_bit(index)) {
                    events.push(InputEvent::Touch { target: format!("encoder{}", index + 1), touched: false });
                }
            }
            for (byte, bit_index, name) in BUTTON_BITS {
                if bit(buttons, (byte, bit_index)) {
                    events.push(InputEvent::Button { name, pressed: false });
                }
            }
        }
        if self.strip_touched {
            events.push(InputEvent::Touch { target: "strip".into(), touched: false });
        }
        *self = InputParser {
            pressure_step: self.pressure_step,
            analog_step: self.analog_step,
            untouched_knob_threshold: self.untouched_knob_threshold,
            ..InputParser::default()
        };
        events
    }
}

/// An LED colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

impl Rgb {
    /// Parses `"#rrggbb"`, `"#rgb"` or `"#rrggbbaa"` (the alpha is ignored for LEDs).
    pub fn from_hex(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix('#').unwrap_or(text);
        if !text.is_ascii() {
            return None;
        }
        let expanded: String = if text.len() == 3 { text.chars().flat_map(|digit| [digit, digit]).collect() } else { text.to_string() };
        if expanded.len() != 6 && expanded.len() != 8 {
            return None;
        }
        let number = u32::from_str_radix(&expanded[..6], 16).ok()?;
        Some(Rgb { r: (number >> 16) as u8, g: (number >> 8) as u8, b: number as u8 })
    }

    fn from_value(value: &Value) -> Option<Self> {
        value.as_str().and_then(Self::from_hex)
    }
}

/// A JSON number as the controller and screen descriptions use them (`true` and `false` read as 1
/// and 0).
pub(crate) fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::Bool(flag) => Some(f64::from(u8::from(*flag))),
        _ => None,
    }
}

/// LED byte encodings.
pub mod led {
    use super::Rgb;

    /// The hue, in degrees, of each of Native Instruments' 16 indexed colours (index 1 to 16, red
    /// to fuchsia). Index 17 is white.
    pub const PALETTE_HUES: [f64; 16] = [0.0, 15.0, 30.0, 45.0, 60.0, 90.0, 120.0, 150.0, 180.0, 210.0, 240.0, 270.0, 300.0, 315.0, 330.0, 345.0];
    /// The palette index of white.
    pub const WHITE: u8 = 17;
    /// Below this value (of the brightest channel, 0 to 1) the LED is off. Dim markers (about 6 %)
    /// must still light.
    pub const OFF_BELOW: f64 = 0.03;
    /// Colours less saturated than this light white. The interface's track palette is muted
    /// (saturation 0.07 to 0.4); a higher threshold turned most groups and pads white.
    pub const WHITE_BELOW_SATURATION: f64 = 0.05;
    /// Bytes of the four backlight levels. As a 7-bit level (the descriptor's logical range) they
    /// are about 9 %, 26 %, 52 % and 100 %; read as a 2-bit index (`byte & 3`) they are 0, 1, 2, 3:
    /// a sensible ramp under both readings of the protocol.
    pub const SINGLE_LEVELS: [u8; 4] = [0x0C, 0x21, 0x42, 0x7F];

    fn intensity(value: f64) -> u8 {
        if value < 0.3 {
            0
        } else if value < 0.55 {
            1
        } else if value < 0.8 {
            2
        } else {
            3
        }
    }

    /// The byte of a pad, group, strip or navigation LED: `(palette index << 2) | intensity`, 0 off.
    pub fn indexed(color: Option<Rgb>) -> u8 {
        let Some(color) = color else { return 0 };
        let (r, g, b) = (f64::from(color.r) / 255.0, f64::from(color.g) / 255.0, f64::from(color.b) / 255.0);
        let (maximum, minimum) = (r.max(g).max(b), r.min(g).min(b));
        if maximum < OFF_BELOW {
            return 0;
        }
        let saturation = if maximum == 0.0 { 0.0 } else { (maximum - minimum) / maximum };
        let intensity = intensity(maximum);
        if saturation < WHITE_BELOW_SATURATION {
            return WHITE << 2 | intensity;
        }
        let delta = maximum - minimum;
        let mut hue = if maximum == r {
            60.0 * (((g - b) / delta) % 6.0)
        } else if maximum == g {
            60.0 * ((b - r) / delta + 2.0)
        } else {
            60.0 * ((r - g) / delta + 4.0)
        };
        if hue < 0.0 {
            hue += 360.0;
        }
        let mut best = 0;
        let mut best_distance = f64::INFINITY;
        for (index, candidate) in PALETTE_HUES.iter().enumerate() {
            let distance = (hue - candidate).abs().min(360.0 - (hue - candidate).abs());
            if distance < best_distance {
                best_distance = distance;
                best = index;
            }
        }
        ((best as u8 + 1) << 2) | intensity
    }

    /// The byte of a single-colour button backlight for a brightness of 0 to 1.
    pub fn single(level: f64) -> u8 {
        if level > 0.001 {
            SINGLE_LEVELS[usize::from(intensity(level))]
        } else {
            0
        }
    }
}

/// Byte index of each button's LED in report `0x80` (byte 0 is the report id).
pub fn button_led_offset(name: &str) -> Option<usize> {
    const OFFSETS: [(&str, usize); 50] = [
        ("channel", 1),
        ("plugin", 2),
        ("arranger", 3),
        ("mixer", 4),
        ("browser", 5),
        ("sampling", 6),
        ("arrowLeft", 7),
        ("arrowRight", 8),
        ("file", 9),
        ("settings", 10),
        ("auto", 11),
        ("macro", 12),
        ("display1", 13),
        ("display2", 14),
        ("display3", 15),
        ("display4", 16),
        ("display5", 17),
        ("display6", 18),
        ("display7", 19),
        ("display8", 20),
        ("volume", 21),
        ("swing", 22),
        ("noteRepeat", 23),
        ("tempo", 24),
        ("lock", 25),
        ("pitch", 26),
        ("mod", 27),
        ("perform", 28),
        ("notes", 29),
        ("restart", 38),
        ("erase", 39),
        ("tap", 40),
        ("follow", 41),
        ("play", 42),
        ("rec", 43),
        ("stop", 44),
        ("shift", 45),
        ("fixedVel", 46),
        ("padMode", 47),
        ("keyboard", 48),
        ("chords", 49),
        ("step", 50),
        ("scene", 51),
        ("pattern", 52),
        ("events", 53),
        ("variation", 54),
        ("duplicate", 55),
        ("select", 56),
        ("solo", 57),
        ("mute", 58),
    ];
    OFFSETS.iter().find(|(button, _)| *button == name).map(|(_, offset)| *offset)
}

/// The names of every button that has an LED, sorted.
pub fn led_button_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = BUTTON_BITS.iter().map(|(_, _, name)| *name).filter(|name| button_led_offset(name).is_some()).collect();
    names.sort_unstable();
    names
}

const GROUP_LED_OFFSET: usize = 30;
const STRIP_LED_OFFSET: usize = 1;
const PAD_LED_OFFSET: usize = 26;

fn nav_led_offset(name: &str) -> Option<usize> {
    match name {
        "up" => Some(59),
        "left" => Some(60),
        "right" => Some(61),
        "down" => Some(62),
        _ => None,
    }
}

/// Everything the controller's LEDs show. `mk3.leds` payloads change parts of it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LedState {
    /// Button backlights by contract name, 0 to 1.
    pub buttons: BTreeMap<String, f64>,
    /// Group buttons A to H.
    pub groups: [Option<Rgb>; 8],
    /// Pads 1 to 16.
    pub pads: [Option<Rgb>; 16],
    /// The 25 LEDs of the touch strip.
    pub strip: [Option<Rgb>; 25],
    /// The LEDs around the main encoder: `up`, `left`, `right`, `down`.
    pub nav: BTreeMap<String, Option<Rgb>>,
}

impl LedState {
    /// Everything off.
    pub fn new() -> Self {
        Self::default()
    }

    fn gray(level: f64) -> Option<Rgb> {
        if level > 0.001 {
            let value = (level * 255.0).round().clamp(0.0, 255.0) as u8;
            Some(Rgb { r: value, g: value, b: value })
        } else {
            None
        }
    }

    /// Applies a partial `mk3.leds` payload:
    /// `{buttons?: {name: 0…1}, groups?: [colour or null × 8], pads?: [… × 16], strip?: [colour or
    /// brightness × 25], nav?: {up|left|right|down: colour or null}}`. Colours are `"#rrggbb"`.
    /// Unknown names are ignored; lists set their first entries.
    pub fn apply(&mut self, payload: &Value) {
        if let Some(values) = payload.get("buttons").and_then(Value::as_object) {
            for (name, value) in values {
                if button_led_offset(name).is_some() {
                    self.buttons.insert(name.clone(), number(value).unwrap_or(0.0).clamp(0.0, 1.0));
                }
            }
        }
        let list = |key: &str| payload.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        for (slot, value) in self.groups.iter_mut().zip(list("groups")) {
            *slot = Rgb::from_value(value);
        }
        for (slot, value) in self.pads.iter_mut().zip(list("pads")) {
            *slot = Rgb::from_value(value);
        }
        for (slot, value) in self.strip.iter_mut().zip(list("strip")) {
            *slot = Rgb::from_value(value).or_else(|| number(value).and_then(Self::gray));
        }
        if let Some(values) = payload.get("nav").and_then(Value::as_object) {
            for (name, value) in values {
                if nav_led_offset(name).is_some() {
                    self.nav.insert(name.clone(), Rgb::from_value(value));
                }
            }
        }
    }

    /// Output report `0x80`: button backlights, group and navigation LEDs.
    pub fn button_report(&self) -> Vec<u8> {
        let mut report = vec![0u8; BUTTON_LED_REPORT_LENGTH];
        report[0] = BUTTON_LED_REPORT_ID;
        for (name, level) in &self.buttons {
            if let Some(offset) = button_led_offset(name) {
                report[offset] = led::single(*level);
            }
        }
        for (index, group) in self.groups.iter().enumerate() {
            report[GROUP_LED_OFFSET + index] = led::indexed(*group);
        }
        for (name, color) in &self.nav {
            if let Some(offset) = nav_led_offset(name) {
                report[offset] = led::indexed(*color);
            }
        }
        report
    }

    /// Output report `0x81`: touch strip and pad LEDs.
    pub fn pad_report(&self) -> Vec<u8> {
        let mut report = vec![0u8; PAD_LED_REPORT_LENGTH];
        report[0] = PAD_LED_REPORT_ID;
        for (index, color) in self.strip.iter().enumerate() {
            report[STRIP_LED_OFFSET + index] = led::indexed(*color);
        }
        for pad in 1..=16u8 {
            report[PAD_LED_OFFSET + hardware_index(pad)] = led::indexed(self.pads[usize::from(pad) - 1]);
        }
        report
    }

    /// Both output reports.
    pub fn reports(&self) -> Vec<Vec<u8>> {
        vec![self.button_report(), self.pad_report()]
    }
}

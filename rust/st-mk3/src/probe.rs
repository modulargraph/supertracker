//! `supertracker --mk3-probe <seconds> [--exclusive] [--raw]`: a hardware diagnostic without a
//! window. It prints every decoded event, lights the pads in a rainbow, steps the button
//! backlights through their levels and, only with `--exclusive`, draws a test pattern on both
//! screens. It always releases the device and restores the vendor's agents.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::driver::{DriverCallbacks, DriverOptions, Mk3Driver};
use crate::protocol::{led, led_button_names};

/// How the probe runs.
pub struct ProbeOptions {
    /// How long to run.
    pub seconds: f64,
    /// Take over the screens and draw the test pattern.
    pub exclusive: bool,
    /// Print every raw input report.
    pub raw: bool,
}

/// `"#rrggbb"` for a hue in degrees at full saturation.
pub fn hue_hex(hue: f64, value: f64) -> String {
    let sector = hue.rem_euclid(360.0) / 60.0;
    let x = value * (1.0 - (sector % 2.0 - 1.0).abs());
    let (r, g, b) = match sector as u32 {
        0 => (value, x, 0.0),
        1 => (x, value, 0.0),
        2 => (0.0, value, x),
        3 => (0.0, x, value),
        4 => (x, 0.0, value),
        _ => (value, 0.0, x),
    };
    format!("#{:02x}{:02x}{:02x}", (r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// The test pattern for one screen: labelled colour bars (to tell RGB565 from BGR565), a frame,
/// four knob rings, a progress bar and a clock.
pub fn screen_ops(side: &str, elapsed: u64, seconds: u64) -> Vec<Value> {
    let mut ops = vec![json!({"op": "clear", "color": "#0b0e14"})];
    for (index, (color, label)) in [("#ff0000", "RED"), ("#00ff00", "GREEN"), ("#0000ff", "BLUE"), ("#ffffff", "WHITE")].into_iter().enumerate() {
        let x = 16 + index * 114;
        ops.push(json!({"op": "rect", "x": x, "y": 52, "w": 106, "h": 60, "color": color, "radius": 6}));
        ops.push(json!({"op": "text", "x": x, "y": 118, "width": 106, "text": label, "size": 14, "color": "#e6e9ef", "align": "center", "weight": "bold"}));
    }
    ops.push(json!({"op": "text", "x": 16, "y": 12, "text": format!("SuperTracker · Maschine MK3 probe · {side} screen"), "size": 20, "color": "#ffffff", "weight": "bold", "width": 448}));
    ops.push(json!({"op": "frame", "x": 0, "y": 0, "w": 480, "h": 272, "color": "#3a7bd5", "width": 2}));
    ops.push(json!({"op": "line", "x1": 16, "y1": 44, "x2": 464, "y2": 44, "color": "#3a7bd5", "width": 1}));
    for knob in 0..4u64 {
        let value = ((elapsed + knob * 3) % 12) as f64 / 11.0;
        let x = 60 + knob * 120;
        ops.push(json!({"op": "arc", "x": x, "y": 190, "r": 26, "value": value, "color": hue_hex(knob as f64 * 90.0, 1.0), "background": "#2a2f3a", "width": 5}));
        ops.push(json!({"op": "text", "x": x, "y": 182, "text": format!("{}", (value * 100.0) as u32), "size": 13, "color": "#e6e9ef", "align": "center", "mono": true}));
    }
    ops.push(json!({"op": "bar", "x": 16, "y": 236, "w": 448, "h": 10, "value": elapsed as f64 / seconds.max(1) as f64, "color": "#5ad19a", "background": "#2a2f3a"}));
    ops.push(json!({"op": "text", "x": 464, "y": 250, "text": format!("{elapsed} / {seconds} s"), "size": 12, "color": "#9aa3b2", "align": "right", "mono": true}));
    ops
}

/// Runs the probe with a driver built from `options` (its callbacks are replaced) and prints to
/// `output`. Returns the exit code: 0, or 2 when no controller was found.
pub fn run(mut options: DriverOptions, probe: ProbeOptions, output: impl Write + Send + 'static) -> i32 {
    let output = Arc::new(Mutex::new(output));
    let line = {
        let output = output.clone();
        move |text: String| {
            let mut output = output.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let _ = writeln!(output, "{text}");
            let _ = output.flush();
        }
    };
    let counts: Arc<Mutex<BTreeMap<String, usize>>> = Arc::default();
    let (log_line, status_line, input_line, raw_line, counted, raw) = (line.clone(), line.clone(), line.clone(), line.clone(), counts.clone(), probe.raw);
    options.callbacks = DriverCallbacks {
        log: Box::new(move |message| log_line(format!("log {message}"))),
        status: Box::new(move |status| status_line(format!("status {}", json!(status)))),
        input: Box::new(move |payload: Map<String, Value>| {
            let kind = payload.get("type").and_then(Value::as_str).unwrap_or("?").to_string();
            *counted.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).entry(kind).or_insert(0) += 1;
            input_line(format!("input {}", Value::Object(payload)));
        }),
        raw_report: Box::new(move |report| {
            if raw {
                let bytes: Vec<String> = report.iter().skip(1).map(|byte| format!("{byte:02x}")).collect();
                raw_line(format!("raw {:#04x} {}", report.first().copied().unwrap_or(0), bytes.join(" ")));
            }
        }),
    };
    let driver = Mk3Driver::new(options);
    line(format!("mk3-probe: {} mode, {} s", if probe.exclusive { "EXCLUSIVE (NI agents will be stopped and restored)" } else { "shared" }, probe.seconds as u64));
    driver.connect(probe.exclusive);
    let start = Instant::now();
    while !driver.status().connected && start.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(100));
    }
    if !driver.status().connected {
        line("mk3-probe: no Maschine MK3 (17cc:1600) found".into());
        driver.disconnect();
        return 2;
    }
    driver.set_leds(&json!({
        "pads": (0..16).map(|pad| hue_hex(f64::from(pad) * 22.5, 1.0)).collect::<Vec<_>>(),
        "groups": (0..8).map(|group| hue_hex(f64::from(group) * 45.0, 1.0)).collect::<Vec<_>>(),
        "strip": (0..25).map(|index| f64::from(index) / 24.0).collect::<Vec<_>>(),
        "nav": {"up": "#ff0000", "right": "#00ff00", "down": "#0000ff", "left": "#ffffff"},
    }));
    line("leds pads rainbow (pad 1 = red, bottom-left), groups A–H rainbow, nav up/right/down/left = red/green/blue/white".into());
    let names = led_button_names();
    let levels = [0.25, 0.5, 0.75, 1.0, 0.0];
    let total = probe.seconds.ceil().max(0.0) as u64;
    let (mut level, mut drawn, mut next_level) = (0usize, None, Instant::now());
    while start.elapsed().as_secs_f64() < probe.seconds {
        let second = start.elapsed().as_secs();
        if Instant::now() >= next_level {
            let value = levels[level % levels.len()];
            driver.set_leds(&json!({"buttons": names.iter().map(|name| (name.to_string(), json!(value))).collect::<Map<String, Value>>()}));
            line(format!("leds all button backlights level {value:.2} → byte {:#04x}", led::single(value)));
            level += 1;
            next_level = Instant::now() + Duration::from_millis(1500);
        }
        if driver.status().screens && drawn != Some(second) {
            drawn = Some(second);
            driver.set_screens(&json!({"left": screen_ops("left", second, total), "right": screen_ops("right", second, total)}));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    line(format!("diagnostics {}", driver.diagnostics()));
    line(format!("event counts {}", json!(*counts.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))));
    driver.disconnect();
    line(format!("final status {}", json!(driver.status())));
    0
}

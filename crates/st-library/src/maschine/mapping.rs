//! Maps the numeric members of Maschine modules onto SuperTracker's own effects, drum models
//! and sampler.
//!
//! The inputs are raw archive values in serializer order (see [`super::archive::Archive::parameters`]):
//! some are normalized 0..1, some signed, some integers or enums, so no array is ever treated
//! as uniformly normalized. The targets are independent processors: where the original unit
//! conversion is known it is applied, where only a percentage is known the conversion is an
//! explicit approximation, and where nothing is known the SuperTracker default stays and a
//! warning says so. The evidence for each mapping is in `docs/research/maschine-*`.
//!
//! Effect mappings are returned as normalized values by parameter address of the target effect
//! (see [`super::catalogue`]); unknown versions and array shapes return `None`.

use super::catalogue::{self, Effect};
use super::curves;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// A Maschine effect module mapped onto a built-in effect.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectMapping {
    /// The component of the target effect, `stfx:<effect id>`.
    pub id: String,
    /// Normalized values by parameter address, ascending. Parameters that are not listed keep
    /// the effect's defaults.
    pub parameters: Vec<(usize, f64)>,
    /// What is approximate, limited or not converted.
    pub warnings: Vec<String>,
}

impl EffectMapping {
    /// The effect's id without the component prefix.
    pub fn effect_id(&self) -> &str {
        self.id.strip_prefix("stfx:").unwrap_or(&self.id)
    }

    /// The displayed physical value the mapping sets for the parameter `key`, if it sets one.
    pub fn physical(&self, key: &str) -> Option<f64> {
        let effect = catalogue::effect(self.effect_id())?;
        let (address, parameter) = effect.parameter(key)?;
        self.parameters.iter().find(|(mapped, _)| *mapped == address).map(|(_, value)| catalogue::physical(parameter, *value))
    }

    /// The mapped values by parameter id.
    pub fn by_id(&self) -> BTreeMap<String, f64> {
        let Some(effect) = catalogue::effect(self.effect_id()) else { return BTreeMap::new() };
        self.parameters.iter().filter_map(|(address, value)| effect.parameters.get(*address).map(|parameter| (parameter.id.to_string(), *value))).collect()
    }
}

fn single(value: u32) -> f64 {
    f64::from(f32::from_bits(value))
}

fn exact_integer(value: f64, low: i64, high: i64) -> Option<i64> {
    (value.is_finite() && value.round() == value && value >= low as f64 && value <= high as f64).then_some(value as i64)
}

struct Mapper<'a> {
    effect: &'static Effect,
    raw: &'a [f64],
    values: BTreeMap<usize, f64>,
    warnings: Vec<String>,
    /// "verified" or "known": how the out-of-range warning words the source range.
    range_word: &'static str,
}

impl<'a> Mapper<'a> {
    fn new(kind: &'a str, effect_id: &str, raw: &'a [f64], range_word: &'static str) -> Option<Mapper<'a>> {
        if !raw.iter().all(|value| value.is_finite()) {
            return None;
        }
        let effect = catalogue::effect(effect_id)?;
        Some(Mapper { effect, raw, values: BTreeMap::new(), warnings: vec![format!("{kind} uses an independent SuperTracker effect; its sound is an approximation of the original.")], range_word })
    }

    fn warn(&mut self, message: impl Into<String>) {
        let message = message.into();
        if !self.warnings.contains(&message) {
            self.warnings.push(message);
        }
    }

    /// The raw value at `index` when it lies in `low..=high`; otherwise a warning.
    fn value_in(&mut self, index: usize, low: f64, high: f64) -> Option<f64> {
        let value = self.raw[index];
        if value < low || value > high {
            self.warn(format!("Original parameter {} is outside its {} range; the SuperTracker default was retained.", index + 1, self.range_word));
            return None;
        }
        Some(value)
    }

    fn value(&mut self, index: usize) -> Option<f64> {
        self.value_in(index, 0.0, 1.0)
    }

    /// Sets the target parameter `key` to a displayed physical value, limited to its range.
    fn parameter(&mut self, key: &str, physical: f64) {
        if !physical.is_finite() {
            return;
        }
        match self.effect.parameter(key) {
            Some((address, parameter)) => {
                if physical < parameter.minimum || physical > parameter.maximum {
                    self.warn(format!("{key} exceeds the SuperTracker range and was limited to the nearest supported value."));
                }
                self.values.insert(address, catalogue::normalized(parameter, physical));
            }
            None => self.warn(format!("{key} has no matching SuperTracker control and was retained only in the original import data.")),
        }
    }

    fn percent(&mut self, index: usize, key: &str) {
        if let Some(value) = self.value(index) {
            self.parameter(key, value * 100.0);
        }
    }

    fn signed_percent(&mut self, index: usize, key: &str) {
        if let Some(value) = self.value_in(index, -1.0, 1.0) {
            self.parameter(key, value * 100.0);
        }
    }

    fn toggle(&mut self, index: usize, key: &str) {
        let value = self.raw[index];
        if value == 0.0 || value == 1.0 {
            self.parameter(key, value);
        } else {
            self.warn(format!("Invalid {key} switch value; the default was retained."));
        }
    }

    fn choice(&mut self, index: usize, key: &str, maximum: i64) -> Option<i64> {
        match exact_integer(self.raw[index], 0, maximum) {
            Some(value) => {
                self.parameter(key, value as f64);
                Some(value)
            }
            None => {
                self.warn(format!("Unknown {key} choice; the SuperTracker default was retained."));
                None
            }
        }
    }

    fn curve(&mut self, index: usize, key: &str, low: f64, high: f64, logarithmic: bool) {
        if let Some(value) = self.value(index) {
            self.parameter(key, if logarithmic { low * (high / low).powf(value) } else { low + (high - low) * value });
        }
    }

    fn finish(self) -> EffectMapping {
        EffectMapping { id: format!("stfx:{}", self.effect.id), parameters: self.values.into_iter().collect(), warnings: self.warnings }
    }
}

fn short_name(kind: &str) -> &str {
    kind.rsplit("::").next().unwrap_or(kind)
}

/// Maps an effect module onto a built-in effect. `None` for a module, version or parameter
/// count without a verified mapping, and for non-finite values.
pub fn map_effect(kind: &str, version: u64, raw: &[f64]) -> Option<EffectMapping> {
    map_core(kind, version, raw).or_else(|| map_spatial(kind, version, raw)).or_else(|| map_additional(kind, version, raw)).or_else(|| map_distortion(kind, version, raw))
}

/// EQ and Compressor.
///
/// EQ version 0: LF frequency and gain, M1 frequency and gain, M2 frequency and gain, HF
/// frequency and gain, M1 width, M2 width, output gain. Compressor version 0: threshold,
/// amount, knee, attack, release, gain; version 1 puts the mode first. The compressor's
/// side-chain filter is a separate object and not part of the array.
pub fn map_core(kind: &str, version: u64, raw: &[f64]) -> Option<EffectMapping> {
    let kind = short_name(kind);
    let id = match kind {
        "EQ" if version == 0 && raw.len() == 11 => "eq",
        "Compressor" if (version == 0 && raw.len() == 6) || (version == 1 && raw.len() == 7) => "compressor",
        _ => return None,
    };
    let mut m = Mapper::new(kind, id, raw, "verified")?;
    if kind == "EQ" {
        for (index, key, low) in [(0, "lowfreq", 20.0), (2, "lowmidfreq", 40.0), (4, "highmidfreq", 40.0), (6, "highfreq", 50.0)] {
            if let Some(value) = m.value(index) {
                m.parameter(key, low * 400f64.powf(value));
            }
        }
        for (index, key) in [(1, "lowgain"), (3, "lowmidgain"), (5, "highmidgain"), (7, "highgain"), (10, "gain")] {
            if let Some(value) = m.value_in(index, -1.0, 1.0) {
                m.parameter(key, 20.0 * value);
            }
        }
        // The source shows these as octaves, not Q; both equalizers expose a physical bandwidth.
        for (index, key) in [(8, "lowmidwidth"), (9, "highmidwidth")] {
            if let Some(value) = m.value(index) {
                m.parameter(key, 0.1 + 3.9 * value);
            }
        }
    } else {
        let start = usize::from(version == 1);
        if version == 0 {
            m.parameter("mode", 0.0);
        } else if raw[0] == 0.0 || raw[0] == 1.0 {
            m.parameter("mode", raw[0]);
        } else {
            m.warn("Unknown compressor mode; the SuperTracker default was retained.");
        }
        if let Some(value) = m.value(start) {
            m.parameter("threshold", -60.0 * value);
        }
        if let Some(value) = m.value(start + 3) {
            m.parameter("attack", 8.0 * value * value * value);
        }
        if let Some(value) = m.value(start + 4) {
            m.parameter("release", single(0x3f8f_9e4d).powf(32.0 + 16.0 * value).clamp(40.0, 251.0));
        }
        if let Some(value) = m.value(start + 5) {
            m.parameter("gain", -36.0 + 36.0 * value);
        }
        m.value(start + 1);
        m.value(start + 2);
        m.warn("Original Amount and Knee use different controls with no verified ratio/dB conversion; SuperTracker Ratio and Knee defaults were retained.");
        m.warn("Original compressor side-chain source and filter settings are stored separately and were not mapped by this helper.");
    }
    Some(m.finish())
}

/// Reverbs, delays, grain effects and Resochord. Parameter order follows the serializers of
/// Maschine 3.7; perceptual curves without a verified unit conversion are approximations.
pub fn map_spatial(kind: &str, version: u64, raw: &[f64]) -> Option<EffectMapping> {
    let kind = short_name(kind);
    let plate_mode = if kind == "PlateReverb" {
        match (version, raw.len()) {
            (0, 5) => Some(2),
            (1, 23) => Some(exact_integer(raw[5], 0, 2)?),
            _ => return None,
        }
    } else {
        None
    };
    let id = match (kind, raw.len()) {
        ("PlateReverb", _) => {
            if plate_mode == Some(2) {
                "plate"
            } else {
                "reverb"
            }
        }
        ("Iceverb" | "Ice", 4) if version == 0 => "ice",
        ("Reflex", 4) if version == 0 => "reflex",
        ("Metaverb", 5) if version == 0 => "metaverb",
        ("Reverb", 8) if version == 0 => "reverblegacy",
        ("BeatDelay", 9) if version == 0 => "beatdelay",
        ("GrainDelay", 9) if version == 0 => "graindelay",
        ("GrainStretch", 7) if version == 0 => "grainstretch",
        ("Resochord", 9) if version == 0 => "resochord",
        _ => return None,
    };
    let mut m = Mapper::new(kind, id, raw, "known")?;
    // A two-slope EQ control with its neutral position in the middle.
    fn eq(m: &mut Mapper, index: usize, key: &str) {
        if let Some(value) = m.value(index) {
            m.parameter(key, if value < 0.5 { (value - 0.5) * 48.0 } else { (value - 0.5) * 24.0 });
        }
    }
    match kind {
        "Iceverb" | "Ice" => {
            m.percent(0, "color");
            m.percent(1, "ice");
            m.percent(2, "size");
            m.percent(3, "mix");
        }
        "Reflex" => {
            m.percent(0, "color");
            m.percent(1, "smooth");
            m.percent(2, "size");
            m.percent(3, "mix");
        }
        "Metaverb" => {
            m.percent(0, "size");
            eq(&mut m, 1, "low");
            eq(&mut m, 2, "high");
            m.signed_percent(3, "pan");
            m.percent(4, "mix");
            m.warn("Low and High EQ curves are approximated around their neutral midpoint.");
        }
        "Reverb" => {
            match exact_integer(raw[0], 0, 3) {
                Some(room) => m.parameter("room", room as f64),
                None => m.warn("Unrecognized legacy room type; General was retained."),
            }
            m.percent(1, "size");
            eq(&mut m, 2, "low");
            eq(&mut m, 3, "high");
            m.signed_percent(4, "pan");
            // The original Stereo is bipolar with zero as its neutral default; the target width is unipolar.
            if let Some(stereo) = m.value_in(5, -1.0, 1.0) {
                m.parameter("stereo", 100.0 + stereo * if stereo < 0.0 { 100.0 } else { 50.0 });
            }
            m.percent(6, "mix");
            m.toggle(7, "freeze");
            m.warn("Legacy room, EQ and stereo response are approximated.");
        }
        "BeatDelay" => {
            match exact_integer(raw[0], 0, 16) {
                Some(time) => m.parameter("time", if time == 0 { 0.0 } else { (time * 2 - 1) as f64 }),
                None => m.warn("Unknown delay Time choice; the default was retained."),
            }
            if raw[1] != 0.0 {
                m.warn("The original delay Offset uses a different secondary-tap routing and was not applied.");
            }
            m.percent(2, "feedback");
            m.percent(3, "crossover");
            m.percent(4, "color");
            m.percent(5, "split");
            m.signed_percent(6, "stereo");
            m.percent(7, "mix");
            match exact_integer(raw[8], 0, 2) {
                Some(unit) => m.parameter("unit", [2.0, 5.0, 1.0][unit as usize]),
                None => m.warn("Unknown delay Unit choice; the default was retained."),
            }
        }
        "GrainDelay" => {
            if let Some(value) = m.value_in(0, -1.0, 1.0) {
                m.parameter("pitch", value * 24.0);
            }
            m.curve(1, "size", 5.0, 500.0, true);
            m.percent(2, "jitter");
            m.toggle(3, "reverse");
            m.curve(4, "space", 0.0, 2000.0, false);
            m.percent(5, "density");
            m.percent(6, "mod");
            m.percent(7, "mix");
            m.percent(8, "stereo");
            m.warn("Grain pitch, size and spacing curves are approximated; grain timing is not NI-identical.");
        }
        "GrainStretch" => {
            // The capture switch is kept, but no captured audio is part of the module's state.
            m.toggle(0, "on");
            m.curve(1, "stretch", 0.0, 100.0, false);
            // The enum's labels are lengths in sixteenth-note steps.
            match exact_integer(raw[2], 0, 8) {
                Some(choice) => m.parameter("loop", [1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0, 32.0][choice as usize]),
                None => m.warn("Unknown Grain Stretch loop length; the default was retained."),
            }
            m.curve(3, "pitch", -24.0, 24.0, false);
            m.toggle(4, "link");
            m.curve(5, "size", 5.0, 300.0, true);
            m.percent(6, "mix");
            m.warn("Grain Stretch pitch, size and stretch curves are approximated. Fresh audio must be captured.");
        }
        "Resochord" => {
            // The original lists String, Chord; this rack presents Chord first.
            match exact_integer(raw[1], 0, 1) {
                Some(mode) => m.parameter("mode", (1 - mode) as f64),
                None => m.warn("Unknown Resochord mode; the default was retained."),
            }
            match exact_integer(raw[0], 0, 1) {
                // Major/minor triad and seventh variants have a close musical counterpart.
                Some(style) => m.parameter("style", if raw[2] == 2.0 || raw[2] == 3.0 { if style == 0 { 2.0 } else { 3.0 } } else { style as f64 }),
                None => m.warn("Resochord alt/meta/frank voicings are not matched; the default Major style was retained."),
            }
            m.percent(3, "spread");
            if let Some(tune) = m.value_in(4, -24.0, 24.0).filter(|tune| tune.round() == *tune) {
                m.parameter("tune", tune);
            }
            m.percent(5, "brightness");
            m.percent(6, "feedback");
            m.curve(7, "decay", 0.03, 20.0, true);
            m.percent(8, "mix");
            m.warn("Resochord voicing and Decay are approximated; NI chord choices are not pitch-root indices.");
        }
        _ => {
            if plate_mode == Some(2) {
                m.curve(0, "predelay", 0.0, 250.0, false);
                m.curve(1, "decay", 0.2, 20.2, true);
                m.percent(2, "mix");
                m.curve(3, "lowshelf", 0.0, -24.0, false);
                m.percent(4, "highdamp");
            } else {
                let mode = plate_mode.unwrap_or(0);
                let start = if mode == 0 { 7 } else { 15 };
                m.parameter("mode", mode as f64);
                m.curve(start, "time", 0.5, 20.2, true);
                m.percent(start + 1, "size");
                m.percent(start + 2, "damping");
                m.percent(start + 3, "modulation");
                m.percent(start + 4, "diffusion");
                m.curve(start + 5, "predelay", 0.0, 250.0, false);
                m.curve(start + 6, "highcut", 20000.0, 2000.0, true);
                m.curve(start + 7, "lowshelf", 0.0, -12.0, false);
                m.percent(2, "mix");
            }
            m.warn("Reverb time, EQ and predelay curves are approximated. The original wet-gain control is not applied.");
        }
    }
    Some(m.finish())
}

/// Gate, Limiter, Maximizer, Transient Master, Cabinet, Chorus, FM, Filter, Flanger, Frequency
/// Shifter and Phaser (see `docs/research/maschine-additional-parameter-mapping.md`).
pub fn map_additional(kind: &str, version: u64, raw: &[f64]) -> Option<EffectMapping> {
    let kind = short_name(kind);
    let (id, expected_version, count) = match kind {
        "Gate" => ("gate", 0, 5),
        "Limiter" => ("limiter", 1, 4),
        "Maximizer" => ("maximizer", 0, 3),
        "TransientDesigner" => ("transient", 0, 4),
        "CabinetEmulation" => ("cabinet", 0, 4),
        "Chorus" => ("chorus", 0, 3),
        "FM" => ("fm", 0, 4),
        "Filter" => ("filter", 0, 11),
        "Flanger" => ("flanger", 0, 8),
        "FreqShifter" => ("freqshifter", 0, 6),
        "Phaser" => ("phaser", 0, 9),
        _ => return None,
    };
    if version != expected_version || raw.len() != count {
        return None;
    }
    let mut m = Mapper::new(kind, id, raw, "known")?;
    fn modulation(m: &mut Mapper, speed: usize, shape: usize, stereo: usize, source: Option<usize>) {
        match source {
            Some(source) => {
                m.choice(source, "source", 2);
            }
            None => m.parameter("source", 0.0), // Flanger version 0 predates the saved Source enum.
        }
        m.curve(speed, "speed", 0.03, 8.0, true);
        m.curve(shape, "envshape", 0.25, 4.0, true);
        m.curve(stereo, "stereo", 0.0, 180.0, false);
        m.warn("Modulation speed, shape and stereo curves are approximated. The original continuously variable LFO shape is not reproduced.");
        if source.is_some_and(|source| m.raw[source] == 1.0) {
            m.warn("The original synced speed choice has no verified division mapping; the four-beat LFO period was retained.");
        }
    }
    fn percent(m: &mut Mapper, index: usize, key: &str, signed: bool) {
        if signed {
            m.signed_percent(index, key)
        } else {
            m.percent(index, key)
        }
    }
    match kind {
        "Gate" => {
            if let Some(value) = m.value_in(0, f64::from(0.1f32), 1.0) {
                m.parameter("threshold", 40.0 * value.log10());
            }
            m.curve(1, "attack", 0.05, 100.0, true);
            m.curve(2, "hold", 0.0, 1000.0, false);
            m.curve(3, "release", 1.0, 2000.0, true);
            m.percent(4, "mix");
            m.warn("Gate Attack, Hold and Release display percentages in the original; their millisecond curves are approximated.");
        }
        "Limiter" => {
            // Serializer order differs from the interface: Threshold, Mode, Release, Ceiling.
            if let Some(value) = m.value_in(0, -40.0, 0.0) {
                m.parameter("threshold", value);
            }
            m.choice(1, "mode", 1);
            if let Some(value) = m.value(2) {
                m.parameter("release", single(0x402d_f854).powf(single(0x40c6_de12) * value).clamp(1.0, 500.0));
            }
            if let Some(value) = m.value(3) {
                m.parameter("ceiling", -40.0 + 39.7 * value);
            }
            m.warn("Original limiter side-chain routing and filter settings are stored separately and were not mapped by this helper.");
        }
        "Maximizer" => {
            m.curve(0, "amount", 0.0, 30.0, false);
            m.percent(1, "curve");
            m.choice(2, "turbo", 1);
            m.warn("Maximizer Amount is translated from its original percentage into an approximate drive in dB.");
        }
        "TransientDesigner" => {
            // The saved order starts with Gain, unlike the interface's Attack/Sustain/Gain.
            if let Some(value) = m.value_in(0, -1.0, 1.0) {
                m.parameter("gain", value * if value < 0.0 { 24.0 } else { 6.0 });
            }
            percent(&mut m, 1, "attack", true);
            percent(&mut m, 2, "sustain", true);
            m.choice(3, "limit", 1);
        }
        "CabinetEmulation" => {
            m.choice(0, "cabinet", 3);
            m.choice(1, "microphone", 5);
            m.percent(2, "distance");
            m.percent(3, "mix");
            m.warn("Cabinet and microphone names correspond to similar filter characters; the original cabinet responses are not included.");
        }
        "Chorus" => {
            m.curve(0, "rate", 0.03, 8.0, true);
            m.percent(1, "amount");
            m.percent(2, "mix");
            m.warn("Chorus Rate is translated from its original percentage using an approximate frequency curve.");
        }
        "FM" => {
            m.curve(0, "rate", 0.1, 10000.0, true);
            m.curve(1, "split", 20.0, 16000.0, true);
            m.percent(2, "contour");
            m.percent(3, "amount");
            m.warn("FM Rate and Split use approximate frequency curves; the original controls display percentages.");
        }
        "Filter" => {
            m.choice(0, "mode", 3);
            // The converter's endpoints are 46 and 12540 Hz.
            if let Some(value) = m.value(1) {
                m.parameter("cutoff", 46.0 * (12540.0f64 / 46.0).powf(value));
            }
            m.percent(2, "resonance");
            if let Some(value) = m.value_in(3, -1.0, 1.0) {
                m.parameter("amount", 5.0 * value);
            }
            m.choice(4, "source", 2);
            m.curve(5, "speed", 0.03, 16.0, true);
            // A continuously morphing source shape is not an enum: the sine default stays.
            m.value(6);
            m.curve(7, "phase", 0.0, 360.0, false);
            m.curve(8, "decay", 5.0, 2000.0, true);
            m.curve(9, "smooth", 0.1, 100.0, true);
            m.curve(10, "envshape", 0.25, 4.0, true);
            m.warn("Filter modulation depth, timing, phase and envelope curves are approximated. The original morphing LFO shape uses the SuperTracker sine default.");
            if raw[4] == 1.0 {
                m.warn("The original synced speed choice has no verified division mapping; the four-beat LFO period was retained.");
            }
        }
        "Flanger" => {
            m.curve(0, "frequency", 20.0, 5000.0, true);
            percent(&mut m, 1, "amount", true);
            modulation(&mut m, 2, 3, 4, None);
            m.percent(5, "feedback");
            m.choice(6, "invert", 1);
            m.percent(7, "mix");
            m.warn("Flanger frequency uses an approximate curve. Version 0 has no saved Source field and uses free LFO mode.");
        }
        "Phaser" => {
            m.curve(0, "frequency", 20.0, 10000.0, true);
            percent(&mut m, 1, "amount", true);
            modulation(&mut m, 3, 4, 5, Some(2));
            m.percent(6, "feedback");
            m.choice(7, "eightpole", 1);
            m.percent(8, "mix");
            m.warn("Phaser frequency uses an approximate curve.");
        }
        _ => {
            // Frequency Shifter: neither percentage control establishes a signed Hz law, so no
            // shift is invented on import.
            m.value(0);
            m.value(1);
            m.percent(2, "feedback");
            m.percent(3, "stereo");
            m.choice(4, "invert", 1);
            m.percent(5, "mix");
            m.warn("Frequency Shifter Coarse and Fine have no verified Hz conversion; both target shifts remain at zero. The original values are retained in import data.");
        }
    }
    Some(m.finish())
}

/// LoFi, Saturator and the Distortion (see `docs/research/maschine-distortion-parameter-mapping.md`).
pub fn map_distortion(kind: &str, version: u64, raw: &[f64]) -> Option<EffectMapping> {
    let kind = short_name(kind);
    let (id, expected_version, count) = match kind {
        "LoFi" => ("lofi", 0, 5),
        "Saturator" => ("saturator", 0, 14),
        "MullhollandDrive" => ("distortion", 1, 15),
        _ => return None,
    };
    if version != expected_version || raw.len() != count {
        return None;
    }
    let mut m = Mapper::new(kind, id, raw, "known")?;
    match kind {
        "LoFi" => {
            // The saved order is SR, Smooth, Bits, Stereo, Mix, unlike the interface.
            if let Some(value) = m.value(0) {
                m.parameter("sample_rate", 44100.0 * single(0x3f87_9c63).powf(-105.5 * value));
            }
            m.percent(1, "smooth");
            if let Some(value) = m.value(2) {
                m.parameter("bits", 12.0 - 11.0 * value);
            }
            m.percent(3, "stereo");
            m.percent(4, "mix");
            m.warn("LoFi Smooth and Stereo use corresponding percentage controls; the original filtering and stereo algorithms are approximated.");
        }
        "Saturator" => {
            // Each source mode saves its own settings; only the active mode is mapped onto the shared knobs.
            let Some(mode) = m.choice(0, "mode", 2) else { return Some(EffectMapping { parameters: Vec::new(), ..m.finish() }) };
            match mode {
                0 => {
                    if let Some(value) = m.value(1) {
                        m.parameter("input", 24.0 * value);
                    }
                    m.percent(2, "drive");
                    m.percent(3, "contour");
                    m.warn("Classic Input is a source percentage control; its conversion to input gain in dB is approximated.");
                }
                1 => {
                    if let Some(value) = m.value(4) {
                        m.parameter("input", 30.0 * value);
                    }
                    // The tape Drive gives -6..6 dB of low-frequency tilt.
                    if let Some(value) = m.value_in(5, -1.0, 1.0) {
                        m.parameter("drive", 50.0 + 25.0 * value);
                    }
                    if let Some(value) = m.value(6) {
                        m.parameter("contour", 100.0 * ((12000.0 + 8000.0 * value) / 1000.0).ln() / 20f64.ln());
                    }
                    m.warn("Tape Drive and Contour preserve the source's displayed dB and frequency using approximate low-frequency tilt and low-pass controls.");
                }
                _ => {
                    m.percent(7, "drive");
                    if let Some(value) = m.value_in(8, 0.0, 4.0) {
                        m.parameter("charge", 25.0 * value);
                    }
                    m.value(9);
                    m.value(10);
                    if let Some(value) = m.value_in(11, -12.0, 12.0) {
                        m.parameter("output", value);
                    }
                    if raw[12] == 0.0 || raw[12] == 1.0 {
                        m.parameter("overload", raw[12] * 12.0);
                    } else {
                        m.warn("Unknown overload choice; the SuperTracker default was retained.");
                    }
                    m.choice(13, "eq_bypass", 1);
                    m.warn("Tube Bass and Treble have no verified dB conversion and remain neutral; their original values are retained in import data. The Overload switch uses an approximate 12 dB boost.");
                }
            }
            m.warn("Inactive Saturator mode settings are retained in original import data; changing modes uses the shared SuperTracker controls.");
        }
        _ => {
            let Some(mode) = m.choice(0, "mode", 1) else { return Some(EffectMapping { parameters: Vec::new(), ..m.finish() }) };
            // Version 1 puts Mode and six Analog fields before the eight Fuzz fields.
            m.percent(1, "saturation");
            m.percent(2, "bass");
            m.choice(4, "focus", 1);
            m.percent(5, "definition");
            m.percent(8, "color");
            for (index, key, label) in [(9, "feedback", "Feedback"), (11, "tone_mod", "Tone Mod")] {
                if let Some(value) = m.value_in(index, -1.0, 1.0) {
                    if value >= 0.0 {
                        m.parameter(key, value * 100.0);
                    } else {
                        m.parameter(key, 0.0);
                        m.warn(format!("Negative {label} is unsupported by this processor and was disabled; its signed source value is retained in import data."));
                    }
                }
            }
            m.choice(12, "gate", 1);
            if let Some(value) = m.value(13) {
                let squared = value * value;
                let shaped = squared + (squared * squared - squared) * single(0x3f4f_41f2);
                m.parameter("gate_release", 1000.0 * (single(0x3c23_d70a) + shaped * single(0x409f_ae14)));
            }
            let tone = |m: &mut Mapper, index: usize| {
                if let Some(value) = m.value(index) {
                    m.parameter("tone", 150.0 * (16000.0f64 / 150.0).powf(value));
                }
            };
            if mode == 0 {
                if let Some(value) = m.value(7) {
                    m.parameter("drive", 42.0 * value);
                }
                tone(&mut m, 10);
                m.percent(14, "mix");
            } else {
                m.parameter("drive", 0.0);
                tone(&mut m, 3);
                m.percent(6, "mix");
            }
            m.parameter("output", 0.0);
            m.warn("Distortion Drive and Tone use approximate gain and frequency curves. Inactive mode values remain in original import data.");
        }
    }
    Some(m.finish())
}

/// A drum-synth module mapped onto one of the engine's drum models.
#[derive(Debug, Clone, PartialEq)]
pub struct DrumMapping {
    /// The model id: `kick`, `snare`, `hihat`, `tom`, `perc` or `cymbal`.
    pub drum_model: String,
    /// The model's eight normalized controls (tune, decay, tone, impact, noise, attack, drive, character).
    pub drum_parameters: [f64; 8],
    /// Which controls were set from the module; the others are the model's defaults.
    pub mapped_parameters: Vec<usize>,
    /// The module's engine selection.
    pub original_engine: i64,
    /// What is approximate or not converted.
    pub warnings: Vec<String>,
}

impl DrumMapping {
    /// The displayed physical value of control `parameter`.
    pub fn physical(&self, parameter: usize) -> f64 {
        catalogue::drum_model(&self.drum_model).and_then(|(_, model)| model.parameters.get(parameter)).map(|info| catalogue::physical(info, self.drum_parameters[parameter])).unwrap_or(0.0)
    }
}

/// Maps a drum-synth module (Kick, Snare, Hihat, Tom, Perc, Cymbal) onto its drum model. Only
/// controls with a verified unit are converted: the relative Tune offset in semitones, and the
/// shared Impact where the selected engine uses it. `None` for other versions and shapes.
pub fn map_drum(kind: &str, version: u64, raw: &[f64]) -> Option<DrumMapping> {
    let kind = short_name(kind);
    let (expected_version, count, model_id, engines): (u64, usize, &str, i64) = match kind {
        "Kick" => (2, 31, "kick", 9),
        "Snare" => (1, 22, "snare", 9),
        "Hihat" => (2, 18, "hihat", 2),
        "Tom" => (1, 23, "tom", 3),
        "Perc" => (1, 29, "perc", 2),
        "Cymbal" => (0, 13, "cymbal", 1),
        _ => return None,
    };
    if version != expected_version || raw.len() != count || !raw.iter().all(|value| value.is_finite()) || raw[0].trunc() != raw[0] || raw[0] < 0.0 || raw[0] > f64::from(i32::MAX) {
        return None;
    }
    let (_, model) = catalogue::drum_model(model_id)?;
    let engine = raw[0] as i64;
    let mut values = [0.0; 8];
    for (index, parameter) in model.parameters.iter().enumerate().take(8) {
        values[index] = catalogue::normalized(parameter, parameter.default);
    }
    let mut mapped: Vec<usize> = Vec::new();
    let mut warnings = vec![format!("{kind} engine {engine} uses an independent SuperTracker drum model; timbre, base pitch and velocity response are approximations.")];
    let value = |index: usize, warnings: &mut Vec<String>| -> Option<f64> {
        if !(0.0..=1.0).contains(&raw[index]) {
            warnings.push(format!("Original {kind} parameter {} is outside its verified range; the corresponding default was retained.", index + 1));
            return None;
        }
        Some(raw[index])
    };
    let mut set = |target: usize, physical: f64, warnings: &mut Vec<String>| {
        let Some(info) = model.parameters.get(target) else { return };
        if physical < info.minimum || physical > info.maximum {
            warnings.push(format!("{} exceeds the SuperTracker range and was limited to its supported range.", info.name));
        }
        values[target] = catalogue::normalized(info, physical);
        mapped.push(target);
    };
    if (0..=engines).contains(&engine) {
        // The Tune display's slope per engine: the semitone offset from the control's midpoint
        // is kept; the model's base tone is its own.
        let span: Option<f64> = match (kind, engine) {
            ("Kick", 0 | 1 | 3 | 4 | 5) => Some(24.0),
            ("Kick", 7) => Some(12.0),
            ("Snare", 0 | 4 | 6 | 7 | 9) => Some(24.0),
            ("Snare", 5) => Some(18.0),
            ("Tom", 0 | 1) => Some(24.0),
            ("Tom", 2 | 3) => Some(14.0),
            ("Perc", 0) => Some(48.0),
            ("Perc", 1) => Some(24.0),
            _ => None,
        };
        match span.and_then(|span| value(1, &mut warnings).map(|tune| span * (tune - 0.5))) {
            Some(semitones) => {
                set(0, semitones, &mut warnings);
                warnings.push("Tune preserves the verified relative semitone offset around the original control midpoint, not the original engine's absolute pitch.".to_string());
            }
            None => warnings.push("This engine's Tune has no verified semitone conversion; the SuperTracker Tune default was retained.".to_string()),
        }
        // The shared Impact is inactive in some engines (Sub Kick, electronic Snares, Shaker):
        // a saved value of an inactive control is never imported.
        let impact_index: Option<usize> = match kind {
            "Kick" => (engine > 0).then_some(4),
            "Snare" => (2..=7).contains(&engine).then_some(4),
            "Tom" => Some(4),
            "Perc" => (engine < 2).then_some(26),
            "Cymbal" => Some(3),
            _ => None, // The Hihat's two Strike controls are distinct.
        };
        if let Some(impact) = impact_index.and_then(|index| value(index, &mut warnings)) {
            set(3, impact * 100.0, &mut warnings);
        }
    } else {
        warnings.push("Unknown original engine selection; all SuperTracker drum defaults were retained.".to_string());
    }
    warnings.push("Original Decay is an engine-specific percentage with no verified millisecond conversion. Decay and other unmapped controls retain SuperTracker defaults; original parameter data must remain in the import metadata.".to_string());
    mapped.sort_unstable();
    Some(DrumMapping { drum_model: model_id.to_string(), drum_parameters: values, mapped_parameters: mapped, original_engine: engine, warnings })
}

/// The native level law of mixer faders and zone gains as an amplitude: 0.75 is unity and 1 is
/// +10 dB (a fifth-order curve, not a fourth-power fader). `None` outside 0..=1.
pub fn level_amplitude(raw: f64) -> Option<f64> {
    if !raw.is_finite() || !(0.0..=1.0).contains(&raw) {
        return None;
    }
    let value = raw as f32;
    let square = value * value;
    let fourth = square * (square * f32::from_bits(0x4019_4494));
    Some(f64::from(square.mul_add(f32::from_bits(0x3f44_78b8), fourth * value)))
}

fn lookup(value: f64, table: &[f32; 128]) -> f32 {
    let scaled = (value as f32 * 127.0).clamp(0.0, 127.0);
    let lower = scaled as usize;
    if lower == 127 {
        return table[lower];
    }
    (table[lower + 1] - table[lower]).mul_add(scaled - lower as f32, table[lower])
}

fn envelope_seconds(value: f64, curve: &[f32; 128], decay: bool) -> f64 {
    let exponent = lookup(value, curve).mul_add(f32::from_bits(if decay { 0x4116_efe0 } else { 0x410e_c3c2 }), f32::from_bits(0x3f31_7218));
    (f64::from(exponent.exp() - 2.0) / 1000.0).max(0.0)
}

fn cutoff_hertz(value: f64, highpass: bool) -> f64 {
    // The source's eight-term exponential approximation, not an assumed log-frequency law.
    let x = lookup(value, &curves::CUTOFF) * f32::from_bits(0x40c4_cccd);
    let mut polynomial = 1.0 + x;
    let mut power = x * x;
    for coefficient in [0.5, f32::from_bits(0x3e2a_aaab), f32::from_bits(0x3d2a_aaab), f32::from_bits(0x3c08_8889), f32::from_bits(0x3ab6_0b61), f32::from_bits(0x3950_0d01), f32::from_bits(0x37d0_0d01)] {
        polynomial = power.mul_add(coefficient, polynomial);
        power *= x;
    }
    let normalized = ((polynomial - 1.0) / f32::from_bits(0x43c2_56a2)).max(0.0);
    if normalized > 1.0 {
        return f64::from(f32::from_bits(if highpass { 0x4691_ce3e } else { 0x46aa_8b00 }));
    }
    let base = (normalized + f32::from_bits(0x3b03_126f)) * 44100.0 * 0.5;
    let scale = f32::from_bits(if highpass { 0x3f6b_851f } else { 0x3f7d_70a4 });
    f64::from(if highpass { base * scale * scale } else { base * scale })
}

/// The Sampler's controls mapped onto a sampler instrument.
#[derive(Debug, Clone, PartialEq)]
pub struct SamplerMapping {
    /// Instrument fields to set: `tune`, `attack`, `decay`, `sustain`, `release`,
    /// `filterAttack` ..., `filterType`, `cutoff`, `resonance`.
    pub instrument: Map<String, Value>,
    /// The voice mode (`poly`, `mono`, `legato`), a setting of the track that plays the
    /// instrument. `None` when the control was not read.
    pub voice_mode: Option<&'static str>,
    /// Recovered physical values (seconds, semitones, Hz), also of controls that are not applied.
    pub physical: BTreeMap<String, f64>,
    /// What is approximate or not converted.
    pub warnings: Vec<String>,
    /// The raw controls as read; non-finite values as text.
    pub source_parameters: Map<String, Value>,
}

/// Maps the Sampler's own controls (see [`super::archive::Archive::sampler_parameters`]; the
/// evidence is `docs/research/maschine-sampler-parameter-evidence.json`). Times and cutoff are
/// recovered physical values; the envelope and filter that play them are SuperTracker's own.
/// Controls without a verified equivalent stay in `source_parameters` and are never guessed.
pub fn map_sampler(raw: &BTreeMap<String, f64>) -> SamplerMapping {
    let mut fields: Map<String, Value> = Map::new();
    let mut voice_mode = None;
    let mut physical: BTreeMap<String, f64> = BTreeMap::new();
    let mut warnings: Vec<String> = Vec::new();
    fn warn(warnings: &mut Vec<String>, text: impl Into<String>) {
        let text = text.into();
        if !warnings.contains(&text) {
            warnings.push(text);
        }
    }
    let value = |key: &str, low: f64, high: f64, warnings: &mut Vec<String>| -> Option<f64> {
        let value = *raw.get(key)?;
        if !value.is_finite() || value < low || value > high {
            warn(warnings, format!("Sampler control {key} is outside its verified range; the destination default was retained."));
            return None;
        }
        Some(value)
    };
    let nonzero = |key: &str| raw.get(key).is_some_and(|value| value.is_finite() && value.abs() > 0.000_001);
    if let Some(tune) = value("0xf8", -1.0, 1.0, &mut warnings) {
        fields.insert("tune".to_string(), json!(36.0 * tune));
        physical.insert("tuneSemitones".to_string(), 36.0 * tune);
    }
    if let Some(voices) = value("0xc8", 0.0, 7.0, &mut warnings).filter(|voices| voices.round() == *voices) {
        voice_mode = Some(if voices == 0.0 { "legato" } else if voices == 1.0 { "mono" } else { "poly" });
        if voices > 1.0 {
            warn(&mut warnings, "Sampler polyphony uses SuperTracker's voice pool; the original per-Sound voice limit is retained in the import data.");
        }
    }
    if nonzero("0xe0") {
        warn(&mut warnings, "The original vintage sampler engine mode is not reproduced; standard sample playback is used.");
    }
    if nonzero("0xf0") {
        warn(&mut warnings, "Sampler Reverse is retained but not applied: SuperTracker's reverse mode also loops, unlike a reversed one-shot.");
    }
    if let Some(start) = value("0x100", 0.0, 1.0, &mut warnings).filter(|start| *start > 0.0) {
        physical.insert("startOffsetFraction".to_string(), start * start);
        warn(&mut warnings, "The sampler-wide Start offset is retained; sample-zone trim positions are preserved without guessing how to combine them.");
    }
    if nonzero("0xd0") {
        warn(&mut warnings, "Sampler Glide is retained but was not mapped to a persistent portamento control.");
    }
    if nonzero("0xd8") {
        warn(&mut warnings, "The sampler Pitchbend range is retained; the original MIDI bend range is not reproduced.");
    }
    let envelope_mode = value("0xe8", 0.0, 2.0, &mut warnings);
    for (prefix, destination) in [("ampEnvelope", ""), ("filterEnvelope", "filter")] {
        let amp = prefix == "ampEnvelope";
        let stages: [(usize, &str, &[f32; 128]); 4] = [(0, "Attack", &curves::ATTACK), (1, "Hold", &curves::HOLD), (2, "Decay", &curves::DECAY), (4, "Release", &curves::DECAY)];
        for (index, field, curve) in stages {
            let Some(position) = value(&format!("{prefix}.{index}"), 0.0, 1.0, &mut warnings) else { continue };
            let seconds = envelope_seconds(position, curve, index == 2 || index == 4);
            physical.insert(format!("{prefix}{field}Seconds"), seconds);
            if index == 1 {
                if seconds > 0.000_001 && (!amp || envelope_mode != Some(0.0)) {
                    warn(&mut warnings, format!("{prefix} Hold has no matching ADSR stage and was retained without applying it."));
                }
            } else if !amp || envelope_mode == Some(1.0) || envelope_mode == Some(2.0) {
                fields.insert(if destination.is_empty() { field.to_lowercase() } else { format!("{destination}{field}") }, json!(seconds));
            }
        }
        if let Some(sustain) = value(&format!("{prefix}.3"), 0.0, 1.0, &mut warnings) {
            let amplitude = sustain * sustain * sustain;
            physical.insert(format!("{prefix}Sustain"), amplitude);
            if !amp || envelope_mode == Some(2.0) {
                fields.insert(if destination.is_empty() { "sustain".to_string() } else { "filterSustain".to_string() }, json!(amplitude));
            }
        }
        if (5..=8).any(|index| nonzero(&format!("{prefix}.{index}"))) {
            warn(&mut warnings, format!("{prefix} modulation to pitch, cutoff, drive or pan is retained; no unverified modulation-depth conversion was applied."));
        }
    }
    match envelope_mode.map(|mode| mode as i64) {
        Some(0) => {
            fields.insert("attack".to_string(), json!(0.0));
            fields.insert("decay".to_string(), json!(0.0));
            fields.insert("sustain".to_string(), json!(1.0));
            warn(&mut warnings, "One-shot envelope timing follows SuperTracker's note gates; Maschine's key-release-independent behavior is not reproduced.");
        }
        Some(1) => {
            fields.insert("sustain".to_string(), json!(0.0));
            warn(&mut warnings, "AHD is approximated by an AD envelope; Hold and key-release behavior differ.");
        }
        Some(2) => warn(&mut warnings, "Sampler ADSR uses recovered physical stage times and sustain, with SuperTracker's independent envelope curves."),
        _ if raw.contains_key("0xe8") => warn(&mut warnings, "Unknown sampler envelope type; the destination envelope was retained."),
        _ => {}
    }
    if let Some(mode) = value("0x148", 0.0, 4.0, &mut warnings).filter(|mode| mode.round() == *mode) {
        if mode == 0.0 {
            fields.insert("cutoff".to_string(), json!(22000.0));
            fields.insert("resonance".to_string(), json!(0.0));
            fields.insert("filterType".to_string(), json!("lowpass"));
            warn(&mut warnings, "Sampler Filter Off is approximated by SuperTracker's fully open low-pass filter.");
        } else if mode == 1.0 || mode == 3.0 {
            fields.insert("filterType".to_string(), json!(if mode == 1.0 { "lowpass" } else { "highpass" }));
            if let Some(position) = value("0x150", 0.0, 1.0, &mut warnings) {
                let hertz = cutoff_hertz(position, mode == 3.0);
                fields.insert("cutoff".to_string(), json!(hertz.clamp(20.0, 22000.0)));
                physical.insert("cutoffHertz".to_string(), hertz);
            }
            if let Some(resonance) = value("0x158", 0.0, 1.0, &mut warnings) {
                fields.insert("resonance".to_string(), json!(resonance.min(0.99)));
                if resonance > 0.0 {
                    warn(&mut warnings, "Filter resonance keeps its normalized amount as an approximation; the original filter's resonance response differs.");
                }
            }
            warn(&mut warnings, "Sampler filter cutoff follows the original physical display value; SuperTracker uses an independent filter implementation.");
        } else {
            warn(&mut warnings, "Sampler band-pass/EQ mode has no verified equivalent cutoff mapping and was retained without applying it.");
        }
    }
    for (key, name) in [("0x128", "Compressor"), ("0x130", "Drive"), ("0x138", "sample-rate reduction"), ("0x140", "bit reduction")] {
        if nonzero(key) {
            warn(&mut warnings, format!("Sampler {name} is retained but its processing/control conversion was not verified."));
        }
    }
    if ["0x178", "0x180", "0x188", "0x198", "0x1a0", "0x1a8", "0x1b0"].iter().any(|key| nonzero(key)) || raw.get("0x190").is_some_and(|value| *value != 1.0) {
        warn(&mut warnings, "Sampler velocity/mod-wheel routes are retained; SuperTracker's standard note-velocity behavior is used.");
    }
    if raw.values().any(|value| !value.is_finite()) {
        warn(&mut warnings, "Nonfinite sampler source controls were preserved as diagnostic strings and were not applied.");
    }
    let source_parameters = raw.iter().map(|(key, value)| (key.clone(), if value.is_finite() { json!(value) } else { json!(format!("{value}")) })).collect();
    SamplerMapping { instrument: fields, voice_mode, physical, warnings, source_parameters }
}

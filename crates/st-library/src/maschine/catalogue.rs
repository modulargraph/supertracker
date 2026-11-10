//! The built-in effects and drum models that Maschine modules are mapped onto: their persistent
//! ids, their parameter ids in address order, and each parameter's range, default and scaling.
//!
//! This is a copy of the engine's catalogue (`tracker-engine`, `effects::capi`, declared in
//! `Sources/TrackerEngine/include/STBuiltinEffects.h`): the tables below were generated from
//! it and the ids are carried over exactly. This crate does not depend on the engine, so the
//! mapping code reads ranges and scaling from here; `normalized` and `physical` are the
//! engine's conversions between displayed (physical) and normalized 0..1 values.
//!
//! When an effect gains, loses or reorders parameters in the engine, regenerate these tables
//! (the host can assert equality at start-up, since it links both crates).

/// The parameter scales logarithmically between its bounds.
pub const LOG: u32 = 1;
/// The parameter takes whole numbers.
pub const INTEGER: u32 = 2;
/// The parameter is a switch.
pub const TOGGLE: u32 = 4;

/// One parameter of an effect or drum model. Ranges and the default are displayed physical values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Parameter {
    /// Persistent id.
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// Smallest value.
    pub minimum: f64,
    /// Largest value.
    pub maximum: f64,
    /// Default value.
    pub default: f64,
    /// [`LOG`], [`INTEGER`], [`TOGGLE`].
    pub flags: u32,
}

/// A built-in effect or drum model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Effect {
    /// Persistent id (`eq`, `compressor`, `kick` ...). Components name effects as `stfx:<id>`.
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// Parameters in address order.
    pub parameters: &'static [Parameter],
}

impl Effect {
    /// The address and description of the parameter `id`.
    pub fn parameter(&self, id: &str) -> Option<(usize, &'static Parameter)> {
        self.parameters.iter().enumerate().find(|(_, parameter)| parameter.id == id)
    }
}

/// The effect with the persistent id `id`.
pub fn effect(id: &str) -> Option<&'static Effect> {
    EFFECTS.iter().find(|effect| effect.id == id)
}

/// The drum model with the id `id` and its model number (1..=6: kick, snare, hihat, tom, perc, cymbal).
pub fn drum_model(id: &str) -> Option<(usize, &'static Effect)> {
    DRUM_MODELS.iter().enumerate().find(|(_, model)| model.id == id).map(|(index, model)| (index + 1, model))
}

/// Displayed physical value to normalized 0..1. Values outside the range are limited; a
/// non-finite value maps the default.
pub fn normalized(parameter: &Parameter, value: f64) -> f64 {
    let mut value = if value.is_finite() { value } else { parameter.default }.clamp(parameter.minimum, parameter.maximum);
    if parameter.flags & (INTEGER | TOGGLE) != 0 {
        value = value.round();
    }
    if parameter.maximum == parameter.minimum {
        return 0.0;
    }
    let position = if parameter.flags & LOG != 0 && parameter.minimum > 0.0 {
        (value / parameter.minimum).ln() / (parameter.maximum / parameter.minimum).ln()
    } else {
        (value - parameter.minimum) / (parameter.maximum - parameter.minimum)
    };
    position.clamp(0.0, 1.0)
}

/// Normalized 0..1 to displayed physical value.
pub fn physical(parameter: &Parameter, normalized: f64) -> f64 {
    let position = if normalized.is_finite() { normalized.clamp(0.0, 1.0) } else { 0.0 };
    let mut value = if parameter.flags & LOG != 0 && parameter.minimum > 0.0 {
        parameter.minimum * (parameter.maximum / parameter.minimum).powf(position)
    } else {
        parameter.minimum + (parameter.maximum - parameter.minimum) * position
    };
    if parameter.flags & (INTEGER | TOGGLE) != 0 {
        value = value.round();
    }
    value.clamp(parameter.minimum, parameter.maximum)
}

// Generated from the engine's catalogue (tracker-engine `effects::capi`); see the module documentation.
pub(crate) const EFFECTS: &[Effect] = &[
    Effect { id: "compressor", name: "Compressor", parameters: &[
        Parameter { id: "mode", name: "Mode", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 2 },
        Parameter { id: "threshold", name: "Threshold", minimum: -60.0, maximum: 0.0, default: -18.0, flags: 0 },
        Parameter { id: "ratio", name: "Ratio", minimum: 1.0, maximum: 20.0, default: 4.0, flags: 1 },
        Parameter { id: "knee", name: "Knee", minimum: 0.0, maximum: 24.0, default: 6.0, flags: 0 },
        Parameter { id: "attack", name: "Attack", minimum: 0.1, maximum: 200.0, default: 10.0, flags: 1 },
        Parameter { id: "release", name: "Release", minimum: 5.0, maximum: 2000.0, default: 100.0, flags: 1 },
        Parameter { id: "gain", name: "Gain", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 100.0, flags: 0 },
        Parameter { id: "scgain", name: "Detector gain", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "scfilter", name: "Detector filter", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "sccenter", name: "Detector frequency", minimum: 20.0, maximum: 20000.0, default: 632.5, flags: 1 },
        Parameter { id: "scwidth", name: "Detector width", minimum: 0.25, maximum: 6.0, default: 3.0, flags: 1 },
    ] },
    Effect { id: "gate", name: "Gate", parameters: &[
        Parameter { id: "threshold", name: "Threshold", minimum: -80.0, maximum: 0.0, default: -36.0, flags: 0 },
        Parameter { id: "attack", name: "Attack", minimum: 0.05, maximum: 100.0, default: 1.0, flags: 1 },
        Parameter { id: "hold", name: "Hold", minimum: 0.0, maximum: 1000.0, default: 30.0, flags: 0 },
        Parameter { id: "release", name: "Release", minimum: 1.0, maximum: 2000.0, default: 80.0, flags: 1 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 100.0, flags: 0 },
        Parameter { id: "scgain", name: "Detector gain", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "scfilter", name: "Detector filter", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "sccenter", name: "Detector frequency", minimum: 20.0, maximum: 20000.0, default: 632.5, flags: 1 },
        Parameter { id: "scwidth", name: "Detector width", minimum: 0.25, maximum: 6.0, default: 3.0, flags: 1 },
    ] },
    Effect { id: "transient", name: "Transient Shaper", parameters: &[
        Parameter { id: "gain", name: "Input gain", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "attack", name: "Attack", minimum: -100.0, maximum: 100.0, default: 0.0, flags: 0 },
        Parameter { id: "sustain", name: "Sustain", minimum: -100.0, maximum: 100.0, default: 0.0, flags: 0 },
        Parameter { id: "limit", name: "Limit", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
    ] },
    Effect { id: "limiter", name: "Peak Limiter", parameters: &[
        Parameter { id: "mode", name: "Mode", minimum: 0.0, maximum: 1.0, default: 1.0, flags: 2 },
        Parameter { id: "threshold", name: "Threshold", minimum: -40.0, maximum: 0.0, default: 0.0, flags: 0 },
        Parameter { id: "release", name: "Release", minimum: 1.0, maximum: 500.0, default: 50.0, flags: 1 },
        Parameter { id: "ceiling", name: "Ceiling", minimum: -40.0, maximum: -0.3, default: -0.3, flags: 0 },
        Parameter { id: "scgain", name: "Detector gain", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "scfilter", name: "Detector filter", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "sccenter", name: "Detector frequency", minimum: 20.0, maximum: 20000.0, default: 632.5, flags: 1 },
        Parameter { id: "scwidth", name: "Detector width", minimum: 0.25, maximum: 6.0, default: 3.0, flags: 1 },
    ] },
    Effect { id: "maximizer", name: "Maximizer", parameters: &[
        Parameter { id: "amount", name: "Amount", minimum: 0.0, maximum: 30.0, default: 3.0, flags: 0 },
        Parameter { id: "curve", name: "Curve", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "turbo", name: "Turbo", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "scgain", name: "Detector gain", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "scfilter", name: "Detector filter", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "sccenter", name: "Detector frequency", minimum: 20.0, maximum: 20000.0, default: 632.5, flags: 1 },
        Parameter { id: "scwidth", name: "Detector width", minimum: 0.25, maximum: 6.0, default: 3.0, flags: 1 },
    ] },
    Effect { id: "eq", name: "Four-band EQ", parameters: &[
        Parameter { id: "lowfreq", name: "Low frequency", minimum: 20.0, maximum: 8000.0, default: 100.0, flags: 1 },
        Parameter { id: "lowgain", name: "Low gain", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "lowmidfreq", name: "Low-mid frequency", minimum: 40.0, maximum: 16000.0, default: 500.0, flags: 1 },
        Parameter { id: "lowmidgain", name: "Low-mid gain", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "highmidfreq", name: "High-mid frequency", minimum: 40.0, maximum: 16000.0, default: 2500.0, flags: 1 },
        Parameter { id: "highmidgain", name: "High-mid gain", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "highfreq", name: "High frequency", minimum: 50.0, maximum: 20000.0, default: 8000.0, flags: 1 },
        Parameter { id: "highgain", name: "High gain", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "lowmidwidth", name: "Low-mid width", minimum: 0.1, maximum: 6.0, default: 2.0, flags: 1 },
        Parameter { id: "highmidwidth", name: "High-mid width", minimum: 0.1, maximum: 6.0, default: 2.0, flags: 1 },
        Parameter { id: "gain", name: "Output gain", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
    ] },
    Effect { id: "filter", name: "Multimode Filter", parameters: &[
        Parameter { id: "mode", name: "Mode", minimum: 0.0, maximum: 3.0, default: 0.0, flags: 2 },
        Parameter { id: "cutoff", name: "Cutoff", minimum: 20.0, maximum: 20000.0, default: 16000.0, flags: 1 },
        Parameter { id: "resonance", name: "Resonance", minimum: 0.0, maximum: 100.0, default: 0.0, flags: 0 },
        Parameter { id: "amount", name: "Modulation", minimum: -5.0, maximum: 5.0, default: 0.0, flags: 0 },
        Parameter { id: "source", name: "Modulation source", minimum: 0.0, maximum: 2.0, default: 0.0, flags: 2 },
        Parameter { id: "speed", name: "LFO speed", minimum: 0.03, maximum: 16.0, default: 1.0, flags: 1 },
        Parameter { id: "beats", name: "LFO period", minimum: 0.125, maximum: 64.0, default: 4.0, flags: 1 },
        Parameter { id: "shape", name: "LFO shape", minimum: 0.0, maximum: 3.0, default: 0.0, flags: 2 },
        Parameter { id: "phase", name: "LFO phase", minimum: 0.0, maximum: 360.0, default: 0.0, flags: 0 },
        Parameter { id: "decay", name: "Envelope decay", minimum: 5.0, maximum: 2000.0, default: 200.0, flags: 1 },
        Parameter { id: "smooth", name: "Envelope smooth", minimum: 0.1, maximum: 100.0, default: 5.0, flags: 1 },
        Parameter { id: "envshape", name: "Envelope shape", minimum: 0.25, maximum: 4.0, default: 1.0, flags: 1 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 100.0, flags: 0 },
        Parameter { id: "scgain", name: "Detector gain", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "scfilter", name: "Detector filter", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "sccenter", name: "Detector frequency", minimum: 20.0, maximum: 20000.0, default: 632.5, flags: 1 },
        Parameter { id: "scwidth", name: "Detector width", minimum: 0.25, maximum: 6.0, default: 3.0, flags: 1 },
    ] },
    Effect { id: "cabinet", name: "Cabinet", parameters: &[
        Parameter { id: "cabinet", name: "Cabinet", minimum: 0.0, maximum: 3.0, default: 1.0, flags: 2 },
        Parameter { id: "microphone", name: "Microphone", minimum: 0.0, maximum: 5.0, default: 0.0, flags: 2 },
        Parameter { id: "distance", name: "Distance", minimum: 0.0, maximum: 100.0, default: 0.0, flags: 0 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 100.0, flags: 0 },
    ] },
    Effect { id: "chorus", name: "Chorus", parameters: &[
        Parameter { id: "rate", name: "Rate", minimum: 0.03, maximum: 8.0, default: 0.8, flags: 1 },
        Parameter { id: "amount", name: "Amount", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 35.0, flags: 0 },
        Parameter { id: "stereo", name: "Stereo", minimum: 0.0, maximum: 180.0, default: 90.0, flags: 0 },
    ] },
    Effect { id: "flanger", name: "Flanger", parameters: &[
        Parameter { id: "frequency", name: "Frequency", minimum: 20.0, maximum: 5000.0, default: 500.0, flags: 1 },
        Parameter { id: "feedback", name: "Feedback", minimum: -95.0, maximum: 95.0, default: 35.0, flags: 0 },
        Parameter { id: "invert", name: "Invert", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "amount", name: "Amount", minimum: -100.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "source", name: "Modulation source", minimum: 0.0, maximum: 2.0, default: 0.0, flags: 2 },
        Parameter { id: "speed", name: "LFO speed", minimum: 0.03, maximum: 8.0, default: 0.3, flags: 1 },
        Parameter { id: "beats", name: "LFO period", minimum: 0.25, maximum: 64.0, default: 4.0, flags: 1 },
        Parameter { id: "envshape", name: "Envelope shape", minimum: 0.25, maximum: 4.0, default: 1.0, flags: 1 },
        Parameter { id: "stereo", name: "Stereo", minimum: 0.0, maximum: 180.0, default: 90.0, flags: 0 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
    ] },
    Effect { id: "fm", name: "Audio FM", parameters: &[
        Parameter { id: "rate", name: "Rate", minimum: 0.1, maximum: 10000.0, default: 100.0, flags: 1 },
        Parameter { id: "split", name: "Split", minimum: 20.0, maximum: 16000.0, default: 500.0, flags: 1 },
        Parameter { id: "contour", name: "Contour", minimum: 0.0, maximum: 100.0, default: 0.0, flags: 0 },
        Parameter { id: "amount", name: "Amount", minimum: 0.0, maximum: 100.0, default: 30.0, flags: 0 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 100.0, flags: 0 },
    ] },
    Effect { id: "freqshifter", name: "Frequency Shifter", parameters: &[
        Parameter { id: "coarse", name: "Coarse", minimum: -5000.0, maximum: 5000.0, default: 0.0, flags: 0 },
        Parameter { id: "fine", name: "Fine", minimum: -10.0, maximum: 10.0, default: 0.0, flags: 0 },
        Parameter { id: "feedback", name: "Feedback", minimum: -95.0, maximum: 95.0, default: 0.0, flags: 0 },
        Parameter { id: "stereo", name: "Stereo", minimum: 0.0, maximum: 100.0, default: 0.0, flags: 0 },
        Parameter { id: "invert", name: "Invert", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 100.0, flags: 0 },
    ] },
    Effect { id: "phaser", name: "Phaser", parameters: &[
        Parameter { id: "frequency", name: "Frequency", minimum: 20.0, maximum: 10000.0, default: 600.0, flags: 1 },
        Parameter { id: "feedback", name: "Feedback", minimum: -95.0, maximum: 95.0, default: 30.0, flags: 0 },
        Parameter { id: "eightpole", name: "Eight poles", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "amount", name: "Amount", minimum: -100.0, maximum: 100.0, default: 60.0, flags: 0 },
        Parameter { id: "source", name: "Modulation source", minimum: 0.0, maximum: 2.0, default: 0.0, flags: 2 },
        Parameter { id: "speed", name: "LFO speed", minimum: 0.03, maximum: 8.0, default: 0.4, flags: 1 },
        Parameter { id: "beats", name: "LFO period", minimum: 0.25, maximum: 64.0, default: 4.0, flags: 1 },
        Parameter { id: "envshape", name: "Envelope shape", minimum: 0.25, maximum: 4.0, default: 1.0, flags: 1 },
        Parameter { id: "stereo", name: "Stereo", minimum: 0.0, maximum: 180.0, default: 90.0, flags: 0 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
    ] },
    Effect { id: "ice", name: "Ice", parameters: &[
        Parameter { id: "color", name: "Color", minimum: 0.0, maximum: 100.0, default: 65.0, flags: 0 },
        Parameter { id: "ice", name: "Ice", minimum: 0.0, maximum: 100.0, default: 35.0, flags: 0 },
        Parameter { id: "size", name: "Size", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 30.0, flags: 0 },
    ] },
    Effect { id: "metaverb", name: "Metaverb", parameters: &[
        Parameter { id: "size", name: "Size", minimum: 0.0, maximum: 100.0, default: 45.0, flags: 0 },
        Parameter { id: "low", name: "Low", minimum: -24.0, maximum: 12.0, default: 0.0, flags: 0 },
        Parameter { id: "high", name: "High", minimum: -24.0, maximum: 12.0, default: 0.0, flags: 0 },
        Parameter { id: "pan", name: "Dry Pan", minimum: -100.0, maximum: 100.0, default: 0.0, flags: 0 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 30.0, flags: 0 },
    ] },
    Effect { id: "reflex", name: "Reflex", parameters: &[
        Parameter { id: "color", name: "Color", minimum: 0.0, maximum: 100.0, default: 60.0, flags: 0 },
        Parameter { id: "smooth", name: "Smooth", minimum: 0.0, maximum: 100.0, default: 45.0, flags: 0 },
        Parameter { id: "size", name: "Size", minimum: 0.0, maximum: 100.0, default: 25.0, flags: 0 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 30.0, flags: 0 },
    ] },
    Effect { id: "reverblegacy", name: "Reverb Legacy", parameters: &[
        Parameter { id: "room", name: "Room", minimum: 0.0, maximum: 3.0, default: 0.0, flags: 2 },
        Parameter { id: "size", name: "Size", minimum: 0.0, maximum: 100.0, default: 40.0, flags: 0 },
        Parameter { id: "low", name: "Low", minimum: -24.0, maximum: 12.0, default: 0.0, flags: 0 },
        Parameter { id: "high", name: "High", minimum: -24.0, maximum: 12.0, default: 0.0, flags: 0 },
        Parameter { id: "pan", name: "Dry Pan", minimum: -100.0, maximum: 100.0, default: 0.0, flags: 0 },
        Parameter { id: "stereo", name: "Stereo", minimum: 0.0, maximum: 150.0, default: 100.0, flags: 0 },
        Parameter { id: "freeze", name: "Freeze", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 30.0, flags: 0 },
    ] },
    Effect { id: "reverb", name: "Reverb", parameters: &[
        Parameter { id: "mode", name: "Mode", minimum: 0.0, maximum: 2.0, default: 0.0, flags: 2 },
        Parameter { id: "time", name: "Reverb Time", minimum: 0.5, maximum: 20.2, default: 1.0, flags: 1 },
        Parameter { id: "size", name: "Size", minimum: 0.0, maximum: 100.0, default: 20.0, flags: 0 },
        Parameter { id: "damping", name: "Damping", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "modulation", name: "Modulation", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "diffusion", name: "Diffusion / Softness", minimum: 0.0, maximum: 100.0, default: 80.0, flags: 0 },
        Parameter { id: "predelay", name: "Pre Delay", minimum: 0.0, maximum: 250.0, default: 0.0, flags: 0 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "highcut", name: "High Cut", minimum: 2000.0, maximum: 20000.0, default: 20000.0, flags: 1 },
        Parameter { id: "lowshelf", name: "Low Shelf", minimum: -12.0, maximum: 0.0, default: 0.0, flags: 0 },
    ] },
    Effect { id: "plate", name: "Plate Reverb", parameters: &[
        Parameter { id: "decay", name: "Decay", minimum: 0.2, maximum: 20.2, default: 2.0, flags: 1 },
        Parameter { id: "lowshelf", name: "Low Shelf", minimum: -24.0, maximum: 6.0, default: 0.0, flags: 0 },
        Parameter { id: "highdamp", name: "High Damp", minimum: 0.0, maximum: 100.0, default: 45.0, flags: 0 },
        Parameter { id: "predelay", name: "Pre Delay", minimum: 0.0, maximum: 250.0, default: 0.0, flags: 0 },
        Parameter { id: "highcut", name: "High Cut", minimum: 2000.0, maximum: 20000.0, default: 16000.0, flags: 1 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 35.0, flags: 0 },
    ] },
    Effect { id: "beatdelay", name: "Beat Delay", parameters: &[
        Parameter { id: "time", name: "Time", minimum: 0.0, maximum: 31.0, default: 7.0, flags: 2 },
        Parameter { id: "offset", name: "Offset", minimum: -49.0, maximum: 100.0, default: 0.0, flags: 0 },
        Parameter { id: "feedback", name: "Feedback", minimum: 0.0, maximum: 97.0, default: 35.0, flags: 0 },
        Parameter { id: "crossover", name: "Crossover", minimum: 0.0, maximum: 100.0, default: 0.0, flags: 0 },
        Parameter { id: "color", name: "Color", minimum: 0.0, maximum: 100.0, default: 75.0, flags: 0 },
        Parameter { id: "split", name: "Split", minimum: 0.0, maximum: 100.0, default: 0.0, flags: 0 },
        Parameter { id: "stereo", name: "Stereo", minimum: -100.0, maximum: 100.0, default: 100.0, flags: 0 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 30.0, flags: 0 },
        Parameter { id: "unit", name: "Unit", minimum: 0.0, maximum: 7.0, default: 2.0, flags: 2 },
    ] },
    Effect { id: "graindelay", name: "Grain Delay", parameters: &[
        Parameter { id: "pitch", name: "Pitch", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "size", name: "Size", minimum: 5.0, maximum: 500.0, default: 80.0, flags: 1 },
        Parameter { id: "jitter", name: "Jitter", minimum: 0.0, maximum: 100.0, default: 10.0, flags: 0 },
        Parameter { id: "reverse", name: "Reverse", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "space", name: "Space", minimum: 0.0, maximum: 2000.0, default: 180.0, flags: 0 },
        Parameter { id: "density", name: "Density", minimum: 0.0, maximum: 100.0, default: 45.0, flags: 0 },
        Parameter { id: "mod", name: "Mod", minimum: 0.0, maximum: 100.0, default: 0.0, flags: 0 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 35.0, flags: 0 },
        Parameter { id: "stereo", name: "Stereo", minimum: 0.0, maximum: 100.0, default: 80.0, flags: 0 },
    ] },
    Effect { id: "grainstretch", name: "Grain Stretch", parameters: &[
        Parameter { id: "on", name: "On / Capture", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "stretch", name: "Stretch", minimum: 0.0, maximum: 200.0, default: 50.0, flags: 0 },
        Parameter { id: "loop", name: "Loop", minimum: 1.0, maximum: 32.0, default: 16.0, flags: 2 },
        Parameter { id: "pitch", name: "Pitch", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "link", name: "Link", minimum: 0.0, maximum: 1.0, default: 1.0, flags: 4 },
        Parameter { id: "size", name: "Size", minimum: 5.0, maximum: 300.0, default: 80.0, flags: 1 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 100.0, flags: 0 },
    ] },
    Effect { id: "resochord", name: "Resochord", parameters: &[
        Parameter { id: "mode", name: "Mode", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 2 },
        Parameter { id: "style", name: "Style", minimum: 0.0, maximum: 7.0, default: 0.0, flags: 2 },
        Parameter { id: "chord", name: "Chord Root", minimum: 0.0, maximum: 11.0, default: 0.0, flags: 2 },
        Parameter { id: "tune", name: "Tune", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "brightness", name: "Brightness", minimum: 0.0, maximum: 100.0, default: 70.0, flags: 0 },
        Parameter { id: "feedback", name: "Feedback", minimum: 0.0, maximum: 99.0, default: 90.0, flags: 0 },
        Parameter { id: "decay", name: "Decay", minimum: 0.03, maximum: 20.0, default: 2.0, flags: 1 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 35.0, flags: 0 },
        Parameter { id: "spread", name: "Spread", minimum: 0.0, maximum: 100.0, default: 25.0, flags: 0 },
    ] },
    Effect { id: "distortion", name: "Distortion", parameters: &[
        Parameter { id: "drive", name: "Drive", minimum: 0.0, maximum: 42.0, default: 12.0, flags: 0 },
        Parameter { id: "color", name: "Fuzz Color", minimum: 0.0, maximum: 100.0, default: 65.0, flags: 0 },
        Parameter { id: "feedback", name: "Fuzz Feedback", minimum: 0.0, maximum: 95.0, default: 15.0, flags: 0 },
        Parameter { id: "tone", name: "Tone", minimum: 150.0, maximum: 16000.0, default: 5000.0, flags: 1 },
        Parameter { id: "tone_mod", name: "Fuzz Tone Mod", minimum: 0.0, maximum: 100.0, default: 0.0, flags: 0 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 100.0, flags: 0 },
        Parameter { id: "output", name: "Output", minimum: -24.0, maximum: 12.0, default: -6.0, flags: 0 },
        Parameter { id: "mode", name: "Mode", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 2 },
        Parameter { id: "bass", name: "Analog Bass", minimum: 0.0, maximum: 100.0, default: 100.0, flags: 0 },
        Parameter { id: "focus", name: "Analog Focus", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "definition", name: "Analog Definition", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "gate", name: "Fuzz Gate", minimum: 0.0, maximum: 1.0, default: 1.0, flags: 4 },
        Parameter { id: "gate_release", name: "Gate Release", minimum: 5.0, maximum: 2000.0, default: 100.0, flags: 1 },
        Parameter { id: "gate_threshold", name: "Gate Threshold", minimum: -96.0, maximum: -18.0, default: -60.0, flags: 0 },
        Parameter { id: "mod_rate", name: "Fuzz Mod Rate", minimum: 0.05, maximum: 10.0, default: 0.4, flags: 1 },
        Parameter { id: "saturation", name: "Analog Saturation", minimum: 0.0, maximum: 100.0, default: 25.0, flags: 0 },
    ] },
    Effect { id: "lofi", name: "Lofi", parameters: &[
        Parameter { id: "sample_rate", name: "Sample Rate", minimum: 99.5, maximum: 48000.0, default: 12000.0, flags: 1 },
        Parameter { id: "bits", name: "Bits", minimum: 1.0, maximum: 24.0, default: 12.0, flags: 0 },
        Parameter { id: "smooth", name: "Smooth", minimum: 0.0, maximum: 100.0, default: 0.0, flags: 0 },
        Parameter { id: "stereo", name: "Stereo Width", minimum: 0.0, maximum: 200.0, default: 100.0, flags: 0 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 100.0, flags: 0 },
        Parameter { id: "input", name: "Input", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "output", name: "Output", minimum: -24.0, maximum: 12.0, default: 0.0, flags: 0 },
        Parameter { id: "jitter", name: "Clock Jitter", minimum: 0.0, maximum: 100.0, default: 0.0, flags: 0 },
    ] },
    Effect { id: "saturator", name: "Saturator", parameters: &[
        Parameter { id: "mode", name: "Mode", minimum: 0.0, maximum: 2.0, default: 0.0, flags: 2 },
        Parameter { id: "input", name: "Input", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "contour", name: "Contour", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "drive", name: "Drive", minimum: 0.0, maximum: 100.0, default: 25.0, flags: 0 },
        Parameter { id: "charge", name: "Tube Charge", minimum: 0.0, maximum: 100.0, default: 25.0, flags: 0 },
        Parameter { id: "overload", name: "Tube Overload", minimum: 0.0, maximum: 18.0, default: 0.0, flags: 0 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 100.0, default: 100.0, flags: 0 },
        Parameter { id: "output", name: "Output", minimum: -24.0, maximum: 12.0, default: 0.0, flags: 0 },
        Parameter { id: "bass", name: "Tube Bass", minimum: -18.0, maximum: 18.0, default: 0.0, flags: 0 },
        Parameter { id: "treble", name: "Tube Treble", minimum: -18.0, maximum: 18.0, default: 0.0, flags: 0 },
        Parameter { id: "eq_bypass", name: "Tube EQ Bypass", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
    ] },
    Effect { id: "performfilter", name: "Perform Filter", parameters: &[
        Parameter { id: "engage", name: "Engage", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "position", name: "Frequency position", minimum: 0.0, maximum: 1.0, default: 0.65, flags: 0 },
        Parameter { id: "type", name: "Filter type", minimum: 0.0, maximum: 2.0, default: 0.0, flags: 2 },
        Parameter { id: "resonance", name: "Resonance", minimum: 0.0, maximum: 1.05, default: 0.2, flags: 0 },
        Parameter { id: "saturation", name: "Saturation", minimum: 0.0, maximum: 30.0, default: 0.0, flags: 0 },
        Parameter { id: "output", name: "Output", minimum: -36.0, maximum: 12.0, default: 0.0, flags: 0 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 1.0, default: 1.0, flags: 0 },
        Parameter { id: "response", name: "Response", minimum: 1.0, maximum: 100.0, default: 12.0, flags: 1 },
        Parameter { id: "min_frequency", name: "Minimum frequency", minimum: 20.0, maximum: 20000.0, default: 40.0, flags: 1 },
        Parameter { id: "max_frequency", name: "Maximum frequency", minimum: 20.0, maximum: 20000.0, default: 18000.0, flags: 1 },
    ] },
    Effect { id: "performflanger", name: "Perform Flanger", parameters: &[
        Parameter { id: "engage", name: "Engage", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "frequency", name: "Comb frequency", minimum: 10.0, maximum: 4000.0, default: 220.0, flags: 1 },
        Parameter { id: "decay", name: "Decay", minimum: 0.02, maximum: 12.0, default: 1.5, flags: 1 },
        Parameter { id: "saturation", name: "Saturation", minimum: 0.0, maximum: 24.0, default: 3.0, flags: 0 },
        Parameter { id: "stereo", name: "Stereo", minimum: 0.0, maximum: 1.0, default: 0.35, flags: 0 },
        Parameter { id: "polarity", name: "Polarity", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 2 },
        Parameter { id: "tone", name: "Tone", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 2 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 1.0, default: 0.7, flags: 0 },
        Parameter { id: "rate", name: "Motion rate", minimum: 0.0, maximum: 10.0, default: 0.25, flags: 0 },
        Parameter { id: "depth", name: "Motion depth", minimum: 0.0, maximum: 1.0, default: 0.15, flags: 0 },
    ] },
    Effect { id: "burstecho", name: "Burst Echo", parameters: &[
        Parameter { id: "engage", name: "Engage", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "position", name: "Performance position", minimum: 0.0, maximum: 1.0, default: 0.6, flags: 0 },
        Parameter { id: "assignment", name: "Position controls", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 2 },
        Parameter { id: "time", name: "Time", minimum: 0.03125, maximum: 4.0, default: 0.5, flags: 1 },
        Parameter { id: "time_mode", name: "Time mode", minimum: 0.0, maximum: 3.0, default: 0.0, flags: 2 },
        Parameter { id: "feedback", name: "Feedback", minimum: 0.0, maximum: 0.98, default: 0.6, flags: 0 },
        Parameter { id: "filter", name: "Feedback filter", minimum: 0.0, maximum: 1.0, default: 0.2, flags: 0 },
        Parameter { id: "wet", name: "Wet level", minimum: 0.0, maximum: 1.0, default: 0.5, flags: 0 },
        Parameter { id: "feed", name: "Burst length (1 = continuous)", minimum: 0.01, maximum: 1.0, default: 0.15, flags: 0 },
        Parameter { id: "mix_mode", name: "Mix mode", minimum: 0.0, maximum: 2.0, default: 0.0, flags: 2 },
        Parameter { id: "free_time", name: "Free time", minimum: 1.0, maximum: 2000.0, default: 250.0, flags: 1 },
        Parameter { id: "saturation", name: "Saturation", minimum: 0.0, maximum: 18.0, default: 0.0, flags: 0 },
        Parameter { id: "stereo", name: "Ping-pong", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 0 },
        Parameter { id: "output", name: "Output", minimum: -24.0, maximum: 12.0, default: 0.0, flags: 0 },
    ] },
    Effect { id: "resoecho", name: "Reso Echo", parameters: &[
        Parameter { id: "engage", name: "Engage", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "frequency", name: "Resonator frequency", minimum: 20.0, maximum: 4000.0, default: 440.0, flags: 1 },
        Parameter { id: "time", name: "Time", minimum: 0.03125, maximum: 4.0, default: 0.5, flags: 1 },
        Parameter { id: "feedback", name: "Feedback", minimum: 0.0, maximum: 0.98, default: 0.65, flags: 0 },
        Parameter { id: "pump", name: "Pump", minimum: 0.0, maximum: 1.0, default: 0.3, flags: 0 },
        Parameter { id: "wet", name: "Wet level", minimum: 0.0, maximum: 1.0, default: 0.5, flags: 0 },
        Parameter { id: "mix_mode", name: "Mix mode", minimum: 0.0, maximum: 2.0, default: 0.0, flags: 2 },
        Parameter { id: "resonance", name: "Resonance", minimum: 0.0, maximum: 1.0, default: 0.6, flags: 0 },
        Parameter { id: "saturation", name: "Saturation", minimum: 0.0, maximum: 24.0, default: 6.0, flags: 0 },
        Parameter { id: "free_time", name: "Free time", minimum: 1.0, maximum: 2000.0, default: 250.0, flags: 1 },
        Parameter { id: "sync", name: "Tempo sync", minimum: 0.0, maximum: 1.0, default: 1.0, flags: 4 },
        Parameter { id: "spread", name: "Stereo spread", minimum: 0.0, maximum: 1.0, default: 0.25, flags: 0 },
    ] },
    Effect { id: "ring", name: "Ring", parameters: &[
        Parameter { id: "engage", name: "Engage", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "frequency", name: "Frequency", minimum: 5.0, maximum: 6000.0, default: 220.0, flags: 1 },
        Parameter { id: "spread", name: "Oscillator spread", minimum: 0.0, maximum: 1.0, default: 0.15, flags: 0 },
        Parameter { id: "glide", name: "Glide", minimum: 1.0, maximum: 2000.0, default: 25.0, flags: 1 },
        Parameter { id: "reverb_time", name: "Reverb time", minimum: 0.1, maximum: 15.0, default: 2.0, flags: 1 },
        Parameter { id: "reverb_mix", name: "Reverb mix", minimum: 0.0, maximum: 1.0, default: 0.25, flags: 0 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 1.0, default: 0.75, flags: 0 },
        Parameter { id: "mix_mode", name: "Mix mode", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 2 },
        Parameter { id: "output", name: "Output", minimum: -24.0, maximum: 12.0, default: 0.0, flags: 0 },
    ] },
    Effect { id: "stutter", name: "Stutter", parameters: &[
        Parameter { id: "engage", name: "Engage", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "position", name: "Performance position", minimum: 0.0, maximum: 1.0, default: 0.5, flags: 0 },
        Parameter { id: "assignment", name: "Position controls", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 2 },
        Parameter { id: "length", name: "Loop length", minimum: 0.03125, maximum: 4.0, default: 0.5, flags: 1 },
        Parameter { id: "pitch", name: "Pitch", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "gate", name: "Gate open", minimum: 0.0, maximum: 1.0, default: 1.0, flags: 0 },
        Parameter { id: "direction", name: "Direction", minimum: 0.0, maximum: 2.0, default: 0.0, flags: 2 },
        Parameter { id: "quantize", name: "Tempo quantize", minimum: 0.0, maximum: 1.0, default: 1.0, flags: 4 },
        Parameter { id: "free_length", name: "Free loop length", minimum: 5.0, maximum: 2000.0, default: 125.0, flags: 1 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 1.0, default: 1.0, flags: 0 },
        Parameter { id: "fade", name: "Seam fade", minimum: 0.1, maximum: 20.0, default: 2.0, flags: 1 },
    ] },
    Effect { id: "tremolo", name: "Tremolo", parameters: &[
        Parameter { id: "engage", name: "Engage", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "position", name: "Performance position", minimum: 0.0, maximum: 1.0, default: 0.5, flags: 0 },
        Parameter { id: "blend", name: "Tremolo / Vibrato", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 0 },
        Parameter { id: "vibrato_mode", name: "Vibrato mode", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 2 },
        Parameter { id: "response", name: "Attack / Release", minimum: 1.0, maximum: 1000.0, default: 20.0, flags: 1 },
        Parameter { id: "stereo", name: "Stereo phase", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 0 },
        Parameter { id: "depth_min", name: "Depth minimum", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 0 },
        Parameter { id: "depth_max", name: "Depth maximum", minimum: 0.0, maximum: 1.0, default: 1.0, flags: 0 },
        Parameter { id: "rate_min", name: "Rate minimum", minimum: 0.05, maximum: 30.0, default: 1.0, flags: 1 },
        Parameter { id: "rate_max", name: "Rate maximum", minimum: 0.05, maximum: 30.0, default: 12.0, flags: 1 },
        Parameter { id: "sync", name: "Tempo sync", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "division", name: "Period", minimum: 0.03125, maximum: 4.0, default: 0.5, flags: 1 },
        Parameter { id: "output", name: "Output", minimum: -24.0, maximum: 12.0, default: 0.0, flags: 0 },
    ] },
    Effect { id: "scratcher", name: "Scratcher", parameters: &[
        Parameter { id: "engage", name: "Engage", minimum: 0.0, maximum: 1.0, default: 0.0, flags: 4 },
        Parameter { id: "position", name: "Scratch position", minimum: 0.0, maximum: 1.0, default: 0.5, flags: 0 },
        Parameter { id: "delay", name: "Swirl delay", minimum: 1.0, maximum: 1000.0, default: 125.0, flags: 1 },
        Parameter { id: "feedback", name: "Feedback", minimum: 0.0, maximum: 0.95, default: 0.3, flags: 0 },
        Parameter { id: "frequency_shift", name: "Feedback shift", minimum: -2000.0, maximum: 2000.0, default: 0.0, flags: 0 },
        Parameter { id: "wet", name: "Swirl level", minimum: 0.0, maximum: 1.0, default: 0.25, flags: 0 },
        Parameter { id: "brake", name: "Brake time", minimum: 0.01, maximum: 3.0, default: 0.25, flags: 1 },
        Parameter { id: "capture", name: "Capture length", minimum: 0.05, maximum: 4.0, default: 1.0, flags: 1 },
        Parameter { id: "mix", name: "Mix", minimum: 0.0, maximum: 1.0, default: 1.0, flags: 0 },
        Parameter { id: "output", name: "Output", minimum: -24.0, maximum: 12.0, default: 0.0, flags: 0 },
    ] },
];

pub(crate) const DRUM_MODELS: &[Effect] = &[
    Effect { id: "kick", name: "Kick", parameters: &[
        Parameter { id: "tune", name: "Tune", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "decay", name: "Decay", minimum: 20.0, maximum: 3000.0, default: 350.0, flags: 1 },
        Parameter { id: "tone", name: "Tone", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "impact", name: "Impact", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "noise", name: "Noise", minimum: 0.0, maximum: 100.0, default: 25.0, flags: 0 },
        Parameter { id: "attack", name: "Attack", minimum: 0.05, maximum: 50.0, default: 0.2, flags: 1 },
        Parameter { id: "drive", name: "Drive", minimum: 0.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "character", name: "Character", minimum: 0.0, maximum: 100.0, default: 30.0, flags: 0 },
    ] }, // model 1
    Effect { id: "snare", name: "Snare", parameters: &[
        Parameter { id: "tune", name: "Tune", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "decay", name: "Decay", minimum: 20.0, maximum: 2000.0, default: 220.0, flags: 1 },
        Parameter { id: "tone", name: "Tone", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "impact", name: "Impact", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "noise", name: "Noise", minimum: 0.0, maximum: 100.0, default: 25.0, flags: 0 },
        Parameter { id: "attack", name: "Attack", minimum: 0.05, maximum: 50.0, default: 0.2, flags: 1 },
        Parameter { id: "drive", name: "Drive", minimum: 0.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "character", name: "Character", minimum: 0.0, maximum: 100.0, default: 30.0, flags: 0 },
    ] }, // model 2
    Effect { id: "hihat", name: "Hi-hat", parameters: &[
        Parameter { id: "tune", name: "Tune", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "decay", name: "Decay", minimum: 10.0, maximum: 3000.0, default: 120.0, flags: 1 },
        Parameter { id: "tone", name: "Tone", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "impact", name: "Impact", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "noise", name: "Noise", minimum: 0.0, maximum: 100.0, default: 25.0, flags: 0 },
        Parameter { id: "attack", name: "Attack", minimum: 0.05, maximum: 50.0, default: 0.2, flags: 1 },
        Parameter { id: "drive", name: "Drive", minimum: 0.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "character", name: "Character", minimum: 0.0, maximum: 100.0, default: 30.0, flags: 0 },
    ] }, // model 3
    Effect { id: "tom", name: "Tom", parameters: &[
        Parameter { id: "tune", name: "Tune", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "decay", name: "Decay", minimum: 30.0, maximum: 4000.0, default: 450.0, flags: 1 },
        Parameter { id: "tone", name: "Tone", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "impact", name: "Impact", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "noise", name: "Noise", minimum: 0.0, maximum: 100.0, default: 25.0, flags: 0 },
        Parameter { id: "attack", name: "Attack", minimum: 0.05, maximum: 50.0, default: 0.2, flags: 1 },
        Parameter { id: "drive", name: "Drive", minimum: 0.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "character", name: "Character", minimum: 0.0, maximum: 100.0, default: 30.0, flags: 0 },
    ] }, // model 4
    Effect { id: "perc", name: "Percussion", parameters: &[
        Parameter { id: "tune", name: "Tune", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "decay", name: "Decay", minimum: 10.0, maximum: 4000.0, default: 300.0, flags: 1 },
        Parameter { id: "tone", name: "Tone", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "impact", name: "Impact", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "noise", name: "Noise", minimum: 0.0, maximum: 100.0, default: 25.0, flags: 0 },
        Parameter { id: "attack", name: "Attack", minimum: 0.05, maximum: 50.0, default: 0.2, flags: 1 },
        Parameter { id: "drive", name: "Drive", minimum: 0.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "character", name: "Character", minimum: 0.0, maximum: 100.0, default: 30.0, flags: 0 },
    ] }, // model 5
    Effect { id: "cymbal", name: "Cymbal", parameters: &[
        Parameter { id: "tune", name: "Tune", minimum: -24.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "decay", name: "Decay", minimum: 100.0, maximum: 12000.0, default: 2500.0, flags: 1 },
        Parameter { id: "tone", name: "Tone", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "impact", name: "Impact", minimum: 0.0, maximum: 100.0, default: 50.0, flags: 0 },
        Parameter { id: "noise", name: "Noise", minimum: 0.0, maximum: 100.0, default: 25.0, flags: 0 },
        Parameter { id: "attack", name: "Attack", minimum: 0.05, maximum: 50.0, default: 0.2, flags: 1 },
        Parameter { id: "drive", name: "Drive", minimum: 0.0, maximum: 24.0, default: 0.0, flags: 0 },
        Parameter { id: "character", name: "Character", minimum: 0.0, maximum: 100.0, default: 30.0, flags: 0 },
    ] }, // model 6
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalogue_is_consistent() {
        assert_eq!((EFFECTS.len(), DRUM_MODELS.len()), (34, 6));
        for effect in EFFECTS.iter().chain(DRUM_MODELS) {
            let mut ids: Vec<&str> = effect.parameters.iter().map(|parameter| parameter.id).collect();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), effect.parameters.len(), "{}: parameter ids are unique", effect.id);
            for parameter in effect.parameters {
                assert!(parameter.minimum < parameter.maximum && (parameter.minimum..=parameter.maximum).contains(&parameter.default), "{}.{}", effect.id, parameter.id);
                let round_trip = physical(parameter, normalized(parameter, parameter.default));
                assert!((round_trip - parameter.default).abs() <= 1e-9 * parameter.default.abs().max(1.0), "{}.{}", effect.id, parameter.id);
            }
        }
        // Ids the Maschine mappings rely on.
        for id in ["eq", "compressor", "gate", "limiter", "maximizer", "transient", "cabinet", "chorus", "fm", "filter", "flanger", "freqshifter", "phaser", "ice", "reflex", "metaverb", "reverblegacy", "beatdelay", "graindelay", "grainstretch", "resochord", "plate", "reverb", "lofi", "saturator", "distortion"] {
            assert!(effect(id).is_some(), "{id}");
        }
        assert_eq!(DRUM_MODELS.iter().map(|model| model.id).collect::<Vec<_>>(), ["kick", "snare", "hihat", "tom", "perc", "cymbal"]);
        assert_eq!(drum_model("tom").map(|(number, _)| number), Some(4));
        // Normalized defaults of the engine's drum controls (from the engine's own conversion).
        let defaults = |model: &Effect| model.parameters.iter().map(|parameter| normalized(parameter, parameter.default)).collect::<Vec<_>>();
        assert_eq!(defaults(&DRUM_MODELS[0]), [0.5, 0.5712251466998278, 0.5, 0.5, 0.25, 0.20068666377598746, 0.0, 0.3]);
        assert_eq!(defaults(&DRUM_MODELS[5])[1], 0.6723512013824777);
        let eq = effect("eq").unwrap();
        assert_eq!(eq.parameter("lowfreq").map(|(address, parameter)| (address, parameter.flags & LOG)), Some((0, LOG)));
        assert_eq!(normalized(eq.parameter("lowfreq").unwrap().1, f64::NAN), normalized(eq.parameter("lowfreq").unwrap().1, eq.parameters[0].default));
    }
}

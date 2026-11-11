//! Parameter metadata: from what an `AUParameter` reports to the protocol's
//! [`ParameterInfo`]. Platform independent (tested on Linux); the macOS side only reads
//! the properties and hands them over as a [`RawParameter`].
//!
//! Rules:
//! values stay in the unit's own range (min..max); a parameter is *stepped* only when the
//! unit says so in a way that can be trusted (unit "boolean" starting at 0, unit
//! "indexed" with integer bounds, or a label list that covers the whole integer range);
//! labels are passed on only when they describe every step.

use stpc_runtime::{ParameterInfo, PARAM_AUTOMATABLE, PARAM_BOOLEAN, PARAM_HIDDEN, PARAM_READ_ONLY, PARAM_STEPPED};

// `AudioUnitParameterUnit` values (AudioUnitProperties.h).
pub const UNIT_GENERIC: u32 = 0;
pub const UNIT_INDEXED: u32 = 1;
pub const UNIT_BOOLEAN: u32 = 2;
pub const UNIT_PERCENT: u32 = 3;
pub const UNIT_SECONDS: u32 = 4;
pub const UNIT_SAMPLE_FRAMES: u32 = 5;
pub const UNIT_PHASE: u32 = 6;
pub const UNIT_RATE: u32 = 7;
pub const UNIT_HERTZ: u32 = 8;
pub const UNIT_CENTS: u32 = 9;
pub const UNIT_RELATIVE_SEMITONES: u32 = 10;
pub const UNIT_MIDI_NOTE_NUMBER: u32 = 11;
pub const UNIT_MIDI_CONTROLLER: u32 = 12;
pub const UNIT_DECIBELS: u32 = 13;
pub const UNIT_LINEAR_GAIN: u32 = 14;
pub const UNIT_DEGREES: u32 = 15;
pub const UNIT_EQUAL_POWER_CROSSFADE: u32 = 16;
pub const UNIT_MIXER_FADER_CURVE1: u32 = 17;
pub const UNIT_PAN: u32 = 18;
pub const UNIT_METERS: u32 = 19;
pub const UNIT_ABSOLUTE_CENTS: u32 = 20;
pub const UNIT_OCTAVES: u32 = 21;
pub const UNIT_BPM: u32 = 22;
pub const UNIT_BEATS: u32 = 23;
pub const UNIT_MILLISECONDS: u32 = 24;
pub const UNIT_RATIO: u32 = 25;
pub const UNIT_CUSTOM: u32 = 26;
pub const UNIT_MIDI2_CONTROLLER: u32 = 27;

// `AudioUnitParameterOptions` bits (AudioUnitProperties.h).
pub const FLAG_METER_READ_ONLY: u32 = 1 << 15;
pub const FLAG_VALUES_HAVE_STRINGS: u32 = 1 << 21;
pub const FLAG_DISPLAY_LOGARITHMIC: u32 = 1 << 22;
pub const FLAG_IS_HIGH_RESOLUTION: u32 = 1 << 23;
pub const FLAG_NON_REAL_TIME: u32 = 1 << 24;
pub const FLAG_CAN_RAMP: u32 = 1 << 25;
pub const FLAG_EXPERT_MODE: u32 = 1 << 26;
pub const FLAG_IS_READABLE: u32 = 1 << 30;
pub const FLAG_IS_WRITABLE: u32 = 1 << 31;

/// Most labels fetched for one stepped parameter when the unit has none in its tree.
pub const MAX_ENUMERATED_LABELS: u32 = 256;

/// The label shown after a value. A custom unit uses the name the plug-in gives; units
/// that are not a quantity (index, switch, pan position, MIDI numbers) have none.
pub fn unit_label(unit: u32, custom_name: Option<&str>) -> String {
    let label = match unit {
        UNIT_PERCENT | UNIT_EQUAL_POWER_CROSSFADE => "%",
        UNIT_SECONDS => "s",
        UNIT_SAMPLE_FRAMES => "samples",
        UNIT_PHASE | UNIT_DEGREES => "°",
        UNIT_RATE => "x",
        UNIT_HERTZ => "Hz",
        UNIT_CENTS | UNIT_ABSOLUTE_CENTS => "cents",
        UNIT_RELATIVE_SEMITONES => "st",
        UNIT_DECIBELS => "dB",
        UNIT_METERS => "m",
        UNIT_OCTAVES => "oct",
        UNIT_BPM => "BPM",
        UNIT_BEATS => "beats",
        UNIT_MILLISECONDS => "ms",
        UNIT_RATIO => ":1",
        UNIT_CUSTOM => custom_name.unwrap_or(""),
        // Generic, indexed, boolean, MIDI numbers, linear gain, fader curve, pan and
        // anything a future system adds. A unit may still give a name (some do for
        // generic parameters): show it.
        _ => custom_name.unwrap_or(""),
    };
    label.trim().to_string()
}

/// Number of intervals of a stepped parameter (a switch has 1), or `None` if the
/// parameter is continuous or its discreteness cannot be trusted.
///
/// `label_count` is the length of the unit's value-string list, if it has one.
pub fn step_count(unit: u32, min: f64, max: f64, label_count: Option<usize>) -> Option<u32> {
    if !min.is_finite() || !max.is_finite() || max <= min {
        return None;
    }
    if unit == UNIT_BOOLEAN {
        // A boolean that does not start at 0 is a plug-in bug; do not guess.
        return (min == 0.0).then_some(1);
    }
    let width = max - min;
    if min.trunc() != min || max.trunc() != max || width > i32::MAX as f64 {
        return None;
    }
    // A label list corroborates only when it describes the entire integer range.
    let labelled = label_count.is_some_and(|count| count >= 2 && (count - 1) as f64 == width);
    (unit == UNIT_INDEXED || labelled).then_some(width as u32)
}

/// Protocol flags from the unit's option bits.
pub fn parameter_flags(options: u32, unit: u32, steps: Option<u32>) -> u32 {
    let readable = options & FLAG_IS_READABLE != 0;
    let writable = options & FLAG_IS_WRITABLE != 0;
    let mut flags = 0;
    if !writable {
        flags |= PARAM_READ_ONLY;
    }
    // "Non real time" parameters may not be changed while rendering: no automation.
    if writable && options & FLAG_NON_REAL_TIME == 0 {
        flags |= PARAM_AUTOMATABLE;
    }
    // Neither readable nor writable: nothing a user could do with it.
    if !readable && !writable {
        flags |= PARAM_HIDDEN;
    }
    if let Some(steps) = steps {
        flags |= PARAM_STEPPED;
        if steps == 1 && unit == UNIT_BOOLEAN {
            flags |= PARAM_BOOLEAN;
        }
    }
    flags
}

/// Whether timed changes of this parameter may carry a ramp.
pub fn can_ramp(options: u32) -> bool {
    options & FLAG_CAN_RAMP != 0 && options & FLAG_IS_WRITABLE != 0 && options & FLAG_NON_REAL_TIME == 0
}

/// What the macOS side reads from one `AUParameter`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawParameter {
    pub address: u64,
    /// `displayName`.
    pub name: String,
    /// `displayNameWithLength:` for a short length, or empty.
    pub short_name: String,
    /// `keyPath`.
    pub key_path: String,
    /// Display names of the enclosing groups, outermost first.
    pub groups: Vec<String>,
    pub unit: u32,
    pub unit_name: Option<String>,
    pub options: u32,
    pub min: f32,
    pub max: f32,
    /// The default if the unit states one (version 2 units do), else the value the
    /// parameter had right after instantiation, else `None`.
    pub default: Option<f32>,
    /// `valueStrings`.
    pub value_strings: Vec<String>,
}

/// A value the unit would accept: finite, inside the range.
pub fn sanitize_value(value: f64, min: f32, max: f32) -> Option<f32> {
    if !value.is_finite() {
        return None;
    }
    let (low, high) = if min <= max { (min, max) } else { (max, min) };
    if !low.is_finite() || !high.is_finite() {
        return Some(value as f32);
    }
    Some((value as f32).clamp(low, high))
}

/// Builds the protocol description. A unit can report a broken range (NaN, infinite,
/// max below min): the description is then finite and ordered all the same, so that it
/// cannot break whoever stores or draws it.
pub fn parameter_info(raw: &RawParameter) -> ParameterInfo {
    let finite = |v: f32| if v.is_finite() { v as f64 } else { 0.0 };
    let (mut min, mut max) = (finite(raw.min), finite(raw.max));
    if max < min {
        std::mem::swap(&mut min, &mut max);
    }
    let has_labels = !raw.value_strings.is_empty();
    let steps = step_count(raw.unit, min, max, has_labels.then_some(raw.value_strings.len()));
    let value_strings = match steps {
        Some(steps) if raw.value_strings.len() == steps as usize + 1 => raw.value_strings.clone(),
        _ => Vec::new(),
    };
    let default = raw.default.filter(|d| d.is_finite()).map_or(min, |d| (d as f64).clamp(min, max));
    let short_name = if raw.short_name.is_empty() || raw.short_name == raw.name {
        raw.name.chars().take(8).collect()
    } else {
        raw.short_name.clone()
    };
    ParameterInfo {
        id: raw.address,
        name: raw.name.clone(),
        short_name,
        identifier: raw.key_path.clone(),
        group: raw.groups.iter().filter(|g| !g.is_empty()).cloned().collect::<Vec<_>>().join("/"),
        unit_label: unit_label(raw.unit, raw.unit_name.as_deref()),
        flags: parameter_flags(raw.options, raw.unit, steps),
        min,
        max,
        default,
        step_count: steps.unwrap_or(0),
        value_strings,
    }
}

/// Whether the labels of a stepped parameter should be fetched one by one from the unit
/// (`stringFromValue:`): it says its values have names, but its tree lists none.
pub fn should_enumerate_labels(raw: &RawParameter) -> Option<u32> {
    if !raw.value_strings.is_empty() || raw.options & FLAG_VALUES_HAVE_STRINGS == 0 {
        return None;
    }
    let steps = step_count(raw.unit, raw.min as f64, raw.max as f64, None)?;
    (steps <= MAX_ENUMERATED_LABELS).then_some(steps)
}

/// Keeps the first parameter of every address: a third-party tree may list one address
/// twice, and whoever keys a table by address must not trap on it.
pub fn dedup_by_address(parameters: Vec<RawParameter>) -> Vec<RawParameter> {
    let mut seen = std::collections::HashSet::with_capacity(parameters.len());
    parameters.into_iter().filter(|p| seen.insert(p.address)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(unit: u32, min: f32, max: f32, options: u32) -> RawParameter {
        RawParameter {
            address: 7,
            name: "Cutoff Frequency".into(),
            key_path: "filter.cutoff".into(),
            groups: vec!["Filter".into()],
            unit,
            options,
            min,
            max,
            ..RawParameter::default()
        }
    }

    const RW: u32 = FLAG_IS_READABLE | FLAG_IS_WRITABLE;

    #[test]
    fn unit_labels() {
        assert_eq!(unit_label(UNIT_HERTZ, None), "Hz");
        assert_eq!(unit_label(UNIT_DECIBELS, None), "dB");
        assert_eq!(unit_label(UNIT_MILLISECONDS, None), "ms");
        assert_eq!(unit_label(UNIT_SECONDS, None), "s");
        assert_eq!(unit_label(UNIT_PERCENT, None), "%");
        assert_eq!(unit_label(UNIT_BPM, None), "BPM");
        assert_eq!(unit_label(UNIT_DEGREES, None), "°");
        assert_eq!(unit_label(UNIT_RELATIVE_SEMITONES, None), "st");
        assert_eq!(unit_label(UNIT_GENERIC, None), "");
        assert_eq!(unit_label(UNIT_INDEXED, None), "");
        assert_eq!(unit_label(UNIT_BOOLEAN, None), "");
        assert_eq!(unit_label(UNIT_PAN, None), "");
        assert_eq!(unit_label(UNIT_CUSTOM, Some(" voices ")), "voices");
        assert_eq!(unit_label(UNIT_CUSTOM, None), "");
        // A named generic unit and a unit this code has never heard of.
        assert_eq!(unit_label(UNIT_GENERIC, Some("steps")), "steps");
        assert_eq!(unit_label(4711, None), "");
        // The name never overrides a known quantity.
        assert_eq!(unit_label(UNIT_HERTZ, Some("kHz")), "Hz");
        // Every defined unit maps to something without panicking.
        for unit in 0..=UNIT_MIDI2_CONTROLLER {
            let _ = unit_label(unit, None);
        }
    }

    #[test]
    fn step_counts_follow_the_trusted_cases_only() {
        assert_eq!(step_count(UNIT_BOOLEAN, 0.0, 1.0, None), Some(1));
        assert_eq!(step_count(UNIT_BOOLEAN, 1.0, 2.0, None), None);
        assert_eq!(step_count(UNIT_INDEXED, 0.0, 7.0, None), Some(7));
        assert_eq!(step_count(UNIT_INDEXED, -2.0, 2.0, None), Some(4));
        assert_eq!(step_count(UNIT_INDEXED, 0.0, 7.5, None), None);
        assert_eq!(step_count(UNIT_INDEXED, 0.0, 0.0, None), None);
        assert_eq!(step_count(UNIT_INDEXED, 0.0, 1.0e12, None), None);
        // Generic with a complete label list is stepped; with a partial one it is not.
        assert_eq!(step_count(UNIT_GENERIC, 0.0, 3.0, Some(4)), Some(3));
        assert_eq!(step_count(UNIT_GENERIC, 0.0, 3.0, Some(3)), None);
        assert_eq!(step_count(UNIT_GENERIC, 0.0, 3.0, Some(1)), None);
        assert_eq!(step_count(UNIT_GENERIC, 0.0, 3.0, None), None);
        assert_eq!(step_count(UNIT_HERTZ, 20.0, 20000.0, None), None);
        for (min, max) in [(f64::NAN, 1.0), (0.0, f64::INFINITY), (f64::NEG_INFINITY, 0.0), (1.0, 0.0)] {
            assert_eq!(step_count(UNIT_INDEXED, min, max, None), None);
            assert_eq!(step_count(UNIT_BOOLEAN, min, max, None), None);
        }
    }

    #[test]
    fn flags() {
        assert_eq!(parameter_flags(RW, UNIT_HERTZ, None), PARAM_AUTOMATABLE);
        assert_eq!(parameter_flags(FLAG_IS_READABLE, UNIT_HERTZ, None), PARAM_READ_ONLY);
        assert_eq!(parameter_flags(RW | FLAG_NON_REAL_TIME, UNIT_HERTZ, None), 0);
        assert_eq!(parameter_flags(0, UNIT_GENERIC, None), PARAM_READ_ONLY | PARAM_HIDDEN);
        assert_eq!(parameter_flags(FLAG_IS_WRITABLE, UNIT_GENERIC, None), PARAM_AUTOMATABLE);
        assert_eq!(parameter_flags(RW, UNIT_INDEXED, Some(4)), PARAM_AUTOMATABLE | PARAM_STEPPED);
        assert_eq!(parameter_flags(RW, UNIT_BOOLEAN, Some(1)), PARAM_AUTOMATABLE | PARAM_STEPPED | PARAM_BOOLEAN);
        // A two-entry list is stepped, but only unit "boolean" is a switch.
        assert_eq!(parameter_flags(RW, UNIT_INDEXED, Some(1)), PARAM_AUTOMATABLE | PARAM_STEPPED);
        // Expert-mode and meter parameters stay visible.
        assert_eq!(parameter_flags(RW | FLAG_EXPERT_MODE, UNIT_GENERIC, None), PARAM_AUTOMATABLE);
        assert_eq!(parameter_flags(FLAG_IS_READABLE | FLAG_METER_READ_ONLY, UNIT_DECIBELS, None), PARAM_READ_ONLY);
        assert!(can_ramp(RW | FLAG_CAN_RAMP));
        assert!(!can_ramp(RW));
        assert!(!can_ramp(FLAG_IS_READABLE | FLAG_CAN_RAMP));
        assert!(!can_ramp(RW | FLAG_CAN_RAMP | FLAG_NON_REAL_TIME));
    }

    #[test]
    fn continuous_parameter() {
        let mut p = raw(UNIT_HERTZ, 20.0, 20000.0, RW | FLAG_CAN_RAMP | FLAG_DISPLAY_LOGARITHMIC);
        p.default = Some(1000.0);
        p.short_name = "Cutoff".into();
        let info = parameter_info(&p);
        assert_eq!(info.id, 7);
        assert_eq!(info.name, "Cutoff Frequency");
        assert_eq!(info.short_name, "Cutoff");
        assert_eq!(info.identifier, "filter.cutoff");
        assert_eq!(info.group, "Filter");
        assert_eq!(info.unit_label, "Hz");
        assert_eq!(info.flags, PARAM_AUTOMATABLE);
        assert_eq!((info.min, info.max, info.default), (20.0, 20000.0, 1000.0));
        assert_eq!(info.step_count, 0);
        assert!(info.value_strings.is_empty());
    }

    #[test]
    fn indexed_parameter_with_labels() {
        let mut p = raw(UNIT_INDEXED, 0.0, 2.0, RW);
        p.groups = vec!["Osc".into(), String::new(), "Shape".into()];
        p.value_strings = vec!["Sine".into(), "Saw".into(), "Square".into()];
        let info = parameter_info(&p);
        assert_eq!(info.group, "Osc/Shape");
        assert_eq!(info.step_count, 2);
        assert_eq!(info.value_strings, ["Sine", "Saw", "Square"]);
        assert_eq!(info.flags, PARAM_AUTOMATABLE | PARAM_STEPPED);
        assert_eq!(info.short_name, "Cutoff F");
        // No default known: the minimum.
        assert_eq!(info.default, 0.0);

        // Labels that do not cover the range are not passed on, and prove nothing.
        p.value_strings = vec!["Off".into(), "On".into()];
        let info = parameter_info(&p);
        assert_eq!(info.step_count, 2);
        assert!(info.value_strings.is_empty());
        p.unit = UNIT_GENERIC;
        let info = parameter_info(&p);
        assert_eq!(info.step_count, 0);
        assert_eq!(info.flags, PARAM_AUTOMATABLE);
    }

    #[test]
    fn broken_ranges_and_defaults_come_out_finite() {
        let mut p = raw(UNIT_GENERIC, f32::NAN, f32::INFINITY, RW);
        p.default = Some(f32::NAN);
        let info = parameter_info(&p);
        assert_eq!((info.min, info.max, info.default), (0.0, 0.0, 0.0));
        let mut p = raw(UNIT_GENERIC, 10.0, -10.0, RW);
        p.default = Some(99.0);
        let info = parameter_info(&p);
        assert_eq!((info.min, info.max, info.default), (-10.0, 10.0, 10.0));
        assert_eq!(info.step_count, 0);
    }

    #[test]
    fn value_sanitising() {
        assert_eq!(sanitize_value(0.5, 0.0, 1.0), Some(0.5));
        assert_eq!(sanitize_value(2.0, 0.0, 1.0), Some(1.0));
        assert_eq!(sanitize_value(-2.0, 0.0, 1.0), Some(0.0));
        assert_eq!(sanitize_value(f64::NAN, 0.0, 1.0), None);
        assert_eq!(sanitize_value(f64::INFINITY, 0.0, 1.0), None);
        // Reversed and broken ranges do not panic.
        assert_eq!(sanitize_value(5.0, 1.0, 0.0), Some(1.0));
        assert_eq!(sanitize_value(5.0, f32::NAN, 1.0), Some(5.0));
    }

    #[test]
    fn label_enumeration_and_duplicates() {
        let mut p = raw(UNIT_INDEXED, 0.0, 3.0, RW | FLAG_VALUES_HAVE_STRINGS);
        assert_eq!(should_enumerate_labels(&p), Some(3));
        p.value_strings = vec!["a".into()];
        assert_eq!(should_enumerate_labels(&p), None);
        p.value_strings.clear();
        p.options = RW;
        assert_eq!(should_enumerate_labels(&p), None);
        p.options = RW | FLAG_VALUES_HAVE_STRINGS;
        p.max = 100_000.0;
        assert_eq!(should_enumerate_labels(&p), None);
        p.unit = UNIT_HERTZ;
        p.max = 3.0;
        assert_eq!(should_enumerate_labels(&p), None);

        let mut a = raw(UNIT_GENERIC, 0.0, 1.0, RW);
        a.name = "first".into();
        let mut b = a.clone();
        b.name = "second".into();
        let mut c = a.clone();
        c.address = 8;
        let list = dedup_by_address(vec![a.clone(), b, c.clone()]);
        assert_eq!(list, vec![a, c]);
    }
}

//! Inserts: the processors of a strip's insert chain, behind one trait.
//!
//! The graph renders whatever implements [`InsertProcessor`]. Built-in effects implement it
//! here, on top of `tracker_engine::effects`. A plug-in instance in a container process
//! implements it in [`crate::plugins::insert`] (its real-time `process` call through the
//! client, one call per block with timed parameter changes); an insert whose plug-in cannot be
//! made passes its signal through and is reported as unavailable. Processors come from an
//! [`InsertFactory`], on the control thread.
//!
//! # Threads
//!
//! `prepare` runs on the control thread before the processor is first published. From then on
//! the processor belongs to the render owner: `process`, `parameter`, `set_bypass` and `reset`
//! are called only from the graph render, which never allocates, locks or blocks in them. The
//! processor is dropped on the control thread, after the last render plan that refers to it
//! has been retired (see [`crate::graph::plan`]).

use super::spec::InsertSpec;
use std::ffi::CString;
use tracker_engine::effects::{self, common};

/// What a processor may want to know about the block it renders.
#[derive(Debug, Clone, Copy)]
pub struct ProcessContext {
    pub sample_rate: f64,
    /// Project tempo in BPM.
    pub tempo: f64,
    /// The transport and the render cycle of the block (for plug-ins).
    pub block: BlockInfo,
}

/// The transport at the start of a block, and how long the render call it belongs to may take.
/// The renderer fills it in once per block ([`crate::graph::plan::Plan::set_block`]).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BlockInfo {
    pub playing: bool,
    pub recording: bool,
    /// Tempo of the engine's clock (BPM); 0 when unknown.
    pub bpm: f64,
    pub numerator: u16,
    pub denominator: u16,
    /// Song position of the block's first frame, in frames and in quarter notes.
    pub sample_position: i64,
    pub beat: f64,
    pub bar_start_beat: f64,
    /// When the render call that contains this block should be finished, in nanoseconds on
    /// the clock of [`monotonic_ns`]; 0 when the caller has no cycle (no deadline).
    pub cycle_end_ns: u64,
}

/// Nanoseconds since the first call in this process (a monotonic clock; no allocation).
pub fn monotonic_ns() -> u64 {
    static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    ORIGIN.get_or_init(std::time::Instant::now).elapsed().as_nanos() as u64 + 1
}

/// One insert effect instance.
pub trait InsertProcessor: Send {
    /// Control thread, once, before the first block: allocate everything for blocks of up to
    /// `max_frames` frames at `sample_rate`.
    fn prepare(&mut self, sample_rate: f64, max_frames: usize);
    /// Latency in frames, constant after `prepare`. The graph compensates for it.
    fn latency(&self) -> u32 {
        0
    }
    /// How long the output rings after the input stops, in seconds (for export and freeze tails).
    fn tail_seconds(&self) -> f64 {
        0.0
    }
    /// Whether the processor reads the sidechain input it is given.
    fn accepts_sidechain(&self) -> bool {
        false
    }
    /// In-place stereo render. `sidechain` has the block's length when a sidechain is routed in.
    fn process(&mut self, left: &mut [f32], right: &mut [f32], sidechain: Option<(&[f32], &[f32])>, context: &ProcessContext);
    /// A normalized (0..1) parameter value that applies from the next `process` on. The graph
    /// splits blocks at automation points, so this is sample accurate.
    fn parameter(&mut self, address: u64, normalized: f64);
    fn set_bypass(&mut self, bypass: bool);
    /// Forget all signal history.
    fn reset(&mut self);
    /// True while silent input would give exactly silent output, so a silent block can be skipped.
    fn is_idle(&self) -> bool {
        false
    }
    /// True for a processor that takes parameter changes with their frame offset
    /// ([`InsertProcessor::parameter_at`]) and renders each block in one `process` call: a
    /// plug-in in a container, where every call is a round trip. The graph then does not
    /// split its blocks at automation points.
    fn timed_parameters(&self) -> bool {
        false
    }
    /// A normalized value that applies `offset` frames into the next `process` call. Only
    /// called when [`InsertProcessor::timed_parameters`] is true.
    fn parameter_at(&mut self, address: u64, normalized: f64, _offset: u32) {
        self.parameter(address, normalized);
    }
    /// A base value given when a plan is adopted or automation ends (the insert's saved or
    /// live value). A plug-in keeps its own values and only undoes what automation moved.
    fn base_parameter(&mut self, address: u64, normalized: f64) {
        self.parameter(address, normalized);
    }
    /// [`InsertProcessor::base_parameter`] for `offset` frames into the next `process` call
    /// (a step lock's row ended there). Only called when
    /// [`InsertProcessor::timed_parameters`] is true.
    fn base_parameter_at(&mut self, address: u64, normalized: f64, _offset: u32) {
        self.base_parameter(address, normalized);
    }
    /// A note or controller for an instrument, `offset` frames into the next `process` call
    /// (the engine's note events for a track whose instrument is a plug-in). `sequenced`
    /// tells a note of the pattern from a live one.
    fn midi(&mut self, _offset: u32, _bytes: [u8; 3], _sequenced: bool) {}
}

/// Makes processors for insert slots. Control thread only.
pub trait InsertFactory: Send + Sync {
    /// A prepared-to-be-prepared processor for `insert`, or why there is none.
    fn create(&self, insert: &InsertSpec, sample_rate: f64) -> Result<Box<dyn InsertProcessor>, String>;
    /// The parameter values (address, normalized) an insert starts from: its saved state over
    /// its defaults. Applied on the render thread when a plan with the insert is adopted.
    fn base_parameters(&self, insert: &InsertSpec) -> Result<Vec<(u64, f64)>, String>;
}

/// The factory of this crate: built-in effects. Everything else is unavailable.
pub struct BuiltinFactory;

impl InsertFactory for BuiltinFactory {
    fn create(&self, insert: &InsertSpec, sample_rate: f64) -> Result<Box<dyn InsertProcessor>, String> {
        let descriptor = builtin_descriptor(insert)?;
        Ok(Box::new(BuiltinInsert::new(descriptor, sample_rate)))
    }

    fn base_parameters(&self, insert: &InsertSpec) -> Result<Vec<(u64, f64)>, String> {
        let descriptor = builtin_descriptor(insert)?;
        let saved = match insert.state.as_deref().filter(|state| !state.is_empty()) {
            Some(state) => decode_builtin_state(insert.builtin_id().unwrap_or_default(), state)?,
            None => Vec::new(),
        };
        Ok(descriptor
            .params
            .iter()
            .enumerate()
            .map(|(index, info)| {
                let id = info.id().to_string_lossy();
                let value = saved.iter().find(|(name, _)| *name == id).map(|(_, value)| *value);
                (index as u64, value.unwrap_or_else(|| info.normalized(info.default_value)))
            })
            .collect())
    }
}

fn builtin_descriptor(insert: &InsertSpec) -> Result<&'static common::Descriptor, String> {
    let name = if insert.name.is_empty() { insert.component.as_str() } else { insert.name.as_str() };
    let Some(id) = insert.builtin_id() else {
        return Err(format!("{name} is a plug-in. Plug-ins are not hosted yet, so this insert passes its signal through."));
    };
    CString::new(id).ok().and_then(|id| effects::find(&id)).ok_or_else(|| format!("{name} names a built-in effect this version does not have ({id})."))
}

/// The saved state of a built-in effect: base64 of
/// `{"format":"SuperTracker.BuiltinFX","version":1,"id":"<effect>","parameters":{"<id>":<0..1>}}`.
pub fn encode_builtin_state(effect_id: &str, values: &[(String, f64)]) -> String {
    let parameters: serde_json::Map<String, serde_json::Value> = values.iter().map(|(id, value)| (id.clone(), serde_json::json!(value))).collect();
    let state = serde_json::json!({ "format": "SuperTracker.BuiltinFX", "version": 1, "id": effect_id, "parameters": parameters });
    base64_encode(state.to_string().as_bytes())
}

/// Reads [`encode_builtin_state`]'s text: `(parameter id, normalized value)`.
pub fn decode_builtin_state(effect_id: &str, state: &str) -> Result<Vec<(String, f64)>, String> {
    let mismatch = || "The saved effect state does not match this studio effect.".to_string();
    let bytes = base64_decode(state).ok_or_else(mismatch)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| mismatch())?;
    if value["format"] != "SuperTracker.BuiltinFX" || value["version"] != 1 || value["id"] != effect_id {
        return Err(mismatch());
    }
    let mut values = Vec::new();
    for (id, value) in value["parameters"].as_object().ok_or_else(mismatch)? {
        match value.as_f64() {
            Some(value) if (0.0..=1.0).contains(&value) => values.push((id.clone(), value)),
            _ => return Err("The saved effect contains an invalid parameter.".to_string()),
        }
    }
    Ok(values)
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let word = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for position in 0..4 {
            if position <= chunk.len() {
                out.push(BASE64[(word >> (18 - 6 * position) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub(crate) fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut word, mut bits) = (0u32, 0u32);
    for byte in text.bytes().filter(|byte| !byte.is_ascii_whitespace() && *byte != b'=') {
        let value = BASE64.iter().position(|candidate| *candidate == byte)? as u32;
        word = word << 6 | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((word >> bits) as u8);
        }
    }
    Some(out)
}

/// Inputs and outputs below this peak count as silence for the idle gate.
const SILENCE: f32 = 1e-6;

/// A built-in effect of `tracker_engine::effects` as an insert.
pub struct BuiltinInsert {
    descriptor: &'static common::Descriptor,
    sample_rate: f64,
    processor: Option<Box<dyn common::Processor>>,
    /// Physical parameter values set before `prepare`.
    max_frames: usize,
    latency: u32,
    bypass: bool,
    // Idle gate: frames of silent input so far, whether the output has decayed, and after how
    // many silent frames the effect may be skipped (recomputed when a parameter or tempo moves).
    silent_frames: u64,
    output_decayed: bool,
    gate_frames: Option<u64>,
}

impl BuiltinInsert {
    pub fn new(descriptor: &'static common::Descriptor, sample_rate: f64) -> BuiltinInsert {
        BuiltinInsert {
            descriptor,
            sample_rate,
            processor: None,
            max_frames: 0,
            latency: 0,
            bypass: false,
            silent_frames: 0,
            output_decayed: false,
            gate_frames: None,
        }
    }

    fn gate_threshold(&mut self) -> u64 {
        if let Some(frames) = self.gate_frames {
            return frames;
        }
        let Some(processor) = &self.processor else { return u64::MAX };
        let core = processor.core();
        let tail = processor.tail_for_parameters(&core.p, core.tempo);
        // Sustaining and freeze modes report the 120 s export cap: never skip those.
        let frames = if !processor.gateable() || !tail.is_finite() || tail >= 120.0 {
            u64::MAX
        } else {
            (((tail + 0.1) * core.sr) as u64).saturating_add(self.latency as u64).saturating_add(self.max_frames as u64)
        };
        self.gate_frames = Some(frames);
        frames
    }
}

/// Scrubs non-finite and runaway samples (the effects expect clean input) and returns the peak.
fn clean(block: &mut [f32]) -> f32 {
    let mut peak = 0.0f32;
    for sample in block {
        let value = if sample.is_finite() { sample.clamp(-1e6, 1e6) } else { 0.0 };
        *sample = value;
        peak = peak.max(value.abs());
    }
    peak
}

impl InsertProcessor for BuiltinInsert {
    fn prepare(&mut self, sample_rate: f64, max_frames: usize) {
        self.sample_rate = sample_rate;
        self.max_frames = max_frames.max(1);
        let mut processor = (self.descriptor.create)(sample_rate, self.max_frames as u32);
        self.latency = processor.latency();
        for (slot, info) in processor.core_mut().p.iter_mut().zip(self.descriptor.params.iter()) {
            *slot = info.default_value;
        }
        processor.reset();
        self.processor = Some(processor);
    }

    fn latency(&self) -> u32 {
        self.latency
    }

    fn tail_seconds(&self) -> f64 {
        match &self.processor {
            Some(processor) => processor.tail_for_parameters(&processor.core().p, processor.core().tempo),
            None => 0.0,
        }
    }

    fn accepts_sidechain(&self) -> bool {
        self.descriptor.info.flags & common::EFFECT_SIDECHAIN != 0
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32], sidechain: Option<(&[f32], &[f32])>, context: &ProcessContext) {
        if self.bypass {
            return;
        }
        let gate_frames = self.gate_threshold();
        let Some(processor) = self.processor.as_mut() else { return };
        if processor.core().tempo != context.tempo {
            processor.core_mut().tempo = context.tempo;
            self.gate_frames = None;
        }
        let frames = left.len().min(right.len());
        let mut offset = 0;
        while offset < frames {
            let count = self.max_frames.min(frames - offset);
            let (l, r) = (&mut left[offset..offset + count], &mut right[offset..offset + count]);
            let side = sidechain.map(|(side_l, side_r)| (&side_l[offset..offset + count], &side_r[offset..offset + count]));
            let side_peak = side.map_or(0.0, |(side_l, side_r)| side_l.iter().chain(side_r).fold(0.0f32, |peak, sample| peak.max(sample.abs())));
            let silent_input = clean(l).max(clean(r)).max(side_peak) < SILENCE;
            if silent_input && self.output_decayed && self.silent_frames >= gate_frames {
                l.fill(0.0);
                r.fill(0.0);
            } else {
                processor.process(l, r, side.map(|side| side.0), side.map(|side| side.1));
                let out = clean(l).max(clean(r));
                if silent_input {
                    self.silent_frames = self.silent_frames.saturating_add(count as u64);
                    self.output_decayed = out < SILENCE;
                } else {
                    self.silent_frames = 0;
                    self.output_decayed = false;
                }
            }
            offset += count;
        }
    }

    fn parameter(&mut self, address: u64, normalized: f64) {
        let (Some(processor), Some(info)) = (self.processor.as_mut(), self.descriptor.params.get(address as usize)) else { return };
        if (address as usize) < common::MAX_PARAMETERS && normalized.is_finite() {
            processor.core_mut().p[address as usize] = info.physical(normalized);
            self.gate_frames = None;
        }
    }

    fn set_bypass(&mut self, bypass: bool) {
        if self.bypass && !bypass {
            self.reset();
        }
        self.bypass = bypass;
    }

    fn reset(&mut self) {
        if let Some(processor) = self.processor.as_mut() {
            processor.reset();
        }
        self.silent_frames = 0;
        self.output_decayed = false;
    }

    fn is_idle(&self) -> bool {
        self.bypass || (self.output_decayed && self.gate_frames.is_some_and(|frames| self.silent_frames >= frames))
    }
}

/// What stands in for an insert that cannot be created: the signal passes unchanged.
pub struct PassThrough;

impl InsertProcessor for PassThrough {
    fn prepare(&mut self, _: f64, _: usize) {}
    fn process(&mut self, _: &mut [f32], _: &mut [f32], _: Option<(&[f32], &[f32])>, _: &ProcessContext) {}
    fn parameter(&mut self, _: u64, _: f64) {}
    fn set_bypass(&mut self, _: bool) {}
    fn reset(&mut self) {}
    fn is_idle(&self) -> bool {
        true
    }
}

/// Metadata of every built-in effect, as `builtin.effects` returns it: the index of a parameter
/// is its address.
pub fn builtin_effects_metadata() -> serde_json::Value {
    let text = |value: &std::ffi::CStr| value.to_string_lossy().into_owned();
    let effects: Vec<serde_json::Value> = effects::catalogue()
        .map(|descriptor| {
            let parameters: Vec<serde_json::Value> = descriptor
                .params
                .iter()
                .enumerate()
                .map(|(address, info)| {
                    let labels: Vec<String> = (0..info.label_count()).filter_map(|index| info.label(index).map(text)).collect();
                    serde_json::json!({
                        "address": address, "id": text(info.id()), "name": text(info.name()), "unit": text(info.unit()),
                        "min": info.minimum, "max": info.maximum, "defaultValue": info.default_value,
                        "logarithmic": info.flags & common::PARAMETER_LOG != 0,
                        "integer": info.flags & common::PARAMETER_INTEGER != 0,
                        "toggle": info.flags & common::PARAMETER_TOGGLE != 0,
                        "labels": labels,
                    })
                })
                .collect();
            serde_json::json!({
                "id": format!("stfx:{}", text(descriptor.info.id())), "name": text(descriptor.info.name()),
                "category": text(descriptor.info.category()),
                "sidechain": descriptor.info.flags & common::EFFECT_SIDECHAIN != 0, "parameters": parameters,
            })
        })
        .collect();
    serde_json::Value::Array(effects)
}

/// Processors for tests of the graph: exact, trivially predictable signal changes.
pub mod testing {
    use super::*;

    /// A factory that knows the `test:` components below and otherwise asks [`BuiltinFactory`].
    ///
    /// * `test:latency:<frames>`: a pure delay that reports its length as latency;
    /// * `test:gain`: multiplies by twice parameter 0 (default 0.5: unity);
    /// * `test:key`: multiplies by the sidechain's absolute left sample (silence without one).
    pub struct TestFactory;

    impl InsertFactory for TestFactory {
        fn create(&self, insert: &InsertSpec, sample_rate: f64) -> Result<Box<dyn InsertProcessor>, String> {
            let parts: Vec<&str> = insert.component.split(':').collect();
            match parts.as_slice() {
                ["test", "latency", frames] => Ok(Box::new(LatencyInsert { line: crate::graph::pdc::DelayLine::new(frames.parse().map_err(|_| "bad latency")?), bypass: false })),
                ["test", "gain"] => Ok(Box::new(GainInsert { gain: 1.0, bypass: false })),
                ["test", "key"] => Ok(Box::new(KeyInsert)),
                _ => BuiltinFactory.create(insert, sample_rate),
            }
        }

        fn base_parameters(&self, insert: &InsertSpec) -> Result<Vec<(u64, f64)>, String> {
            match insert.component.as_str() {
                "test:gain" => Ok(vec![(0, insert.state.as_deref().and_then(|state| state.parse().ok()).unwrap_or(0.5))]),
                component if component.starts_with("test:") => Ok(Vec::new()),
                _ => BuiltinFactory.base_parameters(insert),
            }
        }
    }

    pub struct LatencyInsert {
        line: crate::graph::pdc::DelayLine,
        bypass: bool,
    }

    impl InsertProcessor for LatencyInsert {
        fn prepare(&mut self, _: f64, _: usize) {}
        fn latency(&self) -> u32 {
            self.line.frames() as u32
        }
        fn process(&mut self, left: &mut [f32], right: &mut [f32], _: Option<(&[f32], &[f32])>, _: &ProcessContext) {
            if self.bypass {
                return;
            }
            // In place, one frame at a time: the line reads before it writes.
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                let (x_l, x_r) = ([*l], [*r]);
                self.line.process(&x_l, &x_r, std::slice::from_mut(l), std::slice::from_mut(r));
            }
        }
        fn parameter(&mut self, _: u64, _: f64) {}
        fn set_bypass(&mut self, bypass: bool) {
            self.bypass = bypass;
        }
        fn reset(&mut self) {
            self.line.clear();
        }
    }

    pub struct GainInsert {
        gain: f32,
        bypass: bool,
    }

    impl InsertProcessor for GainInsert {
        fn prepare(&mut self, _: f64, _: usize) {}
        fn process(&mut self, left: &mut [f32], right: &mut [f32], _: Option<(&[f32], &[f32])>, _: &ProcessContext) {
            if !self.bypass {
                left.iter_mut().chain(right.iter_mut()).for_each(|sample| *sample *= self.gain);
            }
        }
        fn parameter(&mut self, address: u64, normalized: f64) {
            if address == 0 {
                self.gain = (normalized * 2.0) as f32;
            }
        }
        fn set_bypass(&mut self, bypass: bool) {
            self.bypass = bypass;
        }
        fn reset(&mut self) {}
    }

    pub struct KeyInsert;

    impl InsertProcessor for KeyInsert {
        fn prepare(&mut self, _: f64, _: usize) {}
        fn accepts_sidechain(&self) -> bool {
            true
        }
        fn process(&mut self, left: &mut [f32], right: &mut [f32], sidechain: Option<(&[f32], &[f32])>, _: &ProcessContext) {
            for (index, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
                let key = sidechain.map_or(0.0, |(side, _)| side[index].abs());
                *l *= key;
                *r *= key;
            }
        }
        fn parameter(&mut self, _: u64, _: f64) {}
        fn set_bypass(&mut self, _: bool) {}
        fn reset(&mut self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert(component: &str, state: Option<String>) -> InsertSpec {
        InsertSpec { key: "k".into(), instance_id: "k".into(), slot: 0, instrument_chain: false, component: component.into(), name: String::new(), bypass: false, state, instrument: None }
    }

    #[test]
    fn base64_round_trips() {
        for text in ["", "a", "ab", "abc", "abcd", "{\"format\":1}"] {
            assert_eq!(base64_decode(&base64_encode(text.as_bytes())).unwrap(), text.as_bytes());
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert!(base64_decode("!!").is_none());
    }

    #[test]
    fn every_built_in_effect_can_be_created_and_renders_finite_audio() {
        let context = ProcessContext { sample_rate: 48_000.0, tempo: 120.0, block: BlockInfo::default() };
        let mut count = 0;
        for descriptor in effects::catalogue() {
            let id = descriptor.info.id().to_string_lossy().into_owned();
            let spec = insert(&format!("stfx:{id}"), None);
            let mut processor = BuiltinFactory.create(&spec, 48_000.0).unwrap();
            processor.prepare(48_000.0, 256);
            for (address, value) in BuiltinFactory.base_parameters(&spec).unwrap() {
                processor.parameter(address, value);
            }
            let mut left: Vec<f32> = (0..1000).map(|i| ((i as f32) * 0.05).sin() * 0.5).collect();
            let mut right = left.clone();
            processor.process(&mut left, &mut right, None, &context);
            assert!(left.iter().chain(&right).all(|sample| sample.is_finite()), "{id}");
            count += 1;
        }
        assert!(count > 10, "the catalogue has effects");
        assert!(builtin_effects_metadata().as_array().unwrap().len() == count);
    }

    #[test]
    fn a_saved_state_overrides_the_defaults_and_a_foreign_state_is_refused() {
        let descriptor = effects::catalogue().next().unwrap();
        let id = descriptor.info.id().to_string_lossy().into_owned();
        let first = descriptor.params[0].id().to_string_lossy().into_owned();
        let state = encode_builtin_state(&id, &[(first.clone(), 0.25)]);
        assert_eq!(decode_builtin_state(&id, &state).unwrap(), [(first, 0.25)]);
        let base = BuiltinFactory.base_parameters(&insert(&format!("stfx:{id}"), Some(state.clone()))).unwrap();
        assert_eq!(base[0], (0, 0.25));
        assert_eq!(base.len(), descriptor.params.len());
        assert!(decode_builtin_state("another", &state).is_err());
        assert!(BuiltinFactory.create(&insert("com.vendor.plugin", None), 48_000.0).err().unwrap().contains("not hosted yet"));
        assert!(BuiltinFactory.create(&insert("stfx:no-such-effect", None), 48_000.0).is_err());
    }
}

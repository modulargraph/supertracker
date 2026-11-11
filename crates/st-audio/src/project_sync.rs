//! From a published project to the engine: timing, tracks, instruments and their samples,
//! patterns (steps, note columns, step effects, automation lanes), the song and its loop, and
//! timeline clips.
//!
//! The interface publishes the whole project after every edit. [`ProjectSync::apply`] sends it
//! to the engine as one update (`begin_update` ... `commit`), which the audio thread adopts at
//! a block boundary. Two things keep that cheap while a song plays:
//!
//! * **Patterns are reused.** What was sent for each pattern is remembered together with the
//!   engine's pattern epoch; a pattern whose content and resolution context (track count, the
//!   numbering of host automation destinations, their default values) are unchanged is not sent
//!   again: the engine shares its previous copy (`reuse_pattern`). Editing a note re-sends one
//!   pattern.
//! * **Samples are installed once.** Each instrument slot, multisample zone set and clip asset
//!   remembers which decoded PCM it holds; only changes reach the engine.
//!
//! Mixing is the host's: a track's bus of the engine is the track's raw signal (see
//! [`crate::graph`]). Track volume, pan and the two built-in sends still travel through the
//! engine as *values*: its sequencer owns their lanes and step effects and reports every change
//! as a timed event that the graph applies. Automation of everything else the host owns goes in
//! as host lanes, numbered by [`GraphSpec::destinations`].

use crate::graph::spec::{track_owner, Destination, GraphSpec};
use crate::samples::{SampleLookup, SamplePcm, SampleSource};
use st_project::model::{
    FilterType, InstrumentKind, Interpolation, LfoShape, LfoTarget, Pattern, Project, SamplerMode, Step, StepEffect, StepEffectKind, VoiceMode,
};
use std::collections::HashMap;
use std::sync::Arc;
use tracker_engine::engine::abi::*;
use tracker_engine::engine::edits::Pcm;
use tracker_engine::engine::host::ControlHalf;

/// The engine's automation target of a lane name, for the lanes the engine plays itself.
pub fn native_target(name: &str) -> Option<i32> {
    Some(match name {
        "volume" => 0,
        "pan" => 1,
        "cutoff" => 2,
        "resonance" => 3,
        "delay" => 4,
        "reverb" => 5,
        "pitch" => 6,
        "attack" => 8,
        "decay" => 9,
        "sustain" => 10,
        "release" => 11,
        "drive" => 12,
        "bitDepth" => 13,
        "lfoRate" => 14,
        "lfoDepth" => 15,
        "start" => 16,
        "end" => 17,
        "loopStart" => 18,
        "loopEnd" => 19,
        "grainSize" => 20,
        "grainPosition" => 21,
        "sliceCount" => 22,
        "instrumentVolume" => 23,
        "instrumentPan" => 24,
        "tune" => 25,
        "filterAttack" => 26,
        "filterDecay" => 27,
        "filterSustain" => 28,
        "filterRelease" => 29,
        "filterEnvAmount" => 30,
        "lfo2Rate" => 31,
        "lfo2Depth" => 32,
        "lfoBeats" => 33,
        "lfo2Beats" => 34,
        "wavetablePosition" => 35,
        "grainDensity" => 36,
        "grainSpray" => 37,
        "grainPitchSpread" => 38,
        "grainScan" => 39,
        "grainReverse" => 40,
        "grainStereo" => 41,
        _ => return None,
    })
}

/// The engine target of a live instrument parameter (`engine.instrumentParameter`): the filter,
/// or an instrument automation target.
pub fn live_instrument_target(name: &str) -> Option<i32> {
    match name {
        "cutoff" => Some(2),
        "resonance" => Some(3),
        _ => native_target(name).filter(|target| *target >= 8),
    }
}

fn step_effect(effect: Option<&StepEffect>) -> STStepFX {
    let kind = match effect.and_then(|effect| effect.kind) {
        Some(StepEffectKind::Volume) => ST_FX_VOLUME,
        Some(StepEffectKind::Pan) => ST_FX_PAN,
        Some(StepEffectKind::Pitch) => ST_FX_PITCH,
        Some(StepEffectKind::Cutoff) => ST_FX_CUTOFF,
        Some(StepEffectKind::Resonance) => ST_FX_RESONANCE,
        Some(StepEffectKind::Delay) => ST_FX_DELAY,
        Some(StepEffectKind::Reverb) => ST_FX_REVERB,
        Some(StepEffectKind::Probability) => ST_FX_PROBABILITY,
        Some(StepEffectKind::Ratchet) => ST_FX_RATCHET,
        Some(StepEffectKind::Reverse) => ST_FX_REVERSE,
        Some(StepEffectKind::Offset) => ST_FX_OFFSET,
        Some(StepEffectKind::Glide) => ST_FX_GLIDE,
        Some(StepEffectKind::Microtiming) => ST_FX_MICROTIMING,
        Some(StepEffectKind::Arpeggio) => ST_FX_ARPEGGIO,
        Some(StepEffectKind::Slice) => ST_FX_SLICE,
        Some(StepEffectKind::Position) => ST_FX_POSITION,
        // A plug-in lock is not an engine effect: it is sent as a lock (`set_plugin_step_fx`).
        Some(StepEffectKind::Plugin) | None => ST_FX_NONE,
    };
    STStepFX { type_: kind, value: if kind == ST_FX_NONE { 0.0 } else { effect.map_or(0.0, |effect| effect.value() as f32) } }
}

fn clamp_i32(value: i64) -> i32 {
    value.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

/// The engine's step for a project step (`AudioProject.step`).
pub fn engine_step(step: &Step) -> STStep {
    STStep {
        note: clamp_i32(step.note()),
        instrument: clamp_i32(step.instrument()),
        velocity: clamp_i32(step.velocity()),
        gate: step.gate() as f32,
        fx1: step_effect(step.fx1.as_ref()),
        fx2: step_effect(step.fx2.as_ref()),
    }
}

fn engine_instrument(item: &st_project::model::Instrument) -> STInstrument {
    let lfo_target = |target: LfoTarget| match target {
        LfoTarget::Cutoff => ST_MOD_CUTOFF,
        LfoTarget::Pitch => ST_MOD_PITCH,
        LfoTarget::Volume => ST_MOD_VOLUME,
        LfoTarget::Pan => ST_MOD_PAN,
        LfoTarget::Position => ST_MOD_POSITION,
        LfoTarget::GrainSize => ST_MOD_GRAIN_SIZE,
    };
    let lfo_shape = |shape: LfoShape| match shape {
        LfoShape::Sine => ST_LFO_SINE,
        LfoShape::Triangle => ST_LFO_TRIANGLE,
        LfoShape::Saw => ST_LFO_SAW,
        LfoShape::Square => ST_LFO_SQUARE,
        LfoShape::Random => ST_LFO_RANDOM,
    };
    let mut p = default_instrument();
    p.type_ = match item.kind() {
        InstrumentKind::Sample => ST_INSTRUMENT_SAMPLE,
        InstrumentKind::Plugin => ST_INSTRUMENT_AU,
        InstrumentKind::Midi => ST_INSTRUMENT_MIDI,
    };
    p.mode = match item.mode() {
        SamplerMode::OneShot => ST_MODE_ONESHOT,
        SamplerMode::Forward => ST_MODE_FORWARD,
        SamplerMode::Reverse => ST_MODE_REVERSE,
        SamplerMode::PingPong => ST_MODE_PINGPONG,
        SamplerMode::Slice => ST_MODE_SLICE,
        SamplerMode::Wavetable => ST_MODE_WAVETABLE,
        SamplerMode::Granular => ST_MODE_GRANULAR,
    };
    p.filter_type = match item.filter_type() {
        FilterType::LowPass => ST_FILTER_LOWPASS,
        FilterType::HighPass => ST_FILTER_HIGHPASS,
        FilterType::BandPass => ST_FILTER_BANDPASS,
    };
    p.root_note = clamp_i32(item.root_note());
    p.slice_count = clamp_i32(item.slice_count());
    p.bit_depth = clamp_i32(item.bit_depth());
    p.volume = item.volume() as f32;
    p.pan = item.pan() as f32;
    p.tune = item.tune() as f32;
    p.start = item.start() as f32;
    p.end = item.end() as f32;
    p.loop_start = item.loop_start() as f32;
    p.loop_end = item.loop_end() as f32;
    p.attack = item.attack() as f32;
    p.decay = item.decay() as f32;
    p.sustain = item.sustain() as f32;
    p.release = item.release() as f32;
    p.cutoff = item.cutoff() as f32;
    p.resonance = item.resonance() as f32;
    p.drive = item.drive() as f32;
    p.lfo_rate = item.lfo_rate() as f32;
    p.lfo_depth = item.lfo_depth() as f32;
    p.grain_size = item.grain_size() as f32;
    p.grain_position = item.grain_position() as f32;
    p.wavetable_position = item.wavetable_position() as f32;
    p.wavetable_frame_size = clamp_i32(item.wavetable_frame_size());
    p.grain_density = clamp_i32(item.grain_density());
    p.grain_spray = item.grain_spray() as f32;
    p.grain_pitch_spread = item.grain_pitch_spread() as f32;
    p.grain_scan = item.grain_scan() as f32;
    p.grain_reverse = item.grain_reverse() as f32;
    p.grain_stereo = item.grain_stereo() as f32;
    p.voice_mode = voice_mode(item.voice_mode());
    p.choke_group = clamp_i32(item.choke_group());
    p.filter_attack = item.filter_attack() as f32;
    p.filter_decay = item.filter_decay() as f32;
    p.filter_sustain = item.filter_sustain() as f32;
    p.filter_release = item.filter_release() as f32;
    p.filter_env_amount = item.filter_env_amount() as f32;
    p.lfo_target = lfo_target(item.lfo_target());
    p.lfo_shape = lfo_shape(item.lfo_shape());
    p.lfo2_target = lfo_target(item.lfo2_target());
    p.lfo2_shape = lfo_shape(item.lfo2_shape());
    p.lfo2_rate = item.lfo2_rate() as f32;
    p.lfo2_depth = item.lfo2_depth() as f32;
    p.lfo_sync = item.lfo_sync() as i32;
    p.lfo2_sync = item.lfo2_sync() as i32;
    p.lfo_beats = item.lfo_beats() as f32;
    p.lfo2_beats = item.lfo2_beats() as f32;
    p
}

fn voice_mode(mode: VoiceMode) -> i32 {
    match mode {
        VoiceMode::Poly => 0,
        VoiceMode::Mono => 1,
        VoiceMode::Legato => 2,
    }
}

/// A sample the engine does not have: still decoding, or unavailable.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SampleProblem {
    /// Instrument index, for an instrument's sample or zone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instrument: Option<usize>,
    pub path: String,
    /// `None` while the sample is being decoded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What one [`ProjectSync::apply`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ApplyReport {
    pub patterns_sent: usize,
    pub patterns_reused: usize,
    pub samples_installed: usize,
    pub problems: Vec<SampleProblem>,
}

/// One point of a host automation lane, resolved to its destination number.
#[derive(Clone, Copy)]
struct HostPoint {
    destination: usize,
    address: u64,
    stepped: i32,
    row: f64,
    value: f64,
}

/// One plug-in step lock, resolved to its destination number.
#[derive(Clone, Copy)]
struct Lock {
    track: i32,
    row: i32,
    /// 0 for the step's first effect, 1 for its second.
    column: i32,
    slot: i32,
    destination: i32,
    address: u64,
    value: f64,
}

/// Identity of an installed sample: its path and which decoded PCM it was.
#[derive(Clone, PartialEq)]
struct Installed {
    path: String,
    pcm: usize,
}

/// What is known to be in the engine. One per engine; control thread only.
pub struct ProjectSync {
    samples: Arc<dyn SampleSource>,
    epoch: u64,
    context: String,
    patterns: HashMap<i32, (Pattern, Vec<u64>)>,
    instrument_samples: Vec<Option<Installed>>,
    zone_signatures: Vec<Option<String>>,
    clip_assets: HashMap<String, (i32, Option<usize>)>,
}

/// The default value of a host destination's address (a fader, a route gain, an insert's
/// parameter): what the destination returns to where no lane drives it.
pub type DestinationDefault<'a> = &'a dyn Fn(&Destination, u64) -> Option<f64>;

impl ProjectSync {
    pub fn new(samples: Arc<dyn SampleSource>) -> ProjectSync {
        ProjectSync { samples, epoch: 0, context: String::new(), patterns: HashMap::new(), instrument_samples: Vec::new(), zone_signatures: Vec::new(), clip_assets: HashMap::new() }
    }

    /// Replaces the sample source. Everything is installed again by the next `apply`.
    pub fn set_samples(&mut self, samples: Arc<dyn SampleSource>) {
        self.samples = samples;
    }

    fn lookup(&self, path: &str, instrument: Option<usize>, problems: &mut Vec<SampleProblem>) -> Option<Arc<SamplePcm>> {
        match self.samples.sample(path) {
            SampleLookup::Ready(pcm) if pcm.frames() > 0 => Some(pcm),
            SampleLookup::Ready(_) => {
                problems.push(SampleProblem { instrument, path: path.to_string(), error: Some("The sample is empty.".to_string()) });
                None
            }
            SampleLookup::Pending => {
                problems.push(SampleProblem { instrument, path: path.to_string(), error: None });
                None
            }
            SampleLookup::Missing(error) => {
                problems.push(SampleProblem { instrument, path: path.to_string(), error: Some(error) });
                None
            }
        }
    }

    /// The engine asset holding the audio of `path` (a frozen track's print), installed if it is
    /// not yet; `None` while the file is not decoded or cannot be. Like timeline clips, prints
    /// share the engine's 256 asset slots, one per distinct file.
    fn print_asset(&mut self, control: &mut tracker_engine::engine::control::Control, shared: &tracker_engine::engine::state::Shared, path: &str, report: &mut ApplyReport) -> Result<Option<i32>, String> {
        if !self.clip_assets.contains_key(path) {
            let used: Vec<i32> = self.clip_assets.values().map(|(slot, _)| *slot).collect();
            let Some(slot) = (0..256).find(|slot| !used.contains(slot)) else { return Ok(None) };
            self.clip_assets.insert(path.to_string(), (slot, None));
        }
        let pcm = self.lookup(path, None, &mut report.problems);
        let (slot, installed) = self.clip_assets[path];
        let identity = pcm.as_ref().map(|pcm| Arc::as_ptr(pcm) as usize);
        if installed != identity {
            let accepted = control.set_audio_asset(shared, slot, pcm.as_ref().map(|pcm| Pcm { left: &pcm.left, right: Some(&pcm.right), rate: pcm.sample_rate }));
            if pcm.is_some() && !accepted {
                return Err(format!("The engine could not load the frozen audio {path}."));
            }
            report.samples_installed += pcm.is_some() as usize;
            self.clip_assets.insert(path.to_string(), (slot, identity));
        }
        Ok(pcm.is_some().then_some(slot))
    }

    /// Sends `project` to the engine and publishes it. `spec` is the project's mixer (for the
    /// numbering of host automation). On an error the engine keeps the project it had.
    pub fn apply(&mut self, engine: &mut ControlHalf, project: &Project, spec: &GraphSpec, defaults: DestinationDefault<'_>) -> Result<ApplyReport, String> {
        let result = self.send(engine, project, spec, defaults);
        if result.is_err() {
            let (control, _, _) = engine.parts();
            control.cancel_update();
            // The cancelled draft took the samples of this update with it.
            self.epoch = 0;
            self.patterns.clear();
            self.instrument_samples.clear();
            self.zone_signatures.clear();
            self.clip_assets.clear();
        }
        result
    }

    fn send(&mut self, engine: &mut ControlHalf, project: &Project, spec: &GraphSpec, defaults: DestinationDefault<'_>) -> Result<ApplyReport, String> {
        let mut report = ApplyReport::default();
        let tracks = project.track_list();
        let instruments = project.instrument_list();
        let (control, shared, _) = engine.parts();
        control.begin_update(shared);
        control.clear_project();
        control.set_host_automation_map(spec.host_map);
        control.set_timing(project.bpm(), clamp_i32(project.lines_per_beat()), project.swing() as f32);
        let (numerator, denominator) = project.meter();
        control.set_metronome(project.metronome(), clamp_i32(numerator), clamp_i32(denominator), project.metronome_volume() as f32);
        if tracks.len() > ST_TRACK_SANITY_LIMIT as usize || !control.set_track_count(tracks.len().max(1) as i32) {
            return Err(format!("A project supports at most {ST_TRACK_SANITY_LIMIT} tracks."));
        }
        if !control.set_instrument_count(instruments.len().clamp(1, ST_INSTRUMENT_SANITY_LIMIT as usize) as i32) {
            return Err(format!("A project supports at most {ST_INSTRUMENT_SANITY_LIMIT} instruments."));
        }
        // Track choke groups are scoped to the track's group (1 + its index), 0 for ungrouped tracks.
        let group_scope = |id: &str| project.groups.as_deref().unwrap_or(&[]).iter().position(|group| group.id.as_deref() == Some(id)).map_or(0, |index| index + 1);
        for (index, item) in tracks.iter().enumerate() {
            let choke = item.choke_group().clamp(0, 16) as i32;
            let mode = item.voice_mode.map_or(-1, voice_mode);
            if choke > 0 || mode >= 0 {
                let scope = item.group_id.as_deref().map_or(0, group_scope).min(ST_TRACK_SANITY_LIMIT as usize) as i32;
                if !control.set_track_voice(index as i32, choke, scope, mode) {
                    return Err("The engine rejected a track's choke group or voice mode.".to_string());
                }
            }
            control.set_track(
                index as i32,
                STTrack {
                    volume: item.volume() as f32,
                    pan: item.pan() as f32,
                    delay_send: item.delay_send() as f32,
                    reverb_send: item.reverb_send() as f32,
                    mute: item.mute() as i32,
                    solo: item.solo() as i32,
                    instrument: clamp_i32(item.instrument()),
                },
            );
        }

        // Instruments and their samples.
        let count = instruments.len().min(ST_INSTRUMENT_SANITY_LIMIT as usize);
        self.instrument_samples.resize(self.instrument_samples.len().max(count), None);
        self.zone_signatures.resize(self.zone_signatures.len().max(count), None);
        for (index, item) in instruments.iter().take(count).enumerate() {
            control.set_instrument(index as i32, engine_instrument(item));
            let slices: Vec<f32> = item.slice_markers().iter().map(|value| *value as f32).collect();
            control.set_slice_points(index as i32, &slices);
            let wanted = item.sample_path.as_deref().filter(|path| !path.is_empty() && item.kind() == InstrumentKind::Sample);
            let pcm = wanted.and_then(|path| self.lookup(path, Some(index), &mut report.problems));
            let installed = wanted.zip(pcm.as_ref()).map(|(path, pcm)| Installed { path: path.to_string(), pcm: Arc::as_ptr(pcm) as usize });
            if self.instrument_samples[index] != installed {
                match &pcm {
                    Some(pcm) => {
                        if !control.set_sample(shared, index as i32, Some(Pcm { left: &pcm.left, right: Some(&pcm.right), rate: pcm.sample_rate })) {
                            return Err(format!("The engine could not load the sample of instrument {}.", index + 1));
                        }
                        control.set_sample_cycle(shared, index as i32, pcm.cycle_frames.unwrap_or(0) as i32);
                        report.samples_installed += 1;
                    }
                    None => {
                        control.set_sample(shared, index as i32, None);
                    }
                }
                self.instrument_samples[index] = installed;
            }
            // Multisample zones: all of an instrument's zones are replaced together.
            let zones = item.zones.as_deref().unwrap_or(&[]);
            let zone_pcm: Vec<Option<Arc<SamplePcm>>> = zones.iter().map(|zone| zone.file().and_then(|path| self.lookup(path, Some(index), &mut report.problems))).collect();
            let signature = (!zones.is_empty()).then(|| format!("{zones:?}|{:?}", zone_pcm.iter().map(|pcm| pcm.as_ref().map_or(0, |pcm| Arc::as_ptr(pcm) as usize)).collect::<Vec<_>>()));
            if self.zone_signatures[index] != signature {
                control.clear_zones(index as i32);
                for (zone, pcm) in zones.iter().zip(&zone_pcm) {
                    let Some(pcm) = pcm else { continue };
                    let accepted = control.add_sample_zone(
                        shared,
                        index as i32,
                        Pcm { left: &pcm.left, right: Some(&pcm.right), rate: pcm.sample_rate },
                        clamp_i32(zone.key_low()),
                        clamp_i32(zone.key_high()),
                        clamp_i32(zone.velocity_low()),
                        clamp_i32(zone.velocity_high()),
                        clamp_i32(zone.root_note()),
                        zone.tune() as f32,
                        zone.volume() as f32,
                    );
                    if !accepted {
                        return Err(format!("The engine rejected a multisample zone of instrument {}.", index + 1));
                    }
                    report.samples_installed += 1;
                }
                self.zone_signatures[index] = signature;
            }
        }
        for index in count..self.instrument_samples.len() {
            if self.instrument_samples[index].take().is_some() {
                control.set_sample(shared, index as i32, None);
            }
            if self.zone_signatures[index].take().is_some() {
                control.clear_zones(index as i32);
            }
        }

        // Host automation: every (destination, address) some lane or lock drives, anywhere.
        let mut driven: Vec<(usize, u64)> = Vec::new();
        let mut host_points: HashMap<i32, Vec<HostPoint>> = HashMap::new();
        let mut locks: HashMap<i32, Vec<Lock>> = HashMap::new();
        let patterns = project.pattern_list();
        let numbered: Vec<(i32, &Pattern)> = patterns.iter().take(ST_MAX_PATTERNS as usize).enumerate().map(|(index, pattern)| (clamp_i32(pattern.number(index)), pattern)).collect();
        for (number, pattern) in &numbered {
            for lane in pattern.automation.as_deref().unwrap_or(&[]) {
                let Some(owner) = lane.owner.as_deref() else { continue };
                let stepped = (lane.interpolation() == Interpolation::Step) as i32;
                for (destination, address) in spec.lane_destinations(owner, lane.target_name()) {
                    let Some(index) = spec.destination_index(&destination) else { continue };
                    if !driven.contains(&(index, address)) {
                        driven.push((index, address));
                    }
                    let points = host_points.entry(*number).or_default();
                    // An insert's parameter is normalized; nothing below zero may reach the
                    // mixer from a lane, where it would read as "the base value".
                    let normalized = matches!(destination, Destination::Insert(_));
                    for point in lane.points.as_deref().unwrap_or(&[]) {
                        let value = if normalized { point.value().clamp(0.0, 1.0) } else { point.value() };
                        points.push(HostPoint { destination: index, address, stepped, row: point.row(), value });
                    }
                }
            }
            for (track, rows) in pattern.steps.as_deref().unwrap_or(&[]).iter().enumerate().take(tracks.len()) {
                for (row, step) in rows.iter().enumerate().take(ST_MAX_ROWS as usize) {
                    let Some(step) = step else { continue };
                    for (column, effect) in [step.fx1.as_ref(), step.fx2.as_ref()].into_iter().enumerate() {
                        let Some(effect) = effect.filter(|effect| effect.kind == Some(StepEffectKind::Plugin)) else { continue };
                        let (Some(instance), Some(address)) = (effect.instance_id.as_deref(), effect.plugin_address()) else { continue };
                        let Some((destination, slot)) = spec.lock_destination(track, instance) else { continue };
                        let Some(index) = spec.destination_index(&destination) else { continue };
                        if !driven.contains(&(index, address)) {
                            driven.push((index, address));
                        }
                        locks.entry(*number).or_default().push(Lock { track: track as i32, row: row as i32, column: column as i32, slot: slot as i32, destination: index as i32, address, value: effect.value().clamp(0.0, 1.0) });
                    }
                }
            }
        }
        // A pattern without a lane for a driven destination holds that destination's default,
        // so the value returns when playback leaves the pattern (or the row) that drove it.
        // For an insert's parameter the default is "the base value", which the insert resolves
        // when the event arrives: the user may have turned the control since this publication.
        for (number, _) in &numbered {
            let points = host_points.entry(*number).or_default();
            for (index, address) in &driven {
                if !points.iter().any(|point| point.destination == *index && point.address == *address) {
                    let Some(destination) = spec.destinations.get(*index) else { continue };
                    if let Some(value) = defaults(destination, *address) {
                        let value = if matches!(destination, Destination::Insert(_)) { f64::from(crate::graph::automation::BASE_VALUE) } else { value };
                        points.push(HostPoint { destination: *index, address: *address, stepped: 1, row: 0.0, value });
                    }
                }
            }
        }

        // Patterns, reusing what the engine already has.
        let context = format!("{}|{:x}|{}", tracks.len(), spec.host_map, (0..tracks.len()).map(|index| track_owner(project, index)).collect::<Vec<_>>().join(","));
        let reusable = self.epoch != 0 && self.epoch == control.pattern_epoch() && self.context == context;
        let mut sent: HashMap<i32, (Pattern, Vec<u64>)> = HashMap::with_capacity(numbered.len());
        for (number, pattern) in &numbered {
            let number = *number;
            let mut derived: Vec<u64> = Vec::new();
            for point in host_points.get(&number).into_iter().flatten() {
                derived.extend([0, point.destination as u64, point.address, point.stepped as u64, point.row.to_bits(), point.value.to_bits()]);
            }
            for lock in locks.get(&number).into_iter().flatten() {
                derived.extend([1, lock.track as u64, lock.row as u64, lock.column as u64, lock.slot as u64, lock.destination as u64, lock.address, lock.value.to_bits()]);
            }
            let unchanged = reusable && self.patterns.get(&number).is_some_and(|(source, key)| *key == derived && source == *pattern);
            if unchanged && control.reuse_pattern(number) {
                report.patterns_reused += 1;
                sent.insert(number, self.patterns.remove(&number).unwrap_or_else(|| ((*pattern).clone(), derived)));
                continue;
            }
            report.patterns_sent += 1;
            control.set_pattern(number, clamp_i32(pattern.length()));
            for (track, rows) in pattern.steps.as_deref().unwrap_or(&[]).iter().enumerate().take(tracks.len()) {
                let extra_columns = tracks[track].note_column_count() - 1;
                for (row, step) in rows.iter().enumerate().take(ST_MAX_ROWS as usize) {
                    let Some(step) = step else { continue };
                    let chord: Vec<i32> = step.chord.as_deref().unwrap_or(&[]).iter().take(ST_MAX_CHORD).map(|note| clamp_i32(*note)).collect();
                    control.set_step(number, track as i32, row as i32, engine_step(step), (!chord.is_empty()).then_some(&chord[..]));
                    // Extra note columns: `columns[i]` is note column `i + 1`; a missing gate is no gate.
                    for (column, cell) in step.columns.as_deref().unwrap_or(&[]).iter().take(extra_columns).enumerate() {
                        let Some(cell) = cell else { continue };
                        let cell = STStep { note: clamp_i32(cell.note()), instrument: clamp_i32(cell.instrument()), velocity: clamp_i32(cell.velocity()), gate: cell.gate() as f32, ..default_step() };
                        control.set_step_column(number, track as i32, row as i32, column as i32 + 1, cell);
                    }
                }
            }
            // The engine's sequencer plays the lanes of a track's own mix and instrument
            // parameters, which name the track by index. Lanes with an owner are the mixer's
            // (the host points above).
            for lane in pattern.automation.as_deref().unwrap_or(&[]).iter().filter(|lane| lane.owner.is_none()) {
                // Unknown names are skipped, never retargeted to track volume.
                let Some(target) = native_target(lane.target_name()) else { continue };
                let stepped = (lane.interpolation() == Interpolation::Step) as i32;
                for point in lane.points.as_deref().unwrap_or(&[]) {
                    control.add_automation_point(number, clamp_i32(lane.track()), target, stepped, point.row(), point.value());
                }
            }
            for point in host_points.get(&number).into_iter().flatten() {
                control.add_host_automation_point(number, point.destination as i32, point.address, point.stepped, point.row, point.value);
            }
            for lock in locks.get(&number).into_iter().flatten() {
                control.set_plugin_step_fx(number, lock.track, lock.row, lock.column, lock.destination, lock.address, lock.value);
            }
            sent.insert(number, ((*pattern).clone(), derived));
        }

        let sequence: Vec<i32> = project.song_sequence().iter().take(1024).map(|number| clamp_i32(*number)).collect();
        control.set_sequence(&sequence, project.loop_song(), clamp_i32(project.active_pattern()));
        // The song section loop is honoured only while Loop song is on.
        let section = project.song_loop.as_ref().filter(|_| project.loop_song());
        control.set_song_loop(section.and_then(|section| section.start).map_or(-1, clamp_i32), section.and_then(|section| section.end).map_or(-1, clamp_i32));

        // Timeline clips: each distinct file is an engine asset.
        control.clear_audio_clips();
        let clips = project.clip_list();
        // Prints of frozen tracks that are not timeline clips (`crate::freeze`): they use assets too.
        let prints = crate::freeze::engine_prints(project);
        let mut wanted: Vec<&str> = clips.iter().filter_map(|clip| clip.file()).collect();
        wanted.extend(prints.iter().flat_map(|print| print.paths()));
        let stale: Vec<String> = self.clip_assets.keys().filter(|path| !wanted.contains(&path.as_str())).cloned().collect();
        for path in stale {
            if let Some((slot, Some(_))) = self.clip_assets.remove(&path) {
                control.set_audio_asset(shared, slot, None);
            }
        }
        for clip in clips {
            let Some(path) = clip.file() else { continue };
            if !self.clip_assets.contains_key(path) {
                let used: Vec<i32> = self.clip_assets.values().map(|(slot, _)| *slot).collect();
                let Some(slot) = (0..256).find(|slot| !used.contains(slot)) else {
                    return Err("A project supports at most 256 distinct audio clip files.".to_string());
                };
                self.clip_assets.insert(path.to_string(), (slot, None));
            }
            let pcm = self.lookup(path, None, &mut report.problems);
            let (slot, installed) = self.clip_assets[path];
            let identity = pcm.as_ref().map(|pcm| Arc::as_ptr(pcm) as usize);
            if installed != identity {
                let accepted = control.set_audio_asset(shared, slot, pcm.as_ref().map(|pcm| Pcm { left: &pcm.left, right: Some(&pcm.right), rate: pcm.sample_rate }));
                if pcm.is_some() && !accepted {
                    return Err(format!("The engine could not load the audio clip {path}."));
                }
                report.samples_installed += pcm.is_some() as usize;
                self.clip_assets.insert(path.to_string(), (slot, identity));
            }
            if pcm.is_some() {
                control.add_audio_clip(
                    slot,
                    clamp_i32(clip.track()),
                    clip.start_beat(),
                    clip.duration_beats(),
                    clip.source_offset(),
                    clip.gain() as f32,
                    clip.fade_in() as f32,
                    clip.fade_out() as f32,
                    clip.muted(),
                );
            }
        }
        for print in &prints {
            match print {
                crate::freeze::EnginePrint::Pattern { track, pattern, head, loop_print } => {
                    // Both files are asked for at once, so that they are decoded together.
                    let head = self.print_asset(control, shared, head, &mut report)?;
                    let loop_asset = match loop_print {
                        Some(path) => self.print_asset(control, shared, path, &mut report)?.unwrap_or(-1),
                        None => -1,
                    };
                    if let Some(head) = head {
                        control.add_pattern_clip(head, loop_asset, *track as i32, clamp_i32(*pattern), 1.0);
                    }
                }
                crate::freeze::EnginePrint::SongTail { track, path, start_frame } => {
                    if let Some(asset) = self.print_asset(control, shared, path, &mut report)? {
                        control.add_song_tail_clip(asset, *track as i32, *start_frame, 1.0);
                    }
                }
            }
        }
        report.problems.dedup();
        control.commit(shared)?;
        self.epoch = control.pattern_epoch();
        self.context = context;
        self.patterns = sent;
        Ok(report)
    }
}

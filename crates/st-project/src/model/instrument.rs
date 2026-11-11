//! Instruments: the sampler (all its modes), plug-in instruments and external MIDI instruments.
//!
//! Defaults are those of `instrumentDefaults` in `ui/app-project-model.js`.

use crate::json::{model, string_enum};
use crate::model::plugin::PluginDescriptor;
use std::collections::BTreeMap;

string_enum! {
    /// What produces an instrument's sound.
    pub enum InstrumentKind {
        /// The built-in sampler (one-shot, loops, slices, wavetable, granular).
        Sample = "sample",
        /// A plug-in instrument; its descriptor is the `audioPlugins` entry of kind `instrument`
        /// on each track that plays it (`Instrument::plugin` while none does). Any plug-in format.
        Plugin = "plugin",
        /// An external MIDI instrument: notes go to a MIDI output, nothing is rendered.
        Midi = "midi",
    }
}

string_enum! {
    /// How the sampler plays its sample.
    pub enum SamplerMode {
        /// Plays the region once.
        OneShot = "oneshot",
        /// Loops forward between the loop points.
        Forward = "forward",
        /// Plays the region backwards.
        Reverse = "reverse",
        /// Loops forward and backward between the loop points.
        PingPong = "pingpong",
        /// Notes select slices of the sample.
        Slice = "slice",
        /// The sample is a wavetable; `wavetablePosition` selects the frame.
        Wavetable = "wavetable",
        /// Granular playback around `grainPosition`.
        Granular = "granular",
    }
}

string_enum! {
    /// The sampler's filter.
    pub enum FilterType {
        /// Low-pass.
        LowPass = "lowpass",
        /// High-pass.
        HighPass = "highpass",
        /// Band-pass.
        BandPass = "bandpass",
    }
}

string_enum! {
    /// What an LFO modulates.
    pub enum LfoTarget {
        /// Filter cutoff.
        Cutoff = "cutoff",
        /// Pitch.
        Pitch = "pitch",
        /// Volume.
        Volume = "volume",
        /// Pan.
        Pan = "pan",
        /// Wavetable or grain position.
        Position = "position",
        /// Grain size.
        GrainSize = "grainSize",
    }
}

string_enum! {
    /// An LFO's waveform.
    pub enum LfoShape {
        /// Sine.
        Sine = "sine",
        /// Triangle.
        Triangle = "triangle",
        /// Saw.
        Saw = "saw",
        /// Square.
        Square = "square",
        /// Sample and hold.
        Random = "random",
    }
}

string_enum! {
    /// How many notes sound at once.
    pub enum VoiceMode {
        /// Every note gets a voice.
        Poly = "poly",
        /// One voice; a new note retriggers.
        Mono = "mono",
        /// One voice; overlapping notes glide without retriggering.
        Legato = "legato",
    }
}

model! {
    /// One sample of a multisample instrument, with its key and velocity range.
    /// Created by the sampler page (`sampler.js`): `{samplePath, keyLow: 0, keyHigh: 127,
    /// velocityLow: 0, velocityHigh: 127, rootNote, tune: 0, volume: 1}`.
    pub struct Zone {
        /// The sample file (media path).
        "samplePath" => sample_path: String,
        /// Alias of `samplePath` written by imports; both are media paths and are kept in step.
        "path" => path: String,
        /// Lowest key (MIDI note) the zone answers.
        "keyLow" => key_low: i64 = 0,
        /// Highest key the zone answers.
        "keyHigh" => key_high: i64 = 127,
        /// Lowest velocity the zone answers.
        "velocityLow" => velocity_low: i64 = 0,
        /// Highest velocity the zone answers.
        "velocityHigh" => velocity_high: i64 = 127,
        /// The note at which the sample plays at its recorded pitch.
        "rootNote" => root_note: i64 = 60,
        /// Tuning in semitones.
        "tune" => tune: f64 = 0.0,
        /// Level, 1 = unchanged.
        "volume" => volume: f64 = 1.0,
    }
}

impl Zone {
    /// The zone's sample file: `samplePath`, else `path`.
    pub fn file(&self) -> Option<&str> {
        self.sample_path.as_deref().filter(|path| !path.is_empty()).or(self.path.as_deref().filter(|path| !path.is_empty()))
    }
}

model! {
    /// An instrument. Tracks and steps refer to instruments by their index in `project.instruments`.
    pub struct Instrument {
        /// The instrument's index, as the interface numbers it.
        "id" => id: i64,
        /// Display name.
        "name" => name: String,
        /// What produces the sound. Absent reads as the sampler.
        "type" => kind: InstrumentKind = InstrumentKind::Sample,
        /// The sample file: an absolute path at run time, relative to the project file on disk, or
        /// a `builtin:` reference to application content. Empty for none.
        "samplePath" => sample_path: String,
        /// The note at which the sample plays at its recorded pitch.
        "rootNote" => root_note: i64 = 60,
        /// Level, 0..2.
        "volume" => volume: f64 = 0.7,
        /// Pan, -1..1.
        "pan" => pan: f64 = 0.0,
        /// Tuning in semitones.
        "tune" => tune: f64 = 0.0,
        /// Sampler playback mode.
        "mode" => mode: SamplerMode = SamplerMode::OneShot,
        /// Region start, 0..1 of the sample.
        "start" => start: f64 = 0.0,
        /// Region end, 0..1.
        "end" => end: f64 = 1.0,
        /// Loop start, 0..1.
        "loopStart" => loop_start: f64 = 0.0,
        /// Loop end, 0..1.
        "loopEnd" => loop_end: f64 = 1.0,
        /// Amplitude envelope attack, seconds.
        "attack" => attack: f64 = 0.005,
        /// Amplitude envelope decay, seconds.
        "decay" => decay: f64 = 0.2,
        /// Amplitude envelope sustain, 0..1.
        "sustain" => sustain: f64 = 0.7,
        /// Amplitude envelope release, seconds.
        "release" => release: f64 = 0.2,
        /// Filter cutoff, Hz.
        "cutoff" => cutoff: f64 = 18000.0,
        /// Filter resonance, 0..1.
        "resonance" => resonance: f64 = 0.0,
        /// Filter type.
        "filterType" => filter_type: FilterType = FilterType::LowPass,
        /// Drive, 0..1.
        "drive" => drive: f64 = 0.0,
        /// Bit depth reduction, 1..24 (16 = clean).
        "bitDepth" => bit_depth: i64 = 16,
        /// LFO 1 rate, Hz (when not synced).
        "lfoRate" => lfo_rate: f64 = 0.0,
        /// LFO 1 follows the tempo.
        "lfoSync" => lfo_sync: bool = false,
        /// LFO 1 period in beats when synced.
        "lfoBeats" => lfo_beats: f64 = 1.0,
        /// LFO 1 depth, 0..1.
        "lfoDepth" => lfo_depth: f64 = 0.0,
        /// LFO 1 destination.
        "lfoTarget" => lfo_target: LfoTarget = LfoTarget::Cutoff,
        /// LFO 1 waveform.
        "lfoShape" => lfo_shape: LfoShape = LfoShape::Sine,
        /// LFO 2 rate, Hz.
        "lfo2Rate" => lfo2_rate: f64 = 0.0,
        /// LFO 2 follows the tempo.
        "lfo2Sync" => lfo2_sync: bool = false,
        /// LFO 2 period in beats when synced.
        "lfo2Beats" => lfo2_beats: f64 = 1.0,
        /// LFO 2 depth, 0..1.
        "lfo2Depth" => lfo2_depth: f64 = 0.0,
        /// LFO 2 destination.
        "lfo2Target" => lfo2_target: LfoTarget = LfoTarget::Cutoff,
        /// LFO 2 waveform.
        "lfo2Shape" => lfo2_shape: LfoShape = LfoShape::Sine,
        /// Filter envelope attack, seconds.
        "filterAttack" => filter_attack: f64 = 0.005,
        /// Filter envelope decay, seconds.
        "filterDecay" => filter_decay: f64 = 0.2,
        /// Filter envelope sustain, 0..1.
        "filterSustain" => filter_sustain: f64 = 0.0,
        /// Filter envelope release, seconds.
        "filterRelease" => filter_release: f64 = 0.2,
        /// Filter envelope amount, -1..1.
        "filterEnvAmount" => filter_env_amount: f64 = 0.0,
        /// Number of equal slices in Slice mode when there are no slice markers, 1..128.
        "sliceCount" => slice_count: i64 = 16,
        /// Slice markers, 0..1 of the sample, ascending. Empty or absent: `sliceCount` equal slices.
        "slicePoints" => slice_points: Vec<f64>,
        /// Mirror of `slicePoints` the sampler page writes alongside it.
        "slices" => slices: Vec<f64>,
        /// Grain length, seconds.
        "grainSize" => grain_size: f64 = 0.08,
        /// Grain position, 0..1 of the sample.
        "grainPosition" => grain_position: f64 = 0.5,
        /// Overlapping grains, 1..8.
        "grainDensity" => grain_density: i64 = 2,
        /// Random position spread, 0..1.
        "grainSpray" => grain_spray: f64 = 0.0,
        /// Random pitch spread, semitones 0..24.
        "grainPitchSpread" => grain_pitch_spread: f64 = 0.0,
        /// Scan speed through the sample, -4..4 (0 = frozen).
        "grainScan" => grain_scan: f64 = 0.0,
        /// Share of reversed grains, 0..1.
        "grainReverse" => grain_reverse: f64 = 0.0,
        /// Stereo spread of grains, 0..1.
        "grainStereo" => grain_stereo: f64 = 0.0,
        /// Wavetable frame position, 0..1.
        "wavetablePosition" => wavetable_position: f64 = 0.0,
        /// Samples per wavetable frame; 0 = detect.
        "wavetableFrameSize" => wavetable_frame_size: i64 = 0,
        /// Multisample zones (up to 128). With none, the main sample plays across the keyboard.
        "zones" => zones: Vec<Zone>,
        /// Voice mode. A track's own `voiceMode` overrides it.
        "voiceMode" => voice_mode: VoiceMode = VoiceMode::Poly,
        /// The plug-in of a plug-in instrument that no track plays: its descriptor without owner
        /// and instance, kept so the instrument can be assigned to a track again with its settings.
        /// Absent otherwise: while a track plays the instrument, the plug-in is that track's
        /// `audioPlugins` entry of kind `instrument` and nowhere else. The host loads nothing
        /// from here.
        "plugin" => plugin: PluginDescriptor,
        /// MIDI input device for playing this instrument (0 = any).
        "midiInput" => midi_input: i64 = 0,
        /// MIDI output device of an external instrument (0 = none).
        "midiOutput" => midi_output: i64 = 0,
        /// MIDI input channel filter, 0 = all, else 1..16.
        "midiInputChannel" => midi_input_channel: i64 = 0,
        /// MIDI output channel, 1..16. MIDI export falls back to the track's channel when absent.
        "midiChannel" => midi_channel: i64 = 1,
        /// Program change sent before notes (MIDI export, external instruments), 0..127.
        "midiProgram" => midi_program: i64,
        /// Bank select MSB that goes with `midiProgram`.
        "midiBankMSB" => midi_bank_msb: i64 = 0,
        /// Bank select LSB that goes with `midiProgram`.
        "midiBankLSB" => midi_bank_lsb: i64 = 0,
        /// Controller numbers for automation targets and step effects, e.g. `{"cutoff": 74}`.
        "midiCCMap" => midi_cc_map: BTreeMap<String, i64>,
        /// The parameter range `[minimum, maximum]` that maps to controller values 0..127.
        "midiCCRanges" => midi_cc_ranges: BTreeMap<String, Vec<f64>>,
        /// Monitor the instrument's input while record-armed.
        "monitor" => monitor: bool = false,
    }
}

impl Instrument {
    /// The name shown for the instrument at `index`.
    pub fn display_name(&self, index: usize) -> String {
        match self.name.as_deref() {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => format!("Instrument {}", index + 1),
        }
    }

    /// The slice markers in effect: `slicePoints`, else `slices`.
    pub fn slice_markers(&self) -> &[f64] {
        match (&self.slice_points, &self.slices) {
            (Some(points), _) if !points.is_empty() => points,
            (_, Some(points)) => points,
            (Some(points), None) => points,
            (None, None) => &[],
        }
    }
}

/// The prefix of sample references that name application content (built-in wavetables and
/// drums) rather than files: never collected, never missing.
pub const BUILTIN_PREFIX: &str = "builtin:";

/// Whether `path` names built-in application content.
pub fn is_builtin(path: &str) -> bool {
    path.starts_with(BUILTIN_PREFIX)
}

/// The reference to a built-in single-cycle or morph wavetable, e.g. `("Basic", "Saw")`.
pub fn builtin_wavetable(group: &str, name: &str) -> String {
    format!("{BUILTIN_PREFIX}wavetables/{group}/{name}.wav")
}

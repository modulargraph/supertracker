//! A plug-in instance as a processor of the mixer graph, and the factory that makes them.
//!
//! [`PluginInsert`] implements [`InsertProcessor`] on top of the client's real-time `process`
//! call. One call renders one block: the graph hands it parameter changes with their frame
//! offsets ([`InsertProcessor::parameter_at`]) instead of splitting the block, so every block
//! costs one round trip to the container whatever the automation does.
//!
//! * **Effect.** The strip's signal goes in and comes back in place; a routed sidechain is the
//!   second input bus. While the plug-in cannot answer (deadline missed, container gone,
//!   a control operation holds it) the client returns the dry signal delayed by the plug-in's
//!   latency, so the mixer's delay compensation stays right: a lost insert passes through.
//! * **Instrument.** The first processor of a track strip. The engine's note events of the
//!   track arrive through [`InsertProcessor::midi`] with their frame offsets; controllers,
//!   pressure and pitch bend played live ([`Hosted::send_midi`]) arrive at the first frame of
//!   the next block, after the notes of that frame; the plug-in's
//!   stereo output, times the instrument's volume and balanced by its pan, is **added** to
//!   the strip's signal (the engine's own channel: clips and sample voices of the track), as
//!   the previous host mixed both sources of a channel. The strip's signal is delayed by the
//!   plug-in's latency first, so both stay aligned. A lost instrument is silent.
//!
//! Deadline: a block may take what is left of the render call it belongs to
//! ([`BlockInfo::cycle_end_ns`], set by the renderer from the device callback's length), but
//! at least the manager's `min_deadline`. Instances of an offline manager have no deadline.
//!
//! Notes: the processor tracks the notes it sent on. After `reset` (panic), after a block the
//! client reports as the first one following dropped blocks, and when bypass ends, it sends
//! "all notes off" and "all sound off" before anything else, and a note-off for every note it
//! believes held, at frame 0 of the next block.
//!
//! Base values: a parameter a lane or a step lock moved is remembered ([`Hosted`]'s automated
//! set) and goes back to the value the user left it at when the graph says so
//! ([`InsertProcessor::base_parameter`]: a new plan, a changed project, every start, stop
//! and seek of the transport, and the end of a step lock's row where no lane takes over).
//! Parameters automation never touched are left alone: the plug-in holds its own values.
//!
//! Nothing here allocates, locks or blocks beyond the client's bounded wait.

use super::manager::{Hosted, Lookup, PluginManager, EVENT_CAPACITY};
use crate::graph::insert::{monotonic_ns, BlockInfo, InsertFactory, InsertProcessor, ProcessContext};
use crate::graph::pdc::DelayLine;
use crate::graph::spec::InsertSpec;
use std::sync::Arc;
use stpc_client::{
    Event, RawBlock, Transport, DEADLINE_NONE, EVENT_FLAG_SEQUENCED, PROCESS_OK, PROCESS_RESYNCED, TRANSPORT_PLAYING, TRANSPORT_PPQ_VALID, TRANSPORT_RECORDING, TRANSPORT_TEMPO_VALID,
    TRANSPORT_TIME_SIG_VALID,
};

/// Reserved addresses of an instrument's mix, given as base parameters: the instrument's
/// volume (0..2) and pan (-1..1), not normalized.
pub const INSTRUMENT_VOLUME: u64 = u64::MAX - 1;
pub const INSTRUMENT_PAN: u64 = u64::MAX - 2;

struct InstrumentState {
    volume: f32,
    pan: f32,
    /// Notes sent on and not yet off.
    held: [u64; 2],
    out_left: Vec<f32>,
    out_right: Vec<f32>,
    /// Aligns the strip's own signal with the plug-in's latency.
    dry: Option<DelayLine>,
}

pub struct PluginInsert {
    hosted: Arc<Hosted>,
    instrument: Option<InstrumentState>,
    bypass: bool,
    latency: u32,
    sample_rate: f64,
    /// Parameter changes of the next block, in frame order.
    parameters: Vec<Event>,
    /// Notes and controllers of the next block, in arrival order.
    notes: Vec<Event>,
    merged: Vec<Event>,
    out_events: Vec<Event>,
    scratch_left: Vec<f32>,
    scratch_right: Vec<f32>,
    silence_notes: bool,
    /// The last block was answered by the fallback.
    dropped: bool,
    /// Blocks answered by the fallback so far (for tests and diagnostics).
    pub fallback_blocks: u64,
}

impl PluginInsert {
    pub fn new(hosted: Arc<Hosted>, mix: Option<(f32, f32)>) -> PluginInsert {
        let instrument = hosted.instrument.then(|| {
            let (volume, pan) = mix.unwrap_or((1.0, 0.0));
            InstrumentState { volume, pan, held: [0; 2], out_left: Vec::new(), out_right: Vec::new(), dry: None }
        });
        PluginInsert {
            hosted,
            instrument,
            bypass: false,
            latency: 0,
            sample_rate: 48_000.0,
            parameters: Vec::new(),
            notes: Vec::new(),
            merged: Vec::new(),
            out_events: Vec::new(),
            scratch_left: Vec::new(),
            scratch_right: Vec::new(),
            silence_notes: false,
            dropped: false,
            fallback_blocks: 0,
        }
    }

    fn transport(block: &BlockInfo, tempo: f64) -> Transport {
        let mut flags = TRANSPORT_TEMPO_VALID | TRANSPORT_TIME_SIG_VALID | TRANSPORT_PPQ_VALID;
        if block.playing {
            flags |= TRANSPORT_PLAYING;
        }
        if block.recording {
            flags |= TRANSPORT_RECORDING;
        }
        Transport {
            flags,
            time_sig_numerator: block.numerator.max(1),
            time_sig_denominator: block.denominator.max(1),
            tempo_bpm: if block.bpm > 0.0 { block.bpm } else { tempo },
            sample_position: block.sample_position,
            ppq_position: block.beat,
            bar_start_ppq: block.bar_start_beat,
            loop_start_ppq: 0.0,
            loop_end_ppq: 0.0,
        }
    }

    /// Builds the block's event list: note silence first when it is due, then notes and
    /// parameter changes by frame (a parameter change before a note on the same frame, so a
    /// step's lock applies to the step's note).
    fn merge_events(&mut self, frames: u32, silence: bool) {
        self.merged.clear();
        if let (true, Some(instrument)) = (silence, self.instrument.as_mut()) {
            for controller in [123u8, 120] {
                self.merged.push(Event::midi(0, &[0xB0, controller, 0]));
            }
            for note in 0..128u8 {
                if instrument.held[(note / 64) as usize] & (1 << (note % 64)) != 0 {
                    self.merged.push(Event::note_off(0, 0, note));
                }
            }
            instrument.held = [0; 2];
        }
        let (mut note, mut parameter) = (0, 0);
        while note < self.notes.len() || parameter < self.parameters.len() {
            let take_parameter = match (self.notes.get(note), self.parameters.get(parameter)) {
                (Some(a), Some(b)) => b.offset <= a.offset,
                (None, Some(_)) => true,
                _ => false,
            };
            let mut event = if take_parameter {
                parameter += 1;
                self.parameters[parameter - 1]
            } else {
                note += 1;
                self.notes[note - 1]
            };
            event.offset = event.offset.min(frames.saturating_sub(1));
            if self.merged.len() == self.merged.capacity() {
                break;
            }
            // Arrival order is frame order but for rare re-ordering: keep the list sorted.
            let mut at = self.merged.len();
            while at > 0 && self.merged[at - 1].offset > event.offset {
                at -= 1;
            }
            self.merged.insert(at, event);
        }
        self.notes.clear();
        self.parameters.clear();
    }

    fn deadline_ns(&self, block: &BlockInfo) -> u64 {
        if self.hosted.offline {
            return DEADLINE_NONE;
        }
        if block.cycle_end_ns == 0 {
            return 0; // the instance's own default: the block's duration
        }
        block.cycle_end_ns.saturating_sub(monotonic_ns()).max(self.hosted.min_deadline_ns).max(1)
    }
}

impl InsertProcessor for PluginInsert {
    fn prepare(&mut self, sample_rate: f64, max_frames: usize) {
        self.sample_rate = sample_rate;
        self.latency = self.hosted.latency();
        self.hosted.planned_latency.store(self.latency, std::sync::atomic::Ordering::Relaxed);
        self.parameters = Vec::with_capacity(EVENT_CAPACITY);
        self.notes = Vec::with_capacity(EVENT_CAPACITY);
        self.merged = Vec::with_capacity(EVENT_CAPACITY);
        self.out_events = vec![Event::default(); 64];
        self.scratch_left = vec![0.0; max_frames.max(1)];
        self.scratch_right = vec![0.0; max_frames.max(1)];
        if let Some(instrument) = self.instrument.as_mut() {
            instrument.out_left = vec![0.0; max_frames.max(1)];
            instrument.out_right = vec![0.0; max_frames.max(1)];
            instrument.dry = (self.latency > 0).then(|| DelayLine::new(self.latency as usize));
        }
    }

    fn latency(&self) -> u32 {
        self.latency
    }

    fn tail_seconds(&self) -> f64 {
        self.hosted.tail_seconds(self.sample_rate)
    }

    fn accepts_sidechain(&self) -> bool {
        self.hosted.sidechain
    }

    fn timed_parameters(&self) -> bool {
        true
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32], sidechain: Option<(&[f32], &[f32])>, context: &ProcessContext) {
        let frames = left.len().min(right.len()).min(self.scratch_left.len());
        if frames == 0 {
            return;
        }
        if self.bypass {
            // The graph delays a bypassed insert that has latency itself.
            self.parameters.clear();
            self.notes.clear();
            return;
        }
        if self.instrument.is_some() {
            while let Some(bytes) = self.hosted.pop_midi() {
                if self.notes.len() < self.notes.capacity() {
                    self.notes.push(Event::midi(0, &bytes[..(bytes[3] as usize).min(3)]));
                }
            }
        }
        let silence = std::mem::take(&mut self.silence_notes);
        self.merge_events(frames as u32, silence);
        let transport = Self::transport(&context.block, context.tempo);
        let deadline_ns = self.deadline_ns(&context.block);
        let outcome = match self.instrument.as_mut() {
            None => {
                let (side_left, side_right) = sidechain.map_or((std::ptr::null(), std::ptr::null()), |(side_left, side_right)| (side_left.as_ptr(), side_right.as_ptr()));
                let inputs = [left.as_ptr(), right.as_ptr(), side_left, side_right];
                let outputs = [left.as_mut_ptr(), right.as_mut_ptr()];
                let block = RawBlock {
                    frames: frames as u32,
                    inputs: inputs.as_ptr(),
                    num_inputs: 4,
                    outputs: outputs.as_ptr(),
                    num_outputs: 2,
                    events: self.merged.as_ptr(),
                    num_events: self.merged.len() as u32,
                    out_events: self.out_events.as_mut_ptr(),
                    out_event_capacity: self.out_events.len() as u32,
                    transport: &transport,
                    deadline_ns,
                };
                // SAFETY: every pointer is a live slice of at least `frames` samples (the
                // sidechain has the block's length, or is null); the outputs are the main
                // inputs, which the client allows; the events are `merged`'s elements.
                unsafe { self.hosted.instance.process_raw(&block) }
            }
            Some(instrument) => {
                // The strip's own signal waits for the plug-in's latency.
                if let Some(dry) = instrument.dry.as_mut() {
                    self.scratch_left[..frames].copy_from_slice(&left[..frames]);
                    self.scratch_right[..frames].copy_from_slice(&right[..frames]);
                    dry.process(&self.scratch_left[..frames], &self.scratch_right[..frames], &mut left[..frames], &mut right[..frames]);
                }
                for event in &self.merged {
                    let bytes = event.midi_bytes();
                    if event.kind == stpc_client::EVENT_MIDI && bytes.len() == 3 {
                        let (word, bit) = ((bytes[1] / 64) as usize & 1, 1u64 << (bytes[1] % 64));
                        match bytes[0] & 0xF0 {
                            0x90 if bytes[2] > 0 => instrument.held[word] |= bit,
                            0x80 | 0x90 => instrument.held[word] &= !bit,
                            _ => {}
                        }
                    }
                }
                let outputs = [instrument.out_left.as_mut_ptr(), instrument.out_right.as_mut_ptr()];
                let block = RawBlock {
                    frames: frames as u32,
                    inputs: std::ptr::null(),
                    num_inputs: 0,
                    outputs: outputs.as_ptr(),
                    num_outputs: 2,
                    events: self.merged.as_ptr(),
                    num_events: self.merged.len() as u32,
                    out_events: self.out_events.as_mut_ptr(),
                    out_event_capacity: self.out_events.len() as u32,
                    transport: &transport,
                    deadline_ns,
                };
                // SAFETY: the outputs are two distinct buffers of at least `frames` samples;
                // there are no inputs; the events are `merged`'s elements.
                let outcome = unsafe { self.hosted.instance.process_raw(&block) };
                if outcome.flags & PROCESS_OK != 0 {
                    let pan = instrument.pan.clamp(-1.0, 1.0);
                    let gain_left = instrument.volume * if pan > 0.0 { 1.0 - pan } else { 1.0 };
                    let gain_right = instrument.volume * if pan < 0.0 { 1.0 + pan } else { 1.0 };
                    for (out, sample) in left[..frames].iter_mut().zip(&instrument.out_left[..frames]) {
                        *out += *sample * gain_left;
                    }
                    for (out, sample) in right[..frames].iter_mut().zip(&instrument.out_right[..frames]) {
                        *out += *sample * gain_right;
                    }
                }
                outcome
            }
        };
        let processed = outcome.flags & PROCESS_OK != 0;
        if !processed {
            self.fallback_blocks += 1;
        }
        // Notes of dropped blocks are gone: the plug-in may hold notes nobody will end. The
        // first block it answers again starts with note silence.
        if self.instrument.is_some() && (outcome.flags & PROCESS_RESYNCED != 0 || (processed && self.dropped)) {
            self.silence_notes = true;
        }
        self.dropped = !processed;
    }

    fn parameter(&mut self, address: u64, normalized: f64) {
        self.parameter_at(address, normalized, 0);
    }

    fn parameter_at(&mut self, address: u64, normalized: f64, offset: u32) {
        // A bypassed insert is not processed; the engine sends its lanes again when the
        // bypass ends (a new plan is adopted).
        if self.bypass {
            return;
        }
        let Some(value) = self.hosted.table().plain(address, normalized) else { return };
        if self.parameters.len() < self.parameters.capacity() {
            self.parameters.push(Event::param(offset, address, value));
        }
        self.hosted.automated.insert(address);
    }

    fn base_parameter(&mut self, address: u64, normalized: f64) {
        self.base_parameter_at(address, normalized, 0);
    }

    fn base_parameter_at(&mut self, address: u64, normalized: f64, offset: u32) {
        match (address, self.instrument.as_mut()) {
            (INSTRUMENT_VOLUME, Some(instrument)) => instrument.volume = (normalized as f32).clamp(0.0, 2.0),
            (INSTRUMENT_PAN, Some(instrument)) => instrument.pan = (normalized as f32).clamp(-1.0, 1.0),
            (INSTRUMENT_VOLUME | INSTRUMENT_PAN, None) => {}
            _ => {
                // The plug-in holds its own values; only what automation moved goes back, to
                // the base the instance knows now (the plan's copy may be older: the user can
                // have turned the control since). A bypassed insert is not processed: what
                // automation moved stays remembered until it is.
                let table = self.hosted.table();
                if !self.bypass && self.hosted.automated.remove(address) {
                    if let Some(value) = table.plain(address, table.base(address).unwrap_or(normalized)) {
                        if self.parameters.len() < self.parameters.capacity() {
                            self.parameters.push(Event::param(offset, address, value));
                        }
                    }
                }
            }
        }
    }

    fn midi(&mut self, offset: u32, bytes: [u8; 3], sequenced: bool) {
        if self.instrument.is_none() || self.bypass || self.notes.len() >= self.notes.capacity() {
            return;
        }
        let mut event = Event::midi(offset, &bytes);
        if sequenced {
            event.flags |= EVENT_FLAG_SEQUENCED;
        }
        self.notes.push(event);
    }

    fn set_bypass(&mut self, bypass: bool) {
        if self.bypass != bypass {
            self.silence_notes = true;
        }
        self.bypass = bypass;
    }

    fn reset(&mut self) {
        self.notes.clear();
        self.parameters.clear();
        self.silence_notes = true;
        if let Some(dry) = self.instrument.as_mut().and_then(|instrument| instrument.dry.as_mut()) {
            dry.clear();
        }
    }
}

/// The insert factory of an engine that hosts plug-ins: plug-in inserts from the manager,
/// everything else from the factory it wraps (built-in effects).
pub struct PluginFactory {
    manager: Arc<PluginManager>,
    inner: Arc<dyn InsertFactory>,
}

impl PluginFactory {
    pub fn new(manager: Arc<PluginManager>, inner: Arc<dyn InsertFactory>) -> PluginFactory {
        PluginFactory { manager, inner }
    }
}

impl InsertFactory for PluginFactory {
    fn create(&self, insert: &InsertSpec, sample_rate: f64) -> Result<Box<dyn InsertProcessor>, String> {
        match self.manager.lookup(&insert.key) {
            Lookup::Hosted(hosted) => Ok(Box::new(PluginInsert::new(hosted, insert.instrument.map(|mix| (mix.volume, mix.pan))))),
            Lookup::Unavailable(reason) => Err(reason),
            Lookup::Unknown => self.inner.create(insert, sample_rate),
        }
    }

    fn base_parameters(&self, insert: &InsertSpec) -> Result<Vec<(u64, f64)>, String> {
        match self.manager.lookup(&insert.key) {
            Lookup::Hosted(hosted) => {
                let mut base = hosted.base_values();
                if let Some(mix) = insert.instrument {
                    base.push((INSTRUMENT_VOLUME, mix.volume as f64));
                    base.push((INSTRUMENT_PAN, mix.pan as f64));
                }
                Ok(base)
            }
            Lookup::Unavailable(reason) => Err(reason),
            Lookup::Unknown => self.inner.base_parameters(insert),
        }
    }
}

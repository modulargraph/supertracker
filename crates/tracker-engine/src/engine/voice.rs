//! Voices (ported from the C++ engine's `TrackerEngine.cpp`): note start (`noteOn`, zones, slices, voice
//! modes, choke groups), note end (`externalOff`, `kill`, `release`, `allOff`), live parameters,
//! and the per-sample DSP (`voiceSample`): envelopes, two LFOs, sampler modes (one-shot, loops,
//! ping-pong, slices), wavetable and granular playback, the state-variable filter, drive, bit
//! reduction and panning. Also the playhead snapshot for the sample editor.
//!
//! Arithmetic keeps the C++ operand types (`float` vs `double`) and operation order so the PCM is
//! bit-identical; comments mark the places where C++ integer/float promotion decides the type.

use super::abi::*;
use super::project::{sample_ptr, Sample};
use super::state::*;
use super::tables::TrackState;
use super::util::{clamp, cmax, cmin, finite, lowest_bit, midi_frequency, random_bipolar, PI};
use std::sync::atomic::{fence, Ordering};

/// `noteOn`'s arguments.
#[derive(Clone, Copy)]
pub struct NoteOn {
    pub track: i32,
    pub instrument: i32,
    pub note: i32,
    pub velocity: i32,
    /// Frames until release; negative = no gate.
    pub gate: f64,
    pub offset: f32,
    /// -1 instrument direction, 0 forward, 1 backward.
    pub reverse: i32,
    pub glide: f32,
    pub arp: i32,
    pub from_sequencer: bool,
    /// Explicit zero-based slice, or -1.
    pub slice: i32,
    pub preview: bool,
    pub preview_pitch: f32,
    /// Mapped gate end in beats; negative = none.
    pub mapped_end: f64,
    pub sequence_track: i32,
}

impl NoteOn {
    /// A live (keyboard/MIDI input) note: `noteOn(track,instrument,note,velocity,-1,0,-1,0,0)`.
    pub fn live(track: i32, instrument: i32, note: i32, velocity: i32) -> NoteOn {
        NoteOn {
            track,
            instrument,
            note,
            velocity,
            gate: -1.0,
            offset: 0.0,
            reverse: -1,
            glide: 0.0,
            arp: 0,
            from_sequencer: false,
            slice: -1,
            preview: false,
            preview_pitch: 0.0,
            mapped_end: -1.0,
            sequence_track: -1,
        }
    }
}

/// What ending a voice needs besides the voice itself: the active-voice bitset and the event
/// queue. Borrowed separately from the voice array so voices can be ended while one is borrowed.
pub struct VoiceSink<'s> {
    pub masks: &'s mut [u64; VOICE_GROUPS],
    pub out: &'s EventOut,
    pub sh: &'s Shared,
}

impl VoiceSink<'_> {
    /// `externalOff`: an AU/MIDI voice sends its note-off exactly once.
    #[inline]
    pub fn external_off(&mut self, v: &mut Voice) {
        if v.external && !v.off_sent {
            let event = STEvent {
                type_: ST_EVENT_NOTE_OFF,
                track: v.track,
                instrument: v.instrument,
                note: v.note,
                slot: if v.sequenced { 1 } else { 0 },
                ..STEvent::default()
            };
            emit_event(self.out, self.sh, event);
            v.off_sent = true;
        }
    }

    /// `kill`: end the voice now and return its sample reference.
    #[inline]
    pub fn kill(&mut self, v: &mut Voice, index: usize) {
        if !v.active {
            return;
        }
        self.external_off(v);
        if !v.sample.is_null() {
            // SAFETY: an active voice's non-null `sample` holds a reference taken in `note_on`,
            // so `collect()` has not freed it. Only the count changes here; the sample itself is
            // destroyed on the control thread.
            unsafe { &*v.sample }.refs.fetch_sub(1, Ordering::Release);
        }
        v.sample = std::ptr::null();
        v.active = false;
        if let Some(mask) = self.masks.get_mut(index / 64) {
            *mask &= !(1u64 << (index % 64));
        }
    }

    /// `release`: enter the release stage (external voices end at once after their note-off).
    #[inline]
    pub fn release(&mut self, v: &mut Voice, index: usize) {
        if !v.active || v.releasing {
            return;
        }
        self.external_off(v);
        v.releasing = true;
        v.env_stage = 3;
        v.release_start = v.env;
        v.elapsed = 0.0;
        v.filter_env_stage = 3;
        v.filter_release_start = v.filter_env;
        v.filter_elapsed = 0.0;
        if v.external {
            self.kill(v, index);
        }
    }
}

/// The voice's sample, if any.
#[inline(always)]
fn voice_sample_ref<'s>(v: &Voice) -> Option<&'s Sample> {
    if v.sample.is_null() {
        None
    } else {
        // SAFETY: a non-null voice sample pointer is counted (taken in `note_on`, returned in
        // `kill`), so the sample is alive; its PCM is immutable after creation. The reference is
        // only used within the current render call, during which the voice keeps its count or
        // the reference is not used after `kill`.
        Some(unsafe { &*v.sample })
    }
}

#[inline(always)]
fn read_sample(a: &[f32], pos: f64) -> f32 {
    if a.is_empty() {
        return 0.0;
    }
    let last = a.len() - 1;
    let pos = clamp(pos, 0.0, last as f64);
    let i = pos as usize;
    let j = cmin(i + 1, last);
    match (a.get(i), a.get(j)) {
        (Some(&x), Some(&y)) => x + (y - x) * (pos - i as f64) as f32,
        _ => 0.0,
    }
}

/// `readSample` on both channels of a timeline asset, scaled by `gain`.
#[inline]
pub fn read_stereo(sample: &Sample, pos: f64, gain: f32) -> (f32, f32) {
    (read_sample(&sample.l, pos) * gain, read_sample(&sample.r, pos) * gain)
}

/// Region interpolation bounds change only with sample geometry.
fn update_sample_bounds(v: &mut Voice, sample: &Sample) {
    let n = cmin(sample.l.len(), sample.r.len());
    let last = n.saturating_sub(1) as f64;
    v.sample_first = clamp(v.begin.floor(), 0.0, last) as usize;
    v.sample_last = clamp(v.end.ceil() - 1.0, v.sample_first as f64, last) as usize;
}

#[inline(always)]
fn hermite(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

/// Four-point Hermite interpolation reduces interpolation droop, while integer positions retain
/// exact source frames. The taps are clamped to the selected region: excluded transients and
/// neighbouring slices must never leak into playback. Both channels share the tap indices and
/// fraction.
#[inline(always)]
fn read_hermite2(s: &Sample, pos: f64, first: usize, last: usize) -> (f32, f32) {
    let pos = clamp(pos, first as f64, last as f64);
    let i = pos as usize;
    let t = (pos - i as f64) as f32;
    let (left, right) = (&s.l[..], &s.r[..]);
    // `last` was clamped to the sample length in `update_sample_bounds`; the length check keeps
    // every tap in bounds without relying on it.
    let i3 = cmin(i + 2, last);
    if i3 >= left.len() || i3 >= right.len() || i > i3 {
        return (0.0, 0.0);
    }
    if t == 0.0 {
        return (left[i], right[i]);
    }
    let i0 = if i > first { i - 1 } else { cmin(first, i) };
    let i2 = cmin(i + 1, last);
    (
        hermite(left[i0], left[i], left[i2], left[i3], t),
        hermite(right[i0], right[i], right[i2], right[i3], t),
    )
}

/// The four cyclic taps of one position, shared by both channels and both crossfaded frames.
struct CycleTaps {
    k0: usize,
    k1: usize,
    k2: usize,
    k3: usize,
    t: f32,
}

/// Reads a single-cycle frame cyclically (phase 0..1), so the end of the cycle interpolates into
/// its start.
#[inline(always)]
fn cycle_taps(width: usize, phase: f64) -> CycleTaps {
    let x = phase * width as f64;
    let mut i = x as usize;
    if i >= width {
        i %= width;
    }
    let t = (x - x.floor()) as f32;
    // (i+k)%width without dividing: i<width, so one wrap suffices once width>=3.
    if width >= 3 {
        CycleTaps {
            k0: if i + width - 1 >= width { i - 1 } else { i + width - 1 },
            k1: i,
            k2: if i + 1 >= width { i + 1 - width } else { i + 1 },
            k3: if i + 2 >= width { i + 2 - width } else { i + 2 },
            t,
        }
    } else {
        CycleTaps { k0: (i + width - 1) % width, k1: i % width, k2: (i + 1) % width, k3: (i + 2) % width, t }
    }
}

#[inline(always)]
fn read_cycle_taps(a: &[f32], base: usize, c: &CycleTaps) -> f32 {
    let Some(last) = a.len().checked_sub(1) else { return 0.0 };
    hermite(
        a[cmin(last, base + c.k0)],
        a[cmin(last, base + c.k1)],
        a[cmin(last, base + c.k2)],
        a[cmin(last, base + c.k3)],
        c.t,
    )
}

/// fmod(fmod(x,range)+range,range) for range>=1, without dividing when x is already in
/// [0,range): fmod(x,range) is then x, and the sum lies in [range,2*range], where fmod subtracts
/// range exactly (or yields 0 at 2*range). Identical results, bit for bit.
#[inline(always)]
fn wrap_range(x: f64, range: f64) -> f64 {
    if x >= 0.0 && x < range {
        let a = x + range;
        return if a < 2.0 * range { a - range } else { 0.0 };
    }
    (x % range + range) % range
}

/// Granular parameters shared by automation and live parameter edits (other targets: false).
#[inline]
fn set_grain_target(p: &mut STInstrument, target: i32, x: f32) -> bool {
    match target {
        ST_TARGET_GRAIN_DENSITY => p.grain_density = clamp(x, 1.0, ST_GRANULAR_MAX_GRAINS as f32).round() as i32,
        ST_TARGET_GRAIN_SPRAY => p.grain_spray = clamp(x, 0.0, 1.0),
        ST_TARGET_GRAIN_PITCH_SPREAD => p.grain_pitch_spread = clamp(x, 0.0, 24.0),
        ST_TARGET_GRAIN_SCAN => p.grain_scan = clamp(x, -4.0, 4.0),
        ST_TARGET_GRAIN_REVERSE => p.grain_reverse = clamp(x, 0.0, 1.0),
        ST_TARGET_GRAIN_STEREO => p.grain_stereo = clamp(x, 0.0, 1.0),
        _ => return false,
    }
    true
}

/// Loop-seam crossfade. Approaching the wrap point, blend toward the audio that sits next to the
/// opposite loop point (outside the loop when the region has it), so wrapping never jumps however
/// the loop points are placed. Prefers audio before the loop start (forward) or after the loop
/// end (backward); otherwise it fades the first moments after each wrap from the audio beyond the
/// wrap point. Loops spanning the whole region keep a plain wrap. Both channels share the
/// positions and fade weights.
#[inline(always)]
fn loop_read2(s: &Sample, v: &Voice, x: f64, fade: f64) -> (f32, f32) {
    let read = |position: f64| read_hermite2(s, position, v.sample_first, v.sample_last);
    let (mut l, mut r) = read(x);
    let (lo, hi) = (v.loop_begin, v.loop_end);
    let width = hi - lo;
    if fade < 1.0 || x < lo || x >= hi {
        return (l, r);
    }
    let pre = cmin(fade, lo - v.begin);
    let post = cmin(fade, v.end - hi);
    if v.dir > 0 {
        if pre >= 1.0 && x >= hi - pre {
            let t = ((x - (hi - pre)) / pre) as f32;
            let (ol, or) = read(x - width);
            l = l + (ol - l) * t;
            r = r + (or - r) * t;
            return (l, r);
        }
        if pre < 1.0 && post >= 1.0 && v.looped && x < lo + post {
            let t = ((x - lo) / post) as f32;
            let (ol, or) = read(x + width);
            l = ol + (l - ol) * t;
            r = or + (r - or) * t;
            return (l, r);
        }
    } else {
        if post >= 1.0 && x < lo + post {
            let t = ((lo + post - x) / post) as f32;
            let (ol, or) = read(x + width);
            l = l + (ol - l) * t;
            r = r + (or - r) * t;
            return (l, r);
        }
        if post < 1.0 && pre >= 1.0 && v.looped && x >= hi - pre {
            let t = ((hi - x) / pre) as f32;
            let (ol, or) = read(x - width);
            l = ol + (l - ol) * t;
            r = or + (r - or) * t;
            return (l, r);
        }
    }
    (l, r)
}

/// `instrumentAutomation`: apply the channel's automated instrument targets to a voice.
/// `geometry` also moves the sample region of a sounding voice.
fn instrument_automation(v: &mut Voice, ts: &TrackState, geometry: bool) {
    if ts.instrument_mask == 0 {
        return;
    }
    let values = &ts.instrument;
    let mut remaining = ts.instrument_mask;
    while remaining != 0 {
        let n = lowest_bit(remaining);
        remaining &= remaining - 1;
        let Some(&x) = values.get(n) else { continue };
        if !x.is_finite() {
            continue;
        }
        let p = &mut v.p;
        match n as i32 + ST_TARGET_ATTACK {
            ST_TARGET_ATTACK => p.attack = clamp(x, 0.0, 60.0),
            ST_TARGET_DECAY => p.decay = clamp(x, 0.0, 60.0),
            ST_TARGET_SUSTAIN => p.sustain = clamp(x, 0.0, 1.0),
            ST_TARGET_RELEASE => p.release = clamp(x, 0.0001, 60.0),
            ST_TARGET_DRIVE => p.drive = clamp(x, 0.0, 1.0),
            ST_TARGET_BIT_DEPTH => p.bit_depth = clamp(x, 2.0, 16.0) as i32,
            ST_TARGET_LFO_RATE => p.lfo_rate = clamp(x, 0.0, 100.0),
            ST_TARGET_LFO_DEPTH => p.lfo_depth = clamp(x, 0.0, 1.0),
            ST_TARGET_START => p.start = clamp(x, 0.0, 0.999999),
            ST_TARGET_END => p.end = clamp(x, p.start + 0.000001, 1.0),
            ST_TARGET_LOOP_START => p.loop_start = clamp(x, 0.0, 0.999999),
            ST_TARGET_LOOP_END => p.loop_end = clamp(x, p.loop_start + 0.000001, 1.0),
            ST_TARGET_GRAIN_SIZE => p.grain_size = clamp(x, 0.002, 0.5),
            ST_TARGET_GRAIN_POSITION => p.grain_position = clamp(x, 0.0, 1.0),
            ST_TARGET_SLICE_COUNT => p.slice_count = clamp(x, 1.0, 128.0) as i32,
            ST_TARGET_INSTRUMENT_VOLUME => p.volume = clamp(x, 0.0, 2.0) * v.zone_volume,
            ST_TARGET_INSTRUMENT_PAN => p.pan = clamp(x + v.zone_pan, -1.0, 1.0),
            ST_TARGET_TUNE => p.tune = clamp(x, -96.0, 96.0) + v.zone_tune,
            ST_TARGET_FILTER_ATTACK => p.filter_attack = clamp(x, 0.0, 60.0),
            ST_TARGET_FILTER_DECAY => p.filter_decay = clamp(x, 0.0, 60.0),
            ST_TARGET_FILTER_SUSTAIN => p.filter_sustain = clamp(x, 0.0, 1.0),
            ST_TARGET_FILTER_RELEASE => p.filter_release = clamp(x, 0.0001, 60.0),
            ST_TARGET_FILTER_ENV_AMOUNT => p.filter_env_amount = clamp(x, -48.0, 48.0),
            ST_TARGET_LFO2_RATE => p.lfo2_rate = clamp(x, 0.0, 100.0),
            ST_TARGET_LFO2_DEPTH => p.lfo2_depth = clamp(x, 0.0, 1.0),
            ST_TARGET_LFO_BEATS => p.lfo_beats = clamp(x, 1.0 / 64.0, 64.0),
            ST_TARGET_LFO2_BEATS => p.lfo2_beats = clamp(x, 1.0 / 64.0, 64.0),
            ST_TARGET_WAVETABLE_POSITION => p.wavetable_position = clamp(x, 0.0, 1.0),
            target => {
                set_grain_target(p, target, x);
            }
        }
    }
    if v.zone_region {
        v.p.start = clamp(v.p.start, v.zone_start, v.zone_end - 0.000001);
        v.p.end = clamp(v.p.end, v.p.start + 0.000001, v.zone_end);
    }
    if !geometry || v.p.mode == ST_MODE_SLICE {
        return;
    }
    let Some(sample) = voice_sample_ref(v) else { return };
    let automated = |target: i32| values[(target - ST_TARGET_ATTACK) as usize].is_finite();
    let (old_begin, old_end) = (v.begin, v.end);
    let frames = sample.l.len() as f64;
    if automated(ST_TARGET_START) {
        v.begin = clamp(v.p.start as f64 * frames, 0.0, frames - 1.0);
    }
    // A moved start re-derives the end as at note start, so an un-automated end can never fall
    // below it.
    if automated(ST_TARGET_START) || automated(ST_TARGET_END) {
        v.end = clamp(v.p.end as f64 * frames, v.begin + 1.0, frames);
    }
    if automated(ST_TARGET_LOOP_START) {
        v.loop_begin = clamp(v.p.loop_start as f64 * frames, v.begin, v.end - 1.0);
    }
    if automated(ST_TARGET_LOOP_END) {
        v.loop_end = clamp(v.p.loop_end as f64 * frames, v.loop_begin + 1.0, v.end);
    }
    if v.zone_region {
        v.loop_begin = clamp(v.loop_begin, v.begin, v.end - 1.0);
        v.loop_end = clamp(v.loop_end, v.loop_begin + 1.0, v.end);
    }
    if old_begin != v.begin || old_end != v.end {
        // The loop follows the moved region as at note start; left outside it, the next wrap
        // would end the voice.
        if !v.zone_region {
            v.loop_begin = clamp(v.p.loop_start as f64 * frames, v.begin, v.end - 1.0);
            v.loop_end = clamp(v.p.loop_end as f64 * frames, v.loop_begin + 1.0, v.end);
        }
        v.pos = clamp(v.pos, v.begin, v.end - 1.0);
        update_sample_bounds(v, sample);
    }
}

/// Granular: grainDensity overlapping Hann grains at equal phase offsets, each reading around the
/// (scanning) position. With density 2 and no randomness this is exactly the historical two-grain
/// crossfade.
#[inline]
fn granular_sample(v: &mut Voice, s: &Sample, sample_rate: f64, rate: f64, mod_position: f32, mod_size: f32) -> (f32, f32) {
    let p = v.p;
    let length = clamp(p.grain_size as f64 * mod_size as f64 * sample_rate, 64.0, sample_rate * 0.5);
    let range = cmax(1.0, v.end - v.begin);
    if p.grain_scan != 0.0 {
        v.grain_scan = wrap_range(v.grain_scan + p.grain_scan as f64 * s.rate / sample_rate, range);
    }
    let center =
        v.begin + clamp(p.grain_position as f64 + mod_position as f64, 0.0, 1.0) * (v.end - v.begin - 1.0) + v.grain_scan;
    v.grain_center = center;
    let count = if p.grain_density >= 1 && p.grain_density <= ST_GRANULAR_MAX_GRAINS as i32 {
        p.grain_density
    } else {
        ST_GRANULAR_DEFAULT_DENSITY
    };
    let random = p.grain_spray > 0.0 || p.grain_pitch_spread > 0.0 || p.grain_reverse > 0.0 || p.grain_stereo > 0.0;
    let a = v.grain_phase;
    v.grain_count = count;
    v.grain_span = length * rate * 0.5 + p.grain_spray as f64 * s.rate;
    let (mut l, mut r);
    if count == 2 && !random {
        let c = a + 0.5;
        // fmod(c,1), exact for c in [1,2)
        let b = if c >= 0.0 && c < 1.0 {
            c
        } else if c >= 1.0 && c < 2.0 {
            c - 1.0
        } else {
            c % 1.0
        };
        let mut pa = center + (a - 0.5) * length * rate;
        let mut pb = center + (b - 0.5) * length * rate;
        pa = v.begin + wrap_range(pa - v.begin, range);
        pb = v.begin + wrap_range(pb - v.begin, range);
        let wa = (0.5 - 0.5 * (2.0 * PI * a).cos()) as f32;
        let wb = 1.0 - wa;
        let (al, ar) = read_hermite2(s, pa, v.sample_first, v.sample_last);
        let (bl, br) = read_hermite2(s, pb, v.sample_first, v.sample_last);
        l = al * wa + bl * wb;
        r = ar * wa + br * wb;
        let g0 = &mut v.grains[0];
        g0.phase = a;
        g0.read = pa;
        g0.window = wa;
        g0.dir = 1;
        let g1 = &mut v.grains[1];
        g1.phase = b;
        g1.read = pb;
        g1.window = wb;
        g1.dir = 1;
    } else {
        l = 0.0;
        r = 0.0;
        for k in 0..count as usize {
            let mut g = v.grains[k];
            let mut phase = a + k as f64 / count as f64;
            phase -= phase.floor();
            if phase < g.phase {
                g.offset = 0.0;
                g.ratio = 1.0;
                g.dir = 1;
                g.gain_l = 1.0;
                g.gain_r = 1.0;
                if random {
                    if p.grain_spray > 0.0 {
                        g.offset = random_bipolar(&mut v.rng) as f64 * p.grain_spray as f64 * s.rate;
                    }
                    if p.grain_pitch_spread > 0.0 {
                        g.ratio = (random_bipolar(&mut v.rng) * p.grain_pitch_spread / 12.0).exp2();
                    }
                    if p.grain_reverse > 0.0 && (random_bipolar(&mut v.rng) + 1.0) * 0.5 < p.grain_reverse {
                        g.dir = -1;
                    }
                    if p.grain_stereo > 0.0 {
                        let pan = random_bipolar(&mut v.rng) * p.grain_stereo;
                        g.gain_l = if pan > 0.0 { 1.0 - pan } else { 1.0 };
                        g.gain_r = if pan < 0.0 { 1.0 + pan } else { 1.0 };
                    }
                }
            }
            g.phase = phase;
            let w = (0.5 - 0.5 * (2.0 * PI * phase).cos()) as f32;
            let mut at = center + g.offset + g.dir as f64 * (phase - 0.5) * length * rate * g.ratio as f64;
            at = v.begin + wrap_range(at - v.begin, range);
            let (gl, gr) = read_hermite2(s, at, v.sample_first, v.sample_last);
            l += gl * w * g.gain_l;
            r += gr * w * g.gain_r;
            g.read = at;
            g.window = w;
            v.grains[k] = g;
        }
        // Between amplitude (coherent) and power (decorrelated grains) normalization; 1 at
        // density 1 and 2.
        let gain = if count >= 2 { (2.0f32 / count as f32).powf(0.75) } else { 1.0 };
        l *= gain;
        r *= gain;
    }
    v.grain_phase += 1.0 / length;
    v.grain_phase -= v.grain_phase.floor();
    (l, r)
}

/// One live instrument parameter on one voice (see `set_instrument_parameter`). Region
/// targets only shape notes that start later (`onset`).
fn apply_live(v: &mut Voice, target: i32, x: f32, onset: bool) {
    let (zone_volume, zone_pan, zone_tune) = (v.zone_volume, v.zone_pan, v.zone_tune);
    let p = &mut v.p;
    match target {
        ST_TARGET_CUTOFF => p.cutoff = clamp(x, 20.0, 22000.0),
        ST_TARGET_RESONANCE => p.resonance = clamp(x, 0.0, 0.99),
        ST_TARGET_INSTRUMENT_VOLUME => p.volume = clamp(x, 0.0, 2.0) * zone_volume,
        ST_TARGET_INSTRUMENT_PAN => p.pan = clamp(clamp(x, -1.0, 1.0) + zone_pan, -1.0, 1.0),
        ST_TARGET_TUNE => p.tune = clamp(x, -96.0, 96.0) + zone_tune,
        ST_TARGET_ATTACK => p.attack = clamp(x, 0.0, 60.0),
        ST_TARGET_DECAY => p.decay = clamp(x, 0.0, 60.0),
        ST_TARGET_SUSTAIN => p.sustain = clamp(x, 0.0, 1.0),
        ST_TARGET_RELEASE => p.release = clamp(x, 0.0001, 60.0),
        ST_TARGET_DRIVE => p.drive = clamp(x, 0.0, 1.0),
        ST_TARGET_BIT_DEPTH => p.bit_depth = clamp(x, 2.0, 16.0) as i32,
        ST_TARGET_LFO_RATE => p.lfo_rate = clamp(x, 0.0, 100.0),
        ST_TARGET_LFO_DEPTH => p.lfo_depth = clamp(x, 0.0, 1.0),
        ST_TARGET_LFO2_RATE => p.lfo2_rate = clamp(x, 0.0, 100.0),
        ST_TARGET_LFO2_DEPTH => p.lfo2_depth = clamp(x, 0.0, 1.0),
        ST_TARGET_LFO_BEATS => p.lfo_beats = clamp(x, 1.0 / 64.0, 64.0),
        ST_TARGET_LFO2_BEATS => p.lfo2_beats = clamp(x, 1.0 / 64.0, 64.0),
        ST_TARGET_FILTER_ATTACK => p.filter_attack = clamp(x, 0.0, 60.0),
        ST_TARGET_FILTER_DECAY => p.filter_decay = clamp(x, 0.0, 60.0),
        ST_TARGET_FILTER_SUSTAIN => p.filter_sustain = clamp(x, 0.0, 1.0),
        ST_TARGET_FILTER_RELEASE => p.filter_release = clamp(x, 0.0001, 60.0),
        ST_TARGET_FILTER_ENV_AMOUNT => p.filter_env_amount = clamp(x, -48.0, 48.0),
        ST_TARGET_GRAIN_SIZE => p.grain_size = clamp(x, 0.002, 0.5),
        ST_TARGET_GRAIN_POSITION => p.grain_position = clamp(x, 0.0, 1.0),
        ST_TARGET_WAVETABLE_POSITION => p.wavetable_position = clamp(x, 0.0, 1.0),
        ST_TARGET_START => {
            if onset {
                p.start = clamp(x, 0.0, 0.999999)
            }
        }
        ST_TARGET_END => {
            if onset {
                p.end = clamp(x, p.start + 0.000001, 1.0)
            }
        }
        ST_TARGET_LOOP_START => {
            if onset {
                p.loop_start = clamp(x, 0.0, 0.999999)
            }
        }
        ST_TARGET_LOOP_END => {
            if onset {
                p.loop_end = clamp(x, p.loop_start + 0.000001, 1.0)
            }
        }
        ST_TARGET_SLICE_COUNT => {
            if onset {
                p.slice_count = clamp(x, 1.0, 128.0) as i32
            }
        }
        _ => {
            set_grain_target(p, target, x);
        }
    }
}

/// Amplitude ADSR, advanced one sample. Ends the voice when its release completes.
#[inline(always)]
fn envelope(v: &mut Voice, sample_rate: f64, sink: &mut VoiceSink, index: usize) -> f32 {
    if v.releasing {
        let duration = cmax(0.0001, v.p.release as f64) * sample_rate;
        v.env = v.release_start * cmax(0.0, 1.0 - v.elapsed / duration) as f32;
        if v.elapsed >= duration {
            sink.kill(v, index);
            return 0.0;
        }
    } else if v.env_stage == 0 {
        let duration = cmax(0.0, v.p.attack as f64) * sample_rate;
        v.env = if duration < 1.0 { 1.0 } else { (v.elapsed / duration) as f32 };
        if v.env >= 1.0 {
            v.env = 1.0;
            v.env_stage = 1;
            v.elapsed = 0.0;
        }
    } else if v.env_stage == 1 {
        let duration = cmax(0.0001, v.p.decay as f64) * sample_rate;
        v.env = 1.0 + (v.p.sustain - 1.0) * cmin(1.0, v.elapsed / duration) as f32;
        if v.elapsed >= duration {
            v.env = v.p.sustain;
            v.env_stage = 2;
        }
    } else {
        v.env = v.p.sustain;
    }
    v.elapsed += 1.0;
    v.env
}

/// Filter ADSR, advanced one sample (independent of the amplitude envelope).
#[inline(always)]
fn filter_envelope(v: &mut Voice, sample_rate: f64) -> f32 {
    if v.releasing {
        let length = cmax(0.0001, v.p.filter_release as f64) * sample_rate;
        v.filter_env = v.filter_release_start * cmax(0.0, 1.0 - v.filter_elapsed / length) as f32;
    } else if v.filter_env_stage == 0 {
        let length = cmax(0.0, v.p.filter_attack as f64) * sample_rate;
        v.filter_env = if length < 1.0 { 1.0 } else { cmin(1.0, v.filter_elapsed / length) as f32 };
        if v.filter_env >= 1.0 {
            v.filter_env_stage = 1;
            v.filter_elapsed = 0.0;
        }
    } else if v.filter_env_stage == 1 {
        let length = cmax(0.0001, v.p.filter_decay as f64) * sample_rate;
        v.filter_env = 1.0 + (v.p.filter_sustain - 1.0) * cmin(1.0, v.filter_elapsed / length) as f32;
        if v.filter_elapsed >= length {
            v.filter_env = v.p.filter_sustain;
            v.filter_env_stage = 2;
        }
    } else {
        v.filter_env = v.p.filter_sustain;
    }
    v.filter_elapsed += 1.0;
    v.filter_env
}

/// One LFO sample; the phase always advances, the waveform is only evaluated when audible.
#[inline(always)]
fn lfo_sample(phase: &mut f64, rate: f64, shape: i32, held: &mut f32, random: &mut u32, audible: bool, sample_rate: f64) -> f32 {
    let unit = *phase / (2.0 * PI);
    let mut value = 0.0f32;
    if audible {
        value = match shape {
            ST_LFO_TRIANGLE => 1.0 - 4.0 * (unit as f32 - 0.5).abs(),
            ST_LFO_SAW => (2.0 * unit - 1.0) as f32,
            ST_LFO_SQUARE => {
                if unit < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            ST_LFO_RANDOM => *held,
            _ => (*phase as f32).sin(),
        };
    }
    *phase += 2.0 * PI * rate / sample_rate;
    if *phase >= 2.0 * PI {
        *phase -= 2.0 * PI * (*phase / (2.0 * PI)).floor();
        if shape == ST_LFO_RANDOM {
            *held = random_bipolar(random);
        }
    }
    value
}

struct Modulation {
    cutoff_octaves: f32,
    pitch: f32,
    volume: f32,
    pan: f32,
    position: f32,
    grain_size: f32,
}

#[inline(always)]
fn route_modulation(m: &mut Modulation, value: f32, depth: f32, target: i32) {
    if depth <= 0.0 {
        return;
    }
    let amount = value * depth;
    match target {
        ST_MOD_PITCH => m.pitch += amount * 12.0,
        ST_MOD_VOLUME => m.volume *= 1.0 - depth * (value + 1.0) * 0.5,
        ST_MOD_PAN => m.pan += amount,
        ST_MOD_POSITION => m.position += amount * 0.5,
        ST_MOD_GRAIN_SIZE => m.grain_size *= (amount * 2.0).exp2(),
        _ => m.cutoff_octaves += amount * 4.0,
    }
}

#[inline(always)]
fn modulation(v: &mut Voice, sample_rate: f64, effective_bpm: f64) -> Modulation {
    let mut m = Modulation { cutoff_octaves: 0.0, pitch: 0.0, volume: 1.0, pan: 0.0, position: 0.0, grain_size: 1.0 };
    m.cutoff_octaves = filter_envelope(v, sample_rate) * v.p.filter_env_amount / 12.0;
    let p = v.p;
    let rate1 = if p.lfo_sync != 0 { effective_bpm / (60.0 * p.lfo_beats as f64) } else { p.lfo_rate as f64 };
    let rate2 = if p.lfo2_sync != 0 { effective_bpm / (60.0 * p.lfo2_beats as f64) } else { p.lfo2_rate as f64 };
    let source1 = if p.lfo_depth > 0.0 || rate1 > 0.0 {
        lfo_sample(&mut v.lfo_phase, rate1, p.lfo_shape, &mut v.lfo_random, &mut v.lfo_rng, p.lfo_depth > 0.0, sample_rate)
    } else {
        0.0
    };
    let source2 = if p.lfo2_depth > 0.0 || rate2 > 0.0 {
        lfo_sample(&mut v.lfo2_phase, rate2, p.lfo2_shape, &mut v.lfo2_random, &mut v.lfo2_rng, p.lfo2_depth > 0.0, sample_rate)
    } else {
        0.0
    };
    route_modulation(&mut m, source1, p.lfo_depth, p.lfo_target);
    route_modulation(&mut m, source2, p.lfo2_depth, p.lfo2_target);
    m
}

/// Topology-preserving state-variable filter coefficients, recomputed only when an input changes.
#[inline(always)]
fn filter_coefficients(v: &mut Voice, cutoff: f32, resonance: f32, sample_rate: f64) {
    let cutoff = clamp(cutoff, 20.0, (sample_rate * 0.45) as f32);
    let resonance = clamp(resonance, 0.0, 0.99);
    if cutoff == v.coefficient_cutoff && resonance == v.coefficient_resonance {
        return;
    }
    v.coefficient_cutoff = cutoff;
    v.coefficient_resonance = resonance;
    let g = (PI as f32 * cutoff / sample_rate as f32).tan();
    v.filter_k = 2.0 - 1.95 * resonance;
    v.filter_a1 = 1.0 / (1.0 + g * (g + v.filter_k));
    v.filter_a2 = g * v.filter_a1;
    v.filter_a3 = g * v.filter_a2;
}

#[inline(always)]
fn filter(input: f32, ic1: &mut f32, ic2: &mut f32, k: f32, a1: f32, a2: f32, a3: f32, kind: i32) -> f32 {
    let v3 = input - *ic2;
    let v1 = a1 * *ic1 + a2 * v3;
    let v2 = *ic2 + a2 * *ic1 + a3 * v3;
    *ic1 = 2.0 * v1 - *ic1;
    *ic2 = 2.0 * v2 - *ic2;
    if ic1.abs() < 1e-20 {
        *ic1 = 0.0;
    }
    if ic2.abs() < 1e-20 {
        *ic2 = 0.0;
    }
    if kind == ST_FILTER_HIGHPASS {
        input - k * v1 - v2
    } else if kind == ST_FILTER_BANDPASS {
        v1
    } else {
        v2
    }
}

impl<'a> Rt<'a> {
    #[inline]
    pub fn kill(&mut self, index: usize) {
        let e = &mut *self.e;
        if let Some(v) = e.voices.get_mut(index) {
            VoiceSink { masks: &mut e.active_voice_masks, out: &e.out, sh: self.sh }.kill(v, index);
        }
    }

    #[inline]
    pub fn release(&mut self, index: usize) {
        let e = &mut *self.e;
        if let Some(v) = e.voices.get_mut(index) {
            VoiceSink { masks: &mut e.active_voice_masks, out: &e.out, sh: self.sh }.release(v, index);
        }
    }

    /// `allOff`: end every voice (`playback_only`: sequenced voices only), at once or through
    /// its release.
    pub fn all_off(&mut self, immediate: bool, playback_only: bool) {
        let e = &mut *self.e;
        let mut sink = VoiceSink { masks: &mut e.active_voice_masks, out: &e.out, sh: self.sh };
        for (index, v) in e.voices.iter_mut().enumerate() {
            if v.active && (!playback_only || v.sequenced) {
                if immediate {
                    sink.kill(v, index);
                } else {
                    sink.release(v, index);
                }
            }
        }
    }

    #[inline]
    pub fn computational_track_enabled(&self, track: i32) -> bool {
        self.p.track_computational(track)
    }

    /// Whether a new AU/MIDI note may sound on `track`.
    pub fn external_track_enabled(&self, track: i32, kind: i32) -> bool {
        let p = self.p;
        if p.host_mixing && kind == ST_INSTRUMENT_AU {
            return self.computational_track_enabled(track);
        }
        match p.tracks.get(track as u32 as usize) {
            Some(mix) => mix.mute == 0 && (!p.any_solo || mix.solo != 0),
            None => false,
        }
    }

    pub fn note_on(&mut self, n: NoteOn) {
        let p = self.p;
        let NoteOn { track, instrument, note, velocity, gate, offset, reverse, glide, arp, .. } = n;
        let NoteOn { from_sequencer, slice, preview, preview_pitch, mapped_end, mut sequence_track, .. } = n;
        if !self.channel_enabled(track) || note < 0 || note > 127 {
            return;
        }
        let Some(instrument_data) = (if instrument >= 0 { p.instruments.get(instrument as usize) } else { None }) else {
            return;
        };
        if from_sequencer && sequence_track < 0 {
            sequence_track = track;
        }
        // A zero-velocity hit is a note-off for this lane, matching explicit OFF/CUT ownership.
        if velocity <= 0 {
            for index in 0..MAX_VOICES {
                let v = &self.e.voices[index];
                if v.active
                    && v.sequenced == from_sequencer
                    && v.preview == preview
                    && v.track == track
                    && v.note == note
                    && (!from_sequencer || v.sequence_track == sequence_track)
                {
                    self.release(index);
                }
            }
            return;
        }
        // A track's own choke group and voice mode win over its instrument's. Track choke groups
        // are scoped to the track's group (scope); instrument choke groups choke any track.
        let track_voice = p.track_voices.get(track as usize).copied().unwrap_or_default();
        let voice_mode = if track_voice.mode >= 0 { track_voice.mode } else { instrument_data.p.voice_mode };
        let choke_group = if track_voice.choke > 0 { track_voice.choke } else { instrument_data.p.choke_group };
        let choke_scope = if track_voice.choke > 0 { track_voice.scope } else { -1 };
        let kind = instrument_data.p.type_;
        if kind == ST_INSTRUMENT_AU || kind == ST_INSTRUMENT_MIDI {
            // MIDI 1.0 has no source ownership on a shared channel/pitch. Suppress colliding
            // previews; a real note first ends the pitch it retriggers (preview or not), so the
            // earlier voice's later note-off cannot cut the new note short in the plug-in.
            for index in 0..MAX_VOICES {
                let v = &self.e.voices[index];
                if v.active && v.external && v.track == track && v.instrument == instrument && v.note == note {
                    if preview && !v.preview {
                        return;
                    }
                    if !preview {
                        self.kill(index);
                    }
                }
            }
        }
        let total_frames = self.e.total_frames;
        let velocity_level = clamp(velocity, 0, 127) as f32 / 127.0;
        let note_on_event = STEvent {
            type_: ST_EVENT_NOTE_ON,
            track,
            instrument,
            note,
            velocity,
            slot: if from_sequencer { 1 } else { 0 },
            ..STEvent::default()
        };
        if !preview && slice < 0 && voice_mode == 2 {
            for index in 0..MAX_VOICES {
                let v = &self.e.voices[index];
                if !(v.active && !v.releasing && !v.preview && !v.explicit_slice && v.track == track && v.instrument == instrument)
                {
                    continue;
                }
                // MIDI legato: a plug-in or hardware synth slurs only when the new note starts
                // before the old one ends, so its note-off follows the new note-on (off first
                // would make legato the same as mono). The extra note-on is admitted only while
                // the queue still holds a note-off for every voice.
                let slur = v.external
                    && !v.off_sent
                    && v.note != note
                    && self.external_track_enabled(track, kind)
                    && self.sh.events.available() > MAX_VOICES as u32 + 1;
                let off = STEvent {
                    type_: ST_EVENT_NOTE_OFF,
                    track: v.track,
                    instrument: v.instrument,
                    note: v.note,
                    slot: if v.sequenced { 1 } else { 0 },
                    ..STEvent::default()
                };
                let enabled = self.external_track_enabled(track, kind);
                let e = &mut *self.e;
                let v = &mut e.voices[index];
                let mut sink = VoiceSink { masks: &mut e.active_voice_masks, out: &e.out, sh: self.sh };
                if !slur {
                    sink.external_off(v);
                }
                v.sequenced = from_sequencer;
                v.sequence_track = sequence_track;
                v.note = note;
                v.velocity = velocity_level;
                v.gate = gate;
                v.mapped_gate_end = mapped_end;
                v.onset_frame = total_frames;
                v.off_sent = false;
                if v.external {
                    if !enabled {
                        v.external = false;
                        sink.kill(v, index);
                        return;
                    }
                    let started = emit_event(sink.out, sink.sh, note_on_event);
                    if slur {
                        emit_event(sink.out, sink.sh, off);
                    }
                    if !started {
                        v.off_sent = true;
                        sink.kill(v, index);
                    }
                }
                return;
            }
        }
        // Tracker note lifetime: a new sequenced note releases the lane's gateless notes
        // (note-off with their release envelope). Notes of the same onset (a chord) leave each
        // other sounding.
        if from_sequencer && !preview {
            for index in 0..MAX_VOICES {
                let v = &self.e.voices[index];
                if v.active
                    && !v.releasing
                    && v.sequenced
                    && v.sequence_track == sequence_track
                    && v.gate < 0.0
                    && v.mapped_gate_end < 0.0
                    && v.onset_frame < total_frames
                {
                    self.release(index);
                }
            }
        }
        // Mono cuts the track's earlier notes: a chord (different pitches starting on the same
        // frame) stays whole, and a sequenced note cuts only its own note column (another
        // column's note keeps sounding).
        if !preview {
            for index in 0..MAX_VOICES {
                let v = &self.e.voices[index];
                if !v.active {
                    continue;
                }
                let mono_cuts = voice_mode > 0
                    && v.track == track
                    && v.instrument == instrument
                    && (v.onset_frame < total_frames || v.note == note)
                    && (!from_sequencer || !v.sequenced || v.sequence_track == sequence_track);
                if mono_cuts || (choke_group > 0 && v.choke_scope == choke_scope && v.choke_group == choke_group) {
                    self.kill(index);
                }
            }
        }
        let mut slot = self.e.voices.iter().position(|v| !v.active);
        if slot.is_none() {
            if preview {
                self.sh.dropped_events.fetch_add(1, Ordering::Relaxed);
                return;
            }
            // Steal the oldest voice.
            let mut oldest = 0;
            for index in 0..MAX_VOICES {
                if self.e.voices[index].age < self.e.voices[oldest].age {
                    oldest = index;
                }
            }
            self.kill(oldest);
            slot = Some(oldest);
        }
        let Some(index) = slot else { return };
        let enabled = self.external_track_enabled(track, kind);
        let Some(ts) = self.tb.track_state.get(track as usize) else { return };
        let e = &mut *self.e;
        e.serial = e.serial.wrapping_add(1);
        e.rng = e.rng.wrapping_add(0x9e3779b9);
        let v = &mut e.voices[index];
        *v = Voice::NEW;
        v.active = true;
        e.active_voice_masks[index / 64] |= 1u64 << (index % 64);
        let mut sink = VoiceSink { masks: &mut e.active_voice_masks, out: &e.out, sh: self.sh };
        v.sequenced = from_sequencer;
        v.preview = preview;
        v.explicit_slice = slice >= 0;
        v.track = track;
        v.sequence_track = sequence_track;
        v.choke_group = choke_group;
        v.choke_scope = choke_scope;
        v.instrument = instrument;
        v.note = note;
        v.age = e.serial;
        v.p = instrument_data.p;
        v.velocity = velocity_level;
        v.gate = gate;
        v.mapped_gate_end = mapped_end;
        v.onset_frame = total_frames;
        v.glide = glide as f64;
        v.arp = arp;
        v.rng = e.rng | 1;
        v.lfo_rng = (v.rng ^ 0x7f4a7c15) | 1;
        v.lfo2_rng = (v.rng ^ 0x517cc1b7) | 1;
        v.lfo_random = random_bipolar(&mut v.lfo_rng);
        v.lfo2_random = random_bipolar(&mut v.lfo2_rng);
        v.sample = sample_ptr(&instrument_data.sample);
        v.external = v.p.type_ == ST_INSTRUMENT_AU || v.p.type_ == ST_INSTRUMENT_MIDI;
        // applyLiveParams
        for entry in e.live_params.iter().take(e.live_param_count) {
            if entry.instrument == v.instrument {
                apply_live(v, entry.target, entry.value, true);
            }
        }
        // Parameters only: the region is derived below, once the zone is known.
        instrument_automation(v, ts, false);
        if !instrument_data.zones.is_empty() {
            v.sample = std::ptr::null();
            for zone in &instrument_data.zones {
                if note >= zone.key_low && note <= zone.key_high && velocity >= zone.velocity_low && velocity <= zone.velocity_high {
                    v.sample = sample_ptr(&zone.sample);
                    v.zone_root = zone.root_note;
                    v.zone_tune = zone.tune;
                    v.zone_volume = zone.volume;
                    v.p.root_note = zone.root_note;
                    v.p.tune += zone.tune;
                    v.p.volume *= zone.volume;
                    v.zone_pan = zone.pan;
                    v.p.pan = clamp(v.p.pan + v.zone_pan, -1.0, 1.0);
                    if zone.loop_enabled >= 0 {
                        v.p.mode = if zone.loop_enabled != 0 { ST_MODE_FORWARD } else { ST_MODE_ONESHOT };
                    }
                    if zone.has_region {
                        v.zone_region = true;
                        v.zone_start = zone.start;
                        v.zone_end = zone.end;
                        v.p.start = zone.start;
                        v.p.end = zone.end;
                        v.p.loop_start = zone.loop_start;
                        v.p.loop_end = zone.loop_end;
                    }
                    break;
                }
            }
        }
        v.cycle_width = (if v.p.wavetable_frame_size > 0 {
            v.p.wavetable_frame_size
        } else if instrument_data.cycle_frames > 0 {
            instrument_data.cycle_frames
        } else {
            ST_WAVETABLE_DEFAULT_FRAME
        }) as usize;
        v.position_smooth = clamp(v.p.wavetable_position as f64, 0.0, 1.0);
        v.zone_tune += preview_pitch;
        v.p.tune += preview_pitch;
        if slice >= 0 && v.p.type_ == ST_INSTRUMENT_SAMPLE {
            v.p.mode = ST_MODE_SLICE;
        }
        v.volume_smooth = v.p.volume;
        v.pan_smooth = v.p.pan;
        v.cutoff_smooth = v.p.cutoff;
        if !v.sample.is_null() {
            // SAFETY: the pointer comes from the current project's instrument or zone, whose
            // counted reference keeps the sample alive; the voice takes its own count here and
            // returns it in `kill`.
            unsafe { &*v.sample }.refs.fetch_add(1, Ordering::Relaxed);
        }
        if v.external {
            if !enabled {
                v.external = false;
                sink.kill(v, index);
                return;
            }
            if !emit_event(sink.out, sink.sh, note_on_event) {
                v.off_sent = true;
                sink.kill(v, index);
            }
            return;
        }
        let Some(sample) = voice_sample_ref(v) else {
            sink.kill(v, index);
            return;
        };
        if sample.l.is_empty() || sample.r.len() != sample.l.len() {
            // Never constructed (samples hold 1..INT32_MAX frames on both channels).
            sink.kill(v, index);
            return;
        }
        let frames = sample.l.len() as f64;
        v.begin = clamp(v.p.start as f64 * frames, 0.0, frames - 1.0);
        v.end = clamp(v.p.end as f64 * frames, v.begin + 1.0, frames);
        v.loop_begin = clamp(v.p.loop_start as f64 * frames, v.begin, v.end - 1.0);
        v.loop_end = clamp(v.p.loop_end as f64 * frames, v.loop_begin + 1.0, v.end);
        if v.p.mode == ST_MODE_SLICE {
            let count = clamp(v.p.slice_count, 1, 128);
            let slices = &instrument_data.slices;
            if slices.is_empty() {
                let selected =
                    if slice >= 0 { clamp(slice, 0, count - 1) } else { (note.wrapping_sub(v.p.root_note) % count + count) % count };
                let width = (v.end - v.begin) / count as f64;
                v.begin += selected as f64 * width;
                v.end = v.begin + width;
            } else {
                let size = slices.len() as i32;
                let selected =
                    if slice >= 0 { clamp(slice, 0, size - 1) } else { (note.wrapping_sub(v.p.root_note) % size + size) % size };
                let lo = v.begin;
                let range = v.end - v.begin;
                let first = slices.get(selected as usize).copied().unwrap_or(0.0);
                let next = if selected + 1 < size { slices.get(selected as usize + 1).copied().unwrap_or(1.0) } else { 1.0 };
                v.begin = lo + first as f64 * range;
                v.end = lo + next as f64 * range;
            }
            v.loop_begin = v.begin;
            v.loop_end = v.end;
        }
        v.dir = if v.p.mode == ST_MODE_REVERSE { -1 } else { 1 };
        if reverse >= 0 {
            v.dir = if reverse != 0 { -1 } else { 1 };
        }
        v.pos = if v.dir > 0 {
            v.begin + offset as f64 * (v.end - v.begin - 1.0)
        } else {
            v.end - 1.0 - offset as f64 * (v.end - v.begin - 1.0)
        };
        v.declick_in = v.dir < 0 || v.pos >= 1.0;
        update_sample_bounds(v, sample);
    }

    /// Immediate audition snapshot; replaces only prior preview voices.
    pub fn preview_step(&mut self, command: &Command) {
        for index in 0..MAX_VOICES {
            let v = &self.e.voices[index];
            if v.active && v.preview {
                self.kill(index);
            }
        }
        let step = &command.step;
        if step.note < 0 || step.velocity <= 0 || !self.channel_enabled(command.a) {
            return;
        }
        let instrument = if step.instrument >= 0 {
            step.instrument
        } else {
            self.p.tracks.get(command.a as usize).map_or(0, |track| track.instrument)
        };
        let (mut slice, mut reverse) = (-1, -1);
        let (mut offset, mut pitch) = (0.0f32, 0.0f32);
        for fx in [step.fx1, step.fx2] {
            match fx.type_ {
                ST_FX_SLICE => slice = clamp(finite(fx.value, 1.0), 1.0, 128.0) as i32 - 1,
                ST_FX_REVERSE => reverse = (fx.value >= 0.5) as i32,
                ST_FX_OFFSET => offset = clamp(finite(fx.value, 0.0), 0.0, 0.9999),
                ST_FX_PITCH => pitch = clamp(finite(fx.value, 0.0), -48.0, 48.0),
                _ => {}
            }
        }
        let count = clamp(command.chord_count, 0, ST_MAX_CHORD as i32) as usize;
        for n in 0..count {
            self.note_on(NoteOn {
                track: command.a,
                instrument,
                note: step.note.wrapping_add(command.chord[n]),
                velocity: step.velocity,
                gate: self.e.sample_rate * command.value as f64,
                offset,
                reverse,
                glide: 0.0,
                arp: 0,
                from_sequencer: false,
                slice,
                preview: true,
                preview_pitch: pitch,
                mapped_end: -1.0,
                sequence_track: -1,
            });
        }
    }

    /// Live instrument parameter: remembered for notes that start later and applied to the
    /// instrument's sounding native voices.
    pub fn set_live_param(&mut self, instrument: i32, target: i32, value: f32) {
        if !self.p.has_instrument(instrument) || !value.is_finite() {
            return;
        }
        let e = &mut *self.e;
        let capacity = e.live_params.len();
        let mut slot = None;
        for n in 0..cmin(e.live_param_count, capacity) {
            if e.live_params[n].instrument == instrument && e.live_params[n].target == target {
                slot = Some(n);
                break;
            }
        }
        let slot = match slot {
            Some(slot) => slot,
            None => {
                if e.live_param_count >= capacity {
                    e.live_params.copy_within(1.., 0);
                    e.live_param_count = capacity - 1;
                }
                e.live_param_count += 1;
                e.live_param_count - 1
            }
        };
        e.live_params[slot] = LiveParam { instrument, target, value };
        for v in e.voices.iter_mut() {
            if v.active && !v.external && v.instrument == instrument {
                apply_live(v, target, value, false);
            }
        }
    }

    /// Voice playhead snapshot for the sample editor; only while watched, a no-op otherwise.
    pub fn publish_voice_positions(&mut self) {
        let watch = self.sh.voice_watch.load(Ordering::Relaxed);
        if watch < -1 {
            if !self.e.voice_snapshot_empty {
                self.e.voice_snapshot_empty = true;
                self.write_voice_snapshot(0);
            }
            return;
        }
        let mut count = 0usize;
        for group in 0..VOICE_GROUPS {
            if count >= ST_VOICE_POSITION_CAPACITY {
                break;
            }
            let mut active = self.e.active_voice_masks[group];
            while active != 0 && count < ST_VOICE_POSITION_CAPACITY {
                let bit = lowest_bit(active);
                active &= active - 1;
                let Some(v) = self.e.voices.get(group * 64 + bit) else { continue };
                if !v.active || v.external || v.sample.is_null() || (watch >= 0 && v.instrument != watch) {
                    continue;
                }
                let position = self.voice_position(v);
                self.e.voice_stage[count] = position;
                count += 1;
            }
        }
        if count == 0 && self.e.voice_snapshot_empty {
            return;
        }
        self.e.voice_snapshot_empty = count == 0;
        self.write_voice_snapshot(count);
    }

    fn voice_position(&self, v: &Voice) -> STVoicePosition {
        let mut o = STVoicePosition::default();
        let p = &v.p;
        o.instrument = v.instrument;
        o.track = v.track;
        o.note = v.note;
        o.mode = p.mode;
        o.level = v.env;
        let zone = match self.p.instruments.get(v.instrument as u32 as usize) {
            Some(instrument) => sample_ptr(&instrument.sample) != v.sample,
            None => false,
        };
        o.flags = (if v.releasing { ST_VOICE_POSITION_RELEASING } else { 0 })
            | (if v.preview { ST_VOICE_POSITION_PREVIEW } else { 0 })
            | (if v.sequenced { ST_VOICE_POSITION_SEQUENCED } else { 0 })
            | (if zone { ST_VOICE_POSITION_ZONE } else { 0 });
        o.sample_frames = voice_sample_ref(v).map_or(0.0, |sample| sample.l.len() as f64);
        o.direction = v.dir;
        o.position = v.pos;
        o.window_start = v.pos;
        o.window_end = v.pos;
        if p.mode == ST_MODE_WAVETABLE {
            let region = v.end - v.begin;
            let width = cmax(1.0, cmin(v.cycle_width as f64, region.floor()));
            let frames = cmax(1.0, (region / width).floor());
            let frame = cmin(frames - 1.0, (v.position_smooth * (frames - 1.0) + 0.5).floor());
            o.position = v.begin + frame * width;
            o.window_start = o.position;
            o.window_end = o.window_start + width;
            o.direction = 1;
        } else if p.mode == ST_MODE_GRANULAR {
            let range = cmax(1.0, v.end - v.begin);
            o.position = v.begin + wrap_range(v.grain_center - v.begin, range);
            o.window_start = o.position - v.grain_span;
            o.window_end = o.position + v.grain_span;
            o.direction = if p.grain_scan > 0.0 {
                1
            } else if p.grain_scan < 0.0 {
                -1
            } else {
                0
            };
            o.grain_count = v.grain_count;
            let grains = clamp(v.grain_count, 0, ST_GRANULAR_MAX_GRAINS as i32) as usize;
            for k in 0..grains {
                o.grain_positions[k] = v.grains[k].read;
                o.grain_levels[k] = v.grains[k].window;
            }
        }
        o
    }

    /// Seqlock writer: the sequence is odd while the snapshot is being replaced.
    fn write_voice_snapshot(&mut self, count: usize) {
        let sh = self.sh;
        let seq = sh.voice_seq.load(Ordering::Relaxed);
        sh.voice_seq.store(seq.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        let count = cmin(count, ST_VOICE_POSITION_CAPACITY);
        // SAFETY: the render thread is the only writer of the snapshot. Readers copy it without
        // forming references and discard the copy unless `voice_seq` was even and unchanged
        // around it (seqlock, as in the C++ engine), so a concurrent read is never used.
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.e.voice_stage.as_ptr(),
                sh.voice_snapshot.get() as *mut STVoicePosition,
                count,
            );
        }
        sh.voice_snapshot_count.store(count as i32, Ordering::Relaxed);
        sh.voice_serial.fetch_add(1, Ordering::Relaxed);
        sh.voice_seq.store(seq.wrapping_add(2), Ordering::Release);
    }

    /// `voiceSample`: one stereo sample of one voice. Returns silence for external voices (which
    /// only track their gate here).
    #[inline]
    pub fn voice_sample(&mut self, index: usize) -> (f32, f32) {
        let p = self.p;
        let e = &mut *self.e;
        let sample_rate = e.sample_rate;
        let Some(v) = e.voices.get_mut(index) else { return (0.0, 0.0) };
        let mut sink = VoiceSink { masks: &mut e.active_voice_masks, out: &e.out, sh: self.sh };
        let (mut l, mut r) = (0.0f32, 0.0f32);
        if v.mapped_gate_end >= 0.0 && e.musical_beat + e.loop_beat_offset >= v.mapped_gate_end {
            sink.release(v, index);
        }
        if v.gate >= 0.0 {
            if v.gate <= 0.0 {
                sink.release(v, index);
            } else {
                v.gate = cmax(0.0, v.gate - 1.0);
            }
        }
        if !v.active || v.external {
            return (l, r);
        }
        let track = v.track as u32 as usize;
        let Some(t) = self.tb.track_state.get(track) else { return (l, r) };
        instrument_automation(v, t, true);
        let env = envelope(v, sample_rate, &mut sink, index);
        if !v.active {
            return (l, r);
        }
        let m = modulation(v, sample_rate, e.effective_bpm);
        let bend = self.tb.pitch_bend.get(track).copied().unwrap_or(0.0);
        // The pitch sum is single precision (`int + float + ...`), widened afterwards.
        let mut pitch = (v.note as f32 + v.p.tune + t.pitch + bend + m.pitch) as f64;
        if v.glide != 0.0 {
            pitch += v.glide * (1.0 - (-v.lifetime / (sample_rate * 0.1)).exp());
        }
        if v.arp != 0 {
            let stage = if p.timing.is_some() {
                ((e.musical_beat + e.loop_beat_offset) * 12.0).floor() as i32 % 3
            } else {
                (e.total_frames / (sample_rate * 60.0 / p.bpm / 12.0)) as i32 % 3
            };
            pitch += if stage == 1 {
                ((v.arp >> 4) & 15) as f64
            } else if stage == 2 {
                (v.arp & 15) as f64
            } else {
                0.0
            };
        }
        v.lifetime += 1.0;
        if v.p.mode == ST_MODE_WAVETABLE && pitch != v.frequency_pitch {
            v.frequency_pitch = pitch;
            v.frequency = clamp(midi_frequency(pitch), 1.0, (sample_rate * 0.45) as f32);
        }
        let frequency = v.frequency;
        if let Some(s) = voice_sample_ref(v) {
            let rate_pitch = if v.p.mode == ST_MODE_SLICE && !v.explicit_slice {
                (v.p.tune + t.pitch + bend + m.pitch) as f64
            } else {
                pitch - v.p.root_note as f64
            };
            if rate_pitch != v.rate_pitch {
                v.rate_pitch = rate_pitch;
                v.sample_increment = s.rate / sample_rate * (rate_pitch / 12.0).exp2();
            }
            let rate = v.sample_increment;
            if v.p.mode == ST_MODE_WAVETABLE {
                // The region holds consecutive single cycles of cycleWidth samples (the whole
                // region when shorter). Position picks a frame and crossfades to its neighbour;
                // it slews at most the full range per 5 ms, so automation and step FX jumps glide
                // between frames instead of clicking.
                let region = v.end - v.begin;
                let width = cmax(1.0, cmin(v.cycle_width as f64, region.floor())) as usize;
                let width = cmax(width, 1);
                let frames = cmax(1, (region / width as f64) as usize);
                let target = clamp(v.p.wavetable_position as f64 + m.position as f64, 0.0, 1.0);
                let slew = 1.0 / (0.005 * sample_rate);
                v.position_smooth = if target > v.position_smooth {
                    cmin(target, v.position_smooth + slew)
                } else {
                    cmax(target, v.position_smooth - slew)
                };
                let location = v.position_smooth * (frames - 1) as f64;
                let f0 = cmin(frames - 1, location as usize);
                let f1 = cmin(frames - 1, f0 + 1);
                let mix = (location - f0 as f64) as f32;
                let base = v.begin as usize;
                let taps = cycle_taps(width, v.phase);
                l = read_cycle_taps(&s.l, base + f0 * width, &taps);
                r = read_cycle_taps(&s.r, base + f0 * width, &taps);
                if mix > 0.0 && f1 != f0 {
                    l += (read_cycle_taps(&s.l, base + f1 * width, &taps) - l) * mix;
                    r += (read_cycle_taps(&s.r, base + f1 * width, &taps) - r) * mix;
                }
                v.phase += frequency as f64 / sample_rate;
                v.phase -= v.phase.floor();
            } else if v.p.mode == ST_MODE_GRANULAR {
                (l, r) = granular_sample(v, s, sample_rate, rate, m.position, m.grain_size);
            } else {
                if v.pos < v.begin || v.pos >= v.end {
                    sink.kill(v, index);
                    return (0.0, 0.0);
                }
                let mode = v.p.mode;
                // Forward and reverse are Polyend-style loops (reverse starts at the end and
                // loops backwards); ping-pong bounces between the loop points; one-shot and
                // slices play once.
                let wrap_loop = mode == ST_MODE_FORWARD || mode == ST_MODE_REVERSE;
                let looping = wrap_loop || mode == ST_MODE_PINGPONG;
                // The lead-in (before the loop in forward mode, after it in reverse mode) only
                // wraps from inside the loop: a voice turned around there runs out to the region
                // edge, with the one-shot end fade.
                let leaving = wrap_loop
                    && (if mode == ST_MODE_REVERSE {
                        v.dir > 0 && v.pos >= v.loop_end
                    } else {
                        v.dir < 0 && v.pos < v.loop_begin
                    });
                let mut read_position = v.pos + m.position as f64 * (v.end - v.begin);
                if looping {
                    let range = cmax(1.0, v.end - v.begin);
                    read_position = v.begin + wrap_range(read_position - v.begin, range);
                } else {
                    read_position = clamp(read_position, v.begin, v.end - 1.0);
                }
                if wrap_loop {
                    let fade = cmin(s.rate * 0.005, (v.loop_end - v.loop_begin) * 0.5);
                    (l, r) = loop_read2(s, v, read_position, fade);
                } else {
                    (l, r) = read_hermite2(s, read_position, v.sample_first, v.sample_last);
                }
                // Declick: a ~1 ms fade-in when starting inside the audio and a ~2 ms fade-out
                // before a one-shot or slice ends.
                let step = cmax(rate, 1e-9);
                let region_frames = (v.end - v.begin) / step;
                let mut declick = 1.0f32;
                if v.declick_in {
                    let fade_in = cmin(sample_rate * 0.001, region_frames * 0.25);
                    if v.lifetime < fade_in {
                        declick = (v.lifetime / fade_in) as f32;
                    } else {
                        v.declick_in = false;
                    }
                }
                if !looping || leaving {
                    let remaining = (if v.dir > 0 { v.end - v.pos } else { v.pos - v.begin + 1.0 }) / step;
                    let fade_out = cmin(sample_rate * 0.002, region_frames * 0.25);
                    if remaining < fade_out {
                        declick *= cmax(0.0, remaining / fade_out) as f32;
                    }
                }
                l *= declick;
                r *= declick;
                if v.sample_transition {
                    v.sample_transition = false;
                    if v.sample_history {
                        v.sample_correction_l = v.sample_last_l - l;
                        v.sample_correction_r = v.sample_last_r - r;
                        v.sample_transition_frames = cmax(1, (sample_rate * 0.002).round() as i32);
                        v.sample_transition_left = v.sample_transition_frames;
                    }
                }
                if v.sample_transition_left > 0 {
                    let phase = v.sample_transition_left as f32 / v.sample_transition_frames as f32;
                    // Smoothstep has zero slope at both ends; subsequent reversals start from
                    // the corrected signal rather than stale source PCM.
                    let weight = phase * phase * (3.0 - 2.0 * phase);
                    l += v.sample_correction_l * weight;
                    r += v.sample_correction_r * weight;
                    v.sample_transition_left -= 1;
                }
                v.sample_last_l = l;
                v.sample_last_r = r;
                v.sample_history = true;
                v.pos += rate * v.dir as f64;
                if wrap_loop && !leaving {
                    let width = cmax(1.0, v.loop_end - v.loop_begin);
                    if v.dir > 0 && v.pos >= v.loop_end {
                        v.pos = v.loop_begin + (v.pos - v.loop_begin) % width;
                        v.looped = true;
                    } else if v.dir < 0 && v.pos < v.loop_begin {
                        v.pos = v.loop_begin + ((v.pos - v.loop_begin) % width + width) % width;
                        v.looped = true;
                    }
                } else if mode == ST_MODE_PINGPONG {
                    let lo = v.loop_begin;
                    let hi = v.loop_end - 1.0;
                    let width = hi - lo;
                    if width <= 0.0 {
                        v.pos = lo;
                    } else if (v.dir > 0 && v.pos > hi) || (v.dir < 0 && v.pos < lo) {
                        let phase = ((v.pos - lo) % (width * 2.0) + width * 2.0) % (width * 2.0);
                        if phase <= width {
                            v.pos = lo + phase;
                        } else {
                            v.pos = hi - (phase - width);
                            v.dir = -v.dir;
                        }
                    }
                }
            }
        }
        let mut cutoff = if t.cutoff >= 0.0 { t.cutoff } else { v.p.cutoff };
        if m.cutoff_octaves != 0.0 {
            cutoff *= m.cutoff_octaves.exp2();
        }
        let wheel = self.tb.mod_wheel.get(track).copied().unwrap_or(0.0);
        if wheel > 0.0 {
            cutoff *= (-5.25f32 * wheel).exp2();
        }
        v.cutoff_smooth += (cutoff - v.cutoff_smooth) * 0.01;
        cutoff = v.cutoff_smooth;
        let resonance = if t.resonance >= 0.0 { t.resonance } else { v.p.resonance };
        filter_coefficients(v, cutoff, resonance, sample_rate);
        let (k, a1, a2, a3, kind) = (v.filter_k, v.filter_a1, v.filter_a2, v.filter_a3, v.p.filter_type);
        l = filter(l, &mut v.filter_l1, &mut v.filter_l2, k, a1, a2, a3, kind);
        r = filter(r, &mut v.filter_r1, &mut v.filter_r2, k, a1, a2, a3, kind);
        if v.p.drive > 0.00001 {
            if v.p.drive != v.drive_amount {
                v.drive_amount = v.p.drive;
                v.drive_gain = 1.0 + v.p.drive * 24.0;
                v.drive_scale = 1.0 / v.drive_gain.tanh();
            }
            l = (l * v.drive_gain).tanh() * v.drive_scale;
            r = (r * v.drive_gain).tanh() * v.drive_scale;
        }
        if v.p.bit_depth < 16 {
            let levels = (1u32 << (clamp(v.p.bit_depth, 2, 16) - 1)) as f32;
            l = (l * levels).round() / levels;
            r = (r * levels).round() / levels;
        }
        v.volume_smooth += (v.p.volume - v.volume_smooth) * 0.005;
        v.pan_smooth += (v.p.pan - v.pan_smooth) * 0.005;
        let pan = clamp(v.pan_smooth + m.pan, -1.0, 1.0);
        let gain = env * v.velocity * v.volume_smooth * m.volume;
        if pan != v.pan_law_pan {
            v.pan_law_pan = pan;
            v.pan_law_l = (1.0 - pan).sqrt();
            v.pan_law_r = (1.0 + pan).sqrt();
        }
        l *= gain * v.pan_law_l;
        r *= gain * v.pan_law_r;
        if !l.is_finite() || !r.is_finite() {
            l = 0.0;
            r = 0.0;
            sink.kill(v, index);
        }
        (l, r)
    }
}

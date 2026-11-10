//! Filtering processors: four-band EQ, multimode filter and cabinet.
//!
//! Ported from the C++ `EffectsFiltering.hpp` (namespace `stfx::filtering`).

use super::common::{
    clamp, clean, core_accessors, db_gain, fast_exp, fast_sin_2pi, frame_buffers, max, mix, pole,
    select, wrap, Biquad, BiquadType, Core, DelayLine, Descriptor, Label, ParameterCache,
    ParameterInfo as P, Processor, Smoother, EFFECT_SIDECHAIN, PARAMETER_INTEGER as INTEGER,
    PARAMETER_LOG as LOG, PARAMETER_TOGGLE as TOGGLE, PI,
};

static EQ_PARAMS: [P; 11] = [
    P::new(
        c"lowfreq",
        c"Low frequency",
        c"Hz",
        20.0,
        8000.0,
        100.0,
        LOG,
    ),
    P::new(c"lowgain", c"Low gain", c"dB", -24.0, 24.0, 0.0, 0),
    P::new(
        c"lowmidfreq",
        c"Low-mid frequency",
        c"Hz",
        40.0,
        16000.0,
        500.0,
        LOG,
    ),
    P::new(c"lowmidgain", c"Low-mid gain", c"dB", -24.0, 24.0, 0.0, 0),
    P::new(
        c"highmidfreq",
        c"High-mid frequency",
        c"Hz",
        40.0,
        16000.0,
        2500.0,
        LOG,
    ),
    P::new(c"highmidgain", c"High-mid gain", c"dB", -24.0, 24.0, 0.0, 0),
    P::new(
        c"highfreq",
        c"High frequency",
        c"Hz",
        50.0,
        20000.0,
        8000.0,
        LOG,
    ),
    P::new(c"highgain", c"High gain", c"dB", -24.0, 24.0, 0.0, 0),
    P::new(c"lowmidwidth", c"Low-mid width", c"oct", 0.1, 6.0, 2.0, LOG),
    P::new(
        c"highmidwidth",
        c"High-mid width",
        c"oct",
        0.1,
        6.0,
        2.0,
        LOG,
    ),
    P::new(c"gain", c"Output gain", c"dB", -24.0, 24.0, 0.0, 0),
];

/// Q of a peaking band from its width in octaves.
#[inline]
fn width_q(octaves: f64) -> f64 {
    1.0 / (2.0 * (2.0f64.ln() * octaves / 2.0).sinh())
}

pub struct Eq {
    core: Core,
    bands: [[Biquad; 4]; 2],
    smoothed: [Smoother; 11],
    control: u32,
    gain: f64,
    // Filters are redesigned only when the smoothed controls actually move.
    designed: [f64; 11],
    have_design: bool,
}

impl Eq {
    pub fn new(sr: f64) -> Self {
        Eq {
            core: Core::new(sr),
            bands: [[Biquad::default(); 4]; 2],
            smoothed: [Smoother::default(); 11],
            control: 0,
            gain: 1.0,
            designed: [0.0; 11],
            have_design: false,
        }
    }
}

impl Processor for Eq {
    core_accessors!(core);

    fn tail(&self) -> f64 {
        8.0
    }

    fn reset(&mut self) {
        for channel in self.bands.iter_mut() {
            for b in channel.iter_mut() {
                b.reset();
            }
        }
        for v in self.smoothed.iter_mut() {
            v.reset();
        }
        self.control = 0;
        self.gain = 1.0;
        self.have_design = false;
    }

    fn process(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        _sc_l: Option<&[f32]>,
        _sc_r: Option<&[f32]>,
    ) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let p = self.core.p;
        let sr = self.core.sr;
        let smooth = (-32.0 / (0.01 * sr)).exp();
        for i in 0..l.len() {
            if self.control == 0 {
                let mut v = [0.0f64; 11];
                for j in 0..11 {
                    v[j] = self.smoothed[j].tick(p[j], smooth);
                }
                if !self.have_design || v != self.designed {
                    self.designed = v;
                    self.have_design = true;
                    for channel in self.bands.iter_mut() {
                        channel[0].set(BiquadType::LowShelf, v[0], 0.707106781, v[1], sr);
                        channel[1].set(BiquadType::Peak, v[2], width_q(v[8]), v[3], sr);
                        channel[2].set(BiquadType::Peak, v[4], width_q(v[9]), v[5], sr);
                        channel[3].set(BiquadType::HighShelf, v[6], 0.707106781, v[7], sr);
                    }
                    self.gain = db_gain(v[10]);
                }
            }
            self.control = (self.control + 1) % 32;
            let mut yl = l[i] as f64;
            let mut yr = r[i] as f64;
            for j in 0..4 {
                yl = self.bands[0][j].tick(yl);
                yr = self.bands[1][j].tick(yr);
            }
            l[i] = (yl * self.gain) as f32;
            r[i] = (yr * self.gain) as f32;
        }
    }
}

const FILTER_MODES: &[Label] = &[
    Label::new(c"Low-pass"),
    Label::new(c"Band-pass"),
    Label::new(c"High-pass"),
    Label::new(c"Notch"),
];
const SOURCES: &[Label] = &[
    Label::new(c"LFO"),
    Label::new(c"LFO Sync"),
    Label::new(c"Envelope"),
];
const SHAPES: &[Label] = &[
    Label::new(c"Sine"),
    Label::new(c"Triangle"),
    Label::new(c"Saw"),
    Label::new(c"Square"),
];
static FILTER_PARAMS: [P; 17] = [
    P::choice(c"mode", c"Mode", c"", 0.0, 3.0, 0.0, INTEGER, FILTER_MODES),
    P::new(c"cutoff", c"Cutoff", c"Hz", 20.0, 20000.0, 16000.0, LOG),
    P::new(c"resonance", c"Resonance", c"%", 0.0, 100.0, 0.0, 0),
    P::new(c"amount", c"Modulation", c"oct", -5.0, 5.0, 0.0, 0),
    P::choice(
        c"source",
        c"Modulation source",
        c"",
        0.0,
        2.0,
        0.0,
        INTEGER,
        SOURCES,
    ),
    P::new(c"speed", c"LFO speed", c"Hz", 0.03, 16.0, 1.0, LOG),
    P::new(c"beats", c"LFO period", c"beats", 0.125, 64.0, 4.0, LOG),
    P::choice(c"shape", c"LFO shape", c"", 0.0, 3.0, 0.0, INTEGER, SHAPES),
    P::new(c"phase", c"LFO phase", c"deg", 0.0, 360.0, 0.0, 0),
    P::new(c"decay", c"Envelope decay", c"ms", 5.0, 2000.0, 200.0, LOG),
    P::new(c"smooth", c"Envelope smooth", c"ms", 0.1, 100.0, 5.0, LOG),
    P::new(c"envshape", c"Envelope shape", c"", 0.25, 4.0, 1.0, LOG),
    P::new(c"mix", c"Mix", c"%", 0.0, 100.0, 100.0, 0),
    P::new(c"scgain", c"Detector gain", c"dB", -24.0, 24.0, 0.0, 0),
    P::new(c"scfilter", c"Detector filter", c"", 0.0, 1.0, 0.0, TOGGLE),
    P::new(
        c"sccenter",
        c"Detector frequency",
        c"Hz",
        20.0,
        20000.0,
        632.5,
        LOG,
    ),
    P::new(c"scwidth", c"Detector width", c"oct", 0.25, 6.0, 3.0, LOG),
];

/// Bipolar LFO value for a phase in cycles: sine, triangle, saw or square.
#[inline(always)]
pub fn lfo(phase: f64, shape: i32) -> f64 {
    let phase = wrap(phase);
    match shape {
        1 => 1.0 - 4.0 * (phase - 0.5).abs(),
        2 => 2.0 * phase - 1.0,
        3 => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        _ => fast_sin_2pi(phase),
    }
}

pub struct Filter {
    core: Core,
    ic1: [f64; 2],
    ic2: [f64; 2],
    phase: f64,
    envelope: f64,
    a1: f64,
    a2: f64,
    a3: f64,
    k: f64,
    control: u32,
    sc: [Biquad; 2],
    cutoff: Smoother,
    resonance: Smoother,
    wet: Smoother,
    designed_f: f64,
    designed_q: f64,
    cache: ParameterCache<17>,
    attack: f64,
    release: f64,
    gain: f64,
    smooth: f64,
}

impl Filter {
    pub fn new(sr: f64) -> Self {
        Filter {
            core: Core::new(sr),
            ic1: [0.0; 2],
            ic2: [0.0; 2],
            phase: 0.0,
            envelope: 0.0,
            a1: 1.0,
            a2: 0.0,
            a3: 0.0,
            k: 1.0,
            control: 0,
            sc: [Biquad::default(); 2],
            cutoff: Smoother::default(),
            resonance: Smoother::default(),
            wet: Smoother::default(),
            designed_f: -1.0,
            designed_q: -1.0,
            cache: ParameterCache::default(),
            attack: 0.0,
            release: 0.0,
            gain: 1.0,
            smooth: 0.0,
        }
    }
}

impl Processor for Filter {
    core_accessors!(core);

    fn tail(&self) -> f64 {
        20.0
    }

    fn reset(&mut self) {
        self.ic1 = [0.0; 2];
        self.ic2 = [0.0; 2];
        self.phase = 0.0;
        self.envelope = 0.0;
        self.control = 0;
        for v in self.sc.iter_mut() {
            v.reset();
        }
        self.cutoff.reset();
        self.resonance.reset();
        self.wet.reset();
        self.designed_f = -1.0;
        self.designed_q = -1.0;
        self.cache.reset();
    }

    fn process(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        sc_l: Option<&[f32]>,
        sc_r: Option<&[f32]>,
    ) {
        let (l, r, sc_l, sc_r) = frame_buffers(l, r, sc_l, sc_r);
        let p = self.core.p;
        let sr = self.core.sr;
        if self.cache.changed(&self.core) {
            self.attack = pole(p[10], sr);
            self.release = pole(p[9], sr);
            self.gain = db_gain(p[13]);
            self.smooth = pole(5.0, sr);
            for v in self.sc.iter_mut() {
                v.set(BiquadType::BandPass, p[15], width_q(p[16]), 0.0, sr);
            }
        }
        let hz = if p[4] == 1.0 {
            self.core.tempo / (60.0 * p[6])
        } else {
            p[5]
        };
        let smooth = self.smooth;
        for i in 0..l.len() {
            let dl = l[i] as f64;
            let dr = r[i] as f64;
            let mut sl = match (sc_l, sc_r) {
                (Some(s), _) => s[i] as f64,
                (None, Some(s)) => s[i] as f64,
                (None, None) => dl,
            };
            let mut ss = match (sc_r, sc_l) {
                (Some(s), _) => s[i] as f64,
                (None, Some(s)) => s[i] as f64,
                (None, None) => dr,
            };
            if p[14] >= 0.5 {
                sl = self.sc[0].tick(sl);
                ss = self.sc[1].tick(ss);
            }
            let pk = max(sl.abs(), ss.abs()) * self.gain;
            self.envelope = clean(
                pk + select(pk > self.envelope, self.attack, self.release) * (self.envelope - pk),
            );
            // Zero modulation amount: exp2(0) == 1, the LFO/envelope value is unused.
            let modulation = if p[3] == 0.0 {
                0.0
            } else if p[4] == 2.0 {
                clamp(self.envelope, 0.0, 1.0).powf(p[11])
            } else {
                lfo(self.phase + p[8] / 360.0, p[7] as i32)
            };
            self.phase = wrap(self.phase + hz / sr);
            let frequency = self.cutoff.tick(
                clamp(
                    if p[3] == 0.0 {
                        p[1]
                    } else {
                        p[1] * fast_exp(0.6931471805599453 * (p[3] * modulation))
                    },
                    10.0,
                    sr * 0.45,
                ),
                smooth,
            );
            let q = self
                .resonance
                .tick(0.707106781 + 19.3 * p[2] / 100.0, smooth);
            if self.control == 0 && (frequency != self.designed_f || q != self.designed_q) {
                let g = (PI * frequency / sr).tan();
                self.k = 1.0 / q;
                self.a1 = 1.0 / (1.0 + g * (g + self.k));
                self.a2 = g * self.a1;
                self.a3 = g * self.a2;
                self.designed_f = frequency;
                self.designed_q = q;
            }
            self.control = (self.control + 1) % 8;
            let w = self.wet.tick(p[12] / 100.0, smooth);
            let (a1, a2, a3, k) = (self.a1, self.a2, self.a3, self.k);
            for c in 0..2 {
                let x = if c != 0 { dr } else { dl };
                let v3 = x - self.ic2[c];
                let v1 = a1 * self.ic1[c] + a2 * v3;
                let v2 = self.ic2[c] + a2 * self.ic1[c] + a3 * v3;
                self.ic1[c] = clean(2.0 * v1 - self.ic1[c]);
                self.ic2[c] = clean(2.0 * v2 - self.ic2[c]);
                let out = if p[0] == 1.0 {
                    v1 * k
                } else if p[0] == 2.0 {
                    x - k * v1 - v2
                } else if p[0] == 3.0 {
                    x - k * v1
                } else {
                    v2
                };
                let y = mix(x, out, w) as f32;
                if c != 0 {
                    r[i] = y;
                } else {
                    l[i] = y;
                }
            }
        }
    }
}

const CABINET_TYPES: &[Label] = &[
    Label::new(c"American"),
    Label::new(c"British"),
    Label::new(c"Vintage"),
    Label::new(c"Modern"),
];
const MICROPHONE_TYPES: &[Label] = &[
    Label::new(c"Dynamic 57"),
    Label::new(c"Dynamic 421"),
    Label::new(c"Dynamic 441"),
    Label::new(c"Ribbon 121"),
    Label::new(c"Condenser 47"),
    Label::new(c"Condenser 67"),
];
static CABINET_PARAMS: [P; 4] = [
    P::choice(
        c"cabinet",
        c"Cabinet",
        c"",
        0.0,
        3.0,
        1.0,
        INTEGER,
        CABINET_TYPES,
    ),
    P::choice(
        c"microphone",
        c"Microphone",
        c"",
        0.0,
        5.0,
        0.0,
        INTEGER,
        MICROPHONE_TYPES,
    ),
    P::new(c"distance", c"Distance", c"%", 0.0, 100.0, 0.0, 0),
    P::new(c"mix", c"Mix", c"%", 0.0, 100.0, 100.0, 0),
];

pub struct Cabinet {
    core: Core,
    b: [[Biquad; 5]; 2],
    delay_l: DelayLine,
    delay_r: DelayLine,
    distance: Smoother,
    wet: Smoother,
    // Filters are redesigned only when Cabinet, Microphone or Distance change.
    designed: [f64; 3],
}

impl Cabinet {
    pub fn new(sr: f64) -> Self {
        Cabinet {
            core: Core::new(sr),
            b: [[Biquad::default(); 5]; 2],
            delay_l: DelayLine::new((sr * 0.04) as usize + 4),
            delay_r: DelayLine::new((sr * 0.04) as usize + 4),
            distance: Smoother::default(),
            wet: Smoother::default(),
            designed: [-1.0; 3],
        }
    }
}

impl Processor for Cabinet {
    core_accessors!(core);

    fn reset(&mut self) {
        for channel in self.b.iter_mut() {
            for v in channel.iter_mut() {
                v.reset();
            }
        }
        self.delay_l.reset();
        self.delay_r.reset();
        self.distance.reset();
        self.wet.reset();
        self.designed[0] = -1.0;
    }

    fn tail(&self) -> f64 {
        0.05
    }

    fn process(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        _sc_l: Option<&[f32]>,
        _sc_r: Option<&[f32]>,
    ) {
        let (l, r, _, _) = frame_buffers(l, r, None, None);
        let p = self.core.p;
        let sr = self.core.sr;
        let cab = p[0] as i32;
        let mic = p[1] as i32;
        const HP: [f64; 4] = [65.0, 85.0, 105.0, 55.0];
        const LP: [f64; 4] = [5700.0, 4800.0, 4100.0, 6500.0];
        const MID: [f64; 4] = [1700.0, 2400.0, 1250.0, 3100.0];
        const MIC_GAIN: [f64; 6] = [3.0, 1.5, 0.5, -3.5, 1.0, 2.0];
        const MIC_HZ: [f64; 6] = [4000.0, 3000.0, 5000.0, 2600.0, 6000.0, 4500.0];
        if self.designed[0] != cab as f64
            || self.designed[1] != mic as f64
            || self.designed[2] != p[2]
        {
            self.designed = [cab as f64, mic as f64, p[2]];
            // The host keeps both selectors inside their ranges; out-of-range values (which the
            // C++ would index out of bounds with) select the nearest table entry.
            let ci = cab.clamp(0, 3) as usize;
            let mi = mic.clamp(0, 5) as usize;
            for channel in self.b.iter_mut() {
                channel[0].set(BiquadType::HighPass, HP[ci], 0.707, 0.0, sr);
                channel[1].set(BiquadType::Peak, 120.0, 0.9, 3.0 - p[2] * 0.03, sr);
                channel[2].set(BiquadType::Peak, MID[ci], 1.1, 3.5, sr);
                channel[3].set(BiquadType::HighShelf, MIC_HZ[mi], 0.707, MIC_GAIN[mi], sr);
                channel[4].set(
                    BiquadType::LowPass,
                    LP[ci] * (1.0 - 0.35 * p[2] / 100.0),
                    0.707,
                    0.0,
                    sr,
                );
            }
        }
        let smooth = pole(10.0, sr);
        for i in 0..l.len() {
            let dl = l[i] as f64;
            let dr = r[i] as f64;
            let d = self.distance.tick(p[2] / 100.0, smooth);
            let w = self.wet.tick(p[3] / 100.0, smooth);
            let mut yl = dl;
            let mut yr = dr;
            for j in 0..5 {
                yl = self.b[0][j].tick(yl);
                yr = self.b[1][j].tick(yr);
            }
            let el = self.delay_l.read(sr * (0.001 + d * 0.008));
            let er = self.delay_r.read(sr * (0.0013 + d * 0.009));
            self.delay_l.push(yl);
            self.delay_r.push(yr);
            l[i] = mix(dl, (yl + el * d * 0.25) / (1.0 + d * 0.25), w) as f32;
            r[i] = mix(dr, (yr + er * d * 0.25) / (1.0 + d * 0.25), w) as f32;
        }
    }
}

static DESCRIPTORS: [Descriptor; 3] = [
    Descriptor::new(
        c"eq",
        c"Four-band EQ",
        c"Filtering",
        0,
        &EQ_PARAMS,
        |sr, _| Box::new(Eq::new(sr)),
        |_, _, _| 8.0,
    ),
    Descriptor::new(
        c"filter",
        c"Multimode Filter",
        c"Filtering",
        EFFECT_SIDECHAIN,
        &FILTER_PARAMS,
        |sr, _| Box::new(Filter::new(sr)),
        |_, _, _| 20.0,
    ),
    Descriptor::new(
        c"cabinet",
        c"Cabinet",
        c"Filtering",
        0,
        &CABINET_PARAMS,
        |sr, _| Box::new(Cabinet::new(sr)),
        |_, _, _| 0.05,
    ),
];

/// `stfx::filteringDescriptors`.
pub fn descriptors() -> &'static [Descriptor] {
    &DESCRIPTORS
}

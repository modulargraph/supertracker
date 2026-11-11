//! Dynamics processors: compressor, gate, transient shaper, peak limiter and maximizer.

use super::common::{
    clamp, clean, core_accessors, db_gain, fast_db_gain, frame_buffers, gain_db, max, min, mix,
    pole, select, Biquad, BiquadType, Core, DelayLine, Descriptor, Label, Memo, ParameterCache,
    ParameterInfo as P, Processor, Smoother, EFFECT_SIDECHAIN, PARAMETER_INTEGER as INTEGER,
    PARAMETER_LOG as LOG, PARAMETER_TOGGLE as TOGGLE,
};

const MODES: &[Label] = &[Label::new(c"Classic"), Label::new(c"Feedback")];
static COMPRESSOR_PARAMS: [P; 12] = [
    P::choice(c"mode", c"Mode", c"", 0.0, 1.0, 0.0, INTEGER, MODES),
    P::new(c"threshold", c"Threshold", c"dB", -60.0, 0.0, -18.0, 0),
    P::new(c"ratio", c"Ratio", c":1", 1.0, 20.0, 4.0, LOG),
    P::new(c"knee", c"Knee", c"dB", 0.0, 24.0, 6.0, 0),
    P::new(c"attack", c"Attack", c"ms", 0.1, 200.0, 10.0, LOG),
    P::new(c"release", c"Release", c"ms", 5.0, 2000.0, 100.0, LOG),
    P::new(c"gain", c"Gain", c"dB", -24.0, 24.0, 0.0, 0),
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

/// Resolves the optional sidechain channels to an external key pair: a missing channel
/// duplicates the other one; `None` means internal detection.
#[inline(always)]
fn external_key<'a>(
    left: Option<&'a [f32]>,
    right: Option<&'a [f32]>,
) -> Option<(&'a [f32], &'a [f32])> {
    match (left, right) {
        (Some(l), Some(r)) => Some((l, r)),
        (Some(l), None) => Some((l, l)),
        (None, Some(r)) => Some((r, r)),
        (None, None) => None,
    }
}

/// Band-pass filtered stereo peak detector shared by the keyed processors.
#[derive(Clone, Copy, Default)]
pub struct Detector {
    f: [Biquad; 2],
}

impl Detector {
    pub fn configure(&mut self, hz: f64, width: f64, sr: f64) {
        for v in self.f.iter_mut() {
            v.set(
                BiquadType::BandPass,
                hz,
                1.0 / (2.0 * (2.0f64.ln() * width / 2.0).sinh()),
                0.0,
                sr,
            );
        }
    }

    #[inline(always)]
    pub fn sample(&mut self, mut l: f64, mut r: f64, filter: bool, gain: f64) -> f64 {
        if filter {
            l = self.f[0].tick(l);
            r = self.f[1].tick(r);
        }
        max(l.abs(), r.abs()) * gain
    }

    pub fn reset(&mut self) {
        for v in self.f.iter_mut() {
            v.reset();
        }
    }
}

/// Gain reduction in dB for a level above the threshold, with a quadratic soft knee.
#[inline(always)]
pub fn reduction(level: f64, threshold: f64, ratio: f64, knee: f64) -> f64 {
    let over = level - threshold;
    let slope = 1.0 - 1.0 / ratio;
    if knee > 0.0 && over.abs() < knee / 2.0 {
        return slope * (over + knee / 2.0) * (over + knee / 2.0) / (2.0 * knee);
    }
    if over > 0.0 {
        slope * over
    } else {
        0.0
    }
}

pub struct Compressor {
    core: Core,
    envelope: f64,
    feedback_l: f64,
    feedback_r: f64,
    detector: Detector,
    makeup: Smoother,
    wet: Smoother,
    reduction_gain: Smoother,
    // Control-rate values, derived only when parameters change.
    cache: ParameterCache<12>,
    attack: f64,
    release: f64,
    gain: f64,
    sc_gain: f64,
    smooth: f64,
    gain_smooth: f64,
}

impl Compressor {
    pub fn new(sr: f64) -> Self {
        Compressor {
            core: Core::new(sr),
            envelope: 0.0,
            feedback_l: 0.0,
            feedback_r: 0.0,
            detector: Detector::default(),
            makeup: Smoother::default(),
            wet: Smoother::default(),
            reduction_gain: Smoother::default(),
            cache: ParameterCache::default(),
            attack: 0.0,
            release: 0.0,
            gain: 1.0,
            sc_gain: 1.0,
            smooth: 0.0,
            gain_smooth: 0.0,
        }
    }
}

impl Processor for Compressor {
    core_accessors!(core);

    fn reset(&mut self) {
        self.envelope = 0.0;
        self.feedback_l = 0.0;
        self.feedback_r = 0.0;
        self.detector.reset();
        self.makeup.reset();
        self.wet.reset();
        self.reduction_gain.reset();
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
            self.attack = pole(p[4], sr);
            self.release = pole(p[5], sr);
            self.gain = db_gain(p[6]);
            self.sc_gain = db_gain(p[8]);
            self.smooth = pole(5.0, sr);
            self.gain_smooth = pole(0.3, sr);
            self.detector.configure(p[10], p[11], sr);
        }
        let key = external_key(sc_l, sc_r);
        for i in 0..l.len() {
            let dl = l[i] as f64;
            let dr = r[i] as f64;
            let (det_l, det_r) = match key {
                Some((kl, kr)) => (kl[i] as f64, kr[i] as f64),
                None => {
                    if p[0] >= 0.5 {
                        (self.feedback_l, self.feedback_r)
                    } else {
                        (dl, dr)
                    }
                }
            };
            let peak = self
                .detector
                .sample(det_l, det_r, p[9] >= 0.5, self.sc_gain);
            let c = select(peak > self.envelope, self.attack, self.release);
            self.envelope = clean(peak + c * (self.envelope - peak));
            // A feedback detector needs R-1 rather than 1-1/R to reach the
            // requested steady-state ratio, because it observes reduced audio.
            let mut gr = reduction(gain_db(self.envelope), p[1], p[2], p[3]);
            if p[0] >= 0.5 && key.is_none() {
                gr *= p[2];
            }
            let g = self
                .reduction_gain
                .tick(fast_db_gain(-min(120.0, gr)), self.gain_smooth);
            self.feedback_l = dl * g;
            self.feedback_r = dr * g;
            let mg = self.makeup.tick(self.gain, self.smooth);
            let w = self.wet.tick(p[7] / 100.0, self.smooth);
            l[i] = mix(dl, self.feedback_l * mg, w) as f32;
            r[i] = mix(dr, self.feedback_r * mg, w) as f32;
        }
    }
}

static GATE_PARAMS: [P; 9] = [
    P::new(c"threshold", c"Threshold", c"dB", -80.0, 0.0, -36.0, 0),
    P::new(c"attack", c"Attack", c"ms", 0.05, 100.0, 1.0, LOG),
    P::new(c"hold", c"Hold", c"ms", 0.0, 1000.0, 30.0, 0),
    P::new(c"release", c"Release", c"ms", 1.0, 2000.0, 80.0, LOG),
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

pub struct Gate {
    core: Core,
    gain: f64,
    remaining: u32,
    detector: Detector,
    wet: Smoother,
    cache: ParameterCache<9>,
    threshold: f64,
    attack: f64,
    release: f64,
    sc_gain: f64,
    smooth: f64,
    hold: u32,
}

impl Gate {
    pub fn new(sr: f64) -> Self {
        Gate {
            core: Core::new(sr),
            gain: 0.0,
            remaining: 0,
            detector: Detector::default(),
            wet: Smoother::default(),
            cache: ParameterCache::default(),
            threshold: 1.0,
            attack: 0.0,
            release: 0.0,
            sc_gain: 1.0,
            smooth: 0.0,
            hold: 0,
        }
    }
}

impl Processor for Gate {
    core_accessors!(core);

    fn reset(&mut self) {
        self.gain = 0.0;
        self.remaining = 0;
        self.detector.reset();
        self.wet.reset();
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
            self.threshold = db_gain(p[0]);
            self.attack = pole(p[1], sr);
            self.release = pole(p[3], sr);
            self.sc_gain = db_gain(p[5]);
            self.smooth = pole(5.0, sr);
            self.hold = (p[2] * 0.001 * sr) as u32;
            self.detector.configure(p[7], p[8], sr);
        }
        let key = external_key(sc_l, sc_r);
        for i in 0..l.len() {
            let dl = l[i] as f64;
            let dr = r[i] as f64;
            let (kl, kr) = match key {
                Some((kl, kr)) => (kl[i] as f64, kr[i] as f64),
                None => (dl, dr),
            };
            let peak = self.detector.sample(kl, kr, p[6] >= 0.5, self.sc_gain);
            // 3 dB hysteresis prevents threshold chatter on long decays.
            if peak
                > self.threshold
                    * (if self.remaining != 0 {
                        0.707945784
                    } else {
                        1.0
                    })
            {
                self.remaining = self.hold.wrapping_add(1);
            }
            let target = if self.remaining != 0 { 1.0 } else { 0.0 };
            if self.remaining != 0 {
                self.remaining -= 1;
            }
            let c = select(target > self.gain, self.attack, self.release);
            self.gain = clean(target + c * (self.gain - target));
            let g = mix(1.0, self.gain, self.wet.tick(p[4] / 100.0, self.smooth));
            l[i] = (dl * g) as f32;
            r[i] = (dr * g) as f32;
        }
    }
}

static TRANSIENT_PARAMS: [P; 4] = [
    P::new(c"gain", c"Input gain", c"dB", -24.0, 24.0, 0.0, 0),
    P::new(c"attack", c"Attack", c"%", -100.0, 100.0, 0.0, 0),
    P::new(c"sustain", c"Sustain", c"%", -100.0, 100.0, 0.0, 0),
    P::new(c"limit", c"Limit", c"", 0.0, 1.0, 0.0, TOGGLE),
];

pub struct Transient {
    core: Core,
    fast: f64,
    slow: f64,
    tail_env: f64,
    input: Smoother,
    shape: Smoother,
    fa: f64,
    fr: f64,
    sa: f64,
    ss: f64,
    tr: f64,
    smooth: f64,
    input_db: Memo,
}

impl Transient {
    pub fn new(sr: f64) -> Self {
        Transient {
            core: Core::new(sr),
            fast: 0.0,
            slow: 0.0,
            tail_env: 0.0,
            input: Smoother::default(),
            shape: Smoother::default(),
            fa: pole(0.2, sr),
            fr: pole(20.0, sr),
            sa: pole(15.0, sr),
            ss: pole(180.0, sr),
            tr: pole(450.0, sr),
            smooth: pole(2.0, sr),
            input_db: Memo::default(),
        }
    }
}

impl Processor for Transient {
    core_accessors!(core);

    fn reset(&mut self) {
        self.fast = 0.0;
        self.slow = 0.0;
        self.tail_env = 0.0;
        self.input.reset();
        self.shape.reset();
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
        let input_gain = self.input_db.get([p[0]], || db_gain(p[0]));
        for i in 0..l.len() {
            let g = self.input.tick(input_gain, self.smooth);
            let dl = l[i] as f64 * g;
            let dr = r[i] as f64 * g;
            let peak = max(dl.abs(), dr.abs());
            self.fast =
                clean(peak + select(peak > self.fast, self.fa, self.fr) * (self.fast - peak));
            self.slow =
                clean(peak + select(peak > self.slow, self.sa, self.ss) * (self.slow - peak));
            self.tail_env = clean(max(peak, self.tail_env * self.tr));
            let attack = clamp((self.fast - self.slow) / (self.fast + 1e-10), 0.0, 1.0);
            let sustain = clamp(
                (self.tail_env - self.fast) / (self.tail_env + 1e-10),
                0.0,
                1.0,
            );
            let change = (p[1] * 0.18) * attack + (p[2] * 0.18) * sustain;
            let sg = self.shape.tick(fast_db_gain(change), self.smooth);
            l[i] = (if p[3] >= 0.5 {
                clamp(dl * sg, -1.0, 1.0)
            } else {
                dl * sg
            }) as f32;
            r[i] = (if p[3] >= 0.5 {
                clamp(dr * sg, -1.0, 1.0)
            } else {
                dr * sg
            }) as f32;
        }
    }
}

const LIMITER_MODES: &[Label] = &[Label::new(c"Legacy"), Label::new(c"Transparent")];
static LIMITER_PARAMS: [P; 8] = [
    P::choice(c"mode", c"Mode", c"", 0.0, 1.0, 1.0, INTEGER, LIMITER_MODES),
    P::new(c"threshold", c"Threshold", c"dB", -40.0, 0.0, 0.0, 0),
    P::new(c"release", c"Release", c"ms", 1.0, 500.0, 50.0, LOG),
    P::new(c"ceiling", c"Ceiling", c"dB", -40.0, -0.3, -0.3, 0),
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

pub struct Limiter {
    core: Core,
    ahead: u32,
    dl: DelayLine,
    dr: DelayLine,
    envelope: f64,
    held: u32,
    detector: Detector,
    drive: Smoother,
    cache: ParameterCache<8>,
    release: f64,
    ceiling: f64,
    sc_gain: f64,
    smooth: f64,
    threshold: f64,
}

impl Limiter {
    pub fn new(sr: f64) -> Self {
        let ahead = (sr * 0.003).round() as u32;
        Limiter {
            core: Core::new(sr),
            ahead,
            dl: DelayLine::new(ahead as usize + 4),
            dr: DelayLine::new(ahead as usize + 4),
            envelope: 0.0,
            held: 0,
            detector: Detector::default(),
            drive: Smoother::default(),
            cache: ParameterCache::default(),
            release: 0.0,
            ceiling: 1.0,
            sc_gain: 1.0,
            smooth: 0.0,
            threshold: 1.0,
        }
    }
}

impl Processor for Limiter {
    core_accessors!(core);

    fn latency(&self) -> u32 {
        self.ahead
    }

    fn tail(&self) -> f64 {
        0.003
    }

    fn reset(&mut self) {
        self.dl.reset();
        self.dr.reset();
        self.envelope = 0.0;
        self.held = 0;
        self.detector.reset();
        self.drive.reset();
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
            self.release = pole(p[2], sr);
            self.ceiling = db_gain(p[3]);
            self.sc_gain = db_gain(p[4]);
            self.smooth = pole(5.0, sr);
            self.threshold = db_gain(-p[1]);
            self.detector.configure(p[6], p[7], sr);
        }
        let key = external_key(sc_l, sc_r);
        let ahead = self.ahead as f64;
        let ceiling = self.ceiling;
        for i in 0..l.len() {
            let boost = self.drive.tick(self.threshold, self.smooth);
            let xl = l[i] as f64 * boost;
            let xr = r[i] as f64 * boost;
            let (sl, sr_) = match key {
                Some((kl, kr)) => (kl[i] as f64 * boost, kr[i] as f64 * boost),
                None => (xl, xr),
            };
            let detected = self.detector.sample(sl, sr_, p[5] >= 0.5, self.sc_gain);
            // Main peak also protects the ceiling when an external key is quiet.
            let peak = max(detected, max(xl.abs(), xr.abs()));
            if peak >= self.envelope {
                self.envelope = peak;
                self.held = self.ahead;
            } else if self.held != 0 {
                self.held -= 1;
            } else {
                self.envelope = clean(peak + self.release * (self.envelope - peak));
            }
            let g = min(1.0, ceiling / max(self.envelope, 1e-12));
            let mut yl = self.dl.read(ahead) * g;
            let mut yr = self.dr.read(ahead) * g;
            self.dl.push(xl);
            self.dr.push(xr);
            if p[0] < 0.5 {
                yl = ceiling * (yl / ceiling).tanh();
                yr = ceiling * (yr / ceiling).tanh();
            }
            l[i] = clamp(yl, -ceiling, ceiling) as f32;
            r[i] = clamp(yr, -ceiling, ceiling) as f32;
        }
    }
}

static MAXIMIZER_PARAMS: [P; 7] = [
    P::new(c"amount", c"Amount", c"dB", 0.0, 30.0, 3.0, 0),
    P::new(c"curve", c"Curve", c"%", 0.0, 100.0, 50.0, 0),
    P::new(c"turbo", c"Turbo", c"", 0.0, 1.0, 0.0, TOGGLE),
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

pub struct Maximizer {
    core: Core,
    envelope: f64,
    second: f64,
    detector: Detector,
    drive: Smoother,
    cache: ParameterCache<7>,
    release: f64,
    smooth: f64,
    sc_gain: f64,
    ceiling: f64,
    amount: f64,
}

impl Maximizer {
    pub fn new(sr: f64) -> Self {
        Maximizer {
            core: Core::new(sr),
            envelope: 0.0,
            second: 0.0,
            detector: Detector::default(),
            drive: Smoother::default(),
            cache: ParameterCache::default(),
            release: 0.0,
            smooth: 0.0,
            sc_gain: 1.0,
            ceiling: 1.0,
            amount: 1.0,
        }
    }
}

impl Processor for Maximizer {
    core_accessors!(core);

    fn reset(&mut self) {
        self.envelope = 0.0;
        self.second = 0.0;
        self.detector.reset();
        self.drive.reset();
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
            self.release = pole(180.0 - 150.0 * p[1] / 100.0, sr);
            self.smooth = pole(5.0, sr);
            self.sc_gain = db_gain(p[3]);
            self.ceiling = db_gain(-0.3);
            self.amount = db_gain(p[0]);
            self.detector.configure(p[5], p[6], sr);
        }
        let key = external_key(sc_l, sc_r);
        let release = self.release;
        let ceiling = self.ceiling;
        for i in 0..l.len() {
            let boost = self.drive.tick(self.amount, self.smooth);
            let xl = l[i] as f64 * boost;
            let xr = r[i] as f64 * boost;
            let (kl, kr) = match key {
                Some((kl, kr)) => (kl[i] as f64 * boost, kr[i] as f64 * boost),
                None => (xl, xr),
            };
            let key_level = self.detector.sample(kl, kr, p[4] >= 0.5, self.sc_gain);
            let peak = max(key_level, max(xl.abs(), xr.abs()));
            self.envelope = clean(max(peak, peak + release * (self.envelope - peak)));
            let knee = 12.0 * (1.0 - p[1] / 100.0);
            let gr = reduction(gain_db(self.envelope), -3.0, 20.0, knee);
            let g = fast_db_gain(-gr);
            let mut yl = xl * g;
            let mut yr = xr * g;
            if p[2] >= 0.5 {
                yl *= 1.8;
                yr *= 1.8;
                let pk = max(yl.abs(), yr.abs());
                self.second = clean(max(pk, pk + release * (self.second - pk)));
                let g2 = fast_db_gain(-reduction(gain_db(self.second), -3.0, 20.0, knee));
                yl *= g2;
                yr *= g2;
            }
            l[i] = clamp(yl, -ceiling, ceiling) as f32;
            r[i] = clamp(yr, -ceiling, ceiling) as f32;
        }
    }
}

static DESCRIPTORS: [Descriptor; 5] = [
    Descriptor::new(
        c"compressor",
        c"Compressor",
        c"Dynamics",
        EFFECT_SIDECHAIN,
        &COMPRESSOR_PARAMS,
        |sr, _| Box::new(Compressor::new(sr)),
        |_, _, _| 0.0,
    ),
    Descriptor::new(
        c"gate",
        c"Gate",
        c"Dynamics",
        EFFECT_SIDECHAIN,
        &GATE_PARAMS,
        |sr, _| Box::new(Gate::new(sr)),
        |_, _, _| 0.0,
    ),
    Descriptor::new(
        c"transient",
        c"Transient Shaper",
        c"Dynamics",
        0,
        &TRANSIENT_PARAMS,
        |sr, _| Box::new(Transient::new(sr)),
        |_, _, _| 0.0,
    ),
    Descriptor::new(
        c"limiter",
        c"Peak Limiter",
        c"Dynamics",
        EFFECT_SIDECHAIN,
        &LIMITER_PARAMS,
        |sr, _| Box::new(Limiter::new(sr)),
        |_, _, _| 0.003,
    ),
    Descriptor::new(
        c"maximizer",
        c"Maximizer",
        c"Dynamics",
        EFFECT_SIDECHAIN,
        &MAXIMIZER_PARAMS,
        |sr, _| Box::new(Maximizer::new(sr)),
        |_, _, _| 0.0,
    ),
];

/// The catalogue entries of the dynamics effects.
pub fn descriptors() -> &'static [Descriptor] {
    &DESCRIPTORS
}

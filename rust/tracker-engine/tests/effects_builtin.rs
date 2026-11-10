//! Port of `tests/builtin_effects_tests.cpp`: the built-in effect catalogue and the effect
//! handle's contract (bypass, reset, parameter extremes, block partitioning), measured responses,
//! silence gating, zero mix, concurrent parameter changes, tail estimates and a throughput smoke
//! test.
//!
//! The C++ program counted its `check(...)` calls and printed "Builtin effects: N checks,
//! F failures, P processors". Each group here counts the same way on its own thread, prints its
//! own line and fails if any check failed.
//!
//! The catalogue is used through the safe Rust API; the effect handle through
//! `engine_support_b::fx` (the C functions, marked `TODO(api)` there).
//!
//! Dropped from the C++ program (C pointer mechanics): `st_builtin_effect_find(nullptr)` inside
//! the "invalid catalogue lookup" check (the other two lookups of that check are kept).
#![cfg(feature = "effects")]
// The index loops mirror the C++ loops they translate.
#![allow(clippy::needless_range_loop)]

mod engine_support_b;

use engine_support_b::alloc::{realtime, render_allocations, reset_render_allocations};
use engine_support_b::fx::{Fx, Sidechain};
use std::cell::Cell;
use std::collections::BTreeSet;
use std::f64::consts::PI;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tracker_engine::effects::capi::{normalized, physical};
use tracker_engine::effects::common::{PARAMETER_INTEGER, PARAMETER_LOG, PARAMETER_TOGGLE};
use tracker_engine::effects::{catalogue, find, Descriptor};

thread_local! {
    static CHECKS: Cell<u32> = const { Cell::new(0) };
    static FAILURES: Cell<u32> = const { Cell::new(0) };
}

/// C++ `check`: counts, and reports a failed check without stopping.
fn check(value: bool, name: &str) {
    CHECKS.with(|checks| checks.set(checks.get() + 1));
    if !value {
        FAILURES.with(|failures| failures.set(failures.get() + 1));
        eprintln!("FAIL {name}");
    }
}

/// The end of a group: the program's summary line and its exit status.
fn finish(group: &str) {
    let (checks, failures) = (CHECKS.with(|checks| checks.get()), FAILURES.with(|failures| failures.get()));
    println!("Builtin effects [{group}]: {checks} checks, {failures} failures, {} processors", catalogue().count());
    assert!(failures == 0, "{failures} failed checks in {group}");
}

fn id_of(d: &Descriptor) -> &'static str {
    d.info.id().to_str().expect("effect id")
}

/// C++ `struct FX`.
struct FX {
    id: String,
    p: Option<Fx>,
}

impl FX {
    /// `FX fx(id, sr = 48000, block = 256)`
    fn with(id: &str, sr: f64, block: u32) -> FX {
        let p = Fx::create(id, sr, block);
        check(p.is_some(), &format!("{id} creates"));
        FX { id: id.to_string(), p }
    }

    fn new(id: &str) -> FX {
        FX::with(id, 48000.0, 256)
    }

    fn fx(&mut self) -> &mut Fx {
        self.p.as_mut().expect("effect")
    }

    /// Sets the parameter with the given id to a physical value.
    fn set(&mut self, name: &str, value: f64) {
        let d = find(&std::ffi::CString::new(self.id.as_str()).expect("id")).expect("descriptor");
        for (j, q) in d.params.iter().enumerate() {
            if q.id().to_bytes() == name.as_bytes() {
                let set = self.fx().set_parameter(j as u32, normalized(q, value));
                check(set, &format!("{} sets {}", self.id, name));
                return;
            }
        }
        check(false, &format!("{} missing {}", self.id, name));
    }

    fn run_keyed(&mut self, l: &mut [f32], r: &mut [f32], sc: Option<&[f32]>) {
        let sidechain = sc.map_or(Sidechain::None, Sidechain::Left);
        let rendered = self.fx().process(l, r, sidechain);
        check(rendered, &format!("{} renders", self.id));
    }

    fn run(&mut self, l: &mut [f32], r: &mut [f32]) {
        self.run_keyed(l, r, None);
    }
}

fn rms_from(v: &[f32], start: usize) -> f64 {
    let sum: f64 = v[start..].iter().map(|&x| x as f64 * x as f64).sum();
    (sum / (v.len() - start) as f64).sqrt()
}

fn rms(v: &[f32]) -> f64 {
    rms_from(v, 0)
}

fn tone_gain(sr: f64, hz: f64, n: usize, gain: f64) -> Vec<f32> {
    (0..n).map(|i| (gain * (2.0 * PI * hz * i as f64 / sr).sin()) as f32).collect()
}

/// `tone(sr, hz, n)`: gain 0.25.
fn tone(sr: f64, hz: f64, n: usize) -> Vec<f32> {
    tone_gain(sr, hz, n, 0.25)
}

fn bin(v: &[f32], sr: f64, hz: f64, start: usize) -> f64 {
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for i in start..v.len() {
        re += v[i] as f64 * (2.0 * PI * hz * i as f64 / sr).cos();
        im -= v[i] as f64 * (2.0 * PI * hz * i as f64 / sr).sin();
    }
    re.hypot(im)
}

fn difference(a: &[f32], b: &[f32]) -> f64 {
    let mut peak = 0.0f64;
    for i in 0..a.len() {
        peak = peak.max((a[i] as f64 - b[i] as f64).abs());
    }
    peak
}

/// `memcmp` of two float buffers.
fn same_bits(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}

#[test]
fn catalogue_tests() {
    let count = catalogue().count();
    check(count >= 23, "catalogue includes core, spatial and delays");
    let mut ids = BTreeSet::new();
    for i in 0..count {
        let d = catalogue().nth(i).expect("descriptor");
        check(
            d.info.id().to_str().is_ok() && d.info.name().to_str().is_ok() && d.info.category().to_str().is_ok(),
            "descriptor strings",
        );
        check(ids.insert(d.info.id()), "unique effect id");
        check(find(d.info.id()).is_some_and(|found| std::ptr::eq(found, d)), "find returns stable descriptor");
        let mut params = BTreeSet::new();
        for j in 0..d.info.parameter_count as usize {
            let p = d.params.get(j);
            check(
                p.is_some_and(|p| p.id().to_str().is_ok() && p.name().to_str().is_ok() && p.unit().to_str().is_ok()),
                "parameter strings",
            );
            let p = p.expect("parameter");
            check(params.insert(p.id()), &format!("{} unique parameter", id_of(d)));
            check(
                p.minimum.is_finite()
                    && p.maximum > p.minimum
                    && p.default_value >= p.minimum
                    && p.default_value <= p.maximum,
                "valid physical range",
            );
            if p.flags & PARAMETER_LOG != 0 {
                check(p.minimum > 0.0, "positive log range");
            }
            let mut prev = -1.0f64;
            for k in 0..=100 {
                let raw = physical(p, k as f64 / 100.0);
                let norm = normalized(p, raw);
                check(
                    raw >= p.minimum - 1e-10 && raw <= p.maximum + 1e-10 && norm >= prev - 1e-10,
                    "monotone bounded mapping",
                );
                prev = norm;
                if p.flags & (PARAMETER_INTEGER | PARAMETER_TOGGLE) == 0 {
                    check((norm - k as f64 / 100.0).abs() < 1e-9, "parameter round trip");
                }
            }
            for k in 0..p.label_count() {
                check(p.label(k).is_some(), "enum label");
            }
            check(p.label(p.label_count()).is_none(), "enum bound");
        }
        check(d.params.get(d.info.parameter_count as usize).is_none(), "parameter bound");
    }
    check(catalogue().nth(count).is_none() && find(c"invalid").is_none(), "invalid catalogue lookup");
    check(
        Fx::create("eq", 0.0, 256).is_none()
            && Fx::create("eq", 48000.0, 0).is_none()
            && Fx::create("invalid", 48000.0, 256).is_none(),
        "invalid construction",
    );
    finish("catalogue");
}

fn contract_tests(sr: f64) {
    const N: usize = 8192;
    for d in catalogue() {
        let mut fx = FX::with(id_of(d), sr, 127);
        let id = fx.id.clone();
        let mut original = tone(sr, 719.0, N);
        original[0] = 0.8;
        let (mut l, mut r) = (original.clone(), original.clone());
        fx.fx().set_bypass(true);
        fx.run(&mut l, &mut r);
        check(same_bits(&l, &original) && same_bits(&r, &original), &format!("{id} exact bypass"));
        fx.fx().set_bypass(false);
        fx.run(&mut l, &mut r);
        let (first, first_r) = (l.clone(), r.clone());
        fx.fx().reset();
        l.copy_from_slice(&original);
        r.copy_from_slice(&original);
        fx.run(&mut l, &mut r);
        check(l == first && r == first_r, &format!("{id} deterministic reset"));
        fx.fx().reset();
        l.copy_from_slice(&original);
        r.copy_from_slice(&original);
        reset_render_allocations();
        realtime(|| fx.fx().process(&mut l, &mut r, Sidechain::None));
        check(render_allocations() == 0, &format!("{id} no render allocations"));
        check(
            !fx.fx().set_parameter(d.info.parameter_count, 0.5) && !fx.fx().set_parameter(0, f64::NAN),
            &format!("{id} invalid parameter rejected"),
        );
        for extreme in 0..4 {
            for p in 0..d.info.parameter_count {
                let value = if extreme == 0 {
                    0.0
                } else if extreme == 1 {
                    1.0
                } else if extreme == 2 {
                    if p % 2 != 0 {
                        1.0
                    } else {
                        0.0
                    }
                } else {
                    ((p * 37 + 19) % 101) as f64 / 100.0
                };
                fx.fx().set_parameter(p, value);
            }
            fx.fx().set_tempo(if extreme % 2 != 0 { 20.0 } else { 999.0 });
            let restored_tail = fx.fx().tail_seconds();
            check(
                restored_tail.is_finite() && restored_tail >= 0.0,
                &format!("{id} tail available before rendering restored values"),
            );
            fx.fx().reset();
            l.copy_from_slice(&original);
            r.copy_from_slice(&original);
            l[12] = f32::INFINITY;
            r[70] = f32::NAN;
            reset_render_allocations();
            realtime(|| fx.fx().process(&mut l, &mut r, Sidechain::MainInput));
            let mut finite = true;
            for k in 0..N {
                finite = finite && l[k].is_finite() && r[k].is_finite() && l[k].abs() <= 1e6 && r[k].abs() <= 1e6;
            }
            check(finite, &format!("{id} finite bounded parameter extremes"));
            check(render_allocations() == 0, &format!("{id} no allocation on parameter changes/sidechain"));
            check(
                restored_tail == fx.fx().tail_seconds(),
                &format!("{id} tail estimate independent of whether rendering occurred"),
            );
        }
        check(fx.fx().tail_seconds().is_finite(), &format!("{id} finite tail metadata"));
        // A render call may be any size; maxFrames is scratch capacity, not a
        // correctness constraint. Compare different callback partitionings.
        let (mut chunked, mut single) = (FX::with(id_of(d), sr, 511), FX::with(id_of(d), sr, 31));
        let (mut a, mut b, mut c, mut e) = (original.clone(), original.clone(), original.clone(), original.clone());
        single.run(&mut a, &mut b);
        let mut pos = 0;
        while pos < N {
            let z = ((pos % 97) + 1).min(N - pos);
            chunked.fx().process(&mut c[pos..pos + z], &mut e[pos..pos + z], Sidechain::None);
            pos += z;
        }
        check(difference(&a, &c) < 2e-5 && difference(&b, &e) < 2e-5, &format!("{id} block partition invariance"));
    }
}

#[test]
fn contract_tests_48000() {
    contract_tests(48000.0);
    finish("contract 48000");
}

#[test]
fn contract_tests_96000() {
    contract_tests(96000.0);
    finish("contract 96000");
}

fn response_tests(sr: f64) {
    let n = sr as usize;
    {
        let mut f = FX::with("compressor", sr, 256);
        f.set("threshold", -18.0);
        f.set("ratio", 4.0);
        f.set("knee", 0.0);
        f.set("attack", 0.1);
        let mut l = vec![0.5f32; n];
        let mut r = l.clone();
        f.run(&mut l, &mut r);
        let expected = 10.0f64.powf((-18.0 + (-6.020599913 + 18.0) / 4.0) / 20.0);
        check((rms_from(&l, n / 2) - expected).abs() < 0.001, "compressor steady-state ratio");
        f.fx().reset();
        l.fill(0.03);
        r.copy_from_slice(&l);
        let sc = vec![0.8f32; n];
        f.run_keyed(&mut l, &mut r, Some(&sc));
        check(rms_from(&l, n / 2) < 0.01, "compressor external sidechain ducks quiet source");
    }
    {
        let mut f = FX::with("gate", sr, 256);
        let mut l = vec![0.001f32; n];
        let mut r = l.clone();
        f.run(&mut l, &mut r);
        check(rms_from(&l, n / 2) < 1e-6, "gate closes below threshold");
        f.fx().reset();
        l.fill(0.001);
        r.copy_from_slice(&l);
        let sc = vec![0.5f32; n];
        f.run_keyed(&mut l, &mut r, Some(&sc));
        check(rms_from(&l, n / 2) > 0.00099, "gate opens from external key");
    }
    {
        let mut f = FX::with("limiter", sr, 256);
        f.set("ceiling", -6.0);
        let mut l = vec![0.0f32; n];
        let mut r = l.clone();
        l[100] = 4.0;
        r[100] = 2.0;
        f.run(&mut l, &mut r);
        let latency = f.fx().latency() as usize;
        check((l[100 + latency] as f64 - 10.0f64.powf(-6.0 / 20.0)).abs() < 1e-6, "limiter lookahead and ceiling");
        let mut peak = 0.0f64;
        for v in &l {
            peak = peak.max((*v as f64).abs());
        }
        check(peak <= 10.0f64.powf(-6.0 / 20.0) + 1e-6, "limiter never exceeds sample ceiling");
    }
    {
        let mut f = FX::with("eq", sr, 256);
        let original = tone(sr, 1000.0, n);
        let (mut l, mut r) = (original.clone(), original.clone());
        f.run(&mut l, &mut r);
        check(l == original, "flat EQ exact unity");
        f.set("lowmidfreq", 1000.0);
        f.set("lowmidgain", 6.0);
        f.fx().reset();
        l.copy_from_slice(&original);
        r.copy_from_slice(&original);
        f.run(&mut l, &mut r);
        check(
            (rms_from(&l, n / 2) / rms_from(&original, n / 2) - 10.0f64.powf(6.0 / 20.0)).abs() < 0.01,
            "EQ peak gain at selected center",
        );
    }
    {
        let mut f = FX::with("filter", sr, 256);
        f.set("cutoff", 1000.0);
        f.set("mix", 100.0);
        let mut low = tone(sr, 100.0, n);
        let mut lr = low.clone();
        let mut high = tone(sr, 8000.0, n);
        let mut hr = high.clone();
        f.run(&mut low, &mut lr);
        f.fx().reset();
        f.run(&mut high, &mut hr);
        check(rms_from(&low, n / 2) > rms_from(&high, n / 2) * 20.0, "low-pass attenuates upper band");
        f.set("mode", 2.0);
        f.fx().reset();
        low = tone(sr, 100.0, n);
        lr = low.clone();
        f.run(&mut low, &mut lr);
        f.fx().reset();
        high = tone(sr, 8000.0, n);
        hr = high.clone();
        f.run(&mut high, &mut hr);
        check(rms_from(&high, n / 2) > rms_from(&low, n / 2) * 20.0, "high-pass attenuates lower band");
    }
    {
        let mut f = FX::with("fm", sr, 256);
        f.set("amount", 0.0);
        let original = tone(sr, 900.0, n);
        let (mut l, mut r) = (original.clone(), original.clone());
        f.run(&mut l, &mut r);
        let delay = f.fx().latency() as usize;
        let mut max = 0.0f64;
        for i in delay..n {
            max = max.max((l[i] as f64 - original[i - delay] as f64).abs());
        }
        check(max < 2e-7, "FM zero depth reconstructs delayed input");
    }
    {
        let mut f = FX::with("freqshifter", sr, 256);
        f.set("coarse", 200.0);
        let mut l = tone(sr, 2000.0, n);
        let mut r = l.clone();
        f.run(&mut l, &mut r);
        let (up, down, original) = (bin(&l, sr, 2200.0, n / 2), bin(&l, sr, 1800.0, n / 2), bin(&l, sr, 2000.0, n / 2));
        check(up > down * 20.0 && up > original * 20.0, "frequency shifter moves carrier to upper sideband");
    }
    {
        let mut f = FX::with("transient", sr, 256);
        let original: Vec<f32> = (0..n)
            .map(|i| (0.5 * (-(i as f64) / (sr * 0.03)).exp() * (2.0 * PI * 1000.0 * i as f64 / sr).sin()) as f32)
            .collect();
        let (mut l, mut r) = (original.clone(), original.clone());
        f.run(&mut l, &mut r);
        check(difference(&l, &original) < 1e-7, "neutral transient shaper unity");
        f.set("attack", 100.0);
        f.fx().reset();
        l.copy_from_slice(&original);
        r.copy_from_slice(&original);
        f.run(&mut l, &mut r);
        check(rms(&l) > rms(&original) * 1.2, "transient attack increases burst energy");
    }
}

#[test]
fn response_tests_48000() {
    response_tests(48000.0);
    finish("response 48000");
}

#[test]
fn response_tests_96000() {
    response_tests(96000.0);
    finish("response 96000");
}

#[test]
fn concurrent_tests() {
    let mut fx = FX::new("filter");
    let run = AtomicBool::new(true);
    let controls = fx.fx().controls();
    let mut l = tone(48000.0, 1000.0, 127);
    let mut r = l.clone();
    std::thread::scope(|scope| {
        let run = &run;
        let control = scope.spawn(move || {
            let mut i: i32 = 0;
            while run.load(Ordering::SeqCst) {
                controls.set_parameter(1, (i % 100) as f64 / 100.0);
                controls.set_tempo((20 + i % 980) as f64);
                controls.parameter(1);
                controls.tail_seconds();
                i = i.wrapping_add(1);
            }
        });
        for _ in 0..2000 {
            fx.fx().process(&mut l, &mut r, Sidechain::None);
        }
        run.store(false, Ordering::SeqCst);
        control.join().expect("control thread");
    });
    let mut finite = true;
    for x in &l {
        finite = finite && x.is_finite();
    }
    check(finite, "concurrent parameter mailbox remains finite");
    finish("concurrent");
}

#[test]
fn tail_restore_tests() {
    let mut f = FX::new("beatdelay");
    f.set("time", 31.0);
    f.set("unit", 4.0);
    f.set("offset", 100.0);
    f.set("feedback", 97.0);
    f.fx().set_tempo(20.0);
    let long_tail = f.fx().tail_seconds();
    check(long_tail > 15000.0 && long_tail < 20000.0, "maximum Beat Delay feedback tail before first render");
    f.set("feedback", 0.0);
    check((f.fx().tail_seconds() - 51.0).abs() < 0.01, "one long echo tail updates from restored parameters");
    f.fx().set_tempo(120.0);
    check((f.fx().tail_seconds() - 8.5).abs() < 0.01, "tail uses current tempo without a render");
    let (mut eq, mut filter, mut fm, mut phaser) = (FX::new("eq"), FX::new("filter"), FX::new("fm"), FX::new("phaser"));
    check(
        eq.fx().tail_seconds() > 0.0 && filter.fx().tail_seconds() > 0.0,
        "EQ and resonant filter account for ringing",
    );
    check(
        fm.fx().tail_seconds() >= 0.1 && phaser.fx().tail_seconds() >= 30.0,
        "FM crossover and feedback phaser cover long state",
    );
    finish("tail restore");
}

/// C++ `peakOf(v, from = 0, to = 0)`: `to == 0` means the end.
fn peak_of(v: &[f32], from: usize, mut to: usize) -> f64 {
    if to == 0 {
        to = v.len();
    }
    let mut m = 0.0f64;
    for value in &v[from..to] {
        m = m.max((*value as f64).abs());
    }
    m
}

/// The floating-point control register (`mrs x, fpcr`).
#[cfg(target_arch = "aarch64")]
fn fpcr() -> u64 {
    let value: u64;
    // SAFETY: reads a user-readable system register into a general-purpose register.
    unsafe { std::arch::asm!("mrs {}, fpcr", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

/// Silence gating: once the input has been silent for longer than the tail and the output has
/// decayed, the render is exact silence; the first non-silent block is processed normally.
#[test]
fn gating_tests() {
    let sr = 48000.0f64;
    {
        let mut f = FX::with("reverb", sr, 128);
        let mut l = vec![0.0f32; (sr * 0.25) as usize];
        let mut r = l.clone();
        l[0] = 0.5;
        r[0] = 0.5;
        f.run(&mut l, &mut r);
        let tail = f.fx().tail_seconds();
        let mut quiet = vec![0.0f32; (sr * (tail + 1.0)) as usize];
        let mut qr = quiet.clone();
        f.run(&mut quiet, &mut qr);
        check(
            peak_of(&quiet, quiet.len() - (sr * 0.5) as usize, 0) == 0.0
                && peak_of(&qr, qr.len() - (sr * 0.5) as usize, 0) == 0.0,
            "gated reverb renders exact silence after its tail",
        );
        check(peak_of(&quiet, 0, (sr * 0.1) as usize) > 0.0, "reverb tail is not cut while it rings");
        let mut hit = vec![0.0f32; 256];
        let mut hr = hit.clone();
        hit[10] = 0.5;
        hr[10] = 0.5;
        f.run(&mut hit, &mut hr);
        check(peak_of(&hit, 10, 0) > 0.1, "gated reverb resumes on the first non-silent sample");
        // Fresh instance comparison: the gated state had decayed, so the new hit matches a reset
        // effect closely.
        let mut fresh = FX::with("reverb", sr, 128);
        let mut a = vec![0.0f32; 256];
        let mut b = a.clone();
        a[10] = 0.5;
        b[10] = 0.5;
        fresh.run(&mut a, &mut b);
        check(difference(&a, &hit) < 1e-5, "resumed render matches a decayed (fresh) reverb");
    }
    // Sidechain counts as input: a silent main input with a live key is processed.
    {
        let mut f = FX::with("gate", sr, 128);
        let mut ql = vec![0.0f32; sr as usize];
        let mut qr = ql.clone();
        f.run(&mut ql, &mut qr);
        let key = tone_gain(sr, 200.0, 4800, 0.5);
        let mut l = vec![0.0f32; 4800];
        let mut r = l.clone();
        f.run_keyed(&mut l, &mut r, Some(&key));
        check(peak_of(&l, 0, 0) == 0.0, "silent main input with a key stays silent");
        let mut main = tone_gain(sr, 1000.0, 4800, 0.001);
        let mut mr = main.clone();
        f.run_keyed(&mut main, &mut mr, Some(&key));
        check(rms_from(&main, 2400) > 0.0006, "keyed gate opens after a long silence");
    }
    // Sustaining modes report the export cap and are never gated.
    {
        let mut f = FX::with("reverblegacy", sr, 128);
        f.set("mix", 100.0);
        let mut l = tone(sr, 330.0, (sr * 0.5) as usize);
        let mut r = l.clone();
        f.run(&mut l, &mut r);
        f.set("freeze", 1.0);
        let mut quiet = vec![0.0f32; (sr * 125.0) as usize];
        let mut qr = quiet.clone();
        f.run(&mut quiet, &mut qr);
        check(
            peak_of(&quiet, quiet.len() - sr as usize, 0) > 1e-4,
            "frozen legacy reverb keeps sustaining past the gate horizon",
        );
    }
    {
        let mut f = FX::with("grainstretch", sr, 128);
        f.set("on", 1.0);
        f.set("mix", 100.0);
        let mut l = tone(sr, 330.0, (sr * 4.5) as usize);
        let mut r = l.clone();
        f.run(&mut l, &mut r);
        let mut quiet = vec![0.0f32; (sr * 125.0) as usize];
        let mut qr = quiet.clone();
        f.run(&mut quiet, &mut qr);
        check(peak_of(&quiet, quiet.len() - sr as usize, 0) > 1e-3, "grain stretch capture loop is never gated");
    }
    // Stutter history keeps recording silence (not gateable): engaging after a long silence loops
    // silence.
    {
        let mut f = FX::with("stutter", sr, 128);
        let mut l = tone(sr, 330.0, sr as usize);
        let mut r = l.clone();
        f.run(&mut l, &mut r);
        let mut quiet = vec![0.0f32; (sr * 3.0) as usize];
        let mut qr = quiet.clone();
        f.run(&mut quiet, &mut qr);
        f.set("engage", 1.0);
        let mut after = vec![0.0f32; sr as usize];
        let mut ar = after.clone();
        f.run(&mut after, &mut ar);
        check(peak_of(&after, 0, 0) == 0.0, "stutter captures the recent silence, not stale audio");
    }
    #[cfg(target_arch = "aarch64")]
    {
        let before = fpcr();
        let mut f = FX::with("eq", sr, 128);
        let mut l = tone(sr, 330.0, 512);
        let mut r = l.clone();
        f.run(&mut l, &mut r);
        let after = fpcr();
        check(before == after, "render restores the caller's floating-point control register");
    }
    finish("gating");
}

/// Zero Mix with an identity dry path: exact dry output; after the tail time the wet DSP idles,
/// and raising Mix restarts it cleanly.
#[test]
fn zero_mix_tests() {
    let sr = 48000.0f64;
    for id in ["reverb", "plate", "metaverb", "ice", "reflex", "resochord", "reverblegacy", "saturator", "distortion"] {
        let mut f = FX::with(id, sr, 128);
        f.set("mix", 0.0);
        let original = tone(sr, 440.0, (sr * 8.0) as usize);
        let (mut l, mut r) = (original.clone(), original.clone());
        f.run(&mut l, &mut r);
        check(l == original && r == original, &format!("{id} zero mix is exactly dry"));
        f.set("mix", 50.0);
        let mut wet = tone(sr, 440.0, sr as usize);
        let mut wr = wet.clone();
        f.run(&mut wet, &mut wr);
        let mut finite = true;
        for v in &wet {
            finite = finite && v.is_finite();
        }
        check(
            finite && difference(&wet, &tone(sr, 440.0, sr as usize)) > 1e-3,
            &format!("{id} wet path resumes after zero mix"),
        );
    }
    finish("zero mix");
}

/// The throughput smoke test: every effect renders 200 blocks of 256 frames.
#[test]
fn performance() {
    let mut sum = 0.0f64;
    for d in catalogue() {
        let mut fx = FX::new(id_of(d));
        let original = tone(48000.0, 713.0, 256);
        let (mut l, mut r) = (original.clone(), original.clone());
        let start = Instant::now();
        for _ in 0..200 {
            l.copy_from_slice(&original);
            r.copy_from_slice(&original);
            fx.fx().process(&mut l, &mut r, Sidechain::None);
        }
        let seconds = start.elapsed().as_secs_f64();
        sum += seconds;
        println!(
            "BENCH {:<16} {:.3} % of realtime (48k stereo, 256-frame blocks)",
            id_of(d),
            100.0 * seconds / (200.0 * 256.0 / 48000.0)
        );
    }
    check(sum < 30.0, "aggregate throughput smoke threshold");
    finish("performance");
}

//! Port of `tests/drum_synth_tests.cpp`: the drum voices (`effects::drums`) and the `st_drum_*`
//! C API. Same checks, tolerances and check count (5514) as the C++ program.

mod effects_support;

use effects_support::{allocations, counted, reset_allocations};
use std::ffi::CStr;
use tracker_engine::effects::capi::*;
use tracker_engine::effects::drums::{self, Voice};

struct Checks {
    checks: usize,
    failures: Vec<String>,
}

impl Checks {
    fn check(&mut self, ok: bool, name: &str) {
        self.checks += 1;
        if !ok {
            eprintln!("FAIL {name}");
            self.failures.push(name.to_string());
        }
    }
}

fn defaults(model: i32) -> [f32; 8] {
    std::array::from_fn(|i| drums::default_parameter(model, i as u32))
}

fn render(c: &mut Checks, model: i32, sr: f64, p: &[f32], frames: usize, ratio: f64, seed: u32) -> Vec<f32> {
    let mut out = vec![0.0f32; frames];
    let mut v = Voice::default();
    v.start(model, sr, p, seed);
    reset_allocations();
    counted(|| {
        for sample in out.iter_mut() {
            *sample = v.tick(ratio);
        }
    });
    c.check(allocations() == 0, "drum render allocation-free");
    out
}

fn render_default(c: &mut Checks, model: i32, sr: f64, p: &[f32], frames: usize) -> Vec<f32> {
    render(c, model, sr, p, frames, 1.0, 12345)
}

fn energy(v: &[f32], from: usize, to: usize) -> f64 {
    let to = if to == 0 { v.len() } else { to };
    let e: f64 = v[from..to].iter().map(|&x| x as f64 * x as f64).sum();
    e / (to - from) as f64
}

fn difference(a: &[f32], b: &[f32]) -> f64 {
    let d: f64 = a.iter().zip(b.iter()).map(|(&x, &y)| (x as f64 - y as f64).abs()).sum();
    d / a.len() as f64
}

fn frequency(v: &[f32], sr: f64) -> f64 {
    let (mut first, mut last, mut count) = (0usize, 0usize, 0usize);
    for i in 1..v.len() {
        if v[i - 1] <= 0.0 && v[i] > 0.0 {
            if count == 0 {
                first = i;
            }
            last = i;
            count += 1;
        }
    }
    if count > 1 {
        (count - 1) as f64 * sr / (last - first) as f64
    } else {
        0.0
    }
}

fn drum_render(model: i32, controls: Option<&[f32]>, sr: f64, out: Option<&mut [f32]>) -> u32 {
    let (pointer, count) = controls.map_or((std::ptr::null(), 0), |c| (c.as_ptr(), c.len() as u32));
    let (out_pointer, capacity) = match out {
        Some(buffer) => (buffer.as_mut_ptr(), buffer.len() as u32),
        None => (std::ptr::null_mut(), 48000),
    };
    // SAFETY: the pointers come from live slices of the stated lengths (or are null).
    unsafe { st_drum_render(model, pointer, count, sr, out_pointer, capacity) }
}

#[test]
fn drum_synthesis() {
    let mut c = Checks { checks: 0, failures: Vec::new() };
    c.check(st_drum_model_count() == 6, "six drum model metadata entries");
    for model in 1..=6i32 {
        let info = st_drum_model_info((model - 1) as u32);
        // SAFETY: a non-null info pointer refers to a static catalogue entry.
        let info = unsafe { info.as_ref() };
        c.check(info.is_some_and(|info| unsafe { st_drum_model_for_id(info.id) } == model), "drum model ID stable");
        let info = info.expect("drum model info");
        let model_name = info.id().to_string_lossy().into_owned();
        let p = defaults(model);
        for i in 0..8u32 {
            // SAFETY: as above.
            let meta = unsafe { st_drum_parameter_info(model, i).as_ref() };
            c.check(meta.is_some_and(|m| m.default_value >= m.minimum && m.default_value <= m.maximum), "drum parameter range");
            c.check((st_drum_parameter_default(model, i) - p[i as usize] as f64).abs() < 1e-7, "native default equals C++ default");
            for n in 0..=100 {
                let physical = st_drum_parameter_from_normalized(model, i, n as f64 / 100.0);
                c.check((st_drum_parameter_to_normalized(model, i, physical) - n as f64 / 100.0).abs() < 1e-9, "drum mapping roundtrip");
            }
        }
        for sr in [48000.0f64, 96000.0] {
            let frames = (sr * 0.5) as usize;
            let baseline = render_default(&mut c, model, sr, &[], frames);
            let again = render_default(&mut c, model, sr, &p, frames);
            c.check(difference(&baseline, &again) < 1e-7, "default preset roundtrip");
            c.check(energy(&baseline, 0, 0) > 1e-5, "each family produces audio");
            let repeat = render_default(&mut c, model, sr, &p, frames);
            c.check(repeat == again, "drum deterministic seed/reset");
            for index in 0..8usize {
                let (mut low, mut high) = (p, p);
                low[index] = 0.0;
                high[index] = 1.0;
                let a = render_default(&mut c, model, sr, &low, frames);
                let b = render_default(&mut c, model, sr, &high, frames);
                c.check(difference(&a, &b) > 1e-5, &format!("{model_name} parameter {index} changes sound"));
            }
            for mode in 0..4 {
                let values: [f32; 8] = std::array::from_fn(|i| match mode {
                    0 => 0.0,
                    1 => 1.0,
                    2 => (i % 2) as f32,
                    _ => f32::NAN,
                });
                let ratio = match mode {
                    0 => 0.03125,
                    1 => 32.0,
                    _ => 1.0,
                };
                let extreme = render(&mut c, model, sr, &values, frames, ratio, 12345);
                let finite = extreme.iter().all(|v| v.is_finite() && v.abs() <= 1.0);
                c.check(finite, "drum extreme controls and pitches bounded");
            }
            let mut voice = Voice::default();
            reset_allocations();
            counted(|| voice.start(model, sr, &[], 0));
            c.check(allocations() == 0, "drum trigger allocates nothing");
            voice.stop();
            c.check(!voice.active() && voice.tick(1.0) == 0.0, "drum stop immediately silent");
            voice.start(0, sr, &p, 1);
            c.check(!voice.active() && voice.tick(1.0) == 0.0, "none does not synthesize");
        }
        let mut lifetime = Voice::default();
        let mut shortest = p;
        shortest[1] = 0.0;
        lifetime.start(model, 48000.0, &shortest, 1);
        for _ in 0..48000 {
            lifetime.tick(1.0);
        }
        c.check(!lifetime.active(), "short drum finishes without held note-off");
        for sr in [8000.0f64, 384000.0] {
            let mut v = Voice::default();
            let maximum = [1.0f32; 8];
            v.start(model, sr, &maximum, 1);
            let mut finite = true;
            for i in 0..2048 {
                let sample = v.tick(if i % 2 != 0 { 1e9 } else { 1e-9 });
                finite = finite && sample.is_finite() && sample.abs() <= 1.0;
            }
            c.check(finite, "full supported rates and extreme pitch ratios remain bounded");
        }
    }
    let mut kick = defaults(1);
    kick[1] = 1.0;
    kick[3] = 0.0;
    kick[4] = 0.0;
    kick[7] = 0.0;
    let base = render_default(&mut c, 1, 48000.0, &kick, 48000);
    let octave = render(&mut c, 1, 48000.0, &kick, 48000, 2.0, 12345);
    c.check((frequency(&octave, 48000.0) / frequency(&base, 48000.0) - 2.0).abs() < 0.01, "keyboard octave doubles drum pitch");
    kick[0] = 0.75;
    let tuned = render_default(&mut c, 1, 48000.0, &kick, 48000);
    c.check((frequency(&tuned, 48000.0) / frequency(&base, 48000.0) - 2.0).abs() < 0.01, "Tune +12 doubles drum pitch");
    let sounds: Vec<Vec<f32>> = (1..=6).map(|m| render_default(&mut c, m, 48000.0, &[], 24000)).collect();
    for i in 0..6 {
        for j in i + 1..6 {
            c.check(difference(&sounds[i], &sounds[j]) > 0.005, "six families have distinct PCM");
        }
    }
    let unknown: &CStr = c"unknown";
    // SAFETY: `unknown` is a valid C string.
    let unknown_model = unsafe { st_drum_model_for_id(unknown.as_ptr()) };
    c.check(
        st_drum_model_info(6).is_null() && st_drum_parameter_info(0, 0).is_null() && st_drum_parameter_info(1, 8).is_null() && unknown_model == 0,
        "drum metadata invalid inputs rejected",
    );
    // Offline renders (drum-model synths migrate to samples of these): audible, finite,
    // decaying to the end, deterministic, and bounded by the caller's capacity.
    for model in 1..=6i32 {
        let mut a = vec![0.0f32; 48000 * 13];
        let mut b = vec![0.0f32; 48000 * 13];
        let p = defaults(model);
        let n = drum_render(model, Some(&p), 48000.0, Some(&mut a)) as usize;
        let m = drum_render(model, Some(&p), 48000.0, Some(&mut b)) as usize;
        c.check(n > 480 && n == m && a[..n] == b[..n], "offline drum render is audible and deterministic");
        let finite = a[..n].iter().all(|v| v.is_finite() && v.abs() <= 4.0);
        c.check(finite, "offline drum render is finite");
        c.check(energy(&a, 0, n / 8) > energy(&a, n - n / 8, n), "offline drum render decays");
        c.check(drum_render(model, Some(&p), 48000.0, Some(&mut a[..64])) == 64, "offline drum render honours capacity");
    }
    let nan = [f32::NAN; 8];
    let mut out = vec![0.0f32; 48000];
    let mut reference = vec![0.0f32; 48000];
    let with_nan = drum_render(2, Some(&nan), 48000.0, Some(&mut out));
    let with_defaults = drum_render(2, None, 48000.0, Some(&mut reference));
    c.check(with_nan == with_defaults && difference(&out, &reference) < 1e-4, "nonfinite controls render model defaults");
    c.check(
        drum_render(0, None, 48000.0, Some(&mut out)) == 0 && drum_render(1, None, 48000.0, None) == 0,
        "invalid offline render rejected",
    );
    println!("Drum synthesis: {} checks, {} failures", c.checks, c.failures.len());
    assert!(c.failures.is_empty(), "failed checks: {:?}", c.failures);
    assert_eq!(c.checks, 5514, "same number of checks as tests/drum_synth_tests.cpp");
}

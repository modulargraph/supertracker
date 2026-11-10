//! Test tones for the engine: this batch's copy of `tests/engine_test_tone.hpp`. The built-in
//! synth is retired; tests that need a plain sustained tone use a sampler in Wavetable mode on a
//! generated single cycle, which is also what migrated projects play.

use super::engine::Engine;
use tracker_engine::engine::*;

pub const TEST_SINE: i32 = 0;
pub const TEST_SAW: i32 = 1;
pub const TEST_SQUARE: i32 = 2;
pub const TEST_TRIANGLE: i32 = 3;
pub const TEST_NOISE: i32 = 4;

/// One cycle (2048 samples) of `wave`; noise is one second of white noise.
pub fn test_cycle(wave: i32) -> Vec<f32> {
    let pi = std::f64::consts::PI;
    let frames: usize = if wave == TEST_NOISE { 48000 } else { ST_WAVETABLE_DEFAULT_FRAME as usize };
    let mut cycle = vec![0.0f32; frames];
    let mut state: u32 = 12345;
    for (i, sample) in cycle.iter_mut().enumerate() {
        let phase = i as f64 / frames as f64;
        *sample = match wave {
            TEST_SAW => (2.0 * phase - 1.0) as f32,
            TEST_SQUARE => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            TEST_TRIANGLE => (1.0 - 4.0 * (phase - 0.5).abs()) as f32,
            TEST_NOISE => {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state as f64 / 2147483648.0 - 1.0) as f32
            }
            _ => (2.0 * pi * phase).sin() as f32,
        };
    }
    cycle
}

/// A sampler instrument that plays a loaded test cycle as a sustained tone.
pub fn test_tone_instrument(wave: i32) -> STInstrument {
    let mut p = default_instrument();
    p.type_ = ST_INSTRUMENT_SAMPLE;
    p.mode = if wave == TEST_NOISE { ST_MODE_FORWARD } else { ST_MODE_WAVETABLE };
    p
}

/// Loads `wave` into `instrument` (draft or immediate, like `set_sample`).
pub fn load_test_tone(engine: &mut Engine, instrument: i32, wave: i32) {
    let cycle = test_cycle(wave);
    engine.set_sample(instrument, &cycle, None, 48000.0);
}

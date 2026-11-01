// Test tones for the native engine. The built-in synth is retired; tests that need a
// plain sustained tone use a sampler in Wavetable mode on a generated single cycle,
// which is also what migrated projects play.
#ifndef SUPERTRACKER_ENGINE_TEST_TONE_HPP
#define SUPERTRACKER_ENGINE_TEST_TONE_HPP
#include "TrackerEngine.h"
#include <cmath>
#include <cstdint>
#include <vector>

enum TestWave { TEST_SINE, TEST_SAW, TEST_SQUARE, TEST_TRIANGLE, TEST_NOISE };

/// One cycle (2048 samples) of `wave`; noise is one second of white noise.
inline std::vector<float> testCycle(int wave) {
    const double pi = 3.14159265358979323846;
    size_t frames = wave == TEST_NOISE ? 48000 : ST_WAVETABLE_DEFAULT_FRAME;
    std::vector<float> cycle(frames);
    uint32_t state = 12345;
    for (size_t i = 0; i < frames; ++i) {
        double phase = double(i) / double(frames);
        switch (wave) {
            case TEST_SAW: cycle[i] = float(2 * phase - 1); break;
            case TEST_SQUARE: cycle[i] = phase < .5 ? 1.f : -1.f; break;
            case TEST_TRIANGLE: cycle[i] = float(1 - 4 * std::abs(phase - .5)); break;
            case TEST_NOISE: state ^= state << 13; state ^= state >> 17; state ^= state << 5; cycle[i] = float(double(state) / 2147483648.0 - 1.0); break;
            default: cycle[i] = float(std::sin(2 * pi * phase)); break;
        }
    }
    return cycle;
}

/// A sampler instrument that plays a loaded test cycle as a sustained tone.
inline STInstrument testToneInstrument(int wave = TEST_SINE) {
    STInstrument p = st_default_instrument();
    p.type = ST_INSTRUMENT_SAMPLE;
    p.mode = wave == TEST_NOISE ? ST_MODE_FORWARD : ST_MODE_WAVETABLE;
    return p;
}

/// Loads `wave` into `instrument` (draft or immediate, like st_engine_set_sample).
inline void loadTestTone(STEngine *engine, int instrument, int wave = TEST_SINE) {
    auto cycle = testCycle(wave);
    st_engine_set_sample(engine, instrument, cycle.data(), nullptr, cycle.size(), 48000);
}
#endif

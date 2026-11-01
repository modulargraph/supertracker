#ifndef ST_EFFECTS_DISTORTION_HPP
#define ST_EFFECTS_DISTORTION_HPP

#include "BuiltinEffectsCommon.hpp"

namespace stfx {
namespace distortion_detail {

// Independently written algorithms. The parameter vocabulary follows familiar
// studio effects; these processors do not model or reproduce proprietary DSP.
inline const char * const distortionModes[] = {"Feedback Fuzz", "Analog"};
inline const char * const saturationModes[] = {"Classic", "Tape", "Tube"};
inline const char * const switches[] = {"Off", "On"};

inline const STBuiltinParameterInfo distortionParameters[] = {
    {"drive", "Drive", "dB", 0, 42, 12, 0, nullptr, 0},
    {"color", "Fuzz Color", "%", 0, 100, 65, 0, nullptr, 0},
    {"feedback", "Fuzz Feedback", "%", 0, 95, 15, 0, nullptr, 0},
    {"tone", "Tone", "Hz", 150, 16000, 5000, ST_BUILTIN_PARAMETER_LOG, nullptr, 0},
    {"tone_mod", "Fuzz Tone Mod", "%", 0, 100, 0, 0, nullptr, 0},
    {"mix", "Mix", "%", 0, 100, 100, 0, nullptr, 0},
    {"output", "Output", "dB", -24, 12, -6, 0, nullptr, 0},
    {"mode", "Mode", "", 0, 1, 0, ST_BUILTIN_PARAMETER_INTEGER, distortionModes, 2},
    {"bass", "Analog Bass", "%", 0, 100, 100, 0, nullptr, 0},
    {"focus", "Analog Focus", "", 0, 1, 0, ST_BUILTIN_PARAMETER_TOGGLE, switches, 2},
    {"definition", "Analog Definition", "%", 0, 100, 50, 0, nullptr, 0},
    {"gate", "Fuzz Gate", "", 0, 1, 1, ST_BUILTIN_PARAMETER_TOGGLE, switches, 2},
    {"gate_release", "Gate Release", "ms", 5, 2000, 100, ST_BUILTIN_PARAMETER_LOG, nullptr, 0},
    {"gate_threshold", "Gate Threshold", "dB", -96, -18, -60, 0, nullptr, 0},
    {"mod_rate", "Fuzz Mod Rate", "Hz", 0.05, 10, 0.4, ST_BUILTIN_PARAMETER_LOG, nullptr, 0},
    {"saturation", "Analog Saturation", "%", 0, 100, 25, 0, nullptr, 0}
};

inline const STBuiltinParameterInfo lofiParameters[] = {
    {"sample_rate", "Sample Rate", "Hz", 99.5, 48000, 12000, ST_BUILTIN_PARAMETER_LOG, nullptr, 0},
    {"bits", "Bits", "bits", 1, 24, 12, 0, nullptr, 0},
    {"smooth", "Smooth", "%", 0, 100, 0, 0, nullptr, 0},
    {"stereo", "Stereo Width", "%", 0, 200, 100, 0, nullptr, 0},
    {"mix", "Mix", "%", 0, 100, 100, 0, nullptr, 0},
    {"input", "Input", "dB", -24, 24, 0, 0, nullptr, 0},
    {"output", "Output", "dB", -24, 12, 0, 0, nullptr, 0},
    {"jitter", "Clock Jitter", "%", 0, 100, 0, 0, nullptr, 0}
};

inline const STBuiltinParameterInfo saturationParameters[] = {
    {"mode", "Mode", "", 0, 2, 0, ST_BUILTIN_PARAMETER_INTEGER, saturationModes, 3},
    {"input", "Input", "dB", -24, 24, 0, 0, nullptr, 0},
    {"contour", "Contour", "%", 0, 100, 50, 0, nullptr, 0},
    {"drive", "Drive", "%", 0, 100, 25, 0, nullptr, 0},
    {"charge", "Tube Charge", "%", 0, 100, 25, 0, nullptr, 0},
    {"overload", "Tube Overload", "dB", 0, 18, 0, 0, nullptr, 0},
    {"mix", "Mix", "%", 0, 100, 100, 0, nullptr, 0},
    {"output", "Output", "dB", -24, 12, 0, 0, nullptr, 0},
    {"bass", "Tube Bass", "dB", -18, 18, 0, 0, nullptr, 0},
    {"treble", "Tube Treble", "dB", -18, 18, 0, 0, nullptr, 0},
    {"eq_bypass", "Tube EQ Bypass", "", 0, 1, 0, ST_BUILTIN_PARAMETER_TOGGLE, switches, 2}
};

inline double parameter(const Processor &processor, const STBuiltinParameterInfo *info, size_t i) noexcept {
    const double value = processor.p[i];
    return std::isfinite(value) ? clamp(value, info[i].minimum, info[i].maximum) : info[i].defaultValue;
}

// Smoothed controls. Once every value has reached (snapped to) its target,
// ticking is skipped until a target changes: identical values, no per-sample work.
template<size_t N> struct Controls {
    std::array<Smoother, N> smooth{};
    std::array<double, N> target{}, value{};
    bool settled = false;
    void setTargets(const std::array<double, N> &t) noexcept { if (t != target) { target = t; settled = false; } }
    ST_HOT void tick(double coefficient) noexcept {
        if (settled) return;
        bool all = true;
        for (size_t i = 0; i < N; ++i) { value[i] = smooth[i].tick(target[i], coefficient); all = all && value[i] == target[i]; }
        settled = all;
    }
    void reset() noexcept { for (auto &s : smooth) s.reset(); settled = false; }
};
inline double filterPole(double hz, double sampleRate) noexcept {
    return std::exp(-2 * pi * clamp(hz, 5, sampleRate * 0.45) / sampleRate);
}
// One-pole low-pass and DC blocker are contractive (coefficient < 1), so
// finite input keeps them finite and bounded; render runs with denormals
// flushed, so no per-sample clean-up is needed.
ST_INLINE double lowpass(double input, double &state, double coefficient) noexcept {
    state = input + coefficient * (state - input);
    return state;
}

struct DCBlock {
    double previous = 0, state = 0;
    ST_HOT double tick(double x, double coefficient) noexcept {
        const double y = x - previous + coefficient * state;
        previous = x;
        state = y;
        return y;
    }
    void reset() noexcept { previous = state = 0; }
};

// Fixed 2x interpolation plus a four-pole decimation filter reduces fold-back
// from the nonlinear stages. No explicit buffered latency is introduced.
// The decimation filter is a fixed, stable low-pass: its state needs no
// per-sample clean-up (render runs with denormals flushed).
struct OversampledShape {
    Biquad first, second;
    double previous = 0;
    explicit OversampledShape(double sr) {
        first.set(Biquad::LowPass, sr * 0.45, 0.5411961001, 0, sr * 2);
        second.set(Biquad::LowPass, sr * 0.45, 1.306562965, 0, sr * 2);
    }
    ST_HOT static double step(Biquad &f, double x) noexcept {
        const double y = f.b0 * x + f.z1;
        f.z1 = f.b1 * x - f.a1 * y + f.z2;
        f.z2 = f.b2 * x - f.a2 * y;
        return y;
    }
    template<class Shape> ST_HOT double tick(double input, Shape shape) noexcept {
        step(second, step(first, shape((previous + input) * 0.5)));
        const double result = clean(step(second, step(first, shape(input))));
        previous = input;
        return result;
    }
    void reset() noexcept { previous = 0; first.reset(); second.reset(); }
};

// OversampledShape for both channels at once: identical arithmetic, laid out
// as two lanes so the shaper and decimation filters vectorize.
struct StereoShape {
    double b0, b1, b2, a1, a2, c0, c1, c2, e1, e2;
    double previous[2]{}, z1a[2]{}, z2a[2]{}, z1b[2]{}, z2b[2]{};
    explicit StereoShape(double sr) {
        const OversampledShape mono(sr);
        b0 = mono.first.b0; b1 = mono.first.b1; b2 = mono.first.b2; a1 = mono.first.a1; a2 = mono.first.a2;
        c0 = mono.second.b0; c1 = mono.second.b1; c2 = mono.second.b2; e1 = mono.second.a1; e2 = mono.second.a2;
    }
    template<class Shape> ST_HOT void tick(const double *in, double *out, Shape shape) noexcept {
        double mid[2], cur[2];
        for (int c = 0; c < 2; ++c) { mid[c] = shape((previous[c] + in[c]) * 0.5); cur[c] = shape(in[c]); previous[c] = in[c]; }
        for (int c = 0; c < 2; ++c) {
            double y = b0 * mid[c] + z1a[c]; z1a[c] = b1 * mid[c] - a1 * y + z2a[c]; z2a[c] = b2 * mid[c] - a2 * y;
            double w = c0 * y + z1b[c]; z1b[c] = c1 * y - e1 * w + z2b[c]; z2b[c] = c2 * y - e2 * w;
            y = b0 * cur[c] + z1a[c]; z1a[c] = b1 * cur[c] - a1 * y + z2a[c]; z2a[c] = b2 * cur[c] - a2 * y;
            w = c0 * y + z1b[c]; z1b[c] = c1 * y - e1 * w + z2b[c]; z2b[c] = c2 * y - e2 * w;
            out[c] = w;
        }
        out[0] = clean(out[0]); out[1] = clean(out[1]);
    }
    void reset() noexcept { for (int c = 0; c < 2; ++c) previous[c] = z1a[c] = z2a[c] = z1b[c] = z2b[c] = 0; }
};

class Distortion final : public Processor {
    struct Channel {
        DelayLine feedback;
        DCBlock fuzzDC, analogDC;
        double tone = 0, color = 0, bass = 0, focusLow = 0, focusHigh = 0, analogTone = 0;
        explicit Channel(double sr) : feedback(size_t(std::ceil(sr * 0.012)) + 4) {}
        void reset() noexcept {
            feedback.reset(); fuzzDC.reset(); analogDC.reset();
            tone = color = bass = focusLow = focusHigh = analogTone = 0;
        }
    };
    Channel channels[2];
    StereoShape fuzzShape, analogShape;
    Controls<16> c;
    const double smoothing, gateAttack, envelopeRelease, bassPole, focusLowPole, focusHighPole, dcPole;
    double phase = 0, envelope = 0, gateGain = 0;
    // Paths whose output weight is exactly zero (Mode crossfade settled, or
    // Mix at 0) are not computed; their state restarts when they return.
    bool fuzzLive = false, analogLive = false;

    void resetFuzz() noexcept { fuzzShape.reset(); for (auto &ch : channels) { ch.feedback.reset(); ch.fuzzDC.reset(); ch.tone = ch.color = 0; } }
    void resetAnalog() noexcept { analogShape.reset(); for (auto &ch : channels) { ch.analogDC.reset(); ch.bass = ch.focusLow = ch.focusHigh = ch.analogTone = 0; } }

public:
    explicit Distortion(double sampleRate) : Processor(sampleRate), channels{Channel(sampleRate), Channel(sampleRate)},
        fuzzShape(sampleRate), analogShape(sampleRate),
        smoothing(pole(8, sr)), gateAttack(pole(0.5, sr)), envelopeRelease(pole(10, sr)),
        bassPole(filterPole(180, sr)), focusLowPole(filterPole(500, sr)), focusHighPole(filterPole(2200, sr)),
        dcPole(filterPole(12, sr)) {
        for (size_t i = 0; i < 16; ++i) p[i] = distortionParameters[i].defaultValue;
    }
    ParameterCache<16> cache;
    void process(float *l, float *r, const float *, const float *, uint32_t frames) noexcept override {
        auto value = [&](size_t i) { return parameter(*this, distortionParameters, i); };
        if (cache.changed(*this)) c.setTargets({dbGain(value(0)), filterPole(300 * std::pow(60.0, value(1) * 0.01), sr), value(2) * 0.0099,
            filterPole(value(3), sr), value(4) * 0.01, value(5) * 0.01, dbGain(value(6)), std::round(value(7)),
            value(8) * 0.01, std::round(value(9)), value(10) * 0.01, std::round(value(11)), pole(value(12), sr),
            dbGain(value(13)), value(14) / sr, 1 + value(15) * 0.15});
        for (uint32_t i = 0; i < frames; ++i) {
            c.tick(smoothing);
            const auto &v = c.value;
            const double a = clean(l[i]), b = clean(r[i]), level = std::max(std::abs(a), std::abs(b));
            envelope = std::max(level, envelope * envelopeRelease);
            const double target = envelope >= v[13] ? 1 : 0;
            gateGain = target + (gateGain - target) * (target > gateGain ? gateAttack : v[12]);
            const double gate = mix(1, gateGain, v[11]);
            const bool wet = v[5] != 0;
            const bool fuzzOn = wet && v[7] != 1, analogOn = wet && v[7] != 0;
            if (fuzzOn && !fuzzLive) resetFuzz();
            if (analogOn && !analogLive) resetAnalog();
            fuzzLive = fuzzOn; analogLive = analogOn;
            const double input[2] = {a, b};
            double colored[2] = {0, 0}, rounded[2] = {0, 0};
            if (fuzzOn) {
                // Zero Tone Mod: the delay is exactly constant, skip the LFO.
                const double delay = v[4] == 0 ? sr * 0.0007 : sr * (0.0007 + 0.0045 * v[4] * (0.5 + 0.5 * fastSin2pi(phase)));
                double x[2], fuzz[2];
                for (int k = 0; k < 2; ++k) {
                    const double feedback = lowpass(channels[k].feedback.read(delay), channels[k].tone, v[3]);
                    x[k] = input[k] * v[0] + feedback * v[2];
                }
                fuzzShape.tick(x, fuzz, [](double y) ST_HOT { return fastTanh(y); });
                for (int k = 0; k < 2; ++k) {
                    // Bound filter overshoot before storing feedback. Its loop gain stays
                    // below one, including at the maximum Feedback setting.
                    channels[k].feedback.push(clamp(fuzz[k], -1, 1) * gate);
                    colored[k] = lowpass(channels[k].fuzzDC.tick(fuzz[k], dcPole), channels[k].color, v[1]) * gate;
                }
            }
            if (analogOn) {
                double x[2], shaped[2];
                for (int k = 0; k < 2; ++k) {
                    Channel &ch = channels[k];
                    const double low = lowpass(input[k], ch.bass, bassPole);
                    const double mid = lowpass(input[k], ch.focusHigh, focusHighPole) - lowpass(input[k], ch.focusLow, focusLowPole);
                    x[k] = (input[k] - low * (1 - v[8]) + mid * v[9] * (1 - v[10]) * 1.5) * v[0] * v[15];
                }
                const double drive = v[10];
                analogShape.tick(x, shaped, [drive](double y) ST_HOT { return mix(fastTanh(y), clamp(y, -1, 1), drive); });
                for (int k = 0; k < 2; ++k) rounded[k] = lowpass(channels[k].analogDC.tick(shaped[k], dcPole), channels[k].analogTone, v[3]);
            }
            l[i] = float(clean(mix(a, mix(colored[0], rounded[0], v[7]) * v[6], v[5])));
            r[i] = float(clean(mix(b, mix(colored[1], rounded[1], v[7]) * v[6], v[5])));
            phase = wrap(phase + v[14]);
        }
    }
    void reset() noexcept override { for (auto &ch : channels) ch.reset(); fuzzShape.reset(); analogShape.reset(); c.reset(); cache.reset(); phase = envelope = gateGain = 0; fuzzLive = analogLive = false; }
    double tail() const noexcept override { return 3; }
};

class Lofi final : public Processor {
    Controls<9> c; ParameterCache<8> cache;
    double phase = 1, held[2]{}, pre[2][2]{}, post[2][2]{}, clockScale = 1;
    uint32_t random = 0x5a17b3c9u;
    const double smoothing;
    double randomSigned() noexcept {
        random ^= random << 13; random ^= random >> 17; random ^= random << 5;
        return double(random) / 2147483648.0 - 1;
    }
public:
    explicit Lofi(double sampleRate) : Processor(sampleRate), smoothing(pole(8, sr)) {
        for (size_t i = 0; i < 8; ++i) p[i] = lofiParameters[i].defaultValue;
    }
    void process(float *l, float *r, const float *, const float *, uint32_t frames) noexcept override {
        auto value = [&](size_t i) { return parameter(*this, lofiParameters, i); };
        const double rate = std::min(value(0), sr);
        if (cache.changed(*this)) c.setTargets({rate / sr, std::pow(2.0, value(1) - 1), value(2) * 0.01, value(3) * 0.01,
            value(4) * 0.01, dbGain(value(5)), dbGain(value(6)), value(7) * 0.01, filterPole(rate * 0.4, sr)});
        for (uint32_t i = 0; i < frames; ++i) {
            c.tick(smoothing);
            const auto &v = c.value;
            const double dry[] = {clean(l[i]), clean(r[i])};
            double input[2];
            for (size_t channel = 0; channel < 2; ++channel) {
                const double gained = dry[channel] * v[5];
                const double filtered = lowpass(lowpass(gained, pre[channel][0], v[8]), pre[channel][1], v[8]);
                input[channel] = mix(gained, filtered, v[2]);
            }
            if (phase >= 1) {
                phase -= std::floor(phase);
                // A symmetric, zero-centered quantizer preserves digital
                // silence and both full-scale endpoints. Bits controls its
                // step resolution, not a signed PCM container/code count.
                for (size_t channel = 0; channel < 2; ++channel)
                    held[channel] = std::round(clamp(input[channel], -1, 1) * v[1]) / v[1];
                clockScale = 1 + randomSigned() * v[7] * 0.45;
            }
            phase += std::min(1.0, v[0] * clockScale);
            double wet[2];
            for (size_t channel = 0; channel < 2; ++channel) {
                const double filtered = lowpass(lowpass(held[channel], post[channel][0], v[8]), post[channel][1], v[8]);
                wet[channel] = mix(held[channel], filtered, v[2]);
            }
            const double mid = (wet[0] + wet[1]) * 0.5, side = (wet[0] - wet[1]) * 0.5 * v[3];
            l[i] = float(clean(mix(dry[0], (mid + side) * v[6], v[4])));
            r[i] = float(clean(mix(dry[1], (mid - side) * v[6], v[4])));
        }
    }
    void reset() noexcept override {
        c.reset(); cache.reset(); phase = clockScale = 1; random = 0x5a17b3c9u;
        for (size_t channel = 0; channel < 2; ++channel) {
            held[channel] = 0;
            for (size_t stage = 0; stage < 2; ++stage) pre[channel][stage] = post[channel][stage] = 0;
        }
    }
    double tail() const noexcept override { return 0.5; }
};

inline const double tubeBias = fastTanh(0.18);
inline const float tubeBiasf = fastTanhf(0.18f);
// Saturator, block-processed. The shapers and their 2x decimation filters
// run in single precision (same filter design as OversampledShape); the
// low-frequency one-poles and DC blockers stay double. Classic mode has no
// feedback through the shaper, so a whole sub-block is shaped at once
// (vectorized tanh) and then decimated; Tape and Tube feed their shaper
// output back (memory/charge), so they step sample by sample.
class Saturator final : public Processor {
    static constexpr uint32_t kBlock = 32;
    struct Channel {
        DCBlock tapeDC, tubeDC;
        double envelope = 0, bass = 0, treble = 0, tapeBass = 0, tapeTone = 0,
            tapeMemory = 0, tubeBass = 0, charge = 0;
        void reset() noexcept {
            tapeDC.reset(); tubeDC.reset();
            envelope = bass = treble = tapeBass = tapeTone = tapeMemory = tubeBass = charge = 0;
        }
    };
    // Per-mode stereo decimator state.
    struct Shape {
        float previous[2]{}, z1a[2]{}, z2a[2]{}, z1b[2]{}, z2b[2]{};
        void reset() noexcept { for (int k = 0; k < 2; ++k) previous[k] = z1a[k] = z2a[k] = z1b[k] = z2b[k] = 0; }
    };
    Channel channels[2];
    Shape shapes[3];
    float b0, b1, b2, a1, a2, c0, c1, c2, e1, e2;
    Controls<12> c;
    ParameterCache<11> cache;
    std::array<Smoother, 3> weights{};
    const double smoothing, modeSmoothing, attack, release, bassPole, treblePole, memoryPole, chargePole, dcPole;
    std::array<double, 3> modeWeights{};
    int weightMode = -1; bool weightsSettled = false;
    // Only modes with a nonzero crossfade weight (and a nonzero Mix) run;
    // a mode's state restarts when it fades back in.
    std::array<bool, 3> live{};
    // Per-sample controls of the current sub-block (constant while settled).
    double ctl[15][kBlock];
    bool ctlConstant = false;

    // Two cascaded biquads over the interleaved 2x stream (mid, cur) of one
    // sample, both channels; returns the decimated (cur) output.
    ST_HOT void decimate(Shape &st, const float *mid, const float *cur, float *out) const noexcept {
        for (int k = 0; k < 2; ++k) {
            float y = b0 * mid[k] + st.z1a[k]; st.z1a[k] = b1 * mid[k] - a1 * y + st.z2a[k]; st.z2a[k] = b2 * mid[k] - a2 * y;
            float w = c0 * y + st.z1b[k]; st.z1b[k] = c1 * y - e1 * w + st.z2b[k]; st.z2b[k] = c2 * y - e2 * w;
            y = b0 * cur[k] + st.z1a[k]; st.z1a[k] = b1 * cur[k] - a1 * y + st.z2a[k]; st.z2a[k] = b2 * cur[k] - a2 * y;
            w = c0 * y + st.z1b[k]; st.z1b[k] = c1 * y - e1 * w + st.z2b[k]; st.z2b[k] = c2 * y - e2 * w;
            out[k] = w;
        }
    }
    void resetMode(size_t m) noexcept {
        for (auto &ch : channels) {
            if (m == 1) { ch.tapeDC.reset(); ch.tapeBass = ch.tapeTone = ch.tapeMemory = 0; }
            else if (m == 2) { ch.tubeDC.reset(); ch.bass = ch.treble = ch.tubeBass = ch.charge = 0; }
        }
        shapes[m].reset();
    }
public:
    explicit Saturator(double sampleRate) : Processor(sampleRate),
        smoothing(pole(8, sr)), modeSmoothing(pole(5, sr)), attack(pole(2, sr)), release(pole(100, sr)),
        bassPole(filterPole(180, sr)), treblePole(filterPole(3000, sr)), memoryPole(pole(0.6, sr)),
        chargePole(pole(60, sr)), dcPole(filterPole(12, sr)) {
        const OversampledShape mono(sr);
        b0 = float(mono.first.b0); b1 = float(mono.first.b1); b2 = float(mono.first.b2); a1 = float(mono.first.a1); a2 = float(mono.first.a2);
        c0 = float(mono.second.b0); c1 = float(mono.second.b1); c2 = float(mono.second.b2); e1 = float(mono.second.a1); e2 = float(mono.second.a2);
        for (size_t i = 0; i < 11; ++i) p[i] = saturationParameters[i].defaultValue;
    }
    void process(float *l, float *r, const float *, const float *, uint32_t frames) noexcept override {
        auto value = [&](size_t i) { return parameter(*this, saturationParameters, i); };
        const int mode = int(std::round(value(0)));
        if (cache.changed(*this)) c.setTargets({filterPole(1000 * std::pow(20.0, value(2) * 0.01), sr), dbGain(value(1)), value(2) * 0.01,
            value(3) * 0.01, value(4) * 0.01, dbGain(value(5)), value(6) * 0.01, dbGain(value(7)),
            dbGain(value(8)), dbGain(value(9)), std::round(value(10)), dbGain((value(3) * 0.01 - 0.5) * 12)});
        if (mode != weightMode) { weightMode = mode; weightsSettled = false; }
        for (uint32_t offset = 0; offset < frames; offset += kBlock) {
            const uint32_t n = std::min(kBlock, frames - offset);
            float *bl = l + offset, *br = r + offset;
            // Controls: per-sample while smoothing, otherwise constant.
            if (!c.settled || !weightsSettled) {
                ctlConstant = false;
                for (uint32_t i = 0; i < n; ++i) {
                    c.tick(smoothing);
                    bool all = true;
                    for (size_t m = 0; m < 3; ++m) { modeWeights[m] = weights[m].tick(int(m) == mode ? 1 : 0, modeSmoothing); all = all && modeWeights[m] == (int(m) == mode ? 1 : 0); }
                    weightsSettled = all;
                    for (size_t k = 0; k < 12; ++k) ctl[k][i] = c.value[k];
                    for (size_t m = 0; m < 3; ++m) ctl[12 + m][i] = modeWeights[m];
                }
            } else if (!ctlConstant) {
                for (size_t k = 0; k < 12; ++k) std::fill(ctl[k], ctl[k] + kBlock, c.value[k]);
                for (size_t m = 0; m < 3; ++m) std::fill(ctl[12 + m], ctl[12 + m] + kBlock, modeWeights[m]);
                ctlConstant = true;
            }
            const double *v0 = ctl[0], *v1 = ctl[1], *v2 = ctl[2], *v3 = ctl[3], *v4 = ctl[4], *v5 = ctl[5], *v6 = ctl[6], *v7 = ctl[7],
                *v8 = ctl[8], *v9 = ctl[9], *v10 = ctl[10], *v11 = ctl[11];
            bool anyMix = false; for (uint32_t i = 0; i < n; ++i) anyMix = anyMix || v6[i] != 0;
            bool on[3];
            for (size_t m = 0; m < 3; ++m) {
                bool any = false; for (uint32_t i = 0; i < n; ++i) any = any || ctl[12 + m][i] != 0;
                on[m] = any && anyMix;
                if (on[m] && !live[m]) resetMode(m);
                live[m] = on[m];
            }
            double dry[2][kBlock], input[2][kBlock], wet[2][kBlock];
            for (uint32_t i = 0; i < n; ++i) {
                dry[0][i] = clean(bl[i]); dry[1][i] = clean(br[i]);
                for (int k = 0; k < 2; ++k) { input[k][i] = dry[k][i] * v1[i]; wet[k][i] = 0; }
            }
            // Compression envelope (always tracked, like the per-sample form).
            double compression[2][kBlock];
            for (int k = 0; k < 2; ++k) {
                double env = channels[k].envelope;
                for (uint32_t i = 0; i < n; ++i) {
                    const double level = std::abs(input[k][i]);
                    env = level + (env - level) * (level > env ? attack : release);
                    compression[k][i] = 1 / (1 + v2[i] * std::max(0.0, env - 0.25) * 2);
                }
                channels[k].envelope = env;
            }
            if (on[0]) {
                // Shape the whole sub-block (2x: midpoint + sample), then decimate.
                float shaped[2][2 * kBlock];
                Shape &st = shapes[0];
                for (int k = 0; k < 2; ++k) {
                    float previous = st.previous[k];
                    for (uint32_t i = 0; i < n; ++i) {
                        const float x = float(input[k][i] * (1 + 24 * v3[i] * v3[i]) * compression[k][i]);
                        shaped[k][2 * i] = (previous + x) * 0.5f; shaped[k][2 * i + 1] = x; previous = x;
                    }
                    st.previous[k] = previous;
                    for (uint32_t j = 0; j < 2 * n; ++j) shaped[k][j] = fastTanhf(shaped[k][j]);
                }
                for (uint32_t i = 0; i < n; ++i) {
                    const float mid[2] = {shaped[0][2 * i], shaped[1][2 * i]}, cur[2] = {shaped[0][2 * i + 1], shaped[1][2 * i + 1]};
                    float y[2]; decimate(st, mid, cur, y);
                    for (int k = 0; k < 2; ++k) wet[k][i] += clean(double(y[k])) * ctl[12][i];
                }
            }
            if (on[1]) {
                Shape &st = shapes[1];
                for (uint32_t i = 0; i < n; ++i) {
                    float mid[2], cur[2], y[2];
                    for (int k = 0; k < 2; ++k) {
                        Channel &ch = channels[k];
                        const double tapeLow = lowpass(input[k][i], ch.tapeBass, bassPole);
                        // Tape Drive tilts the low-frequency response; Input drives compression.
                        const float x = float(input[k][i] + tapeLow * (v11[i] - 1) + ch.tapeMemory * 0.12);
                        mid[k] = fastTanhf(1.4f * ((st.previous[k] + x) * 0.5f)); cur[k] = fastTanhf(1.4f * x); st.previous[k] = x;
                    }
                    decimate(st, mid, cur, y);
                    for (int k = 0; k < 2; ++k) {
                        Channel &ch = channels[k];
                        const double shaped = clean(double(y[k]));
                        lowpass(clamp(shaped, -1, 1), ch.tapeMemory, memoryPole);
                        wet[k][i] += lowpass(ch.tapeDC.tick(shaped, dcPole), ch.tapeTone, v0[i]) * ctl[13][i];
                    }
                }
            }
            if (on[2]) {
                Shape &st = shapes[2];
                for (uint32_t i = 0; i < n; ++i) {
                    float mid[2], cur[2], y[2];
                    for (int k = 0; k < 2; ++k) {
                        Channel &ch = channels[k];
                        const double in = input[k][i];
                        const double low = lowpass(in, ch.bass, bassPole);
                        const double high = in - lowpass(in, ch.treble, treblePole);
                        const double eq = in + (low * (v8[i] - 1) + high * (v9[i] - 1)) * (1 - v10[i]);
                        const float x = float((eq + low * (v5[i] - 1)) * (1 + 32 * v3[i] * v3[i]) / (1 + 4 * v4[i] * ch.charge));
                        constexpr float bias = 0.18f;
                        mid[k] = fastTanhf((st.previous[k] + x) * 0.5f + bias) - tubeBiasf;
                        cur[k] = fastTanhf(x + bias) - tubeBiasf; // exactly 0 at x = 0
                        st.previous[k] = x;
                    }
                    decimate(st, mid, cur, y);
                    for (int k = 0; k < 2; ++k) {
                        Channel &ch = channels[k];
                        const double tube = ch.tubeDC.tick(clean(double(y[k])), dcPole);
                        const double chargeLevel = std::abs(lowpass(tube, ch.tubeBass, bassPole));
                        lowpass(chargeLevel, ch.charge, chargeLevel > ch.charge ? attack : chargePole);
                        wet[k][i] += tube * ctl[14][i];
                    }
                }
            }
            for (uint32_t i = 0; i < n; ++i) {
                bl[i] = float(clean(mix(dry[0][i], wet[0][i] * v7[i], v6[i])));
                br[i] = float(clean(mix(dry[1][i], wet[1][i] * v7[i], v6[i])));
            }
        }
    }
    void reset() noexcept override {
        for (auto &ch : channels) ch.reset();
        for (auto &st : shapes) st.reset();
        c.reset(); cache.reset(); modeWeights = {}; live = {}; weightMode = -1; weightsSettled = false; ctlConstant = false;
        for (auto &w : weights) w.reset();
    }
    double tail() const noexcept override { return 0.5; }
};

} // namespace distortion_detail

inline const Descriptor *distortionDescriptors(size_t &count) {
    using namespace distortion_detail;
    static const Descriptor descriptors[] = {
        {{"distortion", "Distortion", "Distortion", 16, 0}, distortionParameters,
            [](double sr, uint32_t) -> Processor * { return new Distortion(sr); }},
        {{"lofi", "Lofi", "Distortion", 8, 0}, lofiParameters,
            [](double sr, uint32_t) -> Processor * { return new Lofi(sr); }},
        {{"saturator", "Saturator", "Distortion", 11, 0}, saturationParameters,
            [](double sr, uint32_t) -> Processor * { return new Saturator(sr); }}
    };
    count = sizeof(descriptors) / sizeof(descriptors[0]);
    return descriptors;
}

} // namespace stfx
#endif

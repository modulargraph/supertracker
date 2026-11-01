// Behavioural PCM checks for the Polyend-style sample play modes.
// A ramp sample encodes the read position in its value, so the rendered output
// reveals where (and in which direction) the voice is reading.
#include "TrackerEngine.h"
#include <algorithm>
#include <cassert>
#include <cmath>
#include <cstdio>
#include <string>
#include <vector>

namespace {
constexpr double pi = 3.14159265358979323846;
constexpr int rate = 48000;
constexpr float volume = .5f;

struct Engine {
    STEngine* e = st_engine_create(rate);
    Engine() { assert(e); }
    ~Engine() { st_engine_destroy(e); }
    operator STEngine*() { return e; }
};

STInstrument sampler(int mode) {
    auto i = st_default_instrument();
    i.type = ST_INSTRUMENT_SAMPLE; i.mode = mode;
    i.attack = 0; i.decay = 0; i.sustain = 1; i.release = .005f;
    i.volume = volume; i.cutoff = 20000; i.rootNote = 60;
    return i;
}

std::vector<float> render(const std::vector<float>& pcm, STInstrument instrument, size_t frames, int note = 60, const std::vector<float>& slices = {}) {
    Engine e;
    st_engine_begin_update(e);
    st_engine_set_track_count(e, 1);
    st_engine_set_timing(e, 120, 4, 0, 1);
    auto track = st_default_track(); track.volume = 1; track.pan = 0; st_engine_set_track(e, 0, track);
    st_engine_set_instrument(e, 0, instrument);
    assert(st_engine_set_sample(e, 0, pcm.data(), nullptr, pcm.size(), rate));
    if (!slices.empty()) st_engine_set_slice_points(e, 0, slices.data(), int32_t(slices.size()));
    st_engine_commit_update(e);
    st_engine_note_on(e, 0, 0, note, 127);
    std::vector<float> l(frames), r(frames);
    for (size_t at = 0; at < frames; at += 256) {
        auto count = uint32_t(std::min<size_t>(256, frames - at));
        st_engine_render(e, l.data() + at, r.data() + at, count);
    }
    for (size_t n = 0; n < frames; ++n) { assert(std::isfinite(l[n])); assert(std::abs(l[n] - r[n]) < 1e-6f); }
    return l;
}

std::vector<float> ramp(size_t n) { std::vector<float> v(n); for (size_t i = 0; i < n; ++i) v[i] = float(i) / float(n); return v; }
std::vector<float> sine(size_t n, double period, float amplitude = .5f, double phase = 0) {
    std::vector<float> v(n); for (size_t i = 0; i < n; ++i) v[i] = amplitude * float(std::sin(2 * pi * (double(i) / period + phase))); return v;
}
// Read position (in source frames) recovered from a ramp render.
double position(float out, size_t n) { return double(out) / volume * double(n); }
float maxStep(const std::vector<float>& a, size_t from, size_t to) {
    float worst = 0; for (size_t i = std::max<size_t>(from, 1); i < to; ++i) worst = std::max(worst, std::abs(a[i] - a[i - 1])); return worst;
}
double rms(const std::vector<float>& a, size_t from, size_t to) { double s = 0; for (size_t i = from; i < to; ++i) s += double(a[i]) * a[i]; return std::sqrt(s / double(to - from)); }

void testOneShot() {
    const size_t n = 4800; auto pcm = ramp(n);
    auto out = render(pcm, sampler(ST_MODE_ONESHOT), 8000);
    // Reads forward at one source frame per output frame.
    for (size_t i = 200; i < 4000; i += 100) assert(std::abs(position(out[i], n) - double(i)) < 3);
    for (size_t i = n + 10; i < out.size(); ++i) assert(out[i] == 0);
    // Ends with a short fade instead of dropping from 0.5 to silence.
    float endStep = maxStep(out, 4000, 6000);
    std::printf("one-shot: end step %.4f\n", endStep);
    assert(endStep < .02f);
    // An octave up reads twice as fast.
    auto fast = render(pcm, sampler(ST_MODE_ONESHOT), 3000, 72);
    for (size_t i = 200; i < 2000; i += 100) assert(std::abs(position(fast[i], n) - 2.0 * double(i)) < 4);
    for (size_t i = n / 2 + 10; i < fast.size(); ++i) assert(fast[i] == 0);
}

void testForwardLoop() {
    const size_t n = 4800; auto pcm = ramp(n);
    auto inst = sampler(ST_MODE_FORWARD); inst.loopStart = .5f; inst.loopEnd = .75f;
    auto out = render(pcm, inst, 48000);
    const double lo = n * .5, hi = n * .75, width = hi - lo;
    // First pass covers the pre-loop region, then playback stays inside the loop
    // (the seam crossfade reads at most a few ms before the loop start).
    assert(std::abs(position(out[1000], n) - 1000) < 3);
    double minimum = 1e9, maximum = -1e9;
    for (size_t i = 4000; i < out.size(); ++i) { minimum = std::min(minimum, position(out[i], n)); maximum = std::max(maximum, position(out[i], n)); }
    std::printf("forward: loop window %.1f..%.1f (loop %.0f..%.0f)\n", minimum, maximum, lo, hi);
    assert(minimum > lo - 250 && maximum < hi + 2);
    // Periodic with the loop length.
    for (size_t i = 20000; i < 30000; ++i) assert(std::abs(out[i] - out[i + size_t(width)]) < 1e-5f);
}

void testReverseLoop() {
    const size_t n = 4800; auto pcm = ramp(n);
    auto inst = sampler(ST_MODE_REVERSE); inst.loopStart = .25f; inst.loopEnd = .5f;
    auto out = render(pcm, inst, 48000);
    const double lo = n * .25, hi = n * .5, width = hi - lo;
    // Starts at the sample end and reads backwards.
    assert(position(out[100], n) > n - 110 && position(out[100], n) < n - 90);
    assert(position(out[1000], n) < position(out[900], n));
    double minimum = 1e9, maximum = -1e9;
    for (size_t i = 4000; i < out.size(); ++i) { minimum = std::min(minimum, position(out[i], n)); maximum = std::max(maximum, position(out[i], n)); }
    std::printf("reverse: loop window %.1f..%.1f (loop %.0f..%.0f)\n", minimum, maximum, lo, hi);
    assert(minimum > lo - 2 && maximum < hi + 250);
    for (size_t i = 20000; i < 30000; ++i) assert(std::abs(out[i] - out[i + size_t(width)]) < 1e-5f);
    // Mostly decreasing inside the loop body.
    int falling = 0, rising = 0; for (size_t i = 20001; i < 30000; ++i) { if (out[i] < out[i - 1]) ++falling; else if (out[i] > out[i - 1]) ++rising; }
    assert(falling > rising * 3); // the rest is the seam crossfade on this ramp
}

void testPingPong() {
    const size_t n = 4800; auto pcm = ramp(n);
    auto inst = sampler(ST_MODE_PINGPONG); inst.loopStart = .5f; inst.loopEnd = .75f;
    auto out = render(pcm, inst, 48000);
    const double lo = n * .5, hi = n * .75;
    double minimum = 1e9, maximum = -1e9;
    for (size_t i = 4000; i < out.size(); ++i) { minimum = std::min(minimum, position(out[i], n)); maximum = std::max(maximum, position(out[i], n)); }
    std::printf("ping-pong: window %.1f..%.1f (loop %.0f..%.0f)\n", minimum, maximum, lo, hi);
    assert(minimum > lo - 2 && maximum < hi + 1);
    // Continuous turnarounds: no jumps, both directions occur.
    assert(maxStep(out, 4000, out.size()) < 2.5f / n * volume);
    int falling = 0, rising = 0; for (size_t i = 4001; i < out.size(); ++i) { if (out[i] < out[i - 1]) ++falling; else if (out[i] > out[i - 1]) ++rising; }
    assert(falling > 10000 && rising > 10000);
}

void testLoopSeamIsSmooth() {
    // Loop points deliberately away from zero crossings of a 100-frame sine.
    const size_t n = 9600; auto pcm = sine(n, 100);
    auto inst = sampler(ST_MODE_FORWARD); inst.loopStart = .3013f; inst.loopEnd = .7771f;
    auto out = render(pcm, inst, 48000);
    const float natural = float(2 * pi / 100 * .5 * volume);
    float step = maxStep(out, 100, out.size());
    std::printf("forward seam: max step %.4f (natural %.4f)\n", step, natural);
    assert(step < natural * 1.6f);
    // Reverse loop seam too.
    inst.mode = ST_MODE_REVERSE; out = render(pcm, inst, 48000);
    step = maxStep(out, 100, out.size());
    std::printf("reverse seam: max step %.4f\n", step);
    assert(step < natural * 1.6f);
    // Loop that ends before the sample end and starts at the sample start uses the post-loop audio.
    inst.mode = ST_MODE_FORWARD; inst.loopStart = 0; inst.loopEnd = .6137f; out = render(pcm, inst, 48000);
    step = maxStep(out, 100, out.size());
    std::printf("forward seam (post-loop crossfade): max step %.4f\n", step);
    assert(step < natural * 1.6f);
}

void testInterpolation() {
    // A 32-frame sine played an octave down must match a 64-frame sine at root.
    const size_t n = 32 * 400;
    auto low = render(sine(n, 32), sampler(ST_MODE_ONESHOT), 12000, 48);
    auto reference = render(sine(n * 2, 64), sampler(ST_MODE_ONESHOT), 12000, 60);
    std::vector<float> diff(12000); for (size_t i = 0; i < diff.size(); ++i) diff[i] = low[i] - reference[i];
    double error = rms(diff, 1000, 11000) / rms(reference, 1000, 11000);
    std::printf("interpolation: relative RMS error %.6f at half speed\n", error);
    assert(error < 2e-4);
}

void testSlices() {
    const size_t n = 4800; auto pcm = ramp(n);
    auto inst = sampler(ST_MODE_SLICE); inst.sliceCount = 4;
    for (int slice = 0; slice < 4; ++slice) {
        auto out = render(pcm, inst, 3000, 60 + slice);
        assert(std::abs(position(out[100], n) - (slice * 1200 + 100)) < 3);
        for (size_t i = 1210; i < out.size(); ++i) assert(out[i] == 0);
        assert(maxStep(out, 2, out.size()) < .02f);
    }
    // Manual markers.
    auto out = render(pcm, inst, 3000, 61, {0, .1f, .6f});
    assert(std::abs(position(out[100], n) - (480 + 100)) < 3);
    for (size_t i = 2410; i < out.size(); ++i) assert(out[i] == 0);
}

size_t risingCrossings(const std::vector<float>& a, size_t from, size_t to) { size_t c = 0; for (size_t i = from + 1; i < to; ++i) if (a[i - 1] <= 0 && a[i] > 0) ++c; return c; }

void testWavetable() {
    // Four 2048-frame single cycles with increasing amplitude.
    std::vector<float> pcm(2048 * 4);
    for (int frame = 0; frame < 4; ++frame) for (int i = 0; i < 2048; ++i) pcm[frame * 2048 + i] = .2f * float(frame + 1) * float(std::sin(2 * pi * i / 2048.0));
    auto inst = sampler(ST_MODE_WAVETABLE);
    double level[5];
    for (int k = 0; k <= 4; ++k) {
        inst.wavetablePosition = k == 4 ? 1.f : float(k) / 4 + .0625f * (k == 1);
        auto out = render(pcm, inst, 48000, 69);
        size_t crossings = risingCrossings(out, 0, 48000);
        level[k] = rms(out, 1000, 47000) / volume * std::sqrt(2.0);
        // Every position is a clean 440 Hz sine: fit sin/cos and measure the residual.
        double s = 0, c = 0, total = 0;
        for (size_t i = 1000; i < 47000; ++i) { double w = 2 * pi * 440 * double(i) / rate; s += out[i] * std::sin(w); c += out[i] * std::cos(w); total += double(out[i]) * out[i]; }
        double fitted = 2 * (s * s + c * c) / double(46000), residual = std::max(0.0, total - fitted) / total;
        std::printf("wavetable: position %.2f -> %zu Hz, amplitude %.3f, non-sine residual %.5f\n", inst.wavetablePosition, crossings, level[k], residual);
        assert(crossings >= 439 && crossings <= 441);
        assert(residual < .002);
    }
    assert(std::abs(level[0] - .2) < .02 && std::abs(level[4] - .8) < .02);
    for (int k = 1; k <= 4; ++k) assert(level[k] > level[k - 1] + .05);
    // Halfway between frames 1 and 2 blends them (no phase-misaligned window).
    assert(std::abs(level[2] - .5) < .02);
}

void testGranular() {
    const size_t n = 48000; auto pcm = ramp(n);
    auto inst = sampler(ST_MODE_GRANULAR);
    auto spread = [&](float where, float size, double& mean, double& width) {
        inst.grainPosition = where; inst.grainSize = size;
        auto out = render(pcm, inst, 24000);
        double sum = 0, lo = 1e9, hi = -1e9; size_t count = 0;
        for (size_t i = 4000; i < out.size(); ++i) { double p = position(out[i], n); sum += p; ++count; lo = std::min(lo, p); hi = std::max(hi, p); }
        mean = sum / double(count); width = hi - lo;
    };
    double m1, w1, m2, w2, m3, w3;
    spread(.25f, .05f, m1, w1); spread(.75f, .05f, m2, w2); spread(.75f, .2f, m3, w3);
    std::printf("granular: mean %.0f/%.0f (expect %.0f/%.0f), span %.0f -> %.0f with larger grains\n", m1, m2, n * .25, n * .75, w2, w3);
    assert(std::abs(m1 - n * .25) < n * .02 && std::abs(m2 - n * .75) < n * .02);
    assert(w2 < n * .06 && w3 > w2 * 2.5);
}
}

int main(int argc, char** argv) {
    struct Case { const char* name; void (*run)(); } cases[] = {
        {"oneshot", testOneShot}, {"forward", testForwardLoop}, {"reverse", testReverseLoop}, {"pingpong", testPingPong},
        {"seam", testLoopSeamIsSmooth}, {"interpolation", testInterpolation}, {"slices", testSlices},
        {"wavetable", testWavetable}, {"granular", testGranular}};
    for (const auto& c : cases) if (argc < 2 || std::string(argv[1]) == c.name) c.run();
    std::puts("sampler mode tests passed");
}

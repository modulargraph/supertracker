// Sampler Wavetable mode: position parameter, automation, step FX, frame size and smoothing.
#include "TrackerEngine.h"
#include <cassert>
#include <cmath>
#include <cstdio>
#include <vector>

static const double pi = 3.14159265358979323846;
struct Engine { STEngine *e = st_engine_create(48000); Engine() { assert(e); } ~Engine() { st_engine_destroy(e); } operator STEngine *() { return e; } };

/// `frames` cycles of `width` samples; frame k is a sine of amplitude .2*(k+1).
static std::vector<float> table(int frames, int width) {
    std::vector<float> pcm(size_t(frames * width));
    for (int k = 0; k < frames; ++k)
        for (int i = 0; i < width; ++i) pcm[size_t(k * width + i)] = .2f * float(k + 1) * float(std::sin(2 * pi * i / width));
    return pcm;
}
static STInstrument wavetable(float position = 0) {
    auto p = st_default_instrument();
    p.mode = ST_MODE_WAVETABLE; p.attack = 0; p.decay = 0; p.sustain = 1; p.release = .01f; p.volume = 1; p.cutoff = 22000;
    p.wavetablePosition = position;
    return p;
}
static void setup(STEngine *e, const STInstrument &p, const std::vector<float> &pcm, int rows = 4) {
    st_engine_begin_update(e);
    st_engine_set_track_count(e, 1);
    st_engine_set_timing(e, 120, 4, 0, 1);
    st_engine_set_host_mixing(e, 1, 1);
    auto track = st_default_track(); track.volume = 1; track.delaySend = track.reverbSend = 0;
    st_engine_set_track(e, 0, track);
    st_engine_set_instrument(e, 0, p);
    assert(st_engine_set_sample(e, 0, pcm.data(), nullptr, pcm.size(), 48000));
    st_engine_set_pattern(e, 0, rows);
    st_engine_commit_update(e);
}
static std::vector<float> render(STEngine *e, size_t frames) {
    std::vector<float> l(frames), r(frames);
    for (size_t at = 0; at < frames; at += 256) st_engine_render(e, l.data() + at, r.data() + at, uint32_t(std::min<size_t>(256, frames - at)));
    return l;
}
/// Peak amplitude over a window (440 Hz, so any 120-sample window holds a full cycle).
static double peak(const std::vector<float> &x, size_t from, size_t to) { double p = 0; for (size_t i = from; i < to; ++i) p = std::max(p, double(std::fabs(x[i]))); return p; }
static double level(const std::vector<float> &x, size_t from, size_t to) { return peak(x, from, to) / std::sqrt(.5); }

int main() {
    int groups = 0;
    // The Position parameter selects frames; between frames it crossfades.
    for (float position : {0.f, 1.f / 3, .5f, 1.f}) {
        Engine e; setup(e, wavetable(position), table(4, 2048));
        st_engine_note_on(e, 0, 0, 69, 127);
        auto out = render(e, 9600);
        double expected = .2 * (1 + 3 * position), got = peak(out, 4800, 9600);
        std::printf("position %.3f -> peak %.3f (expected %.3f)\n", position, got, expected);
        assert(std::fabs(got - expected) < .01);
    }
    std::puts("PASS position selects and crossfades frames"); ++groups;

    // The instrument's frame size overrides the default 2048; the sample's registered cycle is used when it is 0.
    {
        auto pcm = table(8, 256);
        // Half volume keeps the loudest frame (1.6) clear of the output's limits.
        Engine explicitSize; auto p = wavetable(1); p.volume = .5f; p.wavetableFrameSize = 256; setup(explicitSize, p, pcm);
        st_engine_note_on(explicitSize, 0, 0, 69, 127);
        assert(std::fabs(peak(render(explicitSize, 9600), 4800, 9600) - .8) < .01);
        auto half = wavetable(1); half.volume = .5f;
        Engine registered; setup(registered, half, pcm);
        assert(st_engine_set_sample_cycle(registered, 0, 256));
        st_engine_note_on(registered, 0, 0, 69, 127);
        assert(std::fabs(peak(render(registered, 9600), 4800, 9600) - .8) < .01);
        assert(!st_engine_set_sample_cycle(registered, 0, 8) && !st_engine_set_sample_cycle(registered, 0, 9000) && !st_engine_set_sample_cycle(registered, -1, 256));
        // Replacing the sample clears the registered size: the default 2048-sample read then spans all eight cycles.
        auto again = pcm;
        assert(st_engine_set_sample(registered, 0, again.data(), nullptr, again.size(), 48000));
        st_engine_panic(registered); render(registered, 256);
        st_engine_note_on(registered, 0, 0, 69, 127);
        auto defaultRead = render(registered, 9600);
        int crossings = 0;
        for (size_t i = 4801; i < 9600; ++i) if (defaultRead[i - 1] <= 0 && defaultRead[i] > 0) ++crossings;
        assert(crossings > 300); // eight cycles per 2048-sample read instead of one at 440 Hz
    }
    std::puts("PASS explicit and registered frame sizes"); ++groups;

    // Automation lanes move the position for sounding voices.
    {
        Engine e; setup(e, wavetable(0), table(4, 2048), 16);
        st_engine_begin_update(e);
        st_engine_add_automation_point(e, 0, 0, ST_TARGET_WAVETABLE_POSITION, 0, 0, 1, 0, 0);
        st_engine_add_automation_point(e, 0, 0, ST_TARGET_WAVETABLE_POSITION, 0, 0, 1, 8, 1);
        auto step = st_default_step(); step.note = 69; step.velocity = 127; step.gate = 16;
        st_engine_set_step(e, 0, 0, 0, step, nullptr, 0);
        st_engine_commit_update(e);
        st_engine_play(e, 0);
        auto out = render(e, 60000);
        assert(level(out, 1000, 2000) < .3 && level(out, 50000, 52000) > .75);
    }
    std::puts("PASS position automation"); ++groups;

    // Step FX "position" sets the track's position for its voices (value 0..1).
    {
        Engine e; setup(e, wavetable(0), table(4, 2048), 4);
        st_engine_begin_update(e);
        auto step = st_default_step(); step.note = 69; step.velocity = 127; step.gate = 4;
        st_engine_set_step(e, 0, 0, 0, step, nullptr, 0);
        auto fx = st_default_step(); fx.note = -3; fx.fx1 = {ST_FX_POSITION, 1};
        st_engine_set_step(e, 0, 0, 2, fx, nullptr, 0);
        st_engine_commit_update(e);
        st_engine_play(e, 0);
        auto out = render(e, 24000);
        // Rows are 6000 frames at 120 BPM / 4 LPB.
        assert(std::fabs(peak(out, 1000, 6000) - .2) < .02 && std::fabs(peak(out, 13000, 18000) - .8) < .02);
    }
    std::puts("PASS step FX position"); ++groups;

    // Position jumps slew over 5 ms instead of clicking: no sample-to-sample step beyond the waveform's own slope.
    {
        Engine e; setup(e, wavetable(0), table(2, 2048), 4);
        st_engine_note_on(e, 0, 0, 33, 127); // 55 Hz: the waveform itself moves slowly
        render(e, 4800);
        st_engine_begin_update(e); st_engine_set_instrument(e, 0, wavetable(1)); st_engine_commit_update(e);
        auto out = render(e, 4800);
        double largest = 0;
        for (size_t i = 1; i < out.size(); ++i) largest = std::max(largest, double(std::fabs(out[i] - out[i - 1])));
        double slope = .4 * 2 * pi * 55 / 48000; // steepest step of the louder frame
        assert(largest < slope * 1.5 + .2 / 240);
        assert(std::fabs(peak(out, 2400, 4800) - .4) < .01);
    }
    std::puts("PASS position changes are smoothed"); ++groups;

    // Invalid values are clamped or ignored.
    {
        Engine e; auto p = wavetable(7); p.wavetableFrameSize = 3; setup(e, p, table(4, 2048));
        st_engine_note_on(e, 0, 0, 69, 127);
        auto out = render(e, 9600);
        assert(std::fabs(peak(out, 4800, 9600) - .8) < .01);
    }
    std::puts("PASS position clamps to 1 and an invalid frame size reads as automatic"); ++groups;
    std::printf("Wavetable: %d groups passed\n", groups);
}

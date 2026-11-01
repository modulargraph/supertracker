// Sampler Granular parameters, live instrument parameters and the voice playhead snapshot.
#include "TrackerEngine.h"
#include <atomic>
#include <cassert>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <new>
#include <set>
#include <vector>

static std::atomic<bool> realtime{false};
static std::atomic<unsigned> allocations{0};
void *operator new(std::size_t n) { if (realtime) ++allocations; if (void *p = std::malloc(n ? n : 1)) return p; throw std::bad_alloc(); }
void *operator new[](std::size_t n) { return ::operator new(n); }
void operator delete(void *p) noexcept { std::free(p); }
void operator delete[](void *p) noexcept { std::free(p); }
void operator delete(void *p, std::size_t) noexcept { std::free(p); }
void operator delete[](void *p, std::size_t) noexcept { std::free(p); }

static const double pi = 3.14159265358979323846;
static const size_t N = 48000;
struct Engine { STEngine *e = st_engine_create(48000); Engine() { assert(e); } ~Engine() { st_engine_destroy(e); } operator STEngine *() { return e; } };

static STInstrument sampler(int mode) {
    auto p = st_default_instrument();
    p.mode = mode; p.attack = 0; p.decay = 0; p.sustain = 1; p.release = .005f; p.volume = 1; p.cutoff = 22000;
    return p;
}
static std::vector<float> sine(size_t frames = N, double hz = 441) {
    std::vector<float> pcm(frames);
    for (size_t i = 0; i < frames; ++i) pcm[i] = float(.5 * std::sin(2 * pi * hz * double(i) / 48000));
    return pcm;
}
static std::vector<float> noise(size_t frames = N) {
    std::vector<float> pcm(frames); uint32_t seed = 7;
    for (auto &x : pcm) { seed = seed * 1664525 + 1013904223; x = float(int32_t(seed) / 2147483648.0 * .5); }
    return pcm;
}
static void setup(STEngine *e, const STInstrument &p, const std::vector<float> &pcm, int rows = 16) {
    st_engine_begin_update(e);
    st_engine_set_track_count(e, 1);
    st_engine_set_timing(e, 120, 4, 0, 1);
    st_engine_set_host_mixing(e, 1, 1);
    auto track = st_default_track(); track.volume = 1; track.delaySend = track.reverbSend = 0;
    st_engine_set_track(e, 0, track);
    st_engine_set_instrument(e, 0, p);
    assert(st_engine_set_sample(e, 0, pcm.data(), nullptr, pcm.size(), 48000));
    st_engine_set_pattern(e, 0, rows);
}
static void publish(STEngine *e) { st_engine_commit_update(e); }
struct Out { std::vector<float> l, r; };
static void renderInto(STEngine *e, Out &o, size_t block = 256) {
    const size_t frames = o.l.size();
    for (size_t at = 0; at < frames; at += block) st_engine_render(e, o.l.data() + at, o.r.data() + at, uint32_t(std::min(block, frames - at)));
}
static Out render(STEngine *e, size_t frames, size_t block = 256) {
    Out o{std::vector<float>(frames), std::vector<float>(frames)};
    renderInto(e, o, block);
    return o;
}
static double rms(const std::vector<float> &x, size_t from, size_t to) { double s = 0; for (size_t i = from; i < to; ++i) s += double(x[i]) * x[i]; return std::sqrt(s / double(to - from)); }
static std::vector<STVoicePosition> positions(STEngine *e) {
    std::vector<STVoicePosition> v(ST_VOICE_POSITION_CAPACITY); uint64_t serial = 0;
    v.resize(size_t(st_engine_voice_positions(e, v.data(), int(v.size()), &serial))); return v;
}

static void granularParameters() {
    // grainDensity 0 (projects saved before it existed) reads as the historical 2; out-of-range values are clamped.
    {
        Engine a, b; auto p = sampler(ST_MODE_GRANULAR); p.grainDensity = 0; setup(a, p, sine()); publish(a);
        p.grainDensity = 2; setup(b, p, sine()); publish(b);
        st_engine_note_on(a, 0, 0, 60, 100); st_engine_note_on(b, 0, 0, 60, 100);
        auto x = render(a, 9600), y = render(b, 9600);
        for (size_t i = 0; i < x.l.size(); ++i) assert(x.l[i] == y.l[i]);
    }
    // Density: more overlapping grains keep the level (normalized) and stay smooth.
    double levels[3]; int index = 0;
    for (int density : {1, 2, 6}) {
        Engine e; auto p = sampler(ST_MODE_GRANULAR); p.grainDensity = density; p.grainSize = .05f; setup(e, p, noise()); publish(e);
        st_engine_watch_voice_positions(e, 0);
        st_engine_note_on(e, 0, 0, 60, 100); auto o = render(e, 24000);
        levels[index++] = rms(o.l, 4800, 24000);
        auto snap = positions(e); assert(snap.size() == 1 && snap[0].grainCount == density);
        std::printf("density %d -> rms %.3f\n", density, levels[index - 1]);
    }
    assert(levels[1] > .1 && std::fabs(levels[2] / levels[1] - 1) < .3);
    std::puts("PASS density (0 = historical 2, overlap normalized)");

    // Scan 1x moves the read position through the region at the original speed; 0 freezes it; -1 runs backwards.
    for (float scan : {0.f, 1.f, -1.f}) {
        Engine e; auto p = sampler(ST_MODE_GRANULAR); p.grainScan = scan; p.grainPosition = .5f; setup(e, p, sine(N)); publish(e);
        st_engine_watch_voice_positions(e, -1);
        st_engine_note_on(e, 0, 0, 72, 100); render(e, 256);
        double first = positions(e)[0].position; render(e, 9600); auto snap = positions(e);
        double moved = snap[0].position - first;
        std::printf("scan %.0f -> moved %.0f frames, direction %d\n", scan, moved, snap[0].direction);
        assert(std::fabs(moved - scan * 9600) < 2); assert(snap[0].direction == (scan > 0 ? 1 : scan < 0 ? -1 : 0));
    }
    std::puts("PASS scan (pitch independent: the note is an octave up, the scan is still 1x)");

    // Spray spreads grain positions around the scan point; without it every grain reads near the centre.
    for (float spray : {0.f, .1f}) {
        Engine e; auto p = sampler(ST_MODE_GRANULAR); p.grainSpray = spray; p.grainDensity = 8; p.grainSize = .02f; setup(e, p, sine()); publish(e);
        st_engine_watch_voice_positions(e, 0); st_engine_note_on(e, 0, 0, 60, 100);
        double spread = 0;
        for (int block = 0; block < 60; ++block) { render(e, 256); for (const auto &v : positions(e)) for (int k = 0; k < v.grainCount; ++k) spread = std::max(spread, std::fabs(v.grainPositions[k] - v.position)); }
        std::printf("spray %.2f s -> max grain offset %.0f frames\n", spray, spread);
        if (spray == 0) assert(spread <= .02 * 48000 * .5 + 2); else assert(spread > 2000 && spread <= .02 * 48000 * .5 + .1 * 48000 + 2);
    }
    std::puts("PASS spray");

    // Reverse 1: every grain reads backwards (its read position falls while its window advances).
    {
        Engine e; auto p = sampler(ST_MODE_GRANULAR); p.grainReverse = 1; p.grainDensity = 1; p.grainSize = .2f; setup(e, p, sine()); publish(e);
        st_engine_watch_voice_positions(e, 0); st_engine_note_on(e, 0, 0, 60, 100); render(e, 512);
        double a = positions(e)[0].grainPositions[0]; render(e, 256); double b = positions(e)[0].grainPositions[0];
        assert(b < a && std::fabs((a - b) - 256) < 2);
        p.grainReverse = 0; Engine f; setup(f, p, sine()); publish(f); st_engine_watch_voice_positions(f, 0); st_engine_note_on(f, 0, 0, 60, 100); render(f, 512);
        a = positions(f)[0].grainPositions[0]; render(f, 256); b = positions(f)[0].grainPositions[0]; assert(b > a);
    }
    std::puts("PASS reverse probability");

    // Pitch spread changes grain pitch (output differs, stays finite); stereo spread makes a mono sample stereo.
    {
        Engine a, b; auto p = sampler(ST_MODE_GRANULAR); p.grainDensity = 4; setup(a, p, sine()); publish(a);
        p.grainPitchSpread = 12; p.grainStereo = 1; setup(b, p, sine()); publish(b);
        st_engine_note_on(a, 0, 0, 60, 100); st_engine_note_on(b, 0, 0, 60, 100);
        auto x = render(a, 24000), y = render(b, 24000); double diff = 0, side = 0, sideA = 0;
        for (size_t i = 0; i < x.l.size(); ++i) { assert(std::isfinite(y.l[i]) && std::isfinite(y.r[i])); diff += std::fabs(x.l[i] - y.l[i]); side += std::fabs(y.l[i] - y.r[i]); sideA += std::fabs(x.l[i] - x.r[i]); }
        assert(diff > 100 && side > 100 && sideA < 1e-3);
    }
    std::puts("PASS pitch spread and stereo spread");

    // Automation targets, the LFO grain-size destination and POS step effect reach granular voices.
    {
        Engine e; auto p = sampler(ST_MODE_GRANULAR); setup(e, p, sine(), 4);
        STStep step = st_default_step(); step.note = 60; step.instrument = 0; step.velocity = 100; step.gate = 4; step.fx1 = {ST_FX_POSITION, .25f};
        st_engine_set_step(e, 0, 0, 0, step, nullptr, 0);
        st_engine_add_automation_point(e, 0, 0, ST_TARGET_GRAIN_SCAN, -1, 0, 0, 0, 2);
        st_engine_add_automation_point(e, 0, 0, ST_TARGET_GRAIN_DENSITY, -1, 0, 0, 0, 5);
        publish(e); st_engine_watch_voice_positions(e, 0); st_engine_play(e, 0); render(e, 512);
        auto a = positions(e); assert(a.size() == 1 && a[0].grainCount == 5 && a[0].direction == 1);
        double start = a[0].position; assert(std::fabs(start - (.25 * (N - 1) + 2 * 512)) < 600);
        render(e, 2400); auto b = positions(e); assert(std::fabs((b[0].position - start) - 2 * 2400) < 4);
    }
    {
        Engine a, b; auto p = sampler(ST_MODE_GRANULAR); p.grainSize = .02f; setup(a, p, sine()); publish(a);
        p.lfoTarget = ST_MOD_GRAIN_SIZE; p.lfoDepth = 1; p.lfoRate = 3; setup(b, p, sine()); publish(b);
        st_engine_watch_voice_positions(a, 0); st_engine_watch_voice_positions(b, 0);
        st_engine_note_on(a, 0, 0, 60, 100); st_engine_note_on(b, 0, 0, 60, 100);
        std::set<long> spans; for (int i = 0; i < 40; ++i) { render(a, 256); render(b, 256); spans.insert(std::lround(positions(b)[0].windowEnd - positions(b)[0].position)); assert(std::lround(positions(a)[0].windowEnd - positions(a)[0].position) == 480); }
        assert(spans.size() > 10);
    }
    std::puts("PASS grain automation targets, LFO grain size and POS step effect");
}

static void livePositions() {
    // Forward loop: the playhead advances (+1) and wraps inside the loop.
    {
        Engine e; auto p = sampler(ST_MODE_FORWARD); p.loopStart = .5f; p.loopEnd = .6f; setup(e, p, sine()); publish(e);
        assert(positions(e).empty());
        st_engine_note_on(e, 0, 0, 60, 100); render(e, 1024); assert(positions(e).empty()); // not watched yet
        st_engine_watch_voice_positions(e, 1); render(e, 256); assert(positions(e).empty()); // another instrument
        st_engine_watch_voice_positions(e, 0); render(e, 256);
        auto a = positions(e); assert(a.size() == 1 && a[0].direction == 1 && a[0].mode == ST_MODE_FORWARD && a[0].sampleFrames == N);
        assert(std::fabs(a[0].position - 1536) < 2 && a[0].instrument == 0 && a[0].note == 60 && a[0].level > .99f);
        render(e, 48000 - 1536 + 2400); auto b = positions(e); assert(b[0].position >= .5 * N && b[0].position < .6 * N);
        st_engine_watch_voice_positions(e, -2); render(e, 256); assert(positions(e).empty());
    }
    // Backward loop: starts at the end, runs down (-1).
    {
        Engine e; auto p = sampler(ST_MODE_REVERSE); setup(e, p, sine()); publish(e); st_engine_watch_voice_positions(e, 0);
        st_engine_note_on(e, 0, 0, 60, 100); render(e, 256); auto a = positions(e); render(e, 256); auto b = positions(e);
        assert(a[0].direction == -1 && b[0].position < a[0].position && std::fabs(a[0].position - b[0].position - 256) < 2 && a[0].position > N - 300);
    }
    // Ping-pong bounces: both directions appear and the head stays inside the loop.
    {
        Engine e; auto p = sampler(ST_MODE_PINGPONG); p.loopStart = .1f; p.loopEnd = .2f; setup(e, p, sine()); publish(e); st_engine_watch_voice_positions(e, 0);
        st_engine_note_on(e, 0, 0, 84, 100); std::set<int> dirs; render(e, 6000);
        for (int i = 0; i < 100; ++i) { render(e, 256); auto v = positions(e); dirs.insert(v[0].direction); assert(v[0].position >= .1 * N - 1 && v[0].position <= .2 * N); }
        assert(dirs.count(1) && dirs.count(-1));
    }
    // Slice: the slice the note picks.
    {
        Engine e; auto p = sampler(ST_MODE_SLICE); p.sliceCount = 4; setup(e, p, sine()); publish(e); st_engine_watch_voice_positions(e, 0);
        st_engine_note_on(e, 0, 0, 62, 100); render(e, 512); auto v = positions(e);
        assert(v.size() == 1 && v[0].position >= .5 * N && v[0].position < .75 * N && v[0].direction == 1);
    }
    // Wavetable: the current frame region.
    {
        Engine e; auto p = sampler(ST_MODE_WAVETABLE); p.wavetablePosition = 1; p.wavetableFrameSize = 2048; setup(e, p, sine(8 * 2048)); publish(e); st_engine_watch_voice_positions(e, 0);
        st_engine_note_on(e, 0, 0, 60, 100); render(e, 2048); auto v = positions(e);
        assert(v.size() == 1 && v[0].windowStart == 7 * 2048 && v[0].windowEnd == 8 * 2048);
    }
    // Granular: grains around the scan centre; a released voice is flagged; polyphony reports every voice.
    {
        Engine e; auto p = sampler(ST_MODE_GRANULAR); p.grainDensity = 3; p.release = .5f; setup(e, p, sine()); publish(e); st_engine_watch_voice_positions(e, -1);
        st_engine_note_on(e, 0, 0, 60, 100); st_engine_note_on(e, 0, 0, 67, 100); render(e, 1024); auto v = positions(e);
        assert(v.size() == 2 && v[0].grainCount == 3 && v[0].windowStart < v[0].position && v[0].windowEnd > v[0].position);
        for (int k = 0; k < 3; ++k) assert(v[0].grainPositions[k] >= v[0].windowStart - 1 && v[0].grainPositions[k] <= v[0].windowEnd + 1 && v[0].grainLevels[k] >= 0 && v[0].grainLevels[k] <= 1);
        st_engine_note_off(e, 0, -1); render(e, 256); v = positions(e); assert(v.size() == 2 && (v[0].flags & ST_VOICE_POSITION_RELEASING));
    }
    std::puts("PASS voice positions: forward, backward, ping-pong, slice, wavetable, granular, watch filter");
}

static double highBand(const std::vector<float> &x, size_t from, size_t to) { double s = 0; for (size_t i = from + 1; i < to; ++i) { double d = double(x[i]) - x[i - 1]; s += d * d; } return std::sqrt(s / double(to - from)); }

static void liveParameters() {
    // A held note follows volume and cutoff edits within a couple of blocks, without clicks and without a publish.
    std::vector<float> noise(N); uint32_t seed = 1; for (auto &x : noise) { seed = seed * 1664525 + 1013904223; x = float(int32_t(seed) / 2147483648.0 * .5); }
    Engine e; auto p = sampler(ST_MODE_FORWARD); setup(e, p, noise); publish(e);
    st_engine_note_on(e, 0, 0, 60, 100); auto before = render(e, 4800);
    double level = rms(before.l, 2400, 4800), bright = highBand(before.l, 2400, 4800);
    assert(st_engine_set_instrument_parameter(e, 0, ST_TARGET_INSTRUMENT_VOLUME, .25f));
    assert(st_engine_set_instrument_parameter(e, 0, ST_TARGET_CUTOFF, 400));
    assert(!st_engine_set_instrument_parameter(e, 0, ST_TARGET_VOLUME, 1)); // track volume is not an instrument parameter
    assert(!st_engine_set_instrument_parameter(e, 0, 99, 1));
    Out after{std::vector<float>(9600), std::vector<float>(9600)};
    realtime = true; allocations = 0;
    renderInto(e, after);
    realtime = false; assert(allocations == 0);
    double level2 = rms(after.l, 4800, 9600), bright2 = highBand(after.l, 4800, 9600);
    std::printf("live: level %.3f -> %.3f, high band %.3f -> %.3f\n", level, level2, bright, bright2);
    assert(level2 < level * .45 && bright2 < bright * .2);
    // The first blocks already move (smoothing, not a step) and nothing jumps beyond the source's own steps.
    assert(rms(after.l, 256, 512) < level * .98);
    // Notes started before the next publication use the live value; a publication replaces it.
    st_engine_note_off(e, 0, -1); render(e, 4800);
    st_engine_note_on(e, 0, 0, 60, 100); auto next = render(e, 4800); assert(rms(next.l, 2400, 4800) < level * .45);
    st_engine_note_off(e, 0, -1); render(e, 4800);
    setup(e, p, noise); publish(e); render(e, 256);
    st_engine_note_on(e, 0, 0, 60, 100); auto republished = render(e, 4800); assert(rms(republished.l, 2400, 4800) > level * .8);
    // Granular parameters are live too.
    Engine g; auto q = sampler(ST_MODE_GRANULAR); setup(g, q, sine()); publish(g); st_engine_watch_voice_positions(g, 0);
    st_engine_note_on(g, 0, 0, 60, 100); render(g, 512); assert(positions(g)[0].grainCount == 2);
    st_engine_set_instrument_parameter(g, 0, ST_TARGET_GRAIN_DENSITY, 7); st_engine_set_instrument_parameter(g, 0, ST_TARGET_GRAIN_POSITION, .9f);
    render(g, 512); auto v = positions(g); assert(v[0].grainCount == 7 && std::fabs(v[0].position - .9 * (N - 1)) < 2);
    std::puts("PASS live instrument parameters (held notes, next notes, cleared by publication, no allocation)");
}

static void noAllocations() {
    Engine e; auto p = sampler(ST_MODE_GRANULAR); p.grainDensity = 8; p.grainSpray = .2f; p.grainPitchSpread = 7; p.grainScan = .5f; p.grainReverse = .5f; p.grainStereo = 1;
    setup(e, p, sine()); publish(e); st_engine_watch_voice_positions(e, -1);
    for (int n = 0; n < 16; ++n) st_engine_note_on(e, 0, 0, 48 + n, 100);
    render(e, 256);
    Out out{std::vector<float>(48000), std::vector<float>(48000)}; std::vector<STVoicePosition> snap(ST_VOICE_POSITION_CAPACITY);
    realtime = true; allocations = 0; renderInto(e, out, 128); int count = st_engine_voice_positions(e, snap.data(), int(snap.size()), nullptr); realtime = false;
    assert(allocations == 0); assert(count == 16);
    std::puts("PASS granular rendering and snapshots allocate nothing");
}

int main() {
    granularParameters();
    livePositions();
    liveParameters();
    noAllocations();
    std::puts("engine granular tests passed");
}

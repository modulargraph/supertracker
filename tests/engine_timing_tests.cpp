#include "TrackerEngine.h"
#include "engine_test_tone.hpp"
#include "TrackerClock.h"
#include <algorithm>
#include <array>
#include <atomic>
#include <cassert>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <new>
#include <tuple>
#include <thread>
#include <vector>

// Standalone core regression suite: no plug-in, host graph or audio device.
// At 48 kHz our two-tempo fixture maps beat b to 24000*b through b=.375,
// then to 48000*b-9000. All explicit expectations below are exact integers.
static thread_local bool realtime = false;
static std::atomic<unsigned> realtimeAllocations{0};
void* operator new(std::size_t size) {
    if (realtime) ++realtimeAllocations;
    if (void* result = std::malloc(size ? size : 1)) return result;
    throw std::bad_alloc();
}
void* operator new[](std::size_t size) { return ::operator new(size); }
void operator delete(void* p) noexcept { std::free(p); }
void operator delete[](void* p) noexcept { std::free(p); }

struct Engine {
    STEngine* value = st_engine_create(48000);
    Engine() { assert(value); }
    ~Engine() { st_engine_destroy(value); }
    operator STEngine*() const { return value; }
};
struct Note {
    uint64_t frame;
    int type, track, instrument, note, velocity;
    auto key() const { return std::tie(frame, type, track, instrument, note, velocity); }
    bool operator==(const Note& other) const { return key() == other.key(); }
};
struct Rendered {
    std::vector<float> left, right;
    std::vector<Note> notes;
    explicit Rendered(size_t frames) : left(frames), right(frames) {}
};

static void drainNotes(STEngine* engine, uint64_t start, std::vector<Note>& notes) {
    STEvent event{};
    while (st_engine_pop_event(engine, &event)) {
        assert(event.frameOffset >= 0);
        if (event.type == ST_EVENT_NOTE_ON || event.type == ST_EVENT_NOTE_OFF)
            notes.push_back({start + uint64_t(event.frameOffset), event.type,
                event.track, event.instrument, event.note, event.velocity});
    }
}
static Rendered render(STEngine* engine, size_t frames, size_t block = 127) {
    Rendered result(frames);
    for (size_t at = 0; at < frames; at += block) {
        const auto count = uint32_t(std::min(block, frames - at));
        realtime = true;
        st_engine_render(engine, result.left.data() + at, result.right.data() + at, count);
        realtime = false;
        drainNotes(engine, at, result.notes);
    }
    for (size_t at = 0; at < frames; ++at)
        assert(std::isfinite(result.left[at]) && std::isfinite(result.right[at]));
    assert(st_engine_status(engine).droppedCommands == 0);
    assert(st_engine_status(engine).droppedEvents == 0);
    return result;
}
static void flush(STEngine* engine) {
    realtime = true;
    st_engine_render(engine, nullptr, nullptr, 0);
    realtime = false;
    std::vector<Note> ignored;
    drainNotes(engine, 0, ignored);
}
static std::vector<uint64_t> noteFrames(const std::vector<Note>& notes, int type) {
    std::vector<uint64_t> frames;
    for (const auto& note : notes) if (note.type == type) frames.push_back(note.frame);
    return frames;
}
static void expectFrames(const std::vector<Note>& notes, int type,
                         const std::vector<uint64_t>& expected) {
    const auto actual = noteFrames(notes, type);
    if (actual != expected) {
        std::fprintf(stderr, "Event %d frames: expected", type);
        for (auto frame : expected) std::fprintf(stderr, " %llu", (unsigned long long)frame);
        std::fprintf(stderr, "; actual");
        for (auto frame : actual) std::fprintf(stderr, " %llu", (unsigned long long)frame);
        std::fprintf(stderr, "\n");
        std::abort();
    }
}
static void setMap(STEngine* engine, uint64_t revision = 1) {
    STClockTempoEvent tempos[]{{0, 120}, {.375, 60}};
    STClockMeterEvent meters[]{{0, 4, 4}, {.375, 7, 8}};
    char error[256]{};
    assert(st_engine_set_timing_map(engine, tempos, 2, meters, 2, revision, error, sizeof error) == 1);
    assert(error[0] == '\0');
}
static void beginFixture(STEngine* engine, int type, int rows = 4, float swing = 0) {
    st_engine_begin_update(engine);
    st_engine_clear_project(engine);
    st_engine_set_track_count(engine, 3);
    st_engine_set_timing(engine, 120, 4, swing, 1);
    st_engine_set_host_mixing(engine, 1, 7);
    st_engine_set_pattern(engine, 0, rows);
    int32_t sequence[]{0};
    st_engine_set_sequence(engine, sequence, 1, 0, 0);
    // type < 0: a sustained sine test tone (see engine_test_tone.hpp).
    auto instrument = type < 0 ? testToneInstrument() : st_default_instrument();
    if (type >= 0) instrument.type = type;
    else loadTestTone(engine, 0);
    instrument.attack = instrument.decay = 0;
    instrument.sustain = 1;
    instrument.release = .005f;
    instrument.volume = .2f;
    st_engine_set_instrument(engine, 0, instrument);
    setMap(engine);
}
static void putNote(STEngine* engine, int row, int note, float gate,
                    int track = 0, int instrument = 0) {
    auto step = st_default_step();
    step.note = note;
    step.instrument = instrument;
    step.velocity = 100;
    step.gate = gate;
    st_engine_set_step(engine, 0, track, row, step, nullptr, 0);
}
static void commit(STEngine* engine) {
    char error[256]{};
    if (!st_engine_commit_update_checked(engine, error, sizeof error)) {
        std::fprintf(stderr, "Timing fixture commit failed: %s\n", error);
        std::abort();
    }
}
static void start(STEngine* engine, int songMode = 1) {
    commit(engine);
    st_engine_play(engine, songMode);
}

static void testRowsAndGateAcrossTempo() {
    for (int type : {ST_INSTRUMENT_AU, ST_INSTRUMENT_MIDI}) {
        Engine engine;
        beginFixture(engine, type);
        for (int row = 0; row < 4; ++row) putNote(engine, row, 60 + row, row ? .5f : 2.f);
        start(engine);
        assert(st_engine_duration(engine, 1) == 39000.0 / 48000);
        auto result = render(engine, 39001);
        expectFrames(result.notes, ST_EVENT_NOTE_ON, {0, 6000, 15000, 27000});
        expectFrames(result.notes, ST_EVENT_NOTE_OFF, {9000, 15000, 21000, 33000});
        assert(!st_engine_status(engine).playing);
        auto longOff = std::find_if(result.notes.begin(), result.notes.end(), [](const Note& note) {
            return note.type == ST_EVENT_NOTE_OFF && note.note == 60;
        });
        assert(longOff != result.notes.end() && longOff->frame == 15000);
    }
}

static void testExactPostTempoRowOnsets() {
    const std::vector<uint64_t> onsets{0,6000,12000,24000,36000,48000,60000,64000};
    for (int type : {ST_INSTRUMENT_MIDI, ST_INSTRUMENT_SAMPLE}) {
        Engine engine;
        beginFixture(engine, type);
        STClockTempoEvent tempos[]{{0,120},{.5,60},{1.5,180}};
        STClockMeterEvent meters[]{{0,4,4},{.75,3,4}};
        char error[256]{};
        assert(st_engine_set_timing_map(engine,tempos,3,meters,2,42,error,sizeof error));
        st_engine_set_pattern(engine,1,4);
        int32_t sequence[]{0,1};st_engine_set_sequence(engine,sequence,2,0,0);
        for (int pattern=0;pattern<2;++pattern) for (int row=0;row<4;++row) {
            auto step=st_default_step();step.note=60;step.instrument=0;step.velocity=100;step.gate=.5f;
            st_engine_set_step(engine,pattern,0,row,step,nullptr,0);
        }
        if (type==ST_INSTRUMENT_SAMPLE) {
            std::array<float,64> sample{};sample.fill(.5f);
            assert(st_engine_set_sample(engine,0,sample.data(),nullptr,sample.size(),48000));
        }
        start(engine);
        assert(st_engine_duration(engine,1)==68000.0/48000);
        auto result=render(engine,68001,4097);
        if (type==ST_INSTRUMENT_MIDI) {
            expectFrames(result.notes,ST_EVENT_NOTE_ON,onsets);
            expectFrames(result.notes,ST_EVENT_NOTE_OFF,{3000,9000,18000,30000,42000,54000,62000,66000});
        } else {
            for (auto onset:onsets) {
                assert(std::abs(result.left[onset])>1e-5f);
                if(onset)assert(result.left[onset-1]==0);
            }
        }
        assert(!st_engine_status(engine).playing);
    }
}

static void testMicrotimingAndRatchetAcrossTempo() {
    Engine engine;
    beginFixture(engine, ST_INSTRUMENT_MIDI);
    auto step = st_default_step();
    step.gate = 2;
    step.fx1 = {ST_FX_MICROTIMING, .25f};
    step.fx2 = {ST_FX_RATCHET, 2};
    st_engine_set_step(engine, 0, 0, 1, step, nullptr, 0);
    // Isolate the first row's mapped gates; ratchets now persist across empty rows.
    auto stopRepeat = st_default_step(); stopRepeat.note = -3; stopRepeat.fx1 = {ST_FX_RATCHET, 1};
    st_engine_set_step(engine, 0, 0, 2, stopRepeat, nullptr, 0);
    start(engine);
    auto result = render(engine, 20000, 7);
    // Row one starts at beat .25. Its remaining .1875 beats are divided in two;
    // note ends independently integrate .1875 beats from each ratchet onset. The overlapping
    // repeat of the same external pitch first ends the older note (MIDI has no per-voice ownership).
    expectFrames(result.notes, ST_EVENT_NOTE_ON, {7500, 10500});
    expectFrames(result.notes, ST_EVENT_NOTE_OFF, {10500, 19500});
}

static void testRepeatedOddPatternAndSongLoop() {
    Engine engine;
    beginFixture(engine, ST_INSTRUMENT_AU, 3, .25f);
    for (int row = 0; row < 3; ++row) putNote(engine, row, 60 + row, .5f);
    int32_t sequence[]{0, 0};
    st_engine_set_sequence(engine, sequence, 2, 1, 0);
    start(engine);
    assert(st_engine_duration(engine, 1) == 69000.0 / 48000);
    auto result = render(engine, 138001, 512);
    expectFrames(result.notes, ST_EVENT_NOTE_ON,
        {0, 7500, 15000, 30000, 45000, 54000, 69000, 76500, 84000, 99000, 114000, 123000, 138000});
    expectFrames(result.notes, ST_EVENT_NOTE_OFF,
        {3750, 10500, 22500, 37500, 49500, 61500, 72750, 79500, 91500, 106500, 118500, 130500});
    assert(st_engine_status(engine).playing);
}

static void testPatternOriginAndLoop() {
    Engine engine;
    beginFixture(engine, ST_INSTRUMENT_MIDI, 2);
    putNote(engine, 0, 60, .5f);
    putNote(engine, 1, 62, .5f);
    assert(st_engine_set_pattern_timing_origin(engine, .25) == 1);
    start(engine, 0);
    assert(st_engine_duration(engine, 0) == 21000.0 / 48000);
    auto result = render(engine, 42001, 7);
    expectFrames(result.notes, ST_EVENT_NOTE_ON, {0, 9000, 21000, 30000, 42000});
    expectFrames(result.notes, ST_EVENT_NOTE_OFF, {3000, 15000, 24000, 36000});
    assert(!st_engine_status(engine).songMode && st_engine_status(engine).playing);
}

static void testLiveMapEditPreservesMusicalDeadlines() {
    Engine engine;
    beginFixture(engine, ST_INSTRUMENT_MIDI);
    putNote(engine, 0, 60, 3);
    auto delayed = st_default_step();
    delayed.note = 64;
    delayed.gate = 1;
    delayed.fx1 = {ST_FX_MICROTIMING, .5f};
    st_engine_set_step(engine, 0, 0, 1, delayed, nullptr, 0);
    start(engine);
    auto before = render(engine, 7500);
    expectFrames(before.notes, ST_EVENT_NOTE_ON, {0});
    expectFrames(before.notes, ST_EVENT_NOTE_OFF, {});

    // Old cursor beat .3125 is exactly frame7500. The replacement maps that
    // same beat to frame6750; graph time continues from its current position.
    // The already-scheduled onset at beat.375 maps to7500, its off at.5 to9000,
    // and the existing three-row note's off at.75 to12000.
    STClockTempoEvent tempos[]{{0, 120}, {.25, 240}};
    STClockMeterEvent meters[]{{0, 4, 4}};
    char error[128]{};
    st_engine_begin_update(engine);
    assert(st_engine_set_timing_map(engine, tempos, 2, meters, 1, 2, error, sizeof error));
    commit(engine);
    auto after = render(engine, 6000, 7);
    expectFrames(after.notes, ST_EVENT_NOTE_ON, {750});
    expectFrames(after.notes, ST_EVENT_NOTE_OFF, {2250, 5250});
    assert(after.notes.back().note == 60);
}

static void testClipPlacementAndSourceTime() {
    Engine engine;
    beginFixture(engine, -1);
    std::vector<float> sample(65536);
    for (size_t n = 0; n < sample.size(); ++n) sample[n] = float(n) / 65536;
    assert(st_engine_set_audio_asset(engine, 0, sample.data(), nullptr, sample.size(), 48000));
    st_engine_add_audio_clip(engine, 0, 0, .25, .5, .125, 1, 0, 0, 0);
    start(engine);
    auto result = render(engine, 39001, 127);
    for (size_t frame = 0; frame < result.left.size(); ++frame) {
        // Native clip source PCM stays in seconds while the musical clip end
        // follows the tempo map. Its .125-second source offset is 6000 samples.
        const float expected = frame >= 6000 && frame < 27000 ? float(frame) / 65536 : 0;
        assert(std::abs(result.left[frame] - expected) < 1e-6f);
        assert(std::abs(result.right[frame] - expected) < 1e-6f);
    }
    assert(!st_engine_status(engine).playing);

    // Seeking to row two starts at beat .5/frame15000: 9000 samples into clip,
    // plus the 6000-source-frame offset, and exactly12000 samples remain.
    st_engine_seek(engine, 0, 2, 0);
    st_engine_play(engine, 1);
    auto sought = render(engine, 12001, 512);
    for (size_t frame = 0; frame < sought.left.size(); ++frame) {
        const float expected = frame < 12000 ? float(frame + 15000) / 65536 : 0;
        assert(std::abs(sought.left[frame] - expected) < 1e-6f);
    }
}

static void configureMixedAudio(STEngine* engine) {
    beginFixture(engine, -1, 3, .25f);
    auto synth = testToneInstrument(TEST_SAW);
    loadTestTone(engine, 0, TEST_SAW);
    synth.attack = .003f;
    synth.decay = .007f;
    synth.sustain = .7f;
    synth.release = .013f;
    synth.volume = .15f;
    synth.cutoff = 2500;
    synth.lfoSync = 1;
    synth.lfoBeats = .5f;
    synth.lfoDepth = .1f;
    synth.lfoTarget = ST_MOD_PITCH;
    st_engine_set_instrument(engine, 0, synth);
    auto sampler = synth;
    sampler.type = ST_INSTRUMENT_SAMPLE;
    sampler.mode = ST_MODE_FORWARD;
    sampler.pan = .3f;
    sampler.lfoTarget = ST_MOD_VOLUME;
    st_engine_set_instrument(engine, 1, sampler);
    auto external = st_default_instrument();
    external.type = ST_INSTRUMENT_AU;
    st_engine_set_instrument(engine, 2, external);
    std::vector<float> sample(128);
    for (size_t n = 0; n < sample.size(); ++n) sample[n] = float(int(n % 32) - 16) / 32;
    assert(st_engine_set_sample(engine, 1, sample.data(), nullptr, sample.size(), 48000));
    for (int row = 0; row < 3; ++row) {
        putNote(engine, row, 60 + row * 2, .75f, 0, 0);
        putNote(engine, row, 60 + row, 1.5f, 1, 1);
        putNote(engine, row, 72 + row, .5f, 2, 2);
    }
    int32_t sequence[]{0, 0};
    st_engine_set_sequence(engine, sequence, 2, 1, 0);
    start(engine);
}
static void testRenderBlockEquivalence() {
    Engine reference;
    configureMixedAudio(reference);
    auto expected = render(reference, 72001, 1);
    assert(std::any_of(expected.left.begin(), expected.left.end(), [](float value) { return std::abs(value) > .001f; }));
    for (size_t block : {7u, 127u, 512u, 4096u}) {
        Engine engine;
        configureMixedAudio(engine);
        auto actual = render(engine, expected.left.size(), block);
        assert(actual.left == expected.left && actual.right == expected.right && actual.notes == expected.notes);
        assert(st_engine_status(engine).seconds == st_engine_status(reference).seconds);
    }
}

struct BusRendered {
    Rendered first, second;
    explicit BusRendered(size_t frames) : first(frames), second(frames) {}
};
static BusRendered renderBuses(STEngine* engine, size_t frames, bool overlap) {
    BusRendered result(frames);
    std::vector<float> left(4097), right(4097);
    for (size_t at = 0; at < frames; at += 4096) {
        const auto count = uint32_t(std::min<size_t>(4096, frames - at));
        const auto pull = uint32_t(std::min<size_t>(count + (overlap ? 1 : 0), frames - at));
        realtime = true;
        st_engine_render_bus(engine, 0, at, left.data(), right.data(), pull);
        realtime = false;
        std::copy_n(left.begin(), count, result.first.left.begin() + at);
        std::copy_n(right.begin(), count, result.first.right.begin() + at);
        drainNotes(engine, at, result.first.notes);
        realtime = true;
        st_engine_render_bus(engine, 1, at, result.second.left.data() + at, result.second.right.data() + at, count);
        realtime = false;
        drainNotes(engine, at, result.second.notes);
    }
    assert(result.second.notes.empty());
    assert(st_engine_status(engine).droppedEvents == 0);
    return result;
}
static void testOverlappingBusPulls() {
    Engine reference, overlapping;
    configureMixedAudio(reference);
    configureMixedAudio(overlapping);
    auto expected = renderBuses(reference, 72001, false);
    auto actual = renderBuses(overlapping, 72001, true);
    const auto audible = [](float value) { return std::abs(value) > .001f; };
    assert(std::any_of(expected.first.left.begin(), expected.first.left.end(), audible));
    assert(std::any_of(expected.second.left.begin(), expected.second.left.end(), audible));
    assert(actual.first.left == expected.first.left && actual.first.right == expected.first.right);
    assert(actual.second.left == expected.second.left && actual.second.right == expected.second.right);
    assert(actual.first.notes == expected.first.notes);
    assert(st_engine_status(overlapping).seconds == st_engine_status(reference).seconds);
}

static void testDraftPublicationAndInvalidAtomicity() {
    Engine engine;
    beginFixture(engine, ST_INSTRUMENT_MIDI);
    for (int row = 0; row < 4; ++row) putNote(engine, row, 60 + row, .5f);
    commit(engine);
    flush(engine);

    STClockTempoEvent flat[]{{0, 240}};
    STClockMeterEvent meter[]{{0, 4, 4}};
    char error[128]{};
    st_engine_begin_update(engine);
    assert(st_engine_set_timing_map(engine, flat, 1, meter, 1, 2, error, sizeof error));
    // The setter must copy caller arrays, and only commit publishes the draft.
    flat[0].bpm = 20;
    meter[0].numerator = 0;
    st_engine_play(engine, 1);
    auto beforeCommit = render(engine, 15001);
    expectFrames(beforeCommit.notes, ST_EVENT_NOTE_ON, {0, 6000, 15000});
    st_engine_stop(engine);
    flush(engine);

    const STClockTempoEvent validTempo[]{{0, 120}};
    const STClockMeterEvent validMeter[]{{0, 4, 4}};
    const auto rejects = [&](const STClockTempoEvent* tempos, uint32_t tempoCount,
                             const STClockMeterEvent* meters, uint32_t meterCount) {
        std::fill(std::begin(error), std::end(error), 'x');
        assert(st_engine_set_timing_map(engine, tempos, tempoCount, meters, meterCount, 3, error, sizeof error) == 0);
        assert(error[0] != '\0' && std::memchr(error, '\0', sizeof error));
    };
    rejects(nullptr, 1, validMeter, 1);
    rejects(validTempo, 0, validMeter, 1);
    rejects(validTempo, ST_CLOCK_MAX_EVENTS + 1, validMeter, 1);
    rejects(validTempo, 1, nullptr, 1);
    rejects(validTempo, 1, validMeter, 0);
    rejects(validTempo, 1, validMeter, ST_CLOCK_MAX_EVENTS + 1);
    for (const auto bad : std::vector<STClockTempoEvent>{{.25, 120}, {0, 19}, {0, 401},
             {0, std::numeric_limits<double>::quiet_NaN()}, {std::numeric_limits<double>::infinity(), 120}}) {
        rejects(&bad, 1, validMeter, 1);
    }
    STClockTempoEvent duplicate[]{{0, 120}, {0, 60}};
    STClockTempoEvent descending[]{{0, 120}, {1, 60}, {.5, 120}};
    STClockTempoEvent collapsed[]{{0, 120}, {.5000000001, 60}, {.5000000002, 90}};
    rejects(duplicate, 2, validMeter, 1);
    rejects(descending, 3, validMeter, 1);
    rejects(collapsed, 3, validMeter, 1);
    for (const auto bad : std::vector<STClockMeterEvent>{{0, 0, 4}, {0, 4, 3}, {.25, 4, 4}})
        rejects(validTempo, 1, &bad, 1);
    assert(st_engine_set_pattern_timing_origin(engine, -.25) == 0);
    assert(st_engine_set_pattern_timing_origin(engine, std::numeric_limits<double>::quiet_NaN()) == 0);

    commit(engine);
    st_engine_seek(engine, 0, 0, 0);
    st_engine_play(engine, 1);
    auto afterCommit = render(engine, 9001);
    expectFrames(afterCommit.notes, ST_EVENT_NOTE_ON, {0, 3000, 6000, 9000});
    st_engine_stop(engine);
    flush(engine);

    st_engine_begin_update(engine);
    st_engine_clear_timing_map(engine);
    commit(engine);
    st_engine_seek(engine, 0, 0, 0);
    st_engine_play(engine, 1);
    auto cleared = render(engine, 18001);
    expectFrames(cleared.notes, ST_EVENT_NOTE_ON, {0, 6000, 12000, 18000});
}

static int clockSpan(STEngine* engine, int64_t frame, uint32_t requested,
                     int64_t latency, STClockSpan* span) {
    realtime = true;
    const int result = st_engine_clock_span(engine, frame, requested, latency, span);
    realtime = false;
    return result;
}
static STClockSpan preparedSpan(STEngine* engine, int64_t frame,
                                uint32_t requested = 64, int64_t latency = 0) {
    STClockSpan span{};
    assert(clockSpan(engine, frame, requested, latency, &span) == ST_CLOCK_OK);
    return span;
}
static void expectClockError(STEngine* engine, int64_t frame, int64_t latency, int expected) {
    STClockSpan untouched;
    std::memset(&untouched, 0x5a, sizeof untouched);
    unsigned char before[sizeof untouched];
    std::memcpy(before, &untouched, sizeof untouched);
    assert(clockSpan(engine, frame, 64, latency, &untouched) == expected);
    assert(std::memcmp(before, &untouched, sizeof untouched) == 0);
}
static void prepareBus(STEngine* engine, uint64_t startFrame, uint32_t frames) {
    std::vector<float> left(frames), right(frames);
    realtime = true;
    st_engine_render_bus(engine, 0, startFrame, left.data(), right.data(), frames);
    realtime = false;
}
static auto contextKey(const STClockContext& c) {
    return std::tie(c.revision, c.epoch, c.graphFrame, c.contentGraphFrame,
        c.songFrame, c.loopIteration, c.bar, c.sampleRate, c.beat, c.bpm,
        c.barStartBeat, c.barEndBeat, c.numerator, c.denominator, c.flags,
        c.nextBoundaryFrame, c.nextBoundaryKinds, c.loopStartBeat, c.loopEndBeat,
        c.nextQuarterBeatFrame,c.nextQuarterGridFrame);
}
static auto statusKey(const STStatus& s) {
    return std::tie(s.playing, s.pattern, s.row, s.sequence, s.trackCount,
        s.peakL, s.peakR, s.position, s.seconds, s.droppedCommands,
        s.droppedEvents, s.songMode, s.beat, s.bpm, s.barStartBeat, s.bar,
        s.numerator, s.denominator, s.timingRevision,s.recording);
}
static void testRetainedClockSpans() {
    Engine engine;
    beginFixture(engine, ST_INSTRUMENT_MIDI);
    for (int row = 0; row < 4; ++row) putNote(engine, row, 60 + row, .5f);
    start(engine);
    expectClockError(engine, 0, 0, ST_ENGINE_CLOCK_NOT_PREPARED);
    prepareBus(engine, 0, 12000);
    const auto status = st_engine_status(engine);
    // This command and the queued notes must survive all read-only queries.
    st_engine_note_on(engine, 2, 0, 72, 100);
    auto beforeChange = preparedSpan(engine, 8999, 4096);
    assert(beforeChange.frames == 1);
    assert(beforeChange.context.bpm == 120 && beforeChange.context.revision == 1);
    assert(beforeChange.context.numerator == 4 && beforeChange.context.denominator == 4);
    assert(beforeChange.context.graphFrame == 8999 && beforeChange.context.contentGraphFrame == 8999);
    assert(beforeChange.context.songFrame == 8999 && beforeChange.context.nextBoundaryFrame == 9000);
    assert((beforeChange.context.nextBoundaryKinds & (ST_CLOCK_BOUNDARY_TEMPO | ST_CLOCK_BOUNDARY_METER))
        == (ST_CLOCK_BOUNDARY_TEMPO | ST_CLOCK_BOUNDARY_METER));

    const auto oldRevision = preparedSpan(engine, 9000);
    assert(oldRevision.frames == 64 && oldRevision.context.beat == .375);
    assert(oldRevision.context.bpm == 60 && oldRevision.context.revision == 1);
    assert(oldRevision.context.numerator == 7 && oldRevision.context.denominator == 8);
    assert(oldRevision.context.bar == 2 && oldRevision.context.barStartBeat == .375);
    assert(oldRevision.context.flags & ST_CLOCK_PLAYING);

    auto delayed = preparedSpan(engine, 9127, 256, 128);
    assert(delayed.frames == 1 && delayed.context.graphFrame == 9127);
    assert(delayed.context.contentGraphFrame == 8999 && delayed.context.songFrame == 8999);
    assert(delayed.context.beat == beforeChange.context.beat);
    assert(delayed.context.nextBoundaryFrame == 9128);
    if (beforeChange.context.nextQuarterBeatFrame != std::numeric_limits<int64_t>::max())
        assert(delayed.context.nextQuarterBeatFrame == beforeChange.context.nextQuarterBeatFrame + 128);
    expectClockError(engine, 12000, 0, ST_ENGINE_CLOCK_NOT_PREPARED);
    const auto startup = preparedSpan(engine, -1);
    assert(startup.frames == 1 && startup.context.songFrame == -1);
    assert(startup.context.flags & ST_CLOCK_PREROLL);
    const auto delayedStartup = preparedSpan(engine, 0, 64, 1);
    assert(delayedStartup.frames == 1 && delayedStartup.context.contentGraphFrame == -1);
    assert(delayedStartup.context.nextBoundaryFrame == 1);
    assert(statusKey(status) == statusKey(st_engine_status(engine)));
    std::vector<Note> pendingNotes;
    drainNotes(engine, 0, pendingNotes);
    expectFrames(pendingNotes, ST_EVENT_NOTE_ON, {0, 6000});
    expectFrames(pendingNotes, ST_EVENT_NOTE_OFF, {3000, 9000});

    STClockTempoEvent tempos[]{{0, 240}};
    STClockMeterEvent meters[]{{0, 3, 4}};
    char error[128]{};
    st_engine_begin_update(engine);
    assert(st_engine_set_timing_map(engine, tempos, 1, meters, 1, 2, error, sizeof error));
    commit(engine);
    // Pending publication cannot change cached history or consume the live note.
    auto pendingRevision = preparedSpan(engine, 9000);
    assert(contextKey(pendingRevision.context) == contextKey(oldRevision.context));
    assert(statusKey(status) == statusKey(st_engine_status(engine)));
    prepareBus(engine, 12000, 4096);
    std::vector<Note> newNotes;
    drainNotes(engine, 12000, newNotes);
    auto live = std::find_if(newNotes.begin(), newNotes.end(), [](const Note& note) {
        return note.type == ST_EVENT_NOTE_ON && note.note == 72 && note.track == 2;
    });
    assert(live != newNotes.end() && live->frame == 12000);

    const auto currentRevision = preparedSpan(engine, 12000);
    assert(currentRevision.context.revision == 2 && currentRevision.context.bpm == 240);
    assert(currentRevision.context.numerator == 3 && currentRevision.context.denominator == 4);
    assert(currentRevision.context.beat == .4375 && currentRevision.context.songFrame == 5250);
    const auto retainedRevision = preparedSpan(engine, 9000);
    assert(contextKey(retainedRevision.context) == contextKey(oldRevision.context));
    auto revisionEdge = preparedSpan(engine, 11999, 256);
    assert(revisionEdge.frames == 1 && revisionEdge.context.revision == 1);
    auto availablePrefix = preparedSpan(engine, 16080, 256);
    assert(availablePrefix.frames == 16 && availablePrefix.context.revision == 2);
    expectClockError(engine, 16096, 0, ST_ENGINE_CLOCK_NOT_PREPARED);

    // A96k upstream delay still addresses retained clock history after its PCM
    // has expired from the separate65536-frame audio cache.
    prepareBus(engine, 16096, 65536);
    prepareBus(engine, 81632, 30368);
    const auto longLatency = preparedSpan(engine, 105000, 64, 96000);
    assert(longLatency.context.contentGraphFrame == 9000);
    assert(longLatency.context.revision == 1 && longLatency.context.beat == .375);
    assert(longLatency.context.bpm == 60 && longLatency.context.numerator == 7);

    // Expired post-origin history must not be synthesized as startup preroll.
    const uint64_t finalFrame = 16096 + ST_ENGINE_CLOCK_HISTORY_FRAMES;
    for (uint64_t at = 112000; at < finalFrame;) {
        const auto count = uint32_t(std::min<uint64_t>(65536, finalFrame - at));
        prepareBus(engine, at, count);
        at += count;
    }
    expectClockError(engine, 16095, 0, ST_ENGINE_CLOCK_NOT_RETAINED);
    assert(preparedSpan(engine, 16096).context.revision == 2);
    assert(preparedSpan(engine, int64_t(finalFrame - 1)).frames == 1);
    expectClockError(engine, int64_t(finalFrame), 0, ST_ENGINE_CLOCK_NOT_PREPARED);
    assert(preparedSpan(engine, -1).context.revision == 1);
    assert(st_engine_status(engine).droppedCommands == 0 && st_engine_status(engine).droppedEvents == 0);
}

static void testMappedStartupPrerollAndOriginLifetime() {
    Engine engine;
    beginFixture(engine, ST_INSTRUMENT_MIDI, 8);
    putNote(engine, 2, 62, .5f);
    int32_t sequence[]{0};
    st_engine_set_sequence(engine, sequence, 1, 1, 0);
    commit(engine);
    st_engine_seek(engine, 0, 2, 0);
    st_engine_play(engine, 1);
    const auto pendingStatus = st_engine_status(engine);
    expectClockError(engine, 1000, 6000, ST_ENGINE_CLOCK_NOT_PREPARED);
    assert(statusKey(pendingStatus) == statusKey(st_engine_status(engine)));
    prepareBus(engine, 1000, 64);
    const auto status = st_engine_status(engine);

    // Startup seeks to beat.5/frame15000. Latency looks backward through the
    // actual map, including its tempo and meter event at song frame9000.
    const auto preceding = preparedSpan(engine, 1000, 64, 6001);
    assert(preceding.frames == 1 && preceding.context.songFrame == 8999);
    assert(preceding.context.bpm == 120 && preceding.context.numerator == 4);
    assert(preceding.context.denominator == 4 && preceding.context.nextBoundaryFrame == 1001);
    assert(preceding.context.flags & ST_CLOCK_PREROLL);
    const auto boundary = preparedSpan(engine, 1000, 64, 6000);
    assert(boundary.context.songFrame == 9000 && boundary.context.beat == .375);
    assert(boundary.context.bpm == 60 && boundary.context.numerator == 7 && boundary.context.denominator == 8);
    assert(boundary.context.flags & ST_CLOCK_PLAYING);
    const auto negative = preparedSpan(engine, 1000, 64, 16000);
    assert(negative.context.songFrame == -1000 && negative.context.beat < 0);
    assert(negative.context.bar == 0 && negative.context.barEndBeat == 0);
    assert((negative.context.flags & (ST_CLOCK_PREROLL | ST_CLOCK_PLAYING | ST_CLOCK_LOOPING))
        == (ST_CLOCK_PREROLL | ST_CLOCK_PLAYING | ST_CLOCK_LOOPING));
    assert(negative.context.loopIteration == 0);
    const auto edge = preparedSpan(engine, 1000, 64, 1);
    assert(edge.frames == 1 && edge.context.nextBoundaryFrame == 1001);
    assert(edge.context.nextBoundaryKinds & ST_CLOCK_BOUNDARY_ORIGIN);
    assert(statusKey(status) == statusKey(st_engine_status(engine)));
    std::vector<Note> notes;
    drainNotes(engine, 1000, notes);
    expectFrames(notes, ST_EVENT_NOTE_ON, {1000});

    // Even retired project snapshots can be collected while this graph's
    // immutable startup origin keeps its original map alive.
    for (int revision = 2; revision <= 3; ++revision) {
        STClockTempoEvent tempos[]{{0, revision == 2 ? 240. : 96.}};
        STClockMeterEvent meters[]{{0, revision == 2 ? 3 : 5, revision == 2 ? 4 : 8}};
        char error[128]{};
        st_engine_begin_update(engine);
        assert(st_engine_set_timing_map(engine, tempos, 1, meters, 1, revision, error, sizeof error));
        commit(engine);
        st_engine_collect(engine);
        assert(contextKey(preparedSpan(engine, 1000, 64, 6001).context) == contextKey(preceding.context));
        prepareBus(engine, 1064 + uint64_t(revision - 2) * 64, 64);
        st_engine_collect(engine);
        assert(contextKey(preparedSpan(engine, 1000, 64, 6001).context) == contextKey(preceding.context));
    }

    // A stopped graph reset establishes a fresh origin only after queued seek
    // and Play are applied by the first pull, not by the read-only query.
    st_engine_seek(engine, 0, 3, 0);
    st_engine_play(engine, 1);
    st_engine_reset_render_cache(engine);
    expectClockError(engine, 200000, 7500, ST_ENGINE_CLOCK_NOT_PREPARED);
    prepareBus(engine, 200000, 64);
    st_engine_collect(engine);
    const auto reset = preparedSpan(engine, 200000, 64, 7500);
    assert(reset.context.revision == 3 && reset.context.epoch != preceding.context.epoch);
    assert(reset.context.songFrame == 15000 && reset.context.beat == .5);
    assert(reset.context.bpm == 96 && reset.context.numerator == 5 && reset.context.denominator == 8);
    assert(reset.context.flags & ST_CLOCK_PREROLL);
    assert(preparedSpan(engine, 1000).context.revision == 3);
    const auto furthest = preparedSpan(engine, 200000 - (int64_t(1) << 30));
    assert(furthest.frames > 0 && (furthest.context.flags & ST_CLOCK_PREROLL));
    expectClockError(engine, 200000 - (int64_t(1) << 30) - 1, 0, ST_ENGINE_CLOCK_NOT_RETAINED);
}

static void testLegacyFractionalPrerollAndStoppedOrigin() {
    Engine engine;
    beginFixture(engine, ST_INSTRUMENT_MIDI, 64);
    st_engine_clear_timing_map(engine);
    st_engine_set_timing(engine, 999, 4, .25f, 1);
    st_engine_set_metronome(engine, 0, 3, 8, .3f);
    putNote(engine, 1, 61, .5f);
    putNote(engine, 4, 64, .5f);
    commit(engine);
    st_engine_seek(engine, 0, 1, 0);
    st_engine_play(engine, 1);
    prepareBus(engine, 4096, 1);

    // Row one begins at .3125 beats =900000/999 source frames. Preserve that
    // fractional origin: latency1000 reaches the song-zero boundary in100,
    // rather than99, graph samples.
    const auto preroll = preparedSpan(engine, 4096, 256, 1000);
    assert(preroll.frames == 100 && preroll.context.nextBoundaryFrame == 4196);
    assert(preroll.context.nextQuarterBeatFrame == 4196);
    assert(std::abs(preroll.context.beat - (-.034375)) < 1e-14);
    assert(preroll.context.bpm == 999 && preroll.context.bar == 0);
    assert(preroll.context.barStartBeat == -1.5 && preroll.context.barEndBeat == 0);
    assert(preroll.context.flags & ST_CLOCK_PREROLL);
    const auto beforeZero = preparedSpan(engine, 4195, 64, 1000);
    const auto afterZero = preparedSpan(engine, 4196, 64, 1000);
    assert(beforeZero.frames == 1 && beforeZero.context.beat < 0);
    assert(afterZero.context.bar == 1 && afterZero.context.beat > 0);
    const auto negativeBar = preparedSpan(engine, 4096, 1024, 6000);
    assert(negativeBar.frames == 775 && negativeBar.context.nextBoundaryFrame == 4871);
    assert(negativeBar.context.bar == -1 && negativeBar.context.barStartBeat == -3 && negativeBar.context.barEndBeat == -1.5);
    std::vector<Note> notes;
    drainNotes(engine, 4096, notes);
    expectFrames(notes, ST_EVENT_NOTE_ON, {4096});

    st_engine_stop(engine);
    st_engine_seek(engine, 0, 1, 0);
    st_engine_reset_render_cache(engine);
    expectClockError(engine, 10000, 1000, ST_ENGINE_CLOCK_NOT_PREPARED);
    prepareBus(engine, 10000, 64);
    const auto held = preparedSpan(engine, 10000, 64, 1000);
    const auto heldFarther = preparedSpan(engine, 10000, 64, 1000000);
    assert(std::abs(held.context.beat - .3125) < 1e-14 && held.context.beat == heldFarther.context.beat);
    assert(held.context.songFrame == 900 && held.context.bpm == 999);
    assert((held.context.flags & (ST_CLOCK_PLAYING | ST_CLOCK_PREROLL)) == 0);
    assert(held.context.nextQuarterBeatFrame == std::numeric_limits<int64_t>::max());
    notes.clear();
    drainNotes(engine, 10000, notes);
    expectFrames(notes, ST_EVENT_NOTE_ON, {});

    const auto heldStatus = st_engine_status(engine);
    st_engine_seek(engine, 0, 4, 0);
    st_engine_play(engine, 1);
    assert(contextKey(preparedSpan(engine, 10000, 64, 1000).context) == contextKey(held.context));
    assert(statusKey(heldStatus) == statusKey(st_engine_status(engine)));
    notes.clear();
    drainNotes(engine, 10000, notes);
    assert(notes.empty());
    prepareBus(engine, 10064, 1);
    notes.clear();
    drainNotes(engine, 10064, notes);
    expectFrames(notes, ST_EVENT_NOTE_ON, {10064});
    assert(notes.back().note == 64);
    assert(contextKey(preparedSpan(engine, 10000, 64, 1000).context) == contextKey(held.context));
    const auto playing = preparedSpan(engine, 10064);
    assert(playing.context.flags & ST_CLOCK_PLAYING);
    assert(std::abs(playing.context.beat - 1) < 1e-14);
}

static void testLegacyFractionalSongBoundaryPartitioning() {
    for (int loop : {0, 1}) {
        Engine engine;
        beginFixture(engine, ST_INSTRUMENT_MIDI, 7);
        st_engine_clear_timing_map(engine);
        st_engine_set_timing(engine, 123, 4, .25f, 1);
        for (int row = 0; row < 7; ++row) putNote(engine, row, 60 + row, .5f);
        int32_t sequence[]{0};
        st_engine_set_sequence(engine, sequence, 1, loop, 0);
        commit(engine);
        st_engine_seek(engine, 0, 1, 0);
        st_engine_play(engine, 1);
        prepareBus(engine, 1000, 36000);
        std::vector<Note> notes;
        drainNotes(engine, 1000, notes);

        // Seek beat.3125 has a fractional source-frame origin. The seven-row
        // swung pattern ends atbeat1.8125: ceil(1.5*48000*60/123)=35122 frames
        // later, not ceil(endFrame)-floor(originFrame)=35123.
        constexpr int64_t boundary = 1000 + 35122;
        const auto before = preparedSpan(engine, boundary - 1, 256);
        assert(before.frames == 1 && (before.context.flags & ST_CLOCK_PLAYING));
        assert(before.context.nextBoundaryFrame == boundary);
        assert(before.context.nextBoundaryKinds & (loop ? ST_CLOCK_BOUNDARY_LOOP : ST_CLOCK_BOUNDARY_DOMAIN));
        const auto after = preparedSpan(engine, boundary, 256);
        assert(after.frames > 0 && bool(after.context.flags & ST_CLOCK_PLAYING) == bool(loop));
        assert(after.context.epoch != before.context.epoch);
        if (loop) {
            assert(after.context.loopIteration == 1 && after.context.beat == 0);
            std::vector<uint64_t> loopOnsets;
            for (const auto& note : notes)
                if (note.type == ST_EVENT_NOTE_ON && note.note == 60) loopOnsets.push_back(note.frame);
            assert(loopOnsets == std::vector<uint64_t>{uint64_t(boundary)});
        }

        // Every retained sample belongs to a positive span; none crosses an
        // actual stop or wrap even when row accumulation has fractional carry.
        int64_t at = 1000;
        while (at < 37000) {
            const auto span = preparedSpan(engine, at, uint32_t(std::min<int64_t>(65536, 37000 - at)));
            assert(span.frames > 0 && at + span.frames <= 37000);
            const auto last = preparedSpan(engine, at + span.frames - 1, 1);
            assert(last.context.epoch == span.context.epoch);
            assert(last.context.loopIteration == span.context.loopIteration);
            assert((last.context.flags & ST_CLOCK_PLAYING) == (span.context.flags & ST_CLOCK_PLAYING));
            at += span.frames;
        }
    }
}

static void testConcurrentOriginOwnership() {
    Engine engine;
    beginFixture(engine, ST_INSTRUMENT_MIDI, 64);
    start(engine);
    std::atomic<unsigned> published{0}, completed{0};
    std::thread audio([&] {
        std::array<float, 64> left{}, right{};
        for (unsigned round = 0; round < 64; ++round) {
            while (published.load(std::memory_order_acquire) <= round) std::this_thread::yield();
            const uint64_t graph = 4096 + uint64_t(round) * 1024;
            for (uint64_t origin : {graph, graph + 512}) {
                st_engine_reset_render_cache(engine);
                expectClockError(engine, int64_t(origin), 256, ST_ENGINE_CLOCK_NOT_PREPARED);
                realtime = true;
                st_engine_render_bus(engine, 0, origin, left.data(), right.data(), left.size());
                realtime = false;
                const auto preroll = preparedSpan(engine, int64_t(origin), 64, 256);
                assert(preroll.context.revision == round + 2);
                assert(preroll.context.flags & ST_CLOCK_PREROLL);
                assert(preroll.frames > 0 && std::isfinite(preroll.context.beat));
            }
            STEvent ignored{};
            while (st_engine_pop_event(engine, &ignored)) {}
            completed.store(round + 1, std::memory_order_release);
        }
    });
    // Collection races origin acquisition/release on the audio thread. Only
    // this control thread publishes projects; render/reset/query stay serialized.
    for (unsigned round = 0; round < 64; ++round) {
        STClockTempoEvent tempos[]{{0, round % 2 ? 96. : 240.}};
        STClockMeterEvent meters[]{{0, round % 2 ? 5 : 3, 4}};
        char error[128]{};
        st_engine_begin_update(engine);
        assert(st_engine_set_timing_map(engine, tempos, 1, meters, 1, round + 2, error, sizeof error));
        commit(engine);
        published.store(round + 1, std::memory_order_release);
        do {
            st_engine_collect(engine);
            std::this_thread::yield();
        } while (completed.load(std::memory_order_acquire) <= round);
    }
    audio.join();
    st_engine_collect(engine);
    assert(completed.load() == 64);
    assert(st_engine_status(engine).droppedCommands == 0 && st_engine_status(engine).droppedEvents == 0);
}

static void testRecordingContextAndCheckpoint() {
    Engine engine;beginFixture(engine,ST_INSTRUMENT_MIDI);commit(engine);
    st_engine_seek(engine,0,1,0);prepareBus(engine,0,32);
    auto held=preparedSpan(engine,0,32);assert(held.context.beat==.25);
    assert(held.context.nextQuarterBeatFrame==std::numeric_limits<int64_t>::max());
    assert(held.context.nextQuarterGridFrame==33000&&(held.context.flags&ST_CLOCK_GRID_PROJECTION));
    assert(!(held.context.flags&ST_CLOCK_RECORDING)&&st_engine_status(engine).recording==0);

    st_engine_set_recording(engine,1);
    // Publishing intent cannot rewrite already-generated, possibly PDC-delayed
    // context or consume the command during a read-only span query.
    assert(contextKey(preparedSpan(engine,0,32).context)==contextKey(held.context));
    assert(st_engine_status(engine).recording==0);
    prepareBus(engine,32,32);
    auto before=preparedSpan(engine,0,64),armed=preparedSpan(engine,32,32);
    assert(before.frames==32&&before.context.epoch!=armed.context.epoch);
    assert(armed.context.flags&ST_CLOCK_RECORDING);
    assert(!(armed.context.flags&ST_CLOCK_PLAYING)&&st_engine_status(engine).recording==1);
    auto delayed=preparedSpan(engine,96,32,64);
    assert(delayed.context.flags&ST_CLOCK_RECORDING);
    assert(delayed.context.nextQuarterGridFrame==33096);
    st_engine_set_recording(engine,1);prepareBus(engine,64,32);
    assert(preparedSpan(engine,32,64).frames==64); // Redundant intent adds no seam.
    st_engine_play(engine,1);prepareBus(engine,96,32);
    assert((preparedSpan(engine,96,32).context.flags&(ST_CLOCK_PLAYING|ST_CLOCK_RECORDING))
        ==(ST_CLOCK_PLAYING|ST_CLOCK_RECORDING));
    st_engine_stop(engine);prepareBus(engine,128,32);
    auto stopped=preparedSpan(engine,128,32);
    assert(!(stopped.context.flags&ST_CLOCK_PLAYING)&&(stopped.context.flags&ST_CLOCK_RECORDING));

    auto saved=st_engine_capture_transport(engine);assert(saved.recording==1&&!saved.playing);
    st_engine_set_recording(engine,0);auto temporary=st_engine_capture_transport(engine);
    assert(temporary.recording==0&&st_engine_status(engine).recording==0);
    st_engine_restore_transport(engine,saved,0);assert(st_engine_status(engine).recording==1);
    prepareBus(engine,0,32);st_engine_set_recording(engine,0);prepareBus(engine,32,32);
    assert(!(preparedSpan(engine,32,32).context.flags&ST_CLOCK_RECORDING));
    // The cache's pinned stopped origin keeps its original recording context.
    auto origin=preparedSpan(engine,0,32,64);
    assert(origin.context.flags&ST_CLOCK_RECORDING);
    assert(origin.context.flags&ST_CLOCK_GRID_PROJECTION);
    assert(origin.context.nextQuarterGridFrame>origin.context.graphFrame);
}

static void testEngineShortLoopProjectedGrid() {
    Engine engine;beginFixture(engine,ST_INSTRUMENT_MIDI,1);
    st_engine_set_timing(engine,120,32,0,1);
    assert(st_engine_set_pattern_timing_origin(engine,.1));start(engine,0);
    prepareBus(engine,0,1600);
    for(int64_t frame:{0LL,17LL,749LL,750LL,800LL,1500LL}){
        auto c=preparedSpan(engine,frame,1).context;
        assert(c.flags&ST_CLOCK_LOOPING);assert(c.flags&ST_CLOCK_GRID_PROJECTION);
        assert(c.nextQuarterBeatFrame==std::numeric_limits<int64_t>::max());
        assert(c.nextQuarterGridFrame==frame+39000-c.songFrame);
    }
    auto delayed=preparedSpan(engine,864,1,64).context;
    assert(delayed.nextQuarterGridFrame==864+39000-delayed.songFrame);
}

int main(int argc, char** argv) {
    if (argc == 2 && std::strcmp(argv[1], "ownership") == 0) {
        testConcurrentOriginOwnership();
        assert(realtimeAllocations.load() == 0);
        std::puts("Engine timing ownership: 64 concurrent publications/collections, 128 origin resets/preroll queries passed; zero render/query allocations.");
        return 0;
    }
    testRowsAndGateAcrossTempo();
    testExactPostTempoRowOnsets();
    testMicrotimingAndRatchetAcrossTempo();
    testRepeatedOddPatternAndSongLoop();
    testPatternOriginAndLoop();
    testLiveMapEditPreservesMusicalDeadlines();
    testClipPlacementAndSourceTime();
    testRenderBlockEquivalence();
    testOverlappingBusPulls();
    testDraftPublicationAndInvalidAtomicity();
    testRetainedClockSpans();
    testMappedStartupPrerollAndOriginLifetime();
    testLegacyFractionalPrerollAndStoppedOrigin();
    testLegacyFractionalSongBoundaryPartitioning();
    testConcurrentOriginOwnership();
    testRecordingContextAndCheckpoint();
    testEngineShortLoopProjectedGrid();
    assert(realtimeAllocations.load() == 0);
    std::puts("Engine timing: 17 suites passed; mapped playback and exact post-tempo PCM/event onsets, exact PCM block/overlap equivalence, atomic drafts, retained clock history, startup preroll/map lifetime, legacy fractional/stopped origins and song boundaries, concurrent origin ownership, recording history/checkpoints and projected stopped/short-loop grid; zero render/query allocations.");
}

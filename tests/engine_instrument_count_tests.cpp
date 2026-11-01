// Instruments are sized per project, like tracks: more than 64 instruments, bounded indices.
#include "TrackerEngine.h"
#include "engine_test_tone.hpp"
#include <cassert>
#include <cmath>
#include <cstdio>
#include <vector>

struct Engine { STEngine *e = st_engine_create(48000); Engine() { assert(e); } ~Engine() { st_engine_destroy(e); } operator STEngine *() { return e; } };
static double energy(STEngine *e, size_t frames) {
    std::vector<float> l(frames), r(frames);
    st_engine_render(e, l.data(), r.data(), uint32_t(frames));
    double sum = 0; for (size_t i = 0; i < frames; ++i) sum += double(l[i]) * l[i];
    return sum / double(frames);
}

int main() {
    int groups = 0;
    assert(st_instrument_capacity(64) == 64 && st_instrument_capacity(ST_INSTRUMENT_SANITY_LIMIT) == ST_INSTRUMENT_SANITY_LIMIT);
    std::puts("PASS the native instrument table holds exactly the project's instruments"); ++groups;

    // 300 ordinary instruments: instrument 250 plays, a step can name it, a track can use it.
    {
        Engine e;
        st_engine_begin_update(e);
        assert(st_engine_set_track_count(e, 2) && st_engine_set_instrument_count(e, 300));
        st_engine_set_timing(e, 120, 4, 0, 1);
        auto track = st_default_track(); track.instrument = 250; track.volume = 1;
        st_engine_set_track(e, 0, track);
        auto p = testToneInstrument(); p.attack = 0; p.sustain = 1; p.volume = .5f;
        st_engine_set_instrument(e, 250, p); loadTestTone(e, 250);
        st_engine_set_pattern(e, 0, 4);
        auto step = st_default_step(); step.note = 69; step.instrument = 250; step.gate = 4;
        st_engine_set_step(e, 0, 0, 0, step, nullptr, 0);
        st_engine_commit_update(e);
        st_engine_play(e, 0);
        assert(energy(e, 4800) > .01);
        st_engine_stop(e); st_engine_panic(e); energy(e, 512);
        st_engine_note_on(e, 1, 250, 69, 100);
        assert(energy(e, 4800) > .01);
        // Out-of-range indices are rejected, not wrapped onto another instrument.
        st_engine_panic(e); energy(e, 512);
        st_engine_note_on(e, 1, 300, 69, 100);
        assert(energy(e, 4800) == 0);
        assert(!st_engine_set_sample_cycle(e, 300, 0) && st_engine_set_sample_cycle(e, 299, 0));
    }
    std::puts("PASS 300 instruments: steps, tracks and live notes reach instrument 250"); ++groups;

    // Shrinking keeps the remaining instruments and their samples; the count is bounded.
    {
        Engine e;
        st_engine_begin_update(e);
        assert(st_engine_set_instrument_count(e, 200));
        auto p = testToneInstrument(); p.attack = 0; p.sustain = 1; p.volume = .5f;
        st_engine_set_instrument(e, 5, p); loadTestTone(e, 5);
        st_engine_commit_update(e);
        st_engine_begin_update(e);
        assert(st_engine_set_instrument_count(e, 10));
        assert(!st_engine_set_instrument_count(e, 0) && !st_engine_set_instrument_count(e, ST_INSTRUMENT_SANITY_LIMIT + 1));
        st_engine_commit_update(e);
        st_engine_note_on(e, 0, 5, 69, 100);
        assert(energy(e, 4800) > .01);
        st_engine_panic(e); energy(e, 512);
        st_engine_note_on(e, 0, 150, 69, 100);
        assert(energy(e, 4800) == 0);
    }
    std::puts("PASS shrinking keeps remaining instruments and rejects removed ones"); ++groups;
    std::printf("Instrument count: %d groups passed\n", groups);
}

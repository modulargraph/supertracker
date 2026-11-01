// Choke groups and voice modes as per-track properties (st_engine_set_track_voice): an open hat on
// one track chokes a closed hat on another track of the same group, never a track of another group,
// and a track's voice mode overrides its instrument's.
#include "TrackerEngine.h"
#include "engine_test_tone.hpp"
#include <cassert>
#include <cstdio>
#include <vector>

struct Engine { STEngine *e = st_engine_create(48000); Engine() { assert(e); } ~Engine() { st_engine_destroy(e); } operator STEngine *() { return e; } };
static double energy(STEngine *e, size_t frames) {
    std::vector<float> l(frames), r(frames);
    st_engine_render(e, l.data(), r.data(), uint32_t(frames));
    double sum = 0; for (size_t i = 0; i < frames; ++i) sum += double(l[i]) * l[i];
    return sum / double(frames);
}

// Three tracks, one sustained tone instrument each. Voice settings: {choke, scope, mode} per track.
static void setup(Engine &e, const int (*voice)[3]) {
    st_engine_begin_update(e);
    assert(st_engine_set_track_count(e, 3) && st_engine_set_instrument_count(e, 3));
    st_engine_set_timing(e, 120, 4, 0, 1);
    for (int t = 0; t < 3; ++t) {
        auto track = st_default_track(); track.instrument = t; track.volume = 1;
        st_engine_set_track(e, t, track);
        auto p = testToneInstrument(); p.attack = 0; p.sustain = 1; p.release = .001f; p.volume = .5f; p.mode = ST_MODE_FORWARD;
        st_engine_set_instrument(e, t, p); loadTestTone(e, t);
        if (voice) assert(st_engine_set_track_voice(e, t, voice[t][0], voice[t][1], voice[t][2]));
    }
    st_engine_commit_update(e);
}

int main() {
    int groups = 0;
    {
        Engine e;
        assert(!st_engine_set_track_voice(e, -1, 1, 0, -1) && !st_engine_set_track_voice(e, 0, 200, 0, -1) && !st_engine_set_track_voice(e, 0, 1, -1, -1)
               && !st_engine_set_track_voice(e, 0, 1, 0, 3) && !st_engine_set_track_voice(e, ST_TRACK_SANITY_LIMIT, 1, 0, -1));
    }
    std::puts("PASS invalid track voice settings are rejected"); ++groups;
    {
        // Tracks 0 and 1 share choke group 1 in group scope 1; track 2 has choke 1 in another group.
        const int voice[3][3] = {{1, 1, -1}, {1, 1, -1}, {1, 2, -1}};
        Engine e; setup(e, voice);
        st_engine_note_on(e, 0, 0, 69, 100);           // closed hat
        st_engine_note_on(e, 2, 2, 69, 100);           // another group's hat
        double both = energy(e, 4800);
        assert(both > .01);
        st_engine_note_on(e, 1, 1, 69, 100);           // open hat chokes the closed hat on track 0
        energy(e, 2400);
        double after = energy(e, 4800);
        // Two voices remain (track 1 and track 2): the level matches two tones, not three.
        Engine reference; setup(reference, nullptr);
        st_engine_note_on(reference, 1, 1, 69, 100); st_engine_note_on(reference, 2, 2, 69, 100);
        energy(reference, 2400);
        double two = energy(reference, 4800);
        assert(std::abs(after - two) < two * .05);
        std::printf("  energy: two voices %.4f, after choke %.4f\n", two, after);
    }
    std::puts("PASS a track choke group stops another track of the same group, not another group"); ++groups;
    {
        // Mono: a chord (two notes on the same frame) stays whole; a later note cuts both.
        const int voice[3][3] = {{0, 0, 1}, {0, 0, -1}, {0, 0, -1}};
        Engine e; setup(e, voice);
        st_engine_note_on(e, 0, 0, 69, 100); st_engine_note_on(e, 0, 0, 76, 100);
        energy(e, 2400); double chord = energy(e, 4800);
        Engine one; setup(one, voice); st_engine_note_on(one, 0, 0, 69, 100); energy(one, 2400); double single = energy(one, 4800);
        assert(chord > single * 1.5);
        st_engine_note_on(e, 0, 0, 72, 100); energy(e, 2400); double after = energy(e, 4800);
        assert(std::abs(after - single) < single * .1);
        std::printf("  energy: one %.4f, mono chord %.4f, after the next note %.4f\n", single, chord, after);
    }
    std::puts("PASS mono keeps a chord whole and cuts the earlier notes on the next note"); ++groups;
    {
        // No choke group: all three keep sounding (the reference for three voices).
        Engine e; setup(e, nullptr);
        st_engine_note_on(e, 0, 0, 69, 100); st_engine_note_on(e, 1, 1, 69, 100); st_engine_note_on(e, 2, 2, 69, 100);
        energy(e, 2400);
        double three = energy(e, 4800);
        Engine m; const int mono[3][3] = {{0, 0, 1}, {0, 0, -1}, {0, 0, -1}}; setup(m, mono);
        st_engine_note_on(m, 0, 0, 69, 100); st_engine_note_on(m, 0, 0, 69, 100); st_engine_note_on(m, 0, 0, 69, 100);
        energy(m, 2400);
        double monoEnergy = energy(m, 4800);
        Engine one; setup(one, nullptr);
        st_engine_note_on(one, 0, 0, 69, 100);
        energy(one, 2400);
        double single = energy(one, 4800);
        assert(three > single * 2);
        assert(std::abs(monoEnergy - single) < single * .05);
        std::printf("  energy: one %.4f, mono after three notes %.4f, three tracks %.4f\n", single, monoEnergy, three);
    }
    std::puts("PASS a mono track keeps one voice; without settings tracks are independent"); ++groups;
    std::printf("track choke: %d groups passed\n", groups);
}

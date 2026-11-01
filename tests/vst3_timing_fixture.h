#ifndef ST_VST3_TIMING_FIXTURE_H
#define ST_VST3_TIMING_FIXTURE_H
#include <stdint.h>

#define ST_TIMING_FIXTURE_CLASS_ID "53A65D54CFD64B728ECE31B97A729C04"
#define ST_TIMING_FIXTURE_CAPACITY 16384u
#define ST_TIMING_FIXTURE_SNAPSHOT_VERSION 1u

enum {
    ST_TIMING_PLAYING = 1u << 1,
    ST_TIMING_CYCLE_ACTIVE = 1u << 2,
    ST_TIMING_RECORDING = 1u << 3,
    ST_TIMING_PROJECT_MUSIC_VALID = 1u << 9,
    ST_TIMING_TEMPO_VALID = 1u << 10,
    ST_TIMING_BAR_VALID = 1u << 11,
    ST_TIMING_CYCLE_VALID = 1u << 12,
    ST_TIMING_TIME_SIG_VALID = 1u << 13,
    ST_TIMING_CONTINUOUS_VALID = 1u << 17
};

/* Fixed C ABI, 88 bytes on supported macOS arm64/x86_64 targets. Coordinates
 * are copied verbatim from the actual processor call, without interpolation. */
typedef struct STTimingFixtureSnapshot {
    uint32_t processFrames;
    uint32_t state;
    double sampleRate;
    int64_t projectSamples;
    int64_t continuousSamples;
    double musicalBeat;
    double tempo;
    int32_t numerator;
    int32_t denominator;
    double barBeat;
    double cycleStartBeat;
    double cycleEndBeat;
    uint32_t contextPresent;
    uint32_t reserved;
} STTimingFixtureSnapshot;

#ifdef __cplusplus
extern "C" {
#endif

/* All inspection/configuration calls require rendering to be quiescent. These
 * symbols are resolved with dlopen/dlsym; the host does not link the test bundle.
 * One fixed process-wide trace records calls from this original fixture only. */
void st_timing_fixture_reset(void); /* Trace only; never resets notes/delay. */
uint32_t st_timing_fixture_count(void);
uint32_t st_timing_fixture_dropped(void);
int st_timing_fixture_copy(uint32_t index, STTimingFixtureSnapshot *out);
/* Accepts 0 or 127 frames only. Refuses changes while any component exists. */
int st_timing_fixture_set_delay(uint32_t frames);

#ifdef __cplusplus
}
#endif
#endif

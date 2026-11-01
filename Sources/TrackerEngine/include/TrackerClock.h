#ifndef SUPERTRACKER_CLOCK_H
#define SUPERTRACKER_CLOCK_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

/* Immutable timing foundation; optionally adopted by TrackerEngine playback.
   Creation/destruction are control-thread operations. Queries allocate nothing,
   take no locks and never mutate state. Destroy only after readers are quiescent. */
typedef struct STClockMap STClockMap;
typedef struct STClockTimeline STClockTimeline;
typedef struct { double beat,bpm; } STClockTempoEvent;
typedef struct { double beat; int32_t numerator,denominator; } STClockMeterEvent;
enum { ST_CLOCK_OK=1, ST_CLOCK_INVALID=0, ST_CLOCK_OUTSIDE_DOMAIN=-1 };
enum { ST_CLOCK_PLAYING=1, ST_CLOCK_LOOPING=2, ST_CLOCK_PREROLL=4, ST_CLOCK_DOMAIN_END=8,
       ST_CLOCK_RECORDING=16, ST_CLOCK_GRID_PROJECTION=32 };
enum { ST_CLOCK_BOUNDARY_TEMPO=1, ST_CLOCK_BOUNDARY_METER=2, ST_CLOCK_BOUNDARY_BAR=4,
       ST_CLOCK_BOUNDARY_LOOP=8, ST_CLOCK_BOUNDARY_ORIGIN=16, ST_CLOCK_BOUNDARY_DOMAIN=32 };
enum { ST_CLOCK_MAX_EVENTS=1024, ST_CLOCK_MAX_SPAN=65536 };

typedef struct {
    int64_t graphOriginFrame;
    double startBeat;
    int32_t playing,loopEnabled;
    double loopStartBeat,loopEndBeat;
    uint64_t epoch;
} STClockOrigin;

typedef struct {
    uint64_t revision,epoch;
    int64_t graphFrame,contentGraphFrame,songFrame,loopIteration,bar;
    double sampleRate,beat,bpm,barStartBeat,barEndBeat;
    int32_t numerator,denominator,flags;
    int64_t nextBoundaryFrame; /* Graph-frame coordinate; INT64_MAX when stopped. */
    uint32_t nextBoundaryKinds;
    double loopStartBeat,loopEndBeat;
    int64_t nextQuarterBeatFrame; /* Graph coordinate; INT64_MAX if unreachable. */
    /* Musical-grid projection in graph coordinates, integrated through the
       immutable unwrapped map even when stopped or a short loop cannot reach
       an integer beat. This is a distance metric, not a transport prediction.
       ST_CLOCK_GRID_PROJECTION marks an unavailable strict reachable target;
       adapters may then use this field for their documented grid convention. */
    int64_t nextQuarterGridFrame;
} STClockContext;

typedef struct {
    STClockContext context;
    uint32_t frames; /* Positive except at the inclusive map endpoint. */
} STClockSpan;

/* 1..1024 ascending unique events of each kind, both beginning at beat 0.
   Beats 0..1000000, BPM 20..400, meter 1..16 with denominator 2/4/8/16,
   sampleRate 8000..384000. Same-position tempo+meter events are atomic.
   Distinct positions collapsing onto one sample are rejected. Error text is
   copied to caller storage (optional); no partial object is returned on error. */
STClockMap *st_clock_map_create(double sampleRate,const STClockTempoEvent *tempos,uint32_t tempoCount,
    const STClockMeterEvent *meters,uint32_t meterCount,uint64_t revision,char *error,uint32_t errorCapacity);
void st_clock_map_destroy(STClockMap *map);

/* Absolute ceil-to-sample: first sample not earlier than the mapped event.
   No epsilon snapping. Continuous seconds law follows the original TimingMap.
   Continuous beats generally quantize: inverse at the returned sample can be
   less than one sample later. Sample-domain endpoints are inclusive; their
   beat is exactly 1000000. Negative frames are valid initial-tempo preroll.
   On error, output values are left untouched. */
int32_t st_clock_frame_at_beat(const STClockMap *map,double beat,int64_t *frame);
int32_t st_clock_beat_at_frame(const STClockMap *map,int64_t frame,double *beat);
int32_t st_clock_seconds_at_beat(const STClockMap *map,double beat,double *seconds);
int32_t st_clock_beat_at_seconds(const STClockMap *map,double seconds,double *beat);

/* Timeline owns a map reference; the caller may destroy its STClockMap handle.
   Graph origin is within +/-2^52 frames. A seek quantizes startBeat once.
   Loops require 0<=start<end<=1000000 and startBeat<loopEndBeat; an intro before
   loopStart is allowed. Quantized loops repeat [startFrame,endFrame) exactly.
   Queries before graphOrigin do not wrap backwards and are marked preroll.
   A stopped timeline holds its quantized seek position and never advances. */
STClockTimeline *st_clock_timeline_create(const STClockMap *map,STClockOrigin origin,char *error,uint32_t errorCapacity);
void st_clock_timeline_destroy(STClockTimeline *timeline);

/* graphFrame is continuous graph time, within +/-2^52. upstreamLatencyFrames is
   0..2^30 and is subtracted before musical lookup; own output latency is not
   subtracted. Negative song time extrapolates initial tempo/meter and uses bar 0,
   then negative bar numbers. All next-boundary flags at one frame are combined.
   Span ends at tempo/meter/bar/loop/origin/domain boundaries. Queries do not
   render audio, consume events, apply pending commands, or access live status.
   requestedFrames must be 1..65536. The host owns cached-range retention and
   publishing/replacing timeline objects at quiescent render boundaries. */
int32_t st_clock_context(const STClockTimeline *timeline,int64_t graphFrame,int64_t upstreamLatencyFrames,STClockContext *context);
int32_t st_clock_span(const STClockTimeline *timeline,int64_t graphFrame,uint32_t requestedFrames,int64_t upstreamLatencyFrames,STClockSpan *span);

#ifdef __cplusplus
}
#endif
#endif

#ifndef SUPERTRACKER_UNIT_RANGE_H
#define SUPERTRACKER_UNIT_RANGE_H
#include <stdint.h>
#include "TrackerClock.h"
#ifdef __cplusplus
extern "C" {
#endif

typedef struct STUnitRangeCache STUnitRangeCache;

/* Cache-specific errors are separate from callback OSStatus values. A nonzero
   prepare/process result is returned verbatim. A failed process result remains
   latched until reset; no later request may retry a partially advanced processor. */
enum {
    ST_UNIT_RANGE_OK=0,
    ST_UNIT_RANGE_INVALID=-74000,
    ST_UNIT_RANGE_OVERSIZE=-74001,
    ST_UNIT_RANGE_OVERFLOW=-74002,
    ST_UNIT_RANGE_GAP=-74003,
    ST_UNIT_RANGE_EXPIRED=-74004,
    ST_UNIT_RANGE_BUSY=-74005,
    ST_UNIT_RANGE_CLOCK_ERROR=-74006,
    ST_UNIT_RANGE_DOMAIN_END=-74007,
    ST_UNIT_RANGE_CONTEXT_MISMATCH=-74008,
    ST_UNIT_RANGE_SPAN_LIMIT=-74009
};

/* All callbacks execute synchronously on the calling render thread and must not
   throw, allocate, wait or mutate the cache. prepare is optional; span/process
   are required. Only the uncached suffix is prepared/processed. prepare extends
   the native audio/context/event caches first, but must not send events to or
   advance this child. Complete span preflight then finishes before any process
   call or per-child event delivery. Each query requests at most maximumChildFrames.
   Cached-only calls need none of these callbacks. */
typedef int32_t (*STUnitRangePrepareFn)(void *user,int64_t graphFrame,uint32_t frames);
typedef int32_t (*STUnitRangeSpanFn)(void *user,int64_t graphFrame,uint32_t frames,STClockSpan *span);
typedef int32_t (*STUnitRangeProcessFn)(void *user,const STClockContext *context,
    int64_t graphFrame,uint32_t frames,float *const *output);

typedef struct {
    uint64_t requests,successfulRequests,cacheHits,preparedFrames,processedFrames,processCalls;
    int64_t retainedStartFrame,renderedEndFrame;
    uint64_t revision,epoch;
    uint32_t retainedFrames,anchored;
    int32_t lastError,latchedError;
} STUnitRangeStats;

/* Control-thread allocation. channels=1/2, maximumRequestFrames=1..65536,
   historyFrames=maximumRequestFrames..131072, maximumChildFrames=1..maximumRequestFrames,
   maximumSpans=1..256. maximumSpans counts actual child calls, including splitting
   at maximumChildFrames, and must cover ceil(maximumRequestFrames/maximumChildFrames).
   Error text is optional and always NUL terminated. */
STUnitRangeCache *st_unit_range_create(uint32_t channels,uint32_t maximumRequestFrames,
    uint32_t historyFrames,uint32_t maximumChildFrames,uint32_t maximumSpans,
    char *error,uint32_t errorCapacity);
/* Destroy only when callbacks are quiescent. Reset also clears statistics. */
void st_unit_range_destroy(STUnitRangeCache *cache);
int32_t st_unit_range_reset(STUnitRangeCache *cache);

/* First success anchors any clock-valid graph frame. Further requests may repeat
   retained ranges or overlap the retained prefix and append a contiguous suffix.
   Gaps, expired history, bad spans, changed content offset/sample rate, and
   arithmetic overflow fail without changing good cached PCM. Cached-only
   requests invoke no callbacks, including after a caller replaces its clock.
   Revision/epoch changes at span boundaries are forwarded to the child, including
   within one request; they do not invalidate old PCM. Changing sample rate or
   upstream content-frame offset requires a quiescent reset.
   Spans must stay within their context's nextBoundaryFrame and carry finite
   beat/bar/tempo values, positive tempo/numerator, and denominator 2/4/8/16.

   Output is planar, one valid pointer per channel, with frames writable samples.
   Failures clear output; invalid oversized calls clear only maximumRequestFrames
   samples per valid channel, keeping rejection work bounded. A null cache cannot
   determine channels and therefore does not touch output. Concurrent/reentrant
   render is rejected immediately. No render-time allocation, locks or waiting. */
int32_t st_unit_range_render(STUnitRangeCache *cache,int64_t graphFrame,uint32_t frames,
    float *const *output,STUnitRangePrepareFn prepare,STUnitRangeSpanFn span,
    STUnitRangeProcessFn process,void *user);

/* Bounded snapshot; returns BUSY without touching stats when rendering is active. */
int32_t st_unit_range_get_stats(STUnitRangeCache *cache,STUnitRangeStats *stats);

#ifdef __cplusplus
}
#endif
#endif

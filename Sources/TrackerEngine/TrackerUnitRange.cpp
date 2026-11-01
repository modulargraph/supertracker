#include "include/TrackerUnitRange.h"
#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstring>
#include <limits>
#include <memory>
#include <new>

struct STUnitRangeCache {
    uint32_t channels=0,maximumRequest=0,historyFrames=0,maximumChild=0,maximumSpans=0;
    uint32_t writePosition=0;
    int64_t contentOffset=0;
    double sampleRate=0;
    std::atomic_flag rendering=ATOMIC_FLAG_INIT;
    STUnitRangeStats stats{};
    std::unique_ptr<float[]> history,scratch;
    std::unique_ptr<STClockSpan[]> plan;
};

namespace {
struct RenderGuard {
    STUnitRangeCache &cache;
    bool owns;
    explicit RenderGuard(STUnitRangeCache &value) noexcept
        :cache(value),owns(!value.rendering.test_and_set(std::memory_order_acquire)){}
    ~RenderGuard(){if(owns)cache.rendering.clear(std::memory_order_release);}
};

void writeError(char *output,uint32_t capacity,const char *message) noexcept {
    if(!output||!capacity)return;
    const size_t count=std::min<size_t>(std::strlen(message),capacity-1);
    std::memcpy(output,message,count);output[count]='\0';
}

void silence(const STUnitRangeCache &cache,uint32_t frames,float *const *output) noexcept {
    if(!output)return;
    const uint32_t bounded=std::min(frames,cache.maximumRequest);
    for(uint32_t channel=0;channel<cache.channels;++channel)
        if(output[channel])std::fill_n(output[channel],bounded,0.f);
}

int32_t fail(STUnitRangeCache &cache,int32_t error,uint32_t frames,float *const *output) noexcept {
    cache.stats.lastError=error;
    silence(cache,frames,output);
    return error;
}

bool validOutput(const STUnitRangeCache &cache,float *const *output) noexcept {
    if(!output)return false;
    for(uint32_t channel=0;channel<cache.channels;++channel)if(!output[channel])return false;
    return true;
}

void copyCached(const STUnitRangeCache &cache,int64_t start,uint32_t frames,float *const *output) noexcept {
    const uint64_t offset=uint64_t(start)-uint64_t(cache.stats.retainedStartFrame);
    const uint32_t oldest=(cache.writePosition+cache.historyFrames-cache.stats.retainedFrames)%cache.historyFrames;
    const uint32_t position=uint32_t((oldest+offset)%cache.historyFrames);
    const uint32_t first=std::min(frames,cache.historyFrames-position);
    for(uint32_t channel=0;channel<cache.channels;++channel){
        const float *source=cache.history.get()+size_t(channel)*cache.historyFrames;
        std::memcpy(output[channel],source+position,size_t(first)*sizeof(float));
        if(first<frames)std::memcpy(output[channel]+first,source,size_t(frames-first)*sizeof(float));
    }
}

void commit(STUnitRangeCache &cache,int64_t end,uint32_t frames,const STClockContext &last,
            double sampleRate,int64_t contentOffset) noexcept {
    const uint32_t first=std::min(frames,cache.historyFrames-cache.writePosition);
    for(uint32_t channel=0;channel<cache.channels;++channel){
        float *destination=cache.history.get()+size_t(channel)*cache.historyFrames;
        const float *source=cache.scratch.get()+size_t(channel)*cache.maximumRequest;
        std::memcpy(destination+cache.writePosition,source,size_t(first)*sizeof(float));
        if(first<frames)std::memcpy(destination,source+first,size_t(frames-first)*sizeof(float));
    }
    cache.writePosition=(cache.writePosition+frames)%cache.historyFrames;
    cache.stats.retainedFrames=std::min(cache.historyFrames,cache.stats.retainedFrames+frames);
    cache.stats.renderedEndFrame=end;
    // Retention can never precede the first successfully anchored graph frame, so
    // this subtraction cannot underflow even for an anchor near INT64_MIN.
    cache.stats.retainedStartFrame=end-int64_t(cache.stats.retainedFrames);
    cache.stats.anchored=1;
    cache.stats.revision=last.revision;cache.stats.epoch=last.epoch;
    cache.sampleRate=sampleRate;cache.contentOffset=contentOffset;
}
}

extern "C" STUnitRangeCache *st_unit_range_create(uint32_t channels,uint32_t maximumRequestFrames,
    uint32_t historyFrames,uint32_t maximumChildFrames,uint32_t maximumSpans,
    char *error,uint32_t errorCapacity){
    writeError(error,errorCapacity,"");
    if(channels<1||channels>2){writeError(error,errorCapacity,"Unit cache requires one or two channels.");return nullptr;}
    if(maximumRequestFrames<1||maximumRequestFrames>65536){writeError(error,errorCapacity,"Unit request capacity must be 1..65536 frames.");return nullptr;}
    if(historyFrames<maximumRequestFrames||historyFrames>131072){writeError(error,errorCapacity,"Unit history must cover one maximum request and not exceed 131072 frames.");return nullptr;}
    if(maximumChildFrames<1||maximumChildFrames>maximumRequestFrames){writeError(error,errorCapacity,"Child frame limit must fit the request capacity.");return nullptr;}
    if(maximumSpans<1||maximumSpans>256){writeError(error,errorCapacity,"Unit span capacity must be 1..256.");return nullptr;}
    if(maximumSpans<(maximumRequestFrames+maximumChildFrames-1)/maximumChildFrames){
        writeError(error,errorCapacity,"Unit span capacity cannot cover the maximum request at the child frame limit.");return nullptr;
    }
    try{
        auto cache=std::make_unique<STUnitRangeCache>();
        cache->channels=channels;cache->maximumRequest=maximumRequestFrames;
        cache->historyFrames=historyFrames;cache->maximumChild=maximumChildFrames;cache->maximumSpans=maximumSpans;
        cache->history=std::make_unique<float[]>(size_t(channels)*historyFrames);
        cache->scratch=std::make_unique<float[]>(size_t(channels)*maximumRequestFrames);
        cache->plan=std::make_unique<STClockSpan[]>(maximumSpans);
        return cache.release();
    }catch(...){writeError(error,errorCapacity,"Could not allocate bounded unit render storage.");return nullptr;}
}

extern "C" void st_unit_range_destroy(STUnitRangeCache *cache){delete cache;}

extern "C" int32_t st_unit_range_reset(STUnitRangeCache *cache){
    if(!cache)return ST_UNIT_RANGE_INVALID;
    RenderGuard guard(*cache);if(!guard.owns)return ST_UNIT_RANGE_BUSY;
    cache->stats={};cache->writePosition=0;cache->contentOffset=0;cache->sampleRate=0;
    return ST_UNIT_RANGE_OK;
}

extern "C" int32_t st_unit_range_get_stats(STUnitRangeCache *cache,STUnitRangeStats *stats){
    if(!cache||!stats)return ST_UNIT_RANGE_INVALID;
    RenderGuard guard(*cache);if(!guard.owns)return ST_UNIT_RANGE_BUSY;
    *stats=cache->stats;
    return ST_UNIT_RANGE_OK;
}

extern "C" int32_t st_unit_range_render(STUnitRangeCache *cache,int64_t graphFrame,uint32_t frames,
    float *const *output,STUnitRangePrepareFn prepare,STUnitRangeSpanFn span,
    STUnitRangeProcessFn process,void *user){
    if(!cache)return ST_UNIT_RANGE_INVALID;
    RenderGuard guard(*cache);
    if(!guard.owns){silence(*cache,frames,output);return ST_UNIT_RANGE_BUSY;}
    ++cache->stats.requests;
    if(!frames||!validOutput(*cache,output))return fail(*cache,ST_UNIT_RANGE_INVALID,frames,output);
    if(frames>cache->maximumRequest)return fail(*cache,ST_UNIT_RANGE_OVERSIZE,frames,output);
    if(graphFrame>std::numeric_limits<int64_t>::max()-int64_t(frames))
        return fail(*cache,ST_UNIT_RANGE_OVERFLOW,frames,output);
    if(cache->stats.latchedError)return fail(*cache,cache->stats.latchedError,frames,output);
    const int64_t end=graphFrame+int64_t(frames);
    int64_t suffixStart=graphFrame;
    if(cache->stats.anchored){
        if(graphFrame<cache->stats.retainedStartFrame)return fail(*cache,ST_UNIT_RANGE_EXPIRED,frames,output);
        if(graphFrame>cache->stats.renderedEndFrame)return fail(*cache,ST_UNIT_RANGE_GAP,frames,output);
        if(end<=cache->stats.renderedEndFrame){
            copyCached(*cache,graphFrame,frames,output);
            ++cache->stats.cacheHits;++cache->stats.successfulRequests;
            cache->stats.lastError=ST_UNIT_RANGE_OK;
            return ST_UNIT_RANGE_OK;
        }
        suffixStart=cache->stats.renderedEndFrame;
    }
    if(!span||!process)return fail(*cache,ST_UNIT_RANGE_INVALID,frames,output);
    const uint32_t suffixFrames=uint32_t(end-suffixStart);

    // The native provider must first retain the new audio/clock/event range. This
    // is deliberately separate from per-child event dispatch in process below.
    if(prepare){
        const int32_t result=prepare(user,suffixStart,suffixFrames);
        if(result!=0)return fail(*cache,result,frames,output);
        cache->stats.preparedFrames+=suffixFrames;
    }

    uint32_t spanCount=0,planned=0;
    double sampleRate=cache->sampleRate;
    int64_t contentOffset=cache->contentOffset;
    bool haveFormat=cache->stats.anchored!=0;
    while(planned<suffixFrames){
        if(spanCount==cache->maximumSpans)return fail(*cache,ST_UNIT_RANGE_SPAN_LIMIT,frames,output);
        const int64_t cursor=suffixStart+int64_t(planned);
        const uint32_t requested=std::min(suffixFrames-planned,cache->maximumChild);
        STClockSpan &part=cache->plan[spanCount];part={};
        const int32_t result=span(user,cursor,requested,&part);
        if(result!=ST_CLOCK_OK)return fail(*cache,result==ST_CLOCK_OUTSIDE_DOMAIN?
            ST_UNIT_RANGE_DOMAIN_END:ST_UNIT_RANGE_CLOCK_ERROR,frames,output);
        if(!part.frames)return fail(*cache,ST_UNIT_RANGE_DOMAIN_END,frames,output);
        if(part.frames>requested)return fail(*cache,ST_UNIT_RANGE_CONTEXT_MISMATCH,frames,output);
        const STClockContext &context=part.context;
        if(context.graphFrame!=cursor||!std::isfinite(context.sampleRate)||context.sampleRate<=0||
           context.contentGraphFrame>context.graphFrame||context.nextBoundaryFrame<cursor+int64_t(part.frames)||
           !std::isfinite(context.bpm)||context.bpm<=0||!std::isfinite(context.beat)||
           !std::isfinite(context.barStartBeat)||!std::isfinite(context.barEndBeat)||context.numerator<=0||
           (context.denominator!=2&&context.denominator!=4&&context.denominator!=8&&context.denominator!=16))
            return fail(*cache,ST_UNIT_RANGE_CONTEXT_MISMATCH,frames,output);
        const uint64_t offset=uint64_t(context.graphFrame)-uint64_t(context.contentGraphFrame);
        if(offset>(uint64_t(1)<<30))return fail(*cache,ST_UNIT_RANGE_CONTEXT_MISMATCH,frames,output);
        if(!haveFormat){sampleRate=context.sampleRate;contentOffset=int64_t(offset);haveFormat=true;}
        else if(context.sampleRate!=sampleRate||int64_t(offset)!=contentOffset)
            return fail(*cache,ST_UNIT_RANGE_CONTEXT_MISMATCH,frames,output);
        planned+=part.frames;++spanCount;
    }

    // Cache publication is transactional. A child may already have advanced when
    // it reports failure, so its error is latched; scratch PCM is never committed.
    for(uint32_t channel=0;channel<cache->channels;++channel)
        std::fill_n(cache->scratch.get()+size_t(channel)*cache->maximumRequest,suffixFrames,0.f);
    uint32_t processed=0;
    for(uint32_t index=0;index<spanCount;++index){
        const STClockSpan &part=cache->plan[index];
        float *buffers[2]={cache->scratch.get()+processed,nullptr};
        if(cache->channels==2)buffers[1]=cache->scratch.get()+cache->maximumRequest+processed;
        ++cache->stats.processCalls;
        const int32_t result=process(user,&part.context,part.context.graphFrame,part.frames,buffers);
        if(result!=0){cache->stats.latchedError=result;return fail(*cache,result,frames,output);}
        cache->stats.processedFrames+=part.frames;
        processed+=part.frames;
    }
    commit(*cache,end,suffixFrames,cache->plan[spanCount-1].context,sampleRate,contentOffset);
    copyCached(*cache,graphFrame,frames,output);
    ++cache->stats.successfulRequests;cache->stats.lastError=ST_UNIT_RANGE_OK;
    return ST_UNIT_RANGE_OK;
}

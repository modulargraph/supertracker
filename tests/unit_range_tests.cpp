#include "TrackerUnitRange.h"
#include <algorithm>
#include <array>
#include <atomic>
#include <cassert>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <new>
#include <thread>
#include <vector>

static thread_local bool checkingRenderAllocations=false;
static std::atomic<uint64_t> renderAllocations{0};
void *operator new(std::size_t size){
    if(checkingRenderAllocations)++renderAllocations;
    if(void *result=std::malloc(size?size:1))return result;
    throw std::bad_alloc();
}
void *operator new[](std::size_t size){return ::operator new(size);}
void operator delete(void *value)noexcept{std::free(value);}
void operator delete[](void *value)noexcept{std::free(value);}

struct Call {
    int64_t frame;
    uint32_t frames;
    uint64_t revision,epoch;
};

struct Fixture {
    enum class BadSpan { none,zero,tooLong,coordinate,rateNaN,rateZero,negativeOffset,hugeOffset,
        boundary,tempoNaN,tempoZero,beatNaN,barStartNaN,barEndNaN,numerator,denominator };
    STUnitRangeCache *cache;
    uint32_t channels,capacity;
    std::vector<float> left,right;
    float *output[2];
    uint64_t prepares=0,queries=0,processes=0,events=0,advanced=0;
    int64_t preparedStart=0,preparedEnd=0,plannedEnd=0;
    bool processingStarted=false,tagContext=false,requirePrepare=true;
    int32_t prepareError=0,spanError=ST_CLOCK_OK,processError=-12345;
    uint64_t failProcess=0;
    BadSpan bad=BadSpan::none;
    double rate=48000;
    double laterRate=0;
    int64_t offset=0,partition=0;
    int64_t laterOffset=-1;
    int64_t revisionBoundary=std::numeric_limits<int64_t>::max();
    uint64_t revision=1,epoch=1,newRevision=2,newEpoch=2;
    STClockTimeline *timeline=nullptr;
    std::array<Call,1024> queryTrace{},processTrace{};
    uint32_t queryTraceCount=0,processTraceCount=0;
    bool reenter=false;
    std::atomic<bool> pauseProcess{false},processEntered{false},releaseProcess{false};

    Fixture(uint32_t request=16384,uint32_t history=32768,uint32_t child=4096,
            uint32_t spans=64,uint32_t channelCount=2)
        :cache(st_unit_range_create(channelCount,request,history,child,spans,nullptr,0)),
         channels(channelCount),capacity(request),left(request+16,99.f),right(request+16,99.f),
         output{left.data(),right.data()}{assert(cache);}
    ~Fixture(){st_unit_range_destroy(cache);}
    Fixture(const Fixture&)=delete;
    Fixture&operator=(const Fixture&)=delete;

    static int32_t prepare(void *user,int64_t start,uint32_t frames){
        auto &self=*static_cast<Fixture*>(user);
        ++self.prepares;
        self.preparedStart=start;self.preparedEnd=start+frames;self.plannedEnd=start;
        self.processingStarted=false;
        return self.prepareError;
    }
    static int32_t span(void *user,int64_t start,uint32_t frames,STClockSpan *result){
        auto &self=*static_cast<Fixture*>(user);
        ++self.queries;
        assert(!self.processingStarted); // The complete plan precedes every child call.
        if(self.requirePrepare){
            assert(start>=self.preparedStart&&start+frames<=self.preparedEnd);
            assert(start==self.plannedEnd);
        }
        if(self.spanError!=ST_CLOCK_OK)return self.spanError;
        if(self.timeline){
            const int32_t status=st_clock_span(self.timeline,start,frames,self.offset,result);
            if(status!=ST_CLOCK_OK)return status;
        }else{
            *result={};result->frames=frames;
            if(self.partition>0){
                const int64_t remainder=((start%self.partition)+self.partition)%self.partition;
                result->frames=std::min(frames,uint32_t(self.partition-remainder));
            }
            if(start<self.revisionBoundary&&uint64_t(self.revisionBoundary)-uint64_t(start)<result->frames)
                result->frames=uint32_t(uint64_t(self.revisionBoundary)-uint64_t(start));
            auto &context=result->context;
            const int64_t effectiveOffset=start>=self.revisionBoundary&&self.laterOffset>=0?self.laterOffset:self.offset;
            context.graphFrame=start;context.contentGraphFrame=start-effectiveOffset;
            context.songFrame=context.contentGraphFrame;
            context.sampleRate=start>=self.revisionBoundary&&self.laterRate>0?self.laterRate:self.rate;
            context.revision=start>=self.revisionBoundary?self.newRevision:self.revision;
            context.epoch=start>=self.revisionBoundary?self.newEpoch:self.epoch;
            context.beat=double(context.songFrame)/24000;context.bpm=120;
            context.numerator=4;context.denominator=4;context.flags=ST_CLOCK_PLAYING;
            context.nextBoundaryFrame=start+result->frames;
        }
        switch(self.bad){
            case BadSpan::none:break;
            case BadSpan::zero:result->frames=0;break;
            case BadSpan::tooLong:result->frames=frames+1;break;
            case BadSpan::coordinate:++result->context.graphFrame;break;
            case BadSpan::rateNaN:result->context.sampleRate=std::numeric_limits<double>::quiet_NaN();break;
            case BadSpan::rateZero:result->context.sampleRate=0;break;
            case BadSpan::negativeOffset:result->context.contentGraphFrame=start+1;break;
            case BadSpan::hugeOffset:result->context.contentGraphFrame=start-(int64_t(1)<<30)-1;break;
            case BadSpan::boundary:result->context.nextBoundaryFrame=start+result->frames-1;break;
            case BadSpan::tempoNaN:result->context.bpm=std::numeric_limits<double>::infinity();break;
            case BadSpan::tempoZero:result->context.bpm=0;break;
            case BadSpan::beatNaN:result->context.beat=std::numeric_limits<double>::quiet_NaN();break;
            case BadSpan::barStartNaN:result->context.barStartBeat=std::numeric_limits<double>::quiet_NaN();break;
            case BadSpan::barEndNaN:result->context.barEndBeat=std::numeric_limits<double>::infinity();break;
            case BadSpan::numerator:result->context.numerator=0;break;
            case BadSpan::denominator:result->context.denominator=3;break;
        }
        if(self.queryTraceCount<self.queryTrace.size())self.queryTrace[self.queryTraceCount++]={
            start,result->frames,result->context.revision,result->context.epoch};
        self.plannedEnd=start+result->frames;
        return ST_CLOCK_OK;
    }
    static int32_t process(void *user,const STClockContext *context,int64_t start,
                           uint32_t frames,float *const *output){
        auto &self=*static_cast<Fixture*>(user);
        if(self.requirePrepare)assert(self.plannedEnd==self.preparedEnd);
        assert(context->graphFrame==start);
        self.processingStarted=true;
        ++self.processes;++self.events; // Models per-child MIDI/parameter dispatch.
        if(self.processTraceCount<self.processTrace.size())self.processTrace[self.processTraceCount++]={
            start,frames,context->revision,context->epoch};
        if(self.reenter){
            self.reenter=false;
            float l[4]={1,1,1,1},r[4]={1,1,1,1};float *nested[2]={l,r};
            assert(st_unit_range_render(self.cache,start,4,nested,prepare,span,process,&self)==ST_UNIT_RANGE_BUSY);
            assert(std::all_of(l,l+4,[](float value){return value==0;}));
            if(self.channels==2)assert(std::all_of(r,r+4,[](float value){return value==0;}));
            STUnitRangeStats untouched{};untouched.requests=123;
            assert(st_unit_range_get_stats(self.cache,&untouched)==ST_UNIT_RANGE_BUSY&&untouched.requests==123);
            assert(st_unit_range_reset(self.cache)==ST_UNIT_RANGE_BUSY);
        }
        if(self.pauseProcess.load(std::memory_order_relaxed)){
            self.processEntered.store(true,std::memory_order_release);
            // Deliberate test-only stalled processor, so another thread can probe BUSY.
            while(!self.releaseProcess.load(std::memory_order_acquire))std::this_thread::yield();
        }
        const float tag=self.tagContext?float(context->revision*1000+context->epoch*10):0.f;
        for(uint32_t index=0;index<frames;++index){
            const float value=float(++self.advanced)+tag;
            output[0][index]=value;
            if(self.channels==2)output[1][index]=-2*value;
        }
        return self.failProcess==self.processes?self.processError:0;
    }
    int32_t render(int64_t start,uint32_t frames){
        std::fill(left.begin(),left.end(),99.f);std::fill(right.begin(),right.end(),99.f);
        checkingRenderAllocations=true;
        const int32_t status=st_unit_range_render(cache,start,frames,output,prepare,span,process,this);
        checkingRenderAllocations=false;
        return status;
    }
    STUnitRangeStats stats()const{
        STUnitRangeStats value{};assert(st_unit_range_get_stats(cache,&value)==0);return value;
    }
    void expectSequence(uint32_t frames,uint64_t first,float tag=0)const{
        for(uint32_t index=0;index<frames;++index){
            const float expected=float(first+index)+tag;
            assert(left[index]==expected);
            if(channels==2)assert(right[index]==-2*expected);
        }
        assert(left[frames]==99.f);
        if(channels==2)assert(right[frames]==99.f);
    }
    void expectSilent(uint32_t frames)const{
        for(uint32_t index=0;index<frames;++index){assert(left[index]==0);if(channels==2)assert(right[index]==0);}
    }
};

static void testOverlap4097AndCacheHits(){
    Fixture fixture;
    assert(fixture.render(0,4097)==0);fixture.expectSequence(4097,1);
    assert(fixture.processes==2&&fixture.processTrace[0].frames==4096&&fixture.processTrace[1].frames==1);
    assert(fixture.render(4096,4096)==0);fixture.expectSequence(4096,4097);
    assert(fixture.advanced==8192&&fixture.preparedStart==4097&&fixture.preparedEnd==8192);
    assert(fixture.processTrace[2].frame==4097&&fixture.processTrace[2].frames==4095);
    const auto calls=fixture.processes,queries=fixture.queries,prepares=fixture.prepares;
    assert(fixture.render(0,8192)==0);fixture.expectSequence(8192,1);
    assert(fixture.render(4096,4096)==0);fixture.expectSequence(4096,4097);
    assert(fixture.render(4000,100)==0);fixture.expectSequence(100,4001);
    assert(fixture.processes==calls&&fixture.queries==queries&&fixture.prepares==prepares);
    auto stats=fixture.stats();assert(stats.processedFrames==8192&&stats.preparedFrames==8192&&stats.cacheHits==3);
    assert(stats.retainedStartFrame==0&&stats.renderedEndFrame==8192&&stats.retainedFrames==8192);
    // A caller can satisfy a retained request after all clock/processor callbacks disappear.
    assert(st_unit_range_render(fixture.cache,0,10,fixture.output,nullptr,nullptr,nullptr,nullptr)==0);
    for(int i=0;i<10;++i)assert(fixture.left[i]==i+1);
}

static void testRingWrapAndRetention(){
    Fixture fixture(8,8,4,4);
    assert(fixture.render(100,8)==0);fixture.expectSequence(8,1);
    assert(fixture.render(104,8)==0);fixture.expectSequence(8,5);
    assert(fixture.render(110,8)==0);fixture.expectSequence(8,11);
    assert(fixture.render(114,8)==0);fixture.expectSequence(8,15);
    const auto count=fixture.processes;
    assert(fixture.render(113,1)==ST_UNIT_RANGE_EXPIRED);fixture.expectSilent(1);
    assert(fixture.render(123,1)==ST_UNIT_RANGE_GAP);fixture.expectSilent(1);
    assert(fixture.processes==count);
    assert(fixture.render(114,8)==0);fixture.expectSequence(8,15);
    assert(fixture.render(121,8)==0);fixture.expectSequence(8,22);
    auto stats=fixture.stats();assert(stats.retainedStartFrame==121&&stats.renderedEndFrame==129);
    assert(stats.retainedFrames==8&&stats.processedFrames==29&&stats.cacheHits==1);
    Fixture larger(8,13,3,8,1);
    for(int64_t frame=-100;frame<100;frame+=7){
        assert(larger.render(frame,7)==0);larger.expectSequence(7,uint64_t(frame+101));
        assert(larger.render(frame+1,4)==0);larger.expectSequence(4,uint64_t(frame+102));
    }
    assert(larger.stats().retainedFrames==13);
}

static void testFullPreflightBeforeEvents(){
    Fixture fixture(32,64,9,16);
    fixture.partition=7;
    assert(fixture.render(3,32)==0);fixture.expectSequence(32,1);
    assert(fixture.queries==5&&fixture.processes==5&&fixture.events==5);
    for(uint32_t index=0;index<fixture.processTraceCount;++index){
        const auto &call=fixture.processTrace[index];
        assert(call.frame==fixture.queryTrace[index].frame&&call.frames==fixture.queryTrace[index].frames);
        assert(call.frames<=7&&call.frame/7==(call.frame+call.frames-1)/7);
    }
    Fixture limited(16,32,8,2);
    assert(limited.render(0,8)==0);limited.expectSequence(8,1);
    limited.partition=1;
    assert(limited.render(4,12)==ST_UNIT_RANGE_SPAN_LIMIT);limited.expectSilent(12);
    assert(limited.processes==1&&limited.events==1&&limited.advanced==8);
    auto stats=limited.stats();assert(stats.renderedEndFrame==8&&stats.latchedError==0&&stats.preparedFrames==16);
    assert(limited.render(0,8)==0);limited.expectSequence(8,1);
    limited.partition=0;
    assert(limited.render(4,12)==0);limited.expectSequence(12,5);
    assert(limited.processes==2&&limited.events==2&&limited.preparedStart==8&&limited.preparedEnd==16);
}

static void testClockReplacementAndEpochBoundaries(){
    Fixture fixture(32,64,8,16);
    fixture.tagContext=true;fixture.revision=7;fixture.epoch=11;
    fixture.revisionBoundary=13;fixture.newRevision=8;fixture.newEpoch=12;
    assert(fixture.render(0,24)==0);
    for(int index=0;index<24;++index){
        const float expected=float(index+1)+(index<13?7110.f:8120.f);
        assert(fixture.left[index]==expected&&fixture.right[index]==-2*expected);
    }
    assert(fixture.processTrace[0].revision==7&&fixture.processTrace[1].frames==5);
    assert(fixture.processTrace[2].frame==13&&fixture.processTrace[2].revision==8&&fixture.processTrace[2].epoch==12);
    const auto queries=fixture.queries,processes=fixture.processes;
    // Replace the provider's map/origin entirely. Retained samples keep their old interpretation.
    fixture.revisionBoundary=0;fixture.newRevision=9;fixture.newEpoch=99;
    assert(fixture.render(8,16)==0);
    for(int index=0;index<16;++index)assert(fixture.left[index]==float(index+9)+(index<5?7110.f:8120.f));
    assert(fixture.queries==queries&&fixture.processes==processes);
    assert(fixture.render(20,12)==0);
    for(int index=0;index<12;++index)assert(fixture.left[index]==float(index+21)+(index<4?8120.f:9990.f));
    assert(fixture.preparedStart==24&&fixture.processTrace[processes].revision==9&&fixture.processTrace[processes].epoch==99);
    auto stats=fixture.stats();assert(stats.revision==9&&stats.epoch==99&&stats.processedFrames==32);
}

static void testPrepareAndClockFailuresRetry(){
    Fixture fixture(16,32,4,16);
    assert(fixture.render(0,8)==0);fixture.expectSequence(8,1);
    const auto queries=fixture.queries,processes=fixture.processes;
    fixture.prepareError=-901;
    assert(fixture.render(4,12)==-901);fixture.expectSilent(12);
    assert(fixture.queries==queries&&fixture.processes==processes&&fixture.stats().latchedError==0);
    fixture.prepareError=0;
    for(auto bad:{Fixture::BadSpan::zero,Fixture::BadSpan::tooLong,Fixture::BadSpan::coordinate,
                 Fixture::BadSpan::rateNaN,Fixture::BadSpan::rateZero,Fixture::BadSpan::negativeOffset,
                 Fixture::BadSpan::hugeOffset,Fixture::BadSpan::boundary,Fixture::BadSpan::tempoNaN,
                 Fixture::BadSpan::tempoZero,Fixture::BadSpan::beatNaN,Fixture::BadSpan::barStartNaN,
                 Fixture::BadSpan::barEndNaN,Fixture::BadSpan::numerator,Fixture::BadSpan::denominator}){
        fixture.bad=bad;
        const int32_t expected=bad==Fixture::BadSpan::zero?ST_UNIT_RANGE_DOMAIN_END:ST_UNIT_RANGE_CONTEXT_MISMATCH;
        assert(fixture.render(4,12)==expected);fixture.expectSilent(12);
        assert(fixture.processes==processes&&fixture.events==processes);
        assert(fixture.stats().renderedEndFrame==8&&fixture.stats().latchedError==0);
        assert(fixture.render(0,8)==0);fixture.expectSequence(8,1);
    }
    fixture.bad=Fixture::BadSpan::none;
    fixture.spanError=ST_CLOCK_INVALID;
    assert(fixture.render(8,8)==ST_UNIT_RANGE_CLOCK_ERROR);fixture.expectSilent(8);
    fixture.spanError=ST_CLOCK_OUTSIDE_DOMAIN;
    assert(fixture.render(8,8)==ST_UNIT_RANGE_DOMAIN_END);fixture.expectSilent(8);
    fixture.spanError=ST_CLOCK_OK;fixture.rate=44100;
    assert(fixture.render(8,8)==ST_UNIT_RANGE_CONTEXT_MISMATCH);fixture.expectSilent(8);
    fixture.rate=48000;fixture.offset=64;
    assert(fixture.render(8,8)==ST_UNIT_RANGE_CONTEXT_MISMATCH);fixture.expectSilent(8);
    fixture.offset=0;
    assert(fixture.render(4,12)==0);fixture.expectSequence(12,5);
    assert(fixture.advanced==16);
    assert(st_unit_range_reset(fixture.cache)==0);
    fixture.rate=44100;fixture.offset=64;fixture.advanced=0;
    assert(fixture.render(100,16)==0);fixture.expectSequence(16,1);
    // The first request also rejects an inconsistent format in a later span,
    // before committing an anchor or delivering events to any child.
    Fixture first(16,32,8,4);
    first.revisionBoundary=8;first.laterRate=44100;
    assert(first.render(0,16)==ST_UNIT_RANGE_CONTEXT_MISMATCH);first.expectSilent(16);
    assert(first.queries==2&&first.processes==0&&first.events==0&&first.stats().anchored==0);
    first.laterRate=0;first.laterOffset=64;
    assert(first.render(0,16)==ST_UNIT_RANGE_CONTEXT_MISMATCH);first.expectSilent(16);
    assert(first.processes==0&&first.events==0&&first.stats().anchored==0);
    first.laterOffset=-1;
    assert(first.render(0,16)==0);first.expectSequence(16,1);
}

static void testChildFailureLatchesAndReset(){
    Fixture fixture(16,32,4,16);
    assert(fixture.render(0,4)==0);fixture.expectSequence(4,1);
    fixture.failProcess=3;
    assert(fixture.render(2,14)==fixture.processError);fixture.expectSilent(14);
    auto stats=fixture.stats();
    assert(stats.processCalls==3&&stats.processedFrames==8&&stats.renderedEndFrame==4);
    assert(stats.retainedFrames==4&&stats.latchedError==fixture.processError&&fixture.advanced==12);
    const auto calls=fixture.processes,queries=fixture.queries,prepares=fixture.prepares;
    assert(fixture.render(0,4)==fixture.processError);fixture.expectSilent(4);
    assert(fixture.render(4,12)==fixture.processError);fixture.expectSilent(12);
    assert(fixture.processes==calls&&fixture.queries==queries&&fixture.prepares==prepares);
    assert(st_unit_range_reset(fixture.cache)==0);
    stats=fixture.stats();assert(stats.anchored==0&&stats.latchedError==0&&stats.requests==0&&stats.processedFrames==0);
    fixture.advanced=0;fixture.failProcess=0;
    assert(fixture.render(-23,16)==0);fixture.expectSequence(16,1);
    assert(fixture.stats().retainedStartFrame==-23&&fixture.stats().renderedEndFrame==-7);
}

static void testCreationAndRequestValidation(){
    char error[8];
    auto invalid=[&](uint32_t channels,uint32_t request,uint32_t history,uint32_t child,uint32_t spans){
        std::fill(std::begin(error),std::end(error),'x');
        assert(!st_unit_range_create(channels,request,history,child,spans,error,sizeof error));
        assert(error[0]!='\0'&&error[7]=='\0');
    };
    invalid(0,8,8,8,1);invalid(3,8,8,8,1);invalid(1,0,8,8,1);invalid(1,65537,65537,8,1);
    invalid(1,8,7,8,1);invalid(1,8,131073,8,1);invalid(1,8,8,0,1);invalid(1,8,8,9,1);
    invalid(1,8,8,8,0);invalid(1,8,8,8,257);
    invalid(1,16,16,4,3);invalid(1,65536,65536,1,256);
    char tiny='x';assert(!st_unit_range_create(0,8,8,8,1,&tiny,1)&&tiny=='\0');
    assert(!st_unit_range_create(0,8,8,8,1,nullptr,123));
    Fixture fixture(8,16,8,2);
    assert(st_unit_range_render(nullptr,0,8,fixture.output,nullptr,nullptr,nullptr,nullptr)==ST_UNIT_RANGE_INVALID);
    assert(fixture.left[0]==99&&st_unit_range_reset(nullptr)==ST_UNIT_RANGE_INVALID);
    STUnitRangeStats stats{};
    assert(st_unit_range_get_stats(nullptr,&stats)==ST_UNIT_RANGE_INVALID);
    assert(st_unit_range_get_stats(fixture.cache,nullptr)==ST_UNIT_RANGE_INVALID);
    assert(fixture.render(0,0)==ST_UNIT_RANGE_INVALID&&fixture.left[0]==99);
    assert(fixture.render(0,9)==ST_UNIT_RANGE_OVERSIZE);fixture.expectSilent(8);
    assert(fixture.left[8]==99&&fixture.right[8]==99);
    assert(fixture.render(std::numeric_limits<int64_t>::max()-3,8)==ST_UNIT_RANGE_OVERFLOW);fixture.expectSilent(8);
    assert(st_unit_range_render(fixture.cache,0,8,nullptr,nullptr,nullptr,nullptr,nullptr)==ST_UNIT_RANGE_INVALID);
    float *partial[]={fixture.left.data(),nullptr};
    assert(st_unit_range_render(fixture.cache,0,8,partial,nullptr,nullptr,nullptr,nullptr)==ST_UNIT_RANGE_INVALID);
    assert(st_unit_range_render(fixture.cache,0,8,fixture.output,nullptr,nullptr,Fixture::process,&fixture)==ST_UNIT_RANGE_INVALID);
    assert(st_unit_range_render(fixture.cache,0,8,fixture.output,nullptr,Fixture::span,nullptr,&fixture)==ST_UNIT_RANGE_INVALID);
    assert(fixture.prepares==0&&fixture.queries==0&&fixture.processes==0);
    fixture.requirePrepare=false;
    assert(st_unit_range_render(fixture.cache,0,8,fixture.output,nullptr,Fixture::span,Fixture::process,&fixture)==0);
    assert(fixture.prepares==0&&fixture.advanced==8);
    // Maximal allowed storage and split count, plus a nonzero initial graph anchor.
    Fixture maximum(65536,131072,256,256,1);
    assert(maximum.render(123456,65536)==0);maximum.expectSequence(65536,1);
    assert(maximum.processes==256);
    Fixture extreme(8,8,8,1,1);
    assert(extreme.render(std::numeric_limits<int64_t>::min(),8)==0);extreme.expectSequence(8,1);
    assert(extreme.render(std::numeric_limits<int64_t>::min()+4,4)==0);extreme.expectSequence(4,5);
    assert(st_unit_range_reset(extreme.cache)==0);extreme.advanced=0;
    assert(extreme.render(std::numeric_limits<int64_t>::max()-8,8)==0);extreme.expectSequence(8,1);
    assert(extreme.render(std::numeric_limits<int64_t>::max()-4,4)==0);extreme.expectSequence(4,5);
}

static void testReentrancyAndConcurrentRejection(){
    Fixture fixture(16,32,16,2);
    fixture.reenter=true;
    assert(fixture.render(0,16)==0);fixture.expectSequence(16,1);
    fixture.pauseProcess.store(true);
    std::thread renderer([&]{assert(fixture.render(16,16)==0);});
    while(!fixture.processEntered.load(std::memory_order_acquire))std::this_thread::yield();
    std::array<float,16> left{},right{};float *output[]={left.data(),right.data()};
    const auto began=std::chrono::steady_clock::now();
    for(unsigned attempt=0;attempt<10000;++attempt){
        left.fill(99);right.fill(99);
        assert(st_unit_range_render(fixture.cache,16,16,output,nullptr,nullptr,nullptr,nullptr)==ST_UNIT_RANGE_BUSY);
        assert(std::all_of(left.begin(),left.end(),[](float value){return value==0;}));
        assert(std::all_of(right.begin(),right.end(),[](float value){return value==0;}));
        STUnitRangeStats stats{};stats.requests=999;
        assert(st_unit_range_get_stats(fixture.cache,&stats)==ST_UNIT_RANGE_BUSY&&stats.requests==999);
        assert(st_unit_range_reset(fixture.cache)==ST_UNIT_RANGE_BUSY);
    }
    const double elapsed=std::chrono::duration<double>(std::chrono::steady_clock::now()-began).count();
    fixture.releaseProcess.store(true,std::memory_order_release);renderer.join();
    assert(elapsed<2.0);
    fixture.expectSequence(16,17);
    auto stats=fixture.stats();assert(stats.requests==2&&stats.processedFrames==32&&stats.latchedError==0);
    std::printf("  Concurrent rejection: 10,000 attempts in %.4f seconds while child was stalled.\n",elapsed);
}

static void testActualClockTempoMeterAndLatency(){
    STClockTempoEvent tempos[]={{0,120},{.25,60},{.3,180}};
    STClockMeterEvent meters[]={{0,4,4},{.25,5,8},{.3,7,8}};
    auto *map=st_clock_map_create(48000,tempos,3,meters,3,55,nullptr,0);assert(map);
    STClockOrigin origin{0,0,1,0,0,0,88};
    auto *timeline=st_clock_timeline_create(map,origin,nullptr,0);assert(timeline);
    st_clock_map_destroy(map);
    Fixture fixture(16384,32768,4096,64);
    fixture.timeline=timeline;fixture.offset=257;
    assert(fixture.render(0,16384)==0);fixture.expectSequence(16384,1);
    bool preroll=false,tempo=false,meter=false;
    for(uint32_t index=0;index<fixture.processTraceCount;++index){
        const auto &call=fixture.processTrace[index];
        STClockContext first{},last{};
        assert(st_clock_context(timeline,call.frame,257,&first)==ST_CLOCK_OK);
        assert(st_clock_context(timeline,call.frame+call.frames-1,257,&last)==ST_CLOCK_OK);
        assert(first.bpm==last.bpm&&first.numerator==last.numerator&&first.denominator==last.denominator);
        assert(first.barStartBeat==last.barStartBeat&&first.contentGraphFrame==call.frame-257);
        assert(call.revision==55&&call.epoch==88&&call.frames<=4096);
        preroll|=bool(first.flags&ST_CLOCK_PREROLL);
        tempo|=first.bpm==60;meter|=first.numerator==7;
    }
    assert(preroll&&tempo&&meter);
    const auto events=fixture.events;
    assert(fixture.render(6000,8000)==0);fixture.expectSequence(8000,6001);
    assert(fixture.events==events);
    st_clock_timeline_destroy(timeline);fixture.timeline=nullptr;
}

static void testLongRunNoRenderAllocations(){
    Fixture fixture(16384,32768,4096,64);
    fixture.partition=6000;
    uint64_t random=192874312;
    int64_t frontier=0;
    for(unsigned request=0;request<2000;++request){
        random=random*6364136223846793005ULL+1;
        const uint32_t extra=uint32_t(random%8192)+1;
        const uint32_t overlap=uint32_t(std::min<int64_t>(frontier,int64_t((random>>32)%4097)));
        const int64_t start=frontier-overlap;
        assert(fixture.render(start,overlap+extra)==0);
        fixture.expectSequence(overlap+extra,uint64_t(start+1));
        frontier+=extra;
        assert(fixture.advanced==uint64_t(frontier));
        assert(fixture.render(frontier-1,1)==0);fixture.expectSequence(1,uint64_t(frontier));
    }
    assert(renderAllocations.load()==0);
    std::printf("  Stateful overlap stress: %llu frames, 4,000 requests, zero render allocations.\n",
                static_cast<unsigned long long>(fixture.advanced));
}

int main(){
    testOverlap4097AndCacheHits();testRingWrapAndRetention();testFullPreflightBeforeEvents();
    testClockReplacementAndEpochBoundaries();testPrepareAndClockFailuresRetry();
    testChildFailureLatchesAndReset();testCreationAndRequestValidation();
    testReentrancyAndConcurrentRejection();testActualClockTempoMeterAndLatency();testLongRunNoRenderAllocations();
    assert(renderAllocations.load()==0);
    std::puts("Unit range: 10 groups passed; 4097/4096 overlap, retention, complete preflight, revision/epoch changes, failure recovery, child-error latching, validation, concurrent rejection, real clock spans, zero allocations.");
}

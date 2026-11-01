// Offline engine performance and bit-exact differential fixture. No devices or plugins.
#include "TrackerEngine.h"
#include "engine_test_tone.hpp"
#include <algorithm>
#include <array>
#include <atomic>
#include <cassert>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <ctime>
#include <new>
#include <vector>
#ifdef __APPLE__
#include <pthread.h>
#include <pthread/qos.h>
#endif

static bool rendering=false;
static size_t allocations=0;
void* operator new(size_t n){if(rendering)++allocations;if(auto*p=std::malloc(n?n:1))return p;throw std::bad_alloc();}
void* operator new[](size_t n){return ::operator new(n);}
void operator delete(void*p)noexcept{std::free(p);}
void operator delete[](void*p)noexcept{std::free(p);}
struct Scenario{const char*name;int voices,mode;bool modulated=false,mapped=false,host=false,edits=false;};
static constexpr unsigned sampleRate=48000,frames=96000,block=256;
static uint64_t hashWord(uint64_t h,uint32_t word){return (h^word)*1099511628211ull;}
static uint64_t hashDouble(uint64_t h,double value){uint64_t bits;std::memcpy(&bits,&value,8);return hashWord(hashWord(h,uint32_t(bits)),uint32_t(bits>>32));}
static void scene(const Scenario&scenario,int repetition){
    auto*e=st_engine_create(sampleRate);assert(e);st_engine_begin_update(e);st_engine_set_track_count(e,16);st_engine_set_timing(e,127,4,.17f,.7f);
    auto p=st_default_instrument();p.volume=.12f/std::sqrt(float(scenario.voices));p.attack=0;p.decay=.02f;p.sustain=1;p.release=.01f;
    if(scenario.mode<0){p.mode=ST_MODE_WAVETABLE;loadTestTone(e,0,TEST_SAW);}
    if(scenario.mode>=0){p.type=ST_INSTRUMENT_SAMPLE;p.mode=scenario.mode;p.start=.073f;p.end=.917f;p.loopStart=.18f;p.loopEnd=.79f;p.sliceCount=2;
        std::vector<float>left(32768),right(left.size());for(size_t n=0;n<left.size();++n){left[n]=float(std::sin(n*.02)*.6+std::cos(n*.013)*.2);right[n]=float(std::sin(n*.017)*.4-std::cos(n*.023)*.3);}assert(st_engine_set_sample(e,0,left.data(),right.data(),left.size(),44100));}
    if(scenario.modulated){p.cutoff=1600;p.resonance=.5f;p.drive=.15f;p.bitDepth=11;p.lfoRate=1.7f;p.lfoDepth=.3f;p.lfoTarget=ST_MOD_CUTOFF;p.lfo2Rate=3.1f;p.lfo2Depth=.07f;p.lfo2Target=ST_MOD_PITCH;p.filterEnvAmount=12;}
    if(scenario.edits){p.lfoRate=2.37f;p.lfoDepth=0;p.lfo2Rate=3.11f;p.lfo2Depth=0;p.lfo2Shape=ST_LFO_RANDOM;p.lfo2Target=ST_MOD_POSITION;}
    st_engine_set_instrument(e,0,p);for(int n=0;n<16;++n){auto t=st_default_track();t.pan=(n%3-1)*.15f;t.delaySend=.1f;t.reverbSend=.1f;st_engine_set_track(e,n,t);}
    st_engine_set_send_effects(e,.375,.55f,.035f);st_engine_set_pattern(e,0,64);int32_t sequence[]={0};st_engine_set_sequence(e,sequence,1,1,0);
    if(scenario.mapped){STClockTempoEvent tempos[]={{0,127},{1,181},{3,93},{7,149}};STClockMeterEvent meters[]={{0,7,8},{7,4,4}};char error[256];assert(st_engine_set_timing_map(e,tempos,4,meters,2,15,error,sizeof error));}
    if(scenario.host)st_engine_set_host_mixing(e,1,0xffff);
    if(scenario.edits){for(int track=0;track<16;++track){st_engine_add_automation_point(e,0,track,ST_TARGET_START,0,0,0,0,.073);st_engine_add_automation_point(e,0,track,ST_TARGET_START,0,0,0,6,.11);st_engine_add_automation_point(e,0,track,ST_TARGET_END,0,0,0,0,.917);st_engine_add_automation_point(e,0,track,ST_TARGET_END,0,0,0,6,.87);}}
    st_engine_commit_update(e);st_engine_play(e,0);
    for(int n=0;n<scenario.voices;++n)st_engine_note_on(e,n%16,0,36+n%36,80);
    std::array<float,block>left{},right{};std::array<std::array<float,block>,16>busL{},busR{};std::vector<double>blockUs;blockUs.reserve((frames+block-1)/block);uint64_t pcm=1469598103934665603ull,clock=pcm;double energy=0;
    const auto cpu=std::clock();const auto start=std::chrono::steady_clock::now();
    for(unsigned offset=0;offset<frames;offset+=block){
        if(scenario.mode==ST_MODE_SLICE&&offset>0&&offset%8192==0)for(int n=0;n<scenario.voices;++n)st_engine_note_on(e,n%16,0,36+n%36,80);
        if(scenario.edits&&offset==32768){p.lfoDepth=.2f;p.lfo2Depth=.15f;p.bitDepth=9;st_engine_begin_update(e);st_engine_set_instrument(e,0,p);st_engine_commit_update(e);}
        if(scenario.edits&&offset==65536){p.lfoDepth=0;p.lfo2Depth=0;p.pan=.63f;p.bitDepth=16;st_engine_begin_update(e);st_engine_set_instrument(e,0,p);st_engine_commit_update(e);}
        unsigned count=std::min(block,frames-offset);auto begin=std::chrono::steady_clock::now();rendering=true;
        if(scenario.host){for(int bus=0;bus<16;++bus)st_engine_render_bus(e,bus,offset,busL[bus].data(),busR[bus].data(),count);}else st_engine_render(e,left.data(),right.data(),count);
        rendering=false;blockUs.push_back(std::chrono::duration<double,std::micro>(std::chrono::steady_clock::now()-begin).count());
        if(scenario.host){STClockSpan span{};assert(st_engine_clock_span(e,offset,count,0,&span)==ST_CLOCK_OK);clock=hashDouble(hashDouble(hashDouble(clock,span.context.beat),span.context.bpm),double(span.context.nextBoundaryFrame));clock=hashWord(clock,span.frames);}
        for(int bus=0;bus<(scenario.host?16:1);++bus)for(unsigned n=0;n<count;++n)for(float value:{scenario.host?busL[bus][n]:left[n],scenario.host?busR[bus][n]:right[n]}){assert(std::isfinite(value));assert(std::abs(value)<=1);energy+=value*value;uint32_t bits;std::memcpy(&bits,&value,4);pcm=hashWord(pcm,bits);}
        STEvent event{};while(st_engine_pop_event(e,&event)){}
    }
    double cpuMs=double(std::clock()-cpu)*1000/CLOCKS_PER_SEC,wallMs=std::chrono::duration<double,std::milli>(std::chrono::steady_clock::now()-start).count();assert(energy>0);assert(allocations==0);std::sort(blockUs.begin(),blockUs.end());
    std::printf("{\"scenario\":\"%s\",\"repetition\":%d,\"cpu_ms\":%.6f,\"wall_ms\":%.6f,\"block_median_us\":%.6f,\"block_p95_us\":%.6f,\"pcm\":\"%016llx\",\"clock\":\"%016llx\",\"render_allocations\":%zu}\n",scenario.name,repetition,cpuMs,wallMs,blockUs[blockUs.size()/2],blockUs[size_t(blockUs.size()*.95)],(unsigned long long)pcm,(unsigned long long)clock,allocations);std::fflush(stdout);st_engine_destroy(e);
}
int main(int argc,char**argv){
#ifdef __APPLE__
    // Reduce arbitrary background/efficiency-core scheduling in paired runs.
    assert(pthread_set_qos_class_self_np(QOS_CLASS_USER_INITIATED,0)==0);
#endif
    int repetitions=argc>1?std::max(1,std::atoi(argv[1])):1;const Scenario scenarios[]={
    {"saw-16",16,-1},{"saw-64",64,-1},{"saw-256",256,-1},{"loop-64",64,ST_MODE_FORWARD},{"loop-256",256,ST_MODE_FORWARD},
    {"saw-mod-64",64,-1,true},{"saw-mod-256",256,-1,true},{"granular-mod-64",64,ST_MODE_GRANULAR,true},{"wavetable-mod-64",64,ST_MODE_WAVETABLE,true},{"slice-mod-64",64,ST_MODE_SLICE,true},
    {"mapped-host-16",16,-1,false,true,true},{"mapped-host-loop-64",64,ST_MODE_FORWARD,true,true,true},{"automated-region-lfo-64",64,ST_MODE_FORWARD,false,true,false,true}};
    for(int r=0;r<repetitions;++r)for(const auto&s:scenarios)if(argc<3||std::strcmp(argv[2],s.name)==0)scene(s,r);
}

// Behavior preserved by render-loop optimizations. Pure C++; no host or devices.
#include "TrackerEngine.h"
#include "engine_test_tone.hpp"
#include <algorithm>
#include <atomic>
#include <cassert>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <new>
#include <vector>
static thread_local bool rendering=false;
static std::atomic<size_t> allocations{0};
void* operator new(size_t n){if(rendering)++allocations;if(auto*p=std::malloc(n?n:1))return p;throw std::bad_alloc();}
void* operator new[](size_t n){return ::operator new(n);}
void operator delete(void*p)noexcept{std::free(p);}
void operator delete[](void*p)noexcept{std::free(p);}
struct Engine{STEngine*e=st_engine_create(48000);Engine(){assert(e);}~Engine(){st_engine_destroy(e);}operator STEngine*()const{return e;}};
struct Audio{std::vector<float>l,r;explicit Audio(size_t n):l(n),r(n){}};
static Audio render(STEngine*e,size_t n){Audio a(n);rendering=true;for(size_t p=0;p<n;p+=137)st_engine_render(e,a.l.data()+p,a.r.data()+p,uint32_t(std::min<size_t>(137,n-p)));rendering=false;return a;}
static STInstrument instrument(){auto p=testToneInstrument(TEST_SAW);p.attack=0;p.decay=0;p.sustain=1;p.release=.01f;p.volume=.2f;return p;}
static void setup(STEngine*e,STInstrument p){st_engine_begin_update(e);st_engine_set_track_count(e,1);st_engine_set_host_mixing(e,1,1);st_engine_set_instrument(e,0,p);loadTestTone(e,0,TEST_SAW);st_engine_commit_update(e);st_engine_note_on(e,0,0,60,127);}
static void zeroDepthContinuity(){
    for(int slot:{0,1})for(int shape:{ST_LFO_SINE,ST_LFO_TRIANGLE,ST_LFO_SAW,ST_LFO_SQUARE,ST_LFO_RANDOM}){
        Engine continuous,inaudible;auto a=instrument();
        if(slot==0){a.lfoRate=41.3f;a.lfoDepth=.7f;a.lfoShape=shape;a.lfoTarget=ST_MOD_VOLUME;}else{a.lfo2Rate=41.3f;a.lfo2Depth=.7f;a.lfo2Shape=shape;a.lfo2Target=ST_MOD_VOLUME;}
        auto b=a;if(slot==0)b.lfoDepth=0;else b.lfo2Depth=0;setup(continuous,a);setup(inaudible,b);
        render(continuous,12347);render(inaudible,12347);st_engine_begin_update(inaudible);st_engine_set_instrument(inaudible,0,a);st_engine_commit_update(inaudible);
        auto expected=render(continuous,8192),actual=render(inaudible,8192);assert(expected.l==actual.l&&expected.r==actual.r);
    }
}
static void regionAutomationBounds(){
    // Both samples contain the same selected silence, but only one has loud excluded material.
    // Moving each selection edge with automation must never borrow that excluded material.
    for(int mode:{ST_MODE_ONESHOT,ST_MODE_FORWARD,ST_MODE_REVERSE,ST_MODE_PINGPONG,ST_MODE_GRANULAR,ST_MODE_WAVETABLE}){
        Engine e;auto p=instrument();p.type=ST_INSTRUMENT_SAMPLE;p.mode=mode;p.start=.25f;p.end=.75f;p.loopStart=.25f;p.loopEnd=.75f;p.tune=-12;p.grainPosition=.001f;
        std::vector<float>pcm(4096,.8f);std::fill(pcm.begin()+1024,pcm.begin()+3072,0);
        st_engine_begin_update(e);st_engine_set_track_count(e,1);st_engine_set_timing(e,240,16,0,1);st_engine_set_host_mixing(e,1,1);st_engine_set_instrument(e,0,p);assert(st_engine_set_sample(e,0,pcm.data(),nullptr,pcm.size(),48000));
        st_engine_add_automation_point(e,0,0,ST_TARGET_START,0,0,0,0,.25);st_engine_add_automation_point(e,0,0,ST_TARGET_START,0,0,0,2,.375);
        st_engine_add_automation_point(e,0,0,ST_TARGET_END,0,0,0,0,.75);st_engine_add_automation_point(e,0,0,ST_TARGET_END,0,0,0,2,.625);
        st_engine_commit_update(e);st_engine_play(e,0);st_engine_note_on(e,0,0,60,127);auto audio=render(e,8000);
        assert(std::all_of(audio.l.begin(),audio.l.end(),[](float x){return x==0;}));assert(audio.l==audio.r);
    }
}
int main(){zeroDepthContinuity();std::puts("PASS zero-depth phase and random continuity: both LFOs, all five shapes");regionAutomationBounds();std::puts("PASS automated region boundaries isolate excluded PCM in all applicable modes");assert(allocations==0);std::puts("PASS zero render allocations");}

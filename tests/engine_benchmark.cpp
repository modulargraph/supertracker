#include "TrackerEngine.h"
#include "engine_test_tone.hpp"
#include <array>
#include <atomic>
#include <cassert>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <ctime>
#include <cstring>
#include <new>
#include <vector>

// Independent deterministic DSP benchmark. No absolute timing pass threshold:
// CPU time distinguishes engine work from competing builds / OS scheduling.
static bool rendering=false;
static unsigned allocations=0;
void* operator new(std::size_t n){if(rendering)++allocations;if(auto*p=std::malloc(n))return p;throw std::bad_alloc();}
void* operator new[](std::size_t n){return ::operator new(n);}
void operator delete(void*p)noexcept{std::free(p);}
void operator delete[](void*p)noexcept{std::free(p);}
static constexpr size_t frames=96000,block=256;
static void scene(const char*name,int voiceCount,int mode,bool modulated){
    auto*e=st_engine_create(48000);assert(e);
    st_engine_begin_update(e);st_engine_set_track_count(e,16);st_engine_set_timing(e,120,4,0,.7f);
    auto p=st_default_instrument();p.volume=.12f/std::sqrt(float(voiceCount));p.attack=0;p.decay=.02f;p.sustain=1;p.release=.01f;
    // mode < 0: a saw test tone (sampler in Wavetable mode).
    if(mode<0){p.mode=ST_MODE_WAVETABLE;loadTestTone(e,0,TEST_SAW);}
    if(mode>=0){p.type=ST_INSTRUMENT_SAMPLE;p.mode=mode;std::vector<float>sample(16384);for(size_t n=0;n<sample.size();++n)sample[n]=float(std::sin(n*.02)*.6+std::cos(n*.013)*.2);assert(st_engine_set_sample(e,0,sample.data(),nullptr,sample.size(),44100));}
    if(modulated){p.cutoff=1600;p.resonance=.5f;p.drive=.15f;p.lfoRate=1.7f;p.lfoDepth=.3f;p.lfoTarget=ST_MOD_CUTOFF;p.lfo2Rate=3.1f;p.lfo2Depth=.07f;p.lfo2Target=ST_MOD_PITCH;p.filterEnvAmount=12;}
    st_engine_set_instrument(e,0,p);for(int n=0;n<16;++n){auto t=st_default_track();t.pan=(n%3-1)*.15f;t.delaySend=.1f;t.reverbSend=.1f;st_engine_set_track(e,n,t);}st_engine_commit_update(e);
    for(int n=0;n<voiceCount;++n)st_engine_note_on(e,n%16,0,36+n%36,80);
    std::vector<float>left(frames),right(frames);auto wall=std::chrono::steady_clock::now();const auto cpu=std::clock();rendering=true;
    for(size_t offset=0;offset<frames;offset+=block)st_engine_render(e,left.data()+offset,right.data()+offset,uint32_t(std::min(block,frames-offset)));
    rendering=false;double cpuSeconds=double(std::clock()-cpu)/CLOCKS_PER_SEC,wallSeconds=std::chrono::duration<double>(std::chrono::steady_clock::now()-wall).count(),energy=0;uint64_t hash=1469598103934665603ull;
    for(size_t n=0;n<frames;++n)for(float value:{left[n],right[n]}){assert(std::isfinite(value));assert(std::abs(value)<=1);energy+=value*value;uint32_t bits;std::memcpy(&bits,&value,4);hash^=bits;hash*=1099511628211ull;}
    assert(energy>0);assert(allocations==0);
    std::printf("%-14s %3d voices | CPU %.4fs %6.2fx | wall %.4fs %6.2fx | PCM %016llx | RMS %.7f\n",name,voiceCount,cpuSeconds,2/cpuSeconds,wallSeconds,2/wallSeconds,(unsigned long long)hash,std::sqrt(energy/(frames*2)));
    st_engine_destroy(e);
}
int main(){for(int voices:{16,32,64,256})scene("saw",voices,-1,false);for(int voices:{16,32,64,256})scene("sample-loop",voices,ST_MODE_FORWARD,false);scene("saw-modulated",64,-1,true);scene("saw-modulated",256,-1,true);scene("sample-mod",64,ST_MODE_FORWARD,true);scene("granular-mod",64,ST_MODE_GRANULAR,true);std::puts("All benchmark renders finite, audible, bounded and allocation-free.");}

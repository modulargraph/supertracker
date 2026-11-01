// Engine render budget: representative projects rendered the way the host pulls them
// (every channel bus plus the return, per small quantum, host mixing), timed in thread
// CPU against generous budgets, with zero render allocations. Budgets are a coarse
// regression guard that holds under a parallel test run, not a device deadline.
// `--bench` prints the numbers and PCM hashes without enforcing budgets.
#include "TrackerEngine.h"
#include "engine_test_tone.hpp"
#include <algorithm>
#include <atomic>
#include <cassert>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <ctime>
#include <new>
#include <vector>

static thread_local bool realtime=false;
static std::atomic<unsigned> allocations{0};
void* operator new(std::size_t n){if(realtime)++allocations;if(void*p=std::malloc(n?n:1))return p;throw std::bad_alloc();}
void* operator new[](std::size_t n){return ::operator new(n);}
void operator delete(void*p)noexcept{std::free(p);}void operator delete[](void*p)noexcept{std::free(p);}
void operator delete(void*p,std::size_t)noexcept{std::free(p);}void operator delete[](void*p,std::size_t)noexcept{std::free(p);}

static constexpr double rate=48000;
static void commit(STEngine*e){char error[256]{};if(!st_engine_commit_update_checked(e,error,sizeof error)){std::fprintf(stderr,"commit: %s\n",error);std::abort();}}
static double threadSeconds(){timespec t{};clock_gettime(CLOCK_THREAD_CPUTIME_ID,&t);return double(t.tv_sec)+double(t.tv_nsec)*1e-9;}
static void sample(STEngine*e,int instrument,size_t frames,int seed){
    std::vector<float>l(frames),r(frames);uint32_t s=uint32_t(seed*7919+1);
    for(size_t i=0;i<frames;++i){s=s*1664525u+1013904223u;float noise=float(int32_t(s))/2147483647.f,decay=float(std::exp(-double(i)/double(frames)*5)),tone=float(std::sin(double(i)*2*3.141592653589793*(60+seed*40)/rate));l[i]=(tone*.7f+noise*.3f)*decay*.5f;r[i]=(tone*.6f+noise*.4f)*decay*.5f;}
    assert(st_engine_set_sample(e,instrument,l.data(),r.data(),frames,rate));
}
static void step(STEngine*e,int track,int row,int note,float gate,const int32_t*chord=nullptr,int count=0){auto s=st_default_step();s.note=note;s.velocity=100;s.gate=gate;st_engine_set_step(e,0,track,row,s,chord,count);}

/* The user's session: eight empty template tracks, a 16-pad kit of one-shot sampler
   tracks (60 ms .. 1.6 s, a drum pattern), two wavetable pads playing chords and a
   looping sampler bass. */
static void templateKit(STEngine*e){
    st_engine_begin_update(e);assert(st_engine_set_track_count(e,27));st_engine_set_instrument_count(e,32);st_engine_set_timing(e,120,4,0,.8f);st_engine_set_host_mixing_tracks(e,1,nullptr,0,1);st_engine_set_pattern(e,0,64);
    for(int pad=0;pad<16;++pad){auto p=st_default_instrument();p.mode=ST_MODE_ONESHOT;p.volume=.5f;p.attack=0;p.decay=0;p.sustain=1;p.release=.05f;st_engine_set_instrument(e,pad,p);sample(e,pad,size_t(rate*(.06+.1*pad)),pad);
        auto t=st_default_track();t.instrument=pad;st_engine_set_track(e,8+pad,t);st_engine_set_track_voice(e,8+pad,0,1,1);
        for(int row=0;row<64;++row){bool hit=pad==0?row%4==0:pad==1?row%8==4:pad==2?row%2==0:pad==3?row%16==14:row%16==(pad*5)%16;if(hit)step(e,8+pad,row,60,1);}}
    for(int synth=0;synth<3;++synth){const int i=16+synth;auto p=testToneInstrument(TEST_SAW);p.volume=.3f;
        if(synth<2){p.cutoff=4000;p.resonance=.2f;p.attack=.01f;p.decay=.3f;p.sustain=.7f;p.release=.3f;p.lfoRate=.5f;p.lfoDepth=.2f;p.lfoTarget=ST_MOD_POSITION;p.wavetablePosition=.3f;st_engine_set_instrument(e,i,p);loadTestTone(e,i,TEST_SAW);}
        else{p.mode=ST_MODE_FORWARD;p.loopStart=.1f;p.loopEnd=.9f;p.decay=.2f;p.sustain=.8f;p.release=.1f;st_engine_set_instrument(e,i,p);sample(e,i,size_t(rate),40);}
        auto t=st_default_track();t.instrument=i;st_engine_set_track(e,24+synth,t);
        const int32_t chord[]={0,4,7};for(int row=0;row<64;row+=16)step(e,24+synth,row,synth<2?48+synth*7:36,12,synth<2?chord:nullptr,synth<2?3:0);}
    commit(e);
}
/* Sixty-four sampler tracks playing a note every other row. */
static void manyTracks(STEngine*e){
    st_engine_begin_update(e);assert(st_engine_set_track_count(e,64));st_engine_set_instrument_count(e,64);st_engine_set_timing(e,120,4,0,.8f);st_engine_set_host_mixing_tracks(e,1,nullptr,0,1);st_engine_set_pattern(e,0,64);
    for(int t=0;t<64;++t){auto p=st_default_instrument();p.mode=t%2?ST_MODE_ONESHOT:ST_MODE_FORWARD;p.volume=.1f;p.attack=0;p.decay=.1f;p.sustain=.8f;p.release=.05f;st_engine_set_instrument(e,t,p);sample(e,t,size_t(t%2?rate*(.06+.1*(t%16)):rate),t%16);
        auto track=st_default_track();track.instrument=t;track.pan=float(t%5-2)*.2f;st_engine_set_track(e,t,track);
        for(int row=t%2;row<64;row+=2)step(e,t,row,48+(row+t)%24,1.5f);}
    commit(e);
}

struct Result{double percent;uint64_t hash;};
static Result run(void(*build)(STEngine*),bool playing,int tracks,uint32_t block,double seconds){
    STEngine*e=st_engine_create(rate);assert(e);build(e);if(playing)st_engine_play(e,0);
    std::vector<float>l(block),r(block);uint64_t hash=1469598103934665603ull,at=0;const uint64_t total=uint64_t(seconds*rate)/block*block;
    STEvent event{};double cpu=0;
    for(uint64_t done=0;done<total;done+=block,at+=block){
        realtime=true;const double t0=threadSeconds();
        st_engine_render_channel(e,0,at,nullptr,nullptr,block); // the master preflight anchors each quantum
        for(int c=-1;c<tracks;++c){st_engine_render_channel(e,c,at,l.data(),r.data(),block);for(uint32_t n=0;n<block;++n)for(float v:{l[n],r[n]}){uint32_t bits;std::memcpy(&bits,&v,4);hash=(hash^bits)*1099511628211ull;}}
        while(st_engine_pop_event(e,&event)){}
        cpu+=threadSeconds()-t0;realtime=false;
    }
    st_engine_destroy(e);return {cpu/(double(total)/rate)*100,hash};
}

int main(int argc,char**argv){
    const bool bench=argc>1&&!std::strcmp(argv[1],"--bench");
    struct Case{const char*name;void(*build)(STEngine*);bool playing;int tracks;double budget;};
    // Budgets are 4-13x the measured cost on an M4 core (1.2%, 0.4%, 7%): they catch an order-of-magnitude
    // regression, never scheduling noise from a loaded machine.
    const Case cases[]={{"template-kit-playing",templateKit,true,27,10},{"template-kit-stopped",templateKit,false,27,5},{"sixty-four-tracks",manyTracks,true,64,40}};
    for(const auto&c:cases)for(uint32_t block:{128u,512u}){
        allocations=0;auto result=run(c.build,c.playing,c.tracks,block,bench?(argc>2?std::atof(argv[2]):20):6);
        std::printf("%-22s block %3u: engine %.3f%% of real time (budget %.0f%%), PCM %016llx\n",c.name,block,result.percent,c.budget,(unsigned long long)result.hash);
        assert(allocations==0);
        if(!bench&&result.percent>c.budget){std::fprintf(stderr,"FAIL %s exceeded its render budget\n",c.name);return 1;}
    }
    std::puts("Engine render budget: all scenarios within budget with zero render allocations.");
}

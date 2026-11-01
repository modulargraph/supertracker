// Tracks are sized from the project: 200-track mixing/solo/mute, return bus renumbering when
// the track count grows past the legacy base, copy-on-write commits, silent-bus history,
// and zero render allocations. `--bench` prints render CPU and commit latency.
#include "TrackerEngine.h"
#include "engine_test_tone.hpp"
#include <algorithm>
#include <atomic>
#include <cassert>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <new>
#include <vector>

static thread_local bool realtime=false;
static std::atomic<unsigned> allocations{0};
void* operator new(std::size_t n){if(realtime)++allocations;if(void*p=std::malloc(n))return p;throw std::bad_alloc();}
void* operator new[](std::size_t n){return ::operator new(n);}
void operator delete(void*p)noexcept{std::free(p);}void operator delete[](void*p)noexcept{std::free(p);}
void operator delete(void*p,std::size_t)noexcept{std::free(p);}void operator delete[](void*p,std::size_t)noexcept{std::free(p);}

struct Engine{STEngine*p=st_engine_create(48000);Engine(){assert(p);}~Engine(){st_engine_destroy(p);}operator STEngine*(){return p;}};
struct PCM{std::vector<float>l,r;explicit PCM(size_t n):l(n),r(n){}double energy(size_t from=0,size_t to=0)const{if(!to)to=l.size();double e=0;for(size_t i=from;i<to;++i)e+=double(l[i])*l[i]+double(r[i])*r[i];return e/double(to-from);}};
static PCM bus(STEngine*e,int b,uint64_t at,uint32_t n){PCM p(n);realtime=true;st_engine_render_bus(e,b,at,p.l.data(),p.r.data(),n);realtime=false;return p;}
static void commit(STEngine*e){char error[256]{};if(!st_engine_commit_update_checked(e,error,sizeof error)){std::fprintf(stderr,"commit: %s\n",error);std::abort();}}
// A sampler playing the sine test cycle; loadSine() puts the cycle into instrument slot 0 or 64.
static STInstrument sine(){auto p=testToneInstrument();p.attack=0;p.decay=0;p.sustain=1;p.release=.005f;p.volume=1;p.cutoff=22000;return p;}
static float volumeFor(int track,int count){return .25f+1.5f*float(track)/float(count);}

/* Every one of `count` tracks plays the same sine at its own volume. */
static void allTracks(STEngine*e,int count,int pattern=0,int rows=16){
    st_engine_begin_update(e);assert(st_engine_set_track_count(e,count));st_engine_set_timing(e,120,4,0,1);st_engine_set_instrument(e,0,sine());loadTestTone(e,0);st_engine_set_pattern(e,pattern,rows);
    for(int t=0;t<count;++t){auto track=st_default_track();track.volume=volumeFor(t,count);track.instrument=0;st_engine_set_track(e,t,track);auto s=st_default_step();s.note=69;s.velocity=127;s.gate=64;st_engine_set_step(e,pattern,t,0,s,nullptr,0);}
    commit(e);
}

static void mixingSoloMute(){
    const int count=200;
    {   Engine e;allTracks(e,count);st_engine_play(e,0);std::vector<PCM> buses;for(int t=0;t<count;++t)buses.push_back(bus(e,t,0,2048));
        // Every track plays through its own bus with its own gain, sample for sample.
        const auto&reference=buses[0];assert(reference.energy(256,2048)>1e-4);
        for(int t=1;t<count;++t){float scale=volumeFor(t,count)/volumeFor(0,count);for(size_t n=256;n<2048;++n)assert(std::fabs(buses[size_t(t)].l[n]-reference.l[n]*scale)<2e-5f);}
        // The return bus follows the tracks; buses beyond it render silence, not garbage.
        assert(bus(e,st_return_bus(count),0,512).energy()==0);assert(bus(e,st_output_bus_count(count)+3,0,512).energy()==0);
    }
    {   Engine e;allTracks(e,count);st_engine_begin_update(e);for(int t:{7,150,199}){auto track=st_default_track();track.volume=volumeFor(t,count);track.instrument=0;track.mute=t==7;st_engine_set_track(e,t,track);}commit(e);
        st_engine_play(e,0);for(int t=0;t<count;++t){double energy=bus(e,t,0,2048).energy(256,2048);assert((energy==0)==(t==7));}
    }
    {   Engine e;allTracks(e,count);st_engine_begin_update(e);for(int t:{150,199}){auto track=st_default_track();track.volume=volumeFor(t,count);track.instrument=0;track.solo=1;st_engine_set_track(e,t,track);}commit(e);
        st_engine_play(e,0);for(int t=0;t<count;++t){double energy=bus(e,t,0,2048).energy(256,2048);assert((energy>0)==(t==150||t==199));}
    }
    {   // Raw host mixing with a computational bitset beyond 64 tracks.
        Engine e;allTracks(e,count);std::vector<uint64_t> words((count+63)/64,~uint64_t(0));words[180/64]&=~(uint64_t(1)<<(180%64));
        st_engine_begin_update(e);st_engine_set_host_mixing_tracks(e,1,words.data(),uint32_t(words.size()),1);commit(e);st_engine_play(e,0);
        for(int t:{0,63,64,179,180,181,199}){double energy=bus(e,t,0,2048).energy(256,2048);assert((energy==0)==(t==180));}
    }
    std::printf("200 tracks: per-track gain, mute, solo and computational bitset verified on every bus.\n");
}

static void returnBusFollowsTrackCount(){
    Engine e;st_engine_begin_update(e);assert(st_engine_set_track_count(e,3));st_engine_set_timing(e,120,4,0,1);commit(e);
    assert(st_return_bus(3)==16&&st_channel_count(3)==16&&st_output_bus_count(3)==17&&st_channel_output_bus(3,2)==2);
    st_engine_begin_update(e);assert(st_engine_set_track_count(e,40));commit(e);
    assert(st_return_bus(40)==40&&st_channel_count(40)==40&&st_output_bus_count(40)==41);
    assert(!st_engine_set_track_count(e,0)&&!st_engine_set_track_count(e,ST_TRACK_SANITY_LIMIT+1));
    std::printf("The return bus follows the track count (never below 16).\n");
}

static void growWhilePlaying(){
    Engine e;allTracks(e,4);st_engine_play(e,0);uint64_t at=0;assert(bus(e,0,at,1024).energy(256)>0);at+=1024;
    for(int count:{4,16,17,64,200,32,8}){
        st_engine_begin_update(e);assert(st_engine_set_track_count(e,count));for(int t=0;t<count;++t){auto track=st_default_track();track.instrument=0;track.volume=volumeFor(t,count);st_engine_set_track(e,t,track);auto s=st_default_step();s.note=69;s.velocity=127;s.gate=64;st_engine_set_step(e,0,t,0,s,nullptr,0);}commit(e);
        allocations=0;bus(e,0,at,1024);for(int t=0;t<count;t+=7)bus(e,t,at,1024);assert(allocations==0);
        // Track 0 keeps sounding across every reshaping of the tables.
        assert(bus(e,0,at,1024).energy(0)>0);at+=1024;
    }
    // A full pattern pass retriggers every track of the final layout.
    st_engine_seek(e,0,0,0);for(int n=0;n<8;++n){bus(e,0,at,4096);at+=4096;}
    for(int t=0;t<8;++t)assert(bus(e,t,at,1024).energy(0)>0);
    std::printf("Track count changes while playing (4..200) render without allocation and keep voices.\n");
}

static void copyOnWriteCommits(){
    Engine e;const int count=64,patterns=8;st_engine_begin_update(e);assert(st_engine_set_track_count(e,count));st_engine_set_instrument(e,0,sine());loadTestTone(e,0);
    for(int p=0;p<patterns;++p){st_engine_set_pattern(e,p,16);for(int t=0;t<count;++t){auto s=st_default_step();s.note=48+p;s.velocity=100;st_engine_set_step(e,p,t,p,s,nullptr,0);}}
    int32_t seq[patterns];for(int p=0;p<patterns;++p)seq[p]=p;st_engine_set_sequence(e,seq,patterns,0,0);commit(e);
    // Rebuild with every pattern but one reused: the reused ones keep their steps.
    st_engine_begin_update(e);st_engine_clear_project(e);assert(st_engine_set_track_count(e,count));
    for(int p=0;p<patterns;++p){if(p==3){st_engine_set_pattern(e,p,16);continue;}assert(st_engine_reuse_pattern(e,p));}
    assert(!st_engine_reuse_pattern(e,patterns+5));st_engine_set_sequence(e,seq,patterns,0,3);commit(e);
    // Pattern 3 is now empty, pattern 5 kept its steps on every track.
    st_engine_play(e,0);for(int t:{0,31,63})assert(bus(e,t,0,40000).energy()==0);
    st_engine_stop(e);st_engine_begin_update(e);st_engine_set_sequence(e,seq,patterns,0,5);commit(e);st_engine_seek(e,5,0,0);st_engine_play(e,0);
    // Row 5 starts at 5*6000 frames; every track's note arrives there.
    for(int t:{0,31,63}){auto audio=bus(e,t,40000,40000);assert(audio.energy(0,29000)==0&&audio.energy(31000,40000)>0);}
    std::printf("Copy-on-write: reused patterns keep their steps, edited ones are independent.\n");
}

static double seconds(std::chrono::steady_clock::time_point a){return std::chrono::duration<double>(std::chrono::steady_clock::now()-a).count();}
static void bench(){
    std::printf("tracks | full rebuild | reuse+1 pattern | 1-step edit | render all active (x realtime) | render 8 active (x realtime)\n");
    for(int count:{16,64,200}){
        Engine e;const int patterns=32,rows=64;
        auto build=[&](int changed){st_engine_begin_update(e);st_engine_clear_project(e);st_engine_set_track_count(e,count);st_engine_set_instrument(e,0,sine());loadTestTone(e,0);
            for(int t=0;t<count;++t){auto track=st_default_track();track.instrument=0;track.volume=.01f;st_engine_set_track(e,t,track);}
            for(int p=0;p<patterns;++p){if(changed>=0&&p!=changed){st_engine_reuse_pattern(e,p);continue;}st_engine_set_pattern(e,p,rows);for(int t=0;t<count;++t)for(int r=0;r<rows;r+=2){auto s=st_default_step();s.note=48+(r+t)%24;s.velocity=90;s.gate=1.5f;st_engine_set_step(e,p,t,r,s,nullptr,0);}}
            int32_t seq[patterns];for(int p=0;p<patterns;++p)seq[p]=p;st_engine_set_sequence(e,seq,patterns,1,0);commit(e);};
        auto t0=std::chrono::steady_clock::now();build(-1);double full=seconds(t0);
        t0=std::chrono::steady_clock::now();for(int n=0;n<10;++n)build(n%patterns);double reuse=seconds(t0)/10;
        t0=std::chrono::steady_clock::now();for(int n=0;n<100;++n){st_engine_begin_update(e);auto s=st_default_step();s.note=60+n%12;st_engine_set_step(e,n%patterns,n%count,1,s,nullptr,0);commit(e);}double one=seconds(t0)/100;
        st_engine_play(e,1);const uint32_t block=512;const uint64_t total=48000*5;std::vector<float>l(block),r(block);uint64_t at=0;
        // The host pulls every track bus plus the return each cycle.
        auto render=[&](int pulled){auto start=std::chrono::steady_clock::now();for(uint64_t done=0;done<total;done+=block,at+=block)for(int b=0;b<pulled;++b)st_engine_render_bus(e,b,at,l.data(),r.data(),block);return (double(total)/48000)/seconds(start);};
        allocations=0;realtime=true;double dense=render(count);realtime=false;assert(allocations==0);
        st_engine_begin_update(e);st_engine_clear_project(e);st_engine_set_track_count(e,count);st_engine_set_instrument(e,0,sine());loadTestTone(e,0);st_engine_set_pattern(e,0,rows);for(int t=0;t<8;++t)for(int r2=0;r2<rows;r2+=2){auto s=st_default_step();s.note=48+r2%24;s.gate=1.5f;st_engine_set_step(e,0,t,r2,s,nullptr,0);}commit(e);
        realtime=true;double sparse=render(count);realtime=false;assert(allocations==0);
        std::printf("%6d | %9.2f ms | %12.2f ms | %8.3f ms | %10.0fx | %10.0fx\n",count,full*1e3,reuse*1e3,one*1e3,dense,sparse);
    }
}

int main(int argc,char**argv){
    if(argc>1&&!std::strcmp(argv[1],"--bench")){bench();return 0;}
    allocations=0;mixingSoloMute();returnBusFollowsTrackCount();growWhilePlaying();copyOnWriteCommits();
    assert(allocations==0);std::printf("Many-track engine: all groups passed with zero render allocations.\n");
}

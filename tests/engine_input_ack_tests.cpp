#include "TrackerEngine.h"
#include "engine_test_tone.hpp"
#include <array>
#include <atomic>
#include <cassert>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <new>
#include <thread>
#include <vector>

static thread_local bool realtime=false;
static std::atomic<unsigned> rtAllocations{0};
void* operator new(std::size_t n){if(realtime)++rtAllocations;if(auto p=std::malloc(n?n:1))return p;throw std::bad_alloc();}
void* operator new[](std::size_t n){return ::operator new(n);}
void operator delete(void*p)noexcept{std::free(p);}
void operator delete[](void*p)noexcept{std::free(p);}
struct Engine{STEngine*e=st_engine_create(48000);Engine(){assert(e);}~Engine(){st_engine_destroy(e);}operator STEngine*()const{return e;}};
static void render(STEngine*e,uint32_t count){realtime=true;st_engine_render(e,nullptr,nullptr,count);realtime=false;}
static void fixture(STEngine*e,bool mapped=true,int rows=4,bool songLoop=false,float swing=0,bool variable=true){
    st_engine_begin_update(e);st_engine_clear_project(e);st_engine_set_track_count(e,1);st_engine_set_timing(e,120,4,swing,1);
    st_engine_set_pattern(e,0,rows);st_engine_set_pattern(e,1,rows);int32_t sequence[]={0,1};st_engine_set_sequence(e,sequence,2,songLoop,0);
    auto instrument=testToneInstrument();loadTestTone(e,0);instrument.attack=0;instrument.decay=0;instrument.sustain=1;instrument.release=.01;instrument.volume=.3;st_engine_set_instrument(e,0,instrument);
    auto track=st_default_track();track.volume=1;track.delaySend=0;track.reverbSend=0;st_engine_set_track(e,0,track);
    st_engine_set_host_mixing(e,1,1);
    if(mapped){STClockTempoEvent tempos[]{{0,120},{.5,60},{1.5,180}};STClockMeterEvent meters[]{{0,4,4},{.75,7,8}};char error[256]{};
        assert(st_engine_set_timing_map(e,tempos,variable?3:1,meters,variable?2:1,91,error,sizeof error));}
    char error[256]{};assert(st_engine_commit_update_checked(e,error,sizeof error));
}
static bool edge(STEngine*e,uint64_t id,int type=ST_INPUT_NOTE_ON,bool apply=false){return st_engine_input_note(e,id,type,0,0,60,type==ST_INPUT_NOTE_ON?100:0,apply);}
static STInputAck ack(STEngine*e){STInputAck a{};assert(st_engine_pop_input_ack(e,&a));return a;}
static void empty(STEngine*e){STInputAck a{};assert(!st_engine_pop_input_ack(e,&a));}
static void equal(double a,double b){assert(std::abs(a-b)<1e-11);}

static float peak(STEngine*e,uint32_t frames){std::vector<float>l(frames),r(frames);st_engine_render(e,l.data(),r.data(),frames);float m=0;for(uint32_t n=0;n<frames;++n)m=std::max(m,std::abs(l[n]));return m;}
int main(){
    {Engine e;fixture(e);st_engine_play(e,1);assert(edge(e,1));render(e,1);auto first=ack(e);
        assert(first.status==ST_INPUT_ACK_APPLIED&&first.pattern==0&&first.row==0&&first.sequence==0);
        assert(first.requestID==1&&first.timingRevision==91&&first.graphFrame==INT64_MIN&&first.renderFrame==0);equal(first.distanceBeats,0);
        render(e,5999);assert(edge(e,2,ST_INPUT_NOTE_OFF));render(e,1);auto row=ack(e);assert(row.row==1&&row.renderFrame==6000&&row.runID==first.runID);equal(row.distanceBeats,.25);
        render(e,57999);assert(edge(e,3));render(e,1);auto late=ack(e);assert(late.pattern==1&&late.row==3&&late.sequence==1&&late.renderFrame==64000&&late.runID==first.runID);equal(late.distanceBeats,1.75);empty(e);
        puts("PASS exact applied onset row and quarter-beat distance across tempo/meter boundaries");}
    {Engine e;fixture(e,true,2,false,.5,false);st_engine_begin_update(e);assert(st_engine_set_pattern_timing_origin(e,.25));st_engine_commit_update(e);
        st_engine_play(e,0);assert(edge(e,1));render(e,1);auto first=ack(e);equal(first.distanceBeats,.25);
        render(e,8999);assert(edge(e,2));st_engine_set_recording(e,1);render(e,1);auto swung=ack(e);assert(swung.row==1&&swung.runID==first.runID&&(swung.flags&ST_INPUT_CLOCK_RECORDING));equal(swung.distanceBeats,.625);
        render(e,26999);assert(edge(e,3,ST_INPUT_NOTE_OFF));render(e,1);auto looped=ack(e);equal(looped.distanceBeats,1.75);assert(looped.row==0&&looped.runID==first.runID);
        puts("PASS swung pattern onset, arm changes and three complete loops retain recording run identity");}
    {for(bool mapped:{false,true}){Engine e;fixture(e,mapped,2,true,0,false);st_engine_play(e,1);assert(edge(e,1));render(e,1);auto first=ack(e);
            render(e,71999);assert(edge(e,2,ST_INPUT_NOTE_OFF));render(e,1);auto last=ack(e);equal(last.distanceBeats-first.distanceBeats,3);assert(last.runID==first.runID);}
        Engine legacy;fixture(legacy,false,2,false);st_engine_play(legacy,0);assert(edge(legacy,1));render(legacy,36001);auto first=ack(legacy);assert(edge(legacy,2));render(legacy,1);auto last=ack(legacy);equal(last.distanceBeats-first.distanceBeats,36001.0/24000);
        puts("PASS mapped/fixed song wraps and unwrapped legacy pattern travel count every cycle");}
    {Engine e;fixture(e);st_engine_play(e,1);assert(edge(e,1));st_engine_seek(e,0,2,0);assert(edge(e,2,ST_INPUT_NOTE_OFF));render(e,1);
        auto before=ack(e),after=ack(e);assert(before.status==ST_INPUT_ACK_DISCONTINUITY&&after.status==ST_INPUT_ACK_APPLIED&&before.runID!=after.runID&&after.row==2);
        assert(edge(e,3));st_engine_stop(e);assert(edge(e,4,ST_INPUT_NOTE_OFF));render(e,1);before=ack(e);after=ack(e);
        assert(before.status==ST_INPUT_ACK_DISCONTINUITY&&after.status==ST_INPUT_ACK_APPLIED&&!(after.flags&ST_INPUT_CLOCK_PLAYING)&&before.runID!=after.runID);
        auto saved=st_engine_capture_transport(e);auto stoppedRun=after.runID;st_engine_restore_transport(e,saved,0);assert(edge(e,5));render(e,1);assert(ack(e).runID!=stoppedRun);
        assert(edge(e,6));render(e,0);assert(ack(e).status==ST_INPUT_ACK_CONTROL_FLUSH);assert(edge(e,7));(void)st_engine_capture_transport(e);assert(ack(e).status==ST_INPUT_ACK_CONTROL_FLUSH);
        puts("PASS command-order discontinuities, stopped endpoints, restore and zero-audio flush outcomes");}
    {Engine e;fixture(e);st_engine_play(e,1);assert(edge(e,1));render(e,1);auto first=ack(e);
        st_engine_begin_update(e);auto p=testToneInstrument();p.volume=.4;st_engine_set_instrument(e,0,p);st_engine_commit_update(e);assert(edge(e,2));render(e,1);assert(ack(e).runID==first.runID);
        st_engine_begin_update(e);STClockTempoEvent t[]{{0,120},{.5,60},{1.5,180}};STClockMeterEvent m[]{{0,4,4},{.75,7,8}};char error[128]{};assert(st_engine_set_timing_map(e,t,3,m,2,91,error,sizeof error));st_engine_commit_update(e);assert(edge(e,3));render(e,1);assert(ack(e).runID==first.runID);
        st_engine_begin_update(e);t[1].bpm=70;assert(st_engine_set_timing_map(e,t,3,m,2,92,error,sizeof error));st_engine_commit_update(e);assert(edge(e,4));render(e,1);auto changed=ack(e);assert(changed.runID!=first.runID&&changed.timingRevision==92);
        std::array<float,16> pcm{};st_engine_begin_update(e);assert(st_engine_set_audio_asset(e,0,pcm.data(),pcm.data(),pcm.size(),48000));st_engine_add_audio_clip(e,0,0,4,.25,0,1,0,0,0);st_engine_commit_update(e);assert(edge(e,5));render(e,1);assert(ack(e).runID!=changed.runID);
        Engine unsequenced;fixture(unsequenced);st_engine_begin_update(unsequenced);st_engine_set_pattern(unsequenced,2,2);int32_t seq[]={0,1};st_engine_set_sequence(unsequenced,seq,2,0,2);st_engine_commit_update(unsequenced);st_engine_play(unsequenced,0);assert(edge(unsequenced,1));render(unsequenced,1);auto prior=ack(unsequenced);
        st_engine_begin_update(unsequenced);st_engine_set_pattern(unsequenced,2,3);st_engine_commit_update(unsequenced);assert(edge(unsequenced,2));render(unsequenced,1);assert(ack(unsequenced).runID!=prior.runID);
        Engine collision;fixture(collision);st_engine_play(collision,1);assert(edge(collision,1));render(collision,1);prior=ack(collision);
        STClockTempoEvent sameLength[]{{0,120},{.5,80},{1,48},{1.5,180}};st_engine_begin_update(collision);assert(st_engine_set_timing_map(collision,sameLength,4,m,2,91,error,sizeof error));st_engine_commit_update(collision);assert(edge(collision,2));render(collision,1);assert(ack(collision).runID!=prior.runID);
        puts("PASS ordinary publication and identical timing retain run; semantic map edits break it");}
    {Engine ordinary,tracked;fixture(ordinary,false);fixture(tracked,false);st_engine_note_on(ordinary,0,0,60,100);assert(edge(tracked,1,ST_INPUT_NOTE_ON,true));
        std::array<float,1024> al{},ar{},bl{},br{};auto compare=[&](){realtime=true;st_engine_render(ordinary,al.data(),ar.data(),1024);st_engine_render(tracked,bl.data(),br.data(),1024);realtime=false;assert(!std::memcmp(al.data(),bl.data(),sizeof al)&&!std::memcmp(ar.data(),br.data(),sizeof ar));};
        compare();assert(ack(tracked).applyNote==1);assert(edge(tracked,2,ST_INPUT_NOTE_OFF,false));compare();assert(ack(tracked).applyNote==0);double energy=0;for(auto x:bl)energy+=x*x;assert(energy>1);
        st_engine_note_off(ordinary,0,60);assert(edge(tracked,3,ST_INPUT_NOTE_OFF,true));compare();assert(ack(tracked).applyNote==1);compare();
        puts("PASS opt-in note PCM equals ordinary APIs; capture-only sustain release leaves voice sounding");}
    {Engine e;fixture(e);st_engine_play(e,1);assert(edge(e,1));realtime=true;st_engine_render_bus(e,0,0,nullptr,nullptr,4097);realtime=false;auto first=ack(e);assert(first.graphFrame==0);
        assert(edge(e,2));st_engine_render_bus(e,16,0,nullptr,nullptr,4096);empty(e);st_engine_render_bus(e,0,4097,nullptr,nullptr,4096);auto second=ack(e);assert(second.graphFrame==4097&&second.renderFrame==4097&&second.runID==first.runID);
        st_engine_reset_render_cache(e);assert(edge(e,3));st_engine_render_bus(e,0,0,nullptr,nullptr,1);assert(ack(e).runID!=first.runID);
        puts("PASS cached overlapping pulls neither duplicate nor prematurely apply ACKs; reset breaks run");}
    {Engine e;fixture(e,false);st_engine_play(e,0);render(e,1000);assert(edge(e,1));render(e,1);auto first=ack(e);assert(first.row==0&&!first.early);
        render(e,3000);assert(edge(e,2));render(e,1);auto early=ack(e);assert(early.row==1&&early.early&&early.pattern==0);
        // At the last row nothing is ahead in the pattern: the note stays on its row.
        render(e,22000-4002);assert(edge(e,3));render(e,1);auto last=ack(e);assert(last.row==3&&!last.early);
        puts("PASS a note-on in the second half of a row is placed on the next row");}
    {for(bool early:{false,true}){Engine e;fixture(e,false);st_engine_begin_update(e);auto step=st_default_step();step.note=60;step.instrument=0;st_engine_set_step(e,0,0,1,step,nullptr,0);st_engine_commit_update(e);
            st_engine_play(e,0);render(e,4000);if(early){assert(edge(e,1));}float before=peak(e,1990);(void)before;float row1=peak(e,3000);
            assert(early?row1==0:row1>.01f);}
        puts("PASS the placed row skips its own trigger of the early note once");}
    {// Seek, restore and Panic end the live voice the early note stood for, so the placed row triggers normally.
        for(int transport=0;transport<3;++transport){Engine e;fixture(e,false);st_engine_begin_update(e);auto step=st_default_step();step.note=60;step.instrument=0;st_engine_set_step(e,0,0,1,step,nullptr,0);st_engine_commit_update(e);
            st_engine_play(e,0);render(e,4000);assert(edge(e,1,ST_INPUT_NOTE_ON,true));render(e,1);assert(ack(e).early);
            if(transport==0)st_engine_seek(e,0,1,0);                                   // row 1 starts at once
            else if(transport==1){auto saved=st_engine_capture_transport(e);st_engine_restore_transport(e,saved,1);render(e,1999);} // resumes mid-row 0
            else{st_engine_panic(e);render(e,1999);}                                     // playback continues into row 1
            assert(peak(e,3000)>.01f);}
        puts("PASS Seek, restore and Panic forget a pending early note: its row triggers");}
    {Engine e;assert(!edge(e,0));assert(!st_engine_input_note(e,1,3,0,0,60,100,1));assert(!st_engine_input_note(e,1,1,st_channel_count(ST_MIN_RETURN_BUS),0,60,100,1));assert(!st_engine_input_note(e,1,1,0,st_instrument_capacity(64),60,100,1));assert(!st_engine_input_note(e,1,1,0,0,128,100,1));assert(!st_engine_input_note(e,1,1,0,0,60,0,1));assert(!st_engine_input_note(e,1,1,0,0,60,100,2));
        assert(edge(e,1));assert(!edge(e,1));render(e,1);assert(ack(e).requestID==1);
        for(uint64_t id=2;id<=4096;++id){assert(edge(e,id));if(id%1000==0)render(e,1);}render(e,1);assert(!edge(e,4097));assert(ack(e).requestID==2);assert(edge(e,4097));render(e,1);
        for(uint64_t id=3;id<=4097;++id)assert(ack(e).requestID==id);empty(e);
        assert(edge(e,UINT64_MAX));render(e,1);assert(ack(e).requestID==UINT64_MAX);assert(!edge(e,1));assert(st_engine_status(e).droppedEvents==0);
        Engine full;for(int i=0;i<2047;++i)st_engine_pitch_bend(full,0,0);assert(!edge(full,1));render(full,1);assert(edge(full,1));render(full,1);assert(ack(full).requestID==1);
        puts("PASS strict tokens/argument bounds and reserved ACK capacity with command-failure rollback");}
    {Engine e;std::atomic<bool> done{false};std::thread producer([&]{for(uint64_t id=1;id<=20000;++id){while(!edge(e,id))std::this_thread::yield();}done.store(true,std::memory_order_release);});
        uint64_t expected=1;while(!done.load(std::memory_order_acquire)||expected<=20000){render(e,1);STInputAck a{};while(st_engine_pop_input_ack(e,&a)){assert(a.requestID==expected++&&a.status==ST_INPUT_ACK_APPLIED);}}
        producer.join();assert(expected==20001);empty(e);
        puts("PASS concurrent single-producer/audio-consumer 20000-edge delivery without lost ACKs");}
    assert(rtAllocations==0);puts("Input ACKs: 10 groups passed; zero measured render-thread C++ allocations.");
}

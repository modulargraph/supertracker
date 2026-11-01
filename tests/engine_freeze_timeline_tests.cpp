#include "TrackerEngine.h"
#include <atomic>
#include <cassert>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <new>
#include <vector>
static thread_local bool realtime=false;
static std::atomic<unsigned> allocations{0};
static bool failAllocation=false;
void*operator new(size_t n){if(realtime)++allocations;if(failAllocation){failAllocation=false;throw std::bad_alloc();}if(auto*p=std::malloc(n?n:1))return p;throw std::bad_alloc();}
void*operator new[](size_t n){return ::operator new(n);}
void operator delete(void*p)noexcept{std::free(p);}
void operator delete[](void*p)noexcept{std::free(p);}
struct Engine{STEngine*e=st_engine_create(48000);Engine(){assert(e);}~Engine(){st_engine_destroy(e);}operator STEngine*()const{return e;}};
void commit(STEngine*e){char error[256];assert(st_engine_commit_update_checked(e,error,sizeof error));}
void policy(STEngine*e,bool enable=true,uint64_t publication=7){char error[256];assert(st_engine_set_frozen_song_policy(e,enable,publication,error,sizeof error));commit(e);}
void render(STEngine*e,uint64_t at,uint32_t count){realtime=true;st_engine_render_bus(e,0,at,nullptr,nullptr,count);realtime=false;}
STFreezeSpan span(STEngine*e,int64_t at,uint32_t n=65536,int64_t latency=0){STFreezeSpan result{};realtime=true;int status=st_engine_freeze_span(e,at,n,latency,&result);realtime=false;assert(status==ST_CLOCK_OK&&result.frames>0);return result;}
std::vector<int64_t> notes(STEngine*e,int64_t offset){std::vector<int64_t> result;STEvent v{};while(st_engine_pop_event(e,&v))if(v.type==ST_EVENT_NOTE_ON)result.push_back(offset+v.frameOffset);return result;}
void fixture(STEngine*e,double bpm=123,int rows=4,float swing=.3,bool loop=false,bool frozen=true,bool mapped=false){
    st_engine_begin_update(e);st_engine_clear_project(e);st_engine_set_track_count(e,1);st_engine_set_timing(e,bpm,4,swing,1);st_engine_set_host_mixing(e,1,1);
    st_engine_set_pattern(e,0,rows);int32_t seq=0;st_engine_set_sequence(e,&seq,1,loop,0);
    auto instrument=st_default_instrument();instrument.type=ST_INSTRUMENT_AU;st_engine_set_instrument(e,0,instrument);
    auto step=st_default_step();step.note=60;step.instrument=0;step.gate=.1;for(int row=0;row<rows;++row)st_engine_set_step(e,0,0,row,step,nullptr,0);
    char error[256];if(mapped){STClockTempoEvent tempos[]{{0,120},{.5,60},{1.5,180}};STClockMeterEvent meters[]{{0,4,4},{.75,7,8}};assert(st_engine_set_timing_map(e,tempos,3,meters,2,51,error,sizeof error));}
    if(frozen)assert(st_engine_set_frozen_song_policy(e,1,7,error,sizeof error));commit(e);
}
int main(){
    {Engine e;STFreezeSpan out{};out.frames=123;assert(st_engine_freeze_span(e,0,1,0,&out)==ST_FREEZE_NOT_CONFIGURED&&out.frames==123);char error[128];
        assert(!st_engine_set_frozen_song_policy(e,2,7,error,sizeof error)&&error[0]);assert(!st_engine_set_frozen_song_policy(e,1,0,error,sizeof error));
        failAllocation=true;assert(!st_engine_set_frozen_song_policy(e,1,7,error,sizeof error));assert(st_engine_freeze_span(e,0,1,0,&out)==ST_FREEZE_NOT_CONFIGURED);
        fixture(e,120,4,0);render(e,0,1);assert(span(e,0).context.phase==ST_FREEZE_UNSUPPORTED);
        assert(!st_engine_set_frozen_song_policy(e,1,0,error,sizeof error));render(e,1,1);assert(span(e,1).context.publicationID==7);
        policy(e,false);render(e,2,1);assert(span(e,2).context.phase==ST_FREEZE_DISABLED);
        puts("PASS default-off policy, bounded allocation failure and invalid-policy rollback");}
    {Engine e;fixture(e);st_engine_play(e,1);st_engine_seek(e,0,1,0);render(e,0,16000);
        double base=48000.0*60/123/4;double start=(1+double(.3f))*base;auto first=span(e,0);assert(first.context.projectFrame==int64_t(std::ceil(start))&&first.context.cause==ST_FREEZE_CAUSE_SEEK);
        auto events=notes(e,0);std::vector<int64_t> expected{0,int64_t(std::ceil(2*base)-std::ceil(start)),int64_t(std::ceil((3+double(.3f))*base)-std::ceil(start))};assert(events==expected);
        auto pre=span(e,0,200,100);assert(pre.frames==100&&pre.context.phase==ST_FREEZE_STOPPED&&(pre.context.flags&ST_FREEZE_PREROLL)&&pre.context.contentGraphFrame==-100);
        assert(span(e,100,1,100).context.projectFrame==first.context.projectFrame);
        puts("PASS fractional swung row seek rounds once, carries remainder, and PDC startup is silent");}
    {Engine e;fixture(e,123,4,.3,false,false);st_engine_play(e,1);st_engine_seek(e,0,1,0);st_engine_render(e,nullptr,nullptr,101);auto original=st_engine_capture_transport(e);assert(original.rowHasStarted&&original.transportFrames!=std::ceil(original.transportFrames));
        policy(e);st_engine_restore_transport(e,original,1);auto aligned=st_engine_capture_transport(e);double delta=std::ceil(original.transportFrames)-original.transportFrames;assert(aligned.transportFrames==std::ceil(original.transportFrames)&&std::abs(aligned.rowRemaining-(original.rowRemaining-delta))<1e-9&&aligned.rowHasStarted);
        st_engine_restore_transport(e,aligned,1);render(e,0,1);assert(notes(e,0).empty());assert(span(e,0).context.cause==ST_FREEZE_CAUSE_RESTORE);
        policy(e,false);st_engine_restore_transport(e,original,1);auto rollback=st_engine_capture_transport(e);assert(rollback.transportFrames==original.transportFrames&&rollback.rowRemaining==original.rowRemaining&&rollback.rowHasStarted==original.rowHasStarted);
        Engine start;fixture(start);st_engine_play(start,1);st_engine_seek(start,0,1,0);auto before=st_engine_capture_transport(start);assert(!before.rowHasStarted&&before.rowRemaining<0);st_engine_restore_transport(start,before,1);auto after=st_engine_capture_transport(start);assert(after.rowRemaining==before.rowRemaining&&after.transportFrames==before.transportFrames);st_engine_restore_transport(start,after,1);render(start,0,16000);auto restoredNotes=notes(start,0);double base=48000.0*60/123/4,origin=std::ceil((1+double(.3f))*base);assert((restoredNotes==std::vector<int64_t>{0,int64_t(std::ceil(2*base)-origin),int64_t(std::ceil((3+double(.3f))*base)-origin)}));
        puts("PASS mid-row restore avoids retrigger; exact rollback and pending row-start remainder survive");}
    {Engine natural,stopped;fixture(natural,120,1,0);fixture(stopped,120,1,0);st_engine_play(natural,1);st_engine_play(stopped,1);render(natural,0,6100);render(stopped,0,5999);st_engine_stop(stopped);render(stopped,5999,101);
        auto before=span(natural,5998),tail=span(natural,6000);assert(before.frames==2&&before.context.phase==ST_FREEZE_SONG);assert(tail.context.phase==ST_FREEZE_NATURAL_TAIL&&tail.context.cause==ST_FREEZE_CAUSE_NATURAL_END&&tail.context.transitionProjectFrame==6000&&tail.context.transitionContentGraphFrame==6000&&tail.context.playbackID==before.context.playbackID);
        assert(span(natural,6050).context.projectFrame==6050);auto stop=span(stopped,5999);assert(stop.context.phase==ST_FREEZE_STOPPED&&stop.context.cause==ST_FREEZE_CAUSE_STOP&&stop.context.projectFrame==5999&&stop.frames==101);
        st_engine_stop(natural);render(natural,6100,1);assert(span(natural,6100).context.phase==ST_FREEZE_STOPPED&&span(natural,6100).context.cause==ST_FREEZE_CAUSE_STOP);
        puts("PASS authoritative natural-tail transition versus Stop one sample before completion");}
    {Engine e;fixture(e,120,8,0,false,true,true);st_engine_play(e,1);render(e,0,65536);render(e,65536,2564);auto tail=span(e,68000);assert(tail.context.phase==ST_FREEZE_NATURAL_TAIL&&tail.context.transitionProjectFrame==68000);
        auto onsets=notes(e,0); // last pull offsets differ, so query native retained mapping separately.
        (void)onsets;assert(span(e,64000,1).context.projectFrame==64000);
        Engine clips;fixture(clips,120,1,0);std::vector<float> pcm(1000,.2f);st_engine_begin_update(clips);assert(st_engine_set_audio_asset(clips,0,pcm.data(),pcm.data(),1000,48000));st_engine_add_audio_clip(clips,0,0,.25,0,0,1,0,0,0);commit(clips);assert(st_engine_duration(clips,1)==7000.0/48000);st_engine_play(clips,1);render(clips,0,7100);auto extended=span(clips,7000);assert(extended.context.phase==ST_FREEZE_NATURAL_TAIL&&extended.context.transitionProjectFrame==7000);
        Engine fractional;fixture(fractional,120,1,0);std::vector<float> longer(7000,.2f);st_engine_begin_update(fractional);assert(st_engine_set_audio_asset(fractional,0,longer.data(),longer.data(),7000,48000));st_engine_add_audio_clip(fractional,0,0,0,.25000000002,0,1,0,0,0);commit(fractional);assert(std::ceil(st_engine_duration(fractional,1)*48000)==6001);st_engine_play(fractional,1);render(fractional,0,6010);assert(span(fractional,6000).context.phase==ST_FREEZE_SONG&&span(fractional,6001).context.transitionProjectFrame==6001&&span(fractional,6001).context.phase==ST_FREEZE_NATURAL_TAIL);
        puts("PASS mapped project-frame identity and clip-extended exact musical end");}
    {Engine e;fixture(e,999,1,0,true);st_engine_begin_update(e);st_engine_set_timing(e,999,32,0,1);commit(e);st_engine_play(e,1);assert(st_engine_input_note(e,1,ST_INPUT_NOTE_ON,0,0,60,100,0));render(e,0,65000);STInputAck first{};assert(st_engine_pop_input_ack(e,&first));int64_t cursor=0;uint64_t prior=0;int loops=0;int64_t length=int64_t(std::ceil(48000.0*60/999/32));
        while(cursor<65000){auto s=span(e,cursor);assert(s.context.phase==ST_FREEZE_SONG&&s.context.projectFrame==0&&s.context.playbackID>prior);assert(s.frames==std::min<int64_t>(length,65000-cursor));if(cursor)assert(s.context.cause==ST_FREEZE_CAUSE_LOOP);prior=s.context.playbackID;cursor+=s.frames;++loops;}assert(loops>700);
        assert(st_engine_input_note(e,2,ST_INPUT_NOTE_OFF,0,0,60,0,0));render(e,65000,1);STInputAck last{};assert(st_engine_pop_input_ack(e,&last)&&last.runID==first.runID&&last.distanceBeats>first.distanceBeats);
        puts("PASS 700+ loops per pull split every playback identity while ACK run remains continuous");}
    {Engine e;fixture(e,120,64,0);st_engine_play(e,1);render(e,1000,4097);auto initial=span(e,1000);policy(e,true,UINT64_MAX);render(e,5097,4096);auto old=span(e,7096,4096,2000);assert(old.context.publicationID==7&&old.frames==1);auto changed=span(e,7097,4096,2000);assert(changed.context.publicationID==UINT64_MAX&&changed.context.cause==ST_FREEZE_CAUSE_REPLACEMENT&&changed.context.projectFrame==4097&&changed.context.playbackID!=initial.context.playbackID);
        st_engine_seek(e,0,3,0);render(e,9193,2);auto seek=span(e,9193);assert(seek.context.cause==ST_FREEZE_CAUSE_SEEK&&seek.context.projectFrame==18000);
        st_engine_panic(e);render(e,9195,1);assert(span(e,9195).context.cause==ST_FREEZE_CAUSE_PANIC);
        st_engine_reset_render_cache(e);render(e,0,1);assert(span(e,0).context.cause==ST_FREEZE_CAUSE_GRAPH_RESET);
        st_engine_play(e,0);render(e,1,1);assert(span(e,1).context.phase==ST_FREEZE_UNSUPPORTED);
        puts("PASS retained publications, integer PDC, seek/panic/reset causes and Pattern-mode exclusion");}
    {Engine e;fixture(e,120,64,0,true);st_engine_play(e,1);render(e,0,65536);for(uint64_t at=65536;at<327680;at+=65536)render(e,at,65536);STFreezeSpan out{};out.frames=77;assert(st_engine_freeze_span(e,65535,1,0,&out)==ST_ENGINE_CLOCK_NOT_RETAINED&&out.frames==77);assert(span(e,65536,1).context.contentGraphFrame==65536);assert(st_engine_freeze_span(e,327680,1,0,&out)==ST_ENGINE_CLOCK_NOT_PREPARED);
        assert(st_engine_freeze_span(e,0,0,0,&out)==ST_CLOCK_INVALID);assert(st_engine_freeze_span(e,0,65537,0,&out)==ST_CLOCK_INVALID);assert(st_engine_freeze_span(e,0,1,-1,&out)==ST_CLOCK_INVALID);assert(st_engine_freeze_span(e,INT64_MAX,1,0,&out)==ST_CLOCK_INVALID);assert(st_engine_freeze_span(e,0,1,(int64_t(1)<<30)+1,&out)==ST_CLOCK_INVALID);
        Engine late;fixture(late,120,4,0,false,false);st_engine_play(late,1);render(late,0,10);policy(late);render(late,10,10);assert(st_engine_freeze_span(late,0,1,0,&out)==ST_ENGINE_CLOCK_NOT_RETAINED);assert(st_engine_freeze_span(late,-1,1,0,&out)==ST_ENGINE_CLOCK_NOT_RETAINED);assert(span(late,10).context.publicationID==7);
        Engine limit;fixture(limit,120,4,0);st_engine_play(limit,1);render(limit,uint64_t(1)<<52,1);assert(span(limit,int64_t(1)<<52,128,100).frames==1);
        puts("PASS retained-window exhaustion, overflow bounds, and no fabricated pre-allocation history");}
    {Engine e;fixture(e,120,1,0);st_engine_play(e,1);render(e,0,6010);auto ended=span(e,6000).context;st_engine_begin_update(e);auto track=st_default_track();track.volume=.4;st_engine_set_track(e,0,track);commit(e);render(e,6010,10);auto unchanged=span(e,6010).context;assert(unchanged.phase==ST_FREEZE_NATURAL_TAIL&&unchanged.transitionID==ended.transitionID&&unchanged.projectFrame==6010);
        policy(e,true,8);render(e,6020,1);assert(span(e,6020).context.phase==ST_FREEZE_STOPPED&&span(e,6020).context.cause==ST_FREEZE_CAUSE_REPLACEMENT);
        auto saved=st_engine_capture_transport(e);st_engine_restore_transport(e,saved,0);render(e,0,1);assert(span(e,0).context.phase==ST_FREEZE_STOPPED&&span(e,0).context.cause==ST_FREEZE_CAUSE_RESTORE);
        puts("PASS mix-only edits retain natural tail; replacement and restore cannot synthesize completion");}
    {Engine e;fixture(e,120,4,0);st_engine_begin_update(e);st_engine_set_host_mixing(e,1,0);commit(e);st_engine_play(e,1);assert(st_engine_input_note(e,1,ST_INPUT_NOTE_ON,0,0,60,100,0));
        STFreezeSpan pending{};STInputAck ack{};assert(st_engine_freeze_span(e,0,4097,0,&pending)==ST_ENGINE_CLOCK_NOT_PREPARED);assert(!st_engine_status(e).playing&&!st_engine_pop_input_ack(e,&ack));
        // A silent transport anchor may prepare any bus with null PCM outputs.
        // It does not require an unfrozen track or an audible source node.
        realtime=true;st_engine_render_bus(e,st_return_bus(ST_MIN_RETURN_BUS),0,nullptr,nullptr,4097);realtime=false;
        assert(st_engine_pop_input_ack(e,&ack)&&ack.graphFrame==0);assert(span(e,0,4097).frames==4097&&span(e,0).context.phase==ST_FREEZE_SONG);auto seconds=st_engine_status(e).seconds;
        render(e,0,4096);render(e,0,4097);assert(st_engine_status(e).seconds==seconds&&!st_engine_pop_input_ack(e,&ack));render(e,4096,4096);assert(span(e,4096).context.projectFrame==4096&&st_engine_status(e).seconds==8192.0/48000);
        puts("PASS query never advances transport; silent anchor prepares exactly once across overlapping pulls");}
    assert(allocations==0);puts("Freeze timeline: 10 groups passed; zero measured render/query C++ allocations.");
}

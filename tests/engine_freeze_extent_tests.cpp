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
static std::atomic<unsigned> allocations{0},deallocations{0};
static bool failAllocation=false;
void*operator new(size_t n){if(realtime)++allocations;if(failAllocation){failAllocation=false;throw std::bad_alloc();}if(auto*p=std::malloc(n?n:1))return p;throw std::bad_alloc();}
void*operator new[](size_t n){return ::operator new(n);}
void operator delete(void*p)noexcept{if(realtime&&p)++deallocations;std::free(p);}
void operator delete[](void*p)noexcept{::operator delete(p);}
struct Engine{STEngine*e;explicit Engine(double rate=48000):e(st_engine_create(rate)){assert(e);}~Engine(){st_engine_destroy(e);}operator STEngine*()const{return e;}};
static void commit(STEngine*e){char error[256];if(!st_engine_commit_update_checked(e,error,sizeof error)){std::fprintf(stderr,"Commit failed: %s\n",error);std::abort();}}
static void extent(STEngine*e,int64_t frames,uint64_t publication=7){char error[256];assert(st_engine_set_frozen_song_extent(e,publication,frames,error,sizeof error));commit(e);}
static void render(STEngine*e,uint64_t at,uint32_t n){realtime=true;st_engine_render_bus(e,0,at,nullptr,nullptr,n);realtime=false;}
static STFreezeSpan span(STEngine*e,int64_t at,uint32_t n=65536,int64_t latency=0){STFreezeSpan s{};realtime=true;auto status=st_engine_freeze_span(e,at,n,latency,&s);realtime=false;assert(status==ST_CLOCK_OK&&s.frames>0);return s;}
static int onsets(STEngine*e){STEvent event{};int n=0;while(st_engine_pop_event(e,&event))if(event.type==ST_EVENT_NOTE_ON)++n;return n;}
static void fixture(STEngine*e,bool mapped=false,bool loop=false,double bpm=120,int rows=1,float swing=0){
    st_engine_begin_update(e);st_engine_clear_project(e);st_engine_set_track_count(e,1);st_engine_set_timing(e,bpm,4,swing,1);
    st_engine_set_host_mixing(e,1,1);st_engine_set_pattern(e,0,rows);int32_t seq=0;st_engine_set_sequence(e,&seq,1,loop,0);
    auto instrument=st_default_instrument();instrument.type=ST_INSTRUMENT_AU;st_engine_set_instrument(e,0,instrument);
    auto step=st_default_step();step.note=60;step.instrument=0;step.gate=.01;for(int row=0;row<rows;++row)st_engine_set_step(e,0,0,row,step,nullptr,0);
    char error[256];if(mapped){STClockTempoEvent tempos[]{{0,120},{.25,60},{.5,180}};STClockMeterEvent meters[]{{0,4,4},{.75,7,8}};assert(st_engine_set_timing_map(e,tempos,3,meters,2,51,error,sizeof error));}
    assert(st_engine_set_frozen_song_policy(e,1,7,error,sizeof error));commit(e);
}
int main(){
    {Engine e;char error[128];assert(!st_engine_set_frozen_song_extent(e,7,7000,error,sizeof error)&&error[0]);fixture(e);
        assert(!st_engine_set_frozen_song_extent(nullptr,7,7000,error,sizeof error));
        for(int64_t bad:{INT64_MIN,int64_t(-1),int64_t(0),int64_t(100000001),INT64_MAX})assert(!st_engine_set_frozen_song_extent(e,7,bad,error,sizeof error));
        assert(!st_engine_set_frozen_song_extent(e,0,7000,error,sizeof error));assert(!st_engine_set_frozen_song_extent(e,8,7000,error,sizeof error));
        char tiny[2]={'x','y'};assert(!st_engine_set_frozen_song_extent(e,0,7000,tiny,1)&&tiny[0]==0&&tiny[1]=='y');
        failAllocation=true;assert(!st_engine_set_frozen_song_extent(e,7,7000,error,sizeof error));assert(st_engine_duration(e,1)==6000.0/48000);
        extent(e,7000);assert(st_engine_set_frozen_song_extent(e,7,7000,error,sizeof error));assert(!st_engine_set_frozen_song_extent(e,7,7001,error,sizeof error));commit(e);assert(st_engine_duration(e,1)==7000.0/48000);
        assert(st_engine_set_frozen_song_policy(e,1,7,error,sizeof error));commit(e);assert(st_engine_duration(e,1)==7000.0/48000);
        assert(st_engine_set_frozen_song_policy(e,1,8,error,sizeof error));commit(e);assert(st_engine_duration(e,1)==6000.0/48000);
        extent(e,8000,8);assert(st_engine_set_frozen_song_policy(e,0,8,error,sizeof error));commit(e);assert(st_engine_duration(e,1)==6000.0/48000);
        assert(!st_engine_set_frozen_song_extent(e,8,8000,error,sizeof error));assert(st_engine_set_frozen_song_policy(e,1,8,error,sizeof error));commit(e);assert(st_engine_duration(e,1)==6000.0/48000);
        extent(e,8000,8);st_engine_clear_project(e);commit(e);assert(st_engine_duration(e,1)==8);assert(!st_engine_set_frozen_song_extent(e,8,8000,error,sizeof error));
        puts("PASS bounded extent/publication admission, allocation failure, idempotence and policy reset");}
    {for(bool mapped:{false,true})for(int64_t end:{6005,6007}){Engine e;fixture(e,mapped);extent(e,end);assert(st_engine_duration(e,1)==double(end)/48000);assert(st_engine_duration(e,0)==6000.0/48000);
            st_engine_play(e,1);render(e,0,uint32_t(end+50));assert(onsets(e)==1);auto before=span(e,end-1),tail=span(e,end);
            assert(before.frames==1&&before.context.phase==ST_FREEZE_SONG);assert(tail.context.phase==ST_FREEZE_NATURAL_TAIL&&tail.context.cause==ST_FREEZE_CAUSE_NATURAL_END);
            assert(tail.context.transitionProjectFrame==end&&tail.context.transitionContentGraphFrame==end&&tail.context.projectFrame==end);
            assert(span(e,end+20,1).context.projectFrame==end+20&&span(e,end+20,1,20).context.projectFrame==end);
            assert(st_engine_status(e).seconds==double(end)/48000&&!st_engine_status(e).playing);
        }puts("PASS exact6005/6007 ends avoid both floating roundtrip directions in legacy and mapped songs");}
    {for(bool mapped:{false,true}){Engine e;fixture(e,mapped);std::vector<float> pcm(1000,.2f);st_engine_begin_update(e);assert(st_engine_set_audio_asset(e,0,pcm.data(),nullptr,pcm.size(),48000));st_engine_add_audio_clip(e,0,0,.25,0,0,1,0,0,0);commit(e);
            assert(st_engine_duration(e,1)==7000.0/48000);char error[256];assert(st_engine_set_frozen_song_extent(e,7,6999,error,sizeof error));assert(!st_engine_commit_update_checked(e,error,sizeof error)&&std::strstr(error,"shorter"));assert(st_engine_duration(e,1)==7000.0/48000);st_engine_cancel_update(e);
            extent(e,7000);st_engine_begin_update(e);st_engine_clear_audio_clips(e);assert(st_engine_set_audio_asset(e,0,nullptr,nullptr,0,48000));commit(e);assert(st_engine_duration(e,1)==7000.0/48000);
            st_engine_play(e,1);render(e,0,7050);assert(span(e,6999,1).context.phase==ST_FREEZE_SONG&&span(e,7000).context.transitionProjectFrame==7000);assert(onsets(e)==1);
        }puts("PASS removed source-limited clips keep exact full-song end without retaining original PCM");}
    {Engine e;fixture(e);extent(e,6007);char error[256];st_engine_begin_update(e);st_engine_set_pattern(e,0,2);assert(!st_engine_commit_update_checked(e,error,sizeof error));assert(st_engine_duration(e,1)==6007.0/48000);st_engine_cancel_update(e);
        std::vector<float> pcm(7000,.2f);st_engine_begin_update(e);assert(st_engine_set_audio_asset(e,0,pcm.data(),nullptr,pcm.size(),48000));st_engine_add_audio_clip(e,0,0,0,0,0,1,0,0,0);assert(!st_engine_commit_update_checked(e,error,sizeof error));st_engine_cancel_update(e);
        st_engine_play(e,1);render(e,0,6020);assert(span(e,6007).context.transitionProjectFrame==6007&&onsets(e)==1);
        Engine fractional;fixture(fractional);st_engine_begin_update(fractional);assert(st_engine_set_audio_asset(fractional,0,pcm.data(),nullptr,pcm.size(),48000));st_engine_add_audio_clip(fractional,0,0,0,.25000000002,0,1,0,0,0);assert(st_engine_set_frozen_song_extent(fractional,7,6000,error,sizeof error));assert(!st_engine_commit_update_checked(fractional,error,sizeof error));st_engine_cancel_update(fractional);
        st_engine_begin_update(fractional);assert(st_engine_set_audio_asset(fractional,0,pcm.data(),nullptr,pcm.size(),48000));st_engine_add_audio_clip(fractional,0,0,0,.25000000002,0,1,0,0,0);extent(fractional,6001);st_engine_play(fractional,1);render(fractional,0,6020);assert(span(fractional,6001).context.transitionProjectFrame==6001);
        // Mapped projected integer6007 must not ceil a frame->seconds roundtrip.
        for(bool isMapped:{false,true}){Engine projected;fixture(projected,isMapped);std::vector<float> exact(6007,.2f);st_engine_begin_update(projected);assert(st_engine_set_audio_asset(projected,0,exact.data(),nullptr,exact.size(),48000));st_engine_add_audio_clip(projected,0,0,0,0,0,1,0,0,0);extent(projected,6007);st_engine_play(projected,1);render(projected,0,6008);assert(span(projected,6007,1).context.transitionProjectFrame==6007);}
        Engine tiny(8000);fixture(tiny,false,false,960);float one=.2f;st_engine_begin_update(tiny);assert(st_engine_set_audio_asset(tiny,0,&one,nullptr,1,8000));st_engine_add_audio_clip(tiny,0,0,100000,1e-12,0,1,0,0,0);assert(st_engine_set_frozen_song_extent(tiny,7,50000000,error,sizeof error));assert(!st_engine_commit_update_checked(tiny,error,sizeof error));st_engine_cancel_update(tiny);
        st_engine_begin_update(tiny);assert(st_engine_set_audio_asset(tiny,0,&one,nullptr,1,8000));st_engine_add_audio_clip(tiny,0,0,100000,1e-12,0,1,0,0,0);extent(tiny,50000001);
        puts("PASS checked commit rejects later live arrangement changes atomically and compares integer boundaries");}
    {for(bool mapped:{false,true}){Engine e;fixture(e,mapped,true);extent(e,6007);st_engine_play(e,1);assert(st_engine_input_note(e,1,ST_INPUT_NOTE_ON,0,0,60,100,0));render(e,0,18022);STInputAck first{};assert(st_engine_pop_input_ack(e,&first));
            auto initial=span(e,0,60000),next=span(e,6007),third=span(e,12014);assert(initial.frames==6007&&next.frames==6007&&third.frames==6007);
            assert(next.context.projectFrame==0&&next.context.cause==ST_FREEZE_CAUSE_LOOP&&next.context.playbackID>initial.context.playbackID&&third.context.playbackID>next.context.playbackID);
            assert(st_engine_input_note(e,2,ST_INPUT_NOTE_OFF,0,0,60,0,0));render(e,18022,1);STInputAck last{};assert(st_engine_pop_input_ack(e,&last)&&last.runID==first.runID&&last.distanceBeats>first.distanceBeats);
            STClockSpan clock{};assert(st_engine_clock_span(e,6006,1,0,&clock)==ST_CLOCK_OK);double expected=mapped?.25+7.0/48000:6007.0/24000;assert(std::abs(clock.context.loopEndBeat-expected)<1e-12);
        }puts("PASS exact multi-loop span/clock boundaries preserve independent continuous input-ACK run");}
    {for(bool mapped:{false,true}){Engine e;fixture(e,mapped);extent(e,9007);st_engine_play(e,1);render(e,0,7003);auto original=st_engine_capture_transport(e);assert(original.playing&&original.sequenceFinished&&original.transportFrames==7003);
            st_engine_restore_transport(e,original,1);render(e,0,2005);assert(onsets(e)==0);assert(span(e,2003,1).context.projectFrame==9006&&span(e,2004,1).context.phase==ST_FREEZE_NATURAL_TAIL&&span(e,2004,1).context.transitionProjectFrame==9007);
            st_engine_restore_transport(e,original,1);render(e,0,2003);st_engine_stop(e);render(e,2003,2);assert(span(e,2003).context.phase==ST_FREEZE_STOPPED&&span(e,2003).context.cause==ST_FREEZE_CAUSE_STOP&&onsets(e)==0);
        }puts("PASS mid-extension restore keeps cursor without retrigger and Stop preempts natural completion");}
    {for(bool mapped:{false,true}){Engine e;fixture(e,mapped,true);extent(e,12007);st_engine_play(e,1);render(e,0,9000);onsets(e);char error[256];st_engine_begin_update(e);assert(st_engine_set_frozen_song_policy(e,1,8,error,sizeof error));extent(e,7007,8);render(e,9000,100);
            auto s=span(e,9000);assert(s.context.phase==ST_FREEZE_STOPPED&&s.context.cause==ST_FREEZE_CAUSE_REPLACEMENT&&s.context.projectFrame==7007&&!st_engine_status(e).playing&&onsets(e)==0);
            auto invalid=st_engine_capture_transport(e);invalid.playing=1;invalid.transportFrames=9000;st_engine_restore_transport(e,invalid,1);render(e,0,100);
            assert(span(e,0).context.phase==ST_FREEZE_STOPPED&&span(e,0).context.cause==ST_FREEZE_CAUSE_RESTORE&&span(e,0).context.projectFrame==7007&&!st_engine_status(e).playing&&onsets(e)==0);
        }puts("PASS incompatible replacement/restored cursors clamp stopped without future notes, loops or fabricated tails");}
    {Engine e;fixture(e,false,false,123,4,.3f);st_engine_play(e,1);st_engine_seek(e,0,1,0);render(e,0,111);auto original=st_engine_capture_transport(e);char error[256];extent(e,30007);st_engine_restore_transport(e,original,1);render(e,0,100);assert(onsets(e)==0);auto during=st_engine_capture_transport(e);assert(during.transportFrames==original.transportFrames+100);
        assert(st_engine_set_frozen_song_policy(e,0,0,error,sizeof error));commit(e);st_engine_restore_transport(e,original,1);auto rollback=st_engine_capture_transport(e);assert(rollback.transportFrames==original.transportFrames&&rollback.rowRemaining==original.rowRemaining&&rollback.playing==original.playing);
        assert(st_engine_set_frozen_song_policy(e,1,9,error,sizeof error));extent(e,30007,9);st_engine_restore_transport(e,during,1);assert(st_engine_duration(e,1)==30007.0/48000);st_engine_play(e,0);render(e,0,1);assert(span(e,0).context.phase==ST_FREEZE_UNSUPPORTED&&st_engine_duration(e,0)<st_engine_duration(e,1));
        puts("PASS fractional row checkpoint rollback republishes old policy and Pattern audition ignores full-song override");}
    {for(double rate:{8000.0,48000.0,384000.0}){Engine e(rate);fixture(e);extent(e,100000000,7);st_engine_play(e,1);auto near=st_engine_capture_transport(e);near.transportFrames=99999999;near.sequenceFinished=1;near.rowHasStarted=1;near.rowRemaining=1e100;st_engine_restore_transport(e,near,1);render(e,0,2);assert(span(e,0,1).context.projectFrame==99999999&&span(e,1,1).context.transitionProjectFrame==100000000);assert(st_engine_duration(e,1)==100000000.0/rate);
        }puts("PASS maximum admitted extent/sample-rate bounds render exact end without long-duration allocation");}
    assert(allocations==0&&deallocations==0);puts("Frozen extent: 9 groups passed; zero measured render/query C++ allocation/deallocation.");
}

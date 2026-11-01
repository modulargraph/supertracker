// Targeted regressions from the 660cfb9 engine audit. Pure C++, no native host.
#include "TrackerEngine.h"
#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <new>
#include <stdexcept>
#include <vector>

static thread_local bool rendering=false;
static std::atomic<size_t> renderAllocations{0};
void* operator new(size_t n){if(rendering)++renderAllocations;if(void*p=std::malloc(n?n:1))return p;throw std::bad_alloc();}
void* operator new[](size_t n){return ::operator new(n);}
void operator delete(void*p)noexcept{std::free(p);}
void operator delete[](void*p)noexcept{std::free(p);}

static void require(bool ok,const char*message){if(!ok)throw std::runtime_error(message);}
struct Engine{STEngine*e=st_engine_create(48000);Engine(){require(e,"create");}~Engine(){st_engine_destroy(e);}operator STEngine*()const{return e;}};
static std::vector<float> audio(STEngine*e,uint32_t count){std::vector<float>l(count),r(count);rendering=true;st_engine_render(e,l.data(),r.data(),count);rendering=false;return l;}
static void setup(STEngine*e,bool mapped=false,int rows=4){
    st_engine_begin_update(e);st_engine_set_track_count(e,1);st_engine_set_timing(e,120,4,0,1);
    auto track=st_default_track();track.volume=1;st_engine_set_track(e,0,track);st_engine_set_host_mixing(e,1,1);
    st_engine_set_pattern(e,0,rows);int32_t sequence[]={0};st_engine_set_sequence(e,sequence,1,0,0);
    if(mapped){STClockTempoEvent tempo[]={{0,120}};STClockMeterEvent meter[]={{0,4,4}};char error[256];require(st_engine_set_timing_map(e,tempo,1,meter,1,55,error,sizeof error),error);}
    char error[256];require(st_engine_commit_update_checked(e,error,sizeof error),error);
}
static std::vector<float> printPCM(){std::vector<float>pcm(60000);for(size_t n=0;n<pcm.size();++n)pcm[n]=.1f+float(n)*.000005f;return pcm;}
static void installPrint(STEngine*e){auto pcm=printPCM();st_engine_begin_update(e);require(st_engine_set_audio_asset(e,0,pcm.data(),nullptr,pcm.size(),48000),"head asset");for(float&v:pcm)v=-v;require(st_engine_set_audio_asset(e,1,pcm.data(),nullptr,pcm.size(),48000),"loop asset");st_engine_add_pattern_clip(e,0,1,0,0,1);st_engine_commit_update(e);}
static void temporarySongRender(STEngine*e){st_engine_stop(e);st_engine_seek(e,0,0,0);st_engine_play(e,1);audio(e,73);}
static void checkpointPrint(bool mapped,bool afterWrap){
    Engine e;setup(e,mapped);installPrint(e);st_engine_play(e,0);audio(e,afterWrap?25000:1000);
    auto state=st_engine_capture_transport(e);temporarySongRender(e);rendering=true;st_engine_restore_transport(e,state,1);rendering=false;
    auto out=audio(e,500);auto pcm=printPCM();float sign=afterWrap?-1:1;
    if(std::abs(out[0]-sign*pcm[1000])>1e-7f)std::printf("checkpoint mapped=%d loop=%d actual=%f expected=%f\n",mapped,afterWrap,out[0],sign*pcm[1000]);
    for(size_t n=0;n<out.size();++n)require(out[n]==sign*pcm[1000+n],"Pattern print checkpoint must restore exact cursor and head/loop asset");
    require(st_engine_status(e).row==0&&st_engine_status(e).playing,"Checkpoint musical row preserved");
}
static void sectionClock(bool mapped){
    Engine e;setup(e,mapped,1);st_engine_begin_update(e);st_engine_set_pattern(e,1,1);st_engine_set_pattern(e,2,1);int32_t order[]={0,1,2};st_engine_set_sequence(e,order,3,1,0);st_engine_set_song_loop(e,1,1);st_engine_commit_update(e);st_engine_play(e,1);
    std::vector<float>l(16000),r(l.size());rendering=true;st_engine_render_bus(e,0,0,l.data(),r.data(),uint32_t(l.size()));rendering=false;
    STClockSpan span{};require(st_engine_clock_span(e,6000,1,0,&span)==ST_CLOCK_OK,"Retained section context");
    if(span.context.nextBoundaryFrame!=12000)std::printf("section mapped=%d boundary=%lld expected=12000\n",mapped,(long long)span.context.nextBoundaryFrame);
    require(span.context.nextBoundaryFrame==12000,"Section loop context predicts actual loop boundary, not song end");
    require(span.context.nextBoundaryKinds&ST_CLOCK_BOUNDARY_LOOP,"Section loop boundary kind");
    require(span.context.nextQuarterBeatFrame==INT64_MAX,"No integer beat is reachable in a .25-.5 beat loop");
    require(st_engine_clock_span(e,12000,1,0,&span)==ST_CLOCK_OK&&span.context.songFrame==6000,"Section wraps exact project frame");
}
static void sectionNextQuarter(bool mapped){
    Engine e;setup(e,mapped,1);st_engine_begin_update(e);st_engine_set_pattern(e,1,4);st_engine_set_pattern(e,2,1);int32_t order[]={0,1,2};st_engine_set_sequence(e,order,3,1,0);st_engine_set_song_loop(e,1,1);st_engine_commit_update(e);st_engine_play(e,1);
    std::vector<float>l(31000),r(l.size());rendering=true;st_engine_render_bus(e,0,0,l.data(),r.data(),uint32_t(l.size()));rendering=false;
    STClockSpan span{};require(st_engine_clock_span(e,27000,1,0,&span)==ST_CLOCK_OK,"Section next-beat context");
    require(span.context.nextBoundaryFrame==30000,"Quarter query loop boundary");
    require(span.context.nextQuarterBeatFrame==48000,"Next quarter after wrap must subtract nonzero loop origin");
    require(st_engine_clock_span(e,27127,1,127,&span)==ST_CLOCK_OK&&span.context.nextQuarterBeatFrame==48127,"Upstream PDC shifts next quarter exactly once");
}
static void checkpointBounds(){
    for(bool mapped:{false,true}){
        Engine e;setup(e,mapped);installPrint(e);st_engine_play(e,0);audio(e,7250);auto good=st_engine_capture_transport(e);
        for(int invalid=0;invalid<5;++invalid){auto state=good;
            if(invalid==0){state.patternClipValid=0;state.patternClipFrame=0;state.patternClipPattern=0;state.patternClipLooped=0;}
            if(invalid==1)state.patternClipFrame=INT64_MAX;
            if(invalid==2)state.patternClipFrame=-1;
            if(invalid==3)state.patternClipPattern=99;
            if(invalid==4)state.patternClipLooped=2;
            temporarySongRender(e);rendering=true;st_engine_restore_transport(e,state,1);rendering=false;auto out=audio(e,64);auto pcm=printPCM();
            for(size_t n=0;n<out.size();++n)require(out[n]==pcm[7250+n],"Invalid/default print checkpoint reconstructs row-local head position");
        }
        temporarySongRender(e);st_engine_restore_transport(e,good,0);auto silent=audio(e,100);require(std::all_of(silent.begin(),silent.end(),[](float x){return x==0;}),"Canceled export must not resume frozen PCM");
    }
}
static void checkpointAtBoundary(){
    for(bool mapped:{false,true}){
        Engine e;setup(e,mapped);installPrint(e);st_engine_play(e,0);audio(e,24000);auto state=st_engine_capture_transport(e);require(state.rowRemaining==0,"Pending wrap boundary");
        temporarySongRender(e);st_engine_restore_transport(e,state,1);auto out=audio(e,64);auto pcm=printPCM();
        for(size_t n=0;n<out.size();++n)require(out[n]==-pcm[n],"Pending natural wrap restores then selects steady loop print");
    }
}
static void silentSampleRegion(){
    for(int mode:{ST_MODE_ONESHOT,ST_MODE_FORWARD,ST_MODE_REVERSE,ST_MODE_PINGPONG,ST_MODE_GRANULAR,ST_MODE_WAVETABLE}){
        Engine e;setup(e);std::vector<float>pcm(1024,.8f);std::fill(pcm.begin()+256,pcm.begin()+768,0);
        st_engine_begin_update(e);auto instrument=st_default_instrument();instrument.type=ST_INSTRUMENT_SAMPLE;instrument.mode=mode;
        instrument.start=.25f;instrument.end=.75f;instrument.attack=0;instrument.decay=0;instrument.sustain=1;instrument.volume=1;instrument.tune=-12;instrument.grainPosition=.01f;
        st_engine_set_instrument(e,0,instrument);require(st_engine_set_sample(e,0,pcm.data(),nullptr,pcm.size(),48000),"Silent selection asset");st_engine_commit_update(e);st_engine_note_on(e,0,0,60,127);
        auto out=audio(e,3000);float peak=0;for(float value:out)peak=std::max(peak,std::abs(value));if(peak>0)std::printf("silent selected region mode=%d contaminated peak=%g\n",mode,peak);
        require(peak==0,"Interpolation must not borrow excluded audio outside a silent playback region");
    }
}
static void silentSlice(){
    Engine e;setup(e);std::vector<float>pcm(1024,.8f);std::fill(pcm.begin()+512,pcm.end(),0);
    st_engine_begin_update(e);auto instrument=st_default_instrument();instrument.type=ST_INSTRUMENT_SAMPLE;instrument.mode=ST_MODE_SLICE;instrument.sliceCount=2;
    instrument.attack=0;instrument.decay=0;instrument.sustain=1;instrument.volume=1;instrument.tune=-12;
    st_engine_set_instrument(e,0,instrument);require(st_engine_set_sample(e,0,pcm.data(),nullptr,pcm.size(),48000),"Silent slice asset");st_engine_commit_update(e);st_engine_note_on(e,0,0,61,127);
    auto out=audio(e,3000);require(std::all_of(out.begin(),out.end(),[](float v){return v==0;}),"Pitched silent slice must not borrow preceding slice audio");
}
// Legacy (unmapped) frames are bpm-relative: a tempo change while playing keeps the row in both modes.
static void legacyTempoChange(){
    for(int song:{0,1}){
        Engine e;setup(e,false,16);st_engine_begin_update(e);int32_t order[]={0,0};st_engine_set_sequence(e,order,2,0,0);st_engine_commit_update(e);
        st_engine_play(e,song);audio(e,72100);require(st_engine_status(e).row==12,"Row 12 before the tempo change");
        st_engine_begin_update(e);st_engine_set_timing(e,60,4,0,1);st_engine_commit_update(e);audio(e,1);
        auto s=st_engine_status(e);if(s.row!=12)std::printf("tempo change song=%d row=%d expected=12\n",song,s.row);require(s.row==12&&s.sequence==0,"Tempo change keeps the playing row");
        audio(e,3*12000);require(st_engine_status(e).row==15,"Rows then advance at the new tempo");
    }
}
// MIDI has no ownership per pitch: an overlapping retrigger of the same AU pitch must end the older
// note first, or its later note-off cuts the newer note short inside the plug-in.
static void externalRetrigger(){
    Engine e;setup(e,false,8);st_engine_begin_update(e);auto instrument=st_default_instrument();instrument.type=ST_INSTRUMENT_AU;st_engine_set_instrument(e,0,instrument);
    auto step=st_default_step();step.instrument=0;step.gate=4;st_engine_set_step(e,0,0,0,step,nullptr,0);st_engine_set_step(e,0,0,2,step,nullptr,0);st_engine_commit_update(e);
    st_engine_play(e,0);std::vector<std::pair<int,int64_t>>events;int64_t at=0;
    for(int block=0;block<60;++block){audio(e,1000);STEvent event;while(st_engine_pop_event(e,&event))if(event.type==ST_EVENT_NOTE_ON||event.type==ST_EVENT_NOTE_OFF)events.push_back({event.type,at+event.frameOffset});at+=1000;}
    require(events.size()>=4,"Two notes and their note-offs");int held=0;
    for(size_t n=0;n<4;++n){held+=events[n].first==ST_EVENT_NOTE_ON?1:-1;require(held==0||held==1,"Never two note-ons of one pitch at once");}
    require(events[1].first==ST_EVENT_NOTE_OFF&&events[1].second==12000&&events[2].first==ST_EVENT_NOTE_ON&&events[2].second==12000,"Retrigger ends the older note at the new onset");
    require(events[3].first==ST_EVENT_NOTE_OFF&&events[3].second==12000+24000,"The newer note keeps its full gate");
}

int main(){int failures=0;auto run=[&](const char*name,auto fn){try{fn();std::printf("PASS %s\n",name);}catch(const std::exception&e){rendering=false;++failures;std::printf("FAIL %s: %s\n",name,e.what());}};
    run("fixed Pattern head checkpoint",[]{checkpointPrint(false,false);});run("mapped Pattern head checkpoint",[]{checkpointPrint(true,false);});
    run("fixed Pattern loop checkpoint",[]{checkpointPrint(false,true);});run("mapped Pattern loop checkpoint",[]{checkpointPrint(true,true);});
    run("fixed section retained clock",[]{sectionClock(false);});run("mapped section retained clock",[]{sectionClock(true);});
    run("fixed section next quarter and PDC",[]{sectionNextQuarter(false);});run("mapped section next quarter and PDC",[]{sectionNextQuarter(true);});
    run("checkpoint bounds and cancellation",checkpointBounds);run("checkpoint at pending wrap",checkpointAtBoundary);
    run("silent sampler playback region",silentSampleRegion);run("silent pitched slice",silentSlice);run("legacy tempo change keeps position",legacyTempoChange);run("external pitch retrigger",externalRetrigger);
    run("zero render allocations",[]{require(renderAllocations.load()==0,"Render/restore allocated");});
    std::printf("%d failed audit regressions\n",failures);return failures?1:0;
}

#include "TrackerEngine.h"
#include <cassert>
#include <cmath>
#include <cstdio>
#include <vector>
#include <algorithm>
struct Fixture {
    STEngine* e=st_engine_create(48000);
    Fixture(int type){assert(e);st_engine_begin_update(e);st_engine_set_track_count(e,1);st_engine_set_timing(e,120,4,0,.7f);st_engine_set_pattern(e,0,16);auto i=st_default_instrument();i.type=type;i.attack=0;i.decay=0;i.sustain=1;i.release=.001f;i.cutoff=22000;i.volume=.5f;st_engine_set_instrument(e,0,i);}
    ~Fixture(){st_engine_destroy(e);}
    void step(int row,int note,int fx=0,float value=0,int fx2=0,float value2=0,float gate=32){auto s=st_default_step();s.note=note;s.gate=gate;s.fx1={fx,value};s.fx2={fx2,value2};st_engine_set_step(e,0,0,row,s,nullptr,0);}
    void play(){st_engine_commit_update(e);st_engine_play(e,0);}
    std::vector<std::pair<int,int>> notes(int frames,int block){std::vector<std::pair<int,int>> out;for(int at=0;at<frames;at+=block){int n=std::min(block,frames-at);std::vector<float>l(n),r(n);st_engine_render(e,l.data(),r.data(),n);STEvent event;while(st_engine_pop_event(e,&event))if(event.type==ST_EVENT_NOTE_ON)out.push_back({at+event.frameOffset,event.note});}return out;}
    std::vector<int> onsets(int frames,int block){std::vector<int> out;for(auto&n:notes(frames,block))out.push_back(n.first);return out;}
    void ramp(){std::vector<float>sample(48000);for(int i=0;i<48000;++i)sample[i]=.1f+.8f*i/48000;assert(st_engine_set_sample(e,0,sample.data(),nullptr,sample.size(),48000));}
    std::vector<float> pcm(int frames){std::vector<float>l(frames),r(frames);st_engine_render(e,l.data(),r.data(),frames);return l;}
};
// Row 2's plain note stops the sticky ratchet (it plays once); row 3's effect-only Ratchet then repeats it.
static void stickyRatchet(int block,bool mapped=false){Fixture f(ST_INSTRUMENT_MIDI);if(mapped){STClockTempoEvent tempo[]={{0,120}};STClockMeterEvent meter[]={{0,4,4}};char error[256]{};assert(st_engine_set_timing_map(f.e,tempo,1,meter,1,1,error,sizeof error));}f.step(0,60,ST_FX_RATCHET,2);f.step(2,64);f.step(3,-3,ST_FX_RATCHET,4);f.step(4,-3,ST_FX_PAN,.5f);f.step(6,-3,ST_FX_RATCHET,2);f.step(7,-1);f.play();auto on=f.notes(54000,block);std::vector<std::pair<int,int>>expected={{0,60},{3000,60},{6000,60},{9000,60},{12000,64},{18000,64},{19500,64},{21000,64},{22500,64},{36000,64},{39000,64}};assert(on==expected);}
// A note dropped by its probability roll must leave the sticky ratchet exactly as it was: the held note keeps its own rate.
static void probabilityDropKeepsRatchet(){
    {Fixture f(ST_INSTRUMENT_MIDI);f.step(0,60,0,0,0,0,.5f);f.step(1,64,ST_FX_RATCHET,4,ST_FX_PROBABILITY,0,.5f);f.play();assert(f.notes(24000,256)==(std::vector<std::pair<int,int>>{{0,60}}));}
    {Fixture f(ST_INSTRUMENT_MIDI);f.step(0,60,ST_FX_RATCHET,2,0,0,.5f);f.step(1,64,ST_FX_RATCHET,4,ST_FX_PROBABILITY,0,.5f);f.step(3,-1);f.play();assert(f.notes(24000,256)==(std::vector<std::pair<int,int>>{{0,60},{3000,60},{6000,60},{9000,60},{12000,60},{15000,60}}));}
}
// An effect-only cell whose effects were all removed (note -3, no FX) is empty: the sticky ratchet continues through it.
static void emptyEffectCellKeepsRatchet(){Fixture f(ST_INSTRUMENT_MIDI);f.step(0,60,ST_FX_RATCHET,2);f.step(2,-3);f.play();assert(f.onsets(30000,256)==std::vector<int>({0,3000,6000,9000,12000,15000,18000,21000,24000,27000}));}
// Ratchet repeats replay the note's own playback effects: reverse, offset and effect-only Reverse alongside Ratchet.
static void ratchetKeepsPlaybackEffects(){
    {Fixture f(ST_INSTRUMENT_SAMPLE);f.ramp();f.step(0,60,ST_FX_RATCHET,2,ST_FX_REVERSE,1,.9f);f.play();auto l=f.pcm(12000);for(int hit=0;hit<4;++hit){int at=hit*3000;assert(l[at+1500]<l[at+1000]);}}
    {Fixture f(ST_INSTRUMENT_SAMPLE);f.ramp();f.step(0,60,ST_FX_RATCHET,2,ST_FX_OFFSET,.5f,.9f);f.play();auto l=f.pcm(12000);for(int hit=1;hit<4;++hit){int at=hit*3000;assert(std::abs(l[at+200]-l[200])<.01f*l[200]);assert(l[at+1500]>l[at+1000]);}}
    {Fixture f(ST_INSTRUMENT_SAMPLE);f.ramp();f.step(0,60,0,0,0,0,.9f);f.step(1,-3,ST_FX_RATCHET,4,ST_FX_REVERSE,1);f.step(2,-3,ST_FX_RATCHET,1);f.play();auto l=f.pcm(12000);assert(l[600]>l[300]);for(int hit=0;hit<4;++hit){int at=6000+hit*1500;assert(l[at+1000]<l[at+300]&&l[at+1000]>.05f);}}
}
static void ratchetOff(){for(int mode=0;mode<3;++mode){Fixture f(ST_INSTRUMENT_MIDI);f.step(0,60,ST_FX_RATCHET,2);if(mode==0)f.step(1,-3,ST_FX_RATCHET,1);else if(mode==1)f.step(1,-2);else{auto s=st_default_step();s.velocity=0;st_engine_set_step(f.e,0,0,1,s,nullptr,0);}f.play();assert(f.onsets(18000,257)==std::vector<int>({0,3000}));}}
static void acrossPatternsAndRefresh(){Fixture f(ST_INSTRUMENT_MIDI);st_engine_set_pattern(f.e,0,1);st_engine_set_pattern(f.e,1,3);int32_t sequence[]={0,1};st_engine_set_sequence(f.e,sequence,2,0,0);f.step(0,60,ST_FX_RATCHET,2);st_engine_commit_update(f.e);st_engine_play(f.e,1);assert(f.onsets(6000,257)==std::vector<int>({0,3000}));st_engine_begin_update(f.e);auto mix=st_default_track();mix.pan=.25f;st_engine_set_track(f.e,0,mix);st_engine_commit_update(f.e);assert(f.onsets(12000,257)==std::vector<int>({0,3000,6000,9000}));}
static void sampleCommands(){Fixture f(ST_INSTRUMENT_SAMPLE);std::vector<float>sample(48000);for(int i=0;i<48000;++i)sample[i]=.1f+.8f*i/48000;assert(st_engine_set_sample(f.e,0,sample.data(),nullptr,sample.size(),48000));f.step(0,60);f.step(1,-3,ST_FX_REVERSE,1);f.step(2,-3,ST_FX_OFFSET,.25f);f.step(3,-3,ST_FX_REVERSE,0);f.play();std::vector<float>l(24000),r(24000);st_engine_render(f.e,l.data(),r.data(),24000);assert(l[5000]>l[4000]);assert(l[8000]<l[7000]);assert(l[13000]>l[11000]*2);assert(l[15000]<l[14000]);assert(l[21000]>l[20000]);for(float x:l)assert(std::isfinite(x));}

// Ratchet stop rules: a sticky ratchet continues through empty rows only. A new note on the lane plays
// once (unless it has its own Ratchet); OFF and CUT stop the repeats. Each case is per track, so kit pad
// tracks (ordinary tracks in a group) behave the same.
static void ratchetStopRules(){
    // Ratchet then a plain note: the note plays once, the repeats stop.
    {Fixture f(ST_INSTRUMENT_MIDI);f.step(0,60,ST_FX_RATCHET,2);f.step(2,64);f.play();assert(f.notes(30000,256)==(std::vector<std::pair<int,int>>{{0,60},{3000,60},{6000,60},{9000,60},{12000,64}}));}
    // A note with its own Ratchet takes over at its rate.
    {Fixture f(ST_INSTRUMENT_MIDI);f.step(0,60,ST_FX_RATCHET,2);f.step(1,64,ST_FX_RATCHET,3);f.step(2,-1);f.play();assert(f.notes(18000,256)==(std::vector<std::pair<int,int>>{{0,60},{3000,60},{6000,64},{8000,64},{10000,64}}));}
    // Ratchet then OFF / CUT: stops.
    for(int note:{-1,-2}){Fixture f(ST_INSTRUMENT_MIDI);f.step(0,60,ST_FX_RATCHET,2);f.step(2,note);f.play();assert(f.onsets(30000,256)==(std::vector<int>{0,3000,6000,9000}));}
    // Ratchet through empty rows continues.
    {Fixture f(ST_INSTRUMENT_MIDI);f.step(0,60,ST_FX_RATCHET,2);f.step(4,-1);f.play();assert(f.onsets(30000,256)==(std::vector<int>{0,3000,6000,9000,12000,15000,18000,21000}));}
    // Two lanes (two pads of a kit): a note on one lane never stops the other lane's ratchet.
    {STEngine*e=st_engine_create(48000);st_engine_begin_update(e);st_engine_set_track_count(e,2);st_engine_set_timing(e,120,4,0,.7f);st_engine_set_pattern(e,0,16);
     for(int t=0;t<2;++t){auto i=st_default_instrument();i.type=ST_INSTRUMENT_MIDI;st_engine_set_instrument(e,t,i);auto tr=st_default_track();tr.instrument=t;st_engine_set_track(e,t,tr);}
     auto put=[&](int track,int row,int note,int fx,float v){auto s=st_default_step();s.note=note;s.gate=0;s.fx1={fx,v};st_engine_set_step(e,0,track,row,s,nullptr,0);};
     put(0,0,36,ST_FX_RATCHET,2);put(1,1,38,0,0);put(0,2,36,0,0);put(1,3,38,ST_FX_RATCHET,2);put(1,4,-1,0,0);
     st_engine_commit_update(e);st_engine_play(e,0);std::vector<std::pair<int,int>>on;for(int at=0;at<36000;at+=256){float l[256],r[256];st_engine_render(e,l,r,256);STEvent ev;while(st_engine_pop_event(e,&ev))if(ev.type==ST_EVENT_NOTE_ON)on.push_back({at+ev.frameOffset,ev.note});}
     std::sort(on.begin(),on.end());assert(on==(std::vector<std::pair<int,int>>{{0,36},{3000,36},{6000,36},{6000,38},{9000,36},{12000,36},{18000,38},{21000,38}}));st_engine_destroy(e);}
}
// Note lifetime without a gate (gate 0): the note sounds until the next note on its lane (a note-off with
// the release envelope), an OFF/CUT, or stop. Gated notes keep their explicit length.
static std::vector<std::pair<int,int>> noteEvents(Fixture&f,int frames,int block){std::vector<std::pair<int,int>>out;for(int at=0;at<frames;at+=block){int n=std::min(block,frames-at);std::vector<float>l(n),r(n);st_engine_render(f.e,l.data(),r.data(),n);STEvent ev;while(st_engine_pop_event(f.e,&ev))if(ev.type==ST_EVENT_NOTE_ON||ev.type==ST_EVENT_NOTE_OFF)out.push_back({at+ev.frameOffset,ev.type==ST_EVENT_NOTE_ON?ev.note:-ev.note});}return out;}
static void gatelessLifetime(){
    using V=std::vector<std::pair<int,int>>;
    // Next note releases: note-off for 60 at the frame the next note starts, before its note-on.
    {Fixture f(ST_INSTRUMENT_MIDI);f.step(0,60,0,0,0,0,0);f.step(3,64,0,0,0,0,0);f.step(6,-1);f.play();assert(noteEvents(f,48000,256)==(V{{0,60},{18000,-60},{18000,64},{36000,-64}}));}
    // CUT, and the default block size of one frame.
    {Fixture f(ST_INSTRUMENT_MIDI);f.step(0,60,0,0,0,0,0);f.step(5,-2);f.play();assert(noteEvents(f,48000,1)==(V{{0,60},{30000,-60}}));}
    // A chord on one row: its notes leave each other sounding; the next row's note releases all of them.
    {Fixture f(ST_INSTRUMENT_MIDI);auto s=st_default_step();s.note=60;s.gate=0;int32_t chord[]={0,4,7};st_engine_set_step(f.e,0,0,0,s,chord,3);f.step(2,62,0,0,0,0,0);f.step(3,-1);f.play();
     auto ev=noteEvents(f,24000,256);assert(ev.size()==8);for(int n=0;n<3;++n)assert(ev[size_t(n)].first==0&&ev[size_t(n)].second>0);for(int n=3;n<6;++n)assert(ev[size_t(n)].first==12000&&ev[size_t(n)].second<0);assert((ev[6]==std::pair<int,int>{12000,62})&&(ev[7]==std::pair<int,int>{18000,-62}));}
    // A gated note keeps its own length; a later gateless note does not end before the next note.
    {Fixture f(ST_INSTRUMENT_MIDI);f.step(0,60,0,0,0,0,.5f);f.step(1,62,0,0,0,0,0);f.step(8,-1);f.play();assert(noteEvents(f,60000,256)==(V{{0,60},{3000,-60},{6000,62},{48000,-62}}));}
    // An explicit long gate is not shortened by the next note (old songs sound unchanged).
    {Fixture f(ST_INSTRUMENT_MIDI);f.step(0,60,0,0,0,0,4);f.step(1,62,0,0,0,0,.5f);f.play();assert(noteEvents(f,30000,256)==(V{{0,60},{6000,62},{9000,-62},{24000,-60}}));}
    // Stop ends a gateless note.
    {Fixture f(ST_INSTRUMENT_MIDI);f.step(0,60,0,0,0,0,0);f.play();assert(noteEvents(f,12000,256)==(V{{0,60}}));st_engine_stop(f.e);auto after=noteEvents(f,512,256);assert(!after.empty()&&after[0].second==-60);}
    // Microtiming delays the release to the next note's actual onset; a probability-dropped note releases nothing.
    {Fixture f(ST_INSTRUMENT_MIDI);f.step(0,60,0,0,0,0,0);f.step(1,62,ST_FX_MICROTIMING,.5f,0,0,0);f.step(2,64,ST_FX_PROBABILITY,0,0,0,0);f.step(4,-1);f.play();assert(noteEvents(f,30000,256)==(V{{0,60},{9000,-60},{9000,62},{24000,-62}}));}
    // Ratchet without a gate: each repeat lasts one repeat spacing.
    {Fixture f(ST_INSTRUMENT_MIDI);f.step(0,60,ST_FX_RATCHET,2,0,0,0);f.step(1,-1);f.play();assert(noteEvents(f,12000,256)==(V{{0,60},{3000,-60},{3000,60},{6000,-60}}));}
    // Tempo-mapped playback behaves the same.
    {Fixture f(ST_INSTRUMENT_MIDI);STClockTempoEvent tempo[]={{0,120}};STClockMeterEvent meter[]={{0,4,4}};char error[256]{};assert(st_engine_set_timing_map(f.e,tempo,1,meter,1,1,error,sizeof error));
     f.step(0,60,0,0,0,0,0);f.step(3,64,0,0,0,0,0);f.step(6,-1);f.play();assert(noteEvents(f,48000,256)==(V{{0,60},{18000,-60},{18000,64},{36000,-64}}));}
    // A sampler note sounds until the next note releases it, then fades with the release envelope.
    {Fixture f(ST_INSTRUMENT_SAMPLE);std::vector<float>sample(96000,.5f);assert(st_engine_set_sample(f.e,0,sample.data(),nullptr,sample.size(),48000));
     auto i=st_default_instrument();i.type=ST_INSTRUMENT_SAMPLE;i.attack=0;i.decay=0;i.sustain=1;i.release=.05f;i.cutoff=22000;i.volume=.5f;i.mode=ST_MODE_ONESHOT;st_engine_set_instrument(f.e,0,i);
     f.step(0,60,0,0,0,0,0);auto quiet=st_default_step();quiet.note=60;quiet.velocity=1;quiet.gate=0;st_engine_set_step(f.e,0,0,4,quiet,nullptr,0);f.play();auto l=f.pcm(32000);
     assert(std::abs(l[20000])>.05f);                      // still sounding after three rows
     assert(std::abs(l[24000+600])>std::abs(l[31000])*4);  // released by the next (near-silent) note: fading, not cut
     assert(std::abs(l[31000])<std::abs(l[20000])*.05f);for(float x:l)assert(std::isfinite(x));}
}
int main(){ratchetStopRules();gatelessLifetime();stickyRatchet(1);stickyRatchet(257);stickyRatchet(8192);stickyRatchet(257,true);sampleCommands();ratchetOff();acrossPatternsAndRefresh();probabilityDropKeepsRatchet();emptyEffectCellKeepsRatchet();ratchetKeepsPlaybackEffects();puts("step effect playback regressions passed");}

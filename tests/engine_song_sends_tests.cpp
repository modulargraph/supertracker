// Song order/section-loop playback and shared delay/reverb send return parameters.
#include "TrackerEngine.h"
#include <cassert>
#include <cmath>
#include <cstdio>
#include <vector>

struct Engine { STEngine*e; Engine():e(st_engine_create(48000)){} ~Engine(){st_engine_destroy(e);} operator STEngine*()const{return e;} };
struct Audio { std::vector<float> l,r; explicit Audio(size_t n):l(n),r(n){} double energy(size_t a,size_t b)const{double s=0;for(size_t i=a;i<b&&i<l.size();++i)s+=double(l[i])*l[i]+double(r[i])*r[i];return s;} };
struct Timed { uint64_t frame; STEvent e; };
static void render(STEngine*e,Audio&a){st_engine_render(e,a.l.data(),a.r.data(),uint32_t(a.l.size()));}
static std::vector<Timed> noteOns(STEngine*e,size_t frames,size_t block=256){std::vector<Timed> out;for(size_t at=0;at<frames;at+=block){Audio a(std::min(block,frames-at));render(e,a);STEvent v;while(st_engine_pop_event(e,&v))if(v.type==ST_EVENT_NOTE_ON)out.push_back({at+uint64_t(v.frameOffset),v});}return out;}

// Patterns 0..3 have 4 rows (6000 frames per row at 120 BPM, 4 LPB, 48 kHz); row 0 plays note 60+pattern.
static void configureSong(STEngine*e,const std::vector<int32_t>&sequence,bool loop){
    st_engine_begin_update(e);st_engine_set_track_count(e,1);st_engine_set_timing(e,120,4,0,.7f);
    auto i=st_default_instrument();i.type=ST_INSTRUMENT_MIDI;st_engine_set_instrument(e,0,i);
    for(int p=0;p<4;++p){st_engine_set_pattern(e,p,4);auto s=st_default_step();s.note=60+p;s.gate=1;st_engine_set_step(e,p,0,0,s,nullptr,0);}
    st_engine_set_sequence(e,sequence.data(),int32_t(sequence.size()),loop?1:0,0);st_engine_commit_update(e);
}

static void testRenderedOrderMatchesSequence(){
    Engine e;std::vector<int32_t> seq{2,0,0,3,1};configureSong(e,seq,false);st_engine_play(e,1);
    auto on=noteOns(e,24000*6);assert(on.size()==seq.size());
    for(size_t n=0;n<seq.size();++n){assert(on[n].e.note==60+seq[n]);assert(on[n].frame==n*24000);}
    assert(!st_engine_status(e).playing);
    std::puts("PASS rendered song order and timing match the sequence (repeats included)");
}

static void testSectionLoop(){
    Engine e;std::vector<int32_t> seq{0,1,2,3};configureSong(e,seq,true);
    st_engine_begin_update(e);st_engine_set_song_loop(e,1,2);st_engine_commit_update(e);st_engine_play(e,1);
    auto on=noteOns(e,24000*8);
    // 0, then 1 2 1 2 1 2 ... sample-accurately on 24000-frame boundaries.
    const int expected[]={0,1,2,1,2,1,2,1};assert(on.size()==8);
    for(int n=0;n<8;++n){assert(on[n].e.note==60+expected[n]);assert(on[n].frame==uint64_t(n)*24000);}
    auto status=st_engine_status(e);assert(status.playing&&status.sequence>=1&&status.sequence<=2);
    // Playback clock reflects the jump back to the section start (frame 24000 at 0.5 s).
    assert(status.seconds>=.5-1e-6&&status.seconds<1.5);
    std::puts("PASS section loop repeats sequence slots 2-3 sample-accurately");
}

static void testSectionLoopNeedsLoopSongAndJumpsInFromOutside(){
    {   Engine e;std::vector<int32_t> seq{0,1,2};configureSong(e,seq,false);
        st_engine_begin_update(e);st_engine_set_song_loop(e,0,0);st_engine_commit_update(e);st_engine_play(e,1);
        auto on=noteOns(e,24000*4);assert(on.size()==3&&on[2].e.note==62);assert(!st_engine_status(e).playing);}
    {   // Start after the section: play to the end, then wrap into the section start (not slot 1).
        Engine e;std::vector<int32_t> seq{0,1,2,3};configureSong(e,seq,true);
        st_engine_begin_update(e);st_engine_set_song_loop(e,1,1);st_engine_commit_update(e);
        st_engine_seek(e,2,0,2);st_engine_play(e,1);auto on=noteOns(e,24000*4);
        const int expected[]={2,3,1,1};assert(on.size()==4);for(int n=0;n<4;++n)assert(on[n].e.note==60+expected[n]);}
    {   // Clearing the section returns to whole-song looping.
        Engine e;std::vector<int32_t> seq{0,1};configureSong(e,seq,true);
        st_engine_begin_update(e);st_engine_set_song_loop(e,1,1);st_engine_commit_update(e);st_engine_play(e,1);
        auto on=noteOns(e,24000*3);assert(on.size()==3&&on[2].e.note==61);
        st_engine_begin_update(e);st_engine_set_song_loop(e,-1,-1);st_engine_commit_update(e);
        on=noteOns(e,24000*2);assert(on.size()==2&&on[0].e.note==60&&on[1].e.note==61);}
    std::puts("PASS section loop is inactive without Loop song, wraps in from later slots and clears");
}

static void testSectionLoopWithTempoMap(){
    Engine e;std::vector<int32_t> seq{0,1,2,3};configureSong(e,seq,true);
    STClockTempoEvent tempos[]{{0,120},{2,60}};STClockMeterEvent meters[]{{0,4,4}};char error[256]{};
    st_engine_begin_update(e);assert(st_engine_set_timing_map(e,tempos,2,meters,1,7,error,sizeof error)==1);st_engine_set_song_loop(e,1,2);assert(st_engine_commit_update_checked(e,error,sizeof error)==1);
    st_engine_play(e,1);auto on=noteOns(e,200000);
    const int notes[]={0,1,2,1,2,1};const uint64_t frames[]={0,24000,48000,96000,120000,168000};assert(on.size()>=6);
    for(int n=0;n<6;++n){assert(on[n].e.note==60+notes[n]);assert(on[n].frame>=frames[n]&&on[n].frame<=frames[n]+1);}
    std::puts("PASS section loop follows a tempo map across the loop boundary");
}

// An impulse through the delay send; returns the frame of the first wet echo.
static Audio delayImpulse(double beats,float time,float tone,double bpm=120,size_t frames=60000){
    Engine e;st_engine_begin_update(e);st_engine_set_track_count(e,1);st_engine_set_timing(e,bpm,4,0,1);
    auto i=st_default_instrument();i.type=ST_INSTRUMENT_SAMPLE;i.attack=0;i.decay=0;i.sustain=1;i.release=0.001f;i.volume=1;st_engine_set_instrument(e,0,i);
    std::vector<float> click(16,0);click[0]=.8f;st_engine_set_sample(e,0,click.data(),nullptr,click.size(),48000);
    auto t=st_default_track();t.volume=1;t.delaySend=1;t.reverbSend=0;st_engine_set_track(e,0,t);
    st_engine_set_effects(e,time,0,1,.7f,.4f,0);st_engine_set_send_effects(e,beats,tone,0);st_engine_commit_update(e);
    st_engine_note_on(e,0,0,60,127);Audio a(frames);render(e,a);return a;
}
static size_t firstAbove(const Audio&a,size_t from,float level=1e-4f){for(size_t n=from;n<a.l.size();++n)if(std::abs(a.l[n])>level||std::abs(a.r[n])>level)return n;return a.l.size();}

static void testSyncedDelayAndTone(){
    auto free=delayImpulse(0,.1f,1);size_t echo=firstAbove(free,200);assert(echo>=4795&&echo<=4805);
    auto synced=delayImpulse(.5,.1f,1);echo=firstAbove(synced,200);assert(echo>=11995&&echo<=12005); // eighth note at 120 BPM = 0.25 s
    auto faster=delayImpulse(.5,.1f,1,150);echo=firstAbove(faster,200);assert(echo>=9595&&echo<=9605);  // follows tempo
    // Tone 1 is bit-identical to the legacy open path; a dark tone removes high-frequency energy from the echo.
    auto open=delayImpulse(0,.1f,1),dark=delayImpulse(0,.1f,0);
    double openPeak=0,darkPeak=0;for(size_t n=4790;n<4900;++n){openPeak=std::max(openPeak,double(std::abs(open.l[n])+std::abs(open.r[n])));darkPeak=std::max(darkPeak,double(std::abs(dark.l[n])+std::abs(dark.r[n])));}
    assert(darkPeak<openPeak*.25&&darkPeak>0);
    std::puts("PASS delay syncs to tempo divisions, follows BPM, and tone darkens echoes (open tone unchanged)");
}

// A synced delay whose length is one ULP above a whole frame count (0.75 beats at 140 BPM and 44.1 kHz is
// 14175.000000000002 frames): once per pass of the delay line the read position rounds up to exactly the
// line's size. That is its first sample; reading one past the end lost (or corrupted) that echo frame.
static void testSyncedDelayReadWrapsAtLineEnd(){
    STEngine*e=st_engine_create(44100);assert(e);st_engine_begin_update(e);st_engine_set_track_count(e,1);st_engine_set_timing(e,140,4,0,1);
    auto i=st_default_instrument();i.type=ST_INSTRUMENT_SAMPLE;i.attack=0;i.decay=0;i.sustain=1;i.release=0.001f;i.volume=1;st_engine_set_instrument(e,0,i);
    std::vector<float> click(16,0);click[0]=.8f;st_engine_set_sample(e,0,click.data(),nullptr,click.size(),44100);
    auto t=st_default_track();t.volume=1;t.delaySend=1;t.reverbSend=0;st_engine_set_track(e,0,t);
    st_engine_set_effects(e,.1f,0,1,.7f,.4f,0);st_engine_set_send_effects(e,.75,1,0);st_engine_commit_update(e);
    st_engine_note_on(e,0,0,60,127);Audio a(15000);render(e,a);st_engine_destroy(e);
    // The click's first dry frame returns exactly one delay length later, followed by its second.
    assert(std::abs(a.l[0])>.1f&&std::abs(a.l[1])>.1f);
    assert(std::abs(a.l[14175]-a.l[0])<1e-6f&&std::abs(a.r[14175]-a.r[0])<1e-6f);
    assert(std::abs(a.l[14176]-a.l[1])<1e-6f&&std::abs(a.r[14176]-a.r[1])<1e-6f);
    std::puts("PASS synced delay read wraps inside the delay line when its position rounds up to the line size");
}

static void testReverbPredelay(){
    auto run=[](float predelay){Engine e;st_engine_begin_update(e);st_engine_set_track_count(e,1);st_engine_set_timing(e,120,4,0,1);
        auto i=st_default_instrument();i.type=ST_INSTRUMENT_SAMPLE;i.attack=0;i.decay=0;i.sustain=1;i.release=.001f;i.volume=1;st_engine_set_instrument(e,0,i);
        std::vector<float> click(16,0);click[0]=.8f;st_engine_set_sample(e,0,click.data(),nullptr,click.size(),48000);
        auto t=st_default_track();t.volume=1;t.delaySend=0;t.reverbSend=1;st_engine_set_track(e,0,t);
        st_engine_set_effects(e,.3f,0,0,.7f,.4f,1);st_engine_set_send_effects(e,0,1,predelay);st_engine_commit_update(e);
        st_engine_note_on(e,0,0,60,127);Audio a(24000);render(e,a);return a;};
    auto none=run(0),later=run(.1f);size_t a=firstAbove(none,200),b=firstAbove(later,200);
    assert(b>=a+4795&&b<=a+4805);
    std::puts("PASS reverb pre-delay shifts the wet onset by the chosen time");
}

int main(){testRenderedOrderMatchesSequence();testSectionLoop();testSectionLoopNeedsLoopSongAndJumpsInFromOutside();testSectionLoopWithTempoMap();testSyncedDelayAndTone();testSyncedDelayReadWrapsAtLineEnd();testReverbPredelay();std::puts("engine song/send tests passed");}

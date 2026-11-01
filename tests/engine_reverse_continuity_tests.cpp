#include "TrackerEngine.h"
#include <algorithm>
#include <cassert>
#include <cmath>
#include <cstdio>
#include <vector>
struct Engine { STEngine* e=st_engine_create(48000); ~Engine(){st_engine_destroy(e);} };
static std::vector<float> render(bool rapid,int voices,int block,bool reverseStart=false,bool releaseTail=false,bool oneShot=false,bool zeroCross=false){
    Engine engine;auto e=engine.e;st_engine_begin_update(e);st_engine_set_track_count(e,1);st_engine_set_timing(e,rapid?999:120,rapid?32:4,0,1);st_engine_set_pattern(e,0,16);st_engine_set_host_mixing(e,1,0xffff);
    auto instrument=st_default_instrument();instrument.type=ST_INSTRUMENT_SAMPLE;instrument.mode=reverseStart||oneShot?ST_MODE_ONESHOT:ST_MODE_FORWARD;if(oneShot)instrument.end=.606f;instrument.rootNote=60;instrument.attack=0;instrument.decay=0;instrument.sustain=1;instrument.release=.5f;instrument.volume=.25f;instrument.cutoff=22000;instrument.loopStart=.2f;instrument.loopEnd=rapid?.411f:.605f;if(zeroCross){instrument.loopStart=0;instrument.loopEnd=1;}st_engine_set_instrument(e,0,instrument);
    std::vector<float>sample(10000,.8f);if(!reverseStart)std::fill(sample.begin(),sample.begin()+2000,-.8f);if(zeroCross)for(size_t i=0;i<sample.size();++i)sample[i]=.8f*std::sin(2*3.14159265358979323846*i/480);assert(st_engine_set_sample(e,0,sample.data(),nullptr,sample.size(),48000));
    auto note=st_default_step();note.note=60;note.velocity=127;note.gate=releaseTail?.8f:256;int32_t chord[3]={0,0,0};if(reverseStart)note.fx1={ST_FX_REVERSE,1};else if(rapid)note.fx1={ST_FX_OFFSET,.4f};st_engine_set_step(e,0,0,0,note,chord,voices);
    if(!reverseStart)for(int row=1;row<16;++row){auto command=st_default_step();command.note=-3;command.fx1={ST_FX_REVERSE,float(row%2)};st_engine_set_step(e,0,0,row,command,nullptr,0);}
    st_engine_commit_update(e);st_engine_play(e,0);int frames=reverseStart?200:rapid?1400:13000;std::vector<float>l(frames),r(frames);for(int at=0;at<frames;at+=block){auto n=std::min(block,frames-at);st_engine_render_bus(e,0,at,l.data()+at,r.data()+at,n);}return l;
}
static void continuity(bool rapid,bool tail=false,bool oneShot=false,bool zeroCross=false){auto single=render(rapid,1,257,false,tail,oneShot,zeroCross),poly=render(rapid,3,257,false,tail,oneShot,zeroCross),small=render(rapid,3,1,false,tail,oneShot,zeroCross),large=render(rapid,3,8192,false,tail,oneShot,zeroCross);assert(poly==small&&poly==large);for(size_t i=0;i<single.size();++i){assert(std::isfinite(poly[i]));assert(std::abs(poly[i]-3*single[i])<1e-5f);}
    const double rowFrames=rapid?48000*60.0/999/32:6000;float maximum=0;
    for(int row=1;row<(rapid?15:3);++row){int start=int(std::ceil(rowFrames*row));for(int i=start;i<std::min(int(poly.size()),start+80);++i)maximum=std::max(maximum,std::abs(poly[i]-poly[i-1]));}
    std::printf("reverse %s%s: max adjacent PCM delta %.6f (3 voices)\n",rapid?"rapid":"loop-crossfade",tail?" release tails":oneShot?" one-shot fade":zeroCross?" zero crossing":"",maximum);assert(maximum<.035f);
    assert(*std::max_element(single.begin(),single.end())>.1f);
}
// Turning around in a loop sample's lead-in (before the loop in forward mode, after it in reverse mode) must not
// jump into the loop: the voice runs back through the lead-in to the region edge and fades out there.
static void leadInTurnaround(bool reverseMode){
    Engine engine;auto e=engine.e;st_engine_begin_update(e);st_engine_set_track_count(e,1);st_engine_set_timing(e,120,4,0,1);st_engine_set_pattern(e,0,16);
    auto instrument=st_default_instrument();instrument.type=ST_INSTRUMENT_SAMPLE;instrument.mode=reverseMode?ST_MODE_REVERSE:ST_MODE_FORWARD;instrument.loopStart=reverseMode?0:.5f;instrument.loopEnd=reverseMode?.5f:1;instrument.rootNote=60;instrument.attack=0;instrument.decay=0;instrument.sustain=1;instrument.release=.001f;instrument.volume=.5f;instrument.cutoff=22000;st_engine_set_instrument(e,0,instrument);
    // Lead-in: a cosine (at a peak on the region edges, so the edge needs its fade); loop: a constant .8 that a jump would land on.
    std::vector<float>sample(48000);for(int i=0;i<48000;++i){bool lead=reverseMode?i>=24000:i<24000;sample[i]=lead?.8f*std::cos(2*3.14159265358979323846*i/480):.8f;}
    assert(st_engine_set_sample(e,0,sample.data(),nullptr,sample.size(),48000));
    auto note=st_default_step();note.note=60;note.gate=32;st_engine_set_step(e,0,0,0,note,nullptr,0);auto command=st_default_step();command.note=-3;command.fx1={ST_FX_REVERSE,reverseMode?0.f:1.f};st_engine_set_step(e,0,0,1,command,nullptr,0);
    st_engine_commit_update(e);st_engine_play(e,0);std::vector<float>l(16000),r(16000);for(int at=0;at<16000;at+=257){int n=std::min(257,16000-at);st_engine_render(e,l.data()+at,r.data()+at,n);}
    float normal=0,turn=0,end=0;for(int i=3000;i<5900;++i)normal=std::max(normal,std::abs(l[i]-l[i-1]));for(int i=5990;i<6400;++i)turn=std::max(turn,std::abs(l[i]-l[i-1]));for(int i=11000;i<12400;++i)end=std::max(end,std::abs(l[i]-l[i-1]));
    // Still the lead-in sine after the turn (sign changes), silent once the region edge is reached ~6000 frames later.
    int crossings=0;for(int i=6101;i<11000;++i)crossings+=(l[i-1]<0)!=(l[i]<0);float tail=0;for(int i=12200;i<16000;++i)tail=std::max(tail,std::abs(l[i]));
    std::printf("reverse %s lead-in turnaround: turn delta %.4f, edge delta %.4f (normal %.4f), %d crossings\n",reverseMode?"reverse-mode":"forward-mode",turn,end,normal,crossings);
    assert(turn<normal*1.5f&&end<normal*1.5f&&crossings>15&&tail<1e-4f);
}
int main(){leadInTurnaround(false);leadInTurnaround(true);continuity(false);continuity(true);continuity(false,true);continuity(false,false,true);continuity(false,false,false,true);auto onset=render(false,1,1,true);assert(std::abs(onset[0])<.01f);assert(onset[70]>.15f);puts("reverse continuity: polyphony, loop crossfades, repeated reversals, release tails, block invariance and reverse start, lead-in turnarounds passed");}

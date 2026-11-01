// Native finite-but-out-of-range inputs must saturate before float->int conversion.
#include "TrackerEngine.h"
#include <algorithm>
#include <array>
#include <cassert>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <limits>
#include <vector>
struct Engine{STEngine*e=st_engine_create(48000);Engine(){assert(e);}~Engine(){st_engine_destroy(e);}operator STEngine*()const{return e;}};
struct Audio{std::vector<float>l,r;Audio():l(12000),r(12000){}};
static Audio scene(int test,float value){
    Engine e;st_engine_begin_update(e);st_engine_set_track_count(e,1);st_engine_set_timing(e,240,8,0,1);st_engine_set_host_mixing(e,1,1);
    auto p=st_default_instrument();p.type=ST_INSTRUMENT_SAMPLE;p.mode=test==1||test==2||test==3?ST_MODE_SLICE:ST_MODE_FORWARD;p.sliceCount=8;p.attack=0;p.decay=0;p.sustain=1;p.release=.001f;p.volume=.2f;
    std::vector<float>pcm(32768);for(size_t n=0;n<pcm.size();++n)pcm[n]=float(.4*std::sin(n*.07)+.2*std::cos(n*.019));assert(st_engine_set_sample(e,0,pcm.data(),nullptr,pcm.size(),48000));st_engine_set_instrument(e,0,p);
    auto step=st_default_step();step.note=72;step.instrument=0;step.gate=2;
    if(test<2)st_engine_add_automation_point(e,0,0,test==0?ST_TARGET_BIT_DEPTH:ST_TARGET_SLICE_COUNT,0,0,1,0,value);
    else step.fx1={test==2||test==3?ST_FX_SLICE:test==4?ST_FX_RATCHET:ST_FX_ARPEGGIO,value};
    if(test!=2)st_engine_set_step(e,0,0,0,step,nullptr,0);st_engine_commit_update(e);
    if(test==2)st_engine_preview_step(e,0,step,nullptr,0,.2f);else st_engine_play(e,0);
    Audio out;for(size_t n=0;n<out.l.size();n+=127)st_engine_render(e,out.l.data()+n,out.r.data()+n,uint32_t(std::min<size_t>(127,out.l.size()-n)));return out;
}
int main(int argc,char**argv){const char*names[]={"bit-depth automation","slice-count automation","preview slice FX","sequenced slice FX","ratchet FX","arpeggio FX"};int start=argc>1?std::atoi(argv[1]):0,end=argc>1?start+1:6;assert(start>=0&&end<=6);
    for(int test=start;test<end;++test){float lo=test==0?2:test==5?0:1,hi=test==0?16:test==1||test==2||test==3?128:test==4?8:255;
        for(float value:{-std::numeric_limits<float>::max(),-1e20f,1e20f,std::numeric_limits<float>::max(),hi+.75f,lo-.75f}){auto actual=scene(test,value),bounded=scene(test,std::max(lo,std::min(hi,value)));assert(actual.l==bounded.l&&actual.r==bounded.r);}
        std::printf("PASS %s saturates finite extremes before integer conversion\n",names[test]);
    }
}

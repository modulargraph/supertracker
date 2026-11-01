#include "MaschineDrums.hpp"
#include <algorithm>
#include <array>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <limits>
#include <new>
#include <string>
#include <vector>
static bool counting=false;static size_t allocations=0;
void*operator new(size_t n){if(counting)++allocations;if(void*p=std::malloc(n?n:1))return p;throw std::bad_alloc();}
void*operator new[](size_t n){return ::operator new(n);}
void operator delete(void*p)noexcept{std::free(p);}
void operator delete[](void*p)noexcept{std::free(p);}
static int checks=0,failures=0;
static void check(bool ok,const std::string&name){++checks;if(!ok){++failures;std::fprintf(stderr,"FAIL %s\n",name.c_str());}}
static std::array<float,8> defaults(int model){std::array<float,8>p{};for(size_t i=0;i<8;++i)p[i]=stdrums::defaultParameter(model,uint32_t(i));return p;}
static std::vector<float> render(int model,double sr,const float*p,uint32_t count,size_t frames,double ratio=1,uint32_t seed=12345){std::vector<float>out(frames);stdrums::Voice v;v.start(model,sr,p,count,seed);counting=true;allocations=0;for(auto&sample:out)sample=v.tick(ratio);counting=false;check(allocations==0,"drum render allocation-free");return out;}
static double energy(const std::vector<float>&v,size_t from=0,size_t to=0){if(!to)to=v.size();double e=0;for(size_t i=from;i<to;++i)e+=double(v[i])*v[i];return e/(to-from);}
static double difference(const std::vector<float>&a,const std::vector<float>&b){double d=0;for(size_t i=0;i<a.size();++i)d+=std::abs(double(a[i])-b[i]);return d/a.size();}
static double frequency(const std::vector<float>&v,double sr){size_t first=0,last=0,count=0;for(size_t i=1;i<v.size();++i)if(v[i-1]<=0&&v[i]>0){if(!count)first=i;last=i;++count;}return count>1?(count-1)*sr/(last-first):0;}
int main(){
    check(st_drum_model_count()==6,"six drum model metadata entries");
    for(int model=1;model<=6;++model){
        const auto*info=st_drum_model_info(uint32_t(model-1));check(info&&st_drum_model_for_id(info->id)==model,"drum model ID stable");
        auto p=defaults(model);for(uint32_t i=0;i<8;++i){const auto*meta=st_drum_parameter_info(model,i);check(meta&&meta->defaultValue>=meta->minimum&&meta->defaultValue<=meta->maximum,"drum parameter range");check(std::abs(st_drum_parameter_default(model,i)-p[i])<1e-7,"native default equals C++ default");for(int n=0;n<=100;++n){const double physical=st_drum_parameter_from_normalized(model,i,n/100.0);check(std::abs(st_drum_parameter_to_normalized(model,i,physical)-n/100.0)<1e-9,"drum mapping roundtrip");}}
        for(double sr:{48000.,96000.}){
            const size_t frames=size_t(sr*0.5);auto baseline=render(model,sr,nullptr,0,frames),again=render(model,sr,p.data(),8,frames);
            check(difference(baseline,again)<1e-7,"default preset roundtrip");check(energy(baseline)>1e-5,"each family produces audio");
            auto repeat=render(model,sr,p.data(),8,frames);check(repeat==again,"drum deterministic seed/reset");
            for(uint32_t index=0;index<8;++index){auto low=p,high=p;low[index]=0;high[index]=1;auto a=render(model,sr,low.data(),8,frames),b=render(model,sr,high.data(),8,frames);check(difference(a,b)>1e-5,std::string(info->id)+" parameter "+std::to_string(index)+" changes sound");}
            for(int mode=0;mode<4;++mode){std::array<float,8>values{};for(size_t i=0;i<8;++i)values[i]=mode==0?0:mode==1?1:mode==2?float(i%2):std::numeric_limits<float>::quiet_NaN();auto extreme=render(model,sr,values.data(),8,frames,mode==0?0.03125:mode==1?32:1);bool finite=true;for(float v:extreme)finite=finite&&std::isfinite(v)&&std::abs(v)<=1;check(finite,"drum extreme controls and pitches bounded");}
            stdrums::Voice voice;counting=true;allocations=0;voice.start(model,sr,nullptr,0,0);counting=false;check(allocations==0,"drum trigger allocates nothing");
            voice.stop();check(!voice.active()&&voice.tick()==0,"drum stop immediately silent");voice.start(0,sr,p.data(),8);check(!voice.active()&&voice.tick()==0,"none does not synthesize");
        }
        stdrums::Voice lifetime;auto shortest=p;shortest[1]=0;lifetime.start(model,48000,shortest.data(),8);for(int i=0;i<48000;++i)lifetime.tick();check(!lifetime.active(),"short drum finishes without held note-off");
        for(double sr:{8000.,384000.}){stdrums::Voice v;auto maximum=p;maximum.fill(1);v.start(model,sr,maximum.data(),8);bool finite=true;for(int i=0;i<2048;++i){const float sample=v.tick(i%2?1e9:1e-9);finite=finite&&std::isfinite(sample)&&std::abs(sample)<=1;}check(finite,"full supported rates and extreme pitch ratios remain bounded");}
    }
    auto kick=defaults(1);kick[1]=1;kick[3]=0;kick[4]=0;kick[7]=0;auto base=render(1,48000,kick.data(),8,48000),octave=render(1,48000,kick.data(),8,48000,2);check(std::abs(frequency(octave,48000)/frequency(base,48000)-2)<0.01,"keyboard octave doubles drum pitch");kick[0]=0.75;auto tuned=render(1,48000,kick.data(),8,48000);check(std::abs(frequency(tuned,48000)/frequency(base,48000)-2)<0.01,"Tune +12 doubles drum pitch");
    std::array<std::vector<float>,6>sounds;for(int m=1;m<=6;++m)sounds[m-1]=render(m,48000,nullptr,0,24000);for(size_t i=0;i<6;++i)for(size_t j=i+1;j<6;++j)check(difference(sounds[i],sounds[j])>0.005,"six families have distinct PCM");
    check(!st_drum_model_info(6)&&!st_drum_parameter_info(0,0)&&!st_drum_parameter_info(1,8)&&!st_drum_model_for_id("unknown"),"drum metadata invalid inputs rejected");
    // Offline renders (drum-model synths migrate to samples of these): audible, finite,
    // decaying to the end, deterministic, and bounded by the caller's capacity.
    for(int model=1;model<=6;++model){
        std::vector<float>a(48000*13),b(48000*13);auto p=defaults(model);
        uint32_t n=st_drum_render(model,p.data(),8,48000,a.data(),uint32_t(a.size())),m=st_drum_render(model,p.data(),8,48000,b.data(),uint32_t(b.size()));
        check(n>480&&n==m&&std::equal(a.begin(),a.begin()+n,b.begin()),"offline drum render is audible and deterministic");
        bool finite=true;for(uint32_t i=0;i<n;++i)finite=finite&&std::isfinite(a[i])&&std::abs(a[i])<=4;check(finite,"offline drum render is finite");
        check(energy(a,0,n/8)>energy(a,n-n/8,n),"offline drum render decays");
        check(st_drum_render(model,p.data(),8,48000,a.data(),64)==64,"offline drum render honours capacity");
    }
    float nan[8];for(auto&v:nan)v=std::numeric_limits<float>::quiet_NaN();std::vector<float>out(48000),reference(48000);
    check(st_drum_render(2,nan,8,48000,out.data(),48000)==st_drum_render(2,nullptr,0,48000,reference.data(),48000)&&difference(out,reference)<1e-4,"nonfinite controls render model defaults");
    check(st_drum_render(0,nullptr,0,48000,out.data(),48000)==0&&st_drum_render(1,nullptr,0,48000,nullptr,48000)==0,"invalid offline render rejected");
    std::printf("Drum synthesis: %d checks, %d failures\n",checks,failures);return failures?1:0;
}

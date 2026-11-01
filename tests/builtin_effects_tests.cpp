#include "STBuiltinEffects.h"
#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <new>
#include <set>
#include <string>
#include <thread>
#include <vector>

static thread_local bool counting=false;
static thread_local size_t allocations=0;
void*operator new(size_t n){if(counting)++allocations;if(void*p=std::malloc(n?n:1))return p;throw std::bad_alloc();}
void*operator new[](size_t n){return ::operator new(n);}
void operator delete(void*p)noexcept{std::free(p);}
void operator delete[](void*p)noexcept{std::free(p);}
static int failures=0,checks=0;
static void check(bool value,const std::string&name){++checks;if(!value){++failures;std::fprintf(stderr,"FAIL %s\n",name.c_str());}}
static constexpr double pi=3.14159265358979323846;
struct FX {
    std::string id;STBuiltinEffect*p;
    FX(const char*id,double sr=48000,uint32_t block=256):id(id),p(st_builtin_effect_create(id,sr,block)){check(p!=nullptr,this->id+" creates");}
    ~FX(){st_builtin_effect_destroy(p);}
    void set(const char*name,double value){const auto*d=st_builtin_effect_find(id.c_str());for(uint32_t j=0;j<d->parameterCount;++j){const auto*q=st_builtin_effect_parameter_info(id.c_str(),j);if(std::strcmp(name,q->id)==0){check(st_builtin_effect_set_parameter(p,j,st_builtin_effect_to_normalized(id.c_str(),j,value))==1,id+" sets "+name);return;}}check(false,id+" missing "+name);}
    void run(std::vector<float>&l,std::vector<float>&r,const std::vector<float>*sc=nullptr){check(st_builtin_effect_process(p,l.data(),r.data(),sc?sc->data():nullptr,nullptr,uint32_t(l.size()))==1,id+" renders");}
};
static double rms(const std::vector<float>&v,size_t start=0){double sum=0;for(size_t i=start;i<v.size();++i)sum+=double(v[i])*v[i];return std::sqrt(sum/(v.size()-start));}
static std::vector<float> tone(double sr,double hz,size_t n,double gain=0.25){std::vector<float>v(n);for(size_t i=0;i<n;++i)v[i]=float(gain*std::sin(2*pi*hz*i/sr));return v;}
static double bin(const std::vector<float>&v,double sr,double hz,size_t start){double re=0,im=0;for(size_t i=start;i<v.size();++i){re+=v[i]*std::cos(2*pi*hz*i/sr);im-=v[i]*std::sin(2*pi*hz*i/sr);}return std::hypot(re,im);}
static double difference(const std::vector<float>&a,const std::vector<float>&b){double peak=0;for(size_t i=0;i<a.size();++i)peak=std::max(peak,std::abs(double(a[i])-b[i]));return peak;}

static void catalogueTests(){
    const uint32_t count=st_builtin_effect_count();check(count>=23,"catalogue includes core, spatial and delays");std::set<std::string>ids;
    for(uint32_t i=0;i<count;++i){const auto*d=st_builtin_effect_info(i);check(d&&d->id&&d->name&&d->category,"descriptor strings");check(ids.insert(d->id).second,"unique effect id");check(st_builtin_effect_find(d->id)==d,"find returns stable descriptor");
        std::set<std::string>params;
        for(uint32_t j=0;j<d->parameterCount;++j){const auto*p=st_builtin_effect_parameter_info(d->id,j);check(p&&p->id&&p->name&&p->unit,"parameter strings");check(params.insert(p->id).second,std::string(d->id)+" unique parameter");check(std::isfinite(p->minimum)&&p->maximum>p->minimum&&p->defaultValue>=p->minimum&&p->defaultValue<=p->maximum,"valid physical range");
            if(p->flags&ST_BUILTIN_PARAMETER_LOG)check(p->minimum>0,"positive log range");
            double prev=-1;
            for(int k=0;k<=100;++k){const double raw=st_builtin_effect_from_normalized(d->id,j,k/100.0),norm=st_builtin_effect_to_normalized(d->id,j,raw);check(raw>=p->minimum-1e-10&&raw<=p->maximum+1e-10&&norm>=prev-1e-10,"monotone bounded mapping");prev=norm;
                if(!(p->flags&(ST_BUILTIN_PARAMETER_INTEGER|ST_BUILTIN_PARAMETER_TOGGLE)))check(std::abs(norm-k/100.0)<1e-9,"parameter round trip");
            }
            for(uint32_t k=0;k<p->labelCount;++k)check(st_builtin_effect_parameter_label(d->id,j,k)!=nullptr,"enum label");
            check(st_builtin_effect_parameter_label(d->id,j,p->labelCount)==nullptr,"enum bound");
        }
        check(!st_builtin_effect_parameter_info(d->id,d->parameterCount),"parameter bound");
    }
    check(!st_builtin_effect_info(count)&&!st_builtin_effect_find("invalid")&&!st_builtin_effect_find(nullptr),"invalid catalogue lookup");
    check(!st_builtin_effect_create("eq",0,256)&&!st_builtin_effect_create("eq",48000,0)&&!st_builtin_effect_create("invalid",48000,256),"invalid construction");
}
static void contractTests(double sr){
    constexpr size_t n=8192;
    for(uint32_t ei=0;ei<st_builtin_effect_count();++ei){const auto*d=st_builtin_effect_info(ei);FX fx(d->id,sr,127);
        auto original=tone(sr,719,n);original[0]=0.8f;auto l=original,r=original;
        st_builtin_effect_set_bypass(fx.p,1);fx.run(l,r);check(std::memcmp(l.data(),original.data(),n*sizeof(float))==0&&std::memcmp(r.data(),original.data(),n*sizeof(float))==0,fx.id+" exact bypass");st_builtin_effect_set_bypass(fx.p,0);
        fx.run(l,r);auto first=l,firstR=r;st_builtin_effect_reset(fx.p);l=original;r=original;fx.run(l,r);check(l==first&&r==firstR,fx.id+" deterministic reset");
        st_builtin_effect_reset(fx.p);l=original;r=original;
        counting=true;allocations=0;st_builtin_effect_process(fx.p,l.data(),r.data(),nullptr,nullptr,uint32_t(n));counting=false;
        check(allocations==0,fx.id+" no render allocations");
        check(!st_builtin_effect_set_parameter(fx.p,d->parameterCount,0.5)&&!st_builtin_effect_set_parameter(fx.p,0,std::numeric_limits<double>::quiet_NaN()),fx.id+" invalid parameter rejected");
        for(int extreme=0;extreme<4;++extreme){
            for(uint32_t p=0;p<d->parameterCount;++p)st_builtin_effect_set_parameter(fx.p,p,extreme==0?0:extreme==1?1:extreme==2?(p%2?1:0):double((p*37+19)%101)/100);
            st_builtin_effect_set_tempo(fx.p,extreme%2?20:999);
            const double restoredTail=st_builtin_effect_tail_seconds(fx.p);
            check(std::isfinite(restoredTail)&&restoredTail>=0,fx.id+" tail available before rendering restored values");st_builtin_effect_reset(fx.p);
            l=original;r=original;l[12]=std::numeric_limits<float>::infinity();r[70]=std::numeric_limits<float>::quiet_NaN();
            counting=true;allocations=0;st_builtin_effect_process(fx.p,l.data(),r.data(),l.data(),r.data(),uint32_t(n));counting=false;
            bool finite=true;for(size_t k=0;k<n;++k)finite=finite&&std::isfinite(l[k])&&std::isfinite(r[k])&&std::abs(l[k])<=1e6&&std::abs(r[k])<=1e6;
            check(finite,fx.id+" finite bounded parameter extremes");check(allocations==0,fx.id+" no allocation on parameter changes/sidechain");
            check(restoredTail==st_builtin_effect_tail_seconds(fx.p),fx.id+" tail estimate independent of whether rendering occurred");
        }
        check(std::isfinite(st_builtin_effect_tail_seconds(fx.p)),fx.id+" finite tail metadata");
        // A render call may be any size; maxFrames is scratch capacity, not a
        // correctness constraint. Compare different callback partitionings.
        FX chunked(d->id,sr,511),single(d->id,sr,31);auto a=original,b=original,c=original,e=original;
        single.run(a,b);size_t pos=0;while(pos<n){const size_t z=std::min<size_t>((pos%97)+1,n-pos);st_builtin_effect_process(chunked.p,c.data()+pos,e.data()+pos,nullptr,nullptr,uint32_t(z));pos+=z;}
        check(difference(a,c)<2e-5&&difference(b,e)<2e-5,fx.id+" block partition invariance");
    }
}
static void responseTests(double sr){
    const size_t n=size_t(sr);
    {FX f("compressor",sr);f.set("threshold",-18);f.set("ratio",4);f.set("knee",0);f.set("attack",0.1);auto l=std::vector<float>(n,0.5),r=l;f.run(l,r);const double expected=std::pow(10.0,(-18+(-6.020599913+18)/4)/20);check(std::abs(rms(l,n/2)-expected)<0.001,"compressor steady-state ratio");
        st_builtin_effect_reset(f.p);l.assign(n,0.03);r=l;auto sc=std::vector<float>(n,0.8);f.run(l,r,&sc);check(rms(l,n/2)<0.01,"compressor external sidechain ducks quiet source");}
    {FX f("gate",sr);auto l=std::vector<float>(n,0.001),r=l;f.run(l,r);check(rms(l,n/2)<1e-6,"gate closes below threshold");st_builtin_effect_reset(f.p);l.assign(n,0.001);r=l;auto sc=std::vector<float>(n,0.5);f.run(l,r,&sc);check(rms(l,n/2)>0.00099,"gate opens from external key");}
    {FX f("limiter",sr);f.set("ceiling",-6);auto l=std::vector<float>(n,0),r=l;l[100]=4;r[100]=2;f.run(l,r);check(std::abs(l[100+st_builtin_effect_latency(f.p)]-std::pow(10,-6/20.0))<1e-6,"limiter lookahead and ceiling");double peak=0;for(float v:l)peak=std::max(peak,std::abs(double(v)));check(peak<=std::pow(10,-6/20.0)+1e-6,"limiter never exceeds sample ceiling");}
    {FX f("eq",sr);auto original=tone(sr,1000,n),l=original,r=original;f.run(l,r);check(l==original,"flat EQ exact unity");f.set("lowmidfreq",1000);f.set("lowmidgain",6);st_builtin_effect_reset(f.p);l=original;r=original;f.run(l,r);check(std::abs(rms(l,n/2)/rms(original,n/2)-std::pow(10,6/20.0))<0.01,"EQ peak gain at selected center");}
    {FX f("filter",sr);f.set("cutoff",1000);f.set("mix",100);auto low=tone(sr,100,n),lr=low,high=tone(sr,8000,n),hr=high;f.run(low,lr);st_builtin_effect_reset(f.p);f.run(high,hr);check(rms(low,n/2)>rms(high,n/2)*20,"low-pass attenuates upper band");f.set("mode",2);st_builtin_effect_reset(f.p);low=tone(sr,100,n);lr=low;f.run(low,lr);st_builtin_effect_reset(f.p);high=tone(sr,8000,n);hr=high;f.run(high,hr);check(rms(high,n/2)>rms(low,n/2)*20,"high-pass attenuates lower band");}
    {FX f("fm",sr);f.set("amount",0);auto original=tone(sr,900,n),l=original,r=original;f.run(l,r);const size_t delay=st_builtin_effect_latency(f.p);double max=0;for(size_t i=delay;i<n;++i)max=std::max(max,std::abs(double(l[i])-original[i-delay]));check(max<2e-7,"FM zero depth reconstructs delayed input");}
    {FX f("freqshifter",sr);f.set("coarse",200);auto l=tone(sr,2000,n),r=l;f.run(l,r);const double up=bin(l,sr,2200,n/2),down=bin(l,sr,1800,n/2),original=bin(l,sr,2000,n/2);check(up>down*20&&up>original*20,"frequency shifter moves carrier to upper sideband");}
    {FX f("transient",sr);std::vector<float> original(n);for(size_t i=0;i<n;++i)original[i]=float(0.5*std::exp(-double(i)/(sr*0.03))*std::sin(2*pi*1000*i/sr));auto l=original,r=original;f.run(l,r);check(difference(l,original)<1e-7,"neutral transient shaper unity");f.set("attack",100);st_builtin_effect_reset(f.p);l=original;r=original;f.run(l,r);check(rms(l)>rms(original)*1.2,"transient attack increases burst energy");}
}
static void concurrentTests(){
    FX fx("filter");std::atomic<bool>run{true};std::thread control([&]{for(int i=0;run.load();++i){st_builtin_effect_set_parameter(fx.p,1,double(i%100)/100);st_builtin_effect_set_tempo(fx.p,20+i%980);st_builtin_effect_get_parameter(fx.p,1);st_builtin_effect_tail_seconds(fx.p);}});
    auto l=tone(48000,1000,127),r=l;for(int i=0;i<2000;++i){st_builtin_effect_process(fx.p,l.data(),r.data(),nullptr,nullptr,127);}run=false;control.join();bool finite=true;for(float x:l)finite=finite&&std::isfinite(x);check(finite,"concurrent parameter mailbox remains finite");
}
static void tailRestoreTests(){
    FX f("beatdelay");f.set("time",31);f.set("unit",4);f.set("offset",100);f.set("feedback",97);st_builtin_effect_set_tempo(f.p,20);
    const double longTail=st_builtin_effect_tail_seconds(f.p);check(longTail>15000&&longTail<20000,"maximum Beat Delay feedback tail before first render");
    f.set("feedback",0);check(std::abs(st_builtin_effect_tail_seconds(f.p)-51)<0.01,"one long echo tail updates from restored parameters");
    st_builtin_effect_set_tempo(f.p,120);check(std::abs(st_builtin_effect_tail_seconds(f.p)-8.5)<0.01,"tail uses current tempo without a render");
    FX eq("eq"),filter("filter"),fm("fm"),phaser("phaser");check(st_builtin_effect_tail_seconds(eq.p)>0&&st_builtin_effect_tail_seconds(filter.p)>0,"EQ and resonant filter account for ringing");check(st_builtin_effect_tail_seconds(fm.p)>=0.1&&st_builtin_effect_tail_seconds(phaser.p)>=30,"FM crossover and feedback phaser cover long state");
}
static double peakOf(const std::vector<float>&v,size_t from=0,size_t to=0){if(!to)to=v.size();double m=0;for(size_t i=from;i<to;++i)m=std::max(m,std::abs(double(v[i])));return m;}
// Silence gating: once the input has been silent for longer than the tail
// and the output has decayed, the render is exact silence; the first
// non-silent block is processed normally.
static void gatingTests(){
    constexpr double sr=48000;
    {FX f("reverb",sr,128);std::vector<float> l(size_t(sr*.25),0.f),r=l;l[0]=r[0]=.5f;f.run(l,r);
        const double tail=st_builtin_effect_tail_seconds(f.p);std::vector<float> quiet(size_t(sr*(tail+1)),0.f),qr=quiet;f.run(quiet,qr);
        check(peakOf(quiet,quiet.size()-size_t(sr*.5))==0&&peakOf(qr,qr.size()-size_t(sr*.5))==0,"gated reverb renders exact silence after its tail");
        check(peakOf(quiet,0,size_t(sr*.1))>0,"reverb tail is not cut while it rings");
        std::vector<float> hit(256,0.f),hr=hit;hit[10]=hr[10]=.5f;f.run(hit,hr);check(peakOf(hit,10)>0.1,"gated reverb resumes on the first non-silent sample");
        // Fresh instance comparison: the gated state had decayed, so the new hit matches a reset effect closely.
        FX fresh("reverb",sr,128);std::vector<float> a(256,0.f),b=a;a[10]=b[10]=.5f;fresh.run(a,b);check(difference(a,hit)<1e-5,"resumed render matches a decayed (fresh) reverb");}
    // Sidechain counts as input: a silent main input with a live key is processed.
    {FX f("gate",sr,128);std::vector<float> ql(size_t(sr),0.f),qr=ql;f.run(ql,qr);
        auto key=tone(sr,200,4800,0.5);std::vector<float> l(4800,0.f),r=l;f.run(l,r,&key);check(peakOf(l)==0,"silent main input with a key stays silent");
        auto main=tone(sr,1000,4800,0.001),mr=main;f.run(main,mr,&key);check(rms(main,2400)>0.0006,"keyed gate opens after a long silence");}
    // Sustaining modes report the export cap and are never gated.
    {FX f("reverblegacy",sr,128);f.set("mix",100);auto l=tone(sr,330,size_t(sr*.5)),r=l;f.run(l,r);f.set("freeze",1);
        std::vector<float> quiet(size_t(sr*125),0.f),qr=quiet;f.run(quiet,qr);check(peakOf(quiet,quiet.size()-size_t(sr))>1e-4,"frozen legacy reverb keeps sustaining past the gate horizon");}
    {FX f("grainstretch",sr,128);f.set("on",1);f.set("mix",100);auto l=tone(sr,330,size_t(sr*4.5)),r=l;f.run(l,r);
        std::vector<float> quiet(size_t(sr*125),0.f),qr=quiet;f.run(quiet,qr);check(peakOf(quiet,quiet.size()-size_t(sr))>1e-3,"grain stretch capture loop is never gated");}
    // Stutter history keeps recording silence (not gateable): engaging after a long silence loops silence.
    {FX f("stutter",sr,128);auto l=tone(sr,330,size_t(sr)),r=l;f.run(l,r);std::vector<float> quiet(size_t(sr*3),0.f),qr=quiet;f.run(quiet,qr);
        f.set("engage",1);std::vector<float> after(size_t(sr),0.f),ar=after;f.run(after,ar);check(peakOf(after)==0,"stutter captures the recent silence, not stale audio");}
#if defined(__aarch64__)
    {uint64_t before,after;__asm__ volatile("mrs %0, fpcr":"=r"(before));FX f("eq",sr,128);auto l=tone(sr,330,512),r=l;f.run(l,r);__asm__ volatile("mrs %0, fpcr":"=r"(after));check(before==after,"render restores the caller's floating-point control register");}
#endif
}
// Zero Mix with an identity dry path: exact dry output; after the tail time
// the wet DSP idles, and raising Mix restarts it cleanly.
static void zeroMixTests(){
    constexpr double sr=48000;
    for(const char*id:{"reverb","plate","metaverb","ice","reflex","resochord","reverblegacy","saturator","distortion"}){
        FX f(id,sr,128);f.set("mix",0);auto original=tone(sr,440,size_t(sr*8)),l=original,r=original;f.run(l,r);
        check(l==original&&r==original,std::string(id)+" zero mix is exactly dry");
        f.set("mix",50);auto wet=tone(sr,440,size_t(sr)),wr=wet;f.run(wet,wr);bool finite=true;for(float v:wet)finite=finite&&std::isfinite(v);
        check(finite&&difference(wet,tone(sr,440,size_t(sr)))>1e-3,std::string(id)+" wet path resumes after zero mix");
    }
}
static void performance(){
    double sum=0;for(uint32_t i=0;i<st_builtin_effect_count();++i){const auto*d=st_builtin_effect_info(i);FX fx(d->id);auto original=tone(48000,713,256),l=original,r=original;const auto start=std::chrono::steady_clock::now();for(int k=0;k<200;++k){l=original;r=original;st_builtin_effect_process(fx.p,l.data(),r.data(),nullptr,nullptr,256);}const double seconds=std::chrono::duration<double>(std::chrono::steady_clock::now()-start).count();sum+=seconds;std::printf("BENCH %-16s %.3f %% of realtime (48k stereo, 256-frame blocks)\n",d->id,100*seconds/(200*256/48000.0));}check(sum<30,"aggregate throughput smoke threshold");
}
int main(){catalogueTests();contractTests(48000);contractTests(96000);responseTests(48000);responseTests(96000);gatingTests();zeroMixTests();concurrentTests();tailRestoreTests();performance();std::printf("Builtin effects: %d checks, %d failures, %u processors\n",checks,failures,st_builtin_effect_count());return failures?1:0;}

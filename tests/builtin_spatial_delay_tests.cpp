// Offline audio-property tests. No device, plug-in, application, or NI binary is loaded.
// clang++ -std=c++17 -O2 -I Sources/TrackerEngine Tests/builtin_spatial_delay_tests.cpp -o /tmp/spatial-fx-tests
#include "EffectsDelay.hpp"
#include <atomic>
#include <chrono>
#include <complex>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <memory>
#include <new>
#include <string>
#include <unordered_set>

static bool rendering=false;static size_t allocations=0,checks=0;
void* operator new(size_t n){if(rendering)++allocations;if(auto*p=std::malloc(n?n:1))return p;throw std::bad_alloc();}
void* operator new[](size_t n){return ::operator new(n);}void operator delete(void*p)noexcept{std::free(p);}void operator delete[](void*p)noexcept{std::free(p);}
static void require(bool pass,const char*message){++checks;if(!pass){std::fprintf(stderr,"FAIL: %s\n",message);std::exit(1);}}
struct Audio{std::vector<float> l,r;explicit Audio(size_t n):l(n),r(n){} };
static const stfx::Descriptor& find(const char*id){size_t n=0;auto*d=stfx::spatialDescriptors(n);for(size_t i=0;i<n;++i)if(!std::strcmp(d[i].info.id,id))return d[i];d=stfx::delayDescriptors(n);for(size_t i=0;i<n;++i)if(!std::strcmp(d[i].info.id,id))return d[i];std::abort();}
static std::unique_ptr<stfx::Processor> make(const char*id,double sr){const auto&d=find(id);std::unique_ptr<stfx::Processor> p(d.create(sr,256));for(size_t i=0;i<d.info.parameterCount;++i)p->p[i]=d.params[i].defaultValue;return p;}
static void set(stfx::Processor&p,const char*id,const char*key,double value){auto&d=find(id);for(size_t i=0;i<d.info.parameterCount;++i)if(!std::strcmp(d.params[i].id,key)){p.p[i]=value;return;}std::abort();}
static void render(stfx::Processor&p,Audio&a,size_t block=127){for(size_t off=0;off<a.l.size();off+=block){size_t n=std::min(block,a.l.size()-off);rendering=true;p.process(a.l.data()+off,a.r.data()+off,nullptr,nullptr,uint32_t(n));rendering=false;}}
static double energy(const std::vector<float>&a,size_t from=0,size_t until=0){if(!until)until=a.size();double sum=0;for(size_t i=from;i<std::min(until,a.size());++i)sum+=double(a[i])*a[i];return sum;}
static size_t firstSignal(const std::vector<float>&a){for(size_t i=0;i<a.size();++i)if(std::abs(a[i])>1.e-6)return i;return a.size();}
static double dftPower(const std::vector<float>&a,double frequency,double sr,size_t first,size_t count){double re=0,im=0,w=stfx::pi*2*frequency/sr;for(size_t k=0;k<count&&k+first<a.size();++k){double window=.5-.5*std::cos(stfx::pi*2*k/double(count-1));re+=a[first+k]*window*std::cos(w*k);im+=a[first+k]*window*std::sin(w*k);}return re*re+im*im;}
static double peakHz(const std::vector<float>&a,double sr,double low,double high){double power=-1,best=0;for(double f=low;f<=high;f+=.25){double e=dftPower(a,f,sr,size_t(sr*.03),size_t(sr*.5));if(e>power){power=e;best=f;}}return best;}
static Audio breakLike(double sr,double seconds){Audio a(size_t(sr*seconds));uint32_t state=173;for(size_t i=0;i<a.l.size();++i){state=state*1664525u+1013904223u;double t=i/sr,phase=std::fmod(t,.25),env=std::exp(-phase*40.);double noise=(double(state)/2147483648.-1.)*.12*std::exp(-std::fmod(t,.125)*100.);double kick=std::sin(2*stfx::pi*(50*phase+2*(1-std::exp(-phase*35.))))*env*.3;double note=std::sin(t*2*stfx::pi*220)*.08;a.l[i]=float(kick+noise+note);a.r[i]=float(kick-noise*.6+note*.8);}return a;}
static const char*ids[]={"ice","metaverb","reflex","reverblegacy","reverb","plate","beatdelay","graindelay","grainstretch","resochord"};
static void catalogue(){std::unordered_set<std::string> seen;for(auto id:ids){auto&d=find(id);require(seen.insert(d.info.id).second,"unique effect IDs");require(d.info.parameterCount>0&&d.info.parameterCount<=16,"parameters fit two eight-control pages");std::unordered_set<std::string> params;for(size_t i=0;i<d.info.parameterCount;++i){auto&p=d.params[i];require(params.insert(p.id).second,"unique parameter IDs");require(p.minimum<=p.defaultValue&&p.defaultValue<=p.maximum,"defaults in range");if(p.flags&1)require(p.minimum>0,"log parameter positive range");if(p.labelCount)require(p.labelCount==uint32_t(p.maximum-p.minimum+1),"all discrete choices labelled");}}}
static void invariants(){for(double sr:{44100.,48000.,96000.}) { for(auto id:ids){auto p=make(id,sr);auto original=breakLike(sr,.23),dry=original;set(*p,id,"mix",0);if(!std::strcmp(id,"grainstretch"))set(*p,id,"on",1);render(*p,dry);require(dry.l==original.l&&dry.r==original.r,"zero Mix is exact dry");p=make(id,sr);if(!std::strcmp(id,"grainstretch"))set(*p,id,"on",1);auto a=breakLike(sr,.37),b=a;render(*p,a,127);p->reset();render(*p,b,239);require(a.l==b.l&&a.r==b.r,"reset determinism and block-size invariance");
        for(int edge=0;edge<2;++edge){p=make(id,sr);auto&d=find(id);for(size_t k=0;k<d.info.parameterCount;++k)p->p[k]=edge?d.params[k].maximum:d.params[k].minimum;auto audio=breakLike(sr,.55);audio.l.resize(size_t(sr*3),0);audio.r.resize(audio.l.size(),0);render(*p,audio,311);double peak=0;for(size_t i=0;i<audio.l.size();++i){if(!std::isfinite(audio.l[i])||!std::isfinite(audio.r[i]))require(false,"finite range edge and feedback tail");peak=std::max(peak,double(std::max(std::abs(audio.l[i]),std::abs(audio.r[i]))));}require(peak<20,"bounded extreme settings");}
    } }
    require(allocations==0,"zero render allocations");}
static void delayTiming(){for(double sr:{44100.,48000.,96000.})for(double tempo:{60.,120.,240.}){auto p=make("beatdelay",sr);p->tempo=tempo;set(*p,"beatdelay","mix",100);set(*p,"beatdelay","feedback",0);Audio a(size_t(sr*2));a.l[0]=.5f;render(*p,a);require(std::abs(double(firstSignal(a.l))-sr*60./tempo)<=1.,"Beat Delay follows project tempo at every sample rate");require(energy(a.r)==0,"uncrossed mono delay stays on input channel");}
    auto p=make("beatdelay",48000);set(*p,"beatdelay","mix",100);set(*p,"beatdelay","feedback",70);set(*p,"beatdelay","crossover",100);Audio a(96000);a.l[0]=.5;render(*p,a);require(energy(a.l,23000,25000)>0.1,"ping-pong first echo left");require(energy(a.r,47000,49000)>.02,"ping-pong second echo right");require(energy(a.r,0,47000)==0,"crossover acts on feedback not first input");}
static void tailMetadata(){
    auto p=make("beatdelay",48000);set(*p,"beatdelay","feedback",0);require(std::abs(p->tailForParameters(p->p.data(),120)-.5)<1e-9,"delay tail available before first render");
    set(*p,"beatdelay","time",31);set(*p,"beatdelay","unit",4);set(*p,"beatdelay","offset",100);set(*p,"beatdelay","feedback",97);
    double longest=p->tailForParameters(p->p.data(),20);require(longest>19000&&longest<=p->tail(),"high-feedback long-delay tail is not falsely reported as180seconds");
    for(auto id:ids){auto fx=make(id,48000);auto&desc=find(id);for(uint32_t k=0;k<desc.info.parameterCount;++k)fx->p[k]=desc.params[k].maximum;double estimate=fx->tailForParameters(fx->p.data(),20);require(std::isfinite(estimate)&&estimate>0&&estimate<=fx->tail(),"maximum parameter tails fit declared upper bound");}
    auto reflex=make("reflex",48000);set(*reflex,"reflex","size",100);require(reflex->tailForParameters(reflex->p.data(),120)>50,"resonant Reflex maximum tail is preserved");
    auto grain=make("graindelay",48000);set(*grain,"graindelay","size",500);set(*grain,"graindelay","space",2000);set(*grain,"graindelay","density",100);require(grain->tailForParameters(grain->p.data(),120)>200,"dense long grain cloud tail is preserved");
    const double slower=p->tailForParameters(p->p.data(),60),faster=p->tailForParameters(p->p.data(),120);require(std::abs(slower-2*faster)<1e-8,"tail metadata follows target tempo without rendering");
}
static void delayBoundaryTiming(){
    auto p=make("beatdelay",8000);p->tempo=20;set(*p,"beatdelay","time",31);set(*p,"beatdelay","unit",4);set(*p,"beatdelay","offset",100);set(*p,"beatdelay","feedback",0);set(*p,"beatdelay","mix",100);Audio a(408010);a.l[0]=.5f;render(*p,a);require(firstSignal(a.l)==408000,"longest supported delay is not truncated by its buffer");
    p=make("beatdelay",48000);set(*p,"beatdelay","time",1);set(*p,"beatdelay","unit",5);set(*p,"beatdelay","mix",100);set(*p,"beatdelay","feedback",0);Audio triplet(10000);triplet.l[0]=.5f;render(*p,triplet);require(firstSignal(triplet.l)==4000,"sixteenth-triplet is exactly one sixth of a quarter-note beat");
}
static void reverbs(){for(auto id:{"ice","metaverb","reflex","reverblegacy","reverb","plate"}){auto p=make(id,48000);set(*p,id,"mix",100);Audio a(192000);a.l[0]=a.r[0]=.5f;render(*p,a);require(energy(a.l,4800)>1.e-7,"reverb impulse has an audible tail");require(a.l!=a.r,"reverb generates stereo decorrelation");}
    auto p=make("reverb",48000);set(*p,"reverb","mix",100);set(*p,"reverb","predelay",100);set(*p,"reverb","modulation",0);Audio a(24000);a.l[0]=a.r[0]=1;render(*p,a);require(firstSignal(a.l)>=4800,"100ms predelay never emits early wet audio");
    auto shortReverb=make("reverb",48000),longReverb=make("reverb",48000);set(*shortReverb,"reverb","mix",100);set(*longReverb,"reverb","mix",100);set(*shortReverb,"reverb","time",.5);set(*longReverb,"reverb","time",8);Audio sh(144000),lo(144000);sh.l[0]=sh.r[0]=lo.l[0]=lo.r[0]=.5;render(*shortReverb,sh);render(*longReverb,lo);require(energy(lo.l,48000)>energy(sh.l,48000)*100,"Reverb Time changes measured tail decay");
    auto legacy=make("reverblegacy",48000);set(*legacy,"reverblegacy","mix",0);set(*legacy,"reverblegacy","freeze",1);auto frozen=breakLike(48000,.1);render(*legacy,frozen);require(energy(frozen.l)==0&&energy(frozen.r)==0,"legacy Freeze mutes dry signal");
    auto iceLow=make("ice",48000),iceHigh=make("ice",48000);set(*iceLow,"ice","mix",100);set(*iceHigh,"ice","mix",100);set(*iceLow,"ice","ice",0);set(*iceHigh,"ice","ice",100);Audio il(192000),ih(192000);il.l[0]=il.r[0]=ih.l[0]=ih.r[0]=.5;render(*iceLow,il);render(*iceHigh,ih);require(energy(ih.l,96000)>energy(il.l,96000)*20,"high Ice lengthens excited resonant tails");
    // NI reference sweeps showed Color moves the resonant band from bass to treble.
    for(double color:{0.,100.}){auto filter=make("ice",48000);set(*filter,"ice","mix",100);set(*filter,"ice","ice",100);set(*filter,"ice","color",color);Audio impulse(96000);impulse.l[0]=impulse.r[0]=.5;render(*filter,impulse);double bass=0,treble=0;for(double f=30;f<150;f+=3)bass+=dftPower(impulse.l,f,48000,12000,12000);for(double f=7500;f<9500;f+=25)treble+=dftPower(impulse.l,f,48000,12000,12000);require(color==0?bass>treble*100:treble>bass*100,"Ice Color moves metallic resonance across the spectrum");}
    auto measured=make("ice",48000);set(*measured,"ice","color",75);set(*measured,"ice","ice",75);set(*measured,"ice","mix",100);auto input=breakLike(48000,2);double inputEnergy=energy(input.l);input.l.resize(384000);input.r.resize(384000);auto wet=input;render(*measured,wet);double ratio=std::sqrt(energy(wet.l,0,96000)/inputEnergy);require(ratio>.025&&ratio<.10,"Ice wet level remains in the measured reference range");set(*measured,"ice","ice",100);measured->reset();wet=input;render(*measured,wet);require(energy(wet.l,336000,384000)<energy(wet.l,48000,96000)*.001,"maximum Ice decays instead of forcing unconditional self-oscillation");

}
static void resonator(){for(double tune:{0.,12.}){auto p=make("resochord",48000);set(*p,"resochord","mode",1);set(*p,"resochord","spread",0);set(*p,"resochord","tune",tune);set(*p,"resochord","feedback",99);set(*p,"resochord","brightness",100);set(*p,"resochord","decay",10);set(*p,"resochord","mix",100);Audio a(48000);a.l[0]=a.r[0]=1;render(*p,a);double expected=130.81278265*std::pow(2,tune/12),peak=peakHz(a.l,48000,expected*.94,expected*1.04);require(std::abs(peak-expected)<expected*.015,"Resochord comb fundamental follows semitone tuning");}}
static void grains(){auto p=make("graindelay",48000);set(*p,"graindelay","mix",100);set(*p,"graindelay","pitch",12);set(*p,"graindelay","jitter",0);set(*p,"graindelay","space",0);set(*p,"graindelay","size",100);set(*p,"graindelay","mod",0);set(*p,"graindelay","stereo",0);set(*p,"graindelay","density",0);Audio a(96000);for(size_t i=0;i<a.l.size();++i)a.l[i]=a.r[i]=float(std::sin(i*2*stfx::pi*220/48000)*.2);render(*p,a);double shifted=dftPower(a.l,440,48000,48000,48000),original=dftPower(a.l,220,48000,48000,48000);require(shifted>original*100,"Grain Delay octave pitch shift preserves expected tone");
    p=make("grainstretch",48000);set(*p,"grainstretch","on",1);set(*p,"grainstretch","mix",100);set(*p,"grainstretch","loop",8);set(*p,"grainstretch","stretch",50);auto capture=breakLike(48000,4.1);render(*p,capture);Audio tail(48000);render(*p,tail);require(energy(tail.l)>.01,"Grain Stretch retains captured audio after incoming audio stops");set(*p,"grainstretch","on",0);Audio bypass(256);bypass.l[0]=.37;bypass.r[0]=-.29;render(*p,bypass);require(bypass.l[0]==.37f&&bypass.r[0]==-.29f,"Grain Stretch off monitors input");set(*p,"grainstretch","on",1);Audio recapture(48000);render(*p,recapture);require(energy(recapture.l)==0,"retrigger captures silence without leaking old recording");}
static void benchmark(){for(auto id:ids){auto p=make(id,48000);if(!std::strcmp(id,"grainstretch"))set(*p,id,"on",1);auto a=breakLike(48000,2);auto start=std::chrono::steady_clock::now();render(*p,a,256);double ms=std::chrono::duration<double,std::milli>(std::chrono::steady_clock::now()-start).count();std::printf("{\"effect\":\"%s\",\"audio_seconds\":2,\"render_ms\":%.3f,\"realtime_cpu_percent\":%.3f}\n",id,ms,ms/20);}}
static void writeWav(const std::string& path,const Audio& a) {
    FILE* file=std::fopen(path.c_str(),"wb"); require(file!=nullptr,"open audio comparison artifact");
    auto u16=[&](uint16_t v){unsigned char b[2]={static_cast<unsigned char>(v),static_cast<unsigned char>(v>>8)};std::fwrite(b,1,2,file);};
    auto u32=[&](uint32_t v){unsigned char b[4]={static_cast<unsigned char>(v),static_cast<unsigned char>(v>>8),static_cast<unsigned char>(v>>16),static_cast<unsigned char>(v>>24)};std::fwrite(b,1,4,file);};
    uint32_t bytes=uint32_t(a.l.size()*8);std::fwrite("RIFF",1,4,file);u32(36+bytes);std::fwrite("WAVEfmt ",1,8,file);u32(16);u16(3);u16(2);u32(48000);u32(48000*8);u16(8);u16(32);std::fwrite("data",1,4,file);u32(bytes);
    for(size_t i=0;i<a.l.size();++i)for(float v:{a.l[i],a.r[i]}){uint32_t bits;std::memcpy(&bits,&v,4);u32(bits);}std::fclose(file);
}
static void comparisons(const char* directory) {
    std::filesystem::create_directories(directory);auto dry=breakLike(48000,2);dry.l.resize(288000,0);dry.r.resize(288000,0);writeWav(std::string(directory)+"/dry-original-break.wav",dry);
    for(auto id:ids){auto p=make(id,48000);if(!std::strcmp(id,"grainstretch"))set(*p,id,"on",1);if(!std::strcmp(id,"ice"))set(*p,id,"ice",85);auto audio=dry;render(*p,audio,256);writeWav(std::string(directory)+"/"+id+"-original-break.wav",audio);}
}
int main(int argc,char**argv){catalogue();invariants();delayTiming();tailMetadata();delayBoundaryTiming();reverbs();resonator();grains();benchmark();if(argc>1)comparisons(argv[1]);require(allocations==0,"all test rendering is allocation-free");std::printf("PASS: %zu spatial/delay checks, zero render allocations\n",checks);}

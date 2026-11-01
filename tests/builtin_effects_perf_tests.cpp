// Built-in effect CPU budget check and benchmark.
//
//   builtin-effects-perf            budget assertions (used by scripts/test.sh)
//   builtin-effects-perf --bench    full per-effect table, more trials
//   builtin-effects-perf --dump DIR write every case's output (drum loop and
//                                   impulse response) as raw float32 stereo,
//                                   for before/after sound comparisons.
//
// Every catalogue effect is measured through the public C API at 48 kHz in
// 128-frame blocks with drum-like input (decaying noise/kick bursts and
// silence), at default parameters and at a typical Maschine-kit setting.
#include "STBuiltinEffects.h"
#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

namespace {
constexpr double kRate=48000;
constexpr uint32_t kBlock=128;
constexpr double kPi=3.14159265358979323846;

struct Setting { const char *parameter; double value; };
struct Case { const char *effect, *label; std::vector<Setting> settings; };

// Defaults for every effect come from the catalogue; these are extra
// "typical" cases: what a Maschine kit import actually instantiates.
const std::vector<Case>& typicalCases(){
    static const std::vector<Case> cases={
        {"compressor","sidechain-ish",{{"threshold",-24},{"ratio",6},{"attack",1},{"release",80}}},
        {"eq","boosts",{{"lowgain",4},{"lowmidgain",-3},{"highmidgain",2},{"highgain",3}}},
        {"filter","resonant lp",{{"cutoff",1200},{"resonance",60},{"amount",2}}},
        {"cabinet","mix 60",{{"mix",60},{"distance",40}}},
        {"freqshifter","+200 Hz",{{"coarse",200},{"mix",50}}},
        {"ice","ice 85",{{"ice",85}}},
        {"reverb","hall 2.5s",{{"mode",1},{"time",2.5},{"size",60},{"mix",25}}},
        {"reverb","plate mode",{{"mode",2},{"mix",30}}},
        {"reverblegacy","shatter",{{"room",3},{"mix",30}}},
        {"reverblegacy","freeze",{{"freeze",1},{"mix",50}}},
        {"plate","mix 25",{{"mix",25},{"decay",3}}},
        {"metaverb","size 80",{{"size",80},{"mix",40}}},
        {"graindelay","pitch +12",{{"pitch",12},{"mix",50},{"density",70}}},
        {"grainstretch","on",{{"on",1},{"mix",100}}},
        {"resochord","string",{{"mode",1},{"mix",50}}},
        {"distortion","analog",{{"mode",1},{"drive",24}}},
        {"lofi","4 kHz 8 bit",{{"sample_rate",4000},{"bits",8},{"smooth",40}}},
        {"saturator","classic drive",{{"drive",70},{"input",6}}},
        {"saturator","tape",{{"mode",1},{"drive",60}}},
        {"saturator","tube",{{"mode",2},{"drive",60},{"charge",50}}},
        {"saturator","mix 0",{{"mix",0}}},
        {"performfilter","engaged",{{"engage",1},{"position",.4},{"resonance",.6}}},
        {"performflanger","engaged",{{"engage",1}}},
        {"burstecho","engaged",{{"engage",1}}},
        {"resoecho","engaged",{{"engage",1}}},
        {"ring","engaged",{{"engage",1}}},
        {"stutter","engaged",{{"engage",1}}},
        {"tremolo","engaged",{{"engage",1},{"blend",.5},{"vibrato_mode",1}}},
        {"scratcher","engaged",{{"engage",1},{"frequency_shift",300}}},
    };
    return cases;
}
std::vector<Case> allCases(){
    std::vector<Case> cases;
    for(uint32_t i=0;i<st_builtin_effect_count();++i){
        const char *id=st_builtin_effect_info(i)->id;
        cases.push_back({id,"default",{}});
        for(const auto &c:typicalCases())if(std::strcmp(c.effect,id)==0)cases.push_back(c);
    }
    return cases;
}

// Deterministic drum loop: 120 BPM sixteenths, kick/snare/hat bursts that
// decay, plus the second half of every fourth bar in digital silence.
std::vector<float> drumLoop(size_t frames,int channel){
    std::vector<float> out(frames,0.f);
    uint32_t random=0x1234567u+uint32_t(channel)*977u;
    const size_t step=size_t(kRate*.125),bar=step*16;
    for(size_t i=0;i<frames;++i){
        const size_t inBar=i%bar,barIndex=i/bar,s=inBar/step;const double t=double(inBar%step)/kRate;
        if(barIndex%4==3&&s>=8)continue;
        random=random*1664525u+1013904223u;const double noise=double(random)/2147483648.-1.;
        double v=0;
        if(s%4==0)v+=.7*std::sin(2*kPi*(45*t+3*(1-std::exp(-t*30))))*std::exp(-t*18);
        if(s%8==4)v+=.4*noise*std::exp(-t*25)+.2*std::sin(2*kPi*190*t)*std::exp(-t*30);
        v+=.12*noise*std::exp(-t*90);
        out[i]=float(v);
    }
    return out;
}

struct Instance {
    STBuiltinEffect *fx;
    explicit Instance(const Case &c):fx(st_builtin_effect_create(c.effect,kRate,kBlock)){
        if(!fx){std::fprintf(stderr,"cannot create %s\n",c.effect);std::exit(2);}
        st_builtin_effect_set_tempo(fx,120);
        const auto *info=st_builtin_effect_find(c.effect);
        for(const auto &s:c.settings){
            bool found=false;
            for(uint32_t j=0;j<info->parameterCount;++j)if(std::strcmp(st_builtin_effect_parameter_info(c.effect,j)->id,s.parameter)==0){
                st_builtin_effect_set_parameter(fx,j,st_builtin_effect_to_normalized(c.effect,j,s.value));found=true;}
            if(!found){std::fprintf(stderr,"%s has no parameter %s\n",c.effect,s.parameter);std::exit(2);}
        }
        st_builtin_effect_reset(fx);
    }
    ~Instance(){st_builtin_effect_destroy(fx);}
    void render(std::vector<float> &l,std::vector<float> &r){
        for(size_t o=0;o<l.size();o+=kBlock)st_builtin_effect_process(fx,l.data()+o,r.data()+o,nullptr,nullptr,uint32_t(std::min<size_t>(kBlock,l.size()-o)));
    }
};

// Percent of realtime for one instance: median over trials of the time to
// render `seconds` of audio, each trial on a fresh instance (warmed one bar).
double measure(const Case &c,const std::vector<float> &inL,const std::vector<float> &inR,int trials){
    std::vector<double> results;
    std::vector<float> l(inL.size()),r(inR.size());
    for(int t=0;t<trials;++t){
        Instance instance(c);
        std::vector<float> wl(inL.begin(),inL.begin()+kBlock*64),wr(inR.begin(),inR.begin()+kBlock*64);instance.render(wl,wr);
        l=inL;r=inR;
        const auto start=std::chrono::steady_clock::now();
        instance.render(l,r);
        const double elapsed=std::chrono::duration<double>(std::chrono::steady_clock::now()-start).count();
        results.push_back(100*elapsed/(double(inL.size())/kRate));
    }
    std::sort(results.begin(),results.end());
    return results[results.size()/2];
}
std::string name(const Case &c){return std::string(c.effect)+" ["+c.label+"]";}

void writeRaw(const std::string &path,const std::vector<float> &l,const std::vector<float> &r){
    FILE *f=std::fopen(path.c_str(),"wb");if(!f){std::perror(path.c_str());std::exit(2);}
    for(size_t i=0;i<l.size();++i){const float v[2]={l[i],r[i]};std::fwrite(v,sizeof v,1,f);}
    std::fclose(f);
}
std::string fileName(const Case &c){std::string s=std::string(c.effect)+"_"+c.label;for(char &ch:s)if(ch=='+')ch='p';else if(!std::isalnum((unsigned char)ch)&&ch!='_')ch='-';return s;}

int dump(const char *dir){
    const size_t loop=size_t(kRate*8),tail=size_t(kRate*4);
    auto inL=drumLoop(loop,0),inR=drumLoop(loop,1);inL.resize(loop+tail,0.f);inR.resize(loop+tail,0.f);
    for(const auto &c:allCases()){
        {Instance in(c);auto l=inL,r=inR;in.render(l,r);writeRaw(std::string(dir)+"/"+fileName(c)+".loop.f32",l,r);}
        {Instance in(c);std::vector<float> l(size_t(kRate*8),0.f),r=l;l[0]=r[0]=.5f;in.render(l,r);writeRaw(std::string(dir)+"/"+fileName(c)+".ir.f32",l,r);}
        std::printf("%s\n",fileName(c).c_str());
    }
    return 0;
}

int bench(bool assertBudget){
    const double seconds=assertBudget?2:8;const int trials=assertBudget?5:9;
    auto inL=drumLoop(size_t(kRate*seconds),0),inR=drumLoop(size_t(kRate*seconds),1);
    int failures=0;double total=0;
    // Generous ceiling: 3% per instance is ~10x the most expensive effect as
    // measured on an Apple M4 (and leaves room for parallel test jobs), so
    // the check only trips on a real regression.
    const double ceiling=3.0;
    for(const auto &c:allCases()){
        double percent=measure(c,inL,inR,trials);
        if(assertBudget&&percent>ceiling){
            // Re-measure once before failing: timer noise on a loaded machine.
            percent=std::min(percent,measure(c,inL,inR,trials));
        }
        total+=percent;
        const bool over=assertBudget&&percent>ceiling;failures+=over;
        std::printf("PERF %-34s %7.3f %% realtime%s\n",name(c).c_str(),percent,over?"  OVER BUDGET":"");
    }
    std::printf("PERF total %.3f %% realtime over all cases\n",total);

    // Silence gating: after the tail time has elapsed with silent input, a
    // long-tail effect must cost close to nothing (absolute, and relative to
    // the same effect processing audio in this run); a non-silent block must
    // resume processing immediately.
    {
        Case reverb{"reverb","default",{}};
        const double active=measure(reverb,inL,inR,trials);
        Instance in(reverb);
        std::vector<float> l(size_t(kRate*.5),0.f),r=l;l[0]=r[0]=.5f;in.render(l,r);
        std::vector<float> silent(size_t(kRate*(st_builtin_effect_tail_seconds(in.fx)+2)),0.f),sr=silent;in.render(silent,sr);
        bool exact=true;for(size_t i=silent.size()-size_t(kRate);i<silent.size();++i)exact=exact&&silent[i]==0.f&&sr[i]==0.f;
        std::vector<float> quiet(size_t(kRate*4),0.f),qr=quiet;
        const auto start=std::chrono::steady_clock::now();
        for(int k=0;k<5;++k){std::fill(quiet.begin(),quiet.end(),0.f);std::fill(qr.begin(),qr.end(),0.f);in.render(quiet,qr);}
        const double percent=100*std::chrono::duration<double>(std::chrono::steady_clock::now()-start).count()/(5*4.0);
        std::vector<float> hit(kBlock,0.f),hr(kBlock,0.f);hit[3]=hr[3]=.5f;
        st_builtin_effect_process(in.fx,hit.data(),hr.data(),nullptr,nullptr,kBlock);
        bool resumed=false;for(size_t i=3;i<kBlock;++i)resumed=resumed||hit[i]!=0.f;
        std::printf("PERF gated silent reverb %.4f %% realtime (active %.4f %%), exact silence %s, resumes %s\n",percent,active,exact?"yes":"NO",resumed?"yes":"NO");
        if(assertBudget&&(percent>0.2||percent>active*.25||!exact||!resumed)){std::fprintf(stderr,"FAIL silence gating\n");++failures;}
    }
    if(failures)std::fprintf(stderr,"FAIL %d built-in effect budget checks\n",failures);
    return failures?1:0;
}
}

int main(int argc,char **argv){
    if(argc>2&&std::strcmp(argv[1],"--dump")==0)return dump(argv[2]);
    return bench(!(argc>1&&std::strcmp(argv[1],"--bench")==0));
}

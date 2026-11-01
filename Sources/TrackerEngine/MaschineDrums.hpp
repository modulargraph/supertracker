#ifndef ST_MASCHINE_DRUMS_HPP
#define ST_MASCHINE_DRUMS_HPP
#include "BuiltinEffectsCommon.hpp"
#include <array>
#include <cmath>
#include <cstdint>

// Original drum voices inspired by common analog and modal synthesis methods.
// These six families are not NI's individual Drum Synth engines or a decoder
// for their parameter arrays. Callers must map imported controls explicitly.
namespace stdrums {
enum Model { None=0,Kick=1,Snare=2,Hihat=3,Tom=4,Perc=5,Cymbal=6 };
constexpr uint32_t parameterCount=8;
inline bool validModel(int model) noexcept{return model>=Kick&&model<=Cymbal;}
inline const char* modelID(int model) noexcept {static const char*names[]={"none","kick","snare","hihat","tom","perc","cymbal"};return names[validModel(model)?model:0];}
inline const STBuiltinParameterInfo* parameterInfo(int model,uint32_t index) noexcept {
    static const STBuiltinParameterInfo common[]={
        {"tune","Tune","st",-24,24,0,0,nullptr,0},
        {"decay","Decay","ms",20,3000,350,ST_BUILTIN_PARAMETER_LOG,nullptr,0},
        {"tone","Tone","%",0,100,50,0,nullptr,0},
        {"impact","Impact","%",0,100,50,0,nullptr,0},
        {"noise","Noise","%",0,100,25,0,nullptr,0},
        {"attack","Attack","ms",0.05,50,0.2,ST_BUILTIN_PARAMETER_LOG,nullptr,0},
        {"drive","Drive","dB",0,24,0,0,nullptr,0},
        {"character","Character","%",0,100,30,0,nullptr,0}
    };
    static const STBuiltinParameterInfo decays[]={
        {"decay","Decay","ms",20,3000,350,ST_BUILTIN_PARAMETER_LOG,nullptr,0},
        {"decay","Decay","ms",20,2000,220,ST_BUILTIN_PARAMETER_LOG,nullptr,0},
        {"decay","Decay","ms",10,3000,120,ST_BUILTIN_PARAMETER_LOG,nullptr,0},
        {"decay","Decay","ms",30,4000,450,ST_BUILTIN_PARAMETER_LOG,nullptr,0},
        {"decay","Decay","ms",10,4000,300,ST_BUILTIN_PARAMETER_LOG,nullptr,0},
        {"decay","Decay","ms",100,12000,2500,ST_BUILTIN_PARAMETER_LOG,nullptr,0}
    };
    if(!validModel(model)||index>=parameterCount)return nullptr;
    return index==1?&decays[model-1]:&common[index];
}
inline double physicalParameter(int model,uint32_t index,double n) noexcept {
    const auto*p=parameterInfo(model,index);if(!p)return 0;
    if(!std::isfinite(n))return p->defaultValue;n=stfx::clamp(n,0,1);
    return p->flags&ST_BUILTIN_PARAMETER_LOG?p->minimum*std::pow(p->maximum/p->minimum,n):p->minimum+(p->maximum-p->minimum)*n;
}
inline float defaultParameter(int model,uint32_t index) noexcept {
    const auto*p=parameterInfo(model,index);if(!p)return 0;
    return float(p->flags&ST_BUILTIN_PARAMETER_LOG?std::log(p->defaultValue/p->minimum)/std::log(p->maximum/p->minimum):(p->defaultValue-p->minimum)/(p->maximum-p->minimum));
}
struct Voice {
    int model=None;double sr=48000,baseHz=55,decay=0.999,bodyDecay=0.998,noiseDecay=0.997,impactDecay=0.99,attackDecay=0.9;
    double tone=0.5,impact=0.5,noiseMix=0.25,drive=1,character=0.3;
    double envelope=0,bodyEnvelope=0,noiseEnvelope=0,pitchEnvelope=0,attackEnvelope=1;
    double noiseLP=0,noiseHP=0,bodyLP=0,metalHP=0,dcInput=0,dcState=0,lowPole=0.5,highPole=0.9,bodyPole=0.5,dcPole=0.999;
    std::array<double,6> phase{};std::array<double,8> modalPhase{},modalEnvelope{};
    uint32_t noiseState=1;uint64_t age=0,maxAge=0;
    bool running=false;
    void start(int kind,double sampleRate,const float*normalized,uint32_t count,uint32_t seed=1) noexcept;
    float tick(double pitchRatio=1) noexcept;
    bool active()const noexcept{return running;}
    void stop()noexcept{running=false;envelope=bodyEnvelope=noiseEnvelope=0;}
};
inline void Voice::start(int kind,double sampleRate,const float*normalized,uint32_t count,uint32_t seed)noexcept{
    *this=Voice{};model=validModel(kind)?kind:None;if(model==None)return;
    sr=std::isfinite(sampleRate)?stfx::clamp(sampleRate,8000,384000):48000;
    double p[parameterCount];for(uint32_t j=0;j<parameterCount;++j)p[j]=physicalParameter(model,j,normalized&&j<count?normalized[j]:defaultParameter(model,j));
    static constexpr double base[]={0,55,185,420,125,330,370};baseHz=base[model]*std::exp2(p[0]/12);
    const double seconds=p[1]*0.001;decay=std::exp(std::log(0.001)/(seconds*sr));
    bodyDecay=std::exp(std::log(0.001)/(seconds*sr*(model==Snare?0.55:0.85)));
    noiseDecay=std::exp(std::log(0.001)/(seconds*sr*(model==Kick?0.12:model==Tom?0.2:1)));
    impactDecay=std::exp(-1/(sr*(model==Kick?0.025:0.012)));
    attackDecay=std::exp(-1/(sr*p[5]*0.001));tone=p[2]/100;impact=p[3]/100;noiseMix=p[4]/100;drive=stfx::dbGain(p[6]);character=p[7]/100;
    const double upper=std::min(sr*0.44,1800+tone*tone*16000),lower=model==Hihat?2500+tone*3000:model==Cymbal?700+tone*2000:model==Snare?600+tone*1300:100;
    lowPole=std::exp(-2*stfx::pi*upper/sr);highPole=std::exp(-2*stfx::pi*std::min(sr*0.35,lower)/sr);bodyPole=std::exp(-2*stfx::pi*std::min(sr*0.4,600+tone*6000)/sr);dcPole=std::exp(-2*stfx::pi*12/sr);
    envelope=bodyEnvelope=noiseEnvelope=pitchEnvelope=attackEnvelope=1;modalEnvelope.fill(1);noiseState=seed?seed:1;running=true;
    maxAge=uint64_t(std::ceil((seconds*2.7+0.1)*sr));
}
inline double noise(uint32_t&state)noexcept{state^=state<<13;state^=state>>17;state^=state<<5;return double(state)/2147483648.0-1;}
inline double blep(double phase,double dt)noexcept{
    if(phase<dt){const double x=phase/dt;return 2*x-x*x-1;}
    if(phase>1-dt){const double x=(phase-1)/dt;return x*x+2*x+1;}return 0;
}
inline float Voice::tick(double pitchRatio)noexcept{
    if(!running)return 0;pitchRatio=std::isfinite(pitchRatio)?stfx::clamp(pitchRatio,1e-9,1e9):1;
    const double white=noise(noiseState);noiseLP=stfx::clean(white+lowPole*(noiseLP-white));noiseHP=stfx::clean(noiseLP+highPole*(noiseHP-noiseLP));const double n=noiseLP-noiseHP;
    const double pitch=baseHz*pitchRatio;double out=0;
    const double sweep=std::exp2(impact*pitchEnvelope*(model==Kick?2.8:0.8));
    if(model==Kick){
        phase[0]=stfx::wrap(phase[0]+std::min(sr*0.4,pitch*sweep)/sr);
        const double fundamental=std::sin(2*stfx::pi*phase[0]),harmonic=std::sin(4*stfx::pi*phase[0]);
        out=(fundamental*(1-character*0.35)+harmonic*character*0.35)*envelope+n*noiseMix*noiseEnvelope*0.4;
    }else if(model==Snare){
        phase[0]=stfx::wrap(phase[0]+std::min(sr*0.4,pitch*sweep)/sr);phase[1]=stfx::wrap(phase[1]+std::min(sr*0.4,pitch*1.618)/sr);
        const double body=0.7*std::sin(2*stfx::pi*phase[0])+0.3*std::sin(2*stfx::pi*phase[1]);
        const double snap=n*(0.25+noiseMix*1.25)*(1+impact*pitchEnvelope*2);
        out=body*bodyEnvelope*(0.8-noiseMix*0.55)+snap*noiseEnvelope;
        out+=character*0.2*std::sin(2*stfx::pi*(phase[0]+1.7*std::sin(2*stfx::pi*phase[1])))*bodyEnvelope;
    }else if(model==Hihat){
        static constexpr double ratios[]={1,1.342,1.789,2.321,2.891,3.671};double metal=0;
        for(size_t j=0;j<phase.size();++j){const double dt=std::min(0.42,pitch*ratios[j]/sr);phase[j]=stfx::wrap(phase[j]+dt);const double square=(phase[j]<0.5?1:-1)+blep(phase[j],dt)-blep(stfx::wrap(phase[j]+0.5),dt);metal+=square*(j%2?0.17:0.16);}
        metalHP=stfx::clean(metal+highPole*(metalHP-metal));metal-=metalHP;
        out=(metal*(0.7+character*0.3)+n*(0.2+noiseMix))*envelope*(1+impact*pitchEnvelope);
    }else if(model==Tom){
        static constexpr double ratios[]={1,1.593,2.135,2.917};double body=0;
        for(size_t j=0;j<4;++j){phase[j]=stfx::wrap(phase[j]+std::min(sr*0.4,pitch*ratios[j]*(1+impact*pitchEnvelope*0.3))/sr);const double harmonic=j?character/(j+1):1;body+=std::sin(2*stfx::pi*phase[j])*harmonic;}
        out=body*envelope*0.8+n*noiseMix*noiseEnvelope*0.5;
    }else if(model==Perc){
        phase[0]=stfx::wrap(phase[0]+std::min(sr*0.4,pitch*sweep)/sr);phase[1]=stfx::wrap(phase[1]+std::min(sr*0.4,pitch*(1.41+character*4.1))/sr);
        const double mod=std::sin(2*stfx::pi*phase[1])*(0.2+character*5)*bodyEnvelope;
        out=std::sin(2*stfx::pi*phase[0]+mod)*envelope*(1-noiseMix*0.65)+n*noiseMix*noiseEnvelope;
    }else{
        static constexpr double ratios[]={1,1.483,2.187,3.271,4.563,6.119,8.217,11.713};double metal=0;
        for(size_t j=0;j<modalPhase.size();++j){
            const double hz=std::min(sr*0.44,pitch*ratios[j]*(1+character*0.07*j));modalPhase[j]=stfx::wrap(modalPhase[j]+hz/sr);
            metal+=std::sin(2*stfx::pi*modalPhase[j])*modalEnvelope[j]*(1.0/(2+j*0.6));
            // Higher modes damp more quickly; coefficient depends only on the
            // prepared decay and partial index, avoiding allocations/state IO.
            modalEnvelope[j]*=decay;modalEnvelope[j]*=1-(1-decay)*j*0.18;
        }
        metalHP=stfx::clean(metal+highPole*(metalHP-metal));metal-=metalHP;
        out=metal*(0.5+character*0.3)+n*(0.2+noiseMix)*noiseEnvelope*(1+impact*pitchEnvelope);
    }
    bodyLP=stfx::clean(out+bodyPole*(bodyLP-out));if(model==Kick||model==Tom||model==Perc)out=bodyLP;
    out*=1-attackEnvelope;
    out=std::tanh(out*drive)/std::max(1.0,std::sqrt(drive));
    const double dc=out-dcInput+dcPole*dcState;dcInput=out;dcState=stfx::clean(dc);
    envelope*=decay;bodyEnvelope*=bodyDecay;noiseEnvelope*=noiseDecay;pitchEnvelope*=impactDecay;attackEnvelope*=attackDecay;
    if(++age>=maxAge||(envelope<1e-8&&noiseEnvelope<1e-8)){running=false;return 0;}
    return float(stfx::clamp(dcState,-1,1));
}
}
#endif

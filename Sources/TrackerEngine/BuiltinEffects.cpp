#include "BuiltinEffectsCommon.hpp"
#include "EffectsDynamics.hpp"
#include "EffectsFiltering.hpp"
#include "EffectsModulation.hpp"
#include "EffectsSpatial.hpp"
#include "EffectsDelay.hpp"
#include "EffectsDistortion.hpp"
#include "EffectsPerform.hpp"
#include "MaschineDrums.hpp"
#include <atomic>
#include <cstring>
#include <memory>
#if defined(__x86_64__) || defined(__i386__)
#include <xmmintrin.h>
#endif

namespace {
static_assert(std::atomic<double>::is_always_lock_free,"Realtime parameter mailboxes must be lock-free");
struct Catalogue {
    std::array<const stfx::Descriptor*,64> entries{};uint32_t count=0;
    Catalogue(){
        using Family=const stfx::Descriptor*(*)(size_t&);
        const Family families[]={stfx::dynamicsDescriptors,stfx::filteringDescriptors,stfx::modulationDescriptors,
            stfx::spatialDescriptors,stfx::delayDescriptors,
            stfx::distortionDescriptors,
            stfx::performDescriptors,
        };
        for(auto family:families){size_t n=0;const auto*d=family(n);for(size_t i=0;i<n&&count<entries.size();++i)entries[count++]=d+i;}
    }
};
const Catalogue& catalogue(){static const Catalogue c;return c;}
const stfx::Descriptor* find(const char*id){if(!id)return nullptr;const auto&c=catalogue();for(uint32_t i=0;i<c.count;++i)if(std::strcmp(c.entries[i]->info.id,id)==0)return c.entries[i];return nullptr;}
double physical(const STBuiltinParameterInfo&p,double n) noexcept {
    n=std::isfinite(n)?stfx::clamp(n,0,1):0;
    double v=(p.flags&ST_BUILTIN_PARAMETER_LOG) && p.minimum>0?p.minimum*std::pow(p.maximum/p.minimum,n):p.minimum+(p.maximum-p.minimum)*n;
    if(p.flags&(ST_BUILTIN_PARAMETER_INTEGER|ST_BUILTIN_PARAMETER_TOGGLE))v=std::round(v);
    return stfx::clamp(v,p.minimum,p.maximum);
}
double normalized(const STBuiltinParameterInfo&p,double v) noexcept {
    v=stfx::clamp(std::isfinite(v)?v:p.defaultValue,p.minimum,p.maximum);
    if(p.flags&(ST_BUILTIN_PARAMETER_INTEGER|ST_BUILTIN_PARAMETER_TOGGLE))v=std::round(v);
    if(p.maximum==p.minimum)return 0;
    return stfx::clamp((p.flags&ST_BUILTIN_PARAMETER_LOG)&&p.minimum>0?std::log(v/p.minimum)/std::log(p.maximum/p.minimum):(v-p.minimum)/(p.maximum-p.minimum),0,1);
}
// Flush-to-zero / denormals-are-zero for the duration of one render call:
// recursive filters and decaying tails then never hit slow subnormal
// arithmetic. The caller's floating-point environment is restored on exit.
struct FlushDenormals {
#if defined(__aarch64__)
    uint64_t saved;
    FlushDenormals() noexcept { __asm__ volatile("mrs %0, fpcr":"=r"(saved)); const uint64_t fz=saved|(uint64_t(1)<<24); if(fz!=saved)__asm__ volatile("msr fpcr, %0"::"r"(fz)); }
    ~FlushDenormals() { uint64_t now; __asm__ volatile("mrs %0, fpcr":"=r"(now)); if(now!=saved)__asm__ volatile("msr fpcr, %0"::"r"(saved)); }
#elif defined(__x86_64__) || defined(__i386__)
    unsigned saved;
    FlushDenormals() noexcept : saved(_mm_getcsr()) { _mm_setcsr(saved|0x8040u); }
    ~FlushDenormals() { _mm_setcsr(saved); }
#endif
};
// Live input/output clean-up: nonfinite -> 0, |x| <= 1e-30 -> 0, clamp to
// +-1e6 (stfx::clean semantics), without a libm call per sample.
ST_INLINE float cleanSample(float x) noexcept {
    const float a=std::fabs(x);
    return a>1e-30f&&a<=std::numeric_limits<float>::max()?std::fmin(1e6f,std::fmax(-1e6f,x)):0.f;
}
// cleanSample over a block, four lanes at a time (the same per-sample operations, so the
// results are bit-identical): dst[i] = cleanSample(src[i]); returns the largest |dst[i]|.
typedef float CleanLanes __attribute__((ext_vector_type(4)));
typedef int32_t CleanMask __attribute__((ext_vector_type(4)));
ST_INLINE float cleanBlock(const float*src,float*dst,uint32_t count) noexcept {
    const CleanLanes tiny=1e-30f,huge=std::numeric_limits<float>::max(),high=1e6f,low=-1e6f;
    CleanLanes peak=0.f;uint32_t i=0;
    for(;i+4<=count;i+=4){
        CleanLanes x;std::memcpy(&x,src+i,sizeof x);
        const CleanLanes a=__builtin_elementwise_abs(x);
        const CleanMask keep=(a>tiny)&(a<=huge);
        const CleanLanes clamped=__builtin_elementwise_min(high,__builtin_elementwise_max(low,x));
        const CleanLanes y=(CleanLanes)((CleanMask)clamped&keep);
        std::memcpy(dst+i,&y,sizeof y);
        peak=__builtin_elementwise_max(peak,__builtin_elementwise_abs(y));
    }
    float result=std::fmax(std::fmax(peak.x,peak.y),std::fmax(peak.z,peak.w));
    for(;i<count;++i){dst[i]=cleanSample(src[i]);result=std::fmax(result,std::fabs(dst[i]));}
    return result;
}
// Silence gating threshold: -120 dBFS.
constexpr float kSilence=1e-6f;
}
struct STBuiltinEffect {
    const stfx::Descriptor *descriptor;
    std::unique_ptr<stfx::Processor> processor;
    std::array<std::atomic<double>,64> parameters{};
    std::atomic<double> tempo{120};
    std::atomic<uint32_t> bypass{0};
    // Bumped (release) after every parameter/tempo store; render re-reads the
    // parameter mailboxes only when it moved (acquire), not on every call.
    std::atomic<uint32_t> version{1};uint32_t seenVersion=0;
    const uint32_t maxFrames,latency;
    std::vector<float> sideL,sideR;
    // Render-thread only: last normalized value mapped to processor->p.
    std::array<double,64> mapped{};
    // Silence gating: consecutive frames of silent input (main and sidechain),
    // and whether the last processed block's output had decayed below
    // kSilence. Once silent input has lasted longer than the tail (+ margin),
    // process() is skipped and the output is exact silence.
    uint64_t silentFrames=0;bool outputDecayed=false;
    double gateTail=-1;uint64_t gateFrames=0;
    STBuiltinEffect(const stfx::Descriptor*d,double sr,uint32_t frames):descriptor(d),processor(d->create(sr,frames)),maxFrames(frames),latency(processor->latency()),sideL(frames),sideR(frames){
        mapped.fill(std::numeric_limits<double>::quiet_NaN());
        for(uint32_t i=0;i<d->info.parameterCount;++i){parameters[i].store(normalized(d->params[i],d->params[i].defaultValue),std::memory_order_relaxed);processor->p[i]=d->params[i].defaultValue;}
        processor->reset();
    }
    // Returns true when a parameter or the tempo changed since the last call.
    bool refresh() noexcept {
        bool changed=false;
        for(uint32_t i=0;i<descriptor->info.parameterCount;++i){
            const double n=parameters[i].load(std::memory_order_relaxed);
            if(n!=mapped[i]){mapped[i]=n;processor->p[i]=physical(descriptor->params[i],n);changed=true;}
        }
        const double t=tempo.load(std::memory_order_relaxed);
        if(t!=processor->tempo){processor->tempo=t;changed=true;}
        return changed;
    }
    void resetGate() noexcept {silentFrames=0;outputDecayed=false;}
    // Frames of silent input after which the effect output is exactly silent.
    uint64_t gateThreshold(bool changed) noexcept {
        if(changed||gateTail<0){
            const double tail=processor->tailForParameters(processor->p.data(),processor->tempo);
            gateTail=tail;
            // Sustaining/freeze modes report the 120 s export cap: never gate.
            gateFrames=!processor->gateable()||!std::isfinite(tail)||tail>=120?UINT64_MAX:
                uint64_t((tail+0.1)*processor->sr)+latency+maxFrames;
        }
        return gateFrames;
    }
};
extern "C" {
uint32_t st_builtin_effect_count(void){return catalogue().count;}
const STBuiltinEffectInfo*st_builtin_effect_info(uint32_t index){const auto&c=catalogue();return index<c.count?&c.entries[index]->info:nullptr;}
const STBuiltinEffectInfo*st_builtin_effect_find(const char*id){const auto*d=find(id);return d?&d->info:nullptr;}
const STBuiltinParameterInfo*st_builtin_effect_parameter_info(const char*id,uint32_t parameter){const auto*d=find(id);return d&&parameter<d->info.parameterCount?d->params+parameter:nullptr;}
const char*st_builtin_effect_parameter_label(const char*id,uint32_t parameter,uint32_t label){const auto*p=st_builtin_effect_parameter_info(id,parameter);return p&&p->labels&&label<p->labelCount?p->labels[label]:nullptr;}
double st_builtin_effect_to_normalized(const char*id,uint32_t parameter,double value){const auto*p=st_builtin_effect_parameter_info(id,parameter);return p?normalized(*p,value):0;}
double st_builtin_effect_from_normalized(const char*id,uint32_t parameter,double value){const auto*p=st_builtin_effect_parameter_info(id,parameter);return p?physical(*p,std::isfinite(value)?value:normalized(*p,p->defaultValue)):0;}
STBuiltinEffect*st_builtin_effect_create(const char*id,double sampleRate,uint32_t maxFrames){
    const auto*d=find(id);if(!d||d->info.parameterCount>64||!std::isfinite(sampleRate)||sampleRate<8000||sampleRate>384000||maxFrames<1||maxFrames>1048576)return nullptr;
    try{return new STBuiltinEffect(d,sampleRate,maxFrames);}catch(...){return nullptr;}
}
void st_builtin_effect_destroy(STBuiltinEffect*effect){delete effect;}
int32_t st_builtin_effect_set_parameter(STBuiltinEffect*effect,uint32_t parameter,double value){
    if(!effect||parameter>=effect->descriptor->info.parameterCount||!std::isfinite(value))return 0;
    const auto&p=effect->descriptor->params[parameter];effect->parameters[parameter].store(normalized(p,physical(p,value)),std::memory_order_relaxed);
    effect->version.fetch_add(1,std::memory_order_release);return 1;
}
double st_builtin_effect_get_parameter(const STBuiltinEffect*effect,uint32_t parameter){return effect&&parameter<effect->descriptor->info.parameterCount?effect->parameters[parameter].load(std::memory_order_relaxed):0;}
void st_builtin_effect_set_bypass(STBuiltinEffect*effect,int32_t bypass){if(effect)effect->bypass.store(bypass!=0,std::memory_order_relaxed);}
void st_builtin_effect_set_tempo(STBuiltinEffect*effect,double bpm){if(effect&&std::isfinite(bpm)){effect->tempo.store(stfx::clamp(bpm,20,999),std::memory_order_relaxed);effect->version.fetch_add(1,std::memory_order_release);}}
void st_builtin_effect_reset(STBuiltinEffect*effect){if(effect){effect->refresh();effect->processor->reset();effect->resetGate();}}
uint32_t st_builtin_effect_latency(const STBuiltinEffect*effect){return effect?effect->latency:0;}
double st_builtin_effect_tail_seconds(const STBuiltinEffect*effect){
    if(!effect)return 0;
    double values[64]{};for(uint32_t i=0;i<effect->descriptor->info.parameterCount;++i)values[i]=physical(effect->descriptor->params[i],effect->parameters[i].load(std::memory_order_relaxed));
    return effect->processor->tailForParameters(values,effect->tempo.load(std::memory_order_relaxed));
}
int32_t st_builtin_effect_process(STBuiltinEffect*effect,float*left,float*right,const float*sidechainLeft,const float*sidechainRight,uint32_t frames){
    if(!effect)return 0;if(frames==0)return 1;if(!left||!right||left==right)return 0;
    if(effect->bypass.load(std::memory_order_relaxed))return 1;
    FlushDenormals ftz;
    const uint32_t version=effect->version.load(std::memory_order_acquire);
    bool changed=false;
    if(version!=effect->seenVersion){effect->seenVersion=version;changed=effect->refresh();}
    const uint64_t gateFrames=effect->gateThreshold(changed);
    const bool sidechain=sidechainLeft||sidechainRight;
    for(uint32_t offset=0;offset<frames;){
        const uint32_t count=std::min(effect->maxFrames,frames-offset);
        float*l=left+offset,*r=right+offset;
        float peak=0;
        if(sidechain){
            const float*a=(sidechainLeft?sidechainLeft:sidechainRight)+offset,*b=(sidechainRight?sidechainRight:sidechainLeft)+offset;
            float*sl=effect->sideL.data(),*sr=effect->sideR.data();
            peak=std::fmax(cleanBlock(a,sl,count),cleanBlock(b,sr,count));
        }
        peak=std::fmax(peak,std::fmax(cleanBlock(l,l,count),cleanBlock(r,r,count)));
        const bool silentInput=peak<kSilence;
        if(silentInput&&effect->outputDecayed&&effect->silentFrames>=gateFrames){
            // Gated: the tail has run out; skip the DSP, emit exact silence.
            std::fill(l,l+count,0.f);std::fill(r,r+count,0.f);
            offset+=count;continue;
        }
        effect->processor->process(l,r,sidechain?effect->sideL.data():nullptr,sidechain?effect->sideR.data():nullptr,count);
        const float out=std::fmax(cleanBlock(l,l,count),cleanBlock(r,r,count));
        if(silentInput){effect->silentFrames+=count;effect->outputDecayed=out<kSilence;}
        else effect->resetGate();
        offset+=count;
    }
    return 1;
}
uint32_t st_drum_model_count(void){return 6;}
const STBuiltinEffectInfo *st_drum_model_info(uint32_t index){
    static const STBuiltinEffectInfo models[]={
        {"kick","Kick","Drum Synth",8,0},{"snare","Snare","Drum Synth",8,0},
        {"hihat","Hi-hat","Drum Synth",8,0},{"tom","Tom","Drum Synth",8,0},
        {"perc","Percussion","Drum Synth",8,0},{"cymbal","Cymbal","Drum Synth",8,0}
    };return index<6?models+index:nullptr;
}
int32_t st_drum_model_for_id(const char*id){if(id)for(int i=1;i<=6;++i)if(std::strcmp(id,stdrums::modelID(i))==0)return i;return 0;}
const STBuiltinParameterInfo *st_drum_parameter_info(int32_t model,uint32_t parameter){return stdrums::parameterInfo(model,parameter);}
double st_drum_parameter_default(int32_t model,uint32_t parameter){const auto*p=stdrums::parameterInfo(model,parameter);return p?normalized(*p,p->defaultValue):0;}
double st_drum_parameter_to_normalized(int32_t model,uint32_t parameter,double value){const auto*p=stdrums::parameterInfo(model,parameter);return p?normalized(*p,value):0;}
uint32_t st_drum_render(int32_t model,const float*parameters,uint32_t count,double sampleRate,float*out,uint32_t capacity){
    if(!stdrums::validModel(model)||!out||capacity==0||!std::isfinite(sampleRate)||sampleRate<8000)return 0;
    stdrums::Voice voice;voice.start(model,sampleRate,parameters,parameters?count:0,1);
    uint32_t frames=0;
    while(frames<capacity&&voice.active()){out[frames]=float(stfx::clean(voice.tick(1)));++frames;}
    // Trim the inaudible end of the decay.
    while(frames>1&&std::fabs(out[frames-1])<1e-5f)--frames;
    return frames;
}
double st_drum_parameter_from_normalized(int32_t model,uint32_t parameter,double value){const auto*p=stdrums::parameterInfo(model,parameter);return p?physical(*p,std::isfinite(value)?value:normalized(*p,p->defaultValue)):0;}
}

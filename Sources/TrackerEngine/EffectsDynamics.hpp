#ifndef ST_EFFECTS_DYNAMICS_HPP
#define ST_EFFECTS_DYNAMICS_HPP
#include "BuiltinEffectsCommon.hpp"
namespace stfx { namespace dynamics {
constexpr uint32_t Log=ST_BUILTIN_PARAMETER_LOG,Toggle=ST_BUILTIN_PARAMETER_TOGGLE,Integer=ST_BUILTIN_PARAMETER_INTEGER;
#define ST_DYN_PARAM(id,name,unit,lo,hi,def,flag) {id,name,unit,lo,hi,def,flag,nullptr,0}
inline constexpr const char *modes[]={"Classic","Feedback"};
inline constexpr STBuiltinParameterInfo compressorParams[]={
    {"mode","Mode","",0,1,0,Integer,modes,2},
    ST_DYN_PARAM("threshold","Threshold","dB",-60,0,-18,0),
    ST_DYN_PARAM("ratio","Ratio",":1",1,20,4,Log),
    ST_DYN_PARAM("knee","Knee","dB",0,24,6,0),
    ST_DYN_PARAM("attack","Attack","ms",0.1,200,10,Log),
    ST_DYN_PARAM("release","Release","ms",5,2000,100,Log),
    ST_DYN_PARAM("gain","Gain","dB",-24,24,0,0),
    ST_DYN_PARAM("mix","Mix","%",0,100,100,0),
    ST_DYN_PARAM("scgain","Detector gain","dB",-24,24,0,0),
    ST_DYN_PARAM("scfilter","Detector filter","",0,1,0,Toggle),
    ST_DYN_PARAM("sccenter","Detector frequency","Hz",20,20000,632.5,Log),
    ST_DYN_PARAM("scwidth","Detector width","oct",0.25,6,3,Log)
};
struct Detector {
    Biquad f[2];
    void configure(double hz,double width,double sr) noexcept {for(auto&v:f)v.set(Biquad::BandPass,hz,1/(2*std::sinh(std::log(2.0)*width/2)),0,sr);}
    ST_HOT double sample(double l,double r,bool filter,double gain) noexcept {if(filter){l=f[0].tick(l);r=f[1].tick(r);}return std::max(std::abs(l),std::abs(r))*gain;}
    void reset() noexcept {for(auto&v:f)v.reset();}
};
ST_INLINE double reduction(double level,double threshold,double ratio,double knee) noexcept {
    const double over=level-threshold,slope=1-1/ratio;
    if(knee>0 && std::abs(over)<knee/2) return slope*(over+knee/2)*(over+knee/2)/(2*knee);
    return over>0 ? slope*over:0;
}
struct Compressor final:Processor {
    double envelope=0,feedbackL=0,feedbackR=0;
    Detector detector;Smoother makeup,wet,reductionGain;
    // Control-rate values, derived only when parameters change.
    ParameterCache<12> cache;double attack=0,release=0,gain=1,scGain=1,smooth=0,gainSmooth=0;
    explicit Compressor(double sr):Processor(sr){}
    void reset() noexcept override {envelope=feedbackL=feedbackR=0;detector.reset();makeup.reset();wet.reset();reductionGain.reset();cache.reset();}
    void process(float*l,float*r,const float*scL,const float*scR,uint32_t n) noexcept override {
        if(cache.changed(*this)){attack=pole(p[4],sr);release=pole(p[5],sr);gain=dbGain(p[6]);scGain=dbGain(p[8]);smooth=pole(5,sr);gainSmooth=pole(0.3,sr);
            detector.configure(p[10],p[11],sr);}
        for(uint32_t i=0;i<n;++i){
            const double dl=l[i],dr=r[i];
            const bool external=scL||scR;
            const double detL=external ? (scL?scL[i]:scR[i]):(p[0]>=0.5?feedbackL:dl);
            const double detR=external ? (scR?scR[i]:scL[i]):(p[0]>=0.5?feedbackR:dr);
            const double peak=detector.sample(detL,detR,p[9]>=0.5,scGain);
            const double c=peak>envelope?attack:release;envelope=clean(peak+c*(envelope-peak));
            // A feedback detector needs R-1 rather than 1-1/R to reach the
            // requested steady-state ratio, because it observes reduced audio.
            double gr=reduction(gainDB(envelope),p[1],p[2],p[3]);
            if(p[0]>=0.5 && !external)gr*=p[2];
            const double g=reductionGain.tick(fastDbGain(-std::min(120.0,gr)),gainSmooth);
            feedbackL=dl*g;feedbackR=dr*g;
            const double mg=makeup.tick(gain,smooth),w=wet.tick(p[7]/100,smooth);
            l[i]=float(mix(dl,feedbackL*mg,w));r[i]=float(mix(dr,feedbackR*mg,w));
        }
    }
};
inline constexpr STBuiltinParameterInfo gateParams[]={
    ST_DYN_PARAM("threshold","Threshold","dB",-80,0,-36,0),
    ST_DYN_PARAM("attack","Attack","ms",0.05,100,1,Log),
    ST_DYN_PARAM("hold","Hold","ms",0,1000,30,0),
    ST_DYN_PARAM("release","Release","ms",1,2000,80,Log),
    ST_DYN_PARAM("mix","Mix","%",0,100,100,0),
    ST_DYN_PARAM("scgain","Detector gain","dB",-24,24,0,0),
    ST_DYN_PARAM("scfilter","Detector filter","",0,1,0,Toggle),
    ST_DYN_PARAM("sccenter","Detector frequency","Hz",20,20000,632.5,Log),
    ST_DYN_PARAM("scwidth","Detector width","oct",0.25,6,3,Log)
};
struct Gate final:Processor {
    double gain=0;uint32_t remaining=0;Detector detector;Smoother wet;
    ParameterCache<9> cache;double threshold=1,attack=0,release=0,scGain=1,smooth=0;uint32_t hold=0;
    explicit Gate(double sr):Processor(sr){}
    void reset() noexcept override {gain=0;remaining=0;detector.reset();wet.reset();cache.reset();}
    void process(float*l,float*r,const float*scL,const float*scR,uint32_t n) noexcept override {
        if(cache.changed(*this)){threshold=dbGain(p[0]);attack=pole(p[1],sr);release=pole(p[3],sr);scGain=dbGain(p[5]);smooth=pole(5,sr);
            hold=uint32_t(p[2]*0.001*sr);detector.configure(p[7],p[8],sr);}
        for(uint32_t i=0;i<n;++i){
            const double dl=l[i],dr=r[i];
            const double peak=detector.sample(scL?scL[i]:(scR?scR[i]:dl),scR?scR[i]:(scL?scL[i]:dr),p[6]>=0.5,scGain);
            // 3 dB hysteresis prevents threshold chatter on long decays.
            if(peak>threshold*(remaining?0.707945784:1))remaining=hold+1;
            const double target=remaining?1:0;if(remaining)--remaining;
            const double c=target>gain?attack:release;gain=clean(target+c*(gain-target));
            const double g=mix(1,gain,wet.tick(p[4]/100,smooth));l[i]=float(dl*g);r[i]=float(dr*g);
        }
    }
};
inline constexpr STBuiltinParameterInfo transientParams[]={
    ST_DYN_PARAM("gain","Input gain","dB",-24,24,0,0),
    ST_DYN_PARAM("attack","Attack","%",-100,100,0,0),
    ST_DYN_PARAM("sustain","Sustain","%",-100,100,0,0),
    ST_DYN_PARAM("limit","Limit","",0,1,0,Toggle)
};
struct Transient final:Processor {
    double fast=0,slow=0,tailEnv=0;Smoother input,shape;
    const double fa,fr,sa,ss,tr,smooth;Memo inputDb;
    explicit Transient(double sr):Processor(sr),fa(pole(0.2,sr)),fr(pole(20,sr)),sa(pole(15,sr)),ss(pole(180,sr)),tr(pole(450,sr)),smooth(pole(2,sr)){}
    void reset() noexcept override {fast=slow=tailEnv=0;input.reset();shape.reset();}
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override {
        const double inputGain=inputDb([&]{return dbGain(p[0]);},p[0]);
        for(uint32_t i=0;i<n;++i){
            const double g=input.tick(inputGain,smooth),dl=l[i]*g,dr=r[i]*g,peak=std::max(std::abs(dl),std::abs(dr));
            fast=clean(peak+(peak>fast?fa:fr)*(fast-peak));slow=clean(peak+(peak>slow?sa:ss)*(slow-peak));
            tailEnv=clean(std::max(peak,tailEnv*tr));
            const double attack=clamp((fast-slow)/(fast+1e-10),0,1),sustain=clamp((tailEnv-fast)/(tailEnv+1e-10),0,1);
            const double change=(p[1]*0.18)*attack+(p[2]*0.18)*sustain;
            const double sg=shape.tick(fastDbGain(change),smooth);
            l[i]=float(p[3]>=0.5?clamp(dl*sg,-1,1):dl*sg);r[i]=float(p[3]>=0.5?clamp(dr*sg,-1,1):dr*sg);
        }
    }
};
inline constexpr const char *limiterModes[]={"Legacy","Transparent"};
inline constexpr STBuiltinParameterInfo limiterParams[]={
    {"mode","Mode","",0,1,1,Integer,limiterModes,2},
    ST_DYN_PARAM("threshold","Threshold","dB",-40,0,0,0),
    ST_DYN_PARAM("release","Release","ms",1,500,50,Log),
    ST_DYN_PARAM("ceiling","Ceiling","dB",-40,-0.3,-0.3,0),
    ST_DYN_PARAM("scgain","Detector gain","dB",-24,24,0,0),
    ST_DYN_PARAM("scfilter","Detector filter","",0,1,0,Toggle),
    ST_DYN_PARAM("sccenter","Detector frequency","Hz",20,20000,632.5,Log),
    ST_DYN_PARAM("scwidth","Detector width","oct",0.25,6,3,Log)
};
struct Limiter final:Processor {
    const uint32_t ahead;DelayLine dl,dr;double envelope=0;uint32_t held=0;Detector detector;Smoother drive;
    explicit Limiter(double sr):Processor(sr),ahead(uint32_t(std::round(sr*0.003))),dl(ahead+4),dr(ahead+4){}
    uint32_t latency() const noexcept override{return ahead;}
    double tail() const noexcept override{return 0.003;}
    ParameterCache<8> cache;double release=0,ceiling=1,scgain=1,smooth=0,threshold=1;
    void reset() noexcept override{dl.reset();dr.reset();envelope=0;held=0;detector.reset();drive.reset();cache.reset();}
    void process(float*l,float*r,const float*scL,const float*scR,uint32_t n) noexcept override {
        if(cache.changed(*this)){release=pole(p[2],sr);ceiling=dbGain(p[3]);scgain=dbGain(p[4]);smooth=pole(5,sr);threshold=dbGain(-p[1]);detector.configure(p[6],p[7],sr);}
        for(uint32_t i=0;i<n;++i){
            const double boost=drive.tick(threshold,smooth),xl=l[i]*boost,xr=r[i]*boost;
            const double sl=scL?scL[i]*boost:(scR?scR[i]*boost:xl),sr_=scR?scR[i]*boost:(scL?scL[i]*boost:xr);
            const double detected=detector.sample(sl,sr_,p[5]>=0.5,scgain);
            // Main peak also protects the ceiling when an external key is quiet.
            const double peak=std::max(detected,std::max(std::abs(xl),std::abs(xr)));
            if(peak>=envelope){envelope=peak;held=ahead;}else if(held)--held;else envelope=clean(peak+release*(envelope-peak));
            const double g=std::min(1.0,ceiling/std::max(envelope,1e-12));
            double yl=dl.read(ahead)*g,yr=dr.read(ahead)*g;dl.push(xl);dr.push(xr);
            if(p[0]<0.5){yl=ceiling*std::tanh(yl/ceiling);yr=ceiling*std::tanh(yr/ceiling);}
            l[i]=float(clamp(yl,-ceiling,ceiling));r[i]=float(clamp(yr,-ceiling,ceiling));
        }
    }
};
inline constexpr STBuiltinParameterInfo maximizerParams[]={
    ST_DYN_PARAM("amount","Amount","dB",0,30,3,0),
    ST_DYN_PARAM("curve","Curve","%",0,100,50,0),
    ST_DYN_PARAM("turbo","Turbo","",0,1,0,Toggle),
    ST_DYN_PARAM("scgain","Detector gain","dB",-24,24,0,0),
    ST_DYN_PARAM("scfilter","Detector filter","",0,1,0,Toggle),
    ST_DYN_PARAM("sccenter","Detector frequency","Hz",20,20000,632.5,Log),
    ST_DYN_PARAM("scwidth","Detector width","oct",0.25,6,3,Log)
};
struct Maximizer final:Processor {
    double envelope=0,second=0;Detector detector;Smoother drive;
    explicit Maximizer(double sr):Processor(sr){}
    ParameterCache<7> cache;double release=0,smooth=0,scgain=1,ceiling=1,amount=1;
    void reset() noexcept override{envelope=second=0;detector.reset();drive.reset();cache.reset();}
    void process(float*l,float*r,const float*scL,const float*scR,uint32_t n) noexcept override {
        if(cache.changed(*this)){release=pole(180-150*p[1]/100,sr);smooth=pole(5,sr);scgain=dbGain(p[3]);ceiling=dbGain(-0.3);amount=dbGain(p[0]);
            detector.configure(p[5],p[6],sr);}
        for(uint32_t i=0;i<n;++i){
            const double boost=drive.tick(amount,smooth),xl=l[i]*boost,xr=r[i]*boost;
            const double key=detector.sample(scL?scL[i]*boost:(scR?scR[i]*boost:xl),scR?scR[i]*boost:(scL?scL[i]*boost:xr),p[4]>=0.5,scgain);
            const double peak=std::max(key,std::max(std::abs(xl),std::abs(xr)));
            envelope=clean(std::max(peak,peak+release*(envelope-peak)));
            const double knee=12*(1-p[1]/100),gr=reduction(gainDB(envelope),-3,20,knee),g=fastDbGain(-gr);
            double yl=xl*g,yr=xr*g;
            if(p[2]>=0.5){
                yl*=1.8;yr*=1.8;const double pk=std::max(std::abs(yl),std::abs(yr));
                second=clean(std::max(pk,pk+release*(second-pk)));
                const double g2=fastDbGain(-reduction(gainDB(second),-3,20,knee));yl*=g2;yr*=g2;
            }
            l[i]=float(clamp(yl,-ceiling,ceiling));r[i]=float(clamp(yr,-ceiling,ceiling));
        }
    }
};
template<class T> Processor* create(double sr,uint32_t){return new T(sr);}
#undef ST_DYN_PARAM
}
inline const Descriptor* dynamicsDescriptors(size_t&count){
    static const Descriptor d[]={
        {{"compressor","Compressor","Dynamics",12,ST_BUILTIN_EFFECT_SIDECHAIN},dynamics::compressorParams,dynamics::create<dynamics::Compressor>},
        {{"gate","Gate","Dynamics",9,ST_BUILTIN_EFFECT_SIDECHAIN},dynamics::gateParams,dynamics::create<dynamics::Gate>},
        {{"transient","Transient Shaper","Dynamics",4,0},dynamics::transientParams,dynamics::create<dynamics::Transient>},
        {{"limiter","Peak Limiter","Dynamics",8,ST_BUILTIN_EFFECT_SIDECHAIN},dynamics::limiterParams,dynamics::create<dynamics::Limiter>},
        {{"maximizer","Maximizer","Dynamics",7,ST_BUILTIN_EFFECT_SIDECHAIN},dynamics::maximizerParams,dynamics::create<dynamics::Maximizer>}
    };count=sizeof(d)/sizeof(d[0]);return d;
}
}
#endif

#ifndef ST_EFFECTS_FILTERING_HPP
#define ST_EFFECTS_FILTERING_HPP
#include "BuiltinEffectsCommon.hpp"
namespace stfx { namespace filtering {
constexpr uint32_t Log=ST_BUILTIN_PARAMETER_LOG,Toggle=ST_BUILTIN_PARAMETER_TOGGLE,Integer=ST_BUILTIN_PARAMETER_INTEGER;
#define ST_FILTER_PARAM(id,name,unit,lo,hi,def,flag) {id,name,unit,lo,hi,def,flag,nullptr,0}
inline constexpr STBuiltinParameterInfo eqParams[]={
    ST_FILTER_PARAM("lowfreq","Low frequency","Hz",20,8000,100,Log),
    ST_FILTER_PARAM("lowgain","Low gain","dB",-24,24,0,0),
    ST_FILTER_PARAM("lowmidfreq","Low-mid frequency","Hz",40,16000,500,Log),
    ST_FILTER_PARAM("lowmidgain","Low-mid gain","dB",-24,24,0,0),
    ST_FILTER_PARAM("highmidfreq","High-mid frequency","Hz",40,16000,2500,Log),
    ST_FILTER_PARAM("highmidgain","High-mid gain","dB",-24,24,0,0),
    ST_FILTER_PARAM("highfreq","High frequency","Hz",50,20000,8000,Log),
    ST_FILTER_PARAM("highgain","High gain","dB",-24,24,0,0),
    ST_FILTER_PARAM("lowmidwidth","Low-mid width","oct",0.1,6,2,Log),
    ST_FILTER_PARAM("highmidwidth","High-mid width","oct",0.1,6,2,Log),
    ST_FILTER_PARAM("gain","Output gain","dB",-24,24,0,0)
};
struct EQ final:Processor {
    Biquad bands[2][4];std::array<Smoother,11> smoothed{};uint32_t control=0;double gain=1;
    // Filters are redesigned only when the smoothed controls actually move.
    std::array<double,11> designed{};bool haveDesign=false;
    explicit EQ(double sr):Processor(sr){}
    double tail()const noexcept override{return 8;}
    void reset() noexcept override {for(auto&channel:bands)for(auto&b:channel)b.reset();for(auto&v:smoothed)v.reset();control=0;gain=1;haveDesign=false;}
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override {
        const double smooth=std::exp(-32.0/(0.01*sr));
        for(uint32_t i=0;i<n;++i){
            if(control==0){
                std::array<double,11> v;for(size_t j=0;j<11;++j)v[j]=smoothed[j].tick(p[j],smooth);
                if(!haveDesign||v!=designed){designed=v;haveDesign=true;
                for(auto&channel:bands){
                    channel[0].set(Biquad::LowShelf,v[0],0.707106781, v[1],sr);
                    channel[1].set(Biquad::Peak,v[2],1/(2*std::sinh(std::log(2.0)*v[8]/2)),v[3],sr);
                    channel[2].set(Biquad::Peak,v[4],1/(2*std::sinh(std::log(2.0)*v[9]/2)),v[5],sr);
                    channel[3].set(Biquad::HighShelf,v[6],0.707106781,v[7],sr);
                }gain=dbGain(v[10]);}
            }control=(control+1)%32;
            double yl=l[i],yr=r[i];for(size_t j=0;j<4;++j){yl=bands[0][j].tick(yl);yr=bands[1][j].tick(yr);}
            l[i]=float(yl*gain);r[i]=float(yr*gain);
        }
    }
};
inline constexpr const char* filterModes[]={"Low-pass","Band-pass","High-pass","Notch"};
inline constexpr const char* sources[]={"LFO","LFO Sync","Envelope"};
inline constexpr const char* shapes[]={"Sine","Triangle","Saw","Square"};
inline constexpr STBuiltinParameterInfo filterParams[]={
    {"mode","Mode","",0,3,0,Integer,filterModes,4},
    ST_FILTER_PARAM("cutoff","Cutoff","Hz",20,20000,16000,Log),
    ST_FILTER_PARAM("resonance","Resonance","%",0,100,0,0),
    ST_FILTER_PARAM("amount","Modulation","oct",-5,5,0,0),
    {"source","Modulation source","",0,2,0,Integer,sources,3},
    ST_FILTER_PARAM("speed","LFO speed","Hz",0.03,16,1,Log),
    ST_FILTER_PARAM("beats","LFO period","beats",0.125,64,4,Log),
    {"shape","LFO shape","",0,3,0,Integer,shapes,4},
    ST_FILTER_PARAM("phase","LFO phase","deg",0,360,0,0),
    ST_FILTER_PARAM("decay","Envelope decay","ms",5,2000,200,Log),
    ST_FILTER_PARAM("smooth","Envelope smooth","ms",0.1,100,5,Log),
    ST_FILTER_PARAM("envshape","Envelope shape","",0.25,4,1,Log),
    ST_FILTER_PARAM("mix","Mix","%",0,100,100,0),
    ST_FILTER_PARAM("scgain","Detector gain","dB",-24,24,0,0),
    ST_FILTER_PARAM("scfilter","Detector filter","",0,1,0,Toggle),
    ST_FILTER_PARAM("sccenter","Detector frequency","Hz",20,20000,632.5,Log),
    ST_FILTER_PARAM("scwidth","Detector width","oct",0.25,6,3,Log)
};
ST_INLINE double lfo(double phase,int shape) noexcept {
    phase=wrap(phase);
    switch(shape){case 1:return 1-4*std::abs(phase-0.5);case 2:return 2*phase-1;case 3:return phase<0.5?1:-1;default:return fastSin2pi(phase);}
}
struct Filter final:Processor {
    double ic1[2]{},ic2[2]{},phase=0,envelope=0,a1=1,a2=0,a3=0,k=1;
    uint32_t control=0;Biquad sc[2];Smoother cutoff,resonance,wet;double designedF=-1,designedQ=-1;
    ParameterCache<17> cache;double attack=0,release=0,gain=1,smooth=0;
    explicit Filter(double sr):Processor(sr){}
    double tail()const noexcept override{return 20;}
    void reset() noexcept override {for(auto&v:ic1)v=0;for(auto&v:ic2)v=0;phase=envelope=0;control=0;for(auto&v:sc)v.reset();cutoff.reset();resonance.reset();wet.reset();designedF=designedQ=-1;cache.reset();}
    void process(float*l,float*r,const float*scL,const float*scR,uint32_t n) noexcept override {
        if(cache.changed(*this)){attack=pole(p[10],sr);release=pole(p[9],sr);gain=dbGain(p[13]);smooth=pole(5,sr);
            for(auto&v:sc)v.set(Biquad::BandPass,p[15],1/(2*std::sinh(std::log(2.0)*p[16]/2)),0,sr);}
        const double hz=p[4]==1?tempo/(60*p[6]):p[5];
        for(uint32_t i=0;i<n;++i){
            const double dl=l[i],dr=r[i];double sl=scL?scL[i]:(scR?scR[i]:dl),ss=scR?scR[i]:(scL?scL[i]:dr);
            if(p[14]>=0.5){sl=sc[0].tick(sl);ss=sc[1].tick(ss);}
            const double pk=std::max(std::abs(sl),std::abs(ss))*gain;
            envelope=clean(pk+(pk>envelope?attack:release)*(envelope-pk));
            // Zero modulation amount: exp2(0) == 1, the LFO/envelope value is unused.
            const double mod=p[3]==0?0:p[4]==2?std::pow(clamp(envelope,0,1),p[11]):lfo(phase+p[8]/360,int(p[7]));phase=wrap(phase+hz/sr);
            const double frequency=cutoff.tick(clamp(p[3]==0?p[1]:p[1]*fastExp(0.6931471805599453*(p[3]*mod)),10,sr*0.45),smooth),q=resonance.tick(0.707106781+19.3*p[2]/100,smooth);
            if(control==0&&(frequency!=designedF||q!=designedQ)){const double g=std::tan(pi*frequency/sr);k=1/q;a1=1/(1+g*(g+k));a2=g*a1;a3=g*a2;designedF=frequency;designedQ=q;}control=(control+1)%8;
            const double w=wet.tick(p[12]/100,smooth);
            for(int c=0;c<2;++c){
                const double x=c?dr:dl,v3=x-ic2[c],v1=a1*ic1[c]+a2*v3,v2=ic2[c]+a2*ic1[c]+a3*v3;
                ic1[c]=clean(2*v1-ic1[c]);ic2[c]=clean(2*v2-ic2[c]);
                double out=v2;if(p[0]==1)out=v1*k;else if(p[0]==2)out=x-k*v1-v2;else if(p[0]==3)out=x-k*v1;
                (c?r:l)[i]=float(mix(x,out,w));
            }
        }
    }
};
inline constexpr const char* cabinetTypes[]={"American","British","Vintage","Modern"};
inline constexpr const char* microphoneTypes[]={"Dynamic 57","Dynamic 421","Dynamic 441","Ribbon 121","Condenser 47","Condenser 67"};
inline constexpr STBuiltinParameterInfo cabinetParams[]={
    {"cabinet","Cabinet","",0,3,1,Integer,cabinetTypes,4},
    {"microphone","Microphone","",0,5,0,Integer,microphoneTypes,6},
    ST_FILTER_PARAM("distance","Distance","%",0,100,0,0),
    ST_FILTER_PARAM("mix","Mix","%",0,100,100,0)
};
struct Cabinet final:Processor {
    Biquad b[2][5];DelayLine delayL,delayR;Smoother distance,wet;
    // Filters are redesigned only when Cabinet, Microphone or Distance change.
    double designed[3]{-1,-1,-1};
    explicit Cabinet(double sr):Processor(sr),delayL(size_t(sr*0.04)+4),delayR(size_t(sr*0.04)+4){}
    void reset() noexcept override {for(auto&channel:b)for(auto&v:channel)v.reset();delayL.reset();delayR.reset();distance.reset();wet.reset();designed[0]=-1;}
    double tail()const noexcept override{return 0.05;}
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override {
        const int cab=int(p[0]),mic=int(p[1]);
        constexpr double hp[]={65,85,105,55},lp[]={5700,4800,4100,6500},mid[]={1700,2400,1250,3100};
        constexpr double micGain[]={3,1.5,0.5,-3.5,1,2},micHz[]={4000,3000,5000,2600,6000,4500};
        if(designed[0]!=cab||designed[1]!=mic||designed[2]!=p[2]){designed[0]=cab;designed[1]=mic;designed[2]=p[2];
        for(auto&channel:b){channel[0].set(Biquad::HighPass,hp[cab],0.707,0,sr);channel[1].set(Biquad::Peak,120,0.9,3-p[2]*0.03,sr);
            channel[2].set(Biquad::Peak,mid[cab],1.1,3.5,sr);channel[3].set(Biquad::HighShelf,micHz[mic],0.707,micGain[mic],sr);
            channel[4].set(Biquad::LowPass,lp[cab]*(1-0.35*p[2]/100),0.707,0,sr);}}
        const double smooth=pole(10,sr);
        for(uint32_t i=0;i<n;++i){
            const double dl=l[i],dr=r[i],d=distance.tick(p[2]/100,smooth),w=wet.tick(p[3]/100,smooth);
            double yl=dl,yr=dr;for(size_t j=0;j<5;++j){yl=b[0][j].tick(yl);yr=b[1][j].tick(yr);}
            const double el=delayL.read(sr*(0.001+d*0.008)),er=delayR.read(sr*(0.0013+d*0.009));delayL.push(yl);delayR.push(yr);
            l[i]=float(mix(dl,(yl+el*d*0.25)/(1+d*0.25),w));r[i]=float(mix(dr,(yr+er*d*0.25)/(1+d*0.25),w));
        }
    }
};
template<class T> Processor* create(double sr,uint32_t){return new T(sr);}
#undef ST_FILTER_PARAM
}
inline const Descriptor* filteringDescriptors(size_t&count){
    static const Descriptor d[]={
        {{"eq","Four-band EQ","Filtering",11,0},filtering::eqParams,filtering::create<filtering::EQ>},
        {{"filter","Multimode Filter","Filtering",17,ST_BUILTIN_EFFECT_SIDECHAIN},filtering::filterParams,filtering::create<filtering::Filter>},
        {{"cabinet","Cabinet","Filtering",4,0},filtering::cabinetParams,filtering::create<filtering::Cabinet>}
    };count=sizeof(d)/sizeof(d[0]);return d;
}
}
#endif

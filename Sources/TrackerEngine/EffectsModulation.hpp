#ifndef ST_EFFECTS_MODULATION_HPP
#define ST_EFFECTS_MODULATION_HPP
#include "BuiltinEffectsCommon.hpp"
namespace stfx { namespace modulation {
constexpr uint32_t Log=ST_BUILTIN_PARAMETER_LOG,Toggle=ST_BUILTIN_PARAMETER_TOGGLE,Integer=ST_BUILTIN_PARAMETER_INTEGER;
#define ST_MOD_PARAM(id,name,unit,lo,hi,def,flag) {id,name,unit,lo,hi,def,flag,nullptr,0}
inline constexpr STBuiltinParameterInfo chorusParams[]={
    ST_MOD_PARAM("rate","Rate","Hz",0.03,8,0.8,Log),
    ST_MOD_PARAM("amount","Amount","%",0,100,50,0),
    ST_MOD_PARAM("mix","Mix","%",0,100,35,0),
    ST_MOD_PARAM("stereo","Stereo","deg",0,180,90,0)
};
struct Chorus final:Processor {
    DelayLine d[2];double phase=0;Smoother rate,depth,wet,stereo;
    explicit Chorus(double sr):Processor(sr),d{DelayLine(size_t(sr*0.05)+4),DelayLine(size_t(sr*0.05)+4)}{}
    void reset() noexcept override{for(auto&v:d)v.reset();phase=0;rate.reset();depth.reset();wet.reset();stereo.reset();}
    double tail() const noexcept override{return 0.04;}
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override{
        const double smooth=pole(10,sr);
        for(uint32_t i=0;i<n;++i){
            const double speed=rate.tick(p[0],smooth),amount=depth.tick(p[1]/100,smooth),w=wet.tick(p[2]/100,smooth),s=stereo.tick(p[3]/360,smooth);
            const double dl=l[i],dr=r[i];
            const double yl=d[0].read(sr*(0.015+0.006*amount*fastSin2pi(phase))),yr=d[1].read(sr*(0.015+0.006*amount*fastSin2pi(phase+s)));
            d[0].push(dl);d[1].push(dr);phase=wrap(phase+speed/sr);
            l[i]=float(mix(dl,yl,w));r[i]=float(mix(dr,yr,w));
        }
    }
};
inline constexpr const char *sources[]={"LFO","LFO Sync","Envelope"};
inline constexpr STBuiltinParameterInfo flangerParams[]={
    ST_MOD_PARAM("frequency","Frequency","Hz",20,5000,500,Log),
    ST_MOD_PARAM("feedback","Feedback","%",-95,95,35,0),
    ST_MOD_PARAM("invert","Invert","",0,1,0,Toggle),
    ST_MOD_PARAM("amount","Amount","%",-100,100,50,0),
    {"source","Modulation source","",0,2,0,Integer,sources,3},
    ST_MOD_PARAM("speed","LFO speed","Hz",0.03,8,0.3,Log),
    ST_MOD_PARAM("beats","LFO period","beats",0.25,64,4,Log),
    ST_MOD_PARAM("envshape","Envelope shape","",0.25,4,1,Log),
    ST_MOD_PARAM("stereo","Stereo","deg",0,180,90,0),
    ST_MOD_PARAM("mix","Mix","%",0,100,50,0)
};
struct Flanger final:Processor {
    DelayLine d[2];double phase=0,envelope=0;Smoother delay[2],wet,feedback;
    explicit Flanger(double sr):Processor(sr),d{DelayLine(size_t(sr*0.12)+4),DelayLine(size_t(sr*0.12)+4)}{}
    void reset() noexcept override{for(auto&v:d)v.reset();phase=envelope=0;for(auto&v:delay)v.reset();wet.reset();feedback.reset();}
    double tail() const noexcept override{return 15;}
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override{
        const double rate=p[4]==1?tempo/(60*p[6]):p[5],attack=pole(2,sr),release=pole(100,sr),smooth=pole(10,sr);
        for(uint32_t i=0;i<n;++i){
            const double x[2]={l[i],r[i]},peak=std::max(std::abs(x[0]),std::abs(x[1]));envelope=clean(peak+(peak>envelope?attack:release)*(envelope-peak));
            const double w=wet.tick(p[9]/100,smooth),fb=feedback.tick(p[1]/100,smooth),inv=p[2]>=0.5?-1:1;
            for(int c=0;c<2;++c){
                const double mod=p[4]==2?std::pow(clamp(envelope,0,1),p[7]):fastSin2pi(phase+c*p[8]/360);
                const double dt=delay[c].tick(clamp(sr/(2*p[0])*fastExp(0.6931471805599453*(2*p[3]/100*mod)),1,sr*0.115),smooth),y=d[c].read(dt);
                d[c].push(x[c]+fastTanh(y)*fb);(c?r:l)[i]=float(mix(x[c],y*inv,w));
            }phase=wrap(phase+rate/sr);
        }
    }
};
inline constexpr STBuiltinParameterInfo fmParams[]={
    ST_MOD_PARAM("rate","Rate","Hz",0.1,10000,100,Log),
    ST_MOD_PARAM("split","Split","Hz",20,16000,500,Log),
    ST_MOD_PARAM("contour","Contour","%",0,100,0,0),
    ST_MOD_PARAM("amount","Amount","%",0,100,30,0),
    ST_MOD_PARAM("mix","Mix","%",0,100,100,0)
};
struct FM final:Processor {
    DelayLine low[2],high[2],dry[2];double state[2]{},phase=0,envelope=0;Smoother amount,wet,split;
    explicit FM(double sr):Processor(sr),low{DelayLine(132),DelayLine(132)},high{DelayLine(132),DelayLine(132)},dry{DelayLine(132),DelayLine(132)}{}
    uint32_t latency() const noexcept override{return 64;}
    double tail() const noexcept override{return 0.15;}
    void reset() noexcept override{for(auto&v:low)v.reset();for(auto&v:high)v.reset();for(auto&v:dry)v.reset();state[0]=state[1]=phase=envelope=0;amount.reset();wet.reset();split.reset();}
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override{
        const double release=pole(80,sr),smooth=pole(5,sr),rate=std::min(p[0],sr*0.4),splitPole=std::exp(-2*pi*std::min(p[1],sr*0.45)/sr);
        for(uint32_t i=0;i<n;++i){
            const double x[2]={l[i],r[i]},pk=std::max(std::abs(x[0]),std::abs(x[1]));envelope=clean(std::max(pk,envelope*release));
            const double depth=amount.tick(p[3]/100,smooth)*mix(1,clamp(envelope,0,1),p[2]/100),w=wet.tick(p[4]/100,smooth);
            const double a=split.tick(splitPole,smooth),offset=63*depth*fastSin2pi(phase);phase=wrap(phase+rate/sr);
            for(int c=0;c<2;++c){
                state[c]=clean(x[c]+a*(state[c]-x[c]));
                const double yd=dry[c].read(64),yl=low[c].read(64),yh=high[c].read(64+offset);
                low[c].push(state[c]);high[c].push(x[c]-state[c]);dry[c].push(x[c]);
                (c?r:l)[i]=float(mix(yd,yl+yh,w));
            }
        }
    }
};
inline constexpr STBuiltinParameterInfo shiftParams[]={
    ST_MOD_PARAM("coarse","Coarse","Hz",-5000,5000,0,0),
    ST_MOD_PARAM("fine","Fine","Hz",-10,10,0,0),
    ST_MOD_PARAM("feedback","Feedback","%",-95,95,0,0),
    ST_MOD_PARAM("stereo","Stereo","%",0,100,0,0),
    ST_MOD_PARAM("invert","Invert","",0,1,0,Toggle),
    ST_MOD_PARAM("mix","Mix","%",0,100,100,0)
};
struct FrequencyShifter final:Processor {
    static constexpr int taps=63,groupDelay=31;
    // History is mirrored (each sample stored at i and i+taps) so the FIR
    // window is contiguous; only the 32 nonzero (even-index) taps are used.
    std::array<double,taps> coefficients{},dry[2]{};std::array<double,2*taps> history[2]{};
    int cursor=0;double phase[2]{},last[2]{};Smoother shift,feedback,wet;
    explicit FrequencyShifter(double sr):Processor(sr){
        // Windowed analytic-signal FIR, derived directly from the ideal
        // Hilbert transform kernel. Odd tap offsets alone are nonzero.
        for(int i=0;i<taps;++i){const int k=i-groupDelay;if(k && (k%2))coefficients[i]=2/(pi*k)*(0.54-0.46*std::cos(2*pi*i/(taps-1)));}
    }
    uint32_t latency() const noexcept override{return groupDelay;}
    double tail() const noexcept override{return 1;}
    void reset() noexcept override{for(auto&v:history)v.fill(0);for(auto&v:dry)v.fill(0);cursor=0;phase[0]=phase[1]=last[0]=last[1]=0;shift.reset();feedback.reset();wet.reset();}
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override{
        const double smooth=pole(5,sr);
        for(uint32_t i=0;i<n;++i){
            const double freq=shift.tick((p[0]+p[1])*(p[4]>=0.5?-1:1),smooth),fb=feedback.tick(p[2]/100,smooth),w=wet.tick(p[5]/100,smooth);
            for(int c=0;c<2;++c){
                const double x=(c?r:l)[i];const double h=clamp(x+last[c]*fb,-16,16);
                history[c][size_t(cursor)]=history[c][size_t(cursor+taps)]=h;dry[c][size_t(cursor)]=x;
                const int delayed=(cursor+taps-groupDelay)%taps;
                // Newest sample at cursor+taps; tap t reads t samples back.
                // Four partial sums: independent FMA chains instead of one serial chain.
                const double* window=history[c].data()+cursor+taps;double q0=0,q1=0,q2=0,q3=0;
                for(int t=0;t<64;t+=8){q0+=coefficients[size_t(t)]*window[-t];q1+=coefficients[size_t(t+2)]*window[-(t+2)];
                    q2+=coefficients[size_t(t+4)]*window[-(t+4)];if(t+6<taps)q3+=coefficients[size_t(t+6)]*window[-(t+6)];}
                const double q=(q0+q1)+(q2+q3);
                const double real=history[c][size_t(delayed)];
                last[c]=clean(real*fastCos2pi(phase[c])-q*fastSin2pi(phase[c]));
                (c?r:l)[i]=float(mix(dry[c][size_t(delayed)],last[c],w));
                phase[c]=wrap(phase[c]+freq*(c?1-2*p[3]/100:1)/sr);
            }if(++cursor==taps)cursor=0;
        }
    }
};
inline constexpr STBuiltinParameterInfo phaserParams[]={
    ST_MOD_PARAM("frequency","Frequency","Hz",20,10000,600,Log),
    ST_MOD_PARAM("feedback","Feedback","%",-95,95,30,0),
    ST_MOD_PARAM("eightpole","Eight poles","",0,1,0,Toggle),
    ST_MOD_PARAM("amount","Amount","%",-100,100,60,0),
    {"source","Modulation source","",0,2,0,Integer,sources,3},
    ST_MOD_PARAM("speed","LFO speed","Hz",0.03,8,0.4,Log),
    ST_MOD_PARAM("beats","LFO period","beats",0.25,64,4,Log),
    ST_MOD_PARAM("envshape","Envelope shape","",0.25,4,1,Log),
    ST_MOD_PARAM("stereo","Stereo","deg",0,180,90,0),
    ST_MOD_PARAM("mix","Mix","%",0,100,50,0)
};
struct Phaser final:Processor {
    double state[2][8]{},last[2]{},coefficient[2]{},phase=0,envelope=0;uint32_t control=0;Smoother frequency[2],wet,feedback;
    // Only the selected number of all-pass stages runs; stages 5-8 restart
    // from rest when Eight poles is switched on.
    int activeStages=8;
    explicit Phaser(double sr):Processor(sr){}
    void reset() noexcept override{for(auto&c:state)for(auto&v:c)v=0;last[0]=last[1]=phase=envelope=0;control=0;for(auto&v:frequency)v.reset();wet.reset();feedback.reset();activeStages=8;}
    double tail() const noexcept override{return 45;}
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override{
        const double rate=p[4]==1?tempo/(60*p[6]):p[5],attack=pole(2,sr),release=pole(100,sr),smooth=pole(5,sr);const int stages=p[2]>=0.5?8:4;
        if(stages>activeStages)for(auto&c:state)for(int j=activeStages;j<stages;++j)c[j]=0;
        activeStages=stages;
        for(uint32_t i=0;i<n;++i){
            const double x[2]={l[i],r[i]},pk=std::max(std::abs(x[0]),std::abs(x[1]));envelope=clean(pk+(pk>envelope?attack:release)*(envelope-pk));
            const double w=wet.tick(p[9]/100,smooth),fb=feedback.tick(p[1]/100,smooth);
            for(int c=0;c<2;++c){
                const double mod=p[4]==2?std::pow(clamp(envelope,0,1),p[7]):fastSin2pi(phase+c*p[8]/360);
                const double hz=frequency[c].tick(clamp(p[0]*fastExp(0.6931471805599453*(3*p[3]/100*mod)),10,sr*0.45),smooth);
                if(control==0){const double g=std::tan(pi*hz/sr);coefficient[c]=(1-g)/(1+g);}
                double y=x[c]+fastTanh(last[c])*fb;const double a=coefficient[c];
                for(int j=0;j<stages;++j){const double out=-a*y+state[c][j];state[c][j]=clean(y+a*out);y=clean(out);}
                last[c]=y;
                (c?r:l)[i]=float(mix(x[c],last[c],w));
            }control=(control+1)%8;phase=wrap(phase+rate/sr);
        }
    }
};
template<class T>Processor* create(double sr,uint32_t){return new T(sr);}
#undef ST_MOD_PARAM
}
inline const Descriptor* modulationDescriptors(size_t&count){
    static const Descriptor d[]={
        {{"chorus","Chorus","Modulation",4,0},modulation::chorusParams,modulation::create<modulation::Chorus>},
        {{"flanger","Flanger","Modulation",10,0},modulation::flangerParams,modulation::create<modulation::Flanger>},
        {{"fm","Audio FM","Modulation",5,0},modulation::fmParams,modulation::create<modulation::FM>},
        {{"freqshifter","Frequency Shifter","Modulation",6,0},modulation::shiftParams,modulation::create<modulation::FrequencyShifter>},
        {{"phaser","Phaser","Modulation",10,0},modulation::phaserParams,modulation::create<modulation::Phaser>}
    };count=sizeof(d)/sizeof(d[0]);return d;
}
}
#endif

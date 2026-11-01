#pragma once
#include "EffectsSpatial.hpp"

namespace stfx { namespace delay {
using spatial::Line;
using spatial::bounded;
using spatial::clean;
constexpr double tau=6.283185307179586476925286766559;
inline double rate(double semitones) noexcept {return std::pow(2.,bounded(semitones,-36.,36.)/12.);}
inline double bpm(double value) noexcept {return bounded(value,20.,999.);}
inline double wrapPosition(double p,size_t length) noexcept {double len=double(std::max<size_t>(length,1));return p-std::floor(p/len)*len;}
inline float at(const Line& line,double position,size_t length) noexcept {
    length=std::max<size_t>(1,std::min(length,line.data.size()));position=wrapPosition(position,length);
    size_t a=size_t(position),b=a+1==length?0:a+1;float f=float(position-a);return line.data[a]+(line.data[b]-line.data[a])*f;
}
struct BeatDelay final:Processor {
    std::array<Line,2> memory;double low[2]{},del=1.,target=1.,smoothing;bool first=true;
    explicit BeatDelay(double sr):Processor(sr),smoothing(1.-std::exp(-1./(.02*sr))){for(auto& d:memory)d.prepare(size_t(sr*51.)+8);}
    void reset() noexcept override{for(auto& d:memory)d.reset();low[0]=low[1]=0;first=true;}
    double tail() const noexcept override{return 20000.;}
    double tailForParameters(const double* values,double tempoBPM) const noexcept override {
        static constexpr double units[8]={.0625,.125,.25,.5,1.,1./6.,1./3.,2./3.};
        const double duration=std::max(1./sr,60./bpm(tempoBPM)*units[int(bounded(values[8],0,7))]*((values[0]+1.)*.5+values[1]*.01));
        const double feedback=bounded(values[2]*.01,0,.97);
        // Estimated time to -100 dB including the first echo; pure target-data
        // calculation is safe before first render and during concurrent edits.
        return duration*(feedback>0 ? 1.+std::ceil(std::log(1.e-5)/std::log(feedback)) : 1.);
    }
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override {
        static constexpr double units[8]={.0625,.125,.25,.5,1.,1./6.,1./3.,2./3.};
        const double unit=units[int(bounded(p[8],0,7))];target=std::max(1.,sr*60./bpm(tempo)*unit*((p[0]+1.)*.5+p[1]*.01));
        if(first){del=target;first=false;}
        double feedback=bounded(p[2]*.01,0,.97),cross=bounded(p[3]*.01,0,1),split=p[5]*.01,mix=bounded(p[7]*.01,0,1),width=p[6]*.01;
        double cut=250.+std::pow(p[4]*.01,2.)*18000.;double a[2]={spatial::pole(cut*std::pow(2.,-split),sr),spatial::pole(cut*std::pow(2.,split),sr)};
        for(uint32_t i=0;i<n;++i){float x=l[i],y=r[i];del+=smoothing*(target-del);float wet[2]={memory[0].read(del),memory[1].read(del)};
            for(int ch=0;ch<2;++ch)low[ch]+=a[ch]*(wet[ch]-low[ch]);
            memory[0].write(spatial::soft(float(x+feedback*((1-cross)*low[0]+cross*low[1]))));
            memory[1].write(spatial::soft(float(y+feedback*((1-cross)*low[1]+cross*low[0]))));
            const float mid=(wet[0]+wet[1])*.5f,side=float((wet[0]-wet[1])*.5*width);
            l[i]=clean(float(x*(1-mix)+(mid+side)*mix));r[i]=clean(float(y*(1-mix)+(mid-side)*mix));}
    }
};
struct Grain {
    double position=0,step=1,age=0,length=1,pan=0;bool active=false;
};
struct GrainDelay final:Processor {
    std::array<Line,2> memory;std::array<Grain,32> grains{};uint32_t random=0x91e10da5u;double clock=0;float feedback[2]{};
    explicit GrainDelay(double sr):Processor(sr){for(auto& d:memory)d.prepare(size_t(sr*6.)+8);}
    void reset() noexcept override{for(auto&d:memory)d.reset();for(auto&g:grains)g.active=false;clock=0;feedback[0]=feedback[1]=0;random=0x91e10da5u;}
    double tail() const noexcept override{return 500.;}
    double tailForParameters(const double* values,double) const noexcept override {
        const double size=values[1]*.001*(1.+values[2]*.005),space=values[4]*.001;
        const double pitch=rate(values[0])*std::pow(2.,values[6]*.0025);
        const double history=std::min(6.,space+size*std::max(1.,pitch)+std::min(space*.4,.1)*values[2]*.01);
        const double duration=history+size,feedback=bounded(values[5]*.0085,0,.85);
        return duration*(feedback>0?1.+std::ceil(std::log(1.e-5)/std::log(feedback)):1.);
    }
    double noise() noexcept{random^=random<<13;random^=random>>17;random^=random<<5;return double(random)/2147483648.-1.;}
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override{
        double pitch=rate(p[0]),size=std::max(16.,p[1]*.001*sr),jitter=p[2]*.01,space=p[4]*.001*sr,density=p[5]*.01,mod=p[6]*.01,mix=bounded(p[7]*.01,0,1),stereo=p[8]*.01;
        bool reverse=p[3]>=.5;const double interval=std::max(16.,size/(2.+density*10.)+space*.12);
        for(uint32_t i=0;i<n;++i){float x=l[i],y=r[i];
            memory[0].write(spatial::soft(x+feedback[0]*float(density*.85)));memory[1].write(spatial::soft(y+feedback[1]*float(density*.85)));
            if(clock<=0){clock+=interval*(1.+noise()*jitter*.6);for(auto&g:grains)if(!g.active){
                g.active=true;g.age=0;g.length=std::max(16.,size*(1.+noise()*jitter*.5));g.step=pitch*(reverse?-1.:1.)*std::pow(2.,noise()*mod*.25);
                // Forward grains have enough history to finish without crossing the write head.
                double history=std::max(1.,space)+(!reverse?g.length*std::max(1.,g.step):0.)+noise()*jitter*std::min(space*.4,sr*.1);
                g.position=wrapPosition(double(memory[0].pos)-history,memory[0].data.size());g.pan=noise()*stereo;break;}}
            --clock;double sum[2]{},weights=0;
            for(auto&g:grains)if(g.active){double phase=g.age/g.length,weight=4.*phase*(1.-phase);float a=at(memory[0],g.position,memory[0].data.size()),b=at(memory[1],g.position,memory[1].data.size());
                double mid=(a+b)*.5,side=(a-b)*.5*stereo;sum[0]+=(mid+side)*weight*(1.-std::max(0.,g.pan));sum[1]+=(mid-side)*weight*(1.+std::min(0.,g.pan));weights+=weight;
                g.position=wrapPosition(g.position+g.step,memory[0].data.size());if(++g.age>=g.length)g.active=false;}
            double norm=1./std::max(1.,weights);feedback[0]=clean(float(sum[0]*norm));feedback[1]=clean(float(sum[1]*norm));
            l[i]=clean(float(x*(1-mix)+feedback[0]*mix));r[i]=clean(float(y*(1-mix)+feedback[1]*mix));}
    }
};
struct GrainStretch final:Processor {
    std::array<Line,2> capture;std::array<Grain,8> grains{};size_t captured=0,captureLength=0;double cursor=0,clock=0;bool wasOn=false;
    explicit GrainStretch(double sr):Processor(sr){for(auto&d:capture)d.prepare(size_t(sr*24.)+8);}
    void reset() noexcept override{for(auto&d:capture)d.reset();for(auto&g:grains)g.active=false;captured=0;captureLength=0;cursor=clock=0;wasOn=false;}
    double tail() const noexcept override{return 120.;}
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override{
        bool on=p[0]>=.5;if(!on){wasOn=false;return;}
        if(!wasOn){captured=0;captureLength=std::min(capture[0].data.size(),size_t(std::ceil(sr*60./bpm(tempo)*8.)));cursor=clock=0;for(auto&g:grains)g.active=false;wasOn=true;}
        const double pitch=rate(p[3]),speed=p[1]*.01,mix=bounded(p[6]*.01,0,1);double size=std::max(16.,p[5]*.001*sr/(p[4]>=.5?pitch:1.));
        size=std::min(size,sr*.8);size_t wantedLoop=std::max<size_t>(1,size_t(sr*60./bpm(tempo)*.25*p[2]));
        for(uint32_t i=0;i<n;++i){float x=l[i],y=r[i];if(captured<captureLength){capture[0].data[captured]=x;capture[1].data[captured]=y;++captured;}
            const size_t loop=std::max<size_t>(1,std::min(captured,wantedLoop));
            // During the first grain there is no complete source yet: monitor the input.
            if(captured<size){continue;}
            if(clock<=0){clock+=size*.25;for(auto&g:grains)if(!g.active){g.active=true;g.age=0;g.length=size;g.step=pitch;g.position=wrapPosition(cursor,loop);break;}}
            --clock;double sum[2]{},weights=0;
            for(auto&g:grains)if(g.active){double phase=g.age/g.length,weight=4.*phase*(1.-phase);sum[0]+=at(capture[0],g.position,loop)*weight;sum[1]+=at(capture[1],g.position,loop)*weight;weights+=weight;g.position=wrapPosition(g.position+g.step,loop);if(++g.age>=g.length)g.active=false;}
            cursor=wrapPosition(cursor+speed,loop);double norm=1./std::max(1.,weights);
            l[i]=clean(float(x*(1-mix)+sum[0]*norm*mix));r[i]=clean(float(y*(1-mix)+sum[1]*norm*mix));}
    }
};
struct Resochord final:Processor {
    std::array<Line,6> comb;std::array<double,6> lengths{},filtered{},gain{};double smooth;bool initialized=false;spatial::ZeroMix zero;
    explicit Resochord(double sr):Processor(sr),smooth(1.-std::exp(-1./(.01*sr))){for(auto&d:comb)d.prepare(size_t(sr*.15)+8);}
    void reset() noexcept override{for(auto&d:comb)d.reset();filtered.fill(0);initialized=false;zero.reset();}
    double tail() const noexcept override{return 40.;}
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override{
        static constexpr int chords[8][6]={{0,4,7,12,16,19},{0,3,7,12,15,19},{0,4,7,11,14,19},{0,3,7,10,14,19},{0,4,7,10,14,19},{0,5,7,12,17,19},{0,3,6,9,12,15},{0,4,8,12,16,20}};
        const bool string=p[0]>=.5;int style=int(bounded(p[1],0,7)),root=int(bounded(p[2],0,11));double tune=p[3],bright=bounded(p[4]*.01,0,1),feedback=bounded(p[5]*.01,0,.99),decay=bounded(p[6],.03,20.),mix=bounded(p[7]*.01,0,1),spread=p[8]*.01;
        bool restart;
        if(zero.skip(mix==0,*this,n,restart)){for(uint32_t i=0;i<n;++i){l[i]=clean(float(l[i]*(1-mix)));r[i]=clean(float(r[i]*(1-mix)));}return;}
        if(restart){for(auto&d:comb)d.reset();filtered.fill(0);initialized=false;}
        std::array<double,6> target{};double lowpass=spatial::pole(1200.+bright*bright*18500.,sr);
        for(size_t k=0;k<6;++k){double semitones=root+tune+(string?(double(k)-2.5)*spread*.6:chords[style][k]);double hz=130.81278265*std::pow(2.,semitones/12.);target[k]=sr/bounded(hz,12.,sr*.4);gain[k]=std::min(.9998,std::pow(.001,target[k]/(sr*decay))*(.65+feedback*.35));}
        if(!initialized){lengths=target;initialized=true;}
        for(uint32_t i=0;i<n;++i){float x=l[i],y=r[i];double wet[2]{};
            for(size_t k=0;k<6;++k){lengths[k]+=smooth*(target[k]-lengths[k]);float v=comb[k].read(lengths[k]);filtered[k]+=lowpass*(v-filtered[k]);comb[k].write(spatial::soft(float((x+y)*.15+filtered[k]*gain[k])));wet[k&1]+=v*.28;}
            l[i]=clean(float(x*(1-mix)+wet[0]*mix));r[i]=clean(float(y*(1-mix)+wet[1]*mix));}
    }
};
inline Processor* makeBeat(double sr,uint32_t){return new BeatDelay(sr);}inline Processor* makeGrain(double sr,uint32_t){return new GrainDelay(sr);}
inline Processor* makeStretch(double sr,uint32_t){return new GrainStretch(sr);}inline Processor* makeReso(double sr,uint32_t){return new Resochord(sr);}
inline const char* const times[]={"0.5","1","1.5","2","2.5","3","3.5","4","4.5","5","5.5","6","6.5","7","7.5","8","8.5","9","9.5","10","10.5","11","11.5","12","12.5","13","13.5","14","14.5","15","15.5","16"};
inline const char* const units[]={"1/64","1/32","1/16","1/8","1/4","1/16T","1/8T","1/4T"};inline const char* const resoModes[]={"Chord","String"};
inline const char* const styles[]={"Major","Minor","Major 7","Minor 7","Dominant 7","Suspended","Diminished","Augmented"};
inline const char* const roots[]={"C","C#","D","D#","E","F","F#","G","G#","A","A#","B"};
inline const STBuiltinParameterInfo beatParams[]={
    {"time","Time","units",0,31,7,2,times,32},{"offset","Offset","% unit",-49,100,0,0,nullptr,0},{"feedback","Feedback","%",0,97,35,0,nullptr,0},{"crossover","Crossover","%",0,100,0,0,nullptr,0},
    {"color","Color","%",0,100,75,0,nullptr,0},{"split","Split","%",0,100,0,0,nullptr,0},{"stereo","Stereo","%",-100,100,100,0,nullptr,0},{"mix","Mix","%",0,100,30,0,nullptr,0},{"unit","Unit","",0,7,2,2,units,8}};
inline const STBuiltinParameterInfo grainParams[]={
    {"pitch","Pitch","st",-24,24,0,0,nullptr,0},{"size","Size","ms",5,500,80,1,nullptr,0},{"jitter","Jitter","%",0,100,10,0,nullptr,0},{"reverse","Reverse","",0,1,0,4,nullptr,0},
    {"space","Space","ms",0,2000,180,0,nullptr,0},{"density","Density","%",0,100,45,0,nullptr,0},{"mod","Mod","%",0,100,0,0,nullptr,0},{"mix","Mix","%",0,100,35,0,nullptr,0},{"stereo","Stereo","%",0,100,80,0,nullptr,0}};
inline const STBuiltinParameterInfo stretchParams[]={
    {"on","On / Capture","",0,1,0,4,nullptr,0},{"stretch","Stretch","%",0,200,50,0,nullptr,0},{"loop","Loop","1/16 steps",1,32,16,2,nullptr,0},{"pitch","Pitch","st",-24,24,0,0,nullptr,0},
    {"link","Link","",0,1,1,4,nullptr,0},{"size","Size","ms",5,300,80,1,nullptr,0},{"mix","Mix","%",0,100,100,0,nullptr,0}};
inline const STBuiltinParameterInfo resoParams[]={
    {"mode","Mode","",0,1,0,2,resoModes,2},{"style","Style","",0,7,0,2,styles,8},{"chord","Chord Root","",0,11,0,2,roots,12},{"tune","Tune","st",-24,24,0,0,nullptr,0},
    {"brightness","Brightness","%",0,100,70,0,nullptr,0},{"feedback","Feedback","%",0,99,90,0,nullptr,0},{"decay","Decay","s",.03,20,2,1,nullptr,0},{"mix","Mix","%",0,100,35,0,nullptr,0},{"spread","Spread","%",0,100,25,0,nullptr,0}};
} // namespace delay
inline const Descriptor* delayDescriptors(size_t& count){static const Descriptor all[]={
    {{"beatdelay","Beat Delay","Delay",9,0},delay::beatParams,delay::makeBeat},{{"graindelay","Grain Delay","Delay",9,0},delay::grainParams,delay::makeGrain},
    {{"grainstretch","Grain Stretch","Delay",7,0},delay::stretchParams,delay::makeStretch},{{"resochord","Resochord","Delay",9,0},delay::resoParams,delay::makeReso}};count=sizeof(all)/sizeof(all[0]);return all;}
} // namespace stfx

#ifndef ST_EFFECTS_PERFORM_HPP
#define ST_EFFECTS_PERFORM_HPP
#include "BuiltinEffectsCommon.hpp"

// Original performance DSP, informed by the public NI effect-reference feature
// descriptions, not by NI code or presets. Parameter order is persistent and
// grouped into successive banks of eight controls for keyboard/hardware pages.
namespace stfx {
namespace perform_detail {
// Per-sample control derivations (dB gains, poles, pow/tan of smoothed
// values) are memoized: once smoothing has settled they cost a comparison.
struct Base : Processor {
    std::array<Smoother,24> smooth{};
    std::array<Memo,24> poleMemo{};
    double smoothing;
    explicit Base(double rate):Processor(rate),smoothing(pole(12,rate)) {}
    ST_HOT double value(size_t index) noexcept { return smooth[index].tick(p[index],smoothing); }
    ST_HOT double value(size_t index,double ms) noexcept { return smooth[index].tick(p[index],poleMemo[index]([&]{return pole(ms,sr);},ms)); }
    void resetParameters() noexcept { for(auto &v:smooth)v.reset(); }
    double beatSeconds(double beats) const noexcept { return beats*60.0/clamp(tempo,20,999); }
};
struct OnePole {
    double z=0;
    ST_HOT double tick(double x,double coefficient) noexcept { z=clean(x+coefficient*(z-x));return z; }
    void reset() noexcept {z=0;}
};
inline double lowPole(double frequency,double rate) noexcept {return std::exp(-2*pi*clamp(frequency,5,rate*.45)/rate);}
ST_INLINE double bounded(double x) noexcept {return std::isfinite(x)?clamp(x,-16,16):0;}
ST_INLINE double soft(double x) noexcept {return fastTanh(clamp(x,-40,40));}
// T120 bound for a contractive feedback path; noncontractive
// nonlinear loops instead use the explicit host export cap. These helpers read
// only immutable rate and the host's supplied physical parameter snapshot.
inline double feedbackTail(double seconds,double gain) noexcept {
    if(gain>=1)return 120;
    const double repeats=gain>1e-12?std::ceil(std::log(1e-6)/std::log(gain)):0;
    return seconds*(1+std::max(0.,repeats))+.25;
}
inline double division(double beats) noexcept {
    static constexpr double values[]={1./32,1./24,1./16,1./12,1./8,1./6,1./4,1./3,.5,2./3,1,1.5,2,3,4};
    double best=values[0],distance=1e9;
    for(double v:values){const double d=std::abs(std::log2(std::max(beats,1e-9)/v));if(d<distance){distance=d;best=v;}}
    return best;
}
// Stable topology-preserving state-variable filter. Softly bounded integrators
// allow negative damping for controlled self oscillation, rather than a runaway.
struct SVF {
    double s1=0,s2=0;Memo coefficients;double g=0,a=0;
    struct Out { double low,band,high; };
    ST_HOT Out tick(double input,double frequency,double damping,double rate) noexcept {
        coefficients([&]{g=std::tan(pi*clamp(frequency,10,rate*.42)/rate);a=1/(1+g*(g+damping));return 0.;},frequency,damping,rate);
        const double band=a*(s1+g*(input-s2)),low=s2+g*band;
        s1=clean(4*soft((2*band-s1)*.25));s2=clean(4*soft((2*low-s2)*.25));
        return {bounded(low),bounded(band),bounded(input-damping*band-low)};
    }
    // Same filter step, band-pass output only.
    ST_HOT double band(double input,double frequency,double damping,double rate) noexcept {
        coefficients([&]{g=std::tan(pi*clamp(frequency,10,rate*.42)/rate);a=1/(1+g*(g+damping));return 0.;},frequency,damping,rate);
        const double band=a*(s1+g*(input-s2)),low=s2+g*band;
        s1=clean(4*soft((2*band-s1)*.25));s2=clean(4*soft((2*low-s2)*.25));
        return bounded(band);
    }
    void reset() noexcept {s1=s2=0;}
};
// N independent SVF lanes stepped together (structure of arrays), so the
// per-sample work vectorizes. Same arithmetic as SVF, lane by lane.
template<size_t N> struct SVFBank {
    double s1[N]{},s2[N]{},g[N]{},a[N]{};
    static double cleanLane(double x) noexcept {const double m=std::abs(x);return m>1e-30&&m<=std::numeric_limits<double>::max()?std::fmin(1e6,std::fmax(-1e6,x)):0.;}
    static double boundLane(double x) noexcept {return std::abs(x)<=std::numeric_limits<double>::max()?std::fmin(16.,std::fmax(-16.,x)):0.;}
    void set(size_t k,double frequency,double damping,double rate) noexcept {g[k]=std::tan(pi*clamp(frequency,10,rate*.42)/rate);a[k]=1/(1+g[k]*(g[k]+damping));}
    void band(const double* input,double* out) noexcept {
        for(size_t k=0;k<N;k++){
            const double b=a[k]*(s1[k]+g[k]*(input[k]-s2[k])),low=s2[k]+g[k]*b;
            s1[k]=cleanLane(4*fastTanh((2*b-s1[k])*.25));s2[k]=cleanLane(4*fastTanh((2*low-s2[k])*.25));
            out[k]=boundLane(b);
        }
    }
    void reset() noexcept {for(size_t k=0;k<N;k++)s1[k]=s2[k]=0;}
};
// An actual single-sideband shifter: windowed Hilbert FIR plus a matched-delay
// real path. Fixed arrays, 31-sample internal group delay, no heap in processing.
// History is mirrored (each sample stored twice) so the FIR reads one
// contiguous window; only the 32 nonzero (even-index) taps are evaluated.
struct FrequencyShifter {
    std::array<double,126> history{};std::array<double,32> taps{};
    size_t write=0;
    FrequencyShifter() {
        for(int i=0;i<63;i+=2){const int n=i-31;const double w=.42-.5*std::cos(2*pi*i/62)+.08*std::cos(4*pi*i/62);
            taps[size_t(i/2)]=2*w/(pi*n);}
    }
    // Store one input sample; the shifted output can then be read with output().
    ST_HOT void push(double input) noexcept {history[write]=history[write+63]=input;write=write==62?0:write+1;}
    // Output for the most recently pushed sample (write has advanced past it).
    ST_HOT double output(double phase) const noexcept {
        // newest sample at index write+62 (mirrored); tap i multiplies the sample i back.
        const double* window=history.data()+write+62;
        // Four partial sums: independent FMA chains instead of one serial chain.
        double i0=0,i1=0,i2=0,i3=0;
        for(size_t k=0;k<32;k+=4){i0+=taps[k]*window[-ptrdiff_t(2*k)];i1+=taps[k+1]*window[-ptrdiff_t(2*k+2)];i2+=taps[k+2]*window[-ptrdiff_t(2*k+4)];i3+=taps[k+3]*window[-ptrdiff_t(2*k+6)];}
        const double imaginary=(i0+i1)+(i2+i3);
        const double real=window[-31];
        return real*fastCos2pi(phase)-imaginary*fastSin2pi(phase);
    }
    double tick(double input,double phase) noexcept {push(input);return output(phase);}
    void reset() noexcept {history.fill(0);write=0;}
};
// Dense short diffusion + a four-line orthogonal feedback network. Fixed
// incommensurate lengths avoid a single obvious pitched echo in Ring's tail.
struct Plate {
    std::array<DelayLine,4> lines;
    std::array<double,4> lengths;
    OnePole damp[4];double apL=0,apR=0;
    Memo dampPole,gainMemo;std::array<double,4> gains{};
    explicit Plate(double sr):lines{DelayLine(size_t(sr*.0311)+4),DelayLine(size_t(sr*.0379)+4),DelayLine(size_t(sr*.0437)+4),DelayLine(size_t(sr*.0521)+4)},lengths{sr*.0311,sr*.0379,sr*.0437,sr*.0521}{}
    ST_HOT std::array<double,2> tick(double left,double right,double decay,double rate) noexcept {
        const double lp=dampPole([&]{return lowPole(7500,rate);},rate);
        gainMemo([&]{for(size_t j=0;j<4;j++)gains[j]=std::pow(.001,lengths[j]/rate/std::max(.08,decay));return 0.;},decay,rate);
        double v[4];for(size_t j=0;j<4;j++)v[j]=damp[j].tick(lines[j].read(lengths[j]),lp);
        const double h[4]={(v[0]+v[1]+v[2]+v[3])*.5,(v[0]-v[1]+v[2]-v[3])*.5,(v[0]+v[1]-v[2]-v[3])*.5,(v[0]-v[1]-v[2]+v[3])*.5};
        const double a=left*.7+apL*.3,b=right*.7+apR*.3;apL=left-a*.3;apR=right-b*.3;
        const double input[4]={a,b,(a+b)*.5,(a-b)*.5};
        for(size_t j=0;j<4;j++)lines[j].push(bounded(input[j]*.35+h[j]*gains[j]));
        return {(v[0]+v[2]+v[3])*.55,(v[1]+v[2]-v[3])*.55};
    }
    void reset() noexcept {for(auto &d:lines)d.reset();for(auto &f:damp)f.reset();apL=apR=0;}
};
// Stereo capture storage uses floats to keep long, high-rate history practical.
// Freeze is just retaining the write index: no copying or allocation on Engage.
struct Capture {
    std::vector<std::array<float,2>> data;
    size_t write=0,valid=0,end=0,capturedValid=0;uint64_t frames=0;
    explicit Capture(size_t capacity):data(std::max<size_t>(capacity,8)){}
    ST_HOT void push(double l,double r) noexcept {data[write]={float(bounded(l)),float(bounded(r))};if(++write==data.size())write=0;valid=std::min(valid+1,data.size()-2);}
    void capture(size_t back=0) noexcept {back=std::min(back,valid);end=(write+data.size()-back)%data.size();capturedValid=valid-back;}
    ST_HOT std::array<double,2> read(double length,double position) const noexcept {
        if(capturedValid<2)return {0,0};
        length=clamp(length,2,double(std::max<size_t>(2,capturedValid)));position=wrap(position/length)*length;
        double absolute=double(end)-length+position;while(absolute<0)absolute+=double(data.size());
        const size_t a=size_t(absolute)%data.size(),b=(a+1)%data.size();const double f=absolute-std::floor(absolute);
        return {mix(data[a][0],data[b][0],f),mix(data[a][1],data[b][1],f)};
    }
    void reset() noexcept {std::fill(data.begin(),data.end(),std::array<float,2>{});write=valid=end=capturedValid=0;frames=0;}
};
inline double seam(double phase,double length,double fade) noexcept {
    const double edge=std::min(phase,length-phase);return edge<fade?.5-.5*std::cos(pi*clamp(edge/std::max(1.,fade),0,1)):1;
}
}

class PerformFilter final : public perform_detail::Base {
    perform_detail::SVF filter[2];Memo driveGain,outGain,center;
    // While Engage x Mix is exactly zero the output is the dry input: the
    // filter is not run, and restarts from rest when it is audible again.
    bool live=false;
public:
    explicit PerformFilter(double sr):Base(sr){}
    void reset() noexcept override {resetParameters();for(auto &f:filter)f.reset();live=false;}
    double tail() const noexcept override{return 3e18;}
    double tailForParameters(const double* v,double) const noexcept override {
        const double release=.001*clamp(v[7],1,100)*16+.1;
        if(v[0]<.5)return release;
        const double damping=2-2.08*clamp(v[3],0,1.05);
        // Zero/negative damping is an intentional oscillator. Small positive
        // damping still gets its finite decay estimate, however long it is.
        if(damping<=0)return 120;
        const double lo=clamp(v[8],20,sr*.42),hi=std::max(lo,clamp(v[9],20,sr*.42));
        const double frequency=lo*std::pow(hi/lo,clamp(v[1],0,1));
        return std::log(1e6)/(pi*frequency*damping)+release;
    }
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override {
        for(uint32_t i=0;i<n;i++){
            const double ms=value(7),engage=value(0,ms),position=value(1,ms),type=value(2),res=value(3),v4=value(4),v5=value(5),amount=value(6);
            const double drive=driveGain([&]{return dbGain(v4);},v4),out=outGain([&]{return dbGain(v5);},v5);
            const double lo=clamp(value(8),20,sr*.42),hi=clamp(value(9),20,sr*.42),hz=center([&]{return lo*std::pow(std::max(hi,lo)/lo,position);},lo,hi,position);
            const double damping=2-2.08*res;
            const double dry[2]={l[i],r[i]};double wet[2]={0,0};
            const double amountWet=engage*amount;
            if(amountWet!=0){
                if(!live){for(auto &f:filter)f.reset();live=true;}
                for(int c=0;c<2;c++){const auto v=filter[c].tick(perform_detail::soft(dry[c]*drive),hz,damping,sr);wet[c]=(type<1?mix(v.low,v.band,type):mix(v.band,v.high,type-1))*out;}
            }else live=false;
            l[i]=float(perform_detail::bounded(mix(dry[0],wet[0],amountWet)));r[i]=float(perform_detail::bounded(mix(dry[1],wet[1],amountWet)));
        }
    }
};

class PerformFlanger final : public perform_detail::Base {
    DelayLine left,right;double phase=0,ap[2]{};Smoother delay;Memo driveGain,invSqrt,feedbackGain;
    // Disengaged (Engage settled at 0) the output is exactly dry: the comb is
    // not run, and restarts from silence when engaged again.
    bool live=false;
public:
    explicit PerformFlanger(double sr):Base(sr),left(size_t(sr*.2)+8),right(size_t(sr*.2)+8){}
    void reset() noexcept override {resetParameters();left.reset();right.reset();delay.reset();phase=ap[0]=ap[1]=0;live=true;}
    // Target-free bounds include representable feedback arbitrarily below one;
    // callers should use the useful, parameter-dependent estimate below.
    double tail() const noexcept override{return 3e16;}
    double tailForParameters(const double* v,double) const noexcept override {
        if(v[0]<.5)return .25;
        const double base=1/clamp(v[1],10,4000);
        const double gain=std::min(.995,std::pow(.001,base/clamp(v[2],.02,12)))*dbGain(clamp(v[3],0,24)*.5);
        return perform_detail::feedbackTail(base*(1+clamp(v[4],0,1)*.03+clamp(v[9],0,1)*.45),gain);
    }
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override {
        for(uint32_t i=0;i<n;i++){
            const double on=value(0),hz=value(1),decay=value(2),v3=value(3),spread=value(4),polarity=1-2*value(5),tone=value(6),amount=value(7),rate=value(8),depth=value(9);
            phase=wrap(phase+rate/sr);const double base=delay.tick(sr/std::max(10.,hz),smoothing);
            if(on==0&&p[0]<.5){
                if(live){left.reset();right.reset();ap[0]=ap[1]=0;live=false;}
                l[i]=float(perform_detail::bounded(l[i]));r[i]=float(perform_detail::bounded(r[i]));continue;
            }
            live=true;
            const double drive=driveGain([&]{return dbGain(v3);},v3),scale=invSqrt([&]{return 1/std::sqrt(drive);},drive);
            const double dl=clamp(base*(1+depth*.45*fastSin2pi(phase)),1,double(left.data.size()-2));
            const double dr=clamp(base*(1+spread*.03+depth*.45*fastSin2pi(phase+spread*.25)),1,double(right.data.size()-2));
            double a=left.read(dl),b=right.read(dr);const double fb=feedbackGain([&]{return std::min(.995,std::pow(.001,base/sr/std::max(.02,decay)));},base,decay)*polarity;
            const double oldL=ap[0],oldR=ap[1];ap[0]=perform_detail::bounded(a+.55*oldL);ap[1]=perform_detail::bounded(b+.55*oldR);
            a=mix(a,oldL-.55*ap[0],tone);b=mix(b,oldR-.55*ap[1],tone);
            left.push(l[i]*on+perform_detail::soft(mix(a,b,spread*.45)*fb*drive)*scale);
            right.push(r[i]*on+perform_detail::soft(mix(b,a,spread*.45)*fb*drive)*scale);
            l[i]=float(perform_detail::bounded(mix(l[i],(l[i]+a)*.70710678118,on*amount)));
            r[i]=float(perform_detail::bounded(mix(r[i],(r[i]+b)*.70710678118,on*amount)));
        }
    }
};

class BurstEcho final : public perform_detail::Base {
    DelayLine left,right;perform_detail::OnePole low[2],high[2];Smoother delay,dryGain,feedGain;
    bool wasOn=false;uint64_t elapsed=0;
    Memo driveGain,outGain,invSqrt,positionBeats,divided,filterPoles,constants;double lp=0,hp=0,feedPole=0,dryPole=0;
public:
    explicit BurstEcho(double sr):Base(sr),left(size_t(sr*20)+8),right(size_t(sr*20)+8){}
    void reset() noexcept override {resetParameters();left.reset();right.reset();for(auto&f:low)f.reset();for(auto&f:high)f.reset();delay.reset();dryGain.reset();feedGain.reset();wasOn=false;elapsed=0;}
    double tail() const noexcept override{return 3e18;}
    double tailForParameters(const double* v,double bpm) const noexcept override {
        const double assignment=clamp(v[2],0,1),position=clamp(v[1],0,1);
        const double beats=mix(clamp(v[3],1./32,4),std::pow(128.,position)/32.,assignment);
        const double seconds=v[4]>=2.5?clamp(v[10],1,2000)*.001:
            perform_detail::division(beats)*60/clamp(bpm,20,999)*(v[4]<.5?1:v[4]<1.5?1.5:2./3);
        const double gain=mix(clamp(v[5],0,.98),position*.98,1-assignment)*
            (v[0]>=.5?1:.45)*dbGain(clamp(v[11],0,18)*.5);
        return perform_detail::feedbackTail(seconds,gain);
    }
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override {
        constants([&]{feedPole=pole(2,sr);dryPole=pole(4,sr);return 0.;},sr);
        for(uint32_t i=0;i<n;i++){
            const bool on=p[0]>=.5;if(on&&!wasOn)elapsed=0;wasOn=on;
            const double engage=value(0),position=value(1),assignment=value(2),beats=value(3),mode=p[4],feedback=value(5),filter=value(6),wet=value(7),feed=value(8),mixMode=p[9],freeMS=value(10),v11=value(11),stereo=value(12),v13=value(13);
            const double drive=driveGain([&]{return dbGain(v11);},v11),out=outGain([&]{return dbGain(v13);},v13),scale=invSqrt([&]{return 1/std::sqrt(drive);},drive);
            const double selectedBeats=mix(beats,positionBeats([&]{return std::pow(128.,position)/32.;},position),assignment);
            const double seconds=mode>=2.5?freeMS*.001:beatSeconds(divided([&]{return perform_detail::division(selectedBeats);},selectedBeats))*(mode<.5?1:mode<1.5?1.5:2./3);
            const double frames=delay.tick(clamp(seconds*sr,1,double(left.data.size()-2)),smoothing);
            const bool feeding=on&&(feed>=.999||double(elapsed)<feed*sr);
            const double inputGain=feedGain.tick(feeding?1.:0.,feedPole);
            const double fb=mix(feedback,position*.98,1-assignment)*mix(.45,1,engage);
            double a=left.read(frames),b=right.read(frames*(1-stereo*.013));
            filterPoles([&]{lp=perform_detail::lowPole(18000*std::pow(.055,filter),sr);hp=perform_detail::lowPole(15*std::pow(100.,filter),sr);return 0.;},filter);
            const double values[2]={a,b};double loop[2];
            for(int c=0;c<2;c++){const double x=low[c].tick(values[c],lp);loop[c]=x-high[c].tick(x,hp);}
            left.push(l[i]*inputGain+perform_detail::soft(mix(loop[0],loop[1],stereo)*fb*drive)*scale);
            right.push(r[i]*inputGain+perform_detail::soft(mix(loop[1],loop[0],stereo)*fb*drive)*scale);
            const double targetDry=mixMode>=1.5?0:mixMode>=.5&&on&&double(elapsed)>=frames?0:1;
            const double dry=dryGain.tick(targetDry,dryPole);
            l[i]=float(perform_detail::bounded((l[i]*dry+a*wet)*out));r[i]=float(perform_detail::bounded((r[i]*dry+b*wet)*out));
            if(on)elapsed++;
        }
    }
};

class ResoEcho final : public perform_detail::Base {
    DelayLine left,right;perform_detail::SVFBank<6> bank;perform_detail::OnePole envelope[2];Smoother delay,dryGain;
    Memo driveGain,invSqrt,bankMemo,constants;double envelopePole=0;
public:
    explicit ResoEcho(double sr):Base(sr),left(size_t(sr*12)+8),right(size_t(sr*12)+8){}
    void reset() noexcept override {resetParameters();left.reset();right.reset();bank.reset();for(auto&f:envelope)f.reset();delay.reset();dryGain.reset();}
    double tail() const noexcept override{return 3e18;}
    double tailForParameters(const double* v,double bpm) const noexcept override {
        const double damping=.12+1.6*(1-clamp(v[7],0,1));
        const double gain=clamp(v[3],0,.98)*dbGain(clamp(v[8],0,24)*.5)/damping;
        if(gain>=1)return 120;
        const double seconds=mix(clamp(v[9],1,2000)*.001,
            clamp(v[2],1./32,4)*60/clamp(bpm,20,999),clamp(v[10],0,1));
        const double filterDecay=std::log(1e6)/(pi*clamp(v[1],20,4000)*.98*damping);
        return perform_detail::feedbackTail(seconds+filterDecay,gain);
    }
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override {
        constants([&]{envelopePole=pole(30,sr);return 0.;},sr);
        for(uint32_t i=0;i<n;i++){
            const double on=value(0),frequency=value(1),beats=value(2),feedback=value(3),pump=value(4),wet=value(5),mode=value(6),res=value(7),v8=value(8),freeMS=value(9),sync=value(10),spread=value(11);
            const double drive=driveGain([&]{return dbGain(v8);},v8),scale=invSqrt([&]{return 1/std::sqrt(drive);},drive);
            const double frames=delay.tick(clamp(mix(freeMS*.001,beatSeconds(beats),sync)*sr,1,double(left.data.size()-2)),smoothing);
            const double echoes[2]={left.read(frames),right.read(frames*(1-spread*.02))},input[2]={l[i],r[i]};double filtered[2],loop[2];
            const double damping=.12+1.6*(1-res);
            bankMemo([&]{for(int c=0;c<2;c++)for(int j=0;j<3;j++)bank.set(size_t(c*3+j),frequency*std::pow(2.,j*.5+spread*(c?1:-1)*.02),damping,sr);return 0.;},frequency,spread,damping);
            double excite[6],bands[6];
            for(int k=0;k<6;k++)excite[k]=input[k/3]*on+echoes[k/3]*feedback;
            bank.band(excite,bands);
            for(int c=0;c<2;c++){
                double sum=0;for(int j=0;j<3;j++)sum+=bands[c*3+j];
                filtered[c]=sum/3;const double env=envelope[c].tick(std::abs(filtered[c]),envelopePole);
                loop[c]=perform_detail::soft(filtered[c]*drive/(1+pump*env*8))*scale;
            }
            left.push(loop[0]);right.push(loop[1]);
            const double dry=dryGain.tick(1-clamp(mode-1,0,1),smoothing);
            const double directL=mix(input[0],filtered[0]*wet,on*std::min(1.,mode))*dry;
            const double directR=mix(input[1],filtered[1]*wet,on*std::min(1.,mode))*dry;
            l[i]=float(perform_detail::bounded(directL+echoes[0]*wet));r[i]=float(perform_detail::bounded(directR+echoes[1]*wet));
        }
    }
};

class PerformRing final : public perform_detail::Base {
    perform_detail::Plate plate;std::array<double,5> phases{};Smoother frequency;
    Memo glidePole,outGain,increments;std::array<double,5> step{};
public:
    explicit PerformRing(double sr):Base(sr),plate(sr){}
    void reset() noexcept override {resetParameters();plate.reset();phases.fill(0);frequency.reset();}
    double tail() const noexcept override{return 30.25;}
    double tailForParameters(const double* v,double) const noexcept override {
        // Plate decay is T60; twice that covers T120 plus the longest line.
        return clamp(v[4],.1,15)*2+.25;
    }
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override {
        static constexpr double intervals[5]={-12,-7,0,7,12};
        for(uint32_t i=0;i<n;i++){
            const double on=value(0),target=p[1],spread=value(2),glide=value(3),decay=value(4),reverb=value(5),amount=value(6),wetOnly=value(7),v8=value(8);
            const double out=outGain([&]{return dbGain(v8);},v8);
            const double hz=frequency.tick(target,glidePole([&]{return pole(std::max(1.,glide),sr);},glide));
            increments([&]{for(size_t j=0;j<5;j++)step[j]=clamp(hz*std::pow(2.,intervals[j]*spread/12),1,sr*.45)/sr;return 0.;},hz,spread);
            double carrierL=0,carrierR=0;
            for(size_t j=0;j<5;j++)phases[j]=wrap(phases[j]+step[j]);
            // Disengaged, the carriers multiply by zero: skip the oscillators.
            if(on!=0)for(size_t j=0;j<5;j++){carrierL+=fastSin2pi(phases[j]);carrierR+=fastSin2pi(phases[j]+spread*(double(j)-2)*.04);}
            const double a=l[i]*carrierL*.2*on,b=r[i]*carrierR*.2*on;const auto tail=plate.tick(a,b,decay,sr);
            const double dry=(1-wetOnly)*(1-amount*on);
            l[i]=float(perform_detail::bounded((l[i]*dry+mix(a,tail[0],reverb)*amount)*out));r[i]=float(perform_detail::bounded((r[i]*dry+mix(b,tail[1],reverb)*amount)*out));
        }
    }
};

class PerformStutter final : public perform_detail::Base {
    perform_detail::Capture history;bool wasOn=false;double phase=0,lastPosition=.5,direction=1;Smoother loopLength,speed;
    Memo requestedMemo,secondsMemo,pitchMemo;
public:
    explicit PerformStutter(double sr):Base(sr),history(size_t(sr*12)+8){}
    void reset() noexcept override {resetParameters();history.reset();wasOn=false;phase=0;lastPosition=.5;direction=1;loopLength.reset();speed.reset();}
    double tail() const noexcept override{return 120;}
    double tailForParameters(const double* v,double) const noexcept override {return v[0]>=.5?120:.25;}
    // The capture history must keep recording silence.
    bool gateable() const noexcept override {return false;}
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override {
        for(uint32_t i=0;i<n;i++){
            const double inputL=l[i],inputR=r[i];
            const bool on=p[0]>=.5;const double engage=value(0),position=value(1),assign=p[2],beats=value(3),pitch=value(4),gate=value(5),dir=p[6],quantize=p[7],freeMS=value(8),amount=value(9),fadeMS=value(10);
            const double scale=requestedMemo([&]{return std::pow(2.,assign<.5?4*(position-.5):0);},assign,position);
            const double requested=clamp(beats*scale,1./32,4);
            const double seconds=secondsMemo([&]{return quantize>=.5?beatSeconds(perform_detail::division(requested)):clamp(freeMS*scale,5,2000)*.001;},quantize,requested,freeMS*scale,tempo);
            const double targetLength=seconds*sr;
            if(on&&!wasOn){const size_t back=quantize>=.5?size_t(std::fmod(double(history.frames),std::max(2.,targetLength))):0;history.capture(back);phase=0;direction=dir>=.5&&dir<1.5?-1:1;}
            const size_t available=on?history.capturedValid:history.valid;
            const double length=clamp(loopLength.tick(clamp(targetLength,2,double(std::max<size_t>(2,available))),smoothing),2,double(std::max<size_t>(2,available)));
            if(dir<.5)direction=1;else if(dir<1.5)direction=-1;else if(std::abs(position-lastPosition)>1e-7)direction=position>lastPosition?1:-1;
            const double semitones=clamp(pitch+(assign>=.5?(position-.5)*48:0),-48,48);
            lastPosition=position;const double rate=speed.tick(direction*pitchMemo([&]{return std::pow(2.,semitones/12);},semitones),smoothing);
            phase=wrap(phase/length)*length;const auto wet=history.read(length,phase);
            const double cycle=phase/length,duty=.03+.97*gate;
            double shape=perform_detail::seam(phase,length,std::min(fadeMS*.001*sr,length*.05));
            if(gate<.999){const double fade=std::min(.02,duty*.1);shape*=cycle>=duty?0:clamp((duty-cycle)/fade,0,1);}
            l[i]=float(perform_detail::bounded(mix(l[i],wet[0]*shape,engage*amount)));r[i]=float(perform_detail::bounded(mix(r[i],wet[1]*shape,engage*amount)));
            if(!on)history.push(inputL,inputR);phase+=rate;history.frames++;wasOn=on;
        }
    }
};

class PerformTremolo final : public perform_detail::Base {
    DelayLine left,right;perform_detail::FrequencyShifter shifter[2];double phase=0,shiftPhase[2]{};
    Memo outGain,rateMemo;
public:
    explicit PerformTremolo(double sr):Base(sr),left(size_t(sr*.08)+8),right(size_t(sr*.08)+8){}
    void reset() noexcept override {resetParameters();left.reset();right.reset();for(auto&f:shifter)f.reset();phase=shiftPhase[0]=shiftPhase[1]=0;}
    double tail() const noexcept override{return .1;}
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override {
        for(uint32_t i=0;i<n;i++){
            const double position=value(1),blend=value(2),mode=value(3),attack=value(4),on=value(0,attack),stereo=value(5),depth=mix(value(6),value(7),position),minRate=value(8),maxRate=value(9),sync=value(10),beats=value(11),v12=value(12);
            const double out=outGain([&]{return dbGain(v12);},v12);
            const double rate=rateMemo([&]{return mix(minRate*std::pow(std::max(.01,maxRate)/std::max(.01,minRate),position),1/beatSeconds(beats),sync);},minRate,maxRate,position,sync,beats,tempo);
            const double input[2]={l[i],r[i]},osc[2]={fastSin2pi(phase),fastSin2pi(phase+stereo*.5)};phase=wrap(phase+rate/sr);
            left.push(input[0]);right.push(input[1]);double result[2];
            // Terms with an exactly zero weight are not evaluated (the FIR
            // history is still recorded), which leaves the result unchanged.
            const bool vibrato=on!=0&&blend!=0,shifted=vibrato&&mode!=0;
            for(int c=0;c<2;c++){
                shiftPhase[c]=wrap(shiftPhase[c]+osc[c]*depth*80/sr);
                shifter[c].push(input[c]);
                if(on==0){result[c]=input[c]*out;continue;}
                const double trem=input[c]*(1-depth*(.5+.5*osc[c]));
                double moved=0;
                if(vibrato){const double delayed=(c?right:left).read(sr*(.012+.008*depth*osc[c]));moved=shifted?mix(delayed,shifter[c].output(shiftPhase[c]),mode):delayed;}
                result[c]=mix(input[c],mix(trem,moved,blend),on)*out;
            }
            l[i]=float(perform_detail::bounded(result[0]));r[i]=float(perform_detail::bounded(result[1]));
        }
    }
};

class PerformScratcher final : public perform_detail::Base {
    perform_detail::Capture history;DelayLine left,right;perform_detail::FrequencyShifter shifter[2];Smoother delay;
    bool wasOn=false;double phase=0,lastPosition=.5,brake=1,shiftPhase=0;
    Memo outGain,brakePole;
public:
    explicit PerformScratcher(double sr):Base(sr),history(size_t(sr*4)+8),left(size_t(sr*1.01)+8),right(size_t(sr*1.01)+8){}
    void reset() noexcept override {resetParameters();history.reset();left.reset();right.reset();for(auto&f:shifter)f.reset();delay.reset();wasOn=false;phase=0;lastPosition=.5;brake=1;shiftPhase=0;}
    double tail() const noexcept override{return 120;}
    double tailForParameters(const double* v,double) const noexcept override {return v[0]>=.5?120:.25;}
    // The capture history must keep recording silence.
    bool gateable() const noexcept override {return false;}
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override {
        for(uint32_t i=0;i<n;i++){
            const bool on=p[0]>=.5;const double engage=value(0),position=value(1),milliseconds=value(2),feedback=value(3),shift=value(4),wet=value(5),brakeTime=value(6),captureTime=value(7),amount=value(8),v9=value(9);
            const double out=outGain([&]{return dbGain(v9);},v9);
            const double input[2]={l[i],r[i]},length=clamp(captureTime*sr,2,double(std::max<size_t>(2,history.valid)));
            if(on&&!wasOn){history.capture();phase=0;brake=1;lastPosition=position;}
            const double motion=(position-lastPosition)*length*2;lastPosition=position;
            brake*=brakePole([&]{return pole(brakeTime*1000*(.25+position*1.75),sr);},brakeTime,position);
            const double travel=clamp(brake+motion,-16,16);phase=wrap(phase/length)*length;
            const auto captured=history.read(length,phase);const double edge=perform_detail::seam(phase,length,std::min(sr*.002,length*.02));
            const double a=captured[0]*edge,b=captured[1]*edge;phase+=travel;
            const double frames=delay.tick(std::max(1.,milliseconds*.001*sr*(.2+.8*position)),smoothing),dl=left.read(frames),dr=right.read(frames);
            shiftPhase=wrap(shiftPhase+shift/sr);
            // Zero shift gives the shifted path zero weight: record, skip the FIR.
            const double shiftWeight=std::min(1.,std::abs(shift));double sl=0,sR=0;
            shifter[0].push(dl);shifter[1].push(dr);
            if(shiftWeight!=0){sl=shifter[0].output(shiftPhase);sR=shifter[1].output(shiftPhase);}
            left.push(a*engage+perform_detail::soft(mix(dl,sl,shiftWeight)*feedback));right.push(b*engage+perform_detail::soft(mix(dr,sR,shiftWeight)*feedback));
            l[i]=float(perform_detail::bounded(mix(input[0],a+dl*wet,engage*amount)*out));r[i]=float(perform_detail::bounded(mix(input[1],b+dr*wet,engage*amount)*out));
            if(!on)history.push(input[0],input[1]);history.frames++;wasOn=on;
        }
    }
};

inline const Descriptor* performDescriptors(size_t &count) {
    using P=STBuiltinParameterInfo;
    static const char* filterTypes[]={"Low pass","Band pass","High pass"};
    static const char* polarities[]={"Positive","Negative"};
    static const char* tones[]={"Flanger","Phaser"};
    static const char* assignments[]={"Feedback","Time"};
    static const char* timeModes[]={"Straight","Dotted","Triplet","Milliseconds"};
    static const char* echoModes[]={"Mix","Replace","Wet only"};
    static const char* resoModes[]={"Mix","Insert","Wet only"};
    static const char* ringModes[]={"Mix","Wet only"};
    static const char* stutterAssign[]={"Length","Pitch"};
    static const char* directions[]={"Forward","Reverse","Follow movement"};
    static const char* vibratoModes[]={"Varispeed","Frequency shift"};
    constexpr uint32_t log=ST_BUILTIN_PARAMETER_LOG,integer=ST_BUILTIN_PARAMETER_INTEGER,toggle=ST_BUILTIN_PARAMETER_TOGGLE;
    static const P filter[]={
        {"engage","Engage","",0,1,0,toggle,nullptr,0},{"position","Frequency position","%",0,1,.65,0,nullptr,0},{"type","Filter type","",0,2,0,integer,filterTypes,3},{"resonance","Resonance","%",0,1.05,.2,0,nullptr,0},
        {"saturation","Saturation","dB",0,30,0,0,nullptr,0},{"output","Output","dB",-36,12,0,0,nullptr,0},{"mix","Mix","%",0,1,1,0,nullptr,0},{"response","Response","ms",1,100,12,log,nullptr,0},
        {"min_frequency","Minimum frequency","Hz",20,20000,40,log,nullptr,0},{"max_frequency","Maximum frequency","Hz",20,20000,18000,log,nullptr,0}};
    static const P flanger[]={
        {"engage","Engage","",0,1,0,toggle,nullptr,0},{"frequency","Comb frequency","Hz",10,4000,220,log,nullptr,0},{"decay","Decay","s",.02,12,1.5,log,nullptr,0},{"saturation","Saturation","dB",0,24,3,0,nullptr,0},
        {"stereo","Stereo","%",0,1,.35,0,nullptr,0},{"polarity","Polarity","",0,1,0,integer,polarities,2},{"tone","Tone","",0,1,0,integer,tones,2},{"mix","Mix","%",0,1,.7,0,nullptr,0},
        {"rate","Motion rate","Hz",0,10,.25,0,nullptr,0},{"depth","Motion depth","%",0,1,.15,0,nullptr,0}};
    static const P burst[]={
        {"engage","Engage","",0,1,0,toggle,nullptr,0},{"position","Performance position","%",0,1,.6,0,nullptr,0},{"assignment","Position controls","",0,1,0,integer,assignments,2},{"time","Time","beats",1./32,4,.5,log,nullptr,0},
        {"time_mode","Time mode","",0,3,0,integer,timeModes,4},{"feedback","Feedback","%",0,.98,.6,0,nullptr,0},{"filter","Feedback filter","%",0,1,.2,0,nullptr,0},{"wet","Wet level","%",0,1,.5,0,nullptr,0},
        {"feed","Burst length (1 = continuous)","s",.01,1,.15,0,nullptr,0},{"mix_mode","Mix mode","",0,2,0,integer,echoModes,3},{"free_time","Free time","ms",1,2000,250,log,nullptr,0},{"saturation","Saturation","dB",0,18,0,0,nullptr,0},
        {"stereo","Ping-pong","%",0,1,0,0,nullptr,0},{"output","Output","dB",-24,12,0,0,nullptr,0}};
    static const P reso[]={
        {"engage","Engage","",0,1,0,toggle,nullptr,0},{"frequency","Resonator frequency","Hz",20,4000,440,log,nullptr,0},{"time","Time","beats",1./32,4,.5,log,nullptr,0},{"feedback","Feedback","%",0,.98,.65,0,nullptr,0},
        {"pump","Pump","%",0,1,.3,0,nullptr,0},{"wet","Wet level","%",0,1,.5,0,nullptr,0},{"mix_mode","Mix mode","",0,2,0,integer,resoModes,3},{"resonance","Resonance","%",0,1,.6,0,nullptr,0},
        {"saturation","Saturation","dB",0,24,6,0,nullptr,0},{"free_time","Free time","ms",1,2000,250,log,nullptr,0},{"sync","Tempo sync","",0,1,1,toggle,nullptr,0},{"spread","Stereo spread","%",0,1,.25,0,nullptr,0}};
    static const P ring[]={
        {"engage","Engage","",0,1,0,toggle,nullptr,0},{"frequency","Frequency","Hz",5,6000,220,log,nullptr,0},{"spread","Oscillator spread","%",0,1,.15,0,nullptr,0},{"glide","Glide","ms",1,2000,25,log,nullptr,0},
        {"reverb_time","Reverb time","s",.1,15,2,log,nullptr,0},{"reverb_mix","Reverb mix","%",0,1,.25,0,nullptr,0},{"mix","Mix","%",0,1,.75,0,nullptr,0},{"mix_mode","Mix mode","",0,1,0,integer,ringModes,2},{"output","Output","dB",-24,12,0,0,nullptr,0}};
    static const P stutter[]={
        {"engage","Engage","",0,1,0,toggle,nullptr,0},{"position","Performance position","%",0,1,.5,0,nullptr,0},{"assignment","Position controls","",0,1,0,integer,stutterAssign,2},{"length","Loop length","beats",1./32,4,.5,log,nullptr,0},
        {"pitch","Pitch","st",-24,24,0,0,nullptr,0},{"gate","Gate open","%",0,1,1,0,nullptr,0},{"direction","Direction","",0,2,0,integer,directions,3},{"quantize","Tempo quantize","",0,1,1,toggle,nullptr,0},
        {"free_length","Free loop length","ms",5,2000,125,log,nullptr,0},{"mix","Mix","%",0,1,1,0,nullptr,0},{"fade","Seam fade","ms",.1,20,2,log,nullptr,0}};
    static const P tremolo[]={
        {"engage","Engage","",0,1,0,toggle,nullptr,0},{"position","Performance position","%",0,1,.5,0,nullptr,0},{"blend","Tremolo / Vibrato","%",0,1,0,0,nullptr,0},{"vibrato_mode","Vibrato mode","",0,1,0,integer,vibratoModes,2},
        {"response","Attack / Release","ms",1,1000,20,log,nullptr,0},{"stereo","Stereo phase","%",0,1,0,0,nullptr,0},{"depth_min","Depth minimum","%",0,1,0,0,nullptr,0},{"depth_max","Depth maximum","%",0,1,1,0,nullptr,0},
        {"rate_min","Rate minimum","Hz",.05,30,1,log,nullptr,0},{"rate_max","Rate maximum","Hz",.05,30,12,log,nullptr,0},{"sync","Tempo sync","",0,1,0,toggle,nullptr,0},{"division","Period","beats",1./32,4,.5,log,nullptr,0},{"output","Output","dB",-24,12,0,0,nullptr,0}};
    static const P scratch[]={
        {"engage","Engage","",0,1,0,toggle,nullptr,0},{"position","Scratch position","%",0,1,.5,0,nullptr,0},{"delay","Swirl delay","ms",1,1000,125,log,nullptr,0},{"feedback","Feedback","%",0,.95,.3,0,nullptr,0},
        {"frequency_shift","Feedback shift","Hz",-2000,2000,0,0,nullptr,0},{"wet","Swirl level","%",0,1,.25,0,nullptr,0},{"brake","Brake time","s",.01,3,.25,log,nullptr,0},{"capture","Capture length","s",.05,4,1,log,nullptr,0},
        {"mix","Mix","%",0,1,1,0,nullptr,0},{"output","Output","dB",-24,12,0,0,nullptr,0}};
    static const Descriptor descriptors[]={
        {{"performfilter","Perform Filter","Perform",uint32_t(std::size(filter)),0},filter,[](double sr,uint32_t)->Processor*{return new PerformFilter(sr);}},
        {{"performflanger","Perform Flanger","Perform",uint32_t(std::size(flanger)),0},flanger,[](double sr,uint32_t)->Processor*{return new PerformFlanger(sr);}},
        {{"burstecho","Burst Echo","Perform",uint32_t(std::size(burst)),0},burst,[](double sr,uint32_t)->Processor*{return new BurstEcho(sr);}},
        {{"resoecho","Reso Echo","Perform",uint32_t(std::size(reso)),0},reso,[](double sr,uint32_t)->Processor*{return new ResoEcho(sr);}},
        {{"ring","Ring","Perform",uint32_t(std::size(ring)),0},ring,[](double sr,uint32_t)->Processor*{return new PerformRing(sr);}},
        {{"stutter","Stutter","Perform",uint32_t(std::size(stutter)),0},stutter,[](double sr,uint32_t)->Processor*{return new PerformStutter(sr);}},
        {{"tremolo","Tremolo","Perform",uint32_t(std::size(tremolo)),0},tremolo,[](double sr,uint32_t)->Processor*{return new PerformTremolo(sr);}},
        {{"scratcher","Scratcher","Perform",uint32_t(std::size(scratch)),0},scratch,[](double sr,uint32_t)->Processor*{return new PerformScratcher(sr);}}
    };
    count=std::size(descriptors);return descriptors;
}
}
#endif

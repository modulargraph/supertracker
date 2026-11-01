#pragma once
#include "BuiltinEffectsCommon.hpp"
#include <algorithm>
#include <array>
#include <cmath>
#include <cstdint>
#include <limits>
#include <vector>

// Original implementations informed by NI's public effect descriptions. These
// are deliberately not described as bit-compatible NI algorithms or presets.
namespace stfx { namespace spatial {
constexpr double pi = 3.14159265358979323846;
ST_INLINE double bounded(double x, double lo, double hi) noexcept { return std::isfinite(x) ? std::max(lo, std::min(hi, x)) : lo; }
ST_INLINE float clean(float x) noexcept { return std::isfinite(x) && std::abs(x) > 1.e-25f ? x : 0.f; }
ST_INLINE float soft(float x) noexcept { return x / (1.f + .05f * std::abs(x)); }
inline double pole(double hz, double sr) noexcept { return 1. - std::exp(-2. * pi * bounded(hz, 1., .45 * sr) / sr); }
struct Line {
    std::vector<float> data; size_t pos = 0;
    void prepare(size_t count) { data.assign(std::max<size_t>(count, 8), 0.f); pos = 0; }
    void reset() noexcept { std::fill(data.begin(), data.end(), 0.f); pos = 0; }
    ST_HOT float read(double samples) const noexcept {
        samples = bounded(samples, 1., double(data.size() - 2));
        const size_t whole = size_t(samples); const float f = float(samples - double(whole));
        return readAt(whole, f);
    }
    // Unchecked read: 1 <= whole <= size-2 (positions validated per block).
    ST_HOT float readAt(size_t whole, float f) const noexcept {
        const size_t a = pos >= whole ? pos - whole : pos + data.size() - whole;
        const size_t b = a == 0 ? data.size() - 1 : a - 1;
        return data[a] + f * (data[b] - data[a]);
    }
    ST_HOT void write(float x) noexcept { data[pos] = clean(x); if (++pos == data.size()) pos = 0; }
    // Block helpers. readBlock: out[i] is what readAt(whole, f) returns just
    // before the i-th of the next n writes; valid while whole >= n (the
    // block never reads its own writes).
    void readBlock(size_t whole, float f, size_t n, float* out) const noexcept {
        const size_t size = data.size(); size_t a = pos >= whole ? pos - whole : pos + size - whole;
        for (size_t i = 0; i < n; ++i) { const size_t b = a == 0 ? size - 1 : a - 1; out[i] = data[a] + f * (data[b] - data[a]); if (++a == size) a = 0; }
    }
    // Already-cleaned samples.
    void writeBlock(const float* x, size_t n) noexcept {
        const size_t size = data.size();
        for (size_t i = 0; i < n; ) { const size_t run = std::min(n - i, size - pos); std::copy(x + i, x + i + run, data.data() + pos); i += run; pos += run; if (pos == size) pos = 0; }
    }
    ST_HOT float allpass(float x, double samples, float gain) noexcept {
        float d = read(samples), y = d - gain * x;
        write(x + gain * y); return y;
    }
};
// Branch-free spatial::clean (nonfinite or |x| <= 1e-25 -> 0).
ST_INLINE float cleanFast(float x) noexcept { const float a = std::abs(x); return a > 1.e-25f && a <= std::numeric_limits<float>::max() ? x : 0.f; }

// Eight-line orthogonal feedback network: per-line T60 gains avoid a decay
// change with sample rate. Room, Hall and Plate have different delay families,
// diffusion and early-reflection balances; this is an algorithmic plate model.
// Render cost: control-rate values (T60 gains, allpass/predelay positions)
// are derived once per parameter change, smoothed delay times snap to their
// targets when converged, and the per-sample path has no bounds checks.
struct Network {
    double sr; std::array<Line,8> tank; std::array<Line,6> ap;
    std::array<Line,2> pre;
    std::array<double,8> times{}, target{}, gains{}, low{}, phase{}, increment{};
    std::array<double,8> feedback{};
    std::array<size_t,6> apWhole{}; std::array<float,6> apFrac{};
    double lp = .4, modulation = 0., diff = .6, predelay = 1., desiredPre = 1., smooth;
    double maxTime = 1.;
    bool frozen = false, configured = false, timesSettled = false; int mode = 0; double early = .2;
    std::array<double,8> lastArgs{}; bool haveArgs = false;
    explicit Network(double rate) : sr(rate), smooth(1.-std::exp(-1./(.025*rate))) {
        for (auto& d : tank) d.prepare(size_t(sr*.35)+8);
        for (auto& d : ap) d.prepare(size_t(sr*.035)+8);
        for (auto& d : pre) d.prepare(size_t(sr*.26)+8);
        for (size_t k=0;k<8;++k) { times[k] = target[k] = sr*.025; phase[k] = k*.719; increment[k] = (.07+.013*k)*2*pi/sr; }
        static constexpr double aps[6]={.0047,.0073,.0107,.0059,.0089,.0121};
        for (size_t j=0;j<6;++j) { const double t=bounded(aps[j]*sr,1.,double(ap[j].data.size()-2)); apWhole[j]=size_t(t); apFrac[j]=float(t-double(apWhole[j])); }
        maxTime = double(tank[0].data.size()-2);
    }
    void configure(int kind, double size, double t60, double damping, double mod, double diffusion, double preMs, bool freeze=false) noexcept {
        const std::array<double,8> args{double(kind),size,t60,damping,mod,diffusion,preMs,double(freeze)};
        if (haveArgs && args == lastArgs && configured) return;
        lastArgs = args; haveArgs = true;
        mode = kind; frozen = freeze;
        static constexpr double room[8]={.0173,.0239,.0311,.0377,.0419,.0473,.0533,.0599};
        static constexpr double hall[8]={.0297,.0371,.0411,.0539,.0671,.0713,.0797,.0893};
        static constexpr double plate[8]={.0229,.0317,.0401,.0487,.0613,.0689,.0833,.0971};
        static constexpr double meta[8]={.031,.0465,.062,.0775,.093,.1085,.124,.1395};
        const double* base=kind==1?hall:kind==2?plate:kind==3?meta:room;
        double scale=.3+bounded(size,0.,100.)*.015;
        for (size_t k=0;k<8;++k) { target[k]=sr*base[k]*scale; gains[k]=freeze?.999995:std::pow(.001, target[k]/(sr*bounded(t60,.1,30.))); feedback[k]=.3535533905932738*gains[k]; }
        lp=freeze?1.:pole(1200.+std::pow(1.-bounded(damping,0.,100.)*.01,2.)*17000.,sr);
        modulation=bounded(mod,0.,100.)*.000018*sr;
        diff=bounded(.15+diffusion*.0065,.15,.8);
        desiredPre=std::max(1.,bounded(preMs,0.,250.)*.001*sr);
        early=kind==0?.35*(1.-diffusion*.008):kind==1?.12*(1.-diffusion*.009):.04;
        if(!configured){times=target;predelay=desiredPre;configured=true;}
        timesSettled=false;
    }
    void reset() noexcept { for(auto& d:tank)d.reset();for(auto& d:ap)d.reset();for(auto& d:pre)d.reset();low.fill(0);times=target;predelay=desiredPre;configured=false;haveArgs=false;timesSettled=false;for(size_t k=0;k<8;++k)phase[k]=k*.719; }
    static double snap(double value,double goal) noexcept { return std::abs(value-goal)<=1e-9*std::abs(goal)?goal:value; }
    // Largest sub-block whose tank and allpass reads all precede its writes.
    size_t chunkLimit() const noexcept {
        double shortest=1e9;for(size_t k=0;k<8;++k)shortest=std::min(shortest,std::min(times[k],target[k]));
        shortest-=modulation+2.;size_t limit=kChunk;
        for(size_t j=0;j<6;++j)limit=std::min(limit,apWhole[j]);
        return std::max<size_t>(1,std::min<size_t>(limit,shortest>1.?size_t(shortest):1));
    }
    static constexpr size_t kChunk=64;
    // Renders n samples of wet output; identical arithmetic to a per-sample
    // loop (predelay -> 3 allpasses per side -> 8 modulated lines -> damping
    // -> Walsh-Hadamard feedback), but staged block-wise so loops vectorize.
    void process(const float* inL,const float* inR,float* outL,float* outR,size_t n) noexcept {
        for(size_t offset=0;offset<n;){
            const size_t count=std::min(n-offset,chunkLimit());
            chunk(inL+offset,inR+offset,outL+offset,outR+offset,count);offset+=count;
        }
    }
    void chunk(const float* inL,const float* inR,float* outL,float* outR,size_t n) noexcept {
        float x[2][kChunk],earlyOut[2][kChunk],buffer[kChunk];
        const float e=float(early);
        for(size_t i=0;i<n;++i){
            if(predelay!=desiredPre)predelay=snap(predelay+smooth*(desiredPre-predelay),desiredPre);
            const double pd=std::min(predelay,double(pre[0].data.size()-2));
            const size_t pw=size_t(pd);const float pf=float(pd-double(pw));
            x[0][i]=pre[0].readAt(pw,pf);x[1][i]=pre[1].readAt(pw,pf);
            pre[0].write(frozen?0:inL[i]);pre[1].write(frozen?0:inR[i]);
        }
        for(int c=0;c<2;++c)for(size_t i=0;i<n;++i)earlyOut[c][i]=x[c][i]*e;
        const float g=float(diff);
        for(int c=0;c<2;++c)for(int j=0;j<3;++j){
            Line& a=ap[c*3+j];a.readBlock(apWhole[c*3+j],apFrac[c*3+j],n,buffer);
            float* xc=x[c];
            for(size_t i=0;i<n;++i){const float y=buffer[i]-g*xc[i];buffer[i]=cleanFast(xc[i]+g*y);xc[i]=y;}
            a.writeBlock(buffer,n);
        }
        // Per-sample modulated read positions (smoothed times, triangle LFOs).
        double position[8][kChunk];
        for(size_t i=0;i<n;++i){
            if(!timesSettled){bool all=true;for(size_t k=0;k<8;++k){times[k]=snap(times[k]+smooth*(target[k]-times[k]),target[k]);all=all&&times[k]==target[k];}timesSettled=all;}
            for(size_t k=0;k<8;++k){
                double ph=phase[k]+increment[k];ph=ph>2*pi?ph-2*pi:ph;phase[k]=ph;
                // Triangle modulation keeps the inner loop free from transcendental calls.
                const double triangle=1.-2.*std::abs(ph*(1./pi)-1.);
                position[k][i]=std::min(maxTime,std::max(1.,times[k]+modulation*triangle));
            }
        }
        float d[8][kChunk];
        for(size_t k=0;k<8;++k){
            const Line& t=tank[k];const size_t size=t.data.size();const float* data=t.data.data();
            for(size_t i=0;i<n;++i){
                const size_t whole=size_t(position[k][i]);const float f=float(position[k][i]-double(whole));
                // Slot of "whole samples ago" relative to this sample's write position.
                const size_t writeAt=t.pos+i>=size?t.pos+i-size:t.pos+i;
                const size_t a=writeAt>=whole?writeAt-whole:writeAt+size-whole,b=a==0?size-1:a-1;
                d[k][i]=data[a]+f*(data[b]-data[a]);
            }
        }
        const double lpd=lp;
        // Damping one-poles, interleaved across lines so the eight recursions overlap.
        {double state[8];for(size_t k=0;k<8;++k)state[k]=low[k];
         for(size_t i=0;i<n;++i)for(size_t k=0;k<8;++k){state[k]+=lpd*(d[k][i]-state[k]);d[k][i]=float(state[k]);}
         for(size_t k=0;k<8;++k)low[k]=state[k];}
        for(size_t i=0;i<n;++i){
            outL[i]=earlyOut[0][i]+(d[0][i]+d[2][i]-d[4][i]+d[6][i])*.30f;
            outR[i]=earlyOut[1][i]+(d[1][i]+d[3][i]-d[5][i]+d[7][i])*.30f;
        }
        // Normalised Walsh-Hadamard transform has unit spectral radius.
        for(int span=1;span<8;span*=2)for(int i0=0;i0<8;i0+=span*2)for(int j=0;j<span;++j){
            float* p0=d[i0+j];float* p1=d[i0+j+span];
            for(size_t i=0;i<n;++i){const float a=p0[i],b=p1[i];p0[i]=a+b;p1[i]=a-b;}
        }
        for(size_t k=0;k<8;++k){
            const float* in=x[k&1];const double fb=feedback[k];
            for(size_t i=0;i<n;++i)buffer[i]=cleanFast(soft((frozen?0.f:in[i]*.24f)+float(d[k][i]*fb)));
            tank[k].writeBlock(buffer,n);
        }
    }
    void tick(float inL,float inR,float& outL,float& outR) noexcept { process(&inL,&inR,&outL,&outR,1); }
};
// Zero-Mix idling. With Mix at exactly 0 the output is the exact dry path;
// once that has lasted longer than the effect's tail, the wet DSP stops (so a
// short automation dip through 0 is unchanged). When Mix rises again the wet
// state restarts from silence.
struct ZeroMix {
    uint64_t frames=0;bool idle=false;
    // True: skip the wet DSP for this block. restart: reset wet state first.
    bool skip(bool zero,const Processor& fx,uint32_t n,bool& restart) noexcept {
        restart=false;
        if(!zero){restart=idle;idle=false;frames=0;return false;}
        if(idle)return true;
        frames+=n;if(double(frames)>fx.tailForParameters(fx.p.data(),fx.tempo)*fx.sr)idle=true;
        return false;
    }
    void reset() noexcept {frames=0;idle=false;}
};
struct WetEQ {
    double bass[2]{}, treble[2]{}; double lowGain=1., highGain=1., highA=.9, bassA=.03;
    double lastArgs[4]{-1,0,0,0};
    // Called every block; the gains/poles are only re-derived when an argument changes.
    void set(double sr,double lowDb,double highDb,double cutoff=20000.) noexcept {
        if(sr==lastArgs[0]&&lowDb==lastArgs[1]&&highDb==lastArgs[2]&&cutoff==lastArgs[3])return;
        lastArgs[0]=sr;lastArgs[1]=lowDb;lastArgs[2]=highDb;lastArgs[3]=cutoff;
        lowGain=std::pow(10.,lowDb/20.);highGain=std::pow(10.,highDb/20.);highA=pole(cutoff,sr);bassA=pole(300.,sr); }
    ST_HOT float tick(int ch,float x) noexcept { bass[ch]+=bassA*(x-bass[ch]);treble[ch]+=highA*(x-treble[ch]);return clean(float(treble[ch]*highGain+bass[ch]*(lowGain-highGain))); }
    void reset() noexcept {bass[0]=bass[1]=treble[0]=treble[1]=0.;}
};
struct Reverb final : Processor {
    Network net; WetEQ eq; bool legacy, plate; ZeroMix zero;
    Reverb(double rate,bool old=false,bool isPlate=false):Processor(rate),net(rate),legacy(old),plate(isPlate){}
    void reset() noexcept override {net.reset();eq.reset();zero.reset();}
    double tail() const noexcept override {return legacy?120.:40.;}
    double tailForParameters(const double* values,double) const noexcept override {
        if(legacy)return values[6]>=.5?120.:(.4+values[1]*.08)*1.9+.5;
        return (plate?values[0]:values[1])*1.9+.5;
    }
    void process(float* l,float* r,const float*,const float*,uint32_t n) noexcept override {
        double mix,pan=0,width=1;bool freeze=false;
        if(legacy){int type=int(bounded(p[0],0,3));freeze=p[6]>=.5;mix=p[7]*.01;pan=p[4]*.01;width=p[5]*.01;
            net.configure(type==3?3:0,p[1],.4+p[1]*.08,type==1?10:type==2?65:40,type==3?70:12,type==3?35:80,0,freeze);eq.set(sr,p[2],p[3]);}
        else if(plate){mix=p[5]*.01;net.configure(2,55,p[0],p[2],25,95,p[3]);eq.set(sr,p[1],0,p[4]);}
        else{mix=p[7]*.01;net.configure(int(bounded(p[0],0,2)),p[2],p[1],p[3],p[4],p[5],p[6]);eq.set(sr,p[9],0,p[8]);}
        mix=bounded(mix,0,1);const double dryL=pan>0?1-pan:1,dryR=pan<0?1+pan:1;
        bool restart;
        if(zero.skip(mix==0&&!freeze,*this,n,restart)){
            for(uint32_t i=0;i<n;++i){l[i]=clean(float(l[i]*dryL*(1-mix)));r[i]=clean(float(r[i]*dryR*(1-mix)));}
            return;
        }
        if(restart){net.reset();eq.reset();process(l,r,nullptr,nullptr,n);return;}
        float wetL[Network::kChunk],wetR[Network::kChunk];
        for(uint32_t offset=0;offset<n;offset+=Network::kChunk){
            const uint32_t count=std::min<uint32_t>(n-offset,Network::kChunk);float*bl=l+offset,*br=r+offset;
            net.process(bl,br,wetL,wetR,count);
            for(uint32_t i=0;i<count;++i){float a=bl[i],b=br[i],wl=eq.tick(0,wetL[i]),wr=eq.tick(1,wetR[i]);
                float mid=(wl+wr)*.5f,side=float((wl-wr)*.5*width);
                bl[i]=clean(float((freeze?0:a*dryL)*(1-mix)+(mid+side)*mix));br[i]=clean(float((freeze?0:b*dryR)*(1-mix)+(mid-side)*mix));}
        }
    }
};
struct Metaverb final : Processor {
    Network net; WetEQ eq; ZeroMix zero;
    explicit Metaverb(double rate):Processor(rate),net(rate){}
    void reset() noexcept override {net.reset();eq.reset();zero.reset();}
    double tail() const noexcept override {return 20.;}
    double tailForParameters(const double* values,double) const noexcept override {return (1.+values[0]*.09)*1.9+.5;}
    void process(float* l,float* r,const float*,const float*,uint32_t n) noexcept override {
        net.configure(3,p[0],1.+p[0]*.09,28,35,48,0);eq.set(sr,p[1],p[2]);
        const double mix=bounded(p[4]*.01,0,1),pan=bounded(p[3]*.01,-1,1);
        bool restart;
        if(zero.skip(mix==0,*this,n,restart)){
            for(uint32_t i=0;i<n;++i){l[i]=clean(float(l[i]*(1-std::max(0.,pan))*(1-mix)));r[i]=clean(float(r[i]*(1+std::min(0.,pan))*(1-mix)));}
            return;
        }
        if(restart){net.reset();eq.reset();process(l,r,nullptr,nullptr,n);return;}
        float wetL[Network::kChunk],wetR[Network::kChunk];
        for(uint32_t offset=0;offset<n;offset+=Network::kChunk){
            const uint32_t count=std::min<uint32_t>(n-offset,Network::kChunk);float*bl=l+offset,*br=r+offset;
            net.process(bl,br,wetL,wetR,count);
            for(uint32_t i=0;i<count;++i){float a=bl[i],b=br[i],x=eq.tick(0,wetL[i]),y=eq.tick(1,wetR[i]);
                bl[i]=clean(float(a*(1-std::max(0.,pan))*(1-mix)+x*mix));br[i]=clean(float(b*(1+std::min(0.,pan))*(1-mix)+y*mix));}
        }
    }
};
struct Ice final : Processor {
    Network net; std::array<double,12> re{},im{},co{},si{};double colorState[2]{};
    Memo bank; ZeroMix zero;
    explicit Ice(double rate):Processor(rate),net(rate){}
    void reset() noexcept override {net.reset();re.fill(0);im.fill(0);colorState[0]=colorState[1]=0;bank.reset();zero.reset();}
    double tail() const noexcept override {return 45.;}
    double tailForParameters(const double* values,double) const noexcept override {
        const double ice=bounded(values[1]*.01,0,1),size=bounded(values[2]*.01,0,1);
        // Finite decay measured against offline NI reference exports; no forced infinite sustain.
        const double resonance=.12+ice*(.22+size*1.7);
        const double space=.35+size*2.5+ice*ice*(1.2+size*5.);
        return std::log(1.e5)*resonance+space*1.9+.25;
    }
    void process(float* l,float* r,const float*,const float*,uint32_t n) noexcept override {
        const double ice=bounded(p[1]*.01,0,1),size=bounded(p[2]*.01,0,1),mix=bounded(p[3]*.01,0,1),color=bounded(p[0]*.01,0,1);
        bool restart;
        if(zero.skip(mix==0,*this,n,restart)){for(uint32_t i=0;i<n;++i){l[i]=clean(float(l[i]*(1-mix)));r[i]=clean(float(r[i]*(1-mix)));}return;}
        if(restart){net.reset();re.fill(0);im.fill(0);colorState[0]=colorState[1]=0;}
        // Color sweeps the resonant band, rather than merely low-pass filtering a fixed bank.
        // Original resonator placement/voicing calibrated on a generated break fixture.
        bank([&]{
            const double radius=std::exp(-1./(sr*(.12+ice*(.22+size*1.7))));
            static constexpr double ratios[12]={-1.,-.83,-.64,-.47,-.29,-.13,.04,.19,.38,.59,.79,1.};
            double base=65.*std::pow(130.,std::pow(color,1.1)),width=.35-.23*color;
            for(size_t k=0;k<12;++k){double w=2*pi*std::min(sr*.42,base*(1.+width*ratios[k]))/sr;co[k]=std::cos(w)*radius;si[k]=std::sin(w)*radius;}
            return 0.;},ice,size,color);
        const double damping=pole(900.+color*color*18000.,sr);
        const double bankMix=std::pow(ice,4.),bankGain=.12*(.4+.6*std::pow(color/.75,1.6))/(.75+.5*size),spaceGain=.82*(1.-.93*std::pow(ice,1.3))/(.75+.5*size);
        net.configure(1,p[2],.35+size*2.5+ice*ice*(1.2+size*5.),70.-color*60.,6,75,0);
        static constexpr double sign[12]={.22,-.22,.22,.22,-.22,.22,.22,-.22,.22,.22,-.22,.22};
        float netL[Network::kChunk],netR[Network::kChunk],wetL[Network::kChunk],wetR[Network::kChunk];
        double metallicL[Network::kChunk],metallicR[Network::kChunk];
        for(uint32_t offset=0;offset<n;offset+=Network::kChunk){
            const uint32_t count=std::min<uint32_t>(n-offset,Network::kChunk);float*bl=l+offset,*br=r+offset;
            for(uint32_t i=0;i<count;++i){
                const double a=bl[i],b=br[i];
                // Twelve complex one-pole resonators, structure of arrays.
                for(size_t k=0;k<12;++k){const double input=(k&1?b:a)*.045;const double nr=co[k]*re[k]-si[k]*im[k]+input,ni=si[k]*re[k]+co[k]*im[k];
                    // Amplitude-dependent damping bounds high-Q resonances under loud excitation.
                    const double limit=1./(1.+.0005*(nr*nr+ni*ni));re[k]=nr*limit;im[k]=ni*limit;}
                double m0=0,m1=0;for(size_t k=0;k<12;k+=2){m0+=re[k]*sign[k];m1+=re[k+1]*sign[k+1];}
                metallicL[i]=m0;metallicR[i]=m1;
                netL[i]=float(a*.55+m0*ice*.13);netR[i]=float(b*.55+m1*ice*.13);
            }
            net.process(netL,netR,wetL,wetR,count);
            for(uint32_t i=0;i<count;++i){const float a=bl[i],b=br[i];
                colorState[0]+=damping*(wetL[i]*spaceGain+metallicL[i]*bankMix*bankGain-colorState[0]);colorState[1]+=damping*(wetR[i]*spaceGain+metallicR[i]*bankMix*bankGain-colorState[1]);
                bl[i]=clean(float(a*(1-mix)+colorState[0]*mix));br[i]=clean(float(b*(1-mix)+colorState[1]*mix));}
        }
    }
};
struct Reflex final : Processor {
    std::array<Line,6> comb;std::array<Line,4> ap;std::array<double,6> low{},time{};double smoothRate;bool initialized=false;ZeroMix zero;
    explicit Reflex(double rate):Processor(rate),smoothRate(1.-std::exp(-1./(.025*rate))){for(auto&d:comb)d.prepare(size_t(sr*.11)+8);for(auto&d:ap)d.prepare(size_t(sr*.025)+8);time.fill(sr*.01);}
    void reset() noexcept override {for(auto&d:comb)d.reset();for(auto&d:ap)d.reset();low.fill(0);initialized=false;zero.reset();}
    double tail() const noexcept override {return 60.;}
    double tailForParameters(const double* values,double) const noexcept override {
        const double size=bounded(values[2]*.01,0,1),feedback=.72+size*.265;
        return .0101*(.5+size*7.)*(1.+std::ceil(std::log(1.e-5)/std::log(feedback)))+1.;
    }
    void process(float*l,float*r,const float*,const float*,uint32_t n) noexcept override {
        const double size=bounded(p[2]*.01,0,1),smooth=bounded(p[1]*.01,0,1),mix=bounded(p[3]*.01,0,1),a=pole(500.+p[0]*p[0]*1.8,sr),fb=.72+size*.265;
        bool restart;
        if(zero.skip(mix==0,*this,n,restart)){for(uint32_t i=0;i<n;++i){l[i]=clean(float(l[i]*(1-mix)));r[i]=clean(float(r[i]*(1-mix)));}return;}
        if(restart){for(auto&d:comb)d.reset();for(auto&d:ap)d.reset();low.fill(0);initialized=false;}
        static constexpr double ds[6]={.0023,.0037,.0051,.0067,.0083,.0101};
        if(!initialized){for(size_t k=0;k<6;++k)time[k]=ds[k]*(.5+size*7.)*sr;initialized=true;}
        for(uint32_t i=0;i<n;++i){float x=l[i],y=r[i],wet[2]{};
            for(size_t k=0;k<6;++k){time[k]+=smoothRate*(ds[k]*(.5+size*7.)*sr-time[k]);float v=comb[k].read(time[k]);low[k]+=a*(v-low[k]);
                comb[k].write(soft(float((k&1?y:x)*.35+low[k]*fb)));wet[k&1]+=v*.45f;}
            for(int ch=0;ch<2;++ch){float diffuse=ap[ch*2].allpass(wet[ch],(.0071+ch*.0013)*sr,.72f);diffuse=ap[ch*2+1].allpass(diffuse,(.0127+ch*.0017)*sr,.62f);wet[ch]=float(wet[ch]*(1-smooth)+diffuse*smooth);}
            l[i]=clean(float(x*(1-mix)+wet[0]*mix));r[i]=clean(float(y*(1-mix)+wet[1]*mix));}
    }
};

inline Processor* makeIce(double sr,uint32_t){return new Ice(sr);} inline Processor* makeMeta(double sr,uint32_t){return new Metaverb(sr);}
inline Processor* makeReflex(double sr,uint32_t){return new Reflex(sr);} inline Processor* makeReverb(double sr,uint32_t){return new Reverb(sr);}
inline Processor* makeLegacy(double sr,uint32_t){return new Reverb(sr,true);} inline Processor* makePlate(double sr,uint32_t){return new Reverb(sr,false,true);}
inline const char* const modes[]={"Room","Hall","Plate"};inline const char* const rooms[]={"General","Bright","Guitar","Shatter"};
inline const STBuiltinParameterInfo iceParams[]={
    {"color","Color","%",0,100,65,0,nullptr,0},{"ice","Ice","%",0,100,35,0,nullptr,0},{"size","Size","%",0,100,50,0,nullptr,0},{"mix","Mix","%",0,100,30,0,nullptr,0}};
inline const STBuiltinParameterInfo metaParams[]={
    {"size","Size","%",0,100,45,0,nullptr,0},{"low","Low","dB",-24,12,0,0,nullptr,0},{"high","High","dB",-24,12,0,0,nullptr,0},{"pan","Dry Pan","%",-100,100,0,0,nullptr,0},{"mix","Mix","%",0,100,30,0,nullptr,0}};
inline const STBuiltinParameterInfo reflexParams[]={
    {"color","Color","%",0,100,60,0,nullptr,0},{"smooth","Smooth","%",0,100,45,0,nullptr,0},{"size","Size","%",0,100,25,0,nullptr,0},{"mix","Mix","%",0,100,30,0,nullptr,0}};
inline const STBuiltinParameterInfo legacyParams[]={
    {"room","Room","",0,3,0,2,rooms,4},{"size","Size","%",0,100,40,0,nullptr,0},{"low","Low","dB",-24,12,0,0,nullptr,0},{"high","High","dB",-24,12,0,0,nullptr,0},
    {"pan","Dry Pan","%",-100,100,0,0,nullptr,0},{"stereo","Stereo","%",0,150,100,0,nullptr,0},{"freeze","Freeze","",0,1,0,4,nullptr,0},{"mix","Mix","%",0,100,30,0,nullptr,0}};
inline const STBuiltinParameterInfo reverbParams[]={
    {"mode","Mode","",0,2,0,2,modes,3},{"time","Reverb Time","s",.5,20.2,1.,1,nullptr,0},{"size","Size","%",0,100,20,0,nullptr,0},{"damping","Damping","%",0,100,50,0,nullptr,0},
    {"modulation","Modulation","%",0,100,50,0,nullptr,0},{"diffusion","Diffusion / Softness","%",0,100,80,0,nullptr,0},{"predelay","Pre Delay","ms",0,250,0,0,nullptr,0},{"mix","Mix","%",0,100,50,0,nullptr,0},
    {"highcut","High Cut","Hz",2000,20000,20000,1,nullptr,0},{"lowshelf","Low Shelf","dB",-12,0,0,0,nullptr,0}};
inline const STBuiltinParameterInfo plateParams[]={
    {"decay","Decay","s",.2,20.2,2.,1,nullptr,0},{"lowshelf","Low Shelf","dB",-24,6,0,0,nullptr,0},{"highdamp","High Damp","%",0,100,45,0,nullptr,0},{"predelay","Pre Delay","ms",0,250,0,0,nullptr,0},
    {"highcut","High Cut","Hz",2000,20000,16000,1,nullptr,0},{"mix","Mix","%",0,100,35,0,nullptr,0}};
} // namespace spatial
inline const Descriptor* spatialDescriptors(size_t& count){
    static const Descriptor all[]={
        {{"ice","Ice","Spatial",4,0},spatial::iceParams,spatial::makeIce},{{"metaverb","Metaverb","Spatial",5,0},spatial::metaParams,spatial::makeMeta},
        {{"reflex","Reflex","Spatial",4,0},spatial::reflexParams,spatial::makeReflex},{{"reverblegacy","Reverb Legacy","Reverb",8,0},spatial::legacyParams,spatial::makeLegacy},
        {{"reverb","Reverb","Reverb",10,0},spatial::reverbParams,spatial::makeReverb},{{"plate","Plate Reverb","Reverb",6,0},spatial::plateParams,spatial::makePlate}};
    count=sizeof(all)/sizeof(all[0]);return all;
}
} // namespace stfx

#ifndef ST_BUILTIN_EFFECTS_COMMON_HPP
#define ST_BUILTIN_EFFECTS_COMMON_HPP
#include "include/STBuiltinEffects.h"
#include <algorithm>
#include <array>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <limits>
#include <vector>

/* The app builds this target at -Os, where small helpers are otherwise left
   out of line; render-path helpers are forced inline so the hot loops look
   the same as at -O2. */
#define ST_INLINE inline __attribute__((always_inline))
#define ST_HOT __attribute__((always_inline))
namespace stfx {
constexpr double pi=3.1415926535897932384626433832795;
ST_INLINE double clamp(double x,double lo,double hi) noexcept { return std::max(lo,std::min(hi,x)); }
inline double dbGain(double dB) noexcept { return std::pow(10.0,dB/20.0); }
// dbGain for per-sample paths: no libm exp10 call (relative error < 3e-13; exact 1 at 0 dB).
ST_INLINE double fastDbGain(double dB) noexcept;
ST_INLINE double gainDB(double x) noexcept { return 20.0*std::log10(std::max(x,1.0e-12)); }
inline double pole(double milliseconds,double sr) noexcept { return std::exp(-1.0/(std::max(milliseconds,0.001)*0.001*sr)); }
ST_INLINE double clean(double x) noexcept { return std::isfinite(x) && std::abs(x)>1e-30 ? clamp(x,-1e6,1e6) : 0; }
ST_INLINE double mix(double dry,double wet,double amount) noexcept { return dry+(wet-dry)*amount; }
ST_INLINE double wrap(double p) noexcept { return p-std::floor(p); }

/* Branch-free, vectorizable approximations for render loops. Errors are far
   below audibility (and below float output precision), so replacing libm
   calls with these is sound-equivalent:
     fastExp     relative error < 3e-13 over [-700, 700]
     fastTanh    absolute error < 1e-12
     fastSin2pi  sin(2*pi*phase), absolute error < 1e-11 for any finite phase */
ST_INLINE double fastExp(double x) noexcept {
    x=std::max(-700.0,std::min(700.0,x));
    const double n=std::nearbyint(x*1.4426950408889634);
    const double r=(x-n*0.693147180369123816490)-n*1.90821492927058770002e-10;
    // Degree-11 Taylor polynomial, Estrin evaluation (short dependency chain).
    const double r2=r*r,r4=r2*r2,r8=r4*r4;
    const double q0=1.0+r,q1=0.5+r*(1.0/6.0),q2=1.0/24.0+r*(1.0/120.0),q3=1.0/720.0+r*(1.0/5040.0),
        q4=1.0/40320.0+r*(1.0/362880.0),q5=1.0/3628800.0+r*(1.0/39916800.0);
    const double y=(q0+q1*r2)+(q2+q3*r2)*r4+(q4+q5*r2)*r8;
    const int64_t bits=(int64_t(n)+1023)<<52;double scale;std::memcpy(&scale,&bits,sizeof scale);
    return y*scale;
}
ST_INLINE double fastTanh(double x) noexcept {
    const double e=fastExp(-2.0*std::min(std::abs(x),40.0));
    return std::copysign((1.0-e)/(1.0+e),x);
}
ST_INLINE double fastSin2pi(double phase) noexcept {
    double p=phase-std::nearbyint(phase);                 // [-0.5, 0.5]
    const double h=std::copysign(0.5,p);
    p=std::abs(p)>0.25?h-p:p;                             // [-0.25, 0.25]
    const double x=2*pi*p,x2=x*x,x4=x2*x2,x8=x4*x4;
    const double a=-1.0/6.0+x2*(1.0/120.0),b=-1.0/5040.0+x2*(1.0/362880.0),c=-1.0/39916800.0+x2*(1.0/6227020800.0);
    const double y=a+b*x4+(c-x4*(1.0/1307674368000.0))*x8;
    return x+x*x2*y;
}
ST_INLINE double fastCos2pi(double phase) noexcept { return fastSin2pi(phase+0.25); }
ST_INLINE double fastDbGain(double dB) noexcept { return fastExp(dB*0.11512925464970229); }
/* Single-precision tanh for vectorized shapers: |error| < 2e-7 (float
   resolution near 1), exactly 0 at 0 and odd-symmetric. */
ST_INLINE float fastTanhf(float x) noexcept {
    const float ax=std::fmin(std::fabs(x),9.f);
    const float y=-2.f*ax;                                  // [-18, 0]
    const float n=std::nearbyint(y*1.44269504f);
    const float r=(y-n*0.693359375f)+n*2.12194440e-4f;      // Cody-Waite ln2 split
    const float r2=r*r;
    const float p=(1.f+r)+r2*((0.5f+r*(1.f/6.f))+r2*((1.f/24.f+r*(1.f/120.f))+r2*(1.f/720.f+r*(1.f/5040.f))));
    int32_t bits=(int32_t(n)+127)<<23;float scale;std::memcpy(&scale,&bits,sizeof scale);
    const float e=p*scale;
    return std::copysign((1.f-e)/(1.f+e),x);
}
/* Exact memoization of a pure per-sample derivation (dB->gain, pole, tan...):
   while its inputs are unchanged (settled smoothing), reuse the result.
   Usage: memo([&]{ return dbGain(v); }, v). Results are bit-identical. */
struct Memo {
    double key[6]{};double value=0;bool valid=false;
    template<class F> ST_HOT double operator()(F f,double a,double b=0,double c=0,double d=0,double e=0,double g=0) noexcept {
        if(!valid||!(a==key[0])||!(b==key[1])||!(c==key[2])||!(d==key[3])||!(e==key[4])||!(g==key[5])){
            key[0]=a;key[1]=b;key[2]=c;key[3]=d;key[4]=e;key[5]=g;value=f();valid=true;}
        return value;
    }
    void reset() noexcept {valid=false;}
};
struct Processor {
    double sr,tempo=120;
    std::array<double,64> p{};
    explicit Processor(double sampleRate):sr(sampleRate) {}
    virtual ~Processor()=default;
    virtual void process(float*,float*,const float*,const float*,uint32_t) noexcept=0;
    virtual void reset() noexcept=0;
    virtual uint32_t latency() const noexcept { return 0; }
    // Immutable conservative maximum in seconds, independent of p and tempo.
    // Sustaining/freeze processors use an explicit finite export cap (120 s).
    virtual double tail() const noexcept { return 0; }
    // Pure control-side estimate from explicit physical targets. Overrides
    // must not read p or any mutable render state; sr is immutable. This lets
    // long feedback delays report their actual target decay before rendering.
    virtual double tailForParameters(const double*,double) const noexcept { return tail(); }
    // Whether the host may stop calling process() once the input has been
    // silent for longer than the tail and the output has decayed (silence
    // gating). Processors whose state must keep advancing on silence (capture
    // history) return false; sustaining modes are excluded by their 120 s cap.
    virtual bool gateable() const noexcept { return true; }
};
// Recomputes derived control targets only when the raw parameters change
// (keeps pow/exp10 out of the steady-state render path).
template<size_t N> struct ParameterCache {
    std::array<double, N> last{}; bool valid = false;
    bool changed(const Processor &processor) noexcept {
        bool diff = !valid;
        for (size_t i = 0; i < N; ++i) if (!(processor.p[i] == last[i])) { last[i] = processor.p[i]; diff = true; }
        valid = true; return diff;
    }
    void reset() noexcept { valid = false; }
};

struct Descriptor {
    STBuiltinEffectInfo info;
    const STBuiltinParameterInfo *params;
    Processor *(*create)(double,uint32_t);
};

/* DF2T biquads, double state; normalized RBJ coefficients. Coefficients may be
   set per block; zero-gain shelves/peaks use exact identity coefficients. */
struct Biquad {
    double b0=1,b1=0,b2=0,a1=0,a2=0,z1=0,z2=0;
    void reset() noexcept { z1=z2=0; }
    ST_HOT double tick(double x) noexcept { const double y=b0*x+z1;z1=clean(b1*x-a1*y+z2);z2=clean(b2*x-a2*y);return clean(y); }
    enum Type { LowPass,HighPass,BandPass,Notch,Peak,LowShelf,HighShelf };
    void set(Type type,double hz,double q,double gain,double sr) noexcept {
        if ((type==Peak || type==LowShelf || type==HighShelf) && gain==0) {b0=1;b1=b2=a1=a2=0;return;}
        const double w=2*pi*clamp(hz,5,sr*0.45)/sr,c=std::cos(w),s=std::sin(w);
        const double alpha=s/(2*clamp(q,0.1,30)),A=std::pow(10.0,gain/40.0);
        double aa0=1+alpha,aa1=-2*c,aa2=1-alpha,bb0=1,bb1=0,bb2=0;
        switch(type) {
        case LowPass:bb0=(1-c)/2;bb1=1-c;bb2=bb0;break;
        case HighPass:bb0=(1+c)/2;bb1=-(1+c);bb2=bb0;break;
        case BandPass:bb0=alpha;bb1=0;bb2=-alpha;break;
        case Notch:bb0=1;bb1=-2*c;bb2=1;break;
        case Peak:bb0=1+alpha*A;bb1=-2*c;bb2=1-alpha*A;aa0=1+alpha/A;aa2=1-alpha/A;break;
        case LowShelf: {
            const double t=2*std::sqrt(A)*alpha;
            bb0=A*((A+1)-(A-1)*c+t);bb1=2*A*((A-1)-(A+1)*c);bb2=A*((A+1)-(A-1)*c-t);
            aa0=(A+1)+(A-1)*c+t;aa1=-2*((A-1)+(A+1)*c);aa2=(A+1)+(A-1)*c-t;break; }
        case HighShelf: {
            const double t=2*std::sqrt(A)*alpha;
            bb0=A*((A+1)+(A-1)*c+t);bb1=-2*A*((A-1)+(A+1)*c);bb2=A*((A+1)+(A-1)*c-t);
            aa0=(A+1)-(A-1)*c+t;aa1=2*((A-1)-(A+1)*c);aa2=(A+1)-(A-1)*c-t;break; }
        }
        b0=bb0/aa0;b1=bb1/aa0;b2=bb2/aa0;a1=aa1/aa0;a2=aa2/aa0;
    }
};
struct DelayLine {
    std::vector<double> data;
    size_t write=0;
    explicit DelayLine(size_t capacity):data(std::max<size_t>(capacity,4),0) {}
    void reset() noexcept {std::fill(data.begin(),data.end(),0);write=0;}
    ST_HOT void push(double x) noexcept {data[write]=clean(x);if(++write==data.size())write=0;}
    ST_HOT double read(double samples) const noexcept {
        const double delay=clamp(samples,1,double(data.size()-2));
        const size_t whole=size_t(delay);const double fraction=delay-double(whole);
        // write+size-whole lies in [write+2, write+size-1] < 2*size: one conditional subtract replaces the modulo.
        size_t a=write+data.size()-whole;if(a>=data.size())a-=data.size();const size_t b=a? a-1:data.size()-1;
        return data[a]+(data[b]-data[a])*fraction;
    }
};
struct Smoother {
    double value=0;bool initialized=false;
    void reset() noexcept {value=0;initialized=false;}
    // Converged values snap to the target (within 1e-9 relative), so settled
    // controls are exactly constant and per-sample derivations can be memoized.
    ST_HOT double tick(double target,double coefficient) noexcept {
        if(!initialized){value=target;initialized=true;}
        else if(value!=target){value=target+coefficient*(value-target);if(std::abs(value-target)<=1e-12+1e-9*std::abs(target))value=target;}
        return value;
    }
};
}
#endif

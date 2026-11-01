#include "../Sources/TrackerEngine/EffectsPerform.hpp"
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <memory>
#include <set>
#include <string>
#include <vector>

namespace {
size_t checks=0;
void require(bool ok,const std::string& why) {++checks;if(!ok){std::cerr<<"FAIL: "<<why<<'\n';std::exit(1);}}
const stfx::Descriptor& descriptor(const char* id) {
    size_t count=0;const auto* list=stfx::performDescriptors(count);
    for(size_t i=0;i<count;i++)if(!std::strcmp(list[i].info.id,id))return list[i];
    std::abort();
}
struct Effect {
    const stfx::Descriptor& info;std::unique_ptr<stfx::Processor> dsp;double sr;
    explicit Effect(const char* id,double rate=8000):info(descriptor(id)),dsp(info.create(rate,256)),sr(rate){
        for(size_t i=0;i<info.info.parameterCount;i++)dsp->p[i]=info.params[i].defaultValue;
        dsp->reset();
    }
    void set(const char* id,double value){for(size_t i=0;i<info.info.parameterCount;i++)if(!std::strcmp(info.params[i].id,id)){dsp->p[i]=value;return;}std::abort();}
};
struct Audio {std::vector<float> l,r;explicit Audio(size_t n):l(n),r(n){}size_t size()const{return l.size();}};
Audio tone(size_t n,double hz,double sr=8000,double amplitude=.2){Audio a(n);for(size_t i=0;i<n;i++)a.l[i]=a.r[i]=float(amplitude*std::sin(2*stfx::pi*hz*i/sr));return a;}
void render(Effect& effect,Audio& a,size_t block=97){for(size_t offset=0;offset<a.size();offset+=block)effect.dsp->process(a.l.data()+offset,a.r.data()+offset,nullptr,nullptr,uint32_t(std::min(block,a.size()-offset)));}
double energy(const std::vector<float>& a,size_t begin=0,size_t end=0){if(!end)end=a.size();double sum=0;for(size_t i=begin;i<end;i++)sum+=double(a[i])*a[i];return sum;}
double difference(const std::vector<float>& a,const std::vector<float>& b,size_t begin=0){double sum=0;for(size_t i=begin;i<a.size();i++){const double d=a[i]-b[i];sum+=d*d;}return sum;}
double bin(const std::vector<float>& a,double hz,double sr,size_t begin=0){double re=0,im=0;for(size_t i=begin;i<a.size();i++){const double phase=2*stfx::pi*hz*i/sr;re+=a[i]*std::cos(phase);im+=a[i]*std::sin(phase);}return 2*std::hypot(re,im)/double(a.size()-begin);}
double variation(const std::vector<float>& a,size_t begin,size_t end){double sum=0;for(size_t i=begin+1;i<end;i++){const double d=a[i]-a[i-1];sum+=d*d;}return sum;}
void catalogueAndDry(){
    size_t count=0;const auto* all=stfx::performDescriptors(count);require(count==8,"eight Perform effects");
    const char* ids[]={"performfilter","performflanger","burstecho","resoecho","ring","stutter","tremolo","scratcher"};
    for(size_t i=0;i<count;i++){
        require(!std::strcmp(all[i].info.id,ids[i]),"stable effect ID order");require(all[i].info.parameterCount<=16,"at most two encoder pages");
        std::set<std::string> names;
        for(size_t p=0;p<all[i].info.parameterCount;p++){
            const auto& v=all[i].params[p];require(names.insert(v.id).second,"unique parameter IDs");require(v.defaultValue>=v.minimum&&v.defaultValue<=v.maximum,"valid default");
            require(!(v.flags&ST_BUILTIN_PARAMETER_LOG)||v.minimum>0,"positive log range");
            if(v.labelCount)require(v.labels&&v.maximum-v.minimum+1==v.labelCount,"complete enum labels");
        }
        Effect e(ids[i]);Audio input=tone(4097,247),output=input;render(e,output);require(input.l==output.l&&input.r==output.r,std::string(ids[i])+" disengaged dry");
    }
}
void filterAndFlanger(){
    const auto filtered=[](double hz,int mode){Effect e("performfilter",48000);e.set("engage",1);e.set("min_frequency",500);e.set("max_frequency",500);e.set("resonance",0);e.set("type",mode);Audio a=tone(24000,hz,48000,.03);render(e,a);return energy(a.l,12000);};
    require(filtered(100,0)>filtered(6000,0)*100,"low-pass rejects high frequencies");
    require(filtered(6000,2)>filtered(100,2)*100,"high-pass rejects low frequencies");
    const auto flange=[](int polarity,double stereo){Effect e("performflanger");e.set("engage",1);e.set("frequency",100);e.set("rate",0);e.set("depth",0);e.set("stereo",stereo);e.set("saturation",0);e.set("polarity",polarity);e.set("mix",1);Audio a(2000);a.l[0]=.25;render(e,a);return a;};
    const Audio positive=flange(0,0),negative=flange(1,0),stereo=flange(0,1);
    require(positive.l[80]>.17&&energy(positive.l,1,80)==0,"comb echo at reciprocal frequency");
    require(positive.l[160]>0&&negative.l[160]<0,"comb feedback polarity changes second repeat");
    require(energy(positive.r)==0&&energy(stereo.r)>1e-6,"stereo setting crossfeeds feedback");
}
Audio burst(double tempo,double mode,double feed,size_t impulse=0,bool release=false){
    Effect e("burstecho");e.dsp->tempo=tempo;e.set("engage",1);e.set("position",0);e.set("time",1);e.set("time_mode",mode);e.set("feed",feed);e.set("wet",1);e.set("mix_mode",2);e.set("free_time",100);
    Audio a(18000);a.l[impulse]=a.r[impulse]=.5;
    if(!release)render(e,a);else {e.dsp->process(a.l.data(),a.r.data(),nullptr,nullptr,1000);e.set("engage",0);e.dsp->process(a.l.data()+1000,a.r.data()+1000,nullptr,nullptr,uint32_t(a.size()-1000));}
    return a;
}
void burstTimingAndTrigger(){
    const Audio straight=burst(120,0,1),slow=burst(60,0,1),dotted=burst(120,1,1),triplet=burst(120,2,1),free=burst(120,3,1);
    require(straight.l[4000]>.49&&energy(straight.l,0,4000)==0,"quarter-note echo at 120 BPM");
    require(slow.l[8000]>.49&&energy(slow.l,0,8000)==0,"echo follows tempo");
    const Audio fast=burst(999,0,1);require(fast.l[480]+fast.l[481]>.49,"echo follows the full 999 BPM tracker range");
    require(dotted.l[6000]>.49,"dotted echo time");require(triplet.l[2666]+triplet.l[2667]>.49,"triplet fractional echo time");require(free.l[800]>.49,"free millisecond echo time");
    const Audio gated=burst(120,0,.05,1600),continuous=burst(120,0,1,1600),released=burst(120,0,1,0,true);
    require(energy(gated.l)<1e-30&&continuous.l[5600]>.49,"burst stops accepting late input; continuous feed does not");
    require(released.l[4000]>.49,"release retains already captured echo");
    Effect e("burstecho");e.set("engage",1);e.set("time_mode",3);e.set("free_time",50);e.set("feed",.02);e.set("position",0);e.set("mix_mode",2);e.set("wet",1);
    Audio first(1000);first.l[0]=first.r[0]=.5;render(e,first);e.set("engage",0);Audio gap(1000);render(e,gap);e.set("engage",1);Audio second(1000);second.l[80]=second.r[80]=.5;render(e,second);
    require(second.l[480]>.49,"re-engage starts a new burst window");
}
void resonanceAndRing(){
    const auto resonated=[](double frequency,double pump){Effect e("resoecho");e.set("engage",1);e.set("sync",0);e.set("free_time",20);e.set("feedback",.8);e.set("mix_mode",2);e.set("wet",1);e.set("frequency",frequency);e.set("pump",pump);e.set("saturation",0);Audio a=tone(16000,440,8000,.7);render(e,a);return a;};
    const Audio plain=resonated(440,0),pumped=resonated(440,1),detuned=resonated(1600,0);
    require(energy(plain.l,8000)>energy(pumped.l,8000)*2,"pump controls resonant feedback energy");
    require(energy(plain.l,8000)>energy(detuned.l,8000)*2,"resonator frequency selects spectral energy");
    Effect e("ring",48000);e.set("engage",1);e.set("frequency",200);e.set("spread",0);e.set("reverb_mix",0);e.set("mix",1);Audio a=tone(48000,1000,48000,.4);render(e,a);
    require(bin(a.l,800,48000)>.19&&bin(a.l,1200,48000)>.19,"ring creates sum and difference sidebands");require(bin(a.l,1000,48000)<1e-5,"fully wet ring suppresses original carrier");
    Effect plate("ring");plate.set("engage",1);plate.set("mix",1);plate.set("reverb_mix",1);plate.set("reverb_time",3);Audio excite=tone(2000,997);render(plate,excite);plate.set("engage",0);Audio tail(12000);render(plate,tail);
    require(energy(tail.l,2000)>1e-6&&energy(tail.r,2000)>1e-6,"ring plate tail survives release");
}
Audio stutter(double pitch,double gate,int direction=0,double tempo=120,bool quantize=false){
    Effect e("stutter");e.set("quantize",quantize?1:0);e.set("free_length",125);e.set("length",.25);e.set("pitch",pitch);e.set("gate",gate);e.set("direction",direction);e.set("fade",.1);e.dsp->tempo=tempo;
    Audio history=tone(16000,224);render(e,history);e.set("engage",1);Audio a(16000);render(e,a);return a;
}
void captureAndPitch(){
    const Audio normal=stutter(0,1),pitched=stutter(12,1),gated=stutter(0,.15),reverse=stutter(0,1,1);
    require(energy(normal.l,4000)>10,"stutter repeats capture with silent live input");
    double repeatError=0;for(size_t i=6000;i<14000;i++){const double d=normal.l[i]-normal.l[i+1000];repeatError+=d*d;}
    require(repeatError<1e-9,"free stutter repeats requested 125 ms loop");require(bin(pitched.l,448,8000,8000)>.15&&bin(pitched.l,224,8000,8000)<.01,"stutter octave pitch doubles playback rate");
    require(energy(gated.l,4000)<energy(normal.l,4000)*.3,"stutter gate shortens audible part of loop");require(difference(normal.l,reverse.l,4000)>20,"reverse changes captured playback direction");
    const Audio quantized=stutter(0,1,0,60,true);double slowError=0;for(size_t i=6000;i<13000;i++){const double d=quantized.l[i]-quantized.l[i+2000];slowError+=d*d;}require(slowError<1e-9,"quantized stutter length follows tempo");
    Effect nearStart("stutter");Audio shortHistory=tone(70,224);render(nearStart,shortHistory);nearStart.set("engage",1);Audio early(2000);render(nearStart,early);require(energy(early.l)==0,"quantized early capture does not expose unquantized history");
}
void tremoloAndShift(){
    Effect e("tremolo");e.set("engage",1);e.set("depth_min",1);e.set("depth_max",1);e.set("rate_min",4);e.set("rate_max",4);e.set("stereo",1);Audio a(8000);std::fill(a.l.begin(),a.l.end(),1);a.r=a.l;render(e,a);
    double minimum=1,maximum=0,maxSumError=0;for(size_t i=0;i<a.size();i++){minimum=std::min(minimum,double(a.l[i]));maximum=std::max(maximum,double(a.l[i]));maxSumError=std::max(maxSumError,std::abs(double(a.l[i])+a.r[i]-1));}
    require(minimum<1e-6&&maximum>1-1e-6,"full-depth tremolo reaches silence and unity");require(maxSumError<1e-6,"stereo tremolo gives complementary amplitude cycles");
    stfx::perform_detail::FrequencyShifter up,down;Audio shifted(8000);double phase=0;for(size_t i=0;i<shifted.size();i++){phase=stfx::wrap(phase+500./8000);const double input=.5*std::sin(2*stfx::pi*1000*i/8000);shifted.l[i]=float(up.tick(input,phase));shifted.r[i]=float(down.tick(input,-phase));}
    require(bin(shifted.l,1500,8000,4000)>.49&&bin(shifted.l,500,8000,4000)<.002,"positive frequency shift selects upper sideband");require(bin(shifted.r,500,8000,4000)>.49&&bin(shifted.r,1500,8000,4000)<.002,"negative frequency shift selects lower sideband");
    Effect vibrato("tremolo");vibrato.set("engage",1);vibrato.set("blend",1);Audio input=tone(16000,1000),varispeed=input;render(vibrato,varispeed);require(difference(input.l,varispeed.l,4000)>10,"varispeed vibrato changes pitch over time");
}
void scratchBrake(){
    Effect e("scratcher");e.set("wet",0);e.set("brake",.02);Audio history=tone(16000,224);render(e,history);e.set("engage",1);Audio a(8000);render(e,a);
    require(variation(a.l,0,1000)>1e-4,"scratch capture initially moves");require(variation(a.l,6000,8000)<1e-12,"brake brings captured playback to rest");
    e.set("position",.8);Audio moved(2000);render(e,moved);require(variation(moved.l,0,1500)>1e-3,"moving position scratches the braked buffer");
    e.set("engage",0);Audio live=tone(4000,300),released=live;render(e,released);require(difference(live.l,released.l,3000)<1e-12,"releasing scratch returns live input");
}
void tails(){
    Effect burstEffect("burstecho");burstEffect.set("engage",1);burstEffect.set("position",1);burstEffect.set("time",4);burstEffect.set("time_mode",1);burstEffect.set("saturation",0);
    auto targets=burstEffect.dsp->p;
    const double longTail=burstEffect.dsp->tailForParameters(targets.data(),20);
    require(longTail>12000&&longTail<13000,"long finite burst feedback is not truncated to a two-minute tail");
    require(burstEffect.dsp->tailForParameters(targets.data(),120)<longTail*.18,"finite tail follows current tempo without rendering");
    burstEffect.dsp->p.fill(0);burstEffect.dsp->tempo=999;
    require(burstEffect.dsp->tailForParameters(targets.data(),20)==longTail,"tail reads supplied targets rather than mutable DSP parameters");
    targets[1]=0;require(std::abs(burstEffect.dsp->tailForParameters(targets.data(),20)-18.25)<1e-9,"feedback-free long burst includes its first repeat");
    targets[1]=1;targets[11]=18;require(burstEffect.dsp->tailForParameters(targets.data(),20)==120,"driven sustaining burst uses explicit export cap");
    Effect filter("performfilter");filter.set("engage",1);filter.set("resonance",1.05);
    require(filter.dsp->tailForParameters(filter.dsp->p.data(),120)==120,"negative damping filter declares sustaining export cap");
    filter.set("resonance",0);filter.set("min_frequency",20);filter.set("max_frequency",20);
    require(filter.dsp->tailForParameters(filter.dsp->p.data(),120)>.1,"stable filter reports ringing and control release");
    filter.set("resonance",(2-1e-6)/2.08);require(filter.dsp->tailForParameters(filter.dsp->p.data(),120)>200000,"near-critical positive damping keeps its finite decay estimate");
    Effect flange("performflanger");flange.set("engage",1);flange.set("frequency",100);flange.set("decay",12);flange.set("saturation",0);
    const double cleanTail=flange.dsp->tailForParameters(flange.dsp->p.data(),120);require(cleanTail>20&&cleanTail<40,"contractive flanger tail reflects feedback decay");
    flange.set("saturation",24);require(flange.dsp->tailForParameters(flange.dsp->p.data(),120)==120,"driven flanger declares possible sustain");
    Effect reso("resoecho");reso.set("feedback",.98);reso.set("resonance",1);reso.set("saturation",24);
    require(reso.dsp->tailForParameters(reso.dsp->p.data(),120)==120,"resonator feedback declares possible sustain");
    for(const char* id:{"stutter","scratcher"}){
        Effect e(id);e.set("engage",1);require(e.dsp->tailForParameters(e.dsp->p.data(),120)==120,std::string(id)+" capture sustains after live input ends");
        e.set("engage",0);require(e.dsp->tailForParameters(e.dsp->p.data(),120)>.1&&e.dsp->tailForParameters(e.dsp->p.data(),120)<1,std::string(id)+" release includes smoothing without excessive tail");
    }
    Effect tremolo("tremolo");require(tremolo.dsp->tailForParameters(tremolo.dsp->p.data(),120)>=.02,"vibrato delay and FIR history are retained");
    Effect ring("ring");ring.set("reverb_time",15);require(ring.dsp->tailForParameters(ring.dsp->p.data(),120)>=25,"ring plate reports beyond T100 at longest decay");
    // Render a finite long-feedback case far past the reported bound, verifying
    // that it represents decay rather than just a monotonic metadata formula.
    Effect finite("burstecho");finite.set("engage",1);finite.set("position",1);finite.set("time_mode",3);finite.set("free_time",20);finite.set("mix_mode",2);finite.set("wet",1);
    const double end=finite.dsp->tailForParameters(finite.dsp->p.data(),120);Audio impulse(size_t((end+.2)*finite.sr));impulse.l[0]=impulse.r[0]=.5;render(finite,impulse);
    require(energy(impulse.l,size_t(end*finite.sr))<1e-10,"finite burst has decayed by reported tail");
}
void extremes(){
    for(double sr:{8000.,44100.,192000.,384000.})for(const char* id:{"performfilter","performflanger","burstecho","resoecho","ring","stutter","tremolo","scratcher"}){
        Effect e(id,sr);Audio a(257);
        for(int block=0;block<24;block++){
            e.dsp->tempo=block%2?20:999;
            for(size_t p=0;p<e.info.info.parameterCount;p++)e.dsp->p[p]=block%3?e.info.params[p].maximum:e.info.params[p].minimum;
            for(size_t i=0;i<a.size();i++){a.l[i]=float(4*std::sin((i+block*a.size())*.13));a.r[i]=-a.l[i];}render(e,a);
            for(size_t i=0;i<a.size();i++)require(std::isfinite(a.l[i])&&std::isfinite(a.r[i])&&std::abs(a.l[i])<=16&&std::abs(a.r[i])<=16,std::string(id)+" bounded at extreme sample rate and automation");
        }
        e.dsp->reset();e.set("engage",0);Audio silent(257);render(e,silent);require(energy(silent.l)==0&&energy(silent.r)==0,std::string(id)+" reset clears all captured/tail state");
    }
}
}
int main(){catalogueAndDry();filterAndFlanger();burstTimingAndTrigger();resonanceAndRing();captureAndPitch();tremoloAndShift();scratchBrake();tails();extremes();std::cout<<"Perform DSP tests passed ("<<checks<<" checks)\n";}

#include <cassert>
#include "TrackerEngine.h"
#include <algorithm>
#include <array>
#include <atomic>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <memory>
#include <stdexcept>
#include <vector>

namespace {
constexpr double pi=3.1415926535897932384626433832795;
constexpr int maxVoices=256, maxEvents=65536, maxCommands=2048, maxScheduled=8192, maxBlock=4096, cacheCapacity=65536, clockCapacity=ST_ENGINE_CLOCK_HISTORY_FRAMES;
template<class T> T clamp(T x,T a,T b) { return std::max(a,std::min(b,x)); }
float finite(float x,float fallback=0) { return std::isfinite(x)?x:fallback; }
// One-argument calls on a double pass the fallback explicitly: glibc's <math.h> declares ::finite(double), which would make them ambiguous.
double finite(double x,double fallback=0) { return std::isfinite(x)?x:fallback; }
float midiFrequency(double n) { return float(440.0*std::exp2((n-69.0)/12.0)); }
struct Sample { std::atomic<int> refs{0}; std::vector<float> l,r; double rate=48000; };
struct Zone {
    Sample *sample=nullptr;int keyLow=0,keyHigh=127,velocityLow=0,velocityHigh=127,rootNote=60,loopEnabled=-1;float tune=0,volume=1,pan=0,start=0,end=1,loopStart=0,loopEnd=1;bool hasRegion=false;
    Zone()=default;
    Zone(const Zone&o):sample(o.sample),keyLow(o.keyLow),keyHigh(o.keyHigh),velocityLow(o.velocityLow),velocityHigh(o.velocityHigh),rootNote(o.rootNote),loopEnabled(o.loopEnabled),tune(o.tune),volume(o.volume),pan(o.pan),start(o.start),end(o.end),loopStart(o.loopStart),loopEnd(o.loopEnd),hasRegion(o.hasRegion){if(sample)sample->refs.fetch_add(1,std::memory_order_relaxed);}
    Zone &operator=(const Zone&o){if(this!=&o){if(o.sample)o.sample->refs.fetch_add(1,std::memory_order_relaxed);if(sample)sample->refs.fetch_sub(1,std::memory_order_release);sample=o.sample;keyLow=o.keyLow;keyHigh=o.keyHigh;velocityLow=o.velocityLow;velocityHigh=o.velocityHigh;rootNote=o.rootNote;loopEnabled=o.loopEnabled;tune=o.tune;volume=o.volume;pan=o.pan;start=o.start;end=o.end;loopStart=o.loopStart;loopEnd=o.loopEnd;hasRegion=o.hasRegion;}return *this;}
    ~Zone(){if(sample)sample->refs.fetch_sub(1,std::memory_order_release);}
};
// Legacy seeks retain fractional frames and support BPM through999. Derive
// boundaries relative to that exact origin, including negative startup time.
void fixedClockContext(STClockContext&c,double frame,double rate,double bpm,int numerator,int denominator){
    c={};c.sampleRate=rate;c.songFrame=int64_t(frame);c.bpm=bpm;c.numerator=numerator;c.denominator=denominator;c.beat=frame*bpm/(60*rate);
    double width=numerator*4.0/denominator;int64_t bar=int64_t(std::floor(c.beat/width));
    auto offset=[&](double beat){return int64_t(std::ceil(beat*(60*rate)/bpm-frame));};
    for(int n=0;n<2;++n){if(offset(double(bar)*width)>0)--bar;else if(offset(double(bar+1)*width)<=0)++bar;else break;}
    c.bar=bar+1;c.barStartBeat=double(bar)*width;c.barEndBeat=double(bar+1)*width;c.beat=std::max(c.beat,c.barStartBeat);if(c.beat>=c.barEndBeat)c.beat=std::nextafter(c.barEndBeat,-std::numeric_limits<double>::infinity());
    c.nextBoundaryFrame=c.songFrame+offset(c.barEndBeat);c.nextBoundaryKinds=ST_CLOCK_BOUNDARY_BAR;
    double quarter=std::floor(c.beat);int64_t next=offset(quarter);if(next<0)next=offset(quarter+1);c.nextQuarterBeatFrame=c.songFrame+next;c.nextQuarterGridFrame=c.nextQuarterBeatFrame;
}
struct Instrument {
    STInstrument p=st_default_instrument(); Sample *sample=nullptr;std::vector<float>slices;std::vector<Zone>zones;
    int cycleFrames=0; /* Wavetable frame size detected in the sample; 0 unknown. */
    Instrument()=default;
    Instrument(const Instrument &o):p(o.p),sample(o.sample),slices(o.slices),zones(o.zones),cycleFrames(o.cycleFrames) { if(sample) sample->refs.fetch_add(1,std::memory_order_relaxed); }
    Instrument &operator=(const Instrument &o) { if(this==&o)return *this; if(o.sample)o.sample->refs.fetch_add(1,std::memory_order_relaxed); if(sample)sample->refs.fetch_sub(1,std::memory_order_release); p=o.p;sample=o.sample;slices=o.slices;zones=o.zones;cycleFrames=o.cycleFrames;return *this; }
    ~Instrument(){if(sample)sample->refs.fetch_sub(1,std::memory_order_release);}
    void setSample(Sample*s){if(s)s->refs.fetch_add(1,std::memory_order_relaxed);if(sample)sample->refs.fetch_sub(1,std::memory_order_release);sample=s;cycleFrames=0;}
};
struct PluginStepFX { bool present=false;int slot=-1,destination=-1;uint64_t address=0;double value=0; };
struct Step { std::array<PluginStepFX,2> pluginFX{};bool present=false; STStep p=st_default_step(); std::array<int32_t,ST_MAX_CHORD> chord{};int count=1; };
struct Point { double row,value; };
struct Lane { int track,target,slot,interpolation;uint64_t address;std::vector<Point> points; };
std::atomic<uint64_t> patternEpochs{0};
const Step blankStep{};const std::vector<Lane> noLanes;
/* Copy-on-write pattern storage. Each (pattern,track) column and each automation lane
   list is shared between the draft and every published Project until it is edited, so
   a commit copies pointers, not steps. Shared pointers are only copied or released on
   the control thread (drafts, commits and collect()); the render thread only reads
   through the published Project, so use_count()==1 means "owned by this draft alone". */
struct Column { std::vector<Step> rows; };
/* Note columns: column 0 of a track is `steps[track]`; extra note columns 1..ST_MAX_NOTE_COLUMNS-1 live in
   `extra[track*(ST_MAX_NOTE_COLUMNS-1)+column-1]`, copy-on-write per (pattern,track,column) like column 0.
   Each column is its own sequencer lane on the track's channel: lane id track+column*ST_TRACK_SANITY_LIMIT. */
constexpr int extraColumns=ST_MAX_NOTE_COLUMNS-1;
inline int laneTrack(int lane){return lane<0?lane:lane%ST_TRACK_SANITY_LIMIT;}
struct Pattern {
    int length=64;std::vector<std::shared_ptr<Column>> steps,extra;std::shared_ptr<std::vector<Lane>> lanes,hostLanes;bool lanesDirty=false;
    static const Step&at(const std::vector<std::shared_ptr<Column>>&list,size_t index,int row){if(index>=list.size()||!list[index])return blankStep;const auto&rows=list[index]->rows;return size_t(row)<rows.size()?rows[size_t(row)]:blankStep;}
    const Step&step(int lane,int row)const{if(lane<0)return blankStep;return at(steps,size_t(lane),row);}
    // column 1..ST_MAX_NOTE_COLUMNS-1; any other column reads as empty.
    const Step&columnStep(int track,int column,int row)const{if(track<0||column<1||column>extraColumns)return blankStep;return at(extra,size_t(track)*extraColumns+size_t(column-1),row);}
    bool hasExtraColumns(int track)const{const size_t first=size_t(track)*extraColumns;if(track<0||first>=extra.size())return false;for(size_t n=first;n<first+extraColumns&&n<extra.size();++n)if(extra[n])return true;return false;}
    bool hasColumn(int lane)const{return lane>=0&&size_t(lane)<steps.size()&&steps[size_t(lane)];}
    const std::vector<Lane>&laneList()const{return lanes?*lanes:noLanes;}
    const std::vector<Lane>&hostLaneList()const{return hostLanes?*hostLanes:noLanes;}
    static Column&own(std::vector<std::shared_ptr<Column>>&list,size_t index){if(index>=list.size())list.resize(index+1);auto&column=list[index];if(!column)column=std::make_shared<Column>();else if(column.use_count()>1)column=std::make_shared<Column>(*column);return *column;}
    Column&mutableColumn(int lane){return own(steps,size_t(lane));}
    Step&rowOf(Column&column,int row){if(column.rows.size()<=size_t(row))column.rows.resize(size_t(length));return column.rows[size_t(row)];}
    Step&mutableStep(int lane,int row){return rowOf(mutableColumn(lane),row);}
    Step&mutableColumnStep(int track,int column,int row){return rowOf(own(extra,size_t(track)*extraColumns+size_t(column-1)),row);}
    std::vector<Lane>&mutableLanes(bool host){auto&list=host?hostLanes:lanes;if(!list)list=std::make_shared<std::vector<Lane>>();else if(list.use_count()>1)list=std::make_shared<std::vector<Lane>>(*list);lanesDirty=true;return *list;}
    void resize(int n){if(n<length){for(size_t lane=0;lane<steps.size();++lane)if(steps[lane]&&steps[lane]->rows.size()>size_t(n))mutableColumn(int(lane)).rows.resize(size_t(n));for(size_t index=0;index<extra.size();++index)if(extra[index]&&extra[index]->rows.size()>size_t(n))own(extra,index).rows.resize(size_t(n));}length=n;}
};
struct EngineTiming {
    // Control owns map storage; the audio origin pins without shared_ptr deletion.
    mutable std::atomic<unsigned> originPins{0};
    std::shared_ptr<STClockMap> map;
    std::shared_ptr<STClockTimeline> timeline;
    std::vector<STClockTempoEvent> tempos;
    std::vector<STClockMeterEvent> meters;
    uint64_t revision;int64_t maximumFrame=0;
    EngineTiming(double rate,const STClockTempoEvent*t,uint32_t nt,const STClockMeterEvent*m,uint32_t nm,uint64_t version):revision(version){
        char error[256];auto*raw=st_clock_map_create(rate,t,nt,m,nm,version,error,sizeof error);
        if(!raw)throw std::invalid_argument(error);
        map={raw,st_clock_map_destroy};
        STClockOrigin origin{};origin.playing=1;
        auto*clock=st_clock_timeline_create(raw,origin,error,sizeof error);
        if(!clock)throw std::invalid_argument(error);
        timeline={clock,st_clock_timeline_destroy};tempos.assign(t,t+nt);meters.assign(m,m+nm);
        maximumFrame=frame(1000000); // Immutable map endpoint; do not search it for every sample.
    }
    int64_t frame(double beat)const{int64_t result=0;st_clock_frame_at_beat(map.get(),clamp(beat,0.0,1000000.0),&result);return result;}
    double beat(int64_t frame)const{double result=0;st_clock_beat_at_frame(map.get(),frame,&result);return result;}
    bool matches(const EngineTiming&o)const{
        if(revision!=o.revision||tempos.size()!=o.tempos.size()||meters.size()!=o.meters.size())return false;
        for(size_t n=0;n<tempos.size();++n)if(tempos[n].beat!=o.tempos[n].beat||tempos[n].bpm!=o.tempos[n].bpm)return false;
        for(size_t n=0;n<meters.size();++n)if(meters[n].beat!=o.meters[n].beat||meters[n].numerator!=o.meters[n].numerator||meters[n].denominator!=o.meters[n].denominator)return false;
        return true;
    }
};
struct AudioClip {int asset=0,track=0;double startBeat=0,durationBeats=0,sourceOffset=0;float gain=1,fadeIn=.005f,fadeOut=.005f;bool muted=false;int64_t mappedStart=0,mappedEnd=0;};
/* Pattern-mode clip (Track Freeze print of one pattern); see st_engine_add_pattern_clip. */
struct PatternClip {int headAsset=0,loopAsset=-1,track=0,pattern=0;float gain=1;};
/* Song-loop tail (Track Freeze print beyond its musical end); see st_engine_add_song_tail_clip. */
struct SongTailClip {int asset=0,track=0;int64_t start=0;float gain=1;};
struct FreezeHistory {
    std::vector<STFreezeContext> frames;
    FreezeHistory():frames(clockCapacity){for(auto&f:frames)f.contentGraphFrame=INT64_MIN;}
};
struct ChannelTables;
struct Project {
    bool frozenSongPolicy=false;uint64_t frozenPublicationID=0;int64_t frozenSongFrames=0;
    std::shared_ptr<const EngineTiming> timing;double patternOriginBeat=0;std::vector<double> occurrenceBeats;std::vector<int64_t> occurrenceFrames;int64_t mappedPatternStart=0,mappedPatternEnd=0;
    double bpm=120;int lpb=4,trackCount=8,activePattern=0;float swing=0,master=.7f;
    float delayTime=.375f,delayFeedback=.35f,delayMix=.3f,reverbSize=.7f,reverbDamping=.4f,reverbMix=.25f;
    /* Send returns: tempo-synced delay (quarter beats, 0 = free time), feedback-path tone
       low-pass (coefficient 1 = open) and reverb pre-delay. Song section loop: inclusive
       sequence indices, active only while loopSong is on; -1 disables. */
    double delayBeats=0;float delayToneCoef=1.f,reverbPredelay=0.f;int songLoopStart=-1,songLoopEnd=-1;
    bool loopSong=false,metronome=false,hostMixing=false;int numerator=4,denominator=4;float metronomeVolume=.3f;
    /* Computational bitset: bit t of computationalWords is track t; tracks beyond it use computationalBeyond. */
    std::vector<uint64_t> computationalWords;bool computationalBeyond=true;
    /* tracks holds at least returnBus() entries after commit; channels below the return
       bus but at or above trackCount are disabled. */
    /* Per-track voice settings (st_engine_set_track_voice); tracks beyond the vector use the defaults. */
    struct TrackVoice { int choke=0,scope=0,mode=-1; };
    std::vector<TrackVoice> trackVoices;
    std::vector<STTrack> tracks;bool anySolo=false;std::vector<Instrument> instruments;
    std::array<Zone,256>audioAssets;std::vector<AudioClip>audioClips;std::vector<PatternClip>patternClips;std::vector<SongTailClip>songTailClips;std::vector<Pattern> patterns;std::vector<int32_t> sequence{0};Project *retiredNext=nullptr;
    std::shared_ptr<ChannelTables> tables; /* Render-owned per-channel state, sized for this project at commit. */
    /* Identifies the published pattern content: 0 in a draft whose patterns were edited;
       commit then assigns a process-unique value. Unedited commits keep it. */
    uint64_t patternEpoch=0;
    /* Host destination numbering of hostLanes and host locks (st_engine_set_host_automation_map). */
    uint64_t hostMap=0;
    Project():tracks(ST_MIN_RETURN_BUS,st_default_track()),instruments(size_t(st_instrument_capacity(64))){patterns.resize(1);}
    /* Valid index into instruments (render thread: a stale step or mix may name a removed one). */
    bool hasInstrument(int i)const{return i>=0&&size_t(i)<instruments.size();}
    int returnBus()const{return st_return_bus(trackCount);}
    int channels()const{return st_channel_count(trackCount);}
    bool trackComputational(int track)const{size_t word=size_t(track)/64;return word<computationalWords.size()?(computationalWords[word]>>(track%64)&1)!=0:computationalBeyond;}
};
struct Command { int type=0,a=0,b=0,c=0,d=0;float value=0;STStep step{};std::array<int32_t,ST_MAX_CHORD>chord{};int chordCount=1;uint64_t requestID=0;int inputEdge=0,applyInputNote=0; };
template<class T,size_t N> struct Ring {
    std::array<T,N> data{};std::atomic<uint32_t> read{0},write{0};
    uint32_t available()const{auto r=read.load(std::memory_order_acquire),w=write.load(std::memory_order_relaxed);return uint32_t((r+N-w-1)%N);}
    bool push(const T &v){auto w=write.load(std::memory_order_relaxed);uint32_t n=uint32_t((w+1)%N);if(n==read.load(std::memory_order_acquire))return false;data[w]=v;write.store(n,std::memory_order_release);return true;}
    bool pop(T &v){auto r=read.load(std::memory_order_relaxed);if(r==write.load(std::memory_order_acquire))return false;v=data[r];read.store((r+1)%N,std::memory_order_release);return true;}
};
// Sticky ratchet: the held note plus its resolved playback values (direction, offset, slice, glide, arpeggio), so repeats on later rows replay it exactly.
struct RatchetState { bool known=false,handled=false; int rate=1,instrument=0,count=1,reverse=-1,arp=0,slice=-1; float offset=0,glide=0; STStep step{}; std::array<int32_t,ST_MAX_CHORD> chord{}; };
struct TrackState { float volume=.8f,pan=0,cutoff=-1,resonance=-1,delay=0,reverb=0,pitch=0;std::array<float,ST_TARGET_LAST_INSTRUMENT-ST_TARGET_ATTACK+1>instrument{};uint64_t rowLocks=0;uint64_t instrumentMask=0;TrackState(){instrument.fill(std::numeric_limits<float>::quiet_NaN());} };
struct Voice {
    /* Wavetable mode: frame width in samples and the smoothed table position. */
    size_t cycleWidth=ST_WAVETABLE_DEFAULT_FRAME;double positionSmooth=0;
    bool active=false,releasing=false,external=false,offSent=false,sequenced=false,preview=false,explicitSlice=false;int track=0,sequenceTrack=-1,instrument=0,note=60,age=0,envStage=0,dir=1,arp=0;
    /* Choke group and its scope when the voice started (see st_engine_set_track_voice). */
    int chokeGroup=0,chokeScope=-1;
    // Cache only exact input matches; automation/modulation still evaluates each sample.
    double frequencyPitch=std::numeric_limits<double>::quiet_NaN(),ratePitch=std::numeric_limits<double>::quiet_NaN(),sampleIncrement=0;float frequency=0,coefficientCutoff=-1,coefficientResonance=-1,filterK=0,filterA1=0,filterA2=0,filterA3=0,driveAmount=-1,driveGain=1,driveScale=1;
    // Region interpolation bounds change only with sample geometry.
    size_t sampleFirst=0,sampleLast=0;
    // A changed playhead/direction can switch loop crossfade or end-fade branches.
    // Bridge that discontinuity from the last emitted sample without moving either
    // playhead or envelope. Each voice owns its history, including overlapping tails.
    bool sampleHistory=false,sampleTransition=false;int sampleTransitionLeft=0,sampleTransitionFrames=1;
    float sampleLastL=0,sampleLastR=0,sampleCorrectionL=0,sampleCorrectionR=0;
    bool looped=false,declickIn=false,zoneRegion=false;STInstrument p{};Sample *sample=nullptr;double phase=0,pos=0,begin=0,end=0,loopBegin=0,loopEnd=0,grainPhase=0;
    double mappedGateEnd=-1,gate=-1,onsetFrame=0,elapsed=0,glide=0,lfoPhase=0,lfo2Phase=0,lifetime=0,filterElapsed=0;int filterEnvStage=0;float filterEnv=0,filterReleaseStart=0,lfoRandom=0,lfo2Random=0;uint32_t lfoRng=1,lfo2Rng=1;float velocity=1,env=0,releaseStart=0,zoneTune=0,zoneVolume=1,zonePan=0,zoneStart=0,zoneEnd=1,volumeSmooth=.7f,panSmooth=0,cutoffSmooth=18000;int zoneRoot=-1;float filterL1=0,filterL2=0,filterR1=0,filterR2=0;uint32_t rng=1;
    /* Equal-power pan law for the last pan value (recomputed only when it changes). */
    float panLawPan=std::numeric_limits<float>::quiet_NaN(),panLawL=1,panLawR=1;
    /* Granular mode: one slot per overlapping grain (window phase, random offset/pitch/direction/pan drawn when
       the grain starts, last read position for the playhead snapshot), the scan offset and the grain span. */
    struct Grain { double phase=2,offset=0,read=0;float ratio=1,gainL=1,gainR=1,window=0;int dir=1; };
    std::array<Grain,ST_GRANULAR_MAX_GRAINS> grains{};int grainCount=0;double grainScan=0,grainCenter=0,grainSpan=0;
};
struct LaneEventState {double value=0;bool known=false,delivered=false;uint64_t retryFrame=0;};
struct PluginRowLock { PluginStepFX fx{};int track=0;LaneEventState delivery{}; };
/* calloc-backed float storage: large blocks come from fresh zero pages, so buses and
   cache history that are never written never become resident memory. */
struct FloatBlock {
    float*data=nullptr;size_t count=0;
    explicit FloatBlock(size_t n=0):count(n){if(n){data=static_cast<float*>(std::calloc(n,sizeof(float)));if(!data)throw std::bad_alloc();}}
    FloatBlock(const FloatBlock&)=delete;FloatBlock&operator=(const FloatBlock&)=delete;
    ~FloatBlock(){std::free(data);}
};
/* Retained render_bus history for one logical bus (a track or the return).
   Identity follows the logical bus, not its number, so history survives renumbering.
   silentFrom: graph frames >= it read as zeros (the ring is not written while silent).
   generation: the cache epoch the ring belongs to; any other epoch reads as silence. */
struct BusRing {
    FloatBlock l,r;uint64_t silentFrom=0,generation=0;
    BusRing():l(size_t(cacheCapacity)),r(size_t(cacheCapacity)){}
};
/* Peak meters are read from any thread. Storage only grows and old stores live until
   the engine is destroyed, so a reader holding an old pointer stays valid. */
struct PeakStore { size_t count;std::unique_ptr<std::atomic<float>[]> values;explicit PeakStore(size_t n):count(n),values(new std::atomic<float>[n]){for(size_t i=0;i<n;++i)values[i].store(0,std::memory_order_relaxed);} };
struct Scheduled { bool active=false;double due=0,gate=0;int track=0,instrument=0,note=60,velocity=100;float offset=0,glide=0;int reverse=-1,arp=0,slice=-1;bool mapped=false;double onsetBeat=0,endBeat=0;int sequenceTrack=-1;bool open=false; };
/* Every per-channel table the render thread uses, sized for one published project.
   Built (allocated) on the control thread at commit, reused while the shape is the
   same, and migrated by the render thread when a project with a new shape is swapped
   in. The render thread never allocates or frees these. */
/* Per-bus block buffers are written one sample at a time across every touched bus. A
   power-of-two stride (4096 floats = 16 KB) would put all buses' current sample in the
   same L1 cache set and thrash it; the padding staggers them across sets. */
static constexpr size_t busStride=maxBlock+48;
struct ChannelTables {
    int trackCount=0,base=0,channels=0,buses=0;
    std::vector<TrackState> trackState;std::vector<float> pitchBend,modWheel,left,right,peaks;std::vector<uint64_t> touched;
    /* Raw host mixing: whether each channel's bus carries its signal this block (enabled and computational). */
    std::vector<uint8_t> hostAudible;
    std::vector<RatchetState> laneRatchets;
    std::vector<PluginRowLock> pluginRowLocks;
    FloatBlock busL,busR;std::vector<uint8_t> busTouched;std::vector<std::shared_ptr<BusRing>> rings;
    ChannelTables(int tracks):trackCount(tracks),base(st_return_bus(tracks)),channels(st_channel_count(tracks)),buses(st_output_bus_count(tracks)),
        trackState(size_t(channels)),pitchBend(size_t(channels)),modWheel(size_t(channels)),left(size_t(channels)),right(size_t(channels)),peaks(size_t(channels)),touched(size_t(channels+63)/64),hostAudible(size_t(channels)),
        laneRatchets(size_t(tracks)),pluginRowLocks(size_t(tracks)*2),busL(size_t(buses)*busStride),busR(size_t(buses)*busStride),busTouched(size_t(buses)),rings(size_t(buses)){}
    bool sameShape(int tracks)const{return tracks==trackCount;}
    /* Sticky ratchet memory for a lane (its own channel). */
    RatchetState&ratchet(int lane){return laneRatchets[size_t(lane)];}
    float*busLeft(int bus){return busL.data+size_t(bus)*busStride;}
    float*busRight(int bus){return busR.data+size_t(bus)*busStride;}
};
struct Comb { std::vector<float> data;size_t position=0;float filter=0; float process(float x,float fb,float damp){float y=data[position];filter=y*(1-damp)+filter*damp;data[position]=x+filter*fb;if(++position==data.size())position=0;return y;}void clear(){std::fill(data.begin(),data.end(),0);filter=0;position=0;} };
struct Allpass { std::vector<float> data;size_t position=0;float process(float x){float b=data[position],y=b-x;data[position]=x+b*.5f;if(++position==data.size())position=0;return y;}void clear(){std::fill(data.begin(),data.end(),0);position=0;} };
float randomBipolar(uint32_t &s){s^=s<<13;s^=s>>17;s^=s<<5;return float(double(s)/2147483648.0-1.0);}
float readSample(const std::vector<float>&a,double pos){if(a.empty())return 0;pos=clamp(pos,0.0,double(a.size()-1));auto i=static_cast<size_t>(pos);auto j=std::min(i+1,a.size()-1);return a[i]+(a[j]-a[i])*float(pos-double(i));}
// Four-point Hermite interpolation reduces interpolation droop, while integer
// positions retain exact source frames. Clamp the taps to the selected region:
// excluded transients and neighbouring slices must never leak into playback.
void updateSampleBounds(Voice&v){size_t n=v.sample->l.size();v.sampleFirst=size_t(clamp(std::floor(v.begin),0.0,double(n-1)));v.sampleLast=size_t(clamp(std::ceil(v.end)-1,double(v.sampleFirst),double(n-1)));}
inline float hermite(float y0,float y1,float y2,float y3,float t){float c1=.5f*(y2-y0),c2=y0-2.5f*y1+2*y2-.5f*y3,c3=.5f*(y3-y0)+1.5f*(y1-y2);return ((c3*t+c2)*t+c1)*t+y1;}
// Both channels of readHermite at one position: the tap indices and fraction are shared,
// each channel's arithmetic is exactly readHermite's.
inline void readHermite2(const Sample&s,double pos,size_t first,size_t last,float&l,float&r){pos=clamp(pos,double(first),double(last));size_t i=size_t(pos);float t=float(pos-double(i));const float*L=s.l.data(),*R=s.r.data();if(t==0){l=L[i];r=R[i];return;}size_t i0=i>first?i-1:first,i2=std::min(i+1,last),i3=std::min(i+2,last);l=hermite(L[i0],L[i],L[i2],L[i3],t);r=hermite(R[i0],R[i],R[i2],R[i3],t);}
// Reads a single-cycle frame cyclically (phase 0..1), so the end of the cycle interpolates into its start.
// The four cyclic taps of one position, shared by both channels and both crossfaded frames.
struct CycleTaps{size_t k0,k1,k2,k3;float t;};
inline CycleTaps cycleTaps(size_t width,double phase){double x=phase*double(width);size_t i=size_t(x);if(i>=width)i%=width;CycleTaps c;c.t=float(x-std::floor(x));
    // (i+k)%width without dividing: i<width, so one wrap suffices once width>=3.
    if(width>=3){c.k1=i;c.k2=i+1>=width?i+1-width:i+1;c.k3=i+2>=width?i+2-width:i+2;c.k0=i+width-1>=width?i-1:i+width-1;}
    else{c.k0=(i+width-1)%width;c.k1=i%width;c.k2=(i+1)%width;c.k3=(i+2)%width;}return c;}
inline float readCycleTaps(const std::vector<float>&a,size_t base,const CycleTaps&c){size_t last=a.size()-1;return hermite(a[std::min(last,base+c.k0)],a[std::min(last,base+c.k1)],a[std::min(last,base+c.k2)],a[std::min(last,base+c.k3)],c.t);}
// fmod(fmod(x,range)+range,range) for range>=1, without dividing when x is already in
// [0,range): fmod(x,range) is then x, and the sum lies in [range,2*range], where fmod
// subtracts range exactly (or yields 0 at 2*range). Identical results, bit for bit.
inline double wrapRange(double x,double range){if(x>=0&&x<range){double a=x+range;return a<2*range?a-range:0.0;}return std::fmod(std::fmod(x,range)+range,range);}
/* Granular parameters shared by automation and live parameter edits (other targets: false). */
inline bool setGrainTarget(STInstrument&p,int target,float x){switch(target){
    case ST_TARGET_GRAIN_DENSITY:p.grainDensity=int(std::lround(clamp(x,1.f,float(ST_GRANULAR_MAX_GRAINS))));return true;
    case ST_TARGET_GRAIN_SPRAY:p.grainSpray=clamp(x,0.f,1.f);return true;
    case ST_TARGET_GRAIN_PITCH_SPREAD:p.grainPitchSpread=clamp(x,0.f,24.f);return true;
    case ST_TARGET_GRAIN_SCAN:p.grainScan=clamp(x,-4.f,4.f);return true;
    case ST_TARGET_GRAIN_REVERSE:p.grainReverse=clamp(x,0.f,1.f);return true;
    case ST_TARGET_GRAIN_STEREO:p.grainStereo=clamp(x,0.f,1.f);return true;
    default:return false;}}
// Loop-seam crossfade. Approaching the wrap point, blend toward the audio that sits next to the opposite loop
// point (outside the loop when the region has it), so wrapping never jumps however the loop points are placed.
// Prefers audio before the loop start (forward) or after the loop end (backward); otherwise it fades the first
// moments after each wrap from the audio beyond the wrap point. Loops spanning the whole region keep a plain wrap.
// Both channels share the positions and fade weights.
void loopRead2(const Sample&s,const Voice&v,double x,double fade,float&l,float&r){
    auto read=[&](double position,float&a,float&b){readHermite2(s,position,v.sampleFirst,v.sampleLast,a,b);};
    read(x,l,r);double lo=v.loopBegin,hi=v.loopEnd,width=hi-lo;if(fade<1||x<lo||x>=hi)return;
    double pre=std::min(fade,lo-v.begin),post=std::min(fade,v.end-hi);float ol,or_;
    if(v.dir>0){if(pre>=1&&x>=hi-pre){float t=float((x-(hi-pre))/pre);read(x-width,ol,or_);l=l+(ol-l)*t;r=r+(or_-r)*t;return;}
        if(pre<1&&post>=1&&v.looped&&x<lo+post){float t=float((x-lo)/post);read(x+width,ol,or_);l=ol+(l-ol)*t;r=or_+(r-or_)*t;return;}}
    else{if(post>=1&&x<lo+post){float t=float((lo+post-x)/post);read(x+width,ol,or_);l=l+(ol-l)*t;r=r+(or_-r)*t;return;}
        if(post<1&&pre>=1&&v.looped&&x>=hi-pre){float t=float((hi-x)/pre);read(x-width,ol,or_);l=ol+(l-ol)*t;r=or_+(r-or_)*t;return;}}
}
}

struct STEngine {
    double sampleRate;
    std::unique_ptr<Project> control;std::unique_ptr<Project> draft;bool updating=false;
    Project *current=nullptr;std::atomic<Project*> pending{nullptr},retired{nullptr};std::vector<std::unique_ptr<Sample>> samples;std::vector<std::shared_ptr<const EngineTiming>>timingMaps;
    Ring<Command,maxCommands> commands;Ring<STEvent,maxEvents> events;
    Ring<STInputAck,ST_INPUT_ACK_CAPACITY> inputAcks;std::array<STInputAck,maxCommands> pendingInputAcks{};int pendingInputCount=0;
    // Live notes placed early on the next row (see STInputAck.early): that row's own trigger of the note is skipped once.
    // Seek, Panic and restore end the live voice the skip stands for, so they forget pending entries (Play restarts
    // the current row, whose trigger forgets them).
    struct EarlyInput{int track=-1,note=-1,pattern=-1,row=-1;};std::array<EarlyInput,16> earlyInputs{};int earlyInputNext=0;
    bool consumeEarlyInput(int lane,int note,int row){for(auto&e:earlyInputs)if(e.track==lane&&e.note==note&&e.pattern==pattern&&e.row==row){e=EarlyInput{};return true;}return false;}
    void forgetEarlyInputs(){earlyInputs.fill(EarlyInput{});}
    std::atomic<uint32_t> inputOutstanding{0};uint64_t lastInputRequestID=0,inputRunID=1;double inputLoopBeats=0;std::atomic<bool> automationRefreshRequested{false};
    std::array<Voice,maxVoices> voices;std::array<uint64_t,maxVoices/64> activeVoiceMasks{};std::array<Scheduled,maxScheduled> scheduled;std::array<uint64_t,maxScheduled/64> scheduledMasks{};
    /* Mask words that may hold pending notes: slots are taken lowest first, so the per-sample scan stops here. */
    size_t scheduledGroupsUsed=0;
    /* Render view of current->tables; replaced only when a project is swapped in. */
    ChannelTables*tb=nullptr;
    std::array<LaneEventState,256> laneEventState{},hostLaneEventState{};int automationPattern=-1;int pluginRowLockCount=0,pluginLocksPending=0;std::array<bool,256> pluginLaneLocked{},pluginHostLaneLocked{};int64_t clickBeat=-1;double clickPhase=0,clickAge=1e9;bool clickAccent=false;
    std::vector<float> delayL,delayR;size_t delayPos=0;float delayToneL=0,delayToneR=0;std::vector<float> preL,preR;size_t prePos=0;std::array<Comb,8> combL,combR;std::array<Allpass,4> apL,apR;
    std::vector<STClockContext>cachedClock;uint64_t cacheGeneration=1;
    /* Control-owned bus history, keyed by logical bus so renumbering keeps it. */
    std::vector<std::shared_ptr<BusRing>> trackRings;std::shared_ptr<BusRing> returnRing;
    std::atomic<PeakStore*> peakStore{nullptr};std::vector<std::unique_ptr<PeakStore>> peakStores;
    std::atomic<FreezeHistory*> freezeHistory{nullptr};STFreezeContext cacheOriginFreeze{};bool cacheOriginFreezeValid=false;
    uint64_t freezePlaybackID=1,freezeTransitionID=1;STFreezeCause freezeCause=ST_FREEZE_CAUSE_NONE;
    bool freezeTransitionPending=false,freezeNaturalTail=false;int64_t freezeTransitionGraph=INT64_MIN,freezeTransitionProject=0;
    const EngineTiming*cacheOriginTiming=nullptr;STClockContext cacheOriginClock{};double cacheOriginTransport=0;uint64_t cacheOriginGraph=0;bool cacheOriginValid=false;
    std::atomic<uint64_t> cacheEpoch{1};uint64_t observedCacheEpoch=0,cacheBegin=0,cacheEnd=0,clockBegin=0;bool cacheValid=false;
    bool playing=false,recording=false,songMode=false,rowHasStarted=false,sequenceFinished=false,suppressEvents=false;double songEndFrames=0;int scheduledCount=0;int pattern=0,row=0,sequence=0,serial=0,frameOffset=0;
    int loopStartRow=-1,loopEndRow=-1; /* Optional pattern-mode loop range, inclusive rows; -1 disables. */
    int queuedPattern=-1,queuedSequence=-1; /* Optional jump honoured at the next pattern boundary; -1 none. */
    int queuedMode=-1; /* With a queued jump: switch to Song (1) or Pattern (0) mode there; -1 keeps the mode. */
    int64_t countInTotal=0,countInElapsed=0,countInClick=1,countInBar=1;bool countInSong=false; /* Optional pre-roll before Play; 0 none. */
    /* Pattern-mode clip cursor: frames since the current pass's row 0, and printed tails of passes that handed over to another pattern. */
    struct PatternTail {bool active=false,loop=false;int pattern=0;int64_t position=0;};
    int64_t patternClipFrame=0;int patternClipPattern=-1,patternClipRow=-1,patternClipLength=0;bool patternClipValid=false,patternClipLooped=false;std::array<PatternTail,8> patternTails{};
    struct SongTail {bool active=false;int clip=0;int64_t position=0;};std::array<SongTail,8> songTails{};
    double rowRemaining=0,rowDuration=6000,transportFrames=0,totalFrames=0;
    double musicalBeat=0,effectiveBpm=120,patternOriginBeat=0,loopBeatOffset=0,rowStartBeat=0,rowEndBeat=.25;uint64_t timingEpoch=1;int64_t clockLoopIteration=0;STClockContext musicalContext{};uint32_t rng=0xa173cc79;
    std::atomic<int32_t> statusPlaying{0},statusRecording{0},statusPattern{0},statusRow{0},statusSequence{0},statusTracks{8},statusSongMode{0},statusQueuedPattern{-1},statusQueuedSequence{-1},statusCountIn{0};
    std::atomic<double> statusPosition{0},statusSeconds{0},statusBeat{0},statusBpm{120},statusBarStart{0};std::atomic<int64_t> statusBar{1};std::atomic<int32_t>statusNumerator{4},statusDenominator{4};std::atomic<uint64_t>statusTimingRevision{0};std::atomic<float> peakL{0},peakR{0};
    std::atomic<uint64_t> droppedCommands{0},droppedEvents{0};
    /* Live instrument parameters (st_engine_set_instrument_parameter): render-thread only, cleared on publication. */
    struct LiveParam { int instrument=-1,target=0;float value=0; };
    std::array<LiveParam,64> liveParams{};int liveParamCount=0;
    /* Voice playhead snapshot (st_engine_voice_positions): seqlock, odd while the render thread writes. */
    std::atomic<int32_t> voiceWatch{-2};std::atomic<uint64_t> voiceSeq{0};std::atomic<int32_t> voiceSnapshotCount{0};
    std::array<STVoicePosition,ST_VOICE_POSITION_CAPACITY> voiceSnapshot{};uint64_t voiceSerial=0;bool voiceSnapshotEmpty=true;
    /* Play/Stop/count-in are applied by the render thread, so the published status lags a queued
       transport command until the next render (or forever while no render runs). Each one carries a
       serial; until the render has applied and published the latest serial, st_engine_status flags it
       pending together with the requested state (st_status_transport_playing). */
    std::atomic<uint32_t> transportRequested{0},statusTransportApplied{0};std::atomic<int32_t> transportRequestedPlaying{0};uint32_t transportApplied=0;
    std::atomic<uint64_t> statusPublication{0},statusSeekRevision{0};uint64_t seekRevision=0;
    explicit STEngine(double rate):sampleRate(rate),control(new Project),current(new Project){
        cachedClock.resize(clockCapacity);delayL.resize(size_t(sampleRate*4)+1);delayR.resize(delayL.size());preL.assign(size_t(sampleRate*.5)+1,0.f);preR.assign(preL.size(),0.f);
        buildTables(*control);current->tables=control->tables;tb=current->tables.get();
        const int combSizes[]={1116,1188,1277,1356,1422,1491,1557,1617};const int apSizes[]={556,441,341,225};
        for(int i=0;i<8;++i){combL[i].data.resize(size_t(combSizes[i]*sampleRate/44100)+1);combR[i].data.resize(size_t((combSizes[i]+23)*sampleRate/44100)+1);}
        for(int i=0;i<4;++i){apL[i].data.resize(size_t(apSizes[i]*sampleRate/44100)+1);apR[i].data.resize(size_t((apSizes[i]+23)*sampleRate/44100)+1);}
        resetTrackState(false);
    }
    ~STEngine(){delete freezeHistory.load();releaseClockOrigin();for(auto&v:voices)kill(v);delete pending.load();delete current;collect();draft.reset();control.reset();}
    // A render-side release changes only an atomic. collect() owns destruction.
    void releaseClockOrigin(){if(cacheOriginTiming)cacheOriginTiming->originPins.fetch_sub(1,std::memory_order_release);cacheOriginTiming=nullptr;cacheOriginValid=false;cacheOriginFreezeValid=false;}
    Project &edit(){if(!draft)draft.reset(new Project(*control));return *draft;}
    /* The project control-thread edits apply to, without creating a draft. */
    const Project &view()const{return draft?*draft:*control;}
    Project &editPatterns(){auto&p=edit();p.patternEpoch=0;return p;}
    void collect(){auto p=retired.exchange(nullptr,std::memory_order_acq_rel);while(p){auto n=p->retiredNext;delete p;p=n;}samples.erase(std::remove_if(samples.begin(),samples.end(),[](const std::unique_ptr<Sample>&s){return s->refs.load(std::memory_order_acquire)==0;}),samples.end());timingMaps.erase(std::remove_if(timingMaps.begin(),timingMaps.end(),[](const auto&map){return map.use_count()==1&&map->originPins.load(std::memory_order_acquire)==0;}),timingMaps.end());}
    void compileTiming(Project&p){
        if(!p.timing){p.occurrenceBeats.clear();p.occurrenceFrames.clear();return;}
        p.occurrenceBeats.clear();p.occurrenceFrames.clear();p.occurrenceBeats.reserve(p.sequence.size()+1);p.occurrenceFrames.reserve(p.sequence.size()+1);
        double beat=0;for(int index:p.sequence){p.occurrenceBeats.push_back(beat);p.occurrenceFrames.push_back(p.timing->frame(beat));beat+=patternRows(p,index)/p.lpb;if(beat>1000000)throw std::invalid_argument("Song exceeds timing map domain of1000000 quarter beats.");}
        p.occurrenceBeats.push_back(beat);p.occurrenceFrames.push_back(p.timing->frame(beat));
        double patternEnd=p.patternOriginBeat+patternRows(p,p.activePattern)/p.lpb;
        for(size_t index=0;index<p.patterns.size();++index)if(p.patternOriginBeat+patternRows(p,int(index))/p.lpb>1000000)throw std::invalid_argument("Pattern timing origin plus length exceeds map domain.");
        if(patternEnd>1000000)throw std::invalid_argument("Pattern timing origin plus length exceeds map domain.");
        p.mappedPatternStart=p.timing->frame(p.patternOriginBeat);p.mappedPatternEnd=p.timing->frame(patternEnd);
        for(auto&clip:p.audioClips){
            if(clip.startBeat+clip.durationBeats>1000000)throw std::invalid_argument("Clip end exceeds timing map domain.");
            clip.mappedStart=p.timing->frame(clip.startBeat);auto*sample=p.audioAssets[clip.asset].sample;
            double available=sample?std::max(0.0,double(sample->l.size())/sample->rate-clip.sourceOffset):0;
            double sourceEnd=clip.mappedStart+std::ceil(available*sampleRate);
            if(p.frozenSongPolicy&&p.frozenSongFrames&&sample&&available>0){
                // Exact-extent publications admit source-limited clip endpoints
                // in sampled coordinates, avoiding6007/rate*rate ->6008.
                int64_t candidate=int64_t(sourceEnd);
                auto reached=[&](int64_t at){return std::fma(double(at-clip.mappedStart),sample->rate/sampleRate,clip.sourceOffset*sample->rate)>=double(sample->l.size());};
                for(int correction=0;correction<4;++correction){if(candidate>clip.mappedStart&&reached(candidate-1))--candidate;else if(!reached(candidate))++candidate;else break;}
                if(!reached(candidate)||(candidate>clip.mappedStart&&reached(candidate-1)))throw std::invalid_argument("Frozen clip source end cannot be resolved to an exact output sample.");
                sourceEnd=double(candidate);
            }
            double limit=clip.durationBeats>0?double(p.timing->frame(clip.startBeat+clip.durationBeats)):sourceEnd;
            if(std::min(sourceEnd,limit)>p.timing->frame(1000000))throw std::invalid_argument("Clip source duration exceeds timing map domain.");
            clip.mappedEnd=int64_t(std::min(sourceEnd,limit));
        }
    }
    bool commit(char*error=nullptr,uint32_t capacity=0){
        if(error&&capacity)error[0]=0;if(!draft)return true;
        try{normalize(*draft);compileTiming(*draft);validateFrozenExtent(*draft);
            // Only lane lists edited in this draft can be unsorted; shared ones were sorted when first published.
            for(auto&p:draft->patterns)if(p.lanesDirty){for(auto*lanes:{p.lanes.get(),p.hostLanes.get()})if(lanes)for(auto&l:*lanes){std::sort(l.points.begin(),l.points.end(),[](const Point&a,const Point&b){return a.row<b.row;});}p.lanesDirty=false;}
            buildTables(*draft);if(!draft->patternEpoch)draft->patternEpoch=patternEpochs.fetch_add(1,std::memory_order_relaxed)+1;
            auto next=std::unique_ptr<Project>(new Project(*draft));control=std::move(draft);auto old=pending.exchange(next.release(),std::memory_order_acq_rel);delete old;updating=false;collect();return true;
        }catch(const std::exception&problem){if(error&&capacity){size_t count=std::min<size_t>(capacity-1,std::strlen(problem.what()));std::memcpy(error,problem.what(),count);error[count]=0;}return false;}
    }
    static bool projectChannelEnabled(const Project&p,int c){return c>=0&&c<p.trackCount;}
    static void normalize(Project&p){
        size_t base=size_t(p.returnBus());if(p.tracks.size()<base)p.tracks.resize(base,st_default_track());
        p.anySolo=false;for(int c=0;c<p.trackCount;++c)p.anySolo|=p.tracks[size_t(c)].solo!=0;
    }
    /* Control thread: give the project per-channel render tables. The latest published
       tables are reused while the track count is unchanged; otherwise
       new tables (and history rings for new buses) are allocated here, never on render. */
    void buildTables(Project&p){
        if(!(p.tables&&p.tables->sameShape(p.trackCount))){
            if(control&&control->tables&&control->tables->sameShape(p.trackCount))p.tables=control->tables;
            else{
                auto tables=std::make_shared<ChannelTables>(p.trackCount);
                trackRings.resize(size_t(p.trackCount));for(auto&ring:trackRings)if(!ring)ring=std::make_shared<BusRing>();
                if(!returnRing)returnRing=std::make_shared<BusRing>();
                for(int t=0;t<p.trackCount;++t)tables->rings[size_t(t)]=trackRings[size_t(t)];
                tables->rings[size_t(tables->base)]=returnRing;
                p.tables=std::move(tables);
            }
        }
        auto*store=peakStore.load(std::memory_order_acquire);size_t needed=size_t(p.tables->channels);
        if(!store||store->count<needed){auto grown=std::make_unique<PeakStore>(std::max(needed,store?store->count*2:needed));peakStore.store(grown.get(),std::memory_order_release);peakStores.push_back(std::move(grown));}
    }
    /* Render thread, at a project swap whose tables differ: carry sticky ratchets, pitch
       bend and mod wheel over by track. Voices of removed tracks end. No allocation:
       both tables are prebuilt. */
    void migrateTables(const Project&,const Project&next){
        auto&from=*tb;auto&to=*next.tables;
        for(int t=0;t<std::min(from.trackCount,to.trackCount);++t){to.laneRatchets[size_t(t)]=from.laneRatchets[size_t(t)];to.pitchBend[size_t(t)]=from.pitchBend[size_t(t)];to.modWheel[size_t(t)]=from.modWheel[size_t(t)];}
        for(auto&v:voices)if(v.active&&v.track>=to.trackCount)kill(v);
    }
    // Freeze metadata is independent of the musical input run. Looping starts a
    // new print playback, while recording distance continues through that loop.
    void freezeTransition(STFreezeCause cause,bool newPlayback=true,bool naturalTail=false){
        if(freezeTransitionID)freezeTransitionID=freezeTransitionID==UINT64_MAX?0:freezeTransitionID+1;
        if(newPlayback&&freezePlaybackID)freezePlaybackID=freezePlaybackID==UINT64_MAX?0:freezePlaybackID+1;
        freezeCause=cause;freezeNaturalTail=naturalTail;freezeTransitionPending=true;
        freezeTransitionGraph=INT64_MIN;freezeTransitionProject=0;
    }
    bool freezePositionValid(double frame)const{return std::isfinite(frame)&&frame>=0&&frame<=double(uint64_t(1)<<52)&&frame==std::floor(frame);}
    void alignFrozenOrigin(){
        if(!current->frozenSongPolicy||!songMode||!std::isfinite(transportFrames)||transportFrames<0||transportFrames>double(uint64_t(1)<<52))return;
        double aligned=std::ceil(transportFrames),delta=aligned-transportFrames;
        // Negative remainder at an unstarted row carries its original fractional
        // boundary into triggerRow; a started row retains no-retrigger semantics.
        transportFrames=aligned;rowRemaining-=delta;
        // An explicit projected extent is an admission boundary, not evidence
        // that a replacement/restored cursor naturally played through its end.
        if(current->frozenSongFrames&&transportFrames>=double(current->frozenSongFrames)){
            transportFrames=double(current->frozenSongFrames);playing=false;sequenceFinished=true;
            sequence=int(current->sequence.size())-1;pattern=validPattern(current->sequence[sequence]);row=current->patterns[pattern].length-1;rowRemaining=1e100;
            clearScheduled();allOff(false,true);
        }
    }
    void retainFreeze(uint64_t graphFrame,bool origin){
        auto*history=freezeHistory.load(std::memory_order_acquire);if(!history)return;
        STFreezeContext c{};c.publicationID=current->frozenSongPolicy?current->frozenPublicationID:0;
        c.playbackID=freezePlaybackID;c.transitionID=freezeTransitionID;c.graphFrame=int64_t(graphFrame);c.contentGraphFrame=c.graphFrame;c.sampleRate=sampleRate;c.cause=freezeCause;
        bool valid=freezePositionValid(transportFrames)&&freezePlaybackID&&freezeTransitionID;
        if(freezeTransitionPending){freezeTransitionGraph=c.contentGraphFrame;freezeTransitionProject=valid?int64_t(transportFrames):0;freezeTransitionPending=false;}
        c.transitionContentGraphFrame=freezeTransitionGraph;c.transitionProjectFrame=freezeTransitionProject;
        c.projectFrame=valid?int64_t(transportFrames):0;
        if(!current->frozenSongPolicy)c.phase=ST_FREEZE_DISABLED;
        else if(!songMode)c.phase=ST_FREEZE_UNSUPPORTED;
        else if(!valid)c.phase=ST_FREEZE_INVALID_POSITION;
        else if(playing)c.phase=ST_FREEZE_SONG;
        else if(freezeNaturalTail&&freezeCause==ST_FREEZE_CAUSE_NATURAL_END&&freezeTransitionGraph!=INT64_MIN){
            int64_t elapsed=c.contentGraphFrame-freezeTransitionGraph;
            if(elapsed<0||elapsed>(int64_t(1)<<52)-freezeTransitionProject)c.phase=ST_FREEZE_INVALID_POSITION;
            else{c.phase=ST_FREEZE_NATURAL_TAIL;c.projectFrame=freezeTransitionProject+elapsed;}
        }else c.phase=ST_FREEZE_STOPPED;
        history->frames[size_t(graphFrame%clockCapacity)]=c;
        if(origin){cacheOriginFreeze=c;cacheOriginFreezeValid=true;}
    }
    void resetInputRun(){if(inputRunID)inputRunID=inputRunID==UINT64_MAX?0:inputRunID+1;inputLoopBeats=0;}
    void finishInputAcks(bool audioFrame,bool cached=false,uint64_t graphFrame=0){
        for(int n=0;n<pendingInputCount;++n){auto ack=pendingInputAcks[n];
            ack.renderFrame=int64_t(totalFrames);ack.graphFrame=cached&&graphFrame<=(uint64_t(1)<<52)?int64_t(graphFrame):INT64_MIN;
            // An early note keeps its placed row while that row is the one now playing or still ahead.
            if(!ack.early||ack.pattern!=pattern||ack.sequence!=sequence||row>ack.row){ack.pattern=pattern;ack.row=row;ack.sequence=sequence;ack.early=0;}
            ack.status=!audioFrame?ST_INPUT_ACK_CONTROL_FLUSH:(!inputRunID||ack.runID!=inputRunID)?ST_INPUT_ACK_DISCONTINUITY:ST_INPUT_ACK_APPLIED;
            if(ack.status==ST_INPUT_ACK_APPLIED){
                ack.timingRevision=current->timing?current->timing->revision:0;
                ack.distanceBeats=musicalBeat+inputLoopBeats;
                ack.flags=(playing?ST_INPUT_CLOCK_PLAYING:0)|(current->timing?ST_INPUT_CLOCK_MAPPED:0)|(recording?ST_INPUT_CLOCK_RECORDING:0);
                if(!std::isfinite(ack.distanceBeats)||ack.distanceBeats<0){ack.status=ST_INPUT_ACK_DISCONTINUITY;ack.flags=0;ack.distanceBeats=0;}
            }
            // Admission reserves a slot for every accepted command, including
            // queued-but-not-yet-rendered edges; the ACK ring cannot overflow.
            bool pushed=inputAcks.push(ack);assert(pushed);(void)pushed;
        }
        pendingInputCount=0;
    }
    void queue(Command c){if(!commands.push(c))droppedCommands.fetch_add(1,std::memory_order_relaxed);}
    void queueTransport(Command c,bool playingAfter){uint32_t serial=transportRequested.load(std::memory_order_relaxed)+1;if(!serial)serial=1;c.d=int32_t(serial);
        if(!commands.push(c)){droppedCommands.fetch_add(1,std::memory_order_relaxed);return;}
        transportRequestedPlaying.store(playingAfter,std::memory_order_relaxed);transportRequested.store(serial,std::memory_order_release);}
    void appliedTransport(const Command&c){if(c.d)transportApplied=uint32_t(c.d);}
    bool emit(STEvent e){if(suppressEvents)return true;e.frameOffset=std::max(0,frameOffset);uint32_t free=events.available();
        // Reserve one termination for every possible external voice. Automation and
        // fresh notes may be dropped under extreme pressure; accepted notes always
        // retain enough room for note-off, so an oversized AU pull cannot hang notes.
        if(((e.type==ST_EVENT_PARAMETER||e.type==ST_EVENT_HOST_PARAMETER)&&free<=maxVoices*2)||(e.type==ST_EVENT_NOTE_ON&&free<=maxVoices)||!events.push(e)){droppedEvents.fetch_add(1,std::memory_order_relaxed);return false;}return true;}
    void resetAutomationEvents(){for(auto&state:laneEventState)state=LaneEventState{};for(auto&state:hostLaneEventState)state=LaneEventState{};automationPattern=-1;}
    bool channelEnabled(int channel)const{return projectChannelEnabled(*current,channel);}
    const STTrack& channelMix(int channel)const{return current->tracks[size_t(channel)];}
    int channelCount()const{return current->trackCount;}
    int outputBus(int channel)const{return channel;}
    void emitMixState(){for(int t=0;t<channelCount();++t){if(!channelEnabled(t))continue;const auto&s=tb->trackState[size_t(t)];if(!current->hasInstrument(channelMix(t).instrument))continue;const auto&p=current->instruments[size_t(channelMix(t).instrument)].p;float instrumentVolume=s.instrument[ST_TARGET_INSTRUMENT_VOLUME-ST_TARGET_ATTACK],instrumentPan=s.instrument[ST_TARGET_INSTRUMENT_PAN-ST_TARGET_ATTACK];const int targets[]={ST_TARGET_VOLUME,ST_TARGET_PAN,ST_TARGET_DELAY,ST_TARGET_REVERB,ST_TARGET_INSTRUMENT_VOLUME,ST_TARGET_INSTRUMENT_PAN};const float values[]={s.volume,s.pan,s.delay,s.reverb,std::isfinite(instrumentVolume)?instrumentVolume:p.volume,std::isfinite(instrumentPan)?instrumentPan:p.pan};for(int n=0;n<6;++n){STEvent event{};event.type=ST_EVENT_PARAMETER;event.track=t;event.target=targets[n];event.value=values[n];emit(event);}}}
    void clearPluginLocks(){pluginRowLockCount=0;pluginLocksPending=0;pluginLaneLocked.fill(false);pluginHostLaneLocked.fill(false);}
    void clearRatchets(){for(auto&r:tb->laneRatchets)r=RatchetState{};}
    void resetTrackState(bool emitDefaults=true){clearPluginLocks();resetAutomationEvents();for(int t=0;t<tb->channels;++t){auto&p=channelMix(t);auto&s=tb->trackState[size_t(t)];s=TrackState{};s.volume=p.volume;s.pan=p.pan;s.delay=p.delaySend;s.reverb=p.reverbSend;}if(emitDefaults)emitMixState();}
    void refresh(){if(auto next=pending.exchange(nullptr,std::memory_order_acq_rel)){
            auto old=current;double previousBeat=old->timing?old->timing->beat(int64_t(transportFrames)):transportFrames*old->bpm/(60*sampleRate);bool mapChanged=old->timing!=next->timing;if(mapChanged&&!old->timing&&next->timing&&!songMode){double fraction=rowHasStarted?clamp(1-rowRemaining/rowDuration,0.0,1.0):0;previousBeat=next->patternOriginBeat+beatForRow(*old,row)+fraction*(beatForRow(*old,row+1)-beatForRow(*old,row));}
            // Legacy (unmapped) frames are bpm-relative: rescale them so a tempo change keeps the musical position
            // (Song mode re-resolves its row from these frames) and the current row ends at the new tempo.
            if(!old->timing&&!next->timing&&old->bpm!=next->bpm){double scale=old->bpm/next->bpm;transportFrames*=scale;if(rowHasStarted&&rowRemaining<1e99){rowRemaining*=scale;rowDuration*=scale;}
                // The row's pending hits (ratchet repeats, microtiming) and sequenced gates are frame counts too: keep their musical positions.
                for(auto&s:scheduled)if(s.active&&!s.mapped){s.due=totalFrames+(s.due-totalFrames)*scale;if(s.gate>0)s.gate=std::max(1.0,s.gate*scale);}
                for(auto&v:voices)if(v.active&&v.sequenced&&v.gate>0)v.gate*=scale;}
            bool arrangementChanged=old->sequence!=next->sequence||old->lpb!=next->lpb||old->swing!=next->swing||old->patternOriginBeat!=next->patternOriginBeat;
            if(!arrangementChanged)for(int index:next->sequence)if(index>=int(old->patterns.size())||old->patterns[index].length!=next->patterns[index].length){arrangementChanged=true;break;}
            bool timelineChanged=arrangementChanged||old->bpm!=next->bpm||mapChanged;
            double nextSongEndFrames=effectiveSongFrames(*next);
            bool inputPatternChanged=pattern>=int(next->patterns.size())||old->patterns[pattern].length!=next->patterns[pattern].length;
            bool inputTimelineChanged=arrangementChanged||inputPatternChanged||songEndFrames!=nextSongEndFrames||old->bpm!=next->bpm||old->loopSong!=next->loopSong||bool(old->timing)!=bool(next->timing)||(mapChanged&&old->timing&&next->timing&&!old->timing->matches(*next->timing));
            if(inputTimelineChanged)resetInputRun();
            bool freezeChanged=old->frozenSongPolicy!=next->frozenSongPolicy||old->frozenPublicationID!=next->frozenPublicationID||(next->frozenSongPolicy&&inputTimelineChanged);
            // Mixer default refreshes and pattern transitions must not erase tracker
            // repeat memory; tables only change with the track count.
            if(next->tables.get()!=tb)migrateTables(*old,*next);
            current=next;tb=current->tables.get();if(old->hostMixing!=current->hostMixing)clearEffects();songEndFrames=nextSongEndFrames;
            pattern=clamp(pattern,0,int(current->patterns.size()-1));row=clamp(row,0,current->patterns[pattern].length-1);sequence=clamp(sequence,0,int(current->sequence.size()-1));patternOriginBeat=current->patternOriginBeat;
            if(mapChanged){
                transportFrames=current->timing?current->timing->frame(previousBeat):previousBeat*60/current->bpm*sampleRate;
                for(auto&v:voices)if(v.active&&v.sequenced){if(current->timing&&!old->timing&&v.gate>=0){v.mappedGateEnd=previousBeat+loopBeatOffset+v.gate*old->bpm/(60*sampleRate);v.gate=-1;}else if(!current->timing&&v.mappedGateEnd>=0){v.gate=std::max(0.0,v.mappedGateEnd-previousBeat-loopBeatOffset)*60/current->bpm*sampleRate;v.mappedGateEnd=-1;}}
                if(!current->timing)for(auto&s:scheduled)if(s.active&&s.mapped){s.due=totalFrames+std::max(0.0,s.onsetBeat-previousBeat-loopBeatOffset)*60/current->bpm*sampleRate;s.gate=s.open?-1.0:std::max(1.0,(s.endBeat-s.onsetBeat)*60/current->bpm*sampleRate);s.mapped=false;}
            }
            if(timelineChanged){++timingEpoch;if(current->timing)resolveMappedTimeline(arrangementChanged);else if(playing&&songMode)resolveTimeline();else if(mapChanged&&!current->timing&&!songMode){double local=std::max(0.0,previousBeat-old->patternOriginBeat);double width=patternRows(*current,pattern)/current->lpb;local=std::fmod(local,width);transportFrames=local*60/current->bpm*sampleRate;row=0;while(row+1<current->patterns[pattern].length&&beatForRow(*current,row+1)<=local)++row;double start=beatForRow(*current,row)*60/current->bpm*sampleRate;double end=beatForRow(*current,row+1)*60/current->bpm*sampleRate;rowDuration=end-start;rowHasStarted=transportFrames>start;rowRemaining=rowHasStarted?end-transportFrames:0;}}
            else if(playing&&songMode&&sequenceFinished)resolveTimeline();
            if(freezeChanged){alignFrozenOrigin();freezeTransition(ST_FREEZE_CAUSE_REPLACEMENT);}
            auto head=retired.load(std::memory_order_relaxed);do{old->retiredNext=head;}while(!retired.compare_exchange_weak(head,old,std::memory_order_release,std::memory_order_relaxed));
            liveParamCount=0;resetTrackState();for(auto&v:voices)if(v.active&&!channelEnabled(v.track))kill(v);for(auto&v:voices)if(v.active&&!v.external&&!current->hasInstrument(v.instrument))kill(v);for(auto&v:voices)if(v.active&&!v.external){const auto&p=current->instruments[size_t(v.instrument)].p;if(p.type==v.p.type){v.p.volume=p.volume*v.zoneVolume;v.p.pan=clamp(p.pan+v.zonePan,-1.f,1.f);v.p.tune=p.tune+v.zoneTune;v.p.rootNote=v.zoneRoot>=0?v.zoneRoot:p.rootNote;v.p.attack=p.attack;v.p.decay=p.decay;v.p.sustain=p.sustain;v.p.release=p.release;v.p.cutoff=p.cutoff;v.p.resonance=p.resonance;v.p.drive=p.drive;v.p.bitDepth=p.bitDepth;v.p.lfoRate=p.lfoRate;v.p.lfoDepth=p.lfoDepth;v.p.lfoTarget=p.lfoTarget;v.p.lfoShape=p.lfoShape;v.p.lfo2Rate=p.lfo2Rate;v.p.lfo2Depth=p.lfo2Depth;v.p.lfo2Target=p.lfo2Target;v.p.lfo2Shape=p.lfo2Shape;v.p.lfoSync=p.lfoSync;v.p.lfo2Sync=p.lfo2Sync;v.p.lfoBeats=p.lfoBeats;v.p.lfo2Beats=p.lfo2Beats;v.p.filterAttack=p.filterAttack;v.p.filterDecay=p.filterDecay;v.p.filterSustain=p.filterSustain;v.p.filterRelease=p.filterRelease;v.p.filterEnvAmount=p.filterEnvAmount;v.p.grainSize=p.grainSize;v.p.grainPosition=p.grainPosition;v.p.wavetablePosition=p.wavetablePosition;v.p.grainDensity=p.grainDensity;v.p.grainSpray=p.grainSpray;v.p.grainPitchSpread=p.grainPitchSpread;v.p.grainScan=p.grainScan;v.p.grainReverse=p.grainReverse;v.p.grainStereo=p.grainStereo;}}for(auto&v:voices)if(v.active&&v.external){int type=current->hasInstrument(v.instrument)?current->instruments[size_t(v.instrument)].p.type:ST_INSTRUMENT_SAMPLE;if(!channelEnabled(v.track)||!externalTrackEnabled(v.track,type)||(type!=ST_INSTRUMENT_AU&&type!=ST_INSTRUMENT_MIDI))release(v);}statusTracks.store(current->trackCount,std::memory_order_relaxed);}
        // A concurrent producer may refill the command ring while it is drained.
        // Only opt-in input edges consume this bounded batch; legacy-only command
        // processing and its ordering remain unchanged.
        Command c;while(pendingInputCount<maxCommands&&commands.pop(c))switch(c.type){
            case 1: countInTotal=0;startPlayback(c.a!=0,false);appliedTransport(c);break;
            case 2: clearRatchets();appliedTransport(c);queuedPattern=queuedSequence=queuedMode=-1;countInTotal=0;resetPatternClips();resetInputRun();freezeTransition(ST_FREEZE_CAUSE_STOP);++timingEpoch;playing=false;clearPluginLocks();resetAutomationEvents();allOff(false);clearScheduled();break;
            case 3: ++seekRevision;clearRatchets();forgetEarlyInputs();queuedPattern=queuedSequence=queuedMode=-1;resetPatternClips();resetInputRun();++timingEpoch;clockLoopIteration=0;loopBeatOffset=0;patternOriginBeat=current->patternOriginBeat;allOff(true);pattern=validPattern(c.a);row=clamp(c.b,0,current->patterns[pattern].length-1);sequence=clamp(c.c,0,int(current->sequence.size()-1));rowRemaining=0;rowHasStarted=false;sequenceFinished=false;resetTrackState();transportFrames=seekFrames();alignFrozenOrigin();freezeTransition(ST_FREEZE_CAUSE_SEEK);clickBeat=-1;clearScheduled();break;
            case 4: noteOn(c.a,c.b,c.c,c.d,-1,0,-1,0,0);break;
            case 5: for(auto&v:voices)if(v.active&&!v.sequenced&&!v.preview&&v.track==c.a&&(c.b<0||v.note==c.b))release(v);break;
            case 6: clearRatchets();forgetEarlyInputs();resetPatternClips();resetInputRun();freezeTransition(ST_FREEZE_CAUSE_PANIC);clearPluginLocks();resetAutomationEvents();allOff(true);clearScheduled();clearEffects();std::fill(tb->pitchBend.begin(),tb->pitchBend.end(),0.f);std::fill(tb->modWheel.begin(),tb->modWheel.end(),0.f);queuedPattern=queuedSequence=queuedMode=-1;countInTotal=0;clickAge=1e9;break;
            case 7: if(c.a>=0&&c.a<tb->channels)tb->pitchBend[size_t(c.a)]=clamp(finite(c.value),-48.f,48.f);break;
            case 8: previewStep(c);break;
            case 12: queueJump(c.a,c.b);break;
            case 16: queueModeSwitch(c.a!=0,c.b,c.c);break;
            case 13: beginCountIn(c.a!=0,c.b);appliedTransport(c);break;
            case 14: if(c.a>=0&&c.a<tb->channels)tb->modWheel[size_t(c.a)]=clamp(finite(c.value),0.f,1.f);break;
            case 15: setLiveParam(c.a,c.b,c.value);break;
            case 9: if(recording!=(c.a!=0)){recording=c.a!=0;++timingEpoch;}break;
            case 11: {int start=c.a,end=c.b;if(start<0||end<start){start=-1;end=-1;}if(start==loopStartRow&&end==loopEndRow)break;
                loopStartRow=start;loopEndRow=end;if(songMode)break;resetInputRun();++timingEpoch;
                if(playing&&current->timing){double before=transportFrames;resolveMappedTimeline(false);if(transportFrames!=before)clearScheduled();}break;}
            case 10: {
                if(c.applyInputNote){if(c.inputEdge==ST_INPUT_NOTE_ON)noteOn(c.a,c.b,c.c,c.d,-1,0,-1,0,0);
                    else for(auto&v:voices)if(v.active&&!v.sequenced&&!v.preview&&v.track==c.a&&v.note==c.c)release(v);}
                auto&ack=pendingInputAcks[size_t(pendingInputCount++)];ack={};ack.requestID=c.requestID;ack.runID=inputRunID;
                ack.edge=c.inputEdge;ack.track=c.a;ack.instrument=c.b;ack.note=c.c;ack.velocity=c.d;ack.applyNote=c.applyInputNote;
                ack.early=0;
                if(c.inputEdge==ST_INPUT_NOTE_ON&&playing&&rowHasStarted&&rowDuration>0&&rowRemaining<rowDuration*.5){
                    const int length=current->patterns[size_t(pattern)].length;
                    if(row+1<length&&!(loopEndRow>=0&&row==loopEndRow)){
                        ack.early=1;ack.pattern=pattern;ack.row=row+1;ack.sequence=sequence;
                        earlyInputs[size_t(earlyInputNext)]=EarlyInput{c.a,c.c,pattern,row+1};earlyInputNext=(earlyInputNext+1)%int(earlyInputs.size());
                    }
                }
                break;
            }
        }
        updateClock();
        // This request has a separate mailbox: AU graph preparation may call it on
        // the audio thread while the single UI producer writes normal commands.
        if(automationRefreshRequested.exchange(false,std::memory_order_acq_rel)){emitMixState();resetAutomationEvents();pluginLocksPending=pluginRowLockCount;for(int n=0;n<pluginRowLockCount;++n)tb->pluginRowLocks[size_t(n)].delivery=LaneEventState{};}
    }
    /* Play. A count-in hands over with keepLiveVoices so notes played during the pre-roll keep sounding. */
    void startPlayback(bool song,bool keepLiveVoices){clearRatchets();resetPatternClips();resetInputRun();++timingEpoch;clockLoopIteration=0;loopBeatOffset=0;patternOriginBeat=current->patternOriginBeat;allOff(true,keepLiveVoices);playing=true;songMode=song;if(songMode){sequence=clamp(sequence,0,int(current->sequence.size()-1));pattern=validPattern(current->sequence[sequence]);}else pattern=validPattern(current->activePattern);row=clamp(row,0,current->patterns[pattern].length-1);if(patternLoopActive()&&(row<loopStartRow||row>patternLoopEnd()))row=loopStartRow;rowRemaining=0;rowHasStarted=false;sequenceFinished=false;resetTrackState();transportFrames=seekFrames();alignFrozenOrigin();freezeTransition(ST_FREEZE_CAUSE_PLAY);clickBeat=-1;clearScheduled();queuedPattern=queuedSequence=queuedMode=-1;}
    /* Count-in: one click per meter beat (tempo/meter at the start position), then Play at the exact frame. */
    void beginCountIn(bool song,int bars){if(playing)return;double bpm=current->bpm;int numerator=current->numerator,denominator=current->denominator;
        if(current->timing){int seq=clamp(sequence,0,int(current->sequence.size()-1));int pat=song?validPattern(current->sequence[seq]):validPattern(current->activePattern);int r=clamp(row,0,current->patterns[pat].length-1);double beat=(song?current->occurrenceBeats[size_t(seq)]:current->patternOriginBeat)+beatForRow(*current,r);STClockContext context{};if(st_clock_context(current->timing->timeline.get(),current->timing->frame(beat),0,&context)==ST_CLOCK_OK&&context.bpm>0){bpm=context.bpm;numerator=context.numerator;denominator=context.denominator;}}
        countInClick=std::max<int64_t>(1,std::llround(sampleRate*60/std::max(1.0,bpm)*4/std::max(1,denominator)));countInBar=std::max(1,numerator);countInTotal=countInClick*countInBar*clamp(bars,1,4);countInElapsed=0;countInSong=song;clickAge=1e9;}
    void stepCountIn(){if(countInElapsed>=countInTotal){countInTotal=0;startPlayback(countInSong,true);return;}if(countInElapsed%countInClick==0){clickAge=0;clickPhase=0;clickAccent=(countInElapsed/countInClick)%countInBar==0;}++countInElapsed;}
    double beatForRow(const Project&p,int r)const{return (r+(r%2?p.swing:0))/p.lpb;}
    bool songLoopActive()const{return songMode&&current->loopSong&&!current->frozenSongPolicy&&current->songLoopStart>=0&&current->songLoopEnd>=current->songLoopStart&&current->songLoopStart<int(current->sequence.size());}
    int songLoopLast()const{return std::min(current->songLoopEnd,int(current->sequence.size())-1);}
    double occurrenceBeat(int index)const{if(current->timing)return current->occurrenceBeats[size_t(clamp(index,0,int(current->sequence.size())))];double rows=0;for(int n=0;n<index&&n<int(current->sequence.size());++n)rows+=patternRows(*current,current->sequence[n]);return rows/current->lpb;}
    double occurrenceStartFrames(int index)const{if(current->timing)return double(current->occurrenceFrames[size_t(clamp(index,0,int(current->sequence.size())))]);return occurrenceBeat(index)*sampleRate*60/current->bpm;}
    /* Continue at the section start. From a row boundary the fractional overshoot is kept;
       `exact` (song end with clip tails) restarts the section at its first frame. */
    void loopSongSection(int fromIndex,bool exact,int target=-1){int start=target>=0?target:current->songLoopStart;double beats=occurrenceBeat(fromIndex)-occurrenceBeat(start);
        freezeTransition(ST_FREEZE_CAUSE_LOOP);++timingEpoch;++clockLoopIteration;inputLoopBeats+=beats;clickBeat=-1;if(current->timing)loopBeatOffset+=beats;
        if(exact){transportFrames=occurrenceStartFrames(start);rowRemaining=0;rowHasStarted=false;allOff(false,true);}else transportFrames-=occurrenceStartFrames(fromIndex)-occurrenceStartFrames(start);
        sequence=start;pattern=validPattern(current->sequence[sequence]);row=0;sequenceFinished=false;resetTrackState();}
    bool patternLoopActive()const{return !songMode&&loopStartRow>=0&&loopEndRow>=loopStartRow&&loopStartRow<current->patterns[pattern].length;}
    int patternLoopEnd()const{return std::min(loopEndRow,current->patterns[pattern].length-1);}
    /* Pattern-relative beats for the mapped loop domain; the whole pattern without a range. */
    double patternDomainStartBeat()const{return patternLoopActive()?beatForRow(*current,loopStartRow):0;}
    double patternDomainEndBeat()const{return patternLoopActive()?beatForRow(*current,patternLoopEnd()+1):patternRows(*current,pattern)/current->lpb;}
    double rowBaseBeat()const{return songMode?current->occurrenceBeats[size_t(sequence)]:patternOriginBeat;}
    double musicalPosition()const{return current->timing?row+clamp((musicalBeat-rowStartBeat)/std::max(1e-12,rowEndBeat-rowStartBeat),0.0,1.0):row+clamp(1-rowRemaining/rowDuration,0.0,1.0);}
    void updateClock(){
        if(current->timing){
            const auto&t=*current->timing;int64_t frame=int64_t(clamp(transportFrames,0.0,double(t.maximumFrame)));
            if(st_clock_context(t.timeline.get(),frame,0,&musicalContext)!=ST_CLOCK_OK)musicalContext={};
            musicalBeat=musicalContext.beat;effectiveBpm=musicalContext.bpm;
        }else{
            effectiveBpm=current->bpm;fixedClockContext(musicalContext,transportFrames,sampleRate,effectiveBpm,current->numerator,current->denominator);musicalBeat=musicalContext.beat;
        }
        musicalContext.epoch=timingEpoch;musicalContext.loopIteration=clockLoopIteration;musicalContext.flags=(musicalContext.flags&ST_CLOCK_DOMAIN_END)|(playing?ST_CLOCK_PLAYING:0)|(recording?ST_CLOCK_RECORDING:0);
        if((songMode&&current->loopSong)||(!songMode&&current->timing)){musicalContext.flags|=ST_CLOCK_LOOPING;musicalContext.loopStartBeat=songMode?0:patternOriginBeat+patternDomainStartBeat();musicalContext.loopEndBeat=songMode?(current->timing?current->timing->beat(int64_t(songEndFrames)):songEndFrames*current->bpm/(60*sampleRate)):patternOriginBeat+patternDomainEndBeat();}
        if(songLoopActive()){musicalContext.flags|=ST_CLOCK_LOOPING;musicalContext.loopStartBeat=occurrenceBeat(current->songLoopStart);musicalContext.loopEndBeat=occurrenceBeat(songLoopLast()+1);}
        if(!playing){musicalContext.nextBoundaryFrame=std::numeric_limits<int64_t>::max();musicalContext.nextBoundaryKinds=0;musicalContext.nextQuarterBeatFrame=std::numeric_limits<int64_t>::max();}
        if(musicalContext.nextQuarterBeatFrame==std::numeric_limits<int64_t>::max())musicalContext.flags|=ST_CLOCK_GRID_PROJECTION;
    }
    void resolveMappedTimeline(bool clear=true){
        const auto&t=*current->timing;
        if(songMode){auto next=std::upper_bound(current->occurrenceFrames.begin(),current->occurrenceFrames.end(),int64_t(transportFrames));sequence=clamp(int(next-current->occurrenceFrames.begin())-1,0,int(current->sequence.size())-1);pattern=current->sequence[sequence];
            if(transportFrames>=current->occurrenceFrames.back()){sequenceFinished=true;row=current->patterns[pattern].length-1;rowRemaining=1e100;if(clear)clearScheduled();updateClock();return;}
        }else{double start=t.frame(patternOriginBeat),end=t.frame(patternOriginBeat+patternDomainEndBeat());transportFrames=clamp(transportFrames,start,end-1);}
        sequenceFinished=false;const double base=rowBaseBeat();int lo=0,hi=current->patterns[pattern].length;
        while(lo+1<hi){int mid=(lo+hi)/2;if(t.frame(base+beatForRow(*current,mid))<=transportFrames)lo=mid;else hi=mid;}
        row=lo;rowStartBeat=base+beatForRow(*current,row);rowEndBeat=base+beatForRow(*current,row+1);double start=t.frame(rowStartBeat),end=t.frame(rowEndBeat);rowDuration=end-start;rowHasStarted=transportFrames>start;rowRemaining=rowHasStarted?end-transportFrames:0;if(clear)clearScheduled();updateClock();
    }
    void publishTransport(){statusPublication.fetch_add(1,std::memory_order_seq_cst);std::atomic_thread_fence(std::memory_order_seq_cst);updateClock();statusSeekRevision.store(seekRevision,std::memory_order_relaxed);statusBeat.store(musicalBeat,std::memory_order_relaxed);statusBpm.store(effectiveBpm,std::memory_order_relaxed);statusBar.store(musicalContext.bar,std::memory_order_relaxed);statusBarStart.store(musicalContext.barStartBeat,std::memory_order_relaxed);statusNumerator.store(musicalContext.numerator,std::memory_order_relaxed);statusDenominator.store(musicalContext.denominator,std::memory_order_relaxed);statusTimingRevision.store(musicalContext.revision,std::memory_order_relaxed);statusPlaying.store(playing,std::memory_order_relaxed);statusRecording.store(recording,std::memory_order_relaxed);statusSongMode.store(songMode,std::memory_order_relaxed);statusPattern.store(pattern,std::memory_order_relaxed);statusRow.store(row,std::memory_order_relaxed);statusSequence.store(sequence,std::memory_order_relaxed);statusQueuedPattern.store(queuedPattern,std::memory_order_relaxed);statusQueuedSequence.store(queuedSequence,std::memory_order_relaxed);statusCountIn.store(countInTotal>0?int32_t(std::max<int64_t>(1,(countInTotal-countInElapsed+countInClick-1)/countInClick)):0,std::memory_order_relaxed);statusPosition.store(rowHasStarted?musicalPosition():row,std::memory_order_relaxed);statusSeconds.store(transportFrames/sampleRate,std::memory_order_relaxed);statusTransportApplied.store(transportApplied,std::memory_order_release);statusPublication.fetch_add(1,std::memory_order_seq_cst);}
    void flushQuiescent(){suppressEvents=true;frameOffset=0;refresh();finishInputAcks(false);STEvent event;while(events.pop(event)){};publishTransport();cacheValid=false;releaseClockOrigin();suppressEvents=false;automationRefreshRequested.store(true,std::memory_order_release);}
    double patternRows(const Project&p,int index)const{int n=p.patterns[index].length;return n+(n%2?p.swing:0);}
    double clipSeconds(const Project&p,const AudioClip&clip)const{if(p.timing)return double(clip.mappedEnd-clip.mappedStart)/sampleRate;auto*sample=p.audioAssets[clip.asset].sample;if(!sample)return 0;double available=std::max(0.0,double(sample->l.size())/sample->rate-clip.sourceOffset);return clip.durationBeats>0?std::min(available,clip.durationBeats*60/p.bpm):available;}
    int64_t mappedSongFrames(const Project&p)const{int64_t end=p.occurrenceFrames.empty()?0:p.occurrenceFrames.back();for(const auto&clip:p.audioClips)if(!clip.muted)end=std::max(end,clip.mappedEnd);return end;}
    double projectedSongDuration(const Project&p)const{if(p.timing)return double(mappedSongFrames(p))/sampleRate;double rows=0;for(int index:p.sequence)rows+=patternRows(p,index);double seconds=rows*60/p.bpm/p.lpb;for(const auto&clip:p.audioClips)if(!clip.muted)seconds=std::max(seconds,clip.startBeat*60/p.bpm+clipSeconds(p,clip));return seconds;}
    double songDuration(const Project&p)const{return p.frozenSongPolicy&&p.frozenSongFrames?double(p.frozenSongFrames)/sampleRate:projectedSongDuration(p);}
    // Preserve the old no-override arithmetic exactly. Explicit printed extents
    // must never roundtrip via seconds:6007/48000*48000 is greater than6007.
    double effectiveSongFrames(const Project&p)const{return p.frozenSongPolicy&&p.frozenSongFrames?double(p.frozenSongFrames):projectedSongDuration(p)*sampleRate;}
    bool projectedEndFits(const Project&p,int64_t frames)const{
        if(p.timing)return mappedSongFrames(p)<=frames;
        // Compare the integer boundary in sampled musical/source coordinates,
        // just as the immutable clock does. No epsilon and no source-length ->
        // seconds -> frames roundtrip: an exact6007-frame asset ends at6007.
        double beat=double(frames)*p.bpm/(60*sampleRate),rows=0;
        for(int index:p.sequence)rows+=patternRows(p,index);
        if(beat<rows/p.lpb)return false;
        for(const auto&clip:p.audioClips)if(!clip.muted){
            if(beat<clip.startBeat)return false;
            if(clip.durationBeats>0&&beat-clip.startBeat>=clip.durationBeats)continue;
            auto*sample=p.audioAssets[clip.asset].sample;if(!sample)continue;
            double elapsedFrames=std::fma(-clip.startBeat,(60*sampleRate)/p.bpm,double(frames));
            double sourceFrame=std::fma(elapsedFrames,sample->rate/sampleRate,clip.sourceOffset*sample->rate);
            if(sourceFrame<double(sample->l.size()))return false;
        }
        return true;
    }
    void validateFrozenExtent(const Project&p)const{
        if(!p.frozenSongFrames)return;
        if(!p.frozenSongPolicy||!p.frozenPublicationID||p.frozenSongFrames<1||p.frozenSongFrames>100000000)throw std::invalid_argument("Frozen song extent requires a matching enabled publication and1..100000000 frames.");
        if(!projectedEndFits(p,p.frozenSongFrames))throw std::invalid_argument("Frozen song extent is shorter than the projected live arrangement. Refresh or Unfreeze before changing the song end.");
        if(p.timing&&p.frozenSongFrames>p.timing->frame(1000000))throw std::invalid_argument("Frozen song extent exceeds the timing map domain.");
    }
    void resolveTimeline(){if(current->timing){resolveMappedTimeline();return;}double cursor=0,base=sampleRate*60/current->bpm/current->lpb;for(size_t s=0;s<current->sequence.size();++s){int p=current->sequence[s];double duration=patternRows(*current,p)*base;if(transportFrames<cursor+duration-1e-7){sequence=int(s);pattern=p;double local=transportFrames-cursor;for(int r=0;r<current->patterns[p].length;++r){double length=base*(1+((r&1)?-1:1)*current->swing);if(local<length-1e-7){row=r;rowDuration=length;rowRemaining=length-local;rowHasStarted=true;sequenceFinished=false;clearScheduled();return;}local-=length;}return;}cursor+=duration;}sequence=int(current->sequence.size())-1;pattern=current->sequence[sequence];row=current->patterns[pattern].length-1;sequenceFinished=true;rowRemaining=1e100;clearScheduled();}
    double seekFrames()const{if(current->timing){double base=songMode?current->occurrenceBeats[size_t(sequence)]:patternOriginBeat;return current->timing->frame(base+beatForRow(*current,row));}double rows=0;if(songMode)for(int n=0;n<sequence&&n<int(current->sequence.size());++n)rows+=patternRows(*current,current->sequence[n]);rows+=row+(row%2?current->swing:0);return rows*sampleRate*60/current->bpm/current->lpb;}
    void resetPatternClips(){patternClipValid=false;patternClipLooped=false;patternClipRow=-1;for(auto&t:patternTails)t.active=false;for(auto&t:songTails)t.active=false;}
    /* Whole-song loop: each song tail clip starts sounding its printed tail at the new pass's frame 0,
       as the live chain would ring over the loop point. The oldest tail yields when all slots ring. */
    void pushSongTails(){if(current->timing)return;for(int c=0;c<int(current->songTailClips.size());++c){SongTail*slot=nullptr;for(auto&t:songTails)if(!t.active){slot=&t;break;}if(!slot){slot=&songTails[0];for(auto&t:songTails)if(t.position>slot->position)slot=&t;}slot->active=true;slot->clip=c;slot->position=0;}}
    void mixSongTails(){for(auto&t:songTails){if(!t.active)continue;if(t.clip>=int(current->songTailClips.size())){t.active=false;continue;}const auto&clip=current->songTailClips[size_t(t.clip)];auto*sample=current->audioAssets[clip.asset].sample;if(!sample){t.active=false;continue;}double pos=double(clip.start)+(sample->rate==sampleRate?double(t.position):double(t.position)*sample->rate/sampleRate);if(pos>=double(sample->l.size())){t.active=false;continue;}++t.position;if(clip.track>=current->trackCount||(current->hostMixing&&!computationalTrackEnabled(clip.track)))continue;addChannel(clip.track,readSample(sample->l,pos)*clip.gain,readSample(sample->r,pos)*clip.gain);}}
    double patternRowFrame(int r)const{if(current->timing)return double(current->timing->frame(patternOriginBeat+beatForRow(*current,r))-current->timing->frame(patternOriginBeat));return beatForRow(*current,r)*60/current->bpm*sampleRate;}
    void restorePatternClips(const STTransportState&state){
        resetPatternClips();if(songMode||!rowHasStarted)return;
        // The old head/loop cursor is meaningful only for this restored pattern
        // and row. Ignore malformed/default checkpoint fields without importing
        // any state left by the temporary export. Frame bounds are integer and
        // include the just-finished row while its next trigger is still pending.
        const double begin=patternRowFrame(row),end=patternRowFrame(row+1);
        const bool saved=state.patternClipValid==1&&state.patternClipPattern==pattern&&
            (state.patternClipLooped==0||state.patternClipLooped==1)&&state.patternClipFrame>=0&&
            double(state.patternClipFrame)>=std::floor(begin)&&double(state.patternClipFrame)<=std::ceil(end)&&
            (state.timingVersion==0?!current->timing:(current->timing&&state.timingRevision==current->timing->revision));
        patternClipFrame=saved?state.patternClipFrame:int64_t(std::ceil(begin+clamp(rowDuration-rowRemaining,0.0,rowDuration)-1e-6));
        patternClipLooped=saved&&state.patternClipLooped!=0;patternClipPattern=pattern;
        patternClipRow=row;patternClipLength=current->patterns[pattern].length;patternClipValid=true;
    }
    void pushPatternTail(int pat,bool loop,int64_t position){PatternTail*slot=nullptr;for(auto&t:patternTails)if(!t.active){slot=&t;break;}if(!slot){slot=&patternTails[0];for(auto&t:patternTails)if(t.position>slot->position)slot=&t;}slot->active=true;slot->loop=loop;slot->pattern=pat;slot->position=position;}
    /* Called at every Pattern-mode row trigger. A natural wrap (last row -> row 0)
       restarts the pass; the same pattern then plays its steady-state loop print,
       another pattern keeps the previous print's tail sounding. Any other jump
       (Play, Seek, loop range, pattern change) re-aligns to the row's start frame. */
    void notePatternRow(){
        bool natural=patternClipValid&&row==0&&patternClipRow==patternClipLength-1;
        if(natural){if(pattern==patternClipPattern)patternClipLooped=true;else{for(const auto&clip:current->patternClips)if(clip.pattern==patternClipPattern){pushPatternTail(patternClipPattern,patternClipLooped,patternClipFrame);break;}patternClipLooped=false;}patternClipFrame=0;}
        else if(!(patternClipValid&&pattern==patternClipPattern&&row==patternClipRow+1)){if(pattern!=patternClipPattern)patternClipLooped=false;patternClipFrame=int64_t(std::ceil(patternRowFrame(row)-1e-6));}
        patternClipPattern=pattern;patternClipRow=row;patternClipLength=current->patterns[pattern].length;patternClipValid=true;
    }
    /* Accumulate into a channel's per-sample input and mark it for this sample's mix. */
    /* Raw host mixing into the bus cache: a channel's bus is exactly its summed input, so
       voices and clips add straight into the bus block (same order, same sums) instead of
       a per-sample channel pass; peaks and the mix are taken from the buses per block. */
    bool directBus=false;uint32_t directFrame=0,directFrames=0;std::vector<float> directMixL=std::vector<float>(maxBlock),directMixR=std::vector<float>(maxBlock);
    void addChannel(int channel,float l,float r){auto&T=*tb;
        if(directBus){const size_t bus=size_t(channel);float*bl=T.busLeft(channel),*br=T.busRight(channel);if(!T.busTouched[bus]){T.busTouched[bus]=1;std::fill_n(bl,directFrames,0.f);std::fill_n(br,directFrames,0.f);}if(T.hostAudible[bus]){bl[directFrame]+=l;br[directFrame]+=r;}return;}
T.left[size_t(channel)]+=l;T.right[size_t(channel)]+=r;T.touched[size_t(channel)/64]|=uint64_t(1)<<(channel%64);}
    bool mixPatternClip(int pat,bool loop,int64_t position){bool remaining=false;if(position<0)return false;for(const auto&clip:current->patternClips){if(clip.pattern!=pat)continue;int asset=loop&&clip.loopAsset>=0?clip.loopAsset:clip.headAsset;auto*sample=current->audioAssets[asset].sample;if(!sample)continue;double pos=sample->rate==sampleRate?double(position):double(position)*sample->rate/sampleRate;if(pos>=double(sample->l.size()))continue;remaining=true;if(clip.track>=current->trackCount||(current->hostMixing&&!computationalTrackEnabled(clip.track)))continue;addChannel(clip.track,readSample(sample->l,pos)*clip.gain,readSample(sample->r,pos)*clip.gain);}return remaining;}
    void mixPatternClips(){if(current->patternClips.empty()){for(auto&t:patternTails)t.active=false;}else{if(patternClipValid)mixPatternClip(patternClipPattern,patternClipLooped,patternClipFrame);for(auto&t:patternTails)if(t.active){if(mixPatternClip(t.pattern,t.loop,t.position))++t.position;else t.active=false;}}++patternClipFrame;}
    void mixClips(){if(!playing)return;if(!songMode){mixPatternClips();return;}mixSongTails();double seconds=transportFrames/sampleRate;for(const auto&clip:current->audioClips){if(clip.muted||clip.track>=current->trackCount||(current->hostMixing&&!computationalTrackEnabled(clip.track)))continue;auto*sample=current->audioAssets[clip.asset].sample;if(!sample)continue;double elapsed=current->timing?(transportFrames-clip.mappedStart)/sampleRate:seconds-clip.startBeat*60/current->bpm;if(elapsed<0)continue;double duration=clipSeconds(*current,clip);if(elapsed>=duration)continue;double pos=(clip.sourceOffset+elapsed)*sample->rate;if(pos>=sample->l.size())continue;float gain=clip.gain;if(clip.fadeIn>0)gain*=float(std::min(1.0,elapsed/clip.fadeIn));if(clip.fadeOut>0)gain*=float(std::min(1.0,(duration-elapsed)/clip.fadeOut));addChannel(clip.track,readSample(sample->l,pos)*gain,readSample(sample->r,pos)*gain);}}
    void finishSong(){if(songLoopActive()){loopSongSection(int(current->sequence.size()),true);return;}freezeTransition(current->loopSong?ST_FREEZE_CAUSE_LOOP:ST_FREEZE_CAUSE_NATURAL_END,current->loopSong,!current->loopSong);++timingEpoch;if(current->loopSong){++clockLoopIteration;inputLoopBeats+=current->timing?current->timing->beat(int64_t(songEndFrames)):songEndFrames*current->bpm/(60*sampleRate);}if(current->timing){loopBeatOffset=0;clearScheduled();}if(current->loopSong){pushSongTails();sequence=0;pattern=validPattern(current->sequence[0]);row=0;rowRemaining=0;rowHasStarted=false;sequenceFinished=false;transportFrames=0;clickBeat=-1;resetTrackState();allOff(false,true);}else{playing=false;clearPluginLocks();allOff(false,true);clearScheduled();sequence=int(current->sequence.size())-1;row=current->patterns[pattern].length-1;}}
    int validPattern(int p)const{return clamp(p,0,int(current->patterns.size()-1));}
    void clearScheduled(){for(auto&s:scheduled)s.active=false;scheduledMasks.fill(0);scheduledCount=0;scheduledGroupsUsed=0;}
    Scheduled* scheduledSlot(){for(size_t group=0;group<scheduledMasks.size();++group){uint64_t available=~scheduledMasks[group];if(available){int bit=__builtin_ctzll(available);scheduledMasks[group]|=uint64_t(1)<<bit;scheduledGroupsUsed=std::max(scheduledGroupsUsed,group+1);return &scheduled[group*64+bit];}}return nullptr;}
    void clearEffects(){delayToneL=delayToneR=0;std::fill(preL.begin(),preL.end(),0.f);std::fill(preR.begin(),preR.end(),0.f);prePos=0;std::fill(delayL.begin(),delayL.end(),0);std::fill(delayR.begin(),delayR.end(),0);for(auto&c:combL)c.clear();for(auto&c:combR)c.clear();for(auto&a:apL)a.clear();for(auto&a:apR)a.clear();}
    void externalOff(Voice &v){if(v.external&&!v.offSent){STEvent e{};e.type=ST_EVENT_NOTE_OFF;e.track=v.track;e.instrument=v.instrument;e.note=v.note;e.slot=v.sequenced?1:0;emit(e);v.offSent=true;}}
    void kill(Voice &v){if(!v.active)return;externalOff(v);if(v.sample)v.sample->refs.fetch_sub(1,std::memory_order_release);v.sample=nullptr;v.active=false;size_t index=size_t(&v-voices.data());activeVoiceMasks[index/64]&=~(uint64_t(1)<<(index%64));}
    void release(Voice &v){if(!v.active||v.releasing)return;externalOff(v);v.releasing=true;v.envStage=3;v.releaseStart=v.env;v.elapsed=0;v.filterEnvStage=3;v.filterReleaseStart=v.filterEnv;v.filterElapsed=0;if(v.external)kill(v);}
    void allOff(bool immediate,bool playbackOnly=false){for(auto&v:voices)if(v.active&&(!playbackOnly||v.sequenced)){if(immediate)kill(v);else release(v);}}
    bool computationalTrackEnabled(int track)const{return current->trackComputational(track);}
    bool externalTrackEnabled(int track,int type)const{if(current->hostMixing&&type==ST_INSTRUMENT_AU)return computationalTrackEnabled(track);return !channelMix(track).mute&&(!current->anySolo||channelMix(track).solo);}
    void noteOn(int track,int instrument,int note,int velocity,double gate,float offset,int reverse,float glide,int arp,bool fromSequencer=false,int slice=-1,bool preview=false,float previewPitch=0,double mappedEnd=-1,int sequenceTrack=-1){
        if(!channelEnabled(track)||!current->hasInstrument(instrument)||note<0||note>127)return;
        if(fromSequencer&&sequenceTrack<0)sequenceTrack=track;
        // A zero-velocity hit is a note-off for this lane, matching explicit OFF/CUT ownership.
        if(velocity<=0){for(auto&v:voices)if(v.active&&v.sequenced==fromSequencer&&v.preview==preview&&v.track==track&&v.note==note&&(!fromSequencer||v.sequenceTrack==sequenceTrack))release(v);return;}
        auto&instrumentData=current->instruments[size_t(instrument)];
        // A track's own choke group and voice mode win over its instrument's. Track choke groups are
        // scoped to the track's group (scope); instrument choke groups choke any track.
        const Project::TrackVoice trackVoice=size_t(track)<current->trackVoices.size()?current->trackVoices[size_t(track)]:Project::TrackVoice{};
        const int voiceMode=trackVoice.mode>=0?trackVoice.mode:instrumentData.p.voiceMode;
        const int chokeGroup=trackVoice.choke>0?trackVoice.choke:instrumentData.p.chokeGroup;
        const int chokeScope=trackVoice.choke>0?trackVoice.scope:-1;
        if(instrumentData.p.type==ST_INSTRUMENT_AU||instrumentData.p.type==ST_INSTRUMENT_MIDI){
            // MIDI 1.0 has no source ownership on a shared channel/pitch. Suppress
            // colliding previews; a real note first ends the pitch it retriggers (preview or not), so the
            // earlier voice's later note-off cannot cut the new note short in the plug-in.
            for(auto&v:voices)if(v.active&&v.external&&v.track==track&&v.instrument==instrument&&v.note==note){if(preview&&!v.preview)return;if(!preview)kill(v);}
        }
        if(!preview&&slice<0&&voiceMode==2)for(auto&v:voices)if(v.active&&!v.releasing&&!v.preview&&!v.explicitSlice&&v.track==track&&v.instrument==instrument){
            // MIDI legato: a plug-in or hardware synth slurs only when the new note starts before the old one
            // ends, so its note-off follows the new note-on (off first would make legato the same as mono).
            // The extra note-on is admitted only while the queue still holds a note-off for every voice.
            const bool slur=v.external&&!v.offSent&&v.note!=note&&externalTrackEnabled(track,instrumentData.p.type)&&events.available()>uint32_t(maxVoices)+1;
            STEvent off{};off.type=ST_EVENT_NOTE_OFF;off.track=v.track;off.instrument=v.instrument;off.note=v.note;off.slot=v.sequenced?1:0;
            if(!slur)externalOff(v);v.sequenced=fromSequencer;v.sequenceTrack=sequenceTrack;v.note=note;v.velocity=clamp(velocity,0,127)/127.0f;v.gate=gate;v.mappedGateEnd=mappedEnd;v.onsetFrame=totalFrames;v.offSent=false;if(v.external){if(!externalTrackEnabled(track,instrumentData.p.type)){v.external=false;kill(v);return;}STEvent e{};e.type=ST_EVENT_NOTE_ON;e.track=track;e.instrument=instrument;e.note=note;e.velocity=velocity;e.slot=fromSequencer?1:0;const bool started=emit(e);if(slur)emit(off);if(!started){v.offSent=true;kill(v);}}return;}
        // Tracker note lifetime: a new sequenced note releases the lane's gateless notes (note-off with
        // their release envelope). Notes of the same onset (a chord) leave each other sounding.
        if(fromSequencer&&!preview)for(auto&v:voices)if(v.active&&!v.releasing&&v.sequenced&&v.sequenceTrack==sequenceTrack&&v.gate<0&&v.mappedGateEnd<0&&v.onsetFrame<totalFrames)release(v);
        // Mono cuts the track's earlier notes: a chord (different pitches starting on the same frame) stays whole, and a
        // sequenced note cuts only its own note column (another column's note keeps sounding).
        auto monoCuts=[&](const Voice&v){return voiceMode>0&&v.track==track&&v.instrument==instrument&&(v.onsetFrame<totalFrames||v.note==note)
            &&(!fromSequencer||!v.sequenced||v.sequenceTrack==sequenceTrack);};
        if(!preview)for(auto&v:voices)if(v.active&&(monoCuts(v)||(chokeGroup>0&&v.chokeScope==chokeScope&&v.chokeGroup==chokeGroup)))kill(v);
        Voice* voice=nullptr;for(auto&v:voices)if(!v.active){voice=&v;break;}if(!voice){if(preview){droppedEvents.fetch_add(1,std::memory_order_relaxed);return;}voice=&voices[0];for(auto&v:voices)if(v.age<voice->age)voice=&v;kill(*voice);}
        auto&i=current->instruments[instrument];auto&v=*voice;v=Voice{};v.active=true;size_t voiceIndex=size_t(voice-voices.data());activeVoiceMasks[voiceIndex/64]|=uint64_t(1)<<(voiceIndex%64);v.sequenced=fromSequencer;v.preview=preview;v.explicitSlice=slice>=0;v.track=track;v.sequenceTrack=sequenceTrack;v.chokeGroup=chokeGroup;v.chokeScope=chokeScope;v.instrument=instrument;v.note=note;v.age=++serial;v.p=i.p;v.velocity=clamp(velocity,0,127)/127.0f;v.gate=gate;v.mappedGateEnd=mappedEnd;v.onsetFrame=totalFrames;v.glide=glide;v.arp=arp;v.rng=(rng+=0x9e3779b9u)|1u;v.lfoRng=(v.rng^0x7f4a7c15u)|1u;v.lfo2Rng=(v.rng^0x517cc1b7u)|1u;v.lfoRandom=randomBipolar(v.lfoRng);v.lfo2Random=randomBipolar(v.lfo2Rng);v.sample=i.sample;v.external=v.p.type==ST_INSTRUMENT_AU||v.p.type==3;
        applyLiveParams(v);instrumentAutomation(v,false);if(!i.zones.empty()){v.sample=nullptr;for(const auto&zone:i.zones)if(note>=zone.keyLow&&note<=zone.keyHigh&&velocity>=zone.velocityLow&&velocity<=zone.velocityHigh){v.sample=zone.sample;v.zoneRoot=zone.rootNote;v.zoneTune=zone.tune;v.zoneVolume=zone.volume;v.p.rootNote=zone.rootNote;v.p.tune+=zone.tune;v.p.volume*=zone.volume;v.zonePan=zone.pan;v.p.pan=clamp(v.p.pan+v.zonePan,-1.f,1.f);if(zone.loopEnabled>=0)v.p.mode=zone.loopEnabled?ST_MODE_FORWARD:ST_MODE_ONESHOT;if(zone.hasRegion){v.zoneRegion=true;v.zoneStart=zone.start;v.zoneEnd=zone.end;v.p.start=zone.start;v.p.end=zone.end;v.p.loopStart=zone.loopStart;v.p.loopEnd=zone.loopEnd;}break;}}
        v.cycleWidth=size_t(v.p.wavetableFrameSize>0?v.p.wavetableFrameSize:i.cycleFrames>0?i.cycleFrames:ST_WAVETABLE_DEFAULT_FRAME);v.positionSmooth=clamp(double(v.p.wavetablePosition),0.0,1.0);
        v.zoneTune+=previewPitch;v.p.tune+=previewPitch;if(slice>=0&&v.p.type==ST_INSTRUMENT_SAMPLE)v.p.mode=ST_MODE_SLICE;v.volumeSmooth=v.p.volume;v.panSmooth=v.p.pan;v.cutoffSmooth=v.p.cutoff;if(v.sample)v.sample->refs.fetch_add(1,std::memory_order_relaxed);
        if(v.external){if(!externalTrackEnabled(track,instrumentData.p.type)){v.external=false;kill(v);return;}STEvent e{};e.type=ST_EVENT_NOTE_ON;e.track=track;e.instrument=instrument;e.note=note;e.velocity=velocity;e.slot=fromSequencer?1:0;if(!emit(e)){v.offSent=true;kill(v);}return;}
        if(!v.sample){kill(v);return;}
        if(v.sample){double n=v.sample->l.size();v.begin=clamp(double(v.p.start)*n,0.0,n-1);v.end=clamp(double(v.p.end)*n,v.begin+1,n);v.loopBegin=clamp(double(v.p.loopStart)*n,v.begin,v.end-1);v.loopEnd=clamp(double(v.p.loopEnd)*n,v.loopBegin+1,v.end);
            if(v.p.mode==ST_MODE_SLICE){int count=clamp(v.p.sliceCount,1,128);int selected=slice>=0?clamp(slice,0,count-1):((note-v.p.rootNote)%count+count)%count;double width=(v.end-v.begin)/count;if(i.slices.empty()){v.begin+=selected*width;v.end=v.begin+width;}else{selected=slice>=0?clamp(slice,0,int(i.slices.size())-1):((note-v.p.rootNote)%int(i.slices.size())+int(i.slices.size()))%int(i.slices.size());double lo=v.begin,range=v.end-v.begin;v.begin=lo+i.slices[selected]*range;v.end=lo+(selected+1<int(i.slices.size())?i.slices[selected+1]:1.f)*range;}v.loopBegin=v.begin;v.loopEnd=v.end;}
            v.dir=(v.p.mode==ST_MODE_REVERSE)?-1:1;if(reverse>=0)v.dir=reverse?-1:1;v.pos=v.dir>0?v.begin+offset*(v.end-v.begin-1):v.end-1-offset*(v.end-v.begin-1);v.declickIn=v.dir<0||v.pos>=1;
            updateSampleBounds(v);}
    }
    void scheduleMapped(int track,int instrument,int note,int velocity,double onset,double end,float offset,int reverse,float glide,int arp,int slice,int sequenceTrack=-1){
        if(auto*slot=scheduledSlot()){auto&s=*slot;s={};s.active=true;s.mapped=true;s.onsetBeat=onset+loopBeatOffset;s.endBeat=end+loopBeatOffset;s.track=track;s.instrument=instrument;s.note=note;s.velocity=velocity;s.offset=offset;s.reverse=reverse;s.glide=glide;s.arp=arp;s.slice=slice;s.sequenceTrack=sequenceTrack;s.open=end<0;++scheduledCount;return;}droppedEvents.fetch_add(1,std::memory_order_relaxed);
    }
    void schedule(int track,int instrument,int note,int velocity,double delay,double gate,float offset,int reverse,float glide,int arp,int slice=-1,int sequenceTrack=-1){if(auto*slot=scheduledSlot()){auto&s=*slot;s={true,totalFrames+std::round(delay),std::round(gate),track,instrument,note,velocity,offset,glide,reverse,arp,slice};s.sequenceTrack=sequenceTrack;++scheduledCount;return;}droppedEvents.fetch_add(1,std::memory_order_relaxed);}
    void previewStep(const Command&command){
        for(auto&v:voices)if(v.active&&v.preview)kill(v);
        const auto&step=command.step;if(step.note<0||step.velocity<=0||!channelEnabled(command.a))return;
        int instrument=step.instrument>=0?step.instrument:channelMix(command.a).instrument,slice=-1,reverse=-1;float offset=0,pitch=0;
        for(auto fx:{step.fx1,step.fx2})switch(fx.type){case ST_FX_SLICE:slice=int(clamp(finite(fx.value,1),1.f,128.f))-1;break;case ST_FX_REVERSE:reverse=fx.value>=.5f;break;case ST_FX_OFFSET:offset=clamp(finite(fx.value),0.f,.9999f);break;case ST_FX_PITCH:pitch=clamp(finite(fx.value),-48.f,48.f);break;default:break;}
        for(int n=0;n<command.chordCount;++n)noteOn(command.a,instrument,step.note+command.chord[n],step.velocity,sampleRate*command.value,offset,reverse,0,0,false,slice,true,pitch);
    }
    void applyFX(int track,const STStepFX&fx){auto&t=tb->trackState[size_t(track)];float x=finite(fx.value);switch(fx.type){case ST_FX_VOLUME:t.volume=clamp(x,0.f,2.f);break;case ST_FX_PAN:t.pan=clamp(x,-1.f,1.f);break;case ST_FX_PITCH:t.pitch=clamp(x,-48.f,48.f);break;case ST_FX_CUTOFF:t.cutoff=clamp(x,20.f,22000.f);break;case ST_FX_RESONANCE:t.resonance=clamp(x,0.f,.99f);break;case ST_FX_DELAY:t.delay=clamp(x,0.f,1.f);break;case ST_FX_REVERB:t.reverb=clamp(x,0.f,1.f);break;case ST_FX_POSITION:{
            // Wavetable position for this track's voices, as a row lock on the automation target.
            // Granular: the same value is the grain position.
            const int slot=ST_TARGET_WAVETABLE_POSITION-ST_TARGET_ATTACK;t.instrument[size_t(slot)]=clamp(x,0.f,1.f);t.instrumentMask|=uint64_t(1)<<slot;t.rowLocks|=uint64_t(1)<<ST_TARGET_WAVETABLE_POSITION;
            const int grain=ST_TARGET_GRAIN_POSITION-ST_TARGET_ATTACK;t.instrument[size_t(grain)]=clamp(x,0.f,1.f);t.instrumentMask|=uint64_t(1)<<grain;t.rowLocks|=uint64_t(1)<<ST_TARGET_GRAIN_POSITION;break;}
        default:break;}int target=-1;switch(fx.type){case ST_FX_VOLUME:target=ST_TARGET_VOLUME;break;case ST_FX_PAN:target=ST_TARGET_PAN;break;case ST_FX_PITCH:target=ST_TARGET_PITCH;break;case ST_FX_CUTOFF:target=ST_TARGET_CUTOFF;break;case ST_FX_RESONANCE:target=ST_TARGET_RESONANCE;break;case ST_FX_DELAY:target=ST_TARGET_DELAY;break;case ST_FX_REVERB:target=ST_TARGET_REVERB;break;}if(target>=0){t.rowLocks|=uint64_t(1)<<target;STEvent e{};e.type=ST_EVENT_PARAMETER;e.track=track;e.target=target;e.value=x;emit(e);}}
    // Resolved locks contain only numeric dispatch data. These bounded arrays are
    // rebuilt at row boundaries; no project identity lookup or allocation occurs
    // on the render thread. Duplicate columns coalesce so FX2 wins before delivery.
    void addPluginLock(int track,const PluginStepFX&fx){
        if(!fx.present)return;
        auto&locks=tb->pluginRowLocks;
        for(int n=0;n<pluginRowLockCount;++n){auto&lock=locks[size_t(n)];if(lock.track==track&&lock.fx.slot==fx.slot&&lock.fx.address==fx.address){lock.fx=fx;lock.delivery=LaneEventState{};return;}}
        if(pluginRowLockCount<int(locks.size()))locks[size_t(pluginRowLockCount++)]={fx,track,{}};
    }
    bool pluginLocked(const Lane&lane,bool host)const{
        for(int n=0;n<pluginRowLockCount;++n){const auto&lock=tb->pluginRowLocks[size_t(n)];if(lock.fx.address!=lane.address)continue;
            if(host?(lock.fx.destination>=0&&lock.fx.destination==lane.target):(lane.target==ST_TARGET_PLUGIN&&lock.track==lane.track&&lock.fx.slot==lane.slot))return true;}
        return false;
    }
    void indexPluginLocks(){
        // Resolve the at-most256 lock keys against lane addresses once per row,
        // rather than repeating the key search for every lane on every sample.
        const auto&p=current->patterns[pattern];const auto&lanes=p.laneList(),&hostLanes=p.hostLaneList();for(size_t n=0;n<lanes.size();++n)pluginLaneLocked[n]=pluginLocked(lanes[n],false);
        for(size_t n=0;n<hostLanes.size();++n)pluginHostLaneLocked[n]=pluginLocked(hostLanes[n],true);
        pluginLocksPending=pluginRowLockCount;
    }
    void emitPluginLocks(){
        if(!pluginLocksPending)return;
        const uint64_t frame=uint64_t(totalFrames);
        for(int n=0;n<pluginRowLockCount;++n){auto&lock=tb->pluginRowLocks[size_t(n)];auto&state=lock.delivery;if(state.delivered||(state.known&&frame<state.retryFrame))continue;
            STEvent event{};event.type=lock.fx.destination>=0?ST_EVENT_HOST_PARAMETER:ST_EVENT_PARAMETER;event.track=lock.fx.destination>=0?-1:lock.track;
            event.target=lock.fx.destination>=0?lock.fx.destination:ST_TARGET_PLUGIN;event.slot=lock.fx.slot;event.address=lock.fx.address;event.value=lock.fx.value;if(lock.fx.destination>=0)event.hostMap=current->hostMap;
            state.known=true;state.delivered=emit(event);state.retryFrame=frame+32;if(state.delivered)--pluginLocksPending;
        }
    }
    // Note FX of a row (column 0's FX 1/FX 2), shared by every note column of the track on that row.
    struct RowFX { float probability=1,offset=0,micro=0,glide=0;int ratchet=1,reverse=-1,arp=0,slice=-1,roll=-1; };
    void cancelScheduled(int lane){if(scheduledCount)for(size_t group=0;group<scheduledGroupsUsed;++group){uint64_t pending=scheduledMasks[group];while(pending){const int bit=__builtin_ctzll(pending);pending&=pending-1;auto&q=scheduled[group*64+bit];if(q.active&&q.sequenceTrack==lane){q.active=false;scheduledMasks[group]&=~(uint64_t(1)<<bit);--scheduledCount;}}}}
    /* Extra note columns of track t on this row: each is its own lane on the track's channel. OFF/CUT end only
       that column's notes (and its queued hits); a note releases only that column's gateless notes (noteOn) and
       takes the row's note FX (ratchet, probability with one roll per row, microtiming, offset, reverse, glide,
       arpeggio, slice); channel FX already applied through column 0. */
    void triggerColumns(int t,const Pattern&p,int row,RowFX fx){static const std::array<int32_t,ST_MAX_CHORD> unison{};
        for(int c=1;c<=extraColumns;++c){const Step&s=p.columnStep(t,c,row);if(!s.present)continue;const auto&sp=s.p;const int lane=t+c*ST_TRACK_SANITY_LIMIT;
            if(sp.note==-1||sp.note==-2){cancelScheduled(lane);for(auto&v:voices)if(v.active&&v.sequenced&&v.sequenceTrack==lane){if(sp.note==-2)kill(v);else release(v);}continue;}
            if(sp.note<0)continue;
            if(fx.roll<0)fx.roll=fx.probability>=1||(randomBipolar(rng)+1)*.5f<fx.probability;if(!fx.roll)continue;
            if(fx.ratchet<=1&&consumeEarlyInput(t,sp.note,row))continue;
            scheduleHits(lane,t,sp.instrument>=0?sp.instrument:current->tracks[size_t(t)].instrument,sp,1,unison,fx.ratchet,fx.micro,fx.offset,fx.reverse,fx.glide,fx.arp,fx.slice);}
    }
    void triggerStep(int t,const Step&s,const Pattern&p,int row){const bool columns=p.hasExtraColumns(t);if(!s.present){if(columns)triggerColumns(t,p,row,RowFX{});return;}const auto&sp=s.p;const int channel=t;for(const auto&fx:s.pluginFX)addPluginLock(channel,fx);
            if(sp.note<0&&sp.note!=-3){
                clearLaneRatchets(t);
                // OFF/CUT also cancel delayed triggers already queued for this lane (column 0 only).
                cancelScheduled(t);
                for(auto&v:voices)if(v.active&&v.sequenced&&v.sequenceTrack==t){if(sp.note==-2)kill(v);else release(v);}if(columns)triggerColumns(t,p,row,RowFX{});return;}
            bool hasRatchet=sp.fx1.type==ST_FX_RATCHET||sp.fx2.type==ST_FX_RATCHET;
            bool hasEffect=sp.fx1.type!=ST_FX_NONE||sp.fx2.type!=ST_FX_NONE;
            for(const auto&fx:s.pluginFX)hasEffect=hasEffect||fx.present;
            // A cell with neither note nor effect (left behind when its last effect is removed) is empty: a sticky ratchet continues.
            if(sp.note==-3&&!hasEffect){if(columns)triggerColumns(t,p,row,RowFX{});return;}
            auto&held=tb->ratchet(t);
            RowFX row_{};auto&probability=row_.probability;auto&offset=row_.offset;auto&micro=row_.micro;auto&glide=row_.glide;
            // A row's own Ratchet sets the rate; a new note or another effect row plays once (the sticky
            // rate then stops). Only empty rows continue a held ratchet (see triggerRow).
            auto&ratchet=row_.ratchet;auto&reverse=row_.reverse;auto&arp=row_.arp;auto&slice=row_.slice;bool hasOffset=false,hasGlide=false,hasArp=false;
            for(auto fx:{sp.fx1,sp.fx2})switch(fx.type){case ST_FX_SLICE:slice=int(clamp(finite(fx.value,1),1.f,128.f))-1;break;case ST_FX_PROBABILITY:probability=clamp(finite(fx.value),0.f,1.f);break;case ST_FX_RATCHET:ratchet=int(clamp(finite(fx.value,1),1.f,8.f));break;case ST_FX_REVERSE:reverse=fx.value>=.5f;break;case ST_FX_OFFSET:offset=clamp(finite(fx.value),0.f,.9999f);hasOffset=true;break;case ST_FX_GLIDE:glide=clamp(finite(fx.value),-48.f,48.f);hasGlide=true;break;case ST_FX_MICROTIMING:micro=clamp(finite(fx.value),0.f,.95f);break;case ST_FX_ARPEGGIO:arp=int(clamp(finite(fx.value),0.f,255.f));hasArp=true;break;default:applyFX(channel,fx);}
            if(sp.note==-3){
                held.handled=true;held.rate=ratchet;
                // Effect-only sample commands operate on the existing playhead,
                // preserving the envelope and pitch. Offset is relative to playback direction.
                // They steer every note column's sounding voices of the track.
                for(auto&v:voices)if(v.active&&v.sequenced&&laneTrack(v.sequenceTrack)==t&&v.track==channel&&v.sample){
                    if(reverse>=0&&v.dir!=(reverse?-1:1)){v.sampleTransition=true;v.dir=reverse?-1:1;}
                    if(hasOffset){v.sampleTransition=true;v.pos=v.dir>0?v.begin+offset*(v.end-v.begin-1):v.end-1-offset*(v.end-v.begin-1);v.looped=false;v.grainPhase=0;}
                }
                // The same commands steer the held note's repeats, from this row on.
                if(reverse>=0)held.reverse=reverse;if(slice>=0)held.slice=slice;if(hasOffset)held.offset=offset;if(hasGlide)held.glide=glide;if(hasArp)held.arp=arp;
                if(hasRatchet)repeatRatchet(t,channel,held);if(columns)triggerColumns(t,p,row,row_);return;
            }
            // Roll before touching the sticky state: a dropped note leaves the row as if it held no note.
            // One roll per row: the extra note columns follow it.
            row_.roll=(randomBipolar(rng)+1)*.5f<probability||probability>=1; // the draw can round up to exactly 1
            if(!row_.roll){if(columns)triggerColumns(t,p,row,row_);return;}
            held.handled=true;held.rate=ratchet;
            if(sp.velocity<=0){held=RatchetState{};}
            else{held.known=true;held.step=sp;held.count=s.count;held.chord=s.chord;held.instrument=sp.instrument>=0?sp.instrument:current->tracks[size_t(t)].instrument;held.reverse=reverse;held.offset=offset;held.slice=slice;held.glide=glide;held.arp=arp;}
            int inst=sp.instrument>=0?sp.instrument:current->tracks[size_t(t)].instrument;
            if(!(ratchet<=1&&s.count<=1&&consumeEarlyInput(t,sp.note,row)))scheduleHits(t,channel,inst,sp,s.count,s.chord,ratchet,micro,offset,reverse,glide,arp,slice);
            if(columns)triggerColumns(t,p,row,row_);
    }
    /* Gate is in rows of the hit spacing. No gate (<=0) is tracker semantics: the note sounds until the
       next note on its lane (noteOn releases it), an OFF/CUT, the end of a one-shot, or stop. Ratchet
       repeats of a gateless note each last one repeat spacing. */
    void scheduleHits(int t,int channel,int inst,const STStep&sp,int count,const std::array<int32_t,ST_MAX_CHORD>&chord,int ratchet,float micro,float offset,int reverse,float glide,int arp,int slice){double spacing=rowDuration*(1-micro)/ratchet;
            const bool open=!(sp.gate>0)&&ratchet<=1;const double gate=sp.gate>0?clamp(double(sp.gate),.01,256.0):1.0;
            for(int r=0;r<ratchet;++r)for(int n=0;n<count;++n){if(current->timing){double beatSpan=rowEndBeat-rowStartBeat,beatSpacing=beatSpan*(1-micro)/ratchet;double onset=rowStartBeat+beatSpan*micro+r*beatSpacing;scheduleMapped(channel,inst,sp.note+chord[n],sp.velocity,onset,open?-1:onset+beatSpacing*gate,offset,reverse,glide,arp,slice,t);}else schedule(channel,inst,sp.note+chord[n],sp.velocity,rowDuration*micro+r*spacing,open?-1:std::max(1.0,spacing*gate),offset,reverse,glide,arp,slice,t);}
    }
    // Replays the held note at its sticky rate with its own resolved playback values (no microtiming: repeats fill the row).
    void clearLaneRatchets(int t){tb->laneRatchets[size_t(t)]=RatchetState{};}
    void repeatRatchet(int t,int channel,const RatchetState&r){if(!r.known||r.rate<=1)return;int inst=r.instrument;scheduleHits(t,channel,inst,r.step,r.count,r.chord,r.rate,0,r.offset,r.reverse,r.glide,r.arp,r.slice);}
    void triggerRow(){rowHasStarted=true;if(!songMode)notePatternRow();clearPluginLocks();for(auto&t:tb->trackState)t.rowLocks=0;auto&p=current->patterns[pattern];if(current->timing){rowStartBeat=rowBaseBeat()+beatForRow(*current,row);rowEndBeat=rowBaseBeat()+beatForRow(*current,row+1);double end=current->timing->frame(rowEndBeat);rowDuration=end-current->timing->frame(rowStartBeat);rowRemaining=end-transportFrames;}else{rowDuration=sampleRate*60.0/current->bpm/current->lpb*(1.0+((row&1)?-1:1)*current->swing);rowRemaining+=rowDuration;}
        for(auto&r:tb->laneRatchets)r.handled=false;
        for(int t=0;t<current->trackCount;++t){triggerStep(t,p.step(t,row),p,row);
            // A lane's sticky ratchets live on its own channel.
            auto&r=tb->laneRatchets[size_t(t)];if(!r.handled)repeatRatchet(t,t,r);
        }
        // An early note is skipped only by the very next row trigger.
        forgetEarlyInputs();
        indexPluginLocks();
    }
    /* Queued jump: Song mode resolves a sequence index (or the first occurrence of a pattern), Pattern mode a
       pattern. Ignored while stopped; Play/Stop/Seek/panic clear it. */
    void queueJump(int target,int index){queuedPattern=queuedSequence=queuedMode=-1;if(!playing||(target<0&&index<0))return;int count=int(current->sequence.size());
        if(songMode){if(index<0||index>=count){index=-1;for(int n=0;n<count;++n)if(current->sequence[n]==target){index=n;break;}}if(index<0)return;queuedSequence=index;queuedPattern=validPattern(current->sequence[index]);}
        else{if(target<0)target=index<count?current->sequence[index]:-1;if(target<0||target>=int(current->patterns.size()))return;queuedPattern=target;}}
    /* At a pattern (or loop-range) end: continue sample-accurately in the queued pattern/sequence position.
       fromBeat is the pattern-relative beat being left. Voices keep sounding, as at a natural pattern change. */
    /* Song/Pattern mode change while playing, taken at the next pattern (or loop-range) end like a queued
       jump. To Pattern mode: loop `target` (the host makes it the active pattern). To Song mode: continue at
       sequence index `index`. Voices keep sounding; the position restarts exactly as a seek there would. */
    void queueModeSwitch(bool song,int target,int index){queuedPattern=queuedSequence=queuedMode=-1;if(!playing||song==songMode)return;
        if(song){if(index<0||index>=int(current->sequence.size()))return;queuedSequence=index;queuedPattern=validPattern(current->sequence[index]);}
        else{if(target<0||target>=int(current->patterns.size()))return;queuedPattern=target;}
        queuedMode=song?1:0;}
    bool takeModeSwitch(){bool song=queuedMode==1;int target=queuedPattern,index=queuedSequence;queuedPattern=queuedSequence=queuedMode=-1;
        songMode=song;if(song){sequence=clamp(index,0,int(current->sequence.size())-1);pattern=validPattern(current->sequence[sequence]);}else pattern=validPattern(target);
        row=patternLoopActive()?loopStartRow:0;resetPatternClips();++timingEpoch;clockLoopIteration=0;loopBeatOffset=0;patternOriginBeat=current->patternOriginBeat;
        sequenceFinished=false;resetTrackState();transportFrames=seekFrames();alignFrozenOrigin();freezeTransition(ST_FREEZE_CAUSE_SEEK);clickBeat=-1;return true;}
    bool takeQueuedJump(double fromBeat){if(queuedPattern<0)return false;if(queuedMode>=0)return takeModeSwitch();int target=queuedPattern,index=queuedSequence;queuedPattern=queuedSequence=-1;
        if(songMode){if(index<0||index>=int(current->sequence.size()))return false;loopSongSection(sequence+1,false,index);if(current->frozenSongPolicy)alignFrozenOrigin();return true;}
        pattern=validPattern(target);int start=patternLoopActive()?loopStartRow:0;double to=beatForRow(*current,start);freezeTransition(ST_FREEZE_CAUSE_LOOP);
        if(current->timing){loopBeatOffset+=fromBeat-to;inputLoopBeats+=fromBeat-to;transportFrames=current->timing->frame(patternOriginBeat+to);++timingEpoch;++clockLoopIteration;clickBeat=-1;}
        row=start;resetTrackState();return true;}
    void advanceRow(){
        // Pattern loop range: at (or past) its last row, continue sample-accurately at its first row.
        if(patternLoopActive()&&row>=patternLoopEnd()){if(takeQueuedJump(beatForRow(*current,row+1)))return;double from=beatForRow(*current,row+1),to=beatForRow(*current,loopStartRow);row=loopStartRow;freezeTransition(ST_FREEZE_CAUSE_LOOP);if(current->timing){loopBeatOffset+=from-to;inputLoopBeats+=from-to;transportFrames=current->timing->frame(patternOriginBeat+to);++timingEpoch;++clockLoopIteration;clickBeat=-1;}return;}
        if(++row<current->patterns[pattern].length)return;row=0;if(takeQueuedJump(patternRows(*current,pattern)/current->lpb))return;if(songLoopActive()&&sequence==songLoopLast()){loopSongSection(sequence+1,false);return;}if(!songMode){freezeTransition(ST_FREEZE_CAUSE_LOOP);if(current->timing){loopBeatOffset+=patternRows(*current,pattern)/current->lpb;inputLoopBeats+=patternRows(*current,pattern)/current->lpb;transportFrames=current->timing->frame(patternOriginBeat);++timingEpoch;++clockLoopIteration;clickBeat=-1;}return;}
        if(++sequence>=int(current->sequence.size())){sequence=int(current->sequence.size())-1;if(current->frozenSongPolicy?transportFrames<std::ceil(songEndFrames):transportFrames+1e-6<songEndFrames){sequenceFinished=true;clearPluginLocks();row=current->patterns[pattern].length-1;rowRemaining=1e100;return;}finishSong();return;}pattern=validPattern(current->sequence[sequence]);resetTrackState();
    }
    double laneValue(const Lane&l,double pos){if(l.points.empty())return 0;if(current->timing&&l.interpolation==1){auto next=std::upper_bound(l.points.begin(),l.points.end(),musicalBeat,[&](double beat,const Point&point){int whole=int(std::floor(point.row));double start=beatForRow(*current,whole),end=beatForRow(*current,whole+1);return beat<rowBaseBeat()+start+(point.row-whole)*(end-start);});return next==l.points.begin()?l.points.front().value:(next-1)->value;}if(l.interpolation==1)pos+=1e-10;if(pos<=l.points.front().row)return l.points.front().value;auto it=std::upper_bound(l.points.begin(),l.points.end(),pos,[](double r,const Point&p){return r<p.row;});if(it==l.points.end())return l.points.back().value;auto&a=*(it-1);auto&b=*it;if(l.interpolation==1||b.row==a.row)return a.value;double result=a.value+(b.value-a.value)*(pos-a.row)/(b.row-a.row);if(std::isfinite(result))return result;double fraction=(pos-a.row)/(b.row-a.row);return a.value*(1-fraction)+b.value*fraction;}
    void emitLaneAutomation(const Lane&l,LaneEventState&state,double value,int eventType){bool first=!state.known;bool stepped=l.interpolation==1;uint64_t frame=uint64_t(totalFrames);
        if(first||(stepped&&value!=state.value)){state.known=true;state.value=value;state.delivered=false;state.retryFrame=frame;}
        bool due=stepped?(!state.delivered&&frame>=state.retryFrame):(first||((frame&31)==0)||(!state.delivered&&frame>=state.retryFrame));
        if(due){STEvent e{};e.type=eventType;e.track=l.track;e.target=l.target;e.slot=l.slot;e.address=l.address;e.value=value;if(eventType==ST_EVENT_HOST_PARAMETER)e.hostMap=current->hostMap;state.delivered=emit(e);state.retryFrame=frame+32;if(state.delivered)state.value=value;}}
    void automation(){if(!playing)return;emitPluginLocks();if(automationPattern!=pattern){resetAutomationEvents();automationPattern=pattern;}size_t laneIndex=0;double pos=sequenceFinished?current->patterns[pattern].length:musicalPosition();for(auto&l:current->patterns[pattern].laneList()){auto&state=laneEventState[laneIndex++];if(!channelEnabled(l.track)||l.points.empty())continue;float v=float(laneValue(l,pos));auto&t=tb->trackState[size_t(l.track)];if((t.rowLocks&(uint64_t(1)<<l.target))||pluginLaneLocked[laneIndex-1]){state.known=false;state.delivered=false;continue;}switch(l.target){case ST_TARGET_VOLUME:t.volume=clamp(v,0.f,2.f);break;case ST_TARGET_PAN:t.pan=clamp(v,-1.f,1.f);break;case ST_TARGET_CUTOFF:t.cutoff=clamp(v,20.f,22000.f);break;case ST_TARGET_RESONANCE:t.resonance=clamp(v,0.f,.99f);break;case ST_TARGET_DELAY:t.delay=clamp(v,0.f,1.f);break;case ST_TARGET_REVERB:t.reverb=clamp(v,0.f,1.f);break;case ST_TARGET_PITCH:t.pitch=clamp(v,-48.f,48.f);break;case ST_TARGET_PLUGIN:break;default:if(l.target>=ST_TARGET_ATTACK&&l.target<=ST_TARGET_LAST_INSTRUMENT){t.instrument[l.target-ST_TARGET_ATTACK]=v;t.instrumentMask|=uint64_t(1)<<(l.target-ST_TARGET_ATTACK);}break;}double eventValue=l.target==ST_TARGET_PLUGIN?clamp(double(v),0.0,1.0):v;emitLaneAutomation(l,state,eventValue,ST_EVENT_PARAMETER);}laneIndex=0;for(const auto&l:current->patterns[pattern].hostLaneList()){auto&state=hostLaneEventState[laneIndex++];if(pluginHostLaneLocked[laneIndex-1]){state.known=false;state.delivered=false;continue;}if(!l.points.empty())emitLaneAutomation(l,state,laneValue(l,pos),ST_EVENT_HOST_PARAMETER);}}
    void instrumentAutomation(Voice&v,bool geometry=true){auto&ts=tb->trackState[size_t(v.track)];if(!ts.instrumentMask)return;auto&p=v.p;auto&values=ts.instrument;uint64_t remaining=ts.instrumentMask;while(remaining){int n=__builtin_ctzll(remaining);remaining&=remaining-1;float x=values[n];if(!std::isfinite(x))continue;switch(n+ST_TARGET_ATTACK){case ST_TARGET_ATTACK:p.attack=clamp(x,0.f,60.f);break;case ST_TARGET_DECAY:p.decay=clamp(x,0.f,60.f);break;case ST_TARGET_SUSTAIN:p.sustain=clamp(x,0.f,1.f);break;case ST_TARGET_RELEASE:p.release=clamp(x,.0001f,60.f);break;case ST_TARGET_DRIVE:p.drive=clamp(x,0.f,1.f);break;case ST_TARGET_BIT_DEPTH:p.bitDepth=int(clamp(x,2.f,16.f));break;case ST_TARGET_LFO_RATE:p.lfoRate=clamp(x,0.f,100.f);break;case ST_TARGET_LFO_DEPTH:p.lfoDepth=clamp(x,0.f,1.f);break;case ST_TARGET_START:p.start=clamp(x,0.f,.999999f);break;case ST_TARGET_END:p.end=clamp(x,p.start+.000001f,1.f);break;case ST_TARGET_LOOP_START:p.loopStart=clamp(x,0.f,.999999f);break;case ST_TARGET_LOOP_END:p.loopEnd=clamp(x,p.loopStart+.000001f,1.f);break;case ST_TARGET_GRAIN_SIZE:p.grainSize=clamp(x,.002f,.5f);break;case ST_TARGET_GRAIN_POSITION:p.grainPosition=clamp(x,0.f,1.f);break;case ST_TARGET_SLICE_COUNT:p.sliceCount=int(clamp(x,1.f,128.f));break;case ST_TARGET_INSTRUMENT_VOLUME:p.volume=clamp(x,0.f,2.f)*v.zoneVolume;break;case ST_TARGET_INSTRUMENT_PAN:p.pan=clamp(x+v.zonePan,-1.f,1.f);break;case ST_TARGET_TUNE:p.tune=clamp(x,-96.f,96.f)+v.zoneTune;break;case ST_TARGET_FILTER_ATTACK:p.filterAttack=clamp(x,0.f,60.f);break;case ST_TARGET_FILTER_DECAY:p.filterDecay=clamp(x,0.f,60.f);break;case ST_TARGET_FILTER_SUSTAIN:p.filterSustain=clamp(x,0.f,1.f);break;case ST_TARGET_FILTER_RELEASE:p.filterRelease=clamp(x,.0001f,60.f);break;case ST_TARGET_FILTER_ENV_AMOUNT:p.filterEnvAmount=clamp(x,-48.f,48.f);break;case ST_TARGET_LFO2_RATE:p.lfo2Rate=clamp(x,0.f,100.f);break;case ST_TARGET_LFO2_DEPTH:p.lfo2Depth=clamp(x,0.f,1.f);break;case ST_TARGET_LFO_BEATS:p.lfoBeats=clamp(x,1.f/64,64.f);break;case ST_TARGET_LFO2_BEATS:p.lfo2Beats=clamp(x,1.f/64,64.f);break;case ST_TARGET_WAVETABLE_POSITION:p.wavetablePosition=clamp(x,0.f,1.f);break;default:setGrainTarget(p,n+ST_TARGET_ATTACK,x);break;}}
        if(v.zoneRegion){p.start=clamp(p.start,v.zoneStart,v.zoneEnd-.000001f);p.end=clamp(p.end,p.start+.000001f,v.zoneEnd);}
        if(geometry&&v.sample&&p.mode!=ST_MODE_SLICE){double oldBegin=v.begin,oldEnd=v.end;double frames=v.sample->l.size();if(std::isfinite(values[ST_TARGET_START-ST_TARGET_ATTACK]))v.begin=clamp(double(p.start)*frames,0.0,frames-1);/* A moved start re-derives the end as at note start, so an un-automated end can never fall below it. */if(std::isfinite(values[ST_TARGET_START-ST_TARGET_ATTACK])||std::isfinite(values[ST_TARGET_END-ST_TARGET_ATTACK]))v.end=clamp(double(p.end)*frames,v.begin+1,frames);if(std::isfinite(values[ST_TARGET_LOOP_START-ST_TARGET_ATTACK]))v.loopBegin=clamp(double(p.loopStart)*frames,v.begin,v.end-1);if(std::isfinite(values[ST_TARGET_LOOP_END-ST_TARGET_ATTACK]))v.loopEnd=clamp(double(p.loopEnd)*frames,v.loopBegin+1,v.end);if(v.zoneRegion){v.loopBegin=clamp(v.loopBegin,v.begin,v.end-1);v.loopEnd=clamp(v.loopEnd,v.loopBegin+1,v.end);}if(oldBegin!=v.begin||oldEnd!=v.end){
                /* The loop follows the moved region as at note start; left outside it, the next wrap would end the voice. */
                if(!v.zoneRegion){v.loopBegin=clamp(double(p.loopStart)*frames,v.begin,v.end-1);v.loopEnd=clamp(double(p.loopEnd)*frames,v.loopBegin+1,v.end);}
                v.pos=clamp(v.pos,v.begin,v.end-1);updateSampleBounds(v);}}}
    /* Granular: grainDensity overlapping Hann grains at equal phase offsets, each reading around the (scanning)
       position. With density 2 and no randomness this is exactly the historical two-grain crossfade. */
    void granularSample(Voice&v,const Sample&s,double rate,float modPosition,float modSize,float&l,float&r){
        const auto&p=v.p;double length=clamp(double(p.grainSize)*double(modSize)*sampleRate,64.0,sampleRate*.5);double range=std::max(1.0,v.end-v.begin);
        if(p.grainScan!=0)v.grainScan=wrapRange(v.grainScan+double(p.grainScan)*s.rate/sampleRate,range);
        double center=v.begin+clamp(double(p.grainPosition)+modPosition,0.0,1.0)*(v.end-v.begin-1)+v.grainScan;v.grainCenter=center;
        const int count=p.grainDensity>=1&&p.grainDensity<=ST_GRANULAR_MAX_GRAINS?p.grainDensity:ST_GRANULAR_DEFAULT_DENSITY;
        const bool random=p.grainSpray>0||p.grainPitchSpread>0||p.grainReverse>0||p.grainStereo>0;
        double a=v.grainPhase;v.grainCount=count;v.grainSpan=length*rate*.5+double(p.grainSpray)*s.rate;
        if(count==2&&!random){double c=a+.5,b=c>=0&&c<1?c:c>=1&&c<2?c-1:std::fmod(c,1.0); /* fmod(c,1), exact for c in [1,2) */
            double pa=center+(a-.5)*length*rate,pb=center+(b-.5)*length*rate;pa=v.begin+wrapRange(pa-v.begin,range);pb=v.begin+wrapRange(pb-v.begin,range);float wa=float(.5-.5*std::cos(2*pi*a)),wb=1-wa;float al,ar,bl,br;readHermite2(s,pa,v.sampleFirst,v.sampleLast,al,ar);readHermite2(s,pb,v.sampleFirst,v.sampleLast,bl,br);l=al*wa+bl*wb;r=ar*wa+br*wb;
            auto&g0=v.grains[0],&g1=v.grains[1];g0.phase=a;g0.read=pa;g0.window=wa;g0.dir=1;g1.phase=b;g1.read=pb;g1.window=wb;g1.dir=1;}
        else{l=r=0;
            for(int k=0;k<count;++k){auto&g=v.grains[size_t(k)];double phase=a+double(k)/count;phase-=std::floor(phase);
                if(phase<g.phase){g.offset=0;g.ratio=1;g.dir=1;g.gainL=g.gainR=1;
                    if(random){if(p.grainSpray>0)g.offset=double(randomBipolar(v.rng))*double(p.grainSpray)*s.rate;if(p.grainPitchSpread>0)g.ratio=std::exp2(randomBipolar(v.rng)*p.grainPitchSpread/12);
                        if(p.grainReverse>0&&(randomBipolar(v.rng)+1)*.5f<p.grainReverse)g.dir=-1;if(p.grainStereo>0){float pan=randomBipolar(v.rng)*p.grainStereo;g.gainL=pan>0?1-pan:1;g.gainR=pan<0?1+pan:1;}}}
                g.phase=phase;float w=float(.5-.5*std::cos(2*pi*phase));double at=center+g.offset+g.dir*(phase-.5)*length*rate*g.ratio;at=v.begin+wrapRange(at-v.begin,range);
                float gl,gr;readHermite2(s,at,v.sampleFirst,v.sampleLast,gl,gr);l+=gl*w*g.gainL;r+=gr*w*g.gainR;g.read=at;g.window=w;}
            /* Between amplitude (coherent) and power (decorrelated grains) normalization; 1 at density 1 and 2. */
            const float gain=count>=2?std::pow(2.f/float(count),.75f):1.f;l*=gain;r*=gain;}
        v.grainPhase+=1/length;v.grainPhase-=std::floor(v.grainPhase);}
    /* One live instrument parameter on one voice (see st_engine_set_instrument_parameter). Region targets
       only shape notes that start later (onset=true). */
    void applyLive(Voice&v,int target,float x,bool onset){auto&p=v.p;switch(target){
        case ST_TARGET_CUTOFF:p.cutoff=clamp(x,20.f,22000.f);break;case ST_TARGET_RESONANCE:p.resonance=clamp(x,0.f,.99f);break;
        case ST_TARGET_INSTRUMENT_VOLUME:p.volume=clamp(x,0.f,2.f)*v.zoneVolume;break;case ST_TARGET_INSTRUMENT_PAN:p.pan=clamp(clamp(x,-1.f,1.f)+v.zonePan,-1.f,1.f);break;
        case ST_TARGET_TUNE:p.tune=clamp(x,-96.f,96.f)+v.zoneTune;break;
        case ST_TARGET_ATTACK:p.attack=clamp(x,0.f,60.f);break;case ST_TARGET_DECAY:p.decay=clamp(x,0.f,60.f);break;case ST_TARGET_SUSTAIN:p.sustain=clamp(x,0.f,1.f);break;case ST_TARGET_RELEASE:p.release=clamp(x,.0001f,60.f);break;
        case ST_TARGET_DRIVE:p.drive=clamp(x,0.f,1.f);break;case ST_TARGET_BIT_DEPTH:p.bitDepth=int(clamp(x,2.f,16.f));break;
        case ST_TARGET_LFO_RATE:p.lfoRate=clamp(x,0.f,100.f);break;case ST_TARGET_LFO_DEPTH:p.lfoDepth=clamp(x,0.f,1.f);break;case ST_TARGET_LFO2_RATE:p.lfo2Rate=clamp(x,0.f,100.f);break;case ST_TARGET_LFO2_DEPTH:p.lfo2Depth=clamp(x,0.f,1.f);break;
        case ST_TARGET_LFO_BEATS:p.lfoBeats=clamp(x,1.f/64,64.f);break;case ST_TARGET_LFO2_BEATS:p.lfo2Beats=clamp(x,1.f/64,64.f);break;
        case ST_TARGET_FILTER_ATTACK:p.filterAttack=clamp(x,0.f,60.f);break;case ST_TARGET_FILTER_DECAY:p.filterDecay=clamp(x,0.f,60.f);break;case ST_TARGET_FILTER_SUSTAIN:p.filterSustain=clamp(x,0.f,1.f);break;case ST_TARGET_FILTER_RELEASE:p.filterRelease=clamp(x,.0001f,60.f);break;case ST_TARGET_FILTER_ENV_AMOUNT:p.filterEnvAmount=clamp(x,-48.f,48.f);break;
        case ST_TARGET_GRAIN_SIZE:p.grainSize=clamp(x,.002f,.5f);break;case ST_TARGET_GRAIN_POSITION:p.grainPosition=clamp(x,0.f,1.f);break;case ST_TARGET_WAVETABLE_POSITION:p.wavetablePosition=clamp(x,0.f,1.f);break;
        case ST_TARGET_START:if(onset)p.start=clamp(x,0.f,.999999f);break;case ST_TARGET_END:if(onset)p.end=clamp(x,p.start+.000001f,1.f);break;
        case ST_TARGET_LOOP_START:if(onset)p.loopStart=clamp(x,0.f,.999999f);break;case ST_TARGET_LOOP_END:if(onset)p.loopEnd=clamp(x,p.loopStart+.000001f,1.f);break;
        case ST_TARGET_SLICE_COUNT:if(onset)p.sliceCount=int(clamp(x,1.f,128.f));break;
        default:setGrainTarget(p,target,x);break;}}
    void applyLiveParams(Voice&v){for(int n=0;n<liveParamCount;++n){const auto&e=liveParams[size_t(n)];if(e.instrument==v.instrument)applyLive(v,e.target,e.value,true);}}
    void setLiveParam(int instrument,int target,float value){if(!current->hasInstrument(instrument)||!std::isfinite(value))return;
        int slot=-1;for(int n=0;n<liveParamCount;++n)if(liveParams[size_t(n)].instrument==instrument&&liveParams[size_t(n)].target==target){slot=n;break;}
        if(slot<0){if(liveParamCount==int(liveParams.size())){std::copy(liveParams.begin()+1,liveParams.end(),liveParams.begin());--liveParamCount;}slot=liveParamCount++;}
        liveParams[size_t(slot)]={instrument,target,value};
        for(auto&v:voices)if(v.active&&!v.external&&v.instrument==instrument)applyLive(v,target,value,false);}
    /* Voice playhead snapshot for the sample editor; only while watched, a no-op otherwise. */
    void publishVoicePositions(){const int watch=voiceWatch.load(std::memory_order_relaxed);if(watch<-1){if(!voiceSnapshotEmpty){voiceSnapshotEmpty=true;writeVoiceSnapshot(0);}return;}
        int count=0;for(size_t group=0;group<activeVoiceMasks.size()&&count<ST_VOICE_POSITION_CAPACITY;++group){uint64_t active=activeVoiceMasks[group];while(active&&count<ST_VOICE_POSITION_CAPACITY){int bit=__builtin_ctzll(active);active&=active-1;const auto&v=voices[group*64+size_t(bit)];
            if(!v.active||v.external||!v.sample||(watch>=0&&v.instrument!=watch))continue;voiceStage[size_t(count++)]=voicePosition(v);}}
        if(count==0&&voiceSnapshotEmpty)return;voiceSnapshotEmpty=count==0;writeVoiceSnapshot(count);}
    STVoicePosition voicePosition(const Voice&v)const{STVoicePosition o{};const auto&p=v.p;o.instrument=v.instrument;o.track=v.track;o.note=v.note;o.mode=p.mode;o.level=v.env;
        o.flags=(v.releasing?ST_VOICE_POSITION_RELEASING:0)|(v.preview?ST_VOICE_POSITION_PREVIEW:0)|(v.sequenced?ST_VOICE_POSITION_SEQUENCED:0)|(current->hasInstrument(v.instrument)&&current->instruments[size_t(v.instrument)].sample!=v.sample?ST_VOICE_POSITION_ZONE:0);
        o.sampleFrames=double(v.sample->l.size());o.direction=v.dir;o.position=o.windowStart=o.windowEnd=v.pos;
        if(p.mode==ST_MODE_WAVETABLE){double region=v.end-v.begin;double width=std::max(1.0,std::min(double(v.cycleWidth),std::floor(region)));double frames=std::max(1.0,std::floor(region/width));
            double frame=std::min(frames-1,std::floor(v.positionSmooth*(frames-1)+.5));o.position=o.windowStart=v.begin+frame*width;o.windowEnd=o.windowStart+width;o.direction=1;}
        else if(p.mode==ST_MODE_GRANULAR){double range=std::max(1.0,v.end-v.begin);o.position=v.begin+wrapRange(v.grainCenter-v.begin,range);o.windowStart=o.position-v.grainSpan;o.windowEnd=o.position+v.grainSpan;
            o.direction=p.grainScan>0?1:p.grainScan<0?-1:0;o.grainCount=v.grainCount;for(int k=0;k<v.grainCount&&k<ST_GRANULAR_MAX_GRAINS;++k){o.grainPositions[k]=v.grains[size_t(k)].read;o.grainLevels[k]=v.grains[size_t(k)].window;}}
        return o;}
    void writeVoiceSnapshot(int count){const uint64_t seq=voiceSeq.load(std::memory_order_relaxed);voiceSeq.store(seq+1,std::memory_order_relaxed);std::atomic_thread_fence(std::memory_order_release);
        std::copy(voiceStage.begin(),voiceStage.begin()+count,voiceSnapshot.begin());voiceSnapshotCount.store(count,std::memory_order_relaxed);++voiceSerial;voiceSeq.store(seq+2,std::memory_order_release);}
    std::array<STVoicePosition,ST_VOICE_POSITION_CAPACITY> voiceStage{};
    float envelope(Voice&v){auto&p=v.p;if(v.releasing){double duration=std::max(.0001,double(p.release))*sampleRate;v.env=v.releaseStart*float(std::max(0.0,1-v.elapsed/duration));if(v.elapsed>=duration){kill(v);return 0;}}else if(v.envStage==0){double duration=std::max(0.0,double(p.attack))*sampleRate;v.env=duration<1?1:float(v.elapsed/duration);if(v.env>=1){v.env=1;v.envStage=1;v.elapsed=0;}}else if(v.envStage==1){double duration=std::max(.0001,double(p.decay))*sampleRate;v.env=1+(p.sustain-1)*float(std::min(1.0,v.elapsed/duration));if(v.elapsed>=duration){v.env=p.sustain;v.envStage=2;}}else v.env=p.sustain;v.elapsed+=1;return v.env;}
    float filterEnvelope(Voice&v){const auto&p=v.p;if(v.releasing){double length=std::max(.0001,double(p.filterRelease))*sampleRate;v.filterEnv=v.filterReleaseStart*float(std::max(0.0,1-v.filterElapsed/length));}else if(v.filterEnvStage==0){double length=std::max(0.0,double(p.filterAttack))*sampleRate;v.filterEnv=length<1?1:float(std::min(1.0,v.filterElapsed/length));if(v.filterEnv>=1){v.filterEnvStage=1;v.filterElapsed=0;}}else if(v.filterEnvStage==1){double length=std::max(.0001,double(p.filterDecay))*sampleRate;v.filterEnv=1+(p.filterSustain-1)*float(std::min(1.0,v.filterElapsed/length));if(v.filterElapsed>=length){v.filterEnv=p.filterSustain;v.filterEnvStage=2;}}else v.filterEnv=p.filterSustain;++v.filterElapsed;return v.filterEnv;}
    float lfoSample(double&phase,double rate,int shape,float&held,uint32_t&random,bool audible){double unit=phase/(2*pi);float value=0;if(audible)switch(shape){case ST_LFO_TRIANGLE:value=1-4*std::abs(float(unit)-.5f);break;case ST_LFO_SAW:value=float(2*unit-1);break;case ST_LFO_SQUARE:value=unit<.5?1.f:-1.f;break;case ST_LFO_RANDOM:value=held;break;default:value=std::sin(float(phase));break;}phase+=2*pi*rate/sampleRate;if(phase>=2*pi){phase-=2*pi*std::floor(phase/(2*pi));if(shape==ST_LFO_RANDOM)held=randomBipolar(random);}return value;}
    struct Modulation {float cutoffOctaves=0,pitch=0,volume=1,pan=0,position=0,grainSize=1;};
    void routeModulation(Modulation&mod,float value,float depth,int target){if(depth<=0)return;float amount=value*depth;switch(target){case ST_MOD_PITCH:mod.pitch+=amount*12;break;case ST_MOD_VOLUME:mod.volume*=1-depth*(value+1)*.5f;break;case ST_MOD_PAN:mod.pan+=amount;break;case ST_MOD_POSITION:mod.position+=amount*.5f;break;case ST_MOD_GRAIN_SIZE:mod.grainSize*=std::exp2(amount*2);break;default:mod.cutoffOctaves+=amount*4;break;}}
    Modulation modulation(Voice&v){auto&p=v.p;Modulation mod;mod.cutoffOctaves=filterEnvelope(v)*p.filterEnvAmount/12;double rate1=p.lfoSync?effectiveBpm/(60*double(p.lfoBeats)):p.lfoRate;double rate2=p.lfo2Sync?effectiveBpm/(60*double(p.lfo2Beats)):p.lfo2Rate;float source1=(p.lfoDepth>0||rate1>0)?lfoSample(v.lfoPhase,rate1,p.lfoShape,v.lfoRandom,v.lfoRng,p.lfoDepth>0):0;float source2=(p.lfo2Depth>0||rate2>0)?lfoSample(v.lfo2Phase,rate2,p.lfo2Shape,v.lfo2Random,v.lfo2Rng,p.lfo2Depth>0):0;routeModulation(mod,source1,p.lfoDepth,p.lfoTarget);routeModulation(mod,source2,p.lfo2Depth,p.lfo2Target);return mod;}
    void filterCoefficients(Voice&v,float cutoff,float resonance){
        cutoff=clamp(cutoff,20.f,float(sampleRate*.45));resonance=clamp(resonance,0.f,.99f);
        if(cutoff==v.coefficientCutoff&&resonance==v.coefficientResonance)return;
        v.coefficientCutoff=cutoff;v.coefficientResonance=resonance;
        float g=std::tan(float(pi)*cutoff/float(sampleRate));v.filterK=2-1.95f*resonance;
        v.filterA1=1/(1+g*(g+v.filterK));v.filterA2=g*v.filterA1;v.filterA3=g*v.filterA2;
    }
    float filter(float in,float&ic1,float&ic2,const Voice&v,int type){float v3=in-ic2,v1=v.filterA1*ic1+v.filterA2*v3,v2=ic2+v.filterA2*ic1+v.filterA3*v3;ic1=2*v1-ic1;ic2=2*v2-ic2;if(std::abs(ic1)<1e-20f)ic1=0;if(std::abs(ic2)<1e-20f)ic2=0;return type==ST_FILTER_HIGHPASS?in-v.filterK*v1-v2:type==ST_FILTER_BANDPASS?v1:v2;}
    void voiceSample(Voice&v,float &l,float &r){l=r=0;if(v.mappedGateEnd>=0&&musicalBeat+loopBeatOffset>=v.mappedGateEnd)release(v);if(v.gate>=0){if(v.gate<=0)release(v);else v.gate=std::max(0.0,v.gate-1);}if(!v.active||v.external)return;
        instrumentAutomation(v);float env=envelope(v);if(!v.active)return;auto&p=v.p;auto&t=tb->trackState[size_t(v.track)];auto mod=modulation(v);const float bend=tb->pitchBend[size_t(v.track)];double pitch=v.note+p.tune+t.pitch+bend+mod.pitch;
        if(v.glide!=0)pitch+=v.glide*(1-std::exp(-v.lifetime/(sampleRate*.1)));
        if(v.arp){int stage=current->timing?int(std::floor((musicalBeat+loopBeatOffset)*12))%3:int(totalFrames/(sampleRate*60/current->bpm/12))%3;pitch+=stage==1?((v.arp>>4)&15):stage==2?(v.arp&15):0;}
        ++v.lifetime;if(p.mode==ST_MODE_WAVETABLE&&pitch!=v.frequencyPitch){v.frequencyPitch=pitch;v.frequency=clamp(midiFrequency(pitch),1.f,float(sampleRate*.45));}float frequency=v.frequency;
        if(v.sample){auto&s=*v.sample;double ratePitch=p.mode==ST_MODE_SLICE&&!v.explicitSlice?double(p.tune+t.pitch+bend+mod.pitch):pitch-p.rootNote;if(ratePitch!=v.ratePitch){v.ratePitch=ratePitch;v.sampleIncrement=s.rate/sampleRate*std::exp2(ratePitch/12.0);}double rate=v.sampleIncrement;
            if(p.mode==ST_MODE_WAVETABLE){
                // The region holds consecutive single cycles of cycleWidth samples (the whole region when
                // shorter). Position picks a frame and crossfades to its neighbour; it slews at most the full
                // range per 5 ms, so automation and step FX jumps glide between frames instead of clicking.
                double region=v.end-v.begin;size_t width=size_t(std::max(1.0,std::min(double(v.cycleWidth),std::floor(region))));
                size_t frames=std::max<size_t>(1,size_t(region/double(width)));
                double target=clamp(double(p.wavetablePosition)+mod.position,0.0,1.0),slew=1/(.005*sampleRate);
                v.positionSmooth=target>v.positionSmooth?std::min(target,v.positionSmooth+slew):std::max(target,v.positionSmooth-slew);
                double where=v.positionSmooth*double(frames-1);size_t f0=std::min(frames-1,size_t(where)),f1=std::min(frames-1,f0+1);float mix=float(where-double(f0));size_t base=size_t(v.begin);
                const CycleTaps taps=cycleTaps(width,v.phase);
                l=readCycleTaps(s.l,base+f0*width,taps);r=readCycleTaps(s.r,base+f0*width,taps);
                if(mix>0&&f1!=f0){l+=(readCycleTaps(s.l,base+f1*width,taps)-l)*mix;r+=(readCycleTaps(s.r,base+f1*width,taps)-r)*mix;}
                v.phase+=frequency/sampleRate;v.phase-=std::floor(v.phase);}
            else if(p.mode==ST_MODE_GRANULAR)granularSample(v,s,rate,mod.position,mod.grainSize,l,r);
            else {if(v.pos<v.begin||v.pos>=v.end){kill(v);return;}
                // Forward and reverse are Polyend-style loops (reverse starts at the end and loops backwards);
                // ping-pong bounces between the loop points; one-shot and slices play once.
                bool wrapLoop=p.mode==ST_MODE_FORWARD||p.mode==ST_MODE_REVERSE,looping=wrapLoop||p.mode==ST_MODE_PINGPONG;
                // The lead-in (before the loop in forward mode, after it in reverse mode) only wraps from inside the loop:
                // a voice turned around there runs out to the region edge, with the one-shot end fade.
                bool leaving=wrapLoop&&(p.mode==ST_MODE_REVERSE?v.dir>0&&v.pos>=v.loopEnd:v.dir<0&&v.pos<v.loopBegin);
                double readPosition=v.pos+mod.position*(v.end-v.begin);if(looping){double range=std::max(1.0,v.end-v.begin);readPosition=v.begin+wrapRange(readPosition-v.begin,range);}else readPosition=clamp(readPosition,v.begin,v.end-1);
                if(wrapLoop){double fade=std::min(s.rate*.005,(v.loopEnd-v.loopBegin)*.5);loopRead2(s,v,readPosition,fade,l,r);}else readHermite2(s,readPosition,v.sampleFirst,v.sampleLast,l,r);
                // Declick: a ~1 ms fade-in when starting inside the audio and a ~2 ms fade-out before a one-shot or slice ends.
                double step=std::max(rate,1e-9),regionFrames=(v.end-v.begin)/step;float declick=1;
                if(v.declickIn){double fadeIn=std::min(sampleRate*.001,regionFrames*.25);if(v.lifetime<fadeIn)declick=float(v.lifetime/fadeIn);else v.declickIn=false;}
                if(!looping||leaving){double remaining=(v.dir>0?v.end-v.pos:v.pos-v.begin+1)/step,fadeOut=std::min(sampleRate*.002,regionFrames*.25);if(remaining<fadeOut)declick*=float(std::max(0.0,remaining/fadeOut));}
                l*=declick;r*=declick;
                if(v.sampleTransition){
                    v.sampleTransition=false;
                    if(v.sampleHistory){v.sampleCorrectionL=v.sampleLastL-l;v.sampleCorrectionR=v.sampleLastR-r;v.sampleTransitionFrames=std::max(1,int(std::round(sampleRate*.002)));v.sampleTransitionLeft=v.sampleTransitionFrames;}
                }
                if(v.sampleTransitionLeft>0){
                    float phase=float(v.sampleTransitionLeft)/v.sampleTransitionFrames;
                    // Smoothstep has zero slope at both ends; subsequent reversals
                    // start from the corrected signal rather than stale source PCM.
                    float weight=phase*phase*(3-2*phase);
                    l+=v.sampleCorrectionL*weight;r+=v.sampleCorrectionR*weight;--v.sampleTransitionLeft;
                }
                v.sampleLastL=l;v.sampleLastR=r;v.sampleHistory=true;v.pos+=rate*v.dir;
                if(wrapLoop&&!leaving){double width=std::max(1.0,v.loopEnd-v.loopBegin);if(v.dir>0&&v.pos>=v.loopEnd){v.pos=v.loopBegin+std::fmod(v.pos-v.loopBegin,width);v.looped=true;}else if(v.dir<0&&v.pos<v.loopBegin){v.pos=v.loopBegin+std::fmod(std::fmod(v.pos-v.loopBegin,width)+width,width);v.looped=true;}}
                else if(p.mode==ST_MODE_PINGPONG){double lo=v.loopBegin,hi=v.loopEnd-1,width=hi-lo;if(width<=0){v.pos=lo;}else if((v.dir>0&&v.pos>hi)||(v.dir<0&&v.pos<lo)){double phase=std::fmod(std::fmod(v.pos-lo,width*2)+width*2,width*2);if(phase<=width){v.pos=lo+phase;}else{v.pos=hi-(phase-width);v.dir=-v.dir;}}}
            }
        }
        float cutoff=t.cutoff>=0?t.cutoff:p.cutoff;if(mod.cutoffOctaves!=0)cutoff*=std::exp2(mod.cutoffOctaves);if(const float wheel=tb->modWheel[size_t(v.track)];wheel>0)cutoff*=std::exp2(-5.25f*wheel);
        v.cutoffSmooth+=(cutoff-v.cutoffSmooth)*.01f;cutoff=v.cutoffSmooth;float resonance=t.resonance>=0?t.resonance:p.resonance;filterCoefficients(v,cutoff,resonance);l=filter(l,v.filterL1,v.filterL2,v,p.filterType);
        r=filter(r,v.filterR1,v.filterR2,v,p.filterType);
        if(p.drive>.00001f){if(p.drive!=v.driveAmount){v.driveAmount=p.drive;v.driveGain=1+p.drive*24;v.driveScale=1/std::tanh(v.driveGain);}l=std::tanh(l*v.driveGain)*v.driveScale;r=std::tanh(r*v.driveGain)*v.driveScale;}
        if(p.bitDepth<16){float levels=float(1u<<(clamp(p.bitDepth,2,16)-1));l=std::round(l*levels)/levels;r=std::round(r*levels)/levels;}
        v.volumeSmooth+=(p.volume-v.volumeSmooth)*.005f;v.panSmooth+=(p.pan-v.panSmooth)*.005f;float pan=clamp(v.panSmooth+mod.pan,-1.f,1.f),gain=env*v.velocity*v.volumeSmooth*mod.volume;if(pan!=v.panLawPan){v.panLawPan=pan;v.panLawL=std::sqrt(1-pan);v.panLawR=std::sqrt(1+pan);}l*=gain*v.panLawL;r*=gain*v.panLawR;
        if(!std::isfinite(l)||!std::isfinite(r)){l=r=0;kill(v);}
    }
    void retainClock(uint64_t graphFrame){
        if(graphFrame>(uint64_t(1)<<52)){cachedClock[size_t(graphFrame%clockCapacity)]={};return;}
        STClockContext c=musicalContext;c.graphFrame=int64_t(graphFrame);c.contentGraphFrame=c.graphFrame;
        const int64_t songFrame=int64_t(transportFrames);c.songFrame=songFrame;
        auto translate=[&](int64_t frame){return frame==std::numeric_limits<int64_t>::max()?frame:c.graphFrame+frame-songFrame;};
        c.nextBoundaryFrame=translate(c.nextBoundaryFrame);c.nextQuarterBeatFrame=translate(c.nextQuarterBeatFrame);c.nextQuarterGridFrame=translate(c.nextQuarterGridFrame);
        if(playing&&(songMode||current->timing)){
            // A section loops before the song endpoint, except when playback
            // started beyond the section and must first finish the outro.
            const bool section=songLoopActive();
            const double endpoint=section&&sequence<=songLoopLast()?occurrenceStartFrames(songLoopLast()+1):songEndFrames;
            int64_t end=songMode?int64_t(std::ceil(endpoint)):current->timing->frame(patternOriginBeat+patternDomainEndBeat());
            int64_t until=current->timing?end-songFrame:int64_t(std::ceil(endpoint-transportFrames));
            int64_t boundary=c.graphFrame+std::max<int64_t>(1,until);uint32_t kind=(c.flags&ST_CLOCK_LOOPING)?ST_CLOCK_BOUNDARY_LOOP:ST_CLOCK_BOUNDARY_DOMAIN;
            if(boundary<c.nextBoundaryFrame){c.nextBoundaryFrame=boundary;c.nextBoundaryKinds=kind;}else if(boundary==c.nextBoundaryFrame)c.nextBoundaryKinds|=kind;
            if(c.nextQuarterBeatFrame>=boundary){
                c.nextQuarterBeatFrame=std::numeric_limits<int64_t>::max();
                if(c.flags&ST_CLOCK_LOOPING){double first=std::ceil(c.loopStartBeat);if(first<c.loopEndBeat){int64_t lo=current->timing?current->timing->frame(c.loopStartBeat):0;int64_t next=current->timing?current->timing->frame(first):int64_t(std::ceil(first*60/current->bpm*sampleRate));if(next<end){
                    if(section&&!current->timing){double start=occurrenceStartFrames(current->songLoopStart),overshoot=sequence<=songLoopLast()?transportFrames+double(until)-endpoint:0;double target=first*60/current->bpm*sampleRate;c.nextQuarterBeatFrame=boundary+int64_t(std::max(0.0,std::ceil(target-start-overshoot)));}
                    else c.nextQuarterBeatFrame=boundary+next-lo;
                }}}
            }
        }
        if(c.nextQuarterBeatFrame==std::numeric_limits<int64_t>::max())c.flags|=ST_CLOCK_GRID_PROJECTION;else c.flags&=~ST_CLOCK_GRID_PROJECTION;
        retainFreeze(graphFrame,!cacheOriginValid);
        if(!cacheOriginValid){cacheOriginClock=c;cacheOriginTransport=transportFrames;cacheOriginGraph=graphFrame;cacheOriginTiming=current->timing.get();if(cacheOriginTiming)cacheOriginTiming->originPins.fetch_add(1,std::memory_order_relaxed);cacheOriginValid=true;}
        cachedClock[size_t(graphFrame%clockCapacity)]=c;
    }
    int32_t prerollClock(int64_t graphFrame,int64_t content,uint32_t requested,int64_t latency,STClockSpan*out)const{
        if(!cacheOriginValid||content>=int64_t(cacheOriginGraph)||int64_t(cacheOriginGraph)-content>(int64_t(1)<<30))return ST_ENGINE_CLOCK_NOT_RETAINED;
        bool active=(cacheOriginClock.flags&ST_CLOCK_PLAYING)!=0;double frame=cacheOriginTransport+(active?double(content-int64_t(cacheOriginGraph)):0);
        STClockSpan result{};auto&c=result.context;
        if(cacheOriginTiming){if(st_clock_context(cacheOriginTiming->timeline.get(),int64_t(frame),0,&c)!=ST_CLOCK_OK)return ST_CLOCK_OUTSIDE_DOMAIN;}
        else fixedClockContext(c,frame,sampleRate,cacheOriginClock.bpm,cacheOriginClock.numerator,cacheOriginClock.denominator);
        int64_t songFrame=c.songFrame;c.graphFrame=graphFrame;c.contentGraphFrame=content;c.revision=cacheOriginClock.revision;c.epoch=cacheOriginClock.epoch;c.loopIteration=0;c.loopStartBeat=cacheOriginClock.loopStartBeat;c.loopEndBeat=cacheOriginClock.loopEndBeat;c.flags=(c.flags&ST_CLOCK_DOMAIN_END)|(cacheOriginClock.flags&(ST_CLOCK_PLAYING|ST_CLOCK_LOOPING|ST_CLOCK_RECORDING))|(active?ST_CLOCK_PREROLL:0);
        auto translated=[&](int64_t value){return value==std::numeric_limits<int64_t>::max()?value:graphFrame+value-songFrame;};
        c.nextBoundaryFrame=active?translated(c.nextBoundaryFrame):std::numeric_limits<int64_t>::max();c.nextQuarterBeatFrame=active?translated(c.nextQuarterBeatFrame):std::numeric_limits<int64_t>::max();c.nextQuarterGridFrame=translated(c.nextQuarterGridFrame);
        int64_t origin=int64_t(cacheOriginGraph)+latency;
        if(c.nextBoundaryFrame>origin){c.nextBoundaryFrame=origin;c.nextBoundaryKinds=ST_CLOCK_BOUNDARY_ORIGIN;}else if(c.nextBoundaryFrame==origin)c.nextBoundaryKinds|=ST_CLOCK_BOUNDARY_ORIGIN;
        if(c.nextQuarterBeatFrame>=origin)c.nextQuarterBeatFrame=cacheOriginClock.nextQuarterBeatFrame==std::numeric_limits<int64_t>::max()?cacheOriginClock.nextQuarterBeatFrame:cacheOriginClock.nextQuarterBeatFrame+latency;
        if(c.nextQuarterBeatFrame==std::numeric_limits<int64_t>::max())c.flags|=ST_CLOCK_GRID_PROJECTION;
        result.frames=uint32_t(std::min<int64_t>(requested,std::max<int64_t>(0,c.nextBoundaryFrame-graphFrame)));*out=result;return ST_CLOCK_OK;
    }
    static constexpr uint64_t audible=std::numeric_limits<uint64_t>::max();
    /* Render thread. A bus that was silent since silentFrom has stale ring contents
       there; zero the still-readable part of that gap before writing real audio. */
    void zeroRing(BusRing&ring,uint64_t from,uint64_t to){while(from<to){size_t index=size_t(from%cacheCapacity),count=size_t(std::min<uint64_t>(to-from,cacheCapacity-index));std::fill_n(ring.l.data+index,count,0.f);std::fill_n(ring.r.data+index,count,0.f);from+=count;}}
    void writeRing(BusRing&ring,const float*l,const float*r,uint64_t start,uint32_t count){
        if(ring.generation!=cacheGeneration){ring.generation=cacheGeneration;ring.silentFrom=0;}
        if(ring.silentFrom!=audible){zeroRing(ring,std::max(ring.silentFrom,cacheBegin),start);ring.silentFrom=audible;}
        size_t index=size_t(start%cacheCapacity),first=std::min<size_t>(count,cacheCapacity-index);
        std::memcpy(ring.l.data+index,l,first*sizeof(float));std::memcpy(ring.r.data+index,r,first*sizeof(float));
        if(first<count){std::memcpy(ring.l.data,l+first,(count-first)*sizeof(float));std::memcpy(ring.r.data,r+first,(count-first)*sizeof(float));}
    }
    void metronomeClick(float&wetL,float&wetR){int64_t beat=current->timing?(musicalContext.bar*32+int64_t(std::floor((musicalBeat-musicalContext.barStartBeat)*musicalContext.denominator/4))):int64_t(transportFrames*current->bpm*current->denominator/(sampleRate*240));if(beat!=clickBeat){clickBeat=beat;clickAge=0;clickPhase=0;clickAccent=current->timing?int64_t(std::floor((musicalBeat-musicalContext.barStartBeat)*musicalContext.denominator/4))==0:beat%current->numerator==0;}if(clickAge<sampleRate*.03){float click=std::sin(clickPhase)*std::exp(float(-clickAge/(sampleRate*.005)))*current->metronomeVolume*(current->hostMixing?1:current->master);wetL+=click;wetR+=click;clickPhase+=2*pi*(clickAccent?1500:1000)/sampleRate;++clickAge;}}
    void countInClickSample(float&wetL,float&wetR){if(clickAge<sampleRate*.03){float click=std::sin(clickPhase)*std::exp(float(-clickAge/(sampleRate*.005)))*(current->metronomeVolume>0?current->metronomeVolume:.3f)*(current->hostMixing?1:current->master);wetL+=click;wetR+=click;clickPhase+=2*pi*(clickAccent?1500:1000)/sampleRate;++clickAge;}}
    void silenceRing(BusRing&ring,uint64_t start){if(ring.generation!=cacheGeneration){ring.generation=cacheGeneration;ring.silentFrom=0;}else if(ring.silentFrom==audible)ring.silentFrom=start;}
    void process(float *outL,float*outR,uint32_t frames,bool cache,int eventOffset=0,uint64_t graphStart=0){
        frameOffset=eventOffset;refresh();auto&T=*tb;const int returnBus=T.base;
        // Only channels that receive signal in a sample are mixed; a bus that receives
        // nothing in this block is not written at all (render_bus records it as silent).
        if(cache){std::fill(T.busTouched.begin(),T.busTouched.end(),uint8_t(0));T.busTouched[size_t(returnBus)]=1;}
        float blockPeakL=0,blockPeakR=0;std::fill(T.peaks.begin(),T.peaks.end(),0.f);const bool solo=current->anySolo;
        directBus=cache&&current->hostMixing;directFrames=frames;
        if(directBus)for(int t=0;t<T.channels;++t)T.hostAudible[size_t(t)]=channelEnabled(t)&&computationalTrackEnabled(t);
        for(uint32_t f=0;f<frames;++f){frameOffset=eventOffset+int(f);directFrame=f;if(countInTotal>0)stepCountIn();if(current->frozenSongPolicy&&playing&&songMode&&transportFrames>=std::ceil(songEndFrames))finishSong();if(playing&&songMode&&sequenceFinished&&transportFrames>=songEndFrames)finishSong();if(playing&&!sequenceFinished&&rowRemaining<=0){if(rowHasStarted)advanceRow();if(playing&&!sequenceFinished)triggerRow();}
            if(current->timing||cache||pendingInputCount)updateClock();if(cache)retainClock(graphStart+f);if(pendingInputCount)finishInputAcks(true,cache,graphStart+f);
            automation();if(scheduledCount)for(size_t group=0;group<scheduledGroupsUsed;++group){uint64_t active=scheduledMasks[group];while(active){int bit=__builtin_ctzll(active);active&=active-1;auto&s=scheduled[group*64+bit];if(!(s.mapped?musicalBeat+loopBeatOffset>=s.onsetBeat:s.due<=totalFrames))continue;if(laneTrack(s.sequenceTrack)<current->trackCount)noteOn(s.track,s.instrument,s.note,s.velocity,s.mapped?-1:s.gate,s.offset,s.reverse,s.glide,s.arp,true,s.slice,false,0,s.mapped&&!s.open?s.endBeat:-1,s.sequenceTrack);s.active=false;scheduledMasks[group]&=~(uint64_t(1)<<bit);--scheduledCount;}}

            // Ascending set bits preserve summation order while skipping inactive slots.
            for(size_t group=0;group<activeVoiceMasks.size();++group){uint64_t active=activeVoiceMasks[group];while(active){int bit=__builtin_ctzll(active);active&=active-1;auto&v=voices[group*64+bit];float l,r;voiceSample(v,l,r);if(!v.external)addChannel(v.track,l,r);}}
            mixClips();float mixL=0,mixR=0,delL=0,delR=0,revL=0,revR=0;
            if(directBus){
                // Only the return (metronome) remains per sample; channels are already in their buses.
                float wetL=0,wetR=0;
                if(current->metronome&&playing)metronomeClick(wetL,wetR);
                if(countInTotal>0)countInClickSample(wetL,wetR);
                T.busLeft(returnBus)[f]=wetL;T.busRight(returnBus)[f]=wetR;++totalFrames;if(playing){--rowRemaining;++transportFrames;}continue;
            }
            // Visit only channels that received a voice or clip this sample, in ascending
            // channel order (the historical summation order); silent channels add nothing.
            for(size_t word=0;word<T.touched.size();++word){uint64_t bits=T.touched[word];T.touched[word]=0;while(bits){const int t=int(word*64)+__builtin_ctzll(bits);bits&=bits-1;
                const float inL=T.left[size_t(t)],inR=T.right[size_t(t)];T.left[size_t(t)]=0;T.right[size_t(t)]=0;
                auto&p=channelMix(t);auto&s=T.trackState[size_t(t)];float l=0,r=0;if(current->hostMixing){if(channelEnabled(t)&&computationalTrackEnabled(t)){l=inL;r=inR;}}else{if(channelEnabled(t)&&!p.mute&&(!solo||p.solo)){float g=s.volume;float pan=clamp(s.pan,-1.f,1.f);l=inL*g*std::sqrt(1-pan);r=inR*g*std::sqrt(1+pan);delL+=l*s.delay;delR+=r*s.delay;revL+=l*s.reverb;revR+=r*s.reverb;}l*=current->master;r*=current->master;}
                T.peaks[size_t(t)]=std::max(T.peaks[size_t(t)],std::max(std::abs(l),std::abs(r)));mixL+=l;mixR+=r;
                if(cache){const int bus=outputBus(t);float*bl=T.busLeft(bus),*br=T.busRight(bus);if(!T.busTouched[size_t(bus)]){T.busTouched[size_t(bus)]=1;std::fill_n(bl,frames,0.f);std::fill_n(br,frames,0.f);}bl[f]=l;br[f]=r;}}}
            float wetL=0,wetR=0;if(!current->hostMixing){double delaySamples=clamp((current->delayBeats>0?current->delayBeats*60/std::max(1.0,effectiveBpm):double(current->delayTime))*sampleRate,1.0,double(delayL.size()-1));double read=double(delayPos)-delaySamples;/* A tiny negative read rounds up to exactly size(): that is position 0, not one past the end. */if(read<0){read+=delayL.size();if(read>=double(delayL.size()))read=0;}size_t ia=size_t(read),ib=(ia+1)%delayL.size();float fract=float(read-ia);float wetDL=delayL[ia]+(delayL[ib]-delayL[ia])*fract,wetDR=delayR[ia]+(delayR[ib]-delayR[ia])*fract;if(current->delayToneCoef<1.f){delayToneL+=current->delayToneCoef*(wetDL-delayToneL);delayToneR+=current->delayToneCoef*(wetDR-delayToneR);wetDL=delayToneL;wetDR=delayToneR;}delayL[delayPos]=delL+wetDR*current->delayFeedback;delayR[delayPos]=delR+wetDL*current->delayFeedback;if(++delayPos==delayL.size())delayPos=0;
            if(current->reverbPredelay>0){size_t lag=std::min(preL.size()-1,size_t(double(current->reverbPredelay)*sampleRate));preL[prePos]=revL;preR[prePos]=revR;size_t read=(prePos+preL.size()-lag)%preL.size();revL=preL[read];revR=preR[read];if(++prePos==preL.size())prePos=0;}
            float wetRL=0,wetRR=0,fb=.28f+current->reverbSize*.67f;for(int c=0;c<8;++c){wetRL+=combL[c].process((revL+revR*.2f)*.075f,fb,current->reverbDamping);wetRR+=combR[c].process((revR+revL*.2f)*.075f,fb,current->reverbDamping);}for(int a=0;a<4;++a){wetRL=apL[a].process(wetRL);wetRR=apR[a].process(wetRR);}
            wetL=(wetDL*current->delayMix+wetRL*current->reverbMix)*current->master;wetR=(wetDR*current->delayMix+wetRR*current->reverbMix)*current->master;}if(current->metronome&&playing)metronomeClick(wetL,wetR);
            if(countInTotal>0)countInClickSample(wetL,wetR);
            if(cache){T.busLeft(returnBus)[f]=wetL;T.busRight(returnBus)[f]=wetR;}mixL+=wetL;mixR+=wetR;
            // Bounded safety clip only above full scale; normal gain remains linear.
            mixL=clamp(mixL,-1.f,1.f);mixR=clamp(mixR,-1.f,1.f);if(outL)outL[f]=mixL;if(outR)outR[f]=mixR;blockPeakL=std::max(blockPeakL,std::abs(mixL));blockPeakR=std::max(blockPeakR,std::abs(mixR));++totalFrames;if(playing){--rowRemaining;++transportFrames;}}
        if(directBus){
            // Per-channel peaks and the summed mix, as the per-sample channel pass computes them:
            // channels in ascending order (a bus untouched at a sample adds exactly 0), then the return.
            directBus=false;float*mL=directMixL.data(),*mR=directMixR.data();std::fill_n(mL,frames,0.f);std::fill_n(mR,frames,0.f);
            for(int b=0;b<returnBus;++b){if(!T.busTouched[size_t(b)])continue;const float*bl=T.busLeft(b),*br=T.busRight(b);float peak=0;
                for(uint32_t f=0;f<frames;++f){mL[f]+=bl[f];mR[f]+=br[f];peak=std::max(peak,std::max(std::abs(bl[f]),std::abs(br[f])));}T.peaks[size_t(b)]=peak;}
            const float*wl=T.busLeft(returnBus),*wr=T.busRight(returnBus);
            for(uint32_t f=0;f<frames;++f){float mixL=clamp(mL[f]+wl[f],-1.f,1.f),mixR=clamp(mR[f]+wr[f],-1.f,1.f);blockPeakL=std::max(blockPeakL,std::abs(mixL));blockPeakR=std::max(blockPeakR,std::abs(mixR));}
        }
        if(pendingInputCount)finishInputAcks(false);
        publishVoicePositions();publishTransport();peakL.store(std::max(blockPeakL,peakL.load(std::memory_order_relaxed)*.85f),std::memory_order_relaxed);peakR.store(std::max(blockPeakR,peakR.load(std::memory_order_relaxed)*.85f),std::memory_order_relaxed);if(auto*store=peakStore.load(std::memory_order_acquire)){const size_t count=std::min(store->count,T.peaks.size());for(size_t t=0;t<count;++t)store->values[t].store(std::max(T.peaks[t],store->values[t].load(std::memory_order_relaxed)*.85f),std::memory_order_relaxed);}
    }
};

extern "C" {
STTrack st_default_track(void){return STTrack{.8f,0,0,0,0,0,0};}
STInstrument st_default_instrument(void){STInstrument p{};p.type=ST_INSTRUMENT_SAMPLE;p.rootNote=60;p.sliceCount=16;p.bitDepth=16;p.volume=.7f;p.end=1;p.loopEnd=1;p.attack=.005f;p.decay=.2f;p.sustain=.7f;p.release=.2f;p.cutoff=18000;p.grainSize=.08f;p.grainPosition=.5f;p.grainDensity=ST_GRANULAR_DEFAULT_DENSITY;p.filterAttack=.005f;p.filterDecay=.2f;p.filterSustain=0;p.filterRelease=.2f;p.lfoBeats=1;p.lfo2Beats=1;return p;}
STStep st_default_step(void){STStep s{};s.note=60;s.instrument=-1;s.velocity=100;s.gate=.8f;return s;}
STEngine* st_engine_create(double rate){if(!std::isfinite(rate)||rate<8000||rate>384000)return nullptr;try{return new STEngine(rate);}catch(...){return nullptr;}}
void st_engine_destroy(STEngine*e){delete e;}
void st_engine_begin_update(STEngine*e){if(!e)return;e->collect();e->draft.reset(new Project(*e->control));e->updating=true;}
void st_engine_clear_project(STEngine*e){if(!e)return;auto fresh=std::unique_ptr<Project>(new Project);fresh->patternEpoch=0;fresh->instruments=e->control->instruments;fresh->audioAssets=e->control->audioAssets;e->draft=std::move(fresh);e->updating=true;}
void st_engine_set_timing(STEngine*e,double bpm,int32_t lpb,float swing,float master){if(!e)return;auto&p=e->edit();p.bpm=clamp(finite(bpm,120),20.0,999.0);p.lpb=clamp(lpb,1,32);p.swing=clamp(finite(swing),0.f,.95f);p.master=clamp(finite(master,.7f),0.f,2.f);}
int32_t st_engine_set_frozen_song_policy(STEngine*e,int32_t enabled,uint64_t publication,char*error,uint32_t capacity){
    if(error&&capacity)error[0]=0;
    auto fail=[&](const char*message){if(error&&capacity){size_t n=std::min<size_t>(capacity-1,std::strlen(message));std::memcpy(error,message,n);error[n]=0;}return int32_t(0);};
    if(!e||(enabled!=0&&enabled!=1)||(enabled&&!publication))return fail("Frozen song policy requires a valid engine, boolean enable flag and nonzero publication ID.");
    try{
        // Allocate on control before publishing the policy. Storage lives until
        // quiescent engine destruction, including rejected/discarded drafts.
        if(enabled&&!e->freezeHistory.load(std::memory_order_acquire)){
            auto history=std::make_unique<FreezeHistory>();e->freezeHistory.store(history.release(),std::memory_order_release);
        }
        auto&p=e->edit();if(!enabled||!p.frozenSongPolicy||p.frozenPublicationID!=publication)p.frozenSongFrames=0;p.frozenSongPolicy=enabled!=0;p.frozenPublicationID=enabled?publication:0;return 1;
    }catch(const std::exception&problem){return fail(problem.what());}
}
int32_t st_engine_set_frozen_song_extent(STEngine*e,uint64_t publication,int64_t frames,char*error,uint32_t capacity){
    if(error&&capacity)error[0]=0;
    auto fail=[&](const char*message){if(error&&capacity){size_t n=std::min<size_t>(capacity-1,std::strlen(message));std::memcpy(error,message,n);error[n]=0;}return int32_t(0);};
    if(!e||!publication||frames<1||frames>100000000)return fail("Frozen song extent requires a valid engine, nonzero publication and1..100000000 frames.");
    try{
        const auto&previous=e->draft?*e->draft:*e->control;
        if(!previous.frozenSongPolicy||previous.frozenPublicationID!=publication)return fail("Frozen song extent must match the enabled publication ID.");
        if(previous.frozenSongFrames&&previous.frozenSongFrames!=frames)return fail("A frozen publication cannot change its exact song extent; create a new publication.");
        e->edit().frozenSongFrames=frames;return 1;
    }catch(const std::exception&problem){return fail(problem.what());}
}
int32_t st_engine_set_timing_map(STEngine*e,const STClockTempoEvent*t,uint32_t nt,const STClockMeterEvent*m,uint32_t nm,uint64_t revision,char*error,uint32_t capacity){
    if(error&&capacity)error[0]=0;if(!e)return 0;
    try{auto map=std::make_shared<const EngineTiming>(e->sampleRate,t,nt,m,nm,revision);auto existing=e->draft?e->draft->timing:e->control->timing;if(existing&&existing->matches(*map))map=existing;else e->timingMaps.push_back(map);e->edit().timing=std::move(map);return 1;}
    catch(const std::exception&problem){if(error&&capacity){size_t n=std::min<size_t>(capacity-1,std::strlen(problem.what()));std::memcpy(error,problem.what(),n);error[n]=0;}return 0;}
}
void st_engine_clear_timing_map(STEngine*e){if(e)e->edit().timing.reset();}
int32_t st_engine_set_pattern_timing_origin(STEngine*e,double beat){if(!e||!std::isfinite(beat)||beat<0||beat>1000000)return 0;e->edit().patternOriginBeat=beat;return 1;}
void st_engine_set_metronome(STEngine*e,int32_t enabled,int32_t numerator,int32_t denominator,float volume){if(!e)return;auto&p=e->edit();p.metronome=enabled!=0;p.numerator=clamp(numerator,1,16);p.denominator=denominator==2||denominator==4||denominator==8||denominator==16?denominator:4;p.metronomeVolume=clamp(finite(volume,.3f),0.f,1.f);}
int32_t st_engine_set_track_count(STEngine*e,int32_t count){if(!e||count<1||count>ST_TRACK_SANITY_LIMIT)return 0;auto&p=e->edit();p.trackCount=count;size_t base=size_t(p.returnBus());if(p.tracks.size()<base)p.tracks.resize(base,st_default_track());return 1;}
void st_engine_set_track(STEngine*e,int32_t i,STTrack t){if(!e||i<0||i>=ST_TRACK_SANITY_LIMIT)return;t.volume=clamp(finite(t.volume,.8f),0.f,2.f);t.pan=clamp(finite(t.pan),-1.f,1.f);t.delaySend=clamp(finite(t.delaySend),0.f,1.f);t.reverbSend=clamp(finite(t.reverbSend),0.f,1.f);t.instrument=clamp(t.instrument,0,int(e->view().instruments.size())-1);auto&p=e->edit();if(p.tracks.size()<=size_t(i))p.tracks.resize(size_t(i)+1,st_default_track());p.tracks[size_t(i)]=t;}
int32_t st_engine_set_track_voice(STEngine*e,int32_t track,int32_t chokeGroup,int32_t chokeScope,int32_t voiceMode){if(!e||track<0||track>=ST_TRACK_SANITY_LIMIT||chokeGroup<0||chokeGroup>127||chokeScope<0||chokeScope>ST_TRACK_SANITY_LIMIT||voiceMode< -1||voiceMode>2)return 0;auto&p=e->edit();if(p.trackVoices.size()<=size_t(track))p.trackVoices.resize(size_t(track)+1);p.trackVoices[size_t(track)]={chokeGroup,chokeScope,voiceMode};return 1;}
void st_engine_set_host_mixing(STEngine*e,int32_t enabled,uint32_t mask){if(!e)return;auto&p=e->edit();p.hostMixing=enabled!=0;p.computationalWords.assign(1,uint64_t(mask)|~uint64_t(0xffffffffu));p.computationalBeyond=true;}
void st_engine_set_host_mixing_tracks(STEngine*e,int32_t enabled,const uint64_t*words,uint32_t count,int32_t beyond){if(!e||(count&&!words))return;auto&p=e->edit();p.hostMixing=enabled!=0;p.computationalWords.assign(words,words+std::min<uint32_t>(count,(ST_TRACK_SANITY_LIMIT+63)/64));p.computationalBeyond=beyond!=0;}
int32_t st_engine_set_instrument_count(STEngine*e,int32_t count){if(!e||count<1||count>ST_INSTRUMENT_SANITY_LIMIT)return 0;auto&p=e->edit();p.instruments.resize(size_t(st_instrument_capacity(count)));return 1;}
void st_engine_set_instrument(STEngine*e,int32_t i,STInstrument p){if(!e||!e->view().hasInstrument(i))return;p.type=p.type==ST_INSTRUMENT_AU||p.type==ST_INSTRUMENT_MIDI?p.type:ST_INSTRUMENT_SAMPLE;p.wavetablePosition=clamp(finite(p.wavetablePosition),0.f,1.f);if(p.wavetableFrameSize<ST_WAVETABLE_MIN_FRAME||p.wavetableFrameSize>ST_WAVETABLE_MAX_FRAME)p.wavetableFrameSize=0;p.voiceMode=clamp(p.voiceMode,0,2);p.chokeGroup=clamp(p.chokeGroup,0,127);p.mode=clamp(p.mode,0,6);p.rootNote=clamp(p.rootNote,0,127);p.filterType=clamp(p.filterType,0,2);p.sliceCount=clamp(p.sliceCount,1,128);p.bitDepth=clamp(p.bitDepth,2,16);p.volume=clamp(finite(p.volume,.7f),0.f,2.f);p.pan=clamp(finite(p.pan),-1.f,1.f);p.tune=clamp(finite(p.tune),-96.f,96.f);p.start=clamp(finite(p.start),0.f,.999999f);p.end=clamp(finite(p.end,1),p.start+.000001f,1.f);p.loopStart=clamp(finite(p.loopStart),0.f,.999999f);p.loopEnd=clamp(finite(p.loopEnd,1),p.loopStart+.000001f,1.f);p.attack=clamp(finite(p.attack,.005f),0.f,60.f);p.decay=clamp(finite(p.decay,.2f),0.f,60.f);p.sustain=clamp(finite(p.sustain,.7f),0.f,1.f);p.release=clamp(finite(p.release,.2f),.0001f,60.f);p.cutoff=clamp(finite(p.cutoff,18000),20.f,22000.f);p.resonance=clamp(finite(p.resonance),0.f,.99f);p.drive=clamp(finite(p.drive),0.f,1.f);p.lfoRate=clamp(finite(p.lfoRate),0.f,100.f);p.lfoDepth=clamp(finite(p.lfoDepth),0.f,1.f);p.grainSize=clamp(finite(p.grainSize,.08f),.002f,.5f);p.grainPosition=clamp(finite(p.grainPosition,.5f),0.f,1.f);p.grainDensity=p.grainDensity>=1&&p.grainDensity<=ST_GRANULAR_MAX_GRAINS?p.grainDensity:ST_GRANULAR_DEFAULT_DENSITY;p.grainSpray=clamp(finite(p.grainSpray),0.f,1.f);p.grainPitchSpread=clamp(finite(p.grainPitchSpread),0.f,24.f);p.grainScan=clamp(finite(p.grainScan),-4.f,4.f);p.grainReverse=clamp(finite(p.grainReverse),0.f,1.f);p.grainStereo=clamp(finite(p.grainStereo),0.f,1.f);p.filterAttack=clamp(finite(p.filterAttack,.005f),0.f,60.f);p.filterDecay=clamp(finite(p.filterDecay,.2f),0.f,60.f);p.filterSustain=clamp(finite(p.filterSustain),0.f,1.f);p.filterRelease=clamp(finite(p.filterRelease,.2f),.0001f,60.f);p.filterEnvAmount=clamp(finite(p.filterEnvAmount),-48.f,48.f);p.lfoTarget=clamp(p.lfoTarget,0,5);p.lfo2Target=clamp(p.lfo2Target,0,5);p.lfoShape=clamp(p.lfoShape,0,4);p.lfo2Shape=clamp(p.lfo2Shape,0,4);p.lfo2Rate=clamp(finite(p.lfo2Rate),0.f,100.f);p.lfo2Depth=clamp(finite(p.lfo2Depth),0.f,1.f);p.lfoSync=p.lfoSync!=0;p.lfo2Sync=p.lfo2Sync!=0;p.lfoBeats=clamp(finite(p.lfoBeats,1),1.f/64,64.f);p.lfo2Beats=clamp(finite(p.lfo2Beats,1),1.f/64,64.f);e->edit().instruments[i].p=p;}
void st_engine_set_pattern(STEngine*e,int32_t i,int32_t length){if(!e||i<0||i>=ST_MAX_PATTERNS)return;auto&p=e->editPatterns();if(i>=int(p.patterns.size()))p.patterns.resize(size_t(i)+1);p.patterns[size_t(i)].resize(clamp(length,1,int(ST_MAX_ROWS)));}
int32_t st_engine_reuse_pattern(STEngine*e,int32_t i){if(!e||i<0||i>=ST_MAX_PATTERNS||size_t(i)>=e->control->patterns.size())return 0;auto&p=e->editPatterns();if(i>=int(p.patterns.size()))p.patterns.resize(size_t(i)+1);p.patterns[size_t(i)]=e->control->patterns[size_t(i)];return 1;}
void st_engine_set_step(STEngine*e,int32_t pat,int32_t track,int32_t row,STStep s,const int32_t*chord,int32_t count){if(!e||pat<0||track<0||track>=ST_TRACK_SANITY_LIMIT||row<0)return;auto&p=e->editPatterns();if(pat>=int(p.patterns.size())||row>=p.patterns[pat].length)return;auto&v=p.patterns[size_t(pat)].mutableStep(track,row);v.pluginFX={};v.present=true;s.note=clamp(s.note,-3,127);s.instrument=clamp(s.instrument,-1,int(e->view().instruments.size())-1);s.velocity=clamp(s.velocity,0,127);s.gate=finite(s.gate,0.f)>0?clamp(s.gate,.01f,256.f):0.f;s.fx1.value=finite(s.fx1.value);s.fx2.value=finite(s.fx2.value);v.p=s;v.count=chord&&count>0?clamp(count,1,int(ST_MAX_CHORD)):1;v.chord.fill(0);if(chord&&count>0)for(int n=0;n<v.count;++n)v.chord[n]=clamp(chord[n],-96,96);}
int32_t st_engine_set_plugin_step_fx(STEngine*e,int32_t pat,int32_t track,int32_t row,int32_t column,int32_t slot,int32_t destination,uint64_t address,double value){
    if(!e||pat<0||track<0||track>=ST_TRACK_SANITY_LIMIT||row<0||column<0||column>1||slot< -1||slot>=ST_MAX_INSERTS||destination< -1||destination>=ST_MAX_HOST_DESTINATIONS||!std::isfinite(value)||value<0||value>1)return 0;
    auto&p=e->editPatterns();if(pat>=int(p.patterns.size())||row>=p.patterns[pat].length)return 0;
    if(!p.patterns[size_t(pat)].step(track,row).present)return 0;auto&step=p.patterns[size_t(pat)].mutableStep(track,row);
    step.pluginFX[column]={true,slot,destination,address,value};return 1;
}
void st_engine_clear_step(STEngine*e,int32_t pat,int32_t track,int32_t row){if(!e||pat<0||track<0||track>=ST_TRACK_SANITY_LIMIT||row<0)return;auto&p=e->editPatterns();if(pat<int(p.patterns.size())&&row<p.patterns[pat].length&&p.patterns[size_t(pat)].hasColumn(track))p.patterns[size_t(pat)].mutableStep(track,row)=Step{};if(pat<int(p.patterns.size())&&row<p.patterns[pat].length)for(int c=1;c<=extraColumns;++c)if(p.patterns[size_t(pat)].columnStep(track,c,row).present)p.patterns[size_t(pat)].mutableColumnStep(track,c,row)=Step{};}
void st_engine_set_step_column(STEngine*e,int32_t pat,int32_t track,int32_t row,int32_t column,STStep s){if(!e||pat<0||track<0||track>=ST_TRACK_SANITY_LIMIT||row<0||column<0||column>extraColumns)return;if(column==0){st_engine_set_step(e,pat,track,row,s,nullptr,0);return;}auto&p=e->editPatterns();if(pat>=int(p.patterns.size())||row>=p.patterns[pat].length)return;auto&pattern=p.patterns[size_t(pat)];if(s.note<-2||s.note>127){if(pattern.columnStep(track,column,row).present)pattern.mutableColumnStep(track,column,row)=Step{};return;}auto&v=pattern.mutableColumnStep(track,column,row);v=Step{};v.present=true;s.instrument=clamp(s.instrument,-1,int(e->view().instruments.size())-1);s.velocity=clamp(s.velocity,0,127);s.gate=finite(s.gate,0.f)>0?clamp(s.gate,.01f,256.f):0.f;s.fx1=s.fx2=STStepFX{};v.p=s;}
void st_engine_clear_automation(STEngine*e,int32_t pat){if(!e||pat<0)return;auto&p=e->editPatterns();if(pat<int(p.patterns.size())){p.patterns[size_t(pat)].lanes.reset();p.patterns[size_t(pat)].hostLanes.reset();}}
void st_engine_add_automation_point(STEngine*e,int32_t pat,int32_t track,int32_t target,int32_t slot,uint64_t address,int32_t interpolation,double row,double value){if(!e||pat<0||track<0||track>=st_channel_count(ST_TRACK_SANITY_LIMIT)||target<0||target>ST_TARGET_LAST_INSTRUMENT||!std::isfinite(row)||!std::isfinite(value))return;auto&p=e->editPatterns();if(pat>=int(p.patterns.size()))return;
    {const auto&shared=p.patterns[size_t(pat)].laneList();bool known=false;for(const auto&l:shared)if(l.track==track&&l.target==target&&l.slot==slot&&l.address==address){known=l.points.size()<4096;if(!known)return;break;}if(!known&&shared.size()>=256)return;}
    auto&lanes=p.patterns[size_t(pat)].mutableLanes(false);auto it=std::find_if(lanes.begin(),lanes.end(),[&](const Lane&l){return l.track==track&&l.target==target&&l.slot==slot&&l.address==address;});if(it==lanes.end()){if(lanes.size()>=256)return;lanes.push_back(Lane{track,target,slot,interpolation,address,{}});it=lanes.end()-1;}auto&pts=it->points;if(pts.size()<4096)pts.push_back({clamp(row,0.0,double(p.patterns[pat].length)),value});}
void st_engine_add_host_automation_point(STEngine*e,int32_t pat,int32_t destination,uint64_t address,int32_t interpolation,double row,double value){if(!e||pat<0||destination<0||destination>=ST_MAX_HOST_DESTINATIONS||!std::isfinite(row)||!std::isfinite(value))return;auto&p=e->editPatterns();if(pat>=int(p.patterns.size()))return;
    {const auto&shared=p.patterns[size_t(pat)].hostLaneList();bool known=false;for(const auto&l:shared)if(l.target==destination&&l.address==address){known=l.points.size()<4096;if(!known)return;break;}if(!known&&shared.size()>=256)return;}
    auto&lanes=p.patterns[size_t(pat)].mutableLanes(true);auto it=std::find_if(lanes.begin(),lanes.end(),[&](const Lane&l){return l.target==destination&&l.address==address;});if(it==lanes.end()){if(lanes.size()>=256)return;lanes.push_back(Lane{-1,destination,0,interpolation==1?1:0,address,{}});it=lanes.end()-1;}if(it->points.size()<4096)it->points.push_back({clamp(row,0.0,double(p.patterns[pat].length)),value});}
void st_engine_set_sequence(STEngine*e,const int32_t*patterns,int32_t count,int32_t loop,int32_t active){if(!e)return;auto&p=e->edit();p.sequence.clear();if(patterns&&count>0)for(int i=0;i<std::min(count,4096);++i)p.sequence.push_back(clamp(patterns[i],0,int(p.patterns.size()-1)));if(p.sequence.empty())p.sequence.push_back(0);p.loopSong=loop!=0;p.activePattern=clamp(active,0,int(p.patterns.size()-1));}
void st_engine_set_effects(STEngine*e,float time,float feedback,float mix,float size,float damping,float reverb){if(!e)return;auto&p=e->edit();p.delayTime=clamp(finite(time,.375f),.001f,3.99f);p.delayFeedback=clamp(finite(feedback,.35f),0.f,.98f);p.delayMix=clamp(finite(mix,.3f),0.f,1.f);p.reverbSize=clamp(finite(size,.7f),0.f,1.f);p.reverbDamping=clamp(finite(damping,.4f),0.f,.99f);p.reverbMix=clamp(finite(reverb,.25f),0.f,1.f);}
int32_t st_engine_set_sample(STEngine*e,int32_t i,const float*l,const float*r,uint64_t frames,double rate){if(e&&e->view().hasInstrument(i)&&!l&&frames==0){e->edit().instruments[i].setSample(nullptr);if(!e->updating&&!e->commit())return 0;return 1;}if(!e||!e->view().hasInstrument(i)||!l||frames<1||frames>uint64_t(std::numeric_limits<int32_t>::max())||!std::isfinite(rate)||rate<1000||rate>768000)return 0;try{auto s=std::unique_ptr<Sample>(new Sample);s->rate=rate;s->l.resize(size_t(frames));s->r.resize(size_t(frames));for(size_t n=0;n<frames;++n){s->l[n]=clamp(finite(l[n]),-8.f,8.f);s->r[n]=clamp(finite(r?r[n]:l[n]),-8.f,8.f);}auto*raw=s.get();e->samples.push_back(std::move(s));e->edit().instruments[i].setSample(raw);if(!e->updating&&!e->commit())return 0;return 1;}catch(...){return 0;}}
void st_engine_set_slice_points(STEngine*e,int32_t i,const float*positions,int32_t count){if(!e||!e->view().hasInstrument(i))return;auto&s=e->edit().instruments[i].slices;s.clear();if(positions&&count>0){s.push_back(0);for(int n=0;n<std::min(count,128);++n)if(std::isfinite(positions[n])&&positions[n]>0&&positions[n]<1)s.push_back(positions[n]);std::sort(s.begin(),s.end());s.erase(std::unique(s.begin(),s.end()),s.end());}}
int32_t st_engine_set_sample_cycle(STEngine*e,int32_t i,int32_t frames){if(!e||!e->view().hasInstrument(i)||frames<0||(frames>0&&(frames<ST_WAVETABLE_MIN_FRAME||frames>ST_WAVETABLE_MAX_FRAME)))return 0;e->edit().instruments[i].cycleFrames=frames;if(!e->updating&&!e->commit())return 0;return 1;}
void st_engine_clear_zones(STEngine*e,int32_t i){if(e&&e->view().hasInstrument(i))e->edit().instruments[i].zones.clear();}
int32_t st_engine_add_sample_zone(STEngine*e,int32_t i,const float*l,const float*r,uint64_t frames,double rate,int32_t keyLow,int32_t keyHigh,int32_t velocityLow,int32_t velocityHigh,int32_t rootNote,float tune,float volume){if(!e||!e->view().hasInstrument(i)||!l||frames<1||frames>uint64_t(std::numeric_limits<int32_t>::max())||!std::isfinite(rate)||rate<1000||rate>768000)return 0;try{auto&instrument=e->edit().instruments[i];if(instrument.zones.size()>=128)return 0;auto sample=std::unique_ptr<Sample>(new Sample);sample->rate=rate;sample->l.resize(size_t(frames));sample->r.resize(size_t(frames));for(size_t n=0;n<frames;++n){sample->l[n]=clamp(finite(l[n]),-8.f,8.f);sample->r[n]=clamp(finite(r?r[n]:l[n]),-8.f,8.f);}auto*raw=sample.get();e->samples.push_back(std::move(sample));Zone zone;zone.sample=raw;raw->refs.fetch_add(1,std::memory_order_relaxed);zone.keyLow=clamp(keyLow,0,127);zone.keyHigh=clamp(keyHigh,zone.keyLow,127);zone.velocityLow=clamp(velocityLow,0,127);zone.velocityHigh=clamp(velocityHigh,zone.velocityLow,127);zone.rootNote=clamp(rootNote,0,127);zone.tune=clamp(finite(tune),-96.f,96.f);zone.volume=clamp(finite(volume,1),0.f,2.f);instrument.zones.push_back(zone);if(!e->updating&&!e->commit())return 0;return 1;}catch(...){return 0;}}
int32_t st_engine_set_audio_asset(STEngine*e,int32_t asset,const float*l,const float*r,uint64_t frames,double rate){if(!e||asset<0||asset>=256)return 0;auto&entry=e->edit().audioAssets[asset];if(!l&&frames==0){if(entry.sample)entry.sample->refs.fetch_sub(1,std::memory_order_release);entry.sample=nullptr;if(!e->updating&&!e->commit())return 0;return 1;}if(!l||frames<1||frames>uint64_t(std::numeric_limits<int32_t>::max())||!std::isfinite(rate)||rate<1000||rate>768000)return 0;try{auto sample=std::unique_ptr<Sample>(new Sample);sample->rate=rate;sample->l.resize(size_t(frames));sample->r.resize(size_t(frames));for(size_t n=0;n<frames;++n){sample->l[n]=clamp(finite(l[n]),-8.f,8.f);sample->r[n]=clamp(finite(r?r[n]:l[n]),-8.f,8.f);}auto*raw=sample.get();e->samples.push_back(std::move(sample));raw->refs.fetch_add(1,std::memory_order_relaxed);if(entry.sample)entry.sample->refs.fetch_sub(1,std::memory_order_release);entry.sample=raw;if(!e->updating&&!e->commit())return 0;return 1;}catch(...){return 0;}}
int32_t st_engine_set_sample_zone_loop_enabled(STEngine*e,int32_t instrument,int32_t zoneIndex,int32_t enabled){
    if(!e||!e->view().hasInstrument(instrument)||zoneIndex<0||enabled< -1||enabled>1)return 0;
    auto&zones=e->edit().instruments[instrument].zones;if(size_t(zoneIndex)>=zones.size())return 0;zones[size_t(zoneIndex)].loopEnabled=enabled;return 1;
}

int32_t st_engine_set_sample_zone_region(STEngine*e,int32_t instrument,int32_t zoneIndex,float start,float end,float loopStart,float loopEnd,float pan){
    if(!e||!e->view().hasInstrument(instrument)||zoneIndex<0||!std::isfinite(start)||!std::isfinite(end)||!std::isfinite(loopStart)||!std::isfinite(loopEnd)||!std::isfinite(pan)||start<0||end>1||end-start<.000001f||loopStart<0||loopEnd>1||loopEnd<=loopStart||pan< -1||pan>1)return 0;
    auto&zones=e->edit().instruments[instrument].zones;if(size_t(zoneIndex)>=zones.size())return 0;auto&z=zones[size_t(zoneIndex)];z.start=start;z.end=end;z.loopStart=loopStart;z.loopEnd=loopEnd;z.pan=pan;z.hasRegion=true;return 1;
}
void st_engine_clear_audio_clips(STEngine*e){if(e){e->edit().audioClips.clear();e->edit().patternClips.clear();e->edit().songTailClips.clear();}}
void st_engine_add_song_tail_clip(STEngine*e,int32_t asset,int32_t track,int64_t startFrame,float gain){if(!e||asset<0||asset>=256||track<0||track>=ST_TRACK_SANITY_LIMIT||startFrame<0)return;auto&p=e->edit();if(p.songTailClips.size()>=64)return;SongTailClip clip;clip.asset=asset;clip.track=track;clip.start=startFrame;clip.gain=clamp(finite(gain,1),0.f,4.f);p.songTailClips.push_back(clip);}
void st_engine_add_pattern_clip(STEngine*e,int32_t head,int32_t loop,int32_t track,int32_t pattern,float gain){if(!e||head<0||head>=256||loop>=256||track<0||track>=ST_TRACK_SANITY_LIMIT||pattern<0||pattern>=ST_MAX_PATTERNS)return;auto&p=e->edit();if(p.patternClips.size()>=256)return;PatternClip clip;clip.headAsset=head;clip.loopAsset=loop<0?-1:loop;clip.track=track;clip.pattern=pattern;clip.gain=clamp(finite(gain,1),0.f,4.f);p.patternClips.push_back(clip);}
void st_engine_add_audio_clip(STEngine*e,int32_t asset,int32_t track,double startBeat,double durationBeats,double offset,float gain,float fadeIn,float fadeOut,int32_t muted){if(!e||asset<0||asset>=256||track<0||track>=ST_TRACK_SANITY_LIMIT)return;auto&p=e->edit();if(p.audioClips.size()>=256)return;p.audioClips.push_back({asset,track,clamp(finite(startBeat,0.0),0.0,1000000.0),clamp(finite(durationBeats,0.0),0.0,1000000.0),clamp(finite(offset,0.0),0.0,1000000.0),clamp(finite(gain,1),0.f,4.f),clamp(finite(fadeIn,.005f),0.f,3600.f),clamp(finite(fadeOut,.005f),0.f,3600.f),muted!=0});}
void st_engine_commit_update(STEngine*e){if(e)e->commit();}
int32_t st_engine_commit_update_checked(STEngine*e,char*error,uint32_t capacity){return e&&e->commit(error,capacity);}
void st_engine_cancel_update(STEngine*e){if(e){e->draft.reset();e->updating=false;}}
void st_engine_collect(STEngine*e){if(e)e->collect();}
void st_engine_play(STEngine*e,int32_t song){if(e)e->queueTransport({1,song},true);}
void st_engine_set_song_loop(STEngine*e,int32_t start,int32_t end){if(!e)return;auto&p=e->edit();if(start<0||end<start){start=-1;end=-1;}p.songLoopStart=std::min(start,4095);p.songLoopEnd=std::min(end,4095);}
void st_engine_set_send_effects(STEngine*e,double delayBeats,float delayTone,float reverbPredelay){if(!e)return;auto&p=e->edit();p.delayBeats=std::isfinite(delayBeats)&&delayBeats>0?clamp(delayBeats,1.0/64,16.0):0;float tone=clamp(finite(delayTone,1.f),0.f,1.f);/* 1 = open; otherwise 200 Hz .. 20 kHz on a log scale. */p.delayToneCoef=tone>=1.f?1.f:float(1-std::exp(-2*3.14159265358979323846*200*std::pow(100.0,double(tone))/e->sampleRate));p.reverbPredelay=clamp(finite(reverbPredelay),0.f,.5f);}
void st_engine_set_pattern_loop(STEngine*e,int32_t startRow,int32_t endRow){if(!e)return;if(startRow<0||endRow<startRow){startRow=-1;endRow=-1;}e->queue({11,std::min(startRow,ST_MAX_ROWS-1),std::min(endRow,ST_MAX_ROWS-1)});}
void st_engine_stop(STEngine*e){if(e)e->queueTransport({2},false);}
void st_engine_set_recording(STEngine*e,int32_t enabled){if(e)e->queue({9,enabled!=0});}
void st_engine_seek(STEngine*e,int32_t p,int32_t r,int32_t s){if(e)e->queue({3,p,r,s});}
STTransportState st_engine_capture_transport(STEngine*e){STTransportState state{};if(!e)return state;e->flushQuiescent();state.playing=e->playing;state.songMode=e->songMode;state.pattern=e->pattern;state.row=e->row;state.sequence=e->sequence;state.rowHasStarted=e->rowHasStarted;state.sequenceFinished=e->sequenceFinished;state.rowRemaining=e->rowRemaining;state.rowDuration=e->rowDuration;state.transportFrames=e->transportFrames;state.timingVersion=e->current->timing?1:0;state.timingRevision=e->current->timing?e->current->timing->revision:0;state.beat=e->musicalBeat;state.patternOriginBeat=e->patternOriginBeat;state.loopBeatOffset=e->loopBeatOffset;state.loopIteration=e->clockLoopIteration;state.recording=e->recording;state.patternClipFrame=e->patternClipFrame;state.patternClipPattern=e->patternClipPattern;state.patternClipLooped=e->patternClipLooped;state.patternClipValid=e->patternClipValid;return state;}
void st_engine_restore_transport(STEngine*e,STTransportState state,int32_t resume){
    if(!e)return;e->flushQuiescent();e->resetInputRun();e->forgetEarlyInputs();e->suppressEvents=true;e->allOff(true);e->clearScheduled();e->clearEffects();std::fill(e->tb->pitchBend.begin(),e->tb->pitchBend.end(),0.f);std::fill(e->tb->modWheel.begin(),e->tb->modWheel.end(),0.f);e->queuedPattern=e->queuedSequence=e->queuedMode=-1;e->countInTotal=0;e->clickAge=1e9;e->clickBeat=-1;
    e->playing=state.playing!=0&&resume!=0;e->recording=state.recording!=0;e->songMode=state.songMode!=0;e->pattern=e->validPattern(state.pattern);e->row=clamp(state.row,0,e->current->patterns[e->pattern].length-1);e->sequence=clamp(state.sequence,0,int(e->current->sequence.size())-1);e->rowHasStarted=state.rowHasStarted!=0;e->sequenceFinished=state.sequenceFinished!=0&&e->songMode;
    double defaultDuration=e->sampleRate*60/e->current->bpm/e->current->lpb*(1+((e->row&1)?-1:1)*e->current->swing);e->rowDuration=clamp(finite(state.rowDuration,defaultDuration),1.0,e->sampleRate*6);e->rowRemaining=e->sequenceFinished?1e100:e->rowHasStarted?clamp(finite(state.rowRemaining,0.0),-1.0,e->rowDuration):(e->current->frozenSongPolicy&&e->songMode?clamp(finite(state.rowRemaining,0.0),-1.0,0.0):0);e->transportFrames=clamp(finite(state.transportFrames,0.0),0.0,1e15);e->clickBeat=e->transportFrames>0?int64_t(std::floor((e->transportFrames-1)*e->current->bpm*e->current->denominator/(e->sampleRate*240))):-1;
    e->patternOriginBeat=e->current->patternOriginBeat;e->loopBeatOffset=0;e->clockLoopIteration=0;++e->timingEpoch;
    if(e->current->timing){
        double width=e->patternRows(*e->current,e->pattern)/e->current->lpb;
        if(state.timingVersion==1){e->patternOriginBeat=clamp(finite(state.patternOriginBeat,e->current->patternOriginBeat),0.0,1000000.0-width);e->loopBeatOffset=clamp(finite(state.loopBeatOffset,0.0),0.0,1e12);e->clockLoopIteration=clamp(state.loopIteration,int64_t(0),int64_t(1)<<52);}
        e->transportFrames=clamp(e->transportFrames,0.0,double(e->current->timing->frame(1000000)));
        if(state.timingVersion==1&&state.timingRevision!=e->current->timing->revision){e->transportFrames=e->current->timing->frame(clamp(finite(state.beat,0.0),0.0,1000000.0));e->resolveMappedTimeline();}
        else{e->rowStartBeat=e->rowBaseBeat()+e->beatForRow(*e->current,e->row);e->rowEndBeat=e->rowBaseBeat()+e->beatForRow(*e->current,e->row+1);}
        e->updateClock();STClockContext previous{};if(e->transportFrames>0&&st_clock_context(e->current->timing->timeline.get(),int64_t(e->transportFrames)-1,0,&previous)==ST_CLOCK_OK)e->clickBeat=previous.bar*32+int64_t(std::floor((previous.beat-previous.barStartBeat)*previous.denominator/4));
    }
    e->alignFrozenOrigin();e->freezeTransition(ST_FREEZE_CAUSE_RESTORE);e->restorePatternClips(state);
    e->resetTrackState(false);e->suppressEvents=false;e->automationRefreshRequested.store(true,std::memory_order_release);e->cacheValid=false;e->publishTransport();e->peakL.store(0,std::memory_order_relaxed);e->peakR.store(0,std::memory_order_relaxed);if(auto*store=e->peakStore.load(std::memory_order_acquire))for(size_t n=0;n<store->count;++n)store->values[n].store(0,std::memory_order_relaxed);
}
void st_engine_note_on(STEngine*e,int32_t t,int32_t i,int32_t n,int32_t v){if(e)e->queue({4,t,i,n,v});}
void st_engine_note_off(STEngine*e,int32_t t,int32_t n){if(e)e->queue({5,t,n});}
int32_t st_engine_input_note(STEngine*e,uint64_t request,int32_t edge,int32_t track,int32_t instrument,int32_t note,int32_t velocity,int32_t applyNote){
    if(!e||!request||request<=e->lastInputRequestID||(edge!=ST_INPUT_NOTE_ON&&edge!=ST_INPUT_NOTE_OFF)||track<0||track>=e->control->channels()||!e->view().hasInstrument(instrument)||note<0||note>127||velocity<0||velocity>127||(edge==ST_INPUT_NOTE_ON&&velocity==0)||(applyNote!=0&&applyNote!=1))return 0;
    uint32_t outstanding=e->inputOutstanding.fetch_add(1,std::memory_order_relaxed);
    if(outstanding>=ST_INPUT_ACK_CAPACITY-1){e->inputOutstanding.fetch_sub(1,std::memory_order_relaxed);return 0;}
    Command c;c.type=10;c.a=track;c.b=instrument;c.c=note;c.d=velocity;c.requestID=request;c.inputEdge=edge;c.applyInputNote=applyNote;
    if(!e->commands.push(c)){e->inputOutstanding.fetch_sub(1,std::memory_order_relaxed);e->droppedCommands.fetch_add(1,std::memory_order_relaxed);return 0;}
    e->lastInputRequestID=request;return 1;
}
int32_t st_engine_pop_input_ack(STEngine*e,STInputAck*out){if(!e||!out||!e->inputAcks.pop(*out))return 0;e->inputOutstanding.fetch_sub(1,std::memory_order_relaxed);return 1;}
void st_engine_preview_step(STEngine*e,int32_t track,STStep step,const int32_t*chord,int32_t count,float duration){if(!e)return;Command c;c.type=8;c.a=track;c.step=step;c.step.note=clamp(step.note,-3,127);c.step.instrument=clamp(step.instrument,-1,int(e->view().instruments.size())-1);c.step.velocity=clamp(step.velocity,0,127);c.value=clamp(finite(duration,.35f),.03f,2.f);c.chordCount=chord&&count>0?clamp(count,1,int(ST_MAX_CHORD)):1;for(int n=0;chord&&count>0&&n<c.chordCount;++n)c.chord[n]=clamp(chord[n],-96,96);e->queue(c);}
void st_engine_pitch_bend(STEngine*e,int32_t t,float semitones){if(e)e->queue({7,t,0,0,0,clamp(finite(semitones),-48.f,48.f)});}
void st_engine_mod_wheel(STEngine*e,int32_t t,float value){if(e)e->queue({14,t,0,0,0,clamp(finite(value),0.f,1.f)});}
int32_t st_engine_set_instrument_parameter(STEngine*e,int32_t instrument,int32_t target,float value){
    if(!e||instrument<0||instrument>=ST_INSTRUMENT_SANITY_LIMIT||!std::isfinite(value))return 0;
    if(!(target==ST_TARGET_CUTOFF||target==ST_TARGET_RESONANCE||(target>=ST_TARGET_ATTACK&&target<=ST_TARGET_LAST_INSTRUMENT)))return 0;
    if(e->commands.available()==0){e->droppedCommands.fetch_add(1,std::memory_order_relaxed);return 0;}
    e->queue({15,instrument,target,0,0,value});return 1;}
void st_engine_watch_voice_positions(STEngine*e,int32_t instrument){if(e)e->voiceWatch.store(instrument<-1?-2:instrument,std::memory_order_relaxed);}
int32_t st_engine_voice_positions(STEngine*e,STVoicePosition*out,int32_t capacity,uint64_t*serial){
    if(!e||!out||capacity<=0)return 0;
    for(int attempt=0;attempt<1000;++attempt){const uint64_t before=e->voiceSeq.load(std::memory_order_acquire);if(before&1)continue;
        const int count=std::min<int>(capacity,e->voiceSnapshotCount.load(std::memory_order_relaxed));std::memcpy(out,e->voiceSnapshot.data(),size_t(count)*sizeof(STVoicePosition));const uint64_t published=e->voiceSerial;
        std::atomic_thread_fence(std::memory_order_acquire);if(e->voiceSeq.load(std::memory_order_relaxed)==before){if(serial)*serial=published;return count;}}
    return 0;}
void st_engine_queue_jump(STEngine*e,int32_t pattern,int32_t sequence){if(e)e->queue({12,pattern<0?-1:std::min(pattern,ST_MAX_PATTERNS-1),sequence<0?-1:std::min(sequence,4095)});}
void st_engine_queue_mode(STEngine*e,int32_t songMode,int32_t pattern,int32_t sequence){if(e)e->queue({16,songMode!=0,pattern<0?-1:std::min(pattern,ST_MAX_PATTERNS-1),sequence<0?-1:std::min(sequence,4095)});}
void st_engine_play_count_in(STEngine*e,int32_t songMode,int32_t bars){if(e)e->queueTransport({13,songMode!=0,clamp(bars,1,4)},false);}
void st_engine_set_host_automation_map(STEngine*e,uint64_t map){if(e)e->edit().hostMap=map;}
void st_engine_refresh_automation(STEngine*e){if(e)e->automationRefreshRequested.store(true,std::memory_order_release);}
void st_engine_panic(STEngine*e){if(e)e->queue({6});}
int32_t st_engine_forget_external_voices(STEngine*e,int32_t channel){if(!e||channel<0||channel>=e->tb->channels)return 0;int32_t count=0;for(auto&v:e->voices)if(v.active&&v.external&&v.track==channel){v.offSent=true;e->kill(v);++count;}return count;}
void st_engine_render(STEngine*e,float*l,float*r,uint32_t n){if(!e){if(l)std::fill(l,l+n,0);if(r)std::fill(r,r+n,0);return;}e->cacheValid=false;e->releaseClockOrigin();e->process(l,r,n,false);}
static void renderCached(STEngine*e,int32_t bus,bool channel,uint64_t time,float*l,float*r,uint32_t n){
    auto silence=[&](){if(l)std::fill(l,l+n,0);if(r)std::fill(r,r+n,0);};
    if(!e||(!channel&&bus<0)||n>cacheCapacity||time>std::numeric_limits<uint64_t>::max()-n){silence();return;}
    if(!n)return;
    uint64_t epoch=e->cacheEpoch.load(std::memory_order_acquire);
    if(e->observedCacheEpoch!=epoch){e->cacheValid=false;e->observedCacheEpoch=epoch;e->resetInputRun();}
    if(!e->cacheValid){++e->cacheGeneration;e->releaseClockOrigin();if(!e->freezeTransitionPending||e->freezeNaturalTail)e->freezeTransition(ST_FREEZE_CAUSE_GRAPH_RESET);++e->timingEpoch;e->cacheBegin=time;e->cacheEnd=time;e->clockBegin=time;e->cacheValid=true;}
    const uint64_t end=time+n;
    // A discontinuity larger than retained history requires an explicit graph reset;
    // do not run an unbounded catch-up render on the real-time thread.
    if(time>e->cacheEnd&&time-e->cacheEnd>cacheCapacity){silence();return;}
    while(e->cacheEnd<end){
        uint64_t start=e->cacheEnd;uint32_t count=uint32_t(std::min<uint64_t>(maxBlock,end-start));
        int eventOffset=start>=time?int(start-time):-int(time-start);
        e->process(nullptr,nullptr,count,true,eventOffset,start);
        auto&T=*e->tb;
        for(int b=0;b<T.buses;++b){auto*ring=T.rings[size_t(b)].get();if(!ring)continue;
            if(T.busTouched[size_t(b)])e->writeRing(*ring,T.busLeft(b),T.busRight(b),start,count);else e->silenceRing(*ring,start);}
        e->cacheEnd+=count;if(e->cacheEnd-e->cacheBegin>cacheCapacity)e->cacheBegin=e->cacheEnd-cacheCapacity;if(e->cacheEnd-e->clockBegin>clockCapacity)e->clockBegin=e->cacheEnd-clockCapacity;
    }
    // Channel addressing resolves the bus with the tables just rendered, so a caller
    // keyed by channel never reads another channel's bus across a renumbering.
    if(channel)bus=bus<0?e->tb->base:bus<e->tb->channels?e->outputBus(bus):-1;
    if(bus<0){silence();return;}
    uint32_t lost=time<e->cacheBegin?uint32_t(std::min<uint64_t>(n,e->cacheBegin-time)):0;
    if(l&&lost)std::fill(l,l+lost,0);if(r&&lost)std::fill(r,r+lost,0);
    const auto*ring=bus<e->tb->buses?e->tb->rings[size_t(bus)].get():nullptr;
    // Frames at or after silentFrom (and any ring from another cache epoch) are silence.
    uint64_t audibleEnd=!ring||ring->generation!=e->cacheGeneration?time+lost:std::min<uint64_t>(std::max<uint64_t>(ring->silentFrom,time+lost),end);
    size_t count=size_t(audibleEnd-(time+lost)),index=size_t((time+lost)%cacheCapacity),first=std::min<size_t>(count,cacheCapacity-index);
    if(l){if(count){std::memcpy(l+lost,ring->l.data+index,first*sizeof(float));if(first<count)std::memcpy(l+lost+first,ring->l.data,(count-first)*sizeof(float));}std::fill(l+lost+count,l+n,0.f);}
    if(r){if(count){std::memcpy(r+lost,ring->r.data+index,first*sizeof(float));if(first<count)std::memcpy(r+lost+first,ring->r.data,(count-first)*sizeof(float));}std::fill(r+lost+count,r+n,0.f);}
}
uint64_t st_engine_pattern_epoch(STEngine*e){return e?e->control->patternEpoch:0;}
void st_engine_render_bus(STEngine*e,int32_t bus,uint64_t time,float*l,float*r,uint32_t n){renderCached(e,bus,false,time,l,r,n);}
void st_engine_render_channel(STEngine*e,int32_t channel,uint64_t time,float*l,float*r,uint32_t n){renderCached(e,channel,true,time,l,r,n);}
void st_engine_reset_render_cache(STEngine*e){if(e)e->cacheEpoch.fetch_add(1,std::memory_order_release);}
int32_t st_engine_clock_span(STEngine*e,int64_t graphFrame,uint32_t requested,int64_t latency,STClockSpan*out){
    if(!e||!out||requested<1||requested>ST_CLOCK_MAX_SPAN||latency<0||latency>(int64_t(1)<<30)||graphFrame<-(int64_t(1)<<52)||graphFrame>(int64_t(1)<<52))return ST_CLOCK_INVALID;
    int64_t content=graphFrame-latency;
    if(!e->cacheValid||e->observedCacheEpoch!=e->cacheEpoch.load(std::memory_order_acquire))return ST_ENGINE_CLOCK_NOT_PREPARED;
    if(e->cacheOriginValid&&content<int64_t(e->cacheOriginGraph))return e->prerollClock(graphFrame,content,requested,latency,out);
    if(content<0||uint64_t(content)<e->clockBegin)return ST_ENGINE_CLOCK_NOT_RETAINED;
    if(uint64_t(content)>=e->cacheEnd)return ST_ENGINE_CLOCK_NOT_PREPARED;
    STClockSpan result{};result.context=e->cachedClock[size_t(uint64_t(content)%clockCapacity)];result.context.graphFrame=graphFrame;result.context.contentGraphFrame=content;
    if(result.context.nextBoundaryFrame!=std::numeric_limits<int64_t>::max())result.context.nextBoundaryFrame+=latency;
    if(result.context.nextQuarterBeatFrame!=std::numeric_limits<int64_t>::max())result.context.nextQuarterBeatFrame+=latency;
    if(result.context.nextQuarterGridFrame!=std::numeric_limits<int64_t>::max())result.context.nextQuarterGridFrame+=latency;
    uint32_t count=uint32_t(std::min<uint64_t>(requested,std::min<uint64_t>(e->cacheEnd-uint64_t(content),uint64_t((int64_t(1)<<52)-graphFrame+1))));
    if(result.context.nextBoundaryFrame!=std::numeric_limits<int64_t>::max())count=uint32_t(std::min<int64_t>(count,std::max<int64_t>(0,result.context.nextBoundaryFrame-graphFrame)));
    for(uint32_t n=1;n<count;++n){const auto&next=e->cachedClock[size_t((uint64_t(content)+n)%clockCapacity)];if(next.epoch!=result.context.epoch||next.revision!=result.context.revision){count=n;result.context.nextBoundaryFrame=graphFrame+n;result.context.nextBoundaryKinds=ST_CLOCK_BOUNDARY_ORIGIN;break;}}
    result.frames=count;*out=result;return ST_CLOCK_OK;
}
int32_t st_engine_freeze_span(STEngine*e,int64_t graphFrame,uint32_t requested,int64_t latency,STFreezeSpan*out){
    if(!e||!out||requested<1||requested>ST_CLOCK_MAX_SPAN||latency<0||latency>(int64_t(1)<<30)||graphFrame<-(int64_t(1)<<52)||graphFrame>(int64_t(1)<<52))return ST_CLOCK_INVALID;
    auto*history=e->freezeHistory.load(std::memory_order_acquire);if(!history)return ST_FREEZE_NOT_CONFIGURED;
    if(!e->cacheValid||e->observedCacheEpoch!=e->cacheEpoch.load(std::memory_order_acquire))return ST_ENGINE_CLOCK_NOT_PREPARED;
    int64_t content=graphFrame-latency;STFreezeSpan result{};
    if(e->cacheOriginValid&&content<int64_t(e->cacheOriginGraph)){
        if(!e->cacheOriginFreezeValid||int64_t(e->cacheOriginGraph)-content>(int64_t(1)<<30))return ST_ENGINE_CLOCK_NOT_RETAINED;
        result.context=e->cacheOriginFreeze;result.context.phase=ST_FREEZE_STOPPED;result.context.flags|=ST_FREEZE_PREROLL;
        result.frames=uint32_t(std::min<int64_t>(requested,std::min<int64_t>(int64_t(e->cacheOriginGraph)-content,(int64_t(1)<<52)-graphFrame+1)));
    }else{
        if(content<0||uint64_t(content)<e->clockBegin)return ST_ENGINE_CLOCK_NOT_RETAINED;
        if(uint64_t(content)>=e->cacheEnd)return ST_ENGINE_CLOCK_NOT_PREPARED;
        const auto&first=history->frames[size_t(uint64_t(content)%clockCapacity)];if(first.contentGraphFrame!=content)return ST_ENGINE_CLOCK_NOT_RETAINED;
        result.context=first;result.frames=uint32_t(std::min<uint64_t>(requested,std::min<uint64_t>(e->cacheEnd-uint64_t(content),uint64_t((int64_t(1)<<52)-graphFrame+1))));
        bool advancing=first.phase==ST_FREEZE_SONG||first.phase==ST_FREEZE_NATURAL_TAIL;
        for(uint32_t n=1;n<result.frames;++n){const auto&next=history->frames[size_t((uint64_t(content)+n)%clockCapacity)];
            if(next.contentGraphFrame!=content+n||next.publicationID!=first.publicationID||next.playbackID!=first.playbackID||next.transitionID!=first.transitionID||next.phase!=first.phase||next.cause!=first.cause||next.flags!=first.flags||next.projectFrame!=first.projectFrame+(advancing?n:0)){result.frames=n;break;}
        }
    }
    result.context.graphFrame=graphFrame;result.context.contentGraphFrame=content;*out=result;return ST_CLOCK_OK;
}
int32_t st_engine_pop_event(STEngine*e,STEvent*event){return e&&event&&e->events.pop(*event);}
STStatus st_engine_status(STEngine*e){STStatus s{};if(!e)return s;
    for(;;){const auto publication=e->statusPublication.load(std::memory_order_seq_cst);if(publication&1)continue;
    const uint32_t requested=e->transportRequested.load(std::memory_order_acquire),applied=e->statusTransportApplied.load(std::memory_order_acquire);
    s.transportSeekRevision=e->statusSeekRevision.load(std::memory_order_relaxed);s.transportPending=0;s.requestedPlaying=0;
    s.playing=e->statusPlaying.load(std::memory_order_relaxed);s.recording=e->statusRecording.load(std::memory_order_relaxed);s.pattern=e->statusPattern.load(std::memory_order_relaxed);s.row=e->statusRow.load(std::memory_order_relaxed);s.sequence=e->statusSequence.load(std::memory_order_relaxed);s.trackCount=e->statusTracks.load(std::memory_order_relaxed);s.peakL=e->peakL.load(std::memory_order_relaxed);s.peakR=e->peakR.load(std::memory_order_relaxed);s.position=e->statusPosition.load(std::memory_order_relaxed);s.seconds=e->statusSeconds.load(std::memory_order_relaxed);s.droppedCommands=e->droppedCommands.load(std::memory_order_relaxed);s.droppedEvents=e->droppedEvents.load(std::memory_order_relaxed);s.songMode=e->statusSongMode.load(std::memory_order_relaxed);s.beat=e->statusBeat.load(std::memory_order_relaxed);s.bpm=e->statusBpm.load(std::memory_order_relaxed);s.barStartBeat=e->statusBarStart.load(std::memory_order_relaxed);s.bar=e->statusBar.load(std::memory_order_relaxed);s.numerator=e->statusNumerator.load(std::memory_order_relaxed);s.denominator=e->statusDenominator.load(std::memory_order_relaxed);s.timingRevision=e->statusTimingRevision.load(std::memory_order_relaxed);s.queuedPattern=e->statusQueuedPattern.load(std::memory_order_relaxed);s.queuedSequence=e->statusQueuedSequence.load(std::memory_order_relaxed);s.countIn=e->statusCountIn.load(std::memory_order_relaxed);
    // A queued Play/Stop/count-in not yet rendered: the rendered fields are stale until it is; see st_status_transport_playing.
    if(int32_t(requested-applied)>0){s.transportPending=1;s.requestedPlaying=e->transportRequestedPlaying.load(std::memory_order_relaxed);}
    std::atomic_thread_fence(std::memory_order_seq_cst);if(publication==e->statusPublication.load(std::memory_order_seq_cst))return s;}}
float st_engine_track_peak(STEngine*e,int32_t t){if(!e||t<0)return 0;auto*store=e->peakStore.load(std::memory_order_acquire);return store&&size_t(t)<store->count?store->values[size_t(t)].load(std::memory_order_relaxed):0;}
double st_engine_duration(STEngine*e,int32_t song){if(!e)return 0;auto&p=*e->control;if(song)return e->songDuration(p);return p.timing?double(p.mappedPatternEnd-p.mappedPatternStart)/e->sampleRate:e->patternRows(p,p.activePattern)*60/p.bpm/p.lpb;}
}

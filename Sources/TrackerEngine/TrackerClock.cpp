#include "TrackerClock.h"
#include "ClockTimingMap.hpp"
#include <algorithm>
#include <cmath>
#include <cstring>
#include <limits>
#include <memory>
#include <stdexcept>
#include <vector>

namespace {
using supertracker::clock_detail::TimingMap;
using supertracker::clock_detail::TempoEvent;
using supertracker::clock_detail::MeterEvent;
constexpr int64_t graphLimit = int64_t(1) << 52;
constexpr int64_t latencyLimit = int64_t(1) << 30;
struct Boundary { double beat; int64_t frame; uint32_t kinds; };
struct ClockData {
    double rate;
    uint64_t revision;
    TimingMap map;
    int64_t endFrame;
    std::vector<Boundary> boundaries;
    ClockData(double sampleRate,const std::vector<TempoEvent>&tempos,const std::vector<MeterEvent>&meters,uint64_t version)
        : rate(sampleRate),revision(version),map(tempos,meters),endFrame(int64_t(std::ceil(map.maxSeconds()*rate))) {
        boundaries.reserve(tempos.size()+meters.size());
        size_t t=0,m=0;
        while(t<tempos.size()||m<meters.size()) {
            double beat=t<tempos.size()?tempos[t].beat:std::numeric_limits<double>::infinity();
            if(m<meters.size())beat=std::min(beat,meters[m].beat);
            uint32_t kinds=0;
            if(t<tempos.size()&&tempos[t].beat==beat){kinds|=ST_CLOCK_BOUNDARY_TEMPO;++t;}
            if(m<meters.size()&&meters[m].beat==beat){kinds|=ST_CLOCK_BOUNDARY_METER;++m;}
            const int64_t frame=rawFrameAtBeat(beat);
            if(!boundaries.empty()&&frame<=boundaries.back().frame)
                throw std::invalid_argument("Distinct tempo/meter positions collapse onto one audio sample; move them apart or use the same quarter-beat position.");
            boundaries.push_back({beat,frame,kinds});
        }
    }
    int64_t initialFrameAtBeat(double beat)const noexcept {
        int64_t frame=int64_t(std::ceil(beat*(60*rate)/map.tempos().front().bpm));
        // Far preroll can approach 2^53 elapsed frames even though each graph
        // coordinate is bounded to 2^52. Its floating ULP can span a few frames.
        for(int correction=0;correction<8;++correction){
            if(beatAtFrame(frame-1)>=beat)--frame;
            else if(beatAtFrame(frame)<beat)++frame;
            else break;
        }
        return frame;
    }
    int64_t rawFrameAtBeat(double beat)const noexcept{return int64_t(std::ceil(*map.secondsAtBeat(beat)*rate));}
    int64_t frameAtBeat(double beat)const noexcept {
        int64_t frame=rawFrameAtBeat(beat);
        // At this bounded domain, floating inverse error is much smaller than one
        // sample. Exact neighboring comparisons make sampled round trips stable
        // without an epsilon that could swallow an intentionally late event.
        for(int correction=0;correction<2;++correction){
            if(frame>0&&beatAtFrame(frame-1)>=beat)--frame;
            else if(frame<endFrame&&beatAtFrame(frame)<beat)++frame;
            else break;
        }
        return frame;
    }
    double beatAtFrame(int64_t frame)const noexcept {
        if(frame<0)return double(frame)*map.tempos().front().bpm/(60*rate);
        if(frame==endFrame)return TimingMap::maxBeat;
        // Invert directly in sample coordinates. Rounding frame/rate first can
        // place an exactly aligned beat just below itself (64000/48000 after a
        // 60->180 BPM change), incorrectly delaying it in frameAtBeat's strict
        // neighboring comparison. FMA also retains fractional prefix samples.
        const auto&segments=map.tempos();
        const auto nextTempo=std::upper_bound(segments.begin(),segments.end(),frame,
            [&](int64_t value,const auto&segment){return double(value)<std::ceil(segment.seconds*rate);});
        const auto&segment=*(nextTempo-1);
        const double deltaFrames=std::fma(-segment.seconds,rate,double(frame));
        double beat=std::fma(deltaFrames,segment.bpm/(60*rate),segment.beat);
        // Compiled sample boundaries are authoritative. These one-sided corrections
        // resolve only floating-point inverse rounding, never epsilon-snap input.
        const auto next=std::upper_bound(boundaries.begin(),boundaries.end(),frame,
            [](int64_t value,const Boundary&b){return value<b.frame;});
        if(next!=boundaries.begin())beat=std::max(beat,(next-1)->beat);
        if(next!=boundaries.end()&&beat>=next->beat)beat=std::nextafter(next->beat,-std::numeric_limits<double>::infinity());
        return beat;
    }
};
bool validFrame(int64_t frame)noexcept{return frame>=-graphLimit&&frame<=graphLimit;}
bool validBeat(double beat)noexcept{return std::isfinite(beat)&&beat>=0&&beat<=TimingMap::maxBeat;}
void errorText(char*out,uint32_t capacity,const char*message)noexcept {
    if(!out||!capacity)return;const auto count=std::min<size_t>(std::strlen(message),size_t(capacity-1));std::memcpy(out,message,count);out[count]=0;
}
}
struct STClockMap { std::shared_ptr<const ClockData> data; };
struct STClockTimeline {
    std::shared_ptr<const ClockData> data;
    STClockOrigin origin;
    int64_t startFrame,loopStart,loopEnd;
};

extern "C" {
STClockMap*st_clock_map_create(double rate,const STClockTempoEvent*tempos,uint32_t tempoCount,const STClockMeterEvent*meters,uint32_t meterCount,uint64_t revision,char*error,uint32_t capacity) {
    errorText(error,capacity,"");
    try {
        if(!std::isfinite(rate)||rate<8000||rate>384000)throw std::invalid_argument("Sample rate must be finite and within 8000..384000 Hz.");
        if(!tempos||!meters||tempoCount<1||tempoCount>ST_CLOCK_MAX_EVENTS||meterCount<1||meterCount>ST_CLOCK_MAX_EVENTS)
            throw std::invalid_argument("Provide 1..1024 tempo and meter events, beginning at quarter beat zero.");
        std::vector<TempoEvent> t;std::vector<MeterEvent> m;t.reserve(tempoCount);m.reserve(meterCount);
        for(uint32_t i=0;i<tempoCount;++i)t.push_back({tempos[i].beat,tempos[i].bpm});
        for(uint32_t i=0;i<meterCount;++i)m.push_back({meters[i].beat,meters[i].numerator,meters[i].denominator});
        auto data=std::make_shared<const ClockData>(rate,t,m,revision);
        return new STClockMap{std::move(data)};
    }catch(const std::exception&e){errorText(error,capacity,e.what());}catch(...){errorText(error,capacity,"Unable to create timing map.");}
    return nullptr;
}
void st_clock_map_destroy(STClockMap*map){delete map;}
int32_t st_clock_frame_at_beat(const STClockMap*map,double beat,int64_t*frame){if(!map||!frame)return ST_CLOCK_INVALID;if(!validBeat(beat))return ST_CLOCK_OUTSIDE_DOMAIN;*frame=map->data->frameAtBeat(beat);return ST_CLOCK_OK;}
int32_t st_clock_beat_at_frame(const STClockMap*map,int64_t frame,double*beat){if(!map||!beat)return ST_CLOCK_INVALID;if(!validFrame(frame)||frame>map->data->endFrame)return ST_CLOCK_OUTSIDE_DOMAIN;*beat=map->data->beatAtFrame(frame);return ST_CLOCK_OK;}
int32_t st_clock_seconds_at_beat(const STClockMap*map,double beat,double*seconds){if(!map||!seconds)return ST_CLOCK_INVALID;const auto value=map->data->map.secondsAtBeat(beat);if(!value)return ST_CLOCK_OUTSIDE_DOMAIN;*seconds=*value;return ST_CLOCK_OK;}
int32_t st_clock_beat_at_seconds(const STClockMap*map,double seconds,double*beat){if(!map||!beat)return ST_CLOCK_INVALID;const auto value=map->data->map.beatAtSeconds(seconds);if(!value)return ST_CLOCK_OUTSIDE_DOMAIN;*beat=*value;return ST_CLOCK_OK;}
STClockTimeline*st_clock_timeline_create(const STClockMap*map,STClockOrigin origin,char*error,uint32_t capacity) {
    errorText(error,capacity,"");
    try {
        if(!map)throw std::invalid_argument("A timing map is required.");
        if(!validFrame(origin.graphOriginFrame)||!validBeat(origin.startBeat))throw std::invalid_argument("Graph origin must be within +/-2^52 frames and seek beat within 0..1000000.");
        const auto&data=*map->data;int64_t start=data.frameAtBeat(origin.startBeat),loopStart=0,loopEnd=0;
        if(origin.loopEnabled){
            if(!validBeat(origin.loopStartBeat)||!validBeat(origin.loopEndBeat)||origin.loopStartBeat>=origin.loopEndBeat||origin.startBeat>=origin.loopEndBeat)
                throw std::invalid_argument("Loop requires 0<=start<end<=1000000 and a seek position before its end.");
            loopStart=data.frameAtBeat(origin.loopStartBeat);loopEnd=data.frameAtBeat(origin.loopEndBeat);
            if(loopStart>=loopEnd||start>=loopEnd)throw std::invalid_argument("Loop/seek positions collapse onto the same audio sample.");
        }
        origin.playing=origin.playing!=0;origin.loopEnabled=origin.loopEnabled!=0;
        return new STClockTimeline{map->data,origin,start,loopStart,loopEnd};
    }catch(const std::exception&e){errorText(error,capacity,e.what());}catch(...){errorText(error,capacity,"Unable to create clock timeline.");}
    return nullptr;
}
void st_clock_timeline_destroy(STClockTimeline*timeline){delete timeline;}
int32_t st_clock_context(const STClockTimeline*timeline,int64_t graphFrame,int64_t latency,STClockContext*out) {
    if(!timeline||!out||latency<0||latency>latencyLimit)return ST_CLOCK_INVALID;
    if(!validFrame(graphFrame))return ST_CLOCK_OUTSIDE_DOMAIN;
    const auto&data=*timeline->data;const auto&origin=timeline->origin;
    STClockContext c{};c.revision=data.revision;c.epoch=origin.epoch;c.graphFrame=graphFrame;c.contentGraphFrame=graphFrame-latency;c.sampleRate=data.rate;
    const int64_t elapsed=origin.playing?c.contentGraphFrame-origin.graphOriginFrame:0;
    c.songFrame=timeline->startFrame+elapsed;
    if(origin.playing)c.flags|=ST_CLOCK_PLAYING;
    if(origin.loopEnabled)c.flags|=ST_CLOCK_LOOPING;
    if(origin.playing&&elapsed<0)c.flags|=ST_CLOCK_PREROLL;
    if(origin.loopEnabled&&elapsed>=0&&c.songFrame>=timeline->loopEnd){
        const int64_t width=timeline->loopEnd-timeline->loopStart,distance=c.songFrame-timeline->loopEnd;
        c.loopIteration=1+distance/width;c.songFrame=timeline->loopStart+distance%width;
    }
    if(c.songFrame>data.endFrame)return ST_CLOCK_OUTSIDE_DOMAIN;
    c.beat=data.beatAtFrame(c.songFrame);
    if(c.songFrame<0){
        const auto&meter=data.map.meters().front();c.bpm=data.map.tempos().front().bpm;c.numerator=meter.numerator;c.denominator=meter.denominator;
        auto offset=int64_t(std::floor(c.beat/meter.quartersPerBar));
        // Negative preroll has no compiled meter segment. Use absolute sample
        // boundaries here too: an inverse a fraction below an exact bar line
        // must not select a completed bar and then skip its end entirely.
        for(int correction=0;correction<2;++correction){
            const int64_t start=data.initialFrameAtBeat(double(offset)*meter.quartersPerBar);
            const int64_t end=data.initialFrameAtBeat(double(offset+1)*meter.quartersPerBar);
            if(c.songFrame<start)--offset;
            else if(c.songFrame>=end)++offset;
            else break;
        }
        c.bar=offset+1;c.barStartBeat=double(offset)*meter.quartersPerBar;c.barEndBeat=double(offset+1)*meter.quartersPerBar;
        c.beat=std::max(c.beat,c.barStartBeat);
        if(c.beat>=c.barEndBeat)c.beat=std::nextafter(c.barEndBeat,-std::numeric_limits<double>::infinity());
    }else{
        c.bpm=*data.map.tempoAtBeat(c.beat);const auto bar=*data.map.barAtBeat(c.beat);
        c.bar=int64_t(bar.bar);c.barStartBeat=bar.barStart;c.barEndBeat=bar.barEnd;c.numerator=bar.numerator;c.denominator=bar.denominator;
    }
    c.nextBoundaryFrame=std::numeric_limits<int64_t>::max();
    c.nextQuarterBeatFrame=std::numeric_limits<int64_t>::max();
    c.nextQuarterGridFrame=std::numeric_limits<int64_t>::max();
    c.loopStartBeat=origin.loopEnabled?origin.loopStartBeat:0;
    c.loopEndBeat=origin.loopEnabled?origin.loopEndBeat:0;
    if(c.songFrame==data.endFrame)c.flags|=ST_CLOCK_DOMAIN_END;
    double quarter=std::floor(c.beat);
    auto frameForQuarter=[&](double beat){return beat<0?data.initialFrameAtBeat(beat):data.frameAtBeat(beat);};
    int64_t quarterFrame=frameForQuarter(quarter);
    if(quarterFrame<c.songFrame){quarter+=1;if(quarter<=TimingMap::maxBeat)quarterFrame=frameForQuarter(quarter);}
    if(quarter<=TimingMap::maxBeat&&quarterFrame>=c.songFrame)c.nextQuarterGridFrame=graphFrame+quarterFrame-c.songFrame;
    if(origin.playing){
        int64_t delta=std::numeric_limits<int64_t>::max();uint32_t kinds=0;
        const auto consider=[&](int64_t distance,uint32_t flags){if(distance<0)return;if(distance<delta){delta=distance;kinds=flags;}else if(distance==delta)kinds|=flags;};
        const auto next=std::upper_bound(data.boundaries.begin(),data.boundaries.end(),c.songFrame,[](int64_t frame,const Boundary&b){return frame<b.frame;});
        if(next!=data.boundaries.end())consider(next->frame-c.songFrame,next->kinds);
        if(c.barEndBeat<=TimingMap::maxBeat){
            const int64_t barEnd=c.barEndBeat<0?data.initialFrameAtBeat(c.barEndBeat):data.frameAtBeat(c.barEndBeat);
            if(barEnd>c.songFrame)consider(barEnd-c.songFrame,ST_CLOCK_BOUNDARY_BAR);
        }
        consider(data.endFrame-c.songFrame,ST_CLOCK_BOUNDARY_DOMAIN);
        if(origin.loopEnabled)consider(timeline->loopEnd-c.songFrame,ST_CLOCK_BOUNDARY_LOOP);
        if(elapsed<0)consider(-elapsed,ST_CLOCK_BOUNDARY_ORIGIN);
        if(delta!=std::numeric_limits<int64_t>::max()){c.nextBoundaryFrame=graphFrame+delta;c.nextBoundaryKinds=kinds;}
        if(origin.loopEnabled&&elapsed>=0&&quarterFrame>=timeline->loopEnd){
            const double first=std::ceil(origin.loopStartBeat);
            if(first<origin.loopEndBeat){
                const int64_t firstFrame=frameForQuarter(first);
                if(firstFrame<timeline->loopEnd)c.nextQuarterBeatFrame=graphFrame+timeline->loopEnd-c.songFrame+firstFrame-timeline->loopStart;
            }
        }else if(quarter<=TimingMap::maxBeat&&quarterFrame>=c.songFrame)c.nextQuarterBeatFrame=graphFrame+quarterFrame-c.songFrame;
    }
    if(c.nextQuarterBeatFrame==std::numeric_limits<int64_t>::max())c.flags|=ST_CLOCK_GRID_PROJECTION;
    *out=c;return ST_CLOCK_OK;
}
int32_t st_clock_span(const STClockTimeline*timeline,int64_t graphFrame,uint32_t requested,int64_t latency,STClockSpan*out) {
    if(!out||requested<1||requested>ST_CLOCK_MAX_SPAN)return ST_CLOCK_INVALID;
    STClockSpan span{};const int32_t result=st_clock_context(timeline,graphFrame,latency,&span.context);if(result!=ST_CLOCK_OK)return result;
    int64_t count=std::min<int64_t>(requested,graphLimit-graphFrame+1);
    if(span.context.nextBoundaryFrame!=std::numeric_limits<int64_t>::max())count=std::min(count,span.context.nextBoundaryFrame-graphFrame);
    span.frames=uint32_t(std::max<int64_t>(0,count));*out=span;return ST_CLOCK_OK;
}
}

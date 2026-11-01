#include "TrackerClock.h"
#include <algorithm>
#include <atomic>
#include <cassert>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <new>
#include <thread>
#include <vector>

static thread_local bool querying=false;
static std::atomic<unsigned> queryAllocations{0};
void*operator new(std::size_t size){if(querying)++queryAllocations;if(auto*p=std::malloc(size))return p;throw std::bad_alloc();}
void*operator new[](std::size_t size){return ::operator new(size);}
void operator delete(void*p)noexcept{std::free(p);}
void operator delete[](void*p)noexcept{std::free(p);}
struct Map {
    STClockMap*p=nullptr;
    Map(std::vector<STClockTempoEvent> tempos={{0,120}},std::vector<STClockMeterEvent> meters={{0,4,4}},double rate=48000,uint64_t revision=1){char error[256];p=st_clock_map_create(rate,tempos.data(),uint32_t(tempos.size()),meters.data(),uint32_t(meters.size()),revision,error,sizeof error);if(!p)std::fprintf(stderr,"Map: %s\n",error);assert(p);}
    ~Map(){st_clock_map_destroy(p);}Map(const Map&)=delete;Map&operator=(const Map&)=delete;operator STClockMap*()const{return p;}
};
struct Timeline {
    STClockTimeline*p=nullptr;
    Timeline(STClockMap*map,STClockOrigin origin={0,0,1,0,0,0,1}){char error[256];p=st_clock_timeline_create(map,origin,error,sizeof error);if(!p)std::fprintf(stderr,"Timeline: %s\n",error);assert(p);}
    ~Timeline(){st_clock_timeline_destroy(p);}operator STClockTimeline*()const{return p;}
};
static int64_t frame(STClockMap*m,double beat){int64_t result=-1;assert(st_clock_frame_at_beat(m,beat,&result)==ST_CLOCK_OK);return result;}
static double beat(STClockMap*m,int64_t frame){double result=0;assert(st_clock_beat_at_frame(m,frame,&result)==ST_CLOCK_OK);return result;}
static STClockContext context(STClockTimeline*t,int64_t frame,int64_t latency=0){STClockContext result{};auto status=st_clock_context(t,frame,latency,&result);if(status!=ST_CLOCK_OK)std::fprintf(stderr,"context frame=%lld latency=%lld status=%d\n",(long long)frame,(long long)latency,status);assert(status==ST_CLOCK_OK);assert(std::isfinite(result.beat)&&std::isfinite(result.bpm)&&std::isfinite(result.barStartBeat)&&std::isfinite(result.barEndBeat));return result;}
static STClockSpan span(STClockTimeline*t,int64_t frame,uint32_t count,int64_t latency=0){STClockSpan result{};assert(st_clock_span(t,frame,count,latency,&result)==ST_CLOCK_OK);return result;}
static void testConversionsAndMeters(){Map map({{0,120},{4,60}},{{0,4,4},{6,5,8}});assert(frame(map,0)==0&&frame(map,4)==96000&&frame(map,6)==192000&&frame(map,8.5)==312000);assert(beat(map,192000)==6);Timeline timeline(map);auto a=context(timeline,96000);assert(a.bpm==60&&a.beat==4&&a.numerator==4&&a.bar==2&&a.barStartBeat==4&&a.barEndBeat==6);auto b=context(timeline,192000);assert(b.beat==6&&b.numerator==5&&b.denominator==8&&b.bar==3&&b.barStartBeat==6&&b.barEndBeat==8.5);auto c=context(timeline,312000);assert(c.bar==4&&c.barStartBeat==8.5);double seconds=0,roundtrip=0;assert(st_clock_seconds_at_beat(map,6,&seconds)==1&&seconds==4);assert(st_clock_beat_at_seconds(map,seconds,&roundtrip)==1&&roundtrip==6);}
static void testCeilAndAtomicBoundaries(){Map plain;assert(frame(plain,.25)==6000);assert(frame(plain,std::nextafter(.25,1.0))==6001);assert(frame(plain,std::nextafter(.25,0.0))==6000);const double eventBeat=123.25/24000;Map map({{0,120},{eventBeat,60}},{{0,4,4},{eventBeat,5,8}});Timeline timeline(map);assert(frame(map,eventBeat)==124);auto before=context(timeline,123),after=context(timeline,124);assert(before.bpm==120&&before.numerator==4&&after.bpm==60&&after.numerator==5&&after.denominator==8&&after.beat>=eventBeat);auto part=span(timeline,0,4096);assert(part.frames==124&&(part.context.nextBoundaryKinds&ST_CLOCK_BOUNDARY_TEMPO)&&(part.context.nextBoundaryKinds&ST_CLOCK_BOUNDARY_METER));assert(after.beat-eventBeat<60.0/(60*48000));}
static void testPreroll(){Map map;Timeline timeline(map);auto a=context(timeline,-1);assert(a.flags&ST_CLOCK_PREROLL);assert(a.flags&ST_CLOCK_PLAYING);assert(a.songFrame==-1&&a.bar==0&&a.barStartBeat==-4&&a.barEndBeat==0&&a.beat<0);auto b=context(timeline,-96001);assert(b.bar==-1&&b.barStartBeat==-8&&b.barEndBeat==-4);auto latency=span(timeline,0,4096,64);assert(latency.frames==64&&latency.context.songFrame==-64&&latency.context.nextBoundaryFrame==64&&(latency.context.nextBoundaryKinds&ST_CLOCK_BOUNDARY_ORIGIN));auto atZero=context(timeline,64,64);assert(!(atZero.flags&ST_CLOCK_PREROLL)&&atZero.beat==0&&atZero.contentGraphFrame==0);}
static void testPrerollBarBoundaries(){
    Map exact({{0,400}},{{0,1,16}},8000);Timeline timeline(exact);
    auto at=context(timeline,-156300);assert(at.barStartBeat==-130.25&&at.beat>=at.barStartBeat);
    auto part=span(timeline,-156300,65536);assert(part.frames>0&&part.frames<=301);
    assert(context(timeline,int64_t(-156300)+part.frames-1).bar==at.bar);
    assert(context(timeline,int64_t(-156300)+part.frames).bar!=at.bar);
    for(double rate:{8000.,44100.,48000.,384000.})for(double bpm:{20.,119.99,123.,400.}){
        Map map({{0,bpm}},{{0,1,16}},rate);
        for(int64_t origin:{int64_t(0),int64_t(1)<<52}){
            Timeline current(map,{origin,0,1,0,0,0,1});
            for(int64_t query:{-156301LL,-156300LL,-156299LL,-1LL,-(1LL<<52),-(1LL<<52)+2048}){
                auto range=span(current,query,65536,int64_t(1)<<30);
                assert(range.frames>0);
                auto last=context(current,query+range.frames-1,int64_t(1)<<30);
                assert(last.bar==range.context.bar&&last.barStartBeat==range.context.barStartBeat);
                if(range.context.nextBoundaryKinds&ST_CLOCK_BOUNDARY_BAR)
                    assert(context(current,range.context.nextBoundaryFrame,int64_t(1)<<30).bar!=range.context.bar);
            }
        }
    }
}
static void testSeekLoopAndLatency(){Map map({{0,120},{4,60}},{{0,4,4},{6,5,8}},48000,71);Timeline timeline(map,{100000,6,1,1,4,8,22});auto start=context(timeline,100000);auto delayed=context(timeline,100064,64);assert(start.songFrame==192000&&start.beat==6&&start.epoch==22&&start.revision==71);assert(delayed.songFrame==start.songFrame&&delayed.beat==start.beat&&delayed.barStartBeat==start.barStartBeat&&delayed.contentGraphFrame==start.contentGraphFrame);auto wrap=context(timeline,196000);assert(wrap.songFrame==96000&&wrap.beat==4&&wrap.loopIteration==1&&wrap.bpm==60);auto second=context(timeline,388000);assert(second.songFrame==96000&&second.loopIteration==2);auto before=span(timeline,195999,4096);assert(before.frames==1&&(before.context.nextBoundaryKinds&ST_CLOCK_BOUNDARY_LOOP));auto historical=context(timeline,99999);assert(historical.loopIteration==0&&(historical.flags&ST_CLOCK_PREROLL)&&historical.songFrame==191999);Timeline intro(map,{0,0,1,1,4,8,23});assert(context(intro,0).beat==0&&context(intro,288000).beat==4);auto far=context(timeline,int64_t(1)<<52);assert(far.loopIteration>1000000&&far.songFrame>=96000&&far.songFrame<288000);}
static void testStoppedAndImmutableLifetime(){STClockTempoEvent tempos[]={{0,120}};STClockMeterEvent meters[]={{0,4,4}};auto*map=st_clock_map_create(48000,tempos,1,meters,1,9,nullptr,0);assert(map);Timeline paused(map,{1000,3.125,0,0,0,0,41});st_clock_map_destroy(map);auto a=context(paused,-1000,64),b=context(paused,500000,257);assert(a.songFrame==b.songFrame&&a.beat==3.125&&b.beat==a.beat&&!(b.flags&ST_CLOCK_PLAYING)&&!(b.flags&ST_CLOCK_PREROLL)&&a.revision==9&&a.epoch==41);assert(span(paused,1000,4096).frames==4096);Map newer({{0,60}},{{0,7,8}},48000,10);Timeline next(newer,{1000,3.125,1,0,0,0,42});assert(context(next,1100).revision==10&&context(paused,1100).revision==9&&context(paused,1100).bpm==120);}
static void testValidationAndUntouchedOutputs(){STClockTempoEvent tempo[]={{0,120}};STClockMeterEvent meter[]={{0,4,4}};char error[256];auto invalid=[&](double rate,const STClockTempoEvent*t,uint32_t n,const STClockMeterEvent*m,uint32_t count){auto*p=st_clock_map_create(rate,t,n,m,count,0,error,sizeof error);assert(!p&&error[0]);};invalid(NAN,tempo,1,meter,1);invalid(7999,tempo,1,meter,1);invalid(48000,nullptr,1,meter,1);invalid(48000,tempo,0,meter,1);invalid(48000,tempo,1025,meter,1);STClockTempoEvent order[]={{0,120},{2,60},{1,90}};invalid(48000,order,3,meter,1);STClockTempoEvent value[]={{0,INFINITY}};invalid(48000,value,1,meter,1);STClockMeterEvent malformed[]={{0,5,3}};invalid(48000,tempo,1,malformed,1);STClockTempoEvent collapsed[]={{0,120},{1e-8,100},{2e-8,80}};invalid(48000,collapsed,3,meter,1);assert(std::strstr(error,"collapse"));char tiny='x';assert(!st_clock_map_create(0,nullptr,0,nullptr,0,0,&tiny,1)&&tiny==0);Map good;STClockOrigin origin{0,0,1,1,1,1,0};assert(!st_clock_timeline_create(good,origin,error,sizeof error));origin={0,0,1,1,1e-8,2e-8,0};assert(!st_clock_timeline_create(good,origin,error,sizeof error)&&std::strstr(error,"collapse"));origin={0,NAN,0,0,0,0,0};assert(!st_clock_timeline_create(good,origin,error,sizeof error));Timeline timeline(good);STClockContext output{};output.beat=123;assert(st_clock_context(timeline,0,-1,&output)==ST_CLOCK_INVALID&&output.beat==123);assert(st_clock_context(timeline,std::numeric_limits<int64_t>::max(),0,&output)==ST_CLOCK_OUTSIDE_DOMAIN&&output.beat==123);int64_t f=42;assert(st_clock_frame_at_beat(good,NAN,&f)==ST_CLOCK_OUTSIDE_DOMAIN&&f==42);double b=42;assert(st_clock_beat_at_seconds(good,-1,&b)==ST_CLOCK_OUTSIDE_DOMAIN&&b==42);STClockSpan range{};range.frames=99;assert(st_clock_span(timeline,0,0,0,&range)==ST_CLOCK_INVALID&&range.frames==99);assert(st_clock_span(timeline,0,65537,0,&range)==ST_CLOCK_INVALID);}
static void testDomainExtremes(){for(double rate:{8000.,384000.})for(double bpm:{20.,400.}){Map map({{0,bpm}},{{0,1,16}},rate);const int64_t end=frame(map,1000000);assert(beat(map,end)==1000000);Timeline timeline(map);auto final=span(timeline,end,4096);assert(final.frames==0&&(final.context.flags&ST_CLOCK_DOMAIN_END));STClockContext c{};assert(st_clock_context(timeline,end+1,0,&c)==ST_CLOCK_OUTSIDE_DOMAIN);auto extreme=context(timeline,-(int64_t(1)<<52),int64_t(1)<<30);assert(std::isfinite(extreme.beat)&&extreme.bar<=0&&extreme.nextBoundaryFrame>extreme.graphFrame);Timeline paused(map,{0,1000000,0,0,0,0,0});assert(span(paused,0,4096).frames==4096);}}
static void testSpanPartitioning(){Map map({{0,120},{.17066666666666666,60},{.6,180}},{{0,4,4},{.17066666666666666,5,8},{.4,3,4},{.6,7,8}});Timeline timeline(map,{1000,0,1,1,0,.9,51});for(uint32_t size:{1u,7u,127u,512u,4096u,65536u}){int64_t cursor=0;while(cursor<200000){auto piece=span(timeline,cursor,uint32_t(std::min<int64_t>(size,200000-cursor)),257);assert(piece.frames>0&&piece.frames<=size);const auto&first=piece.context;auto last=context(timeline,cursor+piece.frames-1,257);assert(first.bpm==last.bpm&&first.numerator==last.numerator&&first.denominator==last.denominator&&first.barStartBeat==last.barStartBeat&&first.loopIteration==last.loopIteration&&((first.flags^last.flags)&ST_CLOCK_PREROLL)==0);double expected=first.beat+double(piece.frames-1)*first.bpm/(60*first.sampleRate);assert(std::abs(last.beat-expected)<2e-9);cursor+=piece.frames;}}}
static void testRoundTripsAndRenderQueries(){std::vector<STClockTempoEvent> tempos;std::vector<STClockMeterEvent> meters;for(int n=0;n<1024;++n){tempos.push_back({n*900.0,20.+double(n%381)});meters.push_back({n*900.0,1+n%16,(n%4==0?2:n%4==1?4:n%4==2?8:16)});}Map map(tempos,meters,384000,8);Timeline timeline(map,{0,0,1,1,0,1000000,99});uint64_t random=0x8715692;querying=true;for(int n=0;n<30000;++n){random=random*6364136223846793005ULL+1;double original=double(random>>11)/9007199254740992.0*1000000;int64_t sample=frame(map,original);double after=beat(map,sample);assert(after>=original&&after-original<=400./(60*384000)+2e-9);int64_t sampleAgain=frame(map,after);assert(sampleAgain==sample);double seconds=0;assert(st_clock_seconds_at_beat(map,original,&seconds)==1);assert(double(sample)/384000-seconds<1./384000+2e-9);auto c=context(timeline,sample);assert(c.revision==8);auto range=span(timeline,sample,4096);assert(range.frames>0);}querying=false;
    auto reader=[&]{querying=true;for(int n=0;n<100000;++n){auto c=context(timeline,int64_t(n)*13,64);assert(c.epoch==99);auto range=span(timeline,int64_t(n)*13,127,64);assert(range.frames>0);}querying=false;};std::thread a(reader),b(reader);a.join();b.join();assert(queryAllocations.load()==0);
}
static void testQuarterBeatAndLoopContext(){
    Map map({{0,120},{.375,60}},{{0,4,4},{.375,5,8}});Timeline timeline(map);
    assert(context(timeline,0).nextQuarterBeatFrame==0);
    assert(context(timeline,1).nextQuarterBeatFrame==39000);
    assert(context(timeline,7500).nextQuarterBeatFrame==39000);
    assert(context(timeline,39000).nextQuarterBeatFrame==39000);
    assert(context(timeline,-1).nextQuarterBeatFrame==0);
    Timeline loop(map,{100,.5,1,1,.25,1.25,4});auto first=context(loop,100);assert(first.loopStartBeat==.25&&first.loopEndBeat==1.25&&first.nextQuarterBeatFrame==24100);
    auto near=context(loop,36099);assert(near.nextQuarterBeatFrame==69100);
    auto wrapped=context(loop,36100);assert(wrapped.loopIteration==1&&wrapped.nextQuarterBeatFrame==69100);
    Timeline empty(map,{0,.1,1,1,.1,.2,5});assert(context(empty,0).nextQuarterBeatFrame==std::numeric_limits<int64_t>::max());
    Timeline stopped(map,{0,.2,0,0,0,0,6});assert(context(stopped,0).nextQuarterBeatFrame==std::numeric_limits<int64_t>::max());
    Map fractional({{0,123}});Timeline precise(fractional);auto one=frame(fractional,1);assert(context(precise,one).nextQuarterBeatFrame==one);
}
static void testExactFrameInverseAfterTempoChange(){
    Map map({{0,120},{.5,60},{1.5,180}},{{0,4,4},{.75,3,4}});Timeline timeline(map);
    const int64_t expected[]={0,6000,12000,24000,36000,48000,60000,64000,68000};
    for(int n=0;n<9;++n){
        const double position=n*.25;const int64_t sample=expected[n];
        assert(frame(map,position)==sample&&beat(map,sample)==position);
        if(n){
            // No epsilon snapping: the immediately later representable beat
            // must still wait until the next sample, unlike the exact boundary.
            assert(frame(map,std::nextafter(position,std::numeric_limits<double>::infinity()))==sample+1);
            assert(frame(map,std::nextafter(position,0.0))==sample);
            assert(beat(map,sample-1)<position);
        }
    }
    for(int64_t sample=63980;sample<=64020;++sample)assert(frame(map,beat(map,sample))==sample);
    auto at=context(timeline,64000);assert(at.beat==1.75&&at.bpm==180&&at.nextQuarterBeatFrame==68000);
}
static void testProjectedQuarterGrid(){
    Map map({{0,120},{.375,60}});Timeline stopped(map,{100,.25,0,0,0,0,1});
    // Beat1 is at39000 samples, so the held .25 cursor is33000 samples from
    // that grid line. A latest-BPM guess would incorrectly return18000.
    for(int64_t graph:{-1000LL,100LL,8192LL}){
        auto c=context(stopped,graph,64);assert(c.beat==.25&&c.songFrame==6000);
        assert(c.nextQuarterBeatFrame==std::numeric_limits<int64_t>::max());
        assert(c.nextQuarterGridFrame==graph+33000&&(c.flags&ST_CLOCK_GRID_PROJECTION));
        assert(!(c.flags&(ST_CLOCK_PLAYING|ST_CLOCK_RECORDING)));
    }
    Timeline integer(map,{0,0,0,0,0,0,2});assert(context(integer,712).nextQuarterGridFrame==712);
    Timeline tiny(map,{0,.1,1,1,.1,.2,3});
    for(int64_t graph:{0LL,17LL,2399LL,2400LL,4800LL}){
        auto c=context(tiny,graph);assert(c.nextQuarterBeatFrame==std::numeric_limits<int64_t>::max());
        assert(c.nextQuarterGridFrame==graph+39000-c.songFrame);
        assert(c.nextQuarterGridFrame>graph&&(c.flags&ST_CLOCK_GRID_PROJECTION));
    }
    // Reachable next-cycle beats keep their existing transport meaning, even
    // when the separate unwrapped-grid distance is different.
    Timeline reachable(map,{0,1.125,1,1,.25,1.25,4});auto c=context(reachable,0);
    assert(c.nextQuarterBeatFrame==39000&&c.nextQuarterGridFrame==42000);
    assert(!(c.flags&ST_CLOCK_GRID_PROJECTION));
}
int main(){testConversionsAndMeters();testCeilAndAtomicBoundaries();testPreroll();testPrerollBarBoundaries();testSeekLoopAndLatency();testStoppedAndImmutableLifetime();testValidationAndUntouchedOutputs();testDomainExtremes();testSpanPartitioning();testRoundTripsAndRenderQueries();testQuarterBeatAndLoopContext();testExactFrameInverseAfterTempoChange();testProjectedQuarterGrid();assert(queryAllocations.load()==0);std::puts("Clock: 13 groups passed; conversion, exact post-tempo frame inverse, projected stopped/short-loop grid, ceil boundaries, preroll, loop/seek/latency, immutable lifetime, validation, extremes, span partitioning, concurrent zero-allocation queries.");}

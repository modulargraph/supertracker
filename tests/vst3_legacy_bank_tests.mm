// Original in-memory components only: no plug-in bundle, device, editor or render.
#include "../Sources/VST3Host/VST3Host.mm"
#include "vst3_state_fixture.h"
#include "public.sdk/source/vst/utility/vst2persistence.h"
#include <climits>
#include <cstdio>
#include <functional>
#include <stdexcept>

static int checks=0;
static const uint32_t fixtureID=0x44624238;
static const std::vector<uint8_t> originalBank{0,255,17,128,2,3,0,19,200,8,7,6,5,4};
static void require(bool condition,const char *message) {
    ++checks;if(!condition){fprintf(stderr,"FAIL VST3 legacy bank: %s\n",message);exit(1);}
}
static std::vector<uint8_t> readAll(IBStream *stream) {
    int64 position=-1;require(stream->tell(&position)==kResultOk&&position==0,"component and controller streams begin at byte zero");
    std::vector<uint8_t> result;uint8_t buffer[256];
    for(;;){int32 count=0;require(stream->read(buffer,sizeof(buffer),&count)==kResultOk,"fixture stream is readable");if(!count)break;result.insert(result.end(),buffer,buffer+count);}
    return result;
}
static std::vector<uint8_t> nativeState(int32_t program,const std::vector<uint8_t>& payload) {
    std::vector<uint8_t> result{'O','W','N','3'};
    for(int shift=0;shift<32;shift+=8)result.push_back(uint8_t(uint32_t(program)>>shift));
    result.insert(result.end(),payload.begin(),payload.end());return result;
}
class LegacyComponent final : public StateComponent {
public:
    enum Mode { acceptWrapped,rejectWrappedAcceptRaw,rejectBoth } mode=acceptWrapped;
    uint32_t expectedID=fixtureID;int32_t wrappedProgram=-1;
    std::vector<std::vector<uint8_t>> received;
    std::vector<uint8_t> native=nativeState(0,{});
    bool changeLayout=false,changedLayout=false,oneOutput=false,failBusActivation=false;
    bool failSuspend=false,failReactivate=false,throwState=false;
    int suspends=0,activations=0,captures=0;
    tresult PLUGIN_API getState(IBStream *stream) override { ++captures;return stream->write(native.data(),int32(native.size()),nullptr); }
    tresult PLUGIN_API setState(IBStream *stream) override {
        received.push_back(readAll(stream));const auto& bytes=received.back();
        if(throwState)throw std::runtime_error("original fixture state exception");
        if(changeLayout)changedLayout=true;
        if(bytes.size()>=4&&!memcmp(bytes.data(),"OWN3",4)){native=bytes;return kResultOk;}
        if(bytes==originalBank){if(mode==rejectBoth)return kResultFalse;native=nativeState(0,bytes);return kResultOk;}
        require(bytes.size()>4&&!memcmp(bytes.data(),"VstW",4),"wrapped migration includes SDK bypass metadata");
        MemoryStream view(bytes.data(),bytes.size());
        auto legacy=VST3::tryVst2StateLoad(view);
        require(bool(legacy),"SDK reader accepts the generated VST2 bank wrapper");
        require(uint32_t(legacy->fxUniqueID)==expectedID,"wrapper retains all four unique-ID bytes");
        require(legacy->programs.empty()&&!legacy->isBypassed,"wrapper contains a bank chunk and explicit unbypassed state");
        require(std::vector<uint8_t>(legacy->chunk.begin(),legacy->chunk.end())==originalBank,"wrapper preserves every opaque vendor byte");
        wrappedProgram=legacy->currentProgram;
        if(mode!=acceptWrapped)return kResultFalse;
        native=nativeState(wrappedProgram,originalBank);return kResultOk;
    }
    int32 PLUGIN_API getBusCount(MediaType media,BusDirection direction) override {
        return media==kAudio&&direction==kOutput&&(oneOutput||changedLayout)?1:0;
    }
    tresult PLUGIN_API getBusInfo(MediaType media,BusDirection direction,int32 index,BusInfo& info) override {
        if(index||!getBusCount(media,direction))return kInvalidArgument;
        info={};info.mediaType=media;info.direction=direction;info.channelCount=2;info.busType=kMain;info.name[0]='O';return kResultOk;
    }
    tresult PLUGIN_API activateBus(MediaType,BusDirection,int32,TBool) override { return failBusActivation?kResultFalse:kResultOk; }
    tresult PLUGIN_API setActive(TBool enabled) override {
        if(enabled){++activations;return failReactivate?kResultFalse:kResultOk;}
        ++suspends;return failSuspend?kResultFalse:kResultOk;
    }
};
class LegacyController final : public StateController {
public:
    LegacyComponent *component=nullptr;tresult synchronization=kResultOk;
    std::vector<std::vector<uint8_t>> received;
    tresult PLUGIN_API setComponentState(IBStream *stream) override {
        received.push_back(readAll(stream));
        require(component&&!component->received.empty()&&received.back()==component->received.back(),"controller receives the identical accepted component stream");
        return synchronization;
    }
};
class LegacyProcessor final : public IAudioProcessor {
public:
    bool failStop=false,failStart=false;int renders=0,starts=0,stops=0;
    QUERY_ONE(IAudioProcessor) EMBEDDED_REFS
    tresult PLUGIN_API setBusArrangements(SpeakerArrangement*,int32,SpeakerArrangement*,int32) override{return kResultOk;}
    tresult PLUGIN_API getBusArrangement(BusDirection,int32,SpeakerArrangement& value) override{value=SpeakerArr::kStereo;return kResultOk;}
    tresult PLUGIN_API canProcessSampleSize(int32) override{return kResultOk;}
    uint32 PLUGIN_API getLatencySamples() override{return 0;}
    tresult PLUGIN_API setupProcessing(ProcessSetup&) override{return kResultOk;}
    tresult PLUGIN_API setProcessing(TBool enabled) override{if(enabled){++starts;return failStart?kResultFalse:kResultOk;}++stops;return failStop?kResultFalse:kResultOk;}
    uint32 PLUGIN_API getTailSamples() override{return 0;}
    tresult PLUGIN_API process(ProcessData&) override{++renders;return kResultFalse;}
};
struct LegacyFixture {
    LegacyComponent component;LegacyController controller;LegacyProcessor processor;
    std::unique_ptr<STVST3Instance> host=std::make_unique<STVST3Instance>();
    explicit LegacyFixture(uint32_t id=fixtureID){
        component.expectedID=id;controller.component=&component;
        host->component=&component;host->controller=&controller;host->processor=&processor;
        snprintf(host->uid,sizeof(host->uid),"565354%08X6F726967696E616C31",id);
        host->inputs.configured=host->outputs.configured=true;
    }
    bool restore(int32_t program,int& fallback,char *error,size_t capacity){
        return st_vst3_restore_vst2_bank(host.get(),component.expectedID,program,originalBank.data(),originalBank.size(),&fallback,error,capacity)!=0;
    }
    void refusedCapture(){
        require(st_vst3_required_state_flush_frames(host.get())==0,"failed-capture fixture must exercise an ordinary unprofiled class");
        char error[512]{};uint64_t token=99;const int before=component.captures;
        require(!st_vst3_begin_state_capture(host.get(),0,&token,error,sizeof(error))&&token==0&&error[0],
                "failed generic restore must refuse capture preparation and clear its token");
        void *bytes=reinterpret_cast<void*>(1);size_t size=99;error[0]=0;
        require(!st_vst3_copy_state(host.get(),&bytes,&size,error,sizeof(error))&&!bytes&&!size&&error[0],
                "failed generic restore must refuse serialization and publish no state");
        require(component.captures==before&&!processor.renders,"failed capture must not call component getState or render");
    }
    std::vector<uint8_t> capture(){
        void *bytes=nullptr;size_t size=0;char error[512]{};
        require(st_vst3_copy_state(host.get(),&bytes,&size,error,sizeof(error))&&bytes&&size>=56,"successful migration permits native VST3 capture");
        std::vector<uint8_t> result(static_cast<uint8_t*>(bytes),static_cast<uint8_t*>(bytes)+size);st_vst3_free(bytes);return result;
    }
};
static void nativeRoundTrip(LegacyFixture& fixture){
    const auto saved=fixture.capture();uint64_t componentSize=0,controllerSize=0;
    memcpy(&componentSize,saved.data()+40,8);memcpy(&controllerSize,saved.data()+48,8);
    require(!memcmp(saved.data(),"STV3\1\0\0\0",8)&&!memcmp(saved.data()+8,fixture.host->uid,32),"migration saves a normal native VST3 document with exact class identity");
    require(componentSize==fixture.component.native.size()&&controllerSize==sizeof(double),"native component and controller sections retain their own bounds");
    require(std::vector<uint8_t>(saved.begin()+56,saved.begin()+56+componentSize)==fixture.component.native,"captured component state is native, not the migration wrapper or raw input");
    LegacyFixture restored(fixture.component.expectedID);char error[512]{};
    require(st_vst3_restore_state(restored.host.get(),saved.data(),saved.size(),error,sizeof(error)),"captured native document restores through the ordinary VST3 state API");
    require(restored.component.native==fixture.component.native&&restored.capture()==saved,"native Save/Open round trip remains byte-exact");
    require(!fixture.processor.renders&&!restored.processor.renders,"migration and capture never render audio");
}
static void validMigrations(){
    for(uint32_t id:{fixtureID,uint32_t(0xF1624238),UINT32_MAX})for(int32_t program:{0,3,INT_MAX}){
        LegacyFixture fixture(id);int fallback=99;char error[512]="stale error";
        require(fixture.restore(program,fallback,error,sizeof(error)),"supported wrapped bank migrates");
        require(fallback==0&&!error[0]&&fixture.component.received.size()==1,"wrapped success clears diagnostics and never tries raw input");
        require(fixture.component.wrappedProgram==program,"SDK wrapper retains the selected program without truncation");
        require(fixture.controller.received.size()==1&&!fixture.host->failedStateRestore,"controller synchronized and restore completed");
        nativeRoundTrip(fixture);
    }
    LegacyFixture raw;raw.component.mode=LegacyComponent::rejectWrappedAcceptRaw;int fallback=99;char error[512]{};
    require(raw.restore(3,fallback,error,sizeof(error)),"explicit wrapped-component rejection permits a raw retry");
    require(fallback==1&&!error[0]&&raw.component.received.size()==2&&raw.component.received[1]==originalBank,"raw success is reported after exactly one original-payload retry");
    require(raw.controller.received.size()==1&&raw.controller.received[0]==originalBank,"controller synchronizes only the accepted raw stream");
    nativeRoundTrip(raw);
    LegacyFixture rejected;rejected.component.mode=LegacyComponent::rejectBoth;fallback=99;
    require(!rejected.restore(3,fallback,error,sizeof(error))&&fallback==0&&rejected.component.received.size()==2,"both encodings rejected fails without a success fallback claim");
    require(rejected.controller.received.empty()&&rejected.host->failedStateRestore&&error[0],"rejected component streams are never sent to the controller");
    LegacyFixture optional;optional.controller.synchronization=kNotImplemented;fallback=99;
    require(optional.restore(3,fallback,error,sizeof(error))&&fallback==0,"optional controller synchronization remains supported");
}
static void invalidInputs(){
    auto reject=[](const std::function<void(LegacyFixture&)>& configure,uint32_t id,int32_t program,const void *bytes,size_t size){
        LegacyFixture fixture;configure(fixture);int fallback=99;char error[512]{};
        require(!st_vst3_restore_vst2_bank(fixture.host.get(),id,program,bytes,size,&fallback,error,sizeof(error)),"invalid migration input fails before component state calls");
        require(fallback==0&&error[0]&&fixture.component.received.empty()&&fixture.controller.received.empty(),"invalid migration never touches component/controller or claims fallback");
        require(!fixture.component.suspends&&!fixture.processor.starts&&!fixture.processor.stops,"input validation precedes lifecycle calls");
    };
    const auto unchanged=[](LegacyFixture&){};
    reject(unchanged,0,3,originalBank.data(),originalBank.size());
    reject(unchanged,fixtureID+1,3,originalBank.data(),originalBank.size());
    reject(unchanged,fixtureID,-1,originalBank.data(),originalBank.size());
    reject(unchanged,fixtureID,3,nullptr,originalBank.size());
    reject(unchanged,fixtureID,3,originalBank.data(),0);
    reject(unchanged,fixtureID,3,originalBank.data(),32u*1024u*1024u+1);
    reject([](LegacyFixture& f){copyText(f.host->uid,"0123456789ABCDEF0123456789ABCDEF");},fixtureID,3,originalBank.data(),originalBank.size());
    reject([](LegacyFixture& f){copyText(f.host->uid,"56535444624239AAAAAAAAAAAAAAAAAA");},fixtureID,3,originalBank.data(),originalBank.size());
    reject([](LegacyFixture& f){f.host->captureVendorCallActive=true;},fixtureID,3,originalBank.data(),originalBank.size());
    int fallback=99;char error[512]{};
    require(!st_vst3_restore_vst2_bank(nullptr,fixtureID,3,originalBank.data(),originalBank.size(),&fallback,error,sizeof(error))&&fallback==0,"null instance fails and resets fallback output");
}
static void failuresDoNotFallback(){
    auto failure=[](const std::function<void(LegacyFixture&)>& configure,size_t calls){
        LegacyFixture fixture;configure(fixture);int fallback=99;char error[512]{};
        require(!fixture.restore(3,fallback,error,sizeof(error)),"controller/layout/lifecycle failure must fail migration");
        require(fallback==0&&fixture.component.received.size()==calls,"non-encoding failure must never retry raw component state");
        require(fixture.host->failedStateRestore&&error[0]&&!fixture.processor.renders,"failed restoration remains marked incomplete without rendering");
    };
    for(tresult result:{kResultFalse,kInvalidArgument,kInternalError})failure([result](LegacyFixture& f){f.controller.synchronization=result;},1);
    for(bool rejectsWrapper:{false,true}){
        failure([rejectsWrapper](LegacyFixture& f){f.component.changeLayout=true;if(rejectsWrapper)f.component.mode=LegacyComponent::rejectWrappedAcceptRaw;},1);
        failure([rejectsWrapper](LegacyFixture& f){f.component.oneOutput=true;BusInfo info{};f.component.getBusInfo(kAudio,kOutput,0,info);f.host->outputs.metadata={info};f.host->outputs.activation={1};f.component.failBusActivation=true;if(rejectsWrapper)f.component.mode=LegacyComponent::rejectWrappedAcceptRaw;},1);
        failure([rejectsWrapper](LegacyFixture& f){f.host->active=true;f.component.failReactivate=true;if(rejectsWrapper)f.component.mode=LegacyComponent::rejectWrappedAcceptRaw;},1);
        failure([rejectsWrapper](LegacyFixture& f){f.host->active=f.host->processing=true;f.processor.failStart=true;if(rejectsWrapper)f.component.mode=LegacyComponent::rejectWrappedAcceptRaw;},1);
    }
    failure([](LegacyFixture& f){f.host->active=true;f.component.failSuspend=true;},0);
    failure([](LegacyFixture& f){f.host->active=f.host->processing=true;f.processor.failStop=true;},0);
    failure([](LegacyFixture& f){f.component.throwState=true;},1);
}
static void genericFailureCaptureGuard(){
    for(int mode=0;mode<4;++mode){
        LegacyFixture fixture;int fallback=99;char error[512]{};
        if(mode==0)fixture.component.mode=LegacyComponent::rejectBoth;
        else fixture.controller.synchronization=mode==1?kResultFalse:mode==2?kInternalError:kInvalidArgument;
        require(!fixture.restore(3,fallback,error,sizeof(error))&&fixture.host->failedStateRestore,
                "component or controller rejection marks the generic instance incomplete");
        fixture.refusedCapture();
        fixture.component.mode=LegacyComponent::acceptWrapped;fixture.controller.synchronization=kResultOk;
        require(fixture.restore(3,fallback,error,sizeof(error))&&!fixture.host->failedStateRestore,
                "an explicit valid restoration clears the generic failed-state guard");
        uint64_t token=99;
        require(st_vst3_begin_state_capture(fixture.host.get(),0,&token,error,sizeof(error))&&token==0,
                "repaired generic instance permits capture preparation without an audio flush");
        nativeRoundTrip(fixture);
    }
}
int main(){@autoreleasepool{
    const auto unchanged=originalBank;validMigrations();invalidInputs();failuresDoNotFallback();genericFailureCaptureGuard();
    require(originalBank==unchanged,"all migrations leave original input bytes unchanged");
    printf("PASS VST3 legacy bank: %d checks; SDK wrapper identity/program/payload, explicit component-only fallback, native capture, input bounds, controller/layout/lifecycle failures, generic failed-capture guard and recovery; original in-memory fixtures, no bundles or audio.\n",checks);
}}

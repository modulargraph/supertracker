// Original in-memory protocol fixture: never loads a plug-in or opens MIDI/audio.
#include "../Sources/VST3Host/VST3Host.mm"
#include "vst3_state_fixture.h"
#include <cassert>
class BaselineProcessor final:public IAudioProcessor,public StudioTools::IHardwarePatchBaseline {
public:
 StateComponent& base;double live=.625;bool ready=true;uint32 version=1,captures=0,manualEdits=0;
 explicit BaselineProcessor(StateComponent& value):base(value){}
 tresult PLUGIN_API queryInterface(const TUID id,void** out)override {
  if(FUnknownPrivate::iidEqual(id,StudioTools::hardwarePatchBaselineIID))*out=static_cast<StudioTools::IHardwarePatchBaseline*>(this);
  else if(iidIs(id,IAudioProcessor::iid)||iidIs(id,FUnknown::iid))*out=static_cast<IAudioProcessor*>(this);
  else {*out=nullptr;return kNoInterface;}addRef();return kResultOk;
 }
 EMBEDDED_REFS
 uint32 PLUGIN_API protocolVersion()override{return version;}
 uint32 PLUGIN_API baselineCapabilities()override{return StudioTools::stateIsBaseline|StudioTools::manualParameterEdits|(ready?StudioTools::captureReady:0);}
 tresult PLUGIN_API captureBaseline()override{if(!ready)return kResultFalse;base.value=live;++captures;return kResultOk;}
 tresult PLUGIN_API setBaselineParameter(uint32 id,double value)override{if(id!=73)return kInvalidArgument;base.value=value;++manualEdits;return kResultOk;}
 tresult PLUGIN_API setBusArrangements(SpeakerArrangement*,int32,SpeakerArrangement*,int32)override{return kResultOk;}
 tresult PLUGIN_API getBusArrangement(BusDirection,int32,SpeakerArrangement& a)override{a=SpeakerArr::kStereo;return kResultOk;}
 tresult PLUGIN_API canProcessSampleSize(int32 s)override{return s==kSample32?kResultOk:kResultFalse;}
 uint32 PLUGIN_API getLatencySamples()override{return 0;}
 tresult PLUGIN_API setupProcessing(ProcessSetup&)override{return kResultOk;}
 tresult PLUGIN_API setProcessing(TBool)override{return kResultOk;}
 uint32 PLUGIN_API getTailSamples()override{return 0;}
 tresult PLUGIN_API process(ProcessData& d)override{
  for(int32 i=0;i<d.inputParameterChanges->getParameterCount();++i){auto q=d.inputParameterChanges->getParameterData(i);for(int32 j=0;j<q->getPointCount();++j){int32 at;double value;if(q->getPoint(j,at,value)==kResultOk&&q->getParameterId()==73)live=value;}}
  for(int c=0;c<2;++c)std::fill(d.outputs[0].channelBuffers32[c],d.outputs[0].channelBuffers32[c]+d.numSamples,float(live));return kResultOk;
 }
};
int main(){@autoreleasepool{
 StateComponent component;StateController controller;controller.mode=StateController::optionalEmpty;
 BaselineProcessor processor(component);auto h=std::make_unique<STVST3Instance>();h->component=&component;h->controller=&controller;h->processor=&processor;
 h->active=true;h->processing=true;h->maxFrames=64;h->parameterIDs={73};h->parameterStates=std::make_unique<STVST3Instance::ParameterState[]>(1);
 float left[64]{},right[64]{};float* channels[]={left,right};h->outputs.buses.resize(1);h->outputs.buses[0].numChannels=2;h->outputs.buses[0].channelBuffers32=channels;
 copyText(h->uid,"5354424153454C494E4546495854555245");
 auto save=[&]{void* bytes=nullptr;size_t size=0;assert(st_vst3_copy_state(h.get(),&bytes,&size,nullptr,0));std::vector<uint8_t> result((uint8_t*)bytes,(uint8_t*)bytes+size);st_vst3_free(bytes);return result;};
 auto render=[&](double value){STVST3ParameterChange p{0,73,value};assert(st_vst3_process(h.get(),nullptr,nullptr,left,right,64,nullptr,0,&p,1,nullptr));assert(std::abs(left[63]-value)<1e-6);};
 assert(st_vst3_baseline_capabilities(h.get())==7);const auto original=save();
 for(double value:{.1,.9,.2,.8}){render(value);assert(save()==original);} // play/seek/loop values never alter base
 assert(processor.captures==0&&processor.manualEdits==0);
 h->processing=false;assert(save()==original);h->processing=true; // stop is not promotion
 assert(st_vst3_set_parameter(h.get(),73,.4));assert(processor.manualEdits==1&&component.value==.4);
 // Stopped graph: the pending edit only drains inside process(). Capture must
 // deliver it with a zero-sample flush (no audio advanced), not refuse forever.
 h->processing=false;left[0]=-1;
 char captureError[256]{};assert(st_vst3_capture_baseline(h.get(),captureError,sizeof(captureError)));
 assert(st_vst3_pending_parameter_edits(h.get())==0&&!h->processing&&left[0]==-1);
 assert(component.value==.4&&processor.live==.4&&processor.captures==1);h->processing=true;
 render(.7);assert(component.value==.4);auto manual=save();assert(manual!=original);
 assert(st_vst3_capture_baseline(h.get(),nullptr,0));assert(component.value==.7&&processor.live==.7&&processor.captures==2);
 auto captured=save();render(.05);assert(save()==captured);
 processor.ready=false;assert(!st_vst3_capture_baseline(h.get(),nullptr,0));assert(save()==captured);
 processor.version=2;assert(st_vst3_baseline_capabilities(h.get())==0&&!st_vst3_capture_baseline(h.get(),nullptr,0));processor.version=1;
 assert(st_vst3_restore_state(h.get(),manual.data(),manual.size(),nullptr,0));assert(component.value==.4);
 h->processor=nullptr;assert(st_vst3_baseline_capabilities(h.get())==0&&!st_vst3_capture_baseline(h.get(),nullptr,0));h->processor=&processor;
 puts("PASS baseline protocol: transient audio differs, serialized baseline stable, manual field edit/capture explicit, stop not promotion, stopped pending edit flushed before capture, refusal/version isolation, opaque restore");
}}
